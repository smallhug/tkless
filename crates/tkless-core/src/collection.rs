//! Bounded, declarative list pagination and optional detail extraction.
use crate::limits::*;
use crate::{ExtractionOptions, PageDocument, TklessEngine, TklessError};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use url::Url;

const REQUEST_METHODS: [&str; 3] = ["GET", "POST", "PUT"];
const REQUEST_VARIABLES: [&str; 4] = ["page", "offset", "cursor", "limit"];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RequestSpec {
    /// HTTP method: GET, POST or PUT.
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Body template. Allowed placeholders: {{page}}, {{offset}}, {{cursor}}, {{limit}}.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Value substituted for {{limit}}, i.e. the requested page size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u64>,
}

impl Default for RequestSpec {
    fn default() -> Self {
        Self {
            method: "GET".into(),
            content_type: None,
            headers: BTreeMap::new(),
            body: None,
            page_size: None,
        }
    }
}

impl RequestSpec {
    pub fn uses_variable(&self, name: &str) -> bool {
        self.body
            .as_deref()
            .is_some_and(|body| body.contains(&format!("{{{{{name}}}}}")))
    }

    pub fn validate(&self, pagination: &Pagination) -> Result<(), TklessError> {
        let method = self.method.trim().to_ascii_uppercase();
        if !REQUEST_METHODS.contains(&method.as_str()) {
            return Err(TklessError::InvalidArgument(
                "request.method must be GET, POST or PUT".into(),
            ));
        }
        if method == "GET" && self.body.is_some() {
            return Err(TklessError::InvalidArgument(
                "request.body requires POST or PUT".into(),
            ));
        }
        if self.content_type.is_some() && self.body.is_none() {
            return Err(TklessError::InvalidArgument(
                "request.content_type requires request.body".into(),
            ));
        }
        if let Some(content_type) = &self.content_type {
            if content_type.len() > 128 || content_type.contains(['\r', '\n']) {
                return Err(TklessError::InvalidArgument(
                    "Invalid request.content_type".into(),
                ));
            }
        }
        if self.headers.len() > COLLECTION_MAX_REQUEST_HEADERS {
            return Err(TklessError::InvalidArgument(format!(
                "request.headers supports at most {COLLECTION_MAX_REQUEST_HEADERS} entries"
            )));
        }
        for (name, value) in &self.headers {
            if name.is_empty()
                || name.len() > 128
                || name.contains(['\r', '\n', ' '])
                || value.len() > COLLECTION_MAX_REQUEST_HEADER_BYTES
                || value.contains(['\r', '\n'])
            {
                return Err(TklessError::InvalidArgument(
                    "Invalid request header name or value".into(),
                ));
            }
            if crate::fetcher::is_sensitive_header(name) {
                return Err(TklessError::InvalidArgument(
                    "Request templates must not embed credentials; use --cookie or a cookie file"
                        .into(),
                ));
            }
        }
        if let Some(page_size) = self.page_size {
            if !(1..=COLLECTION_MAX_PAGE_SIZE).contains(&page_size) {
                return Err(TklessError::InvalidArgument(format!(
                    "request.page_size must be 1..{COLLECTION_MAX_PAGE_SIZE}"
                )));
            }
        }
        let Some(body) = &self.body else {
            return Ok(());
        };
        if body.len() > COLLECTION_MAX_REQUEST_BYTES {
            return Err(TklessError::InvalidArgument(format!(
                "request.body exceeds {COLLECTION_MAX_REQUEST_BYTES} bytes"
            )));
        }
        let variables = request_variables(body)?;
        if variables.contains(&"page") && !matches!(pagination, Pagination::PageNumber { .. }) {
            return Err(TklessError::InvalidArgument(
                "{{page}} requires page_number pagination".into(),
            ));
        }
        if variables.contains(&"cursor") && !matches!(pagination, Pagination::Cursor { .. }) {
            return Err(TklessError::InvalidArgument(
                "{{cursor}} requires cursor pagination".into(),
            ));
        }
        Ok(())
    }

