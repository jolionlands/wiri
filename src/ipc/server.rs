// IPC server: named-pipe creation loop, client handler, message processor,
// event broadcaster, and pipe security attributes helper.
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};

use super::messages::{BindingInfo, IpcEvent, IpcMessage, MonitorInfo, WorkspaceInfo};
use super::PIPE_PATH;
use crate::layout::TilingEngine;
use crate::utils::{Rect, WindowId};

/// How long a client waits for the answer to one request before it is told
/// that wiri is busy.  `wiri-ctl` and scripts rely on a reply within about two
/// seconds, whatever the window manager is doing.
const IPC_RESPONSE_BUDGET: std::time::Duration = std::time::Duration::from_millis(1500);

pub struct IpcServer {
    event_tx: broadcast::Sender<IpcEvent>,
    engine: Option<Arc<parking_lot::RwLock<TilingEngine>>>,
    /// Backend handle so IPC handlers can drive engine state-changing methods
    /// (switch_workspace, focus_window, etc.) which need a real backend to
    /// show/hide/position windows.
    backend: Option<crate::backend::BackendHandle>,
    /// Optional one-shot sender to request graceful shutdown of main.
    shutdown_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// Optional sender to request a config reload on the main loop.
    reload_tx: parking_lot::Mutex<Option<mpsc::UnboundedSender<()>>>,
}

