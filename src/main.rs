// Request to Agent B (resources/): default_config.kdl should add example blocks
// for `spawn-at-startup`, `mod-key`, and `strip-frame` so first-run users
// discover these options.

// wiri is a background daemon.  Built as a console program it opened a console
// window every time it started - which wiri then tiled like any other window.
// Release builds use the GUI subsystem instead; the log goes to
// %LOCALAPPDATA%\wiri\wiri.log (see `wiri::logging`) and, when started from a
// shell, to that shell's console.  Debug builds and the test harness keep the
// console so `cargo run` / `cargo test` output stays visible.
#![cfg_attr(not(any(debug_assertions, test)), windows_subsystem = "windows")]

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::signal;
use tokio::time::Duration;
use tracing::{debug, error, info, warn, Level};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use wiri::backend::message_loop::MessageLoop;
use wiri::backend::{Backend, BackendEvent, BackendHandle, MonitorInfo as BackendMonitorInfo};
use wiri::config::{loader::ConfigLoader, Config, ReloadOutcome, ValidationIssue};
use wiri::hooks::{SystemIntegration, TrayAction};
use wiri::input::{
    apply_mouse_config, start_mouse_hook, stop_mouse_hook, MouseFocusConfig, MouseTracker,
};
use wiri::ipc::{IpcEvent, IpcServer, WindowInfoIpc};
use wiri::layout::{LayoutConfig, TilingEngine};
use wiri::overlay::{
    BindingsCheatsheet, OverviewBanner, StatusBar, StatusBarContent, WorkspaceIndicator,
};
use wiri::utils::WindowId;

use parking_lot::RwLock;

/// Build version string. `build.rs` sets `WIRI_GIT_HASH` to the short commit
/// hash when available; otherwise we just render the crate version.
fn version_static() -> &'static str {
    use std::sync::OnceLock;
    static V: OnceLock<String> = OnceLock::new();
    V.get_or_init(|| {
        let pkg = env!("CARGO_PKG_VERSION");
        match option_env!("WIRI_GIT_HASH") {
            Some(h) if !h.is_empty() => format!("{} (commit {})", pkg, h),
            _ => pkg.to_string(),
        }
    })
    .as_str()
}

