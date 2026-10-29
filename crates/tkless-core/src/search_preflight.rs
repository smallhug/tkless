//! Search transport preflight. Connectivity, HTTP policy and human verification are separate states.
use crate::TklessError;
use serde::Serialize;
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, LazyLock, Mutex,
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::{sleep, timeout, Instant};
use tower::{Layer, Service};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Settings {
    pub connect: Duration,
    pub total: Duration,
    pub success_ttl: Duration,
    pub failure_ttl: Duration,
}

impl Settings {
    pub fn load() -> Result<Self, TklessError> {
        fn number(name: &str, default: u64, max: u64) -> Result<u64, TklessError> {
            let value = match std::env::var(name) {
                Ok(raw) => raw.parse::<u64>().ok().filter(|n| (1..=max).contains(n)),
                Err(std::env::VarError::NotPresent) => Some(default),
                Err(_) => None,
            };
            value.ok_or_else(|| {
                TklessError::InvalidArgument(format!(
                    "{name} must be an integer between 1 and {max}"
                ))
            })
        }
        let settings = Self {
            connect: Duration::from_millis(number(
                "TKLESS_SEARCH_CONNECT_TIMEOUT_MS",
                3000,
                60_000,
            )?),
            total: Duration::from_millis(number(
                "TKLESS_SEARCH_PREFLIGHT_TIMEOUT_MS",
                5000,
                120_000,
            )?),
            success_ttl: Duration::from_secs(number(
                "TKLESS_SEARCH_PREFLIGHT_SUCCESS_TTL_SECS",
                60,
                3600,
            )?),
            failure_ttl: Duration::from_secs(number(
                "TKLESS_SEARCH_PREFLIGHT_FAILURE_TTL_SECS",
                5,
                300,
            )?),
        };
        if settings.connect > settings.total {
            return Err(TklessError::InvalidArgument(
                "Search connection timeout cannot exceed preflight timeout".into(),
            ));
        }
        Ok(settings)
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Reachable,
    Unreachable,
    Unknown,
    RateLimited,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Report {
    pub status: Status,
    pub connect_ms: Option<f64>,
    pub response_headers_ms: Option<f64>,
    pub connection_reused: Option<bool>,
    pub http_status: Option<u16>,
    pub retry_after_secs: Option<u64>,
    pub reason: Option<String>,
}

impl Report {
    pub fn unknown(reason: &str) -> Self {
        Self {
            status: Status::Unknown,
            connect_ms: None,
            response_headers_ms: None,
            connection_reused: None,
            http_status: None,
            retry_after_secs: None,
            reason: Some(reason.into()),
        }
    }

    pub fn allow_search(
        &self,
        origin: &str,
        transport: &str,
        cached: bool,
    ) -> Result<(), TklessError> {
        tracing::info!(origin, transport, cached, status = ?self.status, connect_ms = ?self.connect_ms,
            response_headers_ms = ?self.response_headers_ms, connection_reused = ?self.connection_reused,
            http_status = ?self.http_status, "Search connection preflight");
        if self.connect_ms.is_some_and(|ms| ms > 1000.0) {
            tracing::warn!(
                origin,
                transport,
                "Search connection is slow; keeping this engine enabled"
            );
        }
        match self.status {
            Status::Unreachable => Err(TklessError::SearchConnectionError(format!(
                "Connection preflight failed for {origin} ({transport}): {}",
                self.reason.as_deref().unwrap_or("connection failed")
            ))),
            Status::RateLimited => Err(TklessError::SearchRateLimited {
                engine: origin.into(),
                retry_after_secs: self.retry_after_secs.unwrap_or(5),
            }),
            _ => Ok(()),
        }
    }
}

#[derive(Clone)]
struct Cached {
    report: Report,
    expires: Instant,
}
type Slot = Arc<tokio::sync::Mutex<Option<Cached>>>;

#[derive(Clone, Default)]
pub(crate) struct Cache(Arc<Mutex<HashMap<String, Slot>>>);

impl Cache {
    async fn store(&self, key: String, report: Report, ttl: Duration) {
        let slot = self
            .0
            .lock()
            .ok()
            .map(|mut entries| entries.entry(key).or_default().clone());
        if let Some(slot) = slot {
            *slot.lock().await = Some(Cached {
                report,
                expires: Instant::now() + ttl,
            });
        }
    }
    async fn check<F, Fut>(
        &self,
        key: String,
        settings: Settings,
        probe: F,
    ) -> Result<(Report, bool), TklessError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Report>,
    {
        let slot = {
            let mut entries = self.0.lock().map_err(|_| {
                TklessError::SearchEngineError("Preflight cache lock was poisoned".into())
            })?;
            // There are only two normal search origins. Bound library callers' custom contexts.
            if entries.len() >= 256 && !entries.contains_key(&key) {
                entries.retain(|_, slot| Arc::strong_count(slot) > 1);
            }
            entries.entry(key).or_default().clone()
        };
        let mut stored = slot.lock().await;
        if let Some(cached) = stored
            .as_ref()
            .filter(|value| value.expires > Instant::now())
        {
            let mut report = cached.report.clone();
            if report.status == Status::RateLimited {
                report.retry_after_secs = Some(
                    cached
                        .expires
                        .saturating_duration_since(Instant::now())
                        .as_secs()
                        .saturating_add(1),
                );
            }
            return Ok((report, true));
        }
        let report = probe().await;
        let ttl = match report.status {
            Status::Reachable => settings.success_ttl,
            Status::RateLimited => Duration::from_secs(
                report
                    .retry_after_secs
                    .unwrap_or(settings.failure_ttl.as_secs())
                    .max(1),
            ),
            _ => settings.failure_ttl,
        };
        *stored = Some(Cached {
            report: report.clone(),
            expires: Instant::now() + ttl,
        });
        Ok((report, false))
    }
}

fn cache_key(origin: &str, settings: Settings) -> String {
    format!(
        "{origin}|{}|{}|{}|{}",
        settings.connect.as_millis(),
        settings.total.as_millis(),
        settings.success_ttl.as_millis(),
        settings.failure_ttl.as_millis()
    )
}

#[derive(Clone, Default)]
pub(crate) struct HttpState {
    cache: Cache,
}

impl HttpState {
    pub async fn observe_error(&self, origin: &str, error: &TklessError) {
        let Ok(settings) = Settings::load() else {
            return;
        };
        let mut report = Report::unknown("Search transport failed after preflight");
        let ttl = match error {
            TklessError::SearchRateLimited {
                retry_after_secs, ..
            } => {
                report.status = Status::RateLimited;
                report.retry_after_secs = Some(*retry_after_secs);
                Duration::from_secs(*retry_after_secs)
            }
            TklessError::Network(error) if error.is_connect() => {
                report.status = Status::Unreachable;
                report.reason = Some("Search DNS, proxy, TCP or TLS connection failed".into());
                settings.failure_ttl
            }
            TklessError::Network(_) => settings.failure_ttl,
            _ => return,
        };
        self.cache
            .store(cache_key(origin, settings), report, ttl)
            .await;
    }
}

pub(crate) fn rate_limit_error(origin: &str, headers: &reqwest::header::HeaderMap) -> TklessError {
    TklessError::SearchRateLimited {
        engine: origin.into(),
        retry_after_secs: retry_after(
            headers
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok()),
        )
        .unwrap_or(5),
    }
}

#[derive(Clone, Copy)]
struct ConnectionSample {
    elapsed: Duration,
    success: bool,
}
struct Attempt {
    budget: Duration,
    started: AtomicBool,
    samples: Mutex<Vec<ConnectionSample>>,
}
tokio::task_local! { static CURRENT_ATTEMPT: Arc<Attempt>; }

#[derive(Clone)]
pub(crate) struct ConnectionTimingLayer;
#[derive(Clone)]
pub(crate) struct TimedConnector<S>(S);
impl<S> Layer<S> for ConnectionTimingLayer {
    type Service = TimedConnector<S>;
    fn layer(&self, inner: S) -> Self::Service {
        TimedConnector(inner)
    }
}
impl<S, R> Service<R> for TimedConnector<S>
where
    S: Service<R, Error = BoxError>,
    S::Future: Send + 'static,
    S::Response: Send + 'static,
{
    type Response = S::Response;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, BoxError>> + Send>>;
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }
    fn call(&mut self, request: R) -> Self::Future {
        let observer = CURRENT_ATTEMPT.try_with(Arc::clone).ok();
        if let Some(attempt) = &observer {
            attempt.started.store(true, Ordering::Relaxed);
        }
        let future = self.0.call(request);
        Box::pin(async move {
            let started = Instant::now();
            let result = match &observer {
                Some(attempt) => timeout(attempt.budget, future).await.unwrap_or_else(|_| {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Search preflight connection timed out",
                    )
                    .into())
                }),
                None => future.await,
            };
            if let Some(attempt) = observer {
                if let Ok(mut samples) = attempt.samples.lock() {
                    samples.push(ConnectionSample {
                        elapsed: started.elapsed(),
                        success: result.is_ok(),
                    });
                }
            }
            result
        })
    }
}

