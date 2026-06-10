mod ai;
mod bot;
mod config;
mod db;
mod discord;
mod error;
mod extractor;

use std::sync::Arc;
use std::sync::Mutex;
use std::collections::HashMap;

use ai::deepseek::DeepSeekClient;
use bot::handlers::Command;
use teloxide::prelude::*;
use teloxide::types::CallbackQuery;
use teloxide::types::InlineKeyboardButton;
use teloxide::types::InlineKeyboardMarkup;
use teloxide::types::ParseMode;
use teloxide::utils::command::BotCommands;
use teloxide::utils::html;

/// Track a user who was just asked for a restaurant name after AI failure
#[derive(Debug, Clone)]
pub struct PendingAddition {
    pub user_id: i64,
    pub chat_id: i64,
    pub source_url: Option<String>,
    pub title: String,
}

pub struct AppState {
    pub db: db::DbPool,
    pub ai: DeepSeekClient,
    pub allowed_user_ids: Vec<i64>,
    pub discord_allowed_user_ids: Vec<u64>,
    pub pending_additions: Mutex<HashMap<i64, PendingAddition>>,
}

impl AppState {
    /// Check if a Telegram user is allowed to interact with the bot.
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

    let ai_client = DeepSeekClient::new(config.deepseek_api_key.clone());
    tracing::info!("AI client initialized ✓");

    let state: Arc<AppState> = Arc::new(AppState {
        db: pool,
        ai: ai_client,
        allowed_user_ids: config.allowed_user_ids.clone(),
        discord_allowed_user_ids: config.discord_allowed_user_ids.clone(),
        pending_additions: Mutex::new(HashMap::new()),
    });

    // ── Telegram bot ─────────────────────────────────────────────────
    let telegram_state = Arc::clone(&state);
    let telegram_config = config.clone();
    let tg = tokio::spawn(async move {
        run_telegram(telegram_state, &telegram_config).await;
    });

    // ── Discord bot ──────────────────────────────────────────────────
    let discord_state = Arc::clone(&state);
    let dc = tokio::spawn(async move {
        discord::run_discord(discord_state, &config).await;
    });

    // Wait for BOTH to finish — tokio::select! would cancel the other
    // when one completes, which is wrong (Discord returns immediately
    // when no token is configured, killing Telegram mid-connection).
    let (tg_result, dc_result) = tokio::join!(tg, dc);

    if let Err(e) = tg_result {
        tracing::error!("Telegram task panicked: {e}");
    }
    if let Err(e) = dc_result {
        tracing::error!("Discord task panicked: {e}");
    }

    Ok(())
}

/// Run the Telegram bot with its own dispatcher
async fn run_telegram(state: Arc<AppState>, config: &config::Config) {
    let bot = Bot::new(&config.telegram_token);
    let me = match bot.get_me().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("Failed to connect to Telegram: {e}");
            return;
        }
    };
    tracing::info!("Connected to Telegram as {}", me.mention());

    if let Err(e) = bot.set_my_commands(Command::bot_commands()).await {
        tracing::warn!("Failed to register Telegram bot commands: {e}");
    } else {
        tracing::info!("Registered Telegram bot commands");
    }

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

    tracing::info!("Restaurant Backlog Bot (Telegram) is polling for updates. Press Ctrl+C to stop.");
    dispatcher.dispatch().await;
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
        Command::Undo => bot::handlers::cmd_undo(bot, msg, state).await,
        Command::Add(_) => bot::handlers::cmd_add(bot, msg, state).await,
    }
}

// ── Shared link handler ───────────────────────────────────────────────

