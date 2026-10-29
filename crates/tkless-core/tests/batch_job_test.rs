#[path = "support/batch_site.rs"]
pub mod support;
use std::{
    io::Write,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use support::{Site, TempDir};
use tkless_core::{
    batch::{collect_batch, run_batch, BatchOptions, BatchOrder},
    ExtractionOptions, TklessError,
};
use tokio::time::timeout;

fn job(path: std::path::PathBuf) -> BatchOptions {
    BatchOptions {
        checkpoint: Some(path),
        order: BatchOrder::Completion,
        ..Default::default()
    }
}

#[tokio::test]
async fn completion_order_emits_fast_item_before_slow_page_finishes() {
    let site = Site::start().await;
    let urls = site.urls(&["/held", "/fast"]);
    let engine = site.engine();
    let (tx, mut rx) = tokio::sync::mpsc::channel(2);
    let work = tokio::spawn(async move {
        run_batch(
            &engine,
            &urls,
            &ExtractionOptions::default(),
            &BatchOptions {
                order: BatchOrder::Completion,
                ..Default::default()
            },
            |item| {
                let tx = tx.clone();
                async move {
                    tx.send(item.index).await.unwrap();
                    Ok(())
                }
            },
        )
        .await
        .unwrap()
    });
    assert_eq!(
        timeout(Duration::from_secs(5), rx.recv()).await.unwrap(),
        Some(1)
    );
    assert!(!work.is_finished());
    site.release();
    assert_eq!(work.await.unwrap().succeeded, 2);
    assert_eq!(rx.recv().await, Some(0));
}

#[tokio::test]
async fn input_order_is_bounded_and_finished_later_items_are_checkpointed_before_delivery() {
    let dir = TempDir::default();
    let path = dir.file("ordered.jsonl");
    let site = Site::start().await;
    let urls = site.urls(&["/held", "/fast"]);
    let engine = site.engine();
    let options = BatchOptions {
        checkpoint: Some(path.clone()),
        ..Default::default()
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(2);
    let task = tokio::spawn(async move {
        run_batch(
            &engine,
            &urls,
            &ExtractionOptions::default(),
            &options,
            |item| {
                let tx = tx.clone();
                async move {
                    tx.send(item.index).await.unwrap();
                    Ok(())
                }
            },
        )
        .await
    });
    timeout(Duration::from_secs(5), async {
        loop {
            if std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > 500) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(rx.try_recv().is_err());
    task.abort();
    let _ = task.await;
    site.release();
    let resumed = collect_batch(
        &site.engine(),
        &site.urls(&["/held", "/fast"]),
        &ExtractionOptions::default(),
        &BatchOptions {
            checkpoint: Some(path),
            resume: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(resumed.summary.resumed, 1);
    assert_eq!(resumed.summary.fetched, 1);
    assert_eq!(
        resumed
            .results
            .iter()
            .map(|item| item.index)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    assert_eq!(site.count("/fast"), 1);
}

#[tokio::test]
async fn interrupted_consumer_resumes_saved_results_without_fetching_them_again() {
    let dir = TempDir::default();
    let site = Site::start().await;
    let urls = site.urls(&["/held", "/fast"]);
    let engine = site.engine();
    let mut options = job(dir.file("job.jsonl"));
    let error = run_batch(
        &engine,
        &urls,
        &ExtractionOptions::default(),
        &options,
        |_| {
            std::future::ready(Err(TklessError::Io(std::io::Error::other(
                "consumer stopped",
            ))))
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, TklessError::Io(_)));
    site.release();
    options.resume = true;
    let report = collect_batch(&engine, &urls, &ExtractionOptions::default(), &options)
        .await
        .unwrap();
    assert_eq!(
        (
            report.summary.total,
            report.summary.resumed,
            report.summary.fetched
        ),
        (2, 1, 1)
    );
    assert_eq!(site.count("/fast"), 1);
}

#[tokio::test]
async fn retry_failed_only_revisits_failed_indices() {
    let dir = TempDir::default();
    let site = Site::start().await;
    let urls = site.urls(&["/ok", "/flaky"]);
    let engine = site.engine();
    let mut options = job(dir.file("job.jsonl"));
    let first = collect_batch(&engine, &urls, &ExtractionOptions::default(), &options)
        .await
        .unwrap();
    assert_eq!((first.summary.succeeded, first.summary.failed), (1, 1));
    options.resume = true;
    let saved = collect_batch(&engine, &urls, &ExtractionOptions::default(), &options)
        .await
        .unwrap();
    assert_eq!(
        (
            saved.summary.resumed,
            saved.summary.fetched,
            saved.summary.failed
        ),
        (2, 0, 1)
    );
    options.retry_failed = true;
    let retried = collect_batch(&engine, &urls, &ExtractionOptions::default(), &options)
        .await
        .unwrap();
    assert_eq!(
        (
            retried.summary.resumed,
            retried.summary.fetched,
            retried.summary.succeeded
        ),
        (1, 1, 2)
    );
    assert_eq!((site.count("/ok"), site.count("/flaky")), (1, 2));
}

#[tokio::test]
async fn mismatched_checkpoint_configuration_is_rejected_without_modifying_the_file() {
    let dir = TempDir::default();
    let path = dir.file("job.jsonl");
    let site = Site::start().await;
    let urls = site.urls(&["/a", "/b"]);
    let engine = site.engine();
    let mut options = job(path.clone());
    collect_batch(&engine, &urls, &ExtractionOptions::default(), &options)
        .await
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    options.resume = true;
    let changed = ExtractionOptions {
        max_tokens: Some(4),
        ..Default::default()
    };
    for (list, extraction) in [
        (urls.clone(), changed),
        (
            urls.iter().rev().cloned().collect(),
            ExtractionOptions::default(),
        ),
    ] {
        assert!(matches!(
            collect_batch(&engine, &list, &extraction, &options).await,
            Err(TklessError::CheckpointError(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    assert_eq!(site.hits.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn incomplete_tail_is_recovered_but_complete_corruption_is_not_ignored() {
    let dir = TempDir::default();
    let path = dir.file("job.jsonl");
    let site = Site::start().await;
    let urls = site.urls(&["/ok"]);
    let engine = site.engine();
    let mut options = job(path.clone());
    collect_batch(&engine, &urls, &ExtractionOptions::default(), &options)
        .await
        .unwrap();
    let valid = std::fs::read(&path).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"index\":")
        .unwrap();
    options.resume = true;
    let report = collect_batch(&engine, &urls, &ExtractionOptions::default(), &options)
        .await
        .unwrap();
    assert_eq!(report.summary.resumed, 1);
    assert_eq!(std::fs::read(&path).unwrap(), valid);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{corrupt}\n")
        .unwrap();
    assert!(matches!(
        collect_batch(&engine, &urls, &ExtractionOptions::default(), &options).await,
        Err(TklessError::CheckpointError(_))
    ));
    assert_eq!(site.count("/ok"), 1);
}

#[tokio::test]
async fn concurrent_resume_is_rejected_while_the_owner_is_running() {
    let dir = TempDir::default();
    let path = dir.file("job.jsonl");
    let site = Site::start().await;
    let urls = site.urls(&["/held"]);
    let engine = site.engine();
    let options = job(path.clone());
    let task = tokio::spawn({
        let urls = urls.clone();
        async move { collect_batch(&engine, &urls, &ExtractionOptions::default(), &options).await }
    });
    timeout(Duration::from_secs(5), async {
        while site.count("/held") == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let second = collect_batch(
        &site.engine(),
        &urls,
        &ExtractionOptions::default(),
        &BatchOptions {
            checkpoint: Some(path),
            resume: true,
            ..Default::default()
        },
    )
    .await;
    assert!(matches!(second, Err(TklessError::CheckpointError(_))));
    site.release();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn backpressure_does_not_start_an_unbounded_number_of_requests() {
    let site = Site::start().await;
    let urls = site.urls(&["/a", "/b", "/c", "/d", "/e"]);
    let engine = site.engine();
    let gate = Arc::new(tokio::sync::Notify::new());
    let entered = Arc::new(tokio::sync::Notify::new());
    let task = tokio::spawn({
        let gate = gate.clone();
        let entered = entered.clone();
        async move {
            let mut first = true;
            run_batch(
                &engine,
                &urls,
                &ExtractionOptions::default(),
                &BatchOptions {
                    concurrency: 2,
                    ..Default::default()
                },
                |_| {
                    let wait = first;
                    first = false;
                    let gate = gate.clone();
                    let entered = entered.clone();
                    async move {
                        if wait {
                            entered.notify_one();
                            gate.notified().await;
                        }
                        Ok(())
                    }
                },
            )
            .await
            .unwrap()
        }
    });
    timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(site.hits.lock().unwrap().len() <= 2);
    gate.notify_one();
    assert_eq!(task.await.unwrap().succeeded, 5);
}

#[tokio::test]
async fn per_host_concurrency_and_start_intervals_are_enforced() {
    let site = Site::start().await;
    let urls = site.urls(&["/slow1", "/slow2", "/slow3", "/slow4"]);
    collect_batch(
        &site.engine(),
        &urls,
        &ExtractionOptions::default(),
        &BatchOptions {
            concurrency: 4,
            per_host_concurrency: 2,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(site.peak.load(Ordering::SeqCst), 2);
    site.hits.lock().unwrap().clear();
    collect_batch(
        &site.engine(),
        &site.urls(&["/a", "/b", "/c"]),
        &ExtractionOptions::default(),
        &BatchOptions {
            concurrency: 3,
            per_host_delay_ms: 50,
            delay_ms: 20,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let hits = site.hits.lock().unwrap();
    assert!(hits
        .windows(2)
        .all(|pair| pair[1].1.duration_since(pair[0].1) >= Duration::from_millis(35)));
}

#[tokio::test]
async fn invalid_job_options_do_not_create_checkpoints() {
    let dir = TempDir::default();
    let path = dir.file("new.jsonl");
    let site = Site::start().await;
    let options = BatchOptions {
        checkpoint: Some(path.clone()),
        retry_failed: true,
        ..Default::default()
    };
    assert!(matches!(
        collect_batch(
            &site.engine(),
            &site.urls(&["/ok"]),
            &ExtractionOptions::default(),
            &options
        )
        .await,
        Err(TklessError::InvalidArgument(_))
    ));
    assert!(!path.exists());
    assert_eq!(site.count("/ok"), 0);
}

#[tokio::test]
async fn credentials_and_schema_contents_are_part_of_checkpoint_identity() {
    let dir = TempDir::default();
    let site = Site::start().await;
    let urls = site.urls(&["/ok"]);
    let credentials = |value: &str| {
        tkless_core::TklessEngine::with_options(tkless_core::FetchOptions {
            allow_private_addresses: true,
            cookie: Some(value.into()),
            initial_target_host: Some("127.0.0.1".into()),
            ..Default::default()
        })
        .unwrap()
    };
    let mut options = job(dir.file("identity.jsonl"));
    collect_batch(
        &credentials("session=secret-a"),
        &urls,
        &ExtractionOptions::default(),
        &options,
    )
    .await
    .unwrap();
    assert!(
        !std::fs::read_to_string(options.checkpoint.as_ref().unwrap())
            .unwrap()
            .contains("secret-a")
    );
    options.resume = true;
    assert!(matches!(
        collect_batch(
            &credentials("session=secret-b"),
            &urls,
            &ExtractionOptions::default(),
            &options
        )
        .await,
        Err(TklessError::CheckpointError(_))
    ));
    let schema = dir.file("schema.json");
    std::fs::write(&schema, r#"{"title":"h1"}"#).unwrap();
    let extraction = ExtractionOptions {
        schema: Some(schema.to_string_lossy().into_owned()),
        ..Default::default()
    };
    let mut options = job(dir.file("schema-job.jsonl"));
    collect_batch(&site.engine(), &urls, &extraction, &options)
        .await
        .unwrap();
    std::fs::write(schema, r#"{"title":".changed"}"#).unwrap();
    options.resume = true;
    assert!(matches!(
        collect_batch(&site.engine(), &urls, &extraction, &options).await,
        Err(TklessError::CheckpointError(_))
    ));
}
