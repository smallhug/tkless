//! Optional real-browser regression for Bing rendering and task-tab cleanup.
use serde_json::Value;
use std::{
    process::{Child, Command, Stdio},
    time::Duration,
};
use tkless_core::cdp::{fetch_html_via_cdp, locate_system_browser, CdpOptions};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::{sleep, timeout},
};

struct BrowserGuard(Child);
impl Drop for BrowserGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
#[ignore = "requires local Chrome/Edge; opens a disposable headless browser"]
async fn bing_browser_rendering_closes_only_its_task_tab() {
    let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", http.local_addr().unwrap());
    let service = tokio::spawn(async move {
        loop {
            let (mut stream, _) = http.accept().await.unwrap();
            tokio::spawn(async move {
                let mut bytes = [0; 8192];
                let Ok(n) = stream.read(&mut bytes).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                let request = String::from_utf8_lossy(&bytes[..n]);
                let body = if request.starts_with("GET /interactive ") {
                    r#"<input id="query"><button id="expand" type="button" onclick="document.querySelector('#result').textContent=document.querySelector('#query').value;document.querySelector('#result').hidden=false">Expand</button><p id="result" hidden></p><a id="next" href="/next">Next</a>"#
                } else if request.starts_with("GET /next ") {
                    "<h1 id=loaded>Next page loaded</h1>"
                } else {
                    "<title>Bing fixture</title><ol id='b_results'><li class='b_algo'><h2><a href='https://example.com/rust'>Rust language</a></h2><div class='b_caption'><p>Rust programming language fixture.</p></div></li></ol>"
                };
                let response=format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",body.len());
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let profile = std::env::temp_dir().join(format!(
        "tkless-bing-cdp-test-{}-{port}",
        std::process::id()
    ));
    let _browser = BrowserGuard(
        Command::new(locate_system_browser().expect("Chrome/Edge must be installed"))
            .args([
                "--headless=new",
                "--no-first-run",
                "--no-default-browser-check",
                "--no-proxy-server",
            ])
            .arg(format!("--remote-debugging-port={port}"))
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let endpoint = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let mut initial = None;
    for _ in 0..100 {
        if let Ok(response) = client.get(format!("{endpoint}/json/list")).send().await {
            if let Ok(list) = response.json::<Vec<Value>>().await {
                let ids = list
                    .iter()
                    .filter(|page| page["type"] == "page")
                    .map(|page| page["id"].clone())
                    .collect::<Vec<_>>();
                if !ids.is_empty() {
                    initial = Some(ids);
                    break;
                }
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
    let initial = initial.expect("The unrelated initial browser tab must exist");
    let options = CdpOptions {
        allow_private_addresses: true,
        endpoint: endpoint.clone(),
        wait_ms: Some(100),
        timeout_ms: Some(30_000),
        ..Default::default()
    };
    let html = timeout(
        Duration::from_secs(35),
        fetch_html_via_cdp(&format!("{origin}/search?q=Rust"), &options),
    )
    .await
    .unwrap()
    .unwrap();
    let results = tkless_core::search::parse_bing_html_with_query(&html, "", 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].engine, "bing");
    let screenshot = tkless_core::capture_screenshot_via_cdp(&origin, &options, Some("png"))
        .await
        .unwrap();
    assert!(screenshot.starts_with(b"\x89PNG\r\n\x1a\n"));
    let engine = tkless_core::TklessEngine::with_options(tkless_core::FetchOptions {
        allow_private_addresses: true,
        ..Default::default()
    })
    .unwrap();
    use tkless_core::{
        browser_workflow::{browse_page, BrowserPlan},
        cdp::BrowserStep,
    };
    let mut plan = BrowserPlan {
        endpoint: Some(endpoint.clone()),
        steps: vec![
            BrowserStep::WaitFor {
                selector: "#query".into(),
                timeout_ms: 3000,
            },
            BrowserStep::Fill {
                selector: "#query".into(),
                value: "Rust 工具 \"quoted\"".into(),
            },
            BrowserStep::Click {
                selector: "#expand".into(),
            },
            BrowserStep::WaitFor {
                selector: "#result".into(),
                timeout_ms: 3000,
            },
        ],
        ..Default::default()
    };
    let doc = browse_page(
        &engine,
        &format!("{origin}/interactive"),
        &plan,
        &tkless_core::ExtractionOptions::default(),
    )
    .await
    .unwrap();
    assert!(doc.content.contains("Rust 工具"));
    plan.steps = vec![
        BrowserStep::Click {
            selector: "#next".into(),
        },
        BrowserStep::WaitFor {
            selector: "#loaded".into(),
            timeout_ms: 3000,
        },
    ];
    let doc = browse_page(
        &engine,
        &format!("{origin}/interactive"),
        &plan,
        &tkless_core::ExtractionOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(doc.final_url, format!("{origin}/next"));
    let remaining = client
        .get(format!("{endpoint}/json/list"))
        .send()
        .await
        .unwrap()
        .json::<Vec<Value>>()
        .await
        .unwrap();
    let ids = remaining
        .iter()
        .filter(|page| page["type"] == "page")
        .map(|page| page["id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        ids, initial,
        "Rendering must preserve unrelated tabs and close its task tab"
    );
    service.abort();
}

#[tokio::test]
#[ignore = "requires local Chrome/Edge; opens a disposable headless browser"]
async fn shadow_frames_scroll_and_xhr_capture_are_opt_in() {
    const PAGE: &str = r#"<!doctype html><html><body>
        <div id="host"></div>
        <iframe id="frame" src="/frame"></iframe>
        <ul id="list"></ul>
        <script>
            const root = document.getElementById('host').attachShadow({mode:'open'});
            const paragraph = document.createElement('p');
            paragraph.textContent = ['Shadow', 'body', 'text'].join(' ');
            root.appendChild(paragraph);
            fetch('/api/items').then((r) => r.json()).then((data) => {
                const list = document.getElementById('list');
                for (const item of data.items) {
                    const li = document.createElement('li');
                    li.textContent = item.title;
                    list.appendChild(li);
                }
            });
        </script>
    </body></html>"#;
    let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", http.local_addr().unwrap());
    let service = tokio::spawn(async move {
        loop {
            let (mut stream, _) = http.accept().await.unwrap();
            tokio::spawn(async move {
                let mut bytes = [0; 8192];
                let Ok(n) = stream.read(&mut bytes).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                let request = String::from_utf8_lossy(&bytes[..n]).to_string();
                let (content_type, body) = if request.starts_with("GET /api/items ") {
                    ("application/json", r#"{"items":[{"title":"XHR item"}]}"#)
                } else if request.starts_with("GET /frame ") {
                    ("text/html", r#"<h2 id="frame-text">Frame body text</h2>"#)
                } else {
                    ("text/html", PAGE)
                };
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len());
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let profile =
        std::env::temp_dir().join(format!("tkless-ws2-cdp-test-{}-{port}", std::process::id()));
    let _browser = BrowserGuard(
        Command::new(locate_system_browser().expect("Chrome/Edge must be installed"))
            .args([
                "--headless=new",
                "--no-first-run",
                "--no-default-browser-check",
                "--no-proxy-server",
            ])
            .arg(format!("--remote-debugging-port={port}"))
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let endpoint = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let mut ready = false;
    for _ in 0..100 {
        if client
            .get(format!("{endpoint}/json/list"))
            .send()
            .await
            .is_ok()
        {
            ready = true;
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "headless browser did not start");
    let engine = tkless_core::TklessEngine::with_options(tkless_core::FetchOptions {
        allow_private_addresses: true,
        ..Default::default()
    })
    .unwrap();
    use tkless_core::{
        browser_workflow::{browse_page, BrowserPlan},
        cdp::{BrowserStep, NetworkCapture},
    };
    let plan = BrowserPlan {
        endpoint: Some(endpoint.clone()),
        include_shadow_dom: true,
        include_frames: true,
        capture: Some(NetworkCapture {
            mime_prefixes: vec!["application/json".into()],
            max_responses: 5,
            max_body_bytes: 65_536,
            same_origin_only: true,
        }),
        steps: vec![
            BrowserStep::WaitFor {
                selector: "#list li".into(),
                timeout_ms: 5000,
            },
            BrowserStep::ScrollUntilStable {
                max_steps: 5,
                idle_rounds: 1,
                step_px: 200,
            },
        ],
        ..Default::default()
    };
    let doc = browse_page(
        &engine,
        &format!("{origin}/ws2"),
        &plan,
        &tkless_core::ExtractionOptions::default(),
    )
    .await
    .unwrap();
    let rendered = tkless_core::browser_workflow::render(&engine, &format!("{origin}/ws2"), &plan)
        .await
        .unwrap();
    assert!(
        rendered.html.contains("Shadow body text"),
        "{}",
        rendered.html
    );
    assert!(
        rendered.html.contains("Frame body text"),
        "{}",
        rendered.html
    );
    assert!(rendered.html.contains("XHR item"), "{}", rendered.html);
    assert!(
        rendered.html.contains("data-frame-url=\"/frame\""),
        "{}",
        rendered.html
    );
    let entity = doc.entity.as_ref().expect("browser capture entity");
    assert_eq!(entity["type"], "browser_capture");
    assert!(entity["network"][0]["body"]
        .as_str()
        .unwrap()
        .contains("XHR item"));
    assert!(entity["scroll_stopped_reason"].is_string());

    let default_plan = BrowserPlan {
        endpoint: Some(endpoint.clone()),
        steps: vec![BrowserStep::WaitFor {
            selector: "#list li".into(),
            timeout_ms: 5000,
        }],
        ..Default::default()
    };
    let rendered =
        tkless_core::browser_workflow::render(&engine, &format!("{origin}/ws2"), &default_plan)
            .await
            .unwrap();
    assert!(!rendered.html.contains("Shadow body text"));
    assert!(!rendered.html.contains("Frame body text"));
    assert!(rendered.html.contains("XHR item"));
    assert!(rendered.network.is_empty());
    service.abort();
}
