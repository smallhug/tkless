use crate::adapters::{BoxFuture, PlatformAdapter};
use crate::error::TklessError;
use crate::markdown::MarkdownConverter;
use crate::model::{ExtractionOptions, OutputFormat, PageDocument, PageMetadata};
use crate::token::count_tokens;
use scraper::{Html, Selector};
use std::sync::LazyLock;
use tracing::debug;
use url::Url;

static WECHAT_TITLE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("#activity-name, h1.rich_media_title").unwrap());
static WECHAT_AUTHOR_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("#js_name, .profile_nickname, .rich_media_meta_nickname, .rich_media_meta_text, meta[name='author'], meta[property='og:article:author']")
        .unwrap()
});
static WECHAT_CONTENT_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("#js_content").unwrap());
static WECHAT_NOISE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("#js_profile_qrcode, .reward_area, .js_reward_wording, .qr_code_pc, #js_tags, .profile_container")
        .unwrap()
});

static SOGOU_JS_URL_REGEX: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"url\s*\+?=\s*['"]([^'"]*)['"]"#).unwrap());

pub struct WechatAdapter;

impl Default for WechatAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl WechatAdapter {
    pub fn new() -> Self {
        Self
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        debug!(url = %url, "Executing WeChat Official Accounts adapter extraction");

        if url.path() == "/mp/appmsgalbum" {
            let report = crate::wechat::album::collect(fetcher, url, &Default::default()).await?;
            return crate::wechat::report_document(report, url, options);
        }

        // Decrypt intermediate Sogou redirect link if applicable
        let target_url = if url.host_str() == Some("weixin.sogou.com") {
            Self::decrypt_sogou_wechat_redirect(fetcher, url, options).await?
        } else {
            url.clone()
        };

        let target_url = crate::wechat::identity::public_url(target_url.as_str())?;
        let raw = fetcher.fetch_api(target_url.as_str()).await?;
        if raw.truncated || !(200..300).contains(&raw.status) {
            return Err(crate::wechat::identity::failure(
                "Incomplete or unsuccessful WeChat article response",
            ));
        }
        let final_url = crate::wechat::identity::public_url(&raw.final_url)?;
        if !crate::wechat::identity::is_article(&final_url) {
            return Err(crate::wechat::identity::failure(
                "Article redirected to verification or an unsupported route",
            ));
        }
        let mut document = self.parse_article(&raw.html, &final_url, options)?;
        document.url = url.to_string();
        document.status = raw.status;
        Ok(document)
    }

    /// Automatically decrypts and unpacks Sogou WeChat intermediate redirect URLs (`/link?url=...`)
    /// into the genuine `https://mp.weixin.qq.com/s?...` destination article URL.
    pub async fn decrypt_sogou_wechat_redirect(
        fetcher: &crate::fetcher::Fetcher,
        sogou_url: &Url,
        options: &ExtractionOptions,
    ) -> Result<Url, TklessError> {
        debug!(sogou_url = %sogou_url, "正在逆向解析搜狗微信重定向中间跳转页");

        // Resolve existing redirect URLs without issuing a Sogou search or warming its session.
        let req = fetcher
            .get(sogou_url)
            .header("Referer", "https://weixin.sogou.com/")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 Edg/133.0.0.0",
            )
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            );
        let resp = req.send().await?;
        let status_u16 = resp.status().as_u16();
        let html = fetcher.read_text(resp).await?;

        // 2. Static regex extraction of dynamic JavaScript string concatenations
        let mut parts = Vec::new();
        for cap in SOGOU_JS_URL_REGEX.captures_iter(&html) {
            if let Some(m) = cap.get(1) {
                parts.push(m.as_str());
            }
        }

        if !parts.is_empty() {
            let assembled = parts.concat().replace('@', "");
            if let Ok(real_url) = Url::parse(&assembled) {
                if real_url.host_str() == Some("mp.weixin.qq.com") {
                    debug!(real_url = %real_url, "成功从搜狗 JS 片段中逆向还原微信真实文章直达链接");
                    return Ok(real_url);
                }
            }
        }

        // 3. If Sogou anti-spider challenge was served, fall back to auto-managed CDP browser navigation
        let is_challenge = crate::cdp::is_anti_scraping_challenge(&html, status_u16);
        let cdp_ep =
            crate::cdp::resolve_cdp_endpoint(options.cdp_endpoint.as_deref(), options.auto_cdp);

        if is_challenge || parts.is_empty() {
            if let Some(ep) = cdp_ep {
                debug!(endpoint = %ep, "搜狗重定向受阻，自动无缝启动无头浏览器 CDP 执行重定向跟踪");
                let cdp_opts = crate::cdp::CdpOptions {
                    allow_private_addresses: fetcher.options().allow_private_addresses,
                    endpoint: ep,
                    wait_ms: Some(2000),
                    timeout_ms: Some(30_000),
                    ..Default::default()
                };
                if let Ok(rendered_html) =
                    crate::cdp::fetch_html_via_cdp(sogou_url.as_str(), &cdp_opts).await
                {
                    for cap in SOGOU_JS_URL_REGEX.captures_iter(&rendered_html) {
                        if let Some(m) = cap.get(1) {
                            parts.push(m.as_str());
                        }
                    }
                    if !parts.is_empty() {
                        let assembled = parts.concat().replace('@', "");
                        if let Ok(real_url) = Url::parse(&assembled) {
                            return Ok(real_url);
                        }
                    }
                }
            }
        }

        // Return original URL if decryption was unable to resolve
        Ok(sogou_url.clone())
    }

    pub fn parse_article(
        &self,
        raw_html: &str,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        if raw_html.contains("secitptpage/verify.html")
            || raw_html.contains("环境异常，完成验证后即可继续访问")
        {
            return Err(TklessError::AdapterError {
                platform: "wechat",
                message: "WeChat anti-scraping verification challenge triggered (微信反爬安全验证拦截: 环境异常)".to_string(),
            });
        }

        let doc = Html::parse_document(raw_html);

        // 1. Extract Title
        let title = doc
            .select(&WECHAT_TITLE_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .or_else(|| extract_meta_property(&doc, "og:title"));

        // 2. Extract Author / Account Name
        let author = extract_wechat_author(&doc, raw_html);

        // 3. Extract Metadata
        let desc = extract_meta_property(&doc, "og:description")
            .or_else(|| extract_meta_name(&doc, "description"));
        let cover_url = extract_meta_property(&doc, "og:image");
        let pub_date = extract_publish_time(&doc, raw_html);

        let mut metadata = PageMetadata {
            description: desc.clone(),
            author: author.clone(),
            canonical_url: Some(url.to_string()),
            published_time: pub_date.clone(),
            cover_image: cover_url.clone(),
            ..Default::default()
        };
        if let Some(ref t) = title {
            metadata.open_graph.insert("og:title".into(), t.clone());
        }
        if let Some(ref d) = desc {
            metadata
                .open_graph
                .insert("og:description".into(), d.clone());
        }
        if let Some(ref c) = cover_url {
            metadata.open_graph.insert("og:image".into(), c.clone());
        }

        // 4. Extract and Clean Article Body (#js_content)
        let content_el = doc.select(&WECHAT_CONTENT_SELECTOR).next().ok_or_else(|| {
            crate::wechat::identity::failure(
                "WeChat article body missing: page may be deleted, restricted or changed",
            )
        })?;
        let rendered_body = clean_wechat_content(content_el);
        let body_html = crate::cleaner::clean_html(
            &rendered_body,
            Some(url.as_str()),
            crate::ExtractionMode::Full,
        )?
        .html;
        let account = crate::wechat::identity::article_identity(raw_html, url);
        let albums = crate::wechat::identity::album_links(raw_html, url, &account);

        // 5. Convert to requested format
        let content = match options.format {
            OutputFormat::Html => body_html.clone(),
            OutputFormat::Raw => raw_html.to_string(),
            OutputFormat::Text => wechat_html_to_plain_text(&body_html),
            OutputFormat::Markdown | OutputFormat::Json => {
                let converter = MarkdownConverter::new();
                let markdown_body =
                    converter.convert(&body_html, options.strip_images, options.strip_links)?;

                let mut md = String::new();
                if options.frontmatter && options.format == OutputFormat::Markdown {
                    md.push_str("---\n");
                    if let Some(ref t) = title {
                        md.push_str(&format!("title: \"{}\"\n", t.replace('"', "\\\"")));
                    }
                    if let Some(ref a) = author {
                        if !a.trim().is_empty() {
                            md.push_str(&format!("account: \"{}\"\n", a.replace('"', "\\\"")));
                        }
                    }
                    md.push_str(&format!("url: \"{url}\"\n"));
                    if let Some(ref p) = pub_date {
                        if !p.trim().is_empty() {
                            md.push_str(&format!("published: \"{p}\"\n"));
                        }
                    }
                    if let Some(ref c) = cover_url {
                        md.push_str(&format!("cover: \"{c}\"\n"));
                    }
                    md.push_str("---\n\n");
                }

                if let Some(ref t) = title {
                    md.push_str(&format!("# {}\n\n", t));
                }

                let mut header_parts = Vec::new();
                if let Some(ref a) = author {
                    if !a.trim().is_empty() {
                        header_parts.push(format!("**公众号**: {a}"));
                    }
                }
                if let Some(ref p) = pub_date {
                    if !p.trim().is_empty() {
                        header_parts.push(format!("**发布时间**: {p}"));
                    }
                }
                if !header_parts.is_empty() {
                    md.push_str(&format!("> {}\n\n", header_parts.join(" | ")));
                }

                md.push_str(&markdown_body);
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
            title,
            byline: author,
            content,
            metadata,
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
            entity: Some(
                serde_json::json!({"type":"wechat_article","account":account,"albums":albums,"article_id":crate::wechat::identity::article_id_from_html(raw_html, url, &account)}),
            ),
            chunks: None,
            content_hash: None,
            metrics: None,
        })
    }
}

impl PlatformAdapter for WechatAdapter {
    fn allow_html_fallback(&self, _url: &Url) -> bool {
        // Verification and API error pages must never become successful article content.
        false
    }

    fn name(&self) -> &'static str {
        "wechat"
    }

    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("");
        (host == "mp.weixin.qq.com"
            && (crate::wechat::identity::is_article(url) || url.path() == "/mp/appmsgalbum"))
            || (host == "weixin.sogou.com" && url.path() == "/link")
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

/// Cleans WeChat's `#js_content` element:
/// - Promotes `data-src` to `src` for all lazy-loaded images
/// - Drops WeChat-specific QR codes, reward areas, and follow wrappers
fn clean_wechat_content(content_el: scraper::ElementRef<'_>) -> String {
    let mut out = String::with_capacity(4096);
    render_wechat_node(content_el, &mut out);
    out
}

fn render_wechat_node(el: scraper::ElementRef<'_>, out: &mut String) {
    if WECHAT_NOISE_SELECTOR.matches(&el) {
        return;
    }

    let tag_name = el.value().name();

    if tag_name == "img" {
        // Promote data-src to src
        let mut src_url = el
            .value()
            .attr("data-src")
            .or_else(|| el.value().attr("src"))
            .unwrap_or("");
        src_url = src_url.trim();

        if !src_url.is_empty() {
            let alt = el.value().attr("alt").unwrap_or("").trim();
            out.push_str("<img src=\"");
            out.push_str(&src_url.replace('"', "&quot;"));
            out.push_str("\" alt=\"");
            out.push_str(&alt.replace('"', "&quot;"));
            out.push_str("\" />");
        }
        return;
    }

    out.push('<');
    out.push_str(tag_name);

    for (k, v) in el.value().attrs() {
        if k.starts_with("on") || k == "style" {
            continue;
        }
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        if v.contains('"') {
            out.push_str(&v.replace('"', "&quot;"));
        } else {
            out.push_str(v);
        }
        out.push('"');
    }
    out.push('>');

    for child in el.children() {
        if let Some(child_el) = scraper::ElementRef::wrap(child) {
            render_wechat_node(child_el, out);
        } else if let Some(text) = child.value().as_text() {
            out.push_str(text);
        }
    }

    out.push_str("</");
    out.push_str(tag_name);
    out.push('>');
}

fn extract_meta_property(doc: &Html, property: &str) -> Option<String> {
    let selector = Selector::parse(&format!("meta[property='{property}']")).ok()?;
    let el = doc.select(&selector).next()?;
    el.value().attr("content").map(|s| s.trim().to_string())
}

fn extract_meta_name(doc: &Html, name: &str) -> Option<String> {
    let selector = Selector::parse(&format!("meta[name='{name}']")).ok()?;
    let el = doc.select(&selector).next()?;
    el.value().attr("content").map(|s| s.trim().to_string())
}

fn extract_publish_time(doc: &Html, html: &str) -> Option<String> {
    // 1. Check #publish_time DOM element
    static TIME_SEL: LazyLock<Selector> =
        LazyLock::new(|| Selector::parse("#publish_time, em.rich_media_meta_text").unwrap());
    for el in doc.select(&TIME_SEL) {
        let text = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
        if text.starts_with("20") && (text.contains('-') || text.contains('/')) {
            return Some(text);
        }
    }

    // 2. Regex for formatted date string: '2026-08-18 09:00'
    static DATE_STR_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"(?:createTime|create_time)\s*[:=]\s*['"](\d{4}[-/]\d{1,2}[-/]\d{1,2}(?:\s+\d{1,2}:\d{1,2}(?::\d{1,2})?)?)['"]"#).unwrap()
    });
    if let Some(cap) = DATE_STR_RE.captures(html) {
        if let Some(matched) = cap.get(1) {
            let val = matched.as_str().trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }

    // 3. Regex for 10-digit unix timestamp: ori_create_time / create_timestamp / ct
    static TIMESTAMP_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"(?:ori_create_time|create_timestamp|ct)\s*[:=]\s*['"]?(\d{10})['"]?"#)
            .unwrap()
    });
    if let Some(cap) = TIMESTAMP_RE.captures(html) {
        if let Some(matched) = cap.get(1) {
            if let Ok(ts) = matched.as_str().parse::<i64>() {
                if let Some(dt) = chrono::DateTime::from_timestamp(ts, 0) {
                    return Some(dt.format("%Y-%m-%d %H:%M:%S").to_string());
                }
            }
        }
    }

    None
}

