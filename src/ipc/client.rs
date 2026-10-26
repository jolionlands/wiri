//! Reusable newline-framed IPC client for library callers and wiri-ctl.
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::NamedPipeClient;
use tracing::info;

use super::messages::{IpcError, IpcMessage, StateResponse, WindowInfoIpc};
use super::pipe_path;

pub struct IpcClient {
    // Keep the reader across requests: a read can also buffer the next reply
    // or an event following a subscription acknowledgement.
    pipe: Option<BufReader<NamedPipeClient>>,
}

/// Read one line without imposing a reply-size limit. Old daemons may close
/// after a reply without a newline; that final, nonempty line is valid too.
async fn read_line<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<String>, IpcError> {
    let mut bytes = Vec::new();
    match reader.read_until(b'\n', &mut bytes).await {
        Ok(_) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof
            ) => {}
        Err(error) => return Err(IpcError::ReadError(error.to_string())),
    }
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| IpcError::ReadError(error.to_string()))
}

async fn request_on<S: AsyncRead + AsyncWrite + Unpin>(
    pipe: &mut BufReader<S>,
    request: &str,
) -> Result<String, IpcError> {
    // Accept pretty-printed input too, but send exactly one JSON line. This
    // also prevents raw input from accidentally injecting a second request.
    let value: serde_json::Value = serde_json::from_str(request)?;
    let mut data = serde_json::to_vec(&value)?;
    data.push(b'\n');
    pipe.get_mut()
        .write_all(&data)
        .await
        .map_err(|error| IpcError::WriteError(error.to_string()))?;
    pipe.get_mut()
        .flush()
        .await
        .map_err(|error| IpcError::WriteError(error.to_string()))?;
    read_line(pipe).await?.ok_or(IpcError::ClientDisconnected)
}

impl IpcClient {
    pub fn new() -> Self {
        Self { pipe: None }
    }

    pub async fn connect(&mut self) -> Result<(), IpcError> {
        use tokio::net::windows::named_pipe::ClientOptions;

        let path = pipe_path();
        let pipe = ClientOptions::new().open(&path).map_err(|error| {
            IpcError::ConnectionFailed(format!("{} — is wiri running? ({})", path, error))
        })?;
        self.pipe = Some(BufReader::new(pipe));
        info!("IPC client connected to {}", path);
        Ok(())
    }

