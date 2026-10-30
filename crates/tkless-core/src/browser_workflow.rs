use crate::limits::*;
use crate::{
    cdp::{BrowserStep, CdpOptions},
    ExtractionOptions, PageDocument, TklessEngine, TklessError,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BrowserPlan {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    pub timeout_ms: u64,
    pub wait_ms: u64,
    pub allow_cross_origin_navigation: bool,
    pub steps: Vec<BrowserStep>,
}
impl Default for BrowserPlan {
    fn default() -> Self {
        Self {
            endpoint: None,
            timeout_ms: BROWSER_DEFAULT_TIMEOUT_MS,
            wait_ms: BROWSER_DEFAULT_WAIT_MS,
            allow_cross_origin_navigation: false,
            steps: vec![],
        }
    }
}
impl BrowserPlan {
    pub fn validate(&self) -> Result<(), TklessError> {
        if !(1..=BROWSER_MAX_TIMEOUT_MS).contains(&self.timeout_ms)
            || self.wait_ms > BROWSER_MAX_WAIT_MS
        {
            return Err(TklessError::InvalidArgument(
                "Browser timeout_ms must be 1..120000 and wait_ms <= 30000".into(),
            ));
        }
        crate::cdp::validate_steps(&self.steps)
    }
}

pub fn plan_schema() -> serde_json::Value {
    serde_json::json!({"type":"object","additionalProperties":false,"properties":{
        "endpoint":{"type":"string"},"timeout_ms":{"type":"integer","minimum":1,"maximum":BROWSER_MAX_TIMEOUT_MS,"default":BROWSER_DEFAULT_TIMEOUT_MS},
        "wait_ms":{"type":"integer","minimum":0,"maximum":BROWSER_MAX_WAIT_MS,"default":BROWSER_DEFAULT_WAIT_MS},"allow_cross_origin_navigation":{"type":"boolean","default":false},
        "steps":{"type":"array","maxItems":BROWSER_MAX_STEPS,"items":{"anyOf":[
            {"type":"object","additionalProperties":false,"required":["action","selector","timeout_ms"],"properties":{"action":{"const":"wait_for"},"selector":{"type":"string","minLength":1,"maxLength":BROWSER_MAX_SELECTOR_BYTES},"timeout_ms":{"type":"integer","minimum":1,"maximum":BROWSER_MAX_WAIT_MS}}},
            {"type":"object","additionalProperties":false,"required":["action","selector"],"properties":{"action":{"const":"click"},"selector":{"type":"string","minLength":1,"maxLength":BROWSER_MAX_SELECTOR_BYTES}}},
            {"type":"object","additionalProperties":false,"required":["action","selector","value"],"properties":{"action":{"const":"fill"},"selector":{"type":"string","minLength":1,"maxLength":BROWSER_MAX_SELECTOR_BYTES},"value":{"type":"string","maxLength":BROWSER_MAX_FILL_BYTES}}},
            {"type":"object","additionalProperties":false,"required":["action","pixels"],"properties":{"action":{"const":"scroll"},"pixels":{"type":"integer","minimum":-(BROWSER_MAX_SCROLL_PIXELS as i64),"maximum":BROWSER_MAX_SCROLL_PIXELS}}}
        ]}}
    }})
}

pub async fn render(
    engine: &TklessEngine,
    url: &str,
    plan: &BrowserPlan,
) -> Result<crate::cdp::RenderedPage, TklessError> {
    plan.validate()?;
    let target = crate::collection::canonical_url(url)?;
    engine.fetcher().validate_ssrf(&target).await?;
    let endpoint = crate::cdp::resolve_cdp_endpoint(plan.endpoint.as_deref(), true)
        .unwrap_or_else(|| "http://127.0.0.1:9222".into());
    let options = CdpOptions {
        endpoint,
        steps: plan.steps.clone(),
        navigation_origin: (!plan.allow_cross_origin_navigation)
            .then(|| target.origin().ascii_serialization()),
        read_only_requests: true,
        timeout_ms: Some(plan.timeout_ms),
        wait_ms: Some(plan.wait_ms),
        user_agent: engine.fetcher().options().user_agent.clone(),
        cookie: engine.fetcher().resolve_cookie_for_url(&target),
        allow_private_addresses: engine.fetcher().options().allow_private_addresses,
        ..Default::default()
    };
    let page = crate::cdp::fetch_rendered_page(url, &options).await?;
    let final_url = crate::collection::canonical_url(&page.final_url)?;
    engine.fetcher().validate_ssrf(&final_url).await?;
    if !plan.allow_cross_origin_navigation && final_url.origin() != target.origin() {
        return Err(TklessError::CdpError(
            "Browser navigated outside configured origin".into(),
        ));
    }
    if page.html.len() > engine.fetcher().options().max_bytes {
        return Err(TklessError::ResponseTooLarge {
            limit: engine.fetcher().options().max_bytes,
            received: page.html.len(),
        });
    }
    Ok(page)
}

pub async fn browse_page(
    engine: &TklessEngine,
    url: &str,
    plan: &BrowserPlan,
    extraction: &ExtractionOptions,
) -> Result<PageDocument, TklessError> {
    let page = render(engine, url, plan).await?;
    let mut doc = engine.process_html(&page.html, Some(&page.final_url), extraction)?;
    doc.url = url.into();
    doc.final_url = page.final_url;
    doc.extraction.transport = "cdp".into();
    Ok(doc)
}
