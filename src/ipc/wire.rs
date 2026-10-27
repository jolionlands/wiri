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

/// Convert niri's typed action vocabulary into the same validated parser used
/// by binds. Never silently discard a known flag whose semantics we lack.
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
    if let Some(request) = super::action_options::screenshot(&variant, fields)? {
        return Ok(request);
    }
    if let Some(words) = crate::input::system_actions::wire_args(&variant, fields) {
        return Ok(json!({"type":"run_action","data":{
            "name":snake_case(&variant).replace('_',"-"),"args":words?,"quoted":[]
        }}));
    }
    if fields.get("focus") == Some(&Value::Bool(false)) {
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
    if let Some(command) = fields.get("command") {
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
    if let Some(reference) = fields.get("reference") {
        let (value, quote) = if let Some(name) = reference.get("Name").and_then(Value::as_str) {
            (name.to_string(), true)
        } else if let Some(index) = reference.get("Index").and_then(Value::as_u64) {
            (index.to_string(), false)
        } else if let Some(id) = reference.get("Id").and_then(Value::as_u64) {
            return Err(format!(
                "workspace id reference {id} is unsupported; use Index or Name"
            ));
        } else {
            return Err("workspace reference must be {\"Index\":n} or {\"Name\":\"name\"}".into());
        };
        push(value, quote);
    }
    if let Some(change) = fields.get("change") {
        let map = change
            .as_object()
            .filter(|map| map.len() == 1)
            .ok_or("change needs one size variant")?;
        let (kind, number) = map.iter().next().unwrap();
        let number = number
            .as_f64()
            .filter(|n| n.is_finite())
            .ok_or("size must be finite")?;
        let (sign, percent) = match kind.as_str() {
            "SetFixed" => ("", false),
            "AdjustFixed" => (if number >= 0.0 { "+" } else { "" }, false),
            "SetProportion" => ("", true),
            "AdjustProportion" => (if number >= 0.0 { "+" } else { "" }, true),
            _ => return Err(format!("unknown size change '{kind}'")),
        };
        push(
            format!(
                "{sign}{}{}",
                if percent { number * 100.0 } else { number },
                if percent { "%" } else { "" }
            ),
            false,
        );
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
    // Unknown future fields are tolerated, as niri's compatibility contract requires.
    // Known flags that affect behavior are never accepted as no-ops.
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
        let a = decode(json!({"Action":{"FocusWorkspace":{"reference":{"Name":"2"}}}})).unwrap();
        let json = serde_json::to_value(a.message).unwrap();
        assert_eq!(json["data"]["args"], json!(["2"]));
        assert_eq!(json["data"]["quoted"], json!([true]));
        let a = decode(
            json!({"Action":{"SetColumnWidth":{"change":{"AdjustProportion":0.1},"id":44}}}),
        )
        .unwrap();
        let json = serde_json::to_value(a.message).unwrap();
        assert_eq!(json["data"]["args"], json!(["--id", "44", "+10%"]));
        assert!(decode(json!({"Action":{"MoveWindowToWorkspace":{"focus":false}}})).is_err());
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