#[derive(Parser, Debug)]
#[command(
    name = "wiri",
    version = version_static(),
    about = "A scrollable-tiling window manager for Windows"
)]
struct Args {
    /// Path to the KDL config file. If unset, search $WIRI_CONFIG, %APPDATA%/wiri/config.kdl,
    /// then %USERPROFILE%/.config/wiri/config.kdl, then fall back to ./config.kdl.
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Log debug output.  When wiri is started from a console the log is also
    /// shown there (closing that console then stops wiri); it always goes to
    /// %LOCALAPPDATA%\wiri\wiri.log.
    #[arg(short, long)]
    verbose: bool,
    #[arg(long)]
    no_tray: bool,
    /// Register wiri in the Windows Run key so it starts with Windows
    #[arg(long)]
    register_autostart: bool,
    /// Remove wiri from the Windows Run key
    #[arg(long)]
    unregister_autostart: bool,
    /// Skip spawning programs listed in spawn-at-startup config entries (useful for testing)
    #[arg(long)]
    no_autostart: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Declare DPI awareness so Windows doesn't virtualize coordinates
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }

    // A GUI-subsystem process has no console.  When wiri was started from a
    // shell and the user asked for output (`-v`, `--help`, `--version`, a
    // command-line mistake), attach to that shell's console so it appears there
    // (redirected streams are left alone).  A plain `wiri` never attaches: a
    // daemon attached to a console dies when that console is closed.
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(e) => {
            wiri::logging::attach_parent_console();
            e.exit()
        }
    };
    if args.verbose || args.register_autostart || args.unregister_autostart {
        wiri::logging::attach_parent_console();
    }
    // Remember a console that only wiri is attached to (console-subsystem
    // builds) so it is never tiled.  Must run after the attach above.
    wiri::window::init_own_console();

    // // Single-instance check: warn if hotkeys may conflict
    // (RegisterHotKey will fail if another process has the same key combo)

    // Setup logging: stderr when there is one (console or redirect), and always
    // %LOCALAPPDATA%\wiri\wiri.log, rotated by size.
    let filter = if args.verbose {
        EnvFilter::from_default_env().add_directive(Level::DEBUG.into())
    } else {
        EnvFilter::from_default_env().add_directive(Level::INFO.into())
    };
    // stdout stays clean for any future machine-consumable output, so the log
    // goes to stderr (`wiri.exe 2>wiri.log` captures everything that matters).
    let stderr_layer =
        wiri::logging::stderr_is_usable().then(|| fmt::layer().with_writer(std::io::stderr));
    let file_layer = wiri::logging::default_log_path()
        .and_then(|path| wiri::logging::RotatingFile::open(path, wiri::logging::LOG_MAX_BYTES).ok())
        .map(|file| {
            fmt::layer()
                .with_ansi(false)
                .with_writer(wiri::logging::SharedLog::new(file))
        });
    tracing_subscriber::registry()
        .with(stderr_layer)
        .with(file_layer)
        .with(filter)
        .init();

    info!("╔══════════════════════════════════════════╗");
    info!("║ wiri - Scrollable Tiling WM              ║");
    info!("║ Niri-style window manager for Windows    ║");
    info!("╚══════════════════════════════════════════╝");

    // Startup probe: known GPU OEM utilities that hijack chords like
    // Ctrl+Alt+Arrows for screen rotation.  We can't take the chord back
    // from a kernel-mode driver, but we can tell the user why their
    // bindings might be silently broken.  No-op on aarch64.
    for warning in wiri::backend::hotkey_conflicts::detect() {
        warn!("{}", warning);
    }

    // Handle autostart registration flags before full startup
    if args.register_autostart || args.unregister_autostart {
        use wiri::hooks::StartupManager;
        let sm = StartupManager::new();
        if args.register_autostart {
            match sm.register() {
                Ok(()) => info!("Registered wiri for autostart."),
                Err(e) => error!("Failed to register autostart: {}", e),
            }
        } else {
            match sm.unregister() {
                Ok(()) => info!("Unregistered wiri from autostart."),
                Err(e) => error!("Failed to unregister autostart: {}", e),
            }
        }
        return Ok(());
    }

    // Single-instance check.
    //
    // The previous implementation opened the pipe for read+write and called it
    // a positive signal — but in some race-window cases the pipe handle can
    // exist briefly without a server actively reading, producing a false
    // positive. The robust check is `WaitNamedPipeW` with a 0 timeout: it
    // returns true only when a server instance is actually listening on the
    // pipe.
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::System::Pipes::WaitNamedPipeW;
        let pipe_path = wiri::ipc::pipe_path();
        let pipe_w: Vec<u16> = std::ffi::OsStr::new(&pipe_path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let exists = unsafe {
            WaitNamedPipeW(
                windows::core::PCWSTR(pipe_w.as_ptr()),
                1, // 1 ms — "essentially zero, but >0 to satisfy the API"
            )
        };
        if exists.as_bool() {
            error!(
                "Another wiri instance is already running (pipe {} has an active listener).",
                pipe_path
            );
            error!("Use `wiri-ctl quit` to stop it, or kill the process first.");
            std::process::exit(2);
        }
    }

    // Crash safety: windows on inactive workspaces are hidden, and a hidden
    // window has no taskbar button.  If a previous wiri was killed or crashed
    // while some were hidden, show them again now - before the desktop is
    // enumerated, so they are tiled like every other window - and keep
    // recording new hides from here on.
    //
    // The same record keeps where every window was before wiri first moved it, so that a
    // restart after a crash can still put the windows back where they were on `quit`.
    // Persistence goes on first: reading the old record rewrites it.
    if let Some(path) = wiri::backend::hidden::default_path() {
        wiri::backend::hidden::enable_persistence(path.clone());
        wiri::backend::hidden::restore_from_file(&path);
    }
    {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Whatever is going wrong, do not strand hidden windows.
            wiri::backend::hidden::restore_all_hidden();
            default_hook(info);
        }));
    }

    // Resolve config path: explicit --config wins, otherwise search the standard locations.
    let config_path: PathBuf = args
        .config
        .clone()
        .or_else(wiri::config::default_config_path)
        .unwrap_or_else(|| {
            // First-run: drop a default config at %APPDATA%/wiri/config.kdl so
            // the user has something to edit instead of a 'config.kdl' in CWD.
            if let Ok(appdata) = std::env::var("APPDATA") {
                let dir = PathBuf::from(&appdata).join("wiri");
                let path = dir.join("config.kdl");
                if !path.exists() {
                    if let Err(e) = std::fs::create_dir_all(&dir) {
                        warn!("Could not create {}: {}", dir.display(), e);
                    } else if let Err(e) =
                        std::fs::write(&path, include_str!("../resources/default_config.kdl"))
                    {
                        warn!(
                            "Could not write default config to {}: {}",
                            path.display(),
                            e
                        );
                    } else {
                        info!("Wrote default config to {}", path.display());
                    }
                }
                path
            } else {
                PathBuf::from("config.kdl")
            }
        });

    // A config with errors is not loaded (niri does the same): wiri starts with its defaults
    // and says why in a balloon as soon as the tray icon exists.
    let startup_decoded = wiri::config::loader::load_logged(&config_path);
    let mut pending_balloon: Option<(String, String, u32)> = None;
    // What an event-stream client that connects later is told about this load.
    let startup_warnings = startup_decoded.warning_count() as u32;
    let mut startup_error: Option<String> = None;
    let config = if startup_decoded.has_errors() {
        let summary = wiri::config::failure_summary(&config_path, &startup_decoded.issues);
        startup_error = Some(summary.clone());
        warn!("Using defaults (config error: {})", summary);
        pending_balloon = Some((
            "wiri: config error".to_string(),
            format!("{} - using the defaults.", summary),
            0,
        ));
        Config::default_config()
    } else {
        info!(
            "Config: {} ({} warnings)",
            config_path.display(),
            startup_decoded.warning_count()
        );
        startup_decoded.config
    };

    // Start config file watcher for live-reload support. The watcher publishes the result of
    // each reload (the new `Config`, or what is wrong with the file) on the broadcast channel
    // returned by `subscribe()`; the main loop pulls from that channel and acts on it.
    let mut config_loader = ConfigLoader::new(config_path.clone());
    let mut config_reload_rx = config_loader.subscribe();
    if let Err(e) = config_loader.start_watching() {
        warn!(
            "Config watcher could not start: {} (live-reload disabled)",
            e
        );
    }

    // Spawn programs listed in spawn-at-startup config entries (niri parity).
    // --no-autostart suppresses this loop (useful when testing config changes).
    if !args.no_autostart {
        for entry in &config.spawn_at_startup {
            if entry.is_empty() {
                continue;
            }
            info!("spawn-at-startup: {:?}", entry);
            // Same spawner as the `spawn` bind and the IPC request: every
            // argument kept, quoted paths with spaces, programs found on PATH.
            match wiri::hooks::spawner::spawn_argv(entry) {
                Ok(pid) => info!("spawn-at-startup: started {:?} (pid {})", entry, pid),
                Err(e) => warn!("spawn-at-startup failed for {:?}: {}", entry, e),
            }
        }
        for command in &config.spawn_sh_at_startup {
            info!("spawn-sh-at-startup: {:?}", command);
            match wiri::hooks::spawner::spawn_shell(command) {
                Ok(pid) => info!("spawn-sh-at-startup: started {:?} (pid {})", command, pid),
                Err(e) => warn!("spawn-sh-at-startup failed for {:?}: {}", command, e),
            }
        }
    }

    // Push the mouse-config snapshot into the static read by the low-level
    // wheel hook (natural_scroll + scroll_speed).
    apply_mouse_config(&config.input);

    // Lane P: niri's pointing-device options that Windows can be told about ("Enhance pointer
    // precision", button swap, two precision-touchpad values).  Only the options the config names
    // are written, the previous values are remembered, and they are restored on a graceful quit
    // or when a reload stops naming them.
    wiri::input::pointer_settings::apply_config(&config.input);

    // Convert config to layout config
    let layout_config = LayoutConfig::from_config(&config);
    let hide_method = hide_method_of(&config);
    wiri::backend::cloak::set_hide_method(hide_method);

    // Lane D: only explicit, different output settings can call a display setter.
    wiri::backend::outputs::apply(&config);
    // Initialize backend after output configuration so its first snapshot is current.
    let mut backend = Backend::new()?;
    info!("Backend initialized");

    // Show monitors
    let monitors = backend.handle().get_monitors();
    for monitor in &monitors {
        info!(
            "Display: {} {}x{} at ({}, {}) {} DPI={:.0}%",
            monitor.name,
            monitor.bounds.size.w,
            monitor.bounds.size.h,
            monitor.bounds.loc.x,
            monitor.bounds.loc.y,
            if monitor.is_primary { "[primary]" } else { "" },
            monitor.scale_factor * 100.0
        );
    }

    // Initialize tiling engine
    let engine = Arc::new(RwLock::new(TilingEngine::new(layout_config.clone())));
    let animation_wake = Arc::new(tokio::sync::Notify::new());
    engine
        .write()
        .set_animation_notifier(Arc::clone(&animation_wake));
    engine.write().install_recent_windows(
        wiri::overlay::recent_windows::DwmRecentWindows::new(),
        Arc::downgrade(&engine),
    );

    let overview_host = wiri::overlay::DwmThumbnailOverview::new();
    {
        let weak_engine = Arc::downgrade(&engine);
        let overview_backend = backend.handle();
        overview_host.set_handler(Arc::new(move |command| {
            if let Some(engine) = weak_engine.upgrade() {
                engine.write().overview_command(command, &overview_backend);
            }
        }));
    }

    // Install a LayoutRequest channel between the engine and a backend drainer.
    // The engine can opt-in to dispatching ops over this channel via
    // `engine.dispatch_request(...)`. Direct calls still work today; the channel
    // is set up so future async/throttled paths can use it without restructuring.
    let (layout_req_tx, mut layout_req_rx) =
        tokio::sync::mpsc::unbounded_channel::<wiri::backend::LayoutRequest>();
    {
        let drain_handle = backend.handle();
        tokio::spawn(async move {
            use wiri::backend::BackendApi;
            while let Some(req) = layout_req_rx.recv().await {
                if let Err(e) = req.apply(&drain_handle as &dyn BackendApi) {
                    debug!("LayoutRequest::apply failed: {:?}", e);
                }
            }
        });
    }

    {
        let mut engine = engine.write();
        // Lane D: screenshot settings are shared by hotkeys, tray and IPC captures.
        wiri::screenshot::configure(&config);
        // Apply window rules from config
        engine.set_full_config(config.clone());
        engine.set_hide_method(hide_method);
        engine.set_layout_request_channel(layout_req_tx);

        // The overlay layer draws niri's focus ring, border and shadow around the windows.
        // Windows are created on first use on the existing hotkey message pump.
        // No decoration thread, frame clock or recurring wake-up exists at rest.
        let decor_layer: std::sync::Arc<dyn wiri::overlay::decor::DecorSink> =
            std::sync::Arc::new(wiri::overlay::decor::layer::DecorLayer::new());
        wiri::overlay::decor::install_global(decor_layer.clone());
        engine.install_decor_sink(decor_layer);

        // Opt into DWM thumbnail-based overview: shrunken live previews of
        // every window are composited by DWM instead of resizing the real
        // HWNDs. Falls back gracefully if DWM is unavailable.
        engine.install_thumbnail_overview(overview_host);

        // Lane D: case-insensitive identity matching, no named-block index fallback.
        wiri::backend::output_runtime::sync(&config, &backend.handle(), &mut engine);
    }

    // Initialize hotkey manager
    // HotkeyManager is no longer needed — message_loop handles registration

    // Start message loop with hotkey handling
    let message_loop = MessageLoop::new()?;
    let (hotkey_quit_tx, hotkey_quit_rx) = std::sync::mpsc::channel::<()>();
    message_loop.set_quit_sender(hotkey_quit_tx);
    message_loop.start_hotkey_loop(engine.clone(), backend.handle());
    info!("Hotkeys registered and message loop started");

    // Start low-level mouse hook for interactive move/resize
    start_mouse_hook(engine.clone(), backend.handle()).map_err(|e| anyhow::anyhow!("{}", e))?;
    info!("Mouse hook installed");

    // Tile existing windows
    {
        let backend_handle = backend.handle();
        let windows: Vec<_> = backend_handle
            .get_windows()
            .into_iter()
            .filter(|w| should_tile_window(w))
            .collect();

        info!("Tiling {} windows...", windows.len());
        for window in &windows {
            info!("  + {} ({})", window.title, window.class_name);
        }
        // One write of the record for the whole pass, not one per window.
        let record = wiri::backend::hidden::batch();
        for window in windows {
            // How narrow can the window be made?  Asked before the engine lock is taken: the
            // window answers on its own thread.
            let window_id = WindowId::new(window.hwnd);
            let min_width = backend_handle.min_track_width(window.hwnd);
            // Initial enumeration: at_startup=true so window rules with `at-startup` matchers fire.
            let mut eng = engine.write();
            eng.add_window_at_startup(window, &backend_handle);
            eng.note_min_width_answer(window_id, min_width);
        }
        drop(record);
        {
            let mut eng = engine.write();
            // Focus follows what the user is looking at, not whichever window
            // the enumeration happened to list last.
            let foreground =
                unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
            let hwnd = foreground.0 as isize;
            eng.seed_native_foreground(
                hwnd,
                wiri::layout::engine::root_owner_of(hwnd),
                // SAFETY: GetTickCount64 reads uptime and accepts no pointers or handles.
                unsafe { windows::Win32::System::SystemInformation::GetTickCount64() },
                &backend_handle,
            );
            // Lane D: an explicit startup focus takes precedence over enumeration/OS focus.
            wiri::backend::output_runtime::focus_at_startup(&config, &backend_handle, &mut eng);
            eng.reveal_focused();
            eng.apply_all(&backend_handle);
        }
    }

    // System integration (tray icon, autostart, spawner)
    let mut system = SystemIntegration::new(backend.handle()).await;

    // Log autostart registration status at startup
    {
        use wiri::hooks::StartupManager;
        let sm = StartupManager::new();
        if sm.is_registered() {
            info!(
                "Autostart: registered ({})",
                sm.get_registered_path().unwrap_or_default()
            );
        } else {
            info!("Autostart: not registered (use --register-autostart to enable)");
        }
    }
    // Mod+Enter spawn target is now resolved from config by
    // `message_loop::resolve_terminal_command` — the first `spawn` bind or
    // spawn-at-startup entry wins, with `cmd.exe` as the final fallback.

    // Start IPC server — wire a shutdown channel so IPC Quit triggers graceful exit.
    //
    // Item 2 (Option A): the server now builds the named pipe's
    // SECURITY_ATTRIBUTES synchronously inside `IpcServer::run` *before* any
    // `.await`, hands off only the prebuilt `NamedPipeServer` handles to the
    // accept loop, and never holds a `SECURITY_ATTRIBUTES` (or its
    // `*mut c_void` ACL pointer) across await points.  The future is therefore
    // `Send`, so plain `tokio::spawn` on the multi-thread runtime is enough —
    // no dedicated single-thread runtime required.
    let mut ipc_server = IpcServer::new();
    ipc_server.set_engine(engine.clone());
    ipc_server.set_backend(backend.handle());
    let (ipc_shutdown_tx, ipc_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    ipc_server.set_shutdown_sender(ipc_shutdown_tx);
    let (ipc_reload_tx, mut ipc_reload_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    ipc_server.set_reload_sender(ipc_reload_tx);
    let (system_reload_tx, mut system_reload_rx) = tokio::sync::mpsc::unbounded_channel();
    wiri::input::system_actions::set_reload_sender(system_reload_tx);
    ipc_server.set_config_path(config_path.clone());
    let ipc_server = Arc::new(ipc_server);
    let ipc_server_clone = ipc_server.clone();
    tokio::spawn(async move {
        if let Err(e) = ipc_server_clone.run().await {
            error!("IPC server error: {}", e);
        }
    });
    info!("IPC server started on {}", wiri::ipc::pipe_path());
    // A client that subscribes to the event stream is told how the config loaded at startup
    // (nobody is subscribed yet: the server only keeps it for the replay).
    ipc_server.broadcast_event(IpcEvent::ConfigLoaded {
        config_path: config_path.display().to_string(),
        failed: startup_error.is_some(),
        error: startup_error.clone(),
        warnings: startup_warnings,
    });
    // Tells event-stream subscribers what changed, once per burst of changes, and only while
    // somebody is subscribed (no timer: it sleeps until `wiri::ipc::notify_state_changed`).
    let ipc_state_task = ipc_server.clone();
    tokio::spawn(async move {
        ipc_state_task.run_state_task().await;
    });

    // Take event receiver before spawning event handler
    let mut event_rx = backend.take_event_rx();
    let handle = backend.handle();
    let engine_clone = engine.clone();

    // Backend event handler
    let handle_evt = handle.clone();
    let ipc_server_events = ipc_server.clone();
    tokio::spawn(async move {
        loop {
            while let Ok(event) = event_rx.try_recv() {
                handle_backend_event(event, &handle_evt, &engine_clone, &ipc_server_events);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });

    // Periodic reconciliation catches windows that missed CREATE/SHOW events
    // (common with launchers) and reasserts workspace visibility if the shell
    // re-shows a hidden inactive-workspace HWND behind wiri's back.
    let handle_reconcile = handle.clone();
    let engine_reconcile = engine.clone();
    let ipc_server_reconcile = ipc_server.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(750));
        loop {
            interval.tick().await;
            reconcile_visible_windows(&handle_reconcile, &engine_reconcile, &ipc_server_reconcile);
        }
    });

    // Mouse focus tracking. `focus_follows_mouse` is a flat boolean on the
    // InputConfig; user toggles it in their KDL config under `input { ... }`.
    let mouse_config = MouseFocusConfig {
        focus_follows_mouse: config.input.focus_follows_mouse,
        warp_on_focus: config.input.warp_on_focus,
        focus_delay_ms: config.input.focus_delay_ms,
        raise_on_click: true,
    };
    let mouse_tracker = Arc::new(RwLock::new(MouseTracker::new(mouse_config)));
    // Focus-follows-mouse and warp are driven by the input message thread's
    // hook notifications and one-shot delay timer, never by cursor polling.

    // Workspace indicator overlay — shown whenever the active workspace changes.
    let overlay = Arc::new(WorkspaceIndicator::new());
    // Spawn the Win32 message-pump thread; drop the JoinHandle (overlay lives until exit).
    let _ = overlay.clone().spawn();

    // Overview-mode banner — shown while the user is in overview ("Space or
    // Esc to exit").  Engine reaches it via the
    // `overlay::overview_banner::global_*` helpers; we install the singleton
    // here once so those calls actually do something.  Dropping the
    // JoinHandle is intentional — the thread lives until process exit.
    let overview_banner = Arc::new(OverviewBanner::new());
    let _ = overview_banner.clone().spawn();
    wiri::overlay::overview_banner::install_global(overview_banner.clone());

    // Key-bindings cheatsheet overlay — toggled by the ShowKeyBindings action
    // (default chord: prefix+Shift+/ = prefix+?).  Installed as a global so
    // execute_action can reach it without extra plumbing.
    let cheatsheet = Arc::new(BindingsCheatsheet::new());
    wiri::overlay::bindings_cheatsheet::set_global_cheatsheet(cheatsheet.clone());
    info!("Key-bindings cheatsheet overlay installed");
    // niri shows the sheet at startup unless `hotkey-overlay { skip-at-startup }` asks for quiet.
    // `show_hotkey_overlay` lists the bindings Windows actually registered and honours
    // `hotkey-overlay { hide-not-bound }`.
    if config.hotkey_overlay.skip_at_startup {
        info!("Hotkey overlay: skipped at startup (hotkey-overlay {{ skip-at-startup }})");
    } else {
        let rows = wiri::backend::message_loop::show_hotkey_overlay(&engine);
        if rows == 0 {
            warn!("Hotkey overlay: no bindings to show at startup");
        } else {
            info!("Hotkey overlay: shown at startup with {} rows", rows);
        }
    }

    // Status bar overlay — opt-in via `status-bar true` in `layout { }`.
    // TODO(audit): when status_bar is enabled, reserve top 32px from each
    // monitor's work_area so tiles don't overlap the bar.
    if config.layout.status_bar {
        let status_bar = Arc::new(StatusBar::new());
        let _ = status_bar.clone().spawn();

        // Updater task: runs every 1 second; pushes workspace label + focused
        // window title + local time into the bar.
        let sb_engine = engine.clone();
        let sb_bar = status_bar.clone();
        tokio::spawn(async move {
            loop {
                // Workspace label: look up the active workspace on the focused monitor.
                let (workspace_label, focused_title) = {
                    let eng = sb_engine.read();

                    let ws_label = eng
                        .monitors()
                        .focused_id()
                        .and_then(|oid| {
                            let ws_id = eng.monitors().get(&oid)?.active_workspace_id();
                            Some(eng.workspace_name_on(oid, ws_id).unwrap_or_else(|| {
                                format!("[{}]", eng.workspace_number_on(oid, ws_id))
                            }))
                        })
                        .unwrap_or_default();

                    // Focused window title from the focused monitor's focus_window.
                    let title = eng
                        .monitors()
                        .focused()
                        .and_then(|m| m.focus_window)
                        .and_then(|wid| {
                            eng.tiled_windows().get(&wid).map(|info| info.title.clone())
                        })
                        .unwrap_or_default();

                    (ws_label, title)
                };

                // Local time via GetLocalTime (windows-rs 0.58: no args, returns SYSTEMTIME).
                let time_str = unsafe {
                    let st = windows::Win32::System::SystemInformation::GetLocalTime();
                    format!("{:02}:{:02}", st.wHour, st.wMinute)
                };

                sb_bar.update(StatusBarContent {
                    workspace_label,
                    focused_title,
                    time_str,
                });

                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
    }

    print_help();

    // Animations have their own frame worker; idle animations need no polling.
    let animation_worker = {
        let weak_engine = Arc::downgrade(&engine);
        let wake = Arc::clone(&animation_wake);
        let animation_handle = handle.clone();
        tokio::spawn(async move {
            let mut last_tick = std::time::Instant::now();
            loop {
                // Register before probing so an animation starting during the
                // transition to idle cannot leave the worker asleep.
                let notified = wake.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let active = {
                    let Some(engine) = weak_engine.upgrade() else {
                        break;
                    };
                    let active = engine.read().has_active_animations();
                    active
                };
                if !active {
                    notified.await;
                    last_tick = std::time::Instant::now();
                    continue;
                }

                tokio::time::sleep(Duration::from_millis(16)).await;
                let now = std::time::Instant::now();
                let delta_ms = now
                    .duration_since(last_tick)
                    .as_millis()
                    .min(u32::MAX as u128) as u32;
                last_tick = now;
                let Some(engine) = weak_engine.upgrade() else {
                    break;
                };
                let _ = engine
                    .write()
                    .tick_animations_with_backend(delta_ms, &animation_handle);
            }
        })
    };
    let mut ipc_shutdown_rx = ipc_shutdown_rx;
    let mut reload = ReloadCtx {
        config_path: config_path.clone(),
        engine: &engine,
        handle: &handle,
        message_loop: &message_loop,
        mouse_tracker: &mouse_tracker,
        ipc_server: &ipc_server,
    };

    // Main loop: process tray actions + wait for shutdown signal
    'main: loop {
        // Workspace-indicator: detect workspace switches and show the overlay.
        // Polling is cheap — active_workspace is a plain i32 field; no Win32 calls.
        {
            static LAST_WS: parking_lot::Mutex<Option<i32>> = parking_lot::const_mutex(None);
            let current: Option<i32> = engine
                .read()
                .monitors()
                .focused()
                .map(|m| m.active_workspace_id());
            let mut last = LAST_WS.lock();
            if current != *last {
                if let Some(ws_id) = current {
                    // The workspace's name when it has one, otherwise "Workspace N"
                    // with N its position from the top of the monitor.
                    let label = {
                        let eng = engine.read();
                        match eng.monitors().focused_id() {
                            Some(oid) => eng.workspace_label(oid, ws_id),
                            None => format!("Workspace {}", ws_id + 1),
                        }
                    };
                    // Show the overlay centred over the focused monitor so it
                    // lands on the screen the user is looking at — falls back
                    // to the primary monitor if no focused monitor is set.
                    let focused_bounds =
                        engine
                            .read()
                            .monitors()
                            .focused()
                            .map(|m| wiri::overlay::MonitorBounds {
                                x: m.bounds.loc.x,
                                y: m.bounds.loc.y,
                                w: m.bounds.size.w as i32,
                                h: m.bounds.size.h as i32,
                            });
                    match focused_bounds {
                        Some(b) => overlay.show_at(&label, b),
                        None => overlay.show(&label),
                    }
                    // Broadcast workspace switch to IPC subscribers.
                    // OutputId wraps a u64 name-hash; expose it via .as_u64().
                    let focused = {
                        let eng = engine.read();
                        eng.monitors()
                            .focused_id()
                            .map(|oid| (oid, eng.workspace_number_on(oid, ws_id)))
                    };
                    if let Some((focused_oid, number)) = focused {
                        ipc_server.broadcast_event(IpcEvent::WorkspaceSwitched {
                            workspace_id: number,
                            monitor_id: focused_oid.as_u64(),
                        });
                    }
                }
                *last = current;
            }
        }

        // Retain the existing tray/reload cadence; animation timing no longer
        // depends on this loop and sleeps entirely when no timeline is active.
        tokio::time::sleep(Duration::from_millis(33)).await;

        // The config error that was found at startup is shown as soon as the tray icon is there.
        if let Some((title, text, attempts)) = pending_balloon.take() {
            if system.tray_icon().try_balloon(&title, &balloon_text(&text)) {
                info!("Showed the config error balloon");
            } else if attempts < 500 {
                pending_balloon = Some((title, text, attempts + 1));
            } else {
                warn!("The tray icon never came up; the config error balloon was not shown");
            }
        }

        // Handle file-watcher config reloads
        match config_reload_rx.try_recv() {
            Ok(outcome) => reload.outcome(outcome, &system),
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                warn!(
                    "Config reload channel lagged {} events — reloading from disk",
                    n
                );
                reload.from_disk(&system);
            }
            Err(_) => {} // Empty or closed — normal
        }

        // Handle IPC-initiated config reloads.
        loop {
            match ipc_reload_rx.try_recv() {
                Ok(()) => {
                    info!("IPC: Reloading config from disk");
                    reload.from_disk(&system);
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            }
        }

        // System load-config-file: switch the active path and its watcher only after
        // a successful load. A missing path means reload the currently active file.
        while let Ok(path) = system_reload_rx.try_recv() {
            let previous = reload.config_path.clone();
            if let Some(path) = path {
                reload.config_path = path;
            }
            if reload.from_disk(&system) {
                if reload.config_path != previous {
                    ipc_server.set_config_path(reload.config_path.clone());
                    config_loader.stop_watching();
                    config_loader = ConfigLoader::new(reload.config_path.clone());
                    config_reload_rx = config_loader.subscribe();
                    if let Err(error) = config_loader.start_watching() {
                        warn!("Could not watch the selected config: {}", error);
                    }
                }
            } else {
                reload.config_path = previous;
            }
        }

        // Check for IPC-initiated graceful shutdown
        if let Ok(()) = ipc_shutdown_rx.try_recv() {
            info!("Shutting down (IPC Quit)...");
            break 'main;
        }

        if hotkey_quit_rx.try_recv().is_ok() {
            info!("Shutting down (hotkey)...");
            break 'main;
        }

        // Process tray actions
        while let Some(action) = system.poll_tray_action() {
            match action {
                TrayAction::ShowHide => {
                    // Toggle visibility of all tiled windows. We track the
                    // toggle state across iterations of the main loop so the
                    // user can repeatedly click Show/Hide.
                    static TILED_HIDDEN: parking_lot::Mutex<bool> = parking_lot::const_mutex(false);
                    let mut hidden = TILED_HIDDEN.lock();
                    *hidden = !*hidden;
                    let show = !*hidden;
                    info!("Tray: Show/Hide → {}", if show { "show" } else { "hide" });
                    let wids: Vec<_> = engine.read().tiled_windows().keys().copied().collect();
                    for wid in wids {
                        let _ = handle.show_window(wid.as_isize(), show);
                    }
                }
                TrayAction::ReloadConfig => {
                    info!("Tray: Reloading config");
                    // Tray reload bypasses the file-watcher channel (used for explicit
                    // user-driven reloads, e.g. after editing the file in a different
                    // editor) — load directly from disk and re-apply everything.
                    reload.from_disk(&system);
                }
                TrayAction::Quit => {
                    info!("Quit requested via tray icon");
                    break 'main;
                }
                TrayAction::OpenConfigDir => {
                    // Open the folder containing the active config in Explorer.
                    if let Some(dir) = reload.config_path.parent() {
                        info!("Tray: opening config dir {}", dir.display());
                        let _ = std::process::Command::new("explorer.exe")
                            .arg(dir.as_os_str())
                            .spawn();
                    }
                }
                TrayAction::About => {
                    info!(
                        "wiri {} — https://github.com/ (set repo on publish)",
                        env!("CARGO_PKG_VERSION"),
                    );
                }
                TrayAction::FocusPrevious => {
                    info!("Tray: focus previous (MRU)");
                    engine.write().focus_previous_window(&handle);
                    engine.write().apply_all(&handle);
                }
                TrayAction::Screenshot => {
                    // Screenshot is handled inline inside poll_tray_action so
                    // this arm is never reached.  Kept exhaustive so the
                    // compiler flags any new TrayAction variant we forget to
                    // route in main.
                    info!("Tray: screenshot — handled inline by SystemIntegration");
                }
                TrayAction::CaptureWindow => {
                    // Same pattern as Screenshot: SystemIntegration handles
                    // window-capture inline (continues the loop, never
                    // surfaces this variant).  Arm is here only for
                    // exhaustive-match coverage.
                    info!("Tray: window-capture — handled inline by SystemIntegration");
                }
            }
        }

        // Check if we should exit
        if tokio::time::timeout(Duration::from_millis(1), signal::ctrl_c())
            .await
            .is_ok()
        {
            info!("Shutting down (Ctrl+C)...");
            break 'main;
        }
    }

    // Stop animation writes before restoring the original window state.
    animation_worker.abort();
    let _ = animation_worker.await;

    // Lane P: give the user their own pointer settings back before anything else goes away.
    wiri::input::pointer_settings::restore();

    // Cancel and drain interactive previews before final restoration; a late
    // move cleanup must not re-tile or re-clip a window after restore_all.
    stop_mouse_hook();

    // Restore tiled windows to their original positions and styles so the
    // user doesn't lose their layout on quit.
    info!("Restoring windows...");
    engine.write().restore_all(&handle);
    // The show/move requests are posted to the windows' own threads; let them
    // be picked up before the process (and our hooks) go away.
    std::thread::sleep(Duration::from_millis(100));

    wiri::hooks::clear_backend();
    info!("Goodbye!");
    Ok(())
}

/// What reloading the config needs: the places it is applied to and the places the result is
/// reported to (the log, a tray balloon, the IPC event stream).
struct ReloadCtx<'a> {
    config_path: PathBuf,
    engine: &'a Arc<RwLock<TilingEngine>>,
    handle: &'a BackendHandle,
    message_loop: &'a MessageLoop,
    mouse_tracker: &'a Arc<RwLock<MouseTracker>>,
    ipc_server: &'a Arc<IpcServer>,
}

impl<'a> ReloadCtx<'a> {
    /// Read the file now (a reload that was asked for: the tray menu, `wiri-ctl reload-config`).
    fn from_disk(&self, system: &SystemIntegration) -> bool {
        let decoded = wiri::config::loader::load_logged(&self.config_path);
        if decoded.has_errors() {
            self.report_failure(&decoded.issues, system);
            false
        } else {
            let warnings = decoded.warning_count();
            self.apply(decoded.config, warnings);
            info!("Config reloaded successfully");
            true
        }
    }

    /// The result of a reload the file watcher did.
    fn outcome(&self, outcome: ReloadOutcome, system: &SystemIntegration) {
        match outcome {
            ReloadOutcome::Applied { config, warnings } => {
                info!(
                    "Config file changed — reloaded ({} warnings)",
                    warnings.len()
                );
                self.apply(*config, warnings.len());
            }
            ReloadOutcome::Failed { issues, .. } => self.report_failure(&issues, system),
        }
    }

    fn apply(&self, config: Config, warnings: usize) {
        apply_loaded_config(
            config,
            self.engine,
            self.handle,
            self.message_loop,
            self.mouse_tracker,
        );
        self.ipc_server.broadcast_event(IpcEvent::ConfigLoaded {
            config_path: self.config_path.display().to_string(),
            failed: false,
            error: None,
            warnings: warnings as u32,
        });
        // The rules and the workspace names of the new config may have changed windows and
        // workspaces.
        wiri::ipc::notify_state_changed();
    }

    /// The file has errors: the last good config stays, the user is told where the first
    /// error is, and the event stream says the load failed.
    fn report_failure(&self, issues: &[ValidationIssue], system: &SystemIntegration) {
        let summary = wiri::config::failure_summary(&self.config_path, issues);
        error!(
            "Config reload failed - the last good config stays in use: {}",
            summary
        );
        let text = balloon_text(&format!("{} - last good config kept.", summary));
        if !system.tray_icon().try_balloon("wiri: config error", &text) {
            warn!("The config error balloon could not be shown");
        }
        let warnings = issues
            .iter()
            .filter(|i| i.severity == wiri::config::ValidationSeverity::Warning)
            .count();
        self.ipc_server.broadcast_event(IpcEvent::ConfigLoaded {
            config_path: self.config_path.display().to_string(),
            failed: true,
            error: Some(summary),
            warnings: warnings as u32,
        });
    }
}

/// A balloon holds 255 characters; a longer message is cut with an ellipsis.
fn balloon_text(text: &str) -> String {
    const MAX: usize = 240;
    if text.chars().count() <= MAX {
        text.to_string()
    } else {
        let mut cut: String = text.chars().take(MAX - 1).collect();
        cut.push('…');
        cut
    }
}

fn apply_loaded_config(
    new_config: Config,
    engine: &Arc<RwLock<TilingEngine>>,
    handle: &BackendHandle,
    message_loop: &MessageLoop,
    mouse_tracker: &Arc<RwLock<MouseTracker>>,
) {
    wiri::screenshot::configure(&new_config);
    // Lane D: reload physical differences outside the engine lock, then refresh geometry/DPI.
    wiri::backend::outputs::apply(&new_config);
    wiri::backend::output_runtime::refresh(handle);
    let new_layout_config = LayoutConfig::from_config(&new_config);
    let mouse_config = MouseFocusConfig {
        focus_follows_mouse: new_config.input.focus_follows_mouse,
        warp_on_focus: new_config.input.warp_on_focus,
        focus_delay_ms: new_config.input.focus_delay_ms,
        raise_on_click: true,
    };

    apply_mouse_config(&new_config.input);
    // Lane P: pointing-device options follow a reload; the ones the new config no longer names
    // go back to what the OS had before wiri changed them.
    wiri::input::pointer_settings::apply_config(&new_config.input);
    mouse_tracker.write().set_config(mouse_config);
    let hide_method = hide_method_of(&new_config);
    wiri::backend::cloak::set_hide_method(hide_method);
    {
        let mut engine = engine.write();
        engine.update_config(new_layout_config);
        wiri::backend::output_runtime::sync(&new_config, handle, &mut engine);
        engine.set_full_config(new_config);
        engine.set_hide_method(hide_method);
        // The rules were replaced: the windows that are open follow them (their opacity, their
        // size limits; the `open-*` properties are for windows that open).
        engine.reevaluate_all_window_rules(handle);
        engine.apply_all(handle);
    }
    message_loop.reload_hotkeys();
}

/// `layout { hide-method }`: `hide` (the default) or `cloak`.  A value that is neither is
/// reported and ignored.
fn hide_method_of(config: &Config) -> wiri::backend::cloak::HideMethod {
    use wiri::backend::cloak::HideMethod;
    match HideMethod::parse(&config.layout.hide_method) {
        Some(method) => method,
        None => {
            warn!(
                "layout hide-method {:?} is not one of \"hide\", \"cloak\"; using \"hide\"",
                config.layout.hide_method
            );
            HideMethod::Hide
        }
    }
}

fn configured_monitor_scale(
    config: Option<&Config>,
    monitor: &BackendMonitorInfo,
    _index: usize,
) -> Option<f64> {
    let mut effective_scale = monitor.scale_factor;

    if let Some(config) = config {
        let cfg_output = wiri::backend::outputs::matching_config(config, monitor);

        if let Some(out_cfg) = cfg_output {
            if !out_cfg.enable {
                return None;
            }
            if out_cfg.scale != 0.0 {
                effective_scale = out_cfg.scale;
            }
        }
    }

    Some(effective_scale)
}

fn sync_monitors_from_backend(
    handle: &BackendHandle,
    engine: &Arc<RwLock<TilingEngine>>,
    ipc_server: &Arc<IpcServer>,
) {
    // Lane D: refresh the backend snapshot before diffing a display change.
    wiri::backend::output_runtime::refresh(handle);
    let backend_monitors = handle.get_monitors();
    let output_identities = wiri::backend::outputs::OutputMatcher::snapshot().identities;
    let config = engine.read().full_config().cloned();
    {
        let mut engine = engine.write();
        engine.set_output_identities(output_identities);
        let detected_ids: Vec<_> = backend_monitors.iter().map(|m| m.id).collect();
        let to_remove: Vec<_> = engine
            .monitors()
            .keys()
            .filter(|oid| !detected_ids.contains(*oid))
            .copied()
            .collect();

        // The monitors that are there come first (a new one is registered, a known one
        // updated; one that was unplugged gets its windows back), then the ones that went
        // are removed: their windows move to a monitor that is there, not into the void.
        for (idx, monitor) in backend_monitors.iter().enumerate() {
            match configured_monitor_scale(config.as_ref(), monitor, idx) {
                Some(scale) => {
                    engine.sync_monitor_with_scale(
                        monitor.id,
                        monitor.bounds,
                        monitor.work_area,
                        scale,
                    );
                    engine.set_output_name(monitor.id, &monitor.name);
                }
                None => {
                    if engine.monitors().contains_key(&monitor.id) {
                        engine.unregister_monitor(&monitor.id);
                    }
                }
            }
        }

        for oid in to_remove {
            engine.unregister_monitor(&oid);
        }

        engine.apply_all(handle);
    }

    for monitor in backend_monitors {
        ipc_server.broadcast_event(IpcEvent::MonitorChanged {
            monitor_name: monitor.name,
        });
    }
}

fn print_help() {
    info!("══════════════════════════════════════════");
    info!("Navigation │ Ctrl+Alt+←/→ Focus columns");
    info!("           │ Ctrl+Alt+↑/↓ Focus in column");
    info!("──────────────────────────────────────────");
    info!("Window Mgmt│ Ctrl+Alt+Q Close window");
    info!("           │ Ctrl+Alt+Enter Terminal");
    info!("──────────────────────────────────────────");
    info!("Move       │ Ctrl+Alt+Shift+←/→ Move column");
    info!("──────────────────────────────────────────");
    info!("Fullscreen │ Ctrl+Alt+F Toggle fullscreen");
    info!("──────────────────────────────────────────");
    info!("Floating   │ Ctrl+Alt+T Toggle floating");
    info!("──────────────────────────────────────────");
    info!("Scroll     │ Ctrl+Alt+H/L Scroll workspace");
    info!("──────────────────────────────────────────");
    info!("Workspaces │ Ctrl+Alt+1-9 Switch workspace");
    info!("Overview   │ Ctrl+Alt+Space Zoom out grid");
    info!("           │ Escape         Exit overview (overview-only)");
    info!("           │ Ctrl+Alt+O     Select window");
    info!("──────────────────────────────────────────");
    info!("Tabs       │ Ctrl+Alt+\\  Toggle tabbed column");
    info!("           │ Ctrl+Alt+]/[ Next/prev tab");
    info!("──────────────────────────────────────────");
    info!("IPC        │ wiri-ctl state | windows | quit");
    info!("──────────────────────────────────────────");
    info!("Quit: Ctrl+C or Ctrl+Alt+Shift+Q");
    info!("══════════════════════════════════════════");
}

fn handle_backend_event(
    event: BackendEvent,
    handle: &wiri::backend::BackendHandle,
    engine: &Arc<RwLock<TilingEngine>>,
    ipc_server: &Arc<IpcServer>,
) {
    // Whatever the event does to the windows and workspaces, the event-stream clients are told
    // when it is done.  Not for the ones that cannot change what they are told: a window that
    // moves (they come in streams while the layout animates or a window is dragged) and the
    // CBT notices that are only logged.
    let _state_changed = match &event {
        BackendEvent::WindowMoved { .. }
        | BackendEvent::ZOrderChanged
        | BackendEvent::WindowResized { .. }
        | BackendEvent::WindowMoveResizeStart { .. }
        | BackendEvent::CbtCreateWindow { .. }
        | BackendEvent::CbtDestroyWindow { .. }
        | BackendEvent::CbtActivate { .. }
        | BackendEvent::CbtMoveSize { .. }
        | BackendEvent::CbtSetFocus { .. } => None,
        _ => Some(wiri::ipc::StateChangeGuard),
    };
    match event {
        BackendEvent::WindowCreated { hwnd } => {
            // Most windows fail `should_tile_window` at CREATE time because the
            // title/visibility aren't set yet — that's expected; SHOW/NAMECHANGE
            // re-trigger this path. Guard against double-add in case both fire.
            add_window_if_tileable(hwnd, "", handle, engine, ipc_server);
        }
        BackendEvent::WindowDestroyed { hwnd } => {
            info!("- Window {}", hwnd);
            // `window_closed` is sent when the state shows that the window is gone (and only
            // for a window wiri managed), not for every window that is destroyed.
            engine
                .write()
                .window_destroyed_transition(WindowId::new(hwnd));
            engine.write().remove_window(WindowId::new(hwnd), handle);
            engine.write().apply_all(handle);
        }
        BackendEvent::WindowShown { hwnd } => {
            // Idempotent: only add+broadcast if this window is not already
            // tracked by the tiling engine. EVENT_OBJECT_SHOW fires on every
            // un-minimize, every workspace re-show, and on every NAMECHANGE
            // (we re-emit Shown from there to retry classification of windows
            // that had an empty title at CREATE time). Without this guard we
            // would add the same HWND to a new column every time.
            add_window_if_tileable(hwnd, "via show/namechange", handle, engine, ipc_server);
        }
        BackendEvent::WindowHidden { hwnd } => {
            debug!("Window {} hidden", hwnd);
        }
        BackendEvent::WindowMinimized { hwnd } => {
            // The strip closes the gap the window would leave.
            let window_id = WindowId::new(hwnd);
            engine.write().minimize_window(window_id, handle);
            engine.write().apply_all(handle);
        }
        BackendEvent::WindowRestored { hwnd } => {
            // Back into its old column.  A window wiri never managed (it was minimized
            // when wiri started, or opened minimized) is added like a new one.
            let window_id = WindowId::new(hwnd);
            if engine.read().is_minimized(window_id) {
                engine.write().restore_window(window_id, handle);
                engine.write().apply_all(handle);
            } else {
                add_window_if_tileable(hwnd, "via restore", handle, engine, ipc_server);
            }
        }
        BackendEvent::MonitorConnected { .. } | BackendEvent::MonitorDisconnected { .. } => {
            sync_monitors_from_backend(handle, engine, ipc_server);
        }
        BackendEvent::ForegroundChanged { hwnd, stamp } => {
            let window_id = WindowId::new(hwnd);
            let was_managed = engine.read().tiled_windows().contains_key(&window_id);
            debug!("foreground: hwnd={} managed={}", hwnd, was_managed);
            // SAFETY: GetForegroundWindow has no arguments; the returned handle is
            // only compared, not dereferenced or assumed to remain foreground.
            let current_before_retry = unsafe {
                windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize == hwnd
            };
            if current_before_retry {
                add_window_if_tileable(hwnd, "via foreground retry", handle, engine, ipc_server);
                // Ask outside the engine lock, retaining the existing per-window retry cap.
                // Stale notices must not admit/activate an old foreground through this path.
                if was_managed && engine.read().min_width_query_wanted(window_id) {
                    let answer = handle.min_track_width(hwnd);
                    engine.write().learn_min_width(window_id, answer, handle);
                }
            }
            let owner = wiri::layout::engine::root_owner_of(hwnd);
            let mut eng = engine.write();
            let managed = eng.managed_window_for(hwnd, owner);
            // Native occurrences always finish history visits, even if delivered late.
            // Only a fresh raw HWND comparison permits changing live state or IPC.
            // SAFETY: GetForegroundWindow accepts no pointers/handles. Its result
            // is compared immediately under the engine lock, not dereferenced.
            let is_current = unsafe {
                windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize == hwnd
            };
            let (live, location) =
                eng.observe_native_foreground(hwnd, owner, stamp, is_current, handle);
            if live {
                ipc_server.broadcast_event(IpcEvent::WindowFocused { window_hwnd: hwnd });
                if let Some((col_idx, ws_id)) = location {
                    let number = match managed.and_then(|w| eng.locate_window(w)) {
                        Some((oid, _, _)) => eng.workspace_number_on(oid, ws_id),
                        None => wiri::layout::workspace_number(ws_id),
                    };
                    ipc_server.broadcast_event(IpcEvent::ColumnFocused {
                        window_hwnd: hwnd,
                        column_index: col_idx,
                        workspace_id: number,
                    });
                }
            }
        }
        BackendEvent::ZOrderChanged => {
            engine.read().reconcile_float_z_order();
        }
        // Window position/title changes from external sources are
        // overridden by the tiling engine on next layout pass; but we
        // still forward them as IPC events for external listeners.
        BackendEvent::WindowMoved { hwnd, rect } => {
            ipc_server.broadcast_event(IpcEvent::WindowMoved {
                window_hwnd: hwnd,
                x: rect.loc.x,
                y: rect.loc.y,
                width: rect.size.w,
                height: rect.size.h,
            });
        }
        BackendEvent::WindowResized { hwnd, rect } => {
            ipc_server.broadcast_event(IpcEvent::WindowResized {
                window_hwnd: hwnd,
                width: rect.size.w,
                height: rect.size.h,
            });
        }
        BackendEvent::WindowTitleChanged { hwnd } => {
            // A newly created terminal may answer its minimum only once its title is ready.
            // Reuse the same three-query lifetime cap as foreground retries, outside the lock.
            let window_id = WindowId::new(hwnd);
            if engine.read().min_width_query_wanted(window_id) {
                let answer = handle.min_track_width(hwnd);
                engine.write().learn_min_width(window_id, answer, handle);
            }
            // The backend has read the new title already.  The engine keeps it, and the
            // window rules that look at titles are evaluated again for the window (only
            // when some rule does).
            if let Some(info) = handle.get_window(hwnd) {
                engine
                    .write()
                    .window_title_changed(WindowId::new(hwnd), &info.title, handle);
            }
        }

        BackendEvent::WindowSizeClassChanged {
            hwnd,
            zoomed,
            covers_monitor,
        } => {
            // An application went fullscreen on its own (a video player, F11), or its maximize
            // button was pressed: the engine follows it instead of fighting it.
            let mut eng = engine.write();
            let id = WindowId::new(hwnd);
            if !eng.native_move_resize_active(id) {
                eng.window_size_class_changed(id, zoomed, covers_monitor, handle);
            }
        }

        BackendEvent::WindowMoveResizeStart { hwnd } => {
            debug!("User started manual move/resize on hwnd {}", hwnd);
            engine.write().native_move_resize_start(WindowId::new(hwnd));
            ipc_server.broadcast_event(IpcEvent::WindowMoveResizeStart { window_hwnd: hwnd });
        }
        BackendEvent::WindowMoveResizeEnd { hwnd } => {
            debug!("User ended manual move/resize on hwnd {}", hwnd);
            engine
                .write()
                .native_move_resize_end(WindowId::new(hwnd), handle);
            ipc_server.broadcast_event(IpcEvent::WindowMoveResizeEnd { window_hwnd: hwnd });
        }
        BackendEvent::WindowUrgent { hwnd, .. } => {
            let urgent = {
                let mut engine = engine.write();
                engine.window_urgency_changed(WindowId::new(hwnd), handle);
                engine.is_urgent(WindowId::new(hwnd))
            };
            ipc_server.broadcast_event(IpcEvent::WindowUrgent {
                window_hwnd: hwnd,
                urgent,
            });
        }
        BackendEvent::WindowUrgentCleared { hwnd } => {
            ipc_server.broadcast_event(IpcEvent::WindowUrgent {
                window_hwnd: hwnd,
                urgent: false,
            });
            engine
                .write()
                .set_window_urgent(WindowId::new(hwnd), false, handle);
        }

        // CBT events - fire BEFORE window operations
        // These are logged for debugging; main handling happens via WinEvent
        BackendEvent::CbtCreateWindow { hwnd } => {
            debug!("CBT: Window 0x{:x} about to be created", hwnd);
            // At this point, we could intercept and modify CREATESTRUCT
            // to set initial position/size before the window is shown.
            // See src/backend/cbt_hook.rs for details.
        }
        BackendEvent::CbtDestroyWindow { hwnd } => {
            debug!("CBT: Window 0x{:x} about to be destroyed", hwnd);
            // Could be used to save window state before destruction
        }
        BackendEvent::CbtActivate { hwnd } => {
            debug!("CBT: Window 0x{:x} about to be activated", hwnd);
            // Could intercept focus changes before they happen
        }
        BackendEvent::CbtMoveSize { hwnd } => {
            debug!("CBT: Window 0x{:x} about to be moved/sized", hwnd);
            // Could prevent tiling windows from being resized externally
        }
        BackendEvent::CbtSetFocus { hwnd } => {
            debug!("CBT: Window 0x{:x} about to receive focus", hwnd);
            // Could redirect focus to tiled windows
        }
    }
}

fn live_window_info(handle: &BackendHandle, hwnd: isize) -> Option<wiri::backend::WindowInfo> {
    let hwnd_win = windows::Win32::Foundation::HWND(hwnd as *mut std::ffi::c_void);
    if !unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(hwnd_win).as_bool() } {
        return None;
    }

    let window = wiri::backend::WindowInfo::new(hwnd_win);
    handle.update_window(hwnd, window.clone());
    Some(window)
}

