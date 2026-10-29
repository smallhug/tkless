//! Discourse forum & linux.do community topic and discussion adapter.
//!
//! Intercepts `linux.do/t/...`, `users.rust-lang.org/t/...`, `community.openai.com/t/...`, and other
//! Discourse-powered platforms, leveraging native `/t/{id}.json` REST APIs and HTML fallback
//! to extract topic title, category, tags, original post body, and community replies.

use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::TklessError;
use crate::markdown::MarkdownConverter;
use crate::model::{ExtractionOptions, PageDocument, PageMetadata};
use crate::token::count_tokens;
use scraper::{Html, Selector};
use serde_json::Value;
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static DISCOURSE_TITLE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("h1.topic-title a, .title-wrapper h1 a, h1.fancy-title, h1").unwrap()
});
static DISCOURSE_POST_CONTAINER: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("div.topic-post, article.boxed, div.regular").unwrap());
static DISCOURSE_AUTHOR_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("span.username a, span.creator a, .names span.username").unwrap()
});
static DISCOURSE_COOKED_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("div.cooked, div.post").unwrap());

pub struct DiscourseAdapter;

impl Default for DiscourseAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl DiscourseAdapter {
    pub fn new() -> Self {
        Self
    }

    /// Extracts the Discourse domain and topic ID from a URL.
    pub fn extract_topic_info(url: &Url) -> Option<(String, u64)> {
        let host = url.host_str()?.to_lowercase();
        let segments: Vec<&str> = url.path().split('/').filter(|s| !s.is_empty()).collect();

        // Standard Discourse URL patterns:
        // /t/topic-slug/12345
        // /t/topic-slug/12345/2 (pagination / reply offset)
        // /t/12345
        if let Some(pos) = segments.iter().position(|&s| s == "t") {
            for segment in segments.iter().skip(pos + 1) {
                if let Ok(id) = segment.parse::<u64>() {
                    return Some((host, id));
                }
            }
        }

        None
    }

    /// Parses a Discourse JSON payload (from `/t/{id}.json`) into clean structured Markdown.
    pub fn parse_topic_json(val: &Value, domain: &str, _topic_id: u64) -> (String, String) {
        let title = val
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("Discourse Topic")
            .to_string();

        let views = val.get("views").and_then(|v| v.as_u64()).unwrap_or(0);
        let like_count = val.get("like_count").and_then(|l| l.as_u64()).unwrap_or(0);
        let posts_count = val.get("posts_count").and_then(|p| p.as_u64()).unwrap_or(0);

        let mut tags_str = String::new();
        if let Some(tags) = val.get("tags").and_then(|t| t.as_array()) {
            let tag_names: Vec<&str> = tags.iter().filter_map(|t| t.as_str()).collect();
            if !tag_names.is_empty() {
                tags_str = format!(" | **标签**: `{}`", tag_names.join("`, `"));
            }
        }

        let mut out = String::new();
        out.push_str(&format!("# {title}\n\n"));
        out.push_str(&format!(
            "> **社区**: `{domain}` | **浏览量**: {views} | **点赞数**: {like_count} | **回帖数**: {posts_count}{tags_str}\n\n"
        ));

        let converter = MarkdownConverter::new();

        if let Some(post_stream) = val
            .get("post_stream")
            .and_then(|ps| ps.get("posts"))
            .and_then(|p| p.as_array())
        {
            for (idx, post) in post_stream.iter().enumerate() {
                let post_num = post
                    .get("post_number")
                    .and_then(|pn| pn.as_u64())
                    .unwrap_or((idx + 1) as u64);
                let username = post
                    .get("username")
                    .and_then(|u| u.as_str())
                    .unwrap_or("anonymous");
                let post_time = post
                    .get("created_at")
                    .and_then(|ct| ct.as_str())
                    .unwrap_or("");
                let likes = post.get("like_count").and_then(|l| l.as_u64()).unwrap_or(0);
                let cooked_html = post.get("cooked").and_then(|c| c.as_str()).unwrap_or("");

                let clean_body = if !cooked_html.is_empty() {
                    converter
                        .convert(cooked_html, false, false)
                        .unwrap_or_else(|_| cooked_html.to_string())
                } else if let Some(raw) = post.get("raw").and_then(|r| r.as_str()) {
                    raw.to_string()
                } else {
                    String::new()
                };

                if post_num == 1 {
                    out.push_str("## 主楼楼主正文\n\n");
                    out.push_str(&format!(
                        "> **发帖人**: `{username}` | **时间**: {post_time} | **点赞**: {likes}\n\n"
                    ));
                    out.push_str(&clean_body);
                    out.push_str("\n\n---\n\n## 社区精选回帖\n\n");
                } else {
                    out.push_str(&format!(
                        "### 楼层 #{post_num} | `{username}` {}\n\n",
                        if likes > 0 {
                            format!("(点赞: {likes})")
                        } else {
                            String::new()
                        }
                    ));
                    out.push_str(&clean_body);
                    out.push_str("\n\n");
                }
            }
        }

        (out, title)
    }

