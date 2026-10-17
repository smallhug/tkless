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

    // 3. General web search via Bing with multi-stage relaxation and anti-degradation
    match engine {
        SearchEngineKind::Bing => {
            search_bing_with_options(client, clean_query, limit, cdp_options, auto_cdp).await
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

/// Tokenized query elements separated into core entity tokens and temporal tokens.
#[derive(Debug, Clone)]
pub struct QueryTokens {
    /// Non-temporal core content tokens (e.g. "特斯拉", "上海工厂", "产量")
    pub core_tokens: Vec<String>,
    /// Temporal modifier tokens (e.g. "2026", "2025")
    pub temporal_tokens: Vec<String>,
    /// All raw tokens
    pub all_tokens: Vec<String>,
}

/// Tokenizes a user query into core semantic tokens and temporal modifiers.
pub fn tokenize_query(query: &str) -> QueryTokens {
    let clean = query.trim().to_lowercase();
    let raw_tokens: Vec<&str> = clean.split_whitespace().collect();

    let mut core_tokens = Vec::new();
    let mut temporal_tokens = Vec::new();
    let mut all_tokens = Vec::new();

    for token in raw_tokens {
        let t = token.trim();
        if t.is_empty() {
            continue;
        }
        all_tokens.push(t.to_string());
        // Check if 4-digit year (e.g. 2024, 2025, 2026, 2027)
        if t.len() == 4 && t.chars().all(|c| c.is_ascii_digit()) {
            temporal_tokens.push(t.to_string());
        } else {
            core_tokens.push(t.to_string());
        }
    }

    QueryTokens {
        core_tokens,
        temporal_tokens,
        all_tokens,
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

    false
}

/// Evaluates query relevance and scores a candidate result.
/// Returns None if the candidate is completely unrelated or a first-token-only fallback.
pub fn calculate_relevance(title: &str, snippet: &str, query_tokens: &QueryTokens) -> Option<i32> {
    if query_tokens.core_tokens.is_empty() {
        return Some(10);
    }

    let title_lower = title.to_lowercase();
    let snippet_lower = snippet.to_lowercase();

    let mut matched_core = Vec::new();
    let mut score = 0;

    for (idx, token) in query_tokens.core_tokens.iter().enumerate() {
        let in_title = title_lower.contains(token);
        let in_snippet = snippet_lower.contains(token);

        if in_title || in_snippet {
            matched_core.push(idx);
            if in_title {
                score += 15;
            }
            if in_snippet {
                score += 5;
            }
        }
    }

    // Hard Filter 1: Zero Core Tokens Matched -> Completely unrelated (e.g. 阿里云盘 for 个人所得税)
    if matched_core.is_empty() {
        return None;
    }

    // Hard Filter 2: First-Token-Only Fallback (Bing Term Drop)
    // If user asked a multi-token query (>=2 core tokens), but the result ONLY matches token 0,
    // and matches NONE of the other 1..N core tokens (e.g. "北京 小学 入学 政策" -> only matches "北京", becoming "北京市 - 百度百科"),
    // this is a broad degradation fallback where all constraints were lost. REJECT it!
    if query_tokens.core_tokens.len() >= 2 {
        if matched_core == vec![0] {
            return None;
        }
        // If 3 or more core tokens, require at least 2 matched core tokens
        if query_tokens.core_tokens.len() >= 3 && matched_core.len() < 2 {
            return None;
        }
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
    if !results.is_empty() {
        return Ok(results);
    }

    let tokens = tokenize_query(clean_query);

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
    let encoded_q: String = url::form_urlencoded::byte_serialize(search_query.as_bytes()).collect();
    // Use clean global search URL without &setlang=... which disrupts backend search query routing
    let search_url = format!("https://www.bing.com/search?q={encoded_q}");

    debug!(query = search_query, url = %search_url, "Executing Bing web search");

    // If explicit CDP options provided, fetch directly via headless Chrome
    if let Some(cdp_opts) = cdp_options {
        let html = crate::cdp::fetch_html_via_cdp(&search_url, cdp_opts).await?;
        return parse_bing_html_with_query(&html, relevance_query, limit);
    }

    let resp = client
        .get(&search_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 Edg/133.0.0.0",
        )
        .header("Referer", "https://www.bing.com/")
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8")
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("Sec-Ch-Ua", "\"Not(A:Brand\";v=\"99\", \"Microsoft Edge\";v=\"133\", \"Chromium\";v=\"133\"")
        .header("Sec-Ch-Ua-Mobile", "?0")
        .header("Sec-Ch-Ua-Platform", "\"Windows\"")
        .header("Sec-Fetch-Dest", "document")
        .header("Sec-Fetch-Mode", "navigate")
        .header("Sec-Fetch-Site", "same-origin")
        .header("Sec-Fetch-User", "?1")
        .header("Upgrade-Insecure-Requests", "1")
        .send()
        .await?;

    let html = resp.text().await?;
    match parse_bing_html_with_query(&html, relevance_query, limit) {
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
            parse_bing_html_with_query(&cdp_html, relevance_query, limit)
        }
        other => other,
    }
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

        // 2. Filter out completely unrelated items and first-token-only fallbacks
        if let Some(score) = calculate_relevance(&raw_title, &snippet, &query_tokens) {
            scored_results.push(ScoredBingItem {
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

    // Sort by relevance score descending so the most relevant results come first
    scored_results.sort_by_key(|a| std::cmp::Reverse(a.score));

    let results = scored_results
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

/// Formats search results as a clean GitHub Flavored Markdown document with engine source attribution.
pub fn format_search_results_markdown(query: &str, results: &[SearchResultItem]) -> String {
    let mut out = String::new();
    let engine_name = results
        .first()
        .map(|r| r.engine.as_str())
        .unwrap_or("search");

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
    let engine_name = items.first().map(|r| r.engine.as_str()).unwrap_or("search");

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
}
