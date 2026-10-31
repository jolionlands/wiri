//! Explicit `focus-window` converges the live engine/IPC focus on the native foreground
//! (the aa6c884 h15 evidence: Windows accepted `SetForegroundWindow` for k1, raised no
//! EVENT_SYSTEM_FOREGROUND, and managed and stream focus stayed on k3 for good).
//!
//! Model HWNDs are negative, so no real window is ever touched; the native foreground is
//! the simulation of `backend::activation_attempt_test_support`.

use super::{IpcEvent, IpcMessage, IpcServer};
use crate::backend::activation_attempt_test_support::{with_native_foreground, Activation};
use crate::backend::{BackendHandle, ForegroundStamp, WindowInfo, WindowState};
use crate::layout::{LayoutConfig, TilingEngine};
use crate::utils::{OutputId, Rect, WindowId};
use std::sync::Arc;
use tokio::sync::broadcast;

const K1: isize = -9401;
const K2: isize = -9402;
const K3: isize = -9403;

type SharedEngine = Arc<parking_lot::RwLock<TilingEngine>>;

/// Three tiles; the live focus mirrors the native foreground K3 as the hook would have.
fn fixture() -> (IpcServer, SharedEngine, BackendHandle) {
    let backend = BackendHandle::default_for_test();
    let mut engine = TilingEngine::new(LayoutConfig::default());
    engine.register_monitor(
        OutputId::from_name("FocusConvergeMonitor"),
        Rect::new(0, 0, 1920, 1080),
        Rect::new(0, 0, 1920, 1040),
    );
    for hwnd in [K1, K2, K3] {
        engine.add_window(
            WindowInfo {
                hwnd,
                title: format!("k{hwnd}"),
                class_name: "FocusConvergeModel".into(),
                process_id: 1,
                bounds: Rect::new(0, 0, 800, 600),
                state: WindowState::Normal,
                is_visible: true,
            },
            &backend,
        );
    }
    engine.sync_os_foreground(WindowId::new(K3), &backend);
    let engine = Arc::new(parking_lot::RwLock::new(engine));
    let mut server = IpcServer::new();
    server.set_engine(engine.clone());
    server.set_backend(backend.clone());
    (server, engine, backend)
}

fn drain(receiver: &mut broadcast::Receiver<IpcEvent>) -> Vec<IpcEvent> {
    let mut events = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        events.push(event);
    }
    events
}

fn focused(engine: &SharedEngine) -> Option<u64> {
    engine.read().ipc_snapshot().focused_window()
}

fn count(events: &[IpcEvent], wanted: impl Fn(&IpcEvent) -> bool) -> usize {
    events.iter().filter(|event| wanted(event)).count()
}

fn focus_window(server: &IpcServer, hwnd: isize) -> serde_json::Value {
    server.process_message(IpcMessage::FocusWindow { window_hwnd: hwnd })
}

#[test]
fn focus_window_on_a_silently_switched_foreground_converges_and_is_told_once() {
    let (server, engine, backend) = fixture();
    let (mut events, _replay) = server.subscribe_with_replay();
    assert_eq!(focused(&engine), Some(K3 as u64));

    let (reply, attempts) = with_native_foreground(K3, Activation::SwitchWithoutNotice, || {
        focus_window(&server, K1)
    });
    assert_eq!(reply["success"], true, "{reply}");
    assert_eq!(attempts, vec![K1]);
    // The state the stream and `focused_window` describe follows the native query.
    assert_eq!(focused(&engine), Some(K1 as u64));
    server.sync_state();
    let told = drain(&mut events);
    assert_eq!(
        count(
            &told,
            |e| matches!(e, IpcEvent::WindowFocusChanged { id: Some(id) } if *id == K1 as u64)
        ),
        1,
        "{told:?}"
    );
    assert_eq!(
        count(
            &told,
            |e| matches!(e, IpcEvent::WindowFocused { window_hwnd } if *window_hwnd == K1)
        ),
        1,
        "{told:?}"
    );

    // Whenever the native notice does arrive it is the same fact: state only, no second tell.
    let (live, _) = engine.write().observe_native_foreground(
        K1,
        K1,
        ForegroundStamp {
            uptime_ms: u64::MAX / 2,
            sequence: 1,
        },
        true,
        &backend,
    );
    assert!(!live);
    server.sync_state();
    let late = drain(&mut events);
    assert_eq!(
        count(&late, |e| matches!(e, IpcEvent::WindowFocused { .. })),
        0,
        "{late:?}"
    );
    assert_eq!(focused(&engine), Some(K1 as u64));
}

#[test]
fn focus_window_on_the_window_that_already_is_native_foreground_converges() {
    // The harness's retry: the target is already foreground, so Windows is asked for
    // nothing and raises nothing, yet managed focus still named the previous window.
    let (server, engine, _backend) = fixture();
    let (mut events, _replay) = server.subscribe_with_replay();
    let (reply, attempts) =
        with_native_foreground(K1, Activation::Refuse, || focus_window(&server, K1));
    assert_eq!(reply["success"], true, "{reply}");
    assert!(attempts.is_empty());
    assert_eq!(focused(&engine), Some(K1 as u64));
    server.sync_state();
    let told = drain(&mut events);
    assert_eq!(
        count(
            &told,
            |e| matches!(e, IpcEvent::WindowFocused { window_hwnd } if *window_hwnd == K1)
        ),
        1,
        "{told:?}"
    );
    // The same request again changes and tells nothing.
    let (_, attempts) =
        with_native_foreground(K1, Activation::Refuse, || focus_window(&server, K1));
    assert!(attempts.is_empty());
    server.sync_state();
    let again = drain(&mut events);
    assert_eq!(
        count(&again, |e| matches!(
            e,
            IpcEvent::WindowFocused { .. } | IpcEvent::WindowFocusChanged { .. }
        )),
        0,
        "{again:?}"
    );
}

#[test]
fn a_refused_focus_window_moves_neither_managed_nor_stream_focus() {
    let (server, engine, _backend) = fixture();
    let (mut events, _replay) = server.subscribe_with_replay();
    let (reply, attempts) =
        with_native_foreground(K3, Activation::Refuse, || focus_window(&server, K1));
    assert_eq!(reply["success"], true, "{reply}");
    assert!(!attempts.is_empty() && attempts.iter().all(|hwnd| *hwnd == K1));
    assert_eq!(focused(&engine), Some(K3 as u64));
    server.sync_state();
    let told = drain(&mut events);
    assert_eq!(
        count(&told, |e| matches!(
            e,
            IpcEvent::WindowFocused { .. } | IpcEvent::WindowFocusChanged { .. }
        )),
        0,
        "{told:?}"
    );
}

#[test]
fn run_action_focus_window_with_id_converges_like_the_typed_request() {
    let (server, engine, _backend) = fixture();
    let (reply, _) = with_native_foreground(K3, Activation::SwitchWithoutNotice, || {
        server.process_message(IpcMessage::RunAction {
            name: "focus-window".into(),
            args: vec!["--id".into(), K2.to_string()],
            quoted: Vec::new(),
        })
    });
    assert_eq!(reply["success"], true, "{reply}");
    assert_eq!(focused(&engine), Some(K2 as u64));
}
