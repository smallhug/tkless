use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::Notify,
};

pub struct Site {
    pub url: String,
    pub hits: Arc<Mutex<Vec<(String, Instant)>>>,
    pub peak: Arc<AtomicUsize>,
    held: Arc<AtomicBool>,
    release: Arc<Notify>,
    task: tokio::task::JoinHandle<()>,
}

impl Site {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(Mutex::new(Vec::new()));
        let attempts = Arc::new(Mutex::new(HashMap::<String, usize>::new()));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let held = Arc::new(AtomicBool::new(true));
        let release = Arc::new(Notify::new());
        let (h, p, r, gate) = (hits.clone(), peak.clone(), release.clone(), held.clone());
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (hits, attempts, active, peak, release, held) = (
                    h.clone(),
                    attempts.clone(),
                    active.clone(),
                    p.clone(),
                    r.clone(),
                    gate.clone(),
                );
                tokio::spawn(async move {
                    let mut reader = BufReader::new(stream);
                    let mut first = String::new();
                    if reader.read_line(&mut first).await.unwrap_or(0) == 0 {
                        return;
                    }
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).await.unwrap_or(0) == 0 || header == "\r\n"
                        {
                            break;
                        }
                    }
                    let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let attempt = {
                        let mut map = attempts.lock().unwrap();
                        let count = map.entry(path.clone()).or_default();
                        *count += 1;
                        *count
                    };
                    hits.lock().unwrap().push((path.clone(), Instant::now()));
                    let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(count, Ordering::SeqCst);
                    if path.starts_with("/held") {
                        let released = release.notified();
                        tokio::pin!(released);
                        released.as_mut().enable();
                        if held.load(Ordering::SeqCst) {
                            released.await;
                        }
                    }
                    if path.starts_with("/slow") {
                        tokio::time::sleep(Duration::from_millis(60)).await;
                    }
                    let status = if path.starts_with("/flaky") && attempt == 1 {
                        503
                    } else {
                        200
                    };
                    let body = format!("<html><body><h1>{path}</h1><p>Batch fixture source content.</p></body></html>");
                    let response = format!("HTTP/1.1 {status} Test\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = reader.into_inner().write_all(response.as_bytes()).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        Self {
            url,
            hits,
            peak,
            held,
            release,
            task,
        }
    }
    pub fn release(&self) {
        self.held.store(false, Ordering::SeqCst);
        self.release.notify_waiters();
    }
    pub fn urls(&self, paths: &[&str]) -> Vec<String> {
        paths
            .iter()
            .map(|path| format!("{}{path}", self.url))
            .collect()
    }
    pub fn count(&self, path: &str) -> usize {
        self.hits
            .lock()
            .unwrap()
            .iter()
            .filter(|(value, _)| value == path)
            .count()
    }
    pub fn engine(&self) -> tkless_core::TklessEngine {
        tkless_core::TklessEngine::with_options(tkless_core::FetchOptions {
            allow_private_addresses: true,
            timeout_secs: 10,
            ..Default::default()
        })
        .unwrap()
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        self.release();
        self.task.abort();
    }
}

pub struct TempDir(pub std::path::PathBuf);
impl Default for TempDir {
    fn default() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "tkless-batch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl TempDir {
    pub fn file(&self, name: &str) -> std::path::PathBuf {
        self.0.join(name)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        if let (Ok(root), Ok(target)) = (std::env::temp_dir().canonicalize(), self.0.canonicalize())
        {
            if target.parent() == Some(root.as_path())
                && target
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("tkless-batch-"))
            {
                let _ = std::fs::remove_dir_all(target);
            }
        }
    }
}
