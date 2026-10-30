use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tkless_core::{
    collection::{collect_pages, CollectionConfig, Pagination},
    ExtractionOptions, FetchOptions, TklessEngine,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Site {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Site {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Site {
    async fn new(handler: impl Fn(&str) -> (u16, String) + Send + Sync + 'static) -> Self {
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", socket.local_addr().unwrap());
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = socket.accept().await {
                let handler = handler.clone();
                tokio::spawn(async move {
                    let mut input = vec![0; 8192];
                    let size = stream.read(&mut input).await.unwrap();
                    let request = String::from_utf8_lossy(&input[..size]);
                    let path = request.split_whitespace().nth(1).unwrap_or("/");
                    let (status, body) = handler(path);
                    let response = format!("HTTP/1.1 {status} Test\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        Self { url, task }
    }
    fn engine(&self) -> TklessEngine {
        TklessEngine::with_options(FetchOptions {
            allow_private_addresses: true,
            ..Default::default()
        })
        .unwrap()
    }
    fn config(&self) -> CollectionConfig {
        CollectionConfig {
            url: self.url.clone(),
            schema: Some(
                serde_json::json!({"items_selector":".item","fields":{"name":"a","url":"a@href"}}),
            ),
            pagination: Pagination::NextLink {
                selector: "a[rel=next]".into(),
            },
            ..Default::default()
        }
    }
}

#[tokio::test]
async fn html_pagination_collects_details_and_reports_exhaustion() {
    let site = Site::new(|path| {
        (
            200,
            match path {
                "/" => "<div class=item><a href='/a'>A</a></div><a rel=next href='/page2'>next</a>",
                "/page2" => "<div class=item><a href='/b'>B</a></div>",
                _ => "<article><h1>Detail</h1><p>Detail body.</p></article>",
            }
            .into(),
        )
    })
    .await;
    let mut config = site.config();
    config.details = Some(tkless_core::collection::DetailRule {
        url_pointer: "/url".into(),
        schema: None,
        browser: None,
    });
    let report = collect_pages(&site.engine(), &config, &ExtractionOptions::default())
        .await
        .unwrap();
    assert!(report.complete);
    assert_eq!(report.pages_fetched, 2);
    assert_eq!(report.records.len(), 2);
    assert!(report.records.iter().all(|r| r
        .detail
        .as_ref()
        .unwrap()
        .content
        .contains("Detail body")));
    assert_eq!(report.records[0].source_url, format!("{}/", site.url));
}

#[tokio::test]
async fn cursor_tokens_are_encoded_and_repeated_pages_stop() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let site = Site::new(move |path| {
        let call = count.fetch_add(1, Ordering::SeqCst);
        if call > 0 {
            assert!(path.contains("cursor=a%2Fb%26c"));
        }
        (200, r#"{"items":[{"id":1}],"next":"a/b&c"}"#.into())
    })
    .await;
    let config = CollectionConfig {
        url: site.url.clone(),
        pagination: Pagination::Cursor {
            parameter: "cursor".into(),
            items_pointer: "/items".into(),
            next_pointer: "/next".into(),
        },
        ..Default::default()
    };
    let report = collect_pages(&site.engine(), &config, &ExtractionOptions::default())
        .await
        .unwrap();
    assert!(!report.complete);
    assert_eq!(report.stop_reason.as_str(), "repeated_page");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn partial_failure_retains_records_and_limits_never_claim_complete() {
    let site = Site::new(|path| {
        if path == "/" {
            (
                200,
                "<div class=item><a href='/a'>A</a></div><a rel=next href='/failure'>next</a>"
                    .into(),
            )
        } else {
            (500, "failed".into())
        }
    })
    .await;
    let mut config = site.config();
    let report = collect_pages(&site.engine(), &config, &ExtractionOptions::default())
        .await
        .unwrap();
    assert!(!report.complete);
    assert_eq!(report.records.len(), 1);
    assert_eq!(report.stop_reason.as_str(), "page_error");
    config.max_pages = 1;
    let report = collect_pages(&site.engine(), &config, &ExtractionOptions::default())
        .await
        .unwrap();
    assert!(!report.complete);
    assert_eq!(report.stop_reason.as_str(), "page_limit");
}

#[tokio::test]
async fn numbered_pages_require_explicit_empty_marker() {
    let site = Site::new(|path| {
        (
            200,
            if path == "/?page=1" {
                "<div class=item><a href='/a'>A</a></div>"
            } else {
                "<div class=empty>no results</div>"
            }
            .into(),
        )
    })
    .await;
    let mut config = site.config();
    config.pagination = Pagination::PageNumber {
        parameter: "page".into(),
        start: 1,
        empty_selector: Some(".empty".into()),
    };
    let report = collect_pages(&site.engine(), &config, &ExtractionOptions::default())
        .await
        .unwrap();
    assert!(report.complete);
    assert_eq!(report.pages_fetched, 2);
}

#[tokio::test]
async fn mcp_collection_exposes_structured_report() {
    let site = Site::new(|_| (200, "<div class=item><a href='/a'>A</a></div>".into())).await;
    let server =
        tkless_core::McpServer::new(site.engine(), tkless_core::DocumentCache::memory_only(60));
    let result = server
        .dispatch_tool_execution(
            "collect_pages",
            &serde_json::json!({"config":site.config(),"allow_private_addresses":true}),
            None,
            None,
        )
        .await;
    assert!(!result.is_error, "{:?}", result.content);
    assert_eq!(result.structured_content.unwrap()["complete"], true);
}

#[tokio::test]
async fn mcp_rejects_oversized_sync_budget_before_fetching() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let site = Site::new(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
        (200, "<div class=item><a href='/a'>A</a></div>".into())
    })
    .await;
    let server =
        tkless_core::McpServer::new(site.engine(), tkless_core::DocumentCache::memory_only(60));
    for budget in [0, tkless_core::limits::SYNC_MAX_STATE_MB + 1, u64::MAX] {
        let result = server
            .dispatch_tool_execution(
                "sync_collection",
                &serde_json::json!({
                    "config": site.config(), "state_dir": "unused-state", "max_state_mb": budget,
                    "allow_private_addresses": true
                }),
                None,
                None,
            )
            .await;
        assert!(result.is_error);
        assert_eq!(
            result.structured_content.unwrap()["error"]["code"],
            "invalid_argument"
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn sync_records_updates_without_false_deletions_on_partial_runs() {
    let revision = Arc::new(AtomicUsize::new(0));
    let value = revision.clone();
    let site = Site::new(move |path| {
        let rev = value.load(Ordering::SeqCst);
        if path == "/failure" { return (500, "failed".into()); }
        (200, match rev {
            0 => "<div class=item><a href='/a'>A</a></div><div class=item><a href='/b'>B</a></div>",
            1 => "<div class=item><a href='/a'>A updated</a></div><a rel=next href='/failure'>next</a>",
            _ => "<div class=item><a href='/a'>A updated</a></div>",
        }.into())
    }).await;
    let mut config = site.config();
    config.id_pointer = Some("/url".into());
    let dir = std::env::temp_dir().join(format!(
        "tkless-sync-{}",
        site.url.rsplit(':').next().unwrap()
    ));
    let engine = site.engine();
    let options = ExtractionOptions::default();
    let first = tkless_core::sync::sync_collection(&engine, &config, &options, &dir, 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        first
            .events
            .iter()
            .filter(|e| e.kind.as_str() == "added")
            .count(),
        2
    );
    revision.store(1, Ordering::SeqCst);
    let partial = tkless_core::sync::sync_collection(&engine, &config, &options, &dir, 1024 * 1024)
        .await
        .unwrap();
    assert!(!partial.complete);
    assert!(!partial.deletions_applied);
    assert_eq!(
        partial
            .events
            .iter()
            .filter(|e| e.kind.as_str() == "updated")
            .count(),
        1
    );
    assert!(!partial.events.iter().any(|e| e.kind.as_str() == "removed"));
    revision.store(2, Ordering::SeqCst);
    let final_run =
        tkless_core::sync::sync_collection(&engine, &config, &options, &dir, 1024 * 1024)
            .await
            .unwrap();
    assert!(final_run.complete);
    assert_eq!(
        final_run
            .events
            .iter()
            .filter(|e| e.kind.as_str() == "removed")
            .count(),
        1
    );
    assert_eq!(
        final_run
            .events
            .iter()
            .filter(|e| e.kind.as_str() == "unchanged")
            .count(),
        1
    );
    assert!(dir.join("events/1.json").exists());
    assert!(dir.join("events/3.json").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn crawl_checkpoint_resumes_frontier_without_refetching_completed_seed() {
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    let site = Site::new(move |path| {
        if path == "/" {
            count.fetch_add(1, Ordering::SeqCst);
        }
        (
            200,
            if path == "/" {
                "<h1>Seed</h1><a href='/a'>A</a><a href='/b'>B</a>"
            } else {
                "<h1>Child</h1><p>Body</p>"
            }
            .into(),
        )
    })
    .await;
    let path = std::env::temp_dir().join(format!(
        "tkless-crawl-{}.json",
        site.url.rsplit(':').next().unwrap()
    ));
    let mut options = tkless_core::CrawlOptions {
        checkpoint: Some(path.clone()),
        max_pages: 1,
        ..Default::default()
    };
    let engine = site.engine();
    let extraction = ExtractionOptions::default();
    let first = tkless_core::crawl_site(&engine, &site.url, &options, &extraction)
        .await
        .unwrap();
    assert!(!first.complete);
    assert_eq!(first.stop_reason.as_str(), "page_limit");
    assert_eq!(first.pending_urls, 2);
    options.resume = true;
    options.max_pages = 10;
    let second = tkless_core::crawl_site(&engine, &site.url, &options, &extraction)
        .await
        .unwrap();
    assert!(second.complete);
    assert_eq!(second.pages.len(), 3);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn crawl_output_budget_keeps_unfinished_page_resumable() {
    let site = Site::new(|_| (200, "<h1>Page</h1><p>Some body</p>".into())).await;
    let path = std::env::temp_dir().join(format!(
        "tkless-crawl-budget-{}.json",
        site.url.rsplit(':').next().unwrap()
    ));
    let mut options = tkless_core::CrawlOptions {
        checkpoint: Some(path.clone()),
        max_output_bytes: Some(1),
        ..Default::default()
    };
    let report = tkless_core::crawl_site(
        &site.engine(),
        &site.url,
        &options,
        &ExtractionOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(report.stop_reason.as_str(), "output_budget");
    assert_eq!(report.pending_urls, 1);
    options.resume = true;
    options.max_output_bytes = Some(10000);
    assert!(
        tkless_core::crawl_site(
            &site.engine(),
            &site.url,
            &options,
            &ExtractionOptions::default()
        )
        .await
        .unwrap()
        .complete
    );
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn crawl_obeys_robots_before_requesting_disallowed_page() {
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    let site = Site::new(move |path| {
        (
            200,
            match path {
                "/robots.txt" => "User-agent: *\nDisallow: /private\nAllow: /private/public$\n",
                "/" => "<a href='/private'>Private</a>",
                _ => {
                    count.fetch_add(1, Ordering::SeqCst);
                    "should not be requested"
                }
            }
            .into(),
        )
    })
    .await;
    let report = tkless_core::crawl_site(
        &site.engine(),
        &site.url,
        &tkless_core::CrawlOptions {
            respect_robots: true,
            ..Default::default()
        },
        &ExtractionOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert!(report
        .pages
        .iter()
        .any(|p| p.error.as_ref().is_some_and(|e| e.contains("robots.txt"))));
}

#[tokio::test]
async fn crawl_time_budget_preserves_delayed_request_for_resume() {
    let site = Site::new(|path| {
        (
            200,
            if path == "/" {
                "<a href='/child'>Child</a>"
            } else {
                "<h1>Child</h1>"
            }
            .into(),
        )
    })
    .await;
    let path = std::env::temp_dir().join(format!(
        "tkless-crawl-time-{}.json",
        site.url.rsplit(':').next().unwrap()
    ));
    let mut options = tkless_core::CrawlOptions {
        checkpoint: Some(path.clone()),
        max_seconds: Some(1),
        delay_ms: 2000,
        ..Default::default()
    };
    let engine = site.engine();
    let extraction = ExtractionOptions::default();
    let first = tkless_core::crawl_site(&engine, &site.url, &options, &extraction)
        .await
        .unwrap();
    assert_eq!(first.stop_reason.as_str(), "time_budget");
    assert_eq!(first.pending_urls, 1);
    options.resume = true;
    options.delay_ms = 0;
    options.max_seconds = Some(5);
    assert!(
        tkless_core::crawl_site(&engine, &site.url, &options, &extraction)
            .await
            .unwrap()
            .complete
    );
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_file(path.with_extension("lock")).unwrap();
}

#[tokio::test]
async fn browser_mcp_rejects_arbitrary_javascript_before_launch() {
    let server = tkless_core::McpServer::new(
        TklessEngine::new().unwrap(),
        tkless_core::DocumentCache::memory_only(60),
    );
    let result = server.dispatch_tool_execution("browse_page", &serde_json::json!({"url":"http://127.0.0.1:9","plan":{"steps":[{"action":"evaluate","script":"alert(1)"}]}}), None, None).await;
    assert!(result.is_error);
    assert_eq!(
        result.structured_content.unwrap()["error"]["code"],
        "invalid_argument"
    );
}

#[tokio::test]
async fn collection_stops_before_cross_origin_next_page() {
    let site = Site::new(|_| (200, "<div class=item><a href='/a'>A</a></div><a rel=next href='https://outside.invalid/page2'>next</a>".into())).await;
    let report = collect_pages(
        &site.engine(),
        &site.config(),
        &ExtractionOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(report.stop_reason.as_str(), "origin_boundary");
    assert_eq!(report.pages_fetched, 1);
    assert!(!report.complete);
}
