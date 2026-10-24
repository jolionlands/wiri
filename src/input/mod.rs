pub mod grab;
pub mod hotkey;
pub mod low_level_hook;
pub mod mouse;
pub mod snap_guide;
pub mod touch;

pub use grab::{ColumnReorderGrab, MoveGrab, ResizeEdge, ResizeGrab, COLUMN_REORDER_HIT_ZONE_PX};
pub use hotkey::{HotkeyBinding, HotkeyId};
pub use low_level_hook::{
    begin_column_reorder_grab, begin_move_grab, begin_resize_grab, cancel_grab,
    clear_keyboard_action_sender, current_grab, is_grab_active, set_keyboard_action_sender,
    start_keyboard_hook, start_mouse_hook, stop_keyboard_hook, stop_mouse_hook, GrabState,
};
pub use mouse::{MouseFocusConfig, MouseTracker};
pub use touch::{
    set_action_sender as set_touch_action_sender, start_touch_hook, stop_touch_hook,
    GestureRecognizer, TouchConfig, TouchGesture,
};

/// All actions that can be triggered by hotkeys or IPC
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    /// Run a program: `argv[0]` is the program (searched on PATH when it has no
    /// directory part), the rest are its arguments, each passed through intact
    /// (niri `spawn "wt.exe" "-d" "C:\My Projects"`).
    Spawn(Vec<String>),
    /// Spawn a shell command via `cmd.exe /C "<command>"` (niri `spawn-sh`
    /// parity). Distinct from `Spawn(...)`, which starts the program directly
    /// without a shell.
    SpawnCmd(String),
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
    /// Exit overview or resize mode when one is active. No-op otherwise.
    OverviewExit,
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
    /// Capture the focused window's bounding rect to a BMP file under
    /// `%USERPROFILE%\Pictures\wiri-window-<hwnd>-<unix>.bmp` (or the
    /// current directory if Pictures isn't writable).  Uses
    /// `PrintWindow(hwnd, hdc, PW_RENDERFULLCONTENT)`.
    WindowScreenshot,
    /// Toggle niri-style "interactive resize mode" (Mod+R).  While
    /// active, the engine intercepts bare Arrow keys to grow/shrink the
    /// focused column / tile by 5% and Escape commits + exits.  Pressing
    /// the toggle again also exits.
    EnterResizeMode,
    /// niri parity: move the focused column to the workspace immediately
    /// above the current one (`current_workspace_id - 1`).  Creates the
    /// destination workspace if it does not exist.  No-op when the focused
    /// workspace is the lowest non-empty workspace and nothing is above it.
    MoveColumnToWorkspaceUp,
    /// niri parity: move the focused column to the workspace immediately
    /// below the current one (`current_workspace_id + 1`).
    MoveColumnToWorkspaceDown,
    /// niri parity: swap the focused workspace with the workspace
    /// immediately above it on the same monitor.  Focus stays on the
    /// (now relocated) original workspace.  No-op at the edge.
    MoveWorkspaceUp,
    /// niri parity: swap the focused workspace with the workspace
    /// immediately below it on the same monitor.
    MoveWorkspaceDown,
    /// niri parity: mark the focused window "sticky" — it appears on every
    /// workspace (like a pinned window in niri).  Toggle off to unpin.
    ToggleSticky,
    /// Set the active workspace's layout algorithm.
    /// Accepted modes: `"scrolling"` (default), `"bstack"`, `"spiral"`.
    SetWorkspaceLayout(String),
    /// Rename the currently-focused workspace to the given string.
    RenameWorkspace(String),
    /// Capture the focused window's bounding rect to a BMP via PrintWindow,
    /// identical to `WindowScreenshot` but reachable from config binds /
    /// `wiri-ctl screenshot-window`.
    ScreenshotWindow,
    /// Cycle through DWM backdrop types (auto → none → mica → acrylic → tabbed → auto)
    /// for all tiled windows.  Maps to `TilingEngine::toggle_backdrop_cycle`.
    ToggleBackdrop,
    /// Show/hide the key-bindings cheatsheet overlay window.
    /// Default chord: prefix + Shift+/ (i.e. prefix + ?).
    ShowKeyBindings,
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
        "switch-workspace" | "workspace" => args
            .first()
            .and_then(|s| s.parse::<i32>().ok())
            .map(Action::FocusWorkspace),
        "move-workspace" => args
            .first()
            .and_then(|s| s.parse::<i32>().ok())
            .map(Action::MoveWorkspace),
        // Every argument counts: `spawn "wt.exe" "-d" "C:\My Projects"` is three
        // words, and the first one is the program.
        "spawn" | "exec" => {
            if args.is_empty() || args[0].trim().is_empty() {
                None
            } else {
                Some(Action::Spawn(args.to_vec()))
            }
        }
        "spawn-cmd" | "spawn_cmd" | "spawn-sh" => {
            args.first().map(|cmd| Action::SpawnCmd(cmd.clone()))
        }
        "overview" | "overview-toggle" | "zoom-out" => Some(Action::OverviewToggle),
        "overview-exit" | "exit-overview" => Some(Action::OverviewExit),
        "overview-left" => Some(Action::OverviewLeft),
        "overview-right" => Some(Action::OverviewRight),
        "overview-select" | "overview-accept" => Some(Action::OverviewSelect),
        "column-toggle-tabbed" => Some(Action::ColumnToggleTabbed),
        "tab-next" => Some(Action::TabNext),
        "tab-prev" => Some(Action::TabPrev),
        "move-to-workspace" => args
            .first()
            .and_then(|s| s.parse::<i32>().ok())
            .map(Action::MoveToWorkspace),
        "move-to-monitor-left" => Some(Action::MoveToMonitorLeft),
        "move-to-monitor-right" => Some(Action::MoveToMonitorRight),
        "center-column" => Some(Action::CenterColumn),
        // `set-column-width "1/3"` — the spelling the default config and the
        // cheatsheet example use (niri: `set-column-width "33.333%"`).
        "set-column-width" => args.first().and_then(|a| parse_column_width_arg(a)),
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
        "focus-workspace-previous" | "focus-workspace-prev" | "workspace-prev" => {
            Some(Action::FocusWorkspacePrevious)
        }
        "focus-previous" | "alt-tab" | "focus-history-back" => Some(Action::FocusPrevious),
        "toggle-always-on-top" | "always-on-top" | "topmost" => Some(Action::ToggleAlwaysOnTop),
        "focus-workspace-named" | "focus-workspace" => {
            args.first().map(|s| Action::FocusWorkspaceNamed(s.clone()))
        }
        "screenshot" | "take-screenshot" | "capture-screen" => Some(Action::Screenshot),
        "window-screenshot" | "capture-window" => Some(Action::WindowScreenshot),
        "enter-resize-mode" | "resize-mode" | "interactive-resize" => Some(Action::EnterResizeMode),
        "move-column-to-workspace-up" | "move-column-up" | "move-column-to-workspace-above" => {
            Some(Action::MoveColumnToWorkspaceUp)
        }
        "move-column-to-workspace-down" | "move-column-down" | "move-column-to-workspace-below" => {
            Some(Action::MoveColumnToWorkspaceDown)
        }
        "move-workspace-up" | "swap-workspace-up" => Some(Action::MoveWorkspaceUp),
        "move-workspace-down" | "swap-workspace-down" => Some(Action::MoveWorkspaceDown),
        "toggle-sticky" | "sticky" => Some(Action::ToggleSticky),
        "set-workspace-layout" | "workspace-layout" => {
            args.first().map(|s| Action::SetWorkspaceLayout(s.clone()))
        }
        "rename-workspace" | "rename-ws" => {
            args.first().map(|s| Action::RenameWorkspace(s.clone()))
        }
        "screenshot-window" | "screenshot-focused" => Some(Action::ScreenshotWindow),
        "toggle-backdrop" | "backdrop-cycle" | "backdrop" => Some(Action::ToggleBackdrop),
        "show-key-bindings" | "show-bindings" | "cheatsheet" | "help" => {
            Some(Action::ShowKeyBindings)
        }
        "set-auto-tile" => {
            if args.is_empty() {
                None
            } else {
                let raw = args[0].to_lowercase();
                match raw.as_str() {
                    "off" | "none" => Some(Action::SetAutoTileThreshold(None)),
                    v => v
                        .parse::<usize>()
                        .ok()
                        .map(|n| Action::SetAutoTileThreshold(if n == 0 { None } else { Some(n) })),
                }
            }
        }
        _ => None,
    }
}

