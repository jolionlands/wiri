pub mod hotkey;
pub mod grab;
pub mod mouse;
pub mod low_level_hook;
pub mod touch;

pub use hotkey::{HotkeyBinding, HotkeyId};
pub use grab::{MoveGrab, ResizeGrab, ResizeEdge};
pub use low_level_hook::{GrabState, begin_move_grab, begin_resize_grab, cancel_grab, is_grab_active, current_grab, start_mouse_hook, stop_mouse_hook};
pub use mouse::{MouseTracker, MouseFocusConfig};
pub use touch::{TouchConfig, TouchGesture, GestureRecognizer, start_touch_hook, stop_touch_hook};

/// All actions that can be triggered by hotkeys or IPC
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    Spawn(String),
    CloseWindow,
    MoveColumnLeft,
    MoveColumnRight,
    FocusColumnLeft,
    FocusColumnRight,
    FocusUp,
    FocusDown,
    FocusWorkspace(i32),
    MoveWorkspace(i32),
    ToggleFullscreen,
    ToggleFloating,
    Maximize,
    Minimize,
    CenterWindow,
    SwitchMonitor,
    Refresh,
    ScrollLeft,
    ScrollRight,
    /// Toggle overview mode (zoom out to see all columns)
    OverviewToggle,
    /// Navigate left in overview mode (wraps)
    OverviewLeft,
    /// Navigate right in overview mode (wraps)
    OverviewRight,
    /// Select focused window and exit overview
    OverviewSelect,
    /// Toggle tabbed layout for the focused column
    ColumnToggleTabbed,
    /// Focus the next tab in the focused column
    TabNext,
    /// Focus the previous tab in the focused column
    TabPrev,
    /// Move focused window to a specific workspace by ID
    MoveToWorkspace(i32),
    /// Move focused window to the monitor on the left
    MoveToMonitorLeft,
    /// Move focused window to the monitor on the right
    MoveToMonitorRight,
    /// Center the focused column on screen
    CenterColumn,
    /// Cycle through column width presets
    ColumnWidthPresetCycle,
    /// Set column width to 1/2
    ColumnWidthPresetHalf,
    /// Set column width to 1/3
    ColumnWidthPresetThird,
    /// Set column width to 2/3
    ColumnWidthPresetTwoThirds,
    /// Set column width to full
    ColumnWidthPresetFull,
    /// Shrink the focused column by 100 px
    ResizeColumnLeft,
    /// Grow the focused column by 100 px
    ResizeColumnRight,
    /// Focus the next workspace
    FocusWorkspaceNext,
    /// Focus the previous workspace
    FocusWorkspacePrevious,
    /// Focus the previously focused window (niri-style alt-tab / MRU history)
    FocusPrevious,
    /// Toggle always-on-top for the focused window
    ToggleAlwaysOnTop,
    /// Focus a named workspace from config
    FocusWorkspaceNamed(String),
    /// Set auto-tile threshold (None disables)
    SetAutoTileThreshold(Option<usize>),
    /// Capture the full virtual desktop to a BMP file under
    /// `%USERPROFILE%\Pictures\wiri-<timestamp>.bmp` (or the current working
    /// directory if Pictures isn't writable).
    Screenshot,
    /// Set the focused column to 1/4 of the work-area width.
    ColumnWidthPresetQuarter,
    /// Set the focused column to 3/4 of the work-area width.
    ColumnWidthPresetThreeQuarters,
    /// Take the focused tile out of its column and append it to the column
    /// on the right. No-op if already in the rightmost column.
    ConsumeWindowIntoColumn,
    /// Take the focused tile out of its column and place it in a new
    /// column immediately to the right of the source column.
    ExpelWindowFromColumn,
    /// Expand the focused column so it fills the leftover work-area width.
    ExpandColumnToAvailable,
    /// Toggle per-column maximize (full work-area height). Distinct from
    /// `ToggleFullscreen`, which covers the entire monitor.
    MaximizeColumn,
    /// Grow the focused column's width by 5% of the work area (clamped).
    GrowColumnWidth,
    /// Shrink the focused column's width by 5% of the work area (clamped).
    ShrinkColumnWidth,
    /// Grow the focused tile's height inside its column by 5% (clamped).
    GrowTileHeight,
    /// Shrink the focused tile's height inside its column by 5% (clamped).
    ShrinkTileHeight,
    /// Move the focused column wholesale to the monitor on the left.
    MoveColumnToMonitorLeft,
    /// Move the focused column wholesale to the monitor on the right.
    MoveColumnToMonitorRight,
}

