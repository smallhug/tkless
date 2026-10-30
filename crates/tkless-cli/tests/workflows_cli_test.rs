use std::{
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

#[test]
fn collection_sync_and_crawl_commands_roundtrip() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let origin = format!("http://127.0.0.1:{port}");
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let server = std::thread::spawn(move || {
        while !stopped.load(Ordering::SeqCst) {
            if let Ok((mut stream, _)) = listener.accept() {
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = [0; 8192];
                let size = stream.read(&mut bytes).unwrap_or(0);
                let request = String::from_utf8_lossy(&bytes[..size]);
                let body = if request.starts_with("GET / ") {
                    "<div class=item><a href='/detail'>Article</a></div>"
                } else {
                    "<h1>Detail</h1><p>Body</p>"
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    });
    struct Cleanup {
        stop: Arc<AtomicBool>,
        server: Option<std::thread::JoinHandle<()>>,
        dir: std::path::PathBuf,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            let _ = self.server.take().unwrap().join();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    let dir = std::env::temp_dir().join(format!(
        "tkless-workflows-cli-{}-{port}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = Cleanup {
        stop,
        server: Some(server),
        dir: dir.clone(),
    };
    let config = dir.join("collect.json");
    std::fs::write(&config, serde_json::json!({"url":origin,"schema":{"items_selector":".item","fields":{"name":"a","url":"a@href"}},"id_pointer":"/url"}).to_string()).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_tkless"))
            .args(["--allow-private-addresses"])
            .args(args)
            .output()
            .unwrap()
    };
    let collected = run(&["collect", "--config", config.to_str().unwrap()]);
    assert!(
        collected.status.success(),
        "{}",
        String::from_utf8_lossy(&collected.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&collected.stdout).unwrap()["complete"],
        true
    );
    let state = dir.join("state");
    for expected in ["added", "unchanged"] {
        let output = run(&[
            "sync",
            "--config",
            config.to_str().unwrap(),
            "--state",
            state.to_str().unwrap(),
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["events"][0]
                ["kind"],
            expected
        );
    }
    let checkpoint = dir.join("crawl.json");
    let first = run(&[
        "crawl",
        &origin,
        "--checkpoint",
        checkpoint.to_str().unwrap(),
        "--max-pages",
        "1",
        "--allow-partial",
        "-f",
        "json",
    ]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&first.stdout).unwrap()["stop_reason"],
        "page_limit"
    );
    let second = run(&[
        "crawl",
        &origin,
        "--checkpoint",
        checkpoint.to_str().unwrap(),
        "--resume",
        "--max-pages",
        "3",
        "-f",
        "json",
    ]);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let before = std::fs::read(&checkpoint).unwrap();
    let alias = run(&[
        "crawl",
        &origin,
        "--checkpoint",
        checkpoint.to_str().unwrap(),
        "--resume",
        "-o",
        checkpoint.to_str().unwrap(),
    ]);
    assert!(!alias.status.success());
    assert_eq!(std::fs::read(checkpoint).unwrap(), before);
}
