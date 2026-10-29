//! X (Twitter) tweet, threads, and media extractor.
//!
//! Intercepts `x.com/.../status/...` and `twitter.com/.../status/...` URLs,
//! calling Twitter's public Syndication API (zero authentication key, anti-bot resilient)
//! to extract tweet body, author profile, timestamp, high-resolution media, quote tweets, and metrics.

use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::TklessError;
use crate::model::{ExtractionOptions, PageDocument, PageMetadata};
use crate::token::count_tokens;
use serde_json::Value;
use tracing::debug;
use url::Url;

pub struct TwitterAdapter;

impl Default for TwitterAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl TwitterAdapter {
    pub fn new() -> Self {
        Self
    }

    /// Extracts numerical Tweet ID from standard X/Twitter status URLs.
    pub fn extract_tweet_id(url: &Url) -> Option<String> {
        let segments: Vec<&str> = url.path().split('/').filter(|s| !s.is_empty()).collect();
        if let Some(pos) = segments.iter().position(|&s| s == "status") {
            if let Some(id_part) = segments.get(pos + 1) {
                let clean_id = id_part.split('?').next().unwrap_or("").trim();
                if clean_id.chars().all(|c| c.is_ascii_digit()) && !clean_id.is_empty() {
                    return Some(clean_id.to_string());
                }
            }
        }
        None
    }

