//! RSS 2.0 and Atom 1.0 XML feed parser and Markdown formatter.
//!
//! Enables zero-overhead parsing of RSS/Atom syndication feeds from blogs, newsletters,
//! and release streams into structured JSON or clean Markdown reading lists.

use crate::error::PageclawError;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

static RSS_CHANNEL_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("channel").unwrap());
static RSS_ITEM_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse("item").unwrap());
static ATOM_FEED_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse("feed").unwrap());
static ATOM_ENTRY_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("entry").unwrap());

static TITLE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse("title").unwrap());
static LINK_SELECTOR: LazyLock<Selector> = LazyLock::new(|| Selector::parse("link").unwrap());
static DESC_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("description, summary").unwrap());
static AUTHOR_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("author, dc\\:creator, creator").unwrap());
static DATE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("pubdate, updated, published, dc\\:date").unwrap());
static RSS_LINK_TAG_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"(?is)<link[^>]*>(.*?)(?:</link>|$|\n|<)"#).unwrap());

/// Structured syndication feed channel metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedChannel {
    pub title: String,
    pub link: String,
    pub description: String,
    pub feed_type: String,
    pub items: Vec<FeedItem>,
}

/// An individual article entry or post within an RSS/Atom feed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedItem {
    pub title: String,
    pub link: String,
    pub published: Option<String>,
    pub author: Option<String>,
    pub summary: Option<String>,
}

/// Detects whether raw string content represents an RSS or Atom XML syndication feed.
pub fn is_feed_xml(content: &str) -> bool {
    let lower = content.trim_start().to_ascii_lowercase();
    (lower.starts_with("<?xml") || lower.starts_with("<rss") || lower.starts_with("<feed"))
        && (lower.contains("<rss") || lower.contains("<channel>") || lower.contains("<feed"))
}

/// Parses an RSS 2.0 or Atom 1.0 XML string into a structured FeedChannel.
pub fn parse_feed(xml: &str) -> Result<FeedChannel, PageclawError> {
    let doc = Html::parse_document(xml);

    // 1. Try Atom Feed
    if let Some(feed_el) = doc.select(&ATOM_FEED_SELECTOR).next() {
        let title = feed_el
            .select(&TITLE_SELECTOR)
            .next()
            .map(|e| e.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_else(|| "Atom Feed".to_string());

        let link = feed_el
            .select(&LINK_SELECTOR)
            .next()
            .and_then(|e| e.value().attr("href").map(|s| s.trim().to_string()))
            .unwrap_or_default();

        let description = feed_el
            .select(&Selector::parse("subtitle").unwrap())
            .next()
            .map(|e| e.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_default();

        let mut items = Vec::new();
        for entry_el in doc.select(&ATOM_ENTRY_SELECTOR) {
            let item_title = entry_el
                .select(&TITLE_SELECTOR)
                .next()
                .map(|e| e.text().collect::<Vec<_>>().join(" ").trim().to_string())
                .unwrap_or_else(|| "Untitled Entry".to_string());

            let item_link = entry_el
                .select(&LINK_SELECTOR)
                .next()
                .and_then(|e| e.value().attr("href").map(|s| s.trim().to_string()))
                .unwrap_or_default();

            let item_date = entry_el
                .select(&DATE_SELECTOR)
                .next()
                .map(|e| e.text().collect::<Vec<_>>().join(" ").trim().to_string());

            let item_author = entry_el
                .select(&AUTHOR_SELECTOR)
                .next()
                .map(|e| e.text().collect::<Vec<_>>().join(" ").trim().to_string());

            let item_summary = entry_el
                .select(&DESC_SELECTOR)
                .next()
                .map(|e| e.text().collect::<Vec<_>>().join(" ").trim().to_string());

            items.push(FeedItem {
                title: item_title,
                link: item_link,
                published: item_date,
                author: item_author,
                summary: item_summary,
            });
        }

        return Ok(FeedChannel {
            title,
            link,
            description,
            feed_type: "atom".to_string(),
            items,
        });
    }

    // 2. Try RSS 2.0 Channel
    if let Some(channel_el) = doc.select(&RSS_CHANNEL_SELECTOR).next() {
        let title = channel_el
            .select(&TITLE_SELECTOR)
            .next()
            .map(|e| e.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_else(|| "RSS Feed".to_string());

        let link = RSS_LINK_TAG_RE
            .captures(&channel_el.html())
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                channel_el
                    .select(&LINK_SELECTOR)
                    .next()
                    .and_then(|e| e.value().attr("href").map(|s| s.trim().to_string()))
            })
            .unwrap_or_default();

        let description = channel_el
            .select(&DESC_SELECTOR)
            .next()
            .map(|e| e.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_default();

        let mut items = Vec::new();
        for item_el in doc.select(&RSS_ITEM_SELECTOR) {
            let item_title = item_el
                .select(&TITLE_SELECTOR)
                .next()
                .map(|e| e.text().collect::<Vec<_>>().join(" ").trim().to_string())
                .unwrap_or_else(|| "Untitled Post".to_string());

            let item_link = RSS_LINK_TAG_RE
                .captures(&item_el.html())
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    item_el
                        .select(&LINK_SELECTOR)
                        .next()
                        .and_then(|e| e.value().attr("href").map(|s| s.trim().to_string()))
                })
                .unwrap_or_default();

            let item_date = item_el
                .select(&DATE_SELECTOR)
                .next()
                .map(|e| e.text().collect::<Vec<_>>().join(" ").trim().to_string());

            let item_author = item_el
                .select(&AUTHOR_SELECTOR)
                .next()
                .map(|e| e.text().collect::<Vec<_>>().join(" ").trim().to_string());

            let item_summary = item_el
                .select(&DESC_SELECTOR)
                .next()
                .map(|e| e.text().collect::<Vec<_>>().join(" ").trim().to_string());

            items.push(FeedItem {
                title: item_title,
                link: item_link,
                published: item_date,
                author: item_author,
                summary: item_summary,
            });
        }

        return Ok(FeedChannel {
            title,
            link,
            description,
            feed_type: "rss".to_string(),
            items,
        });
    }

    Err(PageclawError::CleaningError(
        "Invalid or unsupported XML feed format (expected RSS 2.0 or Atom 1.0)".to_string(),
    ))
}

