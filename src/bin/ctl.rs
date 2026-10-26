//! wiri-ctl — command-line client for the wiri window manager.
//!
//! Sends IpcMessage requests over the named pipe of this user and this session
//! (`\\.\pipe\wiri_control_<user>_<session>`; `WIRI_PIPE` overrides the name)
//! and pretty-prints the response.  Pass `--json` to any subcommand for the
//! raw JSON body (useful for scripting).

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::io::{Read, Write};

use wiri::ipc::{IpcClient, IpcMessage};

// ---------------------------------------------------------------------------
// Console-safe output
//
// The tables use box-drawing characters and `…` / `→` / `—`.  When stdout is a
// real console Rust writes UTF-16 and they render fine.  When stdout is a pipe
// or a file (PowerShell decodes native-command output with the console's *OEM*
// code page, usually 437) the same UTF-8 bytes come out as `ΓöÇ`.  So output
// that is not going to a console is folded to plain ASCII; set
// `WIRI_UNICODE=1` to keep Unicode anyway, `WIRI_ASCII=1` to force ASCII.
// `--json` output is untouched.
// ---------------------------------------------------------------------------

/// Replace the decorative non-ASCII characters of the human-readable output
/// with ASCII look-alikes, and any other non-ASCII character with `?`.
fn fold_to_ascii(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            c if c.is_ascii() => out.push(c),
            '─' | '━' | '═' | '—' | '–' | '‒' => out.push('-'),
            '│' | '┃' | '║' | '¦' => out.push('|'),
            '┌' | '┐' | '└' | '┘' | '├' | '┤' | '┬' | '┴' | '┼' | '╔' | '╗' | '╚' | '╝' => {
                out.push('+')
            }
            '…' => out.push_str("..."),
            '→' => out.push_str("->"),
            '←' => out.push_str("<-"),
            '↑' => out.push('^'),
            '↓' => out.push('v'),
            '•' | '·' => out.push('*'),
            '✓' => out.push_str("ok"),
            '‘' | '’' => out.push('\''),
            '“' | '”' => out.push('"'),
            _ => out.push('?'),
        }
    }
    out
}

/// Is the given standard stream attached to a console (as opposed to a pipe or
/// a file)?
fn is_console(std_handle: windows::Win32::System::Console::STD_HANDLE) -> bool {
    use windows::Win32::System::Console::{GetConsoleMode, GetStdHandle, CONSOLE_MODE};
    unsafe {
        match GetStdHandle(std_handle) {
            Ok(handle) => {
                let mut mode = CONSOLE_MODE(0);
                GetConsoleMode(handle, &mut mode).is_ok()
            }
            Err(_) => false,
        }
    }
}

/// Whether human-readable output must be folded to ASCII for this stream.
fn ascii_only(to_stderr: bool) -> bool {
    use std::sync::OnceLock;
    use windows::Win32::System::Console::{STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
    static FORCED: OnceLock<Option<bool>> = OnceLock::new();
    let forced = FORCED.get_or_init(|| {
        let on = |name: &str| {
            std::env::var(name)
                .map(|v| !v.is_empty() && v != "0")
                .unwrap_or(false)
        };
        if on("WIRI_UNICODE") {
            Some(false)
        } else if on("WIRI_ASCII") {
            Some(true)
        } else {
            None
        }
    });
    match forced {
        Some(v) => *v,
        None => !is_console(if to_stderr {
            STD_ERROR_HANDLE
        } else {
            STD_OUTPUT_HANDLE
        }),
    }
}

fn print_line(to_stderr: bool, text: String) {
    let text = if ascii_only(to_stderr) {
        fold_to_ascii(&text)
    } else {
        text
    };
    if to_stderr {
        eprintln!("{}", text);
    } else {
        println!("{}", text);
    }
}

macro_rules! outln {
    () => {
        println!()
    };
    ($($arg:tt)*) => {
        print_line(false, format!($($arg)*))
    };
}

macro_rules! errln {
    () => {
        eprintln!()
    };
    ($($arg:tt)*) => {
        print_line(true, format!($($arg)*))
    };
}

/// Build version string. The optional build script (`build.rs`) sets
/// `WIRI_GIT_HASH` to the short commit hash when available; otherwise we just
/// render the crate version from Cargo.toml.
const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const GIT_HASH: Option<&str> = option_env!("WIRI_GIT_HASH");

/// Compute the displayed version string once at startup and intern it for the
/// lifetime of the process (clap wants a `&'static str`).
fn version_static() -> &'static str {
    use std::sync::OnceLock;
    static V: OnceLock<String> = OnceLock::new();
    V.get_or_init(|| match GIT_HASH {
        Some(h) if !h.is_empty() => format!("{} (commit {})", PKG_VERSION, h),
        _ => PKG_VERSION.to_string(),
    })
    .as_str()
}

#[derive(Parser, Debug)]
#[command(
    name = "wiri-ctl",
    version = version_static(),
    about = "Control wiri window manager via IPC",
    long_about = "Send commands to a running wiri window manager over its named-pipe IPC.\n\
                  Output is human-formatted by default; pass --json for the raw JSON body."
)]
struct Cli {
    #[command(subcommand)]
    command: Invocation,

    /// Timeout in seconds for IPC connection
    #[arg(short, long, default_value = "5", global = true)]
    timeout: u64,

    /// Emit the raw JSON response instead of a formatted summary.
    #[arg(long, global = true)]
    json: bool,

    /// Print the request JSON and exit without connecting to the daemon.
    #[arg(long, global = true)]
    print_request: bool,
}

#[derive(Subcommand, Debug)]
enum Invocation {
    #[command(flatten)]
    Direct(Commands),

    /// niri-compatible prefix: `wiri-ctl msg windows` is `wiri-ctl windows`.
    Msg {
        #[command(subcommand)]
        command: Commands,
    },
}

impl Invocation {
    fn into_command(self) -> Commands {
        match self {
            Self::Direct(command) | Self::Msg { command } => command,
        }
    }
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Get the current window manager state
    State,

    /// List the windows wiri manages, in niri's shape: the id is the window
    /// handle and the fields are the ones niri's `Windows` request answers.
    Windows,

    /// List the managed windows in wiri's older shape: the handle, the class,
    /// the program, the geometry and the place in the strip.
    WindowList,

    /// List the workspaces of every monitor, in niri's shape: the number on
    /// the monitor, the name, the monitor and the flags.
    Workspaces,

    /// The window that has the keyboard focus, in niri's shape.
    FocusedWindow,

    /// List connected outputs, in niri's shape.
    Outputs,

    /// The output that currently has focus, or null.
    FocusedOutput,

    /// List keyboard layout names and the current layout index.
    KeyboardLayouts,

    /// Pick a window interactively.
    PickWindow,

    /// Pick a color on screen.
    PickColor,

    /// Configure an output: NAME ACTION [ARGS], or NAME '<external enum JSON>'.
    /// Actions: on, off, mode, scale, transform, position, vrr, max-bpc.
    Output {
        name: String,
        #[arg(required = true, num_args = 1.., allow_negative_numbers = true)]
        action: Vec<String>,
    },

    /// Ask the daemon to return a diagnostic error.
    #[command(alias = "return-error")]
    RequestError,

    /// Send JSON from an argument, or stdin when omitted or '-'; print one raw reply line.
    RawRequest {
        #[arg(value_name = "JSON")]
        request: Option<String>,
    },

    /// Whether the overview is open.
    OverviewState,

    /// The version of the running wiri daemon (the daemon's, not this CLI's:
    /// `wiri-ctl --version` is the CLI's).
    Version,

    /// Switch to a workspace by ID (1-9)
    SwitchWorkspace { id: i32 },

    /// Focus a specific window by HWND
    FocusWindow { hwnd: isize },

    /// Close a specific window by HWND
    CloseWindow { hwnd: isize },

    /// Move a window to a new position
    MoveWindow { hwnd: isize, x: i32, y: i32 },

    /// Resize a window
    ResizeWindow {
        hwnd: isize,
        width: u32,
        height: u32,
    },

    /// Toggle floating state for a specific HWND
    FloatWindow { hwnd: isize },

    /// Force a window back into the tiling layout
    UnfloatWindow { hwnd: isize },

    /// Send an explicit tile request (forces the window into the tiling layout)
    TileWindow {
        hwnd: isize,
        /// Optional target workspace name
        #[arg(short, long)]
        workspace: Option<String>,
    },

    /// Move a specific window by HWND to a workspace (by id)
    MoveWindowToWorkspaceByHwnd { hwnd: isize, workspace_id: i32 },

    /// Reload the configuration file
    ReloadConfig,

    /// Request wiri to quit
    Quit,

    /// Subscribe to events (stream mode).
    ///
    /// By default prints a human-readable summary line per event.
    /// Pass --stream to emit raw newline-delimited JSON (pipe-safe, jq-friendly).
    /// Pass --filter to receive only certain event types.
    #[command(alias = "events")]
    EventStream {
        /// Event types to subscribe to (comma-separated, legacy alias for --filter).
        /// Kept for backwards compatibility; prefer --filter.
        /// The short -t is reserved for the global connection timeout.
        #[arg(long, default_value = "*", hide = true)]
        types: String,

        /// Emit raw newline-delimited JSON instead of the human-readable summary.
        /// Each line is one complete JSON object. Third-party scripts should use
        /// this flag so the output is stable and pipe-safe.
        #[arg(long)]
        stream: bool,

        /// Comma-separated list of event types to subscribe to.
        /// When provided, only the listed types are delivered by the daemon.
        /// Overrides --types when both are supplied.
        /// Example: --filter workspace_switched,window_focused
        #[arg(long, value_name = "TYPES")]
        filter: Option<String>,
    },

    /// Move the focused window to the given workspace
    MoveWindowToWorkspace { workspace_id: i32 },

    /// Move the focused window to the next monitor (direction: left | right)
    MoveWindowToMonitor { direction: String },

