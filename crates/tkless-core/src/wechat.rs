//! Public account article discovery and album enumeration, without backend credentials.
pub mod album;
pub mod history;
pub mod identity;
use crate::{ExtractionOptions, PageDocument, TklessEngine, TklessError};
use identity::{failure, Account};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const DEFAULT_LIMIT: usize = 100;
pub const MAX_LIMIT: usize = 1000;
pub const DEFAULT_PAGES: usize = 5;
pub const MAX_PAGES: usize = 100;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WechatOptions {
    pub limit: usize,
    pub max_pages: usize,
    pub delay_ms: u64,
    pub fetch_content: bool,
    pub follow_albums: bool,
    pub expected_biz: Option<String>,
}
impl Default for WechatOptions {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LIMIT,
            max_pages: DEFAULT_PAGES,
            delay_ms: 300,
            fetch_content: false,
            follow_albums: true,
            expected_biz: None,
        }
    }
}
impl WechatOptions {
    pub fn validate(&self) -> Result<(), TklessError> {
        if !(1..=MAX_LIMIT).contains(&self.limit)
            || !(1..=MAX_PAGES).contains(&self.max_pages)
            || self.delay_ms > 60_000
            || self
                .expected_biz
                .as_ref()
                .is_some_and(|v| !identity::valid_biz(v))
        {
            return Err(TklessError::InvalidArgument(
                "Invalid WeChat limit, page budget, delay or biz identifier".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Article {
    pub id: String,
    pub url: String,
    pub title: String,
    pub account: Account,
    pub published_at: Option<String>,
    pub cover: Option<String>,
    pub document: Option<PageDocument>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Exhausted,
    PageLimit,
    ItemLimit,
    RepeatedCursor,
    RepeatedPage,
    InvalidPage,
    FetchFailed,
    DiscoveryOnly,
    IdentityMismatch,
    SessionRequired,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WechatReport {
    pub source: String,
    pub scope: String,
    pub account_history_complete: bool,
    pub listing_complete: bool,
    pub complete: bool,
    pub stop_reason: StopReason,
    pub pages_fetched: usize,
    pub duplicates: usize,
    pub rejected: usize,
    pub total_hint: Option<u64>,
    pub album_title: Option<String>,
    pub accounts: Vec<Account>,
    pub articles: Vec<Article>,
    pub albums: Vec<String>,
    pub next_urls: Vec<String>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}
impl WechatReport {
    pub(super) fn new(source: &str, scope: &str) -> Self {
        Self {
            source: source.into(),
            scope: scope.into(),
            account_history_complete: false,
            listing_complete: false,
            complete: false,
            stop_reason: StopReason::DiscoveryOnly,
            pages_fetched: 0,
            duplicates: 0,
            rejected: 0,
            total_hint: None,
            album_title: None,
            accounts: vec![],
            articles: vec![],
            albums: vec![],
            next_urls: vec![],
            errors: vec![],
            warnings: vec![],
        }
    }
}

fn from_document(doc: PageDocument, retain: bool) -> Result<(Article, Vec<String>), TklessError> {
    let entity = doc
        .entity
        .as_ref()
        .ok_or_else(|| failure("Page is not a verified WeChat article"))?;
    if entity["type"] != "wechat_article" {
        return Err(failure("Unexpected article entity"));
    }
    let account: Account = serde_json::from_value(entity["account"].clone())?;
    let url = identity::public_url(&doc.final_url)?;
    if !identity::is_article(&url) {
        return Err(failure("Article redirected outside article routes"));
    }
    let albums = entity["albums"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Ok((
        Article {
            id: entity["article_id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| identity::article_id(&url, &account)),
            url: url.to_string(),
            title: doc.title.clone().unwrap_or_default(),
            account,
            published_at: doc.metadata.published_time.clone(),
            cover: doc.metadata.cover_image.clone(),
            error: None,
            document: retain.then_some(doc),
        },
        albums,
    ))
}

pub async fn list_articles(
    engine: &TklessEngine,
    source: &str,
    options: &WechatOptions,
    extraction: &ExtractionOptions,
) -> Result<WechatReport, TklessError> {
    options.validate()?;
    extraction.validate()?;
    if source.trim().is_empty() || source.len() > 4096 {
        return Err(TklessError::InvalidArgument(
            "WeChat source must contain 1..4096 bytes".into(),
        ));
    }
    let source = source.trim();
    let parsed = url::Url::parse(source).ok();
    if let Some(url) = &parsed {
        let url = identity::public_url(url.as_str())?;
        if url.path() == "/mp/appmsgalbum" {
            let mut report = album::collect(engine.fetcher(), &url, options).await?;
            attach_content(engine, &mut report, options, extraction).await;
            return Ok(report);
        }
        if !identity::is_article(&url) {
            return Err(TklessError::InvalidArgument(
                "Only WeChat article and album URLs are supported".into(),
            ));
        }
    } else if source.contains("://") {
        return Err(TklessError::InvalidArgument("Invalid WeChat URL".into()));
    }
    let mut report = WechatReport::new(
        source,
        if parsed.is_some() {
            "article_and_linked_albums"
        } else {
            "indexed_articles"
        },
    );
    let candidates = if let Some(url) = parsed {
        vec![identity::public_url(url.as_str())?.to_string()]
    } else {
        let search = crate::search::web_search(
            engine,
            &format!("\"{}\"", source.replace('"', " ")),
            &crate::search::SearchOptions {
                scope: Some("wechat".into()),
                limit: options.limit.min(20),
                ..Default::default()
            },
        )
        .await?;
        report.warnings.push("Search is limited to indexed articles; matching display names do not prove a unique account. Use expected_biz to disambiguate.".into());
        search.results.into_iter().map(|r| r.url).collect()
    };
    let mut ids = HashSet::new();
    for candidate in candidates {
        let url = match identity::public_url(&candidate) {
            Ok(u) if identity::is_article(&u) => u,
            _ => {
                report.rejected += 1;
                continue;
            }
        };
        tokio::time::sleep(std::time::Duration::from_millis(options.delay_ms)).await;
        let item = engine
            .process_url(url.as_str(), extraction)
            .await
            .and_then(|doc| from_document(doc, options.fetch_content));
        match item {
            Ok((article, albums)) => {
                let matches_name = parsed_name_matches(source, &report.scope, &article.account);
                if !matches_name
                    || options
                        .expected_biz
                        .as_ref()
                        .is_some_and(|biz| article.account.biz.as_ref() != Some(biz))
                {
                    report.rejected += 1;
                    if report.scope != "indexed_articles" {
                        report.stop_reason = StopReason::IdentityMismatch;
                        report
                            .errors
                            .push("Article account does not match expected_biz".into());
                    }
                    continue;
                }
                if !report.accounts.contains(&article.account) {
                    report.accounts.push(article.account.clone());
                }
                for album in albums {
                    if !report.albums.contains(&album) {
                        report.albums.push(album);
                    }
                }
                if ids.insert(article.id.clone()) {
                    report.articles.push(article);
                } else {
                    report.duplicates += 1;
                }
            }
            Err(error) => {
                report.stop_reason = StopReason::FetchFailed;
                report.errors.push(format!("{url}: {error}"));
            }
        }
    }
    if report.scope == "article_and_linked_albums" && options.follow_albums {
        for url in report.albums.clone() {
            if report.pages_fetched >= options.max_pages || report.articles.len() >= options.limit {
                report.next_urls.push(url);
                continue;
            }
            let mut remaining = options.clone();
            remaining.max_pages -= report.pages_fetched;
            remaining.limit = options.limit - report.articles.len();
            remaining.expected_biz = report.accounts.first().and_then(|a| a.biz.clone());
            match album::collect(engine.fetcher(), &identity::public_url(&url)?, &remaining).await {
                Ok(part) => {
                    report.pages_fetched += part.pages_fetched;
                    report.errors.extend(part.errors);
                    report.next_urls.extend(part.next_urls);
                    for article in part.articles {
                        if ids.insert(article.id.clone()) {
                            report.articles.push(article);
                        } else {
                            report.duplicates += 1;
                        }
                    }
                }
                Err(error) => report.errors.push(format!("{url}: {error}")),
            }
        }
    }
    report.warnings.push(
        "Public discovery and linked albums do not establish a complete account history.".into(),
    );
    attach_content(engine, &mut report, options, extraction).await;
    Ok(report)
}

fn parsed_name_matches(source: &str, scope: &str, account: &Account) -> bool {
    scope != "indexed_articles"
        || (account.biz.is_some()
            && account
                .nickname
                .as_ref()
                .is_some_and(|name| name.trim() == source))
}

async fn attach_content(
    engine: &TklessEngine,
    report: &mut WechatReport,
    options: &WechatOptions,
    extraction: &ExtractionOptions,
) {
    if !options.fetch_content {
        return;
    }
    for article in &mut report.articles {
        if article.document.is_some() {
            continue;
        }
        tokio::time::sleep(std::time::Duration::from_millis(options.delay_ms)).await;
        match engine
            .process_url(&article.url, extraction)
            .await
            .and_then(|doc| from_document(doc, true))
        {
            Ok((fetched, _))
                if article
                    .account
                    .biz
                    .as_ref()
                    .is_none_or(|biz| fetched.account.biz.as_ref() == Some(biz)) =>
            {
                article.document = fetched.document
            }
            Ok(_) => article.error = Some("Article account identity mismatch".into()),
            Err(error) => article.error = Some(error.to_string()),
        }
    }
    report.complete &= report.articles.iter().all(|a| a.error.is_none());
}

pub(crate) fn report_document(
    report: WechatReport,
    url: &url::Url,
    options: &ExtractionOptions,
) -> Result<PageDocument, TklessError> {
    if report.articles.is_empty() && !report.errors.is_empty() {
        return Err(failure(report.errors.join("; ")));
    }
    let escape = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let title = report
        .album_title
        .clone()
        .unwrap_or_else(|| "微信公众号合集".into());
    let mut html = format!("<h1>{}</h1><ul>", escape(&title));
    for article in &report.articles {
        html.push_str(&format!(
            "<li><a href=\"{}\">{}</a></li>",
            escape(&article.url),
            escape(&article.title)
        ));
    }
    html.push_str("</ul>");
    let content = match options.format {
        crate::OutputFormat::Html => html,
        crate::OutputFormat::Raw => serde_json::to_string(&report)?,
        crate::OutputFormat::Text => report
            .articles
            .iter()
            .map(|a| format!("{}\n{}", a.title, a.url))
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => crate::markdown::MarkdownConverter::new().convert(
            &html,
            options.strip_images,
            options.strip_links,
        )?,
    };
    let warnings = if report.complete {
        vec![]
    } else {
        vec![crate::DiagnosticWarning {
            code: "wechat_partial_listing".into(),
            message: format!("Incomplete album listing: {:?}", report.stop_reason),
        }]
    };
    Ok(PageDocument {
        extraction: Default::default(),
        warnings,
        url: url.to_string(),
        final_url: url.to_string(),
        status: 200,
        title: Some(title),
        byline: report.accounts.first().and_then(|a| a.nickname.clone()),
        token_count: crate::token::count_tokens(&content, options.tokenizer),
        content,
        metadata: Default::default(),
        json_ld: vec![],
        tokenizer: options.tokenizer.as_str().into(),
        truncated: !report.complete,
        subtitles: None,
        danmaku: None,
        comments: None,
        conclusion: None,
        links: None,
        images: None,
        tables: None,
        entity: Some(serde_json::json!({"type":"wechat_album","report":report})),
        chunks: None,
        content_hash: None,
        metrics: None,
    })
}