/// Formats a FeedChannel into an agent-friendly GFM Markdown document.
pub fn format_feed_markdown(channel: &FeedChannel) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "# 📰 订阅源聚合 ({}): {}\n\n",
        channel.feed_type.to_uppercase(),
        channel.title
    ));
    if !channel.description.is_empty() {
        out.push_str(&format!("> **简介**: {}\n", channel.description));
    }
    if !channel.link.is_empty() {
        out.push_str(&format!("> **站点链接**: {}\n", channel.link));
    }
    out.push_str(&format!("> **文章总数**: {}\n\n", channel.items.len()));

    out.push_str("## 📑 文章阅读列表\n\n");
    for (idx, item) in channel.items.iter().enumerate() {
        out.push_str(&format!(
            "### {}. [{}]({})\n\n",
            idx + 1,
            item.title,
            item.link
        ));

        let mut meta_parts = Vec::new();
        if let Some(ref author) = item.author {
            if !author.is_empty() {
                meta_parts.push(format!("**作者**: {author}"));
            }
        }
        if let Some(ref date) = item.published {
            if !date.is_empty() {
                meta_parts.push(format!("**发布时间**: {date}"));
            }
        }
        if !meta_parts.is_empty() {
            out.push_str(&format!("> {}\n\n", meta_parts.join(" | ")));
        }

        if let Some(ref summary) = item.summary {
            if !summary.is_empty() {
                out.push_str(summary.trim());
                out.push_str("\n\n");
            }
        }

        out.push_str("---\n\n");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rss_feed() {
        let sample = r#"<?xml version="1.0" encoding="UTF-8"?>
            <rss version="2.0">
                <channel>
                    <title>Rust Blog</title>
                    <link>https://blog.rust-lang.org</link>
                    <description>Empowering everyone to build reliable software.</description>
                    <item>
                        <title>Announcing Rust 1.85</title>
                        <link>https://blog.rust-lang.org/2026/02/20/Rust-1.85.0.html</link>
                        <pubDate>Thu, 20 Feb 2026 10:00:00 +0000</pubDate>
                        <creator>The Rust Core Team</creator>
                        <description>The Rust team is excited to announce a new version.</description>
                    </item>
                </channel>
            </rss>
        "#;

        assert!(is_feed_xml(sample));
        let channel = parse_feed(sample).unwrap();
        assert_eq!(channel.title, "Rust Blog");
        assert_eq!(channel.feed_type, "rss");
        assert_eq!(channel.items.len(), 1);
        assert_eq!(channel.items[0].title, "Announcing Rust 1.85");
        assert_eq!(
            channel.items[0].author.as_deref(),
            Some("The Rust Core Team")
        );
    }

    #[test]
    fn test_parse_atom_feed() {
        let sample = r#"<?xml version="1.0" encoding="utf-8"?>
            <feed xmlns="http://www.w3.org/2005/Atom">
                <title>Example Atom Feed</title>
                <subtitle>A sample atom feed</subtitle>
                <link href="https://example.com/"/>
                <entry>
                    <title>Atom Entry Title</title>
                    <link href="https://example.com/entry/1"/>
                    <updated>2026-10-15T18:30:02Z</updated>
                    <author><name>John Doe</name></author>
                    <summary>A brief summary of atom entry.</summary>
                </entry>
            </feed>
        "#;

        assert!(is_feed_xml(sample));
        let channel = parse_feed(sample).unwrap();
        assert_eq!(channel.title, "Example Atom Feed");
        assert_eq!(channel.feed_type, "atom");
        assert_eq!(channel.items.len(), 1);
        assert_eq!(channel.items[0].title, "Atom Entry Title");
        assert_eq!(channel.items[0].link, "https://example.com/entry/1");
    }
}
