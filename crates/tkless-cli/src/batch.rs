use crate::paths::distinct;
use crate::{cli::CliFormat, output};
use anyhow::{Context, Result};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tkless_core::{
    batch::{run_batch, BatchOptions, BatchReport, BatchSummary},
    ExtractionOptions, TklessEngine, TklessError,
};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::Mutex,
};

pub struct BatchOutput<'a> {
    pub format: CliFormat,
    pub path: Option<&'a Path>,
    pub input: Option<&'a Path>,
    pub quiet: bool,
}

struct JsonlSink {
    path: Option<PathBuf>,
    writer: Option<Box<dyn AsyncWrite + Send + Unpin>>,
}

impl JsonlSink {
    async fn write(&mut self, mut bytes: Vec<u8>) -> Result<(), TklessError> {
        if self.writer.is_none() {
            self.writer = Some(match &self.path {
                Some(path) => Box::new(tokio::fs::File::create(path).await?),
                None => Box::new(tokio::io::stdout()),
            });
        }
        bytes.push(b'\n');
        let writer = self.writer.as_mut().expect("writer initialized");
        writer.write_all(&bytes).await?;
        writer.flush().await?;
        Ok(())
    }
}

pub async fn execute(
    engine: &TklessEngine,
    urls: &[String],
    extraction: &ExtractionOptions,
    job: &BatchOptions,
    output_options: BatchOutput<'_>,
) -> Result<BatchSummary> {
    job.validate()?;
    distinct(output_options.path, job.checkpoint.as_deref())?;
    distinct(output_options.input, job.checkpoint.as_deref())?;
    distinct(output_options.input, output_options.path)?;
    let mut done = 0usize;
    let quiet = output_options.quiet;
    let total = urls.len();
    if output_options.format == CliFormat::Jsonl {
        let sink = Arc::new(Mutex::new(JsonlSink {
            path: output_options.path.map(Path::to_owned),
            writer: None,
        }));
        return Ok(run_batch(engine, urls, extraction, job, |item| {
            done += 1;
            if !quiet {
                eprintln!("[TkLess Batch] [{done}/{total}] {}", item.url);
            }
            let sink = sink.clone();
            async move {
                let bytes = serde_json::to_vec(&item)
                    .map_err(|e| TklessError::ConversionError(e.to_string()))?;
                sink.lock().await.write(bytes).await
            }
        })
        .await?);
    }
    let mut results = Vec::with_capacity(urls.len());
    let summary = run_batch(engine, urls, extraction, job, |item| {
        done += 1;
        if !quiet {
            eprintln!("[TkLess Batch] [{done}/{total}] {}", item.url);
        }
        results.push(item);
        std::future::ready(Ok(()))
    })
    .await?;
    let report = BatchReport {
        summary: summary.clone(),
        results,
    };
    let payload = match output_options.format {
        CliFormat::Json => serde_json::to_string_pretty(&report)?,
        _ => report.markdown(),
    };
    output::write_payload(&payload, output_options.path).context("Failed to write batch output")?;
    Ok(summary)
}