fn add_window_if_tileable(
    hwnd: isize,
    source: &str,
    handle: &BackendHandle,
    engine: &Arc<RwLock<TilingEngine>>,
    ipc_server: &Arc<IpcServer>,
) -> bool {
    let window_id = WindowId::new(hwnd);
    if engine.read().tiled_windows().contains_key(&window_id) {
        return false;
    }

    let Some(window) = live_window_info(handle, hwnd) else {
        return false;
    };

    if !should_tile_window(&window) {
        return false;
    }

    // How narrow can the window be made?  Asked here, once, before the engine lock is
    // taken (the window answers on its own thread): a terminal that will not go below
    // 462 px has to get a column that wide, or it lies on top of its neighbour.
    let min_width = handle.min_track_width(hwnd);

    // The check above ran without the write lock: the creation, show and foreground
    // events and the reconcile pass can find the same new window at the same moment.
    // Look again under the lock that adds it, so that only one of them does.
    let opened = WindowInfoIpc {
        hwnd,
        title: window.title.clone(),
        class_name: window.class_name.clone(),
        process_id: window.process_id,
        x: window.bounds.loc.x,
        y: window.bounds.loc.y,
        width: window.bounds.size.w,
        height: window.bounds.size.h,
    };
    {
        let mut eng = engine.write();
        if eng.tiled_windows().contains_key(&window_id) {
            return false;
        }
        if source.is_empty() {
            info!("+ {} ({})", window.title, window.class_name);
        } else {
            info!("+ {} ({}) [{}]", window.title, window.class_name, source);
        }
        eng.add_window(window, handle);
        eng.note_min_width_answer(window_id, min_width);
        // New windows take the focus; scroll the strip so the new column is on screen.
        eng.reveal_focused();
        eng.apply_all(handle);
    }
    ipc_server.broadcast_event(IpcEvent::WindowOpened { window: opened });
    // (The reconcile pass comes here too, not only the events.)
    wiri::ipc::notify_state_changed();
    true
}

