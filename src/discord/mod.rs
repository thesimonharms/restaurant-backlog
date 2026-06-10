use std::sync::Arc;

use serenity::async_trait;
use serenity::builder::CreateMessage;
use serenity::builder::EditMessage;
use serenity::model::channel::Message;
use serenity::prelude::*;
use url::Url;

use crate::config;
use crate::db;
use crate::extractor;
use crate::AppState;

/// Key for accessing shared AppState from serenity's TypeMap
struct StateKey;

impl TypeMapKey for StateKey {
    type Value = Arc<AppState>;
}

/// Discord bot event handler
pub struct DiscordBot;

#[async_trait]
impl EventHandler for DiscordBot {
    /// Handle incoming messages
    async fn message(&self, ctx: Context, msg: Message) {
        // Ignore messages from the bot itself
        if msg.author.bot {
            return;
        }

        // Only respond in DMs — ignore all server/guild channels
        if msg.guild_id.is_some() {
            return;
        }

        let state = {
            let data = ctx.data.read().await;
            match data.get::<StateKey>() {
                Some(s) => s.clone(),
                None => {
                    tracing::error!("AppState not found in Discord context");
                    return;
                }
            }
        };

        let user_id = msg.author.id.get();

        // Auth gate: silently ignore unauthorized users
        if !is_discord_user_allowed(&state, user_id) {
            tracing::warn!(
                discord_user_id = user_id,
                "Ignoring message from unauthorized Discord user"
            );
            return;
        }

        // Clone content before borrowing — we need to reference it
        // but msg may be moved into handlers
        let content = msg.content.clone().trim().to_string();

        // ── Handle !commands ───────────────────────────────────────
        if content.starts_with('!') {
            handle_discord_command(&ctx, &msg, &state, &content).await;
            return;
        }

        // ── Handle links ───────────────────────────────────────────
        if let Some(url) = extractor::extract_url(&content) {
            handle_discord_link(&ctx, &msg, &state, url, user_id).await;
        }
    }
}

/// Check Discord user against the separate allowlist
fn is_discord_user_allowed(state: &AppState, user_id: u64) -> bool {
    state.discord_allowed_user_ids.is_empty() || state.discord_allowed_user_ids.contains(&user_id)
}

/// Dispatch Discord !commands. Errors are logged internally.
async fn handle_discord_command(ctx: &Context, msg: &Message, state: &Arc<AppState>, content: &str) {
    let parts: Vec<&str> = content.splitn(2, ' ').collect();
    let command = parts[0].to_lowercase();

    let result = match command.as_str() {
        "!help" | "!start" => cmd_help(ctx, msg).await,
        "!list" => cmd_list(ctx, msg, state).await,
        "!tags" => cmd_tags(ctx, msg, state).await,
        "!random" => cmd_random(ctx, msg, state).await,
        "!find" => cmd_find(ctx, msg, state, parts.get(1).unwrap_or(&"")).await,
        _ => return,
    };

    if let Err(e) = result {
        tracing::error!("Discord command {command} failed: {e}");
    }
}

/// !help / !start
async fn cmd_help(ctx: &Context, msg: &Message) -> Result<(), serenity::Error> {
    msg.channel_id
        .send_message(&ctx.http, CreateMessage::new().content(
            "👋 **Restaurant Backlog Bot**\n\n\
             Save restaurants you discover on TikTok, YouTube, or anywhere!\n\n\
             **How to use:**\n\
             • Paste a link → I'll extract the restaurant info automatically\n\
             • `!list` — Browse your backlog\n\
             • `!tags` — See all your cuisine tags\n\
             • `!find <query>` — AI-powered recommendations\n\
             • `!random` — Pick a random restaurant\n\
             • `!help` — Show this message",
        ))
        .await?;
    Ok(())
}

/// !list
async fn cmd_list(ctx: &Context, msg: &Message, state: &Arc<AppState>) -> Result<(), serenity::Error> {
    let user_id = msg.author.id.get() as i64;

    let restaurants = db::get_user_restaurants(&state.db, user_id, 10, 0).await;

    match restaurants {
        Ok(restaurants) if restaurants.is_empty() => {
            msg.channel_id
                .send_message(&ctx.http, CreateMessage::new().content("📭 Your backlog is empty! Share a link to get started."))
                .await?;
        }
        Ok(restaurants) => {
            let lines: Vec<String> = restaurants
                .iter()
                .map(|r| {
                    let visited = if r.visited { " ✅" } else { "" };
                    let tags = if r.cuisine_tags.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", r.cuisine_tags.join(", "))
                    };
                    format!("• **{name}**{visited}{tags}", name = r.name)
                })
                .collect();

            let total = db::get_all_restaurants(&state.db, user_id)
                .await
                .map(|v| v.len())
                .unwrap_or(0);

            msg.channel_id
                .send_message(
                    &ctx.http,
                    CreateMessage::new().content(format!(
                        "📋 **Your Backlog** ({total} total)\n\n{}",
                        lines.join("\n")
                    )),
                )
                .await?;
        }
        Err(e) => {
            msg.channel_id
                .send_message(
                    &ctx.http,
                    CreateMessage::new().content(format!("❌ Error: {e}")),
                )
                .await?;
        }
    }

    Ok(())
}

