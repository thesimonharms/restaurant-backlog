use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Restaurant {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub user_id: i64,
    pub name: String,
    pub source_url: Option<String>,
    pub google_maps_url: Option<String>,
    pub description: Option<String>,
    pub cuisine_tags: Vec<String>,
    pub notes: String,
    pub created_at: DateTime<Utc>,
    pub visited: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewRestaurant {
    pub owner_id: Uuid,
    pub user_id: i64,
    pub name: String,
    pub source_url: Option<String>,
    pub google_maps_url: Option<String>,
    pub description: Option<String>,
    pub cuisine_tags: Vec<String>,
}

/// AI-extracted restaurant info from a social media post
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedInfo {
    pub restaurant_name: Option<String>,
    pub cuisine_type: Option<String>,
    pub tags: Vec<String>,
    pub google_maps_query: Option<String>,
    pub description: Option<String>,
}