/// Parse an action name string (from config/IPC) into an Action.
/// Returns None for unknown action names.
pub fn parse_action_name(name: &str, args: &[String]) -> Option<Action> {
    match name {
        "focus-column-left" | "focus-left" => Some(Action::FocusColumnLeft),
        "focus-column-right" | "focus-right" => Some(Action::FocusColumnRight),
        "focus-up" => Some(Action::FocusUp),
        "focus-down" => Some(Action::FocusDown),
        "move-column-left" | "move-left" => Some(Action::MoveColumnLeft),
        "move-column-right" | "move-right" => Some(Action::MoveColumnRight),
        "close-window" | "close" => Some(Action::CloseWindow),
        "toggle-fullscreen" | "fullscreen" => Some(Action::ToggleFullscreen),
        "toggle-floating" | "float" => Some(Action::ToggleFloating),
        "scroll-left" => Some(Action::ScrollLeft),
        "scroll-right" => Some(Action::ScrollRight),
        "quit" | "exit" => Some(Action::Quit),
        "maximize" => Some(Action::Maximize),
        "minimize" => Some(Action::Minimize),
        "center-window" => Some(Action::CenterWindow),
        "switch-monitor" => Some(Action::SwitchMonitor),
        "refresh" => Some(Action::Refresh),
        "switch-workspace" | "workspace" => {
            args.first().and_then(|s| s.parse::<i32>().ok()).map(Action::FocusWorkspace)
        }
        "move-workspace" => {
            args.first().and_then(|s| s.parse::<i32>().ok()).map(Action::MoveWorkspace)
        }
        "spawn" | "exec" => {
            args.first().map(|cmd| Action::Spawn(cmd.clone()))
        }
        "overview" | "overview-toggle" | "zoom-out" => Some(Action::OverviewToggle),
        "overview-left" => Some(Action::OverviewLeft),
        "overview-right" => Some(Action::OverviewRight),
        "overview-select" | "overview-accept" => Some(Action::OverviewSelect),
        "column-toggle-tabbed" => Some(Action::ColumnToggleTabbed),
        "tab-next" => Some(Action::TabNext),
        "tab-prev" => Some(Action::TabPrev),
        "move-to-workspace" => {
            args.first().and_then(|s| s.parse::<i32>().ok()).map(Action::MoveToWorkspace)
        }
        "move-to-monitor-left" => Some(Action::MoveToMonitorLeft),
        "move-to-monitor-right" => Some(Action::MoveToMonitorRight),
        "center-column" => Some(Action::CenterColumn),
        "column-width-preset" | "column-width-cycle" => Some(Action::ColumnWidthPresetCycle),
        "column-width-half" | "column-width-1/2" => Some(Action::ColumnWidthPresetHalf),
        "column-width-third" | "column-width-1/3" => Some(Action::ColumnWidthPresetThird),
        "column-width-two-thirds" | "column-width-2/3" => Some(Action::ColumnWidthPresetTwoThirds),
        "column-width-full" | "column-width-100" => Some(Action::ColumnWidthPresetFull),
        "column-width-quarter" | "column-width-1/4" => Some(Action::ColumnWidthPresetQuarter),
        "column-width-three-quarters" | "column-width-3/4" => {
            Some(Action::ColumnWidthPresetThreeQuarters)
        }
        "consume-window-into-column" | "consume-window" | "consume-into-column" => {
            Some(Action::ConsumeWindowIntoColumn)
        }
        "expel-window-from-column" | "expel-window" | "expel-from-column" => {
            Some(Action::ExpelWindowFromColumn)
        }
        "expand-column-to-available" | "expand-column" => Some(Action::ExpandColumnToAvailable),
        "maximize-column" | "column-maximize" | "toggle-column-maximize" => {
            Some(Action::MaximizeColumn)
        }
        // Percentage-based column resize (Ctrl+Alt+Shift+L/H). Distinct from
        // `resize-column-left/right` (pixel-based) further down — different
        // resolution + different clamping, so we keep both spellings live.
        "grow-column-width" | "grow-column-pct" => Some(Action::GrowColumnWidth),
        "shrink-column-width" | "shrink-column-pct" => Some(Action::ShrinkColumnWidth),
        "grow-tile-height" | "grow-tile" => Some(Action::GrowTileHeight),
        "shrink-tile-height" | "shrink-tile" => Some(Action::ShrinkTileHeight),
        "move-column-to-monitor-left" => Some(Action::MoveColumnToMonitorLeft),
        "move-column-to-monitor-right" => Some(Action::MoveColumnToMonitorRight),
        "resize-column-left" | "shrink-column" => Some(Action::ResizeColumnLeft),
        "resize-column-right" | "grow-column" => Some(Action::ResizeColumnRight),
        "focus-workspace-next" | "workspace-next" => Some(Action::FocusWorkspaceNext),
        "focus-workspace-previous" | "focus-workspace-prev" | "workspace-prev" => Some(Action::FocusWorkspacePrevious),
        "focus-previous" | "alt-tab" | "focus-history-back" => Some(Action::FocusPrevious),
        "toggle-always-on-top" | "always-on-top" | "topmost" => Some(Action::ToggleAlwaysOnTop),
        "focus-workspace-named" | "focus-workspace" => {
            args.first().map(|s| Action::FocusWorkspaceNamed(s.clone()))
        }
        "screenshot" | "take-screenshot" | "capture-screen" => Some(Action::Screenshot),
        "set-auto-tile" => {
            if args.is_empty() {
                None
            } else {
                let raw = args[0].to_lowercase();
                match raw.as_str() {
                    "off" | "none" => Some(Action::SetAutoTileThreshold(None)),
                    v => v.parse::<usize>().ok().map(|n| {
                        Action::SetAutoTileThreshold(if n == 0 { None } else { Some(n) })
                    }),
                }
            }
        }
        _ => None,
    }
}

