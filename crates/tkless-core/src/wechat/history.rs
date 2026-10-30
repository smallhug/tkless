//! Authorized history enumeration. Session secrets never enter reports or checkpoints.
use super::{identity, Account, Article, StopReason, WechatOptions, WechatReport};
use crate::{state_file, ExtractionOptions, TklessEngine, TklessError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    future::Future,
    io::Read,
    path::{Path, PathBuf},
};
use url::Url;

#[derive(Default, Clone)]
pub struct HistoryOptions {
    pub session_file: Option<PathBuf>,
    pub checkpoint: Option<PathBuf>,
    pub resume: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Session {
    biz: String,
    #[serde(default)]
    cookie: String,
    #[serde(default)]
    parameters: BTreeMap<String, String>,
}
impl Session {
    fn read(path: &Path, biz: &str) -> Result<Self, TklessError> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(65537)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 65536 {
            return Err(invalid("Session file exceeds 64 KiB"));
        }
        // Do not include serde diagnostics: an unknown field can itself contain a secret.
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
        let session: Self =
            serde_json::from_slice(bytes).map_err(|_| invalid("Invalid session file schema"))?;
        session.validate(biz)?;
        Ok(session)
    }
    fn validate(&self, biz: &str) -> Result<(), TklessError> {
        if self.biz != biz || self.cookie.contains(['\r', '\n']) {
            return Err(invalid("Session account mismatch or invalid cookie"));
        }
        for (key, value) in &self.parameters {
            if ![
                "uin",
                "key",
                "pass_ticket",
                "appmsg_token",
                "poc_token",
                "poc_sid",
            ]
            .contains(&key.as_str())
                || value.is_empty()
                || value.len() > 16384
                || value.contains(['\r', '\n'])
            {
                return Err(invalid("Unsupported session parameter or invalid value"));
            }
        }
        if self.cookie.is_empty() && self.parameters.is_empty() {
            return Err(invalid("Session file contains no credentials"));
        }
        Ok(())
    }
}

fn invalid(message: &str) -> TklessError {
    TklessError::InvalidArgument(message.into())
}

#[derive(Serialize, Deserialize)]
struct Checkpoint {
    version: u32,
    biz: String,
    offset: u64,
    report: WechatReport,
}
impl Checkpoint {
    fn new(account: Account) -> Self {
        let biz = account.biz.clone().expect("resolved account");
        let mut report = WechatReport::new(&biz, "account_history");
        report.accounts.push(account);
        report.warnings.push("Completion covers only history visible to the authorized session; deleted or inaccessible articles cannot be guaranteed.".into());
        Self {
            version: 1,
            biz,
            offset: 0,
            report,
        }
    }
    fn validate(&self, biz: &str) -> Result<(), TklessError> {
        if self.version != 1
            || self.biz != biz
            || self.report.source != biz
            || self.report.scope != "account_history"
            || self.report.accounts.len() != 1
            || self.report.accounts[0].biz.as_deref() != Some(biz)
        {
            return Err(invalid("Checkpoint version or account mismatch"));
        }
        for article in &self.report.articles {
            let url = identity::public_url(&article.url)?;
            if !identity::is_article(&url)
                || identity::parameter(&url, "__biz").as_deref() != Some(biz)
                || article.account.biz.as_deref() != Some(biz)
                || article.id != identity::article_id(&url, &article.account)
            {
                return Err(invalid("Checkpoint contains an invalid article identity"));
            }
        }
        Ok(())
    }
    fn save(&self, path: Option<&Path>) -> Result<(), TklessError> {
        if let Some(path) = path {
            state_file::write(path, self)?;
        }
        Ok(())
    }
}

async fn resolve_account(
    engine: &TklessEngine,
    source: &str,
    options: &WechatOptions,
    extraction: &ExtractionOptions,
) -> Result<Account, TklessError> {
    let account = if identity::valid_biz(source) && !source.contains("://") {
        Account {
            biz: Some(source.into()),
            ..Default::default()
        }
    } else {
        let url = identity::public_url(source)?;
        if !identity::is_article(&url) {
            return Err(invalid(
                "History source must be an article URL or biz identifier",
            ));
        }
        let doc = engine.process_url(url.as_str(), extraction).await?;
        super::from_document(doc, false)?.0.account
    };
    let biz = account
        .biz
        .as_deref()
        .filter(|s| identity::valid_biz(s))
        .ok_or_else(|| invalid("Article contains no account identifier"))?;
    if options
        .expected_biz
        .as_deref()
        .is_some_and(|expected| expected != biz)
    {
        return Err(invalid("Article does not match expected_biz"));
    }
    Ok(account)
}

