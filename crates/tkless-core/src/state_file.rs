//! Small durable state primitives shared by collection sync and crawl checkpoints.
use crate::TklessError;
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) const MAX_STATE_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) struct StateLock(fs::File);
impl StateLock {
    pub fn acquire(path: &Path) -> Result<Self, TklessError> {
        let path = path.with_extension("lock");
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let mut open = fs::OpenOptions::new();
        open.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open.mode(0o600);
        }
        let mut file = open.open(&path)?;
        file.try_lock().map_err(|e| match e {
            fs::TryLockError::WouldBlock => TklessError::StateBusy(path.display().to_string()),
            fs::TryLockError::Error(error) => TklessError::Io(error),
        })?;
        file.set_len(0)?;
        writeln!(file, "{}", std::process::id())?;
        Ok(Self(file))
    }
}
impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

pub(crate) fn read<T: DeserializeOwned>(path: &Path) -> Result<T, TklessError> {
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > MAX_STATE_BYTES {
        return Err(TklessError::StateBudgetExceeded(
            "State file exceeds 64 MiB".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_STATE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(TklessError::StateBudgetExceeded(
            "State file exceeds 64 MiB".into(),
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| TklessError::StateCorrupt(format!("{}: {e}", path.display())))
}

pub(crate) fn write<T: Serialize>(path: &Path, value: &T) -> Result<(), TklessError> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(TklessError::StateBudgetExceeded(
            "State file exceeds 64 MiB".into(),
        ));
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    let _ = fs::remove_file(temp);
    result.map_err(Into::into)
}

/// Blocking work owns its state and locks, including when its async caller is cancelled.
pub(crate) async fn blocking<T, F>(work: F) -> Result<T, TklessError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, TklessError> + Send + 'static,
{
    tokio::task::spawn_blocking(work).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("tkless-state-{name}-{}.json", std::process::id()))
    }

    #[test]
    fn corrupt_state_and_lock_contention_have_distinct_errors() {
        let path = temp_path("errors");
        fs::write(&path, b"{broken").unwrap();
        let error = read::<serde_json::Value>(&path).unwrap_err();
        assert_eq!(error.details().code, "state_corrupt");
        assert!(!error.details().retryable);
        assert_ne!(error.exit_code(), 2);
        let lock = StateLock::acquire(&path).unwrap();
        let error = match StateLock::acquire(&path) {
            Err(error) => error,
            Ok(_) => panic!("Concurrent lock was accepted"),
        };
        assert_eq!(error.details().code, "state_busy");
        assert!(error.details().retryable);
        drop(lock);
        fs::remove_file(&path).unwrap();
        fs::remove_file(path.with_extension("lock")).unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_caller_does_not_release_running_storage_lock() {
        let path = temp_path("cancellation");
        let worker_path = path.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(blocking(move || {
            let lock = StateLock::acquire(&worker_path)?;
            let _ = started_tx.send(());
            let _ = release_rx.recv();
            write(&worker_path, &serde_json::json!({"committed": true}))?;
            drop(lock);
            let _ = finished_tx.send(());
            Ok(())
        }));
        tokio::time::timeout(std::time::Duration::from_secs(5), started_rx)
            .await
            .unwrap()
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(matches!(
            StateLock::acquire(&path),
            Err(TklessError::StateBusy(_))
        ));
        release_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), finished_rx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(read::<serde_json::Value>(&path).unwrap()["committed"], true);
        drop(StateLock::acquire(&path).unwrap());
        fs::remove_file(&path).unwrap();
        fs::remove_file(path.with_extension("lock")).unwrap();
    }
}