/// Parse a key name string (from config) into a Windows virtual key code.
pub fn parse_key_name(name: &str) -> Option<u32> {
    let k = name.to_lowercase();
    // Letters: a-z → 0x41-0x5A
    // Use chars().count() (not .len()) so the guard is correct for multi-byte UTF-8.
    if k.chars().count() == 1 {
        // Safety: count() == 1 guarantees exactly one char exists.
        let ch = k.chars().next().unwrap_or('\0');
        if ch.is_ascii_alphabetic() {
            return Some(0x41 + (ch as u32) - ('a' as u32));
        }
        if ch.is_ascii_digit() {
            return Some(0x30 + (ch as u32) - ('0' as u32));
        }
    }
    match k.as_str() {
        "left" | "arrow-left" => Some(0x25),
        "right" | "arrow-right" => Some(0x27),
        "up" | "arrow-up" => Some(0x26),
        "down" | "arrow-down" => Some(0x28),
        "enter" | "return" => Some(0x0D),
        "space" => Some(0x20),
        "tab" => Some(0x09),
        "escape" | "esc" => Some(0x1B),
        "backspace" => Some(0x08),
        "delete" | "del" => Some(0x2E),
        "home" => Some(0x24),
        "end" => Some(0x23),
        "page-up" | "prior" => Some(0x21),
        "page-down" | "next" => Some(0x22),
        "insert" | "ins" => Some(0x2D),
        "caps-lock" => Some(0x14),
        "num-lock" => Some(0x90),
        "f1" => Some(0x70), "f2" => Some(0x71), "f3" => Some(0x72),
        "f4" => Some(0x73), "f5" => Some(0x74), "f6" => Some(0x75),
        "f7" => Some(0x76), "f8" => Some(0x77), "f9" => Some(0x78),
        "f10" => Some(0x79), "f11" => Some(0x7A), "f12" => Some(0x7B),
        "print" | "print-screen" => Some(0x2C),
        "scroll-lock" => Some(0x91),
        "pause" => Some(0x13),
        _ => None,
    }
}

