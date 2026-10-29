//! Browser-side HEAD probe: remote browsers are measured on their own network path.
use super::browser::Browser;
use crate::{
    search_preflight::{self, Report, Settings, Status},
    TklessError,
};
use serde_json::json;
use tokio::time::timeout;

fn timing(response: &serde_json::Value) -> (Option<f64>, Option<bool>) {
    let reused = response["connectionReused"].as_bool();
    if reused == Some(true) {
        return (None, reused);
    }
    let data = &response["timing"];
    // sendStart follows proxy resolution, DNS, TCP and TLS; the CDP baseline is milliseconds.
    let connect = data["sendStart"].as_f64().filter(|value| *value >= 0.0);
    (connect, reused)
}

pub(crate) async fn check(endpoint: &str, origin: &str) -> Result<(), TklessError> {
    let settings = Settings::load()?;
    timeout(
        settings.total,
        check_with_settings(endpoint, origin, settings),
    )
    .await
    .unwrap_or_else(|_| {
        Report::unknown("Browser preflight budget expired; availability is unknown")
            .allow_search(origin, "cdp", false)
    })
}

async fn check_with_settings(
    endpoint: &str,
    origin: &str,
    settings: Settings,
) -> Result<(), TklessError> {
    let browser = Browser::connect(endpoint).await?;
    let context = browser.context_key();
    search_preflight::browser(&context, origin, settings, || async move {
        let operation = async {
            let mut page = match browser.background_page().await {
                Ok(page) => page,
                Err(_) => return Report::unknown("Browser did not create a preflight target"),
            };
            let result = async {
                page.call("Network.enable", json!({})).await?;
                page.call("Network.setCacheDisabled", json!({"cacheDisabled":true})).await?;
                // This runs in the selected browser context. No keywords, cookies or auth are sent.
                let script = format!(r#"(async()=>{{ /* tkless-connection-preflight */
                    try {{ await fetch({}, {{method:'HEAD', mode:'no-cors', credentials:'omit', redirect:'manual', cache:'no-store', signal:AbortSignal.timeout({})}}); return {{preflight:true,ok:true}}; }}
                    catch(e) {{ return {{preflight:true,ok:false}}; }}
                }})()"#, serde_json::to_string(origin).unwrap(), settings.total.as_millis());
                let value = page.call("Runtime.evaluate", json!({"expression":script,"returnByValue":true,"awaitPromise":true})).await?;
                Ok::<_, TklessError>(value["result"]["value"].clone())
            }.await;
            let events = page.network_events();
            page.close().await;
            let response = events.iter().rev().filter(|event| event["method"] == "Network.responseReceived")
                .map(|event| &event["params"]["response"]).find(|response| {
                    response["url"].as_str().and_then(|value| url::Url::parse(value).ok())
                        .is_some_and(|url| url.origin().ascii_serialization() == origin.trim_end_matches('/'))
                });
            if let Some(response) = response {
                let (connect_ms, connection_reused) = timing(response);
                let status = response["status"].as_u64().and_then(|status| u16::try_from(status).ok());
                let limited = status == Some(429);
                let retry = response["headers"].as_object().and_then(|headers| headers.iter().find(|(name,_)| name.eq_ignore_ascii_case("retry-after")))
                    .and_then(|(_,value)| value.as_str()).and_then(|value| search_preflight::retry_after(Some(value)));
                return Report { status: if limited { Status::RateLimited } else { Status::Reachable }, connect_ms,
                    response_headers_ms: response["timing"]["receiveHeadersEnd"].as_f64().filter(|value| *value >= 0.0), connection_reused,
                    http_status: status, retry_after_secs: limited.then_some(retry.unwrap_or(settings.failure_ttl.as_secs().max(1))), reason: None };
            }
            // Only explicit network failures mark the browser path unreachable. CORS, CSP and
            // an unsupported browser command are not evidence of a dead search engine.
            let failure = events.iter().rev().filter(|event| event["method"] == "Network.loadingFailed")
                .filter_map(|event| event["params"]["errorText"].as_str()).find(|error| {
                    ["ERR_NAME_NOT_RESOLVED", "ERR_CONNECTION_REFUSED", "ERR_CONNECTION_RESET", "ERR_CONNECTION_TIMED_OUT", "ERR_ADDRESS_UNREACHABLE", "ERR_INTERNET_DISCONNECTED", "ERR_PROXY_CONNECTION_FAILED", "ERR_TUNNEL_CONNECTION_FAILED", "ERR_CERT_", "ERR_SSL_"].iter().any(|known| error.contains(known))
                });
            if failure.is_some() {
                let mut report = Report::unknown("Browser DNS, proxy, TCP or TLS connection failed");
                report.status = Status::Unreachable;
                return report;
            }
            if result.as_ref().is_ok_and(|value| value["preflight"] == true && value["ok"] == true) {
                let mut report = Report::unknown("Browser connection succeeded; detailed timing unavailable");
                report.status = Status::Reachable;
                return report;
            }
            Report::unknown("Browser preflight was inconclusive; search is still allowed")
        };
        timeout(settings.total, operation).await.unwrap_or_else(|_| Report::unknown("Browser preflight timed out; search is still allowed"))
    }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reused_connections_do_not_get_a_zero_handshake_time() {
        assert_eq!(
            timing(&json!({"connectionReused":true,"timing":{"sendStart":0}})),
            (None, Some(true))
        );
        assert_eq!(
            timing(&json!({"connectionReused":false,"timing":{"sendStart":15.5}})),
            (Some(15.5), Some(false))
        );
        assert_eq!(timing(&json!({"timing":{"sendStart":-1}})), (None, None));
    }

    #[tokio::test]
    async fn hidden_target_rejection_falls_back_to_an_owned_background_tab() {
        use futures_util::{SinkExt, StreamExt};
        use std::sync::{Arc, Mutex};
        use tokio::net::TcpListener;
        use tokio_tungstenite::{accept_async, tungstenite::Message};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "ws://{}/devtools/browser/bing-preflight",
            listener.local_addr().unwrap()
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let server = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let captured = captured.clone();
                tokio::spawn(async move {
                    let mut socket = accept_async(stream).await.unwrap();
                    while let Some(Ok(Message::Text(text))) = socket.next().await {
                        let request: serde_json::Value = serde_json::from_str(&text).unwrap();
                        captured.lock().unwrap().push(request.clone());
                        let method = request["method"].as_str().unwrap();
                        let mut response = json!({"id":request["id"],"result":{}});
                        match method {
                            "Target.createTarget" if request["params"]["hidden"] == true => {
                                response = json!({"id":request["id"],"error":{"code":-32000,"message":"Hidden target can be created only when remote debugging is enabled"}});
                            }
                            "Target.createTarget" => {
                                response["result"] = json!({"targetId":"owned-background"})
                            }
                            "Target.attachToTarget" => {
                                response["result"] = json!({"sessionId":"owned-session"})
                            }
                            "Runtime.evaluate" => {
                                let event = json!({"sessionId":"owned-session","method":"Network.responseReceived","params":{"response":{"url":"https://www.bing.com/","status":200,"headers":{},"connectionReused":false,"timing":{"sendStart":12,"receiveHeadersEnd":18}}}});
                                if socket
                                    .send(Message::Text(event.to_string().into()))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                                response["result"] =
                                    json!({"result":{"value":{"preflight":true,"ok":true}}});
                            }
                            _ => {}
                        }
                        if socket
                            .send(Message::Text(response.to_string().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                });
            }
        });
        check(&endpoint, "https://www.bing.com").await.unwrap();
        check(&endpoint, "https://www.bing.com").await.unwrap();
        let requests = requests.lock().unwrap();
        let creates = requests
            .iter()
            .filter(|request| request["method"] == "Target.createTarget")
            .collect::<Vec<_>>();
        assert_eq!(
            creates.len(),
            2,
            "The next request must use the cached connectivity status"
        );
        assert_eq!(creates[0]["params"]["hidden"], true);
        assert!(creates[1]["params"].get("hidden").is_none());
        assert_eq!(creates[1]["params"]["background"], true);
        assert!(creates[1]["params"].get("newWindow").is_none());
        let closed = requests
            .iter()
            .filter(|request| request["method"] == "Target.closeTarget")
            .collect::<Vec<_>>();
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0]["params"]["targetId"], "owned-background");
        assert!(!requests
            .iter()
            .any(|request| request["method"] == "Browser.close"
                || request["method"] == "Page.navigate"));
        server.abort();
    }
}
