//! Optional, origin-scoped persistent cookie jar.
//!
//! Cookies received through `Set-Cookie` are stored in a small JSON file and replayed only
//! to the exact request host and matching paths. `Domain` attributes are intentionally
//! ignored: without a public-suffix list, broadening a cookie to a parent domain could let
//! one host poison cookies for unrelated siblings. `Secure` and expiry attributes are
//! honored.
use crate::error::TklessError;
use reqwest::header::{HeaderMap, SET_COOKIE};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use url::Url;

pub const MAX_COOKIES: usize = 1024;
const MAX_JAR_BYTES: u64 = 4 * 1024 * 1024;
const MAX_NAME_BYTES: usize = 128;
const MAX_VALUE_BYTES: usize = 4096;
const MAX_PATH_BYTES: usize = 1024;
const MAX_DOMAIN_BYTES: usize = 253;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct StoredCookie {
    name: String,
    value: String,
    domain: String,
    path: String,
    #[serde(default)]
    secure: bool,
    #[serde(default)]
    expires_at: Option<i64>,
}

#[derive(Serialize, Deserialize)]
struct CookieJarFile {
    version: u32,
    cookies: Vec<StoredCookie>,
}

/// Persistent cookie store shared by all requests of one fetcher.
pub struct CookieJar {
    path: PathBuf,
    cookies: Mutex<Vec<StoredCookie>>,
}

impl CookieJar {
    /// Opens an existing jar or starts an empty one. Malformed files fail loudly.
    pub fn open(path: &Path) -> Result<Self, TklessError> {
        let mut cookies = Vec::new();
        if path.exists() {
            let metadata = std::fs::metadata(path)?;
            if metadata.len() > MAX_JAR_BYTES {
                return Err(TklessError::InvalidArgument(format!(
                    "Cookie jar exceeds {MAX_JAR_BYTES} bytes"
                )));
            }
            let file: CookieJarFile = crate::state_file::read(path)?;
            if file.version != 1 {
                return Err(TklessError::InvalidArgument(format!(
                    "Unsupported cookie jar version {}; expected 1",
                    file.version
                )));
            }
            if file.cookies.len() > MAX_COOKIES {
                return Err(TklessError::InvalidArgument(format!(
                    "Cookie jar contains more than {MAX_COOKIES} entries"
                )));
            }
            for cookie in file.cookies {
                if !valid_cookie(&cookie) {
                    return Err(TklessError::InvalidArgument(
                        "Cookie jar contains an invalid entry".into(),
                    ));
                }
                cookies.push(cookie);
            }
        }
        Ok(Self {
            path: path.to_path_buf(),
            cookies: Mutex::new(cookies),
        })
    }

    /// Builds a `Cookie` header for the URL, honoring domain, path and `Secure`.
    pub fn header_for(&self, url: &Url) -> Option<String> {
        let host = url.host_str()?.to_ascii_lowercase();
        let path = normalized_path(url.path());
        let secure = url.scheme() == "https";
        let now = chrono::Utc::now().timestamp();
        let cookies = self.lock();
        let mut matching: Vec<&StoredCookie> = cookies
            .iter()
            .filter(|cookie| !expired(cookie, now))
            .filter(|cookie| !cookie.secure || secure)
            .filter(|cookie| cookie.domain == host)
            .filter(|cookie| path_matches(&path, &cookie.path))
            .collect();
        matching.sort_by_key(|cookie| std::cmp::Reverse(cookie.path.len()));
        let mut seen = std::collections::HashSet::new();
        let mut header = String::new();
        for cookie in matching {
            if !seen.insert(cookie.name.as_str()) {
                continue;
            }
            if !header.is_empty() {
                header.push_str("; ");
            }
            header.push_str(&cookie.name);
            header.push('=');
            header.push_str(&cookie.value);
        }
        (!header.is_empty()).then_some(header)
    }

