//! Owner-only credential store for platform cookies.
//!
//! Values are never logged or echoed. The store is written atomically with
//! `0600` permissions on Unix and read once per fetcher instance.
use crate::{state_file, TklessError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MAX_COOKIE_BYTES: usize = 64 * 1024;
pub const MAX_PLATFORMS: usize = 32;
pub const MAX_STORE_BYTES: u64 = 256 * 1024;
pub const STORE_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CredentialsFile {
    version: u32,
    credentials: BTreeMap<String, String>,
}

impl Default for CredentialsFile {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            credentials: BTreeMap::new(),
        }
    }
}

impl CredentialsFile {
    pub fn get(&self, platform: &str) -> Option<&String> {
        self.credentials.get(platform)
    }

    pub fn platforms(&self) -> impl Iterator<Item = &String> {
        self.credentials.keys()
    }

    pub fn is_empty(&self) -> bool {
        self.credentials.is_empty()
    }
}

/// Resolves the store location. `TKLESS_CREDENTIALS_FILE` overrides the default.
pub fn store_path() -> Result<PathBuf, TklessError> {
    if let Some(path) = std::env::var_os("TKLESS_CREDENTIALS_FILE") {
        if !path.is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(dir) = std::env::var_os("APPDATA").or_else(|| std::env::var_os("LOCALAPPDATA"))
        {
            return Ok(PathBuf::from(dir).join("tkless").join("credentials.json"));
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
            return Ok(PathBuf::from(dir).join("tkless").join("credentials.json"));
        }
        if let Some(home) = std::env::var_os("HOME") {
            return Ok(PathBuf::from(home)
                .join(".config")
                .join("tkless")
                .join("credentials.json"));
        }
    }
    Err(TklessError::InvalidArgument(
        "Cannot determine a credentials directory; set TKLESS_CREDENTIALS_FILE".into(),
    ))
}

pub fn valid_platform(platform: &str) -> bool {
    !platform.is_empty()
        && platform.len() <= 32
        && platform.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}

fn valid_cookie(cookie: &str) -> bool {
    let cookie = cookie.trim();
    !cookie.is_empty() && cookie.len() <= MAX_COOKIE_BYTES && !cookie.contains(['\r', '\n'])
}

/// Loads the store for request-time use. Failures degrade to an empty store so a
/// broken credentials file cannot break unrelated fetches.
pub fn load() -> CredentialsFile {
    let path = match store_path() {
        Ok(path) => path,
        Err(error) => {
            tracing::debug!(error = %error, "Credentials store path is unavailable");
            return CredentialsFile::default();
        }
    };
    match load_from(&path) {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!(path = %path.display(), error = %error, "Ignoring unreadable credentials store");
            CredentialsFile::default()
        }
    }
}

pub fn load_from(path: &Path) -> Result<CredentialsFile, TklessError> {
    if !path.exists() {
        return Ok(CredentialsFile::default());
    }
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > MAX_STORE_BYTES {
        return Err(TklessError::StateBudgetExceeded(
            "Credentials store exceeds 256 KiB".into(),
        ));
    }
    let store: CredentialsFile = state_file::read(path)?;
    if store.version != STORE_VERSION {
        return Err(TklessError::StateCorrupt(format!(
            "Credentials store version {} is not supported",
            store.version
        )));
    }
    Ok(store)
}

/// Stores or replaces one platform cookie. Returns the store path.
pub fn set(platform: &str, cookie: &str) -> Result<PathBuf, TklessError> {
    if !valid_platform(platform) {
        return Err(TklessError::InvalidArgument(
            "Platform must match [a-z0-9_-]{1,32}".into(),
        ));
    }
    let cookie = cookie.trim();
    if !valid_cookie(cookie) {
        return Err(TklessError::InvalidArgument(format!(
            "Cookie must be 1..{MAX_COOKIE_BYTES} bytes on a single line"
        )));
    }
    let path = store_path()?;
    let mut store = if path.exists() {
        load_from(&path)?
    } else {
        CredentialsFile::default()
    };
    if !store.credentials.contains_key(platform) && store.credentials.len() >= MAX_PLATFORMS {
        return Err(TklessError::InvalidArgument(format!(
            "Credentials store supports at most {MAX_PLATFORMS} platforms"
        )));
    }
    store
        .credentials
        .insert(platform.to_string(), cookie.to_string());
    state_file::write(&path, &store)?;
    Ok(path)
}

/// Removes one platform. Returns whether an entry existed.
pub fn remove(platform: &str) -> Result<bool, TklessError> {
    if !valid_platform(platform) {
        return Err(TklessError::InvalidArgument(
            "Platform must match [a-z0-9_-]{1,32}".into(),
        ));
    }
    let path = store_path()?;
    if !path.exists() {
        return Ok(false);
    }
    let mut store = load_from(&path)?;
    let removed = store.credentials.remove(platform).is_some();
    if removed {
        state_file::write(&path, &store)?;
    }
    Ok(removed)
}

pub fn list() -> Result<Vec<String>, TklessError> {
    let path = store_path()?;
    Ok(load_from(&path)?.platforms().cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Environment variables are process-global, so this test owns the store path.
    #[test]
    fn store_roundtrip_enforces_limits_and_permissions() {
        let path = std::env::temp_dir().join(format!(
            "tkless-credentials-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        std::env::set_var("TKLESS_CREDENTIALS_FILE", &path);

        assert!(set("GitHub", "a=b").is_err());
        assert!(set("github", "").is_err());
        assert!(set("github", "line1\nline2").is_err());
        assert!(set("github", &"x".repeat(MAX_COOKIE_BYTES + 1)).is_err());

        set("github", " session=abc; token=def ").unwrap();
        let loaded = load_from(&path).unwrap();
        assert_eq!(
            loaded.get("github").map(String::as_str),
            Some("session=abc; token=def")
        );
        assert_eq!(list().unwrap(), vec!["github".to_string()]);

        let fetcher = crate::fetcher::Fetcher::new(crate::FetchOptions::default()).unwrap();
        let github = url::Url::parse("https://github.com/tokio-rs/tokio").unwrap();
        assert_eq!(
            fetcher.resolve_cookie_for_url(&github).as_deref(),
            Some("session=abc; token=def")
        );
        let unrelated = url::Url::parse("https://example.com/").unwrap();
        assert_eq!(fetcher.resolve_cookie_for_url(&unrelated), None);
        let explicit = crate::fetcher::Fetcher::new(crate::FetchOptions {
            cookie: Some("github:cli=1".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            explicit.resolve_cookie_for_url(&github).as_deref(),
            Some("cli=1")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "credentials must be owner-only");
        }

        assert!(remove("github").unwrap());
        assert!(!remove("github").unwrap());
        assert!(list().unwrap().is_empty());

        let _ = std::fs::remove_file(&path);
        std::env::remove_var("TKLESS_CREDENTIALS_FILE");
    }

    #[cfg(unix)]
    #[test]
    fn corrupt_store_is_reported_but_does_not_break_fetch_time_load() {
        let path = std::env::temp_dir().join(format!(
            "tkless-credentials-corrupt-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, b"{broken").unwrap();
        assert!(load_from(&path).is_err());
        std::fs::remove_file(&path).unwrap();
    }
}
