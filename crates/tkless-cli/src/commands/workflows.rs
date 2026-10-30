use crate::{cli::SubCommand, output};
use anyhow::{bail, Result};
use tkless_core::{ExtractionOptions, FetchOptions, TklessEngine};

pub async fn run(
    command: &SubCommand,
    fetch_options: &FetchOptions,
    extraction_options: &ExtractionOptions,
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
            args.output.as_deref(),
        )?;
        if !report.complete && !args.allow_partial {
            bail!(
                "WeChat listing is partial ({:?}); inspect the report or use --allow-partial",
                report.stop_reason
            );
        }
        return Ok(true);
    }
    if let SubCommand::Browse(browse) = command {
        crate::paths::distinct(browse.output.as_deref(), Some(&browse.plan))?;
        let plan: tkless_core::browser_workflow::BrowserPlan =
            serde_json::from_slice(&std::fs::read(&browse.plan)?)?;
        plan.validate()?;
        let mut fetch = fetch_options.clone();
        let url = url::Url::parse(&browse.url)?;
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
        output::write_payload(
            &serde_json::to_string_pretty(&doc)?,
            browse.output.as_deref(),
        )?;
        return Ok(true);
    }
    if let SubCommand::Sync(sync) = command {
        crate::paths::distinct(
            sync.collection.output.as_deref(),
            Some(&sync.collection.config),
        )?;
        let config: tkless_core::collection::CollectionConfig =
            serde_json::from_slice(&std::fs::read(&sync.collection.config)?)?;
        config.validate()?;
        std::fs::create_dir_all(&sync.state)?;
        if let Some(output) = &sync.collection.output {
            let parent = output
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(std::path::Path::new("."));
            if parent
                .canonicalize()?
                .starts_with(sync.state.canonicalize()?)
            {
                bail!("Output must be outside the sync state directory");
            }
        }
        let mut fetch = fetch_options.clone();
        let url = url::Url::parse(&config.url)?;
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
        output::write_payload(
            &serde_json::to_string_pretty(&report)?,
            sync.collection.output.as_deref(),
        )?;
        if !report.complete && !sync.collection.allow_partial {
            bail!("Sync incomplete: {}", report.stop_reason);
        }
        return Ok(true);
    }
    if let SubCommand::Collect(collect) = command {
        crate::paths::distinct(collect.output.as_deref(), Some(&collect.config))?;
        let config: tkless_core::collection::CollectionConfig =
            serde_json::from_slice(&std::fs::read(&collect.config)?)?;
        config.validate()?;
        let mut fetch = fetch_options.clone();
        let url = url::Url::parse(&config.url)?;
        fetch.initial_target_host = url.host_str().map(str::to_owned);
        fetch.initial_target_origin = Some(url.origin().ascii_serialization());
        let engine = TklessEngine::with_options(fetch)?;
        let report =
            tkless_core::collection::collect_pages(&engine, &config, extraction_options).await?;
        output::write_payload(
            &serde_json::to_string_pretty(&report)?,
            collect.output.as_deref(),
        )?;
        if !report.complete && !collect.allow_partial {
            bail!("Collection incomplete: {}", report.stop_reason);
        }
        return Ok(true);
    }
    Ok(false)
}
