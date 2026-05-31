use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("AI service error: {0}")]
    Ai(String),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("Telegram error: {0}")]
    Telegram(#[from] teloxide::RequestError),

    #[error("Discord error: {0}")]
    Discord(String),

    #[error("Config error: {0}")]
    Config(#[from] crate::config::ConfigError),

    #[error("Regex error: {0}")]
    Regex(#[from] regex::Error),
}

impl From<serenity::Error> for AppError {
    fn from(e: serenity::Error) -> Self {
        AppError::Discord(e.to_string())
    }
}
