use super::PageMetadata;
use crate::error::AppError;
use url::Url;

/// Fetch TikTok oEmbed data
pub async fn fetch_tiktok_oembed(url: &Url) -> Result<PageMetadata, AppError> {
    let encoded: String = url::form_urlencoded::byte_serialize(url.as_str().as_bytes()).collect();
    let oembed_url = format!("https://www.tiktok.com/oembed?url={encoded}");

    let client = reqwest::Client::new();
    let resp = client
        .get(&oembed_url)
        .header("User-Agent", "Mozilla/5.0 (compatible; RestaurantBacklogBot/1.0)")
        .send()
        .await?;

    if !resp.status().is_success() {
        // Fall back to OG tags
        return fetch_og_tags(url).await;
    }

    let data = match parse_oembed_json(resp).await {
        Ok(data) => data,
        Err(e) => {
            tracing::warn!("TikTok oEmbed returned an undecodable response, falling back to Open Graph tags: {e}");
            return fetch_og_tags(url).await;
        }
    };

    let title = data["title"]
        .as_str()
        .unwrap_or("Untitled TikTok")
        .to_string();

    let description = data["description"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let author = data["author_name"].as_str().map(|s| s.to_string());

    let content = vec![
        title.clone(),
        description.clone(),
        author.clone().unwrap_or_default(),
    ]
    .join("\n");

    Ok(PageMetadata {
        title,
        description,
        author,
        content,
    })
}

/// Fetch Instagram post by scraping OG tags directly with a mobile User-Agent
///
/// Instagram's oEmbed API now requires a Meta app access token, and the desktop
/// login page has no useful metadata. However, Instagram serves full OG tags
/// (og:title, og:description, name="description" with caption text) to mobile
/// User-Agents even without logging in.
pub async fn fetch_instagram_oembed(url: &Url) -> Result<PageMetadata, AppError> {
    let mut clean_url = url.clone();
    clean_url.set_query(None); // strip tracking params like ?igsh=

    tracing::info!("Fetching Instagram post with mobile UA: {clean_url}");

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36")
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let resp = client.get(clean_url.as_str()).send().await?;

    if !resp.status().is_success() {
        tracing::warn!("Instagram returned status {} for {clean_url}", resp.status());
        return fetch_og_tags_or_url(url, "Instagram Post").await;
    }

    let html = resp.text().await?;
    let document = scraper::Html::parse_document(&html);

    // Extract og:title or twitter:title
    let title_selector =
        scraper::Selector::parse("meta[property='og:title'], meta[name='twitter:title'], title")
            .unwrap();
    let title = document
        .select(&title_selector)
        .next()
        .and_then(|el| {
            if el.value().name() == "title" {
                Some(el.text().collect::<String>())
            } else {
                el.value().attr("content").map(|s| s.to_string())
            }
        })
        .unwrap_or_else(|| "Instagram Post".to_string());

    // Extract og:description or meta description (contains the post caption)
    let desc_selector = scraper::Selector::parse(
        "meta[property='og:description'], meta[name='description'], meta[name='twitter:description']",
    )
    .unwrap();
    let og_description = document
        .select(&desc_selector)
        .next()
        .and_then(|el| el.value().attr("content"))
        .unwrap_or("")
        .to_string();

    // Extract author from twitter:title (format: "Username (@handle) • Instagram post type")
    // or from the og:title prefix
    let author = title
        .split(" on Instagram")
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let content = format!("{title}\n\n{og_description}");

    Ok(PageMetadata {
        title,
        description: og_description,
        author,
        content,
    })
}

/// Fetch YouTube oEmbed data
pub async fn fetch_youtube_oembed(url: &Url) -> Result<PageMetadata, AppError> {
    let encoded: String = url::form_urlencoded::byte_serialize(url.as_str().as_bytes()).collect();
    let oembed_url = format!("https://www.youtube.com/oembed?url={encoded}&format=json");

    let client = reqwest::Client::new();
    let resp = client
        .get(&oembed_url)
        .header("User-Agent", "Mozilla/5.0 (compatible; RestaurantBacklogBot/1.0)")
        .send()
        .await?;

    if !resp.status().is_success() {
        return fetch_og_tags(url).await;
    }

    let data = match parse_oembed_json(resp).await {
        Ok(data) => data,
        Err(e) => {
            tracing::warn!("YouTube oEmbed returned an undecodable response, falling back to Open Graph tags: {e}");
            return fetch_og_tags(url).await;
        }
    };

    let title = data["title"]
        .as_str()
        .unwrap_or("Untitled Video")
        .to_string();

    let author = data["author_name"].as_str().map(|s| s.to_string());

    let description = format!("YouTube video by {}", author.as_deref().unwrap_or("unknown"));

    let content = vec![title.clone(), description.clone()].join("\n");

    Ok(PageMetadata {
        title,
        description,
        author,
        content,
    })
}

async fn fetch_og_tags_or_url(url: &Url, fallback_title: &str) -> Result<PageMetadata, AppError> {
    match fetch_og_tags(url).await {
        Ok(metadata) => Ok(metadata),
        Err(e) => {
            tracing::warn!("Open Graph fallback failed for {url}, using URL-only metadata: {e}");
            Ok(PageMetadata {
                title: fallback_title.to_string(),
                description: String::new(),
                content: format!("{fallback_title}\n{}", url.as_str()),
                ..Default::default()
            })
        }
    }
}

async fn parse_oembed_json(resp: reqwest::Response) -> Result<serde_json::Value, AppError> {
    let body = resp.text().await?;
    serde_json::from_str(&body).map_err(AppError::from)
}

/// Fallback: extract Open Graph tags and page content via HTML scraping
pub async fn fetch_og_tags(url: &Url) -> Result<PageMetadata, AppError> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let resp = client.get(url.as_str()).send().await?;

    if !resp.status().is_success() {
        return Ok(PageMetadata {
            title: "Could not access page".to_string(),
            description: format!("Page returned status {}", resp.status()),
            content: url.as_str().to_string(),
            ..Default::default()
        });
    }

    let html = resp.text().await?;
    let document = scraper::Html::parse_document(&html);

    // Extract title
    let title_selector = scraper::Selector::parse("meta[property='og:title'], meta[name='twitter:title'], title").unwrap();
    let title = document
        .select(&title_selector)
        .next()
        .and_then(|el| {
            if el.value().name() == "title" {
                Some(el.text().collect::<String>())
            } else {
                el.value().attr("content").map(|s| s.to_string())
            }
        })
        .unwrap_or_else(|| "Untitled".to_string());

    // Extract description
    let desc_selector = scraper::Selector::parse("meta[property='og:description'], meta[name='description'], meta[name='twitter:description']").unwrap();
    let description = document
        .select(&desc_selector)
        .next()
        .and_then(|el| el.value().attr("content"))
        .unwrap_or("")
        .to_string();

    // Extract all visible text (limited for AI cost)
    let body_selector = scraper::Selector::parse("p, h1, h2, h3, span, li").unwrap();
    let body_text: Vec<String> = document
        .select(&body_selector)
        .take(50)
        .map(|el| el.text().collect::<String>())
        .filter(|t| !t.trim().is_empty())
        .collect();
    let content = body_text.join("\n");

    let content = if content.len() > 2000 {
        format!("{}\n...", &content[..2000])
    } else {
        content
    };

    Ok(PageMetadata {
        title: title.clone(),
        description,
        content: format!("{}\n\n{}", title, content),
        ..Default::default()
    })
}
