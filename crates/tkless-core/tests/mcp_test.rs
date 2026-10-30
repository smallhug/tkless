use tkless_core::cache::DocumentCache;
use tkless_core::mcp::McpServer;
use tkless_core::TklessEngine;

#[tokio::test]
async fn structured_documents_preserve_text_and_report_strict_schema_errors() {
    let server = McpServer::new(
        TklessEngine::new().unwrap(),
        DocumentCache::memory_only(300),
    );
    let html = "<html><body><h1>Rust runtime</h1><p>Original source content.</p></body></html>";
    let success = server
        .dispatch_tool_execution(
            "clean_html",
            &serde_json::json!({"html":html,"mode":"full","format":"json"}),
            None,
            None,
        )
        .await;
    assert!(!success.is_error);
    let text: serde_json::Value = serde_json::from_str(&success.content[0].text).unwrap();
    assert_eq!(success.structured_content.unwrap()["document"], text);

    let soft = server
        .dispatch_tool_execution(
            "clean_html",
            &serde_json::json!({"html":html,"mode":"full","schema":"{\"title\":\".missing\"}"}),
            None,
            None,
        )
        .await;
    assert!(!soft.is_error);
    assert_eq!(
        soft.structured_content.unwrap()["document"]["warnings"][0]["code"],
        "schema_fallback"
    );
    let strict = server.dispatch_tool_execution("execute_tool", &serde_json::json!({"tool_name":"clean_html","arguments":{"html":html,"schema":"{\"title\":\".missing\"}","schema_strict":true}}), None, None).await;
    assert!(strict.is_error);
    assert_eq!(
        strict.structured_content.as_ref().unwrap()["error"]["code"],
        "schema_error"
    );
    assert_eq!(
        strict.structured_content.unwrap()["error"]["retryable"],
        false
    );
}

#[tokio::test]
async fn catalog_parameter_types_and_dependencies_fail_before_network() {
    let server = McpServer::new(
        TklessEngine::new().unwrap(),
        DocumentCache::memory_only(300),
    );
    for (name, args) in [
        (
            "web_search",
            serde_json::json!({"query":"Rust","diagnostics":"true"}),
        ),
        (
            "web_search",
            serde_json::json!({"query":"Rust","diagnostics":"true"}),
        ),
        (
            "web_search",
            serde_json::json!({"query":"Rust","schema_strict":true}),
        ),
        (
            "web_search",
            serde_json::json!({"query":"Rust","fetch_content":"false"}),
        ),
        (
            "batch_fetch_webpages",
            serde_json::json!({"urls":["https://example.com",17]}),
        ),
        (
            "fetch_webpage",
            serde_json::json!({"url":"https://example.com","schema_strict":true}),
        ),
        (
            "clean_html",
            serde_json::json!({"html":"<p>x</p>","schema_strict":"false"}),
        ),
        (
            "clean_html",
            serde_json::json!({"html":"<p>x</p>","mode":"invalid"}),
        ),
    ] {
        let result = server
            .dispatch_tool_execution(name, &args, None, None)
            .await;
        assert!(result.is_error, "{name}: {args}");
        assert_eq!(
            result.structured_content.unwrap()["error"]["code"],
            "invalid_argument"
        );
    }
    for name in [
        "fetch_webpage",
        "clean_html",
        "web_search",
        "batch_fetch_webpages",
    ] {
        let tools = tkless_core::mcp::get_available_tools();
        let tool = tools.iter().find(|tool| tool.name == name).unwrap();
        assert!(tool.output_schema.is_some());
    }
}

#[tokio::test]
async fn only_current_protocol_is_advertised_and_outputs_never_downgrade() {
    let server = McpServer::new(
        TklessEngine::new().unwrap(),
        DocumentCache::memory_only(300),
    );
    for version in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"] {
        let init = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":version}});
        let initialized = server.handle_message(&init.to_string()).await.unwrap();
        assert_eq!(initialized.result.unwrap()["protocolVersion"], "2025-11-25");
        let list = server
            .handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .await
            .unwrap()
            .result
            .unwrap();
        let tool = list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "fetch_webpage")
            .unwrap();
        assert!(tool.get("outputSchema").is_some());
        let call = serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"clean_html","arguments":{"html":"<p>Source content</p>","mode":"full"}}});
        let result = server
            .handle_message(&call.to_string())
            .await
            .unwrap()
            .result
            .unwrap();
        assert!(result.get("structuredContent").is_some());
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Source content"));
    }
}

