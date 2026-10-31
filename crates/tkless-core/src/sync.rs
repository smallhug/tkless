//! Immutable content snapshots and conservative incremental collection manifests.
use crate::limits::*;
use crate::{
    collection::{collect_pages, digest, CollectionConfig, CollectionRecord},
    state_file, DocumentCache, ExtractionOptions, TklessEngine, TklessError,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

/// Retain this many event files (including the generation being written).
const SYNC_RETAIN_EVENTS: usize = 64;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotEntry {
    pub hash: String,
    pub snapshot: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    identity: String,
    generation: u64,
    records: BTreeMap<String, SnapshotEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncEvent {
    pub kind: crate::workflow::SyncEventKind,
    pub id: String,
    pub previous: Option<SnapshotEntry>,
    pub current: Option<SnapshotEntry>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncReport {
    pub generation: u64,
    pub captured_at: u64,
    pub complete: bool,
    pub stop_reason: crate::workflow::StopReason,
    pub deletions_applied: bool,
    pub events: Vec<SyncEvent>,
    pub errors: Vec<String>,
}

fn record_hash(record: &CollectionRecord) -> Result<String, TklessError> {
    let detail = record.detail.as_ref().map(|doc| {
        serde_json::json!({
            "url":doc.final_url,"title":doc.title,"content":doc.content,"metadata":doc.metadata,
            "entity":doc.entity,"subtitles":doc.subtitles,"comments":doc.comments
        })
    });
    Ok(digest(serde_json::to_vec(&(&record.data, detail))?))
}

/// Reuse stable IDs while retaining unseen records after an incomplete run.
/// The entire operation is locked; the manifest is published last via atomic replacement.
pub async fn sync_collection(
    engine: &TklessEngine,
    config: &CollectionConfig,
    options: &ExtractionOptions,
    directory: &Path,
    max_state_bytes: u64,
) -> Result<SyncReport, TklessError> {
    config.validate()?;
    if config.id_pointer.is_none() && config.details.is_none() {
        return Err(TklessError::InvalidArgument(
            "Sync requires id_pointer or details.url_pointer for stable record identity".into(),
        ));
    }
    if !(MIB..=SYNC_MAX_STATE_MB * MIB).contains(&max_state_bytes) {
        return Err(TklessError::InvalidArgument(format!(
            "Sync state budget must be 1..{SYNC_MAX_STATE_MB} MiB"
        )));
    }

    let mut identity_config = config.clone();
    identity_config.max_pages = COLLECTION_DEFAULT_PAGES;
    identity_config.max_items = COLLECTION_DEFAULT_ITEMS;
    identity_config.delay_ms = 0;
    let request_identity = engine.cache_key(&DocumentCache::memory_only(1), &config.url, options);
    let identity = digest(serde_json::to_vec(&(identity_config, request_identity))?);
    let directory = directory.to_path_buf();
    let load_directory = directory.clone();
    let (mut manifest, lock) = state_file::blocking(move || {
        std::fs::create_dir_all(&load_directory)?;
        let manifest_path = load_directory.join("manifest.json");
        let lock = state_file::StateLock::acquire(&manifest_path)?;
        let manifest: Manifest = if manifest_path.exists() {
            let old: Manifest = state_file::read(&manifest_path)?;
            if old.version != 1 || old.identity != identity {
                return Err(TklessError::InvalidArgument(
                "Sync configuration or request identity changed; use a different state directory"
                    .into(),
            ));
            }
            old
        } else {
            Manifest {
                version: 1,
                identity,
                generation: 0,
                records: BTreeMap::new(),
            }
        };
        Ok((manifest, lock))
    })
    .await?;
    let collected = collect_pages(engine, config, options).await?;
    state_file::blocking(move || {
        let _lock = lock;
        let manifest_path = directory.join("manifest.json");
        let generation = manifest
            .generation
            .checked_add(1)
            .ok_or_else(|| TklessError::StateCorrupt("Snapshot generation overflow".into()))?;
        let captured_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut report = SyncReport {
            generation,
            captured_at,
            complete: collected.complete,
            stop_reason: collected.stop_reason,
            deletions_applied: collected.complete,
            events: vec![],
            errors: collected.errors,
        };
        let mut seen = HashSet::new();
        let mut snapshots = Vec::new();
        for record in collected.records {
            seen.insert(record.id.clone());
            let previous = manifest.records.get(&record.id).cloned();
            if let Some(error) = &record.error {
                report.events.push(SyncEvent {
                    kind: crate::workflow::SyncEventKind::Error,
                    id: record.id,
                    previous,
                    current: None,
                    error: Some(error.clone()),
                });
                continue;
            }
            let hash = record_hash(&record)?;
            let snapshot = format!("objects/{}-{}.json", record.id, hash);
            let current = SnapshotEntry { hash, snapshot };
            let kind = match &previous {
                None => crate::workflow::SyncEventKind::Added,
                Some(old) if old.hash == current.hash => crate::workflow::SyncEventKind::Unchanged,
                Some(_) => crate::workflow::SyncEventKind::Updated,
            };
            if kind != crate::workflow::SyncEventKind::Unchanged {
                snapshots.push((current.snapshot.clone(), serde_json::to_value(&record)?));
            }
            manifest.records.insert(record.id.clone(), current.clone());
            report.events.push(SyncEvent {
                kind,
                id: record.id,
                previous,
                current: Some(current),
                error: None,
            });
        }
        if collected.complete {
            let removed: Vec<_> = manifest
                .records
                .keys()
                .filter(|id| !seen.contains(*id))
                .cloned()
                .collect();
            for id in removed {
                let previous = manifest.records.remove(&id);
                report.events.push(SyncEvent {
                    kind: crate::workflow::SyncEventKind::Removed,
                    id,
                    previous,
                    current: None,
                    error: None,
                });
            }
        }
        manifest.generation = generation;
        let event_path = directory.join("events").join(format!("{generation}.json"));
        // Reclaim history before enforcing the budget. Without this, a long-running
        // sync/watch job eventually crosses max_state_bytes and can never recover.
        let pending_paths: Vec<String> = snapshots.iter().map(|(path, _)| path.clone()).collect();
        prune_sync_state(&directory, &manifest, &pending_paths)?;
        // Bound retained snapshots and history as well as this run's pending writes.
        let mut used = directory_bytes(&directory)?;
        for (_, value) in &snapshots {
            used = used.saturating_add(serde_json::to_vec(value)?.len() as u64);
        }
        used = used
            .saturating_add(serde_json::to_vec(&manifest)?.len() as u64)
            .saturating_add(serde_json::to_vec(&report)?.len() as u64);
        if used > max_state_bytes {
            return Err(TklessError::StateBudgetExceeded(
                "Sync state disk budget exceeded; manifest was not advanced".into(),
            ));
        }
        std::fs::create_dir_all(directory.join("objects"))?;
        std::fs::create_dir_all(directory.join("events"))?;
        for (relative, value) in snapshots {
            state_file::write(&directory.join(relative), &value)?;
        }
        state_file::write(&event_path, &report)?;
        state_file::write(&manifest_path, &manifest)?;
        Ok(report)
    })
    .await
}

fn prune_sync_state(
    directory: &Path,
    manifest: &Manifest,
    pending_snapshots: &[String],
) -> Result<(), TklessError> {
    let events_dir = directory.join("events");
    let mut event_files: Vec<(u64, PathBuf)> = Vec::new();
    let mut unparsable_event = false;
    if events_dir.is_dir() {
        for entry in std::fs::read_dir(&events_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            match path
                .file_stem()
                .and_then(|value| value.to_str())
                .and_then(|value| value.parse::<u64>().ok())
            {
                Some(generation) => event_files.push((generation, path)),
                None => unparsable_event = true,
            }
        }
    }
    event_files.sort_by_key(|(generation, _)| *generation);
    let keep_old = SYNC_RETAIN_EVENTS.saturating_sub(1);
    let retain_from = event_files.len().saturating_sub(keep_old);
    for (_, path) in event_files.iter().take(retain_from) {
        let _ = std::fs::remove_file(path);
    }

    let mut referenced: HashSet<PathBuf> = HashSet::new();
    for entry in manifest.records.values() {
        referenced.insert(directory.join(&entry.snapshot));
    }
    for snapshot in pending_snapshots {
        referenced.insert(directory.join(snapshot));
    }

    // Objects referenced by retained event history must survive even when they are
    // no longer in the current manifest. If an event cannot be parsed, skip object
    // GC rather than guessing.
    let mut can_gc_objects = !unparsable_event;
    for (_, path) in &event_files[retain_from..] {
        match state_file::read::<SyncReport>(path) {
            Ok(report) => {
                for event in report.events {
                    if let Some(previous) = event.previous {
                        referenced.insert(directory.join(previous.snapshot));
                    }
                    if let Some(current) = event.current {
                        referenced.insert(directory.join(current.snapshot));
                    }
                }
            }
            Err(_) => can_gc_objects = false,
        }
    }

    if can_gc_objects {
        let objects_dir = directory.join("objects");
        if objects_dir.is_dir() {
            for entry in std::fs::read_dir(&objects_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                if !referenced.contains(&path) {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }

    Ok(())
}

fn directory_bytes(directory: &Path) -> Result<u64, TklessError> {
    let mut total = 0u64;
    for dir in [
        directory.to_path_buf(),
        directory.join("objects"),
        directory.join("events"),
    ] {
        if !dir.exists() {
            continue;
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                total = total.saturating_add(entry.metadata()?.len());
            }
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::{StopReason, SyncEventKind};
    use std::fs;

    #[test]
    fn prunes_old_events_and_unreferenced_objects() {
        let directory = std::env::temp_dir().join(format!(
            "tkless-sync-gc-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(directory.join("events")).unwrap();
        fs::create_dir_all(directory.join("objects")).unwrap();
        fs::write(directory.join("objects/current.json"), b"{}").unwrap();
        fs::write(directory.join("objects/pending.json"), b"{}").unwrap();
        fs::write(directory.join("objects/history.json"), b"{}").unwrap();
        fs::write(directory.join("objects/orphan.json"), b"{}").unwrap();

        let mut manifest = Manifest {
            version: 1,
            identity: "test".into(),
            generation: 100,
            records: BTreeMap::new(),
        };
        manifest.records.insert(
            "current".into(),
            SnapshotEntry {
                hash: "h".into(),
                snapshot: "objects/current.json".into(),
            },
        );

        for generation in 0..100u64 {
            let report = SyncReport {
                generation,
                captured_at: 0,
                complete: true,
                stop_reason: StopReason::Exhausted,
                deletions_applied: true,
                events: vec![SyncEvent {
                    kind: SyncEventKind::Updated,
                    id: "history".into(),
                    previous: Some(SnapshotEntry {
                        hash: "h".into(),
                        snapshot: "objects/history.json".into(),
                    }),
                    current: None,
                    error: None,
                }],
                errors: Vec::new(),
            };
            state_file::write(
                &directory.join(format!("events/{generation}.json")),
                &report,
            )
            .unwrap();
        }

        prune_sync_state(&directory, &manifest, &["objects/pending.json".to_string()]).unwrap();

        assert_eq!(
            fs::read_dir(directory.join("events")).unwrap().count(),
            SYNC_RETAIN_EVENTS - 1
        );
        assert!(directory.join("objects/current.json").exists());
        assert!(directory.join("objects/pending.json").exists());
        assert!(
            directory.join("objects/history.json").exists(),
            "objects referenced by retained events must survive"
        );
        assert!(!directory.join("objects/orphan.json").exists());

        fs::remove_dir_all(directory).unwrap();
    }
}
