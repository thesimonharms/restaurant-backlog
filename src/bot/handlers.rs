use std::sync::Arc;

use teloxide::prelude::*;
use teloxide::utils::html;
use teloxide::utils::command::BotCommands;
use crate::db;
use crate::AppState;

/// Bot commands
#[derive(BotCommands, Clone, Debug)]
#[command(rename_rule = "lowercase")]
pub enum Command {
    #[command(description = "Show help")]
    Start,
    #[command(description = "Show all available commands")]
    Help,
    #[command(description = "List your restaurant backlog")]
    List,
    #[command(description = "Find a restaurant: /find craving Korean BBQ near me")]
    Find(String),
    #[command(description = "Show all your cuisine tags")]
    Tags,
    #[command(description = "Pick a random restaurant")]
    Random,
    #[command(description = "Mark a restaurant as visited")]
    Visited,
    #[command(description = "Undo: remove the most recent saved restaurant")]
    Undo,
    #[command(description = "Add a restaurant manually: /add Restaurant Name")]
    Add(String),
}

/// /start command
pub async fn cmd_start(bot: Bot, msg: Message) -> Result<(), teloxide::RequestError> {
    bot.send_message(msg.chat.id,
        "👋 <b>Restaurant Backlog Bot</b>\n\n\
        Save restaurants you discover on TikTok, Instagram, or anywhere!\n\n\
        <b>How to use:</b>\n\
        • Forward or paste a link → I'll extract the restaurant info automatically\n\
        • Use /list to browse your backlog\n\
        • Use /find &lt;what you're craving&gt; for AI recommendations\n\
        • Use /tags to see all your cuisine tags\n\
        • Use /random when you can't decide\n\n\
        <b>Pro tip:</b> Share this bot with friends to build the backlog together! 🍜"
    )
    .parse_mode(teloxide::types::ParseMode::Html)
    .await?;
    Ok(())
}

/// /help command
pub async fn cmd_help(bot: Bot, msg: Message) -> Result<(), teloxide::RequestError> {
    bot.send_message(msg.chat.id,
        format!(
            "<b>Commands:</b>\n\n{}",
            Command::descriptions()
        )
    )
    .parse_mode(teloxide::types::ParseMode::Html)
    .await?;
    Ok(())
}

/// /list command — show backlog
pub async fn cmd_list(bot: Bot, msg: Message, state: Arc<AppState>) -> Result<(), teloxide::RequestError> {
    let user_id = msg.from().map(|u| u.id.0 as i64);
    let Some(user_id) = user_id else {
        return Ok(());
    };

    let restaurants = db::get_user_restaurants(&state.db, user_id, 10, 0).await;

    match restaurants {
        Ok(restaurants) if restaurants.is_empty() => {
            bot.send_message(msg.chat.id, "📭 Your backlog is empty! Share a link to get started.")
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
                        let tags = r
                            .cuisine_tags
                            .iter()
                            .map(|tag| html::escape(tag))
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!(" [{tags}]")
                    };
                    format!(
                        "• <b>{name}</b>{visited}{tags}",
                        name = html::escape(&r.name)
                    )
                })
                .collect();

            let total = db::get_all_restaurants(&state.db, user_id)
                .await
                .map(|v| v.len())
                .unwrap_or(0);

            bot.send_message(
                msg.chat.id,
                format!("📋 <b>Your Backlog</b> ({total} total)\n\n{}", lines.join("\n")),
            )
            .parse_mode(teloxide::types::ParseMode::Html)
            .await?;
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("❌ Error: {e}")).await?;
        }
    }

    Ok(())
}

/// /find command — AI-powered recommendations
pub async fn cmd_find(
    bot: Bot,
    msg: Message,
    state: Arc<AppState>,
) -> Result<(), teloxide::RequestError> {
    let user_id = msg.from().map(|u| u.id.0 as i64);
    let Some(user_id) = user_id else {
        return Ok(());
    };

    let query = msg.text().unwrap_or("").trim();
    let query = query
        .strip_prefix("/find")
        .or_else(|| query.strip_prefix("/find@"))
        .unwrap_or("")
        .trim();

    if query.is_empty() {
        bot.send_message(
            msg.chat.id,
            "What are you craving? Try: <code>/find I want Korean BBQ</code>\nor <code>/find something spicy</code>",
        )
        .parse_mode(teloxide::types::ParseMode::Html)
        .await?;
        return Ok(());
    }

    let processing = bot
        .send_message(msg.chat.id, "🤔 Thinking about what to recommend...")
        .await?;

    let restaurants = db::get_all_restaurants(&state.db, user_id)
        .await
        .unwrap_or_default();

    match state.ai.recommend(query, &restaurants).await {
        Ok(recommendation) => {
            bot.edit_message_text(msg.chat.id, processing.id, recommendation)
                .await?;
        }
        Err(e) => {
            bot.edit_message_text(
                msg.chat.id,
                processing.id,
                format!("❌ Couldn't get recommendation: {e}"),
            )
            .await?;
        }
    }

    Ok(())
}

