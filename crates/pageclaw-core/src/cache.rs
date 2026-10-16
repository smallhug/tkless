use crate::error::PageclawError;
use crate::model::{ExtractionOptions, PageDocument};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

/// Cached entry holding extracted document and expiration metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub url: String,
    pub created_at: u64,
    pub ttl_secs: u64,
    pub document: PageDocument,
}

impl CacheEntry {
    /// Checks if this cache entry has expired.
    pub fn is_expired(&self, now: u64) -> bool {
        now >= self.created_at.saturating_add(self.ttl_secs)
    }
}

/// Two-level (L1 In-Memory + L2 Filesystem) document cache.
#[derive(Clone)]
pub struct DocumentCache {
    memory: Arc<RwLock<HashMap<String, CacheEntry>>>,
    cache_dir: Option<PathBuf>,
    default_ttl_secs: u64,
}

impl DocumentCache {
    /// Creates a cache configured with an optional persistent on-disk directory and default TTL.
    pub fn new(cache_dir: Option<PathBuf>, default_ttl_secs: u64) -> Self {
        let dir = cache_dir.or_else(resolve_default_cache_dir);
        if let Some(ref p) = dir {
            let _ = fs::create_dir_all(p);
        }
        Self {
            memory: Arc::new(RwLock::new(HashMap::new())),
            cache_dir: dir,
            default_ttl_secs: default_ttl_secs.max(1),
        }
    }

    /// Creates an in-memory-only cache without filesystem persistence.
    pub fn memory_only(default_ttl_secs: u64) -> Self {
        Self {
            memory: Arc::new(RwLock::new(HashMap::new())),
            cache_dir: None,
            default_ttl_secs: default_ttl_secs.max(1),
        }
    }

    /// Generates a deterministic FNV-1a hash key from the URL and canonical extraction options.
    pub fn generate_key(&self, url: &str, options: &ExtractionOptions) -> String {
        let canonical = format!(
            "{}|{:?}|{:?}|{:?}|{}|{}|{}|{}|{}|{:?}|{}|{}|{}|{}|{}|{:?}|{}|{:?}|{}",
            url.trim(),
            options.mode,
            options.format,
            options.max_tokens,
            options.strip_images,
            options.strip_links,
            options.frontmatter,
            options.no_adapter,
            options.feed,
            options.selector,
            options.extract_links,
            options.extract_images,
            options.extract_tables,
            options.subtitles,
            options.danmaku,
            options.danmaku_limit,
            options.comments,
            options.comments_limit,
            options.conclusion
        );

        let hash = fnv1a_64(canonical.as_bytes());
        let safe_slug: String = url
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .take(24)
            .collect();

        format!("{hash:016x}_{safe_slug}")
    }

    /// Retrieves a document from cache if present and unexpired.
    pub fn get(&self, key: &str) -> Option<PageDocument> {
        let now = current_time_secs();

        // 1. Check L1 in-memory cache
        {
            let mem = self.memory.read().ok()?;
            if let Some(entry) = mem.get(key) {
                if !entry.is_expired(now) {
                    debug!(key = key, "L1 memory cache hit");
                    return Some(entry.document.clone());
                }
            }
        }

        // 2. Check L2 filesystem cache
        if let Some(ref dir) = self.cache_dir {
            let file_path = dir.join(format!("{key}.json"));
            if file_path.is_file() {
                if let Ok(file) = File::open(&file_path) {
                    let reader = BufReader::new(file);
                    if let Ok(entry) = serde_json::from_reader::<_, CacheEntry>(reader) {
                        if !entry.is_expired(now) {
                            debug!(key = key, path = %file_path.display(), "L2 disk cache hit");
                            // Backfill L1 memory
                            if let Ok(mut mem) = self.memory.write() {
                                mem.insert(key.to_string(), entry.clone());
                            }
                            return Some(entry.document);
                        } else {
                            debug!(key = key, "Pruning expired L2 disk cache entry");
                            let _ = fs::remove_file(&file_path);
                        }
                    }
                }
            }
        }

        None
    }

    /// Stores a document into both L1 memory and L2 filesystem caches.
    pub fn set(
        &self,
        key: &str,
        doc: &PageDocument,
        ttl_secs: Option<u64>,
    ) -> Result<(), PageclawError> {
        let now = current_time_secs();
        let ttl = ttl_secs.unwrap_or(self.default_ttl_secs);

        let entry = CacheEntry {
            url: doc.url.clone(),
            created_at: now,
            ttl_secs: ttl,
            document: doc.clone(),
        };

        // Write L1 memory
        if let Ok(mut mem) = self.memory.write() {
            mem.insert(key.to_string(), entry.clone());
        }

        // Write L2 disk
        if let Some(ref dir) = self.cache_dir {
            let file_path = dir.join(format!("{key}.json"));
            match File::create(&file_path) {
                Ok(file) => {
                    let writer = BufWriter::new(file);
                    if let Err(e) = serde_json::to_writer(writer, &entry) {
                        warn!(error = %e, "Failed to serialize document to disk cache");
                    }
                }
                Err(e) => {
                    warn!(error = %e, path = %file_path.display(), "Failed to create disk cache file");
                }
            }
        }

        Ok(())
    }

