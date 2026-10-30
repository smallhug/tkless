use std::path::PathBuf;
use tkless_core::{ExtractionOptions, FetchOptions, TklessEngine};

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start_server() -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer).await;
            let body = "<html><head><title>Fixture title</title></head><body><article><h1>Hello adapter</h1><p>Body text from fixture.</p></article></body></html>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    Server { url, task }
}

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tkless-config-adapter-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn declarative_adapter_extracts_schema_and_markdown() {
    let site = start_server().await;
    let dir = temp_dir();
    std::fs::write(
        dir.join("fixture.json"),
        br#"{
            "name": "local-fixture",
            "hosts": ["127.0.0.1"],
            "schema": {"fields": {"heading": "h1", "paragraph": "p"}}
        }"#,
    )
    .unwrap();
    std::env::set_var("TKLESS_ADAPTERS_DIR", &dir);

    let engine = TklessEngine::with_options(FetchOptions {
        allow_private_addresses: true,
        ..Default::default()
    })
    .unwrap();
    let doc = engine
        .process_url(&format!("{}/post", site.url), &ExtractionOptions::default())
        .await
        .unwrap();
    assert_eq!(doc.extraction.adapter.as_deref(), Some("local-fixture"));
    assert_eq!(doc.extraction.method, "platform_adapter");
    assert_eq!(doc.title.as_deref(), Some("Fixture title"));
    assert!(
        doc.content.contains("Body text from fixture."),
        "{}",
        doc.content
    );
    let entity = doc.entity.expect("schema entity");
    assert_eq!(entity["heading"], "Hello adapter");
    assert_eq!(entity["paragraph"], "Body text from fixture.");

    let _ = std::fs::remove_dir_all(&dir);
    std::env::remove_var("TKLESS_ADAPTERS_DIR");
}
