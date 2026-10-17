//! Reddit community discussion and post adapter.
//!
//! Intercepts `reddit.com/r/.../comments/...` and `redd.it/...` URLs, extracting post title,
//! author, subreddit, score, native selftext Markdown, and hierarchical comment trees
//! without desktop React hydration barriers or login popups.

use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::PageclawError;
use crate::model::{ExtractionOptions, OutputFormat, PageDocument, PageMetadata};
use crate::token::count_tokens;
use serde_json::Value;
use tracing::debug;
use url::Url;

pub struct RedditAdapter;

impl Default for RedditAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl RedditAdapter {
    pub fn new() -> Self {
        Self
    }

    /// Canonicalizes a Reddit URL into its JSON API endpoint (e.g. `https://www.reddit.com/.../.json`).
    pub fn canonicalize_json_url(url: &Url) -> Option<Url> {
        let mut clean_url = url.clone();
        let path = clean_url.path().trim_end_matches('/');

        // Handle redd.it shortlinks: redd.it/{id} -> reddit.com/comments/{id}/.json
        if let Some(host) = clean_url.host_str() {
            if host == "redd.it" {
                let id = path.trim_start_matches('/');
                return Url::parse(&format!("https://www.reddit.com/comments/{id}/.json")).ok();
            }
        }

        if !path.contains("/comments/") {
            return None;
        }

        let json_path = format!("{path}/.json");
        clean_url.set_path(&json_path);
        clean_url.set_query(Some("raw_json=1&limit=25"));
        Some(clean_url)
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        debug!(url = %url, "Executing Reddit adapter extraction");

        let json_url = Self::canonicalize_json_url(url).unwrap_or_else(|| url.clone());

        let resp = fetcher
            .get(&json_url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 PageClaw/0.1",
            )
            .send()
            .await?;

        let text = resp.text().await?;

        // Attempt JSON API parse first
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if let Some(doc) = self.parse_json_payload(&v, url, options) {
                return Ok(doc);
            }
        }

        // Fallback to standard HTML parsing if JSON failed
        let cleaned = crate::cleaner::clean_html(&text, Some(url.as_str()), options.mode)?;
        let converter = crate::markdown::MarkdownConverter::new();
        let md = converter.convert(&cleaned.html, options.strip_images, options.strip_links)?;