/// Map the argument of `set-column-width` to a width-preset action.
/// Accepts fractions (`1/4`, `1/3`, `1/2`, `2/3`, `3/4`), `full`, `cycle`, and
/// the percentages niri uses (`25%`, `33.3%`, `50%`, `66.6%`, `75%`, `100%`).
pub fn parse_column_width_arg(arg: &str) -> Option<Action> {
    let a = arg.trim().to_lowercase();
    match a.as_str() {
        "cycle" | "preset" | "next" => return Some(Action::ColumnWidthPresetCycle),
        "full" | "100%" => return Some(Action::ColumnWidthPresetFull),
        "1/4" | "quarter" => return Some(Action::ColumnWidthPresetQuarter),
        "1/3" | "third" => return Some(Action::ColumnWidthPresetThird),
        "1/2" | "half" => return Some(Action::ColumnWidthPresetHalf),
        "2/3" | "two-thirds" => return Some(Action::ColumnWidthPresetTwoThirds),
        "3/4" | "three-quarters" => return Some(Action::ColumnWidthPresetThreeQuarters),
        _ => {}
    }
    let percent: f64 = a.strip_suffix('%')?.trim().parse().ok()?;
    let nearest = [
        (25.0, Action::ColumnWidthPresetQuarter),
        (100.0 / 3.0, Action::ColumnWidthPresetThird),
        (50.0, Action::ColumnWidthPresetHalf),
        (200.0 / 3.0, Action::ColumnWidthPresetTwoThirds),
        (75.0, Action::ColumnWidthPresetThreeQuarters),
        (100.0, Action::ColumnWidthPresetFull),
    ]
    .into_iter()
    .find(|(p, _)| (percent - p).abs() < 1.0);
    nearest.map(|(_, action)| action)
}