/// /tags command
pub async fn cmd_tags(
    bot: Bot,
    msg: Message,
    state: Arc<AppState>,
) -> Result<(), teloxide::RequestError> {
    let user_id = msg.from().map(|u| u.id.0 as i64);
    let Some(user_id) = user_id else {
        return Ok(());
    };

    match db::get_user_tags(&state.db, user_id).await {
        Ok(tags) if tags.is_empty() => {
            bot.send_message(msg.chat.id, "No tags yet! Save some restaurants first.")
                .await?;
        }
        Ok(tags) => {
            let tag_display = tags
                .iter()
                .map(|t| format!("#{}", html::escape(t)))
                .collect::<Vec<_>>()
                .join("  ");
            bot.send_message(
                msg.chat.id,
                format!("🏷️ <b>Your Cuisine Tags</b>\n\n{tag_display}"),
            )
            .parse_mode(teloxide::types::ParseMode::Html)
            .await?;
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("❌ Error: {e}")).await?;
        }
    }

    Ok(())
}

/// /random command
pub async fn cmd_random(
    bot: Bot,
    msg: Message,
    state: Arc<AppState>,
) -> Result<(), teloxide::RequestError> {
    let user_id = msg.from().map(|u| u.id.0 as i64);
    let Some(user_id) = user_id else {
        return Ok(());
    };

    match db::get_random_restaurant(&state.db, user_id).await {
        Ok(Some(restaurant)) => {
            let tags = if restaurant.cuisine_tags.is_empty() {
                String::new()
            } else {
                let tags = restaurant
                    .cuisine_tags
                    .iter()
                    .map(|tag| html::escape(tag))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("\n🏷️ {tags}")
            };
            let maps = restaurant
                .google_maps_url
                .map(|u| format!("\n📍 {}", html::link(&u, "Google Maps")))
                .unwrap_or_default();
            let desc = restaurant
                .description
                .map(|d| format!("\n{}", html::escape(&d)))
                .unwrap_or_default();
            let name = html::escape(&restaurant.name);

            bot.send_message(
                msg.chat.id,
                format!(
                    "🎲 <b>Random Pick!</b>\n\n🍽️ <b>{name}</b>{desc}{tags}{maps}",
                    name = name
                ),
            )
            .parse_mode(teloxide::types::ParseMode::Html)
            .await?;
        }
        Ok(None) => {
            bot.send_message(msg.chat.id, "📭 Your backlog is empty! Share a link to get started.")
                .await?;
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("❌ Error: {e}")).await?;
        }
    }

    Ok(())
}

/// /visited command
pub async fn cmd_visited(
    bot: Bot,
    msg: Message,
    _state: Arc<AppState>,
) -> Result<(), teloxide::RequestError> {
    bot.send_message(
        msg.chat.id,
        "Reply to a restaurant card with /visited to mark it as visited, or use the button on the card!",
    )
    .await?;
    Ok(())
}

/// /undo command — delete the most recently saved restaurant
pub async fn cmd_undo(
    bot: Bot,
    msg: Message,
    state: Arc<AppState>,
) -> Result<(), teloxide::RequestError> {
    let user_id = msg.from().map(|u| u.id.0 as i64);
    let Some(user_id) = user_id else {
        return Ok(());
    };

    match db::delete_last_restaurants(&state.db, user_id, 1).await {
        Ok(count) if count > 0 => {
            bot.send_message(msg.chat.id, format!("🗑️ Removed {count} restaurant(s) from your backlog."))
                .await?;
        }
        Ok(_) => {
            bot.send_message(msg.chat.id, "📭 Your backlog is empty — nothing to undo.")
                .await?;
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("❌ Couldn't undo: {e}")).await?;
        }
    }

    Ok(())
}

