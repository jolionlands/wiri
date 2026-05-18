// JSON framing helpers for IPC messages and events.
// The existing code uses raw serde_json (no newline delimiter or length prefix);
// these helpers wrap that behaviour identically.
use super::messages::{IpcError, IpcEvent, IpcMessage};

/// Serialize an [`IpcMessage`] to a JSON byte vector.
pub fn serialize_message(msg: &IpcMessage) -> Result<Vec<u8>, IpcError> {
    serde_json::to_vec(msg).map_err(IpcError::from)
}

/// Deserialize an [`IpcMessage`] from a JSON byte slice.
pub fn deserialize_message(buf: &[u8]) -> Result<IpcMessage, IpcError> {
    serde_json::from_slice(buf).map_err(IpcError::from)
}

/// Serialize an [`IpcEvent`] to a JSON byte vector.
pub fn serialize_event(event: &IpcEvent) -> Result<Vec<u8>, IpcError> {
    serde_json::to_vec(event).map_err(IpcError::from)
}

/// Deserialize an [`IpcEvent`] from a JSON byte slice.
pub fn deserialize_event(buf: &[u8]) -> Result<IpcEvent, IpcError> {
    serde_json::from_slice(buf).map_err(IpcError::from)
}

#[cfg(test)]
mod tests {
    use super::super::messages::*;
    use super::*;

    #[test]
    fn test_message_serialization() {
        let msg = IpcMessage::WindowMove {
            window_hwnd: 12345,
            x: 100,
            y: 200,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"window_move\""));
        assert!(json.contains("12345"));
    }

    #[test]
    fn test_event_serialization() {
        let event = IpcEvent::WindowOpened {
            window: WindowInfoIpc {
                hwnd: 12345,
                title: "Test".to_string(),
                class_name: "TestClass".to_string(),
                process_id: 100,
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"window_opened\""));
    }

    #[test]
    fn test_focus_window_message() {
        let msg = IpcMessage::FocusWindow { window_hwnd: 99999 };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("focus_window"));
        assert!(json.contains("99999"));
    }

    #[test]
    fn test_switch_workspace_message() {
        let msg = IpcMessage::SwitchWorkspace { id: 3 };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("switch_workspace"));
        assert!(json.contains("3"));
    }

    #[test]
    fn test_all_message_types_roundtrip() {
        let messages = vec![
            IpcMessage::GetState,
            IpcMessage::WindowList,
            IpcMessage::SwitchWorkspace { id: 5 },
            IpcMessage::FocusWindow { window_hwnd: 123 },
            IpcMessage::CloseWindow { window_hwnd: 456 },
            IpcMessage::ReloadConfig,
            IpcMessage::Quit,
            IpcMessage::WindowMove { window_hwnd: 1, x: 10, y: 20 },
            IpcMessage::WindowResize { window_hwnd: 1, width: 800, height: 600 },
            IpcMessage::TileRequest { window_hwnd: 1, target_workspace: Some("ws2".to_string()) },
            IpcMessage::SubscribeEvents { event_types: vec!["focus".to_string()] },
        ];
        for msg in &messages {
            let json = serde_json::to_string(msg).unwrap();
            let parsed: IpcMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(json, serde_json::to_string(&parsed).unwrap());
        }
    }

    #[test]
    fn test_all_event_types_roundtrip() {
        let events = vec![
            IpcEvent::WindowOpened { window: WindowInfoIpc {
                hwnd: 1, title: "t".into(), class_name: "c".into(),
                process_id: 0, x: 0, y: 0, width: 100, height: 100,
            }},
            IpcEvent::WindowClosed { window_hwnd: 1 },
            IpcEvent::WindowFocused { window_hwnd: 1 },
            IpcEvent::WorkspaceActivated { workspace: "ws-0".into() },
            IpcEvent::ConfigLoaded { config_path: "test.kdl".into() },
            IpcEvent::MonitorChanged { monitor_name: "Monitor1".into() },
            IpcEvent::WorkspacesChanged { workspaces: vec![WorkspaceInfo {
                name: "ws-0".into(), id: 0, window_count: 3,
            }] },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let parsed: IpcEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(json, serde_json::to_string(&parsed).unwrap());
        }
    }

    #[test]
    fn test_state_response_serialization() {
        let resp = StateResponse {
            windows: vec![WindowInfoIpc {
                hwnd: 100, title: "Test".into(), class_name: "Class".into(),
                process_id: 999, x: 10, y: 20, width: 800, height: 600,
            }],
            workspaces: vec![WorkspaceInfo {
                name: "workspace-0".into(), id: 0, window_count: 5,
            }],
            active_workspace: Some("workspace-0".into()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: StateResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.windows.len(), 1);
        assert_eq!(parsed.workspaces.len(), 1);
        assert_eq!(parsed.active_workspace, Some("workspace-0".into()));
    }

    #[test]
    fn test_ipc_error_display() {
        let e = IpcError::ConnectionFailed("test".to_string());
        assert!(e.to_string().contains("test"));
        let e = IpcError::InvalidMessage("bad".to_string());
        assert!(e.to_string().contains("bad"));
        let e = IpcError::Timeout;
        assert!(e.to_string().contains("Timeout"));
    }

    #[test]
    fn test_serialize_message_helper() {
        let msg = IpcMessage::GetState;
        let bytes = serialize_message(&msg).unwrap();
        let parsed = deserialize_message(&bytes).unwrap();
        let bytes2 = serialize_message(&parsed).unwrap();
        assert_eq!(bytes, bytes2);
    }

    #[test]
    fn test_serialize_event_helper() {
        let event = IpcEvent::WindowClosed { window_hwnd: 42 };
        let bytes = serialize_event(&event).unwrap();
        let parsed = deserialize_event(&bytes).unwrap();
        let bytes2 = serialize_event(&parsed).unwrap();
        assert_eq!(bytes, bytes2);
    }
}