    /// Applies all `Set-Cookie` headers for a response. Persistence failures are returned so
    /// callers can log them; callers must not fail the fetch because of a jar write error.
    pub fn store_from_headers(&self, url: &Url, headers: &HeaderMap) -> Result<(), TklessError> {
        let Some(host) = url.host_str().map(|host| host.to_ascii_lowercase()) else {
            return Ok(());
        };
        let now = chrono::Utc::now().timestamp();
        let default_path = default_path(url.path());
        let mut changed = false;
        let snapshot = {
            let mut cookies = self.lock();
            for value in headers.get_all(SET_COOKIE) {
                let Ok(raw) = value.to_str() else {
                    continue;
                };
                match parse_set_cookie(raw, &host, &default_path, now) {
                    Some(SetCookieOutcome::Set(cookie)) => {
                        if let Some(existing) = cookies.iter_mut().find(|stored| {
                            stored.name == cookie.name
                                && stored.domain == cookie.domain
                                && stored.path == cookie.path
                        }) {
                            *existing = cookie;
                        } else {
                            cookies.push(cookie);
                        }
                        changed = true;
                    }
                    Some(SetCookieOutcome::Delete { name, domain, path }) => {
                        let before = cookies.len();
                        cookies.retain(|stored| {
                            !(stored.name == name && stored.domain == domain && stored.path == path)
                        });
                        changed |= cookies.len() != before;
                    }
                    None => {}
                }
            }
            if changed {
                cookies.retain(|cookie| !expired(cookie, now));
                while cookies.len() > MAX_COOKIES {
                    cookies.remove(0);
                }
                cookies.clone()
            } else {
                Vec::new()
            }
        };
        if changed {
            crate::state_file::write(
                &self.path,
                &CookieJarFile {
                    version: 1,
                    cookies: snapshot,
                },
            )?;
        }
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, Vec<StoredCookie>> {
        self.cookies
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

enum SetCookieOutcome {
    Set(StoredCookie),
    Delete {
        name: String,
        domain: String,
        path: String,
    },
}

fn parse_set_cookie(
    raw: &str,
    host: &str,
    default_path: &str,
    now: i64,
) -> Option<SetCookieOutcome> {
    let mut parts = raw.split(';');
    let pair = parts.next()?.trim();
    let (name, value) = pair.split_once('=')?;
    let name = name.trim();
    if !valid_name(name) {
        return None;
    }
    let mut value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value = &value[1..value.len() - 1];
    }
    if !valid_cookie_value(value) {
        return None;
    }

    let domain = host.to_string();
    let mut path = default_path.to_string();
    let mut secure = false;
    let mut max_age = None;
    let mut expires = None;
    for attribute in parts {
        let attribute = attribute.trim();
        let (key, attribute_value) = attribute
            .split_once('=')
            .map(|(key, value)| (key.trim(), value.trim()))
            .unwrap_or((attribute, ""));
        match key.to_ascii_lowercase().as_str() {
            // Domain is parsed but deliberately not applied; cookies stay scoped to the host
            // that received them (see the module documentation).
            "domain" => {}
            "path" => {
                if attribute_value.starts_with('/') && attribute_value.len() <= MAX_PATH_BYTES {
                    path = attribute_value.to_string();
                }
            }
            "secure" => secure = true,
            "max-age" => {
                if let Ok(seconds) = attribute_value.parse::<i64>() {
                    max_age = Some(seconds);
                }
            }
            "expires" => {
                if let Ok(time) = chrono::DateTime::parse_from_rfc2822(attribute_value) {
                    expires = Some(time.timestamp());
                }
            }
            _ => {}
        }
    }

    let expires_at = match max_age {
        Some(seconds) if seconds <= 0 => {
            return Some(SetCookieOutcome::Delete {
                name: name.to_string(),
                domain,
                path,
            })
        }
        Some(seconds) => Some(now.saturating_add(seconds)),
        None => expires,
    };
    if expires_at.is_some_and(|time| time <= now) {
        return Some(SetCookieOutcome::Delete {
            name: name.to_string(),
            domain,
            path,
        });
    }
    Some(SetCookieOutcome::Set(StoredCookie {
        name: name.to_string(),
        value: value.to_string(),
        domain,
        path,
        secure,
        expires_at,
    }))
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// Rejects characters that can break out of a `name=value` cookie pair.
fn valid_cookie_value(value: &str) -> bool {
    value.len() <= MAX_VALUE_BYTES
        && !value
            .chars()
            .any(|c| c.is_control() || matches!(c, ';' | ','))
}

fn valid_cookie(cookie: &StoredCookie) -> bool {
    valid_name(&cookie.name)
        && valid_cookie_value(&cookie.value)
        && !cookie.domain.is_empty()
        && cookie.domain.len() <= MAX_DOMAIN_BYTES
        && cookie.path.starts_with('/')
        && cookie.path.len() <= MAX_PATH_BYTES
}

fn expired(cookie: &StoredCookie, now: i64) -> bool {
    cookie.expires_at.is_some_and(|time| time <= now)
}

fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if !request_path.starts_with(cookie_path) {
        return false;
    }
    cookie_path.ends_with('/') || request_path.as_bytes().get(cookie_path.len()) == Some(&b'/')
}

fn normalized_path(path: &str) -> String {
    if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    }
}

fn default_path(path: &str) -> String {
    if !path.starts_with('/') {
        return "/".to_string();
    }
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(index) => path[..index].to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn jar_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "tkless-cookie-jar-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.append(SET_COOKIE, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn stores_and_replays_cookies_with_scope_and_secure_rules() {
        let path = jar_path();
        let _ = std::fs::remove_file(&path);
        let jar = CookieJar::open(&path).unwrap();
        let https = Url::parse("https://example.com/app/page").unwrap();
        jar.store_from_headers(&https, &headers("sid=abc; Path=/; HttpOnly; Secure"))
            .unwrap();
        assert_eq!(jar.header_for(&https).as_deref(), Some("sid=abc"));
        assert_eq!(
            jar.header_for(&Url::parse("http://example.com/").unwrap()),
            None
        );
        assert_eq!(
            jar.header_for(&Url::parse("https://other.com/").unwrap()),
            None
        );
        assert_eq!(
            jar.header_for(&Url::parse("https://example.com/other").unwrap())
                .as_deref(),
            Some("sid=abc")
        );

        let reopened = CookieJar::open(&path).unwrap();
        assert_eq!(reopened.header_for(&https).as_deref(), Some("sid=abc"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn domain_attributes_do_not_broaden_cookie_scope() {
        let path = jar_path();
        let _ = std::fs::remove_file(&path);
        let jar = CookieJar::open(&path).unwrap();
        let url = Url::parse("https://www.example.com/").unwrap();
        jar.store_from_headers(&url, &headers("shared=1; Domain=example.com; Path=/"))
            .unwrap();
        jar.store_from_headers(&url, &headers("evil=1; Domain=other.com; Path=/"))
            .unwrap();
        assert_eq!(
            jar.header_for(&Url::parse("https://api.example.com/").unwrap()),
            None
        );
        let home = jar
            .header_for(&Url::parse("https://www.example.com/").unwrap())
            .unwrap();
        assert!(home.contains("shared=1"));
        assert!(home.contains("evil=1"));
        assert_eq!(
            jar.header_for(&Url::parse("https://other.com/").unwrap()),
            None
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn host_only_cookies_paths_deletion_and_expiry_are_enforced() {
        let path = jar_path();
        let _ = std::fs::remove_file(&path);
        let jar = CookieJar::open(&path).unwrap();
        let url = Url::parse("https://example.com/app/page").unwrap();
        jar.store_from_headers(&url, &headers("hostonly=1; Path=/app"))
            .unwrap();
        assert_eq!(
            jar.header_for(&Url::parse("https://sub.example.com/app").unwrap()),
            None
        );
        assert_eq!(
            jar.header_for(&Url::parse("https://example.com/application").unwrap()),
            None
        );
        assert_eq!(
            jar.header_for(&Url::parse("https://example.com/app/sub").unwrap())
                .as_deref(),
            Some("hostonly=1")
        );

        jar.store_from_headers(&url, &headers("gone=1; Path=/; Max-Age=0"))
            .unwrap();
        assert!(!jar.header_for(&url).unwrap_or_default().contains("gone=1"));
        jar.store_from_headers(
            &url,
            &headers("gone=1; Path=/; Expires=Wed, 21 Oct 2015 07:28:00 GMT"),
        )
        .unwrap();
        jar.store_from_headers(&url, &headers("hostonly=1; Path=/app; Max-Age=1"))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(!jar
            .header_for(&url)
            .unwrap_or_default()
            .contains("hostonly=1"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_corrupt_or_oversized_files() {
        let path = jar_path();
        std::fs::write(&path, b"not json").unwrap();
        assert!(CookieJar::open(&path).is_err());
        std::fs::write(&path, "{\"version\":2,\"cookies\":[]}").unwrap();
        assert!(CookieJar::open(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