/// Parse modifier name strings into Windows MOD_* flags.
pub fn parse_modifiers(names: &[String]) -> u32 {
    let mut flags = 0u32;
    for m in names {
        match m.to_lowercase().as_str() {
            "ctrl" | "control" => flags |= 0x0002, // MOD_CTRL
            "alt" => flags |= 0x0001,              // MOD_ALT
            "shift" => flags |= 0x0004,            // MOD_SHIFT
            "super" | "win" | "meta" => flags |= 0x0008, // MOD_WIN
            _ => {}
        }
    }
    flags
}

/// Modifier keys used in hotkey bindings
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModKey {
    Alt,
    Ctrl,
    Shift,
    Win,
}

/// Keyboard configuration
#[derive(Debug, Clone)]
pub struct KeyboardConfig {
    pub enabled: bool,
    pub repeat_rate: u32,
    pub repeat_delay: u32,
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self { enabled: true, repeat_rate: 30, repeat_delay: 500 }
    }
}

/// Mouse configuration
#[derive(Debug, Clone)]
pub struct MouseConfig {
    pub sensitivity: f32,
    pub pointer_visibility: PointerVisibility,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self { sensitivity: 1.0, pointer_visibility: PointerVisibility::Visible }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerVisibility {
    Visible,
    Hidden,
    Disabled,
}

/// Returns the key-repeat debounce interval in milliseconds derived from
/// the flat `crate::config::InputConfig`.  The formula is `1000 / repeat_rate`
/// (clamped to a minimum of 1 rep/s so we never divide by zero).
///
/// Used by the backend message loop in place of a hardcoded 125 ms.
pub fn repeat_debounce_ms(config: &crate::config::InputConfig) -> u64 {
    let rate = config.repeat_rate.max(1);
    (1000 / rate) as u64
}

use crate::config::types::InputConfig;
use tracing::info as _info;

/// Logs which `InputConfig` mouse fields are present and updates the global
/// mouse config snapshot consulted by the low-level hook.
///
/// `natural_scroll` and `mouse_speed` are read by the WM_MOUSEWHEEL handler in
/// `low_level_hook` on every wheel event. `mouse_acceleration`/`tap_to_click`
/// remain advisory because Windows mouse pointer acceleration is a system-wide
/// setting (SystemParametersInfo SPI_SETMOUSE), not a per-process flag.
pub fn apply_mouse_config(config: &InputConfig) {
    _info!(
        "apply_mouse_config: mouse_speed={}, mouse_acceleration={}, \
         natural_scroll={}, tap_to_click={}",
        config.mouse_speed,
        config.mouse_acceleration,
        config.natural_scroll,
        config.tap_to_click,
    );
    set_mouse_runtime_config(MouseRuntimeConfig {
        natural_scroll: config.natural_scroll,
        scroll_speed: config.mouse_speed as f32,
    });
}

/// Snapshot of mouse-runtime settings consulted by the low-level wheel hook.
/// Kept deliberately small so it can be copied out of the static under a
/// short lock-hold without performance impact.
#[derive(Debug, Clone, Copy)]
pub struct MouseRuntimeConfig {
    pub natural_scroll: bool,
    pub scroll_speed: f32,
}

impl Default for MouseRuntimeConfig {
    fn default() -> Self {
        Self { natural_scroll: false, scroll_speed: 1.0 }
    }
}

static MOUSE_RUNTIME: std::sync::LazyLock<parking_lot::Mutex<MouseRuntimeConfig>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(MouseRuntimeConfig::default()));

