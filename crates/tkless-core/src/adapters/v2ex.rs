//! V2EX Chinese developer community topic and reply adapter.
//!
//! Intercepts `v2ex.com/t/...` and `www.v2ex.com/t/...` URLs, extracting topic title,
//! author, node/category, post body, and community replies with floor numbers.

use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::TklessError;
use crate::markdown::MarkdownConverter;
use crate::model::{ExtractionOptions, OutputFormat, PageDocument, PageMetadata};
use crate::token::count_tokens;
use scraper::{Html, Selector};
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static V2EX_TITLE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".header h1, h1, meta[property='og:title']").unwrap());

static V2EX_NODE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".header a[href^='/go/']").unwrap());

static V2EX_AUTHOR_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".header small.gray a, .header .gray a, meta[name='author']").unwrap()
});

static V2EX_TIME_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".header small.gray span, .header small.gray").unwrap());

static V2EX_CONTENT_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".topic_content, .markdown_body, .cell .topic_content").unwrap()
});

static V2EX_REPLY_CELL_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".cell[id^='r_']").unwrap());

static V2EX_REPLY_AUTHOR_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a.dark, .dark").unwrap());

static V2EX_REPLY_FLOOR_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".no, span.no").unwrap());

static V2EX_REPLY_CONTENT_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".reply_content").unwrap());

pub struct V2exAdapter;

impl Default for V2exAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl V2exAdapter {
    pub fn new() -> Self {
        Self
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        debug!(url = %url, "Executing V2EX adapter extraction");

        let resp = fetcher
            .get(url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 TkLess/0.1",
            )
            .header("Referer", "https://v2ex.com/")
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8",
            )
            .send()
            .await?;

