// IPC server: named-pipe creation loop, client handler, message processor,
// event broadcaster, and pipe security attributes helper.
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};

use super::messages::{
    BindingInfo, FocusedWindowResponse, IpcEvent, IpcMessage, MonitorInfo, OverviewStateResponse,
    VersionResponse, WindowsResponse, WorkspaceInfo, WorkspacesResponse,
};
use super::pipe_path;
use super::state::{self, IpcSnapshot};
use crate::layout::TilingEngine;
use crate::utils::{Rect, WindowId};

/// How long a client waits for the answer to one request before it is told
/// that wiri is busy.  `wiri-ctl` and scripts rely on a reply within about two
/// seconds, whatever the window manager is doing.
const IPC_RESPONSE_BUDGET: std::time::Duration = std::time::Duration::from_millis(1500);

/// `CreateNamedPipe` with `FILE_FLAG_FIRST_PIPE_INSTANCE` fails with this when
/// the pipe name is already taken: another wiri of this user and session is
/// serving it.
const ERROR_ACCESS_DENIED: i32 = 5;

pub struct IpcServer {
    event_tx: broadcast::Sender<IpcEvent>,
    engine: Option<Arc<parking_lot::RwLock<TilingEngine>>>,
    /// Backend handle so IPC handlers can drive engine state-changing methods
    /// (switch_workspace, focus_window, etc.) which need a real backend to
    /// show/hide/position windows.
    backend: Option<crate::backend::BackendHandle>,
    /// Optional one-shot sender to request graceful shutdown of main.
    shutdown_tx: Arc<parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    /// Optional sender to request a config reload on the main loop.
    reload_tx: parking_lot::Mutex<Option<mpsc::UnboundedSender<()>>>,
    /// The config file wiri loaded, so `reload_config` can answer with what is wrong in it.
    config_path: parking_lot::Mutex<Option<std::path::PathBuf>>,
    /// What event-stream clients were last told about the windows and workspaces: what the
    /// next change is compared with, and what a new subscriber is brought up to date with.
    /// Held while the state is described, compared and sent, so that two such passes never
    /// interleave (lock order: this one, then the engine).
    mirror: parking_lot::Mutex<Mirror>,
    /// The last `config_loaded` event, which a new subscriber gets after the state (niri sends
    /// how the config loaded on connect).
    last_config: parking_lot::Mutex<Option<IpcEvent>>,
}

/// See [`IpcServer::mirror`].
#[derive(Default)]
struct Mirror {
    /// What the subscribers know.
    last: IpcSnapshot,
    /// The file name of the program of each process that has a window (asking the system for
    /// it costs a process handle; a process id is not looked up twice).
    app_ids: std::collections::HashMap<u32, Option<String>>,
}