/// !tags
async fn cmd_tags(ctx: &Context, msg: &Message, state: &Arc<AppState>) -> Result<(), serenity::Error> {
    let user_id = msg.author.id.get() as i64;

    match db::get_user_tags(&state.db, user_id).await {
        Ok(tags) if tags.is_empty() => {
            msg.channel_id
                .send_message(&ctx.http, CreateMessage::new().content("No tags yet! Save some restaurants first."))
                .await?;
        }
        Ok(tags) => {
            let tag_display = tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join("  ");
            msg.channel_id
                .send_message(
                    &ctx.http,
                    CreateMessage::new().content(format!("🏷️ **Your Cuisine Tags**\n\n{tag_display}")),
                )
                .await?;
        }
        Err(e) => {
            msg.channel_id
                .send_message(
                    &ctx.http,
                    CreateMessage::new().content(format!("❌ Error: {e}")),
                )
                .await?;
        }
    }

    Ok(())
}

/// !random
async fn cmd_random(ctx: &Context, msg: &Message, state: &Arc<AppState>) -> Result<(), serenity::Error> {
    let user_id = msg.author.id.get() as i64;

    match db::get_random_restaurant(&state.db, user_id).await {
        Ok(Some(restaurant)) => {
            let tags = if restaurant.cuisine_tags.is_empty() {
                String::new()
            } else {
                format!("\n🏷️ {}", restaurant.cuisine_tags.join(", "))
            };
            let maps = restaurant
                .google_maps_url
                .map(|u| format!("\n📍 {u}"))
                .unwrap_or_default();
            let desc = restaurant
                .description
                .map(|d| format!("\n{d}"))
                .unwrap_or_default();

            msg.channel_id
                .send_message(
                    &ctx.http,
                    CreateMessage::new().content(format!(
                        "🎲 **Random Pick!**\n\n🍽️ **{name}**{desc}{tags}{maps}",
                        name = restaurant.name
                    )),
                )
                .await?;
        }
        Ok(None) => {
            msg.channel_id
                .send_message(&ctx.http, CreateMessage::new().content("📭 Your backlog is empty! Share a link to get started."))
                .await?;
        }
        Err(e) => {
            msg.channel_id
                .send_message(
                    &ctx.http,
                    CreateMessage::new().content(format!("❌ Error: {e}")),
                )
                .await?;
        }
    }

    Ok(())
}

/// !find — AI-powered recommendations
async fn cmd_find(
    ctx: &Context,
    msg: &Message,
    state: &Arc<AppState>,
    query: &str,
) -> Result<(), serenity::Error> {
    let user_id = msg.author.id.get() as i64;
    let query = query.trim();

    if query.is_empty() {
        msg.channel_id
            .send_message(
                &ctx.http,
                CreateMessage::new().content(
                    "What are you craving? Try: `!find I want Korean BBQ`\nor `!find something spicy`",
                ),
            )
            .await?;
        return Ok(());
    }

    let mut processing = msg
        .channel_id
        .send_message(&ctx.http, CreateMessage::new().content("🤔 Thinking about what to recommend..."))
        .await?;

    let restaurants = db::get_all_restaurants(&state.db, user_id)
        .await
        .unwrap_or_default();

    match state.ai.recommend(query, &restaurants).await {
        Ok(recommendation) => {
            processing
                .edit(&ctx.http, EditMessage::new().content(recommendation))
                .await?;
        }
        Err(e) => {
            processing
                .edit(
                    &ctx.http,
                    EditMessage::new().content(format!("❌ Couldn't get recommendation: {e}")),
                )
                .await?;
        }
    }

    Ok(())
}

