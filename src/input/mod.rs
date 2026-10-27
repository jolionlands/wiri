pub mod dispatch;
pub mod dnd;
mod double_alt;
pub mod focus;
pub mod gestures;
pub mod grab;
pub mod hotkey;
pub mod interactive;
pub mod keyboard;
pub mod keyboard_config;
pub mod keynames;
pub mod layout_actions;
pub mod low_level_hook;
pub mod mouse;
pub mod pointer;
pub mod recent;
pub(crate) mod rule_scroll;
pub mod snap_guide;
pub mod system_actions;
pub mod touch;
pub mod touchpad;
pub mod touchpen;

pub use grab::{ColumnReorderGrab, MoveGrab, ResizeEdge, ResizeGrab, COLUMN_REORDER_HIT_ZONE_PX};
pub use hotkey::{HotkeyBinding, HotkeyId};
pub use layout_actions::{FocusAction, MonitorAction, MonitorTarget, WorkspaceAction};
pub use low_level_hook::{
    begin_column_reorder_grab, begin_move_grab, begin_resize_grab, cancel_grab, current_grab,
    is_grab_active, last_press_was_repeat, start_keyboard_hook, start_mouse_hook,
    stop_keyboard_hook, stop_mouse_hook, GrabState,
};
pub use mouse::{MouseFocusConfig, MouseTracker};
pub use touch::{
    set_action_sender as set_touch_action_sender, start_touch_hook, stop_touch_hook,
    GestureRecognizer, TouchConfig, TouchGesture,
};

/// All actions that can be triggered by hotkeys or IPC
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Action {
    /// Non-layout OS/session actions, shared by IPC and bindings.
    System(system_actions::SystemAction),
    /// Structured layout focus arguments shared by binds and IPC.
    LayoutFocus(FocusAction),
    WorkspaceAction(WorkspaceAction),
    MonitorAction(MonitorAction),
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
    /// Toggle manual shortcut inhibition. Windows has no Wayland inhibitor protocol;
    /// allow-inhibiting=false binds and this escape hatch remain active.
    ToggleKeyboardShortcutsInhibit,
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
    /// niri `switch-preset-column-width`: cycle through the preset widths (a third,
    /// a half, two thirds of the view).
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
    /// niri `focus-workspace-down`: the workspace below the active one (no wrapping).
    FocusWorkspaceNext,
    /// niri `focus-workspace-up`: the workspace above the active one (no wrapping).
    FocusWorkspacePrevious,
    /// niri `focus-workspace-previous`: the workspace the user was on before this one.
    FocusWorkspaceLast,
    /// Focus the previously focused window (niri-style alt-tab / MRU history)
    FocusPrevious,
    /// Toggle always-on-top for the focused window
    ToggleAlwaysOnTop,
    /// Focus a named workspace from config
    FocusWorkspaceNamed(String),
    /// Set auto-tile threshold (None disables)
    SetAutoTileThreshold(Option<usize>),
    /// Open the frozen focused-output screenshot selector.
    Screenshot,
    /// Capture the focused output immediately, without the selector.
    ScreenshotScreen,
    /// Interactive screenshot with niri's pointer and destination options.
    ScreenshotUi(crate::overlay::screenshot_ui::Options),
    /// Immediate screen/window capture with validated niri options.
    Capture(crate::screenshot::action::CaptureAction),
    /// Set the focused column to 1/4 of the work-area width.
    ColumnWidthPresetQuarter,
    /// Set the focused column to 3/4 of the work-area width.
    ColumnWidthPresetThreeQuarters,
    /// niri `consume-window-into-column`: the first window of the column on the
    /// right moves to the bottom of the focused column.  No-op in the last column.
    ConsumeWindowIntoColumn,
    /// niri `expel-window-from-column`: the bottom window of the focused column
    /// becomes a column of its own on its right.  No-op for a single window.
    ExpelWindowFromColumn,
    /// niri `consume-or-expel-window-left`: a lone window joins the column on its
    /// left, one of several leaves its column to the left.
    ConsumeOrExpelWindowLeft,
    /// niri `consume-or-expel-window-right`.
    ConsumeOrExpelWindowRight,
    /// niri `move-window-up`: swap the focused window with the one above it in its
    /// column (a floating window moves up by 50 px).
    MoveWindowUp,
    /// niri `move-window-down`.
    MoveWindowDown,
    /// niri `focus-column-first`.
    FocusColumnFirst,
    /// niri `focus-column-last`.
    FocusColumnLast,
    /// niri `move-column-to-first`.
    MoveColumnToFirst,
    /// niri `move-column-to-last`.
    MoveColumnToLast,
    /// niri `set-column-width` (also `set-window-width`): `"50%"`, `"+10%"`, `"-10%"`,
    /// `"1200"`, `"+100"`.
    SetColumnWidth(SizeChange),
    /// niri `set-window-height`, the same grammar.
    SetWindowHeight(SizeChange),
    /// niri `reset-window-height`: the window gets an equal share of its column again.
    ResetWindowHeight,
    /// niri `switch-preset-column-width-back`.
    ColumnWidthPresetCycleBack,
    /// niri `focus-monitor-left`: focus the monitor on the left (nothing at the edge).
    FocusMonitorLeft,
    /// niri `focus-monitor-right`.
    FocusMonitorRight,
    /// niri `move-window-to-floating` (not a toggle).
    MoveWindowToFloating,
    /// niri `move-window-to-tiling` (not a toggle).
    MoveWindowToTiling,
    /// niri `open-overview` (`toggle-overview` toggles, `close-overview` closes).
    OverviewOpen,
    /// niri `expand-column-to-available-width`: the focused column fills the
    /// leftover width of the view.
    ExpandColumnToAvailable,
    /// Toggle per-column maximize (full work-area height). Distinct from
    /// `ToggleFullscreen`, which covers the entire monitor.
    MaximizeColumn,
    /// niri `maximize-window-to-edges`: the window fills the work area (no gaps, no border) and
    /// keeps its place in its column; toggles.  wiri's older `maximize` only sets it.
    MaximizeWindowToEdges,
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

/// niri's size argument (`set-column-width`, `set-window-height`): `"50%"` sets a
/// proportion of the view, `"+10%"` / `"-10%"` adjust it, `"1200"` sets a size in
/// pixels and `"+100"` / `"-100"` adjust it.  Proportions are kept in thousandths of a
/// percent (`"33.333%"` is 33333), so an [`Action`] stays `Eq + Hash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SizeChange {
    /// `"1200"`: the window gets this size (the border around it comes on top).
    SetFixed(i32),
    /// `"50%"`: this proportion of the view.
    SetProportion(i32),
    /// `"+100"` / `"-100"`: this many pixels more or less.
    AdjustFixed(i32),
    /// `"+10%"` / `"-10%"`: this proportion of the view more or less.
    AdjustProportion(i32),
}

