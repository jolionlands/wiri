//! Census of the pinned niri action vocabulary (niri `e19df12cf8`) against the three ways wiri
//! takes an action: a KDL `binds {}` line, `wiri-ctl action <name> <words>` and the typed IPC
//! request `{"Action": {...}}`.
//!
//! Every row carries niri's exact grammar for the action (the KDL node of
//! niri:niri-config/src/binds.rs:105-366, the words niri's clap CLI accepts, the serde JSON of
//! niri:niri-ipc/src/lib.rs:194-962), runs it through the very functions the daemon uses and
//! asserts the outcome: accepted for every action wiri has an executor for, rejected with an
//! explicit error for the actions that are N/A on Windows.  The table is also the source of
//! `docs/parity/action-census.md`: `cargo test --lib print_census_markdown -- --ignored
//! --nocapture` prints it with the *measured* KDL / CLI / typed-IPC columns.
//!
//! The pipeline mirrors `IpcServer::process_message` for `RunAction` up to the effect
//! boundary (src/ipc/server.rs, the `RunAction` arm): `move-floating-window`, the screenshot
//! option parsers, [`super::server::split_action_target`], the system-action parser and
//! [`crate::input::parse_action_flagged`].  Executors need a live desktop and are covered by
//! the engine tests and the live gates the matrices cite.
use serde_json::{json, Value};

use super::server::split_action_target;
use super::{parse_move_floating_window, wire, IpcMessage};
use crate::input::parse_action_flagged;

/// The 136 names of niri's `Action` enum that a KDL bind accepts (binds.rs, every variant that
/// is not `#[knuffel(skip)]`), in declaration order.
const NIRI_KDL_ACTIONS: &[&str] = &[
    "quit", "suspend", "power-off-monitors", "power-on-monitors", "toggle-debug-tint",
    "debug-toggle-opaque-regions", "debug-toggle-damage", "toggle-touchpad", "spawn",
    "spawn-sh", "do-screen-transition", "screenshot", "screenshot-screen", "screenshot-window",
    "toggle-keyboard-shortcuts-inhibit", "close-window", "fullscreen-window",
    "toggle-windowed-fullscreen", "focus-window-in-column", "focus-window-previous",
    "focus-column-left", "focus-column-right", "focus-column-first", "focus-column-last",
    "focus-column-right-or-first", "focus-column-left-or-last", "focus-column",
    "focus-window-or-monitor-up", "focus-window-or-monitor-down",
    "focus-column-or-monitor-left", "focus-column-or-monitor-right", "focus-window-down",
    "focus-window-up", "focus-window-down-or-column-left", "focus-window-down-or-column-right",
    "focus-window-up-or-column-left", "focus-window-up-or-column-right",
    "focus-window-or-workspace-down", "focus-window-or-workspace-up", "focus-window-top",
    "focus-window-bottom", "focus-window-down-or-top", "focus-window-up-or-bottom",
    "move-column-left", "move-column-right", "move-column-to-first", "move-column-to-last",
    "move-column-left-or-to-monitor-left", "move-column-right-or-to-monitor-right",
    "move-column-to-index", "move-window-down", "move-window-up",
    "move-window-down-or-to-workspace-down", "move-window-up-or-to-workspace-up",
    "consume-or-expel-window-left", "consume-or-expel-window-right",
    "consume-window-into-column", "expel-window-from-column", "swap-window-left",
    "swap-window-right", "toggle-column-tabbed-display", "set-column-display", "center-column",
    "center-window", "center-visible-columns", "focus-workspace-down", "focus-workspace-up",
    "focus-workspace", "focus-workspace-previous", "move-window-to-workspace-down",
    "move-window-to-workspace-up", "move-window-to-workspace", "move-column-to-workspace-down",
    "move-column-to-workspace-up", "move-column-to-workspace", "move-workspace-down",
    "move-workspace-up", "move-workspace-to-index", "move-workspace-to-monitor",
    "set-workspace-name", "unset-workspace-name", "focus-monitor-left", "focus-monitor-right",
    "focus-monitor-down", "focus-monitor-up", "focus-monitor-previous", "focus-monitor-next",
    "focus-monitor", "move-window-to-monitor-left", "move-window-to-monitor-right",
    "move-window-to-monitor-down", "move-window-to-monitor-up",
    "move-window-to-monitor-previous", "move-window-to-monitor-next", "move-window-to-monitor",
    "move-column-to-monitor-left", "move-column-to-monitor-right",
    "move-column-to-monitor-down", "move-column-to-monitor-up",
    "move-column-to-monitor-previous", "move-column-to-monitor-next", "move-column-to-monitor",
    "set-window-width", "set-window-height", "reset-window-height",
    "switch-preset-column-width", "switch-preset-column-width-back",
    "switch-preset-window-width", "switch-preset-window-width-back",
    "switch-preset-window-height", "switch-preset-window-height-back", "maximize-column",
    "maximize-window-to-edges", "set-column-width", "expand-column-to-available-width",
    "switch-layout", "show-hotkey-overlay", "move-workspace-to-monitor-left",
    "move-workspace-to-monitor-right", "move-workspace-to-monitor-down",
    "move-workspace-to-monitor-up", "move-workspace-to-monitor-previous",
    "move-workspace-to-monitor-next", "toggle-window-floating", "move-window-to-floating",
    "move-window-to-tiling", "focus-floating", "focus-tiling",
    "switch-focus-between-floating-and-tiling", "toggle-window-rule-opacity",
    "set-dynamic-cast-window", "set-dynamic-cast-monitor", "clear-dynamic-cast-target",
    "toggle-overview", "open-overview", "close-overview",
];

