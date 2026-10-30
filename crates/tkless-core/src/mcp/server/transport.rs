use super::{JsonRpcResponse, McpServer, TklessError, Value, INTERNAL_ERROR, INVALID_REQUEST};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

impl McpServer {
    /// Transport-independent entry point for embedded hosts and protocol regression tests.
    pub async fn run_transport<R, W>(&self, input: R, mut output: W) -> Result<(), TklessError>
    where
        R: tokio::io::AsyncRead + Unpin + Send,
        W: tokio::io::AsyncWrite + Unpin + Send,
    {
        let mut input = BufReader::new(input);
        let mut line_buffer = Vec::new();
        let (notify_tx, mut notify_rx) = tokio::sync::mpsc::channel::<Value>(8);
        let mut tasks = tokio::task::JoinSet::new();
        let mut active = std::collections::HashMap::<String, tokio::task::AbortHandle>::new();
        let mut eof = false;
        loop {
            if eof && tasks.is_empty() {
                while let Ok(notification) = notify_rx.try_recv() {
                    write_rpc(&mut output, &notification).await?;
                }
                break;
            }
            tokio::select! {
                line = read_bounded_line(&mut input, &mut line_buffer), if !eof => {
                    let Some(line) = line? else { eof = true; continue; };
                    let value = serde_json::from_str::<Value>(line.trim().trim_matches('\u{feff}')).ok();
                    if value.as_ref().and_then(|v| v.get("method")).and_then(Value::as_str) == Some("notifications/cancelled") {
                        if let Some(id) = value.as_ref().and_then(|v| v.pointer("/params/requestId")) {
                            if let Some(task) = active.remove(&id.to_string()) { task.abort(); }
                        }
                        continue;
                    }
                    let id = value.as_ref().and_then(|v| v.get("id")).cloned();
                    let tool_call = value.as_ref().and_then(|v| v.get("method")).and_then(Value::as_str) == Some("tools/call");
                    if let (true, Some(id)) = (tool_call, id) {
                        let key = id.to_string();
                        if active.contains_key(&key) || active.len() >= 64 {
                            let response = JsonRpcResponse::error(id, INVALID_REQUEST, "Duplicate request ID or too many concurrent requests (64)");
                            write_rpc(&mut output, &response).await?;
                            continue;
                        }
                        let server = self.clone();
                        let tx = notify_tx.clone();
                        let task_key = key.clone();
                        let task = tasks.spawn(async move {
                            let response = match tokio::time::timeout(std::time::Duration::from_secs(300), server.handle_message_with_notifier(&line, Some(&tx))).await {
                                Ok(response) => response,
                                Err(_) => Some(JsonRpcResponse::error(id, -32001, "Tool call exceeded the 300 second timeout")),
                            };
                            (task_key, response)
                        });
                        active.insert(key, task);
                    } else if let Some(response) = self.handle_message_with_notifier(&line, Some(&notify_tx)).await {
                        write_rpc(&mut output, &response).await?;
                    }
                }
                joined = tasks.join_next_with_id(), if !tasks.is_empty() => {
                    if let Some(Ok((task_id, (key, response)))) = joined {
                        if active.get(&key).is_some_and(|handle| handle.id() == task_id) {
                            active.remove(&key);
                            if let Some(response) = response {
                                let pending = notify_rx.len();
                                for _ in 0..pending {
                                    if let Ok(notification) = notify_rx.try_recv() { write_rpc(&mut output, &notification).await?; }
                                }
                                write_rpc(&mut output, &response).await?;
                            }
                        }
                    }
                }
                Some(notification) = notify_rx.recv() => { write_rpc(&mut output, &notification).await?; }
            }
        }
        Ok(())
    }
}

// Buffer lives outside this future: select! cancellation must not lose a partial message.
async fn read_bounded_line<R: tokio::io::AsyncBufRead + Unpin>(
    input: &mut R,
    buffer: &mut Vec<u8>,
) -> std::io::Result<Option<String>> {
    const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
    loop {
        let available = input.fill_buf().await?;
        if available.is_empty() {
            if buffer.is_empty() {
                return Ok(None);
            }
            break;
        }
        let newline = available.iter().position(|&b| b == b'\n');
        let count = newline.map_or(available.len(), |i| i + 1);
        if buffer.len().saturating_add(count) > MAX_MESSAGE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "MCP message exceeds 1 MiB",
            ));
        }
        buffer.extend_from_slice(&available[..count]);
        input.consume(count);
        if newline.is_some() {
            break;
        }
    }
    String::from_utf8(std::mem::take(buffer))
        .map(Some)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

async fn write_rpc<W: tokio::io::AsyncWrite + Unpin, T: serde::Serialize>(
    output: &mut W,
    value: &T,
) -> Result<(), TklessError> {
    let mut line = serde_json::to_vec(value).map_err(|e| TklessError::McpError {
        code: INTERNAL_ERROR,
        message: e.to_string(),
    })?;
    line.push(b'\n');
    output.write_all(&line).await?;
    output.flush().await?;
    Ok(())
}
