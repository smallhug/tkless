use pageclaw_core::cache::DocumentCache;
use pageclaw_core::mcp::McpServer;
use pageclaw_core::PageclawEngine;

#[tokio::test]
async fn test_mcp_protocol_lifecycle() {
    let engine = PageclawEngine::new().expect("Engine should initialize");
    let cache = DocumentCache::memory_only(300);
    let server = McpServer::new(engine, cache);

    // 1. Initialize
    let init_req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test-client","version":"1.0"}}}"#;
    let init_res = server.handle_message(init_req).await.expect("Should return response");
    assert_eq!(init_res.id, serde_json::json!(1));
    let res_val = init_res.result.expect("Result should be present");
    assert_eq!(res_val["protocolVersion"], "2024-11-05");
    assert_eq!(res_val["serverInfo"]["name"], "pageclaw");

    // 2. Initialized Notification (should return None)
    let initialized_notif = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    assert!(server.handle_message(initialized_notif).await.is_none());

    // 3. Ping
    let ping_req = r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#;
    let ping_res = server.handle_message(ping_req).await.expect("Should return response");
    assert_eq!(ping_res.id, serde_json::json!(2));
    assert!(ping_res.result.is_some());

    // 4. Tools List
    let list_req = r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#;
    let list_res = server.handle_message(list_req).await.expect("Should return response");
    let tools_val = list_res.result.expect("Tools list should be present");
    let tools_array = tools_val["tools"].as_array().expect("Tools should be an array");
    assert_eq!(tools_array.len(), 5);

    let tool_names: Vec<&str> = tools_array
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(tool_names.contains(&"fetch_webpage"));
    assert!(tool_names.contains(&"fetch_bilibili_feed"));
    assert!(tool_names.contains(&"search_bilibili"));
    assert!(tool_names.contains(&"download_media"));
    assert!(tool_names.contains(&"web_search"));

    // Verify fetch_webpage schema contains new Direction 2 & 3 parameters
    let fetch_tool = tools_array
        .iter()
        .find(|t| t["name"] == "fetch_webpage")
        .expect("fetch_webpage must exist");
    let props = &fetch_tool["inputSchema"]["properties"];
    assert!(props.get("selector").is_some());
    assert!(props.get("extract_links").is_some());
    assert!(props.get("extract_images").is_some());
    assert!(props.get("extract_tables").is_some());
    assert!(props.get("subtitles").is_some());
    assert!(props.get("danmaku").is_some());
    assert!(props.get("danmaku_limit").is_some());
    assert!(props.get("comments").is_some());
    assert!(props.get("comments_limit").is_some());
    assert!(props.get("conclusion").is_some());
    assert!(props.get("cookie").is_some());
    assert!(props.get("cookie_file").is_some());

    // 5. Tools Call (Validation Error)
    let call_req = r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_bilibili","arguments":{}}}"#;
    let call_res = server.handle_message(call_req).await.expect("Should return response");
    let call_val = call_res.result.expect("Call result should be present");
    assert_eq!(call_val["isError"], true);

    // 5. Unknown Method Error
    let unknown_req = r#"{"jsonrpc":"2.0","id":4,"method":"unknown/method"}"#;
    let unknown_res = server.handle_message(unknown_req).await.expect("Should return error response");
    assert!(unknown_res.error.is_some());
    assert_eq!(unknown_res.error.unwrap().code, -32601);

    // 6. Malformed JSON
    let malformed_req = "not valid json";
    let malformed_res = server.handle_message(malformed_req).await.expect("Should return parse error");
    assert!(malformed_res.error.is_some());
    assert_eq!(malformed_res.error.unwrap().code, -32700);
}
