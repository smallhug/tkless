//! Minimal HTTP JSON-RPC transport for the MCP server.
//!
//! Supports `POST /mcp` with a single JSON-RPC message per request and
//! `GET /healthz`. Server-initiated notifications are not streamed: this
//! transport is stateless and answers one request per connection.
use super::{JsonRpcResponse, McpServer, TklessError};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tracing::{debug, info};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct HttpTransportOptions {
    /// Required on non-loopback bindings; checked as a Bearer token when set.
    pub auth_token: Option<String>,
    /// Permit binding to a non-loopback address. Requires `auth_token`.
    pub allow_public_bind: bool,
    /// Maximum concurrent in-flight requests.
    pub max_in_flight: usize,
}

impl Default for HttpTransportOptions {
    fn default() -> Self {
        Self {
            auth_token: None,
            allow_public_bind: false,
            max_in_flight: 32,
        }
    }
}

impl HttpTransportOptions {
    pub fn validate(&self, addr: SocketAddr) -> Result<(), TklessError> {
        if !(1..=256).contains(&self.max_in_flight) {
            return Err(TklessError::InvalidArgument(
                "MCP HTTP max_in_flight must be 1..256".into(),
            ));
        }
        if !addr.ip().is_loopback() {
            if !self.allow_public_bind {
                return Err(TklessError::InvalidArgument(
                    "Refusing to bind MCP HTTP to a non-loopback address without --allow-public-bind"
                        .into(),
                ));
            }
            if self.auth_token.as_deref().is_none_or(str::is_empty) {
                return Err(TklessError::InvalidArgument(
                    "Non-loopback MCP HTTP binding requires a Bearer token".into(),
                ));
            }
        }
        Ok(())
    }
}

struct HttpRequest {
    method: String,
    path: String,
    authorization: Option<String>,
    body: Vec<u8>,
}

impl McpServer {
    /// Serves MCP over HTTP until the listener fails or the process exits.
    pub async fn serve_http(
        &self,
        listener: TcpListener,
        options: HttpTransportOptions,
    ) -> Result<(), TklessError> {
        let addr = listener.local_addr()?;
        options.validate(addr)?;
        info!(%addr, "MCP HTTP transport listening on POST /mcp");
        let permits = Arc::new(Semaphore::new(options.max_in_flight));
        loop {
            let (stream, peer) = listener.accept().await?;
            let server = self.clone();
            let options = options.clone();
            let permits = permits.clone();
            tokio::spawn(async move {
                let Ok(_permit) = permits.acquire_owned().await else {
                    return;
                };
                if let Err(error) = handle_connection(server, stream, &options).await {
                    debug!(peer = %peer, error = %error, "MCP HTTP request failed");
                }
            });
        }
    }
}

async fn handle_connection(
    server: McpServer,
    mut stream: TcpStream,
    options: &HttpTransportOptions,
) -> std::io::Result<()> {
    let request = match tokio::time::timeout(REQUEST_TIMEOUT, read_request(&mut stream)).await {
        Ok(Ok(request)) => request,
        Ok(Err(message)) => return write_text(&mut stream, 400, "Bad Request", &message).await,
        Err(_) => return write_text(&mut stream, 408, "Request Timeout", "").await,
    };

    if request.method == "GET" && request.path == "/healthz" {
        return write_json(
            &mut stream,
            200,
            &serde_json::json!({"status":"ok","transport":"http"}),
        )
        .await;
    }
    if request.method != "POST" || request.path != "/mcp" {
        return write_text(&mut stream, 404, "Not Found", "Use POST /mcp").await;
    }
    if request.body.is_empty() {
        return write_text(&mut stream, 400, "Bad Request", "Request body is required").await;
    }
    if let Some(expected) = &options.auth_token {
        let authorized = request
            .authorization
            .as_deref()
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|token| constant_time_eq(token.as_bytes(), expected.as_bytes()));
        if !authorized {
            return write_json(
                &mut stream,
                401,
                &serde_json::json!({"error":"unauthorized"}),
            )
            .await;
        }
    }
    let line = match String::from_utf8(request.body) {
        Ok(line) => line,
        Err(_) => return write_text(&mut stream, 400, "Bad Request", "Invalid UTF-8").await,
    };

    let response = tokio::time::timeout(
        Duration::from_secs(server.tool_timeout_secs),
        server.handle_message_with_notifier(&line, None),
    )
    .await;
    match response {
        Ok(Some(response)) => write_json(&mut stream, 200, &response).await,
        // Notifications have no JSON-RPC response.
        Ok(None) => write_text(&mut stream, 202, "Accepted", "").await,
        Err(_) => {
            let response = JsonRpcResponse::error(
                serde_json::Value::Null,
                -32001,
                format!(
                    "Request exceeded the configured {} second timeout",
                    server.tool_timeout_secs
                ),
            );
            write_json(&mut stream, 200, &response).await
        }
    }
}

async fn read_request(stream: &mut TcpStream) -> Result<HttpRequest, String> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(position) = find_header_end(&buffer) {
            break position;
        }
        if buffer.len() > MAX_HEADER_BYTES {
            return Err("Request headers exceed 16 KiB".into());
        }
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("Connection closed before headers completed".into());
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = std::str::from_utf8(&buffer[..header_end])
        .map_err(|_| "Request headers are not valid UTF-8".to_string())?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or("Missing request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("Missing method")?.to_string();
    let path = parts
        .next()
        .ok_or("Missing path")?
        .split('?')
        .next()
        .unwrap_or("/")
        .to_string();
    let mut authorization = None;
    let mut content_length = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        match name.as_str() {
            "authorization" => authorization = Some(value.to_string()),
            "content-length" => {
                content_length = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| "Invalid Content-Length".to_string())?,
                )
            }
            "transfer-encoding" => {
                return Err("Chunked request bodies are not supported".into());
            }
            _ => {}
        }
    }
    let content_length = content_length.unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        return Err("Request body exceeds 1 MiB".into());
    }
    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("Connection closed before the body completed".into());
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    if content_length == 0 {
        return Ok(HttpRequest {
            method,
            path,
            authorization,
            body: Vec::new(),
        });
    }
    Ok(HttpRequest {
        method,
        path,
        authorization,
        body: buffer[body_start..body_start + content_length].to_vec(),
    })
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        difference |= a ^ b;
    }
    difference == 0
}

async fn write_text(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    message: &str,
) -> std::io::Result<()> {
    let body = message.as_bytes();
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.shutdown().await
}

async fn write_json<T: serde::Serialize>(
    stream: &mut TcpStream,
    status: u16,
    value: &T,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        202 => "Accepted",
        _ => "OK",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_binding_requires_opt_in_and_token() {
        let public = SocketAddr::from(([0, 0, 0, 0], 8787));
        assert!(HttpTransportOptions::default().validate(public).is_err());
        assert!(HttpTransportOptions {
            allow_public_bind: true,
            ..Default::default()
        }
        .validate(public)
        .is_err());
        assert!(HttpTransportOptions {
            allow_public_bind: true,
            auth_token: Some("secret".into()),
            ..Default::default()
        }
        .validate(public)
        .is_ok());

        let loopback = SocketAddr::from(([127, 0, 0, 1], 8787));
        assert!(HttpTransportOptions::default().validate(loopback).is_ok());
    }

    #[test]
    fn bearer_comparison_is_exact() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