    /// Parses a Discourse topic HTML page as a fallback.
    pub fn parse_topic_html(html: &str, domain: &str, topic_id: u64) -> (String, String) {
        let doc = Html::parse_document(html);
        let title = doc
            .select(&DISCOURSE_TITLE_SELECTOR)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_else(|| format!("Discourse Topic #{topic_id}"));

        let mut out = String::new();
        out.push_str(&format!("# {title}\n\n"));
        out.push_str(&format!(
            "> **社区**: `{domain}` | **链接**: https://{domain}/t/{topic_id}\n\n---\n\n"
        ));

        let converter = MarkdownConverter::new();
        let mut count = 0;

        for (idx, post) in doc.select(&DISCOURSE_POST_CONTAINER).enumerate() {
            count += 1;
            let author = post
                .select(&DISCOURSE_AUTHOR_SELECTOR)
                .next()
                .map(|e| e.text().collect::<String>().trim().to_string())
                .unwrap_or_else(|| "anonymous".to_string());

            let body_html = post
                .select(&DISCOURSE_COOKED_SELECTOR)
                .next()
                .map(|e| e.inner_html())
                .unwrap_or_default();

            if body_html.is_empty() {
                continue;
            }

            let md = converter
                .convert(&body_html, false, false)
                .unwrap_or_else(|_| body_html.clone());

            if idx == 0 {
                out.push_str(&format!("## 主楼楼主正文 (`{author}`)\n\n"));
                out.push_str(&md);
                out.push_str("\n\n---\n\n## 社区回帖\n\n");
            } else {
                out.push_str(&format!("### 楼层 #{} | `{author}`\n\n", idx + 1));
                out.push_str(&md);
                out.push_str("\n\n");
            }
        }

        if count == 0 {
            out.push_str("_正文已清洗或需登录访问。_\n\n");
        }

        (out, title)
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        debug!(url = %url, "Executing Discourse adapter extraction");

        let (domain, topic_id) = Self::extract_topic_info(url).ok_or_else(|| {
            TklessError::InvalidArgument(format!("Invalid Discourse topic URL: {url}"))
        })?;

        // 1. Attempt official Discourse JSON API first: /t/{id}.json?include_raw=1
        let json_api_url_str = format!("https://{domain}/t/{topic_id}.json?include_raw=1");
        if let Ok(parsed_api_url) = Url::parse(&json_api_url_str) {
            if let Ok(resp) = fetcher
                .get(&parsed_api_url)
                .header(
                    "User-Agent",
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
                )
                .header("Accept", "application/json")
                .send()
                .await
            {
                if resp.status().is_success() {
                    if let Ok(val) = resp.json::<Value>().await {
                        let (content, title) = Self::parse_topic_json(&val, &domain, topic_id);
                        let token_count = count_tokens(&content, options.tokenizer);
                        return Ok(PageDocument {
            extraction: Default::default(),
            warnings: Vec::new(),
                            url: format!("https://{domain}/t/{topic_id}"),
                            final_url: format!("https://{domain}/t/{topic_id}"),
                            status: 200,
                            title: Some(title),
                            byline: None,
                            content,
                            metadata: PageMetadata {
                                canonical_url: Some(format!("https://{domain}/t/{topic_id}")),
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
        }

        // 2. HTML fallback if JSON API blocked or protected
        let resp = fetcher.get(url).send().await?;
        let html = resp.text().await?;
        let (content, title) = Self::parse_topic_html(&html, &domain, topic_id);
        let token_count = count_tokens(&content, options.tokenizer);

        Ok(PageDocument {
            extraction: Default::default(),
            warnings: Vec::new(),
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: Some(title),
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

impl PlatformAdapter for DiscourseAdapter {
    fn name(&self) -> &'static str {
        "discourse"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("").to_lowercase();
        if crate::fetcher::is_subdomain_or_exact(&host, "linux.do")
            || crate::fetcher::is_subdomain_or_exact(&host, "users.rust-lang.org")
            || crate::fetcher::is_subdomain_or_exact(&host, "community.openai.com")
            || crate::fetcher::is_subdomain_or_exact(&host, "meta.discourse.org")
            || crate::fetcher::is_subdomain_or_exact(&host, "discuss.pytorch.org")
            || crate::fetcher::is_subdomain_or_exact(&host, "forum.nixos.org")
        {
            return true;
        }

        // Generic detection: path has /t/... and parses a numeric topic ID
        if Self::extract_topic_info(url).is_some() {
            let path = url.path();
            if path.starts_with("/t/") {
                return true;
            }
        }

        false
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
    fn test_discourse_extract_topic_info() {
        let u1 = Url::parse("https://linux.do/t/topic-title-slug/12345").unwrap();
        assert_eq!(
            DiscourseAdapter::extract_topic_info(&u1),
            Some(("linux.do".to_string(), 12345))
        );

        let u2 = Url::parse("https://linux.do/t/12345/2").unwrap();
        assert_eq!(
            DiscourseAdapter::extract_topic_info(&u2),
            Some(("linux.do".to_string(), 12345))
        );

        let u3 = Url::parse("https://users.rust-lang.org/t/announcing-rust-1-85/109876").unwrap();
        assert_eq!(
            DiscourseAdapter::extract_topic_info(&u3),
            Some(("users.rust-lang.org".to_string(), 109876))
        );
    }

    #[test]
    fn test_discourse_parse_topic_json() {
        let json_str = r#"{
            "title": "TkLess 高性能清洗引擎上线",
            "views": 2500,
            "like_count": 88,
            "posts_count": 2,
            "created_at": "2026-10-18T12:00:00Z",
            "tags": ["rust", "agent"],
            "post_stream": {
                "posts": [
                    {
                        "post_number": 1,
                        "username": "smallhug",
                        "created_at": "2026-10-18",
                        "like_count": 50,
                        "cooked": "<p>今天正式发布 TkLess v0.4.0！欢迎体验！</p>"
                    },
                    {
                        "post_number": 2,
                        "username": "linus",
                        "created_at": "2026-10-18",
                        "like_count": 38,
                        "cooked": "<p>看起来非常不错，性能很强！</p>"
                    }
                ]
            }
        }"#;

        let val: Value = serde_json::from_str(json_str).unwrap();
        let (md, title) = DiscourseAdapter::parse_topic_json(&val, "linux.do", 12345);
        assert_eq!(title, "TkLess 高性能清洗引擎上线");
        assert!(md.contains("linux.do"));
        assert!(md.contains("smallhug"));
        assert!(md.contains("今天正式发布 TkLess"));
        assert!(md.contains("linus"));
        assert!(md.contains("看起来非常不错"));
    }

    #[test]
    fn test_discourse_parse_topic_html() {
        let html = r#"
        <h1 class="topic-title"><a href="/t/123">Discourse HTML Topic</a></h1>
        <div class="topic-post">
            <span class="username"><a href="/u/neo">neo</a></span>
            <div class="cooked"><p>Hello Matrix.</p></div>
        </div>
        "#;

        let (md, title) = DiscourseAdapter::parse_topic_html(html, "linux.do", 123);
        assert_eq!(title, "Discourse HTML Topic");
        assert!(md.contains("neo"));
        assert!(md.contains("Hello Matrix."));
    }
}
