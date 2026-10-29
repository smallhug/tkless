//! Browser-level CDP sessions with explicit ownership of task-created pages.
use super::normalize_cdp_endpoint;
use crate::TklessError;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

struct Connection {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    events: Vec<Value>,
}

impl Connection {
    async fn call(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, TklessError> {
        let id = self.next_id;
        self.next_id += 1;
        let mut request = json!({"id":id,"method":method,"params":params});
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        timeout(Duration::from_secs(15), async {
            self.socket
                .send(Message::Text(request.to_string().into()))
                .await
                .map_err(cdp_error)?;
            while let Some(message) = self.socket.next().await {
                match message.map_err(cdp_error)? {
                    Message::Text(text) => {
                        let value: Value = serde_json::from_str(&text).map_err(cdp_error)?;
                        if session.is_some()
                            && value["sessionId"].as_str() == session
                            && matches!(
                                value["method"].as_str(),
                                Some("Network.responseReceived" | "Network.loadingFailed")
                            )
                        {
                            if self.events.len() >= 32 {
                                self.events.remove(0);
                            }
                            self.events.push(value.clone());
                        }
                        if value["id"].as_u64() == Some(id) {
                            if let Some(error) = value.get("error") {
                                return Err(cdp_error(format!("{method}: {error}")));
                            }
                            if let Some(error) = value["result"].get("exceptionDetails") {
                                return Err(cdp_error(format!("{method}: {error}")));
                            }
                            return Ok(value["result"].clone());
                        }
                        if value["method"] == "Target.detachedFromTarget"
                            && value["params"]["sessionId"].as_str() == session
                        {
                            return Err(cdp_error("Task page was closed or disconnected"));
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            Err(cdp_error(
                "Browser connection closed; task may have been cancelled",
            ))
        })
        .await
        .map_err(|_| cdp_error(format!("CDP command timed out: {method}")))?
    }
}

pub(super) struct Browser {
    connection: Connection,
    context: Option<String>,
    identity: String,
}

impl Browser {
    pub fn context_key(&self) -> String {
        format!("{}|{:?}", self.identity, self.context)
    }
    pub async fn connect(endpoint: &str) -> Result<Self, TklessError> {
        let (endpoint, direct) = normalize_cdp_endpoint(endpoint);
        let websocket = if direct {
            endpoint.clone()
        } else {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()?;
            let value: Value = client
                .get(format!("{endpoint}/json/version"))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            value["webSocketDebuggerUrl"]
                .as_str()
                .ok_or_else(|| cdp_error("Browser endpoint did not return a WebSocket URL"))?
                .to_owned()
        };
        let (socket, _) = timeout(Duration::from_secs(5), connect_async(&websocket))
            .await
            .map_err(|_| cdp_error("Browser WebSocket connection timed out"))?
            .map_err(cdp_error)?;
        let mut connection = Connection {
            socket,
            next_id: 1,
            events: vec![],
        };
        connection
            .call("Browser.getVersion", json!({}), None)
            .await?;
        let mut context = None;
        if direct {
            if let Ok(url) = url::Url::parse(&endpoint) {
                if url.path().contains("/devtools/page/") {
                    if let Some(id) = url.path_segments().and_then(|mut parts| parts.next_back()) {
                        let info = connection
                            .call("Target.getTargetInfo", json!({"targetId":id}), None)
                            .await?;
                        context = info["targetInfo"]["browserContextId"]
                            .as_str()
                            .map(str::to_owned);
                    }
                }
            }
        }
        Ok(Self {
            connection,
            context,
            identity: websocket,
        })
    }

    pub async fn background_page(mut self) -> Result<Page, TklessError> {
        let mut params = json!({"url":"about:blank","hidden":true,"background":true});
        if let Some(context) = &self.context {
            params["browserContextId"] = json!(context);
        }
        let target = match self
            .connection
            .call("Target.createTarget", params.clone(), None)
            .await
        {
            Ok(target) => target,
            Err(error)
                if error.to_string().contains(
                    "Hidden target can be created only when remote debugging is enabled",
                ) =>
            {
                // Some Chromium sessions expose CDP before a normal frame target exists.
                // The ordinary background target remains task-owned and is cleaned up identically.
                params.as_object_mut().unwrap().remove("hidden");
                self.connection
                    .call("Target.createTarget", params, None)
                    .await?
            }
            Err(error) => return Err(error),
        };
        let id = target["targetId"]
            .as_str()
            .ok_or_else(|| cdp_error("Browser did not create a task target"))?
            .to_owned();
        // Establish ownership before attaching, so failed attachment/cancellation also cleans up.
        let mut page = Page {
            connection: Some(self.connection),
            id,
            session: String::new(),
        };
        let attached = page
            .connection
            .as_mut()
            .unwrap()
            .call(
                "Target.attachToTarget",
                json!({"targetId":page.id,"flatten":true}),
                None,
            )
            .await?;
        page.session = attached["sessionId"]
            .as_str()
            .ok_or_else(|| cdp_error("Browser did not return a target session"))?
            .to_owned();
        Ok(page)
    }
}

pub(super) struct Page {
    connection: Option<Connection>,
    id: String,
    session: String,
}

impl Page {
    pub fn network_events(&mut self) -> Vec<Value> {
        self.connection
            .as_mut()
            .map(|connection| std::mem::take(&mut connection.events))
            .unwrap_or_default()
    }
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, TklessError> {
        self.connection
            .as_mut()
            .ok_or_else(|| cdp_error("Task page is closed"))?
            .call(method, params, Some(&self.session))
            .await
    }

    pub async fn close(&mut self) {
        if let Some(mut connection) = self.connection.take() {
            let _ = timeout(
                Duration::from_secs(2),
                connection.call("Target.closeTarget", json!({"targetId":self.id}), None),
            )
            .await;
        }
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        if let (Some(mut connection), Ok(runtime)) = (
            self.connection.take(),
            tokio::runtime::Handle::try_current(),
        ) {
            let id = self.id.clone();
            runtime.spawn(async move {
                let _ = timeout(
                    Duration::from_secs(2),
                    connection.call("Target.closeTarget", json!({"targetId":id}), None),
                )
                .await;
            });
        }
    }
}

fn cdp_error(error: impl std::fmt::Display) -> TklessError {
    TklessError::CdpError(error.to_string())
}