    /// Center the focused column on screen
    CenterColumn,

    /// Set the width of the focused column: niri's size (50%, +10%, -10%, 1200, +100)
    /// or 1/3, 1/2, 2/3, full, cycle
    SetColumnWidth {
        #[arg(allow_hyphen_values = true)]
        preset: String,
    },

    /// Resize the focused column by ±N pixels
    ResizeColumn { delta_px: i32 },

    /// Focus the workspace below (no wrapping)
    FocusWorkspaceNext,

    /// Focus the workspace above (no wrapping)
    FocusWorkspacePrevious,

    /// Run any action by its niri name, as if its key were pressed:
    /// `wiri-ctl action focus-column-left`, `wiri-ctl action set-column-width +10%`,
    /// `wiri-ctl action focus-workspace 2`.
    Action {
        /// Action name, e.g. close-window, consume-or-expel-window-left
        name: String,
        /// The words that follow the action name in a `binds {}` line
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Spawn a program: `wiri-ctl spawn wt.exe -d "C:\My Projects"`. The
    /// arguments are passed through intact; with none, the program may also be
    /// a whole quoted command line (`wiri-ctl spawn 'wt.exe -d "C:\x y"'`).
    Spawn {
        /// Program to run (searched on PATH when it has no directory part)
        command: String,
        /// Arguments of the program
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Focus the previously focused window (alt-tab through MRU history)
    FocusPrevious,

    /// Toggle always-on-top for the focused window
    ToggleAlwaysOnTop,

    /// Focus a named workspace from config (e.g. `wiri-ctl focus-workspace-named main`)
    FocusWorkspaceNamed { name: String },

    /// Set auto-tile threshold (window count above which to auto-grid). 0 to disable.
    SetAutoTile { threshold: usize },

    /// List every monitor the running wiri daemon has detected, with its
    /// bounds, work area, DPI scale, active workspace, and tiled-window count.
    /// Useful for diagnosing multi-monitor / HiDPI issues.
    ListMonitors,

    /// Validate a config file without sending it to the running daemon.
    ///
    /// Reads the file at `path` (or wiri's default config search order when
    /// omitted), runs the same lenient parser the daemon uses, then prints any
    /// structural errors or warnings (unknown sections, invalid colours,
    /// unbalanced braces, etc.) one per line.  Exits with status 1 when at
    /// least one error is found, 0 otherwise.  Does not require a running
    /// daemon — useful for editor-side checks before reload.
    ValidateConfig {
        /// Path to the config file. If omitted, uses the same search order as
        /// the daemon: $WIRI_CONFIG, %APPDATA%\wiri\config.kdl, then
        /// %USERPROFILE%\.config\wiri\config.kdl.
        path: Option<String>,
    },

    /// niri-parity: take the focused tile out of its column and append it
    /// to the column on its right. No-op when already rightmost.
    ConsumeWindow,

    /// niri-parity: take the focused tile out of its column and place it
    /// in a brand-new column to the right of its source.
    ExpelWindow,

    /// niri-parity: expand the focused column to fill the leftover
    /// horizontal space in the work area.
    ExpandColumn,

    /// niri-parity: toggle the focused column's maximize flag (full
    /// work-area height). Distinct from window fullscreen.
    MaximizeColumn,

    /// niri-parity: grow the focused column width by 5% of the work area.
    GrowColumn,

    /// niri-parity: shrink the focused column width by 5% of the work area.
    ShrinkColumn,

    /// niri-parity: grow the focused tile height by 5% of its column.
    GrowTile,

    /// niri-parity: shrink the focused tile height by 5% of its column.
    ShrinkTile,

    /// niri-parity: move the focused column wholesale to the monitor in
    /// the given direction. `direction` must be `left` or `right`.
    MoveColumnToMonitor { direction: String },

    /// Diagnostic: print every hotkey binding that the running wiri daemon
    /// successfully registered with Windows.  Useful when a chord you set
    /// in `config.kdl` isn't firing — bindings that `RegisterHotKey`
    /// rejected (typically because another tool already owns the chord)
    /// are silently dropped at startup; this command tells you which ones
    /// survived.  Output columns: modifiers, virtual-key, action name.
    TestBindings,

    /// Capture a single window by HWND to a BMP file.  Uses the modern
    /// `PrintWindow(hwnd, hdc, PW_RENDERFULLCONTENT)` flag so UWP /
    /// DirectComposition windows are captured correctly.  When `--path`
    /// is omitted the file lands under
    /// `%USERPROFILE%\Pictures\wiri-window-<hwnd>-<unix>.bmp` (falling
    /// back to the current directory if Pictures is unavailable).
    CaptureWindow {
        hwnd: isize,
        /// Output path for the BMP file. Defaults to the Pictures folder.
        #[arg(short, long)]
        path: Option<String>,
    },

    /// Save the current monitor/workspace/column/tile layout to a JSON
    /// snapshot file under `%APPDATA%\wiri\snapshots\<name>.json`.
    SaveSnapshot { name: String },

    /// Restore a previously-saved snapshot.  Windows whose HWNDs no
    /// longer exist are skipped silently.
    LoadSnapshot { name: String },

    /// List every saved snapshot under `%APPDATA%\wiri\snapshots\`.
    ListSnapshots,

    /// Delete a saved snapshot by name.
    DeleteSnapshot { name: String },

    /// Toggle niri-style interactive resize mode.  While engaged, the
    /// `Mod+Arrow` chords resize the focused column / tile by 5% and
    /// `Esc` (or running this command again) exits the mode.
    ResizeMode,

    /// niri parity: move the focused column to the workspace immediately
    /// above the current one.  Creates the destination workspace if needed.
    MoveColumnUp,
    /// niri parity: move the focused column to the workspace immediately
    /// below the current one.
    MoveColumnDown,
    /// niri parity: swap the focused workspace with the one above on the
    /// same monitor.  Focus follows the moved workspace.
    MoveWorkspaceUp,
    /// niri parity: swap the focused workspace with the one below on the
    /// same monitor.
    MoveWorkspaceDown,

    // ---- Round-4 ----
    /// Toggle sticky (visible on all workspaces) for the focused window.
    ToggleSticky,

    /// Set the active workspace's layout (scrolling | bstack | spiral).
    SetWorkspaceLayout { mode: String },

    /// Rename a workspace by its numeric id.
    RenameWorkspace { workspace_id: i32, name: String },

    /// Screenshot a window (or the focused window when --hwnd is omitted).
    ScreenshotWindow {
        /// HWND of the window to capture. Defaults to the focused window.
        #[arg(long)]
        hwnd: Option<i64>,
        /// Output path for the BMP file. Defaults to the Pictures folder.
        #[arg(long)]
        path: Option<String>,
    },

    /// Get the currently focused window, workspace, and monitor.
    GetFocus,

    /// List all workspaces with their names and window counts.
    GetWorkspaceList,

    /// List all live key-bindings as a human-readable chord table.
    /// Shows what the running daemon actually registered with Windows
    /// (after `RegisterHotKey`), including any custom binds from config.
    /// Pass --json for the raw JSON array.
    Bindings,

    /// Reserve screen-edge pixels for external apps (e.g. status bars).
    ///
    /// Instructs wiri to subtract the given pixel count from each monitor's
    /// tiling work area on the specified edge.  The reservation is in-memory
    /// only — it is reset when wiri restarts.  External bars should re-issue
    /// this command after reconnecting.  Pass `--pixels 0` to release a
    /// previous reservation.
    ReserveArea {
        /// Edge to reserve: top | bottom | left | right
        side: String,
        /// Number of pixels to reserve on that edge (0 = release)
        pixels: u32,
        /// Optional monitor ID (numeric). Currently ignored (global reservation).
        #[arg(long)]
        monitor_id: Option<u64>,
    },
}

fn main() {
    if let Err(error) = real_main() {
        errln!("Error: {:#}", error);
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    let command = cli.command.into_command();

    // Local validation has no IPC request to print, and must never dial a pipe.
    if let Commands::ValidateConfig { path } = &command {
        anyhow::ensure!(
            !cli.print_request,
            "validate-config is local and has no IPC request"
        );
        return validate_config_subcommand(path.as_deref(), cli.json);
    }

    if let Commands::RawRequest { request } = &command {
        let request = raw_request_text(request.as_deref(), &mut std::io::stdin().lock())?;
        if cli.print_request {
            println!("{}", request);
        } else {
            let reply = block_on(async {
                let mut client = connect_ipc(cli.timeout).await?;
                Ok(client.request_raw(&request).await?)
            })?;
            // Intentionally no schema warning or JSON wrapper: niri clients
            // must see the original reply line, regardless of --json.
            println!("{}", reply);
        }
        return Ok(());
    }

    let event_output = match &command {
        Commands::EventStream { stream, .. } => Some(*stream || cli.json),
        _ => None,
    };
    let message = request_for(command)?;
    if cli.print_request {
        // The client compacts through Value as well, so this is the actual
        // request line sent, without the framing newline supplied by println!.
        println!("{}", serde_json::to_value(&message)?);
        return Ok(());
    }
    if let Some(raw) = event_output {
        return stream_events(&message, cli.timeout, raw);
    }

    let response = send_ipc_message(&message, cli.timeout)?;
    warn_schema(&response);
    if cli.json {
        // Keep the existing wiri success/result wrapper, even for requests
        // that also have an unwrapped niri representation.
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }

    print_human_response(&message, &response);
    Ok(())
}

/// The request a subcommand sends to the daemon.
fn request_for(command: Commands) -> Result<IpcMessage> {
    Ok(match command {
        Commands::State => IpcMessage::GetState,
        Commands::Windows => IpcMessage::Windows,
        Commands::WindowList => IpcMessage::WindowList,
        Commands::Workspaces => IpcMessage::Workspaces,
        Commands::FocusedWindow => IpcMessage::FocusedWindow,
        Commands::Outputs => IpcMessage::Outputs,
        Commands::FocusedOutput => IpcMessage::FocusedOutput,
        Commands::KeyboardLayouts => IpcMessage::KeyboardLayouts,
        Commands::PickWindow => IpcMessage::PickWindow,
        Commands::PickColor => IpcMessage::PickColor,
        Commands::Output { name, action } => IpcMessage::Output {
            output: name,
            action: output_action(&action)?,
        },
        Commands::RequestError => IpcMessage::ReturnError,
        Commands::RawRequest { .. } => unreachable!("raw requests handled before IPC dial"),
        Commands::OverviewState => IpcMessage::OverviewState,
        Commands::Version => IpcMessage::Version,
        Commands::SwitchWorkspace { id } => IpcMessage::SwitchWorkspace { id },
        Commands::FocusWindow { hwnd } => IpcMessage::FocusWindow { window_hwnd: hwnd },
        Commands::CloseWindow { hwnd } => IpcMessage::CloseWindow { window_hwnd: hwnd },
        Commands::MoveWindow { hwnd, x, y } => IpcMessage::WindowMove {
            window_hwnd: hwnd,
            x,
            y,
        },
        Commands::ResizeWindow {
            hwnd,
            width,
            height,
        } => IpcMessage::WindowResize {
            window_hwnd: hwnd,
            width,
            height,
        },
        Commands::FloatWindow { hwnd } => IpcMessage::WindowFloat { window_hwnd: hwnd },
        Commands::UnfloatWindow { hwnd } => IpcMessage::WindowUnfloat { window_hwnd: hwnd },
        Commands::TileWindow { hwnd, workspace } => IpcMessage::TileRequest {
            window_hwnd: hwnd,
            target_workspace: workspace,
        },
        Commands::MoveWindowToWorkspaceByHwnd { hwnd, workspace_id } => {
            IpcMessage::WindowMoveToWorkspace {
                window_hwnd: hwnd,
                workspace_id,
            }
        }
        Commands::ReloadConfig => IpcMessage::ReloadConfig,
        Commands::Quit => IpcMessage::Quit,
        Commands::EventStream { types, filter, .. } => IpcMessage::SubscribeEvents {
            event_types: event_types(filter.as_deref().unwrap_or(&types)),
        },
        Commands::MoveWindowToWorkspace { workspace_id } => {
            IpcMessage::MoveWindowToWorkspace { workspace_id }
        }
        Commands::MoveWindowToMonitor { direction } => {
            IpcMessage::MoveWindowToMonitor { direction }
        }
        Commands::CenterColumn => IpcMessage::CenterColumn,
        Commands::SetColumnWidth { preset } => IpcMessage::SetColumnWidth { preset },
        Commands::ResizeColumn { delta_px } => IpcMessage::ResizeColumn { delta_px },
        Commands::FocusWorkspaceNext => IpcMessage::FocusWorkspaceNext,
        Commands::FocusWorkspacePrevious => IpcMessage::FocusWorkspacePrevious,
        Commands::Spawn { command, args } => IpcMessage::SpawnCommand { command, args },
        Commands::Action { name, args } => {
            let (args, quoted) = split_quoted_words(args);
            IpcMessage::RunAction { name, args, quoted }
        }
        Commands::FocusPrevious => IpcMessage::FocusPrevious,
        Commands::ToggleAlwaysOnTop => IpcMessage::ToggleAlwaysOnTop,
        Commands::FocusWorkspaceNamed { name } => IpcMessage::FocusWorkspaceNamed { name },
        Commands::SetAutoTile { threshold } => IpcMessage::SetAutoTileThreshold {
            threshold: if threshold == 0 {
                None
            } else {
                Some(threshold)
            },
        },
        Commands::ListMonitors => IpcMessage::MonitorList,
        Commands::ValidateConfig { .. } => unreachable!("handled above"),
        Commands::ConsumeWindow => IpcMessage::ConsumeWindow,
        Commands::ExpelWindow => IpcMessage::ExpelWindow,
        Commands::ExpandColumn => IpcMessage::ExpandColumn,
        Commands::MaximizeColumn => IpcMessage::MaximizeColumn,
        Commands::GrowColumn => IpcMessage::GrowColumn,
        Commands::ShrinkColumn => IpcMessage::ShrinkColumn,
        Commands::GrowTile => IpcMessage::GrowTile,
        Commands::ShrinkTile => IpcMessage::ShrinkTile,
        Commands::MoveColumnToMonitor { direction } => {
            IpcMessage::MoveColumnToMonitor { direction }
        }
        Commands::TestBindings => IpcMessage::ListBindings,
        Commands::CaptureWindow { hwnd, path } => IpcMessage::CaptureWindow {
            window_hwnd: hwnd,
            path,
        },
        Commands::SaveSnapshot { name } => IpcMessage::SaveSnapshot { name },
        Commands::LoadSnapshot { name } => IpcMessage::LoadSnapshot { name },
        Commands::ListSnapshots => IpcMessage::ListSnapshots,
        Commands::DeleteSnapshot { name } => IpcMessage::DeleteSnapshot { name },
        Commands::ResizeMode => IpcMessage::ToggleResizeMode,
        Commands::MoveColumnUp => IpcMessage::MoveColumnToWorkspaceUp,
        Commands::MoveColumnDown => IpcMessage::MoveColumnToWorkspaceDown,
        Commands::MoveWorkspaceUp => IpcMessage::MoveWorkspaceUp,
        Commands::MoveWorkspaceDown => IpcMessage::MoveWorkspaceDown,
        Commands::ToggleSticky => IpcMessage::ToggleSticky,
        Commands::SetWorkspaceLayout { mode } => IpcMessage::SetWorkspaceLayout { mode },
        Commands::RenameWorkspace { workspace_id, name } => {
            IpcMessage::RenameWorkspace { workspace_id, name }
        }
        Commands::ScreenshotWindow { hwnd, path } => IpcMessage::ScreenshotWindow {
            hwnd: hwnd.map(|h| h as isize),
            path,
        },
        Commands::GetFocus => IpcMessage::GetFocus,
        Commands::GetWorkspaceList => IpcMessage::GetWorkspaceList,
        Commands::Bindings => IpcMessage::GetBindings,
        Commands::ReserveArea {
            side,
            pixels,
            monitor_id,
        } => IpcMessage::ReserveArea {
            side,
            pixels,
            monitor_id,
        },
    })
}

fn event_types(types: &str) -> Vec<String> {
    if types.trim().is_empty() || types.trim() == "*" {
        Vec::new()
    } else {
        types
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect()
    }
}

/// Raw JSON may be pretty-printed on stdin, but the wire uses one line.
fn raw_request_text(argument: Option<&str>, stdin: &mut impl Read) -> Result<String> {
    let mut input = String::new();
    match argument {
        Some(text) if text != "-" => input.push_str(text),
        _ => {
            stdin.read_to_string(&mut input)?;
        }
    }
    // Windows tools often start piped UTF-8 text with a byte-order mark (a .NET process writing
    // to a child's stdin under a UTF-8 console code page does): it is not part of the JSON.
    let value: serde_json::Value = serde_json::from_str(input.trim_start_matches('\u{feff}'))?;
    Ok(serde_json::to_string(&value)?)
}

/// Preserve externally tagged JSON verbatim as a value. Friendly words are
/// only syntactic sugar; capability/range validation belongs to the daemon.
fn output_action(words: &[String]) -> Result<serde_json::Value> {
    use serde_json::json;
    if let [action] = words {
        if let Ok(value) = serde_json::from_str(action) {
            return Ok(value);
        }
    }
    let scalar = |text: &str| {
        serde_json::from_str::<serde_json::Value>(text)
            .unwrap_or_else(|_| serde_json::Value::String(text.to_string()))
    };
    let args: Vec<&str> = words.iter().map(String::as_str).collect();
    Ok(match args.as_slice() {
        ["on"] => json!("On"),
        ["off"] => json!("Off"),
        ["mode", mode] => json!({"Mode": {"mode": mode}}),
        ["scale", scale] => json!({"Scale": {"scale": scalar(scale)}}),
        ["transform", transform] => json!({"Transform": {"transform": transform}}),
        ["position", "auto"] => json!({"Position": {"position": "Auto"}}),
        ["position", x, y] => {
            json!({"Position": {"position": {"x": scalar(x), "y": scalar(y)}}})
        }
        ["vrr", vrr] => {
            let vrr = match *vrr {
                "on" => json!(true),
                "off" => json!(false),
                other => scalar(other),
            };
            json!({"Vrr": {"vrr": vrr}})
        }
        ["max-bpc", max_bpc] => json!({"MaxBpc": {"max_bpc": scalar(max_bpc)}}),
        _ => anyhow::bail!(
            "expected external output-action JSON, or on/off, mode MODE, scale SCALE, \
             transform TRANSFORM, position X Y/auto, vrr on/off, max-bpc BITS"
        ),
    })
}

fn schema_warning(response: &serde_json::Value) -> Option<String> {
    let expected = wiri::ipc::wire::SCHEMA_VERSION;
    match response.get("schema_version") {
        Some(value) if value.as_str() == Some(expected) => None,
        Some(value) => Some(format!(
            "IPC schema mismatch: daemon reports {}, this client expects {}. Update/restart wiri and wiri-ctl together.",
            value, expected
        )),
        None => Some(format!(
            "Daemon has no schema_version (an old, pre-1.3 daemon); this client expects {}. Update/restart wiri.",
            expected
        )),
    }
}

fn warn_schema(response: &serde_json::Value) {
    if let Some(warning) = schema_warning(response) {
        errln!("Warning: {}", warning);
    }
}

// ---------------------------------------------------------------------------
// validate-config subcommand
// ---------------------------------------------------------------------------

/// Locate the config file to validate.
///
/// Resolution order:
///   1. Caller-supplied `path` argument (verbatim — even if it doesn't exist;
///      we surface a clean error message instead of silently falling through).
///   2. `WIRI_CONFIG` environment variable.
///   3. `%APPDATA%\wiri\config.kdl`.
///   4. `%USERPROFILE%\.config\wiri\config.kdl`.
fn resolve_config_path(supplied: Option<&str>) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    if let Some(p) = supplied {
        return Some(PathBuf::from(p));
    }
    wiri::config::default_config_path()
}

/// Implementation of `wiri-ctl validate-config [path]`.
fn validate_config_subcommand(path: Option<&str>, json: bool) -> Result<()> {
    let resolved = match resolve_config_path(path) {
        Some(p) => p,
        None => {
            errln!(
                "error: no config file found. Set WIRI_CONFIG or create %APPDATA%\\wiri\\config.kdl"
            );
            std::process::exit(1);
        }
    };

    if let Err(e) = std::fs::metadata(&resolved) {
        errln!("error: could not read {}: {}", resolved.display(), e);
        std::process::exit(1);
    }

    // The same parser the daemon loads the config with: syntax, unknown keys, values, binds,
    // window rules and the files it includes.
    let issues = wiri::config::validate_config_file(&resolved);
    let error_count = issues
        .iter()
        .filter(|i| i.severity == wiri::config::ValidationSeverity::Error)
        .count();
    let warning_count = issues
        .iter()
        .filter(|i| i.severity == wiri::config::ValidationSeverity::Warning)
        .count();

    if json {
        let root = resolved.display().to_string();
        let json_issues: Vec<serde_json::Value> = issues
            .iter()
            .map(|i| {
                serde_json::json!({
                    "severity": i.severity.to_string(),
                    "file": if i.file.is_empty() { root.clone() } else { i.file.clone() },
                    "line": i.line,
                    "column": i.column,
                    "message": i.message,
                })
            })
            .collect();
        let payload = serde_json::json!({
            "path": resolved.display().to_string(),
            "errors": error_count,
            "warnings": warning_count,
            "issues": json_issues,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        let path_str = resolved.display().to_string();
        if issues.is_empty() {
            outln!("{}: ok (no issues)", path_str);
        } else {
            for issue in &issues {
                // Format mirrors common compiler output: `path:line:col: sev: msg`; an issue
                // in an included file names that file.
                outln!("{}", issue.located(&path_str));
            }
            outln!(
                "{}: {} error(s), {} warning(s)",
                path_str,
                error_count,
                warning_count
            );
        }
    }

    if error_count > 0 {
        std::process::exit(1);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Human-formatted output
// ---------------------------------------------------------------------------

/// Pretty-print the JSON response from the daemon based on the request kind.
/// Falls back to compact JSON for messages without a bespoke formatter.
fn print_human_response(req: &IpcMessage, resp: &serde_json::Value) {
    // First detect a daemon-side error and surface it cleanly.
    if let Some(false) = resp.get("success").and_then(|v| v.as_bool()) {
        let err = resp
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        errln!("error: {}", err);
        std::process::exit(1);
    }

    match req {
        IpcMessage::GetState => print_state(resp),
        IpcMessage::WindowList => print_window_list(resp),
        // The niri-shaped requests.
        IpcMessage::Workspaces => print_workspaces(resp),
        IpcMessage::Windows => print_windows(resp),
        IpcMessage::FocusedWindow => print_focused_window(resp),
        IpcMessage::Outputs
        | IpcMessage::FocusedOutput
        | IpcMessage::KeyboardLayouts
        | IpcMessage::PickWindow
        | IpcMessage::PickColor => outln!("{}", format_query_result(req, resp)),
        IpcMessage::OverviewState => print_overview_state(resp),
        IpcMessage::Version => print_version(resp),
        IpcMessage::ReloadConfig => outln!("Configuration reloaded."),
        IpcMessage::Quit => outln!("Shutdown request sent to wiri."),
        IpcMessage::SwitchWorkspace { id } => {
            outln!("Switched to workspace {}.", id);
        }
        IpcMessage::FocusWindow { window_hwnd } => {
            outln!("Focused window {} (0x{:x}).", window_hwnd, *window_hwnd);
        }
        IpcMessage::CloseWindow { window_hwnd } => {
            outln!(
                "Close request sent to window {} (0x{:x}).",
                window_hwnd,
                *window_hwnd
            );
        }
        IpcMessage::SpawnCommand { command, args } => {
            let what = if args.is_empty() {
                command.clone()
            } else {
                format!("{} {}", command, args.join(" "))
            };
            let pid = resp.get("pid").and_then(|v| v.as_u64());
            match pid {
                Some(p) => outln!("Spawned '{}'  (pid: {}).", what, p),
                None => outln!("Spawned '{}'.", what),
            }
        }
        IpcMessage::WindowFloat { window_hwnd } => {
            outln!("Window {} is now floating.", window_hwnd);
        }
        IpcMessage::WindowUnfloat { window_hwnd } => {
            outln!("Window {} is now tiled.", window_hwnd);
        }
        IpcMessage::TileRequest { window_hwnd, .. } => {
            outln!("Tile request queued for window {}.", window_hwnd);
        }
        IpcMessage::WindowMoveToWorkspace {
            window_hwnd,
            workspace_id,
        } => {
            outln!(
                "Moved window {} to workspace {}.",
                window_hwnd,
                workspace_id
            );
        }
        IpcMessage::MonitorList => print_monitor_list(resp),
        IpcMessage::GetFocus => outln!("{}", format_focus(resp)),
        IpcMessage::ListBindings => print_bindings(resp),
        IpcMessage::CaptureWindow { window_hwnd, .. } => {
            let path = resp
                .get("result")
                .and_then(|r| r.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            outln!("Captured window {} → {}", window_hwnd, path);
        }
        IpcMessage::SaveSnapshot { name } => {
            let path = resp
                .get("result")
                .and_then(|r| r.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            outln!("Saved snapshot '{}' → {}", name, path);
        }
        IpcMessage::LoadSnapshot { name } => {
            let monitors = resp
                .get("result")
                .and_then(|r| r.get("monitors"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            outln!("Restored snapshot '{}' ({} monitor(s)).", name, monitors);
        }
        IpcMessage::ListSnapshots => {
            let names = resp
                .get("result")
                .and_then(|r| r.get("snapshots"))
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if names.is_empty() {
                outln!("No saved snapshots.");
            } else {
                for n in &names {
                    if let Some(s) = n.as_str() {
                        outln!("  {}", s);
                    }
                }
                outln!();
                outln!("  {} snapshot(s)", names.len());
            }
        }
        IpcMessage::DeleteSnapshot { name } => {
            outln!("Deleted snapshot '{}'.", name);
        }
        IpcMessage::ToggleResizeMode => {
            let active = resp
                .get("result")
                .and_then(|r| r.get("active"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if active {
                outln!("Resize mode: ON (Mod+Arrow grows/shrinks; Esc exits)");
            } else {
                outln!("Resize mode: OFF");
            }
        }
        IpcMessage::GetBindings => print_bindings_table(resp),
        IpcMessage::ReserveArea { side, pixels, .. } => {
            if *pixels == 0 {
                outln!("Released {} reservation.", side);
            } else {
                outln!("Reserved {} px on the {} edge.", pixels, side);
            }
        }
        _ => {
            // Generic: report success with a hint when present.
            if let Some(note) = resp.get("note").and_then(|v| v.as_str()) {
                outln!("ok ({})", note);
            } else {
                outln!("ok");
            }
        }
    }
}

/// Human-readable summaries for the new niri-shaped query results. Keep
/// unknown fields visible rather than silently hiding daemon capabilities.
fn format_query_result(req: &IpcMessage, resp: &serde_json::Value) -> String {
    let result = &resp["result"];
    match req {
        IpcMessage::Outputs => match result["outputs"].as_object() {
            Some(outputs) if !outputs.is_empty() => outputs
                .iter()
                .map(|(name, output)| format!("{}: {}", name, output))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => "No outputs.".to_string(),
        },
        IpcMessage::FocusedOutput => match &result["focused_output"] {
            serde_json::Value::Null => "No output is focused.".to_string(),
            output => format!("Focused output: {}", output),
        },
        IpcMessage::KeyboardLayouts => {
            let layouts = &result["keyboard_layouts"];
            let Some(names) = layouts["names"]
                .as_array()
                .filter(|names| !names.is_empty())
            else {
                return "No keyboard layouts.".to_string();
            };
            let current = layouts["current_idx"].as_u64();
            names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    format!(
                        "{} {}: {}",
                        if current == Some(index as u64) {
                            "*"
                        } else {
                            " "
                        },
                        index,
                        name.as_str().unwrap_or("unknown")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        IpcMessage::PickWindow => match &result["window"] {
            serde_json::Value::Null => "No window was picked.".to_string(),
            window => format!("Picked window: {}", window),
        },
        IpcMessage::PickColor => match &result["color"] {
            serde_json::Value::Null => "No color was picked.".to_string(),
            color => format!("Picked color: {}", color),
        },
        _ => unreachable!("only query requests have these result shapes"),
    }
}

/// Format the response to `GetFocus`: the focused window and its workspace.
fn format_focus(resp: &serde_json::Value) -> String {
    let result = resp.get("result").cloned().unwrap_or(serde_json::json!({}));
    let workspace = result
        .get("workspace_id")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    match result.get("window_hwnd").and_then(|v| v.as_i64()) {
        Some(hwnd) => format!(
            "Focused window {} (0x{:x}) on workspace {}.",
            hwnd, hwnd, workspace
        ),
        None => format!("No window is focused (workspace {}).", workspace),
    }
}

/// Format the response to `GetState` as a multi-line summary block.
fn print_state(resp: &serde_json::Value) {
    let result = resp.get("result").cloned().unwrap_or(serde_json::json!({}));
    let windows = result
        .get("windows")
        .and_then(|w| w.as_array())
        .cloned()
        .unwrap_or_default();
    let workspaces = result
        .get("workspaces")
        .and_then(|w| w.as_array())
        .cloned()
        .unwrap_or_default();
    let active = result
        .get("active_workspace")
        .and_then(|v| v.as_str())
        .unwrap_or("<none>");

    let total_windows = windows.len();
    let total_workspaces = workspaces.len();

    outln!("wiri state");
    outln!("──────────");
    outln!("  monitors / workspaces : {}", total_workspaces);
    outln!("  managed windows       : {}", total_windows);
    outln!("  active workspace      : {}", active);

    if !workspaces.is_empty() {
        outln!();
        outln!("  workspace             windows");
        outln!("  ────────────────────  ───────");
        for ws in &workspaces {
            let name = ws.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let count = ws.get("window_count").and_then(|v| v.as_u64()).unwrap_or(0);
            outln!("  {:<20}  {:>7}", truncate(name, 20), count);
        }
    }
}

/// Format the response to `WindowList` as a Hwnd|PID|Class|Title table.
fn print_window_list(resp: &serde_json::Value) {
    let result = resp
        .get("result")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    if result.is_empty() {
        outln!("No managed windows.");
        return;
    }

    // Auto-size title column from terminal width, fall back to 60 chars.
    let term_cols = terminal_cols().unwrap_or(120) as usize;
    let prefix_cols = 12 + 8 + 22 + 6 + 4; // hwnd | pid | class | ws | spaces
    let title_w = term_cols.saturating_sub(prefix_cols).max(20);

    outln!(
        "  {:>10}  {:>6}  {:<20}  {:<3}  {}",
        "HWND",
        "PID",
        "CLASS",
        "WS",
        "TITLE"
    );
    outln!(
        "  {:>10}  {:>6}  {:<20}  {:<3}  {}",
        "─".repeat(10),
        "─".repeat(6),
        "─".repeat(20),
        "─".repeat(3),
        "─".repeat(title_w.min(40)),
    );

    for w in &result {
        let hwnd = w.get("hwnd").and_then(|v| v.as_i64()).unwrap_or(0);
        let pid = w.get("process_id").and_then(|v| v.as_u64()).unwrap_or(0);
        let class = w.get("class_name").and_then(|v| v.as_str()).unwrap_or("");
        let title = w.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let ws = w
            .get("workspace")
            .and_then(|v| v.as_i64())
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".to_string());

        outln!(
            "  {:>10}  {:>6}  {:<20}  {:<3}  {}",
            hwnd,
            pid,
            truncate(class, 20),
            ws,
            truncate(title, title_w)
        );
    }
    outln!();
    outln!(
        "  {} window{}",
        result.len(),
        if result.len() == 1 { "" } else { "s" }
    );
}

/// "yes", "no" or "-" for a flag of a reply.
fn yes_no(value: Option<&serde_json::Value>) -> &'static str {
    match value.and_then(|v| v.as_bool()) {
        Some(true) => "yes",
        Some(false) => "no",
        None => "-",
    }
}

/// Format the reply of the `workspaces` request: niri's workspaces.
fn print_workspaces(resp: &serde_json::Value) {
    let result = resp.get("result").cloned().unwrap_or(serde_json::json!({}));
    let workspaces = result
        .get("workspaces")
        .and_then(|w| w.as_array())
        .cloned()
        .unwrap_or_default();
    if workspaces.is_empty() {
        outln!("No workspaces.");
        return;
    }

    outln!(
        "  {:<5}  {:<16}  {:<16}  {:<7}  {:<8}  {}",
        "IDX",
        "NAME",
        "OUTPUT",
        "ACTIVE",
        "FOCUSED",
        "WINDOW"
    );
    outln!(
        "  {:<5}  {:<16}  {:<16}  {:<7}  {:<8}  {}",
        "─".repeat(5),
        "─".repeat(16),
        "─".repeat(16),
        "─".repeat(7),
        "─".repeat(8),
        "─".repeat(10),
    );

    for workspace in &workspaces {
        let idx = workspace.get("idx").and_then(|v| v.as_u64()).unwrap_or(0);
        let name = workspace
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let output = workspace
            .get("output")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let window = match workspace.get("active_window_id").and_then(|v| v.as_u64()) {
            Some(id) => id.to_string(),
            None => "-".to_string(),
        };
        outln!(
            "  {:<5}  {:<16}  {:<16}  {:<7}  {:<8}  {}",
            idx,
            truncate(name, 16),
            truncate(output, 16),
            yes_no(workspace.get("is_active")),
            yes_no(workspace.get("is_focused")),
            window
        );
    }
    outln!();
    outln!(
        "  {} workspace{}",
        workspaces.len(),
        if workspaces.len() == 1 { "" } else { "s" }
    );
}

/// Format the reply of the `windows` request: niri's windows.
fn print_windows(resp: &serde_json::Value) {
    let result = resp.get("result").cloned().unwrap_or(serde_json::json!({}));
    let windows = result
        .get("windows")
        .and_then(|w| w.as_array())
        .cloned()
        .unwrap_or_default();
    if windows.is_empty() {
        outln!("No managed windows.");
        return;
    }

    let term_cols = terminal_cols().unwrap_or(120) as usize;
    let title_w = term_cols.saturating_sub(10 + 7 + 20 + 6 + 6).max(20);

    outln!(
        "  {:>10}  {:>6}  {:<20}  {:<6}  {:<5}  {}",
        "ID",
        "PID",
        "APP",
        "WORKSPACE",
        "FLAGS",
        "TITLE"
    );
    outln!(
        "  {:>10}  {:>6}  {:<20}  {:<6}  {:<5}  {}",
        "─".repeat(10),
        "─".repeat(6),
        "─".repeat(20),
        "─".repeat(6),
        "─".repeat(5),
        "─".repeat(title_w.min(40)),
    );

    for window in &windows {
        let id = window.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
        let pid = window.get("pid").and_then(|v| v.as_u64()).unwrap_or(0);
        let app = window.get("app_id").and_then(|v| v.as_str()).unwrap_or("-");
        let workspace = match window.get("workspace_id").and_then(|v| v.as_u64()) {
            Some(id) => id.to_string(),
            None => "-".to_string(),
        };
        let mut flags = Vec::new();
        if window.get("is_focused").and_then(|v| v.as_bool()) == Some(true) {
            flags.push("focus");
        }
        if window.get("is_floating").and_then(|v| v.as_bool()) == Some(true) {
            flags.push("float");
        }
        if window.get("is_urgent").and_then(|v| v.as_bool()) == Some(true) {
            flags.push("urgent");
        }
        let title = window.get("title").and_then(|v| v.as_str()).unwrap_or("");
        outln!(
            "  {:>10}  {:>6}  {:<20}  {:<6}  {:<5}  {}",
            id,
            pid,
            truncate(app, 20),
            workspace,
            if flags.is_empty() {
                "-".to_string()
            } else {
                flags.join(",")
            },
            truncate(title, title_w)
        );
    }
    outln!();
    outln!(
        "  {} window{}",
        windows.len(),
        if windows.len() == 1 { "" } else { "s" }
    );
}

/// Format the reply of the `focused_window` request: the window that has the
/// keyboard focus.
fn print_focused_window(resp: &serde_json::Value) {
    let result = resp.get("result").cloned().unwrap_or(serde_json::json!({}));
    let Some(window) = result.get("focused_window").filter(|w| w.is_object()) else {
        outln!("No window is focused.");
        return;
    };
    let id = window.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
    let title = window.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let app = window.get("app_id").and_then(|v| v.as_str()).unwrap_or("-");
    let pid = window.get("pid").and_then(|v| v.as_u64()).unwrap_or(0);
    let workspace = match window.get("workspace_id").and_then(|v| v.as_u64()) {
        Some(id) => id.to_string(),
        None => "-".to_string(),
    };
    outln!("Focused window {} (0x{:x}): {}", id, id, title);
    outln!("  program   : {}", app);
    outln!("  pid       : {}", pid);
    outln!("  workspace : {}", workspace);
}

/// Format the reply of the `overview_state` request.
fn print_overview_state(resp: &serde_json::Value) {
    let is_open = resp
        .get("result")
        .and_then(|r| r.get("is_open"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    outln!("Overview: {}.", if is_open { "open" } else { "closed" });
}

/// Format the reply of the `version` request: the version of the daemon.
fn print_version(resp: &serde_json::Value) {
    let version = resp
        .get("result")
        .and_then(|r| r.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    outln!("wiri {}", version);
}

/// Format `MonitorList` as a human-readable table — bounds, work area, DPI
/// scale, active workspace, and which monitor is focused.
fn print_monitor_list(resp: &serde_json::Value) {
    let result = resp
        .get("result")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    if result.is_empty() {
        outln!("No monitors registered with the running wiri.");
        return;
    }

    outln!(
        "  {:<3}  {:<18}  {:<10}  {:<5}  {:>3}  {:>5}  {:<20}  {}",
        "F",
        "BOUNDS (x,y wxh)",
        "DPI",
        "WS",
        "WIN",
        "ID",
        "WORK AREA (x,y wxh)",
        "NAME"
    );
    outln!(
        "  {:<3}  {:<18}  {:<10}  {:<5}  {:>3}  {:>5}  {:<20}  {}",
        "─".repeat(3),
        "─".repeat(18),
        "─".repeat(10),
        "─".repeat(5),
        "─".repeat(3),
        "─".repeat(5),
        "─".repeat(20),
        "─".repeat(12),
    );

    for m in &result {
        let bx = m.get("bounds_x").and_then(|v| v.as_i64()).unwrap_or(0);
        let by = m.get("bounds_y").and_then(|v| v.as_i64()).unwrap_or(0);
        let bw = m.get("bounds_width").and_then(|v| v.as_u64()).unwrap_or(0);
        let bh = m.get("bounds_height").and_then(|v| v.as_u64()).unwrap_or(0);
        let wx = m.get("work_area_x").and_then(|v| v.as_i64()).unwrap_or(0);
        let wy = m.get("work_area_y").and_then(|v| v.as_i64()).unwrap_or(0);
        let ww = m
            .get("work_area_width")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let wh = m
            .get("work_area_height")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let scale = m
            .get("scale_factor")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);
        let ws = m
            .get("active_workspace")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let win_count = m.get("window_count").and_then(|v| v.as_u64()).unwrap_or(0);
        let id = m.get("output_id").and_then(|v| v.as_u64()).unwrap_or(0);
        let focused = m.get("focused").and_then(|v| v.as_bool()).unwrap_or(false);
        let name = m.get("name").and_then(|v| v.as_str()).unwrap_or("");

        outln!(
            "  {:<3}  {:<18}  {:<10}  {:<5}  {:>3}  {:>5x}  {:<20}  {}",
            if focused { "*" } else { " " },
            format!("{},{} {}x{}", bx, by, bw, bh),
            format!("{:.0}% ({:.1})", scale * 100.0, scale),
            ws,
            win_count,
            id & 0xFFFF,
            format!("{},{} {}x{}", wx, wy, ww, wh),
            name,
        );
    }

    outln!();
    outln!(
        "  {} monitor{}  (* = focused output)",
        result.len(),
        if result.len() == 1 { "" } else { "s" }
    );
}

/// Format the response to `ListBindings` (sent by `test-bindings`) as a
/// per-binding table.  Shows the human-readable chord, the action that
/// fires, and the raw Win32 modifier+VK so power users can correlate with
/// `RegisterHotKey` documentation.
fn print_bindings(resp: &serde_json::Value) {
    let bindings = resp
        .pointer("/result/bindings")
        .and_then(|b| b.as_array())
        .cloned()
        .unwrap_or_default();
    if bindings.is_empty() {
        outln!("No hotkey bindings registered.");
        outln!("If the daemon just started, try again in a moment; otherwise");
        outln!("check the logs for `RegisterHotKey` failures.");
        return;
    }

    outln!(
        "  {:<5}  {:<28}  {:<7}  {:<5}  {}",
        "ID",
        "CHORD",
        "MODS",
        "VK",
        "ACTION"
    );
    outln!(
        "  {:<5}  {:<28}  {:<7}  {:<5}  {}",
        "─".repeat(5),
        "─".repeat(28),
        "─".repeat(7),
        "─".repeat(5),
        "─".repeat(30),
    );

    for b in &bindings {
        let id = b.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
        let chord = b.get("chord").and_then(|v| v.as_str()).unwrap_or("?");
        let mods = b.get("modifiers").and_then(|v| v.as_u64()).unwrap_or(0);
        let vk = b.get("vk_code").and_then(|v| v.as_u64()).unwrap_or(0);
        let action = b.get("action").and_then(|v| v.as_str()).unwrap_or("?");
        outln!(
            "  {:<5}  {:<28}  0x{:04X}   0x{:02X}   {}",
            id,
            truncate(chord, 28),
            mods,
            vk,
            truncate(action, 30),
        );
    }

    outln!();
    outln!(
        "  {} binding{} registered.  See `docs/TROUBLESHOOTING.md` for known",
        bindings.len(),
        if bindings.len() == 1 { "" } else { "s" }
    );
    outln!("  Windows hotkey conflicts that may swallow your chord.");
}

/// Format the response to `GetBindings` (`wiri-ctl bindings`) as a
/// two-column chord+action table.  When the daemon returns an empty list
/// (hotkey thread not yet started), print the hardcoded defaults instead.
fn print_bindings_table(resp: &serde_json::Value) {
    let entries = resp
        .get("result")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    if entries.is_empty() {
        outln!("No live bindings reported by the daemon.");
        outln!("(Hotkey thread may not have started yet — try again in a moment.)");
        outln!();
        outln!("Default bindings (Ctrl+Alt prefix):");
        outln!();
        print_default_bindings_table();
        return;
    }

    let chord_w = 26usize;
    let action_w = 40usize;

    outln!("  {:<cw$}  {}", "Chord", "Action", cw = chord_w);
    outln!(
        "  {:<cw$}  {}",
        "─".repeat(chord_w),
        "─".repeat(action_w),
        cw = chord_w
    );

    for e in &entries {
        let chord = e.get("chord").and_then(|v| v.as_str()).unwrap_or("?");
        let action = e.get("action").and_then(|v| v.as_str()).unwrap_or("?");
        outln!(
            "  {:<cw$}  {}",
            truncate(chord, chord_w),
            truncate(action, action_w),
            cw = chord_w
        );
    }

    outln!();
    outln!(
        "  {} binding{} registered.",
        entries.len(),
        if entries.len() == 1 { "" } else { "s" }
    );
}

/// Print the hardcoded default bindings table as a fallback (no daemon needed).
fn print_default_bindings_table() {
    // Mirror of `default_bindings_table("Ctrl+Alt")` in overlay/bindings_cheatsheet.rs.
    let p = "Ctrl+Alt";
    let s = "Ctrl+Alt+Shift";
    let defaults: &[(&str, &str)] = &[
        ("Ctrl+Alt+Left", "focus-column-left"),
        ("Ctrl+Alt+Right", "focus-column-right"),
        ("Ctrl+Alt+Up", "focus-up"),
        ("Ctrl+Alt+Down", "focus-down"),
        (&format!("{}+Left", s), "move-column-left"),
        (&format!("{}+Right", s), "move-column-right"),
        (&format!("{}+Q", p), "close-window"),
        (&format!("{}+Enter", p), "spawn (terminal)"),
        (&format!("{}+F", p), "toggle-fullscreen"),
        (&format!("{}+T", p), "toggle-floating"),
        (&format!("{}+H", p), "scroll-left"),
        (&format!("{}+L", p), "scroll-right"),
        (&format!("{}+Q", s), "quit"),
        (&format!("{}+Space", p), "overview-toggle"),
        ("Escape", "exit overview / resize mode"),
        (&format!("{}+O", p), "overview-select"),
        (&format!("{}+\\", p), "column-toggle-tabbed"),
        (&format!("{}+]", p), "tab-next"),
        (&format!("{}+[", p), "tab-prev"),
        (&format!("{}+1…9", p), "focus-workspace-N"),
        (&format!("{}+1…9", s), "move-to-workspace-N"),
        (&format!("{}+PageUp", p), "focus-workspace-up"),
        (&format!("{}+PageDown", p), "focus-workspace-down"),
        (&format!("{}+R", p), "enter-resize-mode"),
        (&format!("{}+R", s), "center-column"),
        (&format!("{}+W", p), "column-width-cycle"),
        (&format!("{}+-", p), "resize-column-left"),
        (&format!("{}++", p), "resize-column-right"),
        (&format!("{}+`", p), "focus-previous (alt-tab)"),
        (&format!("{}+P", p), "screenshot"),
        (&format!("{}+A", p), "toggle-always-on-top"),
        (&format!("{}+,", p), "consume-window-into-column"),
        (&format!("{}+.", p), "expel-window-from-column"),
        (&format!("{}+E", p), "expand-column-to-available"),
        (&format!("{}+F", s), "maximize-column"),
        (&format!("{}+L", s), "grow-column-width"),
        (&format!("{}+H", s), "shrink-column-width"),
        (&format!("{}+K", s), "grow-tile-height"),
        (&format!("{}+J", s), "shrink-tile-height"),
        (&format!("{}+PageUp", s), "move-workspace-up"),
        (&format!("{}+PageDown", s), "move-workspace-down"),
        (&format!("{}+Up", s), "move-column-to-workspace-up"),
        (&format!("{}+Down", s), "move-column-to-workspace-down"),
        (&format!("{}+,", s), "move-column-to-monitor-left"),
        (&format!("{}+.", s), "move-column-to-monitor-right"),
        (&format!("{}+S", p), "toggle-sticky"),
        (&format!("{}+?", s), "show-key-bindings (cheatsheet)"),
        (&format!("{}+Home", p), "focus-column-first"),
        (&format!("{}+End", p), "focus-column-last"),
        (&format!("{}+Home", s), "move-column-to-first"),
        (&format!("{}+End", s), "move-column-to-last"),
        (&format!("{}+[", s), "consume-or-expel-window-left"),
        (&format!("{}+]", s), "consume-or-expel-window-right"),
        (&format!("{}+K", p), "move-window-up"),
        (&format!("{}+J", p), "move-window-down"),
    ];

    let chord_w = 28usize;
    let action_w = 36usize;
    outln!("  {:<cw$}  {}", "Chord", "Action", cw = chord_w);
    outln!(
        "  {:<cw$}  {}",
        "─".repeat(chord_w),
        "─".repeat(action_w),
        cw = chord_w
    );
    for (chord, action) in defaults {
        outln!(
            "  {:<cw$}  {}",
            truncate(chord, chord_w),
            action,
            cw = chord_w
        );
    }
    outln!();
    outln!("  {} built-in bindings.", defaults.len());
}

// ---------------------------------------------------------------------------
// Event streaming
// ---------------------------------------------------------------------------

/// Subscribe to the daemon's event broadcast channel and print events as they
/// arrive.
///
/// `raw`: when `true` (selected by `--stream` or `--json`), each event is
/// printed as a single compact JSON object terminated by `\n` — exactly as
/// received from the daemon, so the output is safe to pipe into `jq` or any
/// line-oriented JSON consumer.
///
/// When `raw` is `false` (the default), each event is printed as a
/// human-readable summary line prefixed with a wall-clock timestamp.
fn stream_events(message: &IpcMessage, timeout_secs: u64, raw: bool) -> Result<()> {
    let IpcMessage::SubscribeEvents { event_types } = message else {
        unreachable!("event-stream sends SubscribeEvents");
    };
    block_on(async {
        let mut client = connect_ipc(timeout_secs).await?;
        let ack = client.request(message).await?;
        warn_schema(&ack);
        if ack.get("success").and_then(|value| value.as_bool()) == Some(false) {
            anyhow::bail!(
                "event subscription failed: {}",
                ack.get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error")
            );
        }
        let filter_desc = if event_types.is_empty() {
            "all types".to_string()
        } else {
            event_types.join(",")
        };
        let mode_desc = if raw {
            "newline-delimited JSON"
        } else {
            "human-readable"
        };
        errln!(
            "Subscribed to events ({}) [{}]. Press Ctrl+C to exit.",
            filter_desc,
            mode_desc
        );
        // request() already consumed the acknowledgement. The client's
        // persistent buffer retains any events received alongside it.
        while let Some(value) = client.read_events().await? {
            if raw {
                println!("{}", serde_json::to_string(&value)?);
            } else {
                outln!("{}", format_event(&value));
            }
            std::io::stdout().flush()?;
        }
        Ok(())
    })
}

/// Format an IpcEvent JSON value as a single human-readable line:
///   `HH:MM:SS  event_kind  field=value field=value …`
fn format_event(v: &serde_json::Value) -> String {
    let ts = wall_clock_hms();
    let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("event");
    let mut parts: Vec<String> = Vec::new();

    if let Some(data) = v.get("data") {
        match data {
            serde_json::Value::Object(map) => {
                for (k, val) in map.iter() {
                    parts.push(format!("{}={}", k, summarize_value(val)));
                }
            }
            other => parts.push(summarize_value(other)),
        }
    }

    format!("{}  {:<22}  {}", ts, kind, parts.join("  "))
}

/// Build a compact one-line representation of a JSON value (for event lines).
fn summarize_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => truncate(s, 60).to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            // Compact JSON, capped at 60 chars.
            let s = serde_json::to_string(v).unwrap_or_default();
            truncate(&s, 60).to_string()
        }
        serde_json::Value::Null => "null".into(),
    }
}

// ---------------------------------------------------------------------------
// IPC plumbing
// ---------------------------------------------------------------------------

/// The synchronous CLI drives async pipe I/O on this thread only. No worker
/// threads or timer driver are needed.
fn block_on<T>(future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()?
        .block_on(future)
}

async fn connect_ipc(timeout_secs: u64) -> Result<IpcClient> {
    let mut client = IpcClient::new();
    client.connect_with_timeout(timeout_secs).await?;
    Ok(client)
}

fn send_ipc_message(message: &IpcMessage, timeout_secs: u64) -> Result<serde_json::Value> {
    block_on(async {
        let mut client = connect_ipc(timeout_secs).await?;
        Ok(client.request(message).await?)
    })
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// Truncate `s` to at most `max` chars, appending '…' if anything was dropped.
fn truncate(s: &str, max: usize) -> std::borrow::Cow<'_, str> {
    if s.chars().count() <= max {
        std::borrow::Cow::Borrowed(s)
    } else {
        let cut = s.chars().take(max.saturating_sub(1)).collect::<String>();
        std::borrow::Cow::Owned(format!("{}…", cut))
    }
}

/// Best-effort terminal width on Windows; returns None if it cannot be probed.
fn terminal_cols() -> Option<u32> {
    use windows::Win32::System::Console::{
        GetConsoleScreenBufferInfo, GetStdHandle, CONSOLE_SCREEN_BUFFER_INFO, STD_OUTPUT_HANDLE,
    };
    unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE).ok()?;
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        if GetConsoleScreenBufferInfo(h, &mut info).is_ok() {
            let w = (info.srWindow.Right - info.srWindow.Left + 1).max(20);
            Some(w as u32)
        } else {
            None
        }
    }
}

/// Current local wall-clock time as `HH:MM:SS`.
///
/// In windows-rs 0.58 the `GetLocalTime` binding lives in
/// `Win32::System::SystemInformation` and takes no arguments, returning a
/// `SYSTEMTIME` from `Win32::Foundation`.  We fall back to a UTC-derived
/// "time-of-day" when GetLocalTime is unavailable for any reason.
fn wall_clock_hms() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let st = unsafe { GetLocalTime() };
    if st.wHour == 0 && st.wMinute == 0 && st.wSecond == 0 && st.wMilliseconds == 0 {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let h = (secs / 3600) % 24;
        let m = (secs / 60) % 60;
        let s = secs % 60;
        return format!("{:02}:{:02}:{:02}", h, m, s);
    }
    format!("{:02}:{:02}:{:02}", st.wHour, st.wMinute, st.wSecond)
}

/// The words after an action name as they are in a `binds {}` line: a word written in double
/// quotes (`'"2"'` in a shell, which keeps the quotes) is a name, not a number
/// (`focus-workspace "2"` is the workspace named "2", `focus-workspace 2` the second one).
/// Returns the words without the quotes and which of them had them (empty when none had).
fn split_quoted_words(words: Vec<String>) -> (Vec<String>, Vec<bool>) {
    let mut quoted = Vec::with_capacity(words.len());
    let words: Vec<String> = words
        .into_iter()
        .map(|word| {
            if word.len() >= 2 && word.starts_with('"') && word.ends_with('"') {
                quoted.push(true);
                word[1..word.len() - 1].to_string()
            } else {
                quoted.push(false);
                word
            }
        })
        .collect();
    if !quoted.iter().any(|q| *q) {
        quoted.clear();
    }
    (words, quoted)
}

#[cfg(test)]
mod tests {
    use super::{
        fold_to_ascii, format_focus, format_query_result, raw_request_text, request_for,
        schema_warning, split_quoted_words, Cli, Commands,
    };
    use clap::Parser;
    use wiri::ipc::IpcMessage;

    /// The request a command line sends to the daemon.
    fn request_of(args: &[&str]) -> IpcMessage {
        let cli = Cli::try_parse_from(args).expect("the command line parses");
        request_for(cli.command.into_command()).expect("the request is valid")
    }

    #[test]
    fn the_niri_subcommands_ask_for_the_niri_requests() {
        assert!(matches!(
            request_of(&["wiri-ctl", "workspaces"]),
            IpcMessage::Workspaces
        ));
        assert!(matches!(
            request_of(&["wiri-ctl", "windows"]),
            IpcMessage::Windows
        ));
        assert!(matches!(
            request_of(&["wiri-ctl", "focused-window"]),
            IpcMessage::FocusedWindow
        ));
        assert!(matches!(
            request_of(&["wiri-ctl", "overview-state"]),
            IpcMessage::OverviewState
        ));
        assert!(matches!(
            request_of(&["wiri-ctl", "version"]),
            IpcMessage::Version
        ));
        // The older list of windows (with the class, the geometry and the place
        // in the strip) has a subcommand of its own now.
        assert!(matches!(
            request_of(&["wiri-ctl", "window-list"]),
            IpcMessage::WindowList
        ));
    }

    #[test]
    fn the_niri_subcommands_take_no_arguments() {
        for name in [
            "workspaces",
            "windows",
            "focused-window",
            "outputs",
            "focused-output",
            "keyboard-layouts",
            "pick-window",
            "pick-color",
            "request-error",
            "return-error",
            "overview-state",
            "version",
        ] {
            assert!(Cli::try_parse_from(["wiri-ctl", name]).is_ok(), "{}", name);
            assert!(
                Cli::try_parse_from(["wiri-ctl", name, "something"]).is_err(),
                "{} takes no arguments",
                name
            );
        }
    }

    #[test]
    fn the_json_flag_works_before_and_after_the_subcommand() {
        for args in [
            ["wiri-ctl", "--json", "workspaces"],
            ["wiri-ctl", "workspaces", "--json"],
        ] {
            let cli = Cli::try_parse_from(args).expect("the command line parses");
            assert!(cli.json, "{:?}", args);
        }
        let cli = Cli::try_parse_from(["wiri-ctl", "workspaces"]).expect("parses");
        assert!(!cli.json, "the readable output is the default");
    }

    #[test]
    fn msg_prefix_and_new_queries_generate_the_same_requests() {
        for (command, kind) in [
            ("outputs", "outputs"),
            ("focused-output", "focused_output"),
            ("keyboard-layouts", "keyboard_layouts"),
            ("pick-window", "pick_window"),
            ("pick-color", "pick_color"),
            ("request-error", "return_error"),
            ("return-error", "return_error"),
            ("state", "get_state"),
            ("window-list", "window_list"),
        ] {
            let direct = serde_json::to_value(request_of(&["wiri-ctl", command])).unwrap();
            let prefixed = serde_json::to_value(request_of(&["wiri-ctl", "msg", command])).unwrap();
            assert_eq!(direct, prefixed, "{}", command);
            assert_eq!(direct["type"], kind);
        }
        // The optional prefix must not change the established action parser
        // or reinterpret the program's own command-line flags.
        for tail in [
            vec!["action", "set-column-width", "+10%"],
            vec!["action", "focus-workspace", "\"2\""],
            vec!["spawn", "wt.exe", "-d", "C:\\My Projects"],
        ] {
            let direct: Vec<&str> = std::iter::once("wiri-ctl")
                .chain(tail.iter().copied())
                .collect();
            let prefixed: Vec<&str> = ["wiri-ctl", "msg"].into_iter().chain(tail).collect();
            assert_eq!(
                serde_json::to_value(request_of(&direct)).unwrap(),
                serde_json::to_value(request_of(&prefixed)).unwrap()
            );
        }
    }

    #[test]
    fn print_request_and_json_flags_are_global_with_or_without_msg() {
        for args in [
            vec!["wiri-ctl", "--print-request", "outputs"],
            vec!["wiri-ctl", "outputs", "--print-request"],
            vec!["wiri-ctl", "--print-request", "msg", "outputs"],
            vec!["wiri-ctl", "msg", "--print-request", "outputs"],
            vec!["wiri-ctl", "msg", "outputs", "--print-request"],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert!(cli.print_request);
            let message = request_for(cli.command.into_command()).unwrap();
            assert_eq!(
                serde_json::to_value(message).unwrap(),
                serde_json::json!({"type": "outputs"})
            );
        }
        let cli = Cli::try_parse_from([
            "wiri-ctl",
            "msg",
            "output",
            "DISPLAY1",
            "position",
            "-100",
            "0",
            "--print-request",
            "--json",
        ])
        .unwrap();
        assert!(cli.print_request && cli.json);
        assert!(matches!(
            request_for(cli.command.into_command()).unwrap(),
            IpcMessage::Output { .. }
        ));
        for args in [
            ["wiri-ctl", "--json", "msg", "outputs"],
            ["wiri-ctl", "msg", "--json", "outputs"],
            ["wiri-ctl", "msg", "outputs", "--json"],
        ] {
            assert!(Cli::try_parse_from(args).unwrap().json);
        }
    }

    #[test]
    fn event_stream_alias_and_filters_have_printable_requests() {
        for command in ["event-stream", "events"] {
            let request = request_of(&[
                "wiri-ctl",
                "msg",
                "--print-request",
                command,
                "--types",
                "ignored",
                "--filter",
                " window_opened, window_closed, ",
            ]);
            assert!(matches!(
                request,
                IpcMessage::SubscribeEvents { event_types }
                    if event_types == vec!["window_opened", "window_closed"]
            ));
            assert!(matches!(
                request_of(&["wiri-ctl", command]),
                IpcMessage::SubscribeEvents { event_types } if event_types.is_empty()
            ));
        }
    }

    #[test]
    fn raw_request_argument_or_stdin_is_exactly_one_json_value() {
        let mut stdin = std::io::Cursor::new(b"{\n \"Action\": { \"CloseWindow\": {} }\n}\n");
        let from_stdin = raw_request_text(None, &mut stdin).unwrap();
        assert!(!from_stdin.contains('\n'));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&from_stdin).unwrap(),
            serde_json::json!({"Action": {"CloseWindow": {}}})
        );
        let mut stdin = std::io::Cursor::new(b"\"Windows\"\n");
        assert_eq!(
            raw_request_text(Some("-"), &mut stdin).unwrap(),
            "\"Windows\""
        );
        let mut stdin = std::io::Cursor::new(b"\xEF\xBB\xBF\"KeyboardLayouts\"\r\n");
        assert_eq!(
            raw_request_text(None, &mut stdin).unwrap(),
            "\"KeyboardLayouts\"",
            "a UTF-8 byte-order mark before the JSON is ignored"
        );
        let mut unread = std::io::Cursor::new(b"not JSON");
        assert_eq!(
            raw_request_text(Some(" \"Outputs\" "), &mut unread).unwrap(),
            "\"Outputs\""
        );
        assert_eq!(unread.position(), 0);
        assert!(raw_request_text(Some("\"Outputs\"\n\"Windows\""), &mut unread).is_err());
        assert!(raw_request_text(Some(""), &mut unread).is_err());
        for args in [
            vec!["wiri-ctl", "raw-request"],
            vec!["wiri-ctl", "msg", "raw-request", "-"],
            vec!["wiri-ctl", "--print-request", "raw-request", "\"Outputs\""],
        ] {
            assert!(matches!(
                Cli::try_parse_from(args).unwrap().command.into_command(),
                Commands::RawRequest { .. }
            ));
        }
    }

    #[test]
    fn output_actions_are_external_pascal_enums_and_defer_semantic_validation() {
        for action in [
            "\"On\"",
            "\"Off\"",
            r#"{"Mode":{"mode":"1920x1080@60"}}"#,
            r#"{"Scale":{"scale":1.5}}"#,
            r#"{"Transform":{"transform":"90"}}"#,
            r#"{"Position":{"position":{"x":-100,"y":0}}}"#,
            r#"{"Vrr":{"vrr":true}}"#,
            r#"{"MaxBpc":{"max_bpc":10}}"#,
            r#"{"FutureAction":{"not_validated_here":true}}"#,
        ] {
            let request = request_of(&["wiri-ctl", "msg", "output", "DISPLAY1", action]);
            let IpcMessage::Output {
                output,
                action: value,
            } = request
            else {
                panic!("output request")
            };
            assert_eq!(output, "DISPLAY1");
            assert_eq!(
                value,
                serde_json::from_str::<serde_json::Value>(action).unwrap()
            );
        }
        for (words, expected) in [
            (vec!["off"], serde_json::json!("Off")),
            (vec!["on"], serde_json::json!("On")),
            (
                vec!["mode", "auto"],
                serde_json::json!({"Mode":{"mode":"auto"}}),
            ),
            (
                vec!["scale", "1.5"],
                serde_json::json!({"Scale":{"scale":1.5}}),
            ),
            (
                vec!["transform", "90"],
                serde_json::json!({"Transform":{"transform":"90"}}),
            ),
            (
                vec!["position", "-100", "0"],
                serde_json::json!({"Position":{"position":{"x":-100,"y":0}}}),
            ),
            (
                vec!["position", "auto"],
                serde_json::json!({"Position":{"position":"Auto"}}),
            ),
            (vec!["vrr", "on"], serde_json::json!({"Vrr":{"vrr":true}})),
            (
                vec!["max-bpc", "999"],
                serde_json::json!({"MaxBpc":{"max_bpc":999}}),
            ),
        ] {
            let args: Vec<&str> = ["wiri-ctl", "--print-request", "output", "DISPLAY1"]
                .into_iter()
                .chain(words)
                .collect();
            let IpcMessage::Output { action, .. } = request_of(&args) else {
                panic!("output request")
            };
            assert_eq!(action, expected);
        }
    }

    #[test]
    fn schema_hints_distinguish_matching_mismatched_and_old_daemons() {
        assert!(schema_warning(
            &serde_json::json!({"schema_version": wiri::ipc::wire::SCHEMA_VERSION})
        )
        .is_none());
        let mismatch = schema_warning(&serde_json::json!({"schema_version": "0.9"})).unwrap();
        assert!(
            mismatch.contains("mismatch") && mismatch.contains("0.9") && mismatch.contains("1.3")
        );
        let old = schema_warning(&serde_json::json!({"success": true})).unwrap();
        assert!(old.contains("pre-1.3") && old.contains("old"));
    }

    #[test]
    fn new_query_summaries_read_the_named_result_fields() {
        let response = serde_json::json!({"result": {
            "outputs": {"DISPLAY1": {"name": "DISPLAY1"}},
            "focused_output": {"name": "DISPLAY1"},
            "keyboard_layouts": {"names": ["English", "French"], "current_idx": 1},
            "window": {"id": 42, "title": "Editor"},
            "color": {"r": 1.0, "g": 0.0, "b": 0.0}
        }});
        assert!(format_query_result(&IpcMessage::Outputs, &response).contains("DISPLAY1"));
        assert!(format_query_result(&IpcMessage::FocusedOutput, &response).contains("DISPLAY1"));
        assert_eq!(
            format_query_result(&IpcMessage::KeyboardLayouts, &response),
            "  0: English\n* 1: French"
        );
        assert!(format_query_result(&IpcMessage::PickWindow, &response).contains("Editor"));
        assert!(format_query_result(&IpcMessage::PickColor, &response).contains("1.0"));
        let empty =
            serde_json::json!({"result": {"focused_output": null, "window": null, "color": null}});
        assert_eq!(
            format_query_result(&IpcMessage::FocusedOutput, &empty),
            "No output is focused."
        );
        assert_eq!(
            format_query_result(&IpcMessage::PickWindow, &empty),
            "No window was picked."
        );
        assert_eq!(
            format_query_result(&IpcMessage::PickColor, &empty),
            "No color was picked."
        );
    }

    #[test]
    fn test_a_word_in_double_quotes_is_a_quoted_word() {
        let words = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            split_quoted_words(words(&["\"2\"", "x"])),
            (words(&["2", "x"]), vec![true, false])
        );
        // Nothing quoted: nothing to say about it.
        assert_eq!(
            split_quoted_words(words(&["2", "+10%"])),
            (words(&["2", "+10%"]), vec![])
        );
        // A lone quote or an empty pair are not what niri writes for a name.
        assert_eq!(split_quoted_words(words(&["\""])), (words(&["\""]), vec![]));
        assert_eq!(
            split_quoted_words(words(&["\"\""])),
            (words(&[""]), vec![true])
        );
    }

    #[test]
    fn test_format_focus_names_the_window_and_the_workspace() {
        let resp = serde_json::json!({
            "success": true,
            "result": { "window_hwnd": 4720612, "workspace_id": 2, "monitor_id": 7 }
        });
        assert_eq!(
            format_focus(&resp),
            "Focused window 4720612 (0x4807e4) on workspace 2."
        );
        let empty = serde_json::json!({
            "success": true,
            "result": { "window_hwnd": null, "workspace_id": 1, "monitor_id": 7 }
        });
        assert_eq!(format_focus(&empty), "No window is focused (workspace 1).");
    }

    #[test]
    fn test_fold_to_ascii_replaces_table_decoration() {
        assert_eq!(
            fold_to_ascii("wiri state\n──────────"),
            "wiri state\n----------"
        );
        assert_eq!(fold_to_ascii("  ID  │ CHORD"), "  ID  | CHORD");
        assert_eq!(
            fold_to_ascii("SpawnCmd(\"start wt.exe…\")"),
            "SpawnCmd(\"start wt.exe...\")"
        );
        assert_eq!(
            fold_to_ascii("Captured window 5 → C:\\x.bmp"),
            "Captured window 5 -> C:\\x.bmp"
        );
        assert_eq!(
            fold_to_ascii("Could not find the pipe — is wiri running?"),
            "Could not find the pipe - is wiri running?"
        );
    }

    #[test]
    fn test_fold_to_ascii_leaves_ascii_alone_and_never_emits_non_ascii() {
        let plain = "  workspace-1  3 windows | 100%";
        assert_eq!(fold_to_ascii(plain), plain);
        // A title the folding table does not know becomes '?', not mojibake.
        let folded = fold_to_ascii("Caf\u{e9} \u{1f600} ──");
        assert!(folded.is_ascii(), "{:?}", folded);
        assert_eq!(folded, "Caf? ? --");
    }
}