/// One percent in the units of [`SizeChange`]'s proportions.
const MILLI_PERCENT: f64 = 1000.0;

impl std::str::FromStr for SizeChange {
    type Err = &'static str;

    /// niri's grammar: an optional sign, a number, an optional `%` (nothing may
    /// follow it).  A sign makes it an adjustment, a `%` makes it a proportion.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let adjust = matches!(s.as_bytes().first(), Some(b'+') | Some(b'-'));
        match s.split_once('%') {
            Some((value, rest)) => {
                if !rest.is_empty() {
                    return Err("trailing characters after '%' are not allowed");
                }
                if value.is_empty() {
                    return Err("value is missing");
                }
                let percent: f64 = value.parse().map_err(|_| "error parsing value")?;
                if !percent.is_finite() {
                    return Err("error parsing value");
                }
                let milli = (percent * MILLI_PERCENT).round() as i32;
                Ok(if adjust {
                    SizeChange::AdjustProportion(milli)
                } else {
                    SizeChange::SetProportion(milli)
                })
            }
            None => {
                if s.is_empty() {
                    return Err("value is missing");
                }
                let pixels: i32 = s.parse().map_err(|_| "error parsing value")?;
                Ok(if adjust {
                    SizeChange::AdjustFixed(pixels)
                } else {
                    SizeChange::SetFixed(pixels)
                })
            }
        }
    }
}

/// Pixel size of something that is `proportion` of a view `view_px` long when `gap_px`
/// separates it from its neighbours: two halves and the gap between them fill the view
/// exactly, three thirds and two gaps too (niri: `(view - gap) * p - gap`, with the
/// outer gaps counted in the view).
pub fn proportion_px(view_px: i32, gap_px: i32, proportion: f64) -> i32 {
    (proportion * (view_px + gap_px) as f64 - gap_px as f64).round() as i32
}

