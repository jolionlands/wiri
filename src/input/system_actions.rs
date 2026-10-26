//! Non-layout actions shared by bindings and IPC.
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayoutSwitchTarget {
    Next,
    Prev,
    Index(u8),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Urgency {
    Toggle,
    Set,
    Unset,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SystemAction {
    SwitchLayout(LayoutSwitchTarget),
    WindowUrgent(Urgency),
    Quit { skip_confirmation: bool },
    LoadConfigFile(Option<PathBuf>),
    PowerOffMonitors,
    PowerOnMonitors,
    Suspend,
}

pub fn parse(name: &str, args: &[String]) -> Option<Option<SystemAction>> {
    use SystemAction::*;
    let action = match name {
        "switch-layout" => match args {
            [target] => match target.as_str() {
                "next" => Some(SwitchLayout(LayoutSwitchTarget::Next)),
                "prev" => Some(SwitchLayout(LayoutSwitchTarget::Prev)),
                _ => target
                    .parse::<u8>()
                    .ok()
                    .map(|i| SwitchLayout(LayoutSwitchTarget::Index(i))),
            },
            _ => None,
        },
        "toggle-window-urgent" | "set-window-urgent" | "unset-window-urgent" => {
            if !args.is_empty() {
                None
            } else {
                Some(WindowUrgent(match name {
                    "set-window-urgent" => Urgency::Set,
                    "unset-window-urgent" => Urgency::Unset,
                    _ => Urgency::Toggle,
                }))
            }
        }
        "quit" | "exit" => match args {
            [] => Some(Quit {
                skip_confirmation: false,
            }),
            [flag] if flag == "--skip-confirmation" => Some(Quit {
                skip_confirmation: true,
            }),
            [flag, value] if flag == "--skip-confirmation" => match value.as_str() {
                "true" => Some(Quit {
                    skip_confirmation: true,
                }),
                "false" => Some(Quit {
                    skip_confirmation: false,
                }),
                _ => None,
            },
            _ => None,
        },
        "load-config-file" => match args {
            [] => Some(LoadConfigFile(None)),
            [path] if !path.is_empty() && path != "--path" => {
                Some(LoadConfigFile(Some(PathBuf::from(path))))
            }
            [flag, path] if flag == "--path" && !path.is_empty() => {
                Some(LoadConfigFile(Some(PathBuf::from(path))))
            }
            _ => None,
        },
        "power-off-monitors" if args.is_empty() => Some(PowerOffMonitors),
        "power-on-monitors" if args.is_empty() => Some(PowerOnMonitors),
        "suspend" if args.is_empty() => Some(Suspend),
        "power-off-monitors" | "power-on-monitors" | "suspend" => None,
        _ => return None,
    };
    Some(action)
}

/// Adapter used by the niri-shaped typed IPC decoder. Unknown variants are left to it.
pub fn wire_args(
    variant: &str,
    fields: &Map<String, Value>,
) -> Option<Result<Vec<String>, String>> {
    let (name, allowed): (&str, &[&str]) = match variant {
        "SwitchLayout" => ("switch-layout", &["target"]),
        "ToggleWindowUrgent" => ("toggle-window-urgent", &["id"]),
        "SetWindowUrgent" => ("set-window-urgent", &["id"]),
        "UnsetWindowUrgent" => ("unset-window-urgent", &["id"]),
        "Quit" => ("quit", &["skip_confirmation"]),
        "LoadConfigFile" => ("load-config-file", &["path"]),
        "PowerOffMonitors" => ("power-off-monitors", &[]),
        "PowerOnMonitors" => ("power-on-monitors", &[]),
        "Suspend" => ("suspend", &[]),
        _ => return None,
    };
    Some((|| {
        if let Some(key) = fields.keys().find(|key| !allowed.contains(&key.as_str())) {
            return Err(format!("unknown field {key} for {variant}"));
        }
        let mut args = Vec::new();
        match name {
            "switch-layout" => {
                let target = fields.get("target").ok_or("SwitchLayout requires target")?;
                let word = match target {
                    Value::String(s) => s.to_ascii_lowercase(),
                    Value::Number(n) => n
                        .as_u64()
                        .filter(|n| *n <= 255)
                        .ok_or("layout index must be 0..255")?
                        .to_string(),
                    Value::Object(map) if map.len() == 1 && map.contains_key("Index") => map
                        ["Index"]
                        .as_u64()
                        .filter(|n| *n <= 255)
                        .ok_or("layout index must be 0..255")?
                        .to_string(),
                    _ => return Err("invalid layout target".into()),
                };
                args.push(word);
            }
            "quit" => {
                if let Some(value) = fields.get("skip_confirmation") {
                    if value
                        .as_bool()
                        .ok_or("skip_confirmation must be a boolean")?
                    {
                        args.push("--skip-confirmation".into());
                    }
                }
            }
            "load-config-file" => {
                if let Some(value) = fields.get("path").filter(|v| !v.is_null()) {
                    args.push(
                        value
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .ok_or("path must be a nonempty string or null")?
                            .to_owned(),
                    );
                }
            }
            _ => {}
        }
        if parse(name, &args).flatten().is_none() {
            return Err(format!("invalid arguments for {variant}"));
        }
        if let Some(value) = fields.get("id").filter(|v| !v.is_null()) {
            let id = value
                .as_u64()
                .filter(|id| *id > 0 && *id <= isize::MAX as u64)
                .ok_or("id must be a positive window handle")?;
            args.extend(["--id".into(), id.to_string()]);
        }
        Ok(args)
    })())
}

type ReloadSender = tokio::sync::mpsc::UnboundedSender<Option<PathBuf>>;
static RELOAD: OnceLock<parking_lot::Mutex<Option<ReloadSender>>> = OnceLock::new();
pub fn set_reload_sender(sender: ReloadSender) {
    *RELOAD.get_or_init(Default::default).lock() = Some(sender);
}
pub fn request_reload(path: Option<PathBuf>) -> Result<(), String> {
    let path = path.map(|p| expand_home_path(&p));
    if let Some(path) = &path {
        if !path.is_file() {
            return Err(format!("config file does not exist: {}", path.display()));
        }
        let decoded = crate::config::loader::load_quiet(path);
        if decoded.has_errors() {
            return Err(crate::config::failure_summary(path, &decoded.issues));
        }
    }
    RELOAD
        .get_or_init(Default::default)
        .lock()
        .as_ref()
        .ok_or("reload channel not initialized")?
        .send(path)
        .map_err(|_| "reload channel is closed".into())
}

/// Expand only a leading home component; never interpret shell syntax or other argv words.
pub fn expand_home_with(word: &str, home: Option<&Path>) -> String {
    if word == "~" || word.starts_with("~/") || word.starts_with("~\\") {
        if let Some(home) = home {
            return if word == "~" {
                home.to_string_lossy().into_owned()
            } else {
                home.join(&word[2..]).to_string_lossy().into_owned()
            };
        }
    }
    word.to_owned()
}
fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
}
pub fn expand_home_path(path: &Path) -> PathBuf {
    PathBuf::from(expand_home_with(&path.to_string_lossy(), home().as_deref()))
}
pub fn expand_spawn_argv(argv: &[String]) -> Vec<String> {
    expand_spawn_argv_with(argv, home().as_deref())
}
fn expand_spawn_argv_with(argv: &[String], home: Option<&Path>) -> Vec<String> {
    let mut result = argv.to_vec();
    if let Some(program) = result.first_mut() {
        *program = expand_home_with(program, home);
    }
    result
}

/// Confirmation runs off the hotkey pump and outside the 1500ms IPC budget.
pub fn request_quit(skip: bool, quit: impl FnOnce() + Send + 'static) -> Result<(), String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static PENDING: AtomicBool = AtomicBool::new(false);
    if skip {
        quit();
        return Ok(());
    }
    if PENDING.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let spawned = std::thread::Builder::new()
        .name("wiri-quit-confirmation".into())
        .spawn(move || {
            if confirm_quit() {
                quit();
            }
            PENDING.store(false, Ordering::Release);
        });
    if let Err(error) = spawned {
        PENDING.store(false, Ordering::Release);
        return Err(error.to_string());
    }
    Ok(())
}
fn confirm_quit() -> bool {
    if cfg!(test) || std::env::var_os("WIRI_SYSTEM_ACTIONS_DRY_RUN").is_some() {
        return false;
    }
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_ICONQUESTION, MB_SETFOREGROUND, MB_YESNO,
    };
    unsafe {
        MessageBoxW(
            None,
            windows::core::w!("Quit wiri?"),
            windows::core::w!("wiri"),
            MB_YESNO | MB_ICONQUESTION | MB_SETFOREGROUND,
        ) == IDYES
    }
}

