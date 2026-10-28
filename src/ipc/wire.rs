//! The two IPC dialects and bounded, incremental JSON-line framing.
//! A complete undelimited first wiri value retains the legacy one-shot protocol.
use serde_json::{json, Value};

use super::{IpcEvent, IpcMessage};

pub const SCHEMA_VERSION: &str = "1.3";
pub const MAX_REQUEST_BYTES: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Wiri,
    Niri,
}

pub struct Request {
    pub message: IpcMessage,
    pub dialect: Dialect,
    pub name: String,
}

pub fn dialect(value: &Value) -> Dialect {
    if value.get("type").is_some() {
        Dialect::Wiri
    } else {
        Dialect::Niri
    }
}

/// Unknown fields are deliberately ignored by serde. Unknown variants and bad
/// fields are errors of this request, not errors of the connection.
pub fn decode(value: Value) -> Result<Request, String> {
    let style = dialect(&value);
    let adjacent = match style {
        Dialect::Wiri => value,
        Dialect::Niri => {
            let (name, data) = match value {
                Value::String(name) => (name, Value::Null),
                Value::Object(object) if object.len() == 1 => object.into_iter().next().unwrap(),
                _ => return Err("expected a request name or an externally tagged request".into()),
            };
            match name.as_str() {
                "Action" => action(data)?,
                "EventStream" => json!({"type":"subscribe_events","data":{"event_types":[]}}),
                "Output" => json!({"type":"output","data":data}),
                "Version" | "Outputs" | "Workspaces" | "Windows" | "KeyboardLayouts"
                | "FocusedOutput" | "FocusedWindow" | "PickWindow" | "PickColor"
                | "ReturnError" | "OverviewState" => json!({"type":snake_case(&name)}),
                "Layers" | "Casts" => return Err(format!("{name} is not available on Windows")),
                _ => {
                    return Err(format!(
                        "unknown IPC request '{name}' (schema {SCHEMA_VERSION})"
                    ))
                }
            }
        }
    };
    let message: IpcMessage = serde_json::from_value(adjacent).map_err(|e| e.to_string())?;
    let name = serde_json::to_value(&message).map_err(|e| e.to_string())?["type"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    Ok(Request {
        message,
        dialect: style,
        name,
    })
}

fn snake_case(name: &str) -> String {
    let mut result = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i != 0 {
            result.push('_');
        }
        result.extend(c.to_lowercase());
    }
    result
}

fn pascal_case(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_default()
                + chars.as_str()
        })
        .collect()
}

/// Preserve typed workspace references; other actions enter the validated bind
/// parser. Never silently discard a known flag whose semantics we lack.
fn action(value: Value) -> Result<Value, String> {
    let (variant, fields) = match value {
        Value::String(name) => (name, json!({})),
        Value::Object(map) if map.len() == 1 => map.into_iter().next().unwrap(),
        _ => return Err("Action needs one externally tagged action".into()),
    };
    let mut args = Vec::<Value>::new();
    let mut quoted = Vec::<bool>::new();
    let mut push = |value: String, quote: bool| {
        args.push(Value::String(value));
        quoted.push(quote);
    };
    let fields = if fields.is_null() { json!({}) } else { fields };
    let fields = fields
        .as_object()
        .ok_or("an action's fields must be an object")?;
    if variant == "MoveFloatingWindow" {
        return Ok(json!({"type":"move_floating_window","data":fields}));
    }
    if let Some(request) = super::action_options::screenshot(&variant, fields)? {
        return Ok(request);
    }
    if let Some(words) = crate::input::system_actions::wire_args(&variant, fields) {
        return Ok(json!({"type":"run_action","data":{
            "name":snake_case(&variant).replace('_',"-"),"args":words?,"quoted":[]
        }}));
    }
    // Unknown future fields are tolerated. Known unsupported behavior remains
    // an error even when a workspace action takes the structured path below.
    for key in [
        "skip_empty",
        "skip_empty_workspaces",
        "write_to_disk",
        "show_pointer",
    ] {
        if fields.get(key).is_some_and(|v| !v.is_null()) {
            return Err(format!(
                "action flag '{key}' is not supported on this executor"
            ));
        }
    }
    if matches!(
        variant.as_str(),
        "SetColumnWidth" | "SetWindowWidth" | "SetWindowHeight"
    ) {
        if fields.get("focus") == Some(&Value::Bool(false)) {
            return Err("focus=false is not supported by this Windows action executor".into());
        }
        let action: super::SizeIpcAction =
            serde_json::from_value(json!({(variant):fields})).map_err(|e| e.to_string())?;
        action.into_input_action()?;
        return Ok(json!({"type":"size_action","data":{"action":action}}));
    }
    if matches!(
        variant.as_str(),
        "FocusWorkspace"
            | "MoveWindowToWorkspace"
            | "MoveColumnToWorkspace"
            | "MoveWorkspaceToIndex"
            | "SetWorkspaceName"
            | "UnsetWorkspaceName"
            | "MoveWorkspaceToMonitor"
    ) {
        if !matches!(
            variant.as_str(),
            "MoveWindowToWorkspace" | "MoveColumnToWorkspace"
        ) && fields.get("focus") == Some(&Value::Bool(false))
        {
            return Err("focus=false is not supported by this Windows action executor".into());
        }
        let action: super::WorkspaceIpcAction =
            serde_json::from_value(json!({(variant):fields})).map_err(|e| e.to_string())?;
        return Ok(json!({"type":"workspace_action","data":{"action":action}}));
    }
    let workspace_transfer = matches!(
        variant.as_str(),
        "MoveWindowToWorkspaceDown"
            | "MoveWindowToWorkspaceUp"
            | "MoveColumnToWorkspaceDown"
            | "MoveColumnToWorkspaceUp"
    );
    if workspace_transfer {
        if let Some(focus) = fields.get("focus") {
            let focus = focus.as_bool().ok_or("focus must be a boolean")?;
            push("--focus".into(), false);
            push(focus.to_string(), false);
        }
    } else if fields.get("focus") == Some(&Value::Bool(false)) {
        return Err("focus=false is not supported by this Windows action executor".into());
    }
    for key in ["id", "window_id"] {
        if let Some(id) = fields.get(key).filter(|id| !id.is_null()) {
            let id = id
                .as_u64()
                .ok_or("window id must be a non-negative integer")?;
            push("--id".into(), false);
            push(id.to_string(), false);
        }
    }
    if variant == "SpawnSh" {
        let command = fields
            .get("command")
            .and_then(Value::as_str)
            .ok_or("SpawnSh command must be a string")?;
        push(command.into(), true);
    } else if let Some(command) = fields.get("command") {
        for word in command
            .as_array()
            .ok_or("command must be an array of strings")?
        {
            push(
                word.as_str()
                    .ok_or("command arguments must be strings")?
                    .into(),
                true,
            );
        }
    }
    if variant == "SetColumnDisplay" {
        let display = match fields.get("display").and_then(Value::as_str) {
            Some("Normal") => "normal",
            Some("Tabbed") => "tabbed",
            _ => return Err("display must be \"Normal\" or \"Tabbed\"".into()),
        };
        push(display.into(), false);
    }
    for key in ["output", "name", "index", "layout"] {
        if let Some(value) = fields.get(key).filter(|v| !v.is_null()) {
            let text = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            push(text, value.is_string());
        }
    }
    Ok(
        json!({"type":"run_action","data":{"name":snake_case(&variant).replace('_',"-"),"args":args,"quoted":quoted}}),
    )
}

