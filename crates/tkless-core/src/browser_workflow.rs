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
    #[serde(default)]
    pub include_shadow_dom: bool,
    #[serde(default)]
    pub include_frames: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture: Option<crate::cdp::NetworkCapture>,
    pub steps: Vec<BrowserStep>,
}
impl Default for BrowserPlan {
    fn default() -> Self {
        Self {
            endpoint: None,
            timeout_ms: BROWSER_DEFAULT_TIMEOUT_MS,
            wait_ms: BROWSER_DEFAULT_WAIT_MS,
            allow_cross_origin_navigation: false,
            include_shadow_dom: false,
            include_frames: false,
            capture: None,
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
        crate::cdp::validate_steps(&self.steps)?;
        if let Some(capture) = &self.capture {
            capture.validate()?;
        }
        Ok(())
    }
}

pub fn plan_schema() -> serde_json::Value {
    serde_json::json!({"type":"object","additionalProperties":false,"properties":{
        "endpoint":{"type":"string"},"timeout_ms":{"type":"integer","minimum":1,"maximum":BROWSER_MAX_TIMEOUT_MS,"default":BROWSER_DEFAULT_TIMEOUT_MS},
        "wait_ms":{"type":"integer","minimum":0,"maximum":BROWSER_MAX_WAIT_MS,"default":BROWSER_DEFAULT_WAIT_MS},"allow_cross_origin_navigation":{"type":"boolean","default":false},
        "include_shadow_dom":{"type":"boolean","default":false},"include_frames":{"type":"boolean","default":false},
        "capture":{"type":"object","additionalProperties":false,"properties":{
            "mime_prefixes":{"type":"array","maxItems":BROWSER_CAPTURE_MAX_MIME_PREFIXES,"items":{"type":"string","minLength":1,"maxLength":64}},
            "max_responses":{"type":"integer","minimum":1,"maximum":BROWSER_CAPTURE_MAX_RESPONSES,"default":BROWSER_CAPTURE_DEFAULT_RESPONSES},
            "max_body_bytes":{"type":"integer","minimum":BROWSER_CAPTURE_MIN_BODY_BYTES,"maximum":BROWSER_CAPTURE_MAX_BODY_BYTES,"default":BROWSER_CAPTURE_DEFAULT_BODY_BYTES},
            "same_origin_only":{"type":"boolean","default":true}
        }},
        "steps":{"type":"array","maxItems":BROWSER_MAX_STEPS,"items":{"anyOf":[
            {"type":"object","additionalProperties":false,"required":["action","selector","timeout_ms"],"properties":{"action":{"const":"wait_for"},"selector":{"type":"string","minLength":1,"maxLength":BROWSER_MAX_SELECTOR_BYTES},"timeout_ms":{"type":"integer","minimum":1,"maximum":BROWSER_MAX_WAIT_MS}}},
            {"type":"object","additionalProperties":false,"required":["action","selector"],"properties":{"action":{"const":"click"},"selector":{"type":"string","minLength":1,"maxLength":BROWSER_MAX_SELECTOR_BYTES}}},
            {"type":"object","additionalProperties":false,"required":["action","selector","value"],"properties":{"action":{"const":"fill"},"selector":{"type":"string","minLength":1,"maxLength":BROWSER_MAX_SELECTOR_BYTES},"value":{"type":"string","maxLength":BROWSER_MAX_FILL_BYTES}}},
            {"type":"object","additionalProperties":false,"required":["action","pixels"],"properties":{"action":{"const":"scroll"},"pixels":{"type":"integer","minimum":-(BROWSER_MAX_SCROLL_PIXELS as i64),"maximum":BROWSER_MAX_SCROLL_PIXELS}}},
            {"type":"object","additionalProperties":false,"required":["action","max_steps","idle_rounds","step_px"],"properties":{"action":{"const":"scroll_until_stable"},"max_steps":{"type":"integer","minimum":1,"maximum":BROWSER_SCROLL_MAX_STEPS},"idle_rounds":{"type":"integer","minimum":1,"maximum":BROWSER_SCROLL_MAX_IDLE_ROUNDS},"step_px":{"type":"integer","minimum":1,"maximum":BROWSER_MAX_SCROLL_PIXELS}}}
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
        include_shadow_dom: plan.include_shadow_dom,
        include_frames: plan.include_frames,
        capture: plan.capture.clone(),
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
    doc.final_url = page.final_url.clone();
    doc.extraction.transport = "cdp".into();
    if !page.network.is_empty() || page.scroll_stopped_reason.is_some() {
        let mut entity = serde_json::Map::new();
        if !page.network.is_empty() {
            entity.insert("type".into(), serde_json::json!("browser_capture"));
            entity.insert(
                "network".into(),
                serde_json::to_value(&page.network).map_err(TklessError::Serialization)?,
            );
        }
        if let Some(reason) = &page.scroll_stopped_reason {
            entity.insert("scroll_stopped_reason".into(), serde_json::json!(reason));
        }
        doc.entity = Some(serde_json::Value::Object(entity));
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_schema_exposes_capture_and_stable_scroll() {
        let schema = plan_schema();
        let properties = &schema["properties"];
        assert!(properties.get("capture").is_some());
        assert!(properties.get("include_shadow_dom").is_some());
        assert!(properties.get("include_frames").is_some());
        let steps = properties["steps"]["items"]["anyOf"].as_array().unwrap();
        assert!(steps
            .iter()
            .any(|variant| variant["properties"]["action"]["const"] == "scroll_until_stable"));
    }

    #[test]
    fn plan_rejects_unbounded_capture() {
        let plan = BrowserPlan {
            capture: Some(crate::cdp::NetworkCapture {
                max_responses: 1000,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(plan.validate().is_err());
    }

    #[test]
    fn plan_deserializes_new_fields() {
        let plan: BrowserPlan = serde_json::from_value(serde_json::json!({
            "include_shadow_dom": true,
            "include_frames": true,
            "capture": {"max_responses": 3}
        }))
        .unwrap();
        assert!(plan.include_shadow_dom);
        assert!(plan.include_frames);
        let capture = plan.capture.unwrap();
        assert_eq!(capture.max_responses, 3);
        assert_eq!(
            capture.max_body_bytes,
            crate::limits::BROWSER_CAPTURE_DEFAULT_BODY_BYTES
        );
        assert!(capture.same_origin_only);
    }
}