/// /add command — rename the last added restaurant
pub async fn cmd_add(
    bot: Bot,
    msg: Message,
    state: Arc<AppState>,
) -> Result<(), teloxide::RequestError> {
    let user_id = match msg.from().map(|u| u.id.0 as i64) {
        Some(id) => id,
        None => return Ok(()),
    };

    let text = msg.text().unwrap_or("");
    let name = text
        .strip_prefix("/add")
        .or_else(|| text.strip_prefix("/add@"))
        .unwrap_or("")
        .trim()
        .to_string();

    if name.is_empty() {
        bot.send_message(
            msg.chat.id,
            "Usage: <code>/add Correct Restaurant Name</code>\n\nRenames the last restaurant you added.",
        )
        .parse_mode(teloxide::types::ParseMode::Html)
        .await?;
        return Ok(());
    }

    // Find the last restaurant for this user
    let last = match db::get_last_restaurant(&state.db, user_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            bot.send_message(
                msg.chat.id,
                "📭 Your backlog is empty. Send a link first or try <code>/add Restaurant Name</code> to create one.",
            )
            .parse_mode(teloxide::types::ParseMode::Html)
            .await?;
            return Ok(());
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("❌ Couldn't find last restaurant: {e}"))
                .await?;
            return Ok(());
        }
    };

    let processing = bot
        .send_message(
            msg.chat.id,
            format!("✏️ Renaming <b>{}</b> ...", teloxide::utils::html::escape(&last.name)),
        )
        .parse_mode(teloxide::types::ParseMode::Html)
        .reply_to_message_id(msg.id)
        .await?;

    // Try to enrich with AI for cuisine, tags, description, maps
    let context = if let Some(ref url) = last.source_url {
        format!("From source: {url}")
    } else {
        "Manually renamed by user.".to_string()
    };

    let extracted = match state.ai.enrich_restaurant_name(&name, &context).await {
        Ok(info) => info,
        Err(_) => {
            // AI failed — just use the name with a Google Maps search
            db::models::ExtractedInfo {
                restaurant_name: Some(name.clone()),
                cuisine_type: None,
                tags: vec![],
                google_maps_query: Some(name.clone()),
                description: None,
            }
        }
    };

    let final_name = extracted.restaurant_name.clone().unwrap_or_else(|| name.clone());
    let google_maps_url = extracted.google_maps_query.as_ref().map(|q| {
        let encoded: String = url::form_urlencoded::Serializer::new(String::new())
            .append_key_only(q)
            .finish();
        format!("https://www.google.com/maps/search/{encoded}")
    });

    match db::update_restaurant(
        &state.db,
        last.id,
        &final_name,
        extracted.description.as_deref(),
        &extracted.tags,
        google_maps_url.as_deref(),
    )
    .await
    {
        Ok(updated) => {
            let tags_display = if updated.cuisine_tags.is_empty() {
                "No tags".to_string()
            } else {
                updated.cuisine_tags.iter()
                    .map(|t| format!("#{}", teloxide::utils::html::escape(t)))
                    .collect::<Vec<_>>()
                    .join("  ")
            };
            let maps_link = updated.google_maps_url.as_deref()
                .map(|u| format!("\n📍 {}", teloxide::utils::html::link(u, "Open in Google Maps")))
                .unwrap_or_default();
            let desc = updated.description.as_deref()
                .map(|d| format!("\n📝 {}", teloxide::utils::html::escape(d)))
                .unwrap_or_default();
            let name_escaped = teloxide::utils::html::escape(&updated.name);

            let card = format!(
                "✏️ <b>Renamed!</b>\n\n🍽️ <b>{name_escaped}</b>{desc}\n\n🏷️ {tags_display}{maps_link}",
            );

            let keyboard = teloxide::types::InlineKeyboardMarkup::new(vec![vec![
                teloxide::types::InlineKeyboardButton::callback("✅ Visited", format!("visited:{}", updated.id)),
                teloxide::types::InlineKeyboardButton::callback("🗑️ Delete", format!("delete:{}", updated.id)),
            ]]);

            bot.edit_message_text(msg.chat.id, processing.id, card)
                .parse_mode(teloxide::types::ParseMode::Html)
                .reply_markup(keyboard)
                .await?;
        }
        Err(e) => {
            bot.edit_message_text(
                msg.chat.id,
                processing.id,
                format!("❌ Couldn't rename: {e}"),
            )
            .await?;
        }
    }

    Ok(())
}