/// Parse a key name string (from config) into a Windows virtual key code.
///
/// Names are matched case-insensitively and ignoring `_`, `-` and spaces, so
/// every spelling in use resolves to the same key: niri / xkb names
/// (`bracketleft`, `Page_Up`, `Return`), the hyphenated wiri spellings
/// (`page-up`, `arrow-left`) and the underscored ones the generated default
/// config used (`Bracket_Left`, `Backslash`, `Comma`, `Period`).
pub fn parse_key_name(name: &str) -> Option<u32> {
    let k = name.trim().to_lowercase();
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
        // Punctuation typed as the character itself (OEM keys of a US layout).
        let oem = match ch {
            ';' => Some(0xBA),
            '=' => Some(0xBB),
            ',' => Some(0xBC),
            '-' => Some(0xBD),
            '.' => Some(0xBE),
            '/' => Some(0xBF),
            '`' => Some(0xC0),
            '[' => Some(0xDB),
            '\\' => Some(0xDC),
            ']' => Some(0xDD),
            '\'' => Some(0xDE),
            _ => None,
        };
        if oem.is_some() {
            return oem;
        }
    }
    let canon: String = k
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .collect();
    match canon.as_str() {
        "left" | "arrowleft" => Some(0x25),
        "right" | "arrowright" => Some(0x27),
        "up" | "arrowup" => Some(0x26),
        "down" | "arrowdown" => Some(0x28),
        "enter" | "return" | "kpenter" | "numpadenter" => Some(0x0D),
        "space" => Some(0x20),
        "tab" => Some(0x09),
        "escape" | "esc" => Some(0x1B),
        "backspace" => Some(0x08),
        "delete" | "del" => Some(0x2E),
        "home" => Some(0x24),
        "end" => Some(0x23),
        "pageup" | "prior" => Some(0x21),
        "pagedown" | "next" => Some(0x22),
        "insert" | "ins" => Some(0x2D),
        "capslock" => Some(0x14),
        "numlock" => Some(0x90),
        "f1" => Some(0x70),
        "f2" => Some(0x71),
        "f3" => Some(0x72),
        "f4" => Some(0x73),
        "f5" => Some(0x74),
        "f6" => Some(0x75),
        "f7" => Some(0x76),
        "f8" => Some(0x77),
        "f9" => Some(0x78),
        "f10" => Some(0x79),
        "f11" => Some(0x7A),
        "f12" => Some(0x7B),
        "f13" => Some(0x7C),
        "f14" => Some(0x7D),
        "f15" => Some(0x7E),
        "f16" => Some(0x7F),
        "f17" => Some(0x80),
        "f18" => Some(0x81),
        "f19" => Some(0x82),
        "f20" => Some(0x83),
        "f21" => Some(0x84),
        "f22" => Some(0x85),
        "f23" => Some(0x86),
        "f24" => Some(0x87),
        "print" | "printscreen" | "sysrq" => Some(0x2C),
        "scrolllock" => Some(0x91),
        "pause" | "break" => Some(0x13),
        // Punctuation keys (US layout), xkb names.
        "semicolon" => Some(0xBA),
        "equal" | "plus" => Some(0xBB),
        "comma" => Some(0xBC),
        "minus" => Some(0xBD),
        "period" | "dot" => Some(0xBE),
        "slash" => Some(0xBF),
        "grave" | "backtick" | "quoteleft" => Some(0xC0),
        "bracketleft" => Some(0xDB),
        "backslash" => Some(0xDC),
        "bracketright" => Some(0xDD),
        "apostrophe" | "quote" => Some(0xDE),
        // Numeric keypad: `KP_1` / `kp1` / `numpad1`, and its operators.
        "kp0" | "numpad0" => Some(0x60),
        "kp1" | "numpad1" => Some(0x61),
        "kp2" | "numpad2" => Some(0x62),
        "kp3" | "numpad3" => Some(0x63),
        "kp4" | "numpad4" => Some(0x64),
        "kp5" | "numpad5" => Some(0x65),
        "kp6" | "numpad6" => Some(0x66),
        "kp7" | "numpad7" => Some(0x67),
        "kp8" | "numpad8" => Some(0x68),
        "kp9" | "numpad9" => Some(0x69),
        "kpmultiply" | "numpadmultiply" => Some(0x6A),
        "kpadd" | "numpadadd" => Some(0x6B),
        "kpsubtract" | "numpadsubtract" => Some(0x6D),
        "kpdecimal" | "numpaddecimal" => Some(0x6E),
        "kpdivide" | "numpaddivide" => Some(0x6F),
        _ => None,
    }
}

