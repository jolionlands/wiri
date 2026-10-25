//! wiri-ctl — command-line client for the wiri window manager.
//!
//! Sends IpcMessage requests over the named pipe at `\\.\pipe\wiri_control`
//! and pretty-prints the response.  Pass `--json` to any subcommand for the
//! raw JSON body (useful for scripting).

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::System::Pipes::WaitNamedPipeW;

use wiri::ipc::{IpcMessage, PIPE_PATH};

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
    command: Commands,

    /// Timeout in seconds for IPC connection
    #[arg(short, long, default_value = "5", global = true)]
    timeout: u64,

    /// Emit the raw JSON response instead of a formatted summary.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Get the current window manager state
    State,

    /// List all managed windows
    Windows,

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
    Events {
        /// Event types to subscribe to (comma-separated, legacy alias for --filter).
        /// Kept for backwards compatibility; prefer --filter.
        #[arg(short, long, default_value = "*", hide = true)]
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

    // SubscribeEvents has bespoke streaming behaviour — handle it before the
    // single-request/response path below.
    if let Commands::Events {
        types,
        stream,
        filter,
    } = &cli.command
    {
        // --filter takes precedence over the legacy --types flag.
        let effective_types = filter.as_deref().unwrap_or(types.as_str());
        // --stream or --json both select raw newline-delimited JSON output;
        // --stream is the preferred flag for documentation purposes.
        let raw_output = *stream || cli.json;
        return stream_events(effective_types, cli.timeout, raw_output);
    }

    // validate-config is a pure-local operation — no IPC, no running daemon
    // required — so handle it before we try to dial the named pipe.
    if let Commands::ValidateConfig { path } = &cli.command {
        return validate_config_subcommand(path.as_deref(), cli.json);
    }

    let message = match cli.command {
        Commands::State => IpcMessage::GetState,
        Commands::Windows => IpcMessage::WindowList,
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
        Commands::Events { .. } => unreachable!("events handled above before IPC dial"),
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
    };

    let response = send_ipc_message(&message, cli.timeout)?;

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }

    print_human_response(&message, &response);
    Ok(())
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
                path_str, error_count, warning_count
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
                window_hwnd, *window_hwnd
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
                window_hwnd, workspace_id
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
        "HWND", "PID", "CLASS", "WS", "TITLE"
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
        "F", "BOUNDS (x,y wxh)", "DPI", "WS", "WIN", "ID", "WORK AREA (x,y wxh)", "NAME"
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
        "ID", "CHORD", "MODS", "VK", "ACTION"
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
fn stream_events(types: &str, timeout_secs: u64, raw: bool) -> Result<()> {
    // Parse the effective event-types list.  Empty string or "*" means all.
    let event_types: Vec<String> = if types.is_empty() || types == "*" {
        vec![]
    } else {
        types
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };

    let message = IpcMessage::SubscribeEvents {
        event_types: event_types.clone(),
    };

    let pipe_wide: Vec<u16> = OsStr::new(PIPE_PATH)
        .encode_wide()
        .chain(std::iter::once(0u16))
        .collect();
    let timeout_ms: u32 = timeout_secs.saturating_mul(1000).min(u32::MAX as u64) as u32;

    let wait_ok = unsafe { WaitNamedPipeW(PCWSTR::from_raw(pipe_wide.as_ptr()), timeout_ms) };
    if !wait_ok.as_bool() {
        let os_err = std::io::Error::last_os_error();
        anyhow::bail!(
            "Could not connect to wiri IPC pipe at {} — is wiri running?  ({})",
            PIPE_PATH,
            os_err
        );
    }

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(PIPE_PATH)
        .map_err(|e| {
            anyhow::anyhow!(
                "Could not open wiri IPC pipe at {} — is wiri running?  ({})",
                PIPE_PATH,
                e
            )
        })?;

    let data = serde_json::to_vec(&message)?;
    file.write_all(&data)?;
    file.flush()?;

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
        filter_desc, mode_desc
    );

    // The server streams length-implicit JSON objects back-to-back; serde_json
    // tolerates concatenated values in a Deserializer stream.
    let de = serde_json::Deserializer::from_reader(file).into_iter::<serde_json::Value>();
    for event in de {
        match event {
            Ok(value) => {
                // First message is the subscribe ack: skip it silently.
                if value.get("subscription_id").is_some() {
                    continue;
                }
                if raw {
                    // Emit compact JSON followed by a newline.
                    // Do NOT pretty-print: third-party consumers (jq, etc.)
                    // expect one object per line.
                    println!("{}", serde_json::to_string(&value)?);
                } else {
                    outln!("{}", format_event(&value));
                }
            }
            Err(e) => {
                errln!("event stream closed: {}", e);
                break;
            }
        }
    }
    Ok(())
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

