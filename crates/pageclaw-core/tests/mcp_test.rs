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
    let init_res = server
        .handle_message(init_req)
        .await
        .expect("Should return response");
    assert_eq!(init_res.id, serde_json::json!(1));
    let res_val = init_res.result.expect("Result should be present");
    assert_eq!(res_val["protocolVersion"], "2024-11-05");
    assert_eq!(res_val["serverInfo"]["name"], "pageclaw");

    // 2. Initialized Notification (should return None)
    let initialized_notif = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    assert!(server.handle_message(initialized_notif).await.is_none());

    // 3. Ping
    let ping_req = r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#;
    let ping_res = server
        .handle_message(ping_req)
        .await
        .expect("Should return response");
    assert_eq!(ping_res.id, serde_json::json!(2));
    assert!(ping_res.result.is_some());

    // 4. Tools List
    let list_req = r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#;
    let list_res = server
        .handle_message(list_req)
        .await
        .expect("Should return response");
    let tools_val = list_res.result.expect("Tools list should be present");
    let tools_array = tools_val["tools"]
        .as_array()
        .expect("Tools should be an array");
    assert_eq!(tools_array.len(), 14);

    let tool_names: Vec<&str> = tools_array
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(tool_names.contains(&"fetch_webpage"));
    assert!(tool_names.contains(&"fetch_bilibili_feed"));
    assert!(tool_names.contains(&"search_bilibili"));
    assert!(tool_names.contains(&"download_media"));
    assert!(tool_names.contains(&"web_search"));
    assert!(tool_names.contains(&"crawl_site"));
    assert!(tool_names.contains(&"batch_fetch_webpages"));
    assert!(tool_names.contains(&"clean_html"));
    assert!(tool_names.contains(&"login_platform"));
    assert!(tool_names.contains(&"scaffold_schema"));
    assert!(tool_names.contains(&"compare_similarity"));
    assert!(tool_names.contains(&"manage_cache"));
    assert!(tool_names.contains(&"fetch_feed"));
    assert!(tool_names.contains(&"inspect_links"));

    // Verify web_search schema contains scope, cdp_endpoint, schema
    let search_tool = tools_array
        .iter()
        .find(|t| t["name"] == "web_search")
        .expect("web_search must exist");
    let search_props = &search_tool["inputSchema"]["properties"];
    assert!(search_props.get("scope").is_some());
    assert!(search_props.get("cdp_endpoint").is_some());
    assert!(search_props.get("schema").is_some());
    assert!(search_props.get("proxy").is_some());
    assert!(search_props.get("mobile").is_some());
    assert!(search_props.get("dedup").is_some());

    // Verify crawl_site schema contains max_tokens, cdp_endpoint, schema, sitemap, dedup, proxy, mobile
    let crawl_tool = tools_array
        .iter()
        .find(|t| t["name"] == "crawl_site")
        .expect("crawl_site must exist");
    let crawl_props = &crawl_tool["inputSchema"]["properties"];
    assert!(crawl_props.get("max_tokens").is_some());
    assert!(crawl_props.get("cdp_endpoint").is_some());
    assert!(crawl_props.get("schema").is_some());
    assert!(crawl_props.get("sitemap").is_some());
    assert!(crawl_props.get("dedup").is_some());
    assert!(crawl_props.get("proxy").is_some());
    assert!(crawl_props.get("mobile").is_some());

    // Verify batch_fetch_webpages schema contains proxy, mobile
    let batch_tool = tools_array
        .iter()
        .find(|t| t["name"] == "batch_fetch_webpages")
        .expect("batch_fetch_webpages must exist");
    let batch_props = &batch_tool["inputSchema"]["properties"];
    assert!(batch_props.get("proxy").is_some());
    assert!(batch_props.get("mobile").is_some());

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
    assert!(props.get("chunk").is_some());
    assert!(props.get("chunk_size").is_some());
    assert!(props.get("chunk_overlap").is_some());
    assert!(props.get("proxy").is_some());
    assert!(props.get("download_images").is_some());
    assert!(props.get("assets_dir").is_some());
    assert!(props.get("tables_only").is_some());
    assert!(props.get("table_format").is_some());
    assert!(props.get("mobile").is_some());
    assert!(props.get("stats").is_some());
    assert!(props.get("remove_selector").is_some());

    // 5. Tools Call (Validation Error)
    let call_req = r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_bilibili","arguments":{}}}"#;
    let call_res = server
        .handle_message(call_req)
        .await
        .expect("Should return response");
    let call_val = call_res.result.expect("Call result should be present");
    assert_eq!(call_val["isError"], true);

    // 5b. Tool Call: clean_html
    let clean_req = r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"clean_html","arguments":{"html":"<!DOCTYPE html><html><head><title>PageClaw Clean Test</title></head><body><article><h1>PageClaw Clean Test</h1><p>Content for Agent</p></article><aside class=\"advertisement\"><p>Junk Advertisement</p></aside><script>console.log(\"bad\");</script></body></html>","frontmatter":true}}}"#;
    let clean_res = server
        .handle_message(clean_req)
        .await
        .expect("Should return response");
    let clean_val = clean_res.result.expect("Clean result should be present");
    assert_ne!(clean_val["isError"], true);
    let text = clean_val["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("PageClaw Clean Test"));
    assert!(text.contains("Content for Agent"));
    assert!(!text.contains("Junk Advertisement"));
    assert!(!text.contains("console.log"));

    // 5c. Tool Call: login_platform zhihu instruction
    let login_req = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"login_platform","arguments":{"platform":"zhihu"}}}"#;
    let login_res = server
        .handle_message(login_req)
        .await
        .expect("Should return response");
    let login_val = login_res.result.expect("Login result should be present");
    let login_text = login_val["content"][0]["text"].as_str().unwrap();
    assert!(login_text.contains("https://www.zhihu.com/signin"));
    assert!(login_text.contains("zhihu:"));

    // 5d. Tool Call: scaffold_schema
    let scaffold_req = r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"scaffold_schema","arguments":{"html":"<div class=\"grid\"><div class=\"card\"><h2 class=\"title\">Item 1</h2><span class=\"price\">10</span></div><div class=\"card\"><h2 class=\"title\">Item 2</h2><span class=\"price\">20</span></div><div class=\"card\"><h2 class=\"title\">Item 3</h2><span class=\"price\">30</span></div></div>"}}}"#;
    let scaffold_res = server
        .handle_message(scaffold_req)
        .await
        .expect("Should return response");
    let scaffold_val = scaffold_res
        .result
        .expect("Scaffold result should be present");
    let scaffold_text = scaffold_val["content"][0]["text"].as_str().unwrap();
    assert!(scaffold_text.contains("card"));
    assert!(scaffold_text.contains("```json"));

    // 5e. Tool Call: clean_html with chunking enabled
    let chunk_req = r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"clean_html","arguments":{"html":"<article><h1>Doc Title</h1><p>Introduction paragraph</p><h2>Section 1</h2><p>P1 text for section 1</p><h2>Section 2</h2><p>P2 text for section 2</p></article>","chunk":true,"chunk_size":20}}}"#;
    let chunk_res = server
        .handle_message(chunk_req)
        .await
        .expect("Should return response");
    let chunk_val = chunk_res.result.expect("Chunk result should be present");
    let chunk_text = chunk_val["content"][0]["text"].as_str().unwrap();
    assert!(chunk_text.contains("语义切块输出"));
    assert!(chunk_text.contains("Chunk 1/"));

    // 5f. Tool Call: clean_html with tables_only and table_format: csv
    let table_req = r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"clean_html","arguments":{"html":"<html><body><p>Unwanted article text</p><table><thead><tr><th>Product</th><th>Price</th></tr></thead><tbody><tr><td>Widget A</td><td>100</td></tr><tr><td>Widget B</td><td>200</td></tr></tbody></table></body></html>","tables_only":true,"table_format":"csv"}}}"#;
    let table_res = server
        .handle_message(table_req)
        .await
        .expect("Should return response");
    let table_val = table_res.result.expect("Table result should be present");
    let table_text = table_val["content"][0]["text"].as_str().unwrap();
    assert!(table_text.contains("Product,Price"));
    assert!(table_text.contains("Widget A,100"));
    assert!(!table_text.contains("Unwanted article text"));

    // 5g. Tool Call: compare_similarity
    let sim_req = r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"compare_similarity","arguments":{"text1":"PageClaw is a high-performance, token-efficient web content extraction engine designed specifically for Large Language Models and AI Agents. It removes navigation bars, scripts, advertisements, and boilerplate using heuristic algorithms, converting the core content into LLM-optimized Markdown. Native adapters for Bilibili, WeChat, Zhihu, and GitHub are supported out of the box with zero overhead.","text2":"PageClaw is a high-performance, token-efficient web content extraction engine designed specifically for Large Language Models and AI Agents. It removes navigation bars, scripts, advertisements, and boilerplate using heuristic algorithms, converting the core content into LLM-optimized Markdown. Native adapters for Bilibili, WeChat, Zhihu, and GitHub are supported out of the box with low memory."}}}"#;
    let sim_res = server
        .handle_message(sim_req)
        .await
        .expect("Should return response");
    let sim_val = sim_res.result.expect("Similarity result should be present");
    let sim_text = sim_val["content"][0]["text"].as_str().unwrap();
    let parsed_sim: serde_json::Value = serde_json::from_str(sim_text).unwrap();
    assert_eq!(parsed_sim["is_near_duplicate"], true);
    assert!(parsed_sim["hamming_distance"].as_u64().unwrap() <= 3);

    // 5h. Tool Call: manage_cache (stats)
    let cache_req = r#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"manage_cache","arguments":{"action":"stats"}}}"#;
    let cache_res = server
        .handle_message(cache_req)
        .await
        .expect("Should return response");
    let cache_val = cache_res
        .result
        .expect("Cache stats result should be present");
    let cache_text = cache_val["content"][0]["text"].as_str().unwrap();
    let parsed_cache: serde_json::Value = serde_json::from_str(cache_text).unwrap();
    assert!(parsed_cache.get("memory_entries").is_some());
    assert!(parsed_cache.get("disk_entries").is_some());

    // 5i. Tool Call: fetch_feed
    let feed_sample = r#"<?xml version="1.0" encoding="UTF-8"?><rss version="2.0"><channel><title>Rust Blog</title><link>https://blog.rust-lang.org</link><item><title>Rust 1.85</title><link>https://blog.rust-lang.org/1.85.html</link><description>Release news</description></item></channel></rss>"#;
    let feed_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 13,
        "method": "tools/call",
        "params": {
            "name": "fetch_feed",
            "arguments": {
                "xml": feed_sample,
                "format": "markdown"
            }
        }
    });
    let feed_res = server
        .handle_message(&feed_req.to_string())
        .await
        .expect("Should return response");
    let feed_val = feed_res.result.expect("Feed result should be present");
    let feed_text = feed_val["content"][0]["text"].as_str().unwrap();
    assert!(feed_text.contains("Rust Blog"));
    assert!(feed_text.contains("Rust 1.85"));
    assert!(feed_text.contains("Release news"));

    // 5j. Tool Call: inspect_links
    let inspect_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 14,
        "method": "tools/call",
        "params": {
            "name": "inspect_links",
            "arguments": {
                "urls": ["http://127.0.0.1/blocked_ssrf", "https://example.com/check"],
                "format": "json"
            }
        }
    });
    let inspect_res = server
        .handle_message(&inspect_req.to_string())
        .await
        .expect("Should return response");
    let inspect_val = inspect_res
        .result
        .expect("Inspect result should be present");
    let inspect_text = inspect_val["content"][0]["text"].as_str().unwrap();
    let parsed_links: serde_json::Value = serde_json::from_str(inspect_text).unwrap();
    assert_eq!(parsed_links.as_array().unwrap().len(), 2);
    assert!(parsed_links[0]["error"].as_str().unwrap().contains("SSRF"));

    // 5. Unknown Method Error
    let unknown_req = r#"{"jsonrpc":"2.0","id":4,"method":"unknown/method"}"#;
    let unknown_res = server
        .handle_message(unknown_req)
        .await
        .expect("Should return error response");
    assert!(unknown_res.error.is_some());
    assert_eq!(unknown_res.error.unwrap().code, -32601);

    // 6. Malformed JSON
    let malformed_req = "not valid json";
    let malformed_res = server
        .handle_message(malformed_req)
        .await
        .expect("Should return parse error");
    assert!(malformed_res.error.is_some());
    assert_eq!(malformed_res.error.unwrap().code, -32700);
}