fn reconcile_visible_windows(
    handle: &BackendHandle,
    engine: &Arc<RwLock<TilingEngine>>,
    ipc_server: &Arc<IpcServer>,
) {
    let visible_windows = handle.refresh_visible_windows();
    let mut added_any = false;

    for window in visible_windows {
        let window_id = WindowId::new(window.hwnd);
        if engine.read().tiled_windows().contains_key(&window_id) {
            continue;
        }
        if should_tile_window(&window) {
            added_any |=
                add_window_if_tileable(window.hwnd, "via reconcile", handle, engine, ipc_server);
        }
    }

    if !added_any {
        // Re-hide any inactive-workspace windows that Windows re-shown outside
        // wiri's model. try_write keeps this maintenance loop from blocking IPC
        // or hotkey handlers.
        if let Some(mut eng) = engine.try_write() {
            eng.apply_all(handle);
        }
    }
}

/// Determine if a window should be managed by the tiling engine
fn should_tile_window(window: &wiri::backend::WindowInfo) -> bool {
    // Skip windows with empty titles (usually system/utility windows)
    if window.title.trim().is_empty() {
        return false;
    }

    // Skip known shell/system window classes.
    //
    // ApplicationFrameWindow is deliberately NOT in this list — in Win 11
    // most modern apps (including File Explorer itself) run inside that
    // wrapper, and skipping it makes wiri unable to tile any of them.  The
    // inner Windows.UI.Core.CoreWindow is still skipped below (correct
    // granularity — ApplicationFrameWindow is the OUTER top-level we want).
    let skip_classes = [
        "Shell_TrayWnd",                               // Taskbar
        "Shell_SecondaryTrayWnd",                      // Secondary taskbar
        "Progman",                                     // Desktop
        "WorkerW",                                     // Desktop worker
        "Windows.UI.Core.CoreWindow", // UWP shell (inner child of ApplicationFrameWindow)
        "DV2ControlHost",             // Start menu
        "NotifyIconOverflowWindow",   // Tray overflow
        "ToolbarWindow32",            // Toolbars
        "TaskManagerWindow",          // Task manager
        "SysListView32",              // List views (desktop icons)
        "DesktopUserPicture",         // Lock screen
        "InputIndicator",             // Input indicator
        "Microsoft.UI.Content.DesktopChildSiteBridge", // WinUI
        "WireGuard UI - Manage Tunnels", // WireGuard (stubborn, refuses SetWindowPos)
    ];

    for skip in &skip_classes {
        if window.class_name.contains(skip) {
            return false;
        }
    }

    // Also use the more thorough window module filter
    let hwnd = windows::Win32::Foundation::HWND(window.hwnd as *mut std::ffi::c_void);
    if wiri::window::should_skip_window(hwnd) {
        return false;
    }

    window.is_visible
}
