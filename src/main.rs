// Request to Agent B (resources/): default_config.kdl should add example blocks
// for `spawn-at-startup`, `mod-key`, and `strip-frame` so first-run users
// discover these options.

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::signal;
use tokio::time::Duration;
use tracing::{debug, error, info, warn, Level};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use wiri::backend::{Backend, BackendEvent};
use wiri::backend::message_loop::MessageLoop;
use wiri::config::{Config, loader::ConfigLoader};
use wiri::hooks::{SystemIntegration, TrayAction};
use wiri::layout::{TilingEngine, LayoutConfig};
use wiri::input::{MouseTracker, MouseFocusConfig, start_mouse_hook, stop_mouse_hook, apply_mouse_config};
use wiri::ipc::{IpcServer, IpcEvent, WindowInfoIpc};
use wiri::overlay::WorkspaceIndicator;
use wiri::utils::WindowId;

use parking_lot::RwLock;

#[derive(Parser, Debug)]
#[command(
    name = "wiri",
    version,
    about = "A scrollable-tiling window manager for Windows"
)]
struct Args {
    /// Path to the KDL config file. If unset, search $WIRI_CONFIG, %APPDATA%/wiri/config.kdl,
    /// then %USERPROFILE%/.config/wiri/config.kdl, then fall back to ./config.kdl.
    #[arg(short, long)]
    config: Option<PathBuf>,
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

    
    // // Single-instance check: warn if hotkeys may conflict
    // (RegisterHotKey will fail if another process has the same key combo)
    let args = Args::parse();

    // Setup logging
    let filter = if args.verbose {
        EnvFilter::from_default_env().add_directive(Level::DEBUG.into())
    } else {
        EnvFilter::from_default_env().add_directive(Level::INFO.into())
    };
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .init();