fn extract_wechat_author(doc: &Html, raw_html: &str) -> Option<String> {
    for el in doc.select(&WECHAT_AUTHOR_SELECTOR) {
        let val = if let Some(content) = el.value().attr("content") {
            content.trim().to_string()
        } else {
            el.text().collect::<Vec<_>>().join(" ").trim().to_string()
        };
        if !val.is_empty() {
            return Some(val);
        }
    }

    if let Some(val) = extract_meta_property(doc, "og:article:author") {
        if !val.is_empty() {
            return Some(val);
        }
    }
    if let Some(val) = extract_meta_property(doc, "author") {
        if !val.is_empty() {
            return Some(val);
        }
    }

    static NICKNAME_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"(?:var\s+nickname\s*=\s*['"]([^'"]+)['"]|var\s+appmsg_author_name\s*=\s*['"]([^'"]+)['"]|var\s+user_name\s*=\s*['"]([^'"]+)['"])"#).unwrap()
    });
    if let Some(cap) = NICKNAME_RE.captures(raw_html) {
        for idx in 1..=3 {
            if let Some(m) = cap.get(idx) {
                let val = m.as_str().trim();
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }

    None
}

fn wechat_html_to_plain_text(html: &str) -> String {
    let doc = Html::parse_fragment(html);
    let mut text_parts = Vec::new();

    for text in doc.root_element().text() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            text_parts.push(trimmed);
        }
    }

    text_parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wechat_adapter_matching() {
        let adapter = WechatAdapter::new();
        let url1 = Url::parse("https://mp.weixin.qq.com/s/abcdef123456").unwrap();
        let url2 = Url::parse("https://mp.weixin.qq.com/s?__biz=abc&mid=123").unwrap();
        let url3 = Url::parse("https://weixin.qq.com/").unwrap();

        assert!(adapter.matches(&url1));
        assert!(adapter.matches(&url2));
        assert!(!adapter.matches(&url3));
    }

    #[test]
    fn test_wechat_clean_content_data_src_and_noise() {
        let adapter = WechatAdapter::new();
        let sample_html = r#"
            <!DOCTYPE html>
            <html>
            <head>
                <meta property="og:title" content="Rust 深度解析" />
                <meta property="og:description" content="深入理解 Rust 所有权与生命周期。" />
            </head>
            <body>
                <h1 id="activity-name">Rust 深度解析</h1>
                <div id="js_name">Rust爱好者天地</div>
                <div id="js_content">
                    <p>正文第一段：Rust 是一门系统级编程语言。</p>
                    <img data-src="https://mmbiz.qpic.cn/mmbiz_png/rust_diagram.png" src="data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7" alt="架构图" />
                    <div class="reward_area">
                        <p>赞赏鼓励</p>
                    </div>
                    <div id="js_profile_qrcode">
                        <p>扫码关注公众号</p>
                    </div>
                </div>
            </body>
            </html>
        "#;

        let url = Url::parse("https://mp.weixin.qq.com/s/sample_test").unwrap();
        let doc = adapter
            .parse_article(sample_html, &url, &ExtractionOptions::default())
            .expect("Should parse article");

        assert_eq!(doc.title.as_deref(), Some("Rust 深度解析"));
        assert_eq!(doc.byline.as_deref(), Some("Rust爱好者天地"));
        assert!(doc.content.contains("Rust 是一门系统级编程语言。"));
        // Image data-src must be promoted to src!
        assert!(doc
            .content
            .contains("https://mmbiz.qpic.cn/mmbiz_png/rust_diagram.png"));
        assert!(!doc.content.contains("data:image/gif"));
        // Noise must be stripped!
        assert!(!doc.content.contains("赞赏鼓励"));
        assert!(!doc.content.contains("扫码关注公众号"));
    }

    #[test]
    fn test_wechat_anti_scraping_detection() {
        let adapter = WechatAdapter::new();
        let verify_html = r#"
            <!DOCTYPE html>
            <html>
            <head><title>验证</title></head>
            <body>
                <h2>环境异常</h2>
                <p>当前环境异常，完成验证后即可继续访问。去验证</p>
                <script>var PAGE_MID='mmbizwap:secitptpage/verify.html';</script>
            </body>
            </html>
        "#;

        let url = Url::parse("https://mp.weixin.qq.com/s/blocked_test").unwrap();
        let result = adapter.parse_article(verify_html, &url, &ExtractionOptions::default());
        assert!(result.is_err());
        match result {
            Err(TklessError::AdapterError { platform, message }) => {
                assert_eq!(platform, "wechat");
                assert!(message.contains("微信反爬安全验证拦截"));
            }
            _ => panic!("Expected AdapterError for verification challenge"),
        }
    }

    #[test]
    fn test_wechat_publish_time_and_author_parsing() {
        let adapter = WechatAdapter::new();
        let html_with_script_author_and_time = r#"
            <!DOCTYPE html>
            <html>
            <head>
                <meta property="og:title" content="DeepSeek 实测" />
            </head>
            <body>
                <h1 id="activity-name">DeepSeek 实测</h1>
                <div id="js_content"><p>测试正文内容。</p></div>
                <script>
                    var nickname = "深度探索者";
                    var createTime = '2026-08-18 09:00';
                    var ct = "invalid_hash_token_should_be_ignored";
                </script>
            </body>
            </html>
        "#;

        let url = Url::parse("https://mp.weixin.qq.com/s/author_time_test").unwrap();
        let doc = adapter
            .parse_article(
                html_with_script_author_and_time,
                &url,
                &ExtractionOptions::default(),
            )
            .expect("Should parse");

        assert_eq!(doc.title.as_deref(), Some("DeepSeek 实测"));
        assert_eq!(doc.byline.as_deref(), Some("深度探索者"));
        assert_eq!(
            doc.metadata.published_time.as_deref(),
            Some("2026-08-18 09:00")
        );
        assert!(doc.content.contains("**公众号**: 深度探索者"));
        assert!(doc.content.contains("**发布时间**: 2026-08-18 09:00"));
    }

    #[test]
    fn test_wechat_output_formats() {
        let adapter = WechatAdapter::new();
        let sample_html = r#"
            <!DOCTYPE html>
            <html>
            <head><meta property="og:title" content="格式测试" /></head>
            <body>
                <h1 id="activity-name">格式测试</h1>
                <div id="js_content"><p>这是<strong>加粗文本</strong>和段落。</p></div>
            </body>
            </html>
        "#;

        let url = Url::parse("https://mp.weixin.qq.com/s/format_test").unwrap();

        // 1. Text format
        let text_options = ExtractionOptions {
            format: OutputFormat::Text,
            ..Default::default()
        };
        let doc_text = adapter
            .parse_article(sample_html, &url, &text_options)
            .unwrap();
        assert!(doc_text.content.contains("加粗文本"));
        assert!(doc_text.content.contains("和段落。"));
        assert!(!doc_text.content.contains("<strong>"));
        assert!(!doc_text.content.contains("**加粗文本**"));

        // 2. Html format
        let html_options = ExtractionOptions {
            format: OutputFormat::Html,
            ..Default::default()
        };
        let doc_html = adapter
            .parse_article(sample_html, &url, &html_options)
            .unwrap();
        assert!(doc_html.content.contains("<strong>加粗文本</strong>"));
    }

    #[test]
    fn test_sogou_wechat_redirect_matching_and_js_extraction() {
        let adapter = WechatAdapter::new();
        let sogou_url = Url::parse(
            "https://weixin.sogou.com/link?url=dn9a_-gY295K0Rci_xozVXfdMkSQTLW6cwJThYulHEtVjX",
        )
        .unwrap();

        assert!(
            adapter.matches(&sogou_url),
            "WechatAdapter must match Sogou redirect links"
        );

        // Verify JavaScript string slice reconstruction logic
        let mock_redirect_html = r#"
            <!DOCTYPE HTML>
            <html>
            <head><title>搜狗搜索</title></head>
            <body>
            <script>
                var url = '';
                url += 'https://mp.';
                url += 'weixin.qq.c';
                url += 'om/s?src=11';
                url += '&timestamp=1792273021';
                url += '&ver=7033';
                url += '&signature=abcdef123456';
                url += '&new=1';
                url.replace("@", "");
                window.location.replace(url);
            </script>
            </body>
            </html>
        "#;

        let mut parts = Vec::new();
        for cap in SOGOU_JS_URL_REGEX.captures_iter(mock_redirect_html) {
            if let Some(m) = cap.get(1) {
                parts.push(m.as_str());
            }
        }
        let assembled = parts.concat().replace('@', "");
        let parsed = Url::parse(&assembled).expect("Should parse as valid URL");
        assert_eq!(parsed.host_str(), Some("mp.weixin.qq.com"));
        assert!(parsed.query().unwrap().contains("timestamp=1792273021"));
        assert!(parsed.query().unwrap().contains("signature=abcdef123456"));
    }
}
