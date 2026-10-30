use tkless_core::{mcp::HttpTransportOptions, DocumentCache, McpServer, TklessEngine};

async fn spawn_server(token: Option<&str>) -> (String, tokio::task::JoinHandle<()>) {
    let engine = TklessEngine::with_options(Default::default()).unwrap();
    let server = McpServer::new(engine, DocumentCache::memory_only(60)).with_tool_timeout(10);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let auth_token = token.map(str::to_owned);
    let task = tokio::spawn(async move {
        let _ = server
            .serve_http(
                listener,
                HttpTransportOptions {
                    auth_token,
                    ..Default::default()
                },
            )
            .await;
    });
    (format!("http://{address}"), task)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap()
}

#[tokio::test]
async fn http_transport_serves_json_rpc_and_health() {
    let (base, task) = spawn_server(None).await;
    let client = client();

    let health = client.get(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(health.status(), 200);

    let response = client
        .post(format!("{base}/mcp"))
        .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    assert_eq!(status, 200, "{text}");
    let body: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["result"]["tools"].as_array().unwrap().len(), 31);

    // Malformed payloads still receive a JSON-RPC error rather than a transport panic.
    let response = client
        .post(format!("{base}/mcp"))
        .body("not json")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["error"].is_object());

    let response = client
        .put(format!("{base}/mcp"))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    task.abort();
}

#[tokio::test]
async fn http_transport_enforces_bearer_token() {
    let (base, task) = spawn_server(Some("secret")).await;
    let client = client();

    let unauthorized = client
        .post(format!("{base}/mcp"))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), 401);

    let authorized = client
        .post(format!("{base}/mcp"))
        .header("Authorization", "Bearer secret")
        .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"ping"}))
        .send()
        .await
        .unwrap();
    assert_eq!(authorized.status(), 200);
    let body: serde_json::Value = authorized.json().await.unwrap();
    assert_eq!(body["result"], serde_json::json!({}));
    task.abort();
}
