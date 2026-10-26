//! Typed niri screenshot options, adapted to the common argv validator.
use serde_json::{json, Map, Value};

pub(super) fn screenshot(
    variant: &str,
    fields: &Map<String, Value>,
) -> Result<Option<Value>, String> {
    let name = match variant {
        "Screenshot" => "screenshot",
        _ => return Ok(None),
    };
    let mut args = Vec::<String>::new();
    if let Some(id) = fields.get("id").filter(|v| !v.is_null()) {
        if variant != "ScreenshotWindow" {
            return Err("only screenshot-window accepts id".into());
        }
        let id = id
            .as_u64()
            .filter(|id| *id > 0 && *id <= isize::MAX as u64)
            .ok_or("window id must be a positive native window id")?;
        args.extend(["--id".into(), id.to_string()]);
    }
    for key in ["show_pointer", "write_to_disk"] {
        if let Some(value) = fields.get(key).filter(|v| !v.is_null()) {
            if variant == "Screenshot" && key == "write_to_disk" {
                return Err("screenshot UI uses Ctrl+C for clipboard-only capture".into());
            }
            let value = value
                .as_bool()
                .ok_or_else(|| format!("{key} must be boolean"))?;
            args.extend([format!("--{}", key.replace('_', "-")), value.to_string()]);
        }
    }
    if let Some(path) = fields.get("path").filter(|v| !v.is_null()) {
        args.extend([
            "--path".into(),
            path.as_str().ok_or("path must be a string")?.to_string(),
        ]);
    }
    Ok(Some(
        json!({"type":"run_action","data":{"name":name,"args":args,"quoted":[]}}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_screenshot_fields_reach_the_same_cli_parser() {
        let fields = json!({"show_pointer":false,"path":r"C:\shots\one.png"});
        let value = screenshot("Screenshot", fields.as_object().unwrap())
            .unwrap()
            .unwrap();
        let words: Vec<String> = serde_json::from_value(value["data"]["args"].clone()).unwrap();
        let action = crate::input::parse_action_name("screenshot", &words).unwrap();
        assert!(matches!(action, crate::input::Action::ScreenshotUi(_)));
    }
    #[test]
    fn typed_known_options_never_silently_coerce_wrong_types() {
        for fields in [
            json!({"show_pointer":"false"}),
            json!({"path":1}),
            json!({"write_to_disk":false}),
            json!({"id":4}),
        ] {
            assert!(screenshot("Screenshot", fields.as_object().unwrap()).is_err());
        }
        assert!(screenshot("Unrelated", json!({}).as_object().unwrap())
            .unwrap()
            .is_none());
    }
}