#[tokio::test]
async fn bing_search_schema_and_argument_validation_stay_in_sync() {
    let server = McpServer::new(
        TklessEngine::new().unwrap(),
        DocumentCache::memory_only(300),
    );
    let tools = tkless_core::mcp::tools::get_available_tools();
    for name in ["web_search", "deep_research"] {
        let tool = tools.iter().find(|tool| tool.name == name).unwrap();
        assert!(tool.input_schema["properties"].get("engine").is_none());
        assert_eq!(tool.input_schema["additionalProperties"], false);
        let mut args = if name == "web_search" {
            serde_json::json!({"query":"Rust"})
        } else {
            serde_json::json!({"topic":"Rust"})
        };
        args["engine"] = serde_json::json!("bing");
        let request = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}});
        let response = server
            .handle_message(&request.to_string())
            .await
            .unwrap()
            .result
            .unwrap();
        assert_eq!(response["isError"], true);
        assert!(response["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Unknown argument"));
    }
    for args in [
        serde_json::json!({"query":"Rust","limit":0}),
        serde_json::json!({"query":"Rust","scope":"unknown"}),
        serde_json::json!({"query":"Rust","dedup":true}),
        serde_json::json!({"query":"Rust","fetch_content":true,"max_tokens_per_page":0}),
    ] {
        let request = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"web_search","arguments":args}});
        assert_eq!(
            server
                .handle_message(&request.to_string())
                .await
                .unwrap()
                .result
                .unwrap()["isError"],
            true
        );
    }
}

