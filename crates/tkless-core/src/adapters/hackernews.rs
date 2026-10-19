//! Hacker News story, rankings, and discussion comments tree adapter.
//!
//! Intercepts `news.ycombinator.com` URLs (frontpage listings, /best, /newest, /ask, /show, and /item?id=...),
//! extracting story title, points, external source link, submission author, and hierarchical discussions.

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

static HN_ATHING_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("tr.athing").unwrap());
static HN_RANK_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("span.rank").unwrap());
static HN_TITLE_LINK_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("span.titleline > a, .titleline a").unwrap());
static HN_SITEBIT_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("span.sitebit, span.sitestr").unwrap());
static HN_TOPTEXT_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("div.toptext, tr.toptext td").unwrap());
static HN_COMMENT_TR_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("tr.comtr").unwrap());
static HN_INDENT_IMG_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("td.ind img").unwrap());
static HN_USER_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a.hnuser").unwrap());
static HN_COMMTEXT_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("div.commtext").unwrap());

/// A single item in the Hacker News frontpage or topic rankings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HnStoryEntry {
    pub rank: String,
    pub title: String,
    pub url: String,
    pub domain: String,
    pub score: String,
    pub author: String,
}

pub struct HackerNewsAdapter;

impl Default for HackerNewsAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl HackerNewsAdapter {
    pub fn new() -> Self {
        Self
    }

    /// Checks if a URL represents a specific discussion item thread (`item?id=...`).
    pub fn extract_item_id(url: &Url) -> Option<u64> {
        if url.path().trim_end_matches('/').ends_with("item") {
            for (k, v) in url.query_pairs() {
                if k == "id" {
                    if let Ok(id_num) = v.parse::<u64>() {
                        return Some(id_num);
                    }
                }
            }
        }
        None
    }

    /// Parses frontpage or topic rankings HTML into structured Markdown table.
    pub fn parse_stories_table(html: &str) -> (String, Vec<HnStoryEntry>) {
        let doc = Html::parse_document(html);
        let mut stories = Vec::new();

        for athing in doc.select(&HN_ATHING_SELECTOR) {
            let rank = athing
                .select(&HN_RANK_SELECTOR)
                .next()
                .map(|e| {
                    e.text()
                        .collect::<String>()
                        .trim()
                        .trim_end_matches('.')
                        .to_string()
                })
                .unwrap_or_default();

            let title_el = athing.select(&HN_TITLE_LINK_SELECTOR).next();
            let title = title_el
                .map(|e| e.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            let link = title_el
                .and_then(|e| e.value().attr("href"))
                .unwrap_or("")
                .trim()
                .to_string();

            let site = athing
                .select(&HN_SITEBIT_SELECTOR)
                .next()
                .map(|e| e.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            // Next sibling row contains score, author, comments
            let mut score = String::new();
            let mut author = String::new();

            let mut curr = athing.next_sibling();
            while let Some(node) = curr {
                if let Some(el) = scraper::ElementRef::wrap(node) {
                    if el.value().name() == "tr" {
                        let subtext = el.text().collect::<String>();
                        for part in subtext.split('|').map(|s| s.trim()) {
                            if part.contains("point") {
                                score = part.to_string();
                            }
                        }
                        if let Some(user_el) = el.select(&HN_USER_SELECTOR).next() {
                            author = user_el.text().collect::<String>().trim().to_string();
                        }
                        break;
                    }
                }
                curr = node.next_sibling();
            }

            if !title.is_empty() {
                stories.push(HnStoryEntry {
                    rank,
                    title,
                    url: link,
                    domain: site,
                    score,
                    author,
                });
            }
        }

        let mut out = String::new();
        out.push_str("# 🚀 Hacker News Top Stories\n\n");
        out.push_str("| 排名 | 标题 | 来源域名 | 得分 | 发布者 |\n");
        out.push_str("|:---|:---|:---|:---|:---|\n");
        for s in &stories {
            let title_cell = if s.url.starts_with("http") {
                format!("[{}]({})", s.title.replace('|', "\\|"), s.url)
            } else {
                format!(
                    "[{}](https://news.ycombinator.com/{})",
                    s.title.replace('|', "\\|"),
                    s.url
                )
            };
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                s.rank,
                title_cell,
                if s.domain.is_empty() { "-" } else { &s.domain },
                if s.score.is_empty() { "-" } else { &s.score },
                if s.author.is_empty() { "-" } else { &s.author }
            ));
        }

        (out, stories)
    }