/// The 142 variants of `niri_ipc::Action` (lib.rs:194-962), in declaration order, as kebab-case.
const NIRI_IPC_ACTIONS: &[&str] = &[
    "quit", "power-off-monitors", "power-on-monitors", "spawn", "spawn-sh",
    "do-screen-transition", "screenshot", "screenshot-screen", "screenshot-window",
    "toggle-keyboard-shortcuts-inhibit", "close-window", "fullscreen-window",
    "toggle-windowed-fullscreen", "focus-window", "focus-window-in-column",
    "focus-window-previous", "focus-column-left", "focus-column-right", "focus-column-first",
    "focus-column-last", "focus-column-right-or-first", "focus-column-left-or-last",
    "focus-column", "focus-window-or-monitor-up", "focus-window-or-monitor-down",
    "focus-column-or-monitor-left", "focus-column-or-monitor-right", "focus-window-down",
    "focus-window-up", "focus-window-down-or-column-left", "focus-window-down-or-column-right",
    "focus-window-up-or-column-left", "focus-window-up-or-column-right",
    "focus-window-or-workspace-down", "focus-window-or-workspace-up", "focus-window-top",
    "focus-window-bottom", "focus-window-down-or-top", "focus-window-up-or-bottom",
    "move-column-left", "move-column-right", "move-column-to-first", "move-column-to-last",
    "move-column-left-or-to-monitor-left", "move-column-right-or-to-monitor-right",
    "move-column-to-index", "move-window-down", "move-window-up",
    "move-window-down-or-to-workspace-down", "move-window-up-or-to-workspace-up",
    "consume-or-expel-window-left", "consume-or-expel-window-right",
    "consume-window-into-column", "expel-window-from-column", "swap-window-right",
    "swap-window-left", "toggle-column-tabbed-display", "set-column-display", "center-column",
    "center-window", "center-visible-columns", "focus-workspace-down", "focus-workspace-up",
    "focus-workspace", "focus-workspace-previous", "move-window-to-workspace-down",
    "move-window-to-workspace-up", "move-window-to-workspace", "move-column-to-workspace-down",
    "move-column-to-workspace-up", "move-column-to-workspace", "move-workspace-down",
    "move-workspace-up", "move-workspace-to-index", "set-workspace-name",
    "unset-workspace-name", "focus-monitor-left", "focus-monitor-right", "focus-monitor-down",
    "focus-monitor-up", "focus-monitor-previous", "focus-monitor-next", "focus-monitor",
    "move-window-to-monitor-left", "move-window-to-monitor-right",
    "move-window-to-monitor-down", "move-window-to-monitor-up",
    "move-window-to-monitor-previous", "move-window-to-monitor-next", "move-window-to-monitor",
    "move-column-to-monitor-left", "move-column-to-monitor-right",
    "move-column-to-monitor-down", "move-column-to-monitor-up",
    "move-column-to-monitor-previous", "move-column-to-monitor-next", "move-column-to-monitor",
    "set-window-width", "set-window-height", "reset-window-height",
    "switch-preset-column-width", "switch-preset-column-width-back",
    "switch-preset-window-width", "switch-preset-window-width-back",
    "switch-preset-window-height", "switch-preset-window-height-back", "maximize-column",
    "maximize-window-to-edges", "set-column-width", "expand-column-to-available-width",
    "switch-layout", "show-hotkey-overlay", "move-workspace-to-monitor-left",
    "move-workspace-to-monitor-right", "move-workspace-to-monitor-down",
    "move-workspace-to-monitor-up", "move-workspace-to-monitor-previous",
    "move-workspace-to-monitor-next", "move-workspace-to-monitor", "toggle-debug-tint",
    "debug-toggle-opaque-regions", "debug-toggle-damage", "toggle-touchpad",
    "toggle-window-floating", "move-window-to-floating", "move-window-to-tiling",
    "focus-floating", "focus-tiling", "switch-focus-between-floating-and-tiling",
    "move-floating-window", "toggle-window-rule-opacity", "set-dynamic-cast-window",
    "set-dynamic-cast-monitor", "clear-dynamic-cast-target", "stop-cast", "toggle-overview",
    "open-overview", "close-overview", "toggle-window-urgent", "set-window-urgent",
    "unset-window-urgent", "load-config-file",
];

#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    Accepted(String),
    Rejected(String),
}

impl Outcome {
    fn ok(&self) -> bool {
        matches!(self, Outcome::Accepted(_))
    }
}

/// How an action stands against its niri contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Same words, fields, defaults and omitted-axis meaning on every entry point niri has.
    Full,
    /// The contract matches; the executor still carries a recorded gap (see the note).
    Partial,
    /// Wayland or compositor specific: wiri refuses it with an error on every entry point.
    NotApplicable,
}

struct Row {
    name: &'static str,
    fields: String,
    /// niri accepts the action in a `binds {}` block (not `#[knuffel(skip)]`).
    niri_kdl: bool,
    /// niri has a typed IPC variant and a `niri msg action` subcommand for it.
    niri_ipc: bool,
    kdl: Vec<String>,
    cli: Vec<Vec<String>>,
    ipc: Vec<String>,
    kind: Kind,
    executor: &'static str,
    note: &'static str,
}

fn pascal(name: &str) -> String {
    name.split('-')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

impl Row {
    fn fields(mut self, fields: &str) -> Self {
        self.fields = fields.to_string();
        self
    }
    fn kdl(mut self, samples: &[&str]) -> Self {
        self.kdl = samples.iter().map(|s| s.to_string()).collect();
        self
    }
    fn cli(mut self, samples: &[&[&str]]) -> Self {
        self.cli = samples
            .iter()
            .map(|words| words.iter().map(|w| w.to_string()).collect())
            .collect();
        self
    }
    fn ipc(mut self, samples: &[&str]) -> Self {
        self.ipc = samples.iter().map(|s| s.to_string()).collect();
        self
    }
    fn not_bindable(mut self) -> Self {
        self.niri_kdl = false;
        self
    }
    fn no_niri_ipc(mut self) -> Self {
        self.niri_ipc = false;
        self
    }
    fn kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }
    fn note(mut self, note: &'static str) -> Self {
        self.note = note;
        self
    }
    fn na(self, note: &'static str) -> Self {
        self.kind(Kind::NotApplicable).note(note)
    }
}

fn a(name: &'static str, executor: &'static str) -> Row {
    Row {
        name,
        fields: String::new(),
        niri_kdl: true,
        niri_ipc: true,
        kdl: Vec::new(),
        cli: Vec::new(),
        ipc: Vec::new(),
        kind: Kind::Full,
        executor,
        note: "",
    }
}