    /// Wait for an available pipe using the existing Windows connection wait,
    /// without spawning a thread or installing an async timer. The wait blocks
    /// the calling thread; callers that do not want this can use `connect`.
    pub async fn connect_with_timeout(&mut self, timeout_secs: u64) -> Result<(), IpcError> {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::System::Pipes::WaitNamedPipeW;

        let path = pipe_path();
        let wide: Vec<u16> = OsStr::new(&path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let timeout_ms = timeout_secs.saturating_mul(1000).min(u32::MAX as u64) as u32;
        if !unsafe { WaitNamedPipeW(PCWSTR::from_raw(wide.as_ptr()), timeout_ms) }.as_bool() {
            let error = std::io::Error::last_os_error();
            let hint = match error.raw_os_error() {
                Some(121) => format!(
                    "wiri busy: pipe wait timed out after {}s at {}. Try increasing --timeout, or restart wiri.",
                    timeout_secs, path
                ),
                Some(2) => format!(
                    "Could not find the wiri IPC pipe at {} — is wiri running?",
                    path
                ),
                _ => format!("Could not connect to wiri IPC pipe at {}: {}", path, error),
            };
            return Err(IpcError::ConnectionFailed(hint));
        }
        // Another client can take the instance the wait reported free before this one opens it
        // (ERROR_PIPE_BUSY; two `events --stream` started at the same moment on a hosted CI
        // runner hit this): wait for the next instance until the deadline.
        use tokio::net::windows::named_pipe::ClientOptions;
        const ERROR_PIPE_BUSY: i32 = 231;
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(u64::from(timeout_ms));
        loop {
            match ClientOptions::new().open(&path) {
                Ok(pipe) => {
                    self.pipe = Some(BufReader::new(pipe));
                    info!("IPC client connected to {}", path);
                    return Ok(());
                }
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    let left = deadline.saturating_duration_since(std::time::Instant::now());
                    if left.is_zero() {
                        return Err(IpcError::ConnectionFailed(format!(
                            "{} — every pipe instance stayed busy for {}s ({})",
                            path, timeout_secs, error
                        )));
                    }
                    let wait_ms = left.as_millis().clamp(1, u128::from(u32::MAX)) as u32;
                    if !unsafe { WaitNamedPipeW(PCWSTR::from_raw(wide.as_ptr()), wait_ms) }
                        .as_bool()
                    {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
                Err(error) => {
                    return Err(IpcError::ConnectionFailed(format!(
                        "{} — is wiri running? ({})",
                        path, error
                    )))
                }
            }
        }
    }

    /// Send a typed wiri request and consume exactly one reply, leaving the
    /// connection open for further requests or events. Schema hints belong to
    /// the CLI, not this transport (which also supports niri's raw dialect).
    pub async fn request(&mut self, message: &IpcMessage) -> Result<serde_json::Value, IpcError> {
        let response = self.request_raw(&serde_json::to_string(message)?).await?;
        Ok(serde_json::from_str(&response)?)
    }

    /// Send either IPC dialect and return the raw reply line, without its line
    /// terminator. No schema checks or rewriting are applied to the reply.
    pub async fn request_raw(&mut self, request: &str) -> Result<String, IpcError> {
        let pipe = self.pipe.as_mut().ok_or(IpcError::ClientDisconnected)?;
        request_on(pipe, request).await
    }

    /// Read the next event after a successful SubscribeEvents request. Repeat
    /// until `None` (EOF); buffered events following the ack are retained.
    pub async fn read_events(&mut self) -> Result<Option<serde_json::Value>, IpcError> {
        let pipe = self.pipe.as_mut().ok_or(IpcError::ClientDisconnected)?;
        read_line(pipe)
            .await?
            .map(|line| serde_json::from_str(&line).map_err(IpcError::from))
            .transpose()
    }

    /// Backwards-compatible name for `request`.
    pub async fn send_message(
        &mut self,
        message: &IpcMessage,
    ) -> Result<serde_json::Value, IpcError> {
        self.request(message).await
    }

    pub async fn get_state(&mut self) -> Result<StateResponse, IpcError> {
        let response = self.send_message(&IpcMessage::GetState).await?;
        let state: StateResponse = serde_json::from_value(
            response
                .get("result")
                .ok_or(IpcError::InvalidMessage("No result field".to_string()))?
                .clone(),
        )?;
        Ok(state)
    }

    pub async fn get_window_list(&mut self) -> Result<Vec<WindowInfoIpc>, IpcError> {
        let response = self.send_message(&IpcMessage::WindowList).await?;
        let windows: Vec<WindowInfoIpc> = serde_json::from_value(
            response
                .get("result")
                .ok_or(IpcError::InvalidMessage("No result field".to_string()))?
                .clone(),
        )?;
        Ok(windows)
    }

    pub async fn switch_workspace(&mut self, id: i32) -> Result<(), IpcError> {
        self.send_message(&IpcMessage::SwitchWorkspace { id })
            .await?;
        Ok(())
    }

    pub async fn close_window(&mut self, hwnd: isize) -> Result<(), IpcError> {
        self.send_message(&IpcMessage::CloseWindow { window_hwnd: hwnd })
            .await?;
        Ok(())
    }
}

impl Default for IpcClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{read_line, request_on, IpcClient, IpcError};
    use tokio::io::{AsyncWriteExt, BufReader};

    fn run(future: impl std::future::Future<Output = ()>) {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future);
    }