#[tokio::test]
async fn test_mcp_protocol_lifecycle() {
    let engine = TklessEngine::new().expect("Engine should initialize");
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
    assert_eq!(res_val["protocolVersion"], "2025-11-25");
    assert_eq!(res_val["serverInfo"]["name"], "tkless");

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
    assert_eq!(tools_array.len(), 26);

    let tool_names: Vec<&str> = tools_array
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(tool_names.contains(&"fetch_webpage"));
    assert!(tool_names.contains(&"fetch_bilibili_feed"));
    assert!(tool_names.contains(&"search_bilibili"));
    assert!(tool_names.contains(&"download_media"));
    assert!(tool_names.contains(&"web_search"));
    assert!(tool_names.contains(&"deep_research"));
    assert!(tool_names.contains(&"crawl_site"));
    assert!(tool_names.contains(&"batch_fetch_webpages"));
    assert!(tool_names.contains(&"clean_html"));
    assert!(tool_names.contains(&"login_platform"));
    assert!(tool_names.contains(&"scaffold_schema"));
    assert!(tool_names.contains(&"compare_similarity"));
    assert!(tool_names.contains(&"manage_cache"));
    assert!(tool_names.contains(&"fetch_feed"));
    assert!(tool_names.contains(&"inspect_links"));
    assert!(tool_names.contains(&"check_auth"));
    assert!(tool_names.contains(&"summarize_content"));
    assert!(tool_names.contains(&"split_document"));
    assert!(tool_names.contains(&"chunk_markdown"));
    assert!(tool_names.contains(&"diff_documents"));
    assert!(tool_names.contains(&"take_screenshot"));
    assert!(tool_names.contains(&"export_tables"));
    assert!(tool_names.contains(&"list_adapters"));

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
    let clean_req = r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"clean_html","arguments":{"html":"<!DOCTYPE html><html><head><title>TkLess Clean Test</title></head><body><article><h1>TkLess Clean Test</h1><p>Content for Agent</p></article><aside class=\"advertisement\"><p>Junk Advertisement</p></aside><script>console.log(\"bad\");</script></body></html>","frontmatter":true}}}"#;
    let clean_res = server
        .handle_message(clean_req)
        .await
        .expect("Should return response");
    let clean_val = clean_res.result.expect("Clean result should be present");
    assert_ne!(clean_val["isError"], true);
    let text = clean_val["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("TkLess Clean Test"));
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
    let sim_req = r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"compare_similarity","arguments":{"text1":"TkLess is a high-performance, token-efficient web content extraction engine designed specifically for Large Language Models and AI Agents. It removes navigation bars, scripts, advertisements, and boilerplate using heuristic algorithms, converting the core content into LLM-optimized Markdown. Native adapters for Bilibili, WeChat, Zhihu, and GitHub are supported out of the box with zero overhead. By eliminating aggressive noise, TkLess delivers significant token savings across production AI pipelines.","text2":"TkLess is a high-performance, token-efficient web content extraction engine designed specifically for Large Language Models and AI Agents. It removes navigation bars, scripts, advertisements, and boilerplate using heuristic algorithms, converting the core content into LLM-optimized Markdown. Native adapters for Bilibili, WeChat, Zhihu, and GitHub are supported out of the box with zero overhead. By eliminating aggressive noise, TkLess delivers significant token savings across production AI pipelines. Reposted with permission."}}}"#;
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

    // 5k. Tool Call: check_auth
    let auth_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 15,
        "method": "tools/call",
        "params": {
            "name": "check_auth",
            "arguments": {
                "platform": "wechat",
                "cookie": "fixture_cookie=unverified"
            }
        }
    });
    let auth_res = server
        .handle_message(&auth_req.to_string())
        .await
        .expect("Should return response");
    let auth_val = auth_res.result.expect("Auth result should be present");
    let auth_text = auth_val["content"][0]["text"].as_str().unwrap();
    let auth_status: serde_json::Value = serde_json::from_str(auth_text).unwrap();
    assert_eq!(auth_status["status"], "configured");
    assert_eq!(auth_status["verified"], false);

    // 5l. Tool Call: clean_html with summarize
    let sum_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 16,
        "method": "tools/call",
        "params": {
            "name": "clean_html",
            "arguments": {
                "html": "<article><h1>Rust AI Engineering</h1><p>Rust provides zero-cost abstractions, strong memory safety, and thread concurrency.</p><p>TkLess delivers sub-millisecond local web content extraction for intelligent AI agents.</p><p>Extractive summarization distills key takeaway bullet points without external model API calls.</p></article>",
                "summarize": true
            }
        }
    });
    let sum_res = server
        .handle_message(&sum_req.to_string())
        .await
        .expect("Should return response");
    let sum_val = sum_res.result.expect("Summarize result should be present");
    let sum_text = sum_val["content"][0]["text"].as_str().unwrap();
    assert!(sum_text.contains("核心要点速览 (Key Takeaways)"));
    assert!(sum_text.contains("- "));

    // 5m. Tool Call: summarize_text
    let standalone_sum_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 17,
        "method": "tools/call",
        "params": {
            "name": "summarize_content",
            "arguments": {
                "text": "Artificial Intelligence is transforming engineering workflows across modern development environments. TkLess provides an ultra-fast token-efficient web content extraction engine designed specifically for LLMs. By stripping aggressive advertisements and noisy navigation elements, TkLess reduces LLM context token consumption by up to 90%.",
                "max_bullets": 2,
                "format": "markdown"
            }
        }
    });
    let standalone_sum_res = server
        .handle_message(&standalone_sum_req.to_string())
        .await
        .expect("Should return response");
    let standalone_sum_val = standalone_sum_res
        .result
        .expect("Summarize text result should be present");
    let standalone_sum_text = standalone_sum_val["content"][0]["text"].as_str().unwrap();
    assert!(standalone_sum_text.contains("核心要点速览 (Key Takeaways)"));
    assert!(standalone_sum_text.contains("- "));

    // 5n. Tool Call: split_document
    let split_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 18,
        "method": "tools/call",
        "params": {
            "name": "split_document",
            "arguments": {
                "markdown": "# Chapter 1: Introduction\nThis is chapter 1.\n## Subchapter 1.1\nDetails here.\n# Chapter 2: Conclusion\nFinal remarks.",
                "max_heading_level": 2
            }
        }
    });
    let split_res = server
        .handle_message(&split_req.to_string())
        .await
        .expect("Should return response");
    let split_val = split_res.result.expect("Split result should be present");
    let split_text = split_val["content"][0]["text"].as_str().unwrap();
    let parsed_sections: serde_json::Value = serde_json::from_str(split_text).unwrap();
    let sections_arr = parsed_sections.as_array().unwrap();
    assert_eq!(sections_arr.len(), 3);
    assert_eq!(sections_arr[0]["title"], "Chapter 1: Introduction");
    assert_eq!(sections_arr[1]["title"], "Subchapter 1.1");
    assert_eq!(sections_arr[2]["title"], "Chapter 2: Conclusion");

    // 5o. Tool Call: diff_content
    let diff_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 19,
        "method": "tools/call",
        "params": {
            "name": "diff_documents",
            "arguments": {
                "old_text": "Release 1.0\nBug fixes and stability.\n",
                "new_text": "Release 2.0\nBug fixes and stability.\nNew feature added.\n",
                "format": "json"
            }
        }
    });
    let diff_res = server
        .handle_message(&diff_req.to_string())
        .await
        .expect("Should return response");
    let diff_val = diff_res.result.expect("Diff result should be present");
    let diff_text = diff_val["content"][0]["text"].as_str().unwrap();
    let parsed_diff: serde_json::Value = serde_json::from_str(diff_text).unwrap();
    assert_eq!(parsed_diff["added_lines"], 2);
    assert_eq!(parsed_diff["removed_lines"], 1);
    assert!(parsed_diff["unified_diff"]
        .as_str()
        .unwrap()
        .contains("+Release 2.0"));

    // 5p. Tool Call: capture_screenshot (SSRF rejection guard test)
    let shot_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 20,
        "method": "tools/call",
        "params": {
            "name": "take_screenshot",
            "arguments": {
                "url": "http://169.254.169.254/latest/meta-data/"
            }
        }
    });
    let shot_res = server
        .handle_message(&shot_req.to_string())
        .await
        .expect("Should return response");
    let shot_val = shot_res
        .result
        .expect("Screenshot result should be present");
    assert_eq!(shot_val["isError"], true);
    let shot_text = shot_val["content"][0]["text"].as_str().unwrap();
    assert!(shot_text.contains("SSRF"));

    // 5q. Tool Call: convert_table to SQL
    let table_sample = r#"
        <table>
            <thead>
                <tr><th>Quarter</th><th>Revenue</th><th>Growth</th></tr>
            </thead>
            <tbody>
                <tr><td>Q1 2026</td><td>1500000</td><td>0.25</td></tr>
                <tr><td>Q2 2026</td><td>1950000</td><td>0.30</td></tr>
            </tbody>
        </table>
    "#;
    let convert_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 21,
        "method": "tools/call",
        "params": {
            "name": "export_tables",
            "arguments": {
                "html": table_sample,
                "format": "sql",
                "table_name": "quarterly_financials"
            }
        }
    });
    let convert_res = server
        .handle_message(&convert_req.to_string())
        .await
        .expect("Should return response");
    let convert_val = convert_res
        .result
        .expect("Convert table result should be present");
    let convert_sql = convert_val["content"][0]["text"].as_str().unwrap();
    assert!(convert_sql.contains("CREATE TABLE IF NOT EXISTS quarterly_financials"));
    assert!(convert_sql.contains("INSERT INTO quarterly_financials"));
    assert!(convert_sql.contains("Q1 2026"));

    // 5r. Tool Call: export_tables and CSV format
    let export_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 22,
        "method": "tools/call",
        "params": {
            "name": "export_tables",
            "arguments": {
                "html": "<table><thead><tr><th>Asset</th><th>Value</th></tr></thead><tbody><tr><td>Bitcoin</td><td>95000</td></tr><tr><td>Ethereum</td><td>3500</td></tr></tbody></table>",
                "format": "csv",
                "table_index": 0
            }
        }
    });
    let export_res = server
        .handle_message(&export_req.to_string())
        .await
        .expect("Should return response");
    let export_val = export_res
        .result
        .expect("Export tables result should be present");
    let export_csv = export_val["content"][0]["text"].as_str().unwrap();
    assert!(export_csv.contains("Asset,Value"));
    assert!(export_csv.contains("Bitcoin,95000"));

    // 5s. Tool Call: chunk_markdown
    let chunk_md_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 23,
        "method": "tools/call",
        "params": {
            "name": "chunk_markdown",
            "arguments": {
                "markdown": "# Overview\nTkLess is an ultra-fast token-efficient extraction engine for LLMs.\n## Architecture\nBuilt in pure Rust with strict stream separation and MCP stdio support.\n## Features\nSupports 15+ platform adapters.",
                "chunk_size": 25,
                "chunk_overlap": 5,
                "format": "json"
            }
        }
    });
    let chunk_md_res = server
        .handle_message(&chunk_md_req.to_string())
        .await
        .expect("Should return response");
    let chunk_md_val = chunk_md_res
        .result
        .expect("Chunk markdown result should be present");
    let chunk_md_text = chunk_md_val["content"][0]["text"].as_str().unwrap();
    let parsed_chunks: serde_json::Value = serde_json::from_str(chunk_md_text).unwrap();
    let chunks_arr = parsed_chunks.as_array().expect("Should be array of chunks");
    assert!(chunks_arr.len() >= 2);
    assert!(chunks_arr[0]["content"]
        .as_str()
        .unwrap()
        .contains("TkLess"));

    // 5t. Tool Call: summarize_content
    let sum_content_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 24,
        "method": "tools/call",
        "params": {
            "name": "summarize_content",
            "arguments": {
                "text": "TkLess provides an ultra-fast web content extraction engine designed specifically for LLMs. By stripping aggressive advertisements and noisy navigation elements, TkLess reduces LLM context token consumption by up to 90%. Native adapters for GitHub, Bilibili, WeChat, Zhihu, and Wikipedia are included.",
                "max_bullets": 2,
                "format": "json"
            }
        }
    });
    let sum_content_res = server
        .handle_message(&sum_content_req.to_string())
        .await
        .expect("Should return response");
    let sum_content_val = sum_content_res
        .result
        .expect("Summarize content result should be present");
    let sum_content_text = sum_content_val["content"][0]["text"].as_str().unwrap();
    let parsed_sum: serde_json::Value = serde_json::from_str(sum_content_text).unwrap();
    assert!(parsed_sum.as_array().unwrap().len() <= 2);

    // 5u. Tool Call: diff_documents
    let diff_doc_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 25,
        "method": "tools/call",
        "params": {
            "name": "diff_documents",
            "arguments": {
                "old_text": "Version 1.0\nInitial release",
                "new_text": "Version 1.1\nInitial release\nAdded MCP extensions",
                "format": "markdown"
            }
        }
    });
    let diff_doc_res = server
        .handle_message(&diff_doc_req.to_string())
        .await
        .expect("Should return response");
    let diff_doc_val = diff_doc_res
        .result
        .expect("Diff documents result should be present");
    let diff_doc_text = diff_doc_val["content"][0]["text"].as_str().unwrap();
    assert!(diff_doc_text.contains("+Version 1.1"));
    assert!(diff_doc_text.contains("+Added MCP extensions"));

    // 5v. Tool Call: list_adapters
    let list_adapters_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 26,
        "method": "tools/call",
        "params": {
            "name": "list_adapters",
            "arguments": {
                "format": "markdown"
            }
        }
    });
    let list_adapters_res = server
        .handle_message(&list_adapters_req.to_string())
        .await
        .expect("Should return response");
    let list_adapters_val = list_adapters_res
        .result
        .expect("List adapters result should be present");
    let list_adapters_text = list_adapters_val["content"][0]["text"].as_str().unwrap();
    assert!(list_adapters_text.contains("TkLess 原生平台适配器矩阵"));
    assert!(list_adapters_text.contains("Bilibili"));
    assert!(list_adapters_text.contains("Zhihu"));
    assert!(list_adapters_text.contains("GitHub"));
    assert!(list_adapters_text.contains("ArXiv"));

    // 5w. Token Measurement for CLI vs MCP
    use tkless_core::mcp::tools::get_available_tools;
    use tkless_core::model::TokenizerKind;
    use tkless_core::token::count_tokens;

    let tools = get_available_tools();
    let mcp_json_compact = serde_json::to_string(&tools).unwrap();
    let mcp_json_pretty = serde_json::to_string_pretty(&tools).unwrap();

    let mcp_o200k_compact = count_tokens(&mcp_json_compact, TokenizerKind::O200k);
    let mcp_cl100k_compact = count_tokens(&mcp_json_compact, TokenizerKind::Cl100k);
    let mcp_o200k_pretty = count_tokens(&mcp_json_pretty, TokenizerKind::O200k);
    let mcp_cl100k_pretty = count_tokens(&mcp_json_pretty, TokenizerKind::Cl100k);

    let adapters_md = tkless_core::adapters::format_adapters_markdown(
        &tkless_core::adapters::AdapterRegistry::list_adapter_metadata(),
    );
    let ad_o200k = count_tokens(&adapters_md, TokenizerKind::O200k);
    let ad_cl100k = count_tokens(&adapters_md, TokenizerKind::Cl100k);

    println!("\n=== MCP Tools (22 tools schema) Token Metrics ===");
    println!(
        "Compact JSON: chars={}, o200k={}, cl100k={}",
        mcp_json_compact.len(),
        mcp_o200k_compact,
        mcp_cl100k_compact
    );
    println!(
        "Pretty JSON:  chars={}, o200k={}, cl100k={}",
        mcp_json_pretty.len(),
        mcp_o200k_pretty,
        mcp_cl100k_pretty
    );
    println!(
        "tkless adapters Markdown: chars={}, o200k={}, cl100k={}",
        adapters_md.len(),
        ad_o200k,
        ad_cl100k
    );

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
    let engine = TklessEngine::new().expect("Engine should initialize");
    let cache = DocumentCache::memory_only(300);
    let server = McpServer::new(engine, cache);

    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::channel(16);

    let batch_req = r#"{
        "jsonrpc": "2.0",
        "id": 11,
        "method": "tools/call",
        "params": {
            "name": "batch_fetch_webpages",
            "arguments": {
                "urls": ["http://127.0.0.1/page1", "http://127.0.0.1/page2"]
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

#[tokio::test]
async fn test_mcp_two_stage_dynamic_discovery() {
    use tkless_core::mcp::McpMode;
    use tkless_core::model::TokenizerKind;
    use tkless_core::token::count_tokens;

    let engine = TklessEngine::new().expect("Engine should initialize");
    let cache = DocumentCache::memory_only(300);
    let server = McpServer::new_with_mode(engine, cache, McpMode::Dynamic);
    assert_eq!(server.mode(), McpMode::Dynamic);

    // 1. Initial tools/list in Dynamic Mode: should expose only 6 tools (2 Core + 4 Meta)
    let list_req = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let list_res = server
        .handle_message(list_req)
        .await
        .expect("Should return response");
    let tools_val = list_res.result.expect("Tools list should be present");
    let tools_array = tools_val["tools"].as_array().expect("Tools array");
    assert_eq!(tools_array.len(), 6);

    let tool_names: Vec<&str> = tools_array
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(tool_names.contains(&"fetch_webpage"));
    assert!(tool_names.contains(&"web_search"));
    assert!(tool_names.contains(&"discover_tools"));
    assert!(tool_names.contains(&"get_tool_schema"));
    assert!(tool_names.contains(&"execute_tool"));
    assert!(tool_names.contains(&"activate_tools"));
    // Ensure specialized tools are NOT in the initial tools/list
    assert!(!tool_names.contains(&"export_tables"));
    assert!(!tool_names.contains(&"diff_documents"));

    // Measure Token counts for Two-Stage Dynamic Discovery Mode
    let compact_json = serde_json::to_string(&tools_val).unwrap();
    let pretty_json = serde_json::to_string_pretty(&tools_val).unwrap();
    let dyn_o200k_compact = count_tokens(&compact_json, TokenizerKind::O200k);
    let dyn_cl100k_compact = count_tokens(&compact_json, TokenizerKind::Cl100k);
    let dyn_o200k_pretty = count_tokens(&pretty_json, TokenizerKind::O200k);
    let dyn_cl100k_pretty = count_tokens(&pretty_json, TokenizerKind::Cl100k);

    println!("\n=== MCP Two-Stage Dynamic Discovery Token Metrics ===");
    println!(
        "Dynamic Initial Schema (6 tools): chars={}, o200k={}, cl100k={}",
        compact_json.len(),
        dyn_o200k_compact,
        dyn_cl100k_compact
    );
    println!(
        "Dynamic Initial Pretty JSON:     chars={}, o200k={}, cl100k={}",
        pretty_json.len(),
        dyn_o200k_pretty,
        dyn_cl100k_pretty
    );

    // 2. Stage 1: discover_tools by keyword
    let disc_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "discover_tools",
            "arguments": {
                "query": "table"
            }
        }
    });
    let disc_res = server
        .handle_message(&disc_req.to_string())
        .await
        .expect("Response");
    let disc_text = disc_res.result.unwrap()["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(disc_text.contains("export_tables"));
    assert!(disc_text.contains("analysis"));

    // 3. Stage 1: discover_tools by category
    let disc_cat_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "discover_tools",
            "arguments": {
                "category": "media"
            }
        }
    });
    let disc_cat_res = server
        .handle_message(&disc_cat_req.to_string())
        .await
        .expect("Response");
    let disc_cat_text = disc_cat_res.result.unwrap()["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(disc_cat_text.contains("fetch_bilibili_feed"));
    assert!(disc_cat_text.contains("download_media"));
    assert!(disc_cat_text.contains("take_screenshot"));

    // 4. Stage 1: get_tool_schema on-demand inspection
    let schema_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "get_tool_schema",
            "arguments": {
                "tool_name": "export_tables"
            }
        }
    });
    let schema_res = server
        .handle_message(&schema_req.to_string())
        .await
        .expect("Response");
    let schema_text = schema_res.result.unwrap()["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(schema_text.contains("export_tables"));
    assert!(schema_text.contains("table_format") || schema_text.contains("format"));
    assert!(schema_text.contains("table_name"));

    // 5. Stage 2: execute_tool on-demand execution (without registering to tools/list)
    let sample_table = "<table><thead><tr><th>Language</th><th>Speed</th></tr></thead><tbody><tr><td>Rust</td><td>UltraFast</td></tr></tbody></table>";
    let exec_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "execute_tool",
            "arguments": {
                "tool_name": "export_tables",
                "arguments": {
                    "html": sample_table,
                    "format": "sql",
                    "table_name": "lang_bench"
                }
            }
        }
    });
    let exec_res = server
        .handle_message(&exec_req.to_string())
        .await
        .expect("Response");
    let exec_text = exec_res.result.unwrap()["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(exec_text.contains("CREATE TABLE IF NOT EXISTS lang_bench"));
    assert!(exec_text.contains("INSERT INTO lang_bench"));
    assert!(exec_text.contains("Rust"));

    // 6. Stage 2: activate_tools dynamic activation with list_changed notification
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::channel(16);
    let act_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tools/call",
        "params": {
            "name": "activate_tools",
            "arguments": {
                "tool_names": ["export_tables", "diff_documents"]
            }
        }
    });
    let act_res = server
        .handle_message_with_notifier(&act_req.to_string(), Some(&notify_tx))
        .await
        .expect("Response");
    let act_text = act_res.result.unwrap()["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(act_text.contains("成功激活工具"));
    assert!(act_text.contains("export_tables"));
    assert!(act_text.contains("diff_documents"));

    // Verify notifications/tools/list_changed was emitted
    let notif = notify_rx
        .try_recv()
        .expect("Should receive list_changed notification");
    assert_eq!(notif["method"], "notifications/tools/list_changed");

    // 7. Verify subsequent tools/list now includes the activated tools (6 -> 8)
    let list_req2 = r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#;
    let list_res2 = server
        .handle_message(list_req2)
        .await
        .expect("Should return response");
    let tools_val2 = list_res2.result.expect("Tools list should be present");
    let tools_array2 = tools_val2["tools"].as_array().expect("Tools array");
    assert_eq!(tools_array2.len(), 8);
    let tool_names2: Vec<&str> = tools_array2
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(tool_names2.contains(&"export_tables"));
    assert!(tool_names2.contains(&"diff_documents"));

    // 8. Recursive execution guard
    let rec_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 8,
        "method": "tools/call",
        "params": {
            "name": "execute_tool",
            "arguments": {
                "tool_name": "execute_tool",
                "arguments": {}
            }
        }
    });
    let rec_res = server
        .handle_message(&rec_req.to_string())
        .await
        .expect("Response");
    let rec_val = rec_res.result.unwrap();
    assert_eq!(rec_val["isError"], true);
    assert!(rec_val["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("recursively"));
}
