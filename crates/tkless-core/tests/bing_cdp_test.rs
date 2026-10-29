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
                let body="<title>Bing fixture</title><ol id='b_results'><li class='b_algo'><h2><a href='https://example.com/rust'>Rust language</a></h2><div class='b_caption'><p>Rust programming language fixture.</p></div></li></ol>";
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
            .stderr(Stdio::null())
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
