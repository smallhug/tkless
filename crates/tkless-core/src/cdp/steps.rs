use super::*;
use crate::limits::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrowserStep {
    WaitFor { selector: String, timeout_ms: u64 },
    Click { selector: String },
    Fill { selector: String, value: String },
    Scroll { pixels: i32 },
}

pub(crate) fn validate(steps: &[BrowserStep]) -> Result<(), TklessError> {
    if steps.len() > BROWSER_MAX_STEPS {
        return Err(TklessError::InvalidArgument(
            "At most 32 browser steps are supported".into(),
        ));
    }
    for step in steps {
        let selector = match step {
            BrowserStep::WaitFor {
                selector,
                timeout_ms,
            } => {
                if !(1..=BROWSER_MAX_WAIT_MS).contains(timeout_ms) {
                    return Err(TklessError::InvalidArgument(
                        "wait_for.timeout_ms must be 1..30000".into(),
                    ));
                }
                Some(selector)
            }
            BrowserStep::Click { selector } => Some(selector),
            BrowserStep::Fill { selector, value } => {
                if value.len() > BROWSER_MAX_FILL_BYTES {
                    return Err(TklessError::InvalidArgument(
                        "Fill value exceeds 16 KiB".into(),
                    ));
                }
                Some(selector)
            }
            BrowserStep::Scroll { pixels } => {
                if pixels.unsigned_abs() > BROWSER_MAX_SCROLL_PIXELS {
                    return Err(TklessError::InvalidArgument(
                        "Scroll distance exceeds 10000 pixels".into(),
                    ));
                }
                None
            }
        };
        if selector.is_some_and(|s| s.trim().is_empty() || s.len() > BROWSER_MAX_SELECTOR_BYTES) {
            return Err(TklessError::InvalidArgument(
                "Browser selector must contain 1..1024 bytes".into(),
            ));
        }
    }
    Ok(())
}

async fn evaluate(
    ws: &mut BrowserSocket,
    guard: &mut BrowserRequestGuard,
    id: &mut u64,
    expression: String,
) -> Result<Value, TklessError> {
    *id += 1;
    let current = *id;
    send_cdp_cmd(
        ws,
        current,
        "Runtime.evaluate",
        json!({"expression":expression,"returnByValue":true,"awaitPromise":true}),
    )
    .await?;
    timeout(Duration::from_secs(10), async {
        while let Some(message) = guard.next(ws).await {
            let message = message.map_err(|e| TklessError::CdpError(e.to_string()))?;
            if let Message::Text(text) = message {
                let value: Value = serde_json::from_str(&text).map_err(|e| TklessError::CdpError(e.to_string()))?;
                if value.get("id").and_then(Value::as_u64) == Some(current) {
                    if value.get("error").is_some() || value["result"].get("exceptionDetails").is_some() {
                        return Err(TklessError::CdpError("Browser step failed: selector missing, ambiguous, invalid, or action not permitted".into()));
                    }
                    return Ok(value["result"]["result"]["value"].clone());
                }
            }
        }
        Err(TklessError::CdpError("Browser disconnected during step".into()))
    }).await.map_err(|_| TklessError::CdpError("Browser step timed out".into()))?
}

pub(super) async fn execute(
    ws: &mut BrowserSocket,
    guard: &mut BrowserRequestGuard,
    id: &mut u64,
    steps: &[BrowserStep],
) -> Result<(), TklessError> {
    validate(steps)?;
    for (index, step) in steps.iter().enumerate() {
        let result = async {
            match step {
                BrowserStep::WaitFor { selector, timeout_ms } => {
                    let expression = format!("(() => {{ const e=document.querySelector({}); return !!e && e.getClientRects().length>0; }})()", serde_json::to_string(selector).unwrap());
                    timeout(Duration::from_millis(*timeout_ms), async {
                        loop {
                            if evaluate(ws, guard, id, expression.clone()).await? == true { return Ok::<_, TklessError>(()); }
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                    }).await.map_err(|_| TklessError::CdpError("wait_for timed out".into()))??;
                }
                BrowserStep::Scroll { pixels } => { evaluate(ws, guard, id, format!("window.scrollBy(0,{pixels}); true")).await?; }
                BrowserStep::Click { selector } | BrowserStep::Fill { selector, .. } => {
                    let selector = serde_json::to_string(selector).unwrap();
                    let action = match step {
                        BrowserStep::Fill { value, .. } => format!("if (!(e instanceof HTMLTextAreaElement) && !(e instanceof HTMLInputElement && ['text','search','url','email','tel','number'].includes(e.type))) throw Error('unsupported input'); const p=e instanceof HTMLTextAreaElement?HTMLTextAreaElement.prototype:HTMLInputElement.prototype; Object.getOwnPropertyDescriptor(p,'value').set.call(e,{}); e.dispatchEvent(new Event('input',{{bubbles:true}})); e.dispatchEvent(new Event('change',{{bubbles:true}}));", serde_json::to_string(value).unwrap()),
                        _ => "if (!e.matches('a[href],button,summary,[role=button]') || (e instanceof HTMLButtonElement && e.type==='submit' && e.form)) throw Error('unsupported click'); if(e instanceof HTMLAnchorElement && (e.hasAttribute('download') || (e.target && e.target!=='_self') || !['http:','https:'].includes(e.protocol))) throw Error('unsupported link'); e.click();".into(),
                    };
                    let expression = format!("(() => {{ const all=document.querySelectorAll({selector}); if(all.length!==1) throw Error('ambiguous selector'); const e=all[0]; if(e.disabled || !e.getClientRects().length) throw Error('unavailable'); {action} return true; }})()");
                    evaluate(ws, guard, id, expression).await?;
                }
            }
            if let Some(url) = &guard.blocked_navigation { return Err(TklessError::CdpError(format!("Blocked navigation: {url}"))); }
            Ok::<_, TklessError>(())
        }.await;
        result.map_err(|e| TklessError::CdpError(format!("Step {} failed: {e}", index + 1)))?;
    }
    // Separate the final snapshot command ID from the last action response.
    *id += 1;
    Ok(())
}
