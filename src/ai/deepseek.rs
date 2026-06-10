use baochuan::providers::DeepSeekProvider;
use baochuan::types::{ChatMessage, ChatRequestBuilder};
use baochuan::Provider;

use crate::db::models::{ExtractedInfo, Restaurant};
use crate::error::AppError;

pub struct DeepSeekClient {
    provider: DeepSeekProvider,
}

impl DeepSeekClient {
    pub fn new(api_key: String) -> Self {
        Self {
            provider: DeepSeekProvider::new(api_key),
        }
    }

    /// Call the DeepSeek chat API via baochuan
    async fn chat(&self, system: &str, user: &str) -> Result<String, AppError> {
        let request = ChatRequestBuilder::new("deepseek-chat")
            .message(ChatMessage::system(system))
            .message(ChatMessage::user(user))
            .temperature(0.3)
            .max_tokens(1000)
            .build()
            .map_err(|e| AppError::Ai(format!("Failed to build request: {e}")))?;

        let response = self
            .provider
            .chat(&request)
            .await
            .map_err(|e| AppError::Ai(format!("DeepSeek API error: {e}")))?;

        response
            .content()
            .map(|s| s.to_string())
            .ok_or_else(|| AppError::Ai("Empty response from API".to_string()))
    }

    /// Same as chat but with higher token limit for multi-restaurant responses
    async fn chat_long(&self, system: &str, user: &str) -> Result<String, AppError> {
        let request = ChatRequestBuilder::new("deepseek-chat")
            .message(ChatMessage::system(system))
            .message(ChatMessage::user(user))
            .temperature(0.3)
            .max_tokens(2000)
            .build()
            .map_err(|e| AppError::Ai(format!("Failed to build request: {e}")))?;

        let response = self
            .provider
            .chat(&request)
            .await
            .map_err(|e| AppError::Ai(format!("DeepSeek API error: {e}")))?;

        response
            .content()
            .map(|s| s.to_string())
            .ok_or_else(|| AppError::Ai("Empty response from API".to_string()))
    }

    /// Helper: clean common formatting around AI JSON responses
    fn clean_json(raw: &str) -> String {
        raw.trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim()
            .to_string()
    }

    /// Extract restaurant info from a social media post (single restaurant)
    pub async fn extract_restaurant_info(
        &self,
        page_title: &str,
        page_content: &str,
        source_url: &str,
    ) -> Result<ExtractedInfo, AppError> {
        let system = "You are a restaurant information extractor. Given content from a social media post about food, extract the restaurant details. \
            Return ONLY valid JSON with no markdown formatting or code blocks. Use these fields:\n\
            - restaurant_name: The name of the restaurant (null if unclear)\n\
            - cuisine_type: Short cuisine description e.g. \"Korean BBQ\", \"Italian pasta\" (null if unclear)\n\
            - tags: Array of tags e.g. [\"korean\", \"bbq\", \"authentic\"] (empty array if none)\n\
            - google_maps_query: A search query for Google Maps to find this restaurant (null if unclear)\n\
            - description: A brief 1-2 sentence description of what was shown";

        let user = format!(
            "Source URL: {source_url}\nTitle: {page_title}\nContent:\n{page_content}"
        );

        let raw = self.chat(system, &user).await?;

        let cleaned = Self::clean_json(&raw);

        serde_json::from_str::<ExtractedInfo>(&cleaned).map_err(|e| {
            AppError::Ai(format!(
                "Failed to parse AI response as JSON. Raw: {raw}. Error: {e}"
            ))
        })
    }

    /// Extract ALL restaurants from a social media post (supports videos with multiple venues)
    pub async fn extract_restaurants(
        &self,
        page_title: &str,
        page_content: &str,
        source_url: &str,
    ) -> Result<Vec<ExtractedInfo>, AppError> {
        let system = "You are a restaurant information extractor. Given content from a social media post about food, \
            identify ALL restaurants mentioned or shown in the content. Return ONLY a valid JSON array with \
            no markdown formatting or code blocks.\n\n\
            Even if only one restaurant is found, return it as a single-element array. If no restaurants \
            are mentioned at all, return an empty array [].\n\n\
            Each element has these fields:\n\
            - restaurant_name: The name of the restaurant (null if unclear)\n\
            - cuisine_type: Short cuisine description e.g. \"Korean BBQ\", \"Italian pasta\" (null if unclear)\n\
            - tags: Array of tags e.g. [\"korean\", \"bbq\", \"authentic\"] (empty array if none)\n\
            - google_maps_query: A search query for Google Maps to find this restaurant (null if unclear)\n\
            - description: A brief 1-2 sentence description of what was shown for this restaurant";

        let user = format!(
            "Source URL: {source_url}\nTitle: {page_title}\nContent:\n{page_content}"
        );

        let raw = self.chat_long(system, &user).await?;

        let cleaned = Self::clean_json(&raw);

        serde_json::from_str::<Vec<ExtractedInfo>>(&cleaned).map_err(|e| {
            AppError::Ai(format!(
                "Failed to parse AI response as JSON array. Raw: {raw}. Error: {e}"
            ))
        })
    }

    /// Recommend restaurants based on a user query
    pub async fn recommend(
        &self,
        query: &str,
        restaurants: &[Restaurant],
    ) -> Result<String, AppError> {
        if restaurants.is_empty() {
            return Ok("You don't have any saved restaurants yet! Share a link to start building your backlog.".to_string());
        }

        let restaurant_list: Vec<String> = restaurants
            .iter()
            .map(|r| {
                let visited = if r.visited { " (visited)" } else { "" };
                format!(
                    "- {name}{visited}\n  Tags: {tags}\n  Description: {desc}\n  Maps: {maps}",
                    name = r.name,
                    visited = visited,
                    tags = r.cuisine_tags.join(", "),
                    desc = r.description.as_deref().unwrap_or("No description"),
                    maps = r.google_maps_url.as_deref().unwrap_or("No link")
                )
            })
            .collect();

        let system = "You are a restaurant recommendation assistant. The user has a backlog of saved restaurants. \
            Given their query/mood/craving, recommend 1-3 restaurants from their list that best match. \
            Explain why each recommendation fits their request. Be friendly and enthusiastic. \
            If nothing matches well, suggest the closest options and explain why.";

        let prompt = format!(
            "User's request: {query}\n\nTheir saved restaurants:\n{}",
            restaurant_list.join("\n")
        );

        self.chat(system, &prompt).await
    }
}