    /// Parses a Twitter Syndication API JSON payload into high-fidelity Markdown.
    pub fn parse_tweet_json(val: &Value, tweet_id: &str) -> (String, String, Option<String>) {
        let user = val.get("user");
        let name = user
            .and_then(|u| u.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("Twitter User");
        let screen_name = user
            .and_then(|u| u.get("screen_name"))
            .and_then(|s| s.as_str())
            .unwrap_or("user");
        let verified = user
            .and_then(|u| u.get("verified"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let text = val
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim();
        let created_at = val.get("created_at").and_then(|c| c.as_str()).unwrap_or("");
        let likes = val
            .get("favorite_count")
            .and_then(|f| f.as_u64())
            .unwrap_or(0);
        let replies = val
            .get("conversation_count")
            .and_then(|c| c.as_u64())
            .unwrap_or(0);

        let verified_badge = if verified { " (已认证)" } else { "" };

        let mut out = String::new();
        out.push_str(&format!("# @{screen_name} ({name}{verified_badge})\n\n"));
        out.push_str(&format!(
            "> **推文链接**: [https://x.com/{screen_name}/status/{tweet_id}](https://x.com/{screen_name}/status/{tweet_id}) | **发布时间**: `{created_at}` | **点赞**: {likes} | **回复**: {replies}\n\n"
        ));

        out.push_str("### 推文正文\n\n");
        out.push_str(text);
        out.push_str("\n\n");

        // Media attachments (photos / videos)
        if let Some(media) = val.get("mediaDetails").and_then(|m| m.as_array()) {
            if !media.is_empty() {
                out.push_str("### 附带多媒体\n\n");
                for (idx, item) in media.iter().enumerate() {
                    let media_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("image");
                    let media_url = item
                        .get("media_url_https")
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    if !media_url.is_empty() {
                        out.push_str(&format!("- [{media_type} #{}]({media_url})\n", idx + 1));
                    }
                }
                out.push('\n');
            }
        }

        // Quoted tweet
        if let Some(quote) = val.get("quoted_tweet") {
            let q_user = quote.get("user");
            let q_name = q_user
                .and_then(|u| u.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("User");
            let q_screen = q_user
                .and_then(|u| u.get("screen_name"))
                .and_then(|s| s.as_str())
                .unwrap_or("user");
            let q_text = quote.get("text").and_then(|t| t.as_str()).unwrap_or("");

            out.push_str("### 引用推文 (Quoted Tweet)\n\n");
            out.push_str(&format!("> **@{q_screen} ({q_name})**:\n"));
            for line in q_text.lines() {
                out.push_str(&format!("> {line}\n"));
            }
            out.push('\n');
        }

        let title = format!(
            "@{screen_name}: \"{}\"",
            if text.len() > 60 { &text[..60] } else { text }
        );
        (out, title, Some(screen_name.to_string()))
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        debug!(url = %url, "Executing X/Twitter adapter extraction");

        let tweet_id = Self::extract_tweet_id(url).ok_or_else(|| {
            TklessError::InvalidArgument(format!("Invalid X/Twitter status URL: {url}"))
        })?;

        // 1. Fetch from Twitter Syndication API endpoint (No API key needed)
        let syndication_url_str =
            format!("https://cdn.syndication.twimg.com/tweet-result?id={tweet_id}&token=x");

        if let Ok(parsed_syn_url) = Url::parse(&syndication_url_str) {
            let resp = fetcher
                .get(&parsed_syn_url)
                .header(
                    "User-Agent",
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
                )
                .send()
                .await?;

            if resp.status().is_success() {
                if let Ok(val) = resp.json::<Value>().await {
                    let (content, title, author) = Self::parse_tweet_json(&val, &tweet_id);
                    let token_count = count_tokens(&content, options.tokenizer);

                    return Ok(PageDocument {
                        extraction: Default::default(),
                        warnings: Vec::new(),
                        url: format!("https://x.com/i/status/{tweet_id}"),
                        final_url: format!("https://x.com/i/status/{tweet_id}"),
                        status: 200,
                        title: Some(title),
                        byline: author.clone(),
                        content,
                        metadata: PageMetadata {
                            author,
                            canonical_url: Some(format!("https://x.com/i/status/{tweet_id}")),
                            ..Default::default()
                        },
                        json_ld: Vec::new(),
                        token_count,
                        tokenizer: options.tokenizer.as_str().to_string(),
                        truncated: false,
                        subtitles: None,
                        danmaku: None,
                        comments: None,
                        conclusion: None,
                        links: None,
                        images: None,
                        tables: None,
                        entity: None,
                        chunks: None,
                        content_hash: None,
                        metrics: None,
                    });
                }
            }
        }

        // 2. Fallback to standard web fetching if Syndication fails
        let web_resp = fetcher.get(url).send().await?;
        let html = web_resp.text().await?;
        let cleaned = crate::cleaner::clean_html(&html, Some(url.as_str()), options.mode)?;
        let converter = crate::markdown::MarkdownConverter::new();
        let content =
            converter.convert(&cleaned.html, options.strip_images, options.strip_links)?;
        let token_count = count_tokens(&content, options.tokenizer);

        Ok(PageDocument {
            extraction: Default::default(),
            warnings: Vec::new(),
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: cleaned.title,
            byline: None,
            content,
            metadata: PageMetadata {
                canonical_url: Some(url.to_string()),
                ..Default::default()
            },
            json_ld: Vec::new(),
            token_count,
            tokenizer: options.tokenizer.as_str().to_string(),
            truncated: false,
            subtitles: None,
            danmaku: None,
            comments: None,
            conclusion: None,
            links: None,
            images: None,
            tables: None,
            entity: None,
            chunks: None,
            content_hash: None,
            metrics: None,
        })
    }
}

impl PlatformAdapter for TwitterAdapter {
    fn name(&self) -> &'static str {
        "twitter"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("").to_lowercase();
        (crate::fetcher::is_subdomain_or_exact(&host, "x.com")
            || crate::fetcher::is_subdomain_or_exact(&host, "twitter.com"))
            && url.path().contains("/status/")
    }

    fn extract<'a>(
        &'a self,
        fetcher: &'a crate::fetcher::Fetcher,
        url: &'a Url,
        options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<PageDocument, TklessError>> {
        Box::pin(self.do_extract(fetcher, url, options))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_twitter_extract_tweet_id() {
        let u1 = Url::parse("https://x.com/rustlang/status/1890000000000000000").unwrap();
        assert_eq!(
            TwitterAdapter::extract_tweet_id(&u1),
            Some("1890000000000000000".to_string())
        );

        let u2 = Url::parse("https://twitter.com/ferris/status/987654321?s=20").unwrap();
        assert_eq!(
            TwitterAdapter::extract_tweet_id(&u2),
            Some("987654321".to_string())
        );

        let u3 = Url::parse("https://x.com/home").unwrap();
        assert_eq!(TwitterAdapter::extract_tweet_id(&u3), None);
    }

    #[test]
    fn test_twitter_parse_tweet_json() {
        let json_str = r#"{
            "text": "Announcing TkLess: Spend less tokens, perceive more web!",
            "created_at": "Sun Oct 18 12:00:00 +0000 2026",
            "favorite_count": 250,
            "conversation_count": 42,
            "user": {
                "name": "TkLess Team",
                "screen_name": "tkless_ai",
                "verified": true
            },
            "mediaDetails": [
                {
                    "type": "photo",
                    "media_url_https": "https://pbs.twimg.com/media/example.jpg"
                }
            ],
            "quoted_tweet": {
                "text": "Great tool for LLM scraping!",
                "user": {
                    "name": "Ferris",
                    "screen_name": "rust_crab"
                }
            }
        }"#;

        let val: Value = serde_json::from_str(json_str).unwrap();
        let (md, title, author) = TwitterAdapter::parse_tweet_json(&val, "12345");

        assert_eq!(author, Some("tkless_ai".to_string()));
        assert!(title.contains("@tkless_ai"));
        assert!(md.contains("TkLess Team"));
        assert!(md.contains("Spend less tokens"));
        assert!(md.contains("250"));
        assert!(md.contains("example.jpg"));
        assert!(md.contains("Great tool for LLM scraping!"));
    }
}
