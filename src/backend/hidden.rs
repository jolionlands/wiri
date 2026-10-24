//! Crash-safe record of the windows wiri has hidden.
//!
//! Windows on inactive workspaces are hidden with `SW_HIDE`.  A hidden window
//! has no taskbar button and cannot be reached with Alt+Tab, so if wiri dies
//! (crash, `taskkill`, power loss while hidden) those windows - and whatever
//! unsaved work they hold - are stranded.  Every hide is therefore written to
//! `%LOCALAPPDATA%\wiri\hidden.json` as it happens, and the next start (or a
//! graceful quit, or a panic) shows them again.
//!
//! The file stores `(hwnd, pid, window class)`: HWND values are recycled by
//! Windows, so before un-hiding anything after a restart the entry is checked
//! against the live window, and a recycled handle that now belongs to a
//! different process or class is left alone.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// One window wiri has hidden.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HiddenWindow {
    pub hwnd: isize,
    pub pid: u32,
    pub class: String,
}

/// What is currently known about a window handle (for restore decisions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowProbe {
    pub pid: u32,
    pub class: String,
    pub visible: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct HiddenFile {
    /// The wiri process that wrote the file (diagnostics only).
    wiri_pid: u32,
    windows: Vec<HiddenWindow>,
}

/// Serialise the hidden set.
pub fn to_json(windows: &[HiddenWindow], wiri_pid: u32) -> String {
    let file = HiddenFile {
        wiri_pid,
        windows: windows.to_vec(),
    };
    serde_json::to_string_pretty(&file).unwrap_or_else(|_| "{}".to_string())
}

/// Parse a file written by [`to_json`].  Anything unreadable yields an empty
/// list: a corrupt record must never stop wiri from starting.
pub fn from_json(text: &str) -> Vec<HiddenWindow> {
    serde_json::from_str::<HiddenFile>(text.trim_start_matches('\u{feff}'))
        .map(|f| f.windows)
        .unwrap_or_default()
}

/// Which of the recorded windows should be shown again?
///
/// An entry qualifies when the handle still refers to a window, that window
/// belongs to the same process and has the same class as when it was hidden
/// (guards against recycled handles), and it is currently not visible.
pub fn select_restorable(
    entries: &[HiddenWindow],
    probe: impl Fn(isize) -> Option<WindowProbe>,
) -> Vec<isize> {
    entries
        .iter()
        .filter(|entry| match probe(entry.hwnd) {
            Some(p) => !p.visible && p.pid == entry.pid && p.class == entry.class,
            None => false,
        })
        .map(|entry| entry.hwnd)
        .collect()
}

/// The set of windows hidden right now plus, when persistence is on, the file
/// that mirrors it.
struct Registry {
    windows: Vec<HiddenWindow>,
    path: Option<PathBuf>,
}

impl Registry {
    const fn new() -> Self {
        Self {
            windows: Vec::new(),
            path: None,
        }
    }

    /// Record `window` as hidden.  Returns true when the set changed.
    fn insert(&mut self, window: HiddenWindow) -> bool {
        if let Some(existing) = self.windows.iter_mut().find(|w| w.hwnd == window.hwnd) {
            if *existing == window {
                return false;
            }
            *existing = window;
            return true;
        }
        self.windows.push(window);
        true
    }

    /// Forget `hwnd`.  Returns true when the set changed.
    fn remove(&mut self, hwnd: isize) -> bool {
        let before = self.windows.len();
        self.windows.retain(|w| w.hwnd != hwnd);
        self.windows.len() != before
    }

    fn persist(&self) {
        let Some(path) = &self.path else { return };
        if self.windows.is_empty() {
            let _ = std::fs::remove_file(path);
            return;
        }
        if let Err(e) = write_atomic(path, &to_json(&self.windows, std::process::id())) {
            warn!("could not record hidden windows in {}: {}", path.display(), e);
        }
    }
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry::new());

/// Write `text` to `path` through a temporary file so a crash mid-write can
/// never leave a half-written record behind.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// `%LOCALAPPDATA%\wiri\hidden.json`.
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("wiri").join("hidden.json"))
}

/// Mirror the hidden set into `path` from now on.  Never called by unit tests,
/// which therefore never touch the user's profile.
pub fn enable_persistence(path: PathBuf) {
    REGISTRY.lock().path = Some(path);
}

/// Note that wiri hid (`hidden = true`) or showed (`false`) `hwnd`.
pub fn note_hidden(hwnd: isize, hidden: bool) {
    let mut registry = REGISTRY.lock();
    // The layout pass re-hides inactive-workspace windows every few hundred
    // milliseconds; a window that is already recorded needs no Win32 lookup.
    if hidden && registry.windows.iter().any(|w| w.hwnd == hwnd) {
        return;
    }
    let changed = if hidden {
        registry.insert(probe_window(hwnd).map_or_else(
            || HiddenWindow {
                hwnd,
                pid: 0,
                class: String::new(),
            },
            |p| HiddenWindow {
                hwnd,
                pid: p.pid,
                class: p.class,
            },
        ))
    } else {
        registry.remove(hwnd)
    };
    if changed {
        registry.persist();
    }
}

/// Drop `hwnd` from the record because the window is gone.
pub fn forget(hwnd: isize) {
    note_hidden(hwnd, false);
}

/// The windows wiri currently has hidden.
pub fn snapshot() -> Vec<HiddenWindow> {
    REGISTRY.lock().windows.clone()
}

