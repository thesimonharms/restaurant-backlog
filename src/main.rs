mod ai;
mod bot;
mod config;
mod db;
mod error;
mod extractor;

use std::sync::Arc;

use ai::deepseek::DeepSeekClient;
use bot::handlers::Command;
use teloxide::prelude::*;
use teloxide::types::CallbackQuery;
use teloxide::types::InlineKeyboardButton;
use teloxide::types::InlineKeyboardMarkup;
use teloxide::types::ParseMode;
use teloxide::utils::command::BotCommands;
use teloxide::utils::html;

pub struct AppState {
    pub db: db::DbPool,
    pub ai: DeepSeekClient,
    pub allowed_user_ids: Vec<i64>,
}

impl AppState {
    /// Check if a user is allowed to interact with the bot.
    /// When the allowlist is empty, everyone is allowed.
    pub fn is_user_allowed(&self, user_id: Option<i64>) -> bool {
        match user_id {
            Some(uid) => {
                self.allowed_user_ids.is_empty() || self.allowed_user_ids.contains(&uid)
            }
            None => false, // anonymous users never allowed
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "restaurant_backlog=info".into()),
        )
        .init();

    let config = config::Config::from_env()?;
    tracing::info!("Configuration loaded ✓");

    let pool = db::init_pool(&config.database_url).await?;
    tracing::info!("Database connected ✓");

    let ai_client = DeepSeekClient::new(config.deepseek_api_key);
    tracing::info!("AI client initialized ✓");

    let state: Arc<AppState> = Arc::new(AppState {
        db: pool,
        ai: ai_client,
        allowed_user_ids: config.allowed_user_ids,
    });

    let bot = Bot::new(&config.telegram_token);
    let me = bot.get_me().await?;
    tracing::info!("Connected to Telegram as {}", me.mention());

    bot.set_my_commands(Command::bot_commands()).await?;
    tracing::info!("Registered bot commands");

    // ── Command handlers ──────────────────────────────────────────────
    let cmd_handler = Update::filter_message()
        .filter_command::<Command>()
        .endpoint(handle_command);

    // ── Shared link handler (non-command text messages) ───────────────
    let link_handler = Update::filter_message()
        .branch(
            dptree::filter(|msg: Message| !msg.text().unwrap_or("").starts_with('/'))
                .endpoint(handle_shared_link),
        );

    // ── Callback query handler ────────────────────────────────────────
    let callback_handler = Update::filter_callback_query()
        .endpoint(handle_callback);

    let mut dispatcher = Dispatcher::builder(
        bot,
        dptree::entry()
            .branch(cmd_handler)
            .branch(link_handler)
            .branch(callback_handler),
    )
    .dependencies(dptree::deps![state])
    .enable_ctrlc_handler()
    .build();

    tracing::info!("Restaurant Backlog Bot is polling for updates. Press Ctrl+C to stop.");
    dispatcher.dispatch().await;
    Ok(())
}

// ── Command routing ───────────────────────────────────────────────────

async fn handle_command(
    cmd: Command,
    msg: Message,
    bot: Bot,
    state: Arc<AppState>,
) -> Result<(), teloxide::RequestError> {
    let user_id = msg.from().map(|u| u.id.0 as i64);
    tracing::info!(
        user_id = ?user_id,
        chat_id = msg.chat.id.0,
        command = ?cmd,
        "Received command"
    );

    // Auth gate: silently ignore unauthorized users
    if !state.is_user_allowed(user_id) {
        tracing::warn!(user_id = ?user_id, "Ignoring command from unauthorized user");
        return Ok(());
    }

    match cmd {
        Command::Start => bot::handlers::cmd_start(bot, msg).await,
        Command::Help => bot::handlers::cmd_help(bot, msg).await,
        Command::List => bot::handlers::cmd_list(bot, msg, state).await,
        Command::Find(_) => bot::handlers::cmd_find(bot, msg, state).await,
        Command::Tags => bot::handlers::cmd_tags(bot, msg, state).await,
        Command::Random => bot::handlers::cmd_random(bot, msg, state).await,
        Command::Visited => bot::handlers::cmd_visited(bot, msg, state).await,
    }
}

// ── Shared link handler ───────────────────────────────────────────────

