//! Single-round scheduled collection with change events and bounded notifications.
//!
//! One round calls the existing snapshot sync for every job. Scheduling is intentionally
//! external (cron / Task Scheduler / systemd timer); this module never daemonizes.
use crate::limits::*;
use crate::sync::{sync_collection, SyncEvent};
use crate::workflow::SyncEventKind;
use crate::{collection::CollectionConfig, ExtractionOptions, TklessEngine, TklessError};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use url::Url;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WatchConfig {
    pub round_timeout_ms: u64,
    pub jobs: Vec<WatchJob>,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            round_timeout_ms: WATCH_DEFAULT_ROUND_TIMEOUT_MS,
            jobs: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchJob {
    /// Stable job identifier; also the state subdirectory name.
    pub id: String,
    pub target: CollectionConfig,
    #[serde(default = "default_state_mb")]
    pub max_state_mb: u64,
    #[serde(default)]
    pub notify: Vec<NotifySink>,
    /// Also emit `unchanged` events. Default: false.
    #[serde(default)]
    pub send_unchanged: bool,
}

fn default_state_mb() -> u64 {
    SYNC_DEFAULT_STATE_MB
}

fn default_http_timeout() -> u64 {
    WATCH_DEFAULT_HTTP_TIMEOUT_MS
}

fn default_http_attempts() -> u32 {
    1
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NotifySink {
    /// Append one JSON line per notification to a local file.
    File { path: PathBuf },
    /// POST one JSON document per notification. Redirects are never followed.
    Http {
        url: String,
        #[serde(default = "default_http_timeout")]
        timeout_ms: u64,
        #[serde(default = "default_http_attempts")]
        max_attempts: u32,
        /// Explicitly permit private/loopback webhook targets. Default: false.
        #[serde(default)]
        allow_private_addresses: bool,
        /// Name of the environment variable holding the HMAC-SHA256 secret.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret_env: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WatchReport {
    pub started_at: u64,
    pub complete: bool,
    pub jobs: Vec<JobReport>,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobReport {
    pub id: String,
    pub generation: Option<u64>,
    pub complete: bool,
    pub stop_reason: Option<crate::workflow::StopReason>,
    pub events: Vec<SyncEvent>,
    pub notified: usize,
    pub notify_errors: Vec<String>,
    pub error: Option<String>,
}

impl WatchConfig {
    pub fn validate(&self) -> Result<(), TklessError> {
        if self.jobs.is_empty() || self.jobs.len() > WATCH_MAX_JOBS {
            return Err(TklessError::InvalidArgument(format!(
                "watch config requires 1..{WATCH_MAX_JOBS} jobs"
            )));
        }
        if !(WATCH_MIN_ROUND_TIMEOUT_MS..=WATCH_MAX_ROUND_TIMEOUT_MS)
            .contains(&self.round_timeout_ms)
        {
            return Err(TklessError::InvalidArgument(format!(
                "watch round_timeout_ms must be {WATCH_MIN_ROUND_TIMEOUT_MS}..{WATCH_MAX_ROUND_TIMEOUT_MS}"
            )));
        }
        let mut ids = HashSet::new();
        for job in &self.jobs {
            if !valid_job_id(&job.id) {
                return Err(TklessError::InvalidArgument(format!(
                    "watch job id '{}' must match [a-z0-9][a-z0-9_-]{{0,63}}",
                    job.id
                )));
            }
            if !ids.insert(job.id.as_str()) {
                return Err(TklessError::InvalidArgument(format!(
                    "duplicate watch job id: {}",
                    job.id
                )));
            }
            job.target.validate()?;
            if job.target.id_pointer.is_none() && job.target.details.is_none() {
                return Err(TklessError::InvalidArgument(format!(
                    "watch job '{}' requires target.id_pointer or target.details",
                    job.id
                )));
            }
            crate::limits::sync_state_bytes(job.max_state_mb)?;
            if job.notify.len() > WATCH_MAX_SINKS_PER_JOB {
                return Err(TklessError::InvalidArgument(format!(
                    "watch job '{}' supports at most {WATCH_MAX_SINKS_PER_JOB} notify sinks",
                    job.id
                )));
            }
            for sink in &job.notify {
                sink.validate()?;
            }
        }
        Ok(())
    }
}

impl NotifySink {
    fn validate(&self) -> Result<(), TklessError> {
        match self {
            Self::File { path } => {
                if path.as_os_str().is_empty() {
                    return Err(TklessError::InvalidArgument(
                        "notify file sink requires a path".into(),
                    ));
                }
            }
            Self::Http {
                url,
                timeout_ms,
                max_attempts,
                secret_env,
                ..
            } => {
                let parsed = Url::parse(url)?;
                if !matches!(parsed.scheme(), "http" | "https")
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                {
                    return Err(TklessError::InvalidArgument(
                        "notify URL must be HTTP(S) without embedded credentials".into(),
                    ));
                }
                if !(1..=WATCH_MAX_HTTP_TIMEOUT_MS).contains(timeout_ms) {
                    return Err(TklessError::InvalidArgument(format!(
                        "notify timeout_ms must be 1..{WATCH_MAX_HTTP_TIMEOUT_MS}"
                    )));
                }
                if !(1..=WATCH_MAX_HTTP_ATTEMPTS).contains(max_attempts) {
                    return Err(TklessError::InvalidArgument(format!(
                        "notify max_attempts must be 1..{WATCH_MAX_HTTP_ATTEMPTS}"
                    )));
                }
                if let Some(name) = secret_env {
                    if name.is_empty() || name.len() > 128 || name.contains('=') {
                        return Err(TklessError::InvalidArgument(
                            "notify secret_env must be an environment variable name".into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

fn valid_job_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit())
        && id.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Runs exactly one watch round. Each job keeps its own state directory so an
/// incomplete job never blocks or deletes another job's records.
pub async fn run_once(
    engine: &TklessEngine,
    config: &WatchConfig,
    options: &ExtractionOptions,
    state_root: &Path,
) -> Result<WatchReport, TklessError> {
    config.validate()?;
    let started_at = now_secs();
    let deadline = Instant::now() + Duration::from_millis(config.round_timeout_ms);
    let mut jobs = Vec::with_capacity(config.jobs.len());
    let mut errors = Vec::new();

    for job in &config.jobs {
        let directory = state_root.join(&job.id);
        let budget = crate::limits::sync_state_bytes(job.max_state_mb)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let outcome = if remaining.is_zero() {
            Err(TklessError::ConnectionTimeout(
                "Watch round time budget was exhausted before this job started".into(),
            ))
        } else {
            match tokio::time::timeout(
                remaining,
                sync_collection(engine, &job.target, options, &directory, budget),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Err(TklessError::ConnectionTimeout(
                    "Watch job exceeded the round time budget".into(),
                )),
            }
        };

        let mut report = match outcome {
            Ok(sync) => {
                let events = filter_events(&sync.events, job.send_unchanged);
                let mut job_report = JobReport {
                    id: job.id.clone(),
                    generation: Some(sync.generation),
                    complete: sync.complete,
                    stop_reason: Some(sync.stop_reason),
                    events,
                    notified: 0,
                    notify_errors: Vec::new(),
                    error: None,
                };
                if !job_report.events.is_empty() {
                    match notification_payload(&job_report, sync.captured_at) {
                        Ok(payload) => {
                            for sink in &job.notify {
                                match deliver(sink, &payload).await {
                                    Ok(()) => job_report.notified += 1,
                                    Err(error) => job_report
                                        .notify_errors
                                        .push(format!("{}: {error}", sink_description(sink))),
                                }
                            }
                        }
                        Err(error) => job_report.notify_errors.push(format!("payload: {error}")),
                    }
                }
                job_report
            }
            Err(error) => {
                errors.push(format!("{}: {error}", job.id));
                JobReport {
                    id: job.id.clone(),
                    generation: None,
                    complete: false,
                    stop_reason: None,
                    events: Vec::new(),
                    notified: 0,
                    notify_errors: Vec::new(),
                    error: Some(error.to_string()),
                }
            }
        };
        if !report.complete && report.error.is_none() {
            errors.push(format!("{}: collection incomplete", job.id));
        }
        report.events.shrink_to_fit();
        jobs.push(report);
    }

    let complete = jobs.iter().all(|job| job.complete && job.error.is_none());
    Ok(WatchReport {
        started_at,
        complete,
        jobs,
        errors,
    })
}

fn filter_events(events: &[SyncEvent], send_unchanged: bool) -> Vec<SyncEvent> {
    events
        .iter()
        .filter(|event| send_unchanged || event.kind != SyncEventKind::Unchanged)
        .cloned()
        .collect()
}

fn notification_payload(report: &JobReport, captured_at: u64) -> Result<String, TklessError> {
    let payload = serde_json::to_string(&serde_json::json!({
        "job": report.id,
        "generation": report.generation,
        "captured_at": captured_at,
        "complete": report.complete,
        "stop_reason": report.stop_reason,
        "events": report.events,
    }))?;
    if payload.len() > WATCH_MAX_PAYLOAD_BYTES {
        return Err(TklessError::InvalidArgument(format!(
            "notification payload exceeds {WATCH_MAX_PAYLOAD_BYTES} bytes"
        )));
    }
    Ok(payload)
}

fn sink_description(sink: &NotifySink) -> &'static str {
    match sink {
        NotifySink::File { .. } => "file sink",
        NotifySink::Http { .. } => "http sink",
    }
}

async fn deliver(sink: &NotifySink, payload: &str) -> Result<(), TklessError> {
    match sink {
        NotifySink::File { path } => {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            file.write_all(payload.as_bytes())?;
            file.write_all(b"\n")?;
            file.flush()?;
            Ok(())
        }
        NotifySink::Http {
            url,
            timeout_ms,
            max_attempts,
            allow_private_addresses,
            secret_env,
        } => {
            let mut headers = Vec::new();
            if let Some(name) = secret_env {
                let secret = std::env::var(name).map_err(|_| {
                    TklessError::InvalidArgument(format!(
                        "notify secret_env '{name}' is not set; refusing to send an unsigned notification"
                    ))
                })?;
                headers.push((
                    "X-Tkless-Signature".to_string(),
                    format!(
                        "sha256={}",
                        hmac_sha256_hex(secret.as_bytes(), payload.as_bytes())
                    ),
                ));
            }
            let seconds = timeout_ms.div_ceil(1000).max(1);
            let fetcher = crate::fetcher::Fetcher::new(crate::FetchOptions {
                allow_private_addresses: *allow_private_addresses,
                timeout_secs: seconds,
                connect_timeout_secs: seconds,
                retries: max_attempts.saturating_sub(1),
                max_bytes: 64 * 1024,
                ..Default::default()
            })?;
            let response = fetcher
                .send_api_request(
                    url,
                    "POST",
                    Some("application/json"),
                    &headers,
                    Some(payload),
                )
                .await?;
            if !(200..300).contains(&response.status) {
                return Err(TklessError::HttpStatusError {
                    status: response.status,
                    url: url.clone(),
                });
            }
            Ok(())
        }
    }
}

/// HMAC-SHA256 used for optional webhook signatures (RFC 2104).
fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    const BLOCK: usize = 64;
    let mut key_block = [0u8; BLOCK];
    if key.len() > BLOCK {
        let digest = Sha256::digest(key);
        key_block[..digest.len()].copy_from_slice(&digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    let mut outer = Sha256::new();
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for index in 0..BLOCK {
        ipad[index] ^= key_block[index];
        opad[index] ^= key_block[index];
    }
    inner.update(ipad);
    inner.update(message);
    let inner = inner.finalize();
    outer.update(opad);
    outer.update(inner);
    format!("{:x}", outer.finalize())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection::Pagination;

    fn job(id: &str) -> WatchJob {
        WatchJob {
            id: id.into(),
            target: CollectionConfig {
                url: "https://example.com/list".into(),
                id_pointer: Some("/id".into()),
                pagination: Pagination::Cursor {
                    parameter: "cursor".into(),
                    items_pointer: "/items".into(),
                    next_pointer: "/next".into(),
                },
                ..Default::default()
            },
            max_state_mb: SYNC_DEFAULT_STATE_MB,
            notify: Vec::new(),
            send_unchanged: false,
        }
    }

    #[test]
    fn config_rejects_unsafe_or_duplicate_jobs() {
        let mut config = WatchConfig {
            jobs: vec![job("news")],
            ..Default::default()
        };
        assert!(config.validate().is_ok());

        config.jobs.push(job("news"));
        assert!(config.validate().is_err());

        let config = WatchConfig {
            jobs: vec![job("../escape")],
            ..Default::default()
        };
        assert!(config.validate().is_err());

        let mut config = WatchConfig {
            jobs: vec![job("news")],
            ..Default::default()
        };
        config.round_timeout_ms = 10;
        assert!(config.validate().is_err());

        let mut invalid = job("news");
        invalid.target.id_pointer = None;
        invalid.target.details = None;
        let config = WatchConfig {
            jobs: vec![invalid],
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn notify_sinks_are_bounded() {
        let file = NotifySink::File {
            path: PathBuf::new(),
        };
        assert!(file.validate().is_err());

        let private_ok = NotifySink::Http {
            url: "http://127.0.0.1:9000/hook".into(),
            timeout_ms: WATCH_DEFAULT_HTTP_TIMEOUT_MS,
            max_attempts: 1,
            allow_private_addresses: true,
            secret_env: None,
        };
        assert!(private_ok.validate().is_ok());

        let bad_secret = NotifySink::Http {
            url: "https://example.com/hook".into(),
            timeout_ms: WATCH_DEFAULT_HTTP_TIMEOUT_MS,
            max_attempts: 1,
            allow_private_addresses: false,
            secret_env: Some("BAD=NAME".into()),
        };
        assert!(bad_secret.validate().is_err());

        let bad_timeout = NotifySink::Http {
            url: "https://example.com/hook".into(),
            timeout_ms: 0,
            max_attempts: 1,
            allow_private_addresses: false,
            secret_env: None,
        };
        assert!(bad_timeout.validate().is_err());
    }

    #[test]
    fn hmac_matches_rfc4231_case_two() {
        assert_eq!(
            hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn unchanged_events_are_filtered_by_default() {
        let event = |kind| SyncEvent {
            kind,
            id: "1".into(),
            previous: None,
            current: None,
            error: None,
        };
        let events = vec![event(SyncEventKind::Added), event(SyncEventKind::Unchanged)];
        assert_eq!(filter_events(&events, false).len(), 1);
        assert_eq!(filter_events(&events, true).len(), 2);
    }
}