pub async fn list_history(
    engine: &TklessEngine,
    source: &str,
    options: &WechatOptions,
    history: &HistoryOptions,
    extraction: &ExtractionOptions,
) -> Result<WechatReport, TklessError> {
    options.validate()?;
    extraction.validate()?;
    if history.resume && history.checkpoint.is_none() {
        return Err(invalid("Resume requires a checkpoint file"));
    }
    let account = resolve_account(engine, source.trim(), options, extraction).await?;
    let biz = account.biz.clone().expect("validated identity");
    // Load the session before creating any checkpoint/lock, and reject path aliases.
    let session = history
        .session_file
        .as_deref()
        .map(|p| Session::read(p, &biz))
        .transpose()?;
    if let (Some(session), Some(checkpoint)) = (&history.session_file, &history.checkpoint) {
        let session = session.canonicalize()?;
        for candidate in [checkpoint.clone(), checkpoint.with_extension("lock")] {
            if candidate.exists() && candidate.canonicalize()? == session {
                return Err(invalid("Session and checkpoint paths must differ"));
            }
        }
    }
    let _lock = history
        .checkpoint
        .as_deref()
        .map(state_file::StateLock::acquire)
        .transpose()?;
    let mut state = if history.resume {
        let saved: Checkpoint =
            state_file::read(history.checkpoint.as_deref().expect("validated checkpoint"))?;
        saved.validate(&biz)?;
        saved
    } else {
        if history.checkpoint.as_ref().is_some_and(|p| p.exists()) {
            return Err(invalid(
                "Checkpoint already exists; use resume or a new path",
            ));
        }
        Checkpoint::new(account)
    };
    if !state.report.listing_complete {
        state.report.errors.clear();
        state.report.complete = false;
        if let Some(session) = session {
            collect_with(
                &mut state,
                options,
                history.checkpoint.as_deref(),
                |offset| request_page(engine, &session, offset),
            )
            .await?;
        } else {
            state.report.stop_reason = StopReason::SessionRequired;
            state.report.errors.push("A valid local session file is required. Open this account's history in WeChat and supply an authorized session; no credentials are read automatically.".into());
            state.save(history.checkpoint.as_deref())?;
        }
    }
    // Public article fetching uses the original engine, never the history credentials.
    if options.fetch_content {
        for i in 0..state.report.articles.len() {
            if state.report.articles[i].document.is_some() {
                continue;
            }
            let mut single = WechatReport::new(&biz, "account_history");
            let mut article = state.report.articles[i].clone();
            article.error = None;
            single.articles.push(article);
            super::attach_content(engine, &mut single, options, extraction).await;
            state.report.articles[i] = single.articles.remove(0);
            state.save(history.checkpoint.as_deref())?;
        }
    }
    state.report.complete = state.report.listing_complete
        && (!options.fetch_content
            || state
                .report
                .articles
                .iter()
                .all(|a| a.document.is_some() && a.error.is_none()));
    state.save(history.checkpoint.as_deref())?;
    Ok(state.report)
}

async fn request_page(
    engine: &TklessEngine,
    session: &Session,
    offset: u64,
) -> Result<Value, StopReason> {
    let mut url = Url::parse("https://mp.weixin.qq.com/mp/profile_ext").expect("static URL");
    url.query_pairs_mut()
        .append_pair("action", "getmsg")
        .append_pair("__biz", &session.biz)
        .append_pair("f", "json")
        .append_pair("offset", &offset.to_string())
        .append_pair("count", "10")
        .append_pair("scene", "124")
        .extend_pairs(&session.parameters);
    let fetcher = engine.fetcher();
    fetcher
        .validate_ssrf(&url)
        .await
        .map_err(|_| StopReason::FetchFailed)?;
    // fetch_api logs URLs at debug level. Use the bounded, guarded low-level request
    // here, with redirects disabled, and discard error text containing secret URLs.
    let mut request = fetcher.get(&url);
    if !session.cookie.is_empty() {
        request = request.header(reqwest::header::COOKIE, &session.cookie);
    }
    let response = request.send().await.map_err(|_| StopReason::FetchFailed)?;
    if matches!(response.status().as_u16(), 301..=399 | 401 | 403) {
        return Err(StopReason::SessionRequired);
    }
    if !response.status().is_success() {
        return Err(StopReason::FetchFailed);
    }
    let text = fetcher
        .read_text(response)
        .await
        .map_err(|_| StopReason::FetchFailed)?;
    serde_json::from_str(&text).map_err(|_| StopReason::SessionRequired)
}

