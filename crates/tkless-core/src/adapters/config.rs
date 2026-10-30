//! Declarative adapters loaded from `TKLESS_ADAPTERS_DIR` (`*.json`).
//!
//! Config adapters only run when the environment variable is set, so library
//! users and tests are unaffected unless they opt in. They are registered after
//! built-ins, which means built-ins always win on overlapping hosts.
use super::{AdapterRegistry, BoxFuture, PlatformAdapter};
use crate::error::TklessError;
use crate::markdown::MarkdownConverter;
use crate::model::{
    ExtractionOptions, ExtractionProvenance, OutputFormat, PageDocument, PageMetadata,
};
use crate::token::count_tokens;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use url::Url;

pub const MAX_CONFIG_ADAPTERS: usize = 32;
pub const MAX_CONFIG_BYTES: u64 = 64 * 1024;
pub const MAX_HOSTS: usize = 8;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigAdapterFile {
    name: String,
    hosts: Vec<String>,
    #[serde(default)]
    path_prefix: Option<String>,
    #[serde(default)]
    schema: Option<Value>,
}

pub struct ConfigAdapter {
    name: &'static str,
    hosts: Vec<String>,
    path_prefix: Option<String>,
    schema: Option<crate::schema::ExtractionSchema>,
}

impl ConfigAdapter {
    fn compile(file: ConfigAdapterFile, origin: &Path) -> Result<Self, TklessError> {
        let invalid = |message: &str| {
            TklessError::InvalidArgument(format!(
                "Invalid adapter config '{}': {message}",
                origin.display()
            ))
        };
        if file.name.is_empty()
            || file.name.len() > 32
            || !file.name.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
            })
        {
            return Err(invalid("name must match [a-z0-9_-]{1,32}"));
        }
        if file.hosts.is_empty() || file.hosts.len() > MAX_HOSTS {
            return Err(invalid("hosts must contain 1..8 entries"));
        }
        let mut hosts = Vec::with_capacity(file.hosts.len());
        for host in file.hosts {
            let host = host.trim().to_ascii_lowercase();
            if host.is_empty()
                || host.len() > 255
                || !host
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
                || host.starts_with('.')
                || host.ends_with('.')
                || host.contains("..")
            {
                return Err(invalid("hosts must be lowercase domain names"));
            }
            hosts.push(host);
        }
        if let Some(prefix) = &file.path_prefix {
            if !prefix.starts_with('/') || prefix.len() > 256 {
                return Err(invalid("path_prefix must start with '/'"));
            }
        }
        let schema = file
            .schema
            .map(crate::schema::ExtractionSchema::from_json_value)
            .transpose()?;
        let name: &'static str = Box::leak(file.name.into_boxed_str());
        Ok(Self {
            name,
            hosts,
            path_prefix: file.path_prefix,
            schema,
        })
    }

    async fn do_extract(
        &self,
        fetcher: &crate::fetcher::Fetcher,
        url: &Url,
        options: &ExtractionOptions,
    ) -> Result<PageDocument, TklessError> {
        let response = fetcher.get(url).send().await?;
        let status = response.status().as_u16();
        let final_url = response.url().to_string();
        let html = fetcher.read_text(response).await?;
        let cleaned = crate::cleaner::clean_html(&html, Some(&final_url), options.mode)?;
        let content = match options.format {
            OutputFormat::Raw => html.clone(),
            OutputFormat::Html => cleaned.html.clone(),
            OutputFormat::Text => scraper::Html::parse_document(&cleaned.html)
                .root_element()
                .text()
                .collect::<Vec<_>>()
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            _ => MarkdownConverter::new().convert(
                &cleaned.html,
                options.strip_images,
                options.strip_links,
            )?,
        };
        let entity = match &self.schema {
            Some(schema) => Some(crate::schema::extract_entities(
                &html,
                schema,
                Some(url.as_str()),
            )?),
            None => None,
        };
        Ok(PageDocument {
            extraction: ExtractionProvenance::default(),
            warnings: Vec::new(),
            url: url.to_string(),
            final_url: final_url.clone(),
            status,
            title: cleaned.title,
            byline: cleaned.byline,
            token_count: count_tokens(&content, options.tokenizer),
            content,
            metadata: PageMetadata {
                canonical_url: Some(final_url),
                ..Default::default()
            },
            json_ld: Vec::new(),
            tokenizer: options.tokenizer.as_str().to_string(),
            truncated: false,
            subtitles: None,
            danmaku: None,
            comments: None,
            conclusion: None,
            links: None,
            images: None,
            tables: None,
            entity,
            chunks: None,
            content_hash: None,
            metrics: None,
        })
    }
}

