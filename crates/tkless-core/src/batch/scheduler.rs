use crate::TklessError;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    time::Instant,
};

struct Host {
    permits: Arc<Semaphore>,
    next: Instant,
}
struct State {
    hosts: HashMap<String, Host>,
    next: Instant,
}

pub(super) struct Scheduler {
    state: Mutex<State>,
    per_host: usize,
    delay: Duration,
    host_delay: Duration,
}

impl Scheduler {
    pub(super) fn new(options: &super::BatchOptions) -> Self {
        Self {
            state: Mutex::new(State {
                hosts: HashMap::new(),
                next: Instant::now(),
            }),
            per_host: options.per_host_concurrency,
            delay: Duration::from_millis(options.delay_ms),
            host_delay: Duration::from_millis(options.per_host_delay_ms),
        }
    }

    pub(super) async fn acquire(&self, url: &str) -> Result<OwnedSemaphorePermit, TklessError> {
        let host = url::Url::parse(url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_else(|| "invalid-url".into());
        let permits = {
            let mut state = self
                .state
                .lock()
                .map_err(|e| TklessError::InvalidArgument(e.to_string()))?;
            state
                .hosts
                .entry(host.clone())
                .or_insert_with(|| Host {
                    permits: Arc::new(Semaphore::new(self.per_host)),
                    next: Instant::now(),
                })
                .permits
                .clone()
        };
        let permit = permits
            .acquire_owned()
            .await
            .map_err(|e| TklessError::InvalidArgument(e.to_string()))?;
        loop {
            let wake = {
                let mut state = self
                    .state
                    .lock()
                    .map_err(|e| TklessError::InvalidArgument(e.to_string()))?;
                let now = Instant::now();
                let ready = state.next.max(state.hosts[&host].next);
                if ready <= now {
                    state.next = now + self.delay;
                    state
                        .hosts
                        .get_mut(&host)
                        .expect("host was registered")
                        .next = now + self.host_delay;
                    return Ok(permit);
                }
                ready
            };
            tokio::time::sleep_until(wake).await;
        }
    }
}
