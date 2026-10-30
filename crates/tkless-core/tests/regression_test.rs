use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tkless_core::{
    DocumentCache, ExtractionMode, ExtractionOptions, FetchOptions, OutputFormat, TklessEngine,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

struct Reply {
    status: u16,
    headers: String,
    body: Vec<u8>,
    delay_ms: u64,
}
impl Reply {
    fn html(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            headers: "Content-Type: text/html\r\n".into(),
            body: body.into().into_bytes(),
            delay_ms: 0,
        }
    }
}
struct Site {
    url: String,
    task: tokio::task::JoinHandle<()>,
    peak: Arc<AtomicUsize>,
    hits: Arc<AtomicUsize>,
}
impl Drop for Site {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Site {
    async fn start(handler: impl Fn(&str, usize) -> Reply + Send + Sync + 'static) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let peak = Arc::new(AtomicUsize::new(0));
        let hits = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let handler = Arc::new(handler);
        let (p, h) = (peak.clone(), hits.clone());
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (handler, active, peak, hits) =
                    (handler.clone(), active.clone(), p.clone(), h.clone());
                tokio::spawn(async move {
                    let mut reader = BufReader::new(stream);
                    let mut request = String::new();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).await.unwrap_or(0) == 0 || line == "\r\n" {
                            break;
                        }
                        request.push_str(&line);
                    }
                    let index = hits.fetch_add(1, Ordering::SeqCst);
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    let reply = handler(&request, index);
                    tokio::time::sleep(Duration::from_millis(reply.delay_ms)).await;
                    let headers = format!(
                        "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n",
                        reply.status,
                        reply.body.len(),
                        reply.headers
                    );
                    let mut stream = reader.into_inner();
                    let _ = stream.write_all(headers.as_bytes()).await;
                    let _ = stream.write_all(&reply.body).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        Self {
            url,
            task,
            peak,
            hits,
        }
    }
    fn engine(&self) -> TklessEngine {
        TklessEngine::with_options(FetchOptions {
            allow_private_addresses: true,
            timeout_secs: 3,
            ..Default::default()
        })
        .unwrap()
    }
}

struct UnavailableAdapter;

#[tokio::test]
async fn ignored_range_responses_fall_back_without_corrupting_output() {
    let site = Site::start(|request, _| {
        if request
            .to_ascii_lowercase()
            .contains("range: bytes=0-0\r\n")
        {
            Reply {
                status: 206,
                headers: "Content-Range: bytes 0-0/4194304\r\n".into(),
                body: vec![b'W'],
                delay_ms: 0,
            }
        } else {
            Reply::html("WHOLE")
        }
    })
    .await;
    let destination = std::env::temp_dir().join(format!(
        "tkless-range-{}-{}",
        std::process::id(),
        site.url.rsplit(':').next().unwrap()
    ));
    let bytes = tkless_core::download_file(
        &reqwest::Client::new(),
        tkless_core::DownloadOptions {
            url: &site.url,
            destination: &destination,
            referer: None,
            cookie: None,
            reporter: None,
            concurrency: Some(2),
        },
    )
    .await
    .unwrap();
    assert_eq!(bytes, 5);
    assert_eq!(std::fs::read(&destination).unwrap(), b"WHOLE");
    assert!(!destination.with_extension("part0").exists());
    std::fs::remove_file(destination).unwrap();
}

#[tokio::test]
async fn adapter_reader_rejects_oversized_json() {
    let site = Site::start(|_, _| Reply::html(r#"{"value":"long response"}"#)).await;
    let fetcher = tkless_core::fetcher::Fetcher::new(FetchOptions {
        allow_private_addresses: true,
        max_bytes: 8,
        ..Default::default()
    })
    .unwrap();
    let response = fetcher.get_str(&site.url).send().await.unwrap();
    assert!(matches!(
        fetcher.read_json::<serde_json::Value>(response).await,
        Err(tkless_core::TklessError::ResponseTooLarge { .. })
    ));
}

#[tokio::test]
async fn mcp_rejects_oversized_unterminated_messages() {
    let server =
        tkless_core::McpServer::new(TklessEngine::new().unwrap(), DocumentCache::memory_only(60));
    let input = vec![b' '; 1024 * 1024 + 1];
    let error = server
        .run_transport(&input[..], tokio::io::sink())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("exceeds 1 MiB"));
}

#[test]
fn memory_cache_has_entry_and_document_size_bounds() {
    let engine = TklessEngine::new().unwrap();
    let mut doc = engine
        .process_html("<p>cache</p>", None, &ExtractionOptions::default())
        .unwrap();
    let cache = DocumentCache::memory_only(60);
    for i in 0..100 {
        cache.set(&format!("entry-{i}"), &doc, None).unwrap();
    }
    assert_eq!(cache.stats().memory_entries, 64);
    doc.content = "x".repeat(1024 * 1024 + 1);
    cache.set("large", &doc, None).unwrap();
    assert!(cache.get("large").is_none());
}

#[test]
fn transient_catalog_failures_are_not_cached_but_intentional_limits_are() {
    let mut doc = TklessEngine::new()
        .unwrap()
        .process_html("<p>partial</p>", None, &ExtractionOptions::default())
        .unwrap();
    doc.truncated = true;
    doc.warnings.push(tkless_core::DiagnosticWarning::new(
        "skills_page_failed",
        "temporary failure",
    ));
    let cache = DocumentCache::memory_only(60);
    cache.set("partial", &doc, None).unwrap();
    assert!(cache.get("partial").is_none());
    doc.warnings[0].code = "skills_pagination_incomplete".into();
    cache.set("limited", &doc, None).unwrap();
    assert!(cache.get("limited").is_some());
}

#[tokio::test]
async fn failed_download_preserves_existing_destination() {
    let site = Site::start(|_, _| Reply {
        status: 500,
        headers: String::new(),
        body: vec![],
        delay_ms: 0,
    })
    .await;
    let path = std::env::temp_dir().join(format!(
        "tkless-existing-{}-{}",
        std::process::id(),
        site.url.rsplit(':').next().unwrap()
    ));
    std::fs::write(&path, b"original").unwrap();
    assert!(tkless_core::download_file(
        &reqwest::Client::new(),
        tkless_core::DownloadOptions {
            url: &site.url,
            destination: &path,
            referer: None,
            cookie: None,
            reporter: None,
            concurrency: None,
        }
    )
    .await
    .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn custom_credentials_never_follow_scheme_downgrades_or_host_changes() {
    let fetcher = tkless_core::fetcher::Fetcher::new(FetchOptions {
        initial_target_origin: Some("https://example.com".into()),
        headers: vec![("Authorization".into(), "Bearer FAKE".into())],
        ..Default::default()
    })
    .unwrap();
    for (url, expected) in [
        ("https://example.com/page", true),
        ("http://example.com/page", false),
        ("https://other.example/page", false),
        ("https://example.com:8443/page", false),
    ] {
        let request = fetcher.get_str(url).build().unwrap();
        assert_eq!(request.headers().contains_key("authorization"), expected);
    }
}

#[tokio::test]
async fn credentials_are_scoped_to_origin_across_redirects() {
    let destination = Site::start(|request, _| {
        let lower = request.to_ascii_lowercase();
        assert!(!lower.contains("authorization:"));
        assert!(!lower.contains("cookie:"));
        assert!(!lower.contains("x-api-key:"));
        Reply::html("<p>destination</p>")
    })
    .await;
    let target = destination.url.clone();
    let origin = Site::start(move |request, _| {
        let lower = request.to_ascii_lowercase();
        assert!(lower.contains("authorization: bearer fake"));
        assert!(lower.contains("cookie: fake=value"));
        assert!(lower.contains("x-api-key: fake-key"));
        if request.starts_with("GET /same ") {
            Reply {
                status: 302,
                headers: format!("Location: {target}/end\r\n"),
                body: vec![],
                delay_ms: 0,
            }
        } else {
            Reply {
                status: 302,
                headers: "Location: /same\r\n".into(),
                body: vec![],
                delay_ms: 0,
            }
        }
    })
    .await;
    let fetcher = tkless_core::fetcher::Fetcher::new(FetchOptions {
        allow_private_addresses: true,
        initial_target_origin: Some(origin.url.clone()),
        headers: vec![
            ("Authorization".into(), "Bearer FAKE".into()),
            ("Cookie".into(), "fake=value".into()),
            ("X-API-Key".into(), "fake-key".into()),
        ],
        ..Default::default()
    })
    .unwrap();
    assert!(fetcher
        .fetch(&origin.url)
        .await
        .unwrap()
        .html
        .contains("destination"));
    assert_eq!(destination.hits.load(Ordering::SeqCst), 1);
    assert_eq!(origin.hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn images_do_not_overwrite_other_documents_and_obey_size_limit() {
    let site = Site::start(|request, _| {
        let mut reply = Reply::html(if request.starts_with("GET /a ") {
            "IMAGE_A"
        } else {
            "IMAGE_B"
        });
        reply.headers = "Content-Type: image/png\r\n".into();
        reply
    })
    .await;
    let dir = std::env::temp_dir().join(format!(
        "tkless-security-images-{}-{}",
        std::process::id(),
        site.url.rsplit(':').next().unwrap()
    ));
    let fetcher = tkless_core::fetcher::Fetcher::new(FetchOptions {
        allow_private_addresses: true,
        max_bytes: 7,
        ..Default::default()
    })
    .unwrap();
    let a = format!("![a]({}/a)", site.url);
    let b = format!("![b]({}/b)", site.url);
    let first =
        tkless_core::downloader::download_and_rewrite_markdown_images(&fetcher, &a, &dir, None)
            .await
            .unwrap();
    let second =
        tkless_core::downloader::download_and_rewrite_markdown_images(&fetcher, &b, &dir, None)
            .await
            .unwrap();
    assert!(!first.contains("http://"));
    assert_ne!(first.split('(').nth(1), second.split('(').nth(1));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    let limited = tkless_core::fetcher::Fetcher::new(FetchOptions {
        allow_private_addresses: true,
        max_bytes: 6,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        tkless_core::downloader::download_and_rewrite_markdown_images(&limited, &a, &dir, None)
            .await
            .unwrap(),
        a
    );
    std::fs::remove_dir_all(dir).unwrap();
}
impl tkless_core::PlatformAdapter for UnavailableAdapter {
    fn name(&self) -> &'static str {
        "fixture"
    }
    fn matches(&self, _: &url::Url) -> bool {
        true
    }
    fn extract<'a>(
        &'a self,
        _: &'a tkless_core::fetcher::Fetcher,
        _: &'a url::Url,
        _: &'a ExtractionOptions,
    ) -> tkless_core::adapters::BoxFuture<
        'a,
        Result<tkless_core::PageDocument, tkless_core::TklessError>,
    > {
        Box::pin(async {
            Err(tkless_core::TklessError::AdapterError {
                platform: "fixture",
                message: "upstream layout unavailable".into(),
            })
        })
    }
}

#[tokio::test]
async fn adapter_fallback_provenance_survives_cache_hits() {
    let site = Site::start(|_, _| {
        Reply::html("<html><body><h1>Fallback</h1><p>Actual HTTP source body.</p></body></html>")
    })
    .await;
    let mut adapters = tkless_core::AdapterRegistry::new();
    adapters.register(Arc::new(UnavailableAdapter));
    let engine = TklessEngine::with_options_and_adapters(
        FetchOptions {
            allow_private_addresses: true,
            ..Default::default()
        },
        adapters,
    )
    .unwrap();
    let cache = DocumentCache::memory_only(300);
    for _ in 0..2 {
        let doc = engine
            .process_url_with_cache(&site.url, &ExtractionOptions::default(), &cache)
            .await
            .unwrap();
        assert_eq!(doc.extraction.method, "html");
        assert_eq!(doc.extraction.transport, "http");
        assert_eq!(doc.warnings[0].code, "adapter_fallback");
        assert!(doc.warnings[0].message.contains("fixture"));
    }
    assert_eq!(site.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn mcp_fetch_and_batch_return_structured_documents_and_item_errors() {
    let site = Site::start(|_, _| {
        Reply::html("<html><body><h1>Example</h1><p>Actual HTTP source body.</p></body></html>")
    })
    .await;
    let server = tkless_core::McpServer::new(site.engine(), DocumentCache::memory_only(300));
    let fetched = server
        .dispatch_tool_execution(
            "fetch_webpage",
            &serde_json::json!({"url":site.url,"allow_private_addresses":true,"use_cache":false}),
            None,
            None,
        )
        .await;
    assert!(!fetched.is_error);
    let fetched = fetched.structured_content.unwrap();
    assert_eq!(fetched["document"]["extraction"]["transport"], "http");
    let batch = server
        .dispatch_tool_execution(
            "batch_fetch_webpages",
            &serde_json::json!({"urls":[site.url,"invalid-url"],"format":"json"}),
            None,
            None,
        )
        .await;
    assert!(!batch.is_error);
    let batch = batch.structured_content.unwrap();
    assert_eq!(batch["succeeded"], 1);
    assert_eq!(batch["failed"], 1);
    assert_eq!(batch["results"][1]["error_details"]["code"], "invalid_url");
}

#[test]
fn cache_isolates_every_option_and_request_identity() {
    let cache = DocumentCache::memory_only(60);
    let options = ExtractionOptions::default();
    let original = cache.generate_key("https://example.com", &options);
    let value = serde_json::to_value(&options).unwrap();
    for (name, field) in value.as_object().unwrap() {
        let mut changed = value.clone();
        changed[name] = match field {
            serde_json::Value::Bool(b) => serde_json::json!(!b),
            serde_json::Value::Null => {
                if name.contains("size")
                    || name.contains("overlap")
                    || name.contains("limit")
                    || name.contains("tokens")
                    || name.contains("wait")
                {
                    serde_json::json!(3)
                } else {
                    serde_json::json!("changed")
                }
            }
            serde_json::Value::String(_) => continue,
            _ => continue,
        };
        if let Ok(changed) = serde_json::from_value::<ExtractionOptions>(changed) {
            assert_ne!(
                original,
                cache.generate_key("https://example.com", &changed),
                "{name}"
            );
        }
    }
    let a = TklessEngine::with_options(FetchOptions {
        cookie: Some("test-secret-a".into()),
        initial_target_host: Some("example.com".into()),
        ..Default::default()
    })
    .unwrap();
    let b = TklessEngine::with_options(FetchOptions {
        cookie: Some("test-secret-b".into()),
        initial_target_host: Some("example.com".into()),
        ..Default::default()
    })
    .unwrap();
    let key = a.cache_key(&cache, "https://example.com", &options);
    assert_ne!(key, b.cache_key(&cache, "https://example.com", &options));
    assert!(!key.contains("test-secret"));
}

#[tokio::test]
async fn concurrent_crawl_and_batch_preserve_failures() {
    let site = Site::start(|request, _| {
        let path = request.split_whitespace().nth(1).unwrap();
        if path == "/" { return Reply::html("<a href='/a'>A</a><a href='/b'>B</a><a href='/fail'>Fail</a>"); }
        let mut reply = Reply::html(format!("<article><h1>{path}</h1><p>Document body for a local regression fixture.</p></article>"));
        reply.delay_ms = 100;
        if path == "/fail" { reply.status = 500; }
        reply
    }).await;
    let options = ExtractionOptions {
        mode: ExtractionMode::Full,
        ..Default::default()
    };
    let crawl = tkless_core::crawl_site(
        &site.engine(),
        &site.url,
        &tkless_core::CrawlOptions {
            concurrency: 3,
            max_pages: 4,
            ..Default::default()
        },
        &options,
    )
    .await
    .unwrap();
    assert_eq!(crawl.pages.len(), 4);
    assert_eq!(crawl.pages.iter().filter(|p| p.error.is_some()).count(), 1);
    assert!(site.peak.load(Ordering::SeqCst) >= 2);
    assert!(site.peak.load(Ordering::SeqCst) <= 3);
    let urls = vec![format!("{}/a", site.url), format!("{}/fail", site.url)];
    let batch = tkless_core::batch::collect_batch(
        &site.engine(),
        &urls,
        &options,
        &tkless_core::batch::BatchOptions {
            concurrency: 2,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!((batch.summary.succeeded, batch.summary.failed), (1, 1));
    assert_eq!(batch.results[1].url, urls[1]);
    assert_eq!(batch.results[1].exit_code, 3);
}

#[tokio::test]
async fn sitemap_visits_more_than_three_child_maps() {
    let site = Site::start(|request, _| {
        let path = request.split_whitespace().nth(1).unwrap();
        let host = request
            .lines()
            .find_map(|line| {
                line.strip_prefix("host: ")
                    .or_else(|| line.strip_prefix("Host: "))
            })
            .unwrap()
            .trim();
        if path == "/index.xml" {
            return Reply::html(format!(
                "<sitemapindex>{}</sitemapindex>",
                (0..5)
                    .map(|i| format!("<sitemap><loc>http://{host}/map{i}.xml</loc></sitemap>"))
                    .collect::<String>()
            ));
        }
        if path.starts_with("/map") {
            let id = path.trim_start_matches("/map").trim_end_matches(".xml");
            return Reply::html(format!(
                "<urlset><url><loc>http://{host}/page{id}</loc></url></urlset>"
            ));
        }
        Reply::html(format!("<h1>{path}</h1><p>Local sitemap document.</p>"))
    })
    .await;
    let report = tkless_core::crawl_site(
        &site.engine(),
        &format!("{}/index.xml", site.url),
        &tkless_core::CrawlOptions {
            sitemap: true,
            max_depth: 0,
            max_pages: 5,
            ..Default::default()
        },
        &ExtractionOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(report.pages.len(), 5);
    assert!(report.pages.iter().any(|page| page.url.ends_with("page4")));
}

#[tokio::test]
async fn retries_rate_limits_and_revalidates_expired_cache() {
    let site = Site::start(|request, index| {
        let mut reply = Reply::html("<h1>Cache fixture</h1><p>Stable body.</p>");
        reply.headers.push_str("ETag: \"v1\"\r\n");
        if index == 0 {
            reply.status = 429;
            reply.headers.push_str("Retry-After: 0\r\n");
        } else if request.to_lowercase().contains("if-none-match: \"v1\"") {
            reply.status = 304;
            reply.body.clear();
        }
        reply
    })
    .await;
    let engine = TklessEngine::with_options(FetchOptions {
        allow_private_addresses: true,
        retries: 1,
        ..Default::default()
    })
    .unwrap();
    let dir = std::env::temp_dir().join(format!(
        "tkless-revalidation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let cache = DocumentCache::new(Some(dir.clone()), 1);
    let options = ExtractionOptions::default();
    let first = engine
        .process_url_with_cache(&site.url, &options, &cache)
        .await
        .unwrap();
    assert_eq!(site.hits.load(Ordering::SeqCst), 2);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let reopened = DocumentCache::new(Some(dir.clone()), 1);
    let second = engine
        .process_url_with_cache(&site.url, &options, &reopened)
        .await
        .unwrap();
    assert_eq!(first.content, second.content);
    assert_eq!(site.hits.load(Ordering::SeqCst), 3);
    assert_eq!(
        reopened
            .get_entry(&engine.cache_key(&reopened, &site.url, &options))
            .unwrap()
            .etag
            .as_deref(),
        Some("\"v1\"")
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn mcp_ping_and_cancellation_remain_responsive() {
    let site = Site::start(|_, _| {
        let mut reply = Reply::html("<p>Slow</p>");
        reply.delay_ms = 500;
        reply
    })
    .await;
    let server = tkless_core::McpServer::new_with_mode(
        site.engine(),
        DocumentCache::memory_only(60),
        tkless_core::mcp::McpMode::Dynamic,
    );
    let (client, transport) = tokio::io::duplex(16384);
    let (input, output) = tokio::io::split(transport);
    let task = tokio::spawn(async move {
        server.run_transport(input, output).await.unwrap();
    });
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    let init = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}});
    write
        .write_all(format!("{init}\n").as_bytes())
        .await
        .unwrap();
    let mut line = String::new();
    read.read_line(&mut line).await.unwrap();
    let response: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(
        response["result"]["capabilities"]["tools"]["listChanged"],
        true
    );
    let request = serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"fetch_webpage","arguments":{"url":site.url,"allow_private_addresses":true}}});
    write
        .write_all(
            format!("{request}\n{{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}}\n").as_bytes(),
        )
        .await
        .unwrap();
    line.clear();
    tokio::time::timeout(Duration::from_millis(400), read.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&line).unwrap()["id"],
        3
    );
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":2}}\n").await.unwrap();
    write.shutdown().await.unwrap();
    let mut remaining = String::new();
    read.read_to_string(&mut remaining).await.unwrap();
    assert!(!remaining.contains("\"id\":2"));
    task.await.unwrap();
}

#[test]
fn research_budget_keeps_json_valid() {
    use tkless_core::research::*;
    let report = DeepResearchReport {
        topic: "Fixture".into(),
        truncated: false,
        executive_summary: "Evidence [文献 #1]. ".repeat(80),
        key_takeaways: vec!["Evidence [文献 #1].".repeat(50)],
        dimensions: vec![],
        comparative_analysis: None,
        citations: vec![ResearchCitation {
            id: 1,
            title: "Source".into(),
            url: "https://example.com".into(),
            snippet: "Long snippet. ".repeat(80),
            hop: 1,
            key_points: vec!["A finding.".into()],
        }],
        telemetry: ResearchTelemetry {
            total_queries: 1,
            pages_fetched: 1,
            total_tokens_extracted: 1000,
            elapsed_ms: 1,
        },
    };
    for format in [OutputFormat::Json, OutputFormat::Markdown] {
        let options = ResearchOptions {
            format,
            max_tokens: Some(200),
            ..Default::default()
        };
        let output = render_deep_research(&report, &options).unwrap();
        assert!(
            tkless_core::token::count_tokens(&output, tkless_core::TokenizerKind::default()) <= 200
        );
        if format == OutputFormat::Json {
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&output).unwrap()["truncated"],
                true
            );
        }
    }
}

fn pdf_fixture() -> Vec<u8> {
    use lopdf::{
        content::{Content, Operation},
        dictionary, Object, Stream,
    };
    let mut doc = lopdf::Document::with_version("1.5");
    let pages = doc.new_object_id();
    let font = doc.add_object(
        dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
    );
    let resources = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
    let content = Content { operations: vec![
        Operation::new("BT", vec![]), Operation::new("Tf", vec!["F1".into(), 12.into()]),
        Operation::new("Td", vec![30.into(), 700.into()]),
        Operation::new("Tj", vec![Object::string_literal("PDF extraction must preserve complete paragraphs. Token budgets prevent context overflow. Local fixtures make document tests repeatable.")]),
        Operation::new("ET", vec![]),
    ]}.encode().unwrap();
    let content = doc.add_object(Stream::new(dictionary! {}, content));
    let page =
        doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "Contents" => content });
    doc.objects.insert(pages, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1, "Resources" => resources, "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()] }));
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    doc.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    bytes
}

#[tokio::test]
async fn pdf_summary_chunks_and_cached_extraction_match() {
    let bytes = pdf_fixture();
    let response_bytes = bytes.clone();
    let site = Site::start(move |_, _| Reply {
        status: 200,
        headers: "Content-Type: application/pdf\r\n".into(),
        body: response_bytes.clone(),
        delay_ms: 0,
    })
    .await;
    let options = ExtractionOptions {
        summarize: true,
        chunk: true,
        chunk_size: Some(40),
        ..Default::default()
    };
    let normalized_url = url::Url::parse(&site.url).unwrap().to_string();
    let local =
        tkless_core::extract_pdf_to_document(&bytes, Some(&normalized_url), &options).unwrap();
    let cache = DocumentCache::memory_only(60);
    let remote = site
        .engine()
        .process_url_with_cache(&site.url, &options, &cache)
        .await
        .unwrap();
    assert_eq!(remote.content, local.content);
    assert!(remote.content.contains("Key Takeaways"));
    assert!(!remote.chunks.unwrap().is_empty());
    let _ = site
        .engine()
        .process_url_with_cache(&site.url, &options, &cache)
        .await
        .unwrap();
    assert_eq!(site.hits.load(Ordering::SeqCst), 1);
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn zip_u16(buffer: &mut Vec<u8>, value: u16) {
    buffer.extend_from_slice(&value.to_le_bytes());
}

fn zip_u32(buffer: &mut Vec<u8>, value: u32) {
    buffer.extend_from_slice(&value.to_le_bytes());
}

/// Builds a minimal stored ZIP so tests can serve DOCX bytes without an extra dependency.
fn zip_with(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut local = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = local.len() as u32;
        let crc = crc32(data.as_bytes());
        let size = data.len() as u32;
        zip_u32(&mut local, 0x0403_4b50);
        zip_u16(&mut local, 20);
        zip_u16(&mut local, 0x0800);
        zip_u16(&mut local, 0);
        zip_u16(&mut local, 0);
        zip_u16(&mut local, 0);
        zip_u32(&mut local, crc);
        zip_u32(&mut local, size);
        zip_u32(&mut local, size);
        zip_u16(&mut local, name.len() as u16);
        zip_u16(&mut local, 0);
        local.extend_from_slice(name.as_bytes());
        local.extend_from_slice(data.as_bytes());

        zip_u32(&mut central, 0x0201_4b50);
        zip_u16(&mut central, 20);
        zip_u16(&mut central, 20);
        zip_u16(&mut central, 0x0800);
        zip_u16(&mut central, 0);
        zip_u16(&mut central, 0);
        zip_u16(&mut central, 0);
        zip_u32(&mut central, crc);
        zip_u32(&mut central, size);
        zip_u32(&mut central, size);
        zip_u16(&mut central, name.len() as u16);
        zip_u16(&mut central, 0);
        zip_u16(&mut central, 0);
        zip_u16(&mut central, 0);
        zip_u16(&mut central, 0);
        zip_u32(&mut central, 0);
        zip_u32(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }
    let central_offset = local.len() as u32;
    let central_size = central.len() as u32;
    let mut archive = local;
    archive.extend_from_slice(&central);
    zip_u32(&mut archive, 0x0605_4b50);
    zip_u16(&mut archive, 0);
    zip_u16(&mut archive, 0);
    zip_u16(&mut archive, entries.len() as u16);
    zip_u16(&mut archive, entries.len() as u16);
    zip_u32(&mut archive, central_size);
    zip_u32(&mut archive, central_offset);
    zip_u16(&mut archive, 0);
    archive
}

#[tokio::test]
async fn ooxml_responses_use_native_office_extraction() {
    let docx = zip_with(&[
        (
            "word/document.xml",
            "<w:document><w:body><w:p><w:r><w:t>Quarterly numbers</w:t></w:r></w:p></w:body></w:document>",
        ),
        (
            "docProps/core.xml",
            "<cp:coreProperties><dc:title>Board Deck Notes</dc:title></cp:coreProperties>",
        ),
    ]);
    let site = Site::start(move |_request, _| Reply {
        status: 200,
        headers:
            "Content-Type: application/vnd.openxmlformats-officedocument.wordprocessingml.document\r\n"
                .into(),
        body: docx.clone(),
        delay_ms: 0,
    })
    .await;
    let doc = site
        .engine()
        .process_url(
            &format!("{}/report.docx", site.url),
            &ExtractionOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(doc.extraction.method, "office");
    assert_eq!(doc.title.as_deref(), Some("Board Deck Notes"));
    assert!(doc.content.contains("Quarterly numbers"), "{}", doc.content);
}

#[tokio::test]
async fn cookie_jar_captures_set_cookie_and_replays_on_same_host() {
    let jar_path = std::env::temp_dir().join(format!(
        "tkless-regression-cookie-jar-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&jar_path);
    let site = Site::start(|request, index| {
        if index == 0 {
            Reply {
                status: 200,
                headers: "Content-Type: text/html\r\nSet-Cookie: sid=abc; Path=/; HttpOnly\r\n"
                    .into(),
                body: b"<html><body><p>first</p></body></html>".to_vec(),
                delay_ms: 0,
            }
        } else {
            assert!(request.to_ascii_lowercase().contains("cookie: sid=abc"));
            Reply::html("<html><body><p>second</p></body></html>")
        }
    })
    .await;

    let fetcher = tkless_core::fetcher::Fetcher::new(FetchOptions {
        allow_private_addresses: true,
        cookie_jar: Some(jar_path.clone()),
        ..Default::default()
    })
    .unwrap();
    fetcher.fetch(&site.url).await.unwrap();
    fetcher.fetch(&site.url).await.unwrap();
    assert_eq!(site.hits.load(Ordering::SeqCst), 2);

    let reopened = tkless_core::CookieJar::open(&jar_path).unwrap();
    let parsed = site.url.parse().unwrap();
    assert_eq!(reopened.header_for(&parsed).as_deref(), Some("sid=abc"));
    let _ = std::fs::remove_file(&jar_path);
}
