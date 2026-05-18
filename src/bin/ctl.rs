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

use windows::Win32::System::Pipes::WaitNamedPipeW;
use windows::core::PCWSTR;

use wiri::ipc::{IpcMessage, PIPE_PATH};

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
    ResizeWindow { hwnd: isize, width: u32, height: u32 },

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

    /// Subscribe to events (stream mode)
    Events {
        /// Event types to subscribe to (comma-separated)
        #[arg(short, long, default_value = "*")]
        types: String,
    },

    /// Move the focused window to the given workspace
    MoveWindowToWorkspace { workspace_id: i32 },

    /// Move the focused window to the next monitor (direction: left | right)
    MoveWindowToMonitor { direction: String },

    /// Center the focused column on screen
    CenterColumn,

    /// Set the focused column width preset (1/3, 1/2, 2/3, full, cycle)
    SetColumnWidth { preset: String },

    /// Resize the focused column by ±N pixels
    ResizeColumn { delta_px: i32 },

    /// Focus the next workspace
    FocusWorkspaceNext,

    /// Focus the previous workspace
    FocusWorkspacePrevious,

    /// Spawn a command (e.g. `wiri-ctl spawn 'wt.exe'`)
    Spawn { command: String },

    /// Focus the previously focused window (alt-tab through MRU history)
    FocusPrevious,

    /// Toggle always-on-top for the focused window
    ToggleAlwaysOnTop,

    /// Focus a named workspace from config (e.g. `wiri-ctl focus-workspace-named main`)
    FocusWorkspaceNamed { name: String },

    /// Set auto-tile threshold (window count above which to auto-grid). 0 to disable.
    SetAutoTile { threshold: usize },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // SubscribeEvents has bespoke streaming behaviour — handle it before the
    // single-request/response path below.
    if let Commands::Events { types } = &cli.command {
        return stream_events(types, cli.timeout, cli.json);
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
        Commands::Events { .. } => unreachable!("handled above"),
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
        Commands::Spawn { command } => IpcMessage::SpawnCommand { command },
        Commands::FocusPrevious => IpcMessage::FocusPrevious,
        Commands::ToggleAlwaysOnTop => IpcMessage::ToggleAlwaysOnTop,
        Commands::FocusWorkspaceNamed { name } => IpcMessage::FocusWorkspaceNamed { name },
        Commands::SetAutoTile { threshold } => IpcMessage::SetAutoTileThreshold {
            threshold: if threshold == 0 { None } else { Some(threshold) },
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
// Human-formatted output
// ---------------------------------------------------------------------------

/// Pretty-print the JSON response from the daemon based on the request kind.
/// Falls back to compact JSON for messages without a bespoke formatter.
fn print_human_response(req: &IpcMessage, resp: &serde_json::Value) {
    // First detect a daemon-side error and surface it cleanly.
    if let Some(false) = resp.get("success").and_then(|v| v.as_bool()) {
        let err = resp.get("error").and_then(|v| v.as_str()).unwrap_or("unknown error");
        eprintln!("error: {}", err);
        std::process::exit(1);
    }

    match req {
        IpcMessage::GetState => print_state(resp),
        IpcMessage::WindowList => print_window_list(resp),
        IpcMessage::ReloadConfig => println!("Configuration reloaded."),
        IpcMessage::Quit => println!("Shutdown request sent to wiri."),
        IpcMessage::SwitchWorkspace { id } => {
            println!("Switched to workspace {}.", id);
        }
        IpcMessage::FocusWindow { window_hwnd } => {
            println!("Focused window {} (0x{:x}).", window_hwnd, *window_hwnd);
        }
        IpcMessage::CloseWindow { window_hwnd } => {
            println!("Close request sent to window {} (0x{:x}).", window_hwnd, *window_hwnd);
        }
        IpcMessage::SpawnCommand { command } => {
            let pid = resp.get("pid").and_then(|v| v.as_u64());
            match pid {
                Some(p) => println!("Spawned '{}'  (pid: {}).", command, p),
                None => println!("Spawned '{}'.", command),
            }
        }
        IpcMessage::WindowFloat { window_hwnd } => {
            println!("Window {} is now floating.", window_hwnd);
        }
        IpcMessage::WindowUnfloat { window_hwnd } => {
            println!("Window {} is now tiled.", window_hwnd);
        }
        IpcMessage::TileRequest { window_hwnd, .. } => {
            println!("Tile request queued for window {}.", window_hwnd);
        }
        IpcMessage::WindowMoveToWorkspace { window_hwnd, workspace_id } => {
            println!("Moved window {} to workspace {}.", window_hwnd, workspace_id);
        }
        _ => {
            // Generic: report success with a hint when present.
            if let Some(note) = resp.get("note").and_then(|v| v.as_str()) {
                println!("ok ({})", note);
            } else {
                println!("ok");
            }
        }
    }
}

/// Format the response to `GetState` as a multi-line summary block.
fn print_state(resp: &serde_json::Value) {
    let result = resp.get("result").cloned().unwrap_or(serde_json::json!({}));
    let windows = result.get("windows").and_then(|w| w.as_array()).cloned().unwrap_or_default();
    let workspaces = result.get("workspaces").and_then(|w| w.as_array()).cloned().unwrap_or_default();
    let active = result.get("active_workspace").and_then(|v| v.as_str()).unwrap_or("<none>");

    let total_windows = windows.len();
    let total_workspaces = workspaces.len();

    println!("wiri state");
    println!("──────────");
    println!("  monitors / workspaces : {}", total_workspaces);
    println!("  managed windows       : {}", total_windows);
    println!("  active workspace      : {}", active);

    if !workspaces.is_empty() {
        println!();
        println!("  workspace             windows");
        println!("  ────────────────────  ───────");
        for ws in &workspaces {
            let name = ws.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let count = ws.get("window_count").and_then(|v| v.as_u64()).unwrap_or(0);
            println!("  {:<20}  {:>7}", truncate(name, 20), count);
        }
    }
}

/// Format the response to `WindowList` as a Hwnd|PID|Class|Title table.
fn print_window_list(resp: &serde_json::Value) {
    let result = resp.get("result").and_then(|r| r.as_array()).cloned().unwrap_or_default();
    if result.is_empty() {
        println!("No managed windows.");
        return;
    }

    // Auto-size title column from terminal width, fall back to 60 chars.
    let term_cols = terminal_cols().unwrap_or(120) as usize;
    let prefix_cols = 12 + 8 + 22 + 6 + 4; // hwnd | pid | class | ws | spaces
    let title_w = term_cols.saturating_sub(prefix_cols).max(20);

    println!(
        "  {:>10}  {:>6}  {:<20}  {:<3}  {}",
        "HWND", "PID", "CLASS", "WS", "TITLE"
    );
    println!(
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

        println!(
            "  {:>10}  {:>6}  {:<20}  {:<3}  {}",
            hwnd,
            pid,
            truncate(class, 20),
            ws,
            truncate(title, title_w)
        );
    }
    println!();
    println!("  {} window{}", result.len(), if result.len() == 1 { "" } else { "s" });
}

// ---------------------------------------------------------------------------
// Event streaming
// ---------------------------------------------------------------------------

/// Subscribe to the daemon's event broadcast channel and print events as they
/// arrive.  Each event is printed as a single line, prefixed with a wall-clock
/// timestamp.  Pass `--json` to dump the raw event JSON instead.
fn stream_events(types: &str, timeout_secs: u64, json: bool) -> Result<()> {
    let message = IpcMessage::SubscribeEvents {
        event_types: types.split(',').map(|s| s.trim().to_string()).collect(),
    };

    let pipe_wide: Vec<u16> = OsStr::new(PIPE_PATH)
        .encode_wide()
        .chain(std::iter::once(0u16))
        .collect();
    let timeout_ms: u32 = timeout_secs.saturating_mul(1000).min(u32::MAX as u64) as u32;

    let wait_ok =
        unsafe { WaitNamedPipeW(PCWSTR::from_raw(pipe_wide.as_ptr()), timeout_ms) };
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
        .map_err(|e| anyhow::anyhow!(
            "Could not open wiri IPC pipe at {} — is wiri running?  ({})",
            PIPE_PATH, e
        ))?;

    let data = serde_json::to_vec(&message)?;
    file.write_all(&data)?;
    file.flush()?;

    eprintln!("Subscribed to events ({}). Press Ctrl+C to exit.", types);

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
                if json {
                    println!("{}", serde_json::to_string(&value)?);
                } else {
                    println!("{}", format_event(&value));
                }
            }
            Err(e) => {
                eprintln!("event stream closed: {}", e);
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

    let timeout_ms: u32 = timeout_secs
        .saturating_mul(1000)
        .min(u32::MAX as u64) as u32;

    let wait_ok = unsafe {
        WaitNamedPipeW(PCWSTR::from_raw(pipe_wide.as_ptr()), timeout_ms)
    };

    if !wait_ok.as_bool() {
        let os_err = std::io::Error::last_os_error();
        const ERROR_SEM_TIMEOUT: i32 = 121;
        const ERROR_FILE_NOT_FOUND: i32 = 2;
        if os_err.raw_os_error() == Some(ERROR_SEM_TIMEOUT) {
            anyhow::bail!(
                "wiri busy: pipe wait timed out after {}s at {}.\n\
                 Try increasing --timeout, or restart wiri if it is unresponsive.",
                timeout_secs, PIPE_PATH
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
                PIPE_PATH, os_err
            );
        }
    }

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(PIPE_PATH)
        .map_err(|e| anyhow::anyhow!(
            "Could not open wiri IPC pipe at {} — is wiri running?  ({})",
            PIPE_PATH, e
        ))?;

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
