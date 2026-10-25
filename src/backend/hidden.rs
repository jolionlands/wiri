//! Crash-safe record of the windows wiri has hidden and of where its windows were.
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
//!
//! The same file remembers, for every window wiri manages, the rectangle it had
//! before wiri first moved it.  A graceful quit puts the windows back there; after
//! a crash the restarted wiri meets the windows where the dead one left them (tiled,
//! or parked below the screen), and without this record "quit" could only guess:
//! it used to stack every window it found parked in the middle of the screen.

use crate::utils::Rect;
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

/// Where one managed window was before wiri first moved it (`GetWindowRect`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginalWindow {
    pub hwnd: isize,
    pub pid: u32,
    pub class: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl OriginalWindow {
    fn new(hwnd: isize, pid: u32, class: &str, rect: Rect) -> Self {
        Self {
            hwnd,
            pid,
            class: class.to_string(),
            x: rect.loc.x,
            y: rect.loc.y,
            w: rect.size.w,
            h: rect.size.h,
        }
    }

    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.w, self.h)
    }
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
    /// Where the managed windows were before wiri first moved them.  Files written
    /// before this existed have none.
    #[serde(default)]
    originals: Vec<OriginalWindow>,
}

/// Serialise the hidden set.
pub fn to_json(windows: &[HiddenWindow], wiri_pid: u32) -> String {
    to_json_with_originals(windows, &[], wiri_pid)
}

/// Serialise the hidden set and the original rectangles.
pub fn to_json_with_originals(
    windows: &[HiddenWindow],
    originals: &[OriginalWindow],
    wiri_pid: u32,
) -> String {
    let file = HiddenFile {
        wiri_pid,
        windows: windows.to_vec(),
        originals: originals.to_vec(),
    };
    serde_json::to_string_pretty(&file).unwrap_or_else(|_| "{}".to_string())
}

fn parse_file(text: &str) -> HiddenFile {
    serde_json::from_str::<HiddenFile>(text.trim_start_matches('\u{feff}')).unwrap_or_default()
}

/// Parse a file written by [`to_json`].  Anything unreadable yields an empty
/// list: a corrupt record must never stop wiri from starting.
pub fn from_json(text: &str) -> Vec<HiddenWindow> {
    parse_file(text).windows
}