pub fn error(dialect: Dialect, text: impl Into<String>) -> Value {
    let text = text.into();
    match dialect {
        Dialect::Wiri => json!({"success":false,"error":text,"schema_version":SCHEMA_VERSION}),
        Dialect::Niri => json!({"Err":text}),
    }
}

/// Preserve wiri's payloads and adapt only the envelope for a niri client.
pub fn reply(dialect: Dialect, name: &str, mut response: Value) -> Value {
    if dialect == Dialect::Wiri {
        response["schema_version"] = json!(SCHEMA_VERSION);
        return response;
    }
    if response["success"] != true {
        return error(
            dialect,
            response["error"].as_str().unwrap_or("request failed"),
        );
    }
    let result = &response["result"];
    let (variant, payload) = match name {
        "version" => ("Version", result["version"].clone()),
        "windows" => ("Windows", result["windows"].clone()),
        "workspaces" => ("Workspaces", result["workspaces"].clone()),
        "focused_window" => ("FocusedWindow", result["focused_window"].clone()),
        "outputs" => ("Outputs", result["outputs"].clone()),
        "focused_output" => ("FocusedOutput", result["focused_output"].clone()),
        "keyboard_layouts" => ("KeyboardLayouts", result["keyboard_layouts"].clone()),
        "overview_state" => ("OverviewState", result.clone()),
        "pick_window" => ("PickedWindow", result["window"].clone()),
        "pick_color" => ("PickedColor", result["color"].clone()),
        "output" => ("OutputConfigChanged", json!("Applied")),
        _ => return json!({"Ok":"Handled"}),
    };
    let mut tagged = serde_json::Map::new();
    tagged.insert(variant.into(), payload);
    json!({"Ok":tagged})
}

/// Wiri-only events cannot be decoded by niri-ipc, so do not send those in its
/// dialect. Niri events retain their exact externally tagged names and fields.
pub fn event(dialect: Dialect, event: &IpcEvent) -> Option<Value> {
    let value = serde_json::to_value(event).ok()?;
    if dialect == Dialect::Wiri {
        return Some(value);
    }
    let name = value["type"].as_str()?;
    if !matches!(
        name,
        "workspaces_changed"
            | "workspace_urgency_changed"
            | "workspace_activated"
            | "workspace_active_window_changed"
            | "windows_changed"
            | "window_opened_or_changed"
            | "window_closed"
            | "window_focus_changed"
            | "window_focus_timestamp_changed"
            | "window_urgency_changed"
            | "window_layouts_changed"
            | "keyboard_layouts_changed"
            | "keyboard_layout_switched"
            | "overview_opened_or_closed"
            | "touchpad_toggled"
            | "config_loaded"
            | "screenshot_captured"
    ) {
        return None;
    }
    let mut map = serde_json::Map::new();
    map.insert(pascal_case(name), value["data"].clone());
    Some(Value::Object(map))
}

/// A bounded incremental framer. Oversized lines are drained to the next
/// newline so the following request is still usable, without unbounded memory.
#[derive(Default)]
pub struct Framer {
    pending: Vec<u8>,
    discarding: bool,
    pub lined: bool,
}

impl Framer {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Result<Value, String>> {
        let mut frames = Vec::new();
        for byte in bytes {
            if *byte == b'\n' {
                self.lined = true;
                if self.discarding {
                    frames.push(Err(format!("request exceeds {MAX_REQUEST_BYTES} bytes")));
                } else if !self.pending.iter().all(u8::is_ascii_whitespace) {
                    frames.push(
                        serde_json::from_slice(&self.pending)
                            .map_err(|e| format!("invalid JSON: {e}")),
                    );
                }
                self.pending.clear();
                self.discarding = false;
            } else if !self.discarding {
                if self.pending.len() == MAX_REQUEST_BYTES {
                    self.pending.clear();
                    self.discarding = true;
                } else {
                    self.pending.push(*byte);
                }
            }
        }
        frames
    }