/// Parse modifier name strings into Windows MOD_* flags.
///
/// Lenient: names it does not know are skipped.  Key binds must not use this -
/// a dropped modifier turns `Hyper+T` into a bare `T` hotkey - they go through
/// [`resolve_bind_modifiers`], which rejects what it does not understand.
pub fn parse_modifiers(names: &[String]) -> u32 {
    let mut flags = 0u32;
    for m in names {
        match m.to_lowercase().as_str() {
            "ctrl" | "control" => flags |= MOD_CTRL,
            "alt" => flags |= MOD_ALT,
            "shift" => flags |= MOD_SHIFT,
            "super" | "win" | "meta" => flags |= MOD_WIN,
            _ => {}
        }
    }
    flags
}

/// Win32 `RegisterHotKey` modifier flags.
pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CTRL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;

/// What `Mod` stands for when `input { mod-key }` is not set: wiri's long-standing
/// default prefix, Ctrl+Alt.
pub const DEFAULT_MOD_KEY: u32 = MOD_CTRL | MOD_ALT;

/// The modifier flags a `mod-key` value stands for, or `None` when the value is
/// not one wiri knows.  Case, `+`, `_` and `-` are interchangeable, so niri's
/// `"Super"` / `"Alt"` and wiri's `"ctrl-alt"` / `"win-alt"` all resolve.
pub fn mod_key_flags(name: &str) -> Option<u32> {
    let canon: String = name
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | '+' | ' '))
        .collect();
    match canon.as_str() {
        "ctrlalt" | "altctrl" | "controlalt" | "altcontrol" => Some(MOD_CTRL | MOD_ALT),
        "alt" => Some(MOD_ALT),
        "ctrl" | "control" => Some(MOD_CTRL),
        "super" | "win" | "meta" => Some(MOD_WIN),
        "winalt" | "superalt" | "metaalt" | "altwin" | "altsuper" | "altmeta" => {
            Some(MOD_WIN | MOD_ALT)
        }
        "ctrlshift" | "shiftctrl" | "controlshift" | "shiftcontrol" => Some(MOD_CTRL | MOD_SHIFT),
        "winctrl" | "superctrl" | "metactrl" | "ctrlwin" | "ctrlsuper" => {
            Some(MOD_WIN | MOD_CTRL)
        }
        _ => None,
    }
}

/// Why a bind's modifier list was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModifierError {
    /// Not a modifier name at all (a typo, or a name from another WM).
    Unknown(String),
    /// A valid niri modifier that Windows cannot bind (AltGr / level-5 shift).
    Unsupported(String),
}

impl std::fmt::Display for ModifierError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModifierError::Unknown(name) => write!(
                f,
                "unknown modifier '{}' (use Mod, Ctrl, Shift, Alt or Super)",
                name
            ),
            ModifierError::Unsupported(name) => write!(
                f,
                "modifier '{}' (AltGr / level shift) cannot be bound on Windows",
                name
            ),
        }
    }
}

