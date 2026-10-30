//! Bounded per-host circuit breaker.
//!
//! After a configurable number of consecutive transport/server failures, requests to the
//! host are rejected locally until a cooldown expires. One half-open probe is then allowed;
//! its success resets the host, while another failure doubles the cooldown up to a cap.
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30);
pub const MAX_COOLDOWN: Duration = Duration::from_secs(300);

pub struct CircuitBreaker {
    threshold: u32,
    base_cooldown: Duration,
    max_cooldown: Duration,
    hosts: Mutex<HashMap<String, HostState>>,
}

struct HostState {
    failures: u32,
    open_until: Option<Instant>,
    probing: bool,
    cooldown: Duration,
}

impl CircuitBreaker {
    /// `threshold` of 0 disables the breaker entirely.
    pub fn new(threshold: u32, base_cooldown: Duration, max_cooldown: Duration) -> Self {
        Self {
            threshold,
            base_cooldown,
            max_cooldown,
            hosts: Mutex::new(HashMap::new()),
        }
    }

    pub fn enabled(&self) -> bool {
        self.threshold > 0
    }

    /// Rejects the request while the host is open. On expiry, exactly one probe is admitted.
    pub fn before_request(&self, host: &str) -> Result<(), u64> {
        if !self.enabled() {
            return Ok(());
        }
        let now = Instant::now();
        let mut hosts = self.lock();
        let state = hosts.entry(host.to_string()).or_insert_with(|| HostState {
            failures: 0,
            open_until: None,
            probing: false,
            cooldown: self.base_cooldown,
        });
        match state.open_until {
            Some(until) if now < until => Err(retry_after_secs(until - now)),
            Some(_) if state.probing => Err(1),
            Some(_) => {
                state.probing = true;
                Ok(())
            }
            None => Ok(()),
        }
    }

    pub fn record_success(&self, host: &str) {
        if self.enabled() {
            self.lock().remove(host);
        }
    }

    pub fn record_failure(&self, host: &str) {
        if !self.enabled() {
            return;
        }
        let now = Instant::now();
        let mut hosts = self.lock();
        let state = hosts.entry(host.to_string()).or_insert_with(|| HostState {
            failures: 0,
            open_until: None,
            probing: false,
            cooldown: self.base_cooldown,
        });
        state.failures = state.failures.saturating_add(1);
        if state.probing {
            state.probing = false;
            state.cooldown = (state.cooldown * 2).min(self.max_cooldown);
            state.open_until = Some(now + state.cooldown);
        } else if state.open_until.is_none() && state.failures >= self.threshold {
            state.open_until = Some(now + state.cooldown);
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, HostState>> {
        self.hosts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn retry_after_secs(remaining: Duration) -> u64 {
    let seconds = remaining.as_secs();
    if remaining.subsec_nanos() > 0 {
        seconds.saturating_add(1)
    } else {
        seconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_after_threshold_and_admits_one_half_open_probe() {
        let breaker =
            CircuitBreaker::new(2, Duration::from_millis(100), Duration::from_millis(400));
        assert!(breaker.before_request("example.com").is_ok());
        breaker.record_failure("example.com");
        breaker.record_failure("example.com");
        let retry_after = breaker.before_request("example.com").unwrap_err();
        assert!(retry_after >= 1);
        assert!(breaker.before_request("other.com").is_ok());

        std::thread::sleep(Duration::from_millis(120));
        assert!(breaker.before_request("example.com").is_ok());
        assert!(breaker.before_request("example.com").is_err());
        breaker.record_failure("example.com");
        assert!(breaker.before_request("example.com").is_err());

        std::thread::sleep(Duration::from_millis(220));
        assert!(breaker.before_request("example.com").is_ok());
        breaker.record_success("example.com");
        breaker.record_failure("example.com");
        breaker.record_failure("example.com");
        assert!(breaker.before_request("example.com").is_err());
    }

    #[test]
    fn disabled_breaker_never_rejects() {
        let breaker = CircuitBreaker::new(0, Duration::from_secs(1), Duration::from_secs(2));
        for _ in 0..10 {
            breaker.record_failure("example.com");
        }
        assert!(breaker.before_request("example.com").is_ok());
    }
}