impl IpcServer {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(64);
        Self {
            event_tx,
            engine: None,
            backend: None,
            shutdown_tx: parking_lot::Mutex::new(None),
            reload_tx: parking_lot::Mutex::new(None),
        }
    }

    /// Connect this IPC server to the tiling engine so it can query real state
    pub fn set_engine(&mut self, engine: Arc<parking_lot::RwLock<TilingEngine>>) {
        self.engine = Some(engine);
    }

    /// Provide the backend handle so engine state-changing IPC handlers can
    /// drive show/hide/position calls on real windows.
    pub fn set_backend(&mut self, backend: crate::backend::BackendHandle) {
        self.backend = Some(backend);
    }

    /// Provide a oneshot sender that will signal main to shut down gracefully
    /// when an IPC Quit message is received.
    pub fn set_shutdown_sender(&self, tx: tokio::sync::oneshot::Sender<()>) {
        *self.shutdown_tx.lock() = Some(tx);
    }

    /// Provide a sender that asks main to reload config from disk.
    pub fn set_reload_sender(&self, tx: mpsc::UnboundedSender<()>) {
        *self.reload_tx.lock() = Some(tx);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<IpcEvent> {
        self.event_tx.subscribe()
    }

    pub fn broadcast_event(&self, event: IpcEvent) {
        let _ = self.event_tx.send(event);
    }

    /// Start the IPC server listening on the named pipe.
    /// This spawns a background task that accepts connections.
    ///
    /// Security: the named pipe is created with a DACL that grants only
    /// `BUILTIN\Administrators` and the file owner full access — preventing
    /// other local users from connecting via `\\.\pipe\wiri_control` even on
    /// a shared workstation.  Implemented via tokio's
    /// `ServerOptions::create_with_security_attributes_raw` so the ACL applies
    /// to every pipe instance, not just the first.
    ///
    /// **Item 2 (Option A)** — `SECURITY_ATTRIBUTES` contains a raw
    /// `*mut c_void` ACL pointer which is `!Send`.  We never hold one across
    /// `.await`: every pipe instance is built by the synchronous helper
    /// [`create_pipe_with_sa`] which builds the SA, calls
    /// `ServerOptions::create_with_security_attributes_raw`, and drops the SA
    /// before returning.  Each iteration of the accept loop runs the
    /// synchronous build first, *then* awaits `connect`.  The resulting future
    /// is `Send`, so plain `tokio::spawn` on the multi-thread runtime is fine.
    pub async fn run(self: Arc<Self>) -> Result<()> {
        info!(
            "IPC server starting on {} (ACL: Admins+Owner only)",
            PIPE_PATH
        );

        const MAX_CREATE_RETRIES: u32 = 5;
        let mut retry_count = 0u32;
        let mut first_instance = true;

        loop {
            // Synchronous pipe creation — SECURITY_ATTRIBUTES is materialised
            // and dropped entirely within this call, before any `.await` below.
            let create_result = create_pipe_with_sa(first_instance);
            let server = match create_result {
                Ok(s) => {
                    retry_count = 0;
                    first_instance = false; // Subsequent instances must not set first_pipe_instance.
                    s
                }
                Err(e) if first_instance => {
                    // First-instance attempt failed — fall back to not requesting
                    // first_pipe_instance in case another (stale) handle exists.
                    match create_pipe_with_sa(false) {
                        Ok(s) => {
                            retry_count = 0;
                            first_instance = false;
                            s
                        }
                        Err(e2) => {
                            retry_count += 1;
                            warn!(
                                "Failed to create named pipe (attempt {}/{}): {} / {}",
                                retry_count, MAX_CREATE_RETRIES, e, e2
                            );
                            if retry_count >= MAX_CREATE_RETRIES {
                                error!(
                                    "IPC pipe creation failed {} times — giving up. \
                                     Is another wiri instance already running?",
                                    MAX_CREATE_RETRIES
                                );
                                return Err(anyhow::anyhow!(
                                    "Cannot create IPC pipe after {} attempts: {} / {}",
                                    MAX_CREATE_RETRIES,
                                    e,
                                    e2
                                ));
                            }
                            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                            continue;
                        }
                    }
                }
                Err(e) => {
                    retry_count += 1;
                    warn!(
                        "Failed to create named pipe (attempt {}/{}): {}",
                        retry_count, MAX_CREATE_RETRIES, e
                    );
                    if retry_count >= MAX_CREATE_RETRIES {
                        error!(
                            "IPC pipe creation failed {} times — giving up.",
                            MAX_CREATE_RETRIES
                        );
                        return Err(anyhow::anyhow!(
                            "Cannot create IPC pipe after {} attempts: {}",
                            MAX_CREATE_RETRIES,
                            e
                        ));
                    }
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    continue;
                }
            };

            // Wait for a client to connect.  `server` (a NamedPipeServer) is
            // Send-safe; SECURITY_ATTRIBUTES has already been dropped.
            if let Err(e) = server.connect().await {
                warn!("Pipe connect error: {}", e);
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                continue;
            }

            info!("IPC client connected");

            // Spawn a task to handle this client
            let ipc = Arc::clone(&self);
            tokio::spawn(async move {
                if let Err(e) = ipc.handle_client(server).await {
                    debug!("IPC client handler error: {}", e);
                }
            });
        }
    }

    async fn handle_client(
        self: Arc<Self>,
        mut pipe: tokio::net::windows::named_pipe::NamedPipeServer,
    ) -> Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut buffer = vec![0u8; 65536];

        loop {
            let n = match pipe.read(&mut buffer).await {
                Ok(0) => {
                    // Client disconnected
                    info!("IPC client disconnected");
                    return Ok(());
                }
                Ok(n) => n,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::BrokenPipe {
                        info!("IPC client disconnected (broken pipe)");
                        return Ok(());
                    }
                    return Err(e.into());
                }
            };

            // Parse the message
            let message: IpcMessage = match serde_json::from_slice(&buffer[..n]) {
                Ok(msg) => msg,
                Err(e) => {
                    warn!("IPC: Failed to parse message: {}", e);
                    let error_response = serde_json::json!({
                        "success": false,
                        "error": format!("Invalid message: {}", e)
                    });
                    let response_bytes = serde_json::to_vec(&error_response)?;
                    let _ = pipe.write_all(&response_bytes).await;
                    continue;
                }
            };

            debug!("IPC received: {:?}", message);

            // SubscribeEvents is handled inline: ack the subscription, then
            // forward broadcast events to the client pipe until it disconnects.
            if let IpcMessage::SubscribeEvents { ref event_types } = message {
                info!("IPC: Subscribe to events: {:?}", event_types);
                let mut ack = serde_json::to_vec(&serde_json::json!({
                    "success": true,
                    "subscription_id": 1
                }))?;
                ack.push(b'\n');
                let _ = pipe.write_all(&ack).await;

                // Subscribe to the broadcast channel (64-entry buffer per spec)
                let mut event_rx = self.event_tx.subscribe();
                loop {
                    match event_rx.recv().await {
                        Ok(event) => {
                            let value = match serde_json::to_value(&event) {
                                Ok(v) => v,
                                Err(e) => {
                                    warn!("IPC: Failed to encode event: {}", e);
                                    continue;
                                }
                            };
                            if !subscription_allows_event(event_types, &value) {
                                continue;
                            }
                            let mut bytes = match serde_json::to_vec(&value) {
                                Ok(b) => b,
                                Err(e) => {
                                    warn!("IPC: Failed to serialize event: {}", e);
                                    continue;
                                }
                            };
                            bytes.push(b'\n');
                            if pipe.write_all(&bytes).await.is_err() {
                                debug!("IPC event subscriber disconnected");
                                return Ok(());
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            warn!("IPC event subscriber lagged by {} events", n);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            debug!("IPC event broadcast channel closed");
                            return Ok(());
                        }
                    }
                }
            }

            // Process the message and send response.
            //
            // The handlers take the engine lock and make Win32 calls, which can
            // wait on a busy or hung application.  Run them on a blocking thread
            // (so they never pin a runtime worker) and answer within
            // `IPC_RESPONSE_BUDGET` whatever happens: `wiri-ctl` must never hang.
            let response = self.respond(message).await;
            let response_bytes = serde_json::to_vec(&response)?;
            let _ = pipe.write_all(&response_bytes).await;
            // Flush + disconnect: ctl uses read_to_end which waits for EOF.
            // Each ctl invocation opens a fresh pipe; one round-trip per connection.
            let _ = pipe.flush().await;
            let _ = pipe.shutdown().await;
            return Ok(());
        }
    }

    /// Run `process_message` on a blocking thread and reply within
    /// [`IPC_RESPONSE_BUDGET`].  If the handler is still waiting (typically for
    /// the engine lock while a layout pass is stuck on an unresponsive window)
    /// the client gets a clear "busy" error instead of a hung pipe; the request
    /// itself stays queued and takes effect when the lock frees up.
    async fn respond(self: &Arc<Self>, message: IpcMessage) -> serde_json::Value {
        let ipc = Arc::clone(self);
        let work = tokio::task::spawn_blocking(move || ipc.process_message(message));
        match tokio::time::timeout(IPC_RESPONSE_BUDGET, work).await {
            Ok(Ok(value)) => value,
            Ok(Err(join_error)) => {
                error!("IPC handler panicked: {}", join_error);
                serde_json::json!({
                    "success": false,
                    "error": format!("internal error while handling the request: {}", join_error)
                })
            }
            Err(_) => {
                warn!(
                    "IPC request still running after {:?}; replying 'busy' (engine lock held?)",
                    IPC_RESPONSE_BUDGET
                );
                serde_json::json!({
                    "success": false,
                    "error": "wiri is busy (the window layout is being updated); \
                              the request is queued and will be handled shortly"
                })
            }
        }
    }

    fn process_message(&self, message: IpcMessage) -> serde_json::Value {
        match message {
            IpcMessage::GetState => {
                if let Some(engine) = &self.engine {
                    let eng = engine.read();
                    let windows = build_window_infos(&eng);
                    let mut workspaces = Vec::new();
                    for (_oid, monitor) in eng.monitors().iter() {
                        let ws = monitor.workspace();
                        workspaces.push(WorkspaceInfo {
                            name: format!(
                                "workspace-{}",
                                crate::layout::workspace_number(monitor.active_workspace_id())
                            ),
                            id: crate::layout::workspace_number(monitor.active_workspace_id())
                                as u32,
                            window_count: ws
                                .map(|w| w.columns.iter().map(|c| c.tiles.len()).sum())
                                .unwrap_or(0),
                        });
                    }
                    let active_workspace = workspaces.first().map(|w| w.name.clone());
                    serde_json::json!({
                        "success": true,
                        "result": {
                            "windows": windows,
                            "workspaces": workspaces,
                            "active_workspace": active_workspace
                        }
                    })
                } else {
                    serde_json::json!({
                        "success": true,
                        "result": {
                            "windows": [],
                            "workspaces": [],
                            "active_workspace": null
                        }
                    })
                }
            }
            IpcMessage::WindowList => {
                if let Some(engine) = &self.engine {
                    let eng = engine.read();
                    let windows = build_window_infos(&eng);
                    serde_json::json!({"success": true, "result": windows})
                } else {
                    serde_json::json!({"success": true, "result": []})
                }
            }
            IpcMessage::SwitchWorkspace { id } => {
                info!("IPC: Switch to workspace {}", id);
                match (&self.engine, &self.backend) {
                    (Some(engine), Some(backend)) => {
                        // `id` is the 1-based number users type (see
                        // `crate::layout::workspace_id_from_number`).
                        engine
                            .write()
                            .switch_workspace(crate::layout::workspace_id_from_number(id), backend);
                        serde_json::json!({
                            "success": true,
                            "result": {"workspace_id": id}
                        })
                    }
                    _ => serde_json::json!({
                        "success": false,
                        "error": "engine or backend not initialized"
                    }),
                }
            }
            IpcMessage::FocusWindow { window_hwnd } => {
                info!("IPC: Focus window {}", window_hwnd);
                match (&self.engine, &self.backend) {
                    (Some(engine), Some(backend)) => {
                        if !focus_window_by_hwnd(engine, backend, window_hwnd) {
                            return serde_json::json!({
                                "success": false,
                                "error": format!("Unknown tiled window: hwnd={}", window_hwnd)
                            });
                        }
                        engine.write().apply_all(backend);
                        serde_json::json!({"success": true})
                    }
                    _ => serde_json::json!({
                        "success": false,
                        "error": "engine or backend not initialized"
                    }),
                }
            }
            IpcMessage::CloseWindow { window_hwnd } => {
                info!("IPC: Close window {}", window_hwnd);
                use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
                use windows::Win32::UI::WindowsAndMessaging::{IsWindow, PostMessageW};
                let hwnd = HWND(window_hwnd as *mut std::ffi::c_void);
                // Validate the HWND before posting any message
                if !unsafe { IsWindow(hwnd).as_bool() } {
                    warn!(
                        "IPC CloseWindow: HWND {} is not a valid window",
                        window_hwnd
                    );
                    return serde_json::json!({
                        "success": false,
                        "error": format!("Invalid HWND: {}", window_hwnd)
                    });
                }
                unsafe {
                    let _ = PostMessageW(
                        hwnd,
                        windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
                serde_json::json!({"success": true})
            }
            IpcMessage::ReloadConfig => {
                info!("IPC: Reload config requested");
                if let Some(tx) = self.reload_tx.lock().as_ref() {
                    match tx.send(()) {
                        Ok(()) => serde_json::json!({"success": true}),
                        Err(_) => serde_json::json!({
                            "success": false,
                            "error": "reload channel is closed"
                        }),
                    }
                } else {
                    serde_json::json!({
                        "success": false,
                        "error": "reload channel not initialized"
                    })
                }
            }
            IpcMessage::Quit => {
                info!("IPC: Quit requested — sending shutdown signal to main");
                if let Some(tx) = self.shutdown_tx.lock().take() {
                    let _ = tx.send(());
                } else {
                    // Fallback if no shutdown channel was wired
                    warn!("IPC Quit: no shutdown channel set; calling process::exit");
                    std::process::exit(0);
                }
                serde_json::json!({"success": true})
            }
            IpcMessage::RunAction { name, args } => {
                info!("IPC: RunAction {} {:?}", name, args);
                let Some(action) = crate::input::parse_action_name(&name, &args) else {
                    return serde_json::json!({
                        "success": false,
                        "error": format!("unknown action or bad arguments: {} {:?}", name, args)
                    });
                };
                if matches!(action, crate::input::Action::Quit) {
                    return self.process_message(IpcMessage::Quit);
                }
                match (&self.engine, &self.backend) {
                    (Some(engine), Some(backend)) => {
                        // No quit channel: `quit` is handled above.
                        let no_quit = Arc::new(parking_lot::Mutex::new(None));
                        crate::backend::message_loop::perform_action(
                            &action, engine, backend, &no_quit,
                        );
                        serde_json::json!({"success": true})
                    }
                    _ => serde_json::json!({
                        "success": false,
                        "error": "engine or backend not initialized"
                    }),
                }
            }
            IpcMessage::SubscribeEvents { event_types } => {
                info!("IPC: Subscribe to events: {:?}", event_types);
                serde_json::json!({"success": true, "subscription_id": 1})
            }
            IpcMessage::WindowMove { window_hwnd, x, y } => {
                info!("IPC: Move window {} to ({}, {})", window_hwnd, x, y);
                let Some(current) = self.current_window_bounds(window_hwnd) else {
                    return serde_json::json!({
                        "success": false,
                        "error": format!("Invalid or unknown HWND: {}", window_hwnd)
                    });
                };
                let rect = Rect::new(x, y, current.size.w, current.size.h);
                match self.apply_ipc_window_rect(window_hwnd, rect) {
                    Ok(()) => {
                        self.broadcast_event(IpcEvent::WindowMoved {
                            window_hwnd,
                            x,
                            y,
                            width: rect.size.w,
                            height: rect.size.h,
                        });
                        serde_json::json!({"success": true})
                    }
                    Err(e) => serde_json::json!({"success": false, "error": e}),
                }
            }
            IpcMessage::WindowResize {
                window_hwnd,
                width,
                height,
            } => {
                info!("IPC: Resize window {} to {}x{}", window_hwnd, width, height);
                if width == 0 || height == 0 {
                    return serde_json::json!({
                        "success": false,
                        "error": "width and height must be greater than zero"
                    });
                }
                let Some(current) = self.current_window_bounds(window_hwnd) else {
                    return serde_json::json!({
                        "success": false,
                        "error": format!("Invalid or unknown HWND: {}", window_hwnd)
                    });
                };
                let rect = Rect::new(current.loc.x, current.loc.y, width, height);
                match self.apply_ipc_window_rect(window_hwnd, rect) {
                    Ok(()) => {
                        self.broadcast_event(IpcEvent::WindowResized {
                            window_hwnd,
                            width,
                            height,
                        });
                        serde_json::json!({"success": true})
                    }
                    Err(e) => serde_json::json!({"success": false, "error": e}),
                }
            }
            IpcMessage::TileRequest {
                window_hwnd,
                target_workspace,
            } => {
                info!(
                    "IPC: TileRequest for window {} workspace {:?}",
                    window_hwnd, target_workspace
                );
                // Bring the window back into tiling: if it's currently floating
                // we toggle it; if a target workspace name was provided we
                // additionally move it there.
                match (&self.engine, &self.backend) {
                    (Some(engine), Some(backend)) => {
                        if !focus_window_by_hwnd(engine, backend, window_hwnd) {
                            return serde_json::json!({
                                "success": false,
                                "error": format!("Unknown tiled window: hwnd={}", window_hwnd)
                            });
                        }
                        // Un-float if floating.
                        let wid = crate::utils::WindowId::new(window_hwnd);
                        if engine.read().is_floating(wid) {
                            engine.write().toggle_floating_for(wid, backend);
                        }
                        // Optional workspace move (by name or numeric id).
                        if let Some(name) = target_workspace.as_deref() {
                            let target_id = name
                                .parse::<i32>()
                                .ok()
                                .map(crate::layout::workspace_id_from_number)
                                .or_else(|| engine.read().workspace_id_by_name(name));
                            let Some(target_id) = target_id else {
                                return serde_json::json!({
                                    "success": false,
                                    "error": format!("Unknown workspace: {}", name)
                                });
                            };
                            engine.write().move_window_to_workspace(target_id, backend);
                        }
                        engine.write().apply_all(backend);
                        serde_json::json!({"success": true, "result": {"hwnd": window_hwnd}})
                    }
                    _ => serde_json::json!({
                        "success": false,
                        "error": "engine or backend not initialized"
                    }),
                }
            }
            IpcMessage::WindowMoveToWorkspace {
                window_hwnd,
                workspace_id,
            } => {
                info!(
                    "IPC: Move window {} to workspace {}",
                    window_hwnd, workspace_id
                );
                match (&self.engine, &self.backend) {
                    (Some(engine), Some(backend)) => {
                        if !focus_window_by_hwnd(engine, backend, window_hwnd) {
                            return serde_json::json!({
                                "success": false,
                                "error": format!("Unknown tiled window: hwnd={}", window_hwnd)
                            });
                        }
                        let wid = crate::utils::WindowId::new(window_hwnd);
                        let workspace = crate::layout::workspace_id_from_number(workspace_id);
                        if engine.read().is_floating(wid) {
                            engine.write().float_move_to_workspace(wid, workspace, backend);
                        } else {
                            engine.write().move_window_to_workspace(workspace, backend);
                        }
                        engine.write().apply_all(backend);
                        serde_json::json!({"success": true})
                    }
                    _ => serde_json::json!({
                        "success": false,
                        "error": "engine or backend not initialized"
                    }),
                }
            }
            IpcMessage::WindowFloat { window_hwnd } => {
                info!("IPC: Float window {}", window_hwnd);
                match (&self.engine, &self.backend) {
                    (Some(engine), Some(backend)) => {
                        if !focus_window_by_hwnd(engine, backend, window_hwnd) {
                            return serde_json::json!({
                                "success": false,
                                "error": format!("Unknown tiled window: hwnd={}", window_hwnd)
                            });
                        }
                        let wid = crate::utils::WindowId::new(window_hwnd);
                        // Only toggle if not already floating, so this is idempotent.
                        if !engine.read().is_floating(wid) {
                            engine.write().toggle_floating_for(wid, backend);
                        }
                        serde_json::json!({"success": true, "floating": true})
                    }
                    _ => serde_json::json!({
                        "success": false,
                        "error": "engine or backend not initialized"
                    }),
                }
            }
            IpcMessage::WindowUnfloat { window_hwnd } => {
                info!("IPC: Unfloat window {}", window_hwnd);
                match (&self.engine, &self.backend) {
                    (Some(engine), Some(backend)) => {
                        if !focus_window_by_hwnd(engine, backend, window_hwnd) {
                            return serde_json::json!({
                                "success": false,
                                "error": format!("Unknown tiled window: hwnd={}", window_hwnd)
                            });
                        }
                        let wid = crate::utils::WindowId::new(window_hwnd);
                        if engine.read().is_floating(wid) {
                            engine.write().toggle_floating_for(wid, backend);
                        }
                        serde_json::json!({"success": true, "floating": false})
                    }
                    _ => serde_json::json!({
                        "success": false,
                        "error": "engine or backend not initialized"
                    }),
                }
            }
            IpcMessage::WindowFind { query } => {
                info!("IPC: Find window query={:?}", query);
                serde_json::json!({"success": true, "result": []})
            }
            IpcMessage::WorkspaceList => {
                if let Some(engine) = &self.engine {
                    let eng = engine.read();
                    let workspaces: Vec<_> = eng
                        .monitors()
                        .iter()
                        .map(|(_oid, m)| WorkspaceInfo {
                            name: format!(
                                "workspace-{}",
                                crate::layout::workspace_number(m.active_workspace_id())
                            ),
                            id: crate::layout::workspace_number(m.active_workspace_id()) as u32,
                            window_count: m
                                .workspace()
                                .map(|w| w.columns.iter().map(|c| c.tiles.len()).sum())
                                .unwrap_or(0),
                        })
                        .collect();
                    serde_json::json!({"success": true, "result": workspaces})
                } else {
                    serde_json::json!({"success": true, "result": []})
                }
            }
            IpcMessage::WorkspaceCreate { id } => {
                info!("IPC: Create workspace {:?}", id);
                serde_json::json!({"success": true})
            }
            IpcMessage::WorkspaceDelete { id } => {
                info!("IPC: Delete workspace {}", id);
                serde_json::json!({"success": true})
            }
            IpcMessage::PresetList => {
                serde_json::json!({"success": true, "result": []})
            }
            IpcMessage::PresetSave { name } => {
                info!("IPC: Save preset {:?}", name);
                serde_json::json!({"success": true})
            }
            IpcMessage::PresetLoad { name } => {
                info!("IPC: Load preset {:?}", name);
                serde_json::json!({"success": true})
            }
            IpcMessage::PresetDelete { name } => {
                info!("IPC: Delete preset {:?}", name);
                serde_json::json!({"success": true})
            }
            IpcMessage::LayoutExport => {
                serde_json::json!({"success": true, "result": {}})
            }
            IpcMessage::MoveWindowToWorkspace { workspace_id } => {
                info!("IPC: MoveWindowToWorkspace {}", workspace_id);
                match (&self.engine, &self.backend) {
                    (Some(engine), Some(backend)) => {
                        engine.write().move_window_to_workspace(
                            crate::layout::workspace_id_from_number(workspace_id),
                            backend,
                        );
                        engine.write().apply_all(backend);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine or backend not initialized"})
                    }
                }
            }
            IpcMessage::MoveWindowToMonitor { direction } => {
                info!("IPC: MoveWindowToMonitor direction={}", direction);
                match (&self.engine, &self.backend) {
                    (Some(engine), Some(backend)) => {
                        let dir = match direction.to_lowercase().as_str() {
                            "right" => crate::layout::ScrollDirection::Right,
                            _ => crate::layout::ScrollDirection::Left,
                        };
                        engine.write().move_window_to_monitor(dir, backend);
                        engine.write().apply_all(backend);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine or backend not initialized"})
                    }
                }
            }
            IpcMessage::CenterColumn => {
                info!("IPC: CenterColumn");
                match (&self.engine, &self.backend) {
                    (Some(engine), Some(backend)) => {
                        engine.write().center_focused_column(backend);
                        engine.write().apply_all(backend);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine or backend not initialized"})
                    }
                }
            }
            IpcMessage::SetColumnWidth { preset } => {
                info!("IPC: SetColumnWidth preset={}", preset);
                match (&self.engine, &self.backend) {
                    (Some(engine), Some(backend)) => {
                        let p = match preset.to_lowercase().as_str() {
                            "1/4" | "quarter" => crate::layout::ColumnWidthPreset::OneQuarter,
                            "1/3" | "third" => crate::layout::ColumnWidthPreset::OneThird,
                            "1/2" | "half" => crate::layout::ColumnWidthPreset::Half,
                            "2/3" | "two-thirds" => crate::layout::ColumnWidthPreset::TwoThirds,
                            "3/4" | "three-quarters" => {
                                crate::layout::ColumnWidthPreset::ThreeQuarters
                            }
                            "full" | "100" => crate::layout::ColumnWidthPreset::Full,
                            _ => crate::layout::ColumnWidthPreset::Cycle,
                        };
                        engine.write().set_column_width_preset(p, backend);
                        engine.write().apply_all(backend);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine or backend not initialized"})
                    }
                }
            }
            IpcMessage::ResizeColumn { delta_px } => {
                info!("IPC: ResizeColumn delta_px={}", delta_px);
                match (&self.engine, &self.backend) {
                    (Some(engine), Some(backend)) => {
                        engine.write().resize_focused_column_by(delta_px, backend);
                        engine.write().apply_all(backend);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine or backend not initialized"})
                    }
                }
            }
            IpcMessage::FocusWorkspaceNext => {
                info!("IPC: FocusWorkspaceNext");
                match (&self.engine, &self.backend) {
                    (Some(engine), Some(backend)) => {
                        engine.write().focus_workspace_relative(
                            crate::layout::WorkspaceDirection::Next,
                            backend,
                        );
                        engine.write().apply_all(backend);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine or backend not initialized"})
                    }
                }
            }
            IpcMessage::FocusWorkspacePrevious => {
                info!("IPC: FocusWorkspacePrevious");
                match (&self.engine, &self.backend) {
                    (Some(engine), Some(backend)) => {
                        engine.write().focus_workspace_relative(
                            crate::layout::WorkspaceDirection::Previous,
                            backend,
                        );
                        engine.write().apply_all(backend);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine or backend not initialized"})
                    }
                }
            }
            IpcMessage::FocusPrevious => {
                info!("IPC: FocusPrevious");
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        e.write().focus_previous_window(b);
                        e.write().apply_all(b);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine/backend not initialized"})
                    }
                }
            }
            IpcMessage::ToggleAlwaysOnTop => {
                info!("IPC: ToggleAlwaysOnTop");
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        e.write().toggle_always_on_top_for_focused(b);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine/backend not initialized"})
                    }
                }
            }
            IpcMessage::FocusWorkspaceNamed { name } => {
                info!("IPC: FocusWorkspaceNamed {:?}", name);
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        e.write().focus_workspace_named(&name, b);
                        e.write().apply_all(b);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine/backend not initialized"})
                    }
                }
            }
            IpcMessage::SetAutoTileThreshold { threshold } => {
                info!("IPC: SetAutoTileThreshold {:?}", threshold);
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        e.write().set_auto_tile_threshold(threshold);
                        e.write().apply_all(b);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine/backend not initialized"})
                    }
                }
            }
            IpcMessage::MonitorList => {
                // Enumerate every monitor with its bounds, work area, DPI
                // scale, active workspace, and whether it is the focused
                // output. Pure read-only — no engine mutation.
                if let Some(engine) = &self.engine {
                    let eng = engine.read();
                    let focused = eng.focused_output();
                    let mut out: Vec<MonitorInfo> = eng
                        .monitors()
                        .iter()
                        .map(|(oid, m)| {
                            let window_count: usize = m
                                .workspaces
                                .values()
                                .flat_map(|ws| ws.columns.iter())
                                .map(|c| c.tiles.len())
                                .sum();
                            MonitorInfo {
                                output_id: oid.as_u64(),
                                bounds_x: m.bounds.loc.x,
                                bounds_y: m.bounds.loc.y,
                                bounds_width: m.bounds.size.w,
                                bounds_height: m.bounds.size.h,
                                work_area_x: m.work_area.loc.x,
                                work_area_y: m.work_area.loc.y,
                                work_area_width: m.work_area.size.w,
                                work_area_height: m.work_area.size.h,
                                scale_factor: m.scale_factor,
                                active_workspace: crate::layout::workspace_number(
                                    m.active_workspace_id(),
                                ),
                                window_count,
                                focused: Some(*oid) == focused,
                            }
                        })
                        .collect();
                    // Stable order: sort by x-coordinate of bounds so the user
                    // sees left-to-right monitors in left-to-right output.
                    out.sort_by_key(|m| (m.bounds_x, m.bounds_y));
                    serde_json::json!({"success": true, "result": out})
                } else {
                    serde_json::json!({"success": true, "result": []})
                }
            }
            IpcMessage::SpawnCommand { command, args } => {
                info!("IPC: SpawnCommand {:?} {:?}", command, args);
                let argv = crate::hooks::spawner::spawn_command_argv(&command, &args);
                if argv.is_empty() {
                    return serde_json::json!({"success": false, "error": "empty command"});
                }
                match crate::hooks::spawner::spawn_argv(&argv) {
                    Ok(pid) => serde_json::json!({"success": true, "pid": pid}),
                    Err(e) => serde_json::json!({"success": false, "error": format!("{:#}", e)}),
                }
            }
            IpcMessage::ConsumeWindow => {
                info!("IPC: ConsumeWindow");
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        e.write().consume_window_into_column(b);
                        e.write().apply_all(b);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine/backend not initialized"})
                    }
                }
            }
            IpcMessage::ExpelWindow => {
                info!("IPC: ExpelWindow");
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        e.write().expel_window_from_column(b);
                        e.write().apply_all(b);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine/backend not initialized"})
                    }
                }
            }
            IpcMessage::ExpandColumn => {
                info!("IPC: ExpandColumn");
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        e.write().expand_column_to_available(b);
                        e.write().apply_all(b);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine/backend not initialized"})
                    }
                }
            }
            IpcMessage::MaximizeColumn => {
                info!("IPC: MaximizeColumn");
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        e.write().toggle_maximize_focused_column(b);
                        e.write().apply_all(b);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine/backend not initialized"})
                    }
                }
            }
            IpcMessage::GrowColumn => {
                info!("IPC: GrowColumn");
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        e.write().resize_focused_column_by_percent(5, b);
                        e.write().apply_all(b);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine/backend not initialized"})
                    }
                }
            }
            IpcMessage::ShrinkColumn => {
                info!("IPC: ShrinkColumn");
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        e.write().resize_focused_column_by_percent(-5, b);
                        e.write().apply_all(b);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine/backend not initialized"})
                    }
                }
            }
            IpcMessage::GrowTile => {
                info!("IPC: GrowTile");
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        e.write().resize_focused_tile_height_by_percent(5, b);
                        e.write().apply_all(b);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine/backend not initialized"})
                    }
                }
            }
            IpcMessage::ShrinkTile => {
                info!("IPC: ShrinkTile");
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        e.write().resize_focused_tile_height_by_percent(-5, b);
                        e.write().apply_all(b);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine/backend not initialized"})
                    }
                }
            }
            IpcMessage::ListBindings => {
                // Pull the snapshot from the message-loop module; never blocks,
                // never touches the engine, no backend lookups.  Includes
                // bindings that successfully registered with Windows so users
                // see what the daemon really listens for (vs the config file).
                let raw = crate::backend::message_loop::current_bindings();
                let bindings: Vec<BindingInfo> = raw
                    .into_iter()
                    .map(|b| BindingInfo {
                        id: b.id,
                        modifiers: b.modifiers,
                        vk_code: b.vk_code,
                        action: b.action,
                        chord: format_chord(b.modifiers, b.vk_code),
                    })
                    .collect();
                serde_json::json!({"success": true, "result": { "bindings": bindings }})
            }
            IpcMessage::MoveColumnToMonitor { direction } => {
                info!("IPC: MoveColumnToMonitor direction={}", direction);
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        let dir = match direction.to_lowercase().as_str() {
                            "right" => crate::layout::ScrollDirection::Right,
                            _ => crate::layout::ScrollDirection::Left,
                        };
                        e.write().move_column_to_monitor(dir, b);
                        e.write().apply_all(b);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine/backend not initialized"})
                    }
                }
            }
            IpcMessage::SaveSnapshot { name } => {
                info!("IPC: SaveSnapshot name={}", name);
                match &self.engine {
                    Some(e) => {
                        let snap = e.read().snapshot();
                        match crate::layout::snapshot::save_to(&name, &snap) {
                            Ok(path) => serde_json::json!({
                                "success": true,
                                "result": { "path": path.to_string_lossy(), "name": name },
                            }),
                            Err(e) => serde_json::json!({
                                "success": false,
                                "error": format!("save failed: {}", e),
                            }),
                        }
                    }
                    None => serde_json::json!({
                        "success": false,
                        "error": "engine not initialized",
                    }),
                }
            }
            IpcMessage::LoadSnapshot { name } => {
                info!("IPC: LoadSnapshot name={}", name);
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => match crate::layout::snapshot::load_from(&name) {
                        Ok(snap) => {
                            let mut eng = e.write();
                            eng.apply_snapshot(&snap);
                            eng.apply_all(b);
                            serde_json::json!({
                                "success": true,
                                "result": { "name": name, "monitors": snap.monitors.len() },
                            })
                        }
                        Err(e) => serde_json::json!({
                            "success": false,
                            "error": format!("load failed: {}", e),
                        }),
                    },
                    _ => serde_json::json!({
                        "success": false,
                        "error": "engine/backend not initialized",
                    }),
                }
            }
            IpcMessage::ListSnapshots => {
                info!("IPC: ListSnapshots");
                match crate::layout::snapshot::list_names() {
                    Ok(names) => serde_json::json!({
                        "success": true,
                        "result": { "snapshots": names },
                    }),
                    Err(e) => serde_json::json!({
                        "success": false,
                        "error": format!("list failed: {}", e),
                    }),
                }
            }
            IpcMessage::DeleteSnapshot { name } => {
                info!("IPC: DeleteSnapshot name={}", name);
                match crate::layout::snapshot::delete(&name) {
                    Ok(path) => serde_json::json!({
                        "success": true,
                        "result": { "name": name, "path": path.to_string_lossy() },
                    }),
                    Err(e) => serde_json::json!({
                        "success": false,
                        "error": format!("delete failed: {}", e),
                    }),
                }
            }
            IpcMessage::ToggleResizeMode => {
                info!("IPC: ToggleResizeMode");
                match &self.engine {
                    Some(e) => {
                        let active = e.write().toggle_resize_mode();
                        serde_json::json!({
                            "success": true,
                            "result": { "active": active },
                        })
                    }
                    None => serde_json::json!({
                        "success": false,
                        "error": "engine not initialized",
                    }),
                }
            }
            IpcMessage::MoveColumnToWorkspaceUp => {
                info!("IPC: MoveColumnToWorkspaceUp");
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        e.write().move_focused_column_to_workspace(-1, b);
                        e.write().apply_all(b);
                        serde_json::json!({"success": true})
                    }
                    _ => serde_json::json!({
                        "success": false,
                        "error": "engine/backend not initialized",
                    }),
                }
            }
            IpcMessage::MoveColumnToWorkspaceDown => {
                info!("IPC: MoveColumnToWorkspaceDown");
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        e.write().move_focused_column_to_workspace(1, b);
                        e.write().apply_all(b);
                        serde_json::json!({"success": true})
                    }
                    _ => serde_json::json!({
                        "success": false,
                        "error": "engine/backend not initialized",
                    }),
                }
            }
            IpcMessage::MoveWorkspaceUp => {
                info!("IPC: MoveWorkspaceUp");
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        e.write().move_active_workspace(-1, b);
                        e.write().apply_all(b);
                        serde_json::json!({"success": true})
                    }
                    _ => serde_json::json!({
                        "success": false,
                        "error": "engine/backend not initialized",
                    }),
                }
            }
            IpcMessage::MoveWorkspaceDown => {
                info!("IPC: MoveWorkspaceDown");
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        e.write().move_active_workspace(1, b);
                        e.write().apply_all(b);
                        serde_json::json!({"success": true})
                    }
                    _ => serde_json::json!({
                        "success": false,
                        "error": "engine/backend not initialized",
                    }),
                }
            }
            IpcMessage::ToggleSticky => {
                info!("IPC: ToggleSticky");
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        e.write().toggle_sticky(b);
                        e.write().apply_all(b);
                        serde_json::json!({"success": true})
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine/backend not initialized"})
                    }
                }
            }
            IpcMessage::SetWorkspaceLayout { mode } => {
                info!("IPC: SetWorkspaceLayout mode={}", mode);
                match (&self.engine, &self.backend) {
                    (Some(e), Some(b)) => {
                        let mut eng = e.write();
                        match eng.set_workspace_layout(&mode) {
                            Ok(()) => {
                                eng.apply_all(b);
                                serde_json::json!({"success": true, "result": {"layout": mode}})
                            }
                            Err(err) => serde_json::json!({"success": false, "error": err}),
                        }
                    }
                    _ => {
                        serde_json::json!({"success": false, "error": "engine/backend not initialized"})
                    }
                }
            }
            IpcMessage::RenameWorkspace { workspace_id, name } => {
                info!("IPC: RenameWorkspace id={} name={:?}", workspace_id, name);
                match &self.engine {
                    Some(e) => match e.write().rename_workspace(
                        crate::layout::workspace_id_from_number(workspace_id),
                        &name,
                    ) {
                        Ok(()) => serde_json::json!({"success": true}),
                        Err(err) => serde_json::json!({"success": false, "error": err}),
                    },
                    None => {
                        serde_json::json!({"success": false, "error": "engine not initialized"})
                    }
                }
            }
            IpcMessage::ScreenshotWindow { hwnd, path } => {
                info!("IPC: ScreenshotWindow hwnd={:?} path={:?}", hwnd, path);
                // Resolve target HWND: explicit arg or foreground window.
                let target_hwnd: isize = match hwnd {
                    Some(h) => h,
                    None => {
                        let fg = unsafe {
                            windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow()
                        };
                        if fg.0.is_null() {
                            return serde_json::json!({
                                "success": false,
                                "error": "no foreground window to capture",
                            });
                        }
                        fg.0 as isize
                    }
                };
                // Resolve output path.
                let dest = match path.as_ref().map(std::path::PathBuf::from) {
                    Some(p) => p,
                    None => match crate::hooks::spawner::default_window_capture_path(target_hwnd) {
                        Ok(p) => p,
                        Err(e) => {
                            return serde_json::json!({
                                "success": false,
                                "error": format!("could not resolve default path: {}", e),
                            })
                        }
                    },
                };
                match crate::hooks::spawner::capture_window_to_file(target_hwnd, &dest) {
                    Ok(()) => serde_json::json!({
                        "success": true,
                        "result": { "path": dest.to_string_lossy() },
                    }),
                    Err(e) => serde_json::json!({
                        "success": false,
                        "error": e.to_string(),
                    }),
                }
            }
            IpcMessage::GetFocus => {
                info!("IPC: GetFocus");
                if let Some(engine) = &self.engine {
                    let eng = engine.read();
                    let focused_output = eng.focused_output();
                    let (window_hwnd, workspace_id, monitor_id) = match focused_output {
                        Some(oid) => {
                            let monitor = eng.monitors().get(&oid);
                            let ws_id = crate::layout::workspace_number(
                                monitor.map(|m| m.active_workspace_id()).unwrap_or(0),
                            );
                            let hwnd = monitor
                                .and_then(|m| m.focus_window)
                                .map(|wid| wid.as_isize());
                            (hwnd, ws_id, oid.as_u64())
                        }
                        None => (None, 1, 0u64),
                    };
                    serde_json::json!({
                        "success": true,
                        "result": {
                            "window_hwnd": window_hwnd,
                            "workspace_id": workspace_id,
                            "monitor_id": monitor_id,
                        },
                    })
                } else {
                    serde_json::json!({
                        "success": true,
                        "result": {
                            "window_hwnd": null,
                            "workspace_id": 0,
                            "monitor_id": 0,
                        },
                    })
                }
            }
            IpcMessage::GetWorkspaceList => {
                info!("IPC: GetWorkspaceList");
                if let Some(engine) = &self.engine {
                    let eng = engine.read();
                    let mut workspaces: Vec<serde_json::Value> = Vec::new();
                    for (_oid, monitor) in eng.monitors().iter() {
                        for (&ws_id, ws) in monitor.workspaces.iter() {
                            let window_count: usize =
                                ws.columns.iter().map(|c| c.tiles.len()).sum();
                            let name = eng
                                .workspace_name(ws_id)
                                .unwrap_or_else(|| {
                                    format!("workspace-{}", crate::layout::workspace_number(ws_id))
                                });
                            workspaces.push(serde_json::json!({
                                "id": crate::layout::workspace_number(ws_id),
                                "name": name,
                                "window_count": window_count,
                            }));
                        }
                    }
                    workspaces.sort_by_key(|w| w.get("id").and_then(|v| v.as_i64()).unwrap_or(0));
                    serde_json::json!({"success": true, "result": {"workspaces": workspaces}})
                } else {
                    serde_json::json!({"success": true, "result": {"workspaces": []}})
                }
            }
            IpcMessage::GetBindings => {
                // Return the live registered-bindings snapshot as a
                // chord+action table.  Never blocks; safe to call from any
                // context.  Returns an empty array when registration hasn't
                // completed yet so callers can fall back gracefully.
                let raw = crate::backend::message_loop::current_bindings();
                let entries: Vec<serde_json::Value> = raw
                    .iter()
                    .map(|b| {
                        serde_json::json!({
                            "chord": format_chord(b.modifiers, b.vk_code),
                            "modifiers": b.modifiers,
                            "vk_code": b.vk_code,
                            "action": b.action,
                        })
                    })
                    .collect();
                serde_json::json!({"success": true, "result": entries})
            }
            IpcMessage::ReserveArea {
                side,
                pixels,
                monitor_id: _,
            } => {
                // TODO(audit): per-monitor reservations (monitor_id ignored for v1)
                info!("IPC: ReserveArea side={} pixels={}", side, pixels);
                if let Some(engine) = &self.engine {
                    let mut new_config = engine.read().config().clone();
                    match side.as_str() {
                        "top" => new_config.reserve_top = pixels,
                        "bottom" => new_config.reserve_bottom = pixels,
                        "left" => new_config.reserve_left = pixels,
                        "right" => new_config.reserve_right = pixels,
                        _ => {
                            return serde_json::json!({
                                "success": false,
                                "error": format!("invalid side {:?}: expected top | bottom | left | right", side),
                            })
                        }
                    }
                    engine.write().update_config(new_config);
                    if let Some(backend) = &self.backend {
                        engine.write().apply_all(backend);
                    }
                    serde_json::json!({"success": true})
                } else {
                    serde_json::json!({"success": false, "error": "engine not initialised"})
                }
            }
            IpcMessage::CaptureWindow { window_hwnd, path } => {
                info!("IPC: CaptureWindow hwnd={} path={:?}", window_hwnd, path);
                // Validate that the HWND is one of the tracked windows so we
                // don't let an arbitrary process trigger captures of other
                // apps' windows through the daemon's pipe.
                let is_tracked = match &self.engine {
                    Some(e) => {
                        let eng = e.read();
                        let wid = crate::utils::WindowId::new(window_hwnd);
                        eng.tiled_windows().contains_key(&wid)
                    }
                    None => false,
                };
                if !is_tracked {
                    return serde_json::json!({
                        "success": false,
                        "error": format!("window {} is not tracked by wiri", window_hwnd),
                    });
                }
                // Resolve output path.
                let dest = match path.as_ref().map(std::path::PathBuf::from) {
                    Some(p) => p,
                    None => match crate::hooks::spawner::default_window_capture_path(window_hwnd) {
                        Ok(p) => p,
                        Err(e) => {
                            return serde_json::json!({
                                "success": false,
                                "error": format!("could not resolve default path: {}", e),
                            })
                        }
                    },
                };
                match crate::hooks::spawner::capture_window_to_file(window_hwnd, &dest) {
                    Ok(()) => serde_json::json!({
                        "success": true,
                        "result": { "path": dest.to_string_lossy() },
                    }),
                    Err(e) => serde_json::json!({
                        "success": false,
                        "error": e.to_string(),
                    }),
                }
            }
        }
    }

    fn current_window_bounds(&self, hwnd: isize) -> Option<Rect> {
        if let Some(backend) = &self.backend {
            if let Some(info) = backend.get_window(hwnd) {
                return Some(info.bounds);
            }
        }

        let window_id = WindowId::new(hwnd);
        if let Some(engine) = &self.engine {
            if let Some(info) = engine.read().tiled_windows().get(&window_id) {
                return Some(info.bounds);
            }
        }

        hwnd_bounds(hwnd)
    }

    fn apply_ipc_window_rect(&self, hwnd: isize, rect: Rect) -> std::result::Result<(), String> {
        use windows::Win32::UI::WindowsAndMessaging::{SWP_NOACTIVATE, SWP_NOZORDER};

        let Some(backend) = &self.backend else {
            return Err("backend not initialized".to_string());
        };

        backend
            .set_window_position(hwnd, rect, SWP_NOZORDER | SWP_NOACTIVATE)
            .map_err(|e| format!("SetWindowPos failed for {}: {}", hwnd, e))?;

        if let Some(mut info) = backend.get_window(hwnd) {
            info.bounds = rect;
            backend.update_window(hwnd, info);
        }
        if let Some(engine) = &self.engine {
            engine
                .write()
                .update_window_bounds(WindowId::new(hwnd), rect);
        }

        Ok(())
    }
}