async fn collect_with<F, Fut>(
    state: &mut Checkpoint,
    options: &WechatOptions,
    path: Option<&Path>,
    mut fetch: F,
) -> Result<(), TklessError>
where
    F: FnMut(u64) -> Fut,
    Fut: Future<Output = Result<Value, StopReason>>,
{
    let mut ids: HashSet<String> = state.report.articles.iter().map(|a| a.id.clone()).collect();
    let initial = ids.len();
    let mut page_ids = HashSet::new();
    state.report.stop_reason = StopReason::PageLimit;
    for _ in 0..options.max_pages {
        tokio::time::sleep(std::time::Duration::from_millis(options.delay_ms)).await;
        let page = match fetch(state.offset)
            .await
            .and_then(|v| parse_page(v, &state.report.accounts[0]))
        {
            Ok(page) => page,
            Err(reason) => {
                state.report.stop_reason = reason;
                state.report.errors.push(match reason {
                    StopReason::SessionRequired => "History session is missing, expired or challenged; refresh the local session file and resume.",
                    _ => "History response failed validation or could not be fetched; saved articles and offset are retained.",
                }.into());
                break;
            }
        };
        if page.more && page.next.is_none_or(|n| n <= state.offset) {
            state.report.stop_reason = StopReason::RepeatedCursor;
            state
                .report
                .errors
                .push("History cursor did not advance".into());
            break;
        }
        let signature: Vec<_> = page.articles.iter().map(|a| a.id.clone()).collect();
        if !signature.is_empty() && !page_ids.insert(signature) {
            state.report.stop_reason = StopReason::RepeatedPage;
            state
                .report
                .errors
                .push("History repeated a page despite an advancing offset".into());
            break;
        }
        state.report.pages_fetched += 1;
        let mut partial_page = false;
        for article in page.articles {
            if ids.contains(&article.id) {
                state.report.duplicates += 1;
                continue;
            }
            if ids.len() - initial >= options.limit {
                partial_page = true;
                break;
            }
            ids.insert(article.id.clone());
            state.report.articles.push(article);
        }
        if partial_page {
            // Re-read this page on resume; stable IDs suppress its already saved entries.
            state.report.stop_reason = StopReason::ItemLimit;
            state.save(path)?;
            break;
        }
        if !page.more {
            state.report.listing_complete = true;
            state.report.stop_reason = StopReason::Exhausted;
            state.save(path)?;
            break;
        }
        state.offset = page.next.expect("validated continuation");
        state.save(path)?;
        if ids.len() - initial >= options.limit {
            state.report.stop_reason = StopReason::ItemLimit;
            break;
        }
    }
    state.report.next_urls.clear();
    if !state.report.listing_complete {
        let mut next = Url::parse("https://mp.weixin.qq.com/mp/profile_ext").expect("static URL");
        next.query_pairs_mut()
            .append_pair("action", "getmsg")
            .append_pair("__biz", &state.biz)
            .append_pair("offset", &state.offset.to_string());
        state.report.next_urls.push(next.to_string());
    }
    state.save(path)
}

struct Page {
    articles: Vec<Article>,
    more: bool,
    next: Option<u64>,
}
fn number(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}
fn parse_page(value: Value, account: &Account) -> Result<Page, StopReason> {
    let ret = value
        .get("ret")
        .or_else(|| value.get("base_resp").and_then(|v| v.get("ret")));
    if ret.and_then(number) != Some(0) {
        return Err(
            if ret.is_some_and(|r| r.as_i64() == Some(-3) || r.as_str() == Some("-3")) {
                StopReason::SessionRequired
            } else {
                StopReason::InvalidPage
            },
        );
    }
    let more = match number(&value["can_msg_continue"]) {
        Some(0) => false,
        Some(1) => true,
        _ => return Err(StopReason::InvalidPage),
    };
    let list = &value["general_msg_list"];
    let decoded;
    let list = if let Some(text) = list.as_str() {
        decoded = serde_json::from_str::<Value>(text).map_err(|_| StopReason::InvalidPage)?;
        &decoded
    } else {
        list
    };
    let rows = list["list"].as_array().ok_or(StopReason::InvalidPage)?;
    if rows.len() > 100 || (rows.is_empty() && more) {
        return Err(StopReason::InvalidPage);
    }
    let mut articles = Vec::new();
    for row in rows {
        let Some(main) = row.get("app_msg_ext_info") else {
            if number(&row["comm_msg_info"]["type"]).is_some_and(|kind| kind != 49) {
                continue;
            }
            return Err(StopReason::InvalidPage);
        };
        let mut items = vec![main];
        if let Some(children) = main.get("multi_app_msg_item_list") {
            items.extend(children.as_array().ok_or(StopReason::InvalidPage)?);
        }
        if items.len() > 100 {
            return Err(StopReason::InvalidPage);
        }
        for item in items {
            let raw = item["content_url"]
                .as_str()
                .ok_or(StopReason::InvalidPage)?;
            let url = identity::public_url(raw).map_err(|_| StopReason::InvalidPage)?;
            if !identity::is_article(&url)
                || identity::parameter(&url, "__biz") != account.biz
                || !["mid", "idx"].iter().all(|key| {
                    identity::parameter(&url, key).is_some_and(|s| {
                        !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_digit())
                    })
                })
            {
                return Err(StopReason::InvalidPage);
            }
            // Strip session/tracking parameters from persisted article links.
            let mut clean = Url::parse("https://mp.weixin.qq.com/s").expect("static URL");
            for key in ["__biz", "mid", "idx", "sn"] {
                if let Some(v) = identity::parameter(&url, key) {
                    clean.query_pairs_mut().append_pair(key, &v);
                }
            }
            articles.push(Article {
                id: identity::article_id(&clean, account),
                url: clean.to_string(),
                title: item["title"]
                    .as_str()
                    .ok_or(StopReason::InvalidPage)?
                    .into(),
                account: account.clone(),
                published_at: number(&row["comm_msg_info"]["datetime"]).map(|n| n.to_string()),
                cover: None,
                document: None,
                error: None,
            });
        }
    }
    Ok(Page {
        articles,
        more,
        next: number(&value["next_offset"]),
    })
}

#[cfg(test)]
mod tests;