/// Look `hwnd` up in Windows.  `None` when it is not a window any more.
pub fn probe_window(hwnd: isize) -> Option<WindowProbe> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
    };
    let h = HWND(hwnd as *mut std::ffi::c_void);
    unsafe {
        if !IsWindow(h).as_bool() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let mut buf = [0u16; 256];
        let len = GetClassNameW(h, &mut buf);
        let class = String::from_utf16_lossy(&buf[..(len.max(0) as usize).min(buf.len())]);
        Some(WindowProbe {
            pid,
            class,
            visible: IsWindowVisible(h).as_bool(),
        })
    }
}

/// Show `hwnd` again without activating it.
fn show_again(hwnd: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindowAsync, SW_SHOWNA};
    unsafe {
        let _ = ShowWindowAsync(HWND(hwnd as *mut std::ffi::c_void), SW_SHOWNA);
    }
}

/// Show every window recorded in `path` that is still hidden, then delete the
/// record.  Run at startup, before wiri enumerates the desktop, so the windows
/// a previous run stranded come back and are tiled like any other.
/// Returns how many windows were shown.
pub fn restore_from_file(path: &Path) -> usize {
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    let entries = from_json(&text);
    let restorable = select_restorable(&entries, probe_window);
    for hwnd in &restorable {
        show_again(*hwnd);
    }
    // `ShowWindowAsync` only posts the request; give the owning threads a
    // moment to act on it so the desktop enumeration that follows sees the
    // windows as visible.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
    while std::time::Instant::now() < deadline {
        let all_visible = restorable
            .iter()
            .all(|hwnd| probe_window(*hwnd).map_or(true, |p| p.visible));
        if all_visible {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let _ = std::fs::remove_file(path);
    if !entries.is_empty() {
        info!(
            "Previous run left {} hidden window(s) behind; showed {} of them again",
            entries.len(),
            restorable.len()
        );
    }
    restorable.len()
}

/// Show every window wiri currently has hidden and clear the record.  Used on
/// graceful quit and from the panic hook; `restore_from_file` handles the
/// case where wiri was killed and could not run any of this.
pub fn restore_all_hidden() -> usize {
    let windows = {
        let mut registry = REGISTRY.lock();
        let windows = std::mem::take(&mut registry.windows);
        registry.persist();
        windows
    };
    for window in &windows {
        show_again(window.hwnd);
    }
    windows.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(hwnd: isize, pid: u32, class: &str) -> HiddenWindow {
        HiddenWindow {
            hwnd,
            pid,
            class: class.to_string(),
        }
    }

    #[test]
    fn test_json_round_trip() {
        let windows = vec![win(100, 7, "Notepad"), win(200, 8, "CASCADIA_HOSTING_WINDOW_CLASS")];
        let text = to_json(&windows, 4242);
        assert!(text.contains("\"wiri_pid\": 4242"));
        assert_eq!(from_json(&text), windows);
    }

    #[test]
    fn test_from_json_tolerates_garbage_and_bom() {
        assert!(from_json("").is_empty());
        assert!(from_json("not json at all").is_empty());
        assert!(from_json("{\"windows\": 12}").is_empty());
        let with_bom = format!("\u{feff}{}", to_json(&[win(1, 2, "X")], 3));
        assert_eq!(from_json(&with_bom), vec![win(1, 2, "X")]);
    }

    #[test]
    fn test_select_restorable_checks_pid_class_and_visibility() {
        let entries = vec![
            win(1, 10, "A"), // hidden, same pid + class: restore
            win(2, 10, "A"), // already visible again: leave
            win(3, 10, "A"), // handle recycled by another process: leave
            win(4, 10, "A"), // handle now has another class: leave
            win(5, 10, "A"), // window is gone: leave
        ];
        let probe = |hwnd: isize| match hwnd {
            1 => Some(WindowProbe {
                pid: 10,
                class: "A".into(),
                visible: false,
            }),
            2 => Some(WindowProbe {
                pid: 10,
                class: "A".into(),
                visible: true,
            }),
            3 => Some(WindowProbe {
                pid: 99,
                class: "A".into(),
                visible: false,
            }),
            4 => Some(WindowProbe {
                pid: 10,
                class: "B".into(),
                visible: false,
            }),
            _ => None,
        };
        assert_eq!(select_restorable(&entries, probe), vec![1]);
    }

    #[test]
    fn test_registry_tracks_hides_and_shows_without_duplicates() {
        let mut registry = Registry::new();
        assert!(registry.insert(win(1, 10, "A")));
        // The idle layout pass re-hides inactive windows every few hundred ms;
        // that must not count as a change (it would rewrite the file each time).
        assert!(!registry.insert(win(1, 10, "A")));
        assert!(registry.insert(win(2, 11, "B")));
        assert_eq!(registry.windows.len(), 2);
        assert!(registry.remove(1));
        assert!(!registry.remove(1));
        assert_eq!(registry.windows, vec![win(2, 11, "B")]);
    }

    #[test]
    fn test_registry_persists_only_when_a_path_is_set() {
        // Without a path nothing is written (this is the unit-test default).
        let mut registry = Registry::new();
        registry.insert(win(1, 10, "A"));
        registry.persist(); // must not panic or create files

        let dir = std::env::temp_dir().join(format!("wiri-hidden-test-{}", std::process::id()));
        let path = dir.join("hidden.json");
        registry.path = Some(path.clone());
        registry.persist();
        let text = std::fs::read_to_string(&path).expect("record written");
        assert_eq!(from_json(&text), vec![win(1, 10, "A")]);
        // Emptying the set removes the record entirely.
        registry.remove(1);
        registry.persist();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