fn send_ipc_message(message: &IpcMessage, timeout_secs: u64) -> Result<serde_json::Value> {
    let pipe_wide: Vec<u16> = OsStr::new(PIPE_PATH)
        .encode_wide()
        .chain(std::iter::once(0u16))
        .collect();

    let timeout_ms: u32 = timeout_secs.saturating_mul(1000).min(u32::MAX as u64) as u32;

    let wait_ok = unsafe { WaitNamedPipeW(PCWSTR::from_raw(pipe_wide.as_ptr()), timeout_ms) };

    if !wait_ok.as_bool() {
        let os_err = std::io::Error::last_os_error();
        const ERROR_SEM_TIMEOUT: i32 = 121;
        const ERROR_FILE_NOT_FOUND: i32 = 2;
        if os_err.raw_os_error() == Some(ERROR_SEM_TIMEOUT) {
            anyhow::bail!(
                "wiri busy: pipe wait timed out after {}s at {}.\n\
                 Try increasing --timeout, or restart wiri if it is unresponsive.",
                timeout_secs,
                PIPE_PATH
            );
        } else if os_err.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) {
            anyhow::bail!(
                "Could not find the wiri IPC pipe at {} — is wiri running?",
                PIPE_PATH
            );
        } else {
            anyhow::bail!(
                "WaitNamedPipeW failed for {}: {}\n\
                 (is wiri running?)",
                PIPE_PATH,
                os_err
            );
        }
    }

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(PIPE_PATH)
        .map_err(|e| {
            anyhow::anyhow!(
                "Could not open wiri IPC pipe at {} — is wiri running?  ({})",
                PIPE_PATH,
                e
            )
        })?;

    let data = serde_json::to_vec(message)?;
    file.write_all(&data)?;
    file.flush()?;

    let mut buf = Vec::new();
    match file.read_to_end(&mut buf) {
        Ok(n) if n > 0 => {
            let value: serde_json::Value = serde_json::from_slice(&buf[..n])?;
            Ok(value)
        }
        _ => Ok(serde_json::json!({
            "success": true,
            "note": "no response data"
        })),
    }
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
    use super::{fold_to_ascii, format_focus, split_quoted_words};

    #[test]
    fn test_a_word_in_double_quotes_is_a_quoted_word() {
        let words = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            split_quoted_words(words(&["\"2\"", "x"])),
            (words(&["2", "x"]), vec![true, false])
        );
        // Nothing quoted: nothing to say about it.
        assert_eq!(split_quoted_words(words(&["2", "+10%"])), (words(&["2", "+10%"]), vec![]));
        // A lone quote or an empty pair are not what niri writes for a name.
        assert_eq!(split_quoted_words(words(&["\""])), (words(&["\""]), vec![]));
        assert_eq!(split_quoted_words(words(&["\"\""])), (words(&[""]), vec![true]));
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
        assert_eq!(fold_to_ascii("wiri state\n──────────"), "wiri state\n----------");
        assert_eq!(fold_to_ascii("  ID  │ CHORD"), "  ID  | CHORD");
        assert_eq!(fold_to_ascii("SpawnCmd(\"start wt.exe…\")"), "SpawnCmd(\"start wt.exe...\")");
        assert_eq!(fold_to_ascii("Captured window 5 → C:\\x.bmp"), "Captured window 5 -> C:\\x.bmp");
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