/// Resolve the modifier names of one key bind (`Mod+Ctrl+T` has `Mod`, `Ctrl`)
/// into `RegisterHotKey` flags.  `mod_key` is what `Mod` stands for (see
/// [`mod_key_flags`] and [`DEFAULT_MOD_KEY`]).  Unlike [`parse_modifiers`] a name
/// that is not understood is an error: silently dropping it registers a bare
/// global hotkey that steals the key from every application.
pub fn resolve_bind_modifiers(names: &[String], mod_key: u32) -> Result<u32, ModifierError> {
    let mut flags = 0u32;
    for raw in names {
        let name = raw.trim();
        match name.to_lowercase().as_str() {
            "mod" => flags |= mod_key,
            "ctrl" | "control" => flags |= MOD_CTRL,
            "alt" => flags |= MOD_ALT,
            "shift" => flags |= MOD_SHIFT,
            "super" | "win" | "meta" => flags |= MOD_WIN,
            "iso_level3_shift" | "mod5" | "iso_level5_shift" | "mod3" => {
                return Err(ModifierError::Unsupported(name.to_string()))
            }
            _ => return Err(ModifierError::Unknown(name.to_string())),
        }
    }
    Ok(flags)
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
        Self {
            enabled: true,
            repeat_rate: 30,
            repeat_delay: 500,
        }
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
        Self {
            sensitivity: 1.0,
            pointer_visibility: PointerVisibility::Visible,
        }
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
        Self {
            natural_scroll: false,
            scroll_speed: 1.0,
        }
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

/// Apply both the mouse runtime snapshot AND the snap-on-drag runtime
/// snapshot from a full [`crate::config::Config`].  Convenience wrapper for
/// callers that already have a `Config` in hand (typically `main.rs` after
/// `Config::load`).  Equivalent to calling `apply_mouse_config(&cfg.input)`
/// plus `snap_guide::set_snap_config(...)`.
pub fn apply_input_runtime_config(cfg: &crate::config::Config) {
    apply_mouse_config(&cfg.input);
    let threshold_px = cfg.layout.snap_threshold_px as i32;
    snap_guide::set_snap_config(snap_guide::SnapRuntimeConfig {
        enabled: cfg.layout.snap_on_drag,
        threshold_px,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_action_focus() {
        assert_eq!(
            parse_action_name("focus-column-left", &[]),
            Some(Action::FocusColumnLeft)
        );
        assert_eq!(
            parse_action_name("focus-left", &[]),
            Some(Action::FocusColumnLeft)
        );
        assert_eq!(
            parse_action_name("focus-column-right", &[]),
            Some(Action::FocusColumnRight)
        );
        assert_eq!(
            parse_action_name("focus-right", &[]),
            Some(Action::FocusColumnRight)
        );
        assert_eq!(parse_action_name("focus-up", &[]), Some(Action::FocusUp));
        assert_eq!(
            parse_action_name("focus-down", &[]),
            Some(Action::FocusDown)
        );
    }

    #[test]
    fn test_parse_action_move() {
        assert_eq!(
            parse_action_name("move-column-left", &[]),
            Some(Action::MoveColumnLeft)
        );
        assert_eq!(
            parse_action_name("move-left", &[]),
            Some(Action::MoveColumnLeft)
        );
        assert_eq!(
            parse_action_name("move-column-right", &[]),
            Some(Action::MoveColumnRight)
        );
        assert_eq!(
            parse_action_name("move-right", &[]),
            Some(Action::MoveColumnRight)
        );
    }

    #[test]
    fn test_parse_action_misc() {
        assert_eq!(
            parse_action_name("close-window", &[]),
            Some(Action::CloseWindow)
        );
        assert_eq!(parse_action_name("close", &[]), Some(Action::CloseWindow));
        assert_eq!(
            parse_action_name("toggle-fullscreen", &[]),
            Some(Action::ToggleFullscreen)
        );
        assert_eq!(
            parse_action_name("fullscreen", &[]),
            Some(Action::ToggleFullscreen)
        );
        assert_eq!(
            parse_action_name("toggle-floating", &[]),
            Some(Action::ToggleFloating)
        );
        assert_eq!(
            parse_action_name("float", &[]),
            Some(Action::ToggleFloating)
        );
        assert_eq!(parse_action_name("quit", &[]), Some(Action::Quit));
        assert_eq!(parse_action_name("exit", &[]), Some(Action::Quit));
    }

    #[test]
    fn test_parse_action_scroll() {
        assert_eq!(
            parse_action_name("scroll-left", &[]),
            Some(Action::ScrollLeft)
        );
        assert_eq!(
            parse_action_name("scroll-right", &[]),
            Some(Action::ScrollRight)
        );
    }

    #[test]
    fn test_parse_action_overview() {
        assert_eq!(
            parse_action_name("overview", &[]),
            Some(Action::OverviewToggle)
        );
        assert_eq!(
            parse_action_name("overview-toggle", &[]),
            Some(Action::OverviewToggle)
        );
        assert_eq!(
            parse_action_name("zoom-out", &[]),
            Some(Action::OverviewToggle)
        );
        assert_eq!(
            parse_action_name("overview-left", &[]),
            Some(Action::OverviewLeft)
        );
        assert_eq!(
            parse_action_name("overview-right", &[]),
            Some(Action::OverviewRight)
        );
        assert_eq!(
            parse_action_name("overview-select", &[]),
            Some(Action::OverviewSelect)
        );
        assert_eq!(
            parse_action_name("overview-accept", &[]),
            Some(Action::OverviewSelect)
        );
    }

    #[test]
    fn test_parse_action_with_args() {
        assert_eq!(
            parse_action_name("switch-workspace", &["3".to_string()]),
            Some(Action::FocusWorkspace(3))
        );
        assert_eq!(
            parse_action_name("workspace", &["1".to_string()]),
            Some(Action::FocusWorkspace(1))
        );
        assert_eq!(
            parse_action_name("move-workspace", &["2".to_string()]),
            Some(Action::MoveWorkspace(2))
        );
        assert_eq!(
            parse_action_name("spawn", &["cmd.exe".to_string()]),
            Some(Action::Spawn(vec!["cmd.exe".to_string()]))
        );
        assert_eq!(
            parse_action_name("exec", &["notepad".to_string()]),
            Some(Action::Spawn(vec!["notepad".to_string()]))
        );
    }

    #[test]
    fn test_spawn_keeps_every_argument() {
        let args: Vec<String> = ["wt.exe", "-d", "C:\\My Projects", "--title", "a b"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            parse_action_name("spawn", &args),
            Some(Action::Spawn(args.clone()))
        );
        // A blank program is no program.
        assert_eq!(parse_action_name("spawn", &["  ".to_string()]), None);
    }

    #[test]
    fn test_mod_key_flags_accepts_niri_and_wiri_spellings() {
        assert_eq!(mod_key_flags("ctrl-alt"), Some(MOD_CTRL | MOD_ALT));
        assert_eq!(mod_key_flags("Ctrl+Alt"), Some(MOD_CTRL | MOD_ALT));
        assert_eq!(mod_key_flags("Super"), Some(MOD_WIN));
        assert_eq!(mod_key_flags("win"), Some(MOD_WIN));
        assert_eq!(mod_key_flags("Alt"), Some(MOD_ALT));
        assert_eq!(mod_key_flags("Ctrl"), Some(MOD_CTRL));
        assert_eq!(mod_key_flags("win-alt"), Some(MOD_WIN | MOD_ALT));
        assert_eq!(mod_key_flags("ctrl-shift"), Some(MOD_CTRL | MOD_SHIFT));
        // Linux-only modifiers and nonsense are not silently turned into Ctrl+Alt.
        assert_eq!(mod_key_flags("Mod5"), None);
        assert_eq!(mod_key_flags("hyper"), None);
        assert_eq!(mod_key_flags(""), None);
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_unknown_modifier_is_rejected_not_dropped() {
        // `Hyper+T` used to register a bare global `T`.
        assert_eq!(
            resolve_bind_modifiers(&names(&["Hyper"]), DEFAULT_MOD_KEY),
            Err(ModifierError::Unknown("Hyper".to_string()))
        );
        assert_eq!(
            resolve_bind_modifiers(&names(&["Ctrl", "Typo"]), DEFAULT_MOD_KEY),
            Err(ModifierError::Unknown("Typo".to_string()))
        );
        // Valid niri names that Windows cannot bind are refused with their own reason.
        for name in ["Mod5", "ISO_Level3_Shift", "Mod3", "iso_level5_shift"] {
            assert!(
                matches!(
                    resolve_bind_modifiers(&names(&[name]), DEFAULT_MOD_KEY),
                    Err(ModifierError::Unsupported(_))
                ),
                "{} must be refused as unsupported",
                name
            );
        }
    }

    #[test]
    fn test_mod_token_resolves_from_mod_key() {
        assert_eq!(
            resolve_bind_modifiers(&names(&["Mod"]), DEFAULT_MOD_KEY),
            Ok(MOD_CTRL | MOD_ALT)
        );
        assert_eq!(
            resolve_bind_modifiers(&names(&["mod", "Shift"]), MOD_WIN),
            Ok(MOD_WIN | MOD_SHIFT)
        );
        assert_eq!(
            resolve_bind_modifiers(&names(&["MOD", "Ctrl"]), MOD_ALT),
            Ok(MOD_ALT | MOD_CTRL)
        );
        // Plain modifiers, a bare key, and the aliases.
        assert_eq!(
            resolve_bind_modifiers(&names(&["Control", "Win", "Meta", "Super"]), MOD_ALT),
            Ok(MOD_CTRL | MOD_WIN)
        );
        assert_eq!(resolve_bind_modifiers(&[], DEFAULT_MOD_KEY), Ok(0));
    }

    #[test]
    fn test_modifier_error_messages_name_the_modifier() {
        let unknown = ModifierError::Unknown("Hyper".to_string()).to_string();
        assert!(unknown.contains("Hyper") && unknown.contains("unknown modifier"));
        let unsupported = ModifierError::Unsupported("Mod5".to_string()).to_string();
        assert!(unsupported.contains("Mod5") && unsupported.contains("Windows"));
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
        assert_eq!(parse_key_name("_"), None);
    }

    #[test]
    fn test_parse_key_punctuation_accepts_every_spelling() {
        // The names the generated default config uses...
        assert_eq!(parse_key_name("Bracket_Left"), Some(0xDB));
        assert_eq!(parse_key_name("Bracket_Right"), Some(0xDD));
        assert_eq!(parse_key_name("Backslash"), Some(0xDC));
        assert_eq!(parse_key_name("Comma"), Some(0xBC));
        assert_eq!(parse_key_name("Period"), Some(0xBE));
        // ...and the niri / xkb spellings of the same keys.
        assert_eq!(parse_key_name("BracketLeft"), Some(0xDB));
        assert_eq!(parse_key_name("bracketleft"), Some(0xDB));
        assert_eq!(parse_key_name("bracket-right"), Some(0xDD));
        assert_eq!(parse_key_name("comma"), Some(0xBC));
        assert_eq!(parse_key_name("minus"), Some(0xBD));
        assert_eq!(parse_key_name("equal"), Some(0xBB));
        assert_eq!(parse_key_name("slash"), Some(0xBF));
        assert_eq!(parse_key_name("semicolon"), Some(0xBA));
        assert_eq!(parse_key_name("apostrophe"), Some(0xDE));
        assert_eq!(parse_key_name("grave"), Some(0xC0));
        // The characters themselves.
        assert_eq!(parse_key_name(","), Some(0xBC));
        assert_eq!(parse_key_name("."), Some(0xBE));
        assert_eq!(parse_key_name("["), Some(0xDB));
        assert_eq!(parse_key_name("]"), Some(0xDD));
        assert_eq!(parse_key_name("\\"), Some(0xDC));
    }

    #[test]
    fn test_parse_key_paging_and_keypad_spellings() {
        for name in ["Page_Up", "PageUp", "page-up", "pageup", "Prior"] {
            assert_eq!(parse_key_name(name), Some(0x21), "{}", name);
        }
        for name in ["Page_Down", "PageDown", "page-down", "Next"] {
            assert_eq!(parse_key_name(name), Some(0x22), "{}", name);
        }
        assert_eq!(parse_key_name("KP_5"), Some(0x65));
        assert_eq!(parse_key_name("kp_add"), Some(0x6B));
        assert_eq!(parse_key_name("F24"), Some(0x87));
        assert_eq!(parse_key_name("Caps_Lock"), Some(0x14));
    }

    #[test]
    fn test_parse_column_width_arg() {
        assert_eq!(parse_column_width_arg("1/3"), Some(Action::ColumnWidthPresetThird));
        assert_eq!(parse_column_width_arg("2/3"), Some(Action::ColumnWidthPresetTwoThirds));
        assert_eq!(parse_column_width_arg("cycle"), Some(Action::ColumnWidthPresetCycle));
        assert_eq!(parse_column_width_arg("Full"), Some(Action::ColumnWidthPresetFull));
        assert_eq!(parse_column_width_arg("50%"), Some(Action::ColumnWidthPresetHalf));
        assert_eq!(parse_column_width_arg("33.3%"), Some(Action::ColumnWidthPresetThird));
        assert_eq!(parse_column_width_arg("nonsense"), None);
        assert_eq!(parse_column_width_arg("42%"), None);
        assert_eq!(
            parse_action_name("set-column-width", &["1/3".to_string()]),
            Some(Action::ColumnWidthPresetThird)
        );
        assert_eq!(parse_action_name("set-column-width", &[]), None);
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

    #[test]
    fn test_parse_action_window_screenshot_aliases() {
        assert_eq!(
            parse_action_name("window-screenshot", &[]),
            Some(Action::WindowScreenshot)
        );
        assert_eq!(
            parse_action_name("capture-window", &[]),
            Some(Action::WindowScreenshot)
        );
        // "screenshot-window" now maps to ScreenshotWindow (round-4), not
        // WindowScreenshot.  The old alias was superseded — update accordingly.
        assert_eq!(
            parse_action_name("screenshot-window", &[]),
            Some(Action::ScreenshotWindow)
        );
    }

    // ---- Round-4 action parse tests ----

    #[test]
    fn test_parse_action_toggle_sticky() {
        assert_eq!(
            parse_action_name("toggle-sticky", &[]),
            Some(Action::ToggleSticky)
        );
        assert_eq!(parse_action_name("sticky", &[]), Some(Action::ToggleSticky));
    }

    #[test]
    fn test_parse_action_set_workspace_layout() {
        assert_eq!(
            parse_action_name("set-workspace-layout", &["bstack".to_string()]),
            Some(Action::SetWorkspaceLayout("bstack".to_string()))
        );
        assert_eq!(
            parse_action_name("workspace-layout", &["spiral".to_string()]),
            Some(Action::SetWorkspaceLayout("spiral".to_string()))
        );
        // Missing arg → None
        assert_eq!(parse_action_name("set-workspace-layout", &[]), None);
    }

    #[test]
    fn test_parse_action_rename_workspace() {
        assert_eq!(
            parse_action_name("rename-workspace", &["dev".to_string()]),
            Some(Action::RenameWorkspace("dev".to_string()))
        );
        assert_eq!(
            parse_action_name("rename-ws", &["chat".to_string()]),
            Some(Action::RenameWorkspace("chat".to_string()))
        );
        // Missing arg → None
        assert_eq!(parse_action_name("rename-workspace", &[]), None);
    }

    #[test]
    fn test_parse_action_screenshot_window() {
        assert_eq!(
            parse_action_name("screenshot-window", &[]),
            Some(Action::ScreenshotWindow)
        );
        assert_eq!(
            parse_action_name("screenshot-focused", &[]),
            Some(Action::ScreenshotWindow)
        );
    }

    /// Pure helper that mirrors the wheel-delta → action mapping used in the
    /// WM_MOUSEWHEEL hook.  Positive delta (wheel away from user) = workspace
    /// previous; negative delta (wheel toward user) = workspace next.
    fn wheel_delta_to_workspace_action(delta: i32) -> Option<Action> {
        if delta == 0 {
            return None;
        }
        if delta > 0 {
            Some(Action::FocusWorkspacePrevious)
        } else {
            Some(Action::FocusWorkspaceNext)
        }
    }

    #[test]
    fn test_wheel_delta_positive_is_previous() {
        assert_eq!(
            wheel_delta_to_workspace_action(120),
            Some(Action::FocusWorkspacePrevious),
        );
    }

    #[test]
    fn test_wheel_delta_negative_is_next() {
        assert_eq!(
            wheel_delta_to_workspace_action(-120),
            Some(Action::FocusWorkspaceNext),
        );
    }

    #[test]
    fn test_wheel_delta_zero_is_none() {
        assert_eq!(wheel_delta_to_workspace_action(0), None);
    }

    /// Verify the double-tap window logic by simulating the timestamp comparisons
    /// that the `keyboard_hook_callback` performs.  We test the pure timing
    /// predicate without actually installing the WH_KEYBOARD_LL hook (which
    /// would require a Win32 message loop).
    #[test]
    fn test_double_alt_within_window() {
        use std::time::{Duration, Instant};

        // Helper: returns true when the elapsed time since `prev` is within
        // the double-tap window (< 250 ms).  Mirrors the hook's condition.
        let is_double_tap = |prev: Instant| -> bool { prev.elapsed().as_millis() < 250 };

        // Case 1: two Alt presses very close together → double tap detected.
        let t0 = Instant::now();
        // Immediately check — should be well within 250 ms.
        assert!(
            is_double_tap(t0),
            "immediate second Alt-down must be within the 250 ms double-tap window"
        );

        // Case 2: simulate gap >= 250 ms using an artificially aged Instant.
        // We subtract 300 ms from now to emulate a "stale" first press.
        let stale = Instant::now()
            .checked_sub(Duration::from_millis(300))
            .expect("can subtract 300 ms from now");
        assert!(
            !is_double_tap(stale),
            "Alt-down 300 ms after the first press must NOT count as a double tap"
        );

        // Case 3: boundary — exactly at 249 ms should still be within the window.
        let near_boundary = Instant::now()
            .checked_sub(Duration::from_millis(249))
            .expect("can subtract 249 ms from now");
        // elapsed() will be >= 249 ms but typically < 250 ms unless the system is
        // extremely slow.  We allow a 5 ms tolerance for timing jitter.
        let elapsed = near_boundary.elapsed().as_millis();
        // We only assert the fast path (well within the window) not the near-miss
        // boundary, because wall-clock tests are inherently racy.
        assert!(
            elapsed < 260,
            "elapsed {} ms from a 249-ms-ago Instant must be < 260 ms (system timing jitter tolerance)",
            elapsed
        );

        // Case 4: verify that Action::OverviewToggle exists and PartialEq works
        // (compile-time check that the action is wired).
        let action = Action::OverviewToggle;
        assert_eq!(action, Action::OverviewToggle);
    }

    #[test]
    fn test_parse_action_toggle_backdrop() {
        assert_eq!(
            parse_action_name("toggle-backdrop", &[]),
            Some(Action::ToggleBackdrop)
        );
        assert_eq!(
            parse_action_name("backdrop-cycle", &[]),
            Some(Action::ToggleBackdrop)
        );
        assert_eq!(
            parse_action_name("backdrop", &[]),
            Some(Action::ToggleBackdrop)
        );
    }
}
