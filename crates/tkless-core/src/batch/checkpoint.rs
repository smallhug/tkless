use super::{BatchItem, BatchOptions};
use crate::{DocumentCache, ExtractionOptions, TklessEngine, TklessError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
};

const MAX_RECORD_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Header {
    kind: String,
    version: u32,
    fingerprint: String,
    total: usize,
}

#[derive(Clone, Copy)]
struct Saved {
    offset: u64,
    failed: bool,
}

pub(super) struct Checkpoint {
    file: File,
    records: HashMap<usize, Saved>,
}

fn error(message: impl Into<String>) -> TklessError {
    TklessError::CheckpointError(message.into())
}

/// Identity covers URL order, extraction configuration, Schema file contents and hashed request credentials.
fn fingerprint(engine: &TklessEngine, urls: &[String], extraction: &ExtractionOptions) -> String {
    let mut hash = Sha256::new();
    hash.update(b"tkless-batch-v1\0");
    hash.update(env!("CARGO_PKG_VERSION").as_bytes());
    let cache = DocumentCache::memory_only(1);
    for url in urls {
        hash.update(engine.cache_key(&cache, url, extraction).as_bytes());
    }
    format!("{:x}", hash.finalize())
}

fn line(reader: &mut impl BufRead) -> Result<Vec<u8>, TklessError> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_RECORD_BYTES + 1)
        .read_until(b'\n', &mut bytes)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(error("Checkpoint record exceeds 128 MiB"));
    }
    Ok(bytes)
}

impl Checkpoint {
    pub(super) fn open(
        engine: &TklessEngine,
        urls: &[String],
        extraction: &ExtractionOptions,
        options: &BatchOptions,
    ) -> Result<Option<Self>, TklessError> {
        let Some(path) = options.checkpoint.as_ref() else {
            return Ok(None);
        };
        let mut open = OpenOptions::new();
        open.read(true).write(true).create_new(!options.resume);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open.mode(0o600);
        }
        let mut file = open.open(path).map_err(|e| {
            error(format!(
                "Cannot {} {}: {e}",
                if options.resume { "resume" } else { "create" },
                path.display()
            ))
        })?;
        file.try_lock()
            .map_err(|e| error(format!("Cannot exclusively lock {}: {e}", path.display())))?;
        let expected = fingerprint(engine, urls, extraction);
        let mut records = HashMap::new();
        if options.resume {
            let mut reader = BufReader::new(&mut file);
            let header_line = line(&mut reader)?;
            let header: Header = serde_json::from_slice(&header_line)
                .map_err(|e| error(format!("Invalid checkpoint header: {e}")))?;
            if header.kind != "tkless_batch"
                || header.version != 1
                || header.total != urls.len()
                || header.fingerprint != expected
            {
                return Err(error("Checkpoint does not match the current URL list, extraction options, Schema or request identity"));
            }
            if !header_line.ends_with(b"\n") {
                return Err(error("Incomplete checkpoint header"));
            }
            let mut valid_end = header_line.len() as u64;
            loop {
                let bytes = line(&mut reader)?;
                if bytes.is_empty() || !bytes.ends_with(b"\n") {
                    break;
                }
                let item: BatchItem = serde_json::from_slice(&bytes).map_err(|e| {
                    error(format!(
                        "Invalid completed checkpoint record at byte {valid_end}: {e}"
                    ))
                })?;
                if urls.get(item.index) != Some(&item.url)
                    || item.document.is_some() == item.error.is_some()
                    || (item.error.is_some() && item.exit_code == 0)
                    || (item.document.is_some() && item.exit_code != 0)
                {
                    return Err(error(format!(
                        "Invalid checkpoint result at index {}",
                        item.index
                    )));
                }
                records.insert(
                    item.index,
                    Saved {
                        offset: valid_end,
                        failed: item.error.is_some(),
                    },
                );
                valid_end += bytes.len() as u64;
            }
            // Only an incomplete final record is discarded; complete malformed records are errors.
            drop(reader);
            file.set_len(valid_end)?;
            file.seek(SeekFrom::End(0))?;
        } else {
            let header = Header {
                kind: "tkless_batch".into(),
                version: 1,
                fingerprint: expected,
                total: urls.len(),
            };
            serde_json::to_writer(&mut file, &header).map_err(|e| error(e.to_string()))?;
            file.write_all(b"\n")?;
            file.sync_data()?;
        }
        Ok(Some(Self { file, records }))
    }

    pub(super) fn restore(
        &mut self,
        index: usize,
        retry_failed: bool,
    ) -> Result<Option<BatchItem>, TklessError> {
        let Some(record) = self.records.get(&index).copied() else {
            return Ok(None);
        };
        if retry_failed && record.failed {
            return Ok(None);
        }
        self.file.seek(SeekFrom::Start(record.offset))?;
        let bytes = line(&mut BufReader::new(&mut self.file))?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| error(e.to_string()))
    }

    pub(super) fn save(&mut self, item: &BatchItem) -> Result<(), TklessError> {
        let mut bytes = serde_json::to_vec(item).map_err(|e| error(e.to_string()))?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(error("Checkpoint record exceeds 128 MiB"));
        }
        let offset = self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&bytes)?;
        self.file.sync_data()?;
        self.records.insert(
            item.index,
            Saved {
                offset,
                failed: item.error.is_some(),
            },
        );
        Ok(())
    }
}