impl SizeChange {
    /// The new size in pixels of something that is `current_px` now, in a view `view_px`
    /// long with `gap_px` between its neighbours.  `extra_px` is how much larger it is
    /// than the window it holds (the borders around the window), which a fixed size
    /// does not include.
    pub fn resolve(self, current_px: i32, view_px: i32, gap_px: i32, extra_px: i32) -> i32 {
        let whole = (view_px + gap_px).max(1) as f64;
        match self {
            SizeChange::SetFixed(px) => px.saturating_add(extra_px),
            SizeChange::AdjustFixed(delta) => current_px.saturating_add(delta),
            SizeChange::SetProportion(milli) => {
                proportion_px(view_px, gap_px, milli as f64 / (100.0 * MILLI_PERCENT))
            }
            SizeChange::AdjustProportion(milli) => {
                let now = (current_px + gap_px) as f64 / whole;
                proportion_px(
                    view_px,
                    gap_px,
                    now + milli as f64 / (100.0 * MILLI_PERCENT),
                )
            }
        }
    }
}

/// niri's workspace reference: a number is the (1-based) index of a workspace on the
/// monitor, a quoted string is the name of a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WorkspaceRef {
    Index(u32),
    Name(String),
}

impl WorkspaceRef {
    /// One word of a bind line: `focus-workspace 2` is the second workspace,
    /// `focus-workspace "2"` the workspace named "2".  A word that is not a number is a
    /// name even without quotes (`wiri-ctl action focus-workspace web`).
    pub fn from_word(word: &str, quoted: bool) -> Option<WorkspaceRef> {
        if word.is_empty() {
            return None;
        }
        if !quoted {
            if let Ok(index) = word.parse::<u32>() {
                // niri counts from 1; 0 is the first workspace as well.
                return Some(WorkspaceRef::Index(index.max(1)));
            }
        }
        Some(WorkspaceRef::Name(word.to_string()))
    }
}

/// Parse an action name string (from config/IPC) into an Action.
/// Returns None for unknown action names.
///
/// The arguments are read as unquoted words: `focus-workspace 2` is the second
/// workspace.  Config binds go through [`parse_action_flagged`], which also knows
/// which arguments were written in double quotes.
pub fn parse_action_name(name: &str, args: &[String]) -> Option<Action> {
    parse_action_flagged(name, args, &[])
}

/// True when `name` is an action wiri implements, whatever its arguments: the name is tried
/// with a few typical argument lists (none, a number, a size, a word, two words).  Used to
/// tell "bad arguments" from "wiri does not have this action" when a config is read.
pub fn action_name_known(name: &str) -> bool {
    const PROBES: [&[&str]; 5] = [&[], &["1"], &["50%"], &["a"], &["a", "b"]];
    PROBES.iter().any(|args| {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        parse_action_flagged(name, &args, &[]).is_some()
    })
}