/// Update the global mouse-runtime snapshot (called on config load + reload).
pub fn set_mouse_runtime_config(cfg: MouseRuntimeConfig) {
    *MOUSE_RUNTIME.lock() = cfg;
}

/// Read the global mouse-runtime snapshot (called from the WM_MOUSEWHEEL hook).
pub fn mouse_runtime_config() -> MouseRuntimeConfig {
    *MOUSE_RUNTIME.lock()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_action_focus() {
        assert_eq!(parse_action_name("focus-column-left", &[]), Some(Action::FocusColumnLeft));
        assert_eq!(parse_action_name("focus-left", &[]), Some(Action::FocusColumnLeft));
        assert_eq!(parse_action_name("focus-column-right", &[]), Some(Action::FocusColumnRight));
        assert_eq!(parse_action_name("focus-right", &[]), Some(Action::FocusColumnRight));
        assert_eq!(parse_action_name("focus-up", &[]), Some(Action::FocusUp));
        assert_eq!(parse_action_name("focus-down", &[]), Some(Action::FocusDown));
    }

    #[test]
    fn test_parse_action_move() {
        assert_eq!(parse_action_name("move-column-left", &[]), Some(Action::MoveColumnLeft));
        assert_eq!(parse_action_name("move-left", &[]), Some(Action::MoveColumnLeft));
        assert_eq!(parse_action_name("move-column-right", &[]), Some(Action::MoveColumnRight));
        assert_eq!(parse_action_name("move-right", &[]), Some(Action::MoveColumnRight));
    }

    #[test]
    fn test_parse_action_misc() {
        assert_eq!(parse_action_name("close-window", &[]), Some(Action::CloseWindow));
        assert_eq!(parse_action_name("close", &[]), Some(Action::CloseWindow));
        assert_eq!(parse_action_name("toggle-fullscreen", &[]), Some(Action::ToggleFullscreen));
        assert_eq!(parse_action_name("fullscreen", &[]), Some(Action::ToggleFullscreen));
        assert_eq!(parse_action_name("toggle-floating", &[]), Some(Action::ToggleFloating));
        assert_eq!(parse_action_name("float", &[]), Some(Action::ToggleFloating));
        assert_eq!(parse_action_name("quit", &[]), Some(Action::Quit));
        assert_eq!(parse_action_name("exit", &[]), Some(Action::Quit));
    }

    #[test]
    fn test_parse_action_scroll() {
        assert_eq!(parse_action_name("scroll-left", &[]), Some(Action::ScrollLeft));
        assert_eq!(parse_action_name("scroll-right", &[]), Some(Action::ScrollRight));
    }

    #[test]
    fn test_parse_action_overview() {
        assert_eq!(parse_action_name("overview", &[]), Some(Action::OverviewToggle));
        assert_eq!(parse_action_name("overview-toggle", &[]), Some(Action::OverviewToggle));
        assert_eq!(parse_action_name("zoom-out", &[]), Some(Action::OverviewToggle));
        assert_eq!(parse_action_name("overview-left", &[]), Some(Action::OverviewLeft));
        assert_eq!(parse_action_name("overview-right", &[]), Some(Action::OverviewRight));
        assert_eq!(parse_action_name("overview-select", &[]), Some(Action::OverviewSelect));
        assert_eq!(parse_action_name("overview-accept", &[]), Some(Action::OverviewSelect));
    }

    #[test]
    fn test_parse_action_with_args() {
        assert_eq!(parse_action_name("switch-workspace", &["3".to_string()]), Some(Action::FocusWorkspace(3)));
        assert_eq!(parse_action_name("workspace", &["1".to_string()]), Some(Action::FocusWorkspace(1)));
        assert_eq!(parse_action_name("move-workspace", &["2".to_string()]), Some(Action::MoveWorkspace(2)));
        assert_eq!(parse_action_name("spawn", &["cmd.exe".to_string()]), Some(Action::Spawn("cmd.exe".to_string())));
        assert_eq!(parse_action_name("exec", &["notepad".to_string()]), Some(Action::Spawn("notepad".to_string())));
    }

    #[test]
    fn test_parse_action_with_missing_args() {
        assert_eq!(parse_action_name("switch-workspace", &[]), None);
        assert_eq!(parse_action_name("spawn", &[]), None);
    }

    #[test]
    fn test_parse_action_invalid() {
        assert_eq!(parse_action_name("nonexistent", &[]), None);
        assert_eq!(parse_action_name("", &[]), None);
    }

    #[test]
    fn test_parse_key_letters() {
        assert_eq!(parse_key_name("a"), Some(0x41));
        assert_eq!(parse_key_name("Z"), Some(0x5A));
        assert_eq!(parse_key_name("q"), Some(0x51));
    }

    #[test]
    fn test_parse_key_digits() {
        assert_eq!(parse_key_name("0"), Some(0x30));
        assert_eq!(parse_key_name("9"), Some(0x39));
    }

    #[test]
    fn test_parse_key_arrows() {
        assert_eq!(parse_key_name("Left"), Some(0x25));
        assert_eq!(parse_key_name("Right"), Some(0x27));
        assert_eq!(parse_key_name("Up"), Some(0x26));
        assert_eq!(parse_key_name("Down"), Some(0x28));
        assert_eq!(parse_key_name("arrow-left"), Some(0x25));
    }

    #[test]
    fn test_parse_key_special() {
        assert_eq!(parse_key_name("Enter"), Some(0x0D));
        assert_eq!(parse_key_name("Return"), Some(0x0D));
        assert_eq!(parse_key_name("Space"), Some(0x20));
        assert_eq!(parse_key_name("Tab"), Some(0x09));
        assert_eq!(parse_key_name("Escape"), Some(0x1B));
        assert_eq!(parse_key_name("Esc"), Some(0x1B));
        assert_eq!(parse_key_name("Delete"), Some(0x2E));
        assert_eq!(parse_key_name("Del"), Some(0x2E));
    }

    #[test]
    fn test_parse_key_function() {
        assert_eq!(parse_key_name("F1"), Some(0x70));
        assert_eq!(parse_key_name("F12"), Some(0x7B));
    }

    #[test]
    fn test_parse_key_invalid() {
        assert_eq!(parse_key_name("invalid"), None);
        assert_eq!(parse_key_name(""), None);
    }

    #[test]
    fn test_parse_modifiers_standard() {
        assert_eq!(parse_modifiers(&["Ctrl".to_string()]), 0x0002);
        assert_eq!(parse_modifiers(&["Alt".to_string()]), 0x0001);
        assert_eq!(parse_modifiers(&["Shift".to_string()]), 0x0004);
        assert_eq!(parse_modifiers(&["Super".to_string()]), 0x0008);
        assert_eq!(parse_modifiers(&["Win".to_string()]), 0x0008);
        assert_eq!(parse_modifiers(&["Meta".to_string()]), 0x0008);
    }

    #[test]
    fn test_parse_modifiers_combined() {
        let mods = parse_modifiers(&["Ctrl".to_string(), "Alt".to_string()]);
        assert_eq!(mods, 0x0002 | 0x0001);

        let mods = parse_modifiers(&["Ctrl".to_string(), "Alt".to_string(), "Shift".to_string()]);
        assert_eq!(mods, 0x0002 | 0x0001 | 0x0004);
    }

    #[test]
    fn test_parse_modifiers_case_insensitive() {
        assert_eq!(parse_modifiers(&["ctrl".to_string()]), 0x0002);
        assert_eq!(parse_modifiers(&["CTRL".to_string()]), 0x0002);
        assert_eq!(parse_modifiers(&["Control".to_string()]), 0x0002);
    }

    #[test]
    fn test_parse_modifiers_empty() {
        assert_eq!(parse_modifiers(&[]), 0);
    }

    #[test]
    fn test_parse_modifiers_invalid() {
        assert_eq!(parse_modifiers(&["Invalid".to_string()]), 0);
    }
}
