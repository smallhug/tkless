use crate::error::TklessError;
use crate::model::{ExtractionOptions, SearchEngineKind};
use crate::TklessEngine;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use tracing::{debug, warn};

// Bing Selectors
static BING_ITEM_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("li.b_algo, .b_algo").unwrap());
static BING_TITLE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("h2 a, .b_title a").unwrap());
static BING_SNIPPET_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".b_caption p, .b_algoSlug, .b_snippet, .b_lineclamp2, .b_lineclamp3, .b_lineclamp4, .b_paractl").unwrap()
});
static BING_NEXT_PAGE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"href="(/search\?[^"]*first=11[^"]*)""#).unwrap());

// DuckDuckGo Selectors
static DDG_ITEM_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("div.results_links, div.result, div.web-result").unwrap());
static DDG_TITLE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a.result__a").unwrap());
static DDG_SNIPPET_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a.result__snippet, .result__snippet").unwrap());

// Brave Search Selectors
static BRAVE_ITEM_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("div.snippet[data-type='web'], div#results div.snippet, div.snippet, div.fdb")
        .unwrap()
});
static BRAVE_TITLE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("a.heading-serpresult, .title, a.h4, .snippet-title a, h2 a").unwrap()
});
static BRAVE_SNIPPET_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".snippet-description, .snippet-content, .generic-desc, p.snippet-description")
        .unwrap()
});

// Yandex Selectors
static YANDEX_ITEM_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("li.serp-item, .serp-list .serp-item, div.serp-item").unwrap()
});
static YANDEX_TITLE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("h2.OrganicTitle a, a.OrganicTitle-Link, h2 a, .organic__url").unwrap()
});
static YANDEX_SNIPPET_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".OrganicText, .organic__text, .extended-text, .text-container").unwrap()
});

// Google Selectors
static GOOGLE_ITEM_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("div.g, div.tF2Cxc, div.MjjYud, div[data-hveid], div.Ww4FFb").unwrap()
});
static GOOGLE_TITLE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("h3, a > h3, a.DKV0Md, div[role='heading']").unwrap());
static GOOGLE_LINK_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a:has(h3), a[href]").unwrap());
static GOOGLE_SNIPPET_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(
        "div.VwiC3b, div[data-sncf], .yXK7lf, span.aCOpRe, div[style*='-webkit-line-clamp']",
    )
    .unwrap()
});

/// Structured web search result item with provider attribution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchResultItem {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub engine: String,
}

/// Detailed search result containing concurrently fetched and cleaned page content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchFetchItem {
    pub rank: usize,
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub engine: String,
    pub content: Option<String>,
    pub token_count: usize,
    pub status: u16,
    pub error: Option<String>,
}

/// Executes an all-in-one search & fetch pipeline:
/// 1. Searches Bing / vertical pipelines for the query to get top URLs
/// 2. Concurrently fetches and extracts clean Markdown content for each page via TklessEngine
/// 3. Enforces per-page token budgets to prevent LLM context explosion
pub async fn web_search_and_fetch(
    engine: &TklessEngine,
    query: &str,
    limit: usize,
    search_engine: SearchEngineKind,
    base_options: &ExtractionOptions,
    max_tokens_per_page: Option<usize>,
    dedup: bool,
) -> Result<Vec<SearchFetchItem>, TklessError> {
    web_search_and_fetch_scoped(
        engine,
        query,
        None,
        limit,
        search_engine,
        base_options,
        max_tokens_per_page,
        dedup,
    )
    .await
}

/// Executes an all-in-one search & fetch pipeline with platform scope awareness.
#[allow(clippy::too_many_arguments)]
pub async fn web_search_and_fetch_scoped(
    engine: &TklessEngine,
    query: &str,
    scope: Option<&str>,
    limit: usize,
    search_engine: SearchEngineKind,
    base_options: &ExtractionOptions,
    max_tokens_per_page: Option<usize>,
    dedup: bool,
) -> Result<Vec<SearchFetchItem>, TklessError> {
    let clean_query = query.trim();
    if clean_query.is_empty() {
        return Ok(Vec::new());
    }

    let cdp_opts = base_options
        .cdp_endpoint
        .as_ref()
        .map(|ep| crate::cdp::CdpOptions {
            endpoint: ep.clone(),
            wait_ms: base_options.cdp_wait_ms,
            timeout_ms: None,
            ..Default::default()
        });

    // 1. Search phase using scoped vertical routing
    let search_results = web_search_scoped(
        engine.client(),
        clean_query,
        scope,
        limit,
        search_engine,
        cdp_opts.as_ref(),
        base_options.auto_cdp,
    )
    .await?;
    if search_results.is_empty() {
        return Ok(Vec::new());
    }

    debug!(
        query = clean_query,
        count = search_results.len(),
        "Concurrently fetching search result contents"
    );

    // 2. Parallel fetch phase
    let per_page_tokens = max_tokens_per_page.unwrap_or(1500);
    let mut page_options = base_options.clone();
    page_options.max_tokens = Some(per_page_tokens);

    // Pre-filter duplicate URLs before parallel network fetching
    let mut seen_fetch_urls = std::collections::HashSet::new();
    let mut unique_results = Vec::new();
    for item in search_results {
        let norm = item.url.trim().to_lowercase();
        if seen_fetch_urls.insert(norm) {
            unique_results.push(item);
        }
    }

    let mut join_set = tokio::task::JoinSet::new();

    for (idx, item) in unique_results.into_iter().enumerate() {
        let rank = idx + 1;
        let item_url = item.url.clone();
        let item_title = item.title.clone();
        let item_snippet = item.snippet.clone();
        let item_engine = item.engine.clone();
        let opts = page_options.clone();
        let eng = engine.clone();

        join_set.spawn(async move {
            match eng.process_url(&item_url, &opts).await {
                Ok(doc) => SearchFetchItem {
                    rank,
                    title: if item_title.is_empty() {
                        doc.title.unwrap_or_else(|| "网页内容".into())
                    } else {
                        item_title
                    },
                    url: item_url,
                    snippet: item_snippet,
                    engine: item_engine,
                    content: Some(doc.content),
                    token_count: doc.token_count,
                    status: doc.status,
                    error: None,
                },
                Err(err) => {
                    warn!(url = %item_url, error = %err, "Failed to fetch search result item content");
                    SearchFetchItem {
                        rank,
                        title: item_title,
                        url: item_url,
                        snippet: item_snippet,
                        engine: item_engine,
                        content: None,
                        token_count: 0,
                        status: 0,
                        error: Some(err.to_string()),
                    }
                }
            }
        });
    }

    let mut fetched_items = Vec::new();
    while let Some(res) = join_set.join_next().await {
        if let Ok(item) = res {
            fetched_items.push(item);
        }
    }

    // Sort by original search rank to preserve ranking order
    fetched_items.sort_by_key(|item| item.rank);

    if dedup {
        let mut seen_simhashes: Vec<u64> = Vec::new();
        let mut deduplicated = Vec::new();
        for item in fetched_items {
            if let Some(ref text) = item.content {
                let h = crate::dedup::simhash(text);
                if seen_simhashes
                    .iter()
                    .any(|&prev| crate::dedup::is_near_duplicate(prev, h, 3))
                {
                    debug!(url = %item.url, "Filtered out near-duplicate search result via SimHash");
                    continue;
                }
                seen_simhashes.push(h);
            }
            deduplicated.push(item);
        }
        return Ok(deduplicated);
    }

    Ok(fetched_items)
}

/// Executes a web search using the specified engine (Bing) with default options.
pub async fn web_search_with_engine(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
    engine: SearchEngineKind,
) -> Result<Vec<SearchResultItem>, TklessError> {
    web_search_scoped(client, query, None, limit, engine, None, false).await
}

/// Executes a web search with optional CDP browser fallback to bypass anti-scraping challenges.
pub async fn web_search_with_options(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
    engine: SearchEngineKind,
    cdp_options: Option<&crate::cdp::CdpOptions>,
    auto_cdp: bool,
) -> Result<Vec<SearchResultItem>, TklessError> {
    web_search_scoped(client, query, None, limit, engine, cdp_options, auto_cdp).await
}

/// Executes a scoped web search with intelligent routing to dedicated vertical pipelines
/// (e.g. Baidu Baike native API, Sogou WeChat article search with redirect decryption)
/// before falling back to general web search.
pub async fn web_search_scoped(
    client: &reqwest::Client,
    query: &str,
    scope: Option<&str>,
    limit: usize,
    engine: SearchEngineKind,
    cdp_options: Option<&crate::cdp::CdpOptions>,
    auto_cdp: bool,
) -> Result<Vec<SearchResultItem>, TklessError> {
    let clean_query = query.trim();
    if clean_query.is_empty() {
        return Ok(Vec::new());
    }

    // 1. Dedicated Baidu Baike vertical search pipeline
    let is_baike_scope = scope == Some("baike") || clean_query.contains("site:baike.baidu.com");
    if is_baike_scope {
        let stripped_q = clean_query
            .replace("site:baike.baidu.com", "")
            .trim()
            .to_string();
        debug!(query = %stripped_q, "Routing search through dedicated Baidu Baike vertical pipeline");
        let baike_results = search_baike(client, &stripped_q, limit).await;
        if let Ok(ref results) = baike_results {
            if !results.is_empty() {
                return baike_results;
            }
        }
    }

    // 2. Dedicated WeChat Official Accounts vertical search pipeline
    let is_wechat_scope = scope == Some("wechat") || clean_query.contains("site:mp.weixin.qq.com");
    if is_wechat_scope {
        let stripped_q = clean_query
            .replace("site:mp.weixin.qq.com", "")
            .trim()
            .to_string();
        debug!(query = %stripped_q, "Routing search through dedicated WeChat Official Accounts pipeline");
        let wechat_results = search_wechat_sogou(client, &stripped_q, limit).await;
        if let Ok(ref results) = wechat_results {
            if !results.is_empty() {
                return wechat_results;
            }
        }
    }

    // 3. General web search
    match engine {
        SearchEngineKind::All => {
            let bing_fut =
                search_bing_with_options(client, clean_query, limit, cdp_options, auto_cdp);
            let ddg_fut =
                search_duckduckgo_with_options(client, clean_query, limit, cdp_options, auto_cdp);
            let brave_fut =
                search_brave_with_options(client, clean_query, limit, cdp_options, auto_cdp);

            let (bing_res, ddg_res, brave_res) = tokio::join!(bing_fut, ddg_fut, brave_fut);

            let bing_items = bing_res.unwrap_or_else(|e| {
                debug!(error = %e, "Bing search failed during multi-engine search");
                Vec::new()
            });
            let ddg_items = ddg_res.unwrap_or_else(|e| {
                debug!(error = %e, "DuckDuckGo search failed during multi-engine search");
                Vec::new()
            });
            let brave_items = brave_res.unwrap_or_else(|e| {
                debug!(error = %e, "Brave search failed during multi-engine search");
                Vec::new()
            });

            if bing_items.is_empty() && ddg_items.is_empty() && brave_items.is_empty() {
                return Ok(Vec::new());
            }

            Ok(merge_multi_engine_search_results(
                vec![bing_items, ddg_items, brave_items],
                limit,
            ))
        }
        SearchEngineKind::Bing => {
            search_bing_with_options(client, clean_query, limit, cdp_options, auto_cdp).await
        }
        SearchEngineKind::DuckDuckGo => {
            search_duckduckgo_with_options(client, clean_query, limit, cdp_options, auto_cdp).await
        }
        SearchEngineKind::Brave => {
            search_brave_with_options(client, clean_query, limit, cdp_options, auto_cdp).await
        }
        SearchEngineKind::Yandex => {
            search_yandex_with_options(client, clean_query, limit, cdp_options, auto_cdp).await
        }
        SearchEngineKind::Searxng => search_searxng_with_options(client, clean_query, limit).await,
        SearchEngineKind::Google => {
            search_google_with_options(client, clean_query, limit, cdp_options, auto_cdp).await
        }
    }
}

