//! Bounded batch extraction with durable checkpoints and backpressured per-item delivery.
mod checkpoint;
mod scheduler;

use crate::{ExtractionOptions, PageDocument, TklessEngine, TklessError};
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BatchOrder {
    #[default]
    Input,
    Completion,
}

#[derive(Debug, Clone)]
pub struct BatchOptions {
    pub concurrency: usize,
    pub delay_ms: u64,
    pub per_host_concurrency: usize,
    pub per_host_delay_ms: u64,
    pub order: BatchOrder,
    pub checkpoint: Option<PathBuf>,
    pub resume: bool,
    pub retry_failed: bool,
}

impl Default for BatchOptions {
    fn default() -> Self {
        Self {
            concurrency: 5,
            delay_ms: 0,
            per_host_concurrency: 2,
            per_host_delay_ms: 0,
            order: BatchOrder::Input,
            checkpoint: None,
            resume: false,
            retry_failed: false,
        }
    }
}

impl BatchOptions {
    pub fn validate(&self) -> Result<(), TklessError> {
        if !(1..=64).contains(&self.concurrency) || !(1..=64).contains(&self.per_host_concurrency) {
            return Err(TklessError::InvalidArgument(
                "batch concurrency and per_host_concurrency must be between 1 and 64".into(),
            ));
        }
        if self.delay_ms > 3_600_000 || self.per_host_delay_ms > 3_600_000 {
            return Err(TklessError::InvalidArgument(
                "batch delays must not exceed 3600000 ms".into(),
            ));
        }
        if self.resume && self.checkpoint.is_none() {
            return Err(TklessError::InvalidArgument(
                "resume requires checkpoint".into(),
            ));
        }
        if self.retry_failed && !self.resume {
            return Err(TklessError::InvalidArgument(
                "retry_failed requires resume".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchItem {
    /// Zero-based position in the original URL list; duplicate URLs retain separate positions.
    pub index: usize,
    pub url: String,
    pub document: Option<PageDocument>,
    pub error: Option<String>,
    pub exit_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_details: Option<crate::error::ErrorDetails>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BatchSummary {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub resumed: usize,
    pub fetched: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchReport {
    #[serde(flatten)]
    pub summary: BatchSummary,
    pub results: Vec<BatchItem>,
}

/// Deliver each item once, awaiting the consumer before taking another result.
/// Checkpoints are synced before delivery, even for completed items waiting for input-order output.
pub async fn run_batch<F, Fut>(
    engine: &TklessEngine,
    urls: &[String],
    extraction: &ExtractionOptions,
    options: &BatchOptions,
    mut on_item: F,
) -> Result<BatchSummary, TklessError>
where
    F: FnMut(BatchItem) -> Fut + Send,
    Fut: Future<Output = Result<(), TklessError>> + Send,
{
    extraction.validate()?;
    options.validate()?;
    if urls.is_empty() {
        return Err(TklessError::InvalidArgument(
            "batch URLs must not be empty".into(),
        ));
    }
    let checkpoint = checkpoint::Checkpoint::open(engine, urls, extraction, options)?
        .map(|file| Arc::new(Mutex::new(file)));
    let scheduler = Arc::new(scheduler::Scheduler::new(options));
    let work = stream::iter(urls.iter().enumerate().map(|(index, url)| {
        let checkpoint = checkpoint.clone();
        let scheduler = scheduler.clone();
        async move {
            if let Some(checkpoint) = &checkpoint {
                let restored = checkpoint
                    .lock()
                    .map_err(|e| TklessError::CheckpointError(e.to_string()))?
                    .restore(index, options.retry_failed)?;
                if let Some(item) = restored {
                    return Ok((item, true));
                }
            }
            let _permit = scheduler.acquire(url).await?;
            let item = match engine.process_url(url, extraction).await {
                Ok(document) => BatchItem {
                    index,
                    url: url.clone(),
                    document: Some(document),
                    error: None,
                    exit_code: 0,
                    error_details: None,
                },
                Err(error) => BatchItem {
                    index,
                    url: url.clone(),
                    document: None,
                    error: Some(error.to_string()),
                    exit_code: error.exit_code(),
                    error_details: Some(error.details()),
                },
            };
            if let Some(checkpoint) = &checkpoint {
                checkpoint
                    .lock()
                    .map_err(|e| TklessError::CheckpointError(e.to_string()))?
                    .save(&item)?;
            }
            Ok::<_, TklessError>((item, false))
        }
    }));
    let mut work = match options.order {
        BatchOrder::Input => work.buffered(options.concurrency).boxed(),
        BatchOrder::Completion => work.buffer_unordered(options.concurrency).boxed(),
    };
    let mut summary = BatchSummary {
        total: urls.len(),
        ..Default::default()
    };
    while let Some(outcome) = work.next().await {
        let (item, restored) = outcome?;
        summary.resumed += usize::from(restored);
        summary.fetched += usize::from(!restored);
        if item.error.is_some() {
            summary.failed += 1;
        } else {
            summary.succeeded += 1;
        }
        on_item(item).await?;
    }
    Ok(summary)
}

/// Collect a complete report when the caller explicitly needs all documents in memory.
pub async fn collect_batch(
    engine: &TklessEngine,
    urls: &[String],
    extraction: &ExtractionOptions,
    options: &BatchOptions,
) -> Result<BatchReport, TklessError> {
    let mut results = Vec::with_capacity(urls.len());
    let summary = run_batch(engine, urls, extraction, options, |item| {
        results.push(item);
        std::future::ready(Ok(()))
    })
    .await?;
    Ok(BatchReport { summary, results })
}

impl BatchReport {
    pub fn markdown(&self) -> String {
        self.results
            .iter()
            .map(|item| match &item.document {
                Some(doc) => format!(
                    "## {} ({})\n\n{}",
                    doc.title.as_deref().unwrap_or("Document"),
                    item.url,
                    doc.content
                ),
                None => format!(
                    "## Failed: {}\n\n{}",
                    item.url,
                    item.error.as_deref().unwrap_or("Unknown error")
                ),
            })
            .collect::<Vec<_>>()
            .join("\n\n---\n\n")
    }
}