    /// Parses an item discussion thread HTML page.
    pub fn parse_item_discussion(html: &str, item_id: u64) -> String {
        let doc = Html::parse_document(html);
        let mut out = String::new();

        // 1. Story title and link
        let title = doc
            .select(&HN_TITLE_LINK_SELECTOR)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_else(|| format!("Hacker News Item #{item_id}"));

        let link = doc
            .select(&HN_TITLE_LINK_SELECTOR)
            .next()
            .and_then(|e| e.value().attr("href"))
            .unwrap_or("")
            .trim();

        let author = doc
            .select(&HN_USER_SELECTOR)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        let toptext_html = doc
            .select(&HN_TOPTEXT_SELECTOR)
            .next()
            .map(|e| e.inner_html())
            .unwrap_or_default();

        out.push_str(&format!("# 📰 {title}\n\n"));
        if !link.is_empty() && link.starts_with("http") {
            out.push_str(&format!("> 🔗 **直达原文**: [{link}]({link})\n"));
        }
        out.push_str(&format!(
            "> 💬 **HN 讨论链接**: [https://news.ycombinator.com/item?id={item_id}](https://news.ycombinator.com/item?id={item_id}){}\n\n",
            if author.is_empty() { "".to_string() } else { format!(" | 👤 **作者**: `{author}`") }
        ));

        if !toptext_html.is_empty() {
            let converter = MarkdownConverter::new();
            if let Ok(md_body) = converter.convert(&toptext_html, false, false) {
                out.push_str("### 📝 主楼说明 / 正文\n\n");
                out.push_str(&md_body);
                out.push_str("\n\n---\n\n");
            }
        }

        // 2. Comments Tree
        out.push_str("### 💬 社区精选讨论 (Discussion Tree)\n\n");

        let mut comment_count = 0;
        let converter = MarkdownConverter::new();

        for tr in doc.select(&HN_COMMENT_TR_SELECTOR) {
            comment_count += 1;
            let indent_width: u32 = tr
                .select(&HN_INDENT_IMG_SELECTOR)
                .next()
                .and_then(|img| img.value().attr("width"))
                .and_then(|w| w.parse().ok())
                .unwrap_or(0);

            let indent_level = (indent_width / 40).min(5);
            let quote_prefix = "> ".repeat(indent_level as usize);

            let comment_author = tr
                .select(&HN_USER_SELECTOR)
                .next()
                .map(|e| e.text().collect::<String>().trim().to_string())
                .unwrap_or_else(|| "anonymous".to_string());

            let comment_html = tr
                .select(&HN_COMMTEXT_SELECTOR)
                .next()
                .map(|e| e.inner_html())
                .unwrap_or_default();

            if comment_html.is_empty() {
                continue;
            }

            let comment_md = converter
                .convert(&comment_html, false, false)
                .unwrap_or_else(|_| comment_html.clone());

            out.push_str(&format!("{quote_prefix}👤 **`{comment_author}`**:\n"));
            for line in comment_md.lines() {
                out.push_str(&format!("{quote_prefix}{line}\n"));
            }
            out.push_str(&format!("{quote_prefix}\n"));
        }

        if comment_count == 0 {
            out.push_str("_暂无公开讨论回复。_\n\n");
        }

        out
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        debug!(url = %url, "Executing Hacker News adapter extraction");

        let item_id_opt = Self::extract_item_id(url);

        // 1. If it is an item discussion, try Firebase REST API first for ultra-fast, structured retrieval
        if let Some(item_id) = item_id_opt {
            let api_url_str = format!("https://hacker-news.firebaseio.com/v0/item/{item_id}.json");
            if let Ok(parsed_api_url) = Url::parse(&api_url_str) {
                if let Ok(resp) = fetcher.get(&parsed_api_url).send().await {
                    if let Ok(val) = resp.json::<Value>().await {
                        if let Some(title) = val.get("title").and_then(|t| t.as_str()) {
                            let by = val.get("by").and_then(|b| b.as_str()).unwrap_or("unknown");
                            let score = val.get("score").and_then(|s| s.as_u64()).unwrap_or(0);
                            let target_url = val.get("url").and_then(|u| u.as_str()).unwrap_or("");
                            let text_html = val.get("text").and_then(|t| t.as_str()).unwrap_or("");

                            let mut md = String::new();
                            md.push_str(&format!("# 📰 {title}\n\n"));
                            if !target_url.is_empty() {
                                md.push_str(&format!(
                                    "> 🔗 **直达原文**: [{target_url}]({target_url})\n"
                                ));
                            }
                            md.push_str(&format!(
                                "> 💬 **HN 讨论**: [https://news.ycombinator.com/item?id={item_id}](https://news.ycombinator.com/item?id={item_id}) | ⭐ **得分**: {score} | 👤 **作者**: `{by}`\n\n"
                            ));

                            if !text_html.is_empty() {
                                let converter = MarkdownConverter::new();
                                if let Ok(clean_body) = converter.convert(text_html, false, false) {
                                    md.push_str("### 📝 主楼说明 / 正文\n\n");
                                    md.push_str(&clean_body);
                                    md.push_str("\n\n---\n\n");
                                }
                            }

                            // Concurrently fetch top 5 comments
                            if let Some(kids) = val.get("kids").and_then(|k| k.as_array()) {
                                md.push_str("### 💬 社区热门讨论 (Top Comments)\n\n");
                                let mut comment_set = tokio::task::JoinSet::new();
                                for kid_id in kids.iter().take(5).filter_map(|k| k.as_u64()) {
                                    let f = fetcher.clone();
                                    comment_set.spawn(async move {
                                        let kid_url_str = format!("https://hacker-news.firebaseio.com/v0/item/{kid_id}.json");
                                        if let Ok(parsed_k) = Url::parse(&kid_url_str) {
                                            if let Ok(k_resp) = f.get(&parsed_k).send().await {
                                                return k_resp.json::<Value>().await.ok();
                                            }
                                        }
                                        None
                                    });
                                }

                                let converter = MarkdownConverter::new();
                                while let Some(res) = comment_set.join_next().await {
                                    if let Ok(Some(c_val)) = res {
                                        let c_by = c_val
                                            .get("by")
                                            .and_then(|b| b.as_str())
                                            .unwrap_or("anon");
                                        let c_text = c_val
                                            .get("text")
                                            .and_then(|t| t.as_str())
                                            .unwrap_or("");
                                        if !c_text.is_empty() {
                                            let c_md = converter
                                                .convert(c_text, false, false)
                                                .unwrap_or_else(|_| c_text.to_string());
                                            md.push_str(&format!("- 👤 **`{c_by}`**:\n"));
                                            for line in c_md.lines() {
                                                md.push_str(&format!("  {line}\n"));
                                            }
                                            md.push('\n');
                                        }
                                    }
                                }
                            }

                            let token_count = count_tokens(&md, options.tokenizer);
                            return Ok(PageDocument {
                                url: format!("https://news.ycombinator.com/item?id={item_id}"),
                                final_url: format!(
                                    "https://news.ycombinator.com/item?id={item_id}"
                                ),
                                status: 200,
                                title: Some(title.to_string()),
                                byline: Some(by.to_string()),
                                content: md,
                                metadata: PageMetadata {
                                    author: Some(by.to_string()),
                                    canonical_url: Some(format!(
                                        "https://news.ycombinator.com/item?id={item_id}"
                                    )),
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
        }

        // 2. HTML Fallback
        let resp = fetcher.get(url).send().await?;
        let html = resp.text().await?;

        let (content, page_title) = if let Some(item_id) = item_id_opt {
            let md = Self::parse_item_discussion(&html, item_id);
            (md, format!("Hacker News Item #{item_id}"))
        } else {
            let (md, _) = Self::parse_stories_table(&html);
            (md, "Hacker News Top Stories".to_string())
        };

        let token_count = count_tokens(&content, options.tokenizer);

        Ok(PageDocument {
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: Some(page_title),
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

impl PlatformAdapter for HackerNewsAdapter {
    fn name(&self) -> &'static str {
        "hackernews"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("").to_lowercase();
        crate::fetcher::is_subdomain_or_exact(&host, "news.ycombinator.com")
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
    fn test_hn_extract_item_id() {
        let u1 = Url::parse("https://news.ycombinator.com/item?id=39123456").unwrap();
        assert_eq!(HackerNewsAdapter::extract_item_id(&u1), Some(39123456));

        let u2 = Url::parse("https://news.ycombinator.com/news").unwrap();
        assert_eq!(HackerNewsAdapter::extract_item_id(&u2), None);
    }

    #[test]
    fn test_hn_parse_stories_table() {
        let html = r#"
        <table>
            <tr class="athing" id="101">
                <td class="title"><span class="rank">1.</span></td>
                <td class="title"><span class="titleline"><a href="https://example.com/rust">Show HN: TkLess in Rust</a><span class="sitebit"> (example.com)</span></span></td>
            </tr>
            <tr>
                <td class="subtext">
                    <span class="score">150 points</span> by <a class="hnuser">author1</a> | 45 comments
                </td>
            </tr>
        </table>
        "#;

        let (md, stories) = HackerNewsAdapter::parse_stories_table(html);
        assert_eq!(stories.len(), 1);
        assert_eq!(stories[0].rank, "1");
        assert_eq!(stories[0].title, "Show HN: TkLess in Rust");
        assert!(md.contains("TkLess in Rust"));
        assert!(md.contains("150 points"));
    }

    #[test]
    fn test_hn_parse_discussion() {
        let html = r#"
        <span class="titleline"><a href="https://example.com">Rust 2026 Edition</a></span>
        <a class="hnuser">ferris</a>
        <div class="toptext">This is the description text.</div>
        <table>
            <tr class="comtr" id="1">
                <td><table class="fatitem"><tr><td class="ind"><img width="0" height="1"></td><td><a class="hnuser">commenter1</a><div class="commtext">Great release!</div></td></tr></table></td>
            </tr>
            <tr class="comtr" id="2">
                <td><table class="fatitem"><tr><td class="ind"><img width="40" height="1"></td><td><a class="hnuser">commenter2</a><div class="commtext">Nested reply here.</div></td></tr></table></td>
            </tr>
        </table>
        "#;

        let md = HackerNewsAdapter::parse_item_discussion(html, 100);
        assert!(md.contains("Rust 2026 Edition"));
        assert!(md.contains("ferris"));
        assert!(md.contains("Great release!"));
        assert!(md.contains("Nested reply here."));
    }
}