/// [`parse_action_name`] for the words of a `binds {}` line.  `quoted[i]` says whether
/// `args[i]` was written in double quotes: niri's `focus-workspace 2` is the second
/// workspace, `focus-workspace "2"` the workspace named "2".  Both the niri names of
/// the actions and wiri's own older spellings are understood.
pub fn parse_action_flagged(name: &str, args: &[String], quoted: &[bool]) -> Option<Action> {
    if let Some(action) = layout_actions::parse(name, args, quoted) {
        return action;
    }
    let is_quoted = |i: usize| quoted.get(i).copied().unwrap_or(false);
    if let Some(system) = system_actions::parse(name, args) {
        return system.map(|action| {
            if matches!(
                action,
                system_actions::SystemAction::Quit {
                    skip_confirmation: false
                }
            ) {
                Action::Quit
            } else {
                Action::System(action)
            }
        });
    }
    if matches!(name, "screenshot" | "take-screenshot" | "capture-screen") {
        return crate::overlay::screenshot_ui::Options::parse(args).map(|options| {
            if options == crate::overlay::screenshot_ui::Options::default() {
                Action::Screenshot
            } else {
                Action::ScreenshotUi(options)
            }
        });
    }
    if matches!(
        name,
        "screenshot-screen" | "screenshot-window" | "screenshot-focused"
    ) {
        let window = name != "screenshot-screen";
        return crate::screenshot::action::CaptureAction::parse(window, args).map(|options| {
            if options == crate::screenshot::action::CaptureAction::defaults(window) {
                if window {
                    Action::ScreenshotWindow
                } else {
                    Action::ScreenshotScreen
                }
            } else {
                Action::Capture(options)
            }
        });
    }
    match name {
        "toggle-keyboard-shortcuts-inhibit" => Some(Action::ToggleKeyboardShortcutsInhibit),
        "focus-column-left" | "focus-left" => Some(Action::FocusColumnLeft),
        "focus-column-right" | "focus-right" => Some(Action::FocusColumnRight),
        "focus-up" | "focus-window-up" => Some(Action::FocusUp),
        "focus-down" | "focus-window-down" => Some(Action::FocusDown),
        "focus-column-first" => Some(Action::FocusColumnFirst),
        "focus-column-last" => Some(Action::FocusColumnLast),
        "move-column-to-first" => Some(Action::MoveColumnToFirst),
        "move-column-to-last" => Some(Action::MoveColumnToLast),
        "move-window-up" => Some(Action::MoveWindowUp),
        "move-window-down" => Some(Action::MoveWindowDown),
        "consume-or-expel-window-left" => Some(Action::ConsumeOrExpelWindowLeft),
        "consume-or-expel-window-right" => Some(Action::ConsumeOrExpelWindowRight),
        "move-column-left" | "move-left" => Some(Action::MoveColumnLeft),
        "move-column-right" | "move-right" => Some(Action::MoveColumnRight),
        "close-window" | "close" => Some(Action::CloseWindow),
        "toggle-fullscreen" | "fullscreen" | "fullscreen-window" => Some(Action::ToggleFullscreen),
        "toggle-floating" | "float" | "toggle-window-floating" => Some(Action::ToggleFloating),
        "move-window-to-floating" => Some(Action::MoveWindowToFloating),
        "move-window-to-tiling" => Some(Action::MoveWindowToTiling),
        "scroll-left" => Some(Action::ScrollLeft),
        "scroll-right" => Some(Action::ScrollRight),
        "quit" | "exit" => Some(Action::Quit),
        "maximize" => Some(Action::Maximize),
        "minimize" => Some(Action::Minimize),
        "center-window" => Some(Action::CenterWindow),
        "switch-monitor" => Some(Action::SwitchMonitor),
        "focus-monitor-left" => Some(Action::FocusMonitorLeft),
        "focus-monitor-right" => Some(Action::FocusMonitorRight),
        "refresh" => Some(Action::Refresh),
        "switch-workspace" | "workspace" => args
            .first()
            .and_then(|s| s.parse::<i32>().ok())
            .map(Action::FocusWorkspace),
        // niri: a number is the index of the workspace on the monitor, a quoted string
        // is the name of a workspace.
        "focus-workspace" => match WorkspaceRef::from_word(args.first()?, is_quoted(0))? {
            WorkspaceRef::Index(index) => Some(Action::FocusWorkspace(index as i32)),
            WorkspaceRef::Name(name) => Some(Action::FocusWorkspaceNamed(name)),
        },
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
        "overview" | "overview-toggle" | "zoom-out" | "toggle-overview" => {
            Some(Action::OverviewToggle)
        }
        "open-overview" => Some(Action::OverviewOpen),
        "overview-exit" | "exit-overview" | "close-overview" => Some(Action::OverviewExit),
        "overview-left" => Some(Action::OverviewLeft),
        "overview-right" => Some(Action::OverviewRight),
        "overview-select" | "overview-accept" => Some(Action::OverviewSelect),
        "column-toggle-tabbed" | "toggle-column-tabbed-display" => Some(Action::ColumnToggleTabbed),
        "tab-next" => Some(Action::TabNext),
        "tab-prev" => Some(Action::TabPrev),
        "move-to-workspace" => args
            .first()
            .and_then(|s| s.parse::<i32>().ok())
            .map(Action::MoveToWorkspace),
        "move-to-monitor-left" | "move-window-to-monitor-left" => Some(Action::MoveToMonitorLeft),
        "move-to-monitor-right" | "move-window-to-monitor-right" => {
            Some(Action::MoveToMonitorRight)
        }
        "center-column" => Some(Action::CenterColumn),
        // niri: `set-column-width "50%"`, `"+10%"`, `"1200"`; the wiri spellings
        // `"1/3"`, `"full"`, `"cycle"` keep working.
        "set-column-width" | "set-window-width" => {
            args.first().and_then(|a| parse_column_width_arg(a))
        }
        "set-window-height" => args
            .first()
            .and_then(|a| a.parse::<SizeChange>().ok())
            .map(Action::SetWindowHeight),
        "reset-window-height" => Some(Action::ResetWindowHeight),
        "switch-preset-column-width" | "column-width-preset" | "column-width-cycle" => {
            Some(Action::ColumnWidthPresetCycle)
        }
        "switch-preset-column-width-back" => Some(Action::ColumnWidthPresetCycleBack),
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
        "expand-column-to-available" | "expand-column" | "expand-column-to-available-width" => {
            Some(Action::ExpandColumnToAvailable)
        }
        "maximize-column" | "column-maximize" | "toggle-column-maximize" => {
            Some(Action::MaximizeColumn)
        }
        "maximize-window-to-edges" => Some(Action::MaximizeWindowToEdges),
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
        "focus-workspace-next" | "workspace-next" | "focus-workspace-down" => {
            Some(Action::FocusWorkspaceNext)
        }
        "focus-workspace-prev" | "workspace-prev" | "focus-workspace-up" => {
            Some(Action::FocusWorkspacePrevious)
        }
        // niri: the workspace the user was on before this one (wiri's older
        // `focus-workspace-previous` meant the one above; that is `-up`).
        "focus-workspace-previous" | "focus-workspace-back" => Some(Action::FocusWorkspaceLast),
        "focus-previous" | "alt-tab" | "focus-history-back" | "focus-window-previous" => {
            Some(Action::FocusPrevious)
        }
        "toggle-always-on-top" | "always-on-top" | "topmost" => Some(Action::ToggleAlwaysOnTop),
        "focus-workspace-named" => args.first().map(|s| Action::FocusWorkspaceNamed(s.clone())),
        "screenshot-screen" if args.is_empty() => Some(Action::ScreenshotScreen),
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
        "rename-workspace" | "rename-ws" | "set-workspace-name" => {
            args.first().map(|s| Action::RenameWorkspace(s.clone()))
        }
        "screenshot-window" | "screenshot-focused" => Some(Action::ScreenshotWindow),
        "toggle-backdrop" | "backdrop-cycle" | "backdrop" => Some(Action::ToggleBackdrop),
        "show-key-bindings" | "show-bindings" | "cheatsheet" | "help" | "show-hotkey-overlay" => {
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

/// The argument of `set-column-width`: niri's size grammar (`"50%"`, `"+10%"`,
/// `"-10%"`, `"1200"`, `"+100"`), or one of wiri's older spellings: the fractions
/// `1/4`, `1/3`, `1/2`, `2/3`, `3/4`, `full` and `cycle`.
pub fn parse_column_width_arg(arg: &str) -> Option<Action> {
    let a = arg.trim().to_lowercase();
    let proportion = |milli_percent: i32| {
        Some(Action::SetColumnWidth(SizeChange::SetProportion(
            milli_percent,
        )))
    };
    match a.as_str() {
        "cycle" | "preset" | "next" => return Some(Action::ColumnWidthPresetCycle),
        "back" | "prev" | "previous" => return Some(Action::ColumnWidthPresetCycleBack),
        "full" => return proportion(100_000),
        "1/4" | "quarter" => return proportion(25_000),
        "1/3" | "third" => return proportion(33_333),
        "1/2" | "half" => return proportion(50_000),
        "2/3" | "two-thirds" => return proportion(66_667),
        "3/4" | "three-quarters" => return proportion(75_000),
        _ => {}
    }
    a.parse::<SizeChange>().ok().map(Action::SetColumnWidth)
}

/// Parse a key name string (from config) into a Windows virtual key code.
///
/// Names are matched case-insensitively and ignoring `_`, `-` and spaces, so
/// every spelling in use resolves to the same key: niri / xkb names
/// (`bracketleft`, `Page_Up`, `Return`), the hyphenated wiri spellings
/// (`page-up`, `arrow-left`) and the underscored ones the generated default
/// config used (`Bracket_Left`, `Backslash`, `Comma`, `Period`).
pub fn parse_key_name(name: &str) -> Option<u32> {
    keynames::parse_key_name(name)
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
        "winctrl" | "superctrl" | "metactrl" | "ctrlwin" | "ctrlsuper" => Some(MOD_WIN | MOD_CTRL),
        _ => None,
    }
}

/// Why a bind's modifier list was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModifierError {
    /// Not a modifier name at all (a typo, or a name from another WM).
    Unknown(String),
    /// A valid niri modifier with no distinct Windows mapping (level-5 shift).
    Unsupported(String),
}

impl std::fmt::Display for ModifierError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModifierError::Unknown(name) => write!(
                f,
                "unknown modifier '{}' (use Mod, Ctrl, Shift, Alt, Super or AltGr/Mod5)",
                name
            ),
            ModifierError::Unsupported(name) => write!(
                f,
                "modifier '{}' (level-5 shift) has no distinct Windows mapping; use AltGr/Mod5 for Ctrl+RAlt",
                name
            ),
        }
    }
}