impl Default for IpcServer {
    fn default() -> Self {
        Self::new()
    }
}

fn hwnd_bounds(hwnd: isize) -> Option<Rect> {
    use windows::Win32::Foundation::{HWND, RECT as WinRect};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsWindow};

    let hwnd = HWND(hwnd as *mut std::ffi::c_void);
    if !unsafe { IsWindow(hwnd).as_bool() } {
        return None;
    }

    let mut rect = WinRect::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return None;
    }

    Some(Rect::new(
        rect.left,
        rect.top,
        (rect.right - rect.left).max(0) as u32,
        (rect.bottom - rect.top).max(0) as u32,
    ))
}

/// Build the per-window reply used by both `GetState` and `WindowList`.
///
/// Returns `Vec<serde_json::Value>` rather than `Vec<WindowInfoIpc>` so the
/// wire format can include "workspace" + "floating" fields that the typed
/// struct intentionally does not carry (it stays minimal so library consumers
/// of `WindowInfoIpc` aren't forced to track engine-side state).  Walks every
/// monitor + workspace + column exactly once for O(W+T) total work.
fn build_window_infos(eng: &crate::layout::TilingEngine) -> Vec<serde_json::Value> {
    use crate::utils::WindowId;
    use std::collections::HashMap;

    let mut ws_of: HashMap<WindowId, i32> = HashMap::new();
    for (_oid, monitor) in eng.monitors().iter() {
        for (&ws_id, ws) in monitor.workspaces.iter() {
            for col in &ws.columns {
                for tile in &col.tiles {
                    ws_of.insert(tile.window_id, ws_id);
                }
            }
        }
    }

    let mut out: Vec<serde_json::Value> = Vec::with_capacity(eng.tiled_windows().len());
    for (wid, info) in eng.tiled_windows() {
        out.push(serde_json::json!({
            "hwnd": wid.as_isize(),
            "title": info.title,
            "class_name": info.class_name,
            "process_id": info.process_id,
            "x": info.bounds.loc.x,
            "y": info.bounds.loc.y,
            "width": info.bounds.size.w,
            "height": info.bounds.size.h,
            // A floating window is in no column; it belongs to the workspace it floats on.
            "workspace": ws_of
                .get(wid)
                .copied()
                .or_else(|| eng.float_workspace(*wid))
                .map(crate::layout::workspace_number),
            "floating": eng.is_floating(*wid),
        }));
    }
    // Stable order: hwnd ascending so `wiri-ctl windows` is deterministic.
    out.sort_by(|a, b| {
        a.get("hwnd")
            .and_then(|v| v.as_i64())
            .cmp(&b.get("hwnd").and_then(|v| v.as_i64()))
    });
    out
}

