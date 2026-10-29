#[path = "support/batch_site.rs"]
pub mod support;
use serde_json::{json, Value};
use std::time::Duration;
use support::{Site, TempDir};
use tkless_core::{DocumentCache, McpServer};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    time::timeout,
};

#[tokio::test]
async fn jsonrpc_version_is_required_without_legacy_defaulting() {
    let server = McpServer::new(
        tkless_core::TklessEngine::new().unwrap(),
        DocumentCache::memory_only(10),
    );
    let response = server
        .handle_message(r#"{"id":1,"method":"tools/list"}"#)
        .await
        .unwrap();
    assert_eq!(response.error.unwrap().code, -32600);
    let malformed = server.handle_message("{bad").await.unwrap();
    assert_eq!(malformed.error.unwrap().code, -32700);
}

#[tokio::test]
async fn canonical_tool_names_are_required_in_all_dispatch_paths() {
    let server = McpServer::new(
        tkless_core::TklessEngine::new().unwrap(),
        DocumentCache::memory_only(10),
    );
    for name in [
        "search_web",
        "tkless_web_search",
        "summarize_text",
        "diff_content",
        "convert_table",
    ] {
        let result = server
            .dispatch_tool_execution(name, &json!({}), None, None)
            .await;
        assert!(result.is_error, "{name}");
        let result = server
            .dispatch_tool_execution(
                "execute_tool",
                &json!({"tool_name":name,"arguments":{}}),
                None,
                None,
            )
            .await;
        assert!(result.is_error, "{name}");
        let result = server
            .dispatch_tool_execution("get_tool_schema", &json!({"tool_name":name}), None, None)
            .await;
        assert!(result.is_error, "{name}");
        let result = server
            .dispatch_tool_execution("activate_tools", &json!({"tool_names":[name]}), None, None)
            .await;
        assert!(result.is_error, "{name}");
    }
}

#[tokio::test]
async fn batch_streaming_requires_a_notification_token_before_network_or_checkpoint_creation() {
    let site = Site::start().await;
    let dir = TempDir::default();
    let path = dir.file("job.jsonl");
    let server = McpServer::new(site.engine(), DocumentCache::memory_only(10));
    let result = server
        .dispatch_tool_execution(
            "batch_fetch_webpages",
            &json!({"urls":site.urls(&["/ok"]),"stream_results":true,"checkpoint":path}),
            None,
            None,
        )
        .await;
    assert!(result.is_error);
    assert_eq!(
        result.structured_content.unwrap()["error"]["code"],
        "invalid_argument"
    );
    assert_eq!(site.count("/ok"), 0);
    assert!(!path.exists());
    for extra in [
        json!({"resume":true}),
        json!({"retry_failed":true}),
        json!({"delay_ms":-1}),
        json!({"concurrency":0}),
    ] {
        let mut args = json!({"urls":site.urls(&["/ok"])});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let rejected = server
            .dispatch_tool_execution("batch_fetch_webpages", &args, None, None)
            .await;
        assert!(rejected.is_error);
        assert_eq!(
            rejected.structured_content.unwrap()["error"]["code"],
            "invalid_argument"
        );
    }
    assert_eq!(site.count("/ok"), 0);
}

#[tokio::test]
async fn streamed_items_arrive_before_completion_and_final_response_is_summary_only() {
    let site = Site::start().await;
    let server = McpServer::new(site.engine(), DocumentCache::memory_only(10));
    let (mut input, input_server) = tokio::io::duplex(8192);
    let (output_server, output) = tokio::io::duplex(8192);
    let service = tokio::spawn(async move {
        server
            .run_transport(input_server, output_server)
            .await
            .unwrap()
    });
    let mut output = BufReader::new(output);
    let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"batch_fetch_webpages","_meta":{"progressToken":"job"},"arguments":{"urls":site.urls(&["/held","/fast"]),"order":"completion","stream_results":true,"format":"json"}}});
    input
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    let mut line = String::new();
    timeout(Duration::from_secs(5), output.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let first: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(first["method"], "notifications/progress");
    assert_eq!(first["params"]["_meta"]["tkless/batch_item"]["index"], 1);
    site.release();
    let mut indices = vec![1u64];
    loop {
        line.clear();
        timeout(Duration::from_secs(5), output.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        let message: Value = serde_json::from_str(&line).unwrap();
        if message.get("id") == Some(&json!(1)) {
            assert_eq!(message["result"]["isError"], false);
            let result = &message["result"]["structuredContent"];
            assert_eq!(result["total"], 2);
            assert_eq!(result["streamed"], true);
            assert!(result.get("results").is_none());
            assert_eq!(indices, [1, 0]);
            break;
        }
        indices.push(
            message["params"]["_meta"]["tkless/batch_item"]["index"]
                .as_u64()
                .unwrap(),
        );
    }
    input.shutdown().await.unwrap();
    timeout(Duration::from_secs(5), service)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn bounded_notification_queue_applies_backpressure_and_can_be_drained() {
    let site = Site::start().await;
    let server = McpServer::new(site.engine(), DocumentCache::memory_only(10));
    let urls: Vec<_> = (0..20)
        .map(|index| format!("{}/fast{index}", site.url))
        .collect();
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let task = tokio::spawn(async move {
        server
            .dispatch_tool_execution(
                "batch_fetch_webpages",
                &json!({"urls":urls,"concurrency":2,"stream_results":true}),
                Some(&tx),
                Some(&json!("job")),
            )
            .await
    });
    timeout(Duration::from_secs(5), async {
        while rx.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(site.hits.lock().unwrap().len() <= 4);
    let mut received = 0;
    while timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .is_some()
    {
        received += 1;
    }
    assert_eq!(received, 20);
    let final_result = task.await.unwrap();
    assert!(!final_result.is_error);
    assert_eq!(final_result.structured_content.unwrap()["fetched"], 20);
}

#[tokio::test]
async fn mcp_resume_and_failed_retry_share_the_durable_job_contract() {
    let site = Site::start().await;
    let dir = TempDir::default();
    let path = dir.file("job.jsonl");
    let server = McpServer::new(site.engine(), DocumentCache::memory_only(10));
    let mut args = json!({"urls":site.urls(&["/ok","/flaky"]),"checkpoint":path,"format":"json"});
    let first = server
        .dispatch_tool_execution("batch_fetch_webpages", &args, None, None)
        .await;
    assert!(!first.is_error);
    assert_eq!(first.structured_content.unwrap()["failed"], 1);
    args["resume"] = json!(true);
    args["retry_failed"] = json!(true);
    let resumed = server
        .dispatch_tool_execution("batch_fetch_webpages", &args, None, None)
        .await;
    assert!(!resumed.is_error);
    let result = resumed.structured_content.unwrap();
    assert_eq!(result["resumed"], 1);
    assert_eq!(result["fetched"], 1);
    assert_eq!(result["failed"], 0);
    assert_eq!((site.count("/ok"), site.count("/flaky")), (1, 2));
}