pub(crate) fn execute(
    action: &SystemAction,
    engine: &Arc<parking_lot::RwLock<crate::layout::TilingEngine>>,
    backend: &crate::backend::BackendHandle,
    window: Option<isize>,
) -> Result<(), String> {
    match action {
        SystemAction::SwitchLayout(target) => crate::ipc::keyboard::switch_layout(*target),
        SystemAction::WindowUrgent(mode) => {
            let foreground = crate::layout::query_foreground();
            let mut engine = engine.write();
            let window = window
                .map(crate::utils::WindowId::new)
                .or_else(|| engine.window_for_non_focusing_action(foreground))
                .ok_or("no focused managed window")?;
            if !engine.tiled_windows().contains_key(&window) {
                return Err("window is not managed by wiri".into());
            }
            let urgent = match mode {
                Urgency::Set => true,
                Urgency::Unset => false,
                Urgency::Toggle => !engine.is_urgent(window),
            };
            // The foreground WinEvent may still be queued. Do not mark the
            // actual foreground application urgent, even with an explicit id.
            let is_foreground = foreground
                .and_then(|fg| engine.managed_window_for(fg.hwnd, fg.root_owner))
                == Some(window);
            engine.set_window_urgent(window, urgent && !is_foreground, backend);
            Ok(())
        }
        SystemAction::LoadConfigFile(path) => request_reload(path.clone()),
        SystemAction::PowerOffMonitors | SystemAction::PowerOnMonitors | SystemAction::Suspend => {
            request_power(action)
        }
        SystemAction::Quit { .. } => Err("quit requires the shutdown sender".into()),
    }
}