#[tokio::test]
async fn test_mcp_progress_notification_streaming() {
    let engine = PageclawEngine::new().expect("Engine should initialize");
    let cache = DocumentCache::memory_only(300);
    let server = McpServer::new(engine, cache);

    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel();

    let batch_req = r#"{
        "jsonrpc": "2.0",
        "id": 11,
        "method": "tools/call",
        "params": {
            "name": "batch_fetch_webpages",
            "arguments": {
                "urls": ["https://example.com/page1", "https://example.com/page2"]
            },
            "_meta": {
                "progressToken": "batch-job-42"
            }
        }
    }"#;

    let res = server
        .handle_message_with_notifier(batch_req, Some(&notify_tx))
        .await
        .expect("Should return response");

    assert_eq!(res.id, serde_json::json!(11));

    // Verify progress notifications were emitted
    let mut notifications = Vec::new();
    while let Ok(notif) = notify_rx.try_recv() {
        notifications.push(notif);
    }

    assert_eq!(notifications.len(), 2);
    assert_eq!(notifications[0]["method"], "notifications/progress");
    assert_eq!(notifications[0]["params"]["progressToken"], "batch-job-42");
    assert_eq!(notifications[0]["params"]["progress"], 1);
    assert_eq!(notifications[0]["params"]["total"], 2);

    assert_eq!(notifications[1]["method"], "notifications/progress");
    assert_eq!(notifications[1]["params"]["progressToken"], "batch-job-42");
    assert_eq!(notifications[1]["params"]["progress"], 2);
    assert_eq!(notifications[1]["params"]["total"], 2);
}