/// Process a shared link in Discord. Errors are logged internally.
async fn handle_discord_link(
    ctx: &Context,
    msg: &Message,
    state: &Arc<AppState>,
    url: Url,
    user_id: u64,
) {
    let channel_id = msg.channel_id;

    // Send a "processing" message
    let mut processing = match channel_id
        .send_message(&ctx.http, CreateMessage::new().content("🔍 Looking up this link..."))
        .await
    {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("Failed to send initial message: {e}");
            return;
        }
    };

    let metadata = match extractor::fetch_page_content(&url).await {
        Ok(m) => m,
        Err(e) => {
            let _ = processing
                .edit(&ctx.http, EditMessage::new().content(format!("❌ Couldn't fetch that link: {e}")))
                .await;
            return;
        }
    };

    if let Err(e) = processing
        .edit(&ctx.http, EditMessage::new().content("🧠 AI is extracting restaurant info..."))
        .await
    {
        tracing::error!("Failed to edit message: {e}");
        return;
    }

    let extracted_list = match state
        .ai
        .extract_restaurants(&metadata.title, &metadata.content, url.as_str())
        .await
    {
        Ok(list) => list,
        Err(e) => {
            let _ = processing
                .edit(
                    &ctx.http,
                    EditMessage::new().content(format!(
                        "⚠️ AI had trouble: {e}\n\nWhat's the restaurant name? Just send it as a reply."
                    )),
                )
                .await;
            return;
        }
    };

    if extracted_list.is_empty() {
        let _ = processing
            .edit(&ctx.http, EditMessage::new().content("🤷 Couldn't find any restaurants in that content."))
            .await;
        return;
    }

    // Save all extracted restaurants
    let mut saved_names: Vec<String> = Vec::new();
    let mut save_errors: Vec<String> = Vec::new();

    for extracted in &extracted_list {
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
            owner_id: db::derive_owner_id(user_id as i64),
            user_id: user_id as i64,
            name: restaurant_name,
            source_url: Some(url.to_string()),
            google_maps_url,
            description: extracted.description.clone(),
            cuisine_tags: extracted.tags.clone(),
        };

        match db::save_restaurant(&state.db, &new_restaurant).await {
            Ok(saved) => saved_names.push(saved.name),
            Err(e) => save_errors.push(format!("{}: {e}", extracted.restaurant_name.as_deref().unwrap_or("Unknown"))),
        }
    }

    // Build summary card
    let count = saved_names.len();
    if count == 0 {
        let error_detail = if save_errors.is_empty() {
            String::new()
        } else {
            format!("\n\nErrors:\n{}", save_errors.join("\n"))
        };
        let _ = processing
            .edit(&ctx.http, EditMessage::new().content(format!("❌ Couldn't save any restaurants.{}", error_detail)))
            .await;
    } else if count == 1 {
        // Single restaurant: detailed card
        let extracted = &extracted_list[0];
        let tags_display = if extracted.tags.is_empty() {
            "No tags".to_string()
        } else {
            extracted
                .tags
                .iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join("  ")
        };
        let maps_link = extracted
            .google_maps_query
            .as_ref()
            .map(|q| {
                let encoded: String =
                    url::form_urlencoded::Serializer::new(String::new())
                        .append_key_only(q)
                        .finish();
                format!("\n📍 **Google Maps:** https://www.google.com/maps/search/{encoded}")
            })
            .unwrap_or_default();
        let desc = extracted
            .description
            .as_deref()
            .map(|d| format!("\n📝 {d}"))
            .unwrap_or_default();
        let src = format!("\n🔗 **Source:** {}", url.as_str());

        let card = format!(
            "✅ **Saved!**\n\n🍽️ **{name}**{desc}\n\n🏷️ {tags_display}{maps}{src}",
            name = saved_names[0],
            desc = desc,
            tags_display = tags_display,
            maps = maps_link,
            src = src
        );

        let _ = processing
            .edit(&ctx.http, EditMessage::new().content(card))
            .await;
    } else {
        // Multiple restaurants: summary list
        let names_list: Vec<String> = saved_names
            .iter()
            .map(|n| format!("🍽️ **{n}**"))
            .collect();
        let src = format!("\n🔗 **Source:** {}", url.as_str());

        let mut card = format!(
            "✅ **Saved {count} restaurants!**\n\n{}\n{src}",
            names_list.join("\n"),
        );
        if !save_errors.is_empty() {
            card.push_str(&format!("\n\n⚠️ Some saves failed:\n{}", save_errors.join("\n")));
        }

        let _ = processing
            .edit(&ctx.http, EditMessage::new().content(card))
            .await;
    }
}

/// Start the Discord bot in a new tokio task.
/// If no token is configured, this is a no-op. Errors are logged.
pub async fn run_discord(state: Arc<AppState>, config: &config::Config) {
    if config.discord_token.is_empty() {
        tracing::info!("No DISCORD_BOT_TOKEN set — Discord support disabled");
        return;
    }

    let intents = GatewayIntents::non_privileged() | GatewayIntents::MESSAGE_CONTENT;

    let mut client = match Client::builder(&config.discord_token, intents)
        .event_handler(DiscordBot)
        .await
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to create Discord client: {e}");
            return;
        }
    };

    // Inject shared AppState into serenity's data store
    {
        let mut data = client.data.write().await;
        data.insert::<StateKey>(state);
    }

    tracing::info!("Discord bot starting...");

    if let Err(why) = client.start().await {
        tracing::error!("Discord client error: {why}");
    }
}
