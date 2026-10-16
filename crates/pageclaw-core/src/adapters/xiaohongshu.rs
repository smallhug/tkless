use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::PageclawError;
use crate::model::{ExtractionOptions, OutputFormat, PageDocument, PageMetadata};
use crate::token::count_tokens;
use scraper::{Html, Selector};
use serde_json::Value;
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static XHS_TITLE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("#detail-title, .title, h1.note-title").unwrap());
static XHS_DESC_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("#detail-desc, .desc, .note-text, .content").unwrap());
static XHS_AUTHOR_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".name, .author-name, .username, .user-name, a.name span").unwrap()
});

pub struct XiaohongshuAdapter;

impl Default for XiaohongshuAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl XiaohongshuAdapter {
    pub fn new() -> Self {
        Self
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        debug!(url = %url, "Executing Xiaohongshu adapter extraction");

        let resp = fetcher
            .get(url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
            )
            .header("Referer", "https://www.xiaohongshu.com/")
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8")
            .send()
            .await?;

        let html = resp.text().await?;
        self.parse_note(&html, url, options)
    }

    pub fn parse_note(
        &self,
        raw_html: &str,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        let doc = Html::parse_document(raw_html);

        // 1. Try parsing window.__INITIAL_STATE__ state tree
        if let Some(state) = extract_initial_state(raw_html) {
            if let Some(note_detail) = extract_note_from_state(&state) {
                return self.build_note_doc(
                    &note_detail.title,
                    &note_detail.author,
                    &note_detail.content,
                    note_detail.likes,
                    note_detail.collects,
                    note_detail.image_urls,
                    url,
                    options,
                );
            }
        }

        // 2. Fallback: DOM parsing
        let title = doc
            .select(&XHS_TITLE_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .or_else(|| extract_meta_property(&doc, "og:title"))
            .unwrap_or_else(|| "小红书笔记".to_string());

        let author = doc
            .select(&XHS_AUTHOR_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .or_else(|| extract_meta_property(&doc, "author"))
            .unwrap_or_else(|| "小红书用户".to_string());

        let content = doc
            .select(&XHS_DESC_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join("\n").trim().to_string())
            .or_else(|| extract_meta_property(&doc, "og:description"))
            .unwrap_or_default();

        let mut image_urls = Vec::new();
        let img_sel =
            Selector::parse(".swiper-slide img, .note-slider img, .media-container img").unwrap();
        for img in doc.select(&img_sel) {
            if let Some(src) = img.value().attr("src") {
                if src.starts_with("http") && !src.contains("avatar") {
                    image_urls.push(src.to_string());
                }
            }
        }

        self.build_note_doc(&title, &author, &content, 0, 0, image_urls, url, options)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_note_doc(
        &self,
        title: &str,
        author: &str,
        content_text: &str,
        likes: u64,
        collects: u64,
        images: Vec<String>,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, PageclawError> {
        let content = match options.format {
            OutputFormat::Html => {
                let mut out = format!("<h1>{title}</h1>\n<p>作者: {author} | 点赞: {likes} | 收藏: {collects}</p>\n<p>{content_text}</p>\n");
                for img in &images {
                    out.push_str(&format!("<img src=\"{img}\" />\n"));
                }
                out
            }
            OutputFormat::Raw => format!("{title}\n\n{author}\n\n{content_text}"),
            OutputFormat::Text => {
                format!(
                    "{title}\n作者: {author} | 点赞: {likes} | 收藏: {collects}\n\n{content_text}"
                )
            }
            OutputFormat::Markdown | OutputFormat::Json => {
                let mut md = String::new();
                if options.frontmatter && options.format == OutputFormat::Markdown {
                    md.push_str("---\n");
                    md.push_str(&format!("title: \"{}\"\n", title.replace('"', "\\\"")));
                    md.push_str(&format!("author: \"{}\"\n", author.replace('"', "\\\"")));
                    md.push_str(&format!("url: \"{url}\"\n"));
                    md.push_str(&format!("likes: {likes}\n"));
                    md.push_str(&format!("collects: {collects}\n"));
                    md.push_str("---\n\n");
                }

                md.push_str(&format!("# {}\n\n", title));
                md.push_str(&format!(
                    "> **作者**: {} | **获赞**: {} | **收藏**: {}\n\n",
                    author,
                    format_count(likes),
                    format_count(collects)
                ));
                md.push_str(content_text.trim());
                md.push_str("\n\n");

                if !images.is_empty() && !options.strip_images {
                    md.push_str("### 笔记图文资产:\n\n");
                    for (idx, img_url) in images.iter().enumerate() {
                        md.push_str(&format!("![配图 #{}]({})\n", idx + 1, img_url));
                    }
                    md.push('\n');
                }

                md
            }
        };

        let token_count = count_tokens(&content, options.tokenizer);

        Ok(PageDocument {
            url: url.to_string(),
            final_url: url.to_string(),
            status: 200,
            title: Some(title.to_string()),
            byline: Some(author.to_string()),
            content,
            metadata: PageMetadata {
                author: Some(author.to_string()),
                canonical_url: Some(url.to_string()),
                cover_image: images.first().cloned(),
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
        })
    }
}

impl PlatformAdapter for XiaohongshuAdapter {
    fn name(&self) -> &'static str {
        "xiaohongshu"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        crate::fetcher::is_subdomain_or_exact(host, "xiaohongshu.com")
            || crate::fetcher::is_subdomain_or_exact(host, "xhslink.com")
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

struct XhsNoteDetail {
    title: String,
    author: String,
    content: String,
    likes: u64,
    collects: u64,
    image_urls: Vec<String>,
}

fn extract_initial_state(html: &str) -> Option<Value> {
    let marker = "window.__INITIAL_STATE__";
    let pos = html.find(marker)?;
    let after_marker = &html[pos + marker.len()..];
    let eq_pos = after_marker.find('=')?;
    let json_start = after_marker[eq_pos + 1..].trim_start();

    // Locate closing script tag </script>
    let script_end = json_start.find("</script>")?;
    let mut candidate = json_start[..script_end].trim();
    if candidate.ends_with(';') {
        candidate = candidate[..candidate.len() - 1].trim();
    }

    serde_json::from_str(candidate).ok()
}

fn extract_note_from_state(state: &Value) -> Option<XhsNoteDetail> {
    let note_map = state
        .get("note")
        .and_then(|n| n.get("noteDetailMap"))
        .or_else(|| state.get("noteData"))?;

    let note = if let Some(map) = note_map.as_object() {
        map.values()
            .next()
            .and_then(|v| v.get("note"))
            .or(Some(note_map))?
    } else {
        note_map
    };

    let title = note
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("小红书笔记")
        .to_string();
    let content = note
        .get("desc")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let author = note
        .get("user")
        .and_then(|u| u.get("nickname").or_else(|| u.get("name")))
        .and_then(|v| v.as_str())
        .unwrap_or("小红书用户")
        .to_string();

    let likes = note
        .get("interactInfo")
        .and_then(|i| i.get("likedCount"))
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse().ok())
                .or_else(|| v.as_u64())
        })
        .unwrap_or(0);

    let collects = note
        .get("interactInfo")
        .and_then(|i| i.get("collectedCount"))
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse().ok())
                .or_else(|| v.as_u64())
        })
        .unwrap_or(0);