    fn expand(&self, variables: &BTreeMap<&str, String>) -> Result<ExpandedRequest, TklessError> {
        let body = match &self.body {
            Some(body) => {
                let mut expanded = body.clone();
                for (name, value) in variables {
                    expanded = expanded.replace(&format!("{{{{{name}}}}}"), value);
                }
                if expanded.len() > COLLECTION_MAX_REQUEST_BYTES {
                    return Err(TklessError::InvalidArgument(format!(
                        "Expanded request body exceeds {COLLECTION_MAX_REQUEST_BYTES} bytes"
                    )));
                }
                Some(expanded)
            }
            None => None,
        };
        Ok(ExpandedRequest {
            method: self.method.trim().to_ascii_uppercase(),
            content_type: self.content_type.clone(),
            headers: self
                .headers
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            body,
        })
    }
}

pub(crate) struct ExpandedRequest {
    method: String,
    content_type: Option<String>,
    headers: Vec<(String, String)>,
    body: Option<String>,
}

fn request_variables(body: &str) -> Result<Vec<&str>, TklessError> {
    let mut found = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or_else(|| {
            TklessError::InvalidArgument("Unclosed request template placeholder".into())
        })?;
        let name = after[..end].trim();
        if !REQUEST_VARIABLES.contains(&name) {
            return Err(TklessError::InvalidArgument(format!(
                "Unsupported request template variable: {name}"
            )));
        }
        found.push(name);
        rest = &after[end + 2..];
    }
    Ok(found)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Pagination {
    NextLink {
        selector: String,
    },
    PageNumber {
        parameter: String,
        start: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        empty_selector: Option<String>,
    },
    Cursor {
        parameter: String,
        items_pointer: String,
        next_pointer: String,
    },
    None,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetailRule {
    /// JSON Pointer into each list item, e.g. /url.
    pub url_pointer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browser: Option<crate::browser_workflow::BrowserPlan>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CollectionConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browser: Option<crate::browser_workflow::BrowserPlan>,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    pub pagination: Pagination,
    /// Optional templated HTTP request used for every list page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<RequestSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<DetailRule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id_pointer: Option<String>,
    pub max_pages: usize,
    pub max_items: usize,
    pub allow_cross_origin: bool,
    pub delay_ms: u64,
}

impl Default for CollectionConfig {
    fn default() -> Self {
        Self {
            browser: None,
            url: String::new(),
            schema: None,
            pagination: Pagination::None,
            request: None,
            details: None,
            id_pointer: None,
            max_pages: COLLECTION_DEFAULT_PAGES,
            max_items: COLLECTION_DEFAULT_ITEMS,
            allow_cross_origin: false,
            delay_ms: 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollectionRecord {
    pub id: String,
    pub source_url: String,
    pub data: Value,
    pub detail: Option<PageDocument>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollectionReport {
    pub url: String,
    pub records: Vec<CollectionRecord>,
    pub pages_fetched: usize,
    pub duplicates: usize,
    pub listing_complete: bool,
    pub complete: bool,
    pub stop_reason: crate::workflow::StopReason,
    pub next_url: Option<String>,
    pub errors: Vec<String>,
}

pub(crate) fn digest(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn canonical_url(raw: &str) -> Result<Url, TklessError> {
    let mut url = Url::parse(raw)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(TklessError::InvalidArgument(
            "Expected an HTTP(S) URL without embedded credentials".into(),
        ));
    }
    url.set_fragment(None);
    Ok(url)
}

fn selector(raw: &str) -> Result<Selector, TklessError> {
    Selector::parse(raw)
        .map_err(|_| TklessError::InvalidArgument(format!("Invalid CSS selector: {raw}")))
}

impl CollectionConfig {
    pub fn validate(&self) -> Result<(), TklessError> {
        canonical_url(&self.url)?;
        if let Some(browser) = &self.browser {
            browser.validate()?;
            if matches!(self.pagination, Pagination::Cursor { .. }) {
                return Err(TklessError::InvalidArgument(
                    "Cursor JSON collection uses HTTP, not browser rendering".into(),
                ));
            }
        }
        if !(1..=COLLECTION_MAX_PAGES).contains(&self.max_pages)
            || !(1..=COLLECTION_MAX_ITEMS).contains(&self.max_items)
            || self.delay_ms > COLLECTION_MAX_DELAY_MS
        {
            return Err(TklessError::InvalidArgument(
                "Collection bounds: pages 1..1000, items 1..100000, delay_ms 0..60000".into(),
            ));
        }
        if let Some(request) = &self.request {
            if self.browser.is_some() {
                return Err(TklessError::InvalidArgument(
                    "Browser rendering does not support templated requests".into(),
                ));
            }
            request.validate(&self.pagination)?;
        }
        match &self.pagination {
            Pagination::NextLink { selector: s } => {
                selector(s)?;
            }
            Pagination::PageNumber { parameter, .. } | Pagination::Cursor { parameter, .. }
                if parameter.is_empty() =>
            {
                return Err(TklessError::InvalidArgument(
                    "Pagination parameter cannot be empty".into(),
                ));
            }
            _ => {}
        }
        if let Pagination::PageNumber {
            empty_selector: Some(s),
            ..
        } = &self.pagination
        {
            selector(s)?;
        }
        if !matches!(self.pagination, Pagination::Cursor { .. }) {
            let schema = self.schema.clone().ok_or_else(|| {
                TklessError::InvalidArgument("HTML collection requires a schema".into())
            })?;
            let schema = crate::schema::ExtractionSchema::from_json_value(schema)?;
            if schema.items_selector.is_none() {
                return Err(TklessError::InvalidArgument(
                    "Collection schema requires items_selector".into(),
                ));
            }
            selector(schema.items_selector.as_deref().unwrap())?;
        }
        if let Some(rule) = &self.details {
            if let Some(browser) = &rule.browser {
                browser.validate()?;
            }
            if !rule.url_pointer.starts_with('/') {
                return Err(TklessError::InvalidArgument(
                    "details.url_pointer must be a JSON Pointer".into(),
                ));
            }
            if let Some(schema) = &rule.schema {
                crate::schema::ExtractionSchema::from_json_value(schema.clone())?;
            }
        }
        if let Pagination::Cursor {
            items_pointer,
            next_pointer,
            ..
        } = &self.pagination
        {
            if !items_pointer.starts_with('/') || !next_pointer.starts_with('/') {
                return Err(TklessError::InvalidArgument(
                    "Cursor fields must be JSON Pointers".into(),
                ));
            }
        }
        if self
            .id_pointer
            .as_ref()
            .is_some_and(|p| !p.starts_with('/'))
        {
            return Err(TklessError::InvalidArgument(
                "id_pointer must be a JSON Pointer".into(),
            ));
        }
        Ok(())
    }
}

fn with_parameter(url: &Url, parameter: &str, value: &str) -> Url {
    let mut next = url.clone();
    let pairs: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| key != parameter)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    next.set_query(None);
    next.query_pairs_mut()
        .extend_pairs(pairs)
        .append_pair(parameter, value);
    next
}

pub fn config_schema() -> Value {
    serde_json::json!({"type":"object","required":["url"],"additionalProperties":false,"properties":{
        "url":{"type":"string","minLength":1},"schema":{"type":"object","description":"HTML ExtractionSchema with items_selector and fields"},
        "id_pointer":{"type":"string","description":"JSON Pointer to stable record ID, e.g. /url or /id"},
        "max_pages":{"type":"integer","minimum":1,"maximum":COLLECTION_MAX_PAGES,"default":COLLECTION_DEFAULT_PAGES},"max_items":{"type":"integer","minimum":1,"maximum":COLLECTION_MAX_ITEMS,"default":COLLECTION_DEFAULT_ITEMS},
        "delay_ms":{"type":"integer","minimum":0,"maximum":COLLECTION_MAX_DELAY_MS,"default":0},"allow_cross_origin":{"type":"boolean","default":false},
        "browser":crate::browser_workflow::plan_schema(),
        "request":{"type":"object","additionalProperties":false,"properties":{
            "method":{"type":"string","maxLength":16,"description":"GET, POST or PUT (case-insensitive)"},
            "content_type":{"type":"string","maxLength":128},
            "headers":{"type":"object"},
            "body":{"type":"string","maxLength":COLLECTION_MAX_REQUEST_BYTES},
            "page_size":{"type":"integer","minimum":1,"maximum":COLLECTION_MAX_PAGE_SIZE,"default":COLLECTION_DEFAULT_PAGE_SIZE}
        }},
        "details":{"type":"object","required":["url_pointer"],"additionalProperties":false,"properties":{"url_pointer":{"type":"string"},"schema":{"type":"object"},"browser":crate::browser_workflow::plan_schema()}},
        "pagination":{"anyOf":[
            {"type":"object","required":["kind"],"additionalProperties":false,"properties":{"kind":{"const":"none"}}},
            {"type":"object","required":["kind","selector"],"additionalProperties":false,"properties":{"kind":{"const":"next_link"},"selector":{"type":"string"}}},
            {"type":"object","required":["kind","parameter","start"],"additionalProperties":false,"properties":{"kind":{"const":"page_number"},"parameter":{"type":"string","minLength":1},"start":{"type":"integer","minimum":0},"empty_selector":{"type":"string"}}},
            {"type":"object","required":["kind","parameter","items_pointer","next_pointer"],"additionalProperties":false,"properties":{"kind":{"const":"cursor"},"parameter":{"type":"string","minLength":1},"items_pointer":{"type":"string"},"next_pointer":{"type":"string"}}}
        ]}
    }})
}

/// A partial report is explicit; only a successfully exhausted list can drive deletions in sync.
pub async fn collect_pages(
    engine: &TklessEngine,
    config: &CollectionConfig,
    options: &ExtractionOptions,
) -> Result<CollectionReport, TklessError> {
    config.validate()?;
    let seed = canonical_url(&config.url)?;
    let body_has_page = config
        .request
        .as_ref()
        .is_some_and(|request| request.uses_variable("page"));
    let mut current = match &config.pagination {
        Pagination::PageNumber {
            parameter, start, ..
        } if !body_has_page => with_parameter(&seed, parameter, &start.to_string()),
        _ => seed.clone(),
    };
    let mut cursor_token: Option<String> = None;
    let page_size = config
        .request
        .as_ref()
        .and_then(|request| request.page_size)
        .unwrap_or(COLLECTION_DEFAULT_PAGE_SIZE);
    let schema = config
        .schema
        .clone()
        .map(crate::schema::ExtractionSchema::from_json_value)
        .transpose()?;
    let mut report = CollectionReport {
        url: seed.to_string(),
        records: vec![],
        pages_fetched: 0,
        duplicates: 0,
        listing_complete: false,
        complete: false,
        stop_reason: crate::workflow::StopReason::PageLimit,
        next_url: Some(current.to_string()),
        errors: vec![],
    };
    let (mut requests, mut pages, mut ids) = (HashSet::new(), HashSet::new(), HashSet::new());
    for page_index in 0..config.max_pages {
        if !config.allow_cross_origin && current.origin() != seed.origin() {
            report.stop_reason = crate::workflow::StopReason::OriginBoundary;
            break;
        }
        if page_index > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(config.delay_ms)).await;
        }
        let request = match &config.request {
            Some(spec) => {
                let mut variables: BTreeMap<&str, String> = BTreeMap::new();
                variables.insert("offset", page_index.to_string());
                variables.insert("limit", page_size.to_string());
                variables.insert(
                    "page",
                    match &config.pagination {
                        Pagination::PageNumber { start, .. } => start
                            .checked_add(page_index as u64)
                            .ok_or_else(|| {
                                TklessError::InvalidArgument("Page number overflow".into())
                            })?
                            .to_string(),
                        _ => (page_index as u64 + 1).to_string(),
                    },
                );
                variables.insert("cursor", cursor_token.clone().unwrap_or_default());
                Some(spec.expand(&variables)?)
            }
            None => None,
        };
        let signature = format!(
            "{}\n{}",
            current,
            request
                .as_ref()
                .and_then(|request| request.body.as_deref())
                .unwrap_or_default()
        );
        if !requests.insert(signature) {
            report.stop_reason = crate::workflow::StopReason::RepeatedUrl;
            break;
        }
        let response = match fetch_page(
            engine,
            current.as_str(),
            config.browser.as_ref(),
            request.as_ref(),
        )
        .await
        {
            Ok(response) if !response.truncated && (200..300).contains(&response.status) => {
                response
            }
            Ok(_) => {
                report.stop_reason = crate::workflow::StopReason::PageError;
                report
                    .errors
                    .push("Incomplete or unsuccessful list response".into());
                break;
            }
            Err(error) if page_index == 0 => return Err(error),
            Err(error) => {
                report.stop_reason = crate::workflow::StopReason::PageError;
                report.errors.push(error.to_string());
                break;
            }
        };
        report.pages_fetched += 1;
        let page_url = canonical_url(&response.final_url)?;
        if !config.allow_cross_origin && page_url.origin() != seed.origin() {
            report.stop_reason = crate::workflow::StopReason::OriginBoundary;
            break;
        }
        let parsed = parse_page(
            &response.html,
            &page_url,
            &seed,
            config,
            schema.as_ref(),
            page_index,
        );
        let (items, next) = match parsed {
            Ok(value) => value,
            Err(error) if page_index == 0 => return Err(error),
            Err(error) => {
                report.stop_reason = crate::workflow::StopReason::ParseError;
                report.errors.push(error.to_string());
                break;
            }
        };
        // Compare the item payload, not navigation markup which changes with page numbers.
        let page_hash = digest(serde_json::to_vec(&items)?);
        if !pages.insert(page_hash) {
            report.stop_reason = crate::workflow::StopReason::RepeatedPage;
            break;
        }
        if items.is_empty() {
            report.stop_reason = crate::workflow::StopReason::EmptyPage;
            report.listing_complete = next.is_none();
            report.next_url = Some(current.to_string());
            break;
        }
        for data in items {
            if report.records.len() >= config.max_items {
                report.stop_reason = crate::workflow::StopReason::ItemLimit;
                break;
            }
            let detail_url = config.details.as_ref().map(|rule| {
                let raw = data
                    .pointer(&rule.url_pointer)
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        TklessError::InvalidArgument("Missing detail URL in list item".into())
                    })?;
                let url = canonical_url(page_url.join(raw)?.as_str())?;
                if !config.allow_cross_origin && url.origin() != seed.origin() {
                    return Err(TklessError::InvalidArgument(
                        "Detail URL crosses configured origin".into(),
                    ));
                }
                Ok::<_, TklessError>(url)
            });
            let identity = if let Some(pointer) = &config.id_pointer {
                match data
                    .pointer(pointer)
                    .filter(|v| v.is_string() || v.is_number())
                {
                    Some(value) => value.to_string(),
                    None => {
                        return Err(TklessError::InvalidArgument(format!(
                            "Missing stable item ID: {pointer}"
                        )))
                    }
                }
            } else if let Some(Ok(url)) = &detail_url {
                url.to_string()
            } else {
                serde_json::to_string(&data)?
            };
            let id = digest(identity);
            if !ids.insert(id.clone()) {
                report.duplicates += 1;
                continue;
            }
            let mut record = CollectionRecord {
                id,
                source_url: page_url.to_string(),
                data,
                detail: None,
                error: None,
            };
            if let Some(detail_url) = detail_url {
                let result = match detail_url {
                    Ok(url) => {
                        tokio::time::sleep(std::time::Duration::from_millis(config.delay_ms)).await;
                        let mut opts = options.clone();
                        if let Some(schema) =
                            config.details.as_ref().and_then(|r| r.schema.as_ref())
                        {
                            opts.schema = Some(schema.to_string());
                            opts.schema_strict = true;
                        }
                        match fetch_page(
                            engine,
                            url.as_str(),
                            config.details.as_ref().and_then(|r| r.browser.as_ref()),
                            None,
                        )
                        .await
                        {
                            Ok(raw) if !raw.truncated => engine
                                .process_html(&raw.html, Some(&raw.final_url), &opts)
                                .map(|mut doc| {
                                    doc.url = url.to_string();
                                    doc.final_url = raw.final_url;
                                    doc.status = raw.status;
                                    doc.extraction.transport = if config
                                        .details
                                        .as_ref()
                                        .and_then(|r| r.browser.as_ref())
                                        .is_some()
                                    {
                                        "cdp"
                                    } else {
                                        "http"
                                    }
                                    .into();
                                    doc
                                }),
                            Ok(_) => Err(TklessError::InvalidArgument(
                                "Truncated detail response".into(),
                            )),
                            Err(error) => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                };
                match result {
                    Ok(doc) if !doc.truncated && (200..300).contains(&doc.status) => {
                        record.detail = Some(doc)
                    }
                    Ok(doc) => {
                        record.error = Some("Detail response incomplete or unsuccessful".into());
                        record.detail = Some(doc);
                    }
                    Err(error) => record.error = Some(error.to_string()),
                }
            }
            report.records.push(record);
        }
        if report.stop_reason == crate::workflow::StopReason::ItemLimit {
            report.next_url = Some(page_url.to_string());
            break;
        }
        report.next_url = Some(current.to_string());
        match next {
            Some(next) => {
                if let Some(url) = next.url {
                    current = url;
                }
                cursor_token = next.cursor;
                report.next_url = Some(current.to_string());
            }
            None => {
                report.listing_complete = true;
                report.stop_reason = crate::workflow::StopReason::Exhausted;
                break;
            }
        }
    }
    report.complete = report.listing_complete
        && report.errors.is_empty()
        && report.records.iter().all(|r| r.error.is_none());
    Ok(report)
}

async fn fetch_page(
    engine: &TklessEngine,
    url: &str,
    browser: Option<&crate::browser_workflow::BrowserPlan>,
    request: Option<&ExpandedRequest>,
) -> Result<crate::fetcher::FetchedResponse, TklessError> {
    match browser {
        None => match request {
            None => engine.fetcher().fetch_api(url).await,
            Some(request) => {
                engine
                    .fetcher()
                    .send_api_request(
                        url,
                        &request.method,
                        request.content_type.as_deref(),
                        &request.headers,
                        request.body.as_deref(),
                    )
                    .await
            }
        },
        Some(plan) => {
            let page = crate::browser_workflow::render(engine, url, plan).await?;
            Ok(crate::fetcher::FetchedResponse {
                requested_url: url.into(),
                final_url: page.final_url,
                content_type: None,
                status: 200,
                html: page.html,
                truncated: false,
                etag: None,
                last_modified: None,
                not_modified: false,
                raw_bytes: None,
            })
        }
    }
}

fn parse_page(
    html: &str,
    url: &Url,
    seed: &Url,
    config: &CollectionConfig,
    schema: Option<&crate::schema::ExtractionSchema>,
    index: usize,
) -> Result<(Vec<Value>, Option<NextPage>), TklessError> {
    if let Pagination::Cursor {
        parameter,
        items_pointer,
        next_pointer,
    } = &config.pagination
    {
        let body: Value = serde_json::from_str(html)
            .map_err(|e| TklessError::InvalidArgument(format!("Invalid pagination JSON: {e}")))?;
        let items = body
            .pointer(items_pointer)
            .and_then(Value::as_array)
            .ok_or_else(|| {
                TklessError::InvalidArgument(
                    "Cursor items_pointer did not resolve to an array".into(),
                )
            })?
            .clone();
        let token = body.pointer(next_pointer).ok_or_else(|| {
            TklessError::InvalidArgument("Missing next cursor; use null to mark exhaustion".into())
        })?;
        let next = match token {
            Value::Null => None,
            Value::String(s) if s.is_empty() => None,
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => {
                return Err(TklessError::InvalidArgument(
                    "Next cursor must be a string, number or null".into(),
                ))
            }
        };
        let body_has_cursor = config
            .request
            .as_ref()
            .is_some_and(|request| request.uses_variable("cursor"));
        let next = next.map(|token| NextPage {
            url: if body_has_cursor {
                None
            } else {
                Some(with_parameter(seed, parameter, &token))
            },
            cursor: Some(token),
        });
        return Ok((items, next));
    }
    let schema =
        schema.ok_or_else(|| TklessError::InvalidArgument("Missing HTML schema".into()))?;
    if let Pagination::PageNumber {
        empty_selector: Some(s),
        ..
    } = &config.pagination
    {
        if Html::parse_document(html)
            .select(&selector(s)?)
            .next()
            .is_some()
        {
            return Ok((vec![], None));
        }
    }
    let value = crate::schema::extract_entities(html, schema, Some(url.as_str()))?;
    let items = value
        .as_array()
        .ok_or_else(|| TklessError::InvalidArgument("List schema must yield an array".into()))?
        .clone();
    let next = match &config.pagination {
        Pagination::NextLink { selector: s } => {
            let doc = Html::parse_document(html);
            let sel = selector(s)?;
            match doc.select(&sel).next() {
                Some(element) => {
                    let href = element.value().attr("href").ok_or_else(|| {
                        TklessError::InvalidArgument("Next-page element has no href".into())
                    })?;
                    Some(NextPage {
                        url: Some(canonical_url(url.join(href)?.as_str())?),
                        cursor: None,
                    })
                }
                None => None,
            }
        }
        Pagination::PageNumber {
            parameter, start, ..
        } => {
            let number = start
                .checked_add(index as u64 + 1)
                .ok_or_else(|| TklessError::InvalidArgument("Page number overflow".into()))?;
            let body_has_page = config
                .request
                .as_ref()
                .is_some_and(|request| request.uses_variable("page"));
            Some(NextPage {
                url: if body_has_page {
                    None
                } else {
                    Some(with_parameter(seed, parameter, &number.to_string()))
                },
                cursor: None,
            })
        }
        _ => None,
    };
    Ok((items, next))
}

struct NextPage {
    url: Option<Url>,
    cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor_pagination() -> Pagination {
        Pagination::Cursor {
            parameter: "cursor".into(),
            items_pointer: "/items".into(),
            next_pointer: "/next".into(),
        }
    }

    fn config_with(pagination: Pagination, request: RequestSpec) -> CollectionConfig {
        CollectionConfig {
            url: "https://example.com/list".into(),
            pagination,
            request: Some(request),
            ..Default::default()
        }
    }

    #[test]
    fn rejects_unsafe_request_templates() {
        let unknown = RequestSpec {
            method: "POST".into(),
            body: Some("{\"q\":\"{{unknown}}\"}".into()),
            ..Default::default()
        };
        assert!(config_with(cursor_pagination(), unknown)
            .validate()
            .is_err());

        let get_with_body = RequestSpec {
            method: "GET".into(),
            body: Some("{}".into()),
            ..Default::default()
        };
        assert!(config_with(cursor_pagination(), get_with_body)
            .validate()
            .is_err());

        let mut headers = BTreeMap::new();
        headers.insert("Authorization".into(), "Bearer token".into());
        let credential_header = RequestSpec {
            method: "POST".into(),
            body: Some("{}".into()),
            headers,
            ..Default::default()
        };
        assert!(config_with(cursor_pagination(), credential_header)
            .validate()
            .is_err());

        let oversized = RequestSpec {
            method: "POST".into(),
            body: Some("x".repeat(COLLECTION_MAX_REQUEST_BYTES + 1)),
            ..Default::default()
        };
        assert!(config_with(cursor_pagination(), oversized)
            .validate()
            .is_err());

        let unsupported_method = RequestSpec {
            method: "DELETE".into(),
            ..Default::default()
        };
        assert!(config_with(cursor_pagination(), unsupported_method)
            .validate()
            .is_err());
    }

    #[test]
    fn template_variables_must_match_pagination() {
        let page_body = RequestSpec {
            method: "POST".into(),
            body: Some("{\"page\":{{page}}}".into()),
            ..Default::default()
        };
        assert!(config_with(cursor_pagination(), page_body)
            .validate()
            .is_err());

        let cursor_body = RequestSpec {
            method: "POST".into(),
            body: Some("{\"cursor\":\"{{cursor}}\"}".into()),
            ..Default::default()
        };
        let page_number = Pagination::PageNumber {
            parameter: "page".into(),
            start: 1,
            empty_selector: None,
        };
        assert!(config_with(page_number, cursor_body).validate().is_err());
    }

    #[test]
    fn expands_bounded_templates() {
        let spec = RequestSpec {
            method: "post".into(),
            content_type: Some("application/json".into()),
            body: Some("{\"cursor\":\"{{cursor}}\",\"size\":{{limit}}}".into()),
            page_size: Some(25),
            ..Default::default()
        };
        let mut variables = BTreeMap::new();
        variables.insert("cursor", "a/b".to_string());
        variables.insert("limit", "25".to_string());
        let expanded = spec.expand(&variables).unwrap();
        assert_eq!(expanded.method, "POST");
        assert_eq!(
            expanded.body.as_deref(),
            Some("{\"cursor\":\"a/b\",\"size\":25}")
        );
    }
}