fn subscription_allows_event(event_types: &[String], event: &serde_json::Value) -> bool {
    if event_types.is_empty() || event_types.iter().any(|t| t == "*") {
        return true;
    }

    let Some(event_type) = event.get("type").and_then(|v| v.as_str()) else {
        return false;
    };

    event_types.iter().any(|filter| filter == event_type)
}

/// Locate a window by its HWND across all monitors/workspaces and update the
/// engine's focus state to point at it.  Used by IPC handlers that act on a
/// specific window (TileRequest, WindowFloat, WindowMoveToWorkspace, etc.) so
/// the subsequent focused-window engine call hits the right target.
///
/// Returns `true` on success, `false` when the HWND isn't a tracked tiled
/// (or floating) window.
fn focus_window_by_hwnd(
    engine: &Arc<parking_lot::RwLock<crate::layout::TilingEngine>>,
    backend: &crate::backend::BackendHandle,
    hwnd: isize,
) -> bool {
    let wid = crate::utils::WindowId::new(hwnd);

    // Fast path: if the engine doesn't know this window, give up early.
    if !engine.read().tiled_windows().contains_key(&wid) {
        return false;
    }

    // Find which monitor + workspace + column hosts the window.
    let location = {
        let eng = engine.read();
        let mut found: Option<(crate::utils::OutputId, i32, usize)> = None;
        for (&oid, monitor) in eng.monitors().iter() {
            for (&ws_id, ws) in monitor.workspaces.iter() {
                if let Some(col_idx) = ws.find_window_column(wid) {
                    found = Some((oid, ws_id, col_idx));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        found
    };

    let Some((oid, ws_id, col_idx)) = location else {
        // Registered but not in any column: a floating window (focused here, on its
        // own workspace) or one that is minimized.
        if engine.write().focus_float(wid, backend) {
            engine.write().apply_all(backend);
        }
        return true;
    };

    // Update focused output, workspace, focus_column and focus_window.
    {
        let mut eng = engine.write();
        eng.set_focused_output(oid);
        if let Some(monitor) = eng.monitors_mut().get_mut(&oid) {
            monitor.active_workspace = ws_id;
            monitor.focus_column = Some(col_idx);
            monitor.focus_window = Some(wid);
            monitor.focus_ring.push(wid);
        }
    }

    // Best-effort: bring the Win32 window to the foreground (don't fail on err).
    {
        let eng = engine.read();
        if let Some(window) = eng.tiled_windows().get(&wid) {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::{IsWindow, SetForegroundWindow};
            let h = HWND(window.hwnd as *mut std::ffi::c_void);
            unsafe {
                if IsWindow(h).as_bool() {
                    let _ = SetForegroundWindow(h);
                }
            }
        }
    }

    // Re-apply layout on the target monitor.
    engine.write().apply_all(backend);
    true
}

/// Item 2 (Option A): synchronously build a named-pipe instance with the
/// Admins-only ACL applied, then drop the `SECURITY_ATTRIBUTES` before
/// returning.  This guarantees the !Send `SECURITY_ATTRIBUTES` never crosses
/// an `.await` point and the resulting `NamedPipeServer` is freely Send.
///
/// If `first_instance` is true, the call sets `first_pipe_instance(true)`
/// (fails if another listener already holds the pipe — used as the
/// single-instance probe).
fn create_pipe_with_sa(
    first_instance: bool,
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use tokio::net::windows::named_pipe::ServerOptions;

    // Build SA inline so it lives only for this call.
    let sa_opt = match build_pipe_security_attributes() {
        Ok(s) => Some(s),
        Err(e) => {
            warn!(
                "Failed to build pipe security attributes: {} — falling back to defaults",
                e
            );
            None
        }
    };
    let sa_ptr: *mut std::ffi::c_void = match &sa_opt {
        Some(sa) => sa as *const _ as *mut std::ffi::c_void,
        None => std::ptr::null_mut(),
    };

    // SAFETY: sa_ptr is null (defaults) or points to the local `sa_opt`,
    // which is on this thread's stack and alive for the duration of the
    // create call.  After `create_with_security_attributes_raw` returns,
    // the OS has copied/duplicated whatever security info it needed.
    // `SECURITY_ATTRIBUTES` is `Copy`, so `sa_opt` is dropped automatically
    // when this synchronous function returns — strictly before any `.await`
    // in the caller.
    let server = unsafe {
        ServerOptions::new()
            .first_pipe_instance(first_instance)
            .create_with_security_attributes_raw(PIPE_PATH, sa_ptr)?
    };
    let _ = sa_opt; // keep alive until after the create call.
    Ok(server)
}

/// Format a Win32 modifier bitmask + VK code as a "Ctrl+Alt+Space" style
/// chord string for human display in `wiri-ctl test-bindings`.
///
/// Modifier bits map to (1=Alt, 2=Ctrl, 4=Shift, 8=Win); see
/// `MOD_ALT/MOD_CONTROL/MOD_SHIFT/MOD_WIN` in
/// `windows::Win32::UI::Input::KeyboardAndMouse`.  Unknown VKs are rendered
/// as a hex constant.
pub fn format_chord(modifiers: u32, vk: u32) -> String {
    let mut parts: Vec<&'static str> = Vec::new();
    if modifiers & 0x0002 != 0 {
        parts.push("Ctrl");
    }
    if modifiers & 0x0001 != 0 {
        parts.push("Alt");
    }
    if modifiers & 0x0004 != 0 {
        parts.push("Shift");
    }
    if modifiers & 0x0008 != 0 {
        parts.push("Win");
    }
    let key = match vk {
        0x08 => "Backspace".to_string(),
        0x09 => "Tab".to_string(),
        0x0D => "Enter".to_string(),
        0x13 => "Pause".to_string(),
        0x14 => "CapsLock".to_string(),
        0x1B => "Escape".to_string(),
        0x20 => "Space".to_string(),
        0x21 => "PageUp".to_string(),
        0x22 => "PageDown".to_string(),
        0x23 => "End".to_string(),
        0x24 => "Home".to_string(),
        0x25 => "Left".to_string(),
        0x26 => "Up".to_string(),
        0x27 => "Right".to_string(),
        0x28 => "Down".to_string(),
        0x2C => "PrintScreen".to_string(),
        0x2D => "Insert".to_string(),
        0x2E => "Delete".to_string(),
        0x30..=0x39 => ((b'0' + (vk - 0x30) as u8) as char).to_string(),
        0x41..=0x5A => ((b'A' + (vk - 0x41) as u8) as char).to_string(),
        0x70..=0x7B => format!("F{}", vk - 0x70 + 1),
        0xBA => ";".to_string(),
        0xBB => "+".to_string(),
        0xBC => ",".to_string(),
        0xBD => "-".to_string(),
        0xBE => ".".to_string(),
        0xBF => "/".to_string(),
        0xC0 => "`".to_string(),
        0xDB => "[".to_string(),
        0xDC => "\\".to_string(),
        0xDD => "]".to_string(),
        0xDE => "'".to_string(),
        _ => format!("0x{:02X}", vk),
    };
    if parts.is_empty() {
        key
    } else {
        format!("{}+{}", parts.join("+"), key)
    }
}

/// Build a SECURITY_ATTRIBUTES that grants full access only to
/// BUILTIN\Administrators (BA) and the file owner (OW).
pub fn build_pipe_security_attributes() -> Result<windows::Win32::Security::SECURITY_ATTRIBUTES> {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

    // D: = DACL; (A;;GA;;;BA) = Allow GenericAll to BUILTIN\Administrators
    //           (A;;GA;;;OW) = Allow GenericAll to the object Owner
    let sddl: Vec<u16> = "D:(A;;GA;;;BA)(A;;GA;;;OW)\0".encode_utf16().collect();

    let mut sd = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR::from_raw(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )?;
    }

    Ok(SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: false.into(),
    })
}