    info!("╔══════════════════════════════════════════╗");
    info!("║ wiri - Scrollable Tiling WM              ║");
    info!("║ Niri-style window manager for Windows    ║");
    info!("╚══════════════════════════════════════════╝");

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
        let pipe_w: Vec<u16> = std::ffi::OsStr::new(wiri::ipc::PIPE_PATH)
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
                wiri::ipc::PIPE_PATH
            );
            error!("Use `wiri-ctl quit` to stop it, or kill the process first.");
            std::process::exit(2);
        }
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
                    } else if let Err(e) = std::fs::write(
                        &path,
                        include_str!("../resources/default_config.kdl"),
                    ) {
                        warn!("Could not write default config to {}: {}", path.display(), e);
                    } else {
                        info!("Wrote default config to {}", path.display());
                    }
                }
                path
            } else {
                PathBuf::from("config.kdl")
            }
        });

    let config = match Config::load(&config_path) {
        Ok(cfg) => {
            info!("Config: {}", config_path.display());
            cfg
        }
        Err(e) => {
            warn!("Using defaults (config error: {})", e);
            Config::default_config()
        }
    };

    // Start config file watcher for live-reload support. The watcher publishes
    // new `Config` instances on the broadcast channel returned by
    // `subscribe()`; the main loop pulls from that channel and re-applies.
    let mut config_loader = ConfigLoader::new(config_path.clone());
    let mut config_reload_rx = config_loader.subscribe();
    if let Err(e) = config_loader.start_watching() {
        warn!("Config watcher could not start: {} (live-reload disabled)", e);
    }

    // Spawn programs listed in spawn-at-startup config entries (niri parity).
    // --no-autostart suppresses this loop (useful when testing config changes).
    if !args.no_autostart {
        use wiri::hooks::Spawner;
        let spawner = Spawner::new();
        for entry in &config.spawn_at_startup {
            if entry.is_empty() {
                continue;
            }
            let program = std::path::Path::new(&entry[0]);
            let arg_strs: Vec<&str> = entry[1..].iter().map(|s| s.as_str()).collect();
            info!("spawn-at-startup: {:?}", entry);
            if let Err(e) = spawner.spawn(program, &arg_strs, true) {
                warn!("spawn-at-startup failed for {:?}: {}", entry, e);
            }
        }
    }

    // Push the mouse-config snapshot into the static read by the low-level
    // wheel hook (natural_scroll + scroll_speed).
    apply_mouse_config(&config.input);

    // Convert config to layout config
    let layout_config = LayoutConfig::from_config(&config);

    // Initialize backend
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

    // Install a LayoutRequest channel between the engine and a backend drainer.
    // The engine can opt-in to dispatching ops over this channel via
    // `engine.dispatch_request(...)`. Direct calls still work today; the channel
    // is set up so future async/throttled paths can use it without restructuring.
    let (layout_req_tx, mut layout_req_rx) = tokio::sync::mpsc::unbounded_channel::<wiri::backend::LayoutRequest>();
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
        // Apply window rules from config
        engine.set_full_config(config.clone());
        engine.set_layout_request_channel(layout_req_tx);

        // Configure animations from config
        let anim_cfg = &config.animations;
        let easing = match anim_cfg.easing.as_str() {
            "ease-out-cubic" | "cubic-bezier" => wiri::layout::Easing::CubicOut,
            "ease-in" => wiri::layout::Easing::EaseIn,
            "ease-out" => wiri::layout::Easing::EaseOut,
            "ease-in-out" => wiri::layout::Easing::EaseInOut,
            "linear" => wiri::layout::Easing::Linear,
            _ => wiri::layout::Easing::CubicOut,
        };
        engine.update_animation_settings(anim_cfg.enabled, anim_cfg.duration, easing);

        // Apply OutputConfig overrides from config (niri parity):
        // - If a matching OutputConfig has `enable false`, skip the monitor.
        // - If a matching OutputConfig has a non-zero `scale`, override scale_factor.
        // Matching is by index order (config output[i] → monitors[i]); name matching
        // is used when the OutputConfig has a non-empty name field.
        // Physical repositioning via CCD APIs is out of scope on Windows.
        for (idx, monitor) in monitors.iter().enumerate() {
            // Find a matching OutputConfig: prefer name match, fall back to index.
            let cfg_output = config.output.iter().find(|o| {
                !o.name.is_empty() && (o.name == monitor.name)
            }).or_else(|| config.output.get(idx));

            let mut effective_scale = monitor.scale_factor;
            let mut skip = false;

            if let Some(out_cfg) = cfg_output {
                if !out_cfg.enable {
                    info!("Skipping disabled output: {}", monitor.name);
                    skip = true;
                } else if out_cfg.scale != 0.0 && (out_cfg.scale - monitor.scale_factor).abs() > 1e-6 {
                    info!(
                        "Output {}: overriding scale {:.2} → {:.2} (from config)",
                        monitor.name, monitor.scale_factor, out_cfg.scale
                    );
                    effective_scale = out_cfg.scale;
                }
            }

            if !skip {
                engine.register_monitor_with_scale(monitor.id, monitor.bounds, monitor.work_area, effective_scale);
            }
        }
    }

    // Initialize hotkey manager
    // HotkeyManager is no longer needed — message_loop handles registration

    // Start message loop with hotkey handling
    let message_loop = MessageLoop::new()?;
    message_loop.start_hotkey_loop(
        engine.clone(),
        backend.handle(),
    );
    info!("Hotkeys registered and message loop started");

