//! `WIRI_ACTIONS_DRY_RUN=<file>`: the IPC action path decodes a name and its
//! arguments exactly as it always does and then writes one audit line instead
//! of performing the action.
//!
//! It exists for one gate.  Proving that every action name the decoder ships is
//! accepted over the pipe (`h79_action_names_all_run_over_ipc`) cannot mean
//! performing them: a quit confirmation, the screenshot UI, a suspend or a
//! `close-window` would each end or disturb the run, and the claim under test is
//! "the name is understood", not "the effect works".  The hook therefore sits at
//! the *effect* boundary of the handler — the last line before the screenshot,
//! power or dispatch call — never earlier, so a name or an argument list the live
//! path would reject is rejected here too.  Nothing in this module runs when the
//! variable is unset.

/// The environment variable that names the audit file.  A path, not a flag: one
/// line is appended per accepted action, so a phase can compare the names it
/// sent with the names the daemon took.
pub const DRY_RUN_ENV: &str = "WIRI_ACTIONS_DRY_RUN";

/// The audit file, when the dry run is on.
fn path() -> Option<std::path::PathBuf> {
    std::env::var_os(DRY_RUN_ENV)
        .map(std::path::PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// The reply the handler returns instead of performing `name`, or `None` when
/// the dry run is off.
pub fn reply(name: &str, args: &[String]) -> Option<serde_json::Value> {
    let path = path()?;
    Some(match record(&path, name, args) {
        Ok(()) => serde_json::json!({"success": true}),
        Err(error) => serde_json::json!({
            "success": false,
            "error": format!("action audit {}: {}", path.display(), error)
        }),
    })
}

/// One line per accepted action: `{"name":"...","args":[...]}`.
fn record(path: &std::path::Path, name: &str, args: &[String]) -> std::io::Result<()> {
    use std::io::Write;
    let line = serde_json::json!({"name": name, "args": args}).to_string();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_audit_line_names_the_action_and_its_words() {
        let dir = std::env::temp_dir().join(format!("wiri-action-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("actions.ndjson");
        record(&path, "close-window", &[]).expect("record");
        record(&path, "focus-workspace", &["2".to_string()]).expect("record");
        let text = std::fs::read_to_string(&path).expect("read");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        let first: serde_json::Value = serde_json::from_str(lines[0]).expect("json");
        assert_eq!(first["name"], "close-window");
        assert_eq!(first["args"], serde_json::json!([]));
        let second: serde_json::Value = serde_json::from_str(lines[1]).expect("json");
        assert_eq!(
            second,
            serde_json::json!({"name": "focus-workspace", "args": ["2"]})
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