pub(crate) fn retry_after(value: Option<&str>) -> Option<u64> {
    let raw = value?.trim();
    if let Ok(seconds) = raw.parse::<u64>() {
        return Some(seconds.clamp(1, 86_400));
    }
    let until = chrono::DateTime::parse_from_rfc2822(raw).ok()?;
    Some((until.timestamp() - chrono::Utc::now().timestamp()).clamp(1, 86_400) as u64)
}

async fn http_probe(
    client: &reqwest::Client,
    origin: &str,
    instrumented: bool,
    settings: Settings,
) -> Report {
    let started = Instant::now();
    let mut latest = None;
    let operation = async {
        for attempt_number in 0..2 {
            let attempt = Arc::new(Attempt {
                budget: settings.connect,
                started: AtomicBool::new(false),
                samples: Mutex::new(vec![]),
            });
            latest = Some(attempt.clone());
            // Do not attach platform cookies or query text. Override sensitive client defaults.
            let response = CURRENT_ATTEMPT
                .scope(
                    attempt.clone(),
                    client
                        .head(origin)
                        .header(reqwest::header::COOKIE, "")
                        .header(reqwest::header::AUTHORIZATION, "")
                        .header(reqwest::header::PROXY_AUTHORIZATION, "")
                        .timeout(settings.total)
                        .send(),
                )
                .await;
            let samples = attempt
                .samples
                .lock()
                .map(|values| values.clone())
                .unwrap_or_default();
            let connected = samples.iter().any(|sample| sample.success);
            let connect_ms = (!samples.is_empty()).then(|| {
                samples
                    .iter()
                    .map(|sample| sample.elapsed.as_secs_f64() * 1000.0)
                    .sum()
            });
            let reused = if instrumented {
                Some(samples.is_empty())
            } else {
                None
            };
            match response {
                Ok(response) => {
                    let limited = response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS;
                    return Report {
                        status: if limited {
                            Status::RateLimited
                        } else {
                            Status::Reachable
                        },
                        connect_ms,
                        response_headers_ms: Some(started.elapsed().as_secs_f64() * 1000.0),
                        connection_reused: reused,
                        http_status: Some(response.status().as_u16()),
                        retry_after_secs: limited.then(|| {
                            retry_after(
                                response
                                    .headers()
                                    .get(reqwest::header::RETRY_AFTER)
                                    .and_then(|value| value.to_str().ok()),
                            )
                            .unwrap_or(settings.failure_ttl.as_secs().max(1))
                        }),
                        reason: None,
                    };
                }
                Err(error)
                    if !connected
                        && (error.is_connect() || samples.iter().any(|sample| !sample.success)) =>
                {
                    if attempt_number == 0 {
                        sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                    return Report {
                        status: Status::Unreachable,
                        connect_ms,
                        response_headers_ms: None,
                        connection_reused: None,
                        http_status: None,
                        retry_after_secs: None,
                        reason: Some(
                            if error.is_timeout() {
                                "connection timed out"
                            } else {
                                "DNS, proxy, TCP or TLS connection failed"
                            }
                            .into(),
                        ),
                    };
                }
                Err(_) => {
                    return Report { status: if connected { Status::Reachable } else { Status::Unknown }, connect_ms,
                        response_headers_ms: None, connection_reused: None, http_status: None, retry_after_secs: None,
                        reason: Some("No response headers within the preflight budget; search is still allowed".into()) };
                }
            }
        }
        unreachable!()
    };
    timeout(settings.total, operation)
        .await
        .unwrap_or_else(|_| {
            let mut report = Report::unknown("Preflight budget expired; availability is unknown");
            if let Some(attempt) = latest {
                let samples = attempt
                    .samples
                    .lock()
                    .map(|values| values.clone())
                    .unwrap_or_default();
                if samples.iter().any(|sample| sample.success) {
                    report.status = Status::Reachable;
                    report.connect_ms = Some(
                        samples
                            .iter()
                            .map(|sample| sample.elapsed.as_secs_f64() * 1000.0)
                            .sum(),
                    );
                    report.connection_reused = Some(false);
                } else if attempt.started.load(Ordering::Relaxed) {
                    report.status = Status::Unreachable;
                    report.reason =
                        Some("Connection establishment exceeded the preflight budget".into());
                }
            }
            report
        })
}

pub(crate) async fn http(
    client: &reqwest::Client,
    origin: &str,
    state: Option<&HttpState>,
) -> Result<(), TklessError> {
    let settings = Settings::load()?;
    let ephemeral = HttpState::default();
    let (report, cached) = state
        .unwrap_or(&ephemeral)
        .cache
        .check(cache_key(origin, settings), settings, || {
            http_probe(client, origin, state.is_some(), settings)
        })
        .await?;
    report.allow_search(origin, "http", cached)
}

static CDP_CACHE: LazyLock<Cache> = LazyLock::new(Cache::default);

pub(crate) async fn browser<F, Fut>(
    context: &str,
    origin: &str,
    settings: Settings,
    probe: F,
) -> Result<(), TklessError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Report>,
{
    use sha2::{Digest, Sha256};
    let key = format!(
        "{:x}|{}",
        Sha256::digest(context.as_bytes()),
        cache_key(origin, settings)
    );
    let (report, cached) = CDP_CACHE.check(key, settings, probe).await?;
    report.allow_search(origin, "cdp", cached)
}

#[cfg(test)]
mod tests;