    /// Old clients send one complete JSON value without a delimiter and await
    /// EOF. This compatibility path is used only before any newline is seen.
    pub fn legacy(&mut self) -> Option<Result<Value, String>> {
        if self.lined || self.discarding || self.pending.is_empty() {
            return None;
        }
        match serde_json::from_slice::<Value>(&self.pending) {
            Ok(value) if dialect(&value) == Dialect::Wiri => {
                self.pending.clear();
                Some(Ok(value))
            }
            // External niri clients have always used JSON lines. A complete
            // value is not a frame until its delimiter arrives in a later read.
            Ok(_) => None,
            Err(error) if error.is_eof() => None,
            Err(error) => {
                self.pending.clear();
                Some(Err(format!("invalid JSON: {error}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parity_move_floating_window_preserves_typed_axes_and_defaults() {
        // The pinned action is IPC-only: routing through the bind parser loses its axes.
        let request = decode(json!({"Action":{"MoveFloatingWindow":{
            "id":42,"x":{"SetProportion":50.0}
        }}}))
        .expect("typed floating positioning is accepted");
        assert_eq!(
            serde_json::to_value(request.message).unwrap(),
            json!({
                "type":"move_floating_window",
                "data":{"id":42,"x":{"SetProportion":50.0},"y":{"AdjustFixed":0.0}}
            })
        );
    }

    #[test]
    fn parity_move_floating_window_rejects_bad_typed_axes_and_ids() {
        for fields in [
            json!({"x":50}),
            json!({"x":{"Unknown":1}}),
            json!({"x":{"SetFixed":"20"}}),
            json!({"x":null}),
            json!({"y":{"SetFixed":10,"AdjustFixed":20}}),
            json!({"id":-1}),
            json!({"id":"42"}),
            json!({"id":1.5}),
        ] {
            assert!(decode(json!({"Action":{"MoveFloatingWindow":fields}})).is_err());
            assert!(decode(json!({"type":"move_floating_window","data":fields})).is_err());
        }
        let request = decode(json!({"Action":{"MoveFloatingWindow":{"future":true}}})).unwrap();
        assert_eq!(
            serde_json::to_value(request.message).unwrap(),
            json!({
                "type":"move_floating_window","data":{"id":null,
                "x":{"AdjustFixed":0.0},"y":{"AdjustFixed":0.0}}
            })
        );
    }

    #[test]
    fn documented_schema_examples_use_the_current_wire_contract() {
        let schema: Value =
            serde_json::from_str(include_str!("../../docs/IPC_SCHEMA.json")).unwrap();
        assert_eq!(schema["version"], SCHEMA_VERSION);
        let defs = &schema["$defs"];
        for value in defs["NiriRequest"]["examples"].as_array().unwrap() {
            let request = decode(value.clone()).unwrap();
            assert_eq!(request.dialect, Dialect::Niri);
        }
        let names = defs["NiriEvent"]["propertyNames"]["enum"]
            .as_array()
            .unwrap();
        for value in defs["NiriEvent"]["examples"].as_array().unwrap() {
            let (name, data) = value.as_object().unwrap().iter().next().unwrap();
            assert!(names.contains(&json!(name)));
            let decoded: IpcEvent = serde_json::from_value(json!({
                "type": snake_case(name), "data": data,
            }))
            .unwrap();
            assert_eq!(event(Dialect::Niri, &decoded), Some(value.clone()));
        }
        let results = [
            (
                "keyboard_layouts",
                json!({"keyboard_layouts":{"names":["US"],"current_idx":0}}),
            ),
            ("pick_window", json!({"window":null})),
            ("pick_color", json!({"color":{"rgb":[1.0,0.5,0.0]}})),
            ("output", json!({"output":"DISPLAY1","temporary":true})),
        ];
        let examples = defs["NiriReply"]["examples"].as_array().unwrap();
        for ((name, result), expected) in results.into_iter().zip(examples) {
            assert_eq!(
                reply(Dialect::Niri, name, json!({"success":true,"result":result})),
                *expected
            );
        }
        assert_eq!(
            error(Dialect::Niri, "example error requested by ReturnError"),
            *examples.last().unwrap()
        );
    }

    #[test]
    fn schema_references_resolve_and_new_window_fields_remain_required() {
        let schema: Value =
            serde_json::from_str(include_str!("../../docs/IPC_SCHEMA.json")).unwrap();
        fn visit(value: &Value, schema: &Value) {
            match value {
                Value::Object(map) => {
                    if let Some(reference) = map.get("$ref").and_then(Value::as_str) {
                        assert!(
                            schema
                                .pointer(reference.strip_prefix('#').unwrap())
                                .is_some(),
                            "{reference}"
                        );
                    }
                    for value in map.values() {
                        visit(value, schema);
                    }
                }
                Value::Array(array) => {
                    for value in array {
                        visit(value, schema);
                    }
                }
                _ => {}
            }
        }
        visit(&schema, &schema);
        let defs = &schema["$defs"];
        assert!(defs["WindowIpc"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("focus_timestamp")));
        assert_eq!(
            defs["WindowLayoutIpc"]["required"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
    }

    #[test]
    fn fragmented_and_pipelined_lines_are_independent() {
        let mut f = Framer::default();
        assert!(f.feed(b"{\"type\":\"ver").is_empty());
        assert!(f.legacy().is_none());
        let frames = f.feed(b"sion\"}\r\n\"Windows\"\n{bad}\n\"Version\"\n");
        assert_eq!(frames.len(), 4);
        assert!(decode(frames[0].as_ref().unwrap().clone()).is_ok());
        assert_eq!(frames[1].as_ref().unwrap(), "Windows");
        assert!(frames[2].is_err());
        assert_eq!(frames[3].as_ref().unwrap(), "Version");
        assert!(f.legacy().is_none());
    }

    #[test]
    fn typed_column_display_decodes_both_modes_and_keeps_window_targets() {
        for (display, argument, tabbed) in [("Normal", "normal", false), ("Tabbed", "tabbed", true)]
        {
            let decoded = decode(json!({"Action":{"SetColumnDisplay":{
                "display":display, "future_field":true
            }}}))
            .unwrap();
            let IpcMessage::RunAction { name, args, quoted } = decoded.message else {
                panic!("expected the shared action request");
            };
            assert_eq!(name, "set-column-display");
            assert_eq!(args, vec![argument.to_string()]);
            assert_eq!(quoted, vec![false]);
            assert_eq!(
                crate::input::parse_action_flagged(&name, &args, &quoted),
                Some(crate::input::Action::SetColumnDisplay { tabbed })
            );

            let targeted = decode(json!({"Action":{"SetColumnDisplay":{
                "display":display, "id":44
            }}}))
            .unwrap();
            let value = serde_json::to_value(targeted.message).unwrap();
            assert_eq!(value["data"]["args"], json!(["--id", "44", argument]));
        }
    }

    #[test]
    fn typed_column_display_rejects_missing_invalid_modes_and_unsupported_flags() {
        for fields in [
            json!({}),
            Value::Null,
            json!({"display":null}),
            json!({"display":true}),
            json!({"display":0}),
            json!({"display":"normal"}),
            json!({"display":"Stacked"}),
            json!({"display":{"Tabbed":null}}),
        ] {
            assert!(decode(json!({"Action":{"SetColumnDisplay":fields}})).is_err());
        }
        for (key, flag) in [
            ("focus", json!(false)),
            ("skip_empty", json!(false)),
            ("skip_empty_workspaces", json!(true)),
            ("write_to_disk", json!(false)),
            ("show_pointer", json!(false)),
        ] {
            let mut fields = json!({"display":"Tabbed"});
            fields[key] = flag;
            assert!(decode(json!({"Action":{"SetColumnDisplay":fields}})).is_err());
        }
    }

    #[test]
    fn typed_system_actions_preserve_their_semantic_fields() {
        for (value, expected_name, expected_args) in [
            (
                json!({"SwitchLayout":{"target":{"Index":2}}}),
                "switch-layout",
                vec!["2"],
            ),
            (
                json!({"SetWindowUrgent":{"id":42}}),
                "set-window-urgent",
                vec!["--id", "42"],
            ),
            (
                json!({"Quit":{"skip_confirmation":true}}),
                "quit",
                vec!["--skip-confirmation"],
            ),
            (
                json!({"LoadConfigFile":{"path":r"C:\wiri\other.kdl"}}),
                "load-config-file",
                vec![r"C:\wiri\other.kdl"],
            ),
            (json!("PowerOffMonitors"), "power-off-monitors", vec![]),
            (json!("PowerOnMonitors"), "power-on-monitors", vec![]),
            (json!("Suspend"), "suspend", vec![]),
            (json!("ToggleOverview"), "toggle-overview", vec![]),
            (json!("OpenOverview"), "open-overview", vec![]),
            (json!("CloseOverview"), "close-overview", vec![]),
        ] {
            let request = decode(json!({"Action":value})).unwrap();
            let IpcMessage::RunAction { name, args, .. } = request.message else {
                panic!("not RunAction")
            };
            assert_eq!(name, expected_name);
            assert_eq!(args, expected_args);
        }
    }

    #[test]
    fn typed_system_actions_reject_bad_known_fields() {
        for value in [
            json!({"Quit":{"skip_confirmation":"yes"}}),
            json!({"SwitchLayout":{"target":256}}),
            json!({"SetWindowUrgent":{"id":0}}),
            json!({"LoadConfigFile":{"path":5}}),
        ] {
            assert!(decode(json!({"Action":value})).is_err());
        }
    }

    #[test]
    fn external_request_waits_for_a_fragmented_delimiter() {
        let mut f = Framer::default();
        assert!(f.feed(b"\"Windows\"").is_empty());
        assert!(f.legacy().is_none());
        assert_eq!(f.feed(b"\n")[0].as_ref().unwrap(), "Windows");
    }

    #[test]
    fn oversized_line_recovers_at_the_next_delimiter() {
        let mut f = Framer::default();
        assert!(f.feed(&vec![b'x'; MAX_REQUEST_BYTES + 20]).is_empty());
        assert!(f.pending.is_empty());
        let frames = f.feed(b"\n\"Version\"\n");
        assert!(frames[0].is_err());
        assert_eq!(frames[1].as_ref().unwrap(), "Version");
    }

    #[test]
    fn legacy_complete_value_needs_neither_newline_nor_eof() {
        let mut f = Framer::default();
        assert!(f.feed(b"{\"type\":\"get_state\"}").is_empty());
        assert!(decode(f.legacy().unwrap().unwrap()).is_ok());
    }

    #[test]
    fn niri_and_wiri_spellings_select_the_reply_dialect() {
        for (value, expected) in [
            (json!("Windows"), Dialect::Niri),
            (json!({"type":"windows","extra":3}), Dialect::Wiri),
        ] {
            let request = decode(value).unwrap();
            assert_eq!(request.dialect, expected);
            let reply = reply(
                expected,
                &request.name,
                json!({"success":true,"result":{"windows":[]}}),
            );
            if expected == Dialect::Niri {
                assert_eq!(reply, json!({"Ok":{"Windows":[]}}));
            } else {
                assert_eq!(reply["schema_version"], SCHEMA_VERSION);
            }
        }
    }

    #[test]
    fn unknown_requests_and_bad_fields_have_useful_errors() {
        assert!(decode(json!("NotARequest"))
            .err()
            .unwrap()
            .contains("unknown IPC request"));
        assert!(decode(json!({"type":"focus_window","data":{"window_hwnd":"bad"}})).is_err());
        assert!(
            decode(json!({"type":"focus_window","data":{"window_hwnd":1,"future":true}})).is_ok()
        );
        assert_eq!(error(Dialect::Niri, "test"), json!({"Err":"test"}));
    }

    #[test]
    fn typed_niri_actions_preserve_references_sizes_and_ids() {
        assert_eq!(
            parsed_typed_action(json!({"FocusWorkspace":{"reference":{"Name":"2"}}})),
            crate::input::Action::FocusWorkspaceNamed("2".into())
        );
        let a = decode(
            json!({"Action":{"SetColumnWidth":{"change":{"AdjustProportion":0.1},"id":44}}}),
        )
        .unwrap();
        let IpcMessage::SizeAction { action } = a.message else {
            panic!("typed sizes need lossless transport");
        };
        assert_eq!(action.window_id(), Some(44));
        assert_eq!(
            action.into_input_action().unwrap(),
            crate::input::Action::SetColumnWidth(crate::input::SizeChange::AdjustProportion(100))
        );
        assert!(decode(json!({"Action":{"MoveWindowToWorkspace":{"focus":false}}})).is_err());
    }

    fn parsed_typed_action(value: Value) -> crate::input::Action {
        let request = decode(json!({"Action":value.clone()}))
            .unwrap_or_else(|error| panic!("{value}: {error}"));
        match request.message {
            IpcMessage::RunAction { name, args, quoted } => {
                crate::input::parse_action_flagged(&name, &args, &quoted)
                    .unwrap_or_else(|| panic!("{value}: rejected {name} {args:?} {quoted:?}"))
            }
            IpcMessage::WorkspaceAction { action } => action
                .into_input_action()
                .unwrap_or_else(|error| panic!("{value}: {error}")),
            IpcMessage::SizeAction { action } => action
                .into_input_action()
                .unwrap_or_else(|error| panic!("{value}: {error}")),
            _ => panic!("{value}: expected a shared or structured action request"),
        }
    }

    #[test]
    fn parity_typed_size_changes_preserve_percent_units_and_set_adjust_kinds() {
        use crate::input::{Action, SizeChange};

        let mut failures = Vec::new();
        for (variant, expected_action) in [
            (
                "SetColumnWidth",
                Action::SetColumnWidth as fn(SizeChange) -> Action,
            ),
            (
                "SetWindowWidth",
                Action::SetColumnWidth as fn(SizeChange) -> Action,
            ),
            (
                "SetWindowHeight",
                Action::SetWindowHeight as fn(SizeChange) -> Action,
            ),
        ] {
            // The pin parses "10%" as SetProportion(10.), with no factor of 100.
            // A typed Set with a negative value must retain Set despite argv's sign grammar.
            for (change, expected) in [
                (
                    json!({"SetProportion":50.0}),
                    SizeChange::SetProportion(50_000),
                ),
                (json!({"SetProportion":0.1}), SizeChange::SetProportion(100)),
                (json!({"SetProportion":0.0}), SizeChange::SetProportion(0)),
                (
                    json!({"SetProportion":-10.0}),
                    SizeChange::SetProportion(-10_000),
                ),
                (
                    json!({"AdjustProportion":10.0}),
                    SizeChange::AdjustProportion(10_000),
                ),
                (
                    json!({"AdjustProportion":-10.0}),
                    SizeChange::AdjustProportion(-10_000),
                ),
                (
                    json!({"AdjustProportion":0.0}),
                    SizeChange::AdjustProportion(0),
                ),
                (json!({"SetFixed":50}), SizeChange::SetFixed(50)),
                (json!({"SetFixed":0}), SizeChange::SetFixed(0)),
                (json!({"SetFixed":-10}), SizeChange::SetFixed(-10)),
                (
                    json!({"SetFixed":2147483647i64}),
                    SizeChange::SetFixed(i32::MAX),
                ),
                (
                    json!({"SetFixed":-2147483648i64}),
                    SizeChange::SetFixed(i32::MIN),
                ),
                (json!({"AdjustFixed":10}), SizeChange::AdjustFixed(10)),
                (json!({"AdjustFixed":-10}), SizeChange::AdjustFixed(-10)),
                (json!({"AdjustFixed":0}), SizeChange::AdjustFixed(0)),
                (
                    json!({"AdjustFixed":2147483647i64}),
                    SizeChange::AdjustFixed(i32::MAX),
                ),
                (
                    json!({"AdjustFixed":-2147483648i64}),
                    SizeChange::AdjustFixed(i32::MIN),
                ),
            ] {
                let value = json!({(variant):{"change":change}});
                let actual = parsed_typed_action(value.clone());
                let expected = expected_action(expected);
                if actual != expected {
                    failures.push(format!("{value}: expected {expected:?}, got {actual:?}"));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn parity_typed_size_changes_reject_invalid_variant_payloads() {
        let mut failures = Vec::new();
        for variant in ["SetColumnWidth", "SetWindowWidth", "SetWindowHeight"] {
            for change in [
                json!({"SetFixed":1.5}),
                json!({"AdjustFixed":1.5}),
                json!({"SetFixed":2147483648i64}),
                json!({"AdjustFixed":-2147483649i64}),
                json!({"SetFixed":"50"}),
                json!({"SetProportion":"50"}),
                json!({"AdjustProportion":null}),
                json!({"SetProportion":false}),
                json!({"SetFixed":50,"AdjustFixed":10}),
                json!({"Unknown":50}),
                json!(null),
            ] {
                let value = json!({"Action":{(variant):{"change":change}}});
                if decode(value.clone()).is_ok() {
                    failures.push(format!("accepted {value}"));
                }
            }
            let missing = json!({"Action":{(variant):{}}});
            if decode(missing.clone()).is_ok() {
                failures.push(format!("accepted {missing}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn parity_typed_size_transport_preserves_target_and_original_numeric_change() {
        for variant in ["SetColumnWidth", "SetWindowWidth", "SetWindowHeight"] {
            for id in [Value::Null, json!(42)] {
                let change = json!({"SetProportion":-0.123456});
                let request = decode(json!({"Action":{(variant):{
                    "id":id,"change":change
                }}}))
                .unwrap();
                let value = serde_json::to_value(request.message).unwrap();
                assert_eq!(value["type"], "size_action");
                assert_eq!(value["data"]["action"][variant]["id"], id);
                assert_eq!(value["data"]["action"][variant]["change"], change);
                let roundtrip: IpcMessage = serde_json::from_value(value.clone()).unwrap();
                assert_eq!(serde_json::to_value(roundtrip).unwrap(), value);
            }
        }
    }

    #[test]
    fn parity_typed_workspace_ids_survive_all_seven_action_payloads() {
        fn contains_reference(value: &Value, reference: &Value) -> bool {
            value == reference
                || match value {
                    Value::Object(fields) => fields
                        .values()
                        .any(|value| contains_reference(value, reference)),
                    Value::Array(values) => values
                        .iter()
                        .any(|value| contains_reference(value, reference)),
                    _ => false,
                }
        }

        let mut failures = Vec::new();
        // IDs occupy the full u64 domain and must never become positional words,
        // local workspace keys, names, or native window handles.
        for id in [0, u64::MAX - 17, u64::MAX] {
            let reference = json!({"Id":id});
            for value in [
                json!({"FocusWorkspace":{"reference":reference.clone()}}),
                json!({"MoveWindowToWorkspace":{
                    "window_id":42,"reference":reference.clone(),"focus":false
                }}),
                json!({"MoveColumnToWorkspace":{
                    "reference":reference.clone(),"focus":false
                }}),
                json!({"MoveWorkspaceToIndex":{
                    "index":3,"reference":reference.clone()
                }}),
                json!({"SetWorkspaceName":{
                    "name":"renamed","workspace":reference.clone()
                }}),
                json!({"UnsetWorkspaceName":{"reference":reference.clone()}}),
                json!({"MoveWorkspaceToMonitor":{
                    "output":"DISPLAY2","reference":reference.clone()
                }}),
            ] {
                match decode(json!({"Action":value.clone()})) {
                    Ok(request) => {
                        let message = serde_json::to_value(request.message).unwrap();
                        assert!(
                            contains_reference(&message, &reference),
                            "{value}: lost the typed workspace ID in {message}"
                        );
                    }
                    Err(error) => failures.push(format!("{value}: {error}")),
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn parity_typed_workspace_id_references_reach_input_actions() {
        use crate::input::{
            Action, MonitorAction, MonitorTarget, WorkspaceAction as W, WorkspaceRef,
        };
        let id = u64::MAX - 17;
        let reference = json!({"Id":id});
        for (value, expected) in [
            (
                json!({"FocusWorkspace":{"reference":reference.clone()}}),
                Action::WorkspaceAction(W::Focus {
                    reference: WorkspaceRef::Id(id),
                }),
            ),
            (
                json!({"MoveWindowToWorkspace":{
                    "window_id":42,"reference":reference.clone(),"focus":false
                }}),
                Action::WorkspaceAction(W::MoveWindow {
                    window: Some(crate::utils::WindowId::new(42)),
                    reference: WorkspaceRef::Id(id),
                    focus: false,
                }),
            ),
            (
                json!({"MoveColumnToWorkspace":{"reference":reference.clone()}}),
                Action::WorkspaceAction(W::MoveColumn {
                    reference: WorkspaceRef::Id(id),
                    focus: true,
                }),
            ),
            (
                json!({"MoveWorkspaceToIndex":{"index":3,"reference":reference.clone()}}),
                Action::WorkspaceAction(W::MoveToIndex {
                    index: 3,
                    workspace: Some(WorkspaceRef::Id(id)),
                }),
            ),
            (
                json!({"SetWorkspaceName":{"name":"renamed","workspace":reference.clone()}}),
                Action::WorkspaceAction(W::SetName {
                    name: "renamed".into(),
                    workspace: Some(WorkspaceRef::Id(id)),
                }),
            ),
            (
                json!({"UnsetWorkspaceName":{"reference":reference.clone()}}),
                Action::WorkspaceAction(W::UnsetName {
                    workspace: Some(WorkspaceRef::Id(id)),
                }),
            ),
            (
                json!({"MoveWorkspaceToMonitor":{"output":"DISPLAY2","reference":reference}}),
                Action::MonitorAction(MonitorAction::MoveWorkspace {
                    workspace: Some(WorkspaceRef::Id(id)),
                    target: MonitorTarget::Name("DISPLAY2".into()),
                }),
            ),
        ] {
            assert_eq!(parsed_typed_action(value.clone()), expected, "{value}");
        }
    }

    #[test]
    fn parity_typed_workspace_ids_reject_malformed_references() {
        for reference in [
            json!({"Id":-1}),
            json!({"Id":1.5}),
            json!({"Id":"42"}),
            json!({"Id":null}),
            json!({"Id":true}),
            json!({"Id":[]}),
            json!({"Id":{}}),
            json!({"Id":42,"Index":2}),
            serde_json::from_str::<Value>(r#"{"Id":18446744073709551616}"#).unwrap(),
        ] {
            for value in [
                json!({"FocusWorkspace":{"reference":reference.clone()}}),
                json!({"MoveWindowToWorkspace":{"reference":reference.clone()}}),
                json!({"MoveColumnToWorkspace":{"reference":reference.clone()}}),
                json!({"MoveWorkspaceToIndex":{"index":3,"reference":reference.clone()}}),
                json!({"SetWorkspaceName":{"name":"renamed","workspace":reference.clone()}}),
                json!({"UnsetWorkspaceName":{"reference":reference.clone()}}),
                json!({"MoveWorkspaceToMonitor":{"output":"DISPLAY2","reference":reference.clone()}}),
            ] {
                assert!(decode(json!({"Action":value.clone()})).is_err(), "{value}");
            }
        }
    }

    #[test]
    fn parity_typed_workspace_name_keeps_its_explicit_target() {
        use crate::input::{Action, WorkspaceAction, WorkspaceRef};

        // Numeric and flag-looking names must remain names through argv translation.
        for (reference, expected) in [
            (json!({"Index":2}), WorkspaceRef::Index(2)),
            (json!({"Name":"2"}), WorkspaceRef::Name("2".into())),
            (
                json!({"Name":"--workspace"}),
                WorkspaceRef::Name("--workspace".into()),
            ),
        ] {
            assert_eq!(
                parsed_typed_action(json!({"SetWorkspaceName":{
                    "name":"renamed", "workspace":reference
                }})),
                Action::WorkspaceAction(WorkspaceAction::SetName {
                    name: "renamed".into(),
                    workspace: Some(expected),
                })
            );
        }
    }

    #[test]
    fn parity_typed_workspace_optional_references_reach_the_shared_parser() {
        use crate::input::{Action, MonitorAction, MonitorTarget, WorkspaceAction, WorkspaceRef};

        for (reference, expected) in [
            (json!({"Index":2}), WorkspaceRef::Index(2)),
            (json!({"Name":"2"}), WorkspaceRef::Name("2".into())),
            (
                json!({"Name":"--workspace"}),
                WorkspaceRef::Name("--workspace".into()),
            ),
        ] {
            for (value, action) in [
                (
                    json!({"MoveWorkspaceToIndex":{"index":3,"reference":reference.clone()}}),
                    Action::WorkspaceAction(WorkspaceAction::MoveToIndex {
                        index: 3,
                        workspace: Some(expected.clone()),
                    }),
                ),
                (
                    json!({"UnsetWorkspaceName":{"reference":reference.clone()}}),
                    Action::WorkspaceAction(WorkspaceAction::UnsetName {
                        workspace: Some(expected.clone()),
                    }),
                ),
                (
                    json!({"MoveWorkspaceToMonitor":{
                        "output":"DISPLAY2","reference":reference.clone()
                    }}),
                    Action::MonitorAction(MonitorAction::MoveWorkspace {
                        workspace: Some(expected.clone()),
                        target: MonitorTarget::Name("DISPLAY2".into()),
                    }),
                ),
            ] {
                assert_eq!(parsed_typed_action(value.clone()), action, "{value}");
            }
        }
    }

    #[test]
    fn parity_typed_workspace_null_references_use_the_focused_workspace() {
        use crate::input::{Action, MonitorAction, MonitorTarget, WorkspaceAction};

        // serde serializes the pinned Option fields as null when no target is supplied.
        for (value, expected) in [
            (
                json!({"MoveWorkspaceToIndex":{"index":3,"reference":null}}),
                Action::WorkspaceAction(WorkspaceAction::MoveToIndex {
                    index: 3,
                    workspace: None,
                }),
            ),
            (
                json!({"UnsetWorkspaceName":{"reference":null}}),
                Action::WorkspaceAction(WorkspaceAction::UnsetName { workspace: None }),
            ),
            (
                json!({"MoveWorkspaceToMonitor":{"output":"DISPLAY2","reference":null}}),
                Action::MonitorAction(MonitorAction::MoveWorkspace {
                    workspace: None,
                    target: MonitorTarget::Name("DISPLAY2".into()),
                }),
            ),
            (
                json!({"SetWorkspaceName":{"name":"renamed","workspace":null}}),
                Action::RenameWorkspace("renamed".into()),
            ),
        ] {
            assert_eq!(parsed_typed_action(value.clone()), expected, "{value}");
        }
    }

    #[test]
    fn parity_typed_workspace_transfers_preserve_false_focus() {
        use crate::input::{Action, WorkspaceAction, WorkspaceRef};

        for (value, expected) in [
            (
                json!({"MoveWindowToWorkspace":{
                    "window_id":42,"reference":{"Name":"2"},"focus":false
                }}),
                WorkspaceAction::MoveWindow {
                    window: Some(crate::utils::WindowId::new(42)),
                    reference: WorkspaceRef::Name("2".into()),
                    focus: false,
                },
            ),
            (
                json!({"MoveWindowToWorkspaceDown":{"focus":false}}),
                WorkspaceAction::MoveWindowRelative {
                    window: None,
                    down: true,
                    focus: false,
                },
            ),
            (
                json!({"MoveWindowToWorkspaceUp":{"focus":false}}),
                WorkspaceAction::MoveWindowRelative {
                    window: None,
                    down: false,
                    focus: false,
                },
            ),
            (
                json!({"MoveColumnToWorkspace":{"reference":{"Index":2},"focus":false}}),
                WorkspaceAction::MoveColumn {
                    reference: WorkspaceRef::Index(2),
                    focus: false,
                },
            ),
            (
                json!({"MoveColumnToWorkspaceDown":{"focus":false}}),
                WorkspaceAction::MoveColumnRelative {
                    down: true,
                    focus: false,
                },
            ),
            (
                json!({"MoveColumnToWorkspaceUp":{"focus":false}}),
                WorkspaceAction::MoveColumnRelative {
                    down: false,
                    focus: false,
                },
            ),
        ] {
            assert_eq!(
                parsed_typed_action(value.clone()),
                Action::WorkspaceAction(expected),
                "{value}"
            );
        }
    }

    #[test]
    fn parity_typed_spawn_sh_keeps_the_complete_command_string() {
        let command = r#"echo "two words" & echo %PATH%"#;
        assert_eq!(
            parsed_typed_action(json!({"SpawnSh":{"command":command}})),
            crate::input::Action::SpawnCmd(command.into())
        );
    }

    #[test]
    fn parity_typed_workspace_and_shell_fields_reject_invalid_types() {
        for value in [
            json!({"MoveWindowToWorkspace":{"reference":{"Index":2},"focus":"false"}}),
            json!({"MoveWindowToWorkspaceDown":{"focus":null}}),
            json!({"MoveWindowToWorkspaceUp":{"focus":0}}),
            json!({"MoveColumnToWorkspace":{"reference":null}}),
            json!({"MoveColumnToWorkspaceDown":{"focus":[]}}),
            json!({"MoveColumnToWorkspaceUp":{"focus":{}}}),
            json!({"SetWorkspaceName":{"name":"renamed","workspace":{"Name":2}}}),
            json!({"MoveWorkspaceToIndex":{"index":3,"reference":{"Index":"2"}}}),
            json!({"UnsetWorkspaceName":{"reference":{"Index":256}}}),
            json!({"MoveWorkspaceToMonitor":{
                "output":"DISPLAY2","reference":{"Index":2,"Name":"2"}
            }}),
            json!({"SpawnSh":{"command":["echo","hi"]}}),
            json!({"SpawnSh":{"command":null}}),
            json!({"SpawnSh":{}}),
        ] {
            assert!(decode(json!({"Action":value.clone()})).is_err(), "{value}");
        }
    }

    #[test]
    fn parity_typed_workspace_metadata_rejects_invalid_required_scalars() {
        assert_eq!(
            parsed_typed_action(json!({"MoveWorkspaceToIndex":{"index":300,"reference":null}})),
            crate::input::Action::WorkspaceAction(crate::input::WorkspaceAction::MoveToIndex {
                index: 300,
                workspace: None,
            })
        );
        for (variant, key, valid, invalid) in [
            (
                "SetWorkspaceName",
                "name",
                json!({"name":"renamed"}),
                vec![json!(null), json!(false), json!(3), json!([]), json!({})],
            ),
            (
                "MoveWorkspaceToMonitor",
                "output",
                json!({"output":"DISPLAY2"}),
                vec![json!(null), json!(false), json!(3), json!([]), json!({})],
            ),
            (
                "MoveWorkspaceToIndex",
                "index",
                json!({"index":3}),
                vec![
                    json!(null),
                    json!(false),
                    json!(-1),
                    json!(1.5),
                    json!("3"),
                    json!("+3"),
                    json!([]),
                    json!({}),
                ],
            ),
        ] {
            for value in invalid.into_iter().map(Some).chain(std::iter::once(None)) {
                let mut fields = valid.clone();
                match value {
                    Some(value) => fields[key] = value,
                    None => {
                        fields.as_object_mut().unwrap().remove(key);
                    }
                }
                let request = json!({"Action":{(variant):fields}});
                assert!(decode(request.clone()).is_err(), "{request}");
            }
        }
    }

    #[test]
    fn niri_events_are_external_and_wiri_only_events_are_suppressed() {
        assert_eq!(
            event(Dialect::Niri, &IpcEvent::WindowFocusChanged { id: Some(3) }),
            Some(json!({"WindowFocusChanged":{"id":3}}))
        );
        assert!(event(Dialect::Niri, &IpcEvent::WindowFocused { window_hwnd: 3 }).is_none());
        // niri `TouchpadToggled { on }`: the `toggle-touchpad` action's event.
        assert_eq!(
            event(Dialect::Niri, &IpcEvent::TouchpadToggled { on: true }),
            Some(json!({"TouchpadToggled":{"on":true}}))
        );
        assert_eq!(
            event(Dialect::Wiri, &IpcEvent::TouchpadToggled { on: false }),
            Some(json!({"type":"touchpad_toggled","data":{"on":false}}))
        );
    }
}
