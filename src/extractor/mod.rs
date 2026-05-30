pub mod oembed;

use crate::error::AppError;
use url::Url;

/// Extract the raw URL from a message that might contain multiple URLs or text
pub fn extract_url(text: &str) -> Option<url::Url> {
    // Simple URL extraction: find the first http/https URL
    for word in text.split_whitespace() {
        if let Ok(url) = Url::parse(word) {
            if url.scheme() == "http" || url.scheme() == "https" {
                // Normalize: strip trailing punctuation that's not part of the URL
                let cleaned = clean_url(word);
                if let Ok(cleaned_url) = Url::parse(&cleaned) {
                    return Some(cleaned_url);
                }
            }
        }
    }
    None
}

/// Strip trailing punctuation that isn't part of the URL
fn clean_url(s: &str) -> String {
    s.trim_end_matches(&[
        '.', ',', '!', '?', ';', ':', ')', ']', '}', '"', '\'', '>',
    ])
    .to_string()
}

/// Determine what kind of platform a URL is from
#[derive(Debug, Clone, PartialEq)]
pub enum Platform {
    TikTok,
    Instagram,
    YouTube,
    Other,
}

pub fn identify_platform(url: &Url) -> Platform {
    let host = url.host_str().unwrap_or("");
    match host {
        h if h.contains("tiktok") => Platform::TikTok,
        h if h.contains("instagram") => Platform::Instagram,
        h if h.contains("youtube") || h.contains("youtu.be") => Platform::YouTube,
        _ => Platform::Other,
    }
}

/// Fetch page content via oEmbed (TikTok, Instagram) or Open Graph tags
pub async fn fetch_page_content(url: &Url) -> Result<PageMetadata, AppError> {
    let platform = identify_platform(url);

    match platform {
        Platform::TikTok => oembed::fetch_tiktok_oembed(url).await,
        Platform::Instagram => oembed::fetch_instagram_oembed(url).await,
        Platform::YouTube => oembed::fetch_youtube_oembed(url).await,
        Platform::Other => oembed::fetch_og_tags(url).await,
    }
}

/// Structured metadata extracted from a page
#[derive(Debug, Clone, Default)]
pub struct PageMetadata {
    pub title: String,
    pub description: String,
    pub author: Option<String>,
    pub content: String,
}