/// An action without fields.
fn unit(name: &'static str, executor: &'static str) -> Row {
    let typed = format!(r#"{{"{}":{{}}}}"#, pascal(name));
    a(name, executor)
        .kdl(&[name])
        .cli(&[&[]])
        .ipc(&[typed.as_str()])
}

/// An action whose only field is `id: Option<u64>`.
fn with_id(name: &'static str, executor: &'static str) -> Row {
    let plain = format!(r#"{{"{}":{{}}}}"#, pascal(name));
    let by_id = format!(r#"{{"{}":{{"id":5}}}}"#, pascal(name));
    a(name, executor)
        .fields("id: Option<u64> = None")
        .kdl(&[name])
        .cli(&[&[], &["--id", "5"]])
        .ipc(&[plain.as_str(), by_id.as_str()])
}

/// The six directional / relative variants of a monitor action.
fn monitor_directions(prefix: &'static str, executor: &'static str) -> Vec<Row> {
    ["left", "right", "down", "up", "previous", "next"]
        .iter()
        .map(|direction| {
            let name: &'static str = Box::leak(format!("{prefix}-{direction}").into_boxed_str());
            unit(name, executor)
        })
        .collect()
}

fn rows() -> Vec<Row> {
    const WS: &str = "reference: WorkspaceReferenceArg (Id | Index u8 | Name)";
    let mut rows = vec![
        a("quit", "System(Quit) -> system_actions::request_quit (confirmation unless skipped)")
            .fields("skip_confirmation: bool = false (KDL skip-confirmation=, CLI -s/--skip-confirmation)")
            .kdl(&["quit", "quit skip-confirmation=true", "quit skip-confirmation=false"])
            .cli(&[&[], &["-s"], &["--skip-confirmation"]])
            .ipc(&[r#"{"Quit":{}}"#, r#"{"Quit":{"skip_confirmation":true}}"#]),
        a("suspend", "System(Suspend) -> system_actions::request_power (SetSuspendState)")
            .fields("(no fields)")
            .kdl(&["suspend"])
            .cli(&[&[]])
            .ipc(&[r#"{"Suspend":{}}"#])
            .no_niri_ipc()
            .note("KDL-only in niri (binds.rs:109, no IPC variant); the CLI and typed forms are a wiri extension"),
        unit("power-off-monitors", "System(PowerOffMonitors) -> WM_SYSCOMMAND SC_MONITORPOWER 2"),
        unit("power-on-monitors", "System(PowerOnMonitors) -> WM_SYSCOMMAND SC_MONITORPOWER -1"),
        a("spawn", "Spawn -> message_loop::spawn_process (exact argv)")
            .fields("command: Vec<String> (required)")
            .kdl(&[r#"spawn "wt.exe" "-d" "C:/work""#])
            .cli(&[&["wt.exe", "-d", "C:/work"], &["tool", "--id", "5"]])
            .ipc(&[
                r#"{"Spawn":{"command":["wt.exe","-d","C:/work"]}}"#,
                r#"{"Spawn":{"command":["tool","--id","5"]}}"#,
            ])
            .kind(Kind::Partial)
            .note("matrix B: percent-variable expansion and secure-desktop (locked) behaviour are not pinned requirements; `--id` inside the argv is the program's, not a window target"),
        a("spawn-sh", "SpawnCmd -> message_loop::spawn_shell_command (cmd.exe /C)")
            .fields("command: String (required)")
            .kdl(&[r#"spawn-sh "echo hi""#])
            .cli(&[&["echo hi"]])
            .ipc(&[r#"{"SpawnSh":{"command":"echo hi"}}"#])
            .note("cmd.exe syntax stands in for sh"),
        a("do-screen-transition", "none")
            .fields("delay_ms: Option<u16> = None (KDL delay-ms=, CLI -d/--delay-ms)")
            .kdl(&["do-screen-transition delay-ms=200"])
            .cli(&[&["-d", "200"]])
            .ipc(&[r#"{"DoScreenTransition":{"delay_ms":200}}"#])
            .na("niri's own screenshot-and-crossfade render effect; DWM composes the desktop and wiri animates with its own driver"),
        a("screenshot", "ScreenshotUi(options) -> overlay::screenshot_ui::open")
            .fields("show_pointer: bool = true, path: Option<String> = None (absolute)")
            .kdl(&["screenshot", "screenshot show-pointer=false"])
            .cli(&[&[], &["-p", "false"], &["--path", "C:/shots/a.png"]])
            .ipc(&[
                r#"{"Screenshot":{}}"#,
                r#"{"Screenshot":{"show_pointer":false,"path":"C:/shots/a.png"}}"#,
            ]),
        a("screenshot-screen", "Capture(options) -> screenshot::action::CaptureAction::run")
            .fields("write_to_disk: bool = true, show_pointer: bool = true, path: Option<String> = None")
            .kdl(&["screenshot-screen", "screenshot-screen write-to-disk=false show-pointer=false"])
            .cli(&[&[], &["-d", "false", "-p", "false"]])
            .ipc(&[
                r#"{"ScreenshotScreen":{}}"#,
                r#"{"ScreenshotScreen":{"write_to_disk":false,"show_pointer":false}}"#,
            ]),
        a("screenshot-window", "Capture(options) -> screenshot::action::CaptureAction::run (explicit id, no focus change)")
            .fields("id: Option<u64> = None, write_to_disk: bool = true, show_pointer: bool = FALSE, path: Option<String> = None")
            .kdl(&["screenshot-window", "screenshot-window write-to-disk=false show-pointer=true"])
            .cli(&[&[], &["--id", "5", "-d", "false"]])
            .ipc(&[
                r#"{"ScreenshotWindow":{}}"#,
                r#"{"ScreenshotWindow":{"id":5,"write_to_disk":false}}"#,
            ]),
        unit("toggle-keyboard-shortcuts-inhibit", "ToggleKeyboardShortcutsInhibit -> keyboard::ENGINE.toggle_inhibit")
            .kind(Kind::Partial)
            .note("manual global inhibitor only; Windows has no per-client inhibit protocol (matrix B)"),
        with_id("close-window", "CloseWindow -> engine.close_target (by id: IpcMessage::CloseWindow, focus unchanged)"),
        with_id("fullscreen-window", "ToggleFullscreen -> engine.toggle_fullscreen_for"),
        with_id("toggle-windowed-fullscreen", "none")
            .na("Win32 has no \"you are fullscreen\" state to send to a foreign application (matrix A)"),
        a("focus-window", "ipc::server::focus_window_by_hwnd")
            .fields("id: u64 (required; no KDL form)")
            .kdl(&["focus-window"])
            .cli(&[&["--id", "5"]])
            .ipc(&[r#"{"FocusWindow":{"id":5}}"#])
            .not_bindable(),
        a("focus-window-in-column", "LayoutFocus(WindowInColumn) -> engine.focus_layout_action")
            .fields("index: u8 (1-based; 0 is the first)")
            .kdl(&["focus-window-in-column 2"])
            .cli(&[&["2"]])
            .ipc(&[r#"{"FocusWindowInColumn":{"index":2}}"#]),
        unit("focus-window-previous", "LayoutFocus(Previous) / FocusPrevious -> engine.focus_previous_window"),
        unit("focus-column-left", "FocusColumnLeft -> engine.focus_left"),
        unit("focus-column-right", "FocusColumnRight -> engine.focus_right"),
        unit("focus-column-first", "FocusColumnFirst -> engine.focus_column_first"),
        unit("focus-column-last", "FocusColumnLast -> engine.focus_column_last"),
        unit("focus-column-right-or-first", "LayoutFocus(ColumnRightOrFirst) -> engine.focus_layout_action"),
        unit("focus-column-left-or-last", "LayoutFocus(ColumnLeftOrLast) -> engine.focus_layout_action"),
        a("focus-column", "LayoutFocus(Column) -> engine.focus_layout_action")
            .fields("index: usize (1-based; 0 is the first)")
            .kdl(&["focus-column 3"])
            .cli(&[&["3"]])
            .ipc(&[r#"{"FocusColumn":{"index":3}}"#]),
        unit("focus-window-or-monitor-up", "LayoutFocus(WindowOrMonitorUp) -> engine.focus_layout_action"),
        unit("focus-window-or-monitor-down", "LayoutFocus(WindowOrMonitorDown) -> engine.focus_layout_action"),
        unit("focus-column-or-monitor-left", "LayoutFocus(ColumnOrMonitorLeft) -> engine.focus_layout_action"),
        unit("focus-column-or-monitor-right", "LayoutFocus(ColumnOrMonitorRight) -> engine.focus_layout_action"),
        unit("focus-window-down", "FocusDown -> engine.focus_down"),
        unit("focus-window-up", "FocusUp -> engine.focus_up"),
        unit("focus-window-down-or-column-left", "LayoutFocus(DownOrColumnLeft) -> engine.focus_layout_action"),
        unit("focus-window-down-or-column-right", "LayoutFocus(DownOrColumnRight) -> engine.focus_layout_action"),
        unit("focus-window-up-or-column-left", "LayoutFocus(UpOrColumnLeft) -> engine.focus_layout_action"),
        unit("focus-window-up-or-column-right", "LayoutFocus(UpOrColumnRight) -> engine.focus_layout_action"),
        unit("focus-window-or-workspace-down", "LayoutFocus(WindowOrWorkspaceDown) -> engine.focus_layout_action"),
        unit("focus-window-or-workspace-up", "LayoutFocus(WindowOrWorkspaceUp) -> engine.focus_layout_action"),
        unit("focus-window-top", "LayoutFocus(WindowTop) -> engine.focus_layout_action"),
        unit("focus-window-bottom", "LayoutFocus(WindowBottom) -> engine.focus_layout_action"),
        unit("focus-window-down-or-top", "LayoutFocus(DownOrTop) -> engine.focus_layout_action"),
        unit("focus-window-up-or-bottom", "LayoutFocus(UpOrBottom) -> engine.focus_layout_action"),
        unit("move-column-left", "MoveColumnLeft -> engine.move_column (floating: float_move_by)"),
        unit("move-column-right", "MoveColumnRight -> engine.move_column (floating: float_move_by)"),
        unit("move-column-to-first", "MoveColumnToFirst -> engine.move_column_to_edge(false)"),
        unit("move-column-to-last", "MoveColumnToLast -> engine.move_column_to_edge(true)"),
        unit("move-column-left-or-to-monitor-left", "MoveColumnLeftOrToMonitorLeft -> engine.move_column_or_to_monitor"),
        unit("move-column-right-or-to-monitor-right", "MoveColumnRightOrToMonitorRight -> engine.move_column_or_to_monitor"),
        a("move-column-to-index", "MoveColumnToIndex -> engine.move_column_to_index")
            .fields("index: usize (1-based like niri scrolling.rs:1707; 0 is the first)")
            .kdl(&["move-column-to-index 2"])
            .cli(&[&["2"]])
            .ipc(&[r#"{"MoveColumnToIndex":{"index":2}}"#]),
        unit("move-window-down", "MoveWindowDown -> engine.move_window_in_column(1) (floating: float_move_by)"),
        unit("move-window-up", "MoveWindowUp -> engine.move_window_in_column(-1) (floating: float_move_by)"),
        unit("move-window-down-or-to-workspace-down", "MoveWindowDownOrToWorkspaceDown -> engine.move_window_or_to_workspace(true)"),
        unit("move-window-up-or-to-workspace-up", "MoveWindowUpOrToWorkspaceUp -> engine.move_window_or_to_workspace(false)"),
        with_id("consume-or-expel-window-left", "ConsumeOrExpelWindowLeft -> engine.consume_or_expel_window_left"),
        with_id("consume-or-expel-window-right", "ConsumeOrExpelWindowRight -> engine.consume_or_expel_window_right"),
        unit("consume-window-into-column", "ConsumeWindowIntoColumn -> engine.consume_window_into_column"),
        unit("expel-window-from-column", "ExpelWindowFromColumn -> engine.expel_window_from_column"),
        unit("swap-window-right", "SwapWindowRight -> engine.swap_window_toward(true)"),
        unit("swap-window-left", "SwapWindowLeft -> engine.swap_window_toward(false)"),
        unit("toggle-column-tabbed-display", "ColumnToggleTabbed -> engine.toggle_tabbed_for_focused_column"),
        a("set-column-display", "SetColumnDisplay -> message_loop::set_column_display")
            .fields("display: ColumnDisplay (Normal | Tabbed)")
            .kdl(&[r#"set-column-display "tabbed""#, r#"set-column-display "normal""#])
            .cli(&[&["tabbed"], &["normal"]])
            .ipc(&[
                r#"{"SetColumnDisplay":{"display":"Tabbed"}}"#,
                r#"{"SetColumnDisplay":{"display":"Normal"}}"#,
            ]),
        unit("center-column", "CenterColumn -> engine.center_focused_column (floating: float_center)")
            .kind(Kind::Partial)
            .note("explicit-action evidence for variable-width edge columns and mixed outputs pending (matrix A)"),
        with_id("center-window", "CenterWindow -> engine.center_focused_column / float_center"),
        unit("center-visible-columns", "CenterVisibleColumns -> engine.center_visible_columns"),
        unit("focus-workspace-down", "FocusWorkspaceNext -> engine.focus_workspace_relative(Next)"),
        unit("focus-workspace-up", "FocusWorkspacePrevious -> engine.focus_workspace_relative(Previous)"),
        a("focus-workspace", "FocusWorkspace / FocusWorkspaceNamed / WorkspaceAction(Focus id) -> engine.focus_workspace_*")
            .fields(WS)
            .kdl(&["focus-workspace 2", r#"focus-workspace "web""#])
            .cli(&[&["2"], &["web"]])
            .ipc(&[
                r#"{"FocusWorkspace":{"reference":{"Index":2}}}"#,
                r#"{"FocusWorkspace":{"reference":{"Name":"web"}}}"#,
                r#"{"FocusWorkspace":{"reference":{"Id":7}}}"#,
            ]),
        unit("focus-workspace-previous", "FocusWorkspaceLast -> engine.focus_workspace_last_visited"),
    ];
    for name in [
        "move-window-to-workspace-down",
        "move-window-to-workspace-up",
        "move-column-to-workspace-down",
        "move-column-to-workspace-up",
    ] {
        let typed = pascal(name);
        let plain = format!(r#"{{"{typed}":{{}}}}"#);
        let unfocused = format!(r#"{{"{typed}":{{"focus":false}}}}"#);
        let kdl_unfocused = format!("{name} focus=false");
        rows.push(
            a(name, "WorkspaceAction(MoveWindowRelative / MoveColumnRelative) -> engine.workspace_layout_action")
                .fields("focus: bool = true (KDL focus=, CLI --focus <bool>)")
                .kdl(&[name, kdl_unfocused.as_str()])
                .cli(&[&[], &["--focus", "false"]])
                .ipc(&[plain.as_str(), unfocused.as_str()]),
        );
    }
    rows.extend([
        a("move-window-to-workspace", "WorkspaceAction(MoveWindow) -> engine.workspace_layout_action (explicit id: no focus change)")
            .fields("window_id: Option<u64> = None, reference: WorkspaceReferenceArg, focus: bool = true")
            .kdl(&["move-window-to-workspace 2", r#"move-window-to-workspace "web" focus=false"#])
            .cli(&[&["2"], &["--window-id", "5", "--focus", "false", "web"]])
            .ipc(&[
                r#"{"MoveWindowToWorkspace":{"reference":{"Index":2}}}"#,
                r#"{"MoveWindowToWorkspace":{"window_id":5,"reference":{"Name":"web"},"focus":false}}"#,
            ]),
        a("move-column-to-workspace", "WorkspaceAction(MoveColumn) -> engine.workspace_layout_action")
            .fields("reference: WorkspaceReferenceArg, focus: bool = true")
            .kdl(&["move-column-to-workspace 2", r#"move-column-to-workspace "web" focus=false"#])
            .cli(&[&["2"], &["--focus", "false", "web"]])
            .ipc(&[
                r#"{"MoveColumnToWorkspace":{"reference":{"Index":2}}}"#,
                r#"{"MoveColumnToWorkspace":{"reference":{"Name":"web"},"focus":false}}"#,
            ]),
        unit("move-workspace-down", "MoveWorkspaceDown -> engine.move_active_workspace(1)"),
        unit("move-workspace-up", "MoveWorkspaceUp -> engine.move_active_workspace(-1)"),
        a("move-workspace-to-index", "WorkspaceAction(MoveToIndex) -> engine.workspace_layout_action")
            .fields("index: usize (1-based), reference: Option<WorkspaceReferenceArg> = None (CLI --reference)")
            .kdl(&["move-workspace-to-index 2"])
            .cli(&[&["2"], &["2", "--reference", "web"]])
            .ipc(&[
                r#"{"MoveWorkspaceToIndex":{"index":2}}"#,
                r#"{"MoveWorkspaceToIndex":{"index":2,"reference":{"Name":"web"}}}"#,
            ]),
        a("set-workspace-name", "RenameWorkspace / WorkspaceAction(SetName) -> engine.workspace_layout_action")
            .fields("name: String, workspace: Option<WorkspaceReferenceArg> = None (CLI --workspace)")
            .kdl(&[r#"set-workspace-name "web""#])
            .cli(&[&["web"], &["web", "--workspace", "2"]])
            .ipc(&[
                r#"{"SetWorkspaceName":{"name":"web"}}"#,
                r#"{"SetWorkspaceName":{"name":"web","workspace":{"Index":2}}}"#,
            ]),
        a("unset-workspace-name", "WorkspaceAction(UnsetName) -> engine.workspace_layout_action")
            .fields("reference: Option<WorkspaceReferenceArg> = None (CLI positional)")
            .kdl(&["unset-workspace-name"])
            .cli(&[&[], &["2"]])
            .ipc(&[
                r#"{"UnsetWorkspaceName":{}}"#,
                r#"{"UnsetWorkspaceName":{"reference":{"Name":"web"}}}"#,
            ]),
    ]);
    rows.extend(monitor_directions("focus-monitor", "MonitorAction(Focus) -> engine.monitor_layout_action"));
    rows.push(
        a("focus-monitor", "MonitorAction(Focus(Name)) -> engine.monitor_layout_action")
            .fields("output: String")
            .kdl(&[r#"focus-monitor "DP-1""#])
            .cli(&[&["DP-1"]])
            .ipc(&[r#"{"FocusMonitor":{"output":"DP-1"}}"#]),
    );
    rows.extend(monitor_directions("move-window-to-monitor", "MonitorAction(MoveWindow) -> engine.monitor_layout_action"));
    rows.push(
        a("move-window-to-monitor", "MonitorAction(MoveWindow(Name)) -> engine.monitor_layout_action (explicit id: no focus change)")
            .fields("id: Option<u64> = None, output: String")
            .kdl(&[r#"move-window-to-monitor "DP-1""#])
            .cli(&[&["DP-1"], &["--id", "5", "DP-1"]])
            .ipc(&[
                r#"{"MoveWindowToMonitor":{"output":"DP-1"}}"#,
                r#"{"MoveWindowToMonitor":{"id":5,"output":"DP-1"}}"#,
            ]),
    );
    rows.extend(monitor_directions("move-column-to-monitor", "MonitorAction(MoveColumn) -> engine.monitor_layout_action"));
    rows.push(
        a("move-column-to-monitor", "MonitorAction(MoveColumn(Name)) -> engine.monitor_layout_action")
            .fields("output: String")
            .kdl(&[r#"move-column-to-monitor "DP-1""#])
            .cli(&[&["DP-1"]])
            .ipc(&[r#"{"MoveColumnToMonitor":{"output":"DP-1"}}"#]),
    );
    for (name, executor) in [
        ("set-window-width", "SizeAction / SetColumnWidth -> change_size(Width) (explicit id: set_window_width_for)"),
        ("set-window-height", "SizeAction / SetWindowHeight -> change_size(Height) (explicit id: set_window_height_for)"),
    ] {
        let typed = pascal(name);
        let fixed = format!(r#"{{"{typed}":{{"change":{{"SetFixed":300}}}}}}"#);
        let by_id = format!(
            r#"{{"{typed}":{{"id":5,"change":{{"AdjustProportion":-10.0}}}}}}"#
        );
        let kdl_pct = format!(r#"{name} "+10%""#);
        let kdl_px = format!(r#"{name} "300""#);
        rows.push(
            a(name, executor)
                .fields("id: Option<u64> = None, change: SizeChange (SetFixed i32 | SetProportion f64 | AdjustFixed i32 | AdjustProportion f64)")
                .kdl(&[kdl_pct.as_str(), kdl_px.as_str()])
                .cli(&[&["+10%"], &["--id", "5", "-10%"]])
                .ipc(&[fixed.as_str(), by_id.as_str()]),
        );
    }
    rows.extend([
        with_id("reset-window-height", "ResetWindowHeight -> engine.reset_window_height"),
        unit("switch-preset-column-width", "ColumnWidthPresetCycle -> engine.switch_preset_column_width(true)"),
        unit("switch-preset-column-width-back", "ColumnWidthPresetCycleBack -> engine.switch_preset_column_width(false)"),
        with_id("switch-preset-window-width", "WindowWidthPresetCycle -> engine.switch_preset_window_width(true)"),
        with_id("switch-preset-window-width-back", "WindowWidthPresetCycleBack -> engine.switch_preset_window_width(false)"),
        with_id("switch-preset-window-height", "WindowHeightPresetCycle -> engine.switch_preset_window_height(true)")
            .kind(Kind::Partial)
            .note("native mixed-DPI and some lifecycle routes still need physical evidence (matrix A)"),
        with_id("switch-preset-window-height-back", "WindowHeightPresetCycleBack -> engine.switch_preset_window_height(false)")
            .kind(Kind::Partial)
            .note("same row as switch-preset-window-height (matrix A)"),
        unit("maximize-column", "MaximizeColumn -> engine.toggle_maximize_focused_column"),
        with_id("maximize-window-to-edges", "MaximizeWindowToEdges -> engine.toggle_maximize_window_to_edges_for"),
        a("set-column-width", "SetColumnWidth -> change_size(Width)")
            .fields("change: SizeChange")
            .kdl(&[r#"set-column-width "50%""#, r#"set-column-width "-100""#])
            .cli(&[&["50%"], &["-100"]])
            .ipc(&[
                r#"{"SetColumnWidth":{"change":{"SetProportion":50.0}}}"#,
                r#"{"SetColumnWidth":{"change":{"AdjustFixed":-100}}}"#,
            ]),
        unit("expand-column-to-available-width", "ExpandColumnToAvailable -> engine.expand_column_to_available"),
        a("switch-layout", "System(SwitchLayout) -> ipc::keyboard::switch_layout (WM_INPUTLANGCHANGEREQUEST)")
            .fields("layout: LayoutSwitchTarget (Next | Prev | Index u8)")
            .kdl(&[r#"switch-layout "next""#, r#"switch-layout "prev""#, r#"switch-layout "1""#])
            .cli(&[&["next"], &["prev"], &["1"]])
            .ipc(&[
                r#"{"SwitchLayout":{"layout":"Next"}}"#,
                r#"{"SwitchLayout":{"layout":"Prev"}}"#,
                r#"{"SwitchLayout":{"layout":{"Index":1}}}"#,
            ]),
        unit("show-hotkey-overlay", "ShowKeyBindings -> message_loop::show_hotkey_overlay"),
    ]);
    rows.extend(monitor_directions("move-workspace-to-monitor", "MonitorAction(MoveWorkspace) -> engine.monitor_layout_action"));
    rows.extend([
        a("move-workspace-to-monitor", "MonitorAction(MoveWorkspace(Name)) -> engine.monitor_layout_action")
            .fields("output: String, reference: Option<WorkspaceReferenceArg> = None (CLI --reference)")
            .kdl(&[r#"move-workspace-to-monitor "DP-1""#])
            .cli(&[&["DP-1"], &["DP-1", "--reference", "2"]])
            .ipc(&[
                r#"{"MoveWorkspaceToMonitor":{"output":"DP-1"}}"#,
                r#"{"MoveWorkspaceToMonitor":{"output":"DP-1","reference":{"Index":2}}}"#,
            ]),
        unit("toggle-debug-tint", "none")
            .na("compositor render debugging; there is no wiri renderer to tint"),
        unit("debug-toggle-opaque-regions", "none")
            .na("compositor render debugging (opaque-region overlay)"),
        unit("debug-toggle-damage", "none")
            .na("compositor render debugging (damage overlay)"),
        unit("toggle-touchpad", "System(ToggleTouchpad) -> system_actions::toggle_touchpad"),
        with_id("toggle-window-floating", "ToggleFloating -> engine.toggle_floating_for"),
        with_id("move-window-to-floating", "MoveWindowToFloating -> engine.set_window_floating(true)"),
        with_id("move-window-to-tiling", "MoveWindowToTiling -> engine.set_window_floating(false)"),
        unit("focus-floating", "FocusFloating -> engine.focus_layer_action(Floating)"),
        unit("focus-tiling", "FocusTiling -> engine.focus_layer_action(Tiling)"),
        unit("switch-focus-between-floating-and-tiling", "SwitchFocusBetweenFloatingAndTiling -> engine.focus_layer_action(Switch)"),
        a("move-floating-window", "IpcMessage::MoveFloatingWindow -> engine.move_floating_window")
            .fields("id: Option<u64> = None, x: PositionChange = AdjustFixed(0), y: PositionChange = AdjustFixed(0) (CLI -x/--x, -y/--y)")
            .kdl(&["move-floating-window"])
            .cli(&[&[], &["--id", "5", "-x", "+10", "-y", "50%"]])
            .ipc(&[
                r#"{"MoveFloatingWindow":{}}"#,
                r#"{"MoveFloatingWindow":{"id":5,"x":{"SetProportion":50.0},"y":{"AdjustFixed":-3.0}}}"#,
            ])
            .not_bindable()
            .kind(Kind::Partial)
            .note("physical mixed-DPI evidence pending (matrix A)"),
        with_id("toggle-window-rule-opacity", "ToggleWindowRuleOpacity -> engine.toggle_window_rule_opacity"),
        with_id("set-dynamic-cast-window", "none")
            .na("no PipeWire/portal cast targets on Windows; capture applications choose their own sources"),
        a("set-dynamic-cast-monitor", "none")
            .fields("output: Option<String> = None")
            .kdl(&[r#"set-dynamic-cast-monitor "DP-1""#])
            .cli(&[&[], &["DP-1"]])
            .ipc(&[
                r#"{"SetDynamicCastMonitor":{}}"#,
                r#"{"SetDynamicCastMonitor":{"output":"DP-1"}}"#,
            ])
            .na("no PipeWire/portal cast targets on Windows"),
        unit("clear-dynamic-cast-target", "none")
            .na("no PipeWire/portal cast targets on Windows"),
        a("stop-cast", "none")
            .fields("session_id: u64 (required; no KDL form)")
            .kdl(&["stop-cast"])
            .cli(&[&["--session-id", "5"]])
            .ipc(&[r#"{"StopCast":{"session_id":5}}"#])
            .not_bindable()
            .na("another application's capture session cannot be stopped from outside"),
        unit("toggle-overview", "OverviewToggle -> engine.toggle_overview"),
        unit("open-overview", "OverviewOpen -> engine.enter_overview"),
        unit("close-overview", "OverviewExit -> engine.exit_overview"),
    ]);
    for name in [
        "toggle-window-urgent",
        "set-window-urgent",
        "unset-window-urgent",
    ] {
        let typed = pascal(name);
        let by_id = format!(r#"{{"{typed}":{{"id":5}}}}"#);
        rows.push(
            a(name, "System(WindowUrgent) -> system_actions::execute (engine.set_window_urgent + FlashWindowEx)")
                .fields("id: u64 (required in niri; no KDL form)")
                .kdl(&[name])
                .cli(&[&["--id", "5"]])
                .ipc(&[by_id.as_str()])
                .not_bindable()
                .note("extension: wiri also accepts the action without an id (the focused window) and as a bind"),
        );
    }
    rows.push(
        a("load-config-file", "System(LoadConfigFile) -> system_actions::request_reload")
            .fields("path: Option<String> = None (CLI --path)")
            .kdl(&["load-config-file"])
            .cli(&[&[], &["--path", "C:/x.kdl"]])
            .ipc(&[
                r#"{"LoadConfigFile":{}}"#,
                r#"{"LoadConfigFile":{"path":"C:/x.kdl"}}"#,
            ])
            .not_bindable()
            .note("extension: wiri also accepts it as a bind"),
    );
    rows
}

// ---- the three entry points -------------------------------------------------------------

/// `wiri-ctl action <name> <words>` and typed `RunAction`, up to the effect boundary.
fn run_action_words(name: &str, args: &[String], quoted: &[bool]) -> Outcome {
    use crate::input::system_actions;
    if name == "move-floating-window" {
        return match parse_move_floating_window(args) {
            Ok(message) => Outcome::Accepted(format!("{message:?}")),
            Err(error) => Outcome::Rejected(error),
        };
    }
    if matches!(
        name,
        "screenshot-screen" | "screenshot-window" | "screenshot-focused"
    ) {
        return match crate::screenshot::action::CaptureAction::parse(
            name != "screenshot-screen",
            args,
        ) {
            Some(options) => Outcome::Accepted(format!("{options:?}")),
            None => Outcome::Rejected("invalid screenshot options".into()),
        };
    }
    if matches!(name, "screenshot" | "take-screenshot" | "capture-screen") {
        return match crate::overlay::screenshot_ui::Options::parse(args) {
            Some(options) => Outcome::Accepted(format!("{options:?}")),
            None => Outcome::Rejected("invalid screenshot UI options".into()),
        };
    }
    let (args, quoted, target) = match split_action_target(name, args, quoted) {
        Ok(split) => split,
        Err(error) => return Outcome::Rejected(error),
    };
    if let Some(parsed) = system_actions::parse(name, &args) {
        return match parsed {
            None => Outcome::Rejected("invalid system action arguments".into()),
            Some(system) => {
                if target.is_some() && !matches!(system, system_actions::SystemAction::WindowUrgent(_))
                {
                    Outcome::Rejected("this action does not accept --id".into())
                } else {
                    Outcome::Accepted(format!("{system:?} target={target:?}"))
                }
            }
        };
    }
    if name == "focus-window" {
        return if target.is_some() {
            Outcome::Accepted(format!("focus-window target={target:?}"))
        } else {
            Outcome::Rejected("focus-window needs --id".into())
        };
    }
    match parse_action_flagged(name, &args, &quoted) {
        Some(action) => Outcome::Accepted(format!("{action:?} target={target:?}")),
        None => Outcome::Rejected(format!("unknown action or bad arguments: {name} {args:?}")),
    }
}

fn cli(name: &str, words: &[String]) -> Outcome {
    // `wiri-ctl action`: niri's CLI spellings first, then one RunAction without quote marks
    // (niri's clap words never carry the quotes of a bind line).
    let words = super::niri_cli_words(name, words.to_vec());
    run_action_words(name, &words, &[])
}

fn typed(text: &str) -> Outcome {
    let action: Value = serde_json::from_str(text).expect("census JSON");
    let request = match wire::decode(json!({ "Action": action })) {
        Ok(request) => request,
        Err(error) => return Outcome::Rejected(error),
    };
    match request.message {
        IpcMessage::RunAction { name, args, quoted } => run_action_words(&name, &args, &quoted),
        IpcMessage::WorkspaceAction { action } => match action.into_input_action() {
            Ok(action) => Outcome::Accepted(format!("{action:?}")),
            Err(error) => Outcome::Rejected(error),
        },
        IpcMessage::SizeAction { action } => {
            let window = action.window_id();
            match action.into_input_action() {
                Ok(action) => Outcome::Accepted(format!("{action:?} window={window:?}")),
                Err(error) => Outcome::Rejected(error),
            }
        }
        other => Outcome::Accepted(format!("{other:?}")),
    }
}

fn kdl(action: &str) -> Outcome {
    let decoded = crate::config::decode::decode_text(
        &format!("binds {{ Mod+T {{ {action}; }} }}"),
        "",
        None,
    );
    if let Some(issue) = decoded.first_error() {
        return Outcome::Rejected(issue.to_string());
    }
    match decoded.config.binds.hotkeys.as_slice() {
        [bind] => match bind.parse_action() {
            Some(action) => Outcome::Accepted(format!("{action:?}")),
            None => Outcome::Rejected("the bind has no action".into()),
        },
        _ => Outcome::Rejected(format!(
            "bind dropped: {:?}",
            decoded
                .issues
                .iter()
                .map(|issue| issue.to_string())
                .collect::<Vec<_>>()
        )),
    }
}

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|w| w.to_string()).collect()
}

/// Measured behaviour of one entry point: every sample accepted, every sample rejected, or a
/// mix (a contract mismatch).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Measured {
    AllAccepted,
    AllRejected,
    Mixed,
    NoSample,
}

fn measure(outcomes: &[Outcome]) -> Measured {
    if outcomes.is_empty() {
        Measured::NoSample
    } else if outcomes.iter().all(Outcome::ok) {
        Measured::AllAccepted
    } else if outcomes.iter().all(|o| !o.ok()) {
        Measured::AllRejected
    } else {
        Measured::Mixed
    }
}

struct Evaluated {
    kdl: Vec<Outcome>,
    cli: Vec<Outcome>,
    ipc: Vec<Outcome>,
}

fn evaluate(row: &Row) -> Evaluated {
    Evaluated {
        kdl: row.kdl.iter().map(|text| kdl(text)).collect(),
        cli: row.cli.iter().map(|w| cli(row.name, w)).collect(),
        ipc: row.ipc.iter().map(|text| typed(text)).collect(),
    }
}

/// The cell of one entry point: `ok` (niri's forms all accepted), `n/a` (all refused, by
/// design), `ext` (not a niri form, but wiri takes it), `=` (not a niri form and wiri
/// refuses it too), `REJ` / `MIX` (a contract mismatch).
fn cell(niri_has: bool, kind: Kind, outcomes: &[Outcome]) -> &'static str {
    match (niri_has, measure(outcomes)) {
        (true, Measured::NoSample) => "NOSAMPLE",
        (true, Measured::AllAccepted) if kind != Kind::NotApplicable => "ok",
        (true, Measured::AllRejected) if kind == Kind::NotApplicable => "n/a",
        (true, Measured::AllRejected) => "REJ",
        (true, _) => "MIX",
        (false, Measured::AllAccepted) => "ext",
        (false, Measured::AllRejected) => "=",
        (false, Measured::NoSample) => "-",
        (false, Measured::Mixed) => "MIX",
    }
}

#[test]
fn the_census_covers_exactly_the_pinned_niri_vocabulary() {
    let rows = rows();
    let mut names: Vec<&str> = rows.iter().map(|row| row.name).collect();
    for (i, name) in names.iter().enumerate() {
        assert!(!names[..i].contains(name), "{name} appears twice");
    }
    names.sort_unstable();
    let mut expected: Vec<&str> = NIRI_IPC_ACTIONS.to_vec();
    for name in NIRI_KDL_ACTIONS {
        if !expected.contains(name) {
            expected.push(name);
        }
    }
    expected.sort_unstable();
    assert_eq!(names, expected, "the census must list every pinned action");
    assert_eq!(rows.len(), 143, "142 IPC variants plus the KDL-only suspend");
    assert_eq!(NIRI_KDL_ACTIONS.len(), 136);
    assert_eq!(NIRI_IPC_ACTIONS.len(), 142);
    // The decoder's own vocabulary list is niri's bindable set, in niri's order.
    assert_eq!(
        crate::config::decode::NIRI_ACTIONS,
        NIRI_KDL_ACTIONS,
        "NIRI_ACTIONS drifted from niri:niri-config/src/binds.rs"
    );
    for row in &rows {
        assert_eq!(
            row.niri_kdl,
            NIRI_KDL_ACTIONS.contains(&row.name),
            "{}: KDL bindability",
            row.name
        );
        assert_eq!(
            row.niri_ipc,
            NIRI_IPC_ACTIONS.contains(&row.name),
            "{}: typed IPC variant",
            row.name
        );
    }
}

#[test]
fn every_pinned_action_behaves_as_the_census_says() {
    let mut failures = Vec::new();
    for row in rows() {
        let measured = evaluate(&row);
        for (entry, niri_has, outcomes) in [
            ("KDL", row.niri_kdl, &measured.kdl),
            ("CLI", row.niri_ipc, &measured.cli),
            ("typed IPC", row.niri_ipc, &measured.ipc),
        ] {
            let verdict = cell(niri_has, row.kind, outcomes);
            let expected: &[&str] = if niri_has {
                if row.kind == Kind::NotApplicable {
                    &["n/a"]
                } else {
                    &["ok"]
                }
            } else {
                // Not a niri form: wiri may take it as an extension or refuse it like niri.
                &["ext", "=", "-"]
            };
            if !expected.contains(&verdict) {
                let detail: Vec<String> = outcomes.iter().map(|o| format!("{o:?}")).collect();
                failures.push(format!(
                    "{} {entry}: {verdict} (wanted {expected:?}): {}",
                    row.name,
                    detail.join(" | ")
                ));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// Prints the markdown table of `docs/parity/action-census.md`.
#[test]
#[ignore = "prints the census table; run with --ignored --nocapture"]
fn print_census_markdown() {
    let escape = |text: &str| text.replace('|', "\\|");
    println!("| # | niri action | niri typed fields (defaults) | KDL | CLI | typed IPC | status | wiri executor | notes |");
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    for (i, row) in rows().iter().enumerate() {
        let measured = evaluate(row);
        let status = match row.kind {
            Kind::Full => "full",
            Kind::Partial => "partial",
            Kind::NotApplicable => "N/A",
        };
        println!(
            "| {} | `{}` | {} | {} | {} | {} | {} | {} | {} |",
            i + 1,
            row.name,
            escape(&row.fields),
            cell(row.niri_kdl, row.kind, &measured.kdl),
            cell(row.niri_ipc, row.kind, &measured.cli),
            cell(row.niri_ipc, row.kind, &measured.ipc),
            status,
            escape(row.executor),
            escape(row.note),
        );
    }
}

// ---- fixes this census drove --------------------------------------------------------------

#[test]
fn omitted_typed_fields_take_niris_serde_defaults() {
    // niri:niri-ipc/src/lib.rs:196-201,226-261,497-512,848-868: absent bools and positions
    // default to true / true / false / AdjustFixed(0), absent options to None.
    let same = |omitted: &str, explicit: &str| {
        let (a, b) = (typed(omitted), typed(explicit));
        assert!(a.ok(), "{omitted}: {a:?}");
        assert_eq!(a, b, "{omitted} must equal {explicit}");
    };
    same(r#"{"Quit":{}}"#, r#"{"Quit":{"skip_confirmation":false}}"#);
    same(r#"{"Screenshot":{}}"#, r#"{"Screenshot":{"show_pointer":true}}"#);
    same(
        r#"{"ScreenshotScreen":{}}"#,
        r#"{"ScreenshotScreen":{"write_to_disk":true,"show_pointer":true}}"#,
    );
    same(
        r#"{"ScreenshotWindow":{}}"#,
        r#"{"ScreenshotWindow":{"write_to_disk":true,"show_pointer":false}}"#,
    );
    same(
        r#"{"MoveWindowToWorkspaceDown":{}}"#,
        r#"{"MoveWindowToWorkspaceDown":{"focus":true}}"#,
    );
    same(
        r#"{"MoveWindowToWorkspace":{"reference":{"Index":2}}}"#,
        r#"{"MoveWindowToWorkspace":{"window_id":null,"reference":{"Index":2},"focus":true}}"#,
    );
    same(
        r#"{"MoveColumnToWorkspace":{"reference":{"Index":2}}}"#,
        r#"{"MoveColumnToWorkspace":{"reference":{"Index":2},"focus":true}}"#,
    );
    same(
        r#"{"MoveFloatingWindow":{}}"#,
        r#"{"MoveFloatingWindow":{"id":null,"x":{"AdjustFixed":0.0},"y":{"AdjustFixed":0.0}}}"#,
    );
    same(
        r#"{"MoveFloatingWindow":{"x":{"SetFixed":5.0}}}"#,
        r#"{"MoveFloatingWindow":{"x":{"SetFixed":5.0},"y":{"AdjustFixed":0.0}}}"#,
    );
    same(
        r#"{"LoadConfigFile":{}}"#,
        r#"{"LoadConfigFile":{"path":null}}"#,
    );
}

#[test]
fn required_typed_fields_are_required() {
    // Each of these fails serde in niri (no default): a missing `index`, `reference`,
    // `output`, `command`, `change`, `layout`, `display`, `name`, or `id`.
    for text in [
        r#"{"FocusColumn":{}}"#,
        r#"{"FocusWindowInColumn":{}}"#,
        r#"{"MoveColumnToIndex":{}}"#,
        r#"{"FocusWorkspace":{}}"#,
        r#"{"MoveWindowToWorkspace":{}}"#,
        r#"{"MoveColumnToWorkspace":{}}"#,
        r#"{"MoveWorkspaceToIndex":{}}"#,
        r#"{"SetWorkspaceName":{}}"#,
        r#"{"FocusMonitor":{}}"#,
        r#"{"MoveWindowToMonitor":{}}"#,
        r#"{"MoveColumnToMonitor":{}}"#,
        r#"{"MoveWorkspaceToMonitor":{}}"#,
        r#"{"SetWindowWidth":{}}"#,
        r#"{"SetWindowHeight":{}}"#,
        r#"{"SetColumnWidth":{}}"#,
        r#"{"SwitchLayout":{}}"#,
        r#"{"SetColumnDisplay":{}}"#,
        r#"{"Spawn":{}}"#,
        r#"{"SpawnSh":{}}"#,
        r#"{"FocusWindow":{}}"#,
    ] {
        assert!(!typed(text).ok(), "{text} must be refused: {:?}", typed(text));
    }
}

#[test]
fn spawn_words_are_never_taken_for_a_window_target() {
    // niri:niri-ipc/src/lib.rs:207-212: `spawn` takes the rest of the line as its command, so
    // `--id 5` belongs to the program.  It used to be stripped and made wiri focus window 5.
    for name in ["spawn", "spawn-sh", "exec", "spawn-cmd"] {
        let args = words(&["tool", "--id", "5"]);
        let (rest, _, target) = split_action_target(name, &args, &[]).unwrap();
        assert_eq!(rest, args, "{name}");
        assert_eq!(target, None, "{name}");
    }
    let accepted = cli("spawn", &words(&["tool", "--id", "5"]));
    assert!(
        matches!(&accepted, Outcome::Accepted(text) if text.contains("\"--id\"") && text.contains("target=None")),
        "{accepted:?}"
    );
    // Every other action still reads niri's `--id` and `--window-id`.
    let (rest, _, target) = split_action_target("close-window", &words(&["--id", "5"]), &[]).unwrap();
    assert!(rest.is_empty());
    assert_eq!(target, Some(5));
}

#[test]
fn niri_cli_spellings_of_workspace_references_are_accepted() {
    // niri:niri-ipc/src/lib.rs:579-586 (`--reference`), 595-602 (`--workspace`), 611-618
    // (`unset-workspace-name [REFERENCE]` is positional), 805-812 (`--reference`).
    for (name, args) in [
        ("move-workspace-to-index", vec!["2", "--reference", "web"]),
        ("move-workspace-to-index", vec!["2", "--workspace", "web"]),
        ("move-workspace-to-monitor", vec!["DP-1", "--reference", "2"]),
        ("move-workspace-to-monitor-left", vec!["--reference", "2"]),
        ("set-workspace-name", vec!["web", "--workspace", "2"]),
        ("unset-workspace-name", vec!["web"]),
        ("unset-workspace-name", vec!["2"]),
    ] {
        let outcome = cli(name, &words(&args));
        assert!(outcome.ok(), "{name} {args:?}: {outcome:?}");
    }
    // `--reference` is not a spelling of `set-workspace-name`, and two references are one too many.
    for (name, args) in [
        ("set-workspace-name", vec!["web", "--reference", "2"]),
        (
            "move-workspace-to-index",
            vec!["2", "--reference", "web", "--workspace", "3"],
        ),
        ("unset-workspace-name", vec!["web", "extra"]),
    ] {
        let outcome = cli(name, &words(&args));
        assert!(!outcome.ok(), "{name} {args:?}: {outcome:?}");
    }
    // The positional of `unset-workspace-name` keeps meaning: a number is an index.
    assert_eq!(
        cli("unset-workspace-name", &words(&["2"])),
        cli("unset-workspace-name", &words(&["--workspace", "2"]))
    );
    // niri's KDL `unset-workspace-name` is a unit variant: no argument in a bind.
    assert!(!kdl(r#"unset-workspace-name "web""#).ok());
}

#[test]
fn quit_takes_niris_short_flag() {
    // niri:niri-ipc/src/lib.rs:196-201: `#[arg(short, long)] skip_confirmation`.
    assert_eq!(
        cli("quit", &words(&["-s"])),
        cli("quit", &words(&["--skip-confirmation"]))
    );
    assert_ne!(cli("quit", &words(&["-s"])), cli("quit", &[]));
}

#[test]
fn unit_actions_refuse_surplus_words_like_niris_kdl() {
    // niri:niri-config/src/binds.rs: a unit variant takes no argument and no property.
    for name in [
        "close-window",
        "fullscreen-window",
        "toggle-window-floating",
        "center-column",
        "center-window",
        "move-column-left",
        "move-column-to-first",
        "move-window-up",
        "toggle-overview",
        "maximize-column",
        "maximize-window-to-edges",
        "reset-window-height",
        "switch-preset-column-width",
        "consume-window-into-column",
        "expand-column-to-available-width",
        "show-hotkey-overlay",
    ] {
        assert!(!cli(name, &words(&["surplus"])).ok(), "{name} surplus");
        assert!(!kdl(&format!(r#"{name} "surplus""#)).ok(), "{name} in a bind");
        assert!(kdl(name).ok(), "{name} alone");
    }
}

#[test]
fn index_arguments_have_niris_integer_types() {
    // niri:niri-config/src/binds.rs:163 (`u8`), 175, 198, 248 (`usize`), and lib.rs:1791-1805:
    // a workspace index is a `u8`; 300 and -1 are errors, not a workspace called "300".
    for args in [vec!["300"], vec!["-1"], vec!["256"]] {
        assert!(!cli("focus-workspace", &words(&args)).ok(), "{args:?}");
        assert!(!cli("move-window-to-workspace", &words(&args)).ok(), "{args:?}");
        assert!(!cli("move-column-to-workspace", &words(&args)).ok(), "{args:?}");
    }
    assert!(cli("focus-workspace", &words(&["255"])).ok());
    assert!(cli("focus-workspace", &words(&["0"])).ok());
    // Text that is not an integer is a name.
    assert!(cli("focus-workspace", &words(&["web"])).ok());
    // `focus-window-in-column` is a `u8`, the others are `usize` and never overflow.
    assert!(!cli("focus-window-in-column", &words(&["256"])).ok());
    assert!(cli("focus-window-in-column", &words(&["255"])).ok());
    for name in ["focus-column", "move-column-to-index", "move-workspace-to-index"] {
        assert!(cli(name, &words(&["99999999999"])).ok(), "{name} clamps");
        assert!(!cli(name, &words(&["-1"])).ok(), "{name} negative");
    }
}