    /// Removes expired entries from both memory and disk.
    pub fn prune_expired(&self) -> usize {
        let now = current_time_secs();
        let mut pruned = 0;

        // Prune memory
        if let Ok(mut mem) = self.memory.write() {
            let before = mem.len();
            mem.retain(|_, v| !v.is_expired(now));
            pruned += before.saturating_sub(mem.len());
        }

        // Prune disk
        if let Some(ref dir) = self.cache_dir {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry_res in entries.flatten() {
                    let path = entry_res.path();
                    if path.extension().and_then(|s| s.to_str()) == Some("json") {
                        if let Ok(file) = File::open(&path) {
                            let reader = BufReader::new(file);
                            if let Ok(cached) = serde_json::from_reader::<_, CacheEntry>(reader) {
                                if cached.is_expired(now) {
                                    let _ = fs::remove_file(&path);
                                    pruned += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        pruned
    }

    /// Clears all entries from both memory and disk.
    pub fn clear(&self) -> Result<(), PageclawError> {
        if let Ok(mut mem) = self.memory.write() {
            mem.clear();
        }

        if let Some(ref dir) = self.cache_dir {
            if dir.is_dir() {
                fs::remove_dir_all(dir).map_err(|e| PageclawError::CacheError(e.to_string()))?;
                let _ = fs::create_dir_all(dir);
            }
        }

        Ok(())
    }
}

/// Computes a 64-bit FNV-1a hash over raw bytes.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn current_time_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn resolve_default_cache_dir() -> Option<PathBuf> {
    let local = Path::new(".pageclaw_cache");
    if fs::create_dir_all(local).is_ok() {
        return Some(local.to_path_buf());
    }
    let temp = std::env::temp_dir().join("pageclaw_cache");
    let _ = fs::create_dir_all(&temp);
    Some(temp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ExtractionMode, OutputFormat, PageMetadata};

    #[test]
    fn test_cache_key_generation() {
        let cache = DocumentCache::memory_only(60);
        let opts1 = ExtractionOptions {
            mode: ExtractionMode::Article,
            format: OutputFormat::Markdown,
            ..Default::default()
        };
        let opts2 = ExtractionOptions {
            mode: ExtractionMode::Full,
            format: OutputFormat::Markdown,
            ..Default::default()
        };

        let key1 = cache.generate_key("https://example.com/page", &opts1);
        let key2 = cache.generate_key("https://example.com/page", &opts2);
        let key3 = cache.generate_key("https://example.com/page", &opts1);

        assert_eq!(key1, key3);
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_memory_cache_get_set() {
        let cache = DocumentCache::memory_only(300);
        let key = "test_key_01";
        let doc = PageDocument {
            url: "https://example.com".into(),
            final_url: "https://example.com".into(),
            status: 200,
            title: Some("Cached Page".into()),
            byline: None,
            content: "Hello from cache".into(),
            metadata: PageMetadata::default(),
            json_ld: vec![],
            token_count: 5,
            tokenizer: "o200k".into(),
            truncated: false,
            subtitles: None,
            danmaku: None,
            comments: None,
            conclusion: None,
            links: None,
            images: None,
            tables: None,
            entity: None,
            chunks: None,
            content_hash: None,
        };

        assert!(cache.get(key).is_none());
        cache.set(key, &doc, None).unwrap();

        let retrieved = cache.get(key).expect("Document should be cached");
        assert_eq!(retrieved.title.as_deref(), Some("Cached Page"));
        assert_eq!(retrieved.content, "Hello from cache");
    }

    #[test]
    fn test_cache_expiration() {
        let cache = DocumentCache::memory_only(0); // Expires immediately
        let key = "expire_key";
        let doc = PageDocument {
            url: "https://example.com".into(),
            final_url: "https://example.com".into(),
            status: 200,
            title: Some("Old Page".into()),
            byline: None,
            content: "Old".into(),
            metadata: PageMetadata::default(),
            json_ld: vec![],
            token_count: 1,
            tokenizer: "o200k".into(),
            truncated: false,
            subtitles: None,
            danmaku: None,
            comments: None,
            conclusion: None,
            links: None,
            images: None,
            tables: None,
            entity: None,
            chunks: None,
            content_hash: None,
        };

        cache.set(key, &doc, Some(0)).unwrap();
        // Wait 1ms
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(cache.get(key).is_none());
    }
}
