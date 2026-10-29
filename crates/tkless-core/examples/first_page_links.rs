//! Capture organic result links from one SERP without pagination or query rewriting.
//! cargo run -p tkless-core --example first_page_links -- "OpenAI Codex"
use scraper::{Html, Selector};
use serde::Serialize;
use std::collections::HashSet;
use tkless_core::{cdp, fetcher::Fetcher, FetchOptions, TklessError};
use url::Url;

#[derive(Serialize)]
struct Link {
    title: String,
    url: String,
}

#[derive(Serialize)]
struct Page {
    engine: &'static str,
    query: String,
    page: usize,
    search_url: String,
    status: &'static str,
    result_blocks: usize,
    links: Vec<Link>,
    error: Option<String>,
}

fn recognized_blocks(html: &str) -> usize {
    let selector = Selector::parse("li.b_algo, #b_results .b_algo").unwrap();
    Html::parse_document(html).select(&selector).count()
}

async fn capture(query: &str) -> Page {
    let mut search_url = Url::parse("https://www.bing.com/search").unwrap();
    search_url.query_pairs_mut().append_pair("q", query);
    let mut page = Page {
        engine: "bing",
        query: query.into(),
        page: 1,
        search_url: search_url.to_string(),
        status: "error",
        result_blocks: 0,
        links: vec![],
        error: None,
    };
    let options = cdp::CdpOptions {
        endpoint: cdp::resolve_cdp_endpoint(None, true)
            .unwrap_or_else(|| "http://127.0.0.1:9222".into()),
        wait_ms: Some(1500),
        timeout_ms: Some(30_000),
        ..Default::default()
    };
    let response = async {
            let fetcher = Fetcher::new(FetchOptions {
                user_agent: Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36 Edg/133.0.0.0".into()),
                headers: vec![
                    ("Accept-Language".into(), "en-US,en;q=0.9".into()),
                    ("Referer".into(), "https://www.bing.com/".into()),
                    ("Accept".into(), "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8".into()),
                    ("Sec-Ch-Ua".into(), "\"Not(A:Brand\";v=\"99\", \"Microsoft Edge\";v=\"133\", \"Chromium\";v=\"133\"".into()),
                    ("Sec-Ch-Ua-Mobile".into(), "?0".into()),
                    ("Sec-Ch-Ua-Platform".into(), "\"Windows\"".into()),
                    ("Sec-Fetch-Dest".into(), "document".into()),
                    ("Sec-Fetch-Mode".into(), "navigate".into()),
                    ("Sec-Fetch-Site".into(), "same-origin".into()),
                    ("Sec-Fetch-User".into(), "?1".into()),
                    ("Upgrade-Insecure-Requests".into(), "1".into()),
                ],
                cookie: Some("SRCHD=AF=NOFORM; SRCHHPGUSR=SRCHLANG=en; _EDGE_S=mkt=en-us; _SS=SID=1".into()),
                initial_target_host: Some("www.bing.com".into()),
                ..Default::default()
            })?;
            let response = fetcher.fetch(search_url.as_str()).await?;
            if recognized_blocks(&response.html) == 0 && cdp::is_anti_scraping_challenge(&response.html, response.status) {
                cdp::fetch_html_via_cdp(search_url.as_str(), &options).await
            } else {
                Ok::<_, TklessError>(response.html)
            }
        }.await;
    let html = match response {
        Ok(html) => html,
        Err(error) => {
            page.status = if matches!(error, TklessError::SearchAntiScraping(_)) {
                "blocked"
            } else {
                "error"
            };
            page.error = Some(error.to_string());
            return page;
        }
    };
    if recognized_blocks(&html) == 0 && cdp::is_anti_scraping_challenge(&html, 200) {
        page.status = "blocked";
        page.error = Some("Search response is still a verification page".into());
        return page;
    }
    let document = Html::parse_document(&html);
    let blocks = Selector::parse("li.b_algo, #b_results .b_algo").unwrap();
    let anchors = Selector::parse("a[href]").unwrap();
    let mut seen = HashSet::new();
    for block in document.select(&blocks) {
        if block
            .value()
            .classes()
            .any(|name| matches!(name, "b_ad" | "b_adTop" | "b_adBottom"))
        {
            continue;
        }
        page.result_blocks += 1;
        for anchor in block.select(&anchors) {
            let href = anchor.value().attr("href").unwrap();
            let absolute = match search_url.join(href) {
                Ok(url) if matches!(url.scheme(), "http" | "https") => url,
                _ => continue,
            };
            let unwrapped = tkless_core::search::unwrap_bing_url(absolute.as_str());
            let target = match Url::parse(&unwrapped) {
                Ok(url) if matches!(url.scheme(), "http" | "https") => url,
                _ => continue,
            };
            let host = target.host_str().unwrap_or("");
            if ["bing.com"]
                .iter()
                .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
            {
                continue;
            }
            let url = target.to_string();
            if seen.insert(url.clone()) {
                page.links.push(Link {
                    title: anchor
                        .text()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                    url,
                });
            }
        }
    }
    page.status = if page.result_blocks > 0 {
        "ok"
    } else {
        "unrecognized"
    };
    if page.status == "unrecognized" {
        page.error =
            Some("No recognized result blocks; this does not establish an empty search".into());
    }
    page
}

#[tokio::main]
async fn main() {
    let query = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "OpenAI Codex".into());
    if std::env::args().nth(2).is_some() {
        eprintln!("Usage: first_page_links <query>; searches use Bing directly");
        std::process::exit(2);
    }
    let page = capture(&query).await;
    let complete = page.status == "ok";
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({"query":query,"page":1,"pagination":false,"complete":complete,"result":page})).unwrap());
    if !complete {
        std::process::exit(1);
    }
}