async fn handle_shared_link(
    msg: Message,
    bot: Bot,
    state: Arc<AppState>,
) -> Result<(), teloxide::RequestError> {
    let text = match msg.text() {
        Some(t) => t.to_string(),
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

    let url = match extractor::extract_url(&text) {
        Some(u) => u,
        None => {
            // No URL in message — check if user has a pending name input
            let pending = state.pending_additions.lock().unwrap().remove(&user_id);
            if let Some(p) = pending {
                return handle_name_reply(msg, bot, state, text.to_string(), p).await;
            }
            return Ok(());
        }
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

    let extracted_list = match state
        .ai
        .extract_restaurants(&metadata.title, &metadata.content, url.as_str())
        .await
    {
        Ok(list) => list,
        Err(e) => {
            // Store pending so the user can reply with a name
            state.pending_additions.lock().unwrap().insert(user_id, PendingAddition {
                user_id,
                chat_id: msg.chat.id.0,
                source_url: Some(url.to_string()),
                title: metadata.title.clone(),
            });
            bot.edit_message_text(
                msg.chat.id,
                processing.id,
                format!("⚠️ AI had trouble: {e}\n\nWhat's the restaurant name? Just send it as a reply."),
            )
            .await?;
            return Ok(());
        }
    };

    if extracted_list.is_empty() {
        // Store pending so the user can reply with a name
        state.pending_additions.lock().unwrap().insert(user_id, PendingAddition {
            user_id,
            chat_id: msg.chat.id.0,
            source_url: Some(url.to_string()),
            title: metadata.title.clone(),
        });
        bot.edit_message_text(
            msg.chat.id,
            processing.id,
            "🤷 Couldn't find any restaurants in that content.",
        )
        .await?;
        return Ok(());
    }

    // Save all extracted restaurants
    let mut saved_restaurants: Vec<db::models::Restaurant> = Vec::new();
    let mut save_errors: Vec<String> = Vec::new();

    for (i, extracted) in extracted_list.iter().enumerate() {
        // Better fallback: if AI couldn't extract a name, use a numbered placeholder
        let restaurant_name = extracted
            .restaurant_name
            .clone()
            .unwrap_or_else(|| {
                if extracted_list.len() > 1 {
                    format!("🍽️ #{} from {}", i + 1, metadata.title)
                } else {
                    metadata.title.clone()
                }
            });

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
            owner_id: db::derive_owner_id(user_id),
            user_id,
            name: restaurant_name,
            source_url: Some(url.to_string()),
            google_maps_url,
            description: extracted.description.clone(),
            cuisine_tags: extracted.tags.clone(),
        };

        match db::save_restaurant(&state.db, &new_restaurant).await {
            Ok(saved) => saved_restaurants.push(saved),
            Err(e) => save_errors.push(format!(
                "{}: {e}",
                extracted.restaurant_name.as_deref().unwrap_or("Unknown")
            )),
        }
    }

    let count = saved_restaurants.len();

    // If nothing saved, show error
    if count == 0 {
        let error_detail = if save_errors.is_empty() {
            String::new()
        } else {
            format!("\n\nErrors:\n{}", save_errors.join("\n"))
        };
        bot.edit_message_text(
            msg.chat.id,
            processing.id,
            format!("❌ Couldn't save any restaurants.{}", error_detail),
        )
        .await?;
        return Ok(());
    }

    // Helper: build a restaurant card + inline keyboard
    let build_card = |saved: &db::models::Restaurant| -> (String, InlineKeyboardMarkup) {
        let tags_display = if saved.cuisine_tags.is_empty() {
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

        let src = saved
            .source_url
            .as_deref()
            .map(|u| html::link(u, "Original Post"))
            .unwrap_or_else(|| "Original Post".to_string());

        let name = html::escape(&saved.name);
        let card = format!(
            "🍽️ <b>{name}</b>{desc}\n\n🏷️ {tags_display}{maps_link}\n\n🔗 {src}",
        );

        let keyboard = InlineKeyboardMarkup::new(vec![vec![
            InlineKeyboardButton::callback(
                "✅ Visited",
                format!("visited:{}", saved.id),
            ),
            InlineKeyboardButton::callback(
                "🗑️ Delete",
                format!("delete:{}", saved.id),
            ),
        ]]);

        (card, keyboard)
    };

    // ── Single restaurant ──────────────────────────────────────────
    if count == 1 {
        let saved = &saved_restaurants[0];
        let (card, kb) = build_card(saved);
        let full = format!("✅ <b>Saved!</b>\n\n{card}");

        bot.edit_message_text(msg.chat.id, processing.id, full)
            .parse_mode(ParseMode::Html)
            .reply_markup(kb)
            .await?;
        return Ok(());
    }

    // ── Multiple restaurants ───────────────────────────────────────
    // Turn the processing message into a header
    let header = format!("✅ <b>Saved {count} restaurants!</b>\n\nFrom: {}", html::link(url.as_str(), &metadata.title));
    bot.edit_message_text(msg.chat.id, processing.id, header)
        .parse_mode(ParseMode::Html)
        .await?;

    // Send each restaurant as its own reply with full buttons
    for saved in &saved_restaurants {
        let (card, kb) = build_card(saved);
        bot.send_message(msg.chat.id, &card)
            .parse_mode(ParseMode::Html)
            .reply_markup(kb)
            .reply_to_message_id(msg.id)
            .await?;
    }

    // Report any save failures
    if !save_errors.is_empty() {
        bot.send_message(
            msg.chat.id,
            format!("⚠️ Some saves failed:\n{}", save_errors.join("\n")),
        )
        .await?;
    }

    Ok(())
}

/// Handle a user's reply when they were asked for a restaurant name
/// after the AI failed to extract info from a link.
async fn handle_name_reply(
    msg: Message,
    bot: Bot,
    state: Arc<AppState>,
    name: String,
    pending: PendingAddition,
) -> Result<(), teloxide::RequestError> {
    let user_id = pending.user_id;
    let name = name.trim().to_string();

    if name.is_empty() {
        // Empty reply — set pending back so they can try again
        state.pending_additions.lock().unwrap().insert(user_id, pending);
        bot.send_message(msg.chat.id, "What's the restaurant name? Just type it and send.")
            .reply_to_message_id(msg.id)
            .await?;
        return Ok(());
    }

    let processing = bot
        .send_message(msg.chat.id, "🧠 Looking up info on that restaurant...")
        .reply_to_message_id(msg.id)
        .await?;

    // Try to enrich with AI, using the original post title as context
    let context = format!("From a social media post titled: {}", pending.title);
    let extracted = match state.ai.enrich_restaurant_name(&name, &context).await {
        Ok(info) => info,
        Err(_) => {
            // AI enrichment failed — save with just the name
            db::models::ExtractedInfo {
                restaurant_name: Some(name.clone()),
                cuisine_type: None,
                tags: vec![],
                google_maps_query: Some(name.clone()),
                description: None,
            }
        }
    };

    let restaurant_name = extracted.restaurant_name.clone().unwrap_or_else(|| name.clone());

    let google_maps_url = extracted.google_maps_query.as_ref().map(|q| {
        let encoded: String = url::form_urlencoded::Serializer::new(String::new())
            .append_key_only(q)
            .finish();
        format!("https://www.google.com/maps/search/{encoded}")
    });

    let new_restaurant = db::models::NewRestaurant {
        owner_id: db::derive_owner_id(user_id),
        user_id,
        name: restaurant_name,
        source_url: pending.source_url,
        google_maps_url,
        description: extracted.description,
        cuisine_tags: extracted.tags,
    };

    match db::save_restaurant(&state.db, &new_restaurant).await {
        Ok(saved) => {
            let tags_display = if saved.cuisine_tags.is_empty() {
                "No tags".to_string()
            } else {
                saved.cuisine_tags.iter()
                    .map(|t| format!("#{}", html::escape(t)))
                    .collect::<Vec<_>>()
                    .join("  ")
            };
            let maps_link = saved.google_maps_url.as_deref()
                .map(|u| format!("\n📍 {}", html::link(u, "Open in Google Maps")))
                .unwrap_or_default();
            let desc = saved.description.as_deref()
                .map(|d| format!("\n📝 {}", html::escape(d)))
                .unwrap_or_default();
            let name_escaped = html::escape(&saved.name);

            let card = format!(
                "✅ <b>Saved!</b>\n\n🍽️ <b>{name_escaped}</b>{desc}\n\n🏷️ {tags_display}{maps_link}",
            );

            let keyboard = InlineKeyboardMarkup::new(vec![vec![
                InlineKeyboardButton::callback("✅ Visited", format!("visited:{}", saved.id)),
                InlineKeyboardButton::callback("🗑️ Delete", format!("delete:{}", saved.id)),
            ]]);

            bot.edit_message_text(msg.chat.id, processing.id, card)
                .parse_mode(ParseMode::Html)
                .reply_markup(keyboard)
                .await?;
        }
        Err(e) => {
            bot.edit_message_text(msg.chat.id, processing.id, format!("❌ Couldn't save: {e}"))
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