    #[test]
    fn fragmented_large_reply_and_buffered_next_line() {
        run(async {
            let title = "x".repeat(200_000);
            let reply = serde_json::json!({"result": {"windows": [{"title": title}]}});
            let bytes = format!("{}\r\n{{\"next\":true}}\n", reply);
            let mut reader = BufReader::with_capacity(7, bytes.as_bytes());
            let line = read_line(&mut reader).await.unwrap().unwrap();
            assert!(line.len() > 65_536);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&line).unwrap(),
                reply
            );
            assert_eq!(
                read_line(&mut reader).await.unwrap().unwrap(),
                "{\"next\":true}"
            );
            assert!(read_line(&mut reader).await.unwrap().is_none());
        });
    }

    #[test]
    fn legacy_eof_is_a_reply_but_empty_eof_is_not() {
        run(async {
            let mut reader = BufReader::with_capacity(3, &b"{\"success\":true}"[..]);
            assert_eq!(
                read_line(&mut reader).await.unwrap().unwrap(),
                "{\"success\":true}"
            );
            assert!(read_line(&mut reader).await.unwrap().is_none());
        });
    }

    #[test]
    fn persistent_requests_send_newlines_and_consume_only_one_reply() {
        run(async {
            let (client, server) = tokio::io::duplex(4096);
            let mut client = BufReader::new(client);
            let mut server = BufReader::new(server);
            let replies = "{ \"success\": true }\n{\"Ok\":\"1.3\"}\n";
            let ((), ()) = tokio::join!(
                async {
                    assert_eq!(
                        request_on(&mut client, "{\n  \"type\": \"windows\"\n}")
                            .await
                            .unwrap(),
                        "{ \"success\": true }"
                    );
                    assert_eq!(
                        request_on(&mut client, "\"Version\"").await.unwrap(),
                        "{\"Ok\":\"1.3\"}"
                    );
                },
                async {
                    assert_eq!(
                        read_line(&mut server).await.unwrap().unwrap(),
                        "{\"type\":\"windows\"}"
                    );
                    server
                        .get_mut()
                        .write_all(replies.as_bytes())
                        .await
                        .unwrap();
                    assert_eq!(
                        read_line(&mut server).await.unwrap().unwrap(),
                        "\"Version\""
                    );
                }
            );
        });
    }

    #[test]
    fn subscription_ack_does_not_discard_events_in_the_same_read() {
        run(async {
            let (client, server) = tokio::io::duplex(4096);
            let mut client = BufReader::new(client);
            let mut server = BufReader::new(server);
            tokio::join!(
                async {
                    let ack = request_on(&mut client, "\"EventStream\"").await.unwrap();
                    assert_eq!(ack, "{\"subscription_id\":1}");
                    assert_eq!(
                        read_line(&mut client).await.unwrap().unwrap(),
                        "{\"type\":\"window_opened\"}"
                    );
                    assert!(read_line(&mut client).await.unwrap().is_none());
                },
                async {
                    assert_eq!(
                        read_line(&mut server).await.unwrap().unwrap(),
                        "\"EventStream\""
                    );
                    server
                        .get_mut()
                        .write_all(b"{\"subscription_id\":1}\n{\"type\":\"window_opened\"}\n")
                        .await
                        .unwrap();
                    drop(server);
                }
            );
        });
    }

    #[test]
    fn empty_reply_and_invalid_raw_input_are_errors() {
        run(async {
            let (client, server) = tokio::io::duplex(4096);
            let mut client = BufReader::new(client);
            assert!(matches!(
                request_on(&mut client, "\"Windows\"\n\"Version\"").await,
                Err(IpcError::SerializationError(_))
            ));
            tokio::join!(
                async {
                    assert!(matches!(
                        request_on(&mut client, "\"Windows\"").await,
                        Err(IpcError::ClientDisconnected)
                    ));
                },
                async {
                    let mut server = BufReader::new(server);
                    assert_eq!(
                        read_line(&mut server).await.unwrap().unwrap(),
                        "\"Windows\""
                    );
                    drop(server);
                }
            );
        });
    }

    #[test]
    fn disconnected_client_does_not_fabricate_success() {
        run(async {
            let mut client = IpcClient::new();
            assert!(matches!(
                client.request_raw("\"Windows\"").await,
                Err(IpcError::ClientDisconnected)
            ));
            assert!(matches!(
                client.read_events().await,
                Err(IpcError::ClientDisconnected)
            ));
        });
    }
}