        let html = fetcher.read_text(resp).await?;
        self.parse_page(&html, url, options)
    }

    pub fn parse_page(
        &self,
        raw_html: &str,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        let doc = Html::parse_document(raw_html);

        // 1. Extract Title
        let title = doc
            .select(&V2EX_TITLE_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_else(|| "V2EX 话题讨论".to_string());

        // 2. Extract Node / Category
        let node_name = doc
            .select(&V2EX_NODE_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_else(|| "节点".to_string());

        // 3. Extract Author
        let author = doc
            .select(&V2EX_AUTHOR_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_else(|| "V2EX 会员".to_string());

        // 4. Extract Post Time
        let post_time = doc
            .select(&V2EX_TIME_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string());

        // 5. Extract Topic Content
        let converter = MarkdownConverter::new();
        let body_html = doc
            .select(&V2EX_CONTENT_SELECTOR)
            .next()
            .map(|el| el.html())
            .unwrap_or_default();
        let body_md = converter.convert(&body_html, options.strip_images, options.strip_links)?;

        // 6. Extract Replies
        let mut replies = Vec::new();
        let mut replies_json = Vec::new();

        for cell in doc.select(&V2EX_REPLY_CELL_SELECTOR).take(20) {
            let r_author = cell
                .select(&V2EX_REPLY_AUTHOR_SELECTOR)
                .next()
                .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
                .unwrap_or_else(|| "匿名".to_string());

            let r_floor = cell
                .select(&V2EX_REPLY_FLOOR_SELECTOR)
                .next()
                .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
                .unwrap_or_default();

            let r_content_html = cell
                .select(&V2EX_REPLY_CONTENT_SELECTOR)
                .next()
                .map(|el| el.html())
                .unwrap_or_default();

            if r_content_html.trim().is_empty() {
                continue;
            }

            let r_content_md =
                converter.convert(&r_content_html, options.strip_images, options.strip_links)?;

            replies_json.push(serde_json::json!({
                "floor": r_floor,
                "author": r_author,
                "content": r_content_md
            }));

            let floor_prefix = if r_floor.is_empty() {
                String::new()
            } else {
                format!("#{r_floor} ")
            };
            replies.push(format!(
                "> **{floor_prefix}@{r_author}**:\n> {}\n",
                r_content_md.trim().replace('\n', "\n> ")
            ));
        }

        // 7. Format Output
        let content = match options.format {
            OutputFormat::Json => {
                let json_obj = serde_json::json!({
                    "title": title,
                    "url": url.as_str(),
                    "node": node_name,
                    "author": author,
                    "time": post_time,
                    "content": body_md,
                    "replies": replies_json
                });
                serde_json::to_string_pretty(&json_obj).unwrap_or_default()
            }
            _ => {
                let mut md = String::new();
                if options.frontmatter {
                    md.push_str("---\n");
                    md.push_str(&format!("title: \"{}\"\n", title.replace('"', "\\\"")));
                    md.push_str(&format!("url: \"{url}\"\n"));
                    md.push_str(&format!("author: \"{author}\"\n"));
                    md.push_str(&format!("node: \"{node_name}\"\n"));
                    if let Some(ref t) = post_time {
                        md.push_str(&format!("time: \"{}\"\n", t.replace('"', "\\\"")));
                    }
                    md.push_str("---\n\n");
                }

                md.push_str(&format!("# {title}\n\n"));
                let mut meta_parts = vec![
                    format!("**节点**: `{node_name}`"),
                    format!("**楼主**: @{author}"),
                ];
                if let Some(ref t) = post_time {
                    if !t.is_empty() {
                        meta_parts.push(format!("**发布时间**: {t}"));
                    }
                }
                md.push_str(&format!("> {}\n\n", meta_parts.join(" | ")));

                if !body_md.trim().is_empty() {
                    md.push_str(body_md.trim());
                    md.push_str("\n\n");
                }

                if !replies.is_empty() {
                    md.push_str("---\n\n### 社区精选回帖 (Replies):\n\n");
                    for rep in replies {
                        md.push_str(&rep);
                        md.push('\n');
                    }
                }

                md
            }
        };

        let token_count = count_tokens(&content, options.tokenizer);

        Ok(PageDocument {
            extraction: Default::default(),
            warnings: Vec::new(),
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: Some(title),
            byline: Some(format!("@{author} in {node_name}")),
            content,
            metadata: PageMetadata {
                author: Some(author),
                canonical_url: Some(url.to_string()),
                keywords: vec![node_name],
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

impl PlatformAdapter for V2exAdapter {
    fn name(&self) -> &'static str {
        "v2ex"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        crate::fetcher::is_subdomain_or_exact(host, "v2ex.com") && url.path().starts_with("/t/")
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
    fn test_v2ex_adapter_matching() {
        let adapter = V2exAdapter::new();
        let u1 = Url::parse("https://v2ex.com/t/1098765").unwrap();
        let u2 = Url::parse("https://www.v2ex.com/t/12345#reply20").unwrap();
        let u3 = Url::parse("https://evilv2ex.com/t/123").unwrap();
        let u4 = Url::parse("https://v2ex.com/go/rust").unwrap();

        assert!(adapter.matches(&u1));
        assert!(adapter.matches(&u2));
        assert!(!adapter.matches(&u3));
        assert!(!adapter.matches(&u4));
    }

    #[test]
    fn test_v2ex_parse_page() {
        let adapter = V2exAdapter::new();
        let sample_html = r##"
            <!DOCTYPE html>
            <html>
            <head><title>如何评价 Rust 2026 年的发展态势？ - V2EX</title></head>
            <body>
                <div class="header">
                    <a href="/go/rust">Rust</a>
                    <h1>如何评价 Rust 2026 年的发展态势？</h1>
                    <small class="gray">
                        <a href="/member/dev_user">dev_user</a> · <span>2 小时前</span>
                    </small>
                </div>
                <div class="topic_content">
                    <p>Rust 在高并发服务器和 Linux 内核中的应用越来越广泛。</p>
                    <pre><code>async fn run() {}</code></pre>
                </div>
                <div id="Main">
                    <div class="cell" id="r_1001">
                        <table cellpadding="0" cellspacing="0" border="0" width="100%">
                            <tr>
                                <td width="48" valign="top" align="center"></td>
                                <td width="10" valign="top"></td>
                                <td width="auto" valign="top">
                                    <div class="fr"><span class="no">1</span></div>
                                    <div class="sep3"></div>
                                    <strong><a href="/member/expert" class="dark">expert</a></strong>
                                    <div class="sep5"></div>
                                    <div class="reply_content">完全同意，生态系统已经极其成熟！</div>
                                </td>
                            </tr>
                        </table>
                    </div>
                </div>
            </body>
            </html>
        "##;

        let url = Url::parse("https://v2ex.com/t/1098765").unwrap();
        let doc = adapter
            .parse_page(sample_html, &url, &ExtractionOptions::default())
            .unwrap();

        assert_eq!(
            doc.title.as_deref(),
            Some("如何评价 Rust 2026 年的发展态势？")
        );
        assert!(doc.content.contains("dev_user"));
        assert!(doc.content.contains("Rust"));
        assert!(doc.content.contains("高并发服务器"));
        assert!(doc.content.contains("expert"));
        assert!(doc.content.contains("生态系统已经极其成熟"));
    }
}
