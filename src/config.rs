use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub telegram_token: String,
    pub discord_token: String,
    pub deepseek_api_key: String,
    pub database_url: String,
    pub allowed_user_ids: Vec<i64>,
    pub discord_allowed_user_ids: Vec<u64>,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        dotenvy::dotenv().ok();

        let allowed_raw = env::var("ALLOWED_USER_IDS").unwrap_or_default();
        let allowed_user_ids: Vec<i64> = allowed_raw
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse::<i64>().ok())
            .collect();

        if allowed_user_ids.is_empty() && !allowed_raw.is_empty() {
            tracing::warn!("ALLOWED_USER_IDS was set but no valid IDs could be parsed: {allowed_raw}");
        }

        if allowed_user_ids.is_empty() {
            tracing::info!("No ALLOWED_USER_IDS set — all Telegram users can interact with the bot");
        } else {
            tracing::info!("Telegram allowlist active: {} user(s) can use the bot", allowed_user_ids.len());
        }

        let discord_allowed_raw = env::var("DISCORD_ALLOWED_USER_IDS").unwrap_or_default();
        let discord_allowed_user_ids: Vec<u64> = discord_allowed_raw
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse::<u64>().ok())
            .collect();

        if discord_allowed_user_ids.is_empty() && !discord_allowed_raw.is_empty() {
            tracing::warn!("DISCORD_ALLOWED_USER_IDS was set but no valid IDs could be parsed: {discord_allowed_raw}");
        }

        if discord_allowed_user_ids.is_empty() {
            tracing::info!("No DISCORD_ALLOWED_USER_IDS set — all Discord users can interact with the bot");
        } else {
            tracing::info!("Discord allowlist active: {} user(s) can use the bot", discord_allowed_user_ids.len());
        }

        // Discord token is optional (bot can run Telegram-only)
        let discord_token = env::var("DISCORD_BOT_TOKEN").unwrap_or_default();

        Ok(Self {
            telegram_token: env::var("TELEGRAM_BOT_TOKEN")
                .map_err(|_| ConfigError::Missing("TELEGRAM_BOT_TOKEN"))?,
            discord_token,
            deepseek_api_key: env::var("DEEPSEEK_API_KEY")
                .map_err(|_| ConfigError::Missing("DEEPSEEK_API_KEY"))?,
            database_url: env::var("DATABASE_URL")
                .map_err(|_| ConfigError::Missing("DATABASE_URL"))?,
            allowed_user_ids,
            discord_allowed_user_ids,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Missing required environment variable: {0}")]
    Missing(&'static str),
}
