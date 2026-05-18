// IPC client helpers.
// wiri-ctl is the only consumer and uses raw pipe I/O directly.
// IpcClient below is a convenience wrapper used by library consumers.
use tracing::info;

use super::messages::{IpcError, IpcMessage, StateResponse, WindowInfoIpc};
use super::PIPE_PATH;

pub struct IpcClient {
    pipe: Option<tokio::net::windows::named_pipe::NamedPipeClient>,
}

impl IpcClient {
    pub fn new() -> Self {
        Self { pipe: None }
    }

    pub async fn connect(&mut self) -> Result<(), IpcError> {
        use tokio::net::windows::named_pipe::ClientOptions;

        let pipe = ClientOptions::new()
            .open(PIPE_PATH)
            .map_err(|e| IpcError::ConnectionFailed(e.to_string()))?;

        self.pipe = Some(pipe);
        info!("IPC client connected to {}", PIPE_PATH);
        Ok(())
    }

    pub async fn send_message(&mut self, message: &IpcMessage) -> Result<serde_json::Value, IpcError> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let pipe = self.pipe.as_mut().ok_or(IpcError::ClientDisconnected)?;

        let data = serde_json::to_vec(message)?;
        pipe.write_all(&data)
            .await
            .map_err(|e| IpcError::WriteError(e.to_string()))?;

        // Read response
        let mut buffer = vec![0u8; 65536];
        let n = pipe
            .read(&mut buffer)
            .await
            .map_err(|e| IpcError::ReadError(e.to_string()))?;

        let response: serde_json::Value = serde_json::from_slice(&buffer[..n])?;
        Ok(response)
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
        self.send_message(&IpcMessage::SwitchWorkspace { id }).await?;
        Ok(())
    }

    pub async fn close_window(&mut self, hwnd: isize) -> Result<(), IpcError> {
        self.send_message(&IpcMessage::CloseWindow { window_hwnd: hwnd }).await?;
        Ok(())
    }
}

impl Default for IpcClient {
    fn default() -> Self {
        Self::new()
    }
}