async fn handle_shared_link(
    msg: Message,
    bot: Bot,
    state: Arc<AppState>,
) -> Result<(), teloxide::RequestError> {
    let text = match msg.text() {
        Some(t) => t,
        None => return Ok(()),
    };

    let user_id = match msg.from().map(|u| u.id.0 as i64) {
        Some(id) => id,
        None => return Ok(()),
    };

    tracing::info!(
        user_id,
        chat_id = msg.chat.id.0,
        "Received non-command text message"
    );

    // Auth gate: silently ignore unauthorized users
    if !state.is_user_allowed(Some(user_id)) {
        tracing::warn!(user_id, "Ignoring shared link from unauthorized user");
        return Ok(());
    }

    let url = match extractor::extract_url(text) {
        Some(u) => u,
        None => return Ok(()),
    };

    let processing = bot
        .send_message(msg.chat.id, "🔍 Looking up this link...")
        .reply_to_message_id(msg.id)
        .await?;

    let metadata = match extractor::fetch_page_content(&url).await {
        Ok(m) => m,
        Err(e) => {
            bot.edit_message_text(
                msg.chat.id,
                processing.id,
                format!("❌ Couldn't fetch that link: {e}"),
            )
            .await?;
            return Ok(());
        }
    };

    bot.edit_message_text(
        msg.chat.id,
        processing.id,
        "🧠 AI is extracting restaurant info...",
    )
    .await?;

    let extracted = match state
        .ai
        .extract_restaurant_info(&metadata.title, &metadata.content, url.as_str())
        .await
    {
        Ok(info) => info,
        Err(e) => {
            bot.edit_message_text(
                msg.chat.id,
                processing.id,
                format!("⚠️ AI had trouble: {e}\n\nWhat's the restaurant name? Just send it as a reply."),
            )
            .await?;
            return Ok(());
        }
    };

    let restaurant_name = extracted
        .restaurant_name
        .clone()
        .unwrap_or_else(|| metadata.title.clone());

    let google_maps_url = if let Some(ref query) = extracted.google_maps_query {
        let encoded: String =
            url::form_urlencoded::Serializer::new(String::new())
                .append_key_only(query)
                .finish();
        Some(format!("https://www.google.com/maps/search/{encoded}"))
    } else {
        None
    };

    let new_restaurant = db::models::NewRestaurant {
        user_id,
        name: restaurant_name,
        source_url: Some(url.to_string()),
        google_maps_url,
        description: extracted.description,
        cuisine_tags: extracted.tags,
    };

    match db::save_restaurant(&state.db, &new_restaurant).await {
        Ok(saved) => {
            let tags = if saved.cuisine_tags.is_empty() {
                "No tags".to_string()
            } else {
                saved
                    .cuisine_tags
                    .iter()
                    .map(|t| format!("#{}", html::escape(t)))
                    .collect::<Vec<_>>()
                    .join("  ")
            };

            let maps_link = saved
                .google_maps_url
                .as_deref()
                .map(|u| format!("\n📍 {}", html::link(u, "Open in Google Maps")))
                .unwrap_or_default();

            let desc = saved
                .description
                .as_deref()
                .map(|d| format!("\n📝 {}", html::escape(d)))
                .unwrap_or_default();

            let name = html::escape(&saved.name);
            let src = saved
                .source_url
                .as_deref()
                .map(|u| html::link(u, "Original Post"))
                .unwrap_or_else(|| "Original Post".to_string());
            let card = format!(
                "✅ <b>Saved!</b>\n\n🍽️ <b>{name}</b>{desc}\n\n🏷️ {tags}{maps}\n\n🔗 {src}",
                name = name,
                desc = desc,
                tags = tags,
                maps = maps_link,
                src = src
            );

            let keyboard = InlineKeyboardMarkup::new(vec![vec![
                InlineKeyboardButton::callback(
                    "✅ Visited", format!("visited:{}", saved.id),
                ),
                InlineKeyboardButton::callback(
                    "🗑️ Delete", format!("delete:{}", saved.id),
                ),
            ]]);

            bot.edit_message_text(msg.chat.id, processing.id, card)
                .parse_mode(ParseMode::Html)
                .reply_markup(keyboard)
                .await?;
        }
        Err(e) => {
            bot.edit_message_text(
                msg.chat.id,
                processing.id,
                format!("❌ Couldn't save: {e}"),
            )
            .await?;
        }
    }

    Ok(())
}

// ── Callback query handler ────────────────────────────────────────────

async fn handle_callback(
    q: CallbackQuery,
    bot: Bot,
    state: Arc<AppState>,
) -> Result<(), teloxide::RequestError> {
    let user_id = q.from.id.0 as i64;
    tracing::info!(user_id, "Received callback query");

    // Auth gate: silently ignore unauthorized users
    if !state.is_user_allowed(Some(user_id)) {
        tracing::warn!(user_id, "Ignoring callback from unauthorized user");
        return Ok(());
    }

    let data = match &q.data {
        Some(d) => d.clone(),
        None => return Ok(()),
    };

    let parts: Vec<&str> = data.split(':').collect();
    match parts.as_slice() {
        ["visited", id_str] => {
            if let Ok(id) = uuid::Uuid::parse_str(id_str) {
                if let Err(e) = db::mark_visited(&state.db, id).await {
                    tracing::error!("Failed to mark visited: {e}");
                }
                bot.answer_callback_query(q.id)
                    .text("✅ Marked as visited!")
                    .await?;
                if let Some(msg) = &q.message {
                    let chat_id = msg.chat.id;
                    let msg_id = msg.id;
                    bot.edit_message_reply_markup(chat_id, msg_id).await?;
                }
            }
        }
        ["delete", id_str] => {
            if let Ok(id) = uuid::Uuid::parse_str(id_str) {
                if let Err(e) = db::delete_restaurant(&state.db, id, user_id).await {
                    tracing::error!("Failed to delete: {e}");
                }
                bot.answer_callback_query(q.id)
                    .text("🗑️ Deleted!")
                    .await?;
                if let Some(msg) = &q.message {
                    let chat_id = msg.chat.id;
                    bot.edit_message_text(chat_id, msg.id, "🗑️ Deleted!")
                        .await?;
                }
            }
        }
        _ => {}
    }

    Ok(())
}
