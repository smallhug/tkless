use super::*;
use crate::{fetcher::Fetcher, FetchOptions};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn settings() -> Settings {
    Settings {
        connect: Duration::from_millis(300),
        total: Duration::from_secs(1),
        success_ttl: Duration::from_secs(1),
        failure_ttl: Duration::from_millis(30),
    }
}

fn reachable() -> Report {
    Report {
        status: Status::Reachable,
        connect_ms: Some(2.0),
        response_headers_ms: Some(3.0),
        connection_reused: Some(false),
        http_status: Some(200),
        retry_after_secs: None,
        reason: None,
    }
}

#[tokio::test]
async fn concurrent_checks_share_a_probe_and_failures_expire() {
    let cache = Cache::default();
    let calls = AtomicUsize::new(0);
    let probe = || async {
        calls.fetch_add(1, Ordering::SeqCst);
        sleep(Duration::from_millis(20)).await;
        reachable()
    };
    let (first, second) = tokio::join!(
        cache.check("one".into(), settings(), probe),
        cache.check("one".into(), settings(), probe)
    );
    assert!(!first.unwrap().1);
    assert!(second.unwrap().1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let mut failure = reachable();
    failure.status = Status::Unreachable;
    cache
        .check("two".into(), settings(), || async { failure })
        .await
        .unwrap();
    assert!(
        cache
            .check("two".into(), settings(), || async {
                panic!("Must use cached failure")
            })
            .await
            .unwrap()
            .1
    );
    sleep(Duration::from_millis(45)).await;
    assert!(
        !cache
            .check("two".into(), settings(), || async { reachable() })
            .await
            .unwrap()
            .1
    );
}

async fn server(
    status: u16,
    extra: &str,
    delay: Duration,
) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(vec![]));
    let shared = requests.clone();
    let extra = extra.to_string();
    let task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let shared = shared.clone();
            let extra = extra.clone();
            tokio::spawn(async move {
                loop {
                    let mut bytes = [0; 8192];
                    let Ok(n) = stream.read(&mut bytes).await else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }
                    shared
                        .lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&bytes[..n]).into_owned());
                    sleep(delay).await;
                    if stream
                        .write_all(
                            format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\n{extra}\r\n")
                                .as_bytes(),
                        )
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }
    });
    (origin, requests, task)
}

#[tokio::test]
async fn http_measures_connection_separately_from_response_and_reuses_the_pool() {
    let (origin, requests, task) = server(405, "", Duration::from_millis(100)).await;
    let fetcher = Fetcher::new(FetchOptions {
        headers: vec![
            ("Cookie".into(), "secret-cookie".into()),
            ("Authorization".into(), "secret-auth".into()),
        ],
        ..Default::default()
    })
    .unwrap();
    let first = http_probe(fetcher.client(), &origin, true, settings()).await;
    assert_eq!(first.status, Status::Reachable);
    assert_eq!(first.http_status, Some(405));
    let connect = first
        .connect_ms
        .expect("The connector must inherit the preflight observer");
    assert!(first.response_headers_ms.unwrap() >= connect + 60.0);
    assert_eq!(first.connection_reused, Some(false));
    let second = http_probe(fetcher.client(), &origin, true, settings()).await;
    assert_eq!(second.connection_reused, Some(true));
    assert_eq!(
        second.connect_ms, None,
        "A reused connection has no new handshake sample"
    );
    for request in requests.lock().unwrap().iter() {
        assert!(request.starts_with("HEAD / HTTP/1.1"));
        assert!(!request.contains("secret-cookie") && !request.contains("secret-auth"));
    }
    task.abort();
}