impl PlatformAdapter for ConfigAdapter {
    fn name(&self) -> &'static str {
        self.name
    }

    fn matches(&self, url: &Url) -> bool {
        let Some(host) = url.host_str().map(|host| host.to_ascii_lowercase()) else {
            return false;
        };
        let host_matches = self
            .hosts
            .iter()
            .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")));
        host_matches
            && self
                .path_prefix
                .as_ref()
                .is_none_or(|prefix| url.path().starts_with(prefix.as_str()))
    }

    fn extract<'a>(
        &'a self,
        fetcher: &'a crate::fetcher::Fetcher,
        url: &'a Url,
        options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<PageDocument, TklessError>> {
        Box::pin(async move { self.do_extract(fetcher, url, options).await })
    }
}

impl AdapterRegistry {
    /// Loads `*.json` adapters from `TKLESS_ADAPTERS_DIR`. Unset means no-op.
    pub fn load_config_adapters(&mut self) -> usize {
        let Some(dir) = std::env::var_os("TKLESS_ADAPTERS_DIR") else {
            return 0;
        };
        match load_dir(Path::new(&dir), self) {
            Ok(count) => count,
            Err(error) => {
                tracing::warn!(
                    dir = %Path::new(&dir).display(),
                    error = %error,
                    "Ignoring invalid config adapters"
                );
                0
            }
        }
    }
}

/// Loads and registers every valid adapter config in `dir`. Exposed for tests.
pub fn load_dir(dir: &Path, registry: &mut AdapterRegistry) -> Result<usize, TklessError> {
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut entries = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    let mut loaded = 0;
    for entry in entries {
        if loaded >= MAX_CONFIG_ADAPTERS {
            break;
        }
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        if entry.metadata()?.len() > MAX_CONFIG_BYTES {
            return Err(TklessError::InvalidArgument(format!(
                "Adapter config '{}' exceeds {MAX_CONFIG_BYTES} bytes",
                path.display()
            )));
        }
        let bytes = std::fs::read(&path)?;
        let file: ConfigAdapterFile = serde_json::from_slice(&bytes).map_err(|error| {
            TklessError::InvalidArgument(format!(
                "Invalid adapter config '{}': {error}",
                path.display()
            ))
        })?;
        registry.register(Arc::new(ConfigAdapter::compile(file, &path)?));
        loaded += 1;
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tkless-adapters-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn loads_and_matches_declarative_adapters() {
        let dir = temp_dir("valid");
        std::fs::write(
            dir.join("example.json"),
            br#"{"name":"example","hosts":["example.com"],"path_prefix":"/blog/","schema":{"fields":{"title":"h1"}}}"#,
        )
        .unwrap();
        let mut registry = AdapterRegistry::new();
        assert_eq!(load_dir(&dir, &mut registry).unwrap(), 1);

        let matched = Url::parse("https://news.example.com/blog/post").unwrap();
        assert_eq!(
            registry
                .find_adapter(&matched)
                .map(|adapter| adapter.name()),
            Some("example")
        );
        assert!(registry
            .find_adapter(&Url::parse("https://example.com/other").unwrap())
            .is_none());
        assert!(registry
            .find_adapter(&Url::parse("https://notexample.com/blog/x").unwrap())
            .is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_invalid_declarative_adapters() {
        for (name, body) in [
            (
                "bad-name",
                r#"{"name":"Bad Name","hosts":["example.com"]}"#.as_bytes(),
            ),
            ("no-hosts", br#"{"name":"empty","hosts":[]}"#.as_slice()),
            (
                "bad-host",
                br#"{"name":"bad-host","hosts":["https://example.com"]}"#.as_slice(),
            ),
            (
                "bad-prefix",
                br#"{"name":"bad-prefix","hosts":["example.com"],"path_prefix":"blog"}"#.as_slice(),
            ),
            (
                "bad-schema",
                br#"{"name":"bad-schema","hosts":["example.com"],"schema":"not-an-object"}"#
                    .as_slice(),
            ),
            (
                "unknown-field",
                br#"{"name":"extra","hosts":["example.com"],"code":"alert(1)"}"#.as_slice(),
            ),
        ] {
            let dir = temp_dir(name);
            std::fs::write(dir.join("adapter.json"), body).unwrap();
            let mut registry = AdapterRegistry::new();
            assert!(load_dir(&dir, &mut registry).is_err(), "{name}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