    let mut image_urls = Vec::new();
    if let Some(img_list) = note.get("imageList").and_then(|v| v.as_array()) {
        for img in img_list {
            if let Some(url_str) = img
                .get("urlDefault")
                .or_else(|| img.get("url"))
                .and_then(|v| v.as_str())
            {
                image_urls.push(url_str.to_string());
            }
        }
    }

    Some(XhsNoteDetail {
        title,
        author,
        content,
        likes,
        collects,
        image_urls,
    })
}

fn extract_meta_property(doc: &Html, property: &str) -> Option<String> {
    let selector = Selector::parse(&format!("meta[property='{property}']")).ok()?;
    let el = doc.select(&selector).next()?;
    el.value().attr("content").map(|s| s.trim().to_string())
}

fn format_count(count: u64) -> String {
    if count >= 10_000 {
        format!("{:.1}万", count as f64 / 10_000.0)
    } else {
        count.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xhs_adapter_matching() {
        let adapter = XiaohongshuAdapter::new();
        let u1 = Url::parse("https://www.xiaohongshu.com/explore/654321").unwrap();
        let u2 = Url::parse("https://xhslink.com/a/abc123").unwrap();
        let u3 = Url::parse("https://evilxiaohongshu.com/explore/123").unwrap();

        assert!(adapter.matches(&u1));
        assert!(adapter.matches(&u2));
        assert!(!adapter.matches(&u3));
    }

    #[test]
    fn test_xhs_parse_note_initial_state() {
        let adapter = XiaohongshuAdapter::new();
        let sample_html = r#"
            <!DOCTYPE html>
            <html>
            <head><title>笔记详情</title></head>
            <body>
                <script>window.__INITIAL_STATE__ = {
                    "note": {
                        "noteDetailMap": {
                            "note_123": {
                                "note": {
                                    "title": "上海周末徒步宝藏路线",
                                    "desc": "今天给大家推荐一条绝美的佘山森林徒步路线，全程 8 公里。",
                                    "user": { "nickname": "徒步小达人" },
                                    "interactInfo": { "likedCount": "12500", "collectedCount": "3400" },
                                    "imageList": [
                                        { "urlDefault": "https://sns-img-qc.xhscdn.com/img1.jpg" },
                                        { "urlDefault": "https://sns-img-qc.xhscdn.com/img2.jpg" }
                                    ]
                                }
                            }
                        }
                    }
                };</script>
            </body>
            </html>
        "#;

        let url = Url::parse("https://www.xiaohongshu.com/explore/note_123").unwrap();
        let doc = adapter
            .parse_note(sample_html, &url, &ExtractionOptions::default())
            .unwrap();

        assert_eq!(doc.title.as_deref(), Some("上海周末徒步宝藏路线"));
        assert_eq!(doc.byline.as_deref(), Some("徒步小达人"));
        assert!(doc.content.contains("佘山森林徒步路线"));
        assert!(doc.content.contains("**获赞**: 1.2万"));
        assert!(doc.content.contains("**收藏**: 3400"));
        assert!(doc
            .content
            .contains("https://sns-img-qc.xhscdn.com/img1.jpg"));
    }
}