        let token_count = count_tokens(&md, options.tokenizer);
        Ok(PageDocument {
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: cleaned.title,
            byline: Some("Reddit".to_string()),
            content: md,
            metadata: PageMetadata::default(),
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

    pub fn parse_json_payload(
        &self,
        payload: &Value,
        original_url: &Url,
        options: &ExtractionOptions,
    ) -> Option<PageDocument> {
        let arr = payload.as_array()?;
        let post_listing = arr.first()?;
        let post_data = post_listing.pointer("/data/children/0/data")?;

        let title = post_data
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Reddit Post");
        let author = post_data
            .get("author")
            .and_then(|v| v.as_str())
            .unwrap_or("[deleted]");
        let subreddit = post_data
            .get("subreddit_name_prefixed")
            .and_then(|v| v.as_str())
            .unwrap_or("r/reddit");
        let score = post_data.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
        let upvote_ratio = post_data
            .get("upvote_ratio")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);
        let selftext = post_data
            .get("selftext")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let external_url = post_data.get("url").and_then(|v| v.as_str());

        // Extract comments from second listing element
        let mut comment_lines = Vec::new();
        if let Some(comments_listing) = arr.get(1) {
            if let Some(comments_arr) = comments_listing
                .pointer("/data/children")
                .and_then(|v| v.as_array())
            {
                for comment in comments_arr.iter().take(15) {
                    extract_comment_node(comment, 0, &mut comment_lines);
                }
            }
        }

        let content = match options.format {
            OutputFormat::Json => {
                let json_obj = serde_json::json!({
                    "title": title,
                    "author": author,
                    "subreddit": subreddit,
                    "score": score,
                    "upvote_ratio": upvote_ratio,
                    "selftext": selftext,
                    "external_url": external_url,
                    "comments": comment_lines
                });
                serde_json::to_string_pretty(&json_obj).unwrap_or_default()
            }
            _ => {
                let mut md = String::new();
                if options.frontmatter {
                    md.push_str("---\n");
                    md.push_str(&format!("title: \"{}\"\n", title.replace('"', "\\\"")));
                    md.push_str(&format!("author: \"u/{author}\"\n"));
                    md.push_str(&format!("subreddit: \"{subreddit}\"\n"));
                    md.push_str(&format!("score: {score}\n"));
                    md.push_str(&format!("url: \"{original_url}\"\n"));
                    md.push_str("---\n\n");
                }

                md.push_str(&format!("# {title}\n\n"));
                md.push_str(&format!(
                    "> **Subreddit**: `{subreddit}` | **作者**: u/{author} | **得分**: {score} (赞同率 {:.0}%)\n",
                    upvote_ratio * 100.0
                ));
                if let Some(ext) = external_url {
                    if ext != original_url.as_str() && !ext.contains("reddit.com") {
                        md.push_str(&format!("> **分享外链**: [{ext}]({ext})\n"));
                    }
                }
                md.push('\n');

                if !selftext.trim().is_empty() {
                    md.push_str(selftext.trim());
                    md.push_str("\n\n");
                }

                if !comment_lines.is_empty() {
                    md.push_str("---\n\n### 💬 社区精选评论 (Top Discussion):\n\n");
                    for line in comment_lines {
                        md.push_str(&line);
                        md.push('\n');
                    }
                }

                md
            }
        };

        let token_count = count_tokens(&content, options.tokenizer);

        Some(PageDocument {
            url: original_url.to_string(),
            final_url: original_url.to_string(),
            status: 200,
            title: Some(title.to_string()),
            byline: Some(format!("u/{author} in {subreddit}")),
            content,
            metadata: PageMetadata {
                author: Some(format!("u/{author}")),
                canonical_url: Some(original_url.to_string()),
                description: Some(selftext.chars().take(200).collect()),
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

fn extract_comment_node(node: &Value, depth: usize, out: &mut Vec<String>) {
    let data = match node.get("data") {
        Some(d) => d,
        None => return,
    };

    let author = data
        .get("author")
        .and_then(|v| v.as_str())
        .unwrap_or("[deleted]");
    let body = data.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let score = data.get("score").and_then(|v| v.as_i64()).unwrap_or(0);

    if body.trim().is_empty() || author == "[deleted]" && body == "[deleted]" {
        return;
    }

    let indent = "> ".repeat(depth + 1);
    out.push(format!("{indent}**u/{author}** (赞同: {score}):"));
    for line in body.lines() {
        if !line.trim().is_empty() {
            out.push(format!("{indent}{line}"));
        }
    }
    out.push(indent);

    // Recursively parse replies up to depth 2
    if depth < 2 {
        if let Some(replies_data) = data.get("replies").and_then(|r| r.get("data")) {
            if let Some(children) = replies_data.get("children").and_then(|c| c.as_array()) {
                for child in children.iter().take(3) {
                    extract_comment_node(child, depth + 1, out);
                }
            }
        }
    }
}

impl PlatformAdapter for RedditAdapter {
    fn name(&self) -> &'static str {
        "reddit"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        (crate::fetcher::is_subdomain_or_exact(host, "reddit.com")
            || crate::fetcher::is_subdomain_or_exact(host, "redd.it"))
            && (url.path().contains("/comments/") || host == "redd.it")
    }

    fn extract<'a>(
        &'a self,
        fetcher: &'a crate::fetcher::Fetcher,
        url: &'a Url,
        options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<PageDocument, PageclawError>> {
        Box::pin(self.do_extract(fetcher, url, options))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reddit_adapter_matching() {
        let adapter = RedditAdapter::new();
        let u1 =
            Url::parse("https://www.reddit.com/r/rust/comments/1i8z4a/whats_new_in_rust/").unwrap();
        let u2 =
            Url::parse("https://old.reddit.com/r/MachineLearning/comments/xyz123/paper_summary/")
                .unwrap();
        let u3 = Url::parse("https://redd.it/1i8z4a").unwrap();
        let u4 = Url::parse("https://evilreddit.com/r/rust/comments/123/fake").unwrap();
        let u5 = Url::parse("https://www.reddit.com/r/rust/").unwrap();

        assert!(adapter.matches(&u1));
        assert!(adapter.matches(&u2));
        assert!(adapter.matches(&u3));
        assert!(!adapter.matches(&u4));
        assert!(!adapter.matches(&u5));
    }

    #[test]
    fn test_reddit_canonicalize_json_url() {
        let u =
            Url::parse("https://www.reddit.com/r/rust/comments/1i8z4a/whats_new_in_rust/").unwrap();
        let json_url = RedditAdapter::canonicalize_json_url(&u).unwrap();
        assert!(json_url.path().ends_with("/.json"));
        assert!(json_url.query().unwrap().contains("raw_json=1"));

        let short_u = Url::parse("https://redd.it/1i8z4a").unwrap();
        let short_json = RedditAdapter::canonicalize_json_url(&short_u).unwrap();
        assert!(short_json
            .as_str()
            .contains("reddit.com/comments/1i8z4a/.json"));
    }

    #[test]
    fn test_reddit_parse_json_payload() {
        let adapter = RedditAdapter::new();
        let payload = serde_json::json!([
            {
                "data": {
                    "children": [
                        {
                            "data": {
                                "title": "Announcing PageClaw 1.0",
                                "author": "rust_dev",
                                "subreddit_name_prefixed": "r/rust",
                                "score": 256,
                                "upvote_ratio": 0.98,
                                "selftext": "PageClaw is an ultra-fast web content extractor built in Rust."
                            }
                        }
                    ]
                }
            },
            {
                "data": {
                    "children": [
                        {
                            "data": {
                                "author": "tokio_fan",
                                "score": 42,
                                "body": "Great project! Does it support async scraping?",
                                "replies": ""
                            }
                        }
                    ]
                }
            }
        ]);

        let original_url =
            Url::parse("https://www.reddit.com/r/rust/comments/123/pageclaw/").unwrap();
        let doc = adapter
            .parse_json_payload(&payload, &original_url, &ExtractionOptions::default())
            .unwrap();

        assert_eq!(doc.title.as_deref(), Some("Announcing PageClaw 1.0"));
        assert!(doc.content.contains("r/rust"));
        assert!(doc.content.contains("u/rust_dev"));
        assert!(doc
            .content
            .contains("PageClaw is an ultra-fast web content extractor built in Rust."));
        assert!(doc.content.contains("u/tokio_fan"));
        assert!(doc
            .content
            .contains("Great project! Does it support async scraping?"));
    }
}
