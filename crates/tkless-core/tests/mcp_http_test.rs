use tkless_core::{mcp::HttpTransportOptions, DocumentCache, McpServer, TklessEngine};

const VERSION: &str = "2026-07-28";

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

fn request(id: u64, method: &str, params: serde_json::Value) -> serde_json::Value {
    let mut params = params;
    params["_meta"] = serde_json::json!({
        "io.modelcontextprotocol/protocolVersion": VERSION,
        "io.modelcontextprotocol/clientCapabilities": {}
    });
    serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}

fn post<'a>(
    client: &'a reqwest::Client,
    base: &str,
    body: &'a serde_json::Value,
) -> reqwest::RequestBuilder {
    let method = body["method"].as_str().unwrap_or_default();
    let mut builder = client
        .post(format!("{base}/mcp"))
        .header("MCP-Protocol-Version", VERSION)
        .header("Mcp-Method", method);
    if method == "tools/call" {
        builder = builder.header(
            "Mcp-Name",
            body["params"]["name"].as_str().unwrap_or_default(),
        );
    }
    builder.json(body)
}

#[tokio::test]
async fn http_transport_serves_discovery_tools_and_health() {
    let (base, task) = spawn_server(None).await;
    let client = client();

    let health = client.get(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(health.status(), 200);

    let discover = request(1, "server/discover", serde_json::json!({}));
    let response = post(&client, &base, &discover).send().await.unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    assert_eq!(status, 200, "{text}");
    let body: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["result"]["resultType"], "complete");
    assert_eq!(body["result"]["supportedVersions"][0], VERSION);

    let list = request(2, "tools/list", serde_json::json!({}));
    let response = post(&client, &base, &list).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["result"]["tools"].as_array().unwrap().len(), 31);
    assert_eq!(body["result"]["ttlMs"], 300_000);
    assert_eq!(body["result"]["cacheScope"], "public");

    // Malformed payloads still receive a JSON-RPC error rather than a transport panic.
    let response = client
        .post(format!("{base}/mcp"))
        .header("MCP-Protocol-Version", VERSION)
        .header("Mcp-Method", "tools/list")
        .header("Content-Type", "application/json")
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
    assert_eq!(response.status(), 405);
    task.abort();
}

#[tokio::test]
async fn http_transport_enforces_protocol_headers_and_request_meta() {
    let (base, task) = spawn_server(None).await;
    let client = client();

    let list = request(1, "tools/list", serde_json::json!({}));
    let missing_headers = client
        .post(format!("{base}/mcp"))
        .json(&list)
        .send()
        .await
        .unwrap();
    assert_eq!(missing_headers.status(), 400);
    let body: serde_json::Value = missing_headers.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32020);

    let missing_meta =
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}});
    let response = client
        .post(format!("{base}/mcp"))
        .header("MCP-Protocol-Version", VERSION)
        .header("Mcp-Method", "tools/list")
        .json(&missing_meta)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32602);

    let mut old_version = request(3, "tools/list", serde_json::json!({}));
    old_version["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] =
        serde_json::json!("2025-11-25");
    let response = client
        .post(format!("{base}/mcp"))
        .header("MCP-Protocol-Version", "2025-11-25")
        .header("Mcp-Method", "tools/list")
        .json(&old_version)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32022);
    assert_eq!(body["error"]["data"]["supported"][0], VERSION);

    let call = request(
        4,
        "tools/call",
        serde_json::json!({"name":"clean_html","arguments":{"html":"<p>hi</p>"}}),
    );
    let response = client
        .post(format!("{base}/mcp"))
        .header("MCP-Protocol-Version", VERSION)
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "fetch_webpage")
        .json(&call)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32020);

    let response = post(&client, &base, &call).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["result"]["resultType"], "complete");
    assert_eq!(body["result"]["isError"], false);

    // Legacy clients that omit modern headers still get a deterministic
    // diagnostic naming the supported version.
    let legacy = serde_json::json!({"jsonrpc":"2.0","id":9,"method":"initialize","params":{}});
    let response = client
        .post(format!("{base}/mcp"))
        .json(&legacy)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32601);
    assert_eq!(body["error"]["data"]["supported"][0], VERSION);

    task.abort();
}

#[tokio::test]
async fn http_transport_rejects_cross_origin_and_non_json_requests() {
    let (base, task) = spawn_server(None).await;
    let client = client();
    let list = request(1, "tools/list", serde_json::json!({}));

    let cross_origin = client
        .post(format!("{base}/mcp"))
        .header("Origin", "https://attacker.example")
        .header("MCP-Protocol-Version", VERSION)
        .header("Mcp-Method", "tools/list")
        .json(&list)
        .send()
        .await
        .unwrap();
    assert_eq!(cross_origin.status(), 403);

    // DNS rebinding keeps the socket on loopback but sends the attacker's hostname.
    let rebound_host = client
        .post(format!("{base}/mcp"))
        .header("Host", "attacker.example")
        .header("MCP-Protocol-Version", VERSION)
        .header("Mcp-Method", "tools/list")
        .json(&list)
        .send()
        .await
        .unwrap();
    assert_eq!(rebound_host.status(), 403);

    let simple_form_post = client
        .post(format!("{base}/mcp"))
        .header("Content-Type", "text/plain")
        .header("MCP-Protocol-Version", VERSION)
        .header("Mcp-Method", "tools/list")
        .body(list.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(simple_form_post.status(), 415);

    task.abort();
}

#[tokio::test]
async fn http_transport_enforces_bearer_token() {
    let (base, task) = spawn_server(Some("secret")).await;
    let client = client();
    let discover = request(1, "server/discover", serde_json::json!({}));

    let unauthorized = client
        .post(format!("{base}/mcp"))
        .header("MCP-Protocol-Version", VERSION)
        .header("Mcp-Method", "server/discover")
        .json(&discover)
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), 401);

    let authorized = client
        .post(format!("{base}/mcp"))
        .header("Authorization", "Bearer secret")
        .header("MCP-Protocol-Version", VERSION)
        .header("Mcp-Method", "server/discover")
        .json(&discover)
        .send()
        .await
        .unwrap();
    assert_eq!(authorized.status(), 200);
    let body: serde_json::Value = authorized.json().await.unwrap();
    assert_eq!(body["result"]["supportedVersions"][0], VERSION);
    task.abort();
}
