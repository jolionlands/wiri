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
use clap::{CommandFactory, Parser, Subcommand};
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

use parking_lot::{Mutex, RwLock};
use wiri::input::keyboard_config::KeyboardConfigSession;

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
    /// KDL path; otherwise use nonempty WIRI_CONFIG, NIRI_CONFIG, then APPDATA/wiri/config.kdl.
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<OfflineCommand>,
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
    /// Skip spawning programs listed in spawn-at-startup config entries (useful for testing).
    /// Also keeps the Important Hotkeys sheet closed at startup, like `--no-startup-overlay`.
    #[arg(long)]
    no_autostart: bool,
    /// Do not open the Important Hotkeys sheet at startup, as `hotkey-overlay { skip-at-startup }`
    /// does from the config. The scripted live harness starts every daemon with this so a sheet
    /// never takes the first key press of a scripted scenario.
    #[arg(long)]
    no_startup_overlay: bool,
}

#[derive(Subcommand, Debug)]
enum OfflineCommand {
    /// Validate the config without starting the window manager.
    Validate,
    /// Generate shell completions without starting the window manager.
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

fn offline_command(args: &Args) -> Option<bool> {
    match args.command.as_ref()? {
        OfflineCommand::Validate => {
            let path = wiri::config::search::resolve(args.config.clone()).path;
            let decoded = wiri::config::decode::decode_file(&path);
            for issue in &decoded.issues {
                eprintln!(
                    "{}",
                    wiri::config::diagnostics::render(issue, &path.display().to_string())
                );
            }
            if !decoded.has_errors() {
                println!("Configuration is valid: {}", path.display());
            }
            Some(!decoded.has_errors())
        }
        OfflineCommand::Completions { shell } => {
            clap_complete::generate(*shell, &mut Args::command(), "wiri", &mut std::io::stdout());
            Some(true)
        }
    }
}

fn main() -> Result<()> {
    // A bounded runtime (see `wiri::runtime`): `#[tokio::main]` would start one worker per
    // logical CPU and an unbounded blocking pool.
    wiri::runtime::build()?.block_on(run())
}

async fn run() -> Result<()> {
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
    if args.command.is_some()
        || args.verbose
        || args.register_autostart
        || args.unregister_autostart
    {
        wiri::logging::attach_parent_console();
    }
    if let Some(ok) = offline_command(&args) {
        if !ok {
            std::process::exit(1);
        }
        return Ok(());
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
            // Nor leave the pointer invisible: `cursor { hide-after-inactive-ms }` swaps the
            // system cursors, and only this puts the user's own back after a crash.
            wiri::input::cursor_hide::panic_restore();
            default_hook(info);
        }));
    }
    // A previous run may have died while the pointer was hidden: reload the user's cursors
    // before anything else can hide them again.
    wiri::input::cursor_hide::restore_at_startup();

    // Explicit CLI/env paths are never created, including missing ones.
    let selected_config = wiri::config::search::resolve(args.config.clone());
    let created_config = match selected_config.create_default() {
        Ok(created) => created,
        Err(e) => {
            warn!("Could not create default config: {}", e);
            false
        }
    };
    let config_path = selected_config.path;

    // A config with errors is not loaded (niri does the same): wiri starts with its defaults
    // and says why in a balloon as soon as the tray icon exists.
    let startup_decoded = wiri::config::loader::load_logged(&config_path);
    let mut pending_balloon: Option<(String, String, u32)> = created_config.then(|| {
        (
            "wiri: default config".to_string(),
            format!("Created a default config file at {}", config_path.display()),
            0,
        )
    });
    // What an event-stream client that connects later is told about this load.
    let startup_warnings = startup_decoded.warning_count() as u32;
    let mut startup_error: Option<String> = None;
    let config = if startup_decoded.has_errors() {
        let summary = wiri::config::failure_summary(&config_path, &startup_decoded.issues);
        startup_error = Some(summary.clone());
        warn!("Using defaults (config error: {})", summary);
        // Narrator/NVDA hear the same error the balloon shows (niri announces config errors
        // too); a config that failed to load cannot have turned announcements off.
        wiri::a11y::announce_config_error(&summary);
        if !startup_decoded.config.disable_failed_config_notification {
            pending_balloon = Some((
                "wiri: config error".to_string(),
                format!("{} - using the defaults.", summary),
                0,
            ));
        }
        Config::default_config()
    } else {
        info!(
            "Config: {} ({} warnings)",
            config_path.display(),
            startup_decoded.warning_count()
        );
        startup_decoded.config
    };

    // Screen-reader announcements and `block-out-from` come from the same config the rest
    // of startup reads: announce only when `accessibility` allows it, and let wiri's own
    // overlays follow the rules before any of them exists.
    wiri::a11y::set_enabled(config.accessibility);
    wiri::window::block_out::install(&config.layer_rules, &config.window_rules);

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

    // One explicit-only native-settings lifetime, including reload and early-return cleanup.
    let keyboard_session = Mutex::new(KeyboardConfigSession::from_env());
    configure_session(&config, &keyboard_session);

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
    // precision", button swap, two precision-touchpad values, "hide pointer while typing").
    // Only the options the config names are written, the previous values are remembered, and
    // they are restored on a graceful quit or when a reload stops naming them.
    wiri::input::pointer_settings::apply_config(&config.input, &config.cursor);

    // Lane LAST: `cursor { hide-after-inactive-ms }` rides the mouse hook's own message loop
    // with one one-shot timer; nothing exists at rest and the cursors are always restored.
    wiri::input::cursor_hide::configure(&config.cursor);

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

    let decor_layer = Arc::new(wiri::overlay::decor::layer::DecorLayer::new());
    {
        let weak_engine = Arc::downgrade(&engine);
        let tab_backend = backend.handle();
        decor_layer.set_tab_handler(Arc::new(move |window| {
            if let Some(engine) = weak_engine.upgrade() {
                engine.write().select_tab_window(window, &tab_backend);
            }
        }));
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
        let decor_layer: Arc<dyn wiri::overlay::decor::DecorSink> = decor_layer;
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
    let (hotkey_quit_tx, mut hotkey_quit_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
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
            let native_bounds = backend_handle.native_bounds(window.hwnd);
            // Initial enumeration preserves existing geometry/focus and skips open animations.
            // The rule matcher's startup phase is shared with subsequent CREATE/SHOW events.
            let mut eng = engine.write();
            eng.add_window_with_native_bounds(window, &backend_handle, true, native_bounds);
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
    //
    // It sleeps in `recv` until the WinEvent thread sends something (it used to look every
    // 50 ms, which also delayed every event by up to 50 ms), then drains what queued behind
    // it, in bursts of at most `EVENT_BURST` so that a flood cannot starve the reconcile
    // deadline below.
    //
    // The same task runs the reconcile pass, which catches windows that missed CREATE/SHOW
    // events (common with launchers) and reasserts workspace visibility if the shell
    // re-shows a hidden inactive-workspace HWND behind wiri's back.  It used to run every
    // 750 ms forever; now window events arm it and it rests when several passes in a row
    // found nothing (see `wiri::backend::reconcile`), so a settled desktop has no timer.
    let handle_evt = handle.clone();
    let ipc_server_events = ipc_server.clone();
    tokio::spawn(async move {
        use wiri::backend::reconcile::{event_arms_reconcile, ReconcileSchedule};
        const EVENT_BURST: usize = 256;
        let mut schedule = ReconcileSchedule::starting(std::time::Instant::now());
        loop {
            let due = schedule.deadline();
            tokio::select! {
                event = event_rx.recv() => {
                    let Some(event) = event else {
                        // Every sender is gone: no window event can arrive any more.
                        break;
                    };
                    let mut arms = event_arms_reconcile(&event);
                    handle_backend_event(event, &handle_evt, &engine_clone, &ipc_server_events);
                    for _ in 1..EVENT_BURST {
                        let Ok(event) = event_rx.try_recv() else {
                            break;
                        };
                        arms |= event_arms_reconcile(&event);
                        handle_backend_event(event, &handle_evt, &engine_clone, &ipc_server_events);
                    }
                    if arms {
                        schedule.note_activity(std::time::Instant::now());
                    }
                }
                // (The expression is built even while the branch is disabled; it is not polled.)
                _ = tokio::time::sleep_until(
                    due.map(tokio::time::Instant::from_std)
                        .unwrap_or_else(tokio::time::Instant::now),
                ), if due.is_some() => {
                    let admitted =
                        reconcile_visible_windows(&handle_evt, &engine_clone, &ipc_server_events);
                    schedule.pass_done(std::time::Instant::now(), admitted);
                }
            }
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

    // Workspace indicator overlay — shown whenever the active workspace changes.  Its message-pump
    // thread and window are created by the first `show`, not here.
    let overlay = Arc::new(WorkspaceIndicator::new());

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
    // `hotkey-overlay { hide-not-bound }`.  `--no-startup-overlay` (and `--no-autostart`, which
    // means "no startup extras") keeps it closed for a scripted start.
    let quiet_start = args.no_startup_overlay || args.no_autostart;
    if config.hotkey_overlay.skip_at_startup || quiet_start {
        info!(
            "Hotkey overlay: skipped at startup ({})",
            if config.hotkey_overlay.skip_at_startup {
                "hotkey-overlay { skip-at-startup }"
            } else {
                "command line"
            }
        );
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

    // Animations have their own frame worker; idle animations need no polling. While
    // timelines run it waits for the presentation clock instead of a fixed 16 ms timer:
    // DCompositionWaitForCompositorClock, then DwmFlush, then a sleep derived from the
    // output refresh rate (see `wiri::animation`). A frame's delta is the measured
    // presentation interval, so springs and easings advance by presented frames, and the
    // wait is disarmed the moment the last timeline finishes.
    let animation_worker = {
        let weak_engine = Arc::downgrade(&engine);
        let wake = Arc::clone(&animation_wake);
        let animation_handle = handle.clone();
        tokio::spawn(async move {
            let mut clock = wiri::animation::FrameClock::new();
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
                    // Nothing is armed from here on: no timer, no blocking wait, no thread.
                    clock.disarm();
                    notified.await;
                    continue;
                }

                let frame = clock.next_frame().await;
                let Some(engine) = weak_engine.upgrade() else {
                    break;
                };
                let _ = engine
                    .write()
                    .tick_animations_with_backend(frame.delta_ms, &animation_handle);
            }
        })
    };
    let mut ipc_shutdown_rx = ipc_shutdown_rx;
    // A oneshot (and a closed broadcast) must not be polled again once it has answered.
    let mut ipc_shutdown_open = true;
    let mut config_reload_open = true;
    let mut reload = ReloadCtx {
        config_path: config_path.clone(),
        engine: &engine,
        handle: &handle,
        message_loop: &message_loop,
        mouse_tracker: &mouse_tracker,
        ipc_server: &ipc_server,
        keyboard_session: &keyboard_session,
    };
    // One Ctrl+C future for the whole loop (it used to be built and dropped with a 1 ms
    // timeout on every iteration), and the wake-ups of the two polled sources.
    let ctrl_c = signal::ctrl_c();
    tokio::pin!(ctrl_c);
    let tray_wake = wiri::hooks::tray::tray_action_wake();
    let workspace_watch = wiri::ipc::workspace_watch();
    // Fixed at engine creation: enumeration/reloads cannot extend the startup phase.
    let startup_rules_timeout = tokio::time::sleep_until(tokio::time::Instant::from_std(
        engine.read().startup_rules_deadline(),
    ));
    tokio::pin!(startup_rules_timeout);
    let mut startup_rules_pending = true;

    // Main loop: event driven.  It sleeps until a tray click, a workspace change, a config
    // reload, a quit request or Ctrl+C needs it; with none of those pending there is no
    // repeating timer (the startup rule expiry and a pending tray balloon are bounded).
    'main: loop {
        // Workspace-indicator: detect workspace switches and show the overlay.
        // The compare is cheap — active_workspace is a plain i32 field; no Win32 calls — and
        // runs when `workspace_watch` fires, not on a timer.
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
                    // niri announces a workspace *switch*, not the state wiri starts in: a
                    // screen reader hears "Workspace <name>" (or "Workspace <n>").
                    if last.is_some() {
                        let eng = engine.read();
                        match eng.monitors().focused_id() {
                            Some(oid) => wiri::a11y::announce_workspace(
                                eng.workspace_name_on(oid, ws_id).as_deref(),
                                eng.workspace_number_on(oid, ws_id),
                            ),
                            None => wiri::a11y::announce_workspace(None, ws_id + 1),
                        }
                    }
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

        // Sleep until something needs this loop.  Each source below wakes it by itself; the
        // deadlines are the one startup expiry and a pending tray balloon retry.
        tokio::select! {
            // A tray click and a possible workspace change only wake the loop: the tray
            // actions are drained below, the workspace compare runs at the top.
            _ = tray_wake.notified() => {}
            _ = workspace_watch.notified() => {}

            _ = &mut startup_rules_timeout, if startup_rules_pending => {
                startup_rules_pending = false;
                let expired = engine.write().finish_startup_rules(
                    std::time::Instant::now(),
                    &handle,
                );
                if expired {
                    // Own-overlay refresh queries Windows; release the engine lock first.
                    wiri::window::block_out::finish_startup();
                    wiri::ipc::notify_state_changed();
                }
            }

            // Handle file-watcher config reloads
            outcome = config_reload_rx.recv(), if config_reload_open => match outcome {
                Ok(outcome) => reload.outcome(outcome, &system),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!(
                        "Config reload channel lagged {} events — reloading from disk",
                        n
                    );
                    reload.from_disk(&system);
                }
                // The loader that owns the sender is gone: nothing more can arrive.
                Err(tokio::sync::broadcast::error::RecvError::Closed) => config_reload_open = false,
            },

            // Handle IPC-initiated config reloads.
            Some(()) = ipc_reload_rx.recv() => {
                info!("IPC: Reloading config from disk");
                reload.from_disk(&system);
            }

            // System load-config-file: switch the active path and its watcher only after
            // a successful load. A missing path means reload the currently active file.
            Some(path) = system_reload_rx.recv() => {
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
                        config_reload_open = true;
                        if let Err(error) = config_loader.start_watching() {
                            warn!("Could not watch the selected config: {}", error);
                        }
                    }
                } else {
                    reload.config_path = previous;
                }
            }

            // Check for IPC-initiated graceful shutdown
            answer = &mut ipc_shutdown_rx, if ipc_shutdown_open => {
                ipc_shutdown_open = false;
                if answer.is_ok() {
                    info!("Shutting down (IPC Quit)...");
                    break 'main;
                }
            }

            Some(()) = hotkey_quit_rx.recv() => {
                info!("Shutting down (hotkey)...");
                break 'main;
            }

            _ = &mut ctrl_c => {
                info!("Shutting down (Ctrl+C)...");
                break 'main;
            }

            // The config error that was found at startup is retried until the tray icon is
            // there (see below); this retry is independent of startup rule expiry.
            _ = tokio::time::sleep(Duration::from_millis(50)), if pending_balloon.is_some() => {}
        }

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
                        if let Err(error) = wiri::hooks::spawner::spawn_argv(&[
                            "explorer.exe".into(),
                            dir.to_string_lossy().into_owned(),
                        ]) {
                            warn!(%error, "Could not open active config directory");
                        }
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
    }

    // Stop animation writes before restoring the original window state.
    animation_worker.abort();
    let _ = animation_worker.await;

    // Lane P: give the user their own pointer settings back before anything else goes away.
    wiri::input::pointer_settings::restore();

    // Lane LAST: the pointer must never be left invisible, and the one-shot timer goes with the
    // mouse hook thread below.
    wiri::input::cursor_hide::shutdown();

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

    keyboard_session.lock().restore();
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
    keyboard_session: &'a Mutex<KeyboardConfigSession>,
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
        configure_a11y_and_block_out(&config);
        apply_loaded_config(
            config,
            self.engine,
            self.handle,
            self.message_loop,
            self.mouse_tracker,
            self.keyboard_session,
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
        // A failed reload is announced even when the balloon is off: the screen reader
        // user has no other way to learn about it.
        wiri::a11y::announce_config_error(&summary);
        let text = balloon_text(&format!("{} - last good config kept.", summary));
        let disabled = self
            .engine
            .read()
            .full_config()
            .is_some_and(|c| c.disable_failed_config_notification);
        if !disabled && !system.tray_icon().try_balloon("wiri: config error", &text) {
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

/// The two config-driven pieces that live outside the engine: screen-reader announcements
/// and `block-out-from` on wiri's own overlays.
fn configure_a11y_and_block_out(config: &Config) {
    wiri::a11y::set_enabled(config.accessibility);
    wiri::window::block_out::install(&config.layer_rules, &config.window_rules);
}

/// Publish only good configs. Spawns use child-only environment snapshots;
/// switch subscriptions live on the existing hidden-window pump.
fn configure_session(config: &Config, keyboard_session: &Mutex<KeyboardConfigSession>) {
    if let Err(error) = wiri::hooks::environment::configure(&config.environment) {
        warn!(%error, "Child environment unchanged");
    }
    if let Err(error) = wiri::hooks::switch_events::configure(config.switch_events.subscriptions())
    {
        warn!(%error, "Switch-event subscriptions could not be updated");
    }
    keyboard_session.lock().apply(&config.input);
}

fn apply_loaded_config(
    new_config: Config,
    engine: &Arc<RwLock<TilingEngine>>,
    handle: &BackendHandle,
    message_loop: &MessageLoop,
    mouse_tracker: &Arc<RwLock<MouseTracker>>,
    keyboard_session: &Mutex<KeyboardConfigSession>,
) {
    use wiri::config::reload::{plan as plan_reload, ReloadStep};
    // niri's `reload_config` re-applies only what changed, in a fixed order
    // (niri e19df12 src/niri.rs:1683-1967).  The plan compares the config the engine holds
    // with the one just decoded; an unchanged section is not applied again, so a bind-only
    // reload does not rewrite pointer settings or re-place every window.
    let old_config = engine.read().full_config().cloned();
    let plan = plan_reload(old_config.as_ref(), &new_config);
    if plan.contains(ReloadStep::Environment) {
        if let Err(error) = wiri::hooks::environment::configure(&new_config.environment) {
            warn!(%error, "Child environment unchanged");
        }
    }
    if plan.contains(ReloadStep::SwitchEvents) {
        if let Err(error) =
            wiri::hooks::switch_events::configure(new_config.switch_events.subscriptions())
        {
            warn!(%error, "Switch-event subscriptions could not be updated");
        }
    }
    if plan.contains(ReloadStep::Screenshot) {
        wiri::screenshot::configure(&new_config);
    }
    // Lane D: reload physical differences outside the engine lock, then refresh geometry/DPI.
    if plan.contains(ReloadStep::Outputs) {
        wiri::backend::outputs::apply(&new_config);
        wiri::backend::output_runtime::refresh(handle);
    }
    if plan.contains(ReloadStep::Input) {
        // niri: keyboard repeat and xkb first (`src/niri.rs:1750-1764`, 1888-1891), then the
        // libinput settings on the current devices (`1893-1902`); the cursor theme and the
        // cursor timeout follow the same section.
        keyboard_session.lock().apply(&new_config.input);
        let mouse_config = MouseFocusConfig {
            focus_follows_mouse: new_config.input.focus_follows_mouse,
            warp_on_focus: new_config.input.warp_on_focus,
            focus_delay_ms: new_config.input.focus_delay_ms,
            raise_on_click: true,
        };
        apply_mouse_config(&new_config.input);
        // Lane P: pointing-device options follow a reload; the ones the new config no longer
        // names go back to what the OS had before wiri changed them.
        wiri::input::pointer_settings::apply_config(&new_config.input, &new_config.cursor);
        // Lane LAST: a reload that drops `hide-after-inactive-ms` shows the pointer again (and
        // the one timer goes away); one that adds it waits for the next pointer movement.
        wiri::input::cursor_hide::configure(&new_config.cursor);
        mouse_tracker.write().set_config(mouse_config);
    }
    let layout_changed = plan.contains(ReloadStep::Layout);
    let new_layout_config = layout_changed.then(|| LayoutConfig::from_config(&new_config));
    let hide_method = hide_method_of(&new_config);
    wiri::backend::cloak::set_hide_method(hide_method);
    {
        let mut engine = engine.write();
        if let Some(new_layout_config) = new_layout_config {
            engine.update_config(new_layout_config);
        }
        if layout_changed || plan.contains(ReloadStep::Outputs) {
            wiri::backend::output_runtime::sync(&new_config, handle, &mut engine);
        }
        // Publishing the config is not optional: the engine reads `binds`, the effects, the
        // recent-window options and the workspace declarations from it, and `reload_hotkeys`
        // runs after this block.
        engine.set_full_config(new_config);
        engine.set_hide_method(hide_method);
        // The rules were replaced: the windows that are open follow them (their opacity, their
        // size limits; the `open-*` properties are for windows that open).
        if layout_changed || plan.reevaluates_rules() {
            engine.apply_reloaded_layout(handle);
        }
    }
    if plan.contains(ReloadStep::Binds) {
        message_loop.reload_hotkeys();
    }
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

        engine.invalidate_all_native_bounds();
        engine.apply_all(handle);
    }

    for monitor in backend_monitors {
        ipc_server.broadcast_event(IpcEvent::MonitorChanged {
            monitor_name: monitor.name,
        });
    }
    refresh_native_bounds_batch(handle, engine);
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
        BackendEvent::SwitchEvent(edge) => {
            // Switch binds deliberately bypass keyboard inhibition/lock filtering.
            let command = engine
                .read()
                .full_config()
                .and_then(|config| config.switch_events.command(edge))
                .map(<[String]>::to_vec);
            if let Some(command) = command {
                match wiri::hooks::spawner::spawn_argv(&command) {
                    Ok(pid) => info!(?edge, pid, "switch-events: spawned command"),
                    Err(error) => warn!(?edge, %error, "switch-events: spawn failed"),
                }
            }
        }
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
                // Ask outside the engine lock, using native freshness/backoff.
                // Stale notices must not admit/activate an old foreground through this path.
                if was_managed {
                    refresh_native_bounds_for_window(hwnd, handle, engine);
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
                for event in eng.live_focus_events(hwnd, managed, location) {
                    ipc_server.broadcast_event(event);
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
            refresh_native_bounds_for_window(hwnd, handle, engine);
            ipc_server.broadcast_event(IpcEvent::WindowMoved {
                window_hwnd: hwnd,
                x: rect.loc.x,
                y: rect.loc.y,
                width: rect.size.w,
                height: rect.size.h,
            });
        }
        BackendEvent::WindowResized { hwnd, rect } => {
            refresh_native_bounds_for_window(hwnd, handle, engine);
            ipc_server.broadcast_event(IpcEvent::WindowResized {
                window_hwnd: hwnd,
                width: rect.size.w,
                height: rect.size.h,
            });
        }
        BackendEvent::WindowTitleChanged { hwnd } => {
            refresh_native_bounds_for_window(hwnd, handle, engine);
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
                eng.invalidate_native_bounds(id);
            }
            drop(eng);
            refresh_native_bounds_for_window(hwnd, handle, engine);
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
    let native_bounds = handle.native_bounds(hwnd);

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
        eng.add_window_with_native_bounds(window, handle, false, native_bounds);
        // New windows take the focus; scroll the strip so the new column is on screen.
        eng.reveal_focused();
        eng.apply_all(handle);
    }
    ipc_server.broadcast_event(IpcEvent::WindowOpened { window: opened });
    // (The reconcile pass comes here too, not only the events.)
    wiri::ipc::notify_state_changed();
    true
}

/// Query at most one due application outside the engine lock. Context checks
/// also invalidate physical conversion after a DPI, frame or output change.
fn refresh_native_bounds_for_window(
    hwnd: isize,
    handle: &BackendHandle,
    engine: &Arc<RwLock<TilingEngine>>,
) -> bool {
    let Some(context) = handle.native_bounds_context(hwnd) else {
        return false;
    };
    let query =
        engine
            .write()
            .native_bounds_query(WindowId::new(hwnd), context, std::time::Instant::now());
    let Some(query) = query else {
        return false;
    };
    let answer = handle.native_bounds(hwnd);
    engine
        .write()
        .finish_native_bounds_query(query, answer, std::time::Instant::now(), handle);
    true
}

fn refresh_native_bounds_batch(handle: &BackendHandle, engine: &Arc<RwLock<TilingEngine>>) {
    let candidates = engine.read().native_bounds_candidates();
    let mut queried = 0;
    for id in candidates {
        if refresh_native_bounds_for_window(id.as_isize(), handle, engine) {
            queried += 1;
            if queried == 2 {
                break;
            }
        }
    }
}

/// One reconcile pass.  True when it admitted a window the events had missed.
fn reconcile_visible_windows(
    handle: &BackendHandle,
    engine: &Arc<RwLock<TilingEngine>>,
    ipc_server: &Arc<IpcServer>,
) -> bool {
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

    refresh_native_bounds_batch(handle, engine);
    if !added_any {
        // Re-hide any inactive-workspace windows that Windows re-shown outside
        // wiri's model. try_write keeps this maintenance loop from blocking IPC
        // or hotkey handlers.
        if let Some(mut eng) = engine.try_write() {
            eng.apply_all(handle);
        }
    }
    added_any
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

#[cfg(test)]
mod c2_cli_tests {
    use super::*;
    #[test]
    fn offline_commands_accept_niri_syntax() {
        let args = Args::try_parse_from(["wiri", "validate", "-c", "test.kdl"]).unwrap();
        assert!(matches!(args.command, Some(OfflineCommand::Validate)));
        assert_eq!(args.config, Some(PathBuf::from("test.kdl")));
        for shell in ["bash", "elvish", "fish", "powershell", "zsh"] {
            let args = Args::try_parse_from(["wiri", "completions", shell]).unwrap();
            assert!(matches!(
                args.command,
                Some(OfflineCommand::Completions { .. })
            ));
        }
    }
}