/// The original rectangles in a file written by [`to_json_with_originals`] (none for
/// a file that has none, and none for a file that cannot be read).
pub fn originals_from_json(text: &str) -> Vec<OriginalWindow> {
    parse_file(text).originals
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

/// Which of the recorded original rectangles still belong to a window?  The handle has
/// to refer to a window of the same process and class (handles are recycled).
pub fn select_originals(
    entries: &[OriginalWindow],
    probe: impl Fn(isize) -> Option<WindowProbe>,
) -> Vec<OriginalWindow> {
    entries
        .iter()
        .filter(|entry| match probe(entry.hwnd) {
            Some(p) => p.pid == entry.pid && p.class == entry.class,
            None => false,
        })
        .cloned()
        .collect()
}

/// The set of windows hidden right now, where the managed windows were, and, when
/// persistence is on, the file that mirrors them.
struct Registry {
    windows: Vec<HiddenWindow>,
    /// Where every window wiri manages was before it first moved it.
    originals: Vec<OriginalWindow>,
    /// What the previous run recorded: a window it knew gets that rectangle back as its
    /// original when it is added again (it is not where it was: the dead wiri moved it).
    seeds: Vec<OriginalWindow>,
    path: Option<PathBuf>,
    /// While a [`Batch`] lives, changes only mark the record dirty.
    batch: u32,
    dirty: bool,
}

impl Registry {
    const fn new() -> Self {
        Self {
            windows: Vec::new(),
            originals: Vec::new(),
            seeds: Vec::new(),
            path: None,
            batch: 0,
            dirty: false,
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

    /// Take the previous run's record for exactly this window (same handle, process and
    /// class), if it had one.
    fn claim_seed(&mut self, hwnd: isize, pid: u32, class: &str) -> Option<OriginalWindow> {
        let at = self
            .seeds
            .iter()
            .position(|s| s.hwnd == hwnd && s.pid == pid && s.class == class)?;
        Some(self.seeds.swap_remove(at))
    }

    /// Note where `hwnd` was before wiri first moved it.  `bounds` is where the window is
    /// now; when the previous run knew the window, its record wins (the window is where
    /// the dead wiri put it).  Returns the rectangle to remember and whether the record
    /// changed.
    fn record_original(
        &mut self,
        hwnd: isize,
        pid: u32,
        class: &str,
        bounds: Rect,
    ) -> (Rect, bool) {
        let rect = self
            .claim_seed(hwnd, pid, class)
            .map(|seed| seed.rect())
            .unwrap_or(bounds);
        let entry = OriginalWindow::new(hwnd, pid, class, rect);
        let changed = match self.originals.iter_mut().find(|o| o.hwnd == hwnd) {
            Some(existing) if *existing == entry => false,
            Some(existing) => {
                *existing = entry;
                true
            }
            None => {
                self.originals.push(entry);
                true
            }
        };
        (rect, changed)
    }

    /// Forget the original rectangle of `hwnd` (and the previous run's, if unclaimed).
    /// Returns true when the record changed.
    fn remove_original(&mut self, hwnd: isize) -> bool {
        self.seeds.retain(|s| s.hwnd != hwnd);
        let before = self.originals.len();
        self.originals.retain(|o| o.hwnd != hwnd);
        self.originals.len() != before
    }

    /// Mirror the record into the file (or, inside a [`Batch`], remember to).
    fn persist(&mut self) {
        if self.batch > 0 {
            self.dirty = true;
            return;
        }
        self.write();
    }

    fn write(&mut self) {
        self.dirty = false;
        let Some(path) = &self.path else { return };
        if self.windows.is_empty() && self.originals.is_empty() {
            let _ = std::fs::remove_file(path);
            return;
        }
        let text = to_json_with_originals(&self.windows, &self.originals, std::process::id());
        if let Err(e) = write_atomic(path, &text) {
            warn!("could not record hidden windows in {}: {}", path.display(), e);
        }
    }
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry::new());

/// While a `Batch` is alive the record is not rewritten after every change; it is written
/// once, when the last batch is dropped.  The startup pass that meets every window on the
/// desktop uses one, so thirty windows are one file write, not thirty.
pub struct Batch(());

/// Start a [`Batch`].
pub fn batch() -> Batch {
    REGISTRY.lock().batch += 1;
    Batch(())
}

impl Drop for Batch {
    fn drop(&mut self) {
        let mut registry = REGISTRY.lock();
        registry.batch = registry.batch.saturating_sub(1);
        if registry.batch == 0 && registry.dirty {
            registry.write();
        }
    }
}

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

/// Mirror the record into `path` from now on.  Never called by unit tests,
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

/// Note where `hwnd` was before wiri first moved it, and return the rectangle the engine
/// should treat as that place: `bounds` (where the window is now), unless the previous wiri
/// run - which may have been killed, and moved the window - had recorded the window, in
/// which case its record is returned.
pub fn record_original(hwnd: isize, pid: u32, class: &str, bounds: Rect) -> Rect {
    let mut registry = REGISTRY.lock();
    let (rect, changed) = registry.record_original(hwnd, pid, class, bounds);
    if changed {
        registry.persist();
    }
    rect
}

/// Drop `hwnd` from the record because the window is gone.
pub fn forget(hwnd: isize) {
    let mut registry = REGISTRY.lock();
    let hidden = registry.remove(hwnd);
    let original = registry.remove_original(hwnd);
    if hidden || original {
        registry.persist();
    }
}

/// Every window has been put back where it was: nothing is left to remember.
pub fn clear_originals() {
    let mut registry = REGISTRY.lock();
    let had = !registry.originals.is_empty();
    registry.originals.clear();
    registry.seeds.clear();
    if had {
        registry.persist();
    }
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

/// Show every window recorded in `path` that is still hidden, and take over what the
/// record knows about where the windows were.  Run at startup, after persistence is
/// enabled and before wiri enumerates the desktop, so the windows a previous run
/// stranded come back and are tiled like any other.  The record is rewritten at once:
/// the windows shown again are no longer hidden, and the original rectangles of the
/// windows that are still there are kept (they are handed back to the engine by
/// [`record_original`] when it meets the windows again).
/// Returns how many windows were shown.
pub fn restore_from_file(path: &Path) -> usize {
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    let file = parse_file(&text);
    let entries = file.windows;
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
    let seeds = select_originals(&file.originals, probe_window);
    {
        let mut registry = REGISTRY.lock();
        registry.originals = seeds.clone();
        registry.seeds = seeds;
        if registry.path.as_deref() == Some(path) {
            registry.persist();
        } else {
            // Persistence is not on for this file: the old record is spent.
            let _ = std::fs::remove_file(path);
        }
    }
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
/// case where wiri was killed and could not run any of this.  The original
/// rectangles stay in the record: after a panic the windows are still where
/// wiri put them.
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

    fn orig(hwnd: isize, pid: u32, class: &str, x: i32, y: i32, w: u32, h: u32) -> OriginalWindow {
        OriginalWindow::new(hwnd, pid, class, Rect::new(x, y, w, h))
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
    fn test_original_rectangles_round_trip_with_the_hidden_set() {
        let windows = vec![win(100, 7, "Notepad")];
        let originals = vec![
            orig(100, 7, "Notepad", 120, 80, 800, 600),
            orig(200, 8, "CASCADIA_HOSTING_WINDOW_CLASS", -7, 0, 1380, 735),
        ];
        let text = to_json_with_originals(&windows, &originals, 9);
        assert_eq!(from_json(&text), windows);
        assert_eq!(originals_from_json(&text), originals);
        assert_eq!(originals[1].rect(), Rect::new(-7, 0, 1380, 735));
    }

    #[test]
    fn test_a_record_from_before_the_originals_existed_still_reads() {
        // wiri 0.x wrote only the hidden windows; reading such a file must not fail and
        // must not invent rectangles.
        let old = r#"{ "wiri_pid": 5, "windows": [ { "hwnd": 1, "pid": 2, "class": "X" } ] }"#;
        assert_eq!(from_json(old), vec![win(1, 2, "X")]);
        assert!(originals_from_json(old).is_empty());
        assert!(originals_from_json("garbage").is_empty());
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
    fn test_select_originals_keeps_only_windows_that_are_still_the_same_window() {
        let entries = vec![
            orig(1, 10, "A", 0, 0, 100, 100),  // same window, visible: keep
            orig(2, 10, "A", 0, 0, 100, 100),  // same window, hidden: keep (it is shown again)
            orig(3, 10, "A", 0, 0, 100, 100),  // handle recycled by another process: drop
            orig(4, 10, "A", 0, 0, 100, 100),  // handle now has another class: drop
            orig(5, 10, "A", 0, 0, 100, 100),  // window is gone: drop
        ];
        let probe = |hwnd: isize| match hwnd {
            1 | 2 => Some(WindowProbe {
                pid: 10,
                class: "A".into(),
                visible: hwnd == 1,
            }),
            3 => Some(WindowProbe {
                pid: 99,
                class: "A".into(),
                visible: true,
            }),
            4 => Some(WindowProbe {
                pid: 10,
                class: "B".into(),
                visible: true,
            }),
            _ => None,
        };
        let kept: Vec<isize> = select_originals(&entries, probe).iter().map(|o| o.hwnd).collect();
        assert_eq!(kept, vec![1, 2]);
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

    #[test]
    fn test_a_window_met_for_the_first_time_is_where_it_is() {
        let mut registry = Registry::new();
        let here = Rect::new(100, 120, 800, 600);
        let (rect, changed) = registry.record_original(7, 10, "A", here);
        assert_eq!(rect, here);
        assert!(changed);
        // Met again with the same rectangle: nothing to write.
        let (rect, changed) = registry.record_original(7, 10, "A", here);
        assert_eq!(rect, here);
        assert!(!changed);
        assert_eq!(registry.originals.len(), 1);
    }

    #[test]
    fn test_a_window_the_dead_wiri_knew_gets_its_old_place_back() {
        // The previous run recorded window 7 at (100,120) 800x600 and then moved it; the
        // restarted wiri meets it parked below the screen.
        let mut registry = Registry::new();
        registry.seeds = vec![orig(7, 10, "A", 100, 120, 800, 600)];
        let parked = Rect::new(4, 836, 445, 711);
        let (rect, changed) = registry.record_original(7, 10, "A", parked);
        assert_eq!(rect, Rect::new(100, 120, 800, 600), "the old record wins");
        assert!(changed);
        assert_eq!(registry.originals[0].rect(), rect, "and it is what is kept");
        assert!(registry.seeds.is_empty(), "a record is used once");
        // Another run of the same window (re-added after being hidden by its app): the
        // record is not replaced by wherever the window is now.
        let (again, changed) = registry.record_original(7, 10, "A", parked);
        assert_eq!(again, parked, "no seed left: the engine decides for itself");
        assert!(changed);
    }

    #[test]
    fn test_a_seed_is_not_given_to_another_window() {
        let mut registry = Registry::new();
        registry.seeds = vec![orig(7, 10, "A", 100, 120, 800, 600)];
        let here = Rect::new(0, 0, 300, 200);
        // The handle was recycled: another process, or another class.
        assert_eq!(registry.record_original(7, 11, "A", here).0, here);
        assert_eq!(registry.record_original(7, 10, "B", here).0, here);
        // Another handle altogether.
        assert_eq!(registry.record_original(8, 10, "A", here).0, here);
        assert_eq!(registry.seeds.len(), 1, "the record is still waiting for its window");
    }

    #[test]
    fn test_forgetting_a_window_drops_its_original_and_its_seed() {
        let mut registry = Registry::new();
        registry.seeds = vec![orig(7, 10, "A", 1, 2, 3, 4), orig(8, 10, "A", 1, 2, 3, 4)];
        registry.record_original(9, 10, "A", Rect::new(0, 0, 10, 10));
        assert!(registry.remove_original(9));
        assert!(!registry.remove_original(9), "nothing left to forget");
        // A window that was destroyed before the engine met it leaves no record behind.
        assert!(!registry.remove_original(7), "an unclaimed seed is not a change of the record");
        assert_eq!(registry.seeds, vec![orig(8, 10, "A", 1, 2, 3, 4)]);
    }

    #[test]
    fn test_the_record_survives_without_hidden_windows_and_goes_when_empty() {
        let dir = std::env::temp_dir().join(format!("wiri-orig-test-{}", std::process::id()));
        let path = dir.join("hidden.json");
        let mut registry = Registry::new();
        registry.path = Some(path.clone());
        registry.record_original(1, 10, "A", Rect::new(5, 6, 700, 500));
        registry.persist();
        // No window is hidden, but the originals are what a restarted wiri needs.
        let text = std::fs::read_to_string(&path).expect("record written");
        assert!(from_json(&text).is_empty());
        assert_eq!(originals_from_json(&text), vec![orig(1, 10, "A", 5, 6, 700, 500)]);
        registry.remove_original(1);
        registry.persist();
        assert!(!path.exists(), "an empty record is deleted");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_a_batch_writes_the_record_once() {
        let dir = std::env::temp_dir().join(format!("wiri-batch-test-{}", std::process::id()));
        let path = dir.join("hidden.json");
        let mut registry = Registry::new();
        registry.path = Some(path.clone());
        registry.batch = 1;
        for hwnd in 1..=5 {
            registry.record_original(hwnd, 10, "A", Rect::new(0, 0, 100, 100));
            registry.persist();
        }
        assert!(!path.exists(), "nothing is written while the batch lives");
        assert!(registry.dirty);
        registry.batch = 0;
        registry.write();
        let text = std::fs::read_to_string(&path).expect("record written");
        assert_eq!(originals_from_json(&text).len(), 5);
        assert!(!registry.dirty);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