/// Accept real power requests off-thread: suspend does not return until wake, and
/// a broadcast's timeout is per recipient rather than a single global deadline.
fn request_power(action: &SystemAction) -> Result<(), String> {
    if cfg!(test) || std::env::var_os("WIRI_SYSTEM_ACTIONS_DRY_RUN").is_some() {
        return power(action);
    }
    let action = action.clone();
    std::thread::Builder::new()
        .name("wiri-power-action".into())
        .spawn(move || {
            if let Err(error) = power(&action) {
                tracing::warn!("Power action failed: {}", error);
            }
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn power(action: &SystemAction) -> Result<(), String> {
    use std::io::Write;
    let label = match action {
        SystemAction::PowerOffMonitors => "power-off-monitors",
        SystemAction::PowerOnMonitors => "power-on-monitors",
        SystemAction::Suspend => "suspend",
        _ => return Err("not a power action".into()),
    };
    // Unit tests must stay pure even when the live dry-run hook is configured.
    if cfg!(test) {
        return Ok(());
    }
    if let Some(path) = std::env::var_os("WIRI_SYSTEM_ACTIONS_DRY_RUN") {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        return writeln!(file, "{label}").map_err(|e| e.to_string());
    }
    if matches!(action, SystemAction::Suspend) {
        suspend()?;
    } else {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            SendMessageTimeoutW, HWND_BROADCAST, SC_MONITORPOWER, SMTO_ABORTIFHUNG, WM_SYSCOMMAND,
        };
        let state = if matches!(action, SystemAction::PowerOffMonitors) {
            2
        } else {
            -1
        };
        let result = unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SYSCOMMAND,
                WPARAM(SC_MONITORPOWER as usize),
                LPARAM(state),
                SMTO_ABORTIFHUNG,
                500,
                None,
            )
        };
        if result.0 == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}

/// SetSuspendState requires SeShutdownPrivilege. Restore the exact previous state
/// on both success and failure instead of leaving a process-wide privilege enabled.
fn suspend() -> Result<(), String> {
    use windows::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, ERROR_NOT_ALL_ASSIGNED};
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // Privilege changes affect the process token, so overlapping suspend workers
    // must not restore one another's state while a request is still in flight.
    static LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
    let _lock = LOCK.lock();
    struct ShutdownPrivilege {
        token: HANDLE,
        previous: Option<TOKEN_PRIVILEGES>,
    }
    impl Drop for ShutdownPrivilege {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                if let Err(error) = unsafe {
                    AdjustTokenPrivileges(self.token, false, Some(previous), 0, None, None)
                } {
                    tracing::warn!("Could not restore shutdown privilege: {}", error);
                }
            }
            if let Err(error) = unsafe { CloseHandle(self.token) } {
                tracing::warn!("Could not close shutdown token: {}", error);
            }
        }
    }

    let mut token = HANDLE::default();
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
    }
    .map_err(|error| format!("Could not open shutdown token: {error}"))?;
    let mut privilege = ShutdownPrivilege {
        token,
        previous: None,
    };
    let mut requested = TOKEN_PRIVILEGES::default();
    requested.PrivilegeCount = 1;
    requested.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
    unsafe {
        LookupPrivilegeValueW(
            None,
            windows::core::w!("SeShutdownPrivilege"),
            &mut requested.Privileges[0].Luid,
        )
    }
    .map_err(|error| format!("Could not resolve shutdown privilege: {error}"))?;
    let mut previous = TOKEN_PRIVILEGES::default();
    let mut returned = 0;
    unsafe {
        AdjustTokenPrivileges(
            token,
            false,
            Some(&requested),
            std::mem::size_of::<TOKEN_PRIVILEGES>() as u32,
            Some(&mut previous),
            Some(&mut returned),
        )
    }
    .map_err(|error| format!("Could not enable shutdown privilege: {error}"))?;
    let assignment_error = unsafe { GetLastError() };
    privilege.previous = Some(previous);
    // AdjustTokenPrivileges can succeed without assigning the requested privilege.
    if assignment_error == ERROR_NOT_ALL_ASSIGNED {
        return Err("SeShutdownPrivilege is not assigned to the current process".into());
    }

    #[link(name = "PowrProf")]
    extern "system" {
        fn SetSuspendState(hibernate: u8, force: u8, disable_wake: u8) -> u8;
    }
    if unsafe { SetSuspendState(0, 0, 0) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_system_arguments() {
        assert_eq!(
            parse("switch-layout", &["0".into()]),
            Some(Some(SystemAction::SwitchLayout(LayoutSwitchTarget::Index(
                0
            ))))
        );
        for (name, args) in [
            ("switch-layout", vec!["256".into()]),
            ("quit", vec!["junk".into()]),
            ("suspend", vec!["junk".into()]),
            ("load-config-file", vec!["a".into(), "b".into()]),
        ] {
            assert_eq!(parse(name, &args), Some(None));
        }
        assert_eq!(
            parse("quit", &["--skip-confirmation".into()]),
            Some(Some(SystemAction::Quit {
                skip_confirmation: true
            }))
        );
        for (value, expected) in [("true", true), ("false", false)] {
            assert_eq!(
                parse("quit", &["--skip-confirmation".into(), value.into()]),
                Some(Some(SystemAction::Quit {
                    skip_confirmation: expected
                }))
            );
        }
        assert_eq!(
            parse("quit", &["--skip-confirmation".into(), "yes".into()]),
            Some(None)
        );
        assert_eq!(
            parse("load-config-file", &[]),
            Some(Some(SystemAction::LoadConfigFile(None)))
        );
    }
    #[test]
    fn home_expansion_is_not_shell_expansion() {
        let home = Path::new("C:/Users/test");
        assert_eq!(expand_home_with("~", Some(home)), "C:/Users/test");
        assert_eq!(expand_home_with("~other/a", Some(home)), "~other/a");
        assert_eq!(expand_home_with("x/~/a", Some(home)), "x/~/a");
        assert_eq!(expand_home_with("~/a", None), "~/a");
        assert_eq!(
            PathBuf::from(expand_home_with("~/a", Some(home))),
            home.join("a")
        );
    }
    #[test]
    fn home_expansion_changes_only_the_executable() {
        let home = Path::new("C:/Users/test");
        let argv = vec![
            "~\\bin\\tool.exe".into(),
            "~/argument".into(),
            "%HOME%".into(),
            "a b".into(),
        ];
        let expanded = expand_spawn_argv_with(&argv, Some(home));
        assert_eq!(PathBuf::from(&expanded[0]), home.join("bin\\tool.exe"));
        assert_eq!(expanded[1..], argv[1..]);
        assert_eq!(expand_spawn_argv_with(&argv, None), argv);
        assert!(expand_spawn_argv_with(&[], Some(home)).is_empty());
    }
    #[test]
    fn typed_fields_are_validated() {
        let fields = serde_json::json!({"skip_confirmation": true});
        assert_eq!(
            wire_args("Quit", fields.as_object().unwrap())
                .unwrap()
                .unwrap(),
            vec!["--skip-confirmation"]
        );
        assert!(wire_args("Suspend", fields.as_object().unwrap())
            .unwrap()
            .is_err());
        assert!(wire_args(
            "SwitchLayout",
            serde_json::json!({"target": "Next"}).as_object().unwrap()
        )
        .unwrap()
        .is_ok());
        assert!(wire_args(
            "SetWindowUrgent",
            serde_json::json!({"id": -1}).as_object().unwrap()
        )
        .unwrap()
        .is_err());
    }
    #[test]
    fn typed_targets_and_optional_fields_are_preserved() {
        for target in [
            serde_json::json!("Prev"),
            serde_json::json!(0),
            serde_json::json!({"Index": 2}),
        ] {
            let fields = serde_json::json!({"target": target});
            let args = wire_args("SwitchLayout", fields.as_object().unwrap())
                .unwrap()
                .unwrap();
            assert!(matches!(
                parse("switch-layout", &args),
                Some(Some(SystemAction::SwitchLayout(_)))
            ));
        }
        let fields = serde_json::json!({"id": 42});
        assert_eq!(
            wire_args("SetWindowUrgent", fields.as_object().unwrap())
                .unwrap()
                .unwrap(),
            vec!["--id", "42"]
        );
        for fields in [serde_json::json!({}), serde_json::json!({"path": null})] {
            assert!(wire_args("LoadConfigFile", fields.as_object().unwrap())
                .unwrap()
                .unwrap()
                .is_empty());
        }
        let fields = serde_json::json!({"skip_confirmation": false});
        assert!(wire_args("Quit", fields.as_object().unwrap())
            .unwrap()
            .unwrap()
            .is_empty());
        for target in [
            serde_json::json!(-1),
            serde_json::json!(256),
            serde_json::json!({"Index": 1, "extra": 0}),
        ] {
            let fields = serde_json::json!({"target": target});
            assert!(wire_args("SwitchLayout", fields.as_object().unwrap())
                .unwrap()
                .is_err());
        }
    }
    #[test]
    fn quit_tests_never_display_a_dialog_and_skip_is_immediate() {
        use std::sync::atomic::{AtomicBool, Ordering};
        assert!(!confirm_quit());
        let quit = Arc::new(AtomicBool::new(false));
        let called = quit.clone();
        request_quit(true, move || called.store(true, Ordering::Release)).unwrap();
        assert!(quit.load(Ordering::Acquire));
    }
    #[test]
    fn power_is_safe_in_unit_tests() {
        for action in [
            SystemAction::PowerOffMonitors,
            SystemAction::PowerOnMonitors,
            SystemAction::Suspend,
        ] {
            request_power(&action).unwrap();
        }
    }
}