/// Dedicated Baidu Baike vertical search querying Baidu's native OpenAPI endpoint.
/// Completely immune to Bing site: mangling, zero captcha risk, instant response.
pub async fn search_baike(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, TklessError> {
    let clean = query.trim();
    if clean.is_empty() {
        return Ok(Vec::new());
    }

    let encoded: String = url::form_urlencoded::byte_serialize(clean.as_bytes()).collect();
    let api_url = format!(
        "https://baike.baidu.com/api/openapi/BaikeLemmaCardApi?scope=103&format=json&appid=379020&bk_key={encoded}"
    );

    debug!(query = clean, url = %api_url, "Querying Baidu Baike native LemmaCard OpenAPI");

    let resp = client
        .get(&api_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
        )
        .header("Referer", "https://baike.baidu.com/")
        .send()
        .await?;

    if resp.status().is_success() {
        if let Ok(data) = resp.json::<serde_json::Value>().await {
            if let Some(title) = data.get("title").and_then(|v| v.as_str()) {
                let url = data
                    .get("url")
                    .and_then(|v| v.as_str())
                    .or_else(|| data.get("wapUrl").and_then(|v| v.as_str()))
                    .map(|s| {
                        if s.starts_with("http://") {
                            s.replace("http://", "https://")
                        } else {
                            s.to_string()
                        }
                    })
                    .unwrap_or_else(|| format!("https://baike.baidu.com/item/{title}"));

                let snippet = data
                    .get("abstract")
                    .and_then(|v| v.as_str())
                    .or_else(|| data.get("desc").and_then(|v| v.as_str()))
                    .unwrap_or_default()
                    .to_string();

                return Ok(vec![SearchResultItem {
                    title: format!("{title}_百度百科"),
                    url,
                    snippet,
                    engine: "baike".to_string(),
                }]);
            }
        }
    }

    // Fallback: If OpenAPI didn't return an exact card, search Bing with cleaned keyword
    search_bing(client, &format!("{} 百度百科", clean), limit).await
}

/// Dedicated WeChat Official Accounts vertical search via Sogou WeChat with automated JS redirect decryption.
pub async fn search_wechat_sogou(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, TklessError> {
    let clean = query.trim();
    if clean.is_empty() {
        return Ok(Vec::new());
    }

    let encoded: String = url::form_urlencoded::byte_serialize(clean.as_bytes()).collect();
    let search_url = format!("https://weixin.sogou.com/weixin?type=2&query={encoded}");

    debug!(query = clean, url = %search_url, "Querying Sogou WeChat article search engine");

    let resp = client
        .get(&search_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 Edg/133.0.0.0",
        )
        .header("Referer", "https://weixin.sogou.com/")
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(TklessError::AdapterError {
            platform: "wechat_search",
            message: format!("Sogou WeChat returned HTTP status {}", resp.status()),
        });
    }

    let mut search_cookies = Vec::new();
    for (name, val) in resp.headers() {
        if name == reqwest::header::SET_COOKIE {
            if let Ok(v_str) = val.to_str() {
                let c_part = v_str.split(';').next().unwrap_or("").trim();
                if !c_part.is_empty() {
                    search_cookies.push(c_part);
                }
            }
        }
    }
    let cookie_header = search_cookies.join("; ");

    let html = resp.text().await?;
    let doc = Html::parse_document(&html);

    let box_sel = Selector::parse("div.txt-box").unwrap();
    let title_sel = Selector::parse("h3 a").unwrap();
    let snip_sel = Selector::parse("p.txt-info").unwrap();

    let js_regex = regex::Regex::new(r#"url\s*\+?=\s*['"]([^'"]*)['"]"#).unwrap();

    let mut results = Vec::new();

    for el in doc.select(&box_sel) {
        let title_node = match el.select(&title_sel).next() {
            Some(node) => node,
            None => continue,
        };

        let raw_title = title_node.text().collect::<String>().trim().to_string();
        let raw_href = title_node.value().attr("href").unwrap_or("").trim();

        if raw_title.is_empty() || raw_href.is_empty() {
            continue;
        }

        let redirect_url = if raw_href.starts_with('/') {
            format!("https://weixin.sogou.com{raw_href}")
        } else {
            raw_href.to_string()
        };

        let snippet = el
            .select(&snip_sel)
            .next()
            .map(|s| s.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        // Attempt to decrypt Sogou intermediate redirect into real mp.weixin.qq.com URL
        let mut final_url = redirect_url.clone();
        let mut link_req = client
            .get(&redirect_url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 Edg/133.0.0.0",
            )
            .header("Referer", &search_url);
        if !cookie_header.is_empty() {
            link_req = link_req.header("Cookie", &cookie_header);
        }

        if let Ok(link_resp) = link_req.send().await {
            if let Ok(link_html) = link_resp.text().await {
                let mut parts = Vec::new();
                for cap in js_regex.captures_iter(&link_html) {
                    if let Some(m) = cap.get(1) {
                        parts.push(m.as_str());
                    }
                }
                if !parts.is_empty() {
                    let assembled = parts.concat().replace('@', "");
                    if assembled.starts_with("http") && assembled.contains("mp.weixin.qq.com") {
                        final_url = assembled;
                    }
                }
            }
        }

        results.push(SearchResultItem {
            title: raw_title,
            url: final_url,
            snippet,
            engine: "wechat".to_string(),
        });

        if results.len() >= limit {
            break;
        }
    }

    Ok(results)
}

/// Default web search implementation using the default configured engine (Bing).
pub async fn web_search(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, TklessError> {
    web_search_with_engine(client, query, limit, SearchEngineKind::default()).await
}

/// Tokenized query elements separated into core entity tokens, temporal tokens, domain filters, and exact phrases.
#[derive(Debug, Clone, Default)]
pub struct QueryTokens {
    /// Non-temporal core content tokens (e.g. "特斯拉", "上海工厂", "产量")
    pub core_tokens: Vec<String>,
    /// Temporal modifier tokens (e.g. "2026", "2025")
    pub temporal_tokens: Vec<String>,
    /// All raw tokens
    pub all_tokens: Vec<String>,
    /// Extracted domain constraints (e.g. "zhuanlan.zhihu.com", "zhihu.com")
    pub domain_filters: Vec<String>,
    /// Exact phrase constraints extracted from quotes (e.g. "small language model")
    pub phrase_filters: Vec<String>,
}

/// Tokenizes a user query into core semantic tokens, temporal modifiers, domain filters, and exact phrases.
static STOPWORDS: &[&str] = &[
    "vs", "and", "or", "in", "on", "at", "to", "for", "of", "with", "by", "from", "the", "a", "an",
    "is", "it", "as", "be", "are", "was", "were", "that", "this",
];

pub fn tokenize_query(query: &str) -> QueryTokens {
    let clean = query.trim().to_lowercase();
    let mut phrase_filters = Vec::new();

    // 1. Extract quoted phrases ("..." or “...”)
    let mut in_quote = false;
    let mut current_phrase = String::new();
    for c in query.chars() {
        if c == '"' || c == '“' || c == '”' {
            if in_quote {
                let trimmed = current_phrase.trim().to_lowercase();
                if !trimmed.is_empty() {
                    phrase_filters.push(trimmed);
                }
                current_phrase.clear();
                in_quote = false;
            } else {
                in_quote = true;
            }
        } else if in_quote {
            current_phrase.push(c);
        }
    }

    let mut core_tokens = Vec::new();
    let mut temporal_tokens = Vec::new();
    let mut all_tokens = Vec::new();
    let mut domain_filters = Vec::new();

    let raw_tokens: Vec<&str> = clean.split_whitespace().collect();

    for raw_t in raw_tokens {
        // Strip quotation marks and punctuation edges
        let t = raw_t.trim_matches(|c: char| {
            c == '"'
                || c == '\''
                || c == '“'
                || c == '”'
                || c == '‘'
                || c == '’'
                || c == '('
                || c == ')'
                || c == '['
                || c == ']'
                || c == ','
                || c == ';'
        });
        if t.is_empty() {
            continue;
        }

        all_tokens.push(t.to_string());

        // Check if operator like site:... or domain:... or inurl:...
        if let Some(domain) = t
            .strip_prefix("site:")
            .or_else(|| t.strip_prefix("site："))
            .or_else(|| t.strip_prefix("domain:"))
            .or_else(|| t.strip_prefix("inurl:"))
        {
            let clean_domain = domain
                .trim()
                .trim_start_matches("http://")
                .trim_start_matches("https://");
            if !clean_domain.is_empty() {
                domain_filters.push(clean_domain.to_string());
            }
            continue; // Do not add search operator to core_tokens!
        }

        // Skip negative operators like -site:... or -domain:...
        if t.starts_with("-site:") || t.starts_with("-domain:") {
            continue;
        }

        // Skip English stopwords for core token matching
        if STOPWORDS.contains(&t) {
            continue;
        }

        // Check if 4-digit year (e.g. 2024, 2025, 2026, 2027) or year prefix (e.g. 2026年)
        if t.len() == 4 && t.chars().all(|c| c.is_ascii_digit()) {
            temporal_tokens.push(t.to_string());
        } else if t.len() >= 4
            && (t.starts_with("19") || t.starts_with("20"))
            && t[..4].chars().all(|c| c.is_ascii_digit())
        {
            temporal_tokens.push(t[..4].to_string());
            let rem = t[4..].trim_start_matches('年').trim();
            if !rem.is_empty() {
                core_tokens.push(rem.to_string());
                if rem.chars().any(|c| (c as u32) >= 0x4E00) && rem.chars().count() >= 4 {
                    let chars: Vec<char> = rem.chars().collect();
                    let half = chars.len() / 2;
                    let s1: String = chars[..half].iter().collect();
                    let s2: String = chars[half..].iter().collect();
                    if !core_tokens.contains(&s1) {
                        core_tokens.push(s1);
                    }
                    if !core_tokens.contains(&s2) {
                        core_tokens.push(s2);
                    }
                }
            }
        } else {
            core_tokens.push(t.to_string());
            if t.chars().any(|c| (c as u32) >= 0x4E00) && t.chars().count() >= 4 {
                let chars: Vec<char> = t.chars().collect();
                let half = chars.len() / 2;
                let s1: String = chars[..half].iter().collect();
                let s2: String = chars[half..].iter().collect();
                if !core_tokens.contains(&s1) {
                    core_tokens.push(s1);
                }
                if !core_tokens.contains(&s2) {
                    core_tokens.push(s2);
                }
            }
        }
    }

    QueryTokens {
        core_tokens,
        temporal_tokens,
        all_tokens,
        domain_filters,
        phrase_filters,
    }
}

static AD_CONTAINER_CLASSES: &[&str] = &[
    "b_ad",
    "b_algo_ad",
    "b_promoted",
    "b_sponsored",
    "b_adbottom",
    "b_adtop",
    "b_adtopcontent",
    "b_ans_ad",
    "ads",
];

static PROMO_DOMAINS: &[&str] = &[
    "taobao.com",
    "douyin.com",
    "taptap.cn",
    "alipan.com",
    "pinduoduo.com",
    "yangkeduo.com",
    "poki.com",
    "poki.cn",
    "youtube.com",
    "tiktok.com",
    "missav",
    "kohi-den",
    "e621.net",
    "patreon.com",
    "52pojie.cn",
];

/// Identifies whether a Bing SERP element is a commercial advertisement or disguised promotional card.
pub fn is_bing_ad_or_promotion(
    element: &scraper::ElementRef,
    title: &str,
    _snippet: &str,
    resolved_url: &str,
    query_tokens: &QueryTokens,
) -> bool {
    // 1. Check element class attribute
    if let Some(cls) = element.value().attr("class") {
        let cls_lower = cls.to_lowercase();
        for ad_cls in AD_CONTAINER_CLASSES {
            if cls_lower.contains(ad_cls) {
                return true;
            }
        }
    }

    // 2. Check inner elements for ad indicators (b_adlabel, ad_label, etc.)
    let inner_html = element.html().to_lowercase();
    if inner_html.contains("b_adlabel")
        || inner_html.contains("ad_label")
        || inner_html.contains("class=\"b_ad\"")
        || inner_html.contains("class=\"ad\"")
        || inner_html.contains("class=\"ads\"")
    {
        return true;
    }

    // 3. Check for text prefixes explicitly indicating advertising
    let trimmed_title = title.trim();
    if trimmed_title.starts_with("广告")
        || trimmed_title.starts_with("推广")
        || trimmed_title.starts_with("赞助")
        || trimmed_title.starts_with("Ad ")
        || trimmed_title.starts_with("Sponsored")
    {
        return true;
    }

    // 4. Commercial promo landing domains heuristic
    // If the URL is a commercial landing domain and the user's query did not mention that domain/brand,
    // and neither title nor snippet contains any core query tokens, it is a disguised promotion.
    let url_lower = resolved_url.to_lowercase();
    for promo_domain in PROMO_DOMAINS {
        if url_lower.contains(promo_domain) {
            let query_mentions_domain = query_tokens.all_tokens.iter().any(|t| {
                promo_domain.contains(t)
                    || (t.contains("淘宝") && *promo_domain == "taobao.com")
                    || (t.contains("抖音") && *promo_domain == "douyin.com")
                    || (t.contains("阿里") && *promo_domain == "alipan.com")
                    || (t.contains("taptap") && *promo_domain == "taptap.cn")
            });
            if !query_mentions_domain {
                return true;
            }
        }
    }

    // 5. Local store / offline storefront cards heuristic (e.g. "XX餐厅 (蓝色港湾店)", "XX酒店 (国贸店)")
    if (trimmed_title.contains("店）") || trimmed_title.contains("店)"))
        && !query_tokens.all_tokens.iter().any(|t| t.contains("店"))
    {
        return true;
    }

    false
}

/// Checks if token matches inside haystack, enforcing word boundaries for short ASCII tokens (<= 3 chars).
pub fn is_token_matched(haystack: &str, token: &str) -> bool {
    if token.is_empty() || haystack.is_empty() {
        return false;
    }
    let h_lower = haystack.to_lowercase();
    let t_lower = token.to_lowercase();

    if token.is_ascii() && token.len() <= 3 {
        let bytes = h_lower.as_bytes();
        let t_bytes = t_lower.as_bytes();
        let t_len = t_bytes.len();

        let mut start = 0;
        while let Some(pos) = h_lower[start..].find(&t_lower) {
            let abs_pos = start + pos;
            let left_ok = abs_pos == 0 || !bytes[abs_pos - 1].is_ascii_alphanumeric();
            let right_ok =
                abs_pos + t_len >= bytes.len() || !bytes[abs_pos + t_len].is_ascii_alphanumeric();
            if left_ok && right_ok {
                return true;
            }
            start = abs_pos + 1;
            if start >= h_lower.len() {
                break;
            }
        }
        false
    } else {
        h_lower.contains(&t_lower)
    }
}

/// Returns variations and stems of a word (e.g. "troubleshooting" -> ["troubleshooting", "troubleshoot"]).
pub fn get_word_stems(token: &str) -> Vec<&str> {
    let mut stems = vec![token];
    if token.len() >= 6 && token.is_ascii() {
        if let Some(s) = token.strip_suffix("ing") {
            if s.len() >= 3 {
                stems.push(s);
            }
        }
        if let Some(s) = token.strip_suffix("tion") {
            if s.len() >= 3 {
                stems.push(s);
            }
        }
        if let Some(s) = token.strip_suffix("es") {
            if s.len() >= 3 {
                stems.push(s);
            }
        } else if let Some(s) = token.strip_suffix('s') {
            if s.len() >= 3 {
                stems.push(s);
            }
        }
        if let Some(s) = token.strip_suffix("ed") {
            if s.len() >= 3 {
                stems.push(s);
            }
        }
    }
    stems
}

/// Evaluates query relevance and scores a candidate result with full URL and domain awareness.
/// Returns None if the candidate is completely unrelated or a severe first-token-only fallback.
pub fn calculate_relevance_full(
    title: &str,
    snippet: &str,
    resolved_url: &str,
    query_tokens: &QueryTokens,
) -> Option<i32> {
    if query_tokens.core_tokens.is_empty() && query_tokens.phrase_filters.is_empty() {
        return Some(10);
    }

    let title_lower = title.to_lowercase();
    let snippet_lower = snippet.to_lowercase();
    let url_lower = resolved_url.to_lowercase();

    // Compact forms with spaces and separators removed for entity folding (e.g. "hugging face" == "huggingface")
    let title_compact = title_lower.replace([' ', '-', '_', '·', '|', '—'], "");
    let snippet_compact = snippet_lower.replace([' ', '-', '_', '·', '|', '—'], "");
    let url_compact = url_lower.replace([' ', '-', '_', '/', '.'], "");

    let mut score = 0;
    let mut domain_matched = false;

    // 1. Domain filter matching (e.g. site:zhuanlan.zhihu.com or site:zhihu.com)
    if !query_tokens.domain_filters.is_empty() {
        for df in &query_tokens.domain_filters {
            if url_lower.contains(df) {
                domain_matched = true;
                score += 35;
                break;
            }
        }
    }

    // 2. Exact phrase matching (e.g. "small language model")
    for phrase in &query_tokens.phrase_filters {
        let phrase_compact = phrase.replace(' ', "");
        if title_lower.contains(phrase) || title_compact.contains(&phrase_compact) {
            score += 30;
        } else if snippet_lower.contains(phrase) || snippet_compact.contains(&phrase_compact) {
            score += 15;
        }
    }

    // 3. Core token matching
    let mut matched_core = Vec::new();
    for (idx, token) in query_tokens.core_tokens.iter().enumerate() {
        let token_compact = token.replace([' ', '-', '_'], "");
        let stems = get_word_stems(token);
        let in_title = stems.iter().any(|st| is_token_matched(title, st))
            || (!token_compact.is_empty() && title_compact.contains(&token_compact));
        let in_snippet = stems.iter().any(|st| is_token_matched(snippet, st))
            || (!token_compact.is_empty() && snippet_compact.contains(&token_compact));
        let in_url = !resolved_url.is_empty()
            && (stems.iter().any(|st| is_token_matched(resolved_url, st))
                || (!token_compact.is_empty()
                    && token.len() >= 4
                    && url_compact.contains(&token_compact)));

        if in_title || in_snippet || in_url {
            matched_core.push(idx);
            if in_title {
                score += 20;
            }
            if in_snippet {
                score += 8;
            }
            if in_url {
                score += 10;
            }
        } else if token.is_ascii() && token.len() >= 5 {
            // Partial prefix matching for technical terms (e.g. "smollm" in "smollm2" or vice versa)
            let prefix = &token[..token.len() - 1];
            if title_lower.contains(prefix)
                || snippet_lower.contains(prefix)
                || url_lower.contains(prefix)
            {
                matched_core.push(idx);
                score += 8;
            }
        } else if token.chars().any(|c| (c as u32) >= 0x4E00) && token.chars().count() >= 3 {
            // CJK compound word n-gram matching
            let chars: Vec<char> = token.chars().collect();
            let mut cjk_sub_matches = 0;
            for window in chars.windows(2) {
                let bi_gram: String = window.iter().collect();
                if title_lower.contains(&bi_gram) || snippet_lower.contains(&bi_gram) {
                    cjk_sub_matches += 1;
                }
            }
            if cjk_sub_matches >= 2 || (chars.len() <= 3 && cjk_sub_matches >= 1) {
                matched_core.push(idx);
                score += 15 + cjk_sub_matches * 3;
            }
        }
    }

    // Hard Filter 1: Zero Core Tokens Matched AND No Domain Matched AND No Phrase Matched
    if matched_core.is_empty() && !domain_matched && score == 0 {
        return None;
    }

    // Hard Filter 2: First-Token-Only Fallback (Bing Term Drop)
    // If user asked a multi-token query (>=2 core tokens), but the result ONLY matches token 0,
    // and matches NONE of the other 1..N core tokens (e.g. "Python asyncio", "Rust vs Go", "北京 小学 入学 政策"),
    // reject this term-drop casualty so Stage 1d can retry with quoted discriminating keywords.
    if query_tokens.core_tokens.len() >= 2 && !domain_matched && matched_core == vec![0] {
        return None;
    }

    // Check temporal tokens (e.g. 2026, 2025)
    for year in &query_tokens.temporal_tokens {
        if title_lower.contains(year) {
            score += 12;
        } else if snippet_lower.contains(year) {
            score += 4;
        }
    }

    Some(score)
}

/// Evaluates relaxed query relevance for graceful fallback when strict gatekeeping yields 0 results.
pub fn calculate_relaxed_relevance(
    title: &str,
    snippet: &str,
    resolved_url: &str,
    query_tokens: &QueryTokens,
) -> Option<i32> {
    if query_tokens.core_tokens.is_empty() && query_tokens.phrase_filters.is_empty() {
        return Some(5);
    }

    let title_lower = title.to_lowercase();
    let snippet_lower = snippet.to_lowercase();
    let url_lower = resolved_url.to_lowercase();

    let title_compact = title_lower.replace([' ', '-', '_', '·', '|', '—'], "");
    let snippet_compact = snippet_lower.replace([' ', '-', '_', '·', '|', '—'], "");
    let url_compact = url_lower.replace([' ', '-', '_', '/', '.'], "");

    let mut score = 0;

    // Domain filter matching
    if !query_tokens.domain_filters.is_empty() {
        for df in &query_tokens.domain_filters {
            if url_lower.contains(df) {
                score += 35;
                break;
            }
        }
    }

    // Exact phrase matching
    for phrase in &query_tokens.phrase_filters {
        let phrase_compact = phrase.replace(' ', "");
        if title_lower.contains(phrase) || title_compact.contains(&phrase_compact) {
            score += 25;
        } else if snippet_lower.contains(phrase) || snippet_compact.contains(&phrase_compact) {
            score += 10;
        }
    }

    // Core tokens matching (at least 1 required)
    let mut any_match = false;
    for token in &query_tokens.core_tokens {
        let token_compact = token.replace([' ', '-', '_'], "");
        let stems = get_word_stems(token);
        let in_title = stems.iter().any(|st| is_token_matched(title, st))
            || (!token_compact.is_empty() && title_compact.contains(&token_compact));
        let in_snippet = stems.iter().any(|st| is_token_matched(snippet, st))
            || (!token_compact.is_empty() && snippet_compact.contains(&token_compact));
        let in_url = !resolved_url.is_empty()
            && (stems.iter().any(|st| is_token_matched(resolved_url, st))
                || (!token_compact.is_empty()
                    && token.len() >= 4
                    && url_compact.contains(&token_compact)));

        if in_title || in_snippet || in_url {
            any_match = true;
            if in_title {
                score += 15;
            }
            if in_snippet {
                score += 6;
            }
            if in_url {
                score += 8;
            }
        } else if token.is_ascii() && token.len() >= 5 {
            let prefix = &token[..token.len() - 1];
            if title_lower.contains(prefix) || snippet_lower.contains(prefix) {
                any_match = true;
                score += 5;
            }
        } else if token.chars().any(|c| (c as u32) >= 0x4E00) && token.chars().count() >= 3 {
            let chars: Vec<char> = token.chars().collect();
            let mut cjk_sub_matches = 0;
            for window in chars.windows(2) {
                let bi_gram: String = window.iter().collect();
                if title_lower.contains(&bi_gram) || snippet_lower.contains(&bi_gram) {
                    cjk_sub_matches += 1;
                }
            }
            if cjk_sub_matches >= 1 {
                any_match = true;
                score += 8 + cjk_sub_matches * 2;
            }
        }
    }

    if !any_match && score == 0 {
        return None;
    }

    // Check temporal tokens
    for year in &query_tokens.temporal_tokens {
        if title_lower.contains(year) {
            score += 8;
        } else if snippet_lower.contains(year) {
            score += 3;
        }
    }

    Some(score)
}

/// Evaluates query relevance and scores a candidate result (backward compatible wrapper).
pub fn calculate_relevance(title: &str, snippet: &str, query_tokens: &QueryTokens) -> Option<i32> {
    calculate_relevance_full(title, snippet, "", query_tokens)
}

struct ScoredBingItem {
    item: SearchResultItem,
    score: i32,
}

/// Searches Microsoft Bing for the given query without API keys, with anti-degradation hardening and multi-stage relaxation.
pub async fn search_bing(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, TklessError> {
    search_bing_with_options(client, query, limit, None, false).await
}

/// Searches Microsoft Bing with optional CDP headless browser rendering to bypass anti-scraping challenges.
pub async fn search_bing_with_options(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
    cdp_options: Option<&crate::cdp::CdpOptions>,
    auto_cdp: bool,
) -> Result<Vec<SearchResultItem>, TklessError> {
    let clean_query = query.trim();
    if clean_query.is_empty() {
        return Ok(Vec::new());
    }

    let tokens = tokenize_query(clean_query);

    // Stage 1: Full original query with clean Bing endpoint
    let results = fetch_and_parse_bing(
        client,
        clean_query,
        clean_query,
        limit,
        cdp_options,
        auto_cdp,
    )
    .await?;

    // If query specified domain filters (e.g. site:zhuanlan.zhihu.com or site:zhihu.com),
    // but results do NOT contain any matching domain, treat Stage 1 as missed and proceed to Stage 1b rewrite!
    let domain_satisfied = if tokens.domain_filters.is_empty() {
        !results.is_empty()
    } else {
        results.iter().any(|r| {
            let u_lower = r.url.to_lowercase();
            tokens.domain_filters.iter().any(|df| u_lower.contains(df))
        })
    };

    // A result set is only truly satisfied if it's not a severe single-token term drop on a multi-word query
    let is_relevant = if tokens.core_tokens.len() >= 3 {
        let last_token = tokens.core_tokens.last().unwrap();
        let second_token = &tokens.core_tokens[1];
        let stems_last = get_word_stems(last_token);
        let stems_second = get_word_stems(second_token);
        results.iter().any(|r| {
            let t_low = r.title.to_lowercase();
            let s_low = r.snippet.to_lowercase();
            stems_last
                .iter()
                .any(|st| is_token_matched(&t_low, st) || is_token_matched(&s_low, st))
                || stems_second
                    .iter()
                    .any(|st| is_token_matched(&t_low, st) || is_token_matched(&s_low, st))
        })
    } else if tokens.core_tokens.len() == 2 {
        let second = &tokens.core_tokens[1];
        let stems_second = get_word_stems(second);
        results.iter().any(|r| {
            let t_low = r.title.to_lowercase();
            let s_low = r.snippet.to_lowercase();
            stems_second
                .iter()
                .any(|st| is_token_matched(&t_low, st) || is_token_matched(&s_low, st))
        })
    } else {
        !results.is_empty()
    };

    if is_relevant && domain_satisfied {
        return Ok(results);
    }

    // Stage 1b: Domain filter rewriting (e.g. site:zhuanlan.zhihu.com or site:zhihu.com)
    // Bing's search often breaks on site: prefixes for domestic Chinese platforms.
    // Replace site:zhuanlan.zhihu.com with "知乎专栏" or site:zhihu.com with "知乎"
    if !tokens.domain_filters.is_empty() {
        let mut rewritten_query = clean_query.to_string();
        for df in &tokens.domain_filters {
            let site_prefix = format!("site:{df}");
            let site_prefix_cn = format!("site：{df}");
            if df.contains("zhuanlan.zhihu.com") {
                rewritten_query = rewritten_query
                    .replace(&site_prefix, "知乎专栏")
                    .replace(&site_prefix_cn, "知乎专栏");
            } else if df.contains("zhihu.com") {
                rewritten_query = rewritten_query
                    .replace(&site_prefix, "知乎")
                    .replace(&site_prefix_cn, "知乎");
            } else if df.contains("csdn.net") {
                rewritten_query = rewritten_query
                    .replace(&site_prefix, "csdn")
                    .replace(&site_prefix_cn, "csdn");
            } else if df.contains("juejin.cn") {
                rewritten_query = rewritten_query
                    .replace(&site_prefix, "掘金")
                    .replace(&site_prefix_cn, "掘金");
            } else {
                rewritten_query = rewritten_query
                    .replace(&site_prefix, df)
                    .replace(&site_prefix_cn, df);
            }
        }
        let clean_rewritten = rewritten_query.trim().to_string();
        if clean_rewritten != clean_query {
            debug!(
                original = clean_query,
                rewritten = %clean_rewritten,
                "Stage 1b: Retrying with rewritten platform keyword instead of raw site: syntax"
            );
            let rewritten_results = fetch_and_parse_bing(
                client,
                &clean_rewritten,
                clean_query,
                limit,
                cdp_options,
                auto_cdp,
            )
            .await?;
            if !rewritten_results.is_empty() {
                return Ok(rewritten_results);
            }
        }
    }

    // Stage 1c: Quoted phrase relaxation (strip quotes if exact quote yielded 0 results)
    if !tokens.phrase_filters.is_empty() {
        let unquoted = clean_query.replace(['"', '“', '”'], "");
        let clean_unquoted = unquoted.trim().to_string();
        if clean_unquoted != clean_query {
            debug!(
                original = clean_query,
                unquoted = %clean_unquoted,
                "Stage 1c: Quoted query returned 0 results, retrying without quotation marks"
            );
            let unquoted_results = fetch_and_parse_bing(
                client,
                &clean_unquoted,
                clean_query,
                limit,
                cdp_options,
                auto_cdp,
            )
            .await?;
            if !unquoted_results.is_empty() {
                return Ok(unquoted_results);
            }
        }
    }

    // Stage 1d: Anti-Term-Drop Quoting of Discriminating Keywords
    // If query has 2+ core tokens and Stage 1 failed to find multi-token relevant results (e.g. "Python asyncio best practices", "artificial intelligence history"),
    // quote the discriminating keyword (e.g. `artificial intelligence "history"`) case-insensitively to prevent Bing from dropping it!
    if tokens.core_tokens.len() >= 2 && tokens.phrase_filters.is_empty() {
        let discriminating_token = if tokens.core_tokens.len() >= 3 {
            tokens.core_tokens.last().unwrap()
        } else {
            &tokens.core_tokens[1]
        };

        if discriminating_token.len() >= 2
            && !clean_query.contains(&format!("\"{discriminating_token}\""))
        {
            let re_pattern = format!(r#"(?i)\b{}\b"#, regex::escape(discriminating_token));
            if let Ok(re) = regex::Regex::new(&re_pattern) {
                if let Some(mat) = re.find(clean_query) {
                    let matched_str = mat.as_str();
                    let quoted_query = format!(
                        "{}\"{}\"{}",
                        &clean_query[..mat.start()],
                        matched_str,
                        &clean_query[mat.end()..]
                    );
                    debug!(
                        original = clean_query,
                        quoted = %quoted_query,
                        "Stage 1d: Quoting discriminating keyword to prevent Bing Term Drop"
                    );
                    let quoted_results = fetch_and_parse_bing(
                        client,
                        &quoted_query,
                        clean_query,
                        limit,
                        cdp_options,
                        auto_cdp,
                    )
                    .await?;
                    if !quoted_results.is_empty() {
                        return Ok(quoted_results);
                    }
                }
            }
        }
    }

    // Stage 2: Temporal Relaxation
    // If query has future years (e.g. "2026", "2025") causing Bing to return 0 exact hits or drop terms,
    // retry once with core entity tokens (e.g. "房贷 利率 LPR", "特斯拉 上海工厂 产量")
    if !tokens.temporal_tokens.is_empty() && tokens.core_tokens.len() >= 2 {
        let relaxed_core = tokens.core_tokens.join(" ");
        debug!(
            original = clean_query,
            relaxed = %relaxed_core,
            "Stage 2: Year-constrained query returned 0 relevant results, retrying without temporal constraint"
        );
        let relaxed_results = fetch_and_parse_bing(
            client,
            &relaxed_core,
            clean_query,
            limit,
            cdp_options,
            auto_cdp,
        )
        .await?;
        if !relaxed_results.is_empty() {
            return Ok(relaxed_results);
        }
    }

    // Stage 3: CJK Word Compaction (Prevent Bing Term Drop)
    // In Chinese, space-separated keywords like "新能源汽车 补贴 政策" often confuse Bing's segmenter
    // into dropping all words except the first. Merging CJK tokens ("新能源汽车补贴政策") forces exact phrase matching.
    if tokens.core_tokens.len() >= 2 && clean_query.chars().any(|c| (c as u32) >= 0x4E00) {
        let compacted_cjk = tokens.core_tokens.join("");
        let query_compact = if tokens.temporal_tokens.is_empty() {
            compacted_cjk.clone()
        } else {
            format!("{} {}", compacted_cjk, tokens.temporal_tokens.join(" "))
        };

        if query_compact != clean_query {
            debug!(
                original = clean_query,
                compacted = %query_compact,
                "Stage 3: Retrying with compacted CJK phrase to prevent Bing Term Drop"
            );
            let compacted_results = fetch_and_parse_bing(
                client,
                &query_compact,
                clean_query,
                limit,
                cdp_options,
                auto_cdp,
            )
            .await?;
            if !compacted_results.is_empty() {
                return Ok(compacted_results);
            }
        }

        // Stage 4: Compacted CJK phrase without temporal token
        if !tokens.temporal_tokens.is_empty() {
            debug!(
                original = clean_query,
                compacted = %compacted_cjk,
                "Stage 4: Retrying with pure compacted CJK phrase without temporal token"
            );
            let pure_results = fetch_and_parse_bing(
                client,
                &compacted_cjk,
                clean_query,
                limit,
                cdp_options,
                auto_cdp,
            )
            .await?;
            if !pure_results.is_empty() {
                return Ok(pure_results);
            }
        }
    }

    if !results.is_empty() {
        return Ok(results);
    }

    Ok(Vec::new())
}

async fn fetch_and_parse_bing(
    client: &reqwest::Client,
    search_query: &str,
    relevance_query: &str,
    limit: usize,
    cdp_options: Option<&crate::cdp::CdpOptions>,
    auto_cdp: bool,
) -> Result<Vec<SearchResultItem>, TklessError> {
    // Replace intra-word hyphens in search query to avoid search engine NOT/negation exclusion (e.g. "near-duplicate" -> "near duplicate")
    let normalized_query = search_query.replace('-', " ");
    let encoded_q: String =
        url::form_urlencoded::byte_serialize(normalized_query.as_bytes()).collect();
    // Use clean global search URL without &setlang=... which disrupts backend search query routing
    let search_url = format!("https://www.bing.com/search?q={encoded_q}");

    debug!(query = search_query, url = %search_url, "Executing Bing web search");

    // If explicit CDP options provided, fetch directly via headless Chrome
    if let Some(cdp_opts) = cdp_options {
        let html = crate::cdp::fetch_html_via_cdp(&search_url, cdp_opts).await?;
        return parse_bing_html_with_query(&html, relevance_query, limit);
    }

    let is_cjk = search_query.chars().any(|c| (c as u32) >= 0x4E00);
    // Use worldwide market en-ww instead of zh-cn to avoid severely crippled regional term drop
    let cookie_val = if is_cjk {
        "SRCHD=AF=NOFORM; SRCHHPGUSR=SRCHLANG=zh-Hans; _EDGE_S=mkt=en-ww; _SS=SID=1;"
    } else {
        "SRCHD=AF=NOFORM; SRCHHPGUSR=SRCHLANG=en; _EDGE_S=mkt=en-us; _SS=SID=1;"
    };
    let accept_lang = if is_cjk {
        "zh-CN,zh;q=0.9,en;q=0.8"
    } else {
        "en-US,en;q=0.9,zh-CN;q=0.8"
    };

    let resp = client
        .get(&search_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 Edg/133.0.0.0",
        )
        .header("Referer", "https://www.bing.com/")
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
        )
        .header("Accept-Language", accept_lang)
        .header("Cookie", cookie_val)
        .header(
            "Sec-Ch-Ua",
            "\"Not(A:Brand\";v=\"99\", \"Microsoft Edge\";v=\"133\", \"Chromium\";v=\"133\"",
        )
        .header("Sec-Ch-Ua-Mobile", "?0")
        .header("Sec-Ch-Ua-Platform", "\"Windows\"")
        .header("Sec-Fetch-Dest", "document")
        .header("Sec-Fetch-Mode", "navigate")
        .header("Sec-Fetch-Site", "same-origin")
        .header("Sec-Fetch-User", "?1")
        .header("Upgrade-Insecure-Requests", "1")
        .send()
        .await?;

    let mut page2_cookie = cookie_val.to_string();
    for val in resp.headers().get_all(reqwest::header::SET_COOKIE) {
        if let Ok(s) = val.to_str() {
            if let Some(cookie_pair) = s.split(';').next() {
                page2_cookie.push_str("; ");
                page2_cookie.push_str(cookie_pair.trim());
            }
        }
    }

    let html = resp.text().await?;
    let mut initial_results = match parse_bing_html_with_query(&html, relevance_query, limit) {
        Err(TklessError::SearchAntiScraping(ref err_msg)) if auto_cdp => {
            warn!(
                error = %err_msg,
                "Encountered Bing anti-scraping challenge, auto-falling back to CDP browser rendering"
            );
            let fallback_cdp = crate::cdp::CdpOptions {
                endpoint: "http://127.0.0.1:9222".to_string(),
                wait_ms: Some(1500),
                timeout_ms: Some(30_000),
                ..Default::default()
            };
            let cdp_html = crate::cdp::fetch_html_via_cdp(&search_url, &fallback_cdp).await?;
            parse_bing_html_with_query(&cdp_html, relevance_query, limit)?
        }
        other => other?,
    };

    // Pagination support: if user requested limit > 10 and we have >= 8 results on page 1, fetch page 2 (&first=11 with FPIG token)
    if limit > 10 && initial_results.len() >= 8 {
        let page2_url = if let Some(cap) = BING_NEXT_PAGE_RE.captures(&html) {
            let rel = cap[1].replace("&amp;", "&");
            format!("https://www.bing.com{rel}")
        } else {
            format!("{search_url}&first=11")
        };
        debug!(url = %page2_url, "Fetching Bing page 2 for pagination");
        if let Ok(resp2) = client
            .get(&page2_url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 Edg/133.0.0.0",
            )
            .header("Referer", "https://www.bing.com/")
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
            )
            .header("Accept-Language", accept_lang)
            .header("Cookie", page2_cookie)
            .header(
                "Sec-Ch-Ua",
                "\"Not(A:Brand\";v=\"99\", \"Microsoft Edge\";v=\"133\", \"Chromium\";v=\"133\"",
            )
            .header("Sec-Ch-Ua-Mobile", "?0")
            .header("Sec-Ch-Ua-Platform", "\"Windows\"")
            .send()
            .await
        {
            debug!(status = %resp2.status(), "Page 2 HTTP response received");
            if let Ok(html2) = resp2.text().await {
                match parse_bing_html_with_query(&html2, relevance_query, limit) {
                    Ok(p2_items) => {
                        debug!(count = p2_items.len(), "Page 2 items parsed");
                        for item in &p2_items {
                            debug!(p2_url = %item.url, "P2 item URL");
                        }
                        let mut seen_urls: std::collections::HashSet<String> = initial_results
                            .iter()
                            .map(|i| normalize_url_for_dedup(&i.url))
                            .collect();
                        for item in p2_items {
                            if seen_urls.insert(normalize_url_for_dedup(&item.url)) {
                                initial_results.push(item);
                                if initial_results.len() >= limit {
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        debug!(error = %e, "Page 2 parsing failed");
                    }
                }
            }
        }
    }

    debug!(
        final_bing_count = initial_results.len(),
        "fetch_and_parse_bing done"
    );
    Ok(initial_results)
}

/// Parses Bing search results HTML into structured items with query-aware relevance gatekeeping.
pub fn parse_bing_html_with_query(
    html: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, TklessError> {
    // 0. Active Anti-Scraping / Captcha Challenge Detection
    if html.contains("最后一步 请解决以下难题以继续")
        || html.contains("请解决以下难题以继续")
        || html.contains("b_captcha")
        || html.contains("Please solve the challenge below")
    {
        return Err(TklessError::SearchAntiScraping(
            "遭遇 Bing 搜索人机验证码挑战（'最后一步 请解决以下难题以继续'）。建议操作：1. 稍后重试或更换代理节点；2. 启动本地 Chrome 无头浏览器并配合 `--cdp` 选项绕过挑战。".to_string(),
        ));
    }

    let doc = Html::parse_document(html);
    let mut scored_results: Vec<ScoredBingItem> = Vec::new();
    let mut fallback_results: Vec<ScoredBingItem> = Vec::new();
    let mut raw_non_ad_items: Vec<ScoredBingItem> = Vec::new();
    let mut seen_urls = std::collections::HashSet::new();

    let query_tokens = tokenize_query(query);

    for element in doc.select(&BING_ITEM_SELECTOR) {
        let title_el = match element.select(&BING_TITLE_SELECTOR).next() {
            Some(el) => el,
            None => continue,
        };

        let raw_title = title_el
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string();
        let raw_href = title_el.value().attr("href").unwrap_or("").trim();

        if raw_title.is_empty() || raw_href.is_empty() {
            continue;
        }

        // Filter Bing internal links or invalid protocols
        if !raw_href.starts_with("http://") && !raw_href.starts_with("https://") {
            continue;
        }

        let resolved_url = unwrap_bing_url(raw_href);
        if !seen_urls.insert(resolved_url.clone()) {
            continue;
        }

        let snippet = element
            .select(&BING_SNIPPET_SELECTOR)
            .next()
            .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
            .unwrap_or_default();

        // 1. Filter out ads and commercial promotion widgets
        if is_bing_ad_or_promotion(&element, &raw_title, &snippet, &resolved_url, &query_tokens) {
            continue;
        }

        raw_non_ad_items.push(ScoredBingItem {
            item: SearchResultItem {
                title: raw_title.clone(),
                url: resolved_url.clone(),
                snippet: snippet.clone(),
                engine: "bing".to_string(),
            },
            score: 1,
        });

        // 2. Strict evaluation (Tier 1)
        if let Some(score) =
            calculate_relevance_full(&raw_title, &snippet, &resolved_url, &query_tokens)
        {
            scored_results.push(ScoredBingItem {
                item: SearchResultItem {
                    title: raw_title.clone(),
                    url: resolved_url.clone(),
                    snippet: snippet.clone(),
                    engine: "bing".to_string(),
                },
                score,
            });
        }

        // 3. Relaxed evaluation (Tier 2 Fallback)
        if let Some(score) =
            calculate_relaxed_relevance(&raw_title, &snippet, &resolved_url, &query_tokens)
        {
            fallback_results.push(ScoredBingItem {
                item: SearchResultItem {
                    title: raw_title,
                    url: resolved_url,
                    snippet,
                    engine: "bing".to_string(),
                },
                score,
            });
        }
    }

    let mut final_scored = if !scored_results.is_empty() {
        scored_results
    } else if !fallback_results.is_empty() {
        fallback_results
    } else {
        raw_non_ad_items
    };

    // Sort by relevance score descending so the most relevant results come first
    final_scored.sort_by_key(|a| std::cmp::Reverse(a.score));

    let results = final_scored
        .into_iter()
        .take(limit)
        .map(|sr| sr.item)
        .collect();

    Ok(results)
}

/// Parses Bing search results HTML into structured items (backward compatible).
pub fn parse_bing_html(html: &str, limit: usize) -> Result<Vec<SearchResultItem>, TklessError> {
    parse_bing_html_with_query(html, "", limit)
}

/// Unwraps Bing tracking redirect URLs like `https://www.bing.com/ck/a?!&&...&u=a1aHR0cHM6Ly9ydXN0LWxhbmcub3JnLw...`
pub fn unwrap_bing_url(raw_href: &str) -> String {
    if let Some(pos) = raw_href.find("&u=") {
        let after_u = &raw_href[pos + 3..];
        let encoded = after_u.split('&').next().unwrap_or("");
        // Bing ck URLs prefix base64 with "a1"
        let b64_part = if let Some(stripped) = encoded.strip_prefix("a1") {
            stripped
        } else {
            encoded
        };

        if let Some(decoded_bytes) = decode_base64_url_safe(b64_part) {
            if let Ok(decoded_str) = String::from_utf8(decoded_bytes) {
                if decoded_str.starts_with("http://") || decoded_str.starts_with("https://") {
                    return decoded_str;
                }
            }
        }
    }

    raw_href.to_string()
}

/// Decodes standard and URL-safe Base64 strings.
fn decode_base64_url_safe(input: &str) -> Option<Vec<u8>> {
    let clean = input.trim_end_matches('=');
    let mut bits = 0u32;
    let mut bit_count = 0;
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);

    for b in clean.bytes() {
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        bits = (bits << 6) | (val as u32);
        bit_count += 6;

        if bit_count >= 8 {
            bit_count -= 8;
            out.push((bits >> bit_count) as u8);
            bits &= (1 << bit_count) - 1;
        }
    }

    Some(out)
}

/// Searches DuckDuckGo for the given query without API keys.
pub async fn search_duckduckgo(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, TklessError> {
    search_duckduckgo_with_options(client, query, limit, None, false).await
}

/// Searches DuckDuckGo with optional CDP headless browser rendering to bypass anti-scraping challenges.
pub async fn search_duckduckgo_with_options(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
    cdp_options: Option<&crate::cdp::CdpOptions>,
    auto_cdp: bool,
) -> Result<Vec<SearchResultItem>, TklessError> {
    let clean_query = query.trim();
    if clean_query.is_empty() {
        return Ok(Vec::new());
    }

    let encoded_q: String = url::form_urlencoded::byte_serialize(clean_query.as_bytes()).collect();
    let cdp_search_url = format!("https://duckduckgo.com/?q={encoded_q}");

    // 1. Direct CDP headless Chrome rendering if explicitly configured
    if let Some(cdp_opts) = cdp_options {
        debug!(
            query = clean_query,
            url = %cdp_search_url,
            "Fetching DuckDuckGo via explicit CDP endpoint"
        );
        let cdp_html = crate::cdp::fetch_html_via_cdp(&cdp_search_url, cdp_opts).await?;
        return parse_duckduckgo_html(&cdp_html, limit);
    }

    debug!(
        query = clean_query,
        "Executing DuckDuckGo search via HTML POST endpoint"
    );

    let resp = client
        .post("https://html.duckduckgo.com/html/")
        .version(reqwest::Version::HTTP_11)
        .form(&[("q", clean_query)])
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
        )
        .header("Referer", "https://html.duckduckgo.com/")
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .send()
        .await?;

    let is_blocked = resp.status() == reqwest::StatusCode::ACCEPTED;
    if !resp.status().is_success() && !is_blocked {
        return Err(TklessError::SearchAntiScraping(format!(
            "DuckDuckGo search returned HTTP status {}",
            resp.status()
        )));
    }

    let html = resp.text().await?;
    let has_anomaly = html.contains("anomalyDetectionBlock") || html.contains("anomaly.js");

    // 2. Auto-CDP fallback if anti-scraping challenge is encountered
    if (is_blocked || has_anomaly) && auto_cdp {
        warn!(
            query = clean_query,
            "Encountered DuckDuckGo anti-scraping challenge, auto-falling back to CDP browser rendering"
        );
        let fallback_cdp = crate::cdp::CdpOptions {
            endpoint: "http://127.0.0.1:9222".to_string(),
            wait_ms: Some(2500),
            timeout_ms: Some(30_000),
            block_resources: Some(false),
            ..Default::default()
        };
        let cdp_html = crate::cdp::fetch_html_via_cdp(&cdp_search_url, &fallback_cdp).await?;
        return parse_duckduckgo_html(&cdp_html, limit);
    }

    if is_blocked || has_anomaly {
        return Err(TklessError::SearchAntiScraping(
            "遭遇 DuckDuckGo 搜索反爬拦截 (HTTP 202 Accepted / anomalyDetectionBlock)。建议操作：1. 开启 --auto-cdp 使用本地无头浏览器渲染；2. 使用默认多引擎或 --engine bing 检索。".to_string(),
        ));
    }

    parse_duckduckgo_html(&html, limit)
}

/// Parses DuckDuckGo search results HTML into structured items.
pub fn parse_duckduckgo_html(
    html: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, TklessError> {
    let doc = Html::parse_document(html);
    let mut results = Vec::new();
    let mut seen_urls = std::collections::HashSet::new();

    // 0. Modern DuckDuckGo dynamic React SPA format (rendered via CDP / headless Chrome)
    let spa_item_sel = Selector::parse(
        "article[data-testid='result'], li[data-layout='organic'], div[data-testid='result']",
    )
    .unwrap();
    let spa_title_sel = Selector::parse("a[data-testid='result-title-a'], h2 a, a.result__url, [data-testid='result-extras-url-link']").unwrap();
    let spa_snip_sel = Selector::parse(
        "[data-testid='result-snippet'], div.result__snippet, div[data-result='snippet']",
    )
    .unwrap();

    let spa_items: Vec<_> = doc.select(&spa_item_sel).collect();
    if !spa_items.is_empty() {
        for element in spa_items {
            let link_el = match element.select(&spa_title_sel).next() {
                Some(el) => el,
                None => continue,
            };

            let raw_title = link_el
                .text()
                .collect::<Vec<_>>()
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");

            let raw_href = link_el.value().attr("href").unwrap_or("").trim();
            if raw_title.is_empty() || raw_href.is_empty() {
                continue;
            }

            let resolved_url = unwrap_duckduckgo_url(raw_href);
            if !resolved_url.starts_with("http://") && !resolved_url.starts_with("https://") {
                continue;
            }

            let norm_url = normalize_url_for_dedup(&resolved_url);
            if !seen_urls.insert(norm_url) {
                continue;
            }

            let snippet = element
                .select(&spa_snip_sel)
                .next()
                .map(|el| {
                    el.text()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();

            results.push(SearchResultItem {
                title: raw_title,
                url: resolved_url,
                snippet,
                engine: "duckduckgo".to_string(),
            });

            if results.len() >= limit {
                break;
            }
        }

        if !results.is_empty() {
            return Ok(results);
        }
    }

    // 1. DuckDuckGo Lite format (a.result-link, td.result-snippet)
    let lite_link_sel = Selector::parse("a.result-link").unwrap();
    let lite_snip_sel = Selector::parse("td.result-snippet").unwrap();

    let lite_links: Vec<_> = doc.select(&lite_link_sel).collect();
    let lite_snippets: Vec<_> = doc.select(&lite_snip_sel).collect();

    if !lite_links.is_empty() {
        for (i, link_el) in lite_links.into_iter().enumerate() {
            let raw_title = link_el
                .text()
                .collect::<Vec<_>>()
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");

            let raw_href = link_el.value().attr("href").unwrap_or("").trim();
            if raw_title.is_empty() || raw_href.is_empty() {
                continue;
            }

            if raw_href.contains("duckduckgo.com/y.js") || raw_href.contains("/y.js?") {
                continue;
            }

            let resolved_url = unwrap_duckduckgo_url(raw_href);
            if !resolved_url.starts_with("http://") && !resolved_url.starts_with("https://") {
                continue;
            }

            let norm_url = normalize_url_for_dedup(&resolved_url);
            if !seen_urls.insert(norm_url) {
                continue;
            }

            let snippet = lite_snippets
                .get(i)
                .map(|el| {
                    el.text()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();

            results.push(SearchResultItem {
                title: raw_title,
                url: resolved_url,
                snippet,
                engine: "duckduckgo".to_string(),
            });

            if results.len() >= limit {
                break;
            }
        }

        if !results.is_empty() {
            return Ok(results);
        }
    }

    // 2. DuckDuckGo standard HTML format (div.results_links, div.result, div.web-result)
    for element in doc.select(&DDG_ITEM_SELECTOR) {
        if let Some(cls) = element.value().attr("class") {
            let cls_lower = cls.to_lowercase();
            if cls_lower.contains("result--ad") || cls_lower.contains("badge--ad") {
                continue;
            }
        }

        let title_el = match element.select(&DDG_TITLE_SELECTOR).next() {
            Some(el) => el,
            None => continue,
        };

        let raw_title = title_el
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        let raw_href = title_el.value().attr("href").unwrap_or("").trim();
        if raw_title.is_empty() || raw_href.is_empty() {
            continue;
        }

        if raw_href.contains("duckduckgo.com/y.js") || raw_href.contains("/y.js?") {
            continue;
        }

        let resolved_url = unwrap_duckduckgo_url(raw_href);
        if !resolved_url.starts_with("http://") && !resolved_url.starts_with("https://") {
            continue;
        }

        let norm_url = normalize_url_for_dedup(&resolved_url);
        if !seen_urls.insert(norm_url) {
            continue;
        }

        let snippet = element
            .select(&DDG_SNIPPET_SELECTOR)
            .next()
            .map(|el| {
                el.text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();

        results.push(SearchResultItem {
            title: raw_title,
            url: resolved_url,
            snippet,
            engine: "duckduckgo".to_string(),
        });

        if results.len() >= limit {
            break;
        }
    }

    Ok(results)
}

/// Unwraps DuckDuckGo redirect URLs like `//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust%2Dlang.org%2F&rut=...`
pub fn unwrap_duckduckgo_url(raw_href: &str) -> String {
    let trimmed = raw_href.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let full_url_str = if trimmed.starts_with("//") {
        format!("https:{trimmed}")
    } else {
        trimmed.to_string()
    };

    if let Ok(parsed) = url::Url::parse(&full_url_str) {
        for (k, v) in parsed.query_pairs() {
            if k == "uddg" {
                let target = v.into_owned();
                if target.starts_with("http://") || target.starts_with("https://") {
                    return target;
                }
            }
        }
        return full_url_str;
    }

    trimmed.to_string()
}

/// Searches Brave Search with optional API key or web scraping fallback.
pub async fn search_brave(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, TklessError> {
    search_brave_with_options(client, query, limit, None, false).await
}

/// Searches Brave Search with optional CDP headless browser rendering or BRAVE_API_KEY.
pub async fn search_brave_with_options(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
    cdp_options: Option<&crate::cdp::CdpOptions>,
    auto_cdp: bool,
) -> Result<Vec<SearchResultItem>, TklessError> {
    let clean_query = query.trim();
    if clean_query.is_empty() {
        return Ok(Vec::new());
    }

    // 1. Check for official Brave Search API Key
    if let Ok(api_key) = std::env::var("BRAVE_API_KEY") {
        if !api_key.trim().is_empty() {
            let encoded_q: String =
                url::form_urlencoded::byte_serialize(clean_query.as_bytes()).collect();
            let api_url = format!(
                "https://api.search.brave.com/res/v1/web/search?q={encoded_q}&count={limit}"
            );
            if let Ok(resp) = client
                .get(&api_url)
                .header("Accept", "application/json")
                .header("X-Subscription-Token", api_key.trim())
                .send()
                .await
            {
                if resp.status().is_success() {
                    if let Ok(val) = resp.json::<serde_json::Value>().await {
                        return Ok(parse_brave_json(&val, limit));
                    }
                }
            }
        }
    }

    // 2. Web HTML endpoint
    let encoded_q: String = url::form_urlencoded::byte_serialize(clean_query.as_bytes()).collect();
    let search_url = format!("https://search.brave.com/search?q={encoded_q}");

    if let Some(cdp_opts) = cdp_options {
        let cdp_html = crate::cdp::fetch_html_via_cdp(&search_url, cdp_opts).await?;
        return parse_brave_html(&cdp_html, limit);
    }

    let resp = client
        .get(&search_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
        )
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "en-US,en;q=0.9,zh-CN;q=0.8")
        .send()
        .await?;

    let status = resp.status();
    let is_blocked = status == reqwest::StatusCode::FORBIDDEN
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS;

    if is_blocked && auto_cdp {
        let fallback_cdp = crate::cdp::CdpOptions {
            endpoint: "http://127.0.0.1:9222".to_string(),
            wait_ms: Some(2500),
            timeout_ms: Some(30_000),
            block_resources: Some(false),
            ..Default::default()
        };
        let cdp_html = crate::cdp::fetch_html_via_cdp(&search_url, &fallback_cdp).await?;
        return parse_brave_html(&cdp_html, limit);
    }

    if !status.is_success() {
        return Err(TklessError::SearchAntiScraping(format!(
            "Brave search returned HTTP status {status}"
        )));
    }

    let html = resp.text().await?;
    if (html.contains("cf-challenge") || html.contains("Checking your browser")) && auto_cdp {
        let fallback_cdp = crate::cdp::CdpOptions {
            endpoint: "http://127.0.0.1:9222".to_string(),
            wait_ms: Some(2500),
            timeout_ms: Some(30_000),
            block_resources: Some(false),
            ..Default::default()
        };
        let cdp_html = crate::cdp::fetch_html_via_cdp(&search_url, &fallback_cdp).await?;
        return parse_brave_html(&cdp_html, limit);
    }

    parse_brave_html(&html, limit)
}

/// Parses Brave Search REST API JSON payload.
pub fn parse_brave_json(val: &serde_json::Value, limit: usize) -> Vec<SearchResultItem> {
    let mut results = Vec::new();
    if let Some(res_arr) = val
        .get("web")
        .and_then(|w| w.get("results"))
        .and_then(|r| r.as_array())
    {
        for r in res_arr.iter().take(limit) {
            let title = r
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let url = r
                .get("url")
                .and_then(|u| u.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let snippet = r
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if !title.is_empty() && !url.is_empty() {
                results.push(SearchResultItem {
                    title,
                    url,
                    snippet,
                    engine: "brave".to_string(),
                });
            }
        }
    }
    results
}

/// Parses Brave Search HTML into structured items.
pub fn parse_brave_html(html: &str, limit: usize) -> Result<Vec<SearchResultItem>, TklessError> {
    let doc = Html::parse_document(html);
    let mut results = Vec::new();
    let mut seen_urls = std::collections::HashSet::new();

    for item in doc.select(&BRAVE_ITEM_SELECTOR) {
        let link_el = item.select(&BRAVE_TITLE_SELECTOR).next();
        let href = link_el
            .and_then(|el| el.value().attr("href"))
            .unwrap_or("")
            .trim();
        let title = link_el
            .map(|el| {
                el.text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();

        if href.is_empty() || !href.starts_with("http") || title.is_empty() {
            continue;
        }

        let snippet = item
            .select(&BRAVE_SNIPPET_SELECTOR)
            .next()
            .map(|el| {
                el.text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();

        let norm = normalize_url_for_dedup(href);
        if seen_urls.insert(norm) {
            results.push(SearchResultItem {
                title,
                url: href.to_string(),
                snippet,
                engine: "brave".to_string(),
            });
        }

        if results.len() >= limit {
            break;
        }
    }

    Ok(results)
}

/// Searches Yandex Search with default options.
pub async fn search_yandex(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, TklessError> {
    search_yandex_with_options(client, query, limit, None, false).await
}

/// Searches Yandex with optional CDP headless browser rendering to bypass anti-scraping challenges.
pub async fn search_yandex_with_options(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
    cdp_options: Option<&crate::cdp::CdpOptions>,
    auto_cdp: bool,
) -> Result<Vec<SearchResultItem>, TklessError> {
    let clean_query = query.trim();
    if clean_query.is_empty() {
        return Ok(Vec::new());
    }

    let encoded_q: String = url::form_urlencoded::byte_serialize(clean_query.as_bytes()).collect();
    let search_url = format!("https://yandex.com/search/?text={encoded_q}");

    if let Some(cdp_opts) = cdp_options {
        let cdp_html = crate::cdp::fetch_html_via_cdp(&search_url, cdp_opts).await?;
        return parse_yandex_html(&cdp_html, limit);
    }

    let resp = client
        .get(&search_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
        )
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "en-US,en;q=0.9,ru;q=0.8,zh;q=0.7")
        .send()
        .await?;

    let html = resp.text().await?;
    let is_captcha = html.contains("CheckboxCaptcha")
        || html.contains("smart-captcha")
        || html.contains("showcaptcha")
        || html.contains("yandex.com/showcaptcha");

    if is_captcha && auto_cdp {
        let fallback_cdp = crate::cdp::CdpOptions {
            endpoint: "http://127.0.0.1:9222".to_string(),
            wait_ms: Some(2500),
            timeout_ms: Some(30_000),
            block_resources: Some(false),
            ..Default::default()
        };
        let cdp_html = crate::cdp::fetch_html_via_cdp(&search_url, &fallback_cdp).await?;
        return parse_yandex_html(&cdp_html, limit);
    }

    if is_captcha {
        return Err(TklessError::SearchAntiScraping(
            "遭遇 Yandex 验证码挑战 (SmartCaptcha)。建议操作：开启 --auto-cdp 使用本地无头浏览器渲染。".to_string(),
        ));
    }

    parse_yandex_html(&html, limit)
}

/// Parses Yandex search results HTML into structured items.
pub fn parse_yandex_html(html: &str, limit: usize) -> Result<Vec<SearchResultItem>, TklessError> {
    let doc = Html::parse_document(html);
    let mut results = Vec::new();
    let mut seen_urls = std::collections::HashSet::new();

    for item in doc.select(&YANDEX_ITEM_SELECTOR) {
        let link_el = item.select(&YANDEX_TITLE_SELECTOR).next();
        let href = link_el
            .and_then(|el| el.value().attr("href"))
            .unwrap_or("")
            .trim();
        let title = link_el
            .map(|el| {
                el.text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();

        if href.is_empty() || !href.starts_with("http") || title.is_empty() {
            continue;
        }

        let snippet = item
            .select(&YANDEX_SNIPPET_SELECTOR)
            .next()
            .map(|el| {
                el.text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();

        let norm = normalize_url_for_dedup(href);
        if seen_urls.insert(norm) {
            results.push(SearchResultItem {
                title,
                url: href.to_string(),
                snippet,
                engine: "yandex".to_string(),
            });
        }

        if results.len() >= limit {
            break;
        }
    }

    Ok(results)
}

/// Searches SearXNG metasearch instance (reading SEARXNG_URL or defaulting to public instance).
pub async fn search_searxng(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, TklessError> {
    search_searxng_with_options(client, query, limit).await
}

/// Searches SearXNG with explicit options.
pub async fn search_searxng_with_options(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, TklessError> {
    let clean_query = query.trim();
    if clean_query.is_empty() {
        return Ok(Vec::new());
    }

    let base_url = std::env::var("SEARXNG_URL").unwrap_or_else(|_| "https://searx.be".to_string());
    let base_clean = base_url.trim_end_matches('/');

    let encoded_q: String = url::form_urlencoded::byte_serialize(clean_query.as_bytes()).collect();
    let api_url = format!("{base_clean}/search?q={encoded_q}&format=json&language=auto");

    let resp = client
        .get(&api_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 TkLess/0.4",
        )
        .header("Accept", "application/json")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(TklessError::SearchEngineError(format!(
            "SearXNG instance '{base_clean}' returned HTTP status {}",
            resp.status()
        )));
    }

    let text = resp.text().await?;
    if !text.trim_start().starts_with('{') {
        return Err(TklessError::SearchAntiScraping(format!(
            "SearXNG 实例 '{base_clean}' 返回了非 JSON 验证页面或遭遇 Cloudflare 拦截。建议指定私有部署端点 (--searxng-url 或 SEARXNG_URL)。"
        )));
    }
    let val: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| TklessError::SearchEngineError(e.to_string()))?;
    Ok(parse_searxng_json(&val, limit))
}

/// Parses SearXNG JSON API response.
pub fn parse_searxng_json(val: &serde_json::Value, limit: usize) -> Vec<SearchResultItem> {
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();

    if let Some(res_arr) = val.get("results").and_then(|r| r.as_array()) {
        for item in res_arr {
            let title = item
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let url = item
                .get("url")
                .and_then(|u| u.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let snippet = item
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .trim()
                .to_string();

            if !title.is_empty() && url.starts_with("http") {
                let norm = normalize_url_for_dedup(&url);
                if seen.insert(norm) {
                    results.push(SearchResultItem {
                        title,
                        url,
                        snippet,
                        engine: "searxng".to_string(),
                    });
                }
            }

            if results.len() >= limit {
                break;
            }
        }
    }
    results
}

/// Unwraps Google redirect URLs (e.g. `/url?q=https://...&sa=...`).
pub fn unwrap_google_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with("/url?") {
        let fake = format!("https://www.google.com{trimmed}");
        if let Ok(parsed) = url::Url::parse(&fake) {
            for (k, v) in parsed.query_pairs() {
                if k == "q" || k == "url" {
                    let target = v.into_owned();
                    if target.starts_with("http://") || target.starts_with("https://") {
                        return target;
                    }
                }
            }
        }
    }
    trimmed.to_string()
}

/// Searches Google Search with default options.
pub async fn search_google(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, TklessError> {
    search_google_with_options(client, query, limit, None, false).await
}

/// Searches Google with optional CDP headless browser rendering to bypass anti-scraping challenges.
pub async fn search_google_with_options(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
    cdp_options: Option<&crate::cdp::CdpOptions>,
    auto_cdp: bool,
) -> Result<Vec<SearchResultItem>, TklessError> {
    let clean_query = query.trim();
    if clean_query.is_empty() {
        return Ok(Vec::new());
    }

    let encoded_q: String = url::form_urlencoded::byte_serialize(clean_query.as_bytes()).collect();
    let search_url =
        format!("https://www.google.com/search?q={encoded_q}&hl=en&num={limit}&gws_rd=cr&pws=0");

    if let Some(cdp_opts) = cdp_options {
        let cdp_html = crate::cdp::fetch_html_via_cdp(&search_url, cdp_opts).await?;
        return parse_google_html(&cdp_html, limit);
    }

    let resp = client
        .get(&search_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36",
        )
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await?;

    let status = resp.status();
    let is_blocked = status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::FORBIDDEN;
    let html = resp.text().await?;
    let has_sorry =
        html.contains("sorry/index") || html.contains("recaptcha") || html.contains("enablejs");

    if (is_blocked || has_sorry) && auto_cdp {
        let fallback_cdp = crate::cdp::CdpOptions {
            endpoint: "http://127.0.0.1:9222".to_string(),
            wait_ms: Some(2500),
            timeout_ms: Some(30_000),
            block_resources: Some(false),
            ..Default::default()
        };
        let cdp_html = crate::cdp::fetch_html_via_cdp(&search_url, &fallback_cdp).await?;
        return parse_google_html(&cdp_html, limit);
    }

    if is_blocked || has_sorry {
        return Err(TklessError::SearchAntiScraping(
            "遭遇 Google JS 动态验证或反爬拦截 (enablejs / recaptcha)。建议操作：开启 --auto-cdp 或使用默认多引擎。".to_string(),
        ));
    }

    parse_google_html(&html, limit)
}

/// Parses Google search results HTML into structured items.
pub fn parse_google_html(html: &str, limit: usize) -> Result<Vec<SearchResultItem>, TklessError> {
    let doc = Html::parse_document(html);
    let mut results = Vec::new();
    let mut seen_urls = std::collections::HashSet::new();

    for item in doc.select(&GOOGLE_ITEM_SELECTOR) {
        let title_el = item.select(&GOOGLE_TITLE_SELECTOR).next();
        let title = title_el
            .map(|el| {
                el.text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();

        let link_el = item.select(&GOOGLE_LINK_SELECTOR).next();
        let raw_href = link_el
            .and_then(|el| el.value().attr("href"))
            .unwrap_or("")
            .trim();

        let href = unwrap_google_url(raw_href);

        if href.is_empty() || !href.starts_with("http") || title.is_empty() {
            continue;
        }

        let snippet = item
            .select(&GOOGLE_SNIPPET_SELECTOR)
            .next()
            .map(|el| {
                el.text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();

        let norm = normalize_url_for_dedup(&href);
        if seen_urls.insert(norm) {
            results.push(SearchResultItem {
                title,
                url: href,
                snippet,
                engine: "google".to_string(),
            });
        }

        if results.len() >= limit {
            break;
        }
    }

    Ok(results)
}

/// Merges search results from arbitrary number of engines round-robin with URL deduplication.
pub fn merge_multi_engine_search_results(
    engine_lists: Vec<Vec<SearchResultItem>>,
    limit: usize,
) -> Vec<SearchResultItem> {
    let mut merged = Vec::with_capacity(limit);
    let mut seen = std::collections::HashSet::new();
    let mut iters: Vec<_> = engine_lists
        .into_iter()
        .map(|list| list.into_iter())
        .collect();

    while merged.len() < limit {
        let mut progress = false;
        for iter in &mut iters {
            if merged.len() >= limit {
                break;
            }
            for item in iter.by_ref() {
                let norm = normalize_url_for_dedup(&item.url);
                if seen.insert(norm) {
                    merged.push(item);
                    progress = true;
                    break;
                }
            }
        }
        if !progress {
            break;
        }
    }
    merged
}

/// Normalizes a URL for deduplication across search engines.
pub fn normalize_url_for_dedup(raw: &str) -> String {
    let mut norm = raw.trim().to_lowercase();
    if let Some(stripped) = norm.strip_prefix("http://") {
        norm = format!("https://{stripped}");
    }
    if let Some(stripped) = norm.strip_prefix("https://www.") {
        norm = format!("https://{stripped}");
    }
    norm.trim_end_matches('/').to_string()
}

/// Merges search results from two engines, interleaving items while deduplicating by normalized URL.
pub fn merge_and_dedup_search_results(
    engine1_results: Vec<SearchResultItem>,
    engine2_results: Vec<SearchResultItem>,
    limit: usize,
) -> Vec<SearchResultItem> {
    merge_multi_engine_search_results(vec![engine1_results, engine2_results], limit)
}

/// Formats search results as a clean GitHub Flavored Markdown document with engine source attribution.
pub fn format_search_results_markdown(query: &str, results: &[SearchResultItem]) -> String {
    let mut out = String::new();
    let engine_name = if results.iter().any(|r| r.engine == "duckduckgo")
        && results.iter().any(|r| r.engine == "bing")
    {
        "all"
    } else {
        results
            .first()
            .map(|r| r.engine.as_str())
            .unwrap_or("search")
    };

    out.push_str(&format!("# 网页搜索 ({engine_name}): \"{query}\"\n\n"));

    if results.is_empty() {
        out.push_str("⚠️ 未检索到符合查询约束的高置信度网页结果（已自动甄别并剔除广告推广与首词降级条目）。\n");
        out.push_str("> 建议操作：1. 简化关键词；2. 去除年份等强时效限定；3. 指定平台定向参数（如 --zhihu, --baike, --so）。\n");
        return out;
    }

    for (idx, item) in results.iter().enumerate() {
        out.push_str(&format!("{}. [{}]({})\n", idx + 1, item.title, item.url));
        if !item.snippet.is_empty() {
            out.push_str(&format!("   > {}\n", item.snippet));
        }
        out.push('\n');
    }

    out
}

/// Formats combined search & fetch results into an integrated research report for LLMs.
pub fn format_search_fetch_markdown(query: &str, items: &[SearchFetchItem]) -> String {
    let mut out = String::new();
    let engine_name = if items.iter().any(|r| r.engine == "duckduckgo")
        && items.iter().any(|r| r.engine == "bing")
    {
        "all"
    } else {
        items.first().map(|r| r.engine.as_str()).unwrap_or("search")
    };

    let total_fetched = items.iter().filter(|i| i.content.is_some()).count();
    let total_tokens: usize = items.iter().map(|i| i.token_count).sum();

    out.push_str(&format!(
        "# 搜索与正文集成调研 ({engine_name}): \"{query}\"\n\n> 检索到 {} 条结果 | 成功提取 {} 篇正文 | 累计估算 Tokens: {}\n\n---\n\n",
        items.len(),
        total_fetched,
        total_tokens
    ));

    if items.is_empty() {
        out.push_str("未检索到相关网页结果。\n");
        return out;
    }

    for item in items {
        out.push_str(&format!(
            "## {}. [{}]({})\n\n",
            item.rank, item.title, item.url
        ));

        if !item.snippet.is_empty() {
            out.push_str(&format!("> **摘要**: {}\n\n", item.snippet));
        }

        if let Some(ref content) = item.content {
            out.push_str(&format!("### 正文速览 (Tokens: {}):\n\n", item.token_count));
            out.push_str(content.trim());
            out.push_str("\n\n---\n\n");
        } else if let Some(ref err) = item.error {
            out.push_str(&format!("> *[正文拉取异常: {err}]*\n\n---\n\n"));
        } else {
            out.push_str("> *[未获取到正文内容]*\n\n---\n\n");
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bing_html_sample() {
        let sample = r#"
            <ol id="b_results">
                <li class="b_algo">
                    <h2><a href="https://www.rust-lang.org/">Rust Programming Language</a></h2>
                    <div class="b_caption">
                        <p>A language empowering everyone to build reliable and efficient software.</p>
                    </div>
                </li>
                <li class="b_algo">
                    <h2><a href="https://doc.rust-lang.org/book/">The Rust Programming Language - Book</a></h2>
                    <div class="b_caption">
                        <p>The Book is the primary official resource for learning Rust.</p>
                    </div>
                </li>
            </ol>
        "#;

        let results = parse_bing_html(sample, 5).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert!(results[0].snippet.contains("empowering everyone"));
        assert_eq!(results[0].engine, "bing");
    }

    #[test]
    fn test_unwrap_bing_url() {
        let bing_url =
            "https://www.bing.com/ck/a?!&&p=abc&u=a1aHR0cHM6Ly9ydXN0LWxhbmcub3JnLw&ntb=1";
        let unwrapped = unwrap_bing_url(bing_url);
        assert_eq!(unwrapped, "https://rust-lang.org/");

        let direct_url = "https://example.com/rust";
        assert_eq!(unwrap_bing_url(direct_url), "https://example.com/rust");
    }

    #[test]
    fn test_format_search_fetch_markdown() {
        let items = vec![
            SearchFetchItem {
                rank: 1,
                title: "Rust Guide".to_string(),
                url: "https://www.rust-lang.org/".to_string(),
                snippet: "A language empowering everyone".to_string(),
                engine: "bing".to_string(),
                content: Some("# Rust\nFast and reliable.".to_string()),
                token_count: 50,
                status: 200,
                error: None,
            },
            SearchFetchItem {
                rank: 2,
                title: "Failed Page".to_string(),
                url: "https://example.com/404".to_string(),
                snippet: "Missing".to_string(),
                engine: "bing".to_string(),
                content: None,
                token_count: 0,
                status: 404,
                error: Some("HTTP error 404".to_string()),
            },
        ];

        let md = format_search_fetch_markdown("Rust", &items);
        assert!(md.contains("# 搜索与正文集成调研 (bing): \"Rust\""));
        assert!(md.contains("成功提取 1 篇正文"));
        assert!(md.contains("## 1. [Rust Guide](https://www.rust-lang.org/)"));
        assert!(md.contains("Fast and reliable."));
        assert!(md.contains("HTTP error 404"));
    }

    #[test]
    fn test_search_simhash_deduplication() {
        let text_orig = r#"
            TkLess is a high-performance, token-efficient web content extraction engine designed specifically
            for Large Language Models and AI Agents. It removes navigation bars, scripts, advertisements, and
            boilerplate using heuristic algorithms, converting the core content into LLM-optimized Markdown.
            Native adapters for Bilibili, WeChat, Zhihu, and GitHub are supported out of the box with zero overhead.
            By eliminating aggressive noise, TkLess delivers significant token savings across production AI pipelines.
        "#;
        let text_mirror = r#"
            TkLess is a high-performance, token-efficient web content extraction engine designed specifically
            for Large Language Models and AI Agents. It removes navigation bars, scripts, advertisements, and
            boilerplate using heuristic algorithms, converting the core content into LLM-optimized Markdown.
            Native adapters for Bilibili, WeChat, Zhihu, and GitHub are supported out of the box with zero overhead.
            By eliminating aggressive noise, TkLess delivers significant token savings across production AI pipelines.
            Reposted with permission.
        "#;
        let text_diff = "Tokio is an asynchronous runtime for the Rust programming language, providing event-driven non-blocking I/O.";

        let items = vec![
            SearchFetchItem {
                rank: 1,
                title: "Original Post".to_string(),
                url: "https://example.com/original".to_string(),
                snippet: "Snippet 1".to_string(),
                engine: "bing".to_string(),
                content: Some(text_orig.to_string()),
                token_count: 50,
                status: 200,
                error: None,
            },
            SearchFetchItem {
                rank: 2,
                title: "Mirror Site Copy".to_string(),
                url: "https://mirror.example/repost".to_string(),
                snippet: "Snippet 2".to_string(),
                engine: "bing".to_string(),
                content: Some(text_mirror.to_string()),
                token_count: 50,
                status: 200,
                error: None,
            },
            SearchFetchItem {
                rank: 3,
                title: "Different Subject".to_string(),
                url: "https://tokio.rs/overview".to_string(),
                snippet: "Snippet 3".to_string(),
                engine: "bing".to_string(),
                content: Some(text_diff.to_string()),
                token_count: 40,
                status: 200,
                error: None,
            },
        ];

        let mut seen = Vec::new();
        let mut deduped = Vec::new();
        for item in items {
            if let Some(ref text) = item.content {
                let h = crate::dedup::simhash(text);
                if seen
                    .iter()
                    .any(|&prev| crate::dedup::is_near_duplicate(prev, h, 3))
                {
                    continue;
                }
                seen.push(h);
            }
            deduped.push(item);
        }

        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].url, "https://example.com/original");
        assert_eq!(deduped[1].url, "https://tokio.rs/overview");
    }

    #[test]
    fn test_tokenize_query_and_temporal() {
        let tokens = tokenize_query("北京 小学 入学 政策 2026");
        assert_eq!(tokens.core_tokens, vec!["北京", "小学", "入学", "政策"]);
        assert_eq!(tokens.temporal_tokens, vec!["2026"]);

        let tokens_en = tokenize_query("Tesla Model 3 Production 2025");
        assert_eq!(
            tokens_en.core_tokens,
            vec!["tesla", "model", "3", "production"]
        );
        assert_eq!(tokens_en.temporal_tokens, vec!["2025"]);
    }

    #[test]
    fn test_relevance_first_token_drop_and_zero_relevance() {
        let query_tokens = tokenize_query("北京 小学 入学 政策 2026");

        // 1. First-token-only fallback (Term Drop): matches ONLY "北京", misses "小学", "入学", "政策"
        let fallback_title = "北京市 - 百度百科";
        let fallback_snip = "北京市，简称“京”，是中华人民共和国首都、直辖市、国家中心城市。";
        let fallback_score = calculate_relevance(fallback_title, fallback_snip, &query_tokens);
        assert!(
            fallback_score.is_none(),
            "First-token-only fallback (北京市百科) must be rejected"
        );

        // 2. Completely unrelated item (Zero Core Tokens matched, e.g. 阿里云盘 or 淘宝)
        let ad_title = "阿里云盘 - 备份您的照片与文件";
        let ad_snip = "不限速的高速云存储网盘，支持大文件快速上传下载。";
        let ad_score = calculate_relevance(ad_title, ad_snip, &query_tokens);
        assert!(
            ad_score.is_none(),
            "Completely unrelated promotion item must be rejected"
        );

        // 3. Genuine relevant result: matches "北京", "小学", "入学", "政策", "2026"
        let good_title = "2026年北京各区小学入学政策与信息采集指南公布";
        let good_snip =
            "北京市教委正式发布2026年义务教育阶段小学入学政策，明确报名流程与居住证要求。";
        let good_score = calculate_relevance(good_title, good_snip, &query_tokens);
        assert!(
            good_score.is_some(),
            "Genuine relevant policy article must be accepted"
        );
        assert!(good_score.unwrap() >= 40);
    }

    #[test]
    fn test_parse_bing_html_with_ad_and_relevance_filters() {
        let sample = r#"
            <ol id="b_results">
                <!-- 1. Disguised promotion / ad item -->
                <li class="b_algo b_ad">
                    <h2><a href="https://www.douyin.com/">抖音-记录美好生活</a></h2>
                    <div class="b_caption">
                        <p>抖音让每一个人看见并连接更大的世界，鼓励表达、沟通和记录。</p>
                    </div>
                </li>
                <!-- 2. Term Drop fallback: only matches "北京" -->
                <li class="b_algo">
                    <h2><a href="https://baike.baidu.com/item/%E5%8C%97%E4%BA%AC%E5%B8%82">北京市 - 百度百科</a></h2>
                    <div class="b_caption">
                        <p>北京市，简称“京”，是中华人民共和国首都、国家中心城市。</p>
                    </div>
                </li>
                <!-- 3. Genuine relevant result -->
                <li class="b_algo">
                    <h2><a href="https://jw.beijing.gov.cn/policy/2026">2026年北京小学入学政策官方解答</a></h2>
                    <div class="b_caption">
                        <p>北京市教委发布2026小学入学政策，详述幼升小网上信息采集时间与入学顺位要求。</p>
                    </div>
                </li>
            </ol>
        "#;

        let query = "北京 小学 入学 政策 2026";
        let results = parse_bing_html_with_query(sample, query, 5).unwrap();

        // Must filter out the Douyin ad and the Beijing encyclopedia fallback
        assert_eq!(
            results.len(),
            1,
            "Only the genuine relevant policy result should survive"
        );
        assert_eq!(results[0].title, "2026年北京小学入学政策官方解答");
        assert_eq!(results[0].url, "https://jw.beijing.gov.cn/policy/2026");
    }

    #[test]
    fn test_baike_openapi_response_parsing() {
        let sample_json = serde_json::json!({
            "title": "光合作用",
            "url": "http://baike.baidu.com/subview/8885/20565692.htm",
            "abstract": "光合作用是绿色植物生命活动的核心过程，其本质在于将外界光能转化为化学能。"
        });

        let title = sample_json.get("title").and_then(|v| v.as_str()).unwrap();
        let raw_url = sample_json.get("url").and_then(|v| v.as_str()).unwrap();
        let url = if raw_url.starts_with("http://") {
            raw_url.replace("http://", "https://")
        } else {
            raw_url.to_string()
        };
        let snippet = sample_json
            .get("abstract")
            .and_then(|v| v.as_str())
            .unwrap();

        let item = SearchResultItem {
            title: format!("{title}_百度百科"),
            url,
            snippet: snippet.to_string(),
            engine: "baike".to_string(),
        };

        assert_eq!(item.title, "光合作用_百度百科");
        assert_eq!(
            item.url,
            "https://baike.baidu.com/subview/8885/20565692.htm"
        );
        assert!(item.snippet.contains("光合作用是绿色植物"));
        assert_eq!(item.engine, "baike");
    }

    #[test]
    fn test_sogou_wechat_search_html_parsing() {
        let sample_html = r#"
            <ul class="news-list">
                <li>
                    <div class="txt-box">
                        <h3>
                            <a target="_blank" href="/link?url=sogou_sample_test_link" id="sogou_title_0">
                                <em><!--red_beg-->大模型<!--red_end--></em>想落地，先让大家用得起
                            </a>
                        </h3>
                        <p class="txt-info" id="sogou_summary_0">
                            自2023年开始，全球范围掀起了以大模型为代表的生成式AI浪潮。
                        </p>
                        <div class="s-p">
                            <span class="all-time-y2">钛媒体APP</span>
                        </div>
                    </div>
                </li>
            </ul>
        "#;

        let doc = Html::parse_document(sample_html);
        let box_sel = Selector::parse("div.txt-box").unwrap();
        let title_sel = Selector::parse("h3 a").unwrap();
        let snip_sel = Selector::parse("p.txt-info").unwrap();

        let mut results = Vec::new();
        for el in doc.select(&box_sel) {
            let title_node = el.select(&title_sel).next().unwrap();
            let raw_title = title_node.text().collect::<String>().trim().to_string();
            let raw_href = title_node.value().attr("href").unwrap().trim();
            let snippet = el
                .select(&snip_sel)
                .next()
                .unwrap()
                .text()
                .collect::<String>()
                .trim()
                .to_string();

            results.push(SearchResultItem {
                title: raw_title,
                url: format!("https://weixin.sogou.com{raw_href}"),
                snippet,
                engine: "wechat".to_string(),
            });
        }

        assert_eq!(results.len(), 1);
        assert!(results[0].title.contains("大模型想落地"));
        assert_eq!(
            results[0].url,
            "https://weixin.sogou.com/link?url=sogou_sample_test_link"
        );
        assert!(results[0].snippet.contains("以大模型为代表"));
        assert_eq!(results[0].engine, "wechat");
    }

    #[test]
    fn test_unwrap_duckduckgo_url() {
        let ddg_redirect =
            "//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust%2Dlang.org%2F&rut=abc123xyz";
        assert_eq!(
            unwrap_duckduckgo_url(ddg_redirect),
            "https://www.rust-lang.org/"
        );

        let direct_url = "https://doc.rust-lang.org/book/";
        assert_eq!(
            unwrap_duckduckgo_url(direct_url),
            "https://doc.rust-lang.org/book/"
        );

        let proto_rel = "//github.com/rust-lang/rust";
        assert_eq!(
            unwrap_duckduckgo_url(proto_rel),
            "https://github.com/rust-lang/rust"
        );
    }

    #[test]
    fn test_normalize_url_for_dedup() {
        assert_eq!(
            normalize_url_for_dedup("https://www.rust-lang.org/"),
            "https://rust-lang.org"
        );
        assert_eq!(
            normalize_url_for_dedup("http://rust-lang.org"),
            "https://rust-lang.org"
        );
        assert_eq!(
            normalize_url_for_dedup("HTTPS://RUST-LANG.ORG/"),
            "https://rust-lang.org"
        );
    }

    #[test]
    fn test_merge_and_dedup_search_results() {
        let bing_items = vec![
            SearchResultItem {
                title: "Rust Official (Bing)".to_string(),
                url: "https://www.rust-lang.org/".to_string(),
                snippet: "Empowering everyone...".to_string(),
                engine: "bing".to_string(),
            },
            SearchResultItem {
                title: "Rust Book (Bing)".to_string(),
                url: "https://doc.rust-lang.org/book/".to_string(),
                snippet: "The Book is the primary...".to_string(),
                engine: "bing".to_string(),
            },
        ];

        let ddg_items = vec![
            SearchResultItem {
                title: "Rust Official (DDG)".to_string(),
                url: "https://rust-lang.org".to_string(), // duplicate of bing_items[0]
                snippet: "Fast, memory-safe...".to_string(),
                engine: "duckduckgo".to_string(),
            },
            SearchResultItem {
                title: "Rust Wikipedia (DDG)".to_string(),
                url: "https://en.wikipedia.org/wiki/Rust_(programming_language)".to_string(),
                snippet: "Rust is a general-purpose...".to_string(),
                engine: "duckduckgo".to_string(),
            },
        ];

        let merged = merge_and_dedup_search_results(bing_items, ddg_items, 5);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].title, "Rust Official (Bing)");
        assert_eq!(merged[0].engine, "bing");
        // Second item should be ddg Wikipedia because ddg[0] was deduplicated with bing[0]
        assert_eq!(merged[1].title, "Rust Wikipedia (DDG)");
        assert_eq!(merged[1].engine, "duckduckgo");
        assert_eq!(merged[2].title, "Rust Book (Bing)");
        assert_eq!(merged[2].engine, "bing");
    }

    #[test]
    fn test_parse_duckduckgo_html() {
        let sample_html = r#"
            <div class="results">
                <!-- 1. Ad container with result--ad -->
                <div class="result results_links result--ad">
                    <a class="result__a" href="https://example.com/ad">Ad Product</a>
                    <a class="result__snippet">Buy this now</a>
                </div>

                <!-- 2. Ad container with duckduckgo.com/y.js link -->
                <div class="result results_links">
                    <a class="result__a" href="//duckduckgo.com/y.js?u=sponsored">Sponsored Ad</a>
                    <a class="result__snippet">Sponsored link</a>
                </div>

                <!-- 3. Organic item with redirect -->
                <div class="result results_links">
                    <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust%2Dlang.org%2F&rut=1">
                        <b>Rust</b> Programming Language
                    </a>
                    <a class="result__snippet">
                        <b>Rust</b> is a fast, memory-safe programming language.
                    </a>
                </div>

                <!-- 4. Organic item with direct URL -->
                <div class="result results_links">
                    <a class="result__a" href="https://doc.rust-lang.org/book/">
                        The Rust Book
                    </a>
                    <a class="result__snippet">
                        Official book for learning Rust.
                    </a>
                </div>
            </div>
        "#;

        let results = parse_duckduckgo_html(sample_html, 5).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert!(results[0].snippet.contains("memory-safe"));
        assert_eq!(results[0].engine, "duckduckgo");

        assert_eq!(results[1].title, "The Rust Book");
        assert_eq!(results[1].url, "https://doc.rust-lang.org/book/");
        assert_eq!(results[1].engine, "duckduckgo");
    }

    #[test]
    fn test_tokenize_query_domain_and_phrase_extraction() {
        let tokens = tokenize_query("site:zhuanlan.zhihu.com DeepSeek 深度解析");
        assert_eq!(tokens.domain_filters, vec!["zhuanlan.zhihu.com"]);
        assert_eq!(
            tokens.core_tokens,
            vec!["deepseek", "深度解析", "深度", "解析"]
        );
        assert!(tokens.phrase_filters.is_empty());

        let tokens_quote = tokenize_query("\"small language model\" release");
        assert_eq!(tokens_quote.phrase_filters, vec!["small language model"]);
        assert_eq!(
            tokens_quote.core_tokens,
            vec!["small", "language", "model", "release"]
        );
        // Ensure no quotes polluted the individual core tokens
        for t in &tokens_quote.core_tokens {
            assert!(!t.contains('"'), "Token {t} must not contain quotes");
        }
    }

    #[test]
    fn test_relevance_space_insensitive_and_domain_boost() {
        let tokens_hf = tokenize_query("huggingface trending");
        let title_hf = "Trending Papers - Hugging Face";
        let snip_hf = "Explore community trending models and papers.";
        let score_hf = calculate_relevance_full(
            title_hf,
            snip_hf,
            "https://huggingface.co/papers/trending",
            &tokens_hf,
        );
        assert!(
            score_hf.is_some(),
            "Hugging Face with spaces must match huggingface token"
        );
        assert!(score_hf.unwrap() >= 25);

        let tokens_zhihu = tokenize_query("site:zhuanlan.zhihu.com DeepSeek 深度解析");
        let title_zhihu = "DeepSeek 核心架构与注意力机制详解";
        let snip_zhihu = "深入分析 DeepSeek 的工程实践。";
        let url_zhihu = "https://zhuanlan.zhihu.com/p/12345678";
        let score_zhihu =
            calculate_relevance_full(title_zhihu, snip_zhihu, url_zhihu, &tokens_zhihu);
        assert!(
            score_zhihu.is_some(),
            "Matching domain filter must grant score boost and survive"
        );
        assert!(score_zhihu.unwrap() >= 40);
    }

    #[test]
    fn test_relevance_relaxed_fallback() {
        let tokens_llama = tokenize_query("llama 3.2 1b 3b");
        let title_llama = "Llama · Your AI, on your computer";
        let snip_llama = "Open source artificial intelligence models by Meta.";
        let url_llama = "https://www.llama.com/";
        let relaxed_score =
            calculate_relaxed_relevance(title_llama, snip_llama, url_llama, &tokens_llama);
        assert!(
            relaxed_score.is_some(),
            "Llama query must pass relaxed fallback scoring"
        );
    }

    #[test]
    fn test_parse_brave_html() {
        let sample = r#"
        <div id="results">
            <div class="snippet" data-type="web">
                <a class="heading-serpresult" href="https://www.rust-lang.org/">Rust Programming Language</a>
                <p class="snippet-description">A language empowering everyone to build reliable software.</p>
            </div>
            <div class="snippet" data-type="web">
                <a class="heading-serpresult" href="https://doc.rust-lang.org/book/">The Rust Programming Language</a>
                <p class="snippet-description">The official book for learning Rust.</p>
            </div>
        </div>
        "#;
        let results = parse_brave_html(sample, 5).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert_eq!(results[0].engine, "brave");
        assert!(results[0].snippet.contains("reliable software"));
    }

    #[test]
    fn test_parse_brave_json() {
        let sample = serde_json::json!({
            "web": {
                "results": [
                    {
                        "title": "TkLess on GitHub",
                        "url": "https://github.com/smallhug/tkless",
                        "description": "Spend less tokens, perceive more web."
                    }
                ]
            }
        });
        let results = parse_brave_json(&sample, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "TkLess on GitHub");
        assert_eq!(results[0].url, "https://github.com/smallhug/tkless");
        assert_eq!(results[0].engine, "brave");
    }

    #[test]
    fn test_parse_yandex_html() {
        let sample = r#"
        <ul class="serp-list">
            <li class="serp-item">
                <h2 class="OrganicTitle"><a class="OrganicTitle-Link" href="https://www.rust-lang.org/">Rust Language</a></h2>
                <div class="OrganicText">Fast and memory-efficient.</div>
            </li>
        </ul>
        "#;
        let results = parse_yandex_html(sample, 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert_eq!(results[0].engine, "yandex");
        assert!(results[0].snippet.contains("Fast and memory-efficient"));
    }

    #[test]
    fn test_parse_searxng_json() {
        let sample = serde_json::json!({
            "results": [
                {
                    "title": "SearXNG Project",
                    "url": "https://docs.searxng.org/",
                    "content": "A privacy-respecting, hackable metasearch engine.",
                    "engine": "google"
                }
            ]
        });
        let results = parse_searxng_json(&sample, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "SearXNG Project");
        assert_eq!(results[0].url, "https://docs.searxng.org/");
        assert_eq!(results[0].engine, "searxng");
    }

    #[test]
    fn test_unwrap_google_url_and_parse_html() {
        let raw = "/url?q=https://www.rust-lang.org/&sa=U&ved=2ahUKEwj";
        assert_eq!(unwrap_google_url(raw), "https://www.rust-lang.org/");

        let sample = r#"
        <div id="search">
            <div class="g">
                <a href="/url?q=https://www.rust-lang.org/&sa=U"><h3>Rust Language</h3></a>
                <div class="VwiC3b">Empowering everyone to build reliable software.</div>
            </div>
        </div>
        "#;
        let results = parse_google_html(sample, 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert_eq!(results[0].engine, "google");
    }

    #[test]
    fn test_merge_multi_engine_search_results() {
        let list1 = vec![
            SearchResultItem {
                title: "A1".to_string(),
                url: "https://a.com/1".to_string(),
                snippet: "s1".to_string(),
                engine: "bing".to_string(),
            },
            SearchResultItem {
                title: "A2".to_string(),
                url: "https://a.com/2".to_string(),
                snippet: "s2".to_string(),
                engine: "bing".to_string(),
            },
        ];
        let list2 = vec![SearchResultItem {
            title: "B1".to_string(),
            url: "https://b.com/1".to_string(),
            snippet: "sb1".to_string(),
            engine: "duckduckgo".to_string(),
        }];
        let list3 = vec![SearchResultItem {
            title: "C1".to_string(),
            url: "https://c.com/1".to_string(),
            snippet: "sc1".to_string(),
            engine: "brave".to_string(),
        }];

        let merged = merge_multi_engine_search_results(vec![list1, list2, list3], 5);
        assert_eq!(merged.len(), 4);
        assert_eq!(merged[0].title, "A1");
        assert_eq!(merged[1].title, "B1");
        assert_eq!(merged[2].title, "C1");
        assert_eq!(merged[3].title, "A2");
    }
}