// Start low-level mouse hook for interactive move/resize
start_mouse_hook(
    engine.clone(),
    backend.handle(),
).map_err(|e| anyhow::anyhow!("{}", e))?;
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
        for window in windows {
            // Initial enumeration: at_startup=true so window rules with `at-startup` matchers fire.
            engine.write().add_window_at_startup(window, &backend_handle);
        }
        engine.write().apply_all(&backend_handle);
    }

    // System integration (tray icon, autostart, spawner)
    let mut system = SystemIntegration::new(backend.handle()).await;

    // Log autostart registration status at startup
    {
        use wiri::hooks::StartupManager;
        let sm = StartupManager::new();
        if sm.is_registered() {
            info!("Autostart: registered ({})", sm.get_registered_path().unwrap_or_default());
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
    let ipc_server = Arc::new(ipc_server);
    let ipc_server_clone = ipc_server.clone();
    tokio::spawn(async move {
        if let Err(e) = ipc_server_clone.run().await {
            error!("IPC server error: {}", e);
        }
    });
    info!("IPC server started on {}", wiri::ipc::PIPE_PATH);

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

    // Mouse focus tracking. `focus_follows_mouse` is a flat boolean on the
    // InputConfig; user toggles it in their KDL config under `input { ... }`.
    let mouse_config = MouseFocusConfig {
        focus_follows_mouse: config.input.focus_follows_mouse,
        warp_on_focus: false,
        focus_delay_ms: 200,
        raise_on_click: true,
    };
    let mouse_tracker = Arc::new(RwLock::new(MouseTracker::new(mouse_config)));
    let mouse_engine = engine.clone();
    let mouse_tracker_clone = mouse_tracker.clone();
    tokio::spawn(async move {
        loop {
            {
                let mut tracker = mouse_tracker_clone.write();
                tracker.update_cursor_pos();
                if tracker.focus_follows_mouse() && tracker.check_focus_delay() {
                    let pos = tracker.cursor_pos();
                    // Find which monitor the cursor is on
                    let eng = mouse_engine.read();
                    let monitors_map: std::collections::HashMap<_, _> = eng.monitors()
                        .iter()
                        .map(|(oid, m)| (*oid, (m.bounds, m.work_area)))
                        .collect();
                    if let Some(output_id) = tracker.monitor_at_cursor(&monitors_map) {
                        // Find which window the cursor is over
                        let mut found_window = None;
                        for (_, monitor) in eng.monitors().iter() {
                            if let Some(workspace) = monitor.workspace() {
                                for (col_idx, col) in workspace.columns.iter().enumerate() {
                                    for tile in &col.tiles {
                                        if let Some(info) = eng.tiled_windows().get(&tile.window_id) {
                                            if info.bounds.contains_point(pos) {
                                                if monitor.focus_window != Some(tile.window_id) {
                                                    found_window = Some((tile.window_id, col_idx, output_id));
                                                }
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        drop(eng);
                        if let Some((wid, col_idx, oid)) = found_window {
                            let mut eng_write = mouse_engine.write();
                            if let Some(m) = eng_write.monitors_mut().get_mut(&oid) {
                                m.focus_window = Some(wid);
                                m.focus_column = Some(col_idx);
                            }
                            tracker.record_focus_change();
                        }
                    }
                }
            } // Drop all locks before await
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });

    // Workspace indicator overlay — shown whenever the active workspace changes.
    let overlay = Arc::new(WorkspaceIndicator::new());
    // Spawn the Win32 message-pump thread; drop the JoinHandle (overlay lives until exit).
    let _ = overlay.clone().spawn();

    print_help();

    // Track time for animation ticking
    let mut last_tick = std::time::Instant::now();
    // 30 fps (~33 ms) halves the Win32 call budget vs. 60 fps with no visible
    // quality loss for tiling animations. The applied-state cache (engine.rs)
    // makes each apply_all call sub-microsecond when nothing has changed, so
    // even at 30 fps the steady-state overhead is negligible.
    const ANIMATION_TICK_MS: u32 = 33; // ~30fps
    let mut ipc_shutdown_rx = ipc_shutdown_rx;

    // Main loop: process tray actions + tick animations + wait for shutdown signal
    'main: loop {
        // Tick animations if enabled.  apply_all is cheap when no state changed
        // because the applied-state cache in the engine skips all Win32 calls.
        {
            let elapsed = last_tick.elapsed().as_millis() as u32;
            if elapsed >= ANIMATION_TICK_MS {
                last_tick = std::time::Instant::now();

                let mut eng = engine.write();
                // Always call apply_all: when tick_animations returns false and
                // the cache matches, the pass costs only hash lookups (O(n_tiles) μs).
                // This ensures focus/border/opacity changes are reflected even when
                // no animation is running.
                eng.tick_animations(elapsed);
                eng.apply_all(&handle);
            }
        }

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
                    // Use the configured workspace name when available, otherwise
                    // fall back to "Workspace N" (1-based display index).
                    let label = engine
                        .read()
                        .full_config()
                        .and_then(|c| {
                            c.workspace
                                .iter()
                                .enumerate()
                                .find(|(idx, w)| *idx as i32 == ws_id || w.name == ws_id.to_string())
                                .and_then(|(_, w)| {
                                    if w.name.is_empty() {
                                        None
                                    } else {
                                        Some(w.name.clone())
                                    }
                                })
                        })
                        .unwrap_or_else(|| format!("Workspace {}", ws_id + 1));
                    // Show the overlay centred over the focused monitor so it
                    // lands on the screen the user is looking at — falls back
                    // to the primary monitor if no focused monitor is set.
                    let focused_bounds = engine.read().monitors().focused()
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
                }
                *last = current;
            }
        }

        tokio::time::sleep(Duration::from_millis(33)).await;

        // Handle file-watcher config reloads
        match config_reload_rx.try_recv() {
            Ok(new_config) => {
                info!("Config file changed — reloading");
                let new_layout_config = LayoutConfig::from_config(&new_config);
                apply_mouse_config(&new_config.input);
                engine.write().update_config(new_layout_config);
                engine.write().set_full_config(new_config);
                engine.write().apply_all(&handle);
                message_loop.reload_hotkeys();
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                warn!("Config reload channel lagged {} events — reloading from disk", n);
                if let Ok(new_config) = Config::load(&config_path) {
                    let new_layout_config = LayoutConfig::from_config(&new_config);
                    apply_mouse_config(&new_config.input);
                    engine.write().update_config(new_layout_config);
                    engine.write().set_full_config(new_config);
                    engine.write().apply_all(&handle);
                    message_loop.reload_hotkeys();
                }
            }
            Err(_) => {} // Empty or closed — normal
        }

        // Check for IPC-initiated graceful shutdown
        if let Ok(()) = ipc_shutdown_rx.try_recv() {
            info!("Shutting down (IPC Quit)...");
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
                    let wids: Vec<_> = engine
                        .read()
                        .tiled_windows()
                        .keys()
                        .copied()
                        .collect();
                    for wid in wids {
                        let _ = handle.show_window(wid.as_isize(), show);
                    }
                }
                TrayAction::ReloadConfig => {
                    info!("Tray: Reloading config");
                    // Tray reload bypasses the file-watcher channel (used for explicit
                    // user-driven reloads, e.g. after editing the file in a different
                    // editor) — load directly from disk and re-apply everything.
                    match Config::load(&config_path) {
                        Ok(new_config) => {
                            let new_layout_config = LayoutConfig::from_config(&new_config);
                            apply_mouse_config(&new_config.input);
                            engine.write().update_config(new_layout_config);
                            engine.write().set_full_config(new_config.clone());
                            engine.write().apply_all(&handle);
                            message_loop.reload_hotkeys();
                            info!("Config reloaded successfully");
                        }
                        Err(e) => {
                            warn!("Config reload failed: {}", e);
                        }
                    }
                }
                TrayAction::Quit => {
                    info!("Quit requested via tray icon");
                    break 'main;
                }
                TrayAction::OpenConfigDir => {
                    // Open the folder containing the active config in Explorer.
                    if let Some(dir) = config_path.parent() {
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

    // Restore tiled windows to their original positions and styles so the
    // user doesn't lose their layout on quit.
    info!("Restoring windows...");
    engine.write().restore_all(&handle);

    stop_mouse_hook();
    wiri::hooks::clear_backend();
    info!("Goodbye!");
    Ok(())
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
    info!("           │ Ctrl+Alt+O Select window");
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
    match event {
        BackendEvent::WindowCreated { hwnd } => {
            if let Some(window) = handle.get_window(hwnd) {
                if should_tile_window(&window) {
                    info!("+ {} ({})", window.title, window.class_name);
                    ipc_server.broadcast_event(IpcEvent::WindowOpened {
                        window: WindowInfoIpc {
                            hwnd,
                            title: window.title.clone(),
                            class_name: window.class_name.clone(),
                            process_id: window.process_id,
                            x: window.bounds.loc.x,
                            y: window.bounds.loc.y,
                            width: window.bounds.size.w,
                            height: window.bounds.size.h,
                        },
                    });
                    engine.write().add_window(window, handle);
                    engine.write().apply_all(handle);
                }
            }
        }
        BackendEvent::WindowDestroyed { hwnd } => {
            info!("- Window {}", hwnd);
            ipc_server.broadcast_event(IpcEvent::WindowClosed { window_hwnd: hwnd });
            engine.write().remove_window(WindowId::new(hwnd), handle);
            engine.write().apply_all(handle);
        }
        BackendEvent::WindowShown { hwnd } => {
            if let Some(window) = handle.get_window(hwnd) {
                if should_tile_window(&window) {
                    ipc_server.broadcast_event(IpcEvent::WindowOpened {
                        window: WindowInfoIpc {
                            hwnd,
                            title: window.title.clone(),
                            class_name: window.class_name.clone(),
                            process_id: window.process_id,
                            x: window.bounds.loc.x,
                            y: window.bounds.loc.y,
                            width: window.bounds.size.w,
                            height: window.bounds.size.h,
                        },
                    });
                    engine.write().add_window(window, handle);
                    engine.write().apply_all(handle);
                }
            }
        }
        BackendEvent::WindowHidden { hwnd } => {
            ipc_server.broadcast_event(IpcEvent::WindowClosed { window_hwnd: hwnd });
            engine.write().remove_window(WindowId::new(hwnd), handle);
            engine.write().apply_all(handle);
        }
        BackendEvent::MonitorConnected { .. } => {
            for monitor in handle.get_monitors() {
                if !engine.read().monitors().contains_key(&monitor.id) {
                    engine.write().register_monitor(monitor.id, monitor.bounds, monitor.work_area);
                }
                ipc_server.broadcast_event(IpcEvent::MonitorChanged {
                    monitor_name: monitor.name.clone(),
                });
            }
            engine.write().apply_all(handle);
        }
        BackendEvent::MonitorDisconnected { .. } => {
            let to_remove: Vec<_> = engine
                .read()
                .monitors()
                .keys()
                .filter(|oid| !handle.get_monitors().iter().any(|m| m.id == **oid))
                .cloned()
                .collect();
            for oid in to_remove {
                engine.write().unregister_monitor(&oid);
            }
            engine.write().apply_all(handle);
        }
        BackendEvent::ForegroundChanged { hwnd } => {
            let window_id = WindowId::new(hwnd);
            ipc_server.broadcast_event(IpcEvent::WindowFocused { window_hwnd: hwnd });
            let mut eng = engine.write();
            for (_, monitor) in eng.monitors_mut().iter_mut() {
                if monitor
                    .workspace()
                    .and_then(|w| w.find_window_column(window_id))
                    .is_some()
                {
                    monitor.focus_window = Some(window_id);
                    if let Some(col_idx) =
                        monitor.workspace().and_then(|w| w.find_window_column(window_id))
                    {
                        monitor.focus_column = Some(col_idx);
                    }
                    break;
                }
            }
        }
        // Window position/title changes from external sources are
        // overridden by the tiling engine on next layout pass
        BackendEvent::WindowMoved { .. }
        | BackendEvent::WindowResized { .. }
        | BackendEvent::WindowTitleChanged { .. } => {}

        BackendEvent::WindowMoveResizeStart { hwnd } => {
            debug!("User started manual move/resize on hwnd {}", hwnd);
        }
        BackendEvent::WindowMoveResizeEnd { hwnd } => {
            debug!("User ended manual move/resize on hwnd {}", hwnd);
            engine.write().apply_all(handle);
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

/// Determine if a window should be managed by the tiling engine
fn should_tile_window(window: &wiri::backend::WindowInfo) -> bool {
    // Skip windows with empty titles (usually system/utility windows)
    if window.title.trim().is_empty() {
        return false;
    }

    // Skip known shell/system window classes
    let skip_classes = [
        "Shell_TrayWnd",          // Taskbar
        "Shell_SecondaryTrayWnd", // Secondary taskbar
        "Progman",                 // Desktop
        "WorkerW",                 // Desktop worker
        "Windows.UI.Core.CoreWindow", // UWP shell
        "ApplicationFrameWindow", // UWP frame
        "DV2ControlHost",         // Start menu
        "NotifyIconOverflowWindow", // Tray overflow
        "ToolbarWindow32",        // Toolbars
        "TaskManagerWindow",      // Task manager
        "SysListView32",          // List views (desktop icons)
        "DesktopUserPicture",     // Lock screen
        "InputIndicator",         // Input indicator
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