impl IpcServer {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(64);
        crate::screenshot::set_event_sender(event_tx.clone());
        crate::input::pointer_settings::set_event_sender(event_tx.clone());
        Self {
            event_tx,
            engine: None,
            backend: None,
            shutdown_tx: Arc::new(parking_lot::Mutex::new(None)),
            reload_tx: parking_lot::Mutex::new(None),
            config_path: parking_lot::Mutex::new(None),
            mirror: parking_lot::Mutex::new(Mirror::default()),
            last_config: parking_lot::Mutex::new(None),
        }
    }

    /// Tell the server which config file is in use.
    pub fn set_config_path(&self, path: std::path::PathBuf) {
        *self.config_path.lock() = Some(path);
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
        // How the config loaded is part of what a new subscriber is told first.
        if matches!(event, IpcEvent::ConfigLoaded { .. }) {
            *self.last_config.lock() = Some(event.clone());
        }
        let _ = self.event_tx.send(event);
    }

    // ---- the event stream's state: niri's contract ---------------------------------------------
    //
    // A client that subscribes is told the state first (the workspaces, the windows, whether
    // the overview is open, how the config loaded) and then what changes, so that it never has
    // to ask.  `sync_state` finds the changes by describing the engine and comparing the
    // description with the one the clients were last told; it runs when something that can
    // change the state has happened (`notify_state_changed`), and only while somebody listens.

    /// Describe the engine now: its windows and workspaces, with the program of every window
    /// the window rules had not looked at (they know the program of the windows they
    /// evaluated).  `None` before the engine is connected.
    fn describe(&self, mirror: &mut Mirror) -> Option<IpcSnapshot> {
        let engine = self.engine.as_ref()?;
        let mut snapshot = engine.read().ipc_snapshot();
        snapshot.keyboard_layouts = Some(super::keyboard::snapshot());
        let mut live = std::collections::HashSet::new();
        for window in &mut snapshot.windows {
            let Some(pid) = window.pid else { continue };
            live.insert(pid);
            if window.app_id.is_none() {
                let backend = self.backend.as_ref();
                let name = mirror
                    .app_ids
                    .entry(pid)
                    .or_insert_with(|| backend.and_then(|b| b.process_name_for_pid(pid)));
                window.app_id = name.clone();
            }
        }
        // A process id can be given to another process once the first one has gone.
        mirror.app_ids.retain(|pid, _| live.contains(pid));
        Some(snapshot)
    }

    /// The state as it is now, in the shape the event stream describes it: the
    /// same description a new subscriber is told first, built by the same code
    /// ([`describe`]).  `None` before the engine is connected.
    fn snapshot_now(&self) -> Option<IpcSnapshot> {
        let mut mirror = self.mirror.lock();
        self.describe(&mut mirror)
    }

    /// Send the subscribers what changed since they were last told.  Returns the events that
    /// found no receiver, for the next subscriber's replay.
    pub fn sync_state(&self) {
        if self.event_tx.receiver_count() == 0 {
            return;
        }
        let mut mirror = self.mirror.lock();
        let _ = self.sync_locked(&mut mirror);
    }

    fn sync_locked(&self, mirror: &mut Mirror) -> Vec<IpcEvent> {
        let Some(snapshot) = self.describe(mirror) else {
            return Vec::new();
        };
        for event in state::diff(&mirror.last, &snapshot) {
            let _ = self.event_tx.send(event);
        }
        mirror.last = snapshot;
        // The legacy focus broadcast of a focus change the engine made itself after its own
        // activation request succeeded (no native notice follows one that Windows never
        // raises); a notice that does follow is not broadcast again.
        let mut unheard = Vec::new();
        if let Some(engine) = self.engine.as_ref() {
            for event in engine.read().take_live_focus_events() {
                // Nobody listening yet is not the same as nobody to tell: `send` hands the
                // event back, and the first subscriber is told it in its replay.
                if let Err(broadcast::error::SendError(event)) = self.event_tx.send(event) {
                    unheard.push(event);
                }
            }
        }
        unheard
    }

    /// A new subscriber: bring what the others were told up to date, subscribe, and return
    /// what the new one is told first.  Nothing is sent between the subscription and the
    /// description (the mirror is held), so the replay and the events that follow it neither
    /// miss a change nor repeat one.  A notice the state pass took while the channel still had
    /// no receiver goes into the replay; sent before the subscription it would be lost.
    fn subscribe_with_replay(&self) -> (broadcast::Receiver<IpcEvent>, Vec<IpcEvent>) {
        let mut mirror = self.mirror.lock();
        let unheard = self.sync_locked(&mut mirror);
        let receiver = self.event_tx.subscribe();
        let mut replay = state::replay(&mirror.last);
        replay.extend(unheard);
        if let Some(config) = self.last_config.lock().clone() {
            replay.push(config);
        }
        (receiver, replay)
    }

    /// `subscribe_with_replay` on a thread that may wait for the engine.
    async fn subscribe_blocking(
        self: &Arc<Self>,
    ) -> Option<(broadcast::Receiver<IpcEvent>, Vec<IpcEvent>)> {
        let server = Arc::clone(self);
        tokio::task::spawn_blocking(move || server.subscribe_with_replay())
            .await
            .ok()
    }

    /// Tell the subscribers about changes, once for a burst of them, and only while somebody is
    /// subscribed.  It sleeps until `notify_state_changed` wakes it: no timer, no polling.
    pub async fn run_state_task(self: Arc<Self>) {
        loop {
            super::STATE_CHANGED.notified().await;
            if self.event_tx.receiver_count() == 0 {
                continue;
            }
            let server = Arc::clone(&self);
            if tokio::task::spawn_blocking(move || server.sync_state())
                .await
                .is_err()
            {
                error!("IPC: describing the state for the event stream panicked");
            }
        }
    }

    /// Start the IPC server listening on the named pipe.
    /// This spawns a background task that accepts connections.
    ///
    /// Security: the named pipe is created with a DACL that grants only
    /// `BUILTIN\Administrators` and the file owner full access — preventing
    /// other local users from connecting via `\\.\pipe\wiri_control_<user>_<session>`
    /// even on a shared workstation.  Implemented via tokio's
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
        // The pipe of this user in this session: another user logged in on the
        // same machine has a pipe of its own.
        let pipe = pipe_path();
        // Children launched by binds/startup inherit the actual endpoint, including
        // an explicit override, just as niri exports NIRI_SOCKET.
        std::env::set_var(super::PIPE_ENV_VAR, &pipe);
        info!("IPC server starting on {} (ACL: Admins+Owner only)", pipe);

        // Another wiri of this user and session may already be running.  The
        // probe in `main.rs` looks at the machine-wide pipe name and cannot see
        // the per-user one, so the check is made here, against the pipe that is
        // actually served.
        if super::pipe_has_listener(&pipe) {
            error!(
                "Another wiri instance is already running (pipe {} has an active listener).",
                pipe
            );
            error!("Use `wiri-ctl quit` to stop it, or kill the process first.");
            std::process::exit(2);
        }

        const MAX_CREATE_RETRIES: u32 = 5;
        let mut retry_count = 0u32;
        let mut first_instance = true;

        loop {
            // Synchronous pipe creation — SECURITY_ATTRIBUTES is materialised
            // and dropped entirely within this call, before any `.await` below.
            let create_result = create_pipe_with_sa(first_instance, &pipe);
            let server = match create_result {
                Ok(s) => {
                    retry_count = 0;
                    first_instance = false; // Subsequent instances must not set first_pipe_instance.
                    s
                }
                Err(e) if first_instance => {
                    // The pipe of this user in this session is taken: another
                    // wiri is serving it.  (A pipe name is released as soon as
                    // the last handle to it is closed, so this is not a stale
                    // one.)
                    if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) {
                        error!(
                            "Another wiri instance is already running (pipe {} is taken).",
                            pipe
                        );
                        error!("Use `wiri-ctl quit` to stop it, or kill the process first.");
                        std::process::exit(2);
                    }
                    // First-instance attempt failed for another reason — fall
                    // back to not requesting first_pipe_instance in case another
                    // (stale) handle exists.
                    match create_pipe_with_sa(false, &pipe) {
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

        let mut buffer = [0u8; 8192];
        let mut framer = super::wire::Framer::default();
        let mut queued = std::collections::VecDeque::new();
        loop {
            if queued.is_empty() {
                let n = match pipe.read(&mut buffer).await {
                    Ok(0) => return Ok(()),
                    Ok(n) => n,
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => return Ok(()),
                    Err(e) => return Err(e.into()),
                };
                let mut frames = framer.feed(&buffer[..n]);
                let legacy = frames.is_empty() && !framer.lined;
                if legacy {
                    if let Some(frame) = framer.legacy() {
                        frames.push(frame);
                    }
                }
                queued.extend(frames.into_iter().map(|frame| (frame, legacy)));
            }
            if let Some((frame, legacy)) = queued.pop_front() {
                let dialect = frame
                    .as_ref()
                    .map(super::wire::dialect)
                    .unwrap_or(super::wire::Dialect::Wiri);
                let request = frame.and_then(super::wire::decode);
                let request = match request {
                    Ok(request) => request,
                    Err(error) => {
                        write_line(&mut pipe, &super::wire::error(dialect, error)).await?;
                        if legacy {
                            let _ = pipe.shutdown().await;
                            return Ok(());
                        }
                        continue;
                    }
                };
                debug!("IPC received: {:?}", request.message);
                if let IpcMessage::SubscribeEvents { ref event_types } = request.message {
                    let ack = super::wire::reply(
                        request.dialect,
                        &request.name,
                        serde_json::json!({
                            "success":true,"subscription_id":1
                        }),
                    );
                    write_line(&mut pipe, &ack).await?;
                    return self
                        .stream_events(&mut pipe, event_types, request.dialect)
                        .await;
                }
                let kind = match request.message {
                    IpcMessage::PickWindow => Some(super::pick::PickKind::Window),
                    IpcMessage::PickColor => Some(super::pick::PickKind::Color),
                    _ => None,
                };
                let response = if let Some(kind) = kind {
                    // Interactive requests have no response deadline. Keep reading
                    // solely to detect disconnect and bound pipelined input; dropping
                    // the future releases the pick and its eventual captured release.
                    let pick = super::pick::pick(kind);
                    tokio::pin!(pick);
                    let mut pending_bytes = 0;
                    let result = loop {
                        tokio::select! {
                            result = &mut pick => break result,
                            read = pipe.read(&mut buffer) => {
                                let n = read?;
                                if n == 0 { return Ok(()); }
                                pending_bytes += n;
                                if pending_bytes > super::wire::MAX_REQUEST_BYTES {
                                    return Err(anyhow::anyhow!("too much input queued during pick"));
                                }
                                queued.extend(framer.feed(&buffer[..n]).into_iter().map(|frame| (frame, false)));
                                if queued.len() > 64 { return Err(anyhow::anyhow!("too many requests queued during pick")); }
                            }
                        }
                    };
                    match result {
                        Ok(value) if kind == super::pick::PickKind::Window => {
                            serde_json::json!({"success":true,"result":{"window":value}})
                        }
                        Ok(value) => serde_json::json!({"success":true,"result":{"color":value}}),
                        Err(error) => serde_json::json!({"success":false,"error":error}),
                    }
                } else {
                    self.respond(request.message).await
                };
                let response = super::wire::reply(request.dialect, &request.name, response);
                write_line(&mut pipe, &response).await?;
                if legacy {
                    // Old read_to_end clients do not send a newline. New line
                    // clients keep this same connection for their next request.
                    let _ = pipe.shutdown().await;
                    return Ok(());
                }
            }
        }
    }

    async fn stream_events(
        self: &Arc<Self>,
        pipe: &mut tokio::net::windows::named_pipe::NamedPipeServer,
        event_types: &[String],
        dialect: super::wire::Dialect,
    ) -> Result<()> {
        use tokio::io::AsyncReadExt;
        let Some((mut receiver, replay)) = self.subscribe_blocking().await else {
            return Ok(());
        };
        for event in &replay {
            if !write_event(pipe, event_types, event, dialect).await {
                return Ok(());
            }
        }
        let mut disconnected = [0u8; 1];
        loop {
            tokio::select! {
                // A silent subscriber that closes must release its subscription
                // immediately, not only when the next desktop event happens.
                _ = pipe.read(&mut disconnected) => return Ok(()),
                received = receiver.recv() => match received {
                    Ok(event) => {
                        if !write_event(pipe, event_types, &event, dialect).await {
                            return Ok(());
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let Some((rx, replay)) = self.subscribe_blocking().await else {
                            return Ok(());
                        };
                        receiver = rx;
                        for event in &replay {
                            if !write_event(pipe, event_types, event, dialect).await {
                                return Ok(());
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
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
        // Event-stream clients are told what the request did to the windows and workspaces.
        let _state_changed = (!is_query(&message)).then_some(super::StateChangeGuard);
        if !is_query(&message) {
            if let (Some(engine), Some(backend)) = (&self.engine, &self.backend) {
                engine.write().cancel_composited_transitions(backend);
            }
        }
        match message {
            IpcMessage::GetState => {
                if let Some(engine) = &self.engine {
                    let eng = engine.read();
                    let windows = build_window_infos(&eng);
                    let mut workspaces = Vec::new();
                    for (oid, monitor) in eng.monitors().iter() {
                        let ws = monitor.workspace();
                        let number = eng.workspace_number_on(*oid, monitor.active_workspace_id());
                        workspaces.push(WorkspaceInfo {
                            name: format!("workspace-{}", number),
                            id: number as u32,
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
            // ---- niri's request/reply forms (IPC schema 1.2) ----
            //
            // They answer with the same description the event stream is built
            // from: the windows and the workspaces as niri names them.
            IpcMessage::Workspaces => {
                let reply = WorkspacesResponse {
                    workspaces: self
                        .snapshot_now()
                        .map(|snapshot| snapshot.workspaces)
                        .unwrap_or_default(),
                };
                serde_json::json!({"success": true, "result": reply})
            }
            IpcMessage::Windows => {
                let reply = WindowsResponse {
                    windows: self
                        .snapshot_now()
                        .map(|snapshot| snapshot.windows)
                        .unwrap_or_default(),
                };
                serde_json::json!({"success": true, "result": reply})
            }
            IpcMessage::FocusedWindow => {
                let focused_window = self.snapshot_now().and_then(|snapshot| {
                    let id = snapshot.focused_window()?;
                    snapshot.windows.into_iter().find(|window| window.id == id)
                });
                let reply = FocusedWindowResponse { focused_window };
                serde_json::json!({"success": true, "result": reply})
            }
            IpcMessage::OverviewState => {
                let reply = OverviewStateResponse {
                    is_open: self
                        .snapshot_now()
                        .map(|snapshot| snapshot.overview_open)
                        .unwrap_or(false),
                };
                serde_json::json!({"success": true, "result": reply})
            }
            IpcMessage::Outputs | IpcMessage::FocusedOutput => {
                let focused = matches!(message, IpcMessage::FocusedOutput);
                let Some(engine) = &self.engine else {
                    return serde_json::json!({"success":false,"error":"engine not initialized"});
                };
                let engine = engine.read();
                if focused {
                    serde_json::json!({"success":true,"result":{"focused_output":super::queries::focused_output(&engine, self.backend.as_ref())}})
                } else {
                    serde_json::json!({"success":true,"result":{"outputs":super::queries::outputs(&engine, self.backend.as_ref())}})
                }
            }
            IpcMessage::KeyboardLayouts => {
                serde_json::json!({"success":true,"result":{"keyboard_layouts":super::queries::keyboard_layouts()}})
            }
            IpcMessage::Output { output, action } => {
                let Some(engine) = &self.engine else {
                    return serde_json::json!({"success":false,"error":"engine not initialized"});
                };
                match super::queries::change_output(&output, &action, engine, self.backend.as_ref())
                {
                    Ok(result) => serde_json::json!({"success":true,"result":result}),
                    Err(error) => serde_json::json!({"success":false,"error":error}),
                }
            }
            // Waiting for input is asynchronous and handled before the bounded
            // blocking-request path. Never park a blocking pool thread here.
            IpcMessage::PickWindow | IpcMessage::PickColor => {
                serde_json::json!({"success":false,"error":"interactive picks require a connected IPC client"})
            }
            IpcMessage::ReturnError => {
                serde_json::json!({"success":false,"error":"example error requested by ReturnError"})
            }
            IpcMessage::Version => {
                let reply = VersionResponse {
                    version: crate::version_string(),
                };
                serde_json::json!({"success": true, "result": reply})
            }
            IpcMessage::SwitchWorkspace { id } => {
                info!("IPC: Switch to workspace {}", id);
                match (&self.engine, &self.backend) {
                    (Some(engine), Some(backend)) => {
                        // `id` is the 1-based number users type: the position of the
                        // workspace on the focused monitor (`focus-workspace 2`).
                        engine.write().focus_workspace_number(id, backend);
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
                if let Some(engine) = &self.engine {
                    engine
                        .write()
                        .prepare_close_transition(crate::utils::WindowId::new(window_hwnd));
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
                // The reload itself (and the balloon, and the event) happens on the main loop;
                // the answer says right away whether the file can be loaded.
                let path = self.config_path.lock().clone();
                let problem = path.and_then(|p| {
                    let decoded = crate::config::loader::load_quiet(&p);
                    if decoded.has_errors() {
                        Some(crate::config::failure_summary(&p, &decoded.issues))
                    } else {
                        None
                    }
                });
                if let Some(tx) = self.reload_tx.lock().as_ref() {
                    match tx.send(()) {
                        Ok(()) => match problem {
                            Some(summary) => serde_json::json!({
                                "success": false,
                                "error": summary
                            }),
                            None => serde_json::json!({"success": true}),
                        },
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
                // Preserve wiri's legacy management/shutdown command. Niri's
                // Action::Quit and key bindings use confirmation below.
                if let Some(tx) = self.shutdown_tx.lock().take() {
                    let _ = tx.send(());
                }
                serde_json::json!({"success": true})
            }
            IpcMessage::WorkspaceAction { action } => {
                let name = action.action_name();
                let action = match action.into_input_action() {
                    Ok(action) => action,
                    Err(error) => return serde_json::json!({"success":false,"error":error}),
                };
                let (Some(engine), Some(backend)) = (&self.engine, &self.backend) else {
                    return serde_json::json!({"success":false,"error":"engine or backend not initialized"});
                };
                if let crate::input::Action::WorkspaceAction(
                    crate::input::WorkspaceAction::MoveWindow {
                        window: Some(window),
                        ..
                    },
                ) = &action
                {
                    if !engine.read().tiled_windows().contains_key(window) {
                        return serde_json::json!({"success":false,"error":format!(
                            "no window with id {} (the handle that `wiri-ctl windows` shows)",
                            window.as_isize()
                        )});
                    }
                }
                // The audit accepts the decoded action without running effects.
                // ID references remain structured; there is no invented argv syntax.
                if let Some(reply) = super::action_audit::reply(name, &[]) {
                    return reply;
                }
                let mut engine = engine.write();
                match &action {
                    crate::input::Action::WorkspaceAction(action) => {
                        engine.workspace_layout_action(action, backend)
                    }
                    crate::input::Action::MonitorAction(action) => {
                        engine.monitor_layout_action(action, backend)
                    }
                    crate::input::Action::FocusWorkspace(index) => {
                        engine.focus_workspace_number(*index, backend)
                    }
                    crate::input::Action::FocusWorkspaceNamed(name) => {
                        engine.focus_workspace_named(name, backend)
                    }
                    crate::input::Action::RenameWorkspace(name) => {
                        engine.set_workspace_name(name, None, backend)
                    }
                    _ => unreachable!("typed workspace transport only creates workspace actions"),
                }
                serde_json::json!({"success":true})
            }
            IpcMessage::SizeAction { action } => {
                let name = action.action_name();
                let window = match action.window_id() {
                    Some(id) => match isize::try_from(id).ok().filter(|id| *id > 0) {
                        Some(id) => Some(WindowId::new(id)),
                        None => {
                            return serde_json::json!({
                                "success":false,"error":"window id must be a positive native window id"
                            })
                        }
                    },
                    None => None,
                };
                let action = match action.into_input_action() {
                    Ok(action) => action,
                    Err(error) => return serde_json::json!({"success":false,"error":error}),
                };
                let (Some(engine), Some(backend)) = (&self.engine, &self.backend) else {
                    return serde_json::json!({"success":false,"error":"engine or backend not initialized"});
                };
                if let Some(window) = window {
                    if !engine.read().tiled_windows().contains_key(&window) {
                        return serde_json::json!({"success":false,"error":format!(
                            "no window with id {} (the handle that `wiri-ctl windows` shows)",
                            window.as_isize()
                        )});
                    }
                }
                if let Some(reply) = super::action_audit::reply(name, &[]) {
                    return reply;
                }
                if let (Some(window), crate::input::Action::SetWindowHeight(change)) =
                    (window, &action)
                {
                    engine
                        .write()
                        .set_window_height_for(window, *change, backend);
                    return serde_json::json!({"success":true});
                }
                if let (Some(window), crate::input::Action::SetColumnWidth(change)) =
                    (window, &action)
                {
                    if name == "set-window-width" {
                        engine
                            .write()
                            .set_window_width_for(window, *change, backend);
                        return serde_json::json!({"success":true});
                    }
                }
                if let Some(window) = window {
                    // Preserve Wiri's legacy targeted SetColumnWidth extension.
                    if !focus_window_by_hwnd(engine, backend, window.as_isize()) {
                        return serde_json::json!({"success":false,"error":format!(
                            "no window with id {} to focus", window.as_isize()
                        )});
                    }
                }
                let no_quit = Arc::new(parking_lot::Mutex::new(None));
                crate::backend::message_loop::perform_action_on(
                    &action,
                    engine,
                    backend,
                    &no_quit,
                    window.map(WindowId::as_isize),
                );
                serde_json::json!({"success":true})
            }
            IpcMessage::RunAction { name, args, quoted } => {
                info!("IPC: RunAction {} {:?}", name, args);
                if name == "move-floating-window" {
                    return match super::parse_move_floating_window(&args) {
                        Ok(message) => self.process_message(message),
                        Err(error) => serde_json::json!({"success":false,"error":error}),
                    };
                }
                // Capture an explicit window without focusing it (including a
                // parked/inactive one). Validate before the generic --id path,
                // which otherwise changes focus for layout actions.
                if matches!(
                    name.as_str(),
                    "screenshot-screen" | "screenshot-window" | "screenshot-focused"
                ) {
                    let Some(capture) = crate::screenshot::action::CaptureAction::parse(
                        name != "screenshot-screen",
                        &args,
                    ) else {
                        return serde_json::json!({"success":false,"error":"invalid screenshot options (paths must be absolute)"});
                    };
                    if let Some(id) = capture.id {
                        if !self.engine.as_ref().is_some_and(|engine| {
                            engine
                                .read()
                                .tiled_windows()
                                .contains_key(&WindowId::new(id))
                        }) {
                            return serde_json::json!({"success":false,"error":format!("no managed window with id {id}")});
                        }
                    }
                    // The dry run (`super::action_audit`) records an accepted
                    // screenshot instead of taking one.
                    if let Some(reply) = super::action_audit::reply(&name, &args) {
                        return reply;
                    }
                    return match capture.run() {
                        Ok(path) => serde_json::json!({"success":true,"result":{"path":path}}),
                        Err(error) => {
                            serde_json::json!({"success":false,"error":format!("{error:#}")})
                        }
                    };
                }
                if matches!(
                    name.as_str(),
                    "screenshot" | "take-screenshot" | "capture-screen"
                ) {
                    let Some(options) = crate::overlay::screenshot_ui::Options::parse(&args) else {
                        return serde_json::json!({"success":false,"error":"invalid screenshot UI options (paths must be absolute)"});
                    };
                    if let Some(reply) = super::action_audit::reply(&name, &args) {
                        return reply;
                    }
                    return match crate::overlay::screenshot_ui::open(options) {
                        Ok(()) => serde_json::json!({"success":true}),
                        Err(error) => {
                            serde_json::json!({"success":false,"error":format!("{error:#}")})
                        }
                    };
                }
                // niri's `--id <n>`: the window to act on.  Not for `spawn`: every word after
                // its name is the command line of the program to start, `--id 5` included
                // (`niri msg action spawn -- tool --id 5`, niri:niri-ipc/src/lib.rs:207-212).
                let (args, quoted, target) = match split_action_target(&name, &args, &quoted) {
                    Ok(split) => split,
                    Err(error) => return serde_json::json!({"success": false, "error": error}),
                };
                if let Some(parsed) = crate::input::system_actions::parse(&name, &args) {
                    use crate::input::system_actions::{self, SystemAction};
                    let Some(system) = parsed else {
                        return serde_json::json!({"success": false, "error": "invalid system action arguments"});
                    };
                    if target.is_some() && !matches!(system, SystemAction::WindowUrgent(_)) {
                        return serde_json::json!({"success": false, "error": "this action does not accept --id"});
                    }
                    // The dry run records a decoded system action (quit, power,
                    // touchpad, reload) instead of performing it.
                    if let Some(reply) = super::action_audit::reply(&name, &args) {
                        return reply;
                    }
                    let _changed = crate::ipc::StateChangeGuard;
                    let result = match &system {
                        SystemAction::Quit { skip_confirmation } => {
                            let sender = self.shutdown_tx.clone();
                            system_actions::request_quit(*skip_confirmation, move || {
                                if let Some(tx) = sender.lock().take() {
                                    let _ = tx.send(());
                                }
                            })
                        }
                        SystemAction::LoadConfigFile(path) => {
                            system_actions::request_reload(path.clone())
                        }
                        _ => match (&self.engine, &self.backend) {
                            (Some(engine), Some(backend)) => {
                                system_actions::execute(&system, engine, backend, target)
                            }
                            _ => Err("engine or backend not initialized".into()),
                        },
                    };
                    return match result {
                        Ok(()) => serde_json::json!({"success": true}),
                        Err(error) => serde_json::json!({"success": false, "error": error}),
                    };
                }
                let (Some(engine), Some(backend)) = (&self.engine, &self.backend) else {
                    return serde_json::json!({
                        "success": false,
                        "error": "engine or backend not initialized"
                    });
                };
                if let Some(hwnd) = target {
                    // Only a window wiri manages can be named (a handle is not a licence to
                    // close or move whatever window of the desktop).
                    if !engine
                        .read()
                        .tiled_windows()
                        .contains_key(&WindowId::new(hwnd))
                    {
                        return serde_json::json!({
                            "success": false,
                            "error": format!(
                                "no window with id {} (the handle that `wiri-ctl windows` shows)",
                                hwnd
                            )
                        });
                    }
                    // niri's `close-window --id` closes that window and leaves the focus alone.
                    if matches!(name.as_str(), "close-window" | "close") {
                        return self.process_message(IpcMessage::CloseWindow { window_hwnd: hwnd });
                    }
                    if name == "focus-window" {
                        if !focus_window_by_hwnd(engine, backend, hwnd) {
                            return serde_json::json!({
                                "success": false,
                                "error": format!("no window with id {} to focus", hwnd)
                            });
                        }
                        engine.write().apply_all(backend);
                        return serde_json::json!({"success": true});
                    }
                } else if name == "focus-window" {
                    return serde_json::json!({
                        "success": false,
                        "error": "focus-window needs --id <window id> (the handle that `wiri-ctl windows` shows)"
                    });
                }
                let Some(mut action) = crate::input::parse_action_flagged(&name, &args, &quoted)
                else {
                    return serde_json::json!({
                        "success": false,
                        "error": format!("unknown action or bad arguments: {} {:?}", name, args)
                    });
                };
                // The dry run records the action the live path just decoded and
                // dispatches nothing: no window is focused and no effect runs.
                if let Some(reply) = super::action_audit::reply(&name, &args) {
                    return reply;
                }
                if matches!(action, crate::input::Action::Quit) {
                    return self.process_message(IpcMessage::Quit);
                }
                let target = if let Some(hwnd) = target {
                    if action.set_layout_window_target(WindowId::new(hwnd)) {
                        // Transfers carry their target without stealing focus first.
                        None
                    } else {
                        // These niri ById actions operate directly. Even passing the
                        // explicit HWND to perform_action_on would synchronize focus.
                        match &action {
                            crate::input::Action::ToggleFullscreen => {
                                engine
                                    .write()
                                    .toggle_fullscreen_for(WindowId::new(hwnd), backend);
                                return serde_json::json!({"success": true});
                            }
                            crate::input::Action::SetWindowHeight(change) => {
                                engine.write().set_window_height_for(
                                    WindowId::new(hwnd),
                                    *change,
                                    backend,
                                );
                                return serde_json::json!({"success": true});
                            }
                            crate::input::Action::SetColumnWidth(change)
                                if name == "set-window-width" =>
                            {
                                engine.write().set_window_width_for(
                                    WindowId::new(hwnd),
                                    *change,
                                    backend,
                                );
                                return serde_json::json!({"success": true});
                            }
                            // niri:src/input/mod.rs:1753-1761 and the other `*ById` variants:
                            // the named window changes, the focus does not.
                            crate::input::Action::MaximizeWindowToEdges => {
                                let window = WindowId::new(hwnd);
                                let mut engine = engine.write();
                                // A floating window stays floating (as for the focused one).
                                if !engine.is_floating(window) {
                                    engine.toggle_maximize_window_to_edges_for(window, backend);
                                }
                                return serde_json::json!({"success": true});
                            }
                            crate::input::Action::ToggleWindowRuleOpacity => {
                                engine
                                    .write()
                                    .toggle_window_rule_opacity(WindowId::new(hwnd), backend);
                                return serde_json::json!({"success": true});
                            }
                            // niri's other `*ById` variants (input/mod.rs:1110-1136, 1684-1736,
                            // 2111-2117, 2213-2250): the named window changes in place, on its own
                            // workspace and output, and neither native nor managed focus moves.
                            crate::input::Action::ConsumeOrExpelWindowLeft
                            | crate::input::Action::ConsumeOrExpelWindowRight => {
                                engine.write().consume_or_expel_window_for(
                                    WindowId::new(hwnd),
                                    matches!(
                                        action,
                                        crate::input::Action::ConsumeOrExpelWindowRight
                                    ),
                                    backend,
                                );
                                return serde_json::json!({"success": true});
                            }
                            crate::input::Action::CenterWindow => {
                                engine
                                    .write()
                                    .center_window_for(WindowId::new(hwnd), backend);
                                return serde_json::json!({"success": true});
                            }
                            crate::input::Action::ResetWindowHeight => {
                                engine
                                    .write()
                                    .reset_window_height_for(WindowId::new(hwnd), backend);
                                return serde_json::json!({"success": true});
                            }
                            crate::input::Action::WindowWidthPresetCycle
                            | crate::input::Action::WindowWidthPresetCycleBack => {
                                engine.write().switch_preset_window_width_for(
                                    WindowId::new(hwnd),
                                    matches!(action, crate::input::Action::WindowWidthPresetCycle),
                                    backend,
                                );
                                return serde_json::json!({"success": true});
                            }
                            crate::input::Action::WindowHeightPresetCycle
                            | crate::input::Action::WindowHeightPresetCycleBack => {
                                engine.write().switch_preset_window_height_for(
                                    WindowId::new(hwnd),
                                    matches!(action, crate::input::Action::WindowHeightPresetCycle),
                                    backend,
                                );
                                return serde_json::json!({"success": true});
                            }
                            crate::input::Action::ToggleFloating => {
                                engine
                                    .write()
                                    .toggle_floating_by_id(WindowId::new(hwnd), backend);
                                return serde_json::json!({"success": true});
                            }
                            crate::input::Action::MoveWindowToFloating
                            | crate::input::Action::MoveWindowToTiling => {
                                engine.write().set_floating_by_id(
                                    WindowId::new(hwnd),
                                    Some(matches!(
                                        action,
                                        crate::input::Action::MoveWindowToFloating
                                    )),
                                    backend,
                                );
                                return serde_json::json!({"success": true});
                            }
                            _ => {}
                        }
                        if !focus_window_by_hwnd(engine, backend, hwnd) {
                            return serde_json::json!({
                                "success": false,
                                "error": format!("no window with id {} to focus", hwnd)
                            });
                        }
                        Some(hwnd)
                    }
                } else {
                    None
                };
                // No quit channel: `quit` is handled above. The target is carried
                // by the structured transfer or retained for legacy actions.
                let no_quit = Arc::new(parking_lot::Mutex::new(None));
                crate::backend::message_loop::perform_action_on(
                    &action, engine, backend, &no_quit, target,
                );
                serde_json::json!({"success": true})
            }
            IpcMessage::SubscribeEvents { event_types } => {
                info!("IPC: Subscribe to events: {:?}", event_types);
                serde_json::json!({"success": true, "subscription_id": 1})
            }
            IpcMessage::MoveFloatingWindow { id, x, y } => {
                let (Some(engine), Some(backend)) = (&self.engine, &self.backend) else {
                    return serde_json::json!({"success":false,"error":"engine or backend not initialized"});
                };
                let id = match id.map(isize::try_from).transpose() {
                    Ok(id) => id.map(WindowId::new),
                    Err(_) => {
                        return serde_json::json!({"success":false,"error":"window id exceeds the native handle range"})
                    }
                };
                // Snapshot hook ownership before locking the engine (the hook can enqueue work).
                let grab = crate::input::low_level_hook::current_grab();
                let mut engine = engine.write();
                let moving = floating_move_target(&grab, &engine);
                match engine.move_floating_window(id, x, y, moving, backend) {
                    Ok(()) => serde_json::json!({"success":true}),
                    Err(error) => serde_json::json!({"success":false,"error":error}),
                }
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
                            let reference = name
                                .parse::<i32>()
                                .ok()
                                .map(|number| {
                                    crate::input::WorkspaceRef::Index(number.max(1) as u32)
                                })
                                .unwrap_or_else(|| {
                                    crate::input::WorkspaceRef::Name(name.to_string())
                                });
                            let destination = {
                                let eng = engine.read();
                                eng.monitors().focused_id().and_then(|output| {
                                    eng.resolve_workspace_reference(output, &reference)
                                })
                            };
                            if destination.is_none() {
                                return serde_json::json!({
                                    "success": false,
                                    "error": format!("Unknown workspace: {}", name)
                                });
                            }
                            // The F transfer resolves and carries (output, key).
                            // Never replay a global name's local key on this output.
                            engine.write().move_window_to_workspace_ref(
                                Some(wid),
                                reference,
                                false,
                                backend,
                            );
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
                        let workspace = engine.read().workspace_id_for_number(workspace_id);
                        if engine.read().is_floating(wid) {
                            engine
                                .write()
                                .float_move_to_workspace(wid, workspace, backend);
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
                        .map(|(oid, m)| WorkspaceInfo {
                            name: format!(
                                "workspace-{}",
                                eng.workspace_number_on(*oid, m.active_workspace_id())
                            ),
                            id: eng.workspace_number_on(*oid, m.active_workspace_id()) as u32,
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
                        let workspace = engine.read().workspace_id_for_number(workspace_id);
                        engine.write().move_window_to_workspace(workspace, backend);
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
                info!("IPC: SetColumnWidth {}", preset);
                // Same words as `set-column-width` in a bind: niri's `"50%"`, `"+10%"`,
                // `"1200"`, `"+100"`, or `1/3`, `full`, `cycle`.
                let Some(action) = crate::input::parse_column_width_arg(&preset) else {
                    return serde_json::json!({
                        "success": false,
                        "error": format!(
                            "bad column width '{}': use 50%, +10%, -10%, 1200, +100, 1/3, full or cycle",
                            preset
                        )
                    });
                };
                match (&self.engine, &self.backend) {
                    (Some(engine), Some(backend)) => {
                        let no_quit = Arc::new(parking_lot::Mutex::new(None));
                        crate::backend::message_loop::perform_action(
                            &action, engine, backend, &no_quit,
                        );
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
                                name: eng.output_name(*oid).unwrap_or_default().to_string(),
                                bounds_x: m.bounds.loc.x,
                                bounds_y: m.bounds.loc.y,
                                bounds_width: m.bounds.size.w,
                                bounds_height: m.bounds.size.h,
                                work_area_x: m.work_area.loc.x,
                                work_area_y: m.work_area.loc.y,
                                work_area_width: m.work_area.size.w,
                                work_area_height: m.work_area.size.h,
                                scale_factor: m.scale_factor,
                                active_workspace: eng
                                    .workspace_number_on(*oid, m.active_workspace_id()),
                                window_count,
                                focused: Some(*oid) == focused,
                                unified_grid: eng.unified_grid_for(*oid),
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
                    Some(e) => {
                        let id = e.read().workspace_id_for_number(workspace_id);
                        match e.write().rename_workspace(id, &name) {
                            Ok(()) => serde_json::json!({"success": true}),
                            Err(err) => serde_json::json!({"success": false, "error": err}),
                        }
                    }
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
                // Lane D: the same PNG/clipboard/template pipeline as screenshot-window.
                match crate::screenshot::capture(
                    Some(target_hwnd),
                    path.as_deref().map(std::path::Path::new),
                ) {
                    Ok(saved) => serde_json::json!({
                        "success": true,
                        "result": { "path": saved },
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
                            let ws_id = eng.workspace_number_on(
                                oid,
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
                    for (oid, monitor) in eng.monitors().iter() {
                        for ws_id in monitor.workspace_ids() {
                            let Some(ws) = monitor.workspaces.get(&ws_id) else {
                                continue;
                            };
                            let window_count: usize =
                                ws.columns.iter().map(|c| c.tiles.len()).sum();
                            let number = eng.workspace_number_on(*oid, ws_id);
                            let name = eng
                                .workspace_name_on(*oid, ws_id)
                                .unwrap_or_else(|| format!("workspace-{}", number));
                            workspaces.push(serde_json::json!({
                                "id": number,
                                "name": name,
                                "window_count": window_count,
                                "monitor_id": oid.as_u64(),
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
                    if !engine
                        .write()
                        .set_runtime_reservation(&side, pixels.min(i32::MAX as u32) as i32)
                    {
                        return serde_json::json!({
                            "success": false,
                            "error": format!("invalid side {:?}: expected top | bottom | left | right", side),
                        });
                    }
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
                // Lane D: the same PNG/clipboard/template pipeline as screenshot-window.
                match crate::screenshot::capture(
                    Some(window_hwnd),
                    path.as_deref().map(std::path::Path::new),
                ) {
                    Ok(saved) => serde_json::json!({
                        "success": true,
                        "result": { "path": saved },
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

    // Workspace number, column index and tile index of every window that sits in a strip.
    let mut ws_of: HashMap<WindowId, i32> = HashMap::new();
    let mut place_of: HashMap<WindowId, (usize, usize)> = HashMap::new();
    for (oid, monitor) in eng.monitors().iter() {
        for (&ws_id, ws) in monitor.workspaces.iter() {
            let number = eng.workspace_number_on(*oid, ws_id);
            for (col_idx, col) in ws.columns.iter().enumerate() {
                for (tile_idx, tile) in col.tiles.iter().enumerate() {
                    ws_of.insert(tile.window_id, number);
                    place_of.insert(tile.window_id, (col_idx, tile_idx));
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
            // A floating or minimized window is in no column; it belongs to the workspace
            // it floats on / comes back to.
            "workspace": ws_of
                .get(wid)
                .copied()
                .or_else(|| eng.float_workspace_number(*wid))
                .or_else(|| eng.minimized_workspace_number(*wid)),
            // Position in the strip (0-based); null for a floating or minimized window.
            "column": place_of.get(wid).map(|p| p.0),
            "tile": place_of.get(wid).map(|p| p.1),
            "floating": eng.is_floating(*wid),
            "minimized": eng.is_minimized(*wid),
            // Fullscreen, and maximized to the edges (niri `maximize-window-to-edges`); a window
            // keeps its column while it is either.
            "fullscreen": eng.is_fullscreen(*wid),
            "maximized": eng.is_maximized_to_edges(*wid),
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

/// niri's IPC positioning excludes Moving, while Starting is still adjustable.
fn floating_move_target(
    grab: &crate::input::low_level_hook::GrabState,
    engine: &TilingEngine,
) -> Option<WindowId> {
    use crate::input::low_level_hook::GrabState;
    match grab {
        GrabState::Move(grab)
            if !grab.is_cancelled()
                && (grab.has_drag_started()
                    || engine.native_move_resize_active(grab.window_id)) =>
        {
            Some(grab.window_id)
        }
        _ => None,
    }
}

/// Requests that only read do not wake the task that finds out what changed.
fn is_query(message: &IpcMessage) -> bool {
    matches!(
        message,
        IpcMessage::GetState
            | IpcMessage::WindowList
            | IpcMessage::WindowFind { .. }
            | IpcMessage::WorkspaceList
            | IpcMessage::GetWorkspaceList
            | IpcMessage::MonitorList
            | IpcMessage::GetFocus
            | IpcMessage::ListBindings
            | IpcMessage::GetBindings
            | IpcMessage::PresetList
            | IpcMessage::ListSnapshots
            | IpcMessage::LayoutExport
            | IpcMessage::SubscribeEvents { .. }
            // The niri-shaped requests only read the state.
            | IpcMessage::Workspaces
            | IpcMessage::Windows
            | IpcMessage::FocusedWindow
            | IpcMessage::OverviewState
            | IpcMessage::Outputs
            | IpcMessage::FocusedOutput
            | IpcMessage::KeyboardLayouts
            | IpcMessage::PickWindow
            | IpcMessage::PickColor
            | IpcMessage::Version
            | IpcMessage::ReturnError
    )
}

/// The actions whose words are an argv or a command line, never options: a `--id` among them
/// belongs to the program that is started.
pub(crate) fn is_spawn_action(name: &str) -> bool {
    matches!(
        name,
        "spawn" | "exec" | "spawn-sh" | "spawn-cmd" | "spawn_cmd"
    )
}

/// [`split_window_target`] for the action `name`: the words of a `spawn` are left alone.
pub(crate) fn split_action_target(
    name: &str,
    args: &[String],
    quoted: &[bool],
) -> Result<(Vec<String>, Vec<bool>, Option<isize>), String> {
    if is_spawn_action(name) {
        Ok((args.to_vec(), quoted.to_vec(), None))
    } else {
        split_window_target(args, quoted)
    }
}

/// niri's `--id <n>` (also `--id=<n>`, and `--window-id` where niri's action calls it so) in
/// the arguments of an action: the window to act on instead of the focused one.  Returns the
/// arguments without it, the `quoted` flags that go with them, and the window.
pub(crate) fn split_window_target(
    args: &[String],
    quoted: &[bool],
) -> Result<(Vec<String>, Vec<bool>, Option<isize>), String> {
    let mut rest = Vec::new();
    let mut rest_quoted = Vec::new();
    let mut target = None;
    let mut i = 0;
    while i < args.len() {
        let word = args[i].as_str();
        let value = if quoted.get(i).copied().unwrap_or(false) {
            None
        } else if let Some(v) = word
            .strip_prefix("--id=")
            .or_else(|| word.strip_prefix("--window-id="))
        {
            i += 1;
            Some(v.to_string())
        } else if word == "--id" || word == "--window-id" {
            let Some(v) = args.get(i + 1) else {
                return Err(format!("{} needs a window id", word));
            };
            i += 2;
            Some(v.clone())
        } else {
            None
        };
        match value {
            Some(value) => {
                if target.is_some() {
                    return Err("window id specified more than once".to_string());
                }
                let id: isize = value.trim().parse().map_err(|_| {
                    format!(
                        "`{}` is not a window id (the number that `wiri-ctl windows` shows as hwnd)",
                        value
                    )
                })?;
                target = Some(id);
            }
            None => {
                rest.push(args[i].clone());
                rest_quoted.push(quoted.get(i).copied().unwrap_or(false));
                i += 1;
            }
        }
    }
    Ok((rest, rest_quoted, target))
}

#[cfg(test)]
mod floating_height_parity_tests;

#[cfg(test)]
mod by_id_focus_tests;

#[cfg(test)]
mod focus_converge_tests;

#[cfg(test)]
mod window_target_tests {
    use super::split_window_target;
    use crate::input::{parse_action_flagged, Action, WorkspaceAction, WorkspaceRef};

    fn words(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn duplicate_window_targets_are_rejected_across_aliases_and_syntaxes() {
        for args in [
            vec!["2", "--id", "100", "--id", "101"],
            vec!["--id=100", "--id=101"],
            vec!["--window-id", "100", "--id=101"],
            vec!["--id=100", "--window-id", "101"],
            vec!["--window-id=100", "--window-id=100"],
        ] {
            assert!(split_window_target(&words(&args), &[]).is_err(), "{args:?}");
        }
    }

    #[test]
    fn single_window_target_preserves_other_arguments_and_quote_bits() {
        for target in [
            vec!["--id", "100"],
            vec!["--id=100"],
            vec!["--window-id", "100"],
            vec!["--window-id=100"],
        ] {
            let mut args = words(&["2"]);
            args.extend(words(&target));
            let (rest, quoted, id) = split_window_target(&args, &[true]).unwrap();
            assert_eq!(rest, words(&["2"]));
            assert_eq!(quoted, vec![true]);
            assert_eq!(id, Some(100));
        }
    }

    #[test]
    fn quoted_target_spelling_remains_a_literal_workspace_name() {
        for name in ["--id=100", "--id", "--window-id=100", "--window-id"] {
            let args = words(&["3", "--workspace", name]);
            let (rest, quoted, target) = split_window_target(&args, &[false, false, true]).unwrap();
            assert_eq!(rest, args);
            assert_eq!(quoted, vec![false, false, true]);
            assert_eq!(target, None);
            assert_eq!(
                parse_action_flagged("move-workspace-to-index", &rest, &quoted),
                Some(Action::WorkspaceAction(WorkspaceAction::MoveToIndex {
                    index: 3,
                    workspace: Some(WorkspaceRef::Name(name.to_string())),
                }))
            );
        }
        let args = words(&["--id=100", "--id=101"]);
        assert_eq!(
            split_window_target(&args, &[true, false]).unwrap(),
            (words(&["--id=100"]), vec![true], Some(101))
        );
    }
}

/// Write one event to a subscriber as a line of JSON, when its filter lets the type through.
/// `false` when the client is gone.
async fn write_event(
    pipe: &mut tokio::net::windows::named_pipe::NamedPipeServer,
    event_types: &[String],
    event: &IpcEvent,
    dialect: super::wire::Dialect,
) -> bool {
    let value = match serde_json::to_value(event) {
        Ok(v) => v,
        Err(e) => {
            warn!("IPC: Failed to encode event: {}", e);
            return true;
        }
    };
    if !subscription_allows_event(event_types, &value) {
        return true;
    }
    let Some(value) = super::wire::event(dialect, event) else {
        return true;
    };
    write_line(pipe, &value).await.is_ok()
}

async fn write_line(
    pipe: &mut tokio::net::windows::named_pipe::NamedPipeServer,
    value: &serde_json::Value,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    // A stalled client cannot hold a subscription forever. No clock is armed
    // until an actual write is pending.
    tokio::time::timeout(IPC_RESPONSE_BUDGET, pipe.write_all(&bytes)).await??;
    Ok(())
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
    let mut engine = engine.write();

    // Fast path: if the engine doesn't know this window, give up early.
    if !engine.tiled_windows().contains_key(&wid) {
        return false;
    }

    // Explicit focus must activate even when the layout still caches this tile
    // while a float owns the foreground. Share the hotkey/MRU path, which also
    // restores minimized targets and reveals their workspace before activation.
    if !engine.focus_action_window(wid, backend) {
        return false;
    }
    engine.apply_all(backend);
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
    path: &str,
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
            .create_with_security_attributes_raw(path, sa_ptr)?
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{BackendHandle, WindowInfo, WindowState};
    use crate::layout::LayoutConfig;
    use crate::utils::OutputId;

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    // Float 101 owns the observed foreground, while tile 100 deliberately remains
    // the layout's cached activation target. Returning to 100 is an explicit
    // activation request even though the layout focus has not changed.
    fn server_with_float_over_cached_tile() -> (
        IpcServer,
        Arc<parking_lot::RwLock<TilingEngine>>,
        BackendHandle,
    ) {
        let (mut server, engine) = server_with_windows(&[100, 101]);
        let backend = BackendHandle::default_for_test();
        server.set_backend(backend.clone());
        {
            let mut engine = engine.write();
            engine.toggle_floating_for(WindowId::new(101), &backend);
            assert!(engine.focus_float(WindowId::new(101), &backend));
            engine.apply_all(&backend);
            assert_eq!(
                engine.monitors().focused().unwrap().focus_window,
                Some(WindowId::new(100))
            );
            assert_eq!(engine.ipc_snapshot().focused_window(), Some(101));
        }
        (server, engine, backend)
    }

    #[test]
    fn parity_explicit_focus_legacy_retries_refused_cached_tile_each_request() {
        use crate::backend::activation_attempt_test_support::with_refused_activation_attempts;

        with_refused_activation_attempts(|| {
            let (server, engine, backend) = server_with_float_over_cached_tile();
            for request in 0..2 {
                let (reply, attempts) = with_refused_activation_attempts(|| {
                    server.process_message(IpcMessage::FocusWindow { window_hwnd: 100 })
                });
                assert_eq!(reply["success"], true, "{reply}");
                assert_eq!(
                    attempts,
                    vec![100],
                    "explicit request {request} must activate the cached tile"
                );
                let (_, attempts) =
                    with_refused_activation_attempts(|| engine.write().apply_all(&backend));
                assert!(
                    attempts.is_empty(),
                    "layout must not retry a refused activation"
                );
            }
        });
    }

    #[test]
    fn parity_explicit_focus_run_action_reactivates_cached_tile_after_float() {
        use crate::backend::activation_attempt_test_support::with_refused_activation_attempts;

        with_refused_activation_attempts(|| {
            let (server, _, _) = server_with_float_over_cached_tile();
            let (reply, attempts) = with_refused_activation_attempts(|| {
                server.process_message(action("focus-window", &["--id", "100"]))
            });
            assert_eq!(reply["success"], true, "{reply}");
            assert_eq!(
                attempts,
                vec![100],
                "explicit focus must use backend activation even when the layout target is cached"
            );
        });
    }

    #[test]
    fn parity_explicit_focus_layout_preserves_the_float_foreground() {
        use crate::backend::activation_attempt_test_support::with_refused_activation_attempts;

        with_refused_activation_attempts(|| {
            let (_, engine, backend) = server_with_float_over_cached_tile();
            let (_, attempts) = with_refused_activation_attempts(|| {
                engine.write().apply_all(&backend);
                engine.write().apply_all(&backend);
            });
            assert!(
                attempts.is_empty(),
                "ordinary layout must leave the float focused"
            );
            assert_eq!(engine.read().ipc_snapshot().focused_window(), Some(101));
        });
    }

    #[test]
    fn parity_explicit_focus_unknown_target_does_not_attempt_activation() {
        use crate::backend::activation_attempt_test_support::with_refused_activation_attempts;

        with_refused_activation_attempts(|| {
            let (server, engine, _) = server_with_float_over_cached_tile();
            let (reply, attempts) = with_refused_activation_attempts(|| {
                server.process_message(IpcMessage::FocusWindow { window_hwnd: 999 })
            });
            assert_eq!(reply["success"], false, "{reply}");
            assert!(attempts.is_empty());
            assert_eq!(engine.read().ipc_snapshot().focused_window(), Some(101));
        });
    }

    #[test]
    fn parity_explicit_focus_typed_size_target_does_not_activate_the_tile() {
        use crate::backend::activation_attempt_test_support::with_refused_activation_attempts;

        with_refused_activation_attempts(|| {
            let (server, engine, _) = server_with_float_over_cached_tile();
            let before = engine
                .read()
                .ipc_snapshot()
                .windows
                .into_iter()
                .find(|w| w.id == 100)
                .unwrap()
                .layout
                .tile_size
                .0;
            let request = super::super::wire::decode(serde_json::json!({
                "Action":{"SetWindowWidth":{"id":100,"change":{"SetFixed":300}}}
            }))
            .unwrap();
            let (reply, attempts) =
                with_refused_activation_attempts(|| server.process_message(request.message));
            assert_eq!(reply["success"], true, "{reply}");
            assert!(
                attempts.is_empty(),
                "targeted sizing must not activate its target"
            );
            let snapshot = engine.read().ipc_snapshot();
            assert_eq!(snapshot.focused_window(), Some(101));
            let after = snapshot
                .windows
                .iter()
                .find(|w| w.id == 100)
                .unwrap()
                .layout
                .tile_size
                .0;
            assert!(after < before, "the requested size must still apply");
        });
    }

    #[test]
    fn parity_explicit_focus_workspace_id_transfer_does_not_activate_the_tile() {
        use crate::backend::activation_attempt_test_support::with_refused_activation_attempts;

        with_refused_activation_attempts(|| {
            let (server, engine, _) = server_with_float_over_cached_tile();
            let source = engine.read().monitors().focused_id();
            let destination = OutputId::from_name("explicit-focus-destination");
            let id = {
                let mut engine = engine.write();
                engine.register_monitor(
                    destination,
                    Rect::new(1920, 0, 1200, 800),
                    Rect::new(1920, 0, 1200, 800),
                );
                engine.monitors().get(&destination).unwrap().workspaces[&0].ipc_id
            };
            let request = super::super::wire::decode(serde_json::json!({
                "Action":{"MoveWindowToWorkspace":{"window_id":100,"reference":{"Id":id},"focus":false}}
            }))
            .unwrap();
            let (reply, attempts) =
                with_refused_activation_attempts(|| server.process_message(request.message));
            assert_eq!(reply["success"], true, "{reply}");
            assert!(
                attempts.is_empty(),
                "an explicit workspace transfer must not activate its target"
            );
            let engine = engine.read();
            assert_eq!(engine.monitors().focused_id(), source);
            assert_eq!(engine.ipc_snapshot().focused_window(), Some(101));
            assert_eq!(
                engine
                    .locate_window(WindowId::new(100))
                    .map(|(output, _, _)| output),
                Some(destination)
            );
        });
    }

    #[test]
    fn parity_explicit_focus_recorder_restores_outer_scope_after_unwind() {
        use crate::backend::activation_attempt_test_support::with_refused_activation_attempts;

        let backend = BackendHandle::default_for_test();
        let (_, attempts) = with_refused_activation_attempts(|| {
            assert!(backend.activate_window(100).is_err());
            let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                with_refused_activation_attempts(|| {
                    assert!(backend.activate_window(101).is_err());
                    panic!("exercise recorder restoration");
                });
            }));
            assert!(unwind.is_err());
            assert!(backend.activate_window(102).is_err());
        });
        assert_eq!(attempts, vec![100, 102]);
    }

    #[test]
    fn parity_move_float_typed_and_legacy_requests_apply_the_same_geometry() {
        let (mut server, engine) = server_with_windows(&[100, 101]);
        server.set_backend(BackendHandle::default_for_test());
        engine
            .write()
            .toggle_floating_for(WindowId::new(100), &BackendHandle::default_for_test());
        let request =
            super::super::wire::decode(serde_json::json!({"Action":{"MoveFloatingWindow":{
                "id":100,"x":{"SetProportion":50.0},"y":{"SetFixed":20.0}
            }}}))
            .unwrap();
        let reply = server.process_message(request.message);
        assert_eq!(reply["success"], true, "{reply}");
        let layout = engine
            .read()
            .ipc_snapshot()
            .windows
            .into_iter()
            .find(|w| w.id == 100)
            .unwrap()
            .layout;
        assert_eq!(layout.tile_pos_in_workspace_view, Some((960.0, 20.0)));
        let reply = server.process_message(IpcMessage::RunAction {
            name: "move-floating-window".into(),
            args: words(&["--id=100", "-x", "-10%"]),
            quoted: vec![],
        });
        assert_eq!(reply["success"], true, "{reply}");
        let layout = engine
            .read()
            .ipc_snapshot()
            .windows
            .into_iter()
            .find(|w| w.id == 100)
            .unwrap()
            .layout;
        assert_eq!(layout.tile_pos_in_workspace_view, Some((768.0, 20.0)));
        for request in [
            IpcMessage::MoveFloatingWindow {
                id: Some(999),
                x: Default::default(),
                y: Default::default(),
            },
            IpcMessage::MoveFloatingWindow {
                id: Some(u64::MAX),
                x: Default::default(),
                y: Default::default(),
            },
            IpcMessage::MoveFloatingWindow {
                id: Some(100),
                x: super::super::PositionChange::SetFixed(f64::NAN),
                y: Default::default(),
            },
            IpcMessage::RunAction {
                name: "move-floating-window".into(),
                args: words(&["--x", "20%%"]),
                quoted: vec![],
            },
        ] {
            assert_eq!(server.process_message(request)["success"], false);
        }
    }

    #[test]
    fn parity_move_float_guard_distinguishes_starting_from_moving_without_advancing_it() {
        use crate::input::{grab::MoveGrab, low_level_hook::GrabState};
        use crate::utils::{Point, Size};
        let (_, engine) = server_with_windows(&[100]);
        let window = WindowId::new(100);
        let origin = Point::new(0, 0);
        let grab = MoveGrab::new(window, origin, origin, Size::new(800, 600), 0);
        let queued = GrabState::Move(grab.clone());
        assert_eq!(floating_move_target(&queued, &engine.read()), None);
        assert!(
            !grab.has_drag_started(),
            "the guard must not start a pending move"
        );
        assert!(!grab.tiled_drag_started(Point::new(255, 0), 1.0));
        assert_eq!(floating_move_target(&queued, &engine.read()), None);
        assert!(grab.tiled_drag_started(Point::new(256, 0), 1.0));
        assert_eq!(floating_move_target(&queued, &engine.read()), Some(window));
        grab.cancel();
        assert_eq!(floating_move_target(&queued, &engine.read()), None);

        let float_grab = MoveGrab::new(window, origin, origin, Size::new(800, 600), 0);
        let queued = GrabState::Move(float_grab);
        let be = BackendHandle::default_for_test();
        engine.write().toggle_floating_for(window, &be);
        assert_eq!(floating_move_target(&queued, &engine.read()), None);
        // A floating move enters the native-loop exclusion on its first motion.
        engine.write().native_move_resize_start(window);
        assert_eq!(floating_move_target(&queued, &engine.read()), Some(window));
    }

    #[test]
    fn niris_id_flag_names_the_window_and_is_taken_out_of_the_arguments() {
        let (args, quoted, target) =
            split_window_target(&words(&["--id", "1234", "+10%"]), &[]).unwrap();
        assert_eq!(args, words(&["+10%"]));
        assert_eq!(quoted, vec![false]);
        assert_eq!(target, Some(1234));
        // niri spells it `--window-id` for move-window-to-workspace; `=` works too.
        let (args, _, target) =
            split_window_target(&words(&["--window-id", "77", "2"]), &[]).unwrap();
        assert_eq!((args, target), (words(&["2"]), Some(77)));
        let (args, _, target) = split_window_target(&words(&["2", "--id=9"]), &[]).unwrap();
        assert_eq!((args, target), (words(&["2"]), Some(9)));
        // No flag: the arguments are left alone, with their quoting.
        let (args, quoted, target) =
            split_window_target(&words(&["2", "name"]), &[true, false]).unwrap();
        assert_eq!(args, words(&["2", "name"]));
        assert_eq!(quoted, vec![true, false]);
        assert_eq!(target, None);
    }

    #[test]
    fn a_window_flag_without_a_window_is_an_error_that_says_so() {
        assert!(split_window_target(&words(&["--id"]), &[])
            .unwrap_err()
            .contains("needs a window id"));
        let error = split_window_target(&words(&["--id", "abc"]), &[]).unwrap_err();
        assert!(error.contains("`abc` is not a window id"), "{}", error);
    }

    #[test]
    fn quoting_stays_with_the_argument_it_was_written_on() {
        // `focus-workspace "2"` after the window flag was taken out of the middle.
        let (args, quoted, target) =
            split_window_target(&words(&["--id", "5", "2"]), &[false, false, true]).unwrap();
        assert_eq!(args, words(&["2"]));
        assert_eq!(quoted, vec![true]);
        assert_eq!(target, Some(5));
    }

    #[test]
    fn only_requests_that_can_change_the_state_wake_the_state_task() {
        assert!(is_query(&IpcMessage::GetState));
        assert!(is_query(&IpcMessage::WindowList));
        assert!(is_query(&IpcMessage::MonitorList));
        assert!(is_query(&IpcMessage::GetFocus));
        for message in [
            IpcMessage::Outputs,
            IpcMessage::FocusedOutput,
            IpcMessage::KeyboardLayouts,
            IpcMessage::PickWindow,
            IpcMessage::PickColor,
        ] {
            assert!(is_query(&message));
        }
        assert!(!is_query(&IpcMessage::Output {
            output: "synthetic".into(),
            action: serde_json::json!("On")
        }));
        assert!(!is_query(&IpcMessage::FocusWindow { window_hwnd: 1 }));
        assert!(!is_query(&IpcMessage::RunAction {
            name: "close-window".into(),
            args: Vec::new(),
            quoted: Vec::new(),
        }));
        assert!(!is_query(&IpcMessage::ReloadConfig));
    }

    fn action(name: &str, args: &[&str]) -> IpcMessage {
        IpcMessage::RunAction {
            name: name.to_string(),
            args: words(args),
            quoted: Vec::new(),
        }
    }

    #[test]
    fn an_action_with_id_acts_on_that_window_not_on_the_focused_one() {
        for inactive_workspace in [false, true] {
            let (mut server, engine) = server_with_windows(&[100, 101]);
            server.set_backend(BackendHandle::default_for_test());
            engine
                .write()
                .sync_os_foreground(WindowId::new(101), &BackendHandle::default_for_test());
            assert_eq!(engine.read().ipc_snapshot().focused_window(), Some(101));
            if inactive_workspace {
                engine.write().move_window_to_workspace_ref(
                    Some(WindowId::new(100)),
                    crate::input::WorkspaceRef::Index(2),
                    false,
                    &BackendHandle::default_for_test(),
                );
            }
            let output = engine.read().monitors().focused_id();
            let workspace = engine.read().monitors().focused().unwrap().active_workspace;
            // 101 is the engine's focus (the window that was added last).
            let reply = server.process_message(action("fullscreen-window", &["--id", "100"]));
            assert_eq!(reply["success"], true, "{}", reply);
            assert!(engine.read().is_fullscreen(WindowId::new(100)));
            assert!(!engine.read().is_fullscreen(WindowId::new(101)));
            assert_eq!(engine.read().ipc_snapshot().focused_window(), Some(101));
            assert_eq!(engine.read().monitors().focused_id(), output);
            assert_eq!(
                engine.read().monitors().focused().unwrap().active_workspace,
                workspace,
                "an explicit fullscreen target must not activate its workspace"
            );
            // The same with niri's other spelling.
            let reply = server.process_message(action("fullscreen-window", &["--id=100"]));
            assert_eq!(reply["success"], true, "{}", reply);
            assert!(
                !engine.read().is_fullscreen(WindowId::new(100)),
                "toggled back"
            );
            assert_eq!(engine.read().ipc_snapshot().focused_window(), Some(101));
            assert_eq!(engine.read().monitors().focused_id(), output);
            assert_eq!(
                engine.read().monitors().focused().unwrap().active_workspace,
                workspace
            );
        }
    }

    #[test]
    fn maximize_and_rule_opacity_with_id_leave_the_focus_alone() {
        // niri's MaximizeWindowToEdgesById / ToggleWindowRuleOpacityById name the window and
        // never activate it (niri:src/input/mod.rs:1753-1761); these used to focus it first.
        let (mut server, engine) = server_with_windows(&[100, 101]);
        let backend = BackendHandle::default_for_test();
        server.set_backend(backend.clone());
        engine
            .write()
            .sync_os_foreground(WindowId::new(101), &backend);
        assert_eq!(engine.read().ipc_snapshot().focused_window(), Some(101));
        let output = engine.read().monitors().focused_id();
        let workspace = engine.read().monitors().focused().unwrap().active_workspace;

        let reply = server.process_message(action("maximize-window-to-edges", &["--id", "100"]));
        assert_eq!(reply["success"], true, "{reply}");
        assert!(engine.read().is_maximized_to_edges(WindowId::new(100)));
        assert!(!engine.read().is_maximized_to_edges(WindowId::new(101)));
        assert_eq!(engine.read().ipc_snapshot().focused_window(), Some(101));
        assert_eq!(engine.read().monitors().focused_id(), output);
        assert_eq!(
            engine.read().monitors().focused().unwrap().active_workspace,
            workspace
        );

        let reply = server.process_message(action("maximize-window-to-edges", &["--id=100"]));
        assert_eq!(reply["success"], true, "{reply}");
        assert!(
            !engine.read().is_maximized_to_edges(WindowId::new(100)),
            "a second call toggles it back"
        );
        assert_eq!(engine.read().ipc_snapshot().focused_window(), Some(101));

        let reply = server.process_message(action("toggle-window-rule-opacity", &["--id", "100"]));
        assert_eq!(reply["success"], true, "{reply}");
        assert_eq!(engine.read().ipc_snapshot().focused_window(), Some(101));
        assert_eq!(engine.read().monitors().focused_id(), output);
    }

    #[test]
    fn set_window_height_with_id_preserves_the_focused_tile() {
        let (mut server, engine) = server_with_windows(&[100, 101]);
        let backend = BackendHandle::default_for_test();
        server.set_backend(backend.clone());
        {
            let mut eng = engine.write();
            eng.sync_os_foreground(WindowId::new(100), &backend);
            eng.consume_window_into_column(&backend);
            eng.sync_os_foreground(WindowId::new(101), &backend);
        }
        let output = engine.read().monitors().focused_id();
        let workspace = engine.read().monitors().focused().unwrap().active_workspace;

        let reply = server.process_message(action("set-window-height", &["--id", "100", "300"]));

        assert_eq!(reply["success"], true, "{reply}");
        let eng = engine.read();
        let monitor = eng.monitors().focused().unwrap();
        let tiles = &monitor.workspace().unwrap().columns[0].tiles;
        assert_eq!(tiles.len(), 2);
        let target = tiles
            .iter()
            .find(|tile| tile.window_id == WindowId::new(100))
            .unwrap();
        let other = tiles
            .iter()
            .find(|tile| tile.window_id == WindowId::new(101))
            .unwrap();
        assert!(
            target.height_weight < 1.0,
            "300 px is less than half the column"
        );
        assert_eq!(
            other.height_weight, 1.0,
            "the other tile keeps its share weight"
        );
        assert_eq!(eng.ipc_snapshot().focused_window(), Some(101));
        assert_eq!(eng.monitors().focused_id(), output);
        assert_eq!(monitor.active_workspace, workspace);
    }

    #[test]
    fn parity_explicit_window_width_typed_tiled_preserves_inactive_output_focus() {
        assert_explicit_window_width_preserves_focus(false, true);
    }

    #[test]
    fn parity_explicit_window_width_legacy_tiled_preserves_inactive_output_focus() {
        assert_explicit_window_width_preserves_focus(false, false);
    }

    #[test]
    fn parity_explicit_window_width_typed_floating_preserves_inactive_output_focus() {
        assert_explicit_window_width_preserves_focus(true, true);
    }

    #[test]
    fn parity_explicit_window_width_legacy_floating_preserves_inactive_output_focus() {
        assert_explicit_window_width_preserves_focus(true, false);
    }

    fn width_target_server_fixture(
        floating: bool,
    ) -> (
        IpcServer,
        Arc<parking_lot::RwLock<TilingEngine>>,
        OutputId,
        OutputId,
    ) {
        use crate::layout::{Column, Tile, Workspace};

        let mut eng = TilingEngine::new(LayoutConfig {
            outer_gaps: (0, 0, 0, 0),
            column_gap: 0,
            window_gap: 0,
            border_width: 0,
            reserve_top: 0,
            reserve_bottom: 0,
            reserve_left: 0,
            reserve_right: 0,
            ..LayoutConfig::default()
        });
        let left = OutputId::from_name("width-global");
        let right = OutputId::from_name("width-target");
        eng.register_monitor(left, Rect::new(0, 0, 1200, 900), Rect::new(0, 0, 1200, 900));
        eng.register_monitor_with_scale(
            right,
            Rect::new(1200, 0, 1200, 900),
            Rect::new(1200, 0, 1200, 900),
            1.5,
        );
        let backend = BackendHandle::default_for_test();
        for id in [100, 200, 201, 202, 210] {
            eng.add_window(window(id), &backend);
        }
        let make_workspace = |name: &str, groups: &[&[isize]]| {
            let mut ws = Workspace::new();
            ws.name = Some(name.to_string());
            for group in groups {
                let mut col = Column::new();
                col.tiles = group
                    .iter()
                    .map(|id| Tile::new(WindowId::new(*id)))
                    .collect();
                col.set_width(Some(400));
                col.select_window(WindowId::new(group[0]));
                ws.columns.push(col);
            }
            ws
        };
        {
            let monitor = eng.monitors_mut().get_mut(&left).unwrap();
            monitor.workspaces.clear();
            monitor.insert_workspace(40, make_workspace("global", &[&[100]]));
            monitor.switch_workspace(40);
            monitor.set_focus(WindowId::new(100));
        }
        {
            let monitor = eng.monitors_mut().get_mut(&right).unwrap();
            monitor.workspaces.clear();
            monitor.insert_workspace(40, make_workspace("remote-active", &[&[210]]));
            monitor.insert_workspace(7, make_workspace("remote-inactive", &[&[202], &[200, 201]]));
            monitor.switch_workspace(40);
            monitor.set_focus(WindowId::new(210));
        }
        eng.set_focused_output(left);
        if floating {
            eng.toggle_floating_for(WindowId::new(200), &backend);
        }
        eng.sync_os_foreground(WindowId::new(100), &backend);
        let engine = Arc::new(parking_lot::RwLock::new(eng));
        let mut server = IpcServer::new();
        server.set_engine(engine.clone());
        server.set_backend(backend);
        (server, engine, left, right)
    }

    fn assert_explicit_window_width_preserves_focus(floating: bool, typed: bool) {
        let (server, engine, left, right) = width_target_server_fixture(floating);
        let cursor = |eng: &TilingEngine, output| {
            let monitor = eng.monitors().get(&output).unwrap();
            (
                monitor.active_workspace,
                monitor.focus_column,
                monitor.focus_window,
            )
        };
        let before = engine.read().ipc_snapshot();
        assert_eq!(before.focused_window(), Some(100));
        let cursors = (cursor(&engine.read(), left), cursor(&engine.read(), right));
        assert_eq!(cursors.0 .0, 40);
        assert_eq!(cursors.1 .0, 40);
        assert_eq!(engine.read().monitors().focused_id(), Some(left));
        let request = if typed {
            super::super::wire::decode(serde_json::json!({
                "Action":{"SetWindowWidth":{"id":200,"change":{"SetFixed":160}}}
            }))
            .unwrap()
            .message
        } else {
            action("set-window-width", &["--id=200", "160"])
        };

        let reply = server.process_message(request);

        assert_eq!(reply["success"], true, "{reply}");
        let eng = engine.read();
        let after = eng.ipc_snapshot();
        let target = after
            .windows
            .iter()
            .find(|window| window.id == 200)
            .unwrap();
        assert_eq!(target.is_floating, floating);
        assert_eq!(
            target.layout.window_size.0, 160,
            "160 logical pixels is 240 physical at 150%"
        );
        if !floating {
            let column = &eng.monitors().get(&right).unwrap().workspaces[&7].columns[1];
            assert_eq!(column.width, Some(240));
            assert_eq!(
                after
                    .windows
                    .iter()
                    .find(|window| window.id == 201)
                    .unwrap()
                    .layout
                    .window_size
                    .0,
                160,
                "a tiled window changes its whole column width"
            );
        }
        for id in [100, 202, 210] {
            assert_eq!(
                after
                    .windows
                    .iter()
                    .find(|window| window.id == id)
                    .unwrap()
                    .layout,
                before
                    .windows
                    .iter()
                    .find(|window| window.id == id)
                    .unwrap()
                    .layout,
                "unrelated window {id} must retain its geometry"
            );
        }
        assert_eq!((cursor(&eng, left), cursor(&eng, right)), cursors);
        assert_eq!(eng.monitors().focused_id(), Some(left));
        assert_eq!(after.focused_window(), Some(100));
    }

    #[test]
    fn parity_explicit_window_width_rejects_unmanaged_and_out_of_range_ids() {
        let (server, engine, _, _) = width_target_server_fixture(false);
        let before = engine.read().ipc_snapshot();
        for id in [999, u64::MAX] {
            let request = super::super::wire::decode(serde_json::json!({
                "Action":{"SetWindowWidth":{"id":id,"change":{"SetFixed":160}}}
            }))
            .unwrap();
            let reply = server.process_message(request.message);
            assert_eq!(reply["success"], false, "id={id}: {reply}");
            assert_eq!(engine.read().ipc_snapshot(), before);
        }
    }

    #[test]
    fn parity_typed_size_width_actions_apply_percent_points_and_negative_sets() {
        for variant in ["SetColumnWidth", "SetWindowWidth"] {
            let (mut server, engine) = server_with_windows(&[100]);
            server.set_backend(BackendHandle::default_for_test());
            let apply = |change: serde_json::Value| {
                let request = super::super::wire::decode(serde_json::json!({
                    "Action":{(variant):{"change":change}}
                }))
                .unwrap();
                let reply = server.process_message(request.message);
                assert_eq!(reply["success"], true, "{variant}: {reply}");
            };

            apply(serde_json::json!({"SetProportion":50.0}));
            let half_width = {
                let eng = engine.read();
                let column = &eng
                    .monitors()
                    .focused()
                    .unwrap()
                    .workspace()
                    .unwrap()
                    .columns[0];
                assert_eq!(column.width_proportion, Some(0.5));
                column.width.unwrap()
            };
            assert!(
                (600..=1000).contains(&half_width),
                "{variant}: {half_width}"
            );

            apply(serde_json::json!({"AdjustProportion":-10.0}));
            let adjusted_width = {
                let eng = engine.read();
                let column = &eng
                    .monitors()
                    .focused()
                    .unwrap()
                    .workspace()
                    .unwrap()
                    .columns[0];
                assert!((column.width_proportion.unwrap() - 0.4).abs() < 1e-9);
                column.width.unwrap()
            };
            assert!(adjusted_width < half_width, "{variant}: width must shrink");

            apply(serde_json::json!({"SetFixed":-10}));
            let eng = engine.read();
            let column = &eng
                .monitors()
                .focused()
                .unwrap()
                .workspace()
                .unwrap()
                .columns[0];
            assert_eq!(column.width_fixed_logical, Some(0.0));
            assert!(column.width.unwrap() < adjusted_width);
        }
    }

    #[test]
    fn parity_typed_size_targeted_height_applies_without_changing_focus() {
        let (mut server, engine) = server_with_windows(&[100, 101]);
        let backend = BackendHandle::default_for_test();
        server.set_backend(backend.clone());
        {
            let mut eng = engine.write();
            eng.sync_os_foreground(WindowId::new(100), &backend);
            eng.consume_window_into_column(&backend);
            eng.sync_os_foreground(WindowId::new(101), &backend);
        }
        let focus = engine.read().ipc_snapshot().focused_window();
        let apply = |change: serde_json::Value| {
            let request = super::super::wire::decode(serde_json::json!({
                "Action":{"SetWindowHeight":{"id":100,"change":change}}
            }))
            .unwrap();
            let reply = server.process_message(request.message);
            assert_eq!(reply["success"], true, "{reply}");
            let snapshot = engine.read().ipc_snapshot();
            assert_eq!(snapshot.focused_window(), focus);
            snapshot
                .windows
                .iter()
                .find(|window| window.id == 100)
                .unwrap()
                .layout
                .tile_size
                .1
        };

        let initial = apply(serde_json::json!({"SetProportion":30.0}));
        assert!(
            (200.0..400.0).contains(&initial),
            "30 percent height: {initial}"
        );
        let adjusted = apply(serde_json::json!({"AdjustProportion":-10.0}));
        assert!(adjusted < initial, "negative adjustment must shrink height");
        let clamped = apply(serde_json::json!({"SetFixed":-10}));
        assert!(
            clamped < adjusted,
            "negative Set must clamp, not subtract ten"
        );
    }

    #[test]
    fn an_action_can_only_name_a_window_wiri_manages() {
        let (mut server, engine) = server_with_windows(&[100]);
        server.set_backend(BackendHandle::default_for_test());
        for name in ["fullscreen-window", "close-window", "focus-window"] {
            let reply = server.process_message(action(name, &["--id", "4242"]));
            assert_eq!(reply["success"], false, "{}: {}", name, reply);
            let error = reply["error"].as_str().unwrap_or_default();
            assert!(
                error.contains("no window with id 4242"),
                "{}: {}",
                name,
                error
            );
        }
        assert!(!engine.read().is_fullscreen(WindowId::new(100)));
        // focus-window without a window says what it needs; a bad number says so too.
        let reply = server.process_message(action("focus-window", &[]));
        assert!(
            reply["error"]
                .as_str()
                .unwrap_or_default()
                .contains("needs --id"),
            "{}",
            reply
        );
        let reply = server.process_message(action("fullscreen-window", &["--id", "abc"]));
        assert!(
            reply["error"]
                .as_str()
                .unwrap_or_default()
                .contains("not a window id"),
            "{}",
            reply
        );
    }

    #[test]
    fn a_quoted_workspace_word_reaches_the_action_as_a_name() {
        let (mut server, engine) = server_with_windows(&[100]);
        server.set_backend(BackendHandle::default_for_test());
        let before = engine
            .read()
            .monitors()
            .focused()
            .map(|m| m.active_workspace);
        // `focus-workspace 2` is the second workspace...
        let reply = server.process_message(action("focus-workspace", &["2"]));
        assert_eq!(reply["success"], true, "{}", reply);
        let second = engine
            .read()
            .monitors()
            .focused()
            .map(|m| m.active_workspace);
        assert_ne!(before, second, "the second workspace");
        // ...and `focus-workspace "1"` is the workspace named 1, which there is none of.
        let reply = server.process_message(IpcMessage::RunAction {
            name: "focus-workspace".into(),
            args: words(&["1"]),
            quoted: vec![true],
        });
        assert_eq!(reply["success"], true, "{}", reply);
        assert_eq!(
            engine
                .read()
                .monitors()
                .focused()
                .map(|m| m.active_workspace),
            second,
            "a name that no workspace has does not switch anything"
        );
    }

    #[test]
    fn typed_workspace_ipc_ids_dispatch_all_seven_actions_without_argv_coercion() {
        fn typed(value: serde_json::Value) -> IpcMessage {
            super::super::wire::decode(serde_json::json!({"Action":value}))
                .unwrap()
                .message
        }
        let (mut server, engine) = server_with_windows(&[100, 101]);
        let backend = BackendHandle::default_for_test();
        server.set_backend(backend.clone());
        let source = OutputId::from_name("TestMonitor");
        let destination = OutputId::from_name("typed-id-remote");
        let id = u64::MAX - 17;
        {
            let mut engine = engine.write();
            engine.register_monitor(
                destination,
                Rect::new(1920, 0, 1200, 800),
                Rect::new(1920, 0, 1200, 800),
            );
            engine.set_output_name(source, "DISPLAY1");
            engine.set_output_name(destination, "DISPLAY2");
            let workspace = engine
                .monitors_mut()
                .get_mut(&destination)
                .unwrap()
                .workspaces
                .get_mut(&0)
                .unwrap();
            workspace.ipc_id = id;
            workspace.name = Some("remote-id".into());
            engine.sync_os_foreground(WindowId::new(100), &backend);
        }
        for value in [
            serde_json::json!({"SetWorkspaceName":{"name":"renamed-id","workspace":{"Id":id}}}),
            serde_json::json!({"MoveWorkspaceToIndex":{"index":1,"reference":{"Id":id}}}),
            serde_json::json!({"MoveWindowToWorkspace":{"window_id":101,"reference":{"Id":id},"focus":false}}),
        ] {
            let reply = server.process_message(typed(value.clone()));
            assert_eq!(reply["success"], true, "{value}: {reply}");
        }
        {
            let engine = engine.read();
            assert_eq!(
                engine.workspace_name_on(destination, 0).as_deref(),
                Some("renamed-id")
            );
            assert_eq!(
                engine
                    .locate_window(WindowId::new(101))
                    .map(|(output, key, _)| (output, key)),
                Some((destination, 0))
            );
            assert_eq!(engine.monitors().focused_id(), Some(source));
            assert_eq!(
                engine.monitors().get(&source).unwrap().focus_window,
                Some(WindowId::new(100))
            );
        }
        let reply = server.process_message(typed(serde_json::json!({"MoveColumnToWorkspace":{
            "reference":{"Id":id},"focus":false
        }})));
        assert_eq!(reply["success"], true, "{reply}");
        assert_eq!(
            engine
                .read()
                .locate_window(WindowId::new(100))
                .map(|(output, key, _)| (output, key)),
            Some((destination, 0))
        );
        for value in [
            serde_json::json!({"UnsetWorkspaceName":{"reference":{"Id":id}}}),
            serde_json::json!({"FocusWorkspace":{"reference":{"Id":id}}}),
            serde_json::json!({"MoveWorkspaceToMonitor":{"output":"DISPLAY1","reference":{"Id":id}}}),
        ] {
            let reply = server.process_message(typed(value.clone()));
            assert_eq!(reply["success"], true, "{value}: {reply}");
        }
        let engine = engine.read();
        let snapshot = engine.ipc_snapshot();
        let target = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .unwrap();
        assert_eq!(target.output.as_deref(), Some("DISPLAY1"));
        assert_eq!(target.name, None);
        assert_eq!(
            snapshot
                .windows
                .iter()
                .find(|window| window.id == 100)
                .unwrap()
                .workspace_id,
            Some(id)
        );
        assert_eq!(
            snapshot
                .windows
                .iter()
                .find(|window| window.id == 101)
                .unwrap()
                .workspace_id,
            Some(id)
        );
    }

    #[test]
    fn typed_workspace_ipc_ids_reject_unmanaged_window_targets_without_mutation() {
        let (mut server, engine) = server_with_windows(&[100]);
        server.set_backend(BackendHandle::default_for_test());
        let before = engine.read().ipc_snapshot();
        let id = before.workspaces[0].id;
        for window_id in [0, 999, u64::MAX] {
            let request = super::super::wire::decode(serde_json::json!({"Action":{
                "MoveWindowToWorkspace":{"window_id":window_id,"reference":{"Id":id},"focus":false}
            }}))
            .unwrap();
            let reply = server.process_message(request.message);
            assert_eq!(reply["success"], false, "{window_id}: {reply}");
            assert_eq!(engine.read().ipc_snapshot(), before);
        }
    }

    #[test]
    fn tile_request_named_workspace_keeps_the_destination_output() {
        let (mut server, engine) = server_with_windows(&[100]);
        server.set_backend(BackendHandle::default_for_test());
        let source = OutputId::from_name("TestMonitor");
        let destination = OutputId::from_name("other-monitor");
        let (source_id, destination_id) = {
            let mut eng = engine.write();
            eng.register_monitor(
                destination,
                Rect::new(1920, 0, 1920, 1080),
                Rect::new(1920, 0, 1920, 1040),
            );
            eng.set_output_name(source, "DISPLAY1");
            eng.set_output_name(destination, "DISPLAY2");
            let source_id = eng.monitors().get(&source).unwrap().workspaces[&0].ipc_id;
            let workspace = eng
                .monitors_mut()
                .get_mut(&destination)
                .unwrap()
                .workspaces
                .get_mut(&0)
                .unwrap();
            workspace.name = Some("remote".to_string());
            (source_id, workspace.ipc_id)
        };
        assert_ne!(source_id, destination_id);
        let reply = server.process_message(IpcMessage::TileRequest {
            window_hwnd: 100,
            target_workspace: Some("remote".to_string()),
        });
        assert_eq!(reply["success"], true, "{reply}");
        let eng = engine.read();
        let snapshot = eng.ipc_snapshot();
        let window = snapshot
            .windows
            .iter()
            .find(|window| window.id == 100)
            .unwrap();
        assert_eq!(window.workspace_id, Some(destination_id));
        assert_eq!(window.output.as_deref(), Some("DISPLAY2"));
        assert_eq!(eng.monitors().focused_id(), Some(source));
        assert!(eng.monitors().get(&source).unwrap().workspaces[&0]
            .columns
            .is_empty());
    }

    // ---- the event stream's state ----------------------------------------------------------

    fn window(hwnd: isize) -> WindowInfo {
        WindowInfo {
            hwnd,
            title: format!("Window {}", hwnd),
            class_name: "TestClass".to_string(),
            process_id: 1000,
            bounds: Rect::new(0, 0, 800, 600),
            state: WindowState::Normal,
            is_visible: true,
        }
    }

    fn server_with_windows(ids: &[isize]) -> (IpcServer, Arc<parking_lot::RwLock<TilingEngine>>) {
        let mut engine = TilingEngine::new(LayoutConfig::default());
        engine.register_monitor(
            OutputId::from_name("TestMonitor"),
            Rect::new(0, 0, 1920, 1080),
            Rect::new(0, 0, 1920, 1040),
        );
        let backend = BackendHandle::default_for_test();
        for id in ids {
            engine.add_window(window(*id), &backend);
        }
        let engine = Arc::new(parking_lot::RwLock::new(engine));
        let mut server = IpcServer::new();
        server.set_engine(engine.clone());
        (server, engine)
    }

    fn drain(receiver: &mut broadcast::Receiver<IpcEvent>) -> Vec<IpcEvent> {
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }
        events
    }

    fn types(events: &[IpcEvent]) -> Vec<String> {
        events
            .iter()
            .map(|e| {
                serde_json::to_value(e).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn a_subscriber_is_told_the_state_first_and_how_the_config_loaded() {
        let (server, _engine) = server_with_windows(&[100, 101]);
        server.broadcast_event(IpcEvent::ConfigLoaded {
            config_path: "config.kdl".into(),
            failed: true,
            error: Some("config.kdl:3:5: error: x".into()),
            warnings: 1,
        });
        let (_rx, replay) = server.subscribe_with_replay();
        assert_eq!(
            types(&replay),
            [
                "workspaces_changed",
                "windows_changed",
                "overview_opened_or_closed",
                "keyboard_layouts_changed",
                "config_loaded"
            ]
        );
        match &replay[1] {
            IpcEvent::WindowsChanged { windows } => {
                assert_eq!(
                    windows.iter().map(|w| w.id).collect::<Vec<_>>(),
                    vec![100, 101]
                );
            }
            other => panic!("unexpected {:?}", other),
        }
        assert!(matches!(
            &replay[3],
            IpcEvent::KeyboardLayoutsChanged { .. }
        ));
        match &replay[4] {
            IpcEvent::ConfigLoaded { failed, error, .. } => {
                assert!(*failed);
                assert!(error.as_deref().unwrap().contains("config.kdl:3:5"));
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn a_subscriber_that_comes_late_is_told_the_state_as_it_is_not_as_it_was() {
        // Nobody listened while the windows came: nothing was described (the work is done
        // only for subscribers), yet the first subscriber is told all of them.
        let (server, engine) = server_with_windows(&[100]);
        engine
            .write()
            .add_window(window(101), &BackendHandle::default_for_test());
        server.sync_state();
        assert!(
            server.mirror.lock().last.windows.is_empty(),
            "nobody is listening"
        );
        let (_rx, replay) = server.subscribe_with_replay();
        match &replay[1] {
            IpcEvent::WindowsChanged { windows } => assert_eq!(windows.len(), 2),
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn changes_after_the_subscription_are_sent_once_as_events() {
        let (server, engine) = server_with_windows(&[100]);
        let (mut rx, _replay) = server.subscribe_with_replay();
        // Nothing happened: nothing is sent.
        server.sync_state();
        assert!(drain(&mut rx).is_empty());

        let backend = BackendHandle::default_for_test();
        engine.write().add_window(window(101), &backend);
        server.sync_state();
        let events = drain(&mut rx);
        // The new window is the active one of its workspace (niri tells the workspace first).
        assert_eq!(
            types(&events),
            [
                "workspace_active_window_changed",
                "window_opened_or_changed"
            ]
        );
        // The same state again: nothing more.
        server.sync_state();
        assert!(drain(&mut rx).is_empty());

        engine.write().remove_window(WindowId::new(101), &backend);
        server.sync_state();
        let events = drain(&mut rx);
        let closed = events
            .iter()
            .find_map(|e| match e {
                IpcEvent::WindowClosed { window_hwnd, id } => Some((*window_hwnd, *id)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no window_closed in {:?}", types(&events)));
        assert_eq!(closed, (101, 101));
        assert_eq!(
            types(&events)
                .iter()
                .filter(|t| *t == "window_closed")
                .count(),
            1,
            "{:?}",
            types(&events)
        );
    }

    #[test]
    fn the_focus_the_os_reports_is_the_focus_the_stream_reports() {
        let (server, engine) = server_with_windows(&[100, 101]);
        let (mut rx, replay) = server.subscribe_with_replay();
        match &replay[1] {
            IpcEvent::WindowsChanged { windows } => {
                assert!(
                    windows.iter().all(|w| !w.is_focused),
                    "no foreground report yet"
                );
            }
            other => panic!("unexpected {:?}", other),
        }
        let backend = BackendHandle::default_for_test();
        engine
            .write()
            .sync_os_foreground(WindowId::new(101), &backend);
        server.sync_state();
        let events = drain(&mut rx);
        assert_eq!(
            types(&events),
            ["window_focus_timestamp_changed", "window_focus_changed"]
        );
        assert!(matches!(
            &events[0],
            IpcEvent::WindowFocusTimestampChanged {
                id: 101,
                focus_timestamp: Some(_)
            }
        ));
        match &events[1] {
            IpcEvent::WindowFocusChanged { id } => assert_eq!(*id, Some(101)),
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn an_overview_that_opens_is_reported() {
        let (server, engine) = server_with_windows(&[100]);
        let (mut rx, _replay) = server.subscribe_with_replay();
        engine
            .write()
            .enter_overview(&BackendHandle::default_for_test());
        server.sync_state();
        let events = drain(&mut rx);
        assert!(
            types(&events).contains(&"overview_opened_or_closed".to_string()),
            "{:?}",
            types(&events)
        );
        match events.last().unwrap() {
            IpcEvent::OverviewOpenedOrClosed { is_open } => assert!(*is_open),
            other => panic!("unexpected {:?}", other),
        }
    }

    // ---- niri's request/reply forms (IPC schema 1.2) ----

    #[test]
    fn the_niri_requests_answer_with_the_state_the_stream_would_tell() {
        let (server, engine) = server_with_windows(&[100, 101]);
        let backend = BackendHandle::default_for_test();
        engine
            .write()
            .sync_os_foreground(WindowId::new(101), &backend);

        // The workspaces, in niri's shape.
        let reply = server.process_message(IpcMessage::Workspaces);
        assert_eq!(reply["success"], true, "{}", reply);
        let workspaces = reply["result"]["workspaces"]
            .as_array()
            .expect("the workspaces");
        assert!(!workspaces.is_empty(), "{}", reply);
        let workspace = &workspaces[0];
        for field in [
            "id",
            "idx",
            "name",
            "output",
            "is_active",
            "is_focused",
            "active_window_id",
        ] {
            assert!(
                workspace.get(field).is_some(),
                "workspace.{} is missing: {}",
                field,
                workspace
            );
        }

        // The windows, in niri's shape: the same objects the stream tells.
        let reply = server.process_message(IpcMessage::Windows);
        assert_eq!(reply["success"], true, "{}", reply);
        let windows = reply["result"]["windows"].as_array().expect("the windows");
        assert_eq!(windows.len(), 2, "{}", reply);
        assert_eq!(windows[0]["id"], serde_json::json!(100));
        assert_eq!(windows[1]["id"], serde_json::json!(101));
        assert_eq!(windows[1]["is_focused"], serde_json::json!(true));

        // The focused window: the same object, on its own.
        let reply = server.process_message(IpcMessage::FocusedWindow);
        assert_eq!(reply["success"], true, "{}", reply);
        assert_eq!(
            reply["result"]["focused_window"]["id"],
            serde_json::json!(101)
        );

        // The overview.
        let reply = server.process_message(IpcMessage::OverviewState);
        assert_eq!(
            reply["result"]["is_open"],
            serde_json::json!(false),
            "{}",
            reply
        );
        engine.write().enter_overview(&backend);
        let reply = server.process_message(IpcMessage::OverviewState);
        assert_eq!(
            reply["result"]["is_open"],
            serde_json::json!(true),
            "{}",
            reply
        );

        // The version of the daemon that answers.
        let reply = server.process_message(IpcMessage::Version);
        assert_eq!(reply["success"], true, "{}", reply);
        let version = reply["result"]["version"].as_str().expect("a version");
        assert!(
            version.contains(env!("CARGO_PKG_VERSION")),
            "the version names the crate: {}",
            version
        );
    }

    #[test]
    fn the_niri_requests_answer_with_nothing_before_the_engine_is_there() {
        let server = IpcServer::new();
        assert_eq!(
            server.process_message(IpcMessage::Workspaces)["result"]["workspaces"],
            serde_json::json!([])
        );
        assert_eq!(
            server.process_message(IpcMessage::Windows)["result"]["windows"],
            serde_json::json!([])
        );
        assert!(
            server.process_message(IpcMessage::FocusedWindow)["result"]["focused_window"].is_null()
        );
        assert_eq!(
            server.process_message(IpcMessage::OverviewState)["result"]["is_open"],
            serde_json::json!(false)
        );
    }

    #[test]
    fn the_niri_requests_only_read_the_state() {
        assert!(is_query(&IpcMessage::Workspaces));
        assert!(is_query(&IpcMessage::Windows));
        assert!(is_query(&IpcMessage::FocusedWindow));
        assert!(is_query(&IpcMessage::OverviewState));
        assert!(is_query(&IpcMessage::Version));
    }
}
