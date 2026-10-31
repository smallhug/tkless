use crate::{cli::SubCommand, output};
use anyhow::{bail, Result};
use std::path::Path;
use tkless_core::{ExtractionOptions, FetchOptions, TklessEngine, TklessError};

pub async fn run(
    command: &SubCommand,
    fetch_options: &FetchOptions,
    extraction_options: &ExtractionOptions,
    root_output: Option<&Path>,
) -> Result<bool> {
    if let SubCommand::Wechat(args) = command {
        let options = tkless_core::wechat::WechatOptions {
            limit: args.limit,
            max_pages: args.max_pages,
            delay_ms: args.delay_ms,
            fetch_content: args.fetch_content,
            follow_albums: !args.no_follow_albums,
            expected_biz: args.expected_biz.clone(),
        };
        options.validate()?;
        let mut fetch = fetch_options.clone();
        fetch.initial_target_host = Some("mp.weixin.qq.com".into());
        fetch.initial_target_origin = Some("https://mp.weixin.qq.com".into());
        let engine = TklessEngine::with_options(fetch)?;
        let mut extraction = extraction_options.clone();
        extraction.max_tokens = Some(args.max_tokens_per_article as usize);
        let report =
            tkless_core::wechat::list_articles(&engine, &args.source, &options, &extraction)
                .await?;
        output::write_payload(
            &serde_json::to_string_pretty(&report)?,
            args.output.as_deref().or(root_output),
        )?;
        if !report.complete && !args.allow_partial {
            bail!(
                "WeChat listing is partial ({:?}); inspect the report or use --allow-partial",
                report.stop_reason
            );
        }
        return Ok(true);
    }
    if let SubCommand::Watch(watch) = command {
        let output_path = watch.output.as_deref().or(root_output);
        crate::paths::distinct(Some(&watch.config), output_path)?;
        if watch.interval.is_some() && output_path.is_some() {
            return Err(TklessError::InvalidArgument(
                "--interval prints one JSON line per round and cannot be combined with --output"
                    .into(),
            )
            .into());
        }
        let config: tkless_core::watch::WatchConfig =
            serde_json::from_slice(&std::fs::read(&watch.config)?)?;
        config.validate()?;
        let engine = TklessEngine::with_options(fetch_options.clone())?;
        loop {
            let report =
                tkless_core::watch::run_once(&engine, &config, extraction_options, &watch.state)
                    .await?;
            let payload = if watch.interval.is_some() || output_path.is_none() {
                serde_json::to_string(&report)?
            } else {
                serde_json::to_string_pretty(&report)?
            };
            output::write_payload(&payload, output_path)?;
            if !report.complete && !watch.allow_partial {
                bail!(
                    "Watch round incomplete: {} job error(s); inspect the report or use --allow-partial",
                    report.errors.len()
                );
            }
            let Some(interval) = watch.interval else {
                break;
            };
            tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        }
        return Ok(true);
    }
    if let SubCommand::Browse(browse) = command {
        let output_path = browse.output.as_deref().or(root_output);
        crate::paths::distinct(output_path, Some(&browse.plan))?;
        let plan: tkless_core::browser_workflow::BrowserPlan =
            serde_json::from_slice(&std::fs::read(&browse.plan)?)?;
        plan.validate()?;
        let mut fetch = fetch_options.clone();
        let url = url::Url::parse(&browse.url).map_err(TklessError::UrlParse)?;
        fetch.initial_target_host = url.host_str().map(str::to_owned);
        fetch.initial_target_origin = Some(url.origin().ascii_serialization());
        let engine = TklessEngine::with_options(fetch)?;
        let doc = tkless_core::browser_workflow::browse_page(
            &engine,
            &browse.url,
            &plan,
            extraction_options,
        )
        .await?;
        output::write_payload(&serde_json::to_string_pretty(&doc)?, output_path)?;
        return Ok(true);
    }
    if let SubCommand::Sync(sync) = command {
        let output_path = sync.collection.output.as_deref().or(root_output);
        crate::paths::distinct(output_path, Some(&sync.collection.config))?;
        let config: tkless_core::collection::CollectionConfig =
            serde_json::from_slice(&std::fs::read(&sync.collection.config)?)?;
        config.validate()?;
        std::fs::create_dir_all(&sync.state)?;
        if let Some(output) = output_path {
            let parent = output
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(std::path::Path::new("."));
            if parent
                .canonicalize()?
                .starts_with(sync.state.canonicalize()?)
            {
                return Err(TklessError::InvalidArgument(
                    "Output must be outside the sync state directory".into(),
                )
                .into());
            }
        }
        let mut fetch = fetch_options.clone();
        let url = url::Url::parse(&config.url).map_err(TklessError::UrlParse)?;
        fetch.initial_target_host = url.host_str().map(str::to_owned);
        fetch.initial_target_origin = Some(url.origin().ascii_serialization());
        let engine = TklessEngine::with_options(fetch)?;
        let budget = tkless_core::limits::sync_state_bytes(sync.max_state_mb)?;
        let report = tkless_core::sync::sync_collection(
            &engine,
            &config,
            extraction_options,
            &sync.state,
            budget,
        )
        .await?;
        output::write_payload(&serde_json::to_string_pretty(&report)?, output_path)?;
        if !report.complete && !sync.collection.allow_partial {
            bail!("Sync incomplete: {}", report.stop_reason);
        }
        return Ok(true);
    }
    if let SubCommand::Collect(collect) = command {
        let output_path = collect.output.as_deref().or(root_output);
        crate::paths::distinct(output_path, Some(&collect.config))?;
        let config: tkless_core::collection::CollectionConfig =
            serde_json::from_slice(&std::fs::read(&collect.config)?)?;
        config.validate()?;
        let mut fetch = fetch_options.clone();
        let url = url::Url::parse(&config.url).map_err(TklessError::UrlParse)?;
        fetch.initial_target_host = url.host_str().map(str::to_owned);
        fetch.initial_target_origin = Some(url.origin().ascii_serialization());
        let engine = TklessEngine::with_options(fetch)?;
        let report =
            tkless_core::collection::collect_pages(&engine, &config, extraction_options).await?;
        output::write_payload(&serde_json::to_string_pretty(&report)?, output_path)?;
        if !report.complete && !collect.allow_partial {
            bail!("Collection incomplete: {}", report.stop_reason);
        }
        return Ok(true);
    }
    Ok(false)
}
