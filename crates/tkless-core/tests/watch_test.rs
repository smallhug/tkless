use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use tkless_core::{
    collection::{CollectionConfig, Pagination},
    watch::{run_once, NotifySink, WatchConfig, WatchJob},
    ExtractionOptions, FetchOptions, TklessEngine,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn respond(mut stream: tokio::net::TcpStream, content_type: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = vec![0; 4096];
    loop {
        let read = stream.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buffer[..end]).to_string();
            let length = head
                .lines()
                .find_map(|line| {
                    let line = line.to_ascii_lowercase();
                    line.strip_prefix("content-length:")
                        .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                })
                .unwrap_or(0);
            if buffer.len() >= end + 4 + length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&buffer).to_string()
}

impl Server {
    async fn source(phase: Arc<AtomicUsize>) -> Self {
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", socket.local_addr().unwrap());
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = socket.accept().await {
                let phase = phase.load(Ordering::SeqCst);
                tokio::spawn(async move {
                    let _ = read_request(&mut stream).await;
                    let body = if phase == 0 {
                        r#"{"items":[{"id":1,"title":"a"},{"id":2,"title":"b"}],"next":null}"#
                    } else {
                        r#"{"items":[{"id":1,"title":"a2"}],"next":null}"#
                    };
                    respond(stream, "application/json", body).await;
                });
            }
        });
        Self { url, task }
    }

    async fn hook(bodies: Arc<Mutex<Vec<String>>>) -> Self {
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", socket.local_addr().unwrap());
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = socket.accept().await {
                let bodies = bodies.clone();
                tokio::spawn(async move {
                    let request = read_request(&mut stream).await;
                    if let Some((_, body)) = request.split_once("\r\n\r\n") {
                        bodies.lock().unwrap().push(body.to_string());
                    }
                    respond(stream, "text/plain", "ok").await;
                });
            }
        });
        Self { url, task }
    }
}

fn job_collection(url: &str) -> CollectionConfig {
    CollectionConfig {
        url: url.into(),
        id_pointer: Some("/id".into()),
        pagination: Pagination::Cursor {
            parameter: "cursor".into(),
            items_pointer: "/items".into(),
            next_pointer: "/next".into(),
        },
        ..Default::default()
    }
}

#[tokio::test]
async fn watch_rounds_emit_only_changed_events_to_file_and_webhook() {
    let phase = Arc::new(AtomicUsize::new(0));
    let source = Server::source(phase.clone()).await;
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let hook = Server::hook(bodies.clone()).await;
    let state = std::env::temp_dir().join(format!("tkless-watch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    let events_file = state.join("events.jsonl");
    let config = WatchConfig {
        jobs: vec![WatchJob {
            id: "news".into(),
            target: job_collection(&source.url),
            max_state_mb: 8,
            notify: vec![
                NotifySink::File {
                    path: events_file.clone(),
                },
                NotifySink::Http {
                    url: format!("{}/hook", hook.url),
                    timeout_ms: 5_000,
                    max_attempts: 1,
                    allow_private_addresses: true,
                    secret_env: None,
                },
            ],
            send_unchanged: false,
        }],
        ..Default::default()
    };
    let engine = TklessEngine::with_options(FetchOptions {
        allow_private_addresses: true,
        ..Default::default()
    })
    .unwrap();

    let report = run_once(&engine, &config, &ExtractionOptions::default(), &state)
        .await
        .unwrap();
    assert!(report.complete, "{:?}", report.errors);
    let kinds: Vec<_> = report.jobs[0]
        .events
        .iter()
        .map(|event| event.kind.as_str())
        .collect();
    assert_eq!(kinds, ["added", "added"]);
    assert_eq!(report.jobs[0].notified, 2);
    assert_eq!(
        std::fs::read_to_string(&events_file)
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_eq!(bodies.lock().unwrap().len(), 1);
    assert!(bodies.lock().unwrap()[0].contains("\"job\":\"news\""));
    assert!(bodies.lock().unwrap()[0].contains("\"added\""));

    phase.store(1, Ordering::SeqCst);
    let report = run_once(&engine, &config, &ExtractionOptions::default(), &state)
        .await
        .unwrap();
    let kinds: Vec<_> = report.jobs[0]
        .events
        .iter()
        .map(|event| event.kind.as_str())
        .collect();
    assert_eq!(kinds, ["updated", "removed"]);
    assert_eq!(
        std::fs::read_to_string(&events_file)
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(bodies.lock().unwrap().len(), 2);

    // An unchanged round must not notify anyone.
    let report = run_once(&engine, &config, &ExtractionOptions::default(), &state)
        .await
        .unwrap();
    assert!(report.jobs[0].events.is_empty());
    assert_eq!(report.jobs[0].notified, 0);
    assert_eq!(
        std::fs::read_to_string(&events_file)
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(bodies.lock().unwrap().len(), 2);

    let _ = std::fs::remove_dir_all(&state);
}

#[tokio::test]
async fn watch_reports_unreachable_jobs_without_lying_about_completion() {
    let state = std::env::temp_dir().join(format!("tkless-watch-fail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    let config = WatchConfig {
        jobs: vec![WatchJob {
            id: "offline".into(),
            target: CollectionConfig {
                url: "http://127.0.0.1:1/list".into(),
                id_pointer: Some("/id".into()),
                pagination: Pagination::Cursor {
                    parameter: "cursor".into(),
                    items_pointer: "/items".into(),
                    next_pointer: "/next".into(),
                },
                ..Default::default()
            },
            max_state_mb: 8,
            notify: Vec::new(),
            send_unchanged: false,
        }],
        ..Default::default()
    };
    let engine = TklessEngine::with_options(FetchOptions {
        allow_private_addresses: true,
        ..Default::default()
    })
    .unwrap();
    let report = run_once(&engine, &config, &ExtractionOptions::default(), &state)
        .await
        .unwrap();
    assert!(!report.complete);
    assert!(!report.errors.is_empty());
    assert!(report.jobs[0].error.is_some());
    assert!(report.jobs[0].generation.is_none());
    let _ = std::fs::remove_dir_all(&state);
}

#[tokio::test]
async fn failed_notifications_do_not_flip_round_completion() {
    let phase = Arc::new(AtomicUsize::new(0));
    let source = Server::source(phase).await;
    let state = std::env::temp_dir().join(format!("tkless-watch-sink-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    let config = WatchConfig {
        jobs: vec![WatchJob {
            id: "news".into(),
            target: job_collection(&source.url),
            max_state_mb: 8,
            notify: vec![NotifySink::Http {
                url: "http://127.0.0.1:1/hook".into(),
                timeout_ms: 1_000,
                max_attempts: 1,
                allow_private_addresses: true,
                secret_env: None,
            }],
            send_unchanged: false,
        }],
        ..Default::default()
    };
    let engine = TklessEngine::with_options(FetchOptions {
        allow_private_addresses: true,
        ..Default::default()
    })
    .unwrap();
    let report = run_once(&engine, &config, &ExtractionOptions::default(), &state)
        .await
        .unwrap();
    assert!(report.complete, "collection itself succeeded");
    assert!(report.errors.is_empty());
    assert_eq!(report.jobs[0].notified, 0);
    assert_eq!(report.jobs[0].notify_errors.len(), 1);
    let _ = std::fs::remove_dir_all(&state);
}