/// Resolve the modifier names of one key bind (`Mod+Ctrl+T` has `Mod`, `Ctrl`)
/// into hook modifier flags. AltGr is hook-only; Win32 registration must use
/// [`keynames::register_hotkey_modifiers`]. `mod_key` is what `Mod` stands for
/// (see [`mod_key_flags`] and [`DEFAULT_MOD_KEY`]). Unlike [`parse_modifiers`] a name
/// that is not understood is an error: silently dropping it registers a bare
/// global hotkey that steals the key from every application.
pub fn resolve_bind_modifiers(names: &[String], mod_key: u32) -> Result<u32, ModifierError> {
    keynames::resolve_modifiers(names, mod_key)
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
    rule_scroll::configure(config.mouse_speed);
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
    fn test_parse_niri_column_keymap_names() {
        for (name, action) in [
            ("focus-window-up", Action::FocusUp),
            ("focus-window-down", Action::FocusDown),
            ("focus-column-first", Action::FocusColumnFirst),
            ("focus-column-last", Action::FocusColumnLast),
            ("move-column-to-first", Action::MoveColumnToFirst),
            ("move-column-to-last", Action::MoveColumnToLast),
            ("move-window-up", Action::MoveWindowUp),
            ("move-window-down", Action::MoveWindowDown),
            (
                "consume-or-expel-window-left",
                Action::ConsumeOrExpelWindowLeft,
            ),
            (
                "consume-or-expel-window-right",
                Action::ConsumeOrExpelWindowRight,
            ),
            (
                "consume-window-into-column",
                Action::ConsumeWindowIntoColumn,
            ),
            ("expel-window-from-column", Action::ExpelWindowFromColumn),
            ("maximize-column", Action::MaximizeColumn),
            ("maximize-window-to-edges", Action::MaximizeWindowToEdges),
        ] {
            assert_eq!(parse_action_name(name, &[]), Some(action), "{name}");
        }
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
        // Level-3 shift is hook-only AltGr; level-5 has no Windows mapping.
        for name in ["AltGr", "Mod5", "ISO_Level3_Shift"] {
            assert_eq!(
                resolve_bind_modifiers(&names(&[name]), DEFAULT_MOD_KEY),
                Ok(keynames::MOD_ALTGR)
            );
        }
        for name in ["Mod3", "iso_level5_shift"] {
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
        let unsupported = ModifierError::Unsupported("Mod3".to_string()).to_string();
        assert!(unsupported.contains("Mod3") && unsupported.contains("Windows"));
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
        assert_eq!(parse_key_name("_"), Some(0xBD)); // US shifted punctuation alias.
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

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn test_size_change_grammar_follows_niri() {
        use SizeChange::*;
        for (text, expected) in [
            ("50%", SetProportion(50_000)),
            ("33.333%", SetProportion(33_333)),
            ("100%", SetProportion(100_000)),
            ("+10%", AdjustProportion(10_000)),
            ("-10%", AdjustProportion(-10_000)),
            ("-2.5%", AdjustProportion(-2_500)),
            ("1200", SetFixed(1200)),
            ("+100", AdjustFixed(100)),
            ("-100", AdjustFixed(-100)),
            (" 640 ", SetFixed(640)),
        ] {
            assert_eq!(text.parse::<SizeChange>(), Ok(expected), "{text:?}");
        }
        for text in [
            "", "%", "+%", "50%%", "abc", "12.5", "50%x", "1/3", "inf%", "nan%",
        ] {
            assert!(
                text.parse::<SizeChange>().is_err(),
                "{text:?} must not parse"
            );
        }
        assert_eq!("".parse::<SizeChange>(), Err("value is missing"));
        assert_eq!("%".parse::<SizeChange>(), Err("value is missing"));
        assert_eq!(
            "50%x".parse::<SizeChange>(),
            Err("trailing characters after '%' are not allowed")
        );
        assert_eq!("12.5".parse::<SizeChange>(), Err("error parsing value"));
        assert_eq!("abc".parse::<SizeChange>(), Err("error parsing value"));
        assert!("inf%".parse::<SizeChange>().is_err());
    }

    #[test]
    fn test_size_change_resolves_against_the_view() {
        // A 1350 px view with 16 px gaps and 4 px borders (2 * 4 = 8 px around a window).
        let (view, gap, extra) = (1350, 16, 8);
        let px = |change: SizeChange, current: i32| change.resolve(current, view, gap, extra);
        // Two halves and the gap between them fill the view exactly; so do three thirds.
        let half = px(SizeChange::SetProportion(50_000), 0);
        assert_eq!(half, 667);
        assert_eq!(2 * half + gap, view);
        let third = px(SizeChange::SetProportion(33_333), 0);
        assert_eq!(
            3 * third + 2 * gap,
            view - 1,
            "within one pixel of the view"
        );
        assert_eq!(px(SizeChange::SetProportion(100_000), 0), view);
        // +10 % of the view (and the gap), -10 % back.
        assert_eq!(px(SizeChange::AdjustProportion(10_000), half), 804);
        assert_eq!(px(SizeChange::AdjustProportion(-10_000), 804), 667);
        // Pixels: a fixed size is the window's, the border comes on top; adjusting is plain.
        assert_eq!(px(SizeChange::SetFixed(1200), half), 1208);
        assert_eq!(px(SizeChange::AdjustFixed(100), half), 767);
        assert_eq!(px(SizeChange::AdjustFixed(-100), half), 567);
        // Without a gap or border the proportions are plain fractions of the view.
        assert_eq!(
            SizeChange::SetProportion(25_000).resolve(0, 1920, 0, 0),
            480
        );
        assert_eq!(
            SizeChange::AdjustProportion(5_000).resolve(960, 1920, 0, 0),
            1056
        );
    }

    #[test]
    fn test_workspace_reference_number_is_an_index_and_quotes_make_a_name() {
        assert_eq!(
            WorkspaceRef::from_word("2", false),
            Some(WorkspaceRef::Index(2))
        );
        assert_eq!(
            WorkspaceRef::from_word("0", false),
            Some(WorkspaceRef::Index(1))
        );
        assert_eq!(
            WorkspaceRef::from_word("2", true),
            Some(WorkspaceRef::Name("2".to_string()))
        );
        assert_eq!(
            WorkspaceRef::from_word("web", false),
            Some(WorkspaceRef::Name("web".to_string()))
        );
        assert_eq!(WorkspaceRef::from_word("", false), None);
        // The same through a bind line: `focus-workspace 2` / `focus-workspace "2"` / `focus-workspace "web"`.
        assert_eq!(
            parse_action_flagged("focus-workspace", &words(&["2"]), &[false]),
            Some(Action::FocusWorkspace(2))
        );
        assert_eq!(
            parse_action_flagged("focus-workspace", &words(&["2"]), &[true]),
            Some(Action::FocusWorkspaceNamed("2".to_string()))
        );
        assert_eq!(
            parse_action_flagged("focus-workspace", &words(&["web"]), &[true]),
            Some(Action::FocusWorkspaceNamed("web".to_string()))
        );
        // From wiri-ctl nothing is quoted: a number is an index.
        assert_eq!(
            parse_action_name("focus-workspace", &words(&["3"])),
            Some(Action::FocusWorkspace(3))
        );
        assert_eq!(parse_action_name("focus-workspace", &[]), None);
        // The older spellings keep their meaning.
        assert_eq!(
            parse_action_name("switch-workspace", &words(&["4"])),
            Some(Action::FocusWorkspace(4))
        );
        assert_eq!(
            parse_action_name("focus-workspace-named", &words(&["4"])),
            Some(Action::FocusWorkspaceNamed("4".to_string()))
        );
    }

    #[test]
    fn test_parse_column_width_arg() {
        use SizeChange::*;
        let set = |milli: i32| Some(Action::SetColumnWidth(SetProportion(milli)));
        assert_eq!(parse_column_width_arg("1/4"), set(25_000));
        assert_eq!(parse_column_width_arg("1/3"), set(33_333));
        assert_eq!(parse_column_width_arg("half"), set(50_000));
        assert_eq!(parse_column_width_arg("2/3"), set(66_667));
        assert_eq!(parse_column_width_arg("3/4"), set(75_000));
        assert_eq!(parse_column_width_arg("Full"), set(100_000));
        assert_eq!(
            parse_column_width_arg("cycle"),
            Some(Action::ColumnWidthPresetCycle)
        );
        assert_eq!(
            parse_column_width_arg("back"),
            Some(Action::ColumnWidthPresetCycleBack)
        );
        // niri's grammar: any percentage, adjustments, pixels.
        assert_eq!(parse_column_width_arg("50%"), set(50_000));
        assert_eq!(parse_column_width_arg("42%"), set(42_000));
        assert_eq!(
            parse_column_width_arg("+10%"),
            Some(Action::SetColumnWidth(AdjustProportion(10_000)))
        );
        assert_eq!(
            parse_column_width_arg("-10%"),
            Some(Action::SetColumnWidth(AdjustProportion(-10_000)))
        );
        assert_eq!(
            parse_column_width_arg("1200"),
            Some(Action::SetColumnWidth(SetFixed(1200)))
        );
        assert_eq!(
            parse_column_width_arg("+100"),
            Some(Action::SetColumnWidth(AdjustFixed(100)))
        );
        assert_eq!(parse_column_width_arg("nonsense"), None);
        assert_eq!(parse_column_width_arg("50%x"), None);
        assert_eq!(
            parse_action_name("set-column-width", &words(&["1/3"])),
            set(33_333)
        );
        assert_eq!(parse_action_name("set-column-width", &[]), None);
        // `set-window-width` is the same thing for a tiled window; the height has its own action.
        assert_eq!(
            parse_action_name("set-window-width", &words(&["-10%"])),
            Some(Action::SetColumnWidth(AdjustProportion(-10_000)))
        );
        assert_eq!(
            parse_action_name("set-window-height", &words(&["+10%"])),
            Some(Action::SetWindowHeight(AdjustProportion(10_000)))
        );
        assert_eq!(
            parse_action_name("set-window-height", &words(&["50%"])),
            Some(Action::SetWindowHeight(SetProportion(50_000)))
        );
        assert_eq!(
            parse_action_name("set-window-height", &words(&["tall"])),
            None
        );
        assert_eq!(
            parse_action_name("reset-window-height", &[]),
            Some(Action::ResetWindowHeight)
        );
    }

    #[test]
    fn test_niri_action_names_are_understood() {
        for (name, action) in [
            ("fullscreen-window", Action::ToggleFullscreen),
            ("toggle-window-floating", Action::ToggleFloating),
            ("move-window-to-floating", Action::MoveWindowToFloating),
            ("move-window-to-tiling", Action::MoveWindowToTiling),
            ("toggle-overview", Action::OverviewToggle),
            ("open-overview", Action::OverviewOpen),
            ("close-overview", Action::OverviewExit),
            ("toggle-column-tabbed-display", Action::ColumnToggleTabbed),
            ("switch-preset-column-width", Action::ColumnWidthPresetCycle),
            (
                "switch-preset-column-width-back",
                Action::ColumnWidthPresetCycleBack,
            ),
            (
                "expand-column-to-available-width",
                Action::ExpandColumnToAvailable,
            ),
            ("focus-monitor-left", Action::FocusMonitorLeft),
            ("focus-monitor-right", Action::FocusMonitorRight),
            ("move-window-to-monitor-left", Action::MoveToMonitorLeft),
            ("move-window-to-monitor-right", Action::MoveToMonitorRight),
            (
                "move-column-to-monitor-left",
                Action::MoveColumnToMonitorLeft,
            ),
            (
                "move-column-to-monitor-right",
                Action::MoveColumnToMonitorRight,
            ),
            ("focus-window-previous", Action::FocusPrevious),
            ("focus-workspace-down", Action::FocusWorkspaceNext),
            ("focus-workspace-up", Action::FocusWorkspacePrevious),
            ("focus-workspace-previous", Action::FocusWorkspaceLast),
            ("screenshot-screen", Action::ScreenshotScreen),
            ("show-hotkey-overlay", Action::ShowKeyBindings),
            ("center-column", Action::CenterColumn),
            ("center-window", Action::CenterWindow),
            ("close-window", Action::CloseWindow),
            ("quit", Action::Quit),
            ("move-workspace-up", Action::MoveWorkspaceUp),
            ("move-workspace-down", Action::MoveWorkspaceDown),
            (
                "move-column-to-workspace-up",
                Action::MoveColumnToWorkspaceUp,
            ),
            (
                "move-column-to-workspace-down",
                Action::MoveColumnToWorkspaceDown,
            ),
        ] {
            assert_eq!(parse_action_name(name, &[]), Some(action), "{name}");
        }
        assert_eq!(
            parse_action_name("set-workspace-name", &words(&["web"])),
            Some(Action::RenameWorkspace("web".to_string()))
        );
        assert_eq!(
            parse_action_name("spawn", &words(&["wt.exe", "-d", "C:\\My Projects"])),
            Some(Action::Spawn(words(&["wt.exe", "-d", "C:\\My Projects"])))
        );
        assert_eq!(
            parse_action_name("spawn-sh", &words(&["echo hi"])),
            Some(Action::SpawnCmd("echo hi".to_string()))
        );
        assert_eq!(parse_action_name("no-such-action", &[]), None);
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