#[tokio::test]
async fn head_policy_errors_do_not_mean_offline_and_rate_limits_are_cached() {
    let (origin, requests, task) = server(429, "Retry-After: 3\r\n", Duration::ZERO).await;
    let fetcher = Fetcher::new(FetchOptions::default()).unwrap();
    for _ in 0..2 {
        let error = http(fetcher.client(), &origin, Some(fetcher.search_preflight()))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("rate limited"));
    }
    assert_eq!(requests.lock().unwrap().len(), 1);
    task.abort();
    let (origin, _, task) = server(403, "", Duration::ZERO).await;
    assert_eq!(
        http_probe(fetcher.client(), &origin, true, settings())
            .await
            .status,
        Status::Reachable
    );
    task.abort();
}

#[tokio::test]
async fn http_uses_the_configured_proxy_and_separates_client_caches() {
    let (proxy, requests, task) = server(200, "", Duration::ZERO).await;
    let fetcher = Fetcher::new(FetchOptions {
        proxy: Some(proxy),
        ..Default::default()
    })
    .unwrap();
    http(
        fetcher.client(),
        "http://unresolvable-engine.invalid",
        Some(fetcher.search_preflight()),
    )
    .await
    .unwrap();
    assert!(requests.lock().unwrap()[0]
        .starts_with("HEAD http://unresolvable-engine.invalid/ HTTP/1.1"));
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bad_proxy = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let isolated = Fetcher::new(FetchOptions {
        proxy: Some(bad_proxy),
        ..Default::default()
    })
    .unwrap();
    assert!(http(
        isolated.client(),
        "http://unresolvable-engine.invalid",
        Some(isolated.search_preflight())
    )
    .await
    .is_err());
    task.abort();
}

#[test]
fn retry_after_accepts_seconds_and_http_dates() {
    assert_eq!(retry_after(Some("12")), Some(12));
    assert_eq!(retry_after(Some("not-a-date")), None);
    assert_eq!(retry_after(Some("Wed, 21 Oct 2015 07:28:00 GMT")), Some(1));
}

#[tokio::test]
async fn completed_connection_with_slow_headers_is_not_offline() {
    let (origin, _, task) = server(200, "", Duration::from_millis(500)).await;
    let fetcher = Fetcher::new(FetchOptions::default()).unwrap();
    let mut config = settings();
    config.total = Duration::from_millis(100);
    config.connect = Duration::from_millis(50);
    let report = http_probe(fetcher.client(), &origin, true, config).await;
    assert_eq!(report.status, Status::Reachable);
    assert!(report.connect_ms.is_some());
    assert_eq!(report.response_headers_ms, None);
    task.abort();
}

#[tokio::test]
async fn search_rate_limit_replaces_a_previously_successful_check() {
    let (origin, requests, task) = server(200, "", Duration::ZERO).await;
    let fetcher = Fetcher::new(FetchOptions::default()).unwrap();
    http(fetcher.client(), &origin, Some(fetcher.search_preflight()))
        .await
        .unwrap();
    fetcher
        .search_preflight()
        .observe_error(
            &origin,
            &TklessError::SearchRateLimited {
                engine: origin.clone(),
                retry_after_secs: 3,
            },
        )
        .await;
    assert!(matches!(
        http(fetcher.client(), &origin, Some(fetcher.search_preflight())).await,
        Err(TklessError::SearchRateLimited { .. })
    ));
    assert_eq!(requests.lock().unwrap().len(), 1);
    task.abort();
}

#[tokio::test]
async fn cancelled_probe_does_not_leave_a_locked_or_poisoned_cache_entry() {
    let cache = Cache::default();
    let pending_cache = cache.clone();
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    let pending = tokio::spawn(async move {
        pending_cache
            .check("cancelled".into(), settings(), || async {
                signal.notify_one();
                std::future::pending::<Report>().await
            })
            .await
    });
    started.notified().await;
    pending.abort();
    let _ = pending.await;
    assert!(
        !timeout(
            Duration::from_secs(1),
            cache.check("cancelled".into(), settings(), || async { reachable() })
        )
        .await
        .unwrap()
        .unwrap()
        .1
    );
}
