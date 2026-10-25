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
            IpcMessage::WindowMove {
                window_hwnd: 1,
                x: 10,
                y: 20,
            },
            IpcMessage::WindowResize {
                window_hwnd: 1,
                width: 800,
                height: 600,
            },
            IpcMessage::TileRequest {
                window_hwnd: 1,
                target_workspace: Some("ws2".to_string()),
            },
            IpcMessage::SubscribeEvents {
                event_types: vec!["focus".to_string()],
            },
            // MonitorList is a new variant — make sure it round-trips
            // through the serde tagged-enum format unchanged.
            IpcMessage::MonitorList,
        ];
        for msg in &messages {
            let json = serde_json::to_string(msg).unwrap();
            let parsed: IpcMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(json, serde_json::to_string(&parsed).unwrap());
        }
    }

    /// `MonitorInfo` survives a JSON round-trip with every field preserved.
    /// This is the wire payload returned by `MonitorList`.
    #[test]
    fn test_monitor_info_serialization() {
        let m = crate::ipc::MonitorInfo {
            output_id: 0xDEAD_BEEF_CAFE_F00D,
            name: "\\\\.\\DISPLAY2".to_string(),
            bounds_x: 0,
            bounds_y: 0,
            bounds_width: 2560,
            bounds_height: 1440,
            work_area_x: 0,
            work_area_y: 0,
            work_area_width: 2560,
            work_area_height: 1392,
            scale_factor: 1.5,
            active_workspace: 3,
            window_count: 7,
            focused: true,
        };
        let json = serde_json::to_string(&m).unwrap();
        let parsed: crate::ipc::MonitorInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.output_id, m.output_id);
        assert_eq!(parsed.name, "\\\\.\\DISPLAY2");
        assert_eq!(parsed.bounds_width, m.bounds_width);
        assert_eq!(parsed.work_area_height, m.work_area_height);
        assert!((parsed.scale_factor - m.scale_factor).abs() < f64::EPSILON);
        assert_eq!(parsed.active_workspace, m.active_workspace);
        assert_eq!(parsed.window_count, m.window_count);
        assert!(parsed.focused);
    }

    #[test]
    fn test_all_event_types_roundtrip() {
        let events = vec![
            IpcEvent::WindowOpened {
                window: WindowInfoIpc {
                    hwnd: 1,
                    title: "t".into(),
                    class_name: "c".into(),
                    process_id: 0,
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                },
            },
            IpcEvent::WindowClosed {
                window_hwnd: 1,
                id: 1,
            },
            IpcEvent::WindowFocused { window_hwnd: 1 },
            IpcEvent::WorkspaceActivated {
                id: 4_294_967_296,
                focused: true,
            },
            IpcEvent::ConfigLoaded {
                config_path: "test.kdl".into(),
                failed: true,
                error: Some("test.kdl:3:5: error: unknown option 'x' in 'layout'".into()),
                warnings: 2,
            },
            IpcEvent::MonitorChanged {
                monitor_name: "Monitor1".into(),
            },
            IpcEvent::WorkspacesChanged {
                workspaces: vec![WorkspaceIpc {
                    id: 4_294_967_296,
                    idx: 1,
                    name: Some("web".into()),
                    output: Some("\\\\.\\DISPLAY1".into()),
                    is_urgent: false,
                    is_active: true,
                    is_focused: true,
                    active_window_id: Some(1),
                }],
            },
            IpcEvent::WorkspaceUrgencyChanged {
                id: 4_294_967_296,
                urgent: true,
            },
            IpcEvent::WorkspaceActiveWindowChanged {
                workspace_id: 4_294_967_296,
                active_window_id: None,
            },
            IpcEvent::WindowsChanged {
                windows: vec![sample_window()],
            },
            IpcEvent::WindowOpenedOrChanged {
                window: sample_window(),
            },
            IpcEvent::WindowFocusChanged { id: Some(1) },
            IpcEvent::WindowFocusChanged { id: None },
            IpcEvent::WindowUrgencyChanged { id: 1, urgent: true },
            IpcEvent::WindowLayoutsChanged {
                changes: vec![(
                    1,
                    WindowLayoutIpc {
                        pos_in_scrolling_layout: Some((2, 1)),
                    },
                )],
            },
            IpcEvent::OverviewOpenedOrClosed { is_open: true },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let parsed: IpcEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(json, serde_json::to_string(&parsed).unwrap());
        }
    }

    fn sample_window() -> WindowIpc {
        WindowIpc {
            id: 1,
            title: Some("t".into()),
            app_id: Some("app.exe".into()),
            pid: Some(10),
            workspace_id: Some(4_294_967_296),
            is_focused: true,
            is_floating: false,
            is_urgent: false,
            layout: WindowLayoutIpc {
                pos_in_scrolling_layout: Some((1, 1)),
            },
            class_name: "C".into(),
            output: Some("\\\\.\\DISPLAY1".into()),
            is_fullscreen: false,
            is_maximized: false,
            is_minimized: false,
        }
    }

    #[test]
    fn the_niri_events_carry_niris_field_names() {
        // What a client written against niri's event stream reads: the fields of its
        // `Window` and `Workspace`, in snake_case, inside wiri's {"type","data"} envelope.
        let event = IpcEvent::WindowOpenedOrChanged {
            window: sample_window(),
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["type"], "window_opened_or_changed");
        let window = &value["data"]["window"];
        for field in [
            "id",
            "title",
            "app_id",
            "pid",
            "workspace_id",
            "is_focused",
            "is_floating",
            "is_urgent",
            "layout",
        ] {
            assert!(window.get(field).is_some(), "window.{} is missing: {}", field, window);
        }
        assert_eq!(window["layout"]["pos_in_scrolling_layout"], serde_json::json!([1, 1]));
        let event = IpcEvent::WorkspacesChanged {
            workspaces: vec![WorkspaceIpc {
                id: 4_294_967_297,
                idx: 2,
                name: None,
                output: None,
                is_urgent: false,
                is_active: false,
                is_focused: false,
                active_window_id: None,
            }],
        };
        let value = serde_json::to_value(&event).unwrap();
        let workspace = &value["data"]["workspaces"][0];
        for field in [
            "id",
            "idx",
            "name",
            "output",
            "is_urgent",
            "is_active",
            "is_focused",
            "active_window_id",
        ] {
            assert!(workspace.get(field).is_some(), "workspace.{} is missing: {}", field, workspace);
        }
        let event = IpcEvent::WindowClosed {
            window_hwnd: 7,
            id: 7,
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["data"]["id"], 7);
        assert_eq!(value["data"]["window_hwnd"], 7);
    }

    #[test]
    fn an_old_window_closed_without_an_id_still_reads() {
        let parsed: IpcEvent =
            serde_json::from_str(r#"{"type":"window_closed","data":{"window_hwnd":5}}"#).unwrap();
        match parsed {
            IpcEvent::WindowClosed { window_hwnd, id } => assert_eq!((window_hwnd, id), (5, 0)),
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn test_state_response_serialization() {
        let resp = StateResponse {
            windows: vec![WindowInfoIpc {
                hwnd: 100,
                title: "Test".into(),
                class_name: "Class".into(),
                process_id: 999,
                x: 10,
                y: 20,
                width: 800,
                height: 600,
            }],
            workspaces: vec![WorkspaceInfo {
                name: "workspace-0".into(),
                id: 0,
                window_count: 5,
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
        let event = IpcEvent::WindowClosed {
            window_hwnd: 42,
            id: 42,
        };
        let bytes = serialize_event(&event).unwrap();
        let parsed = deserialize_event(&bytes).unwrap();
        let bytes2 = serialize_event(&parsed).unwrap();
        assert_eq!(bytes, bytes2);
    }
}
