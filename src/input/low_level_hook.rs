//! Low-level mouse hook for interactive move/resize grabs.
//!
//! Uses `SetWindowsHookExW(WH_MOUSE_LL, ...)` to intercept all mouse input
//! globally. When a grab is active (user is dragging a window), the hook
//! consumes mouse events and dispatches them to the grab handler.
//!
//! ## Double-Alt keyboard hook
//!
//! A companion `WH_KEYBOARD_LL` hook is installed by `start_keyboard_hook` /
//! stopped by `stop_keyboard_hook`.  It tracks consecutive Alt key-down
//! events: when two Alt presses arrive within `DOUBLE_TAP_WINDOW_MS` (250 ms)
//! with no intervening non-Alt key, `Action::OverviewToggle` is fired via the
//! action-sender channel wired by `set_keyboard_action_sender`.
//!
//! The hook is a **pass-through**: it calls `CallNextHookEx` for every event
//! and only consumes the very last Alt-down that completes a double-tap
//! (by returning `LRESULT(1)` for that single event).

use std::sync::{Arc, LazyLock};
use std::time::Instant;
use parking_lot::Mutex;
use tracing::{info, warn};
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx,
    WH_MOUSE_LL, WH_KEYBOARD_LL, MSLLHOOKSTRUCT, KBDLLHOOKSTRUCT, HHOOK,
};

use crate::input::grab::{MoveGrab, ResizeGrab, ColumnReorderGrab, COLUMN_REORDER_HIT_ZONE_PX};
use crate::utils::{Point, Rect, WindowId};
use crate::backend::BackendHandle;
use crate::layout::TilingEngine;
use crate::layout::snap::{build_candidates, find_snap, SnapTarget};

// WM_ mouse messages
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MOUSEWHEEL: u32 = 0x020A;

// WM_ keyboard messages used by the double-Alt hook
const WM_KEYDOWN: u32 = 0x0100;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_KEYUP: u32 = 0x0101;
const WM_SYSKEYUP: u32 = 0x0105;

/// Virtual key code for the Alt key (either left or right Alt).
const VK_MENU: u32 = 0x12;

/// Maximum gap between two Alt key-down events that counts as a double-tap (ms).
const DOUBLE_TAP_WINDOW_MS: u128 = 250;

/// Global grab state — what kind of interactive operation is active
#[derive(Debug, Clone)]
pub enum GrabState {
    /// No grab active
    None,
    /// Moving a window by dragging
    Move(MoveGrab),
    /// Resizing a window by dragging an edge
    Resize(ResizeGrab),
    /// Dragging a column to reorder it (Ctrl+Alt+Left-click on the top strip
    /// of a tiled window).  On mouse-up the source and destination columns are
    /// swapped via `TilingEngine::swap_columns`.
    ColumnReorder(ColumnReorderGrab),
}

impl Default for GrabState {
    fn default() -> Self { Self::None }
}

/// Wrapper to make HHOOK Send-safe (it's just a pointer we pass to Win32 API)
struct SendHook(HHOOK);
unsafe impl Send for SendHook {}

/// Shared state accessible from the mouse hook callback
struct HookState {
    grab: GrabState,
    engine: Option<Arc<parking_lot::RwLock<TilingEngine>>>,
    backend: Option<BackendHandle>,
}

static HOOK_STATE: LazyLock<Mutex<HookState>> = LazyLock::new(|| {
    Mutex::new(HookState {
        grab: GrabState::None,
        engine: None,
        backend: None,
    })
});

static HOOK_HANDLE: LazyLock<Mutex<Option<SendHook>>> = LazyLock::new(|| {
    Mutex::new(None)
});

// ---------------------------------------------------------------------------
// Keyboard hook — double-Alt detection
// ---------------------------------------------------------------------------

/// Handle for the installed `WH_KEYBOARD_LL` hook.
static KB_HOOK_HANDLE: LazyLock<Mutex<Option<SendHook>>> = LazyLock::new(|| {
    Mutex::new(None)
});

/// Timestamp of the last Alt key-down event.  `None` means the chain was
/// broken (another key was pressed) or no Alt has been seen yet.
static LAST_ALT_DOWN: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| {
    Mutex::new(None)
});

/// Whether the previous Alt-down is still being held (haven't seen an Alt-up yet).
/// Used to distinguish a held Alt from a fresh second tap.
static ALT_HELD: LazyLock<Mutex<bool>> = LazyLock::new(|| Mutex::new(false));

/// Optional sender for `Action` values produced by the keyboard hook.
/// Wired by `set_keyboard_action_sender`.
static KB_ACTION_TX: LazyLock<Mutex<Option<std::sync::mpsc::Sender<crate::input::Action>>>> =
    LazyLock::new(|| Mutex::new(None));

/// Start the low-level mouse hook. Must be called from a thread with a message pump.
/// Guard against double-install: if the hook is already active, logs a warning and returns.
pub fn start_mouse_hook(
    engine: Arc<parking_lot::RwLock<TilingEngine>>,
    backend: BackendHandle,
) -> Result<(), String> {
    // Double-install guard: prevent leaking the previous HHOOK (e.g. on config reload).
    if HOOK_HANDLE.lock().is_some() {
        warn!("mouse hook already installed; skipping duplicate SetWindowsHookExW");
        return Ok(());
    }

    {
        let mut state = HOOK_STATE.lock();
        state.engine = Some(engine);
        state.backend = Some(backend);
    }

    let hook = unsafe {
        SetWindowsHookExW(
            WH_MOUSE_LL,
            Some(mouse_hook_callback),
            windows::Win32::Foundation::HMODULE::default(),
            0,
        )
    };

    match hook {
        Ok(h) => {
            *HOOK_HANDLE.lock() = Some(SendHook(h));
            info!("Low-level mouse hook installed");
            Ok(())
        }
        Err(e) => {
            let err = std::io::Error::last_os_error();
            Err(format!("SetWindowsHookExW(WH_MOUSE_LL) failed: {:?} os_err={}", e, err))
        }
    }
}

/// Stop the low-level mouse hook
pub fn stop_mouse_hook() {
    if let Some(h) = HOOK_HANDLE.lock().take() {
        unsafe { let _ = UnhookWindowsHookEx(h.0); }
        info!("Low-level mouse hook removed");
    }
}

// ---------------------------------------------------------------------------
// Keyboard hook — double-Alt detection
// ---------------------------------------------------------------------------

/// Wire an `Action` sender so the keyboard hook can fire `OverviewToggle`
/// when a double-Alt is detected.  Must be called before `start_keyboard_hook`.
pub fn set_keyboard_action_sender(tx: std::sync::mpsc::Sender<crate::input::Action>) {
    *KB_ACTION_TX.lock() = Some(tx);
}

/// Install the low-level keyboard hook for double-Alt detection.
///
/// Must be called from a thread that runs a Win32 message loop (the hook
/// callback is driven by `GetMessage` / `DispatchMessage`).  Safe to call
/// again after `stop_keyboard_hook` — a fresh hook is installed.  Calling
/// while already installed logs a warning and returns without re-installing.
pub fn start_keyboard_hook() -> Result<(), String> {
    if KB_HOOK_HANDLE.lock().is_some() {
        warn!("keyboard hook already installed; skipping duplicate SetWindowsHookExW");
        return Ok(());
    }

    let hook = unsafe {
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard_hook_callback),
            windows::Win32::Foundation::HMODULE::default(),
            0,
        )
    };

    match hook {
        Ok(h) => {
            *KB_HOOK_HANDLE.lock() = Some(SendHook(h));
            info!("Low-level keyboard hook installed (double-Alt detection)");
            Ok(())
        }
        Err(e) => {
            Err(format!("SetWindowsHookExW(WH_KEYBOARD_LL) failed: {:?}", e))
        }
    }
}

/// Uninstall the low-level keyboard hook.
pub fn stop_keyboard_hook() {
    if let Some(h) = KB_HOOK_HANDLE.lock().take() {
        unsafe { let _ = UnhookWindowsHookEx(h.0); }
        info!("Low-level keyboard hook removed");
    }
}

/// Low-level keyboard hook callback.
///
/// Pass-through for all keys except Alt.  On Alt-down:
/// - If `LAST_ALT_DOWN` is set, `ALT_HELD` is false, and elapsed < 250 ms →
///   double-tap detected: fire `Action::OverviewToggle`, clear the timestamp,
///   and consume the event (`LRESULT(1)`).
/// - Otherwise, record the timestamp and pass through.
///
/// On any non-Alt key-down: clear `LAST_ALT_DOWN` (breaks the chain).
unsafe extern "system" fn keyboard_hook_callback(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code < 0 {
        return CallNextHookEx(None, n_code, w_param, l_param);
    }

    let msg = w_param.0 as u32;
    let kb = &*(l_param.0 as *const KBDLLHOOKSTRUCT);
    let vk = kb.vkCode;

    let is_key_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let is_key_up   = msg == WM_KEYUP   || msg == WM_SYSKEYUP;

    if is_key_down {
        if vk == VK_MENU {
            // Alt key-down event.
            let mut last = LAST_ALT_DOWN.lock();
            let mut held = ALT_HELD.lock();

            if let Some(prev) = *last {
                // There was a previous Alt-down.  Check that:
                //   1. The key was released between the two presses (!held).
                //   2. The gap is within the double-tap window.
                if !*held && prev.elapsed().as_millis() < DOUBLE_TAP_WINDOW_MS {
                    // Double-tap confirmed.
                    *last = None;
                    *held = false;
                    drop(last);
                    drop(held);
                    // Fire OverviewToggle.
                    if let Some(tx) = KB_ACTION_TX.lock().as_ref() {
                        let _ = tx.send(crate::input::Action::OverviewToggle);
                    }
                    // Consume this Alt-down so the system doesn't act on it.
                    return LRESULT(1);
                } else {
                    // Chain broken or gap too large — start fresh.
                    *last = Some(Instant::now());
                    *held = true;
                }
            } else {
                // First Alt-down.
                *last = Some(Instant::now());
                *held = true;
            }
        } else {
            // Any non-Alt key-down breaks the double-tap chain.
            *LAST_ALT_DOWN.lock() = None;
            *ALT_HELD.lock() = false;
        }
    } else if is_key_up && vk == VK_MENU {
        // Alt released — mark as not held so the next Alt-down can complete a double-tap.
        *ALT_HELD.lock() = false;
    }

    CallNextHookEx(None, n_code, w_param, l_param)
}

/// Start a move grab (called when Alt+LButton is pressed on a tiled window)
pub fn begin_move_grab(grab: MoveGrab) {
    info!("Starting move grab for window {:?}", grab.window_id);
    HOOK_STATE.lock().grab = GrabState::Move(grab);
}

/// Start a resize grab (called when Alt+RButton is pressed on a window edge)
pub fn begin_resize_grab(grab: ResizeGrab) {
    info!("Starting resize grab for window {:?}", grab.window_id);
    HOOK_STATE.lock().grab = GrabState::Resize(grab);
}

/// Start a column-reorder grab (called when Ctrl+Alt+LButton is pressed on
/// the top strip of a tiled window).
pub fn begin_column_reorder_grab(grab: ColumnReorderGrab) {
    info!(
        "Starting column-reorder grab for window {:?} (source col {})",
        grab.window_id, grab.source_col
    );
    HOOK_STATE.lock().grab = GrabState::ColumnReorder(grab);
}

/// Cancel any active grab
pub fn cancel_grab() {
    let mut state = HOOK_STATE.lock();
    if !matches!(state.grab, GrabState::None) {
        info!("Cancelling active grab");
        state.grab = GrabState::None;
    }
}

/// Check if a grab is currently active
pub fn is_grab_active() -> bool {
    let state = HOOK_STATE.lock();
    matches!(state.grab, GrabState::None) == false
}

/// Get a copy of the current grab state
pub fn current_grab() -> GrabState {
    HOOK_STATE.lock().grab.clone()
}

unsafe extern "system" fn mouse_hook_callback(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code < 0 {
        return CallNextHookEx(None, n_code, w_param, l_param);
    }

    let msg = w_param.0 as u32;

    // FAST PATH: filter out events we never care about.
    // WM_MOUSEMOVE is the vast majority of events (1000+/sec).
    // When no grab is active, skip it immediately with try_lock
    // to avoid mutex contention that causes mouse lag.
    if msg == WM_MOUSEMOVE {
        if let Some(state) = HOOK_STATE.try_lock() {
            if matches!(state.grab, GrabState::None) {
                return CallNextHookEx(None, n_code, w_param, l_param);
            }
        } else {
            // Couldn't get lock — don't block the mouse, pass through
            return CallNextHookEx(None, n_code, w_param, l_param);
        }
    }

    // Skip events we don't handle at all
    if msg != WM_LBUTTONDOWN && msg != WM_RBUTTONDOWN
        && msg != WM_LBUTTONUP && msg != WM_RBUTTONUP
        && msg != WM_MOUSEMOVE && msg != WM_MOUSEWHEEL
    {
        return CallNextHookEx(None, n_code, w_param, l_param);
    }

    let hook_struct = &*(l_param.0 as *const MSLLHOOKSTRUCT);
    let cursor = Point::new(hook_struct.pt.x, hook_struct.pt.y);

    // --- WM_MOUSEWHEEL: two modifier-gated behaviours:
    //
    //   Ctrl+Alt held → workspace navigation (round-4 parity).
    //     Wheel forward (positive delta) = FocusWorkspacePrevious.
    //     Wheel backward (negative delta) = FocusWorkspaceNext.
    //     TODO: gate behind MouseFocusConfig.wheel_workspace_nav (default true).
    //
    //   Alt-only held → horizontal column scroll (existing behaviour).
    //     Respects natural_scroll and scroll_speed from input config.
    //
    //   No modifier held → fall through to the app under the cursor.
    if msg == WM_MOUSEWHEEL {
        let alt_held: bool = unsafe {
            let vk: i16 = windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(
                windows::Win32::UI::Input::KeyboardAndMouse::VK_MENU.0 as i32,
            );
            (vk as u16 & 0x8000) != 0
        };
        if !alt_held {
            return CallNextHookEx(None, n_code, w_param, l_param);
        }

        // mouseData high-word = signed wheel delta (multiples of WHEEL_DELTA=120)
        let raw = hook_struct.mouseData as i32;
        let delta = (raw >> 16) as i16 as i32;
        if delta == 0 {
            return CallNextHookEx(None, n_code, w_param, l_param);
        }

        // Check if Ctrl is ALSO held → workspace navigation.
        let ctrl_held: bool = unsafe {
            let vk: i16 = windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(
                windows::Win32::UI::Input::KeyboardAndMouse::VK_CONTROL.0 as i32,
            );
            (vk as u16 & 0x8000) != 0
        };

        if ctrl_held {
            // Ctrl+Alt+Wheel → switch workspace.
            // Positive (forward) = previous workspace; negative (back) = next.
            // TODO: gate on MouseFocusConfig.wheel_workspace_nav (default true).
            let state = HOOK_STATE.lock();
            if let (Some(engine), Some(backend)) = (&state.engine, &state.backend) {
                let engine = engine.clone();
                let backend = backend.clone();
                drop(state);
                if delta > 0 {
                    engine.write().focus_workspace_relative(
                        crate::layout::WorkspaceDirection::Previous,
                        &backend,
                    );
                } else {
                    engine.write().focus_workspace_relative(
                        crate::layout::WorkspaceDirection::Next,
                        &backend,
                    );
                }
                engine.write().apply_all(&backend);
            }
            // Consume the event — do not forward to the app under the cursor.
            return LRESULT(1);
        }

        // Alt-only held → horizontal column scroll (existing behaviour).
        let mouse_cfg = crate::input::mouse_runtime_config();
        // Invert direction when natural_scroll is enabled (touchpad convention).
        let effective_delta = if mouse_cfg.natural_scroll { -delta } else { delta };
        let direction = if effective_delta > 0 {
            crate::layout::ScrollDirection::Right
        } else {
            crate::layout::ScrollDirection::Left
        };
        // Treat scroll_speed as a multiplier: ≥ 1.0 → at least one step per click;
        // < 1.0 → quantize to one step per N clicks via a static accumulator.
        let steps: u32 = {
            let raw_steps = (mouse_cfg.scroll_speed.max(0.1) as f64).round() as u32;
            raw_steps.max(1).min(8)
        };
        let state = HOOK_STATE.lock();
        if let (Some(engine), Some(backend)) = (&state.engine, &state.backend) {
            let engine = engine.clone();
            let backend = backend.clone();
            drop(state);
            for _ in 0..steps {
                engine.write().scroll(direction, &backend);
            }
        }
        // Swallow the event so the app under the cursor doesn't also scroll.
        return LRESULT(1);
    }

    // --- Check if a grab is active ---
    let grab_is_none = {
        let state = HOOK_STATE.lock();
        matches!(state.grab, GrabState::None)
    };

    // --- No grab active: check for grab-initiating gestures ---
    if grab_is_none {
        if msg == WM_LBUTTONDOWN || msg == WM_RBUTTONDOWN {
            let alt_held: bool = unsafe {
                let vk: i16 = windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(
                    windows::Win32::UI::Input::KeyboardAndMouse::VK_MENU.0 as i32,
                );
                (vk as u16 & 0x8000) != 0
            };

            if alt_held {
                // Check whether Ctrl is also held — used for column-reorder mode.
                let ctrl_held: bool = unsafe {
                    let vk: i16 = windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(
                        windows::Win32::UI::Input::KeyboardAndMouse::VK_CONTROL.0 as i32,
                    );
                    (vk as u16 & 0x8000) != 0
                };

                let hwnd_at_cursor = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::WindowFromPoint(
                        POINT { x: cursor.x, y: cursor.y },
                    )
                };

                if !hwnd_at_cursor.is_invalid() {
                    let wid = WindowId::new(hwnd_at_cursor.0 as isize);
                    let state = HOOK_STATE.lock();

                    if let Some(engine) = &state.engine {
                        let eng = engine.read();
                        if eng.tiled_windows().contains_key(&wid) && !eng.is_floating(wid) {
                            if msg == WM_LBUTTONDOWN {
                                if let Some(info) = eng.tiled_windows().get(&wid) {
                                    let pos = info.bounds.loc;
                                    let size = info.bounds.size;
                                    let col_idx = eng.monitors().values()
                                        .find_map(|m| m.workspace()
                                            .and_then(|w| w.find_window_column(wid)))
                                        .unwrap_or(0);

                                    // Ctrl+Alt+LButton on the top strip of the tile →
                                    // column-reorder mode.  The hit-zone is the top
                                    // COLUMN_REORDER_HIT_ZONE_PX pixels of the tile.
                                    if ctrl_held
                                        && cursor.y >= pos.y
                                        && cursor.y <= pos.y + COLUMN_REORDER_HIT_ZONE_PX
                                    {
                                        let grab = ColumnReorderGrab::new(wid, col_idx, cursor);
                                        drop(eng);
                                        drop(state);
                                        begin_column_reorder_grab(grab);
                                        return LRESULT(1);
                                    }

                                    let grab = MoveGrab::new(wid, cursor, pos, size, col_idx);
                                    drop(eng);
                                    drop(state);
                                    begin_move_grab(grab);
                                    return LRESULT(1);
                                }
                            } else if msg == WM_RBUTTONDOWN {
                                if let Some(info) = eng.tiled_windows().get(&wid) {
                                    let border_w = eng.config().border_width.max(4);
                                    if let Some(edge) = crate::input::grab::resize_edge_from_point(
                                        cursor, info.bounds, border_w
                                    ) {
                                        let grab = ResizeGrab::new(wid, edge, cursor, info.bounds);
                                        drop(eng);
                                        drop(state);
                                        begin_resize_grab(grab);
                                        return LRESULT(1);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        return CallNextHookEx(None, n_code, w_param, l_param);
    }

    // --- Grab is active: process grab events ---
    let grab_state = HOOK_STATE.lock().grab.clone();

    match &grab_state {
        GrabState::Move(grab) => {
            match msg {
                WM_MOUSEMOVE => {
                    let mut new_pos = grab.new_position(cursor);
                    let window_id = grab.window_id;
                    // Use the window's actual size captured at grab start to avoid
                    // resizing the window on every mouse-move event.
                    let grab_size = grab.initial_size;

                    // ---- Snap-on-drag (niri parity) ----
                    // Build the snap-candidate list (column edges + mid-screen
                    // + monitor edges) and, when the cursor's projected X is
                    // within `snap-threshold-px`, lock onto the nearest
                    // candidate.  Disabled with snap-on-drag false.
                    let snap_cfg = crate::input::snap_guide::snap_config();
                    let mut snap_overlay: Option<(i32, i32, i32)> = None;
                    if snap_cfg.enabled {
                        let state = HOOK_STATE.lock();
                        if let Some(engine) = &state.engine {
                            let eng = engine.read();
                            if let Some(oid) = eng.focused_output() {
                                if let Some(monitor) = eng.monitors().get(&oid) {
                                    let mut lefts: Vec<i32> = Vec::new();
                                    let mut rights: Vec<i32> = Vec::new();
                                    if let Some(workspace) = monitor.workspace() {
                                        for col in &workspace.columns {
                                            // Exclude the dragged window's own
                                            // column so we don't snap to the
                                            // starting position.
                                            let is_self = col
                                                .tiles
                                                .iter()
                                                .any(|t| t.window_id == window_id);
                                            if is_self {
                                                continue;
                                            }
                                            for tile in &col.tiles {
                                                if let Some(info) =
                                                    eng.tiled_windows().get(&tile.window_id)
                                                {
                                                    lefts.push(info.bounds.loc.x);
                                                    rights
                                                        .push(info.bounds.right());
                                                }
                                            }
                                        }
                                    }
                                    let wa = monitor.work_area;
                                    let cands = build_candidates(
                                        &lefts,
                                        &rights,
                                        wa.loc.x,
                                        wa.size.w as i32,
                                    );
                                    if let Some(SnapTarget { x, .. }) = find_snap(
                                        new_pos.x,
                                        &cands,
                                        snap_cfg.threshold_px,
                                    ) {
                                        new_pos.x = x;
                                        snap_overlay =
                                            Some((x, wa.loc.y, wa.size.h as i32));
                                    }
                                }
                            }
                        }
                    }

                    let state = HOOK_STATE.lock();
                    if let Some(backend) = &state.backend {
                        let rect = Rect::new(new_pos.x, new_pos.y, grab_size.w, grab_size.h);
                        let _ = backend.set_window_position(
                            window_id.as_isize(), rect,
                            windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                                | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
                        );
                    }
                    drop(state);

                    // Reposition or hide the snap-guide overlay.  De-dup so
                    // we only thread-cross when the snap X changes.
                    let new_snap_x = snap_overlay.map(|(x, _, _)| x);
                    if crate::input::snap_guide::note_snap_x(new_snap_x) {
                        let g = crate::input::snap_guide::guide();
                        match snap_overlay {
                            Some((x, top, height)) => g.show_at(x, top, height),
                            None => g.hide(),
                        }
                    }

                    LRESULT(1)
                }
                WM_LBUTTONUP => {
                    let window_id = grab.window_id;
                    info!("Move grab finished: window {:?}", window_id);

                    // Tear the snap guide down so it never lingers after
                    // release.  Always force-hide — cheap if already hidden.
                    let _ = crate::input::snap_guide::note_snap_x(None);
                    crate::input::snap_guide::guide().hide();

                    let state = HOOK_STATE.lock();
                    if let Some(engine) = &state.engine {
                        let mut eng = engine.write();
                        if let Some(oid) = eng.focused_output() {
                            // Extract config values before mutable borrow
                            let col_width = eng.config().column_width as i32;
                            let col_gap = eng.config().column_gap;
                            if let Some(monitor) = eng.monitors_mut().get_mut(&oid) {
                                if let Some(workspace) = monitor.workspace_mut() {
                                    let num_cols = workspace.columns.len().max(1);
                                    let target_col = grab.target_column(
                                        cursor,
                                        col_width,
                                        col_gap,
                                        num_cols,
                                    );
                                    let _ = workspace.move_window(window_id, target_col, 0);
                                }
                            }
                            if let Some(backend) = &state.backend {
                                eng.apply_layout_for_monitor(oid, backend);
                            }
                        }
                    }
                    drop(state);
                    HOOK_STATE.lock().grab = GrabState::None;
                    LRESULT(1)
                }
                _ => CallNextHookEx(None, n_code, w_param, l_param),
            }
        }
        GrabState::Resize(grab) => {
            match msg {
                WM_MOUSEMOVE => {
                    let new_rect = grab.new_rect(cursor, (100, 100));
                    let window_id = grab.window_id;
                    let state = HOOK_STATE.lock();
                    if let Some(backend) = &state.backend {
                        let _ = backend.set_window_position(
                            window_id.as_isize(), new_rect,
                            windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                                | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
                        );
                    }
                    LRESULT(1)
                }
                WM_RBUTTONUP => {
                    info!("Resize grab finished: window {:?}", grab.window_id);
                    let state = HOOK_STATE.lock();
                    if let Some(engine) = &state.engine {
                        let mut eng = engine.write();
                        if let Some(backend) = &state.backend {
                            eng.apply_all(backend);
                        }
                    }
                    drop(state);
                    HOOK_STATE.lock().grab = GrabState::None;
                    LRESULT(1)
                }
                _ => CallNextHookEx(None, n_code, w_param, l_param),
            }
        }
        GrabState::ColumnReorder(grab) => {
            match msg {
                WM_MOUSEMOVE => {
                    // No visual feedback in this minimal implementation — the
                    // snap-guide overlay from MoveGrab covers the reorder case
                    // adequately. Pass through so the cursor remains responsive.
                    CallNextHookEx(None, n_code, w_param, l_param)
                }
                WM_LBUTTONUP => {
                    let src = grab.source_col;
                    info!(
                        "Column-reorder grab finished: source col {} cursor {:?}",
                        src, cursor
                    );

                    // Extract engine + backend references before clearing the grab
                    // so we don't need to re-lock HOOK_STATE after the clear.
                    let (maybe_engine, maybe_backend) = {
                        let state = HOOK_STATE.lock();
                        (state.engine.clone(), state.backend.clone())
                    };
                    HOOK_STATE.lock().grab = GrabState::None;

                    // Find which column the cursor is over and swap.
                    if let (Some(engine), Some(backend)) = (maybe_engine, maybe_backend) {
                        let dst = engine.read().column_at_x(cursor.x);
                        if let Some(dst_col) = dst {
                            engine.write().swap_columns(src, dst_col, &backend);
                        }
                    }
                    LRESULT(1)
                }
                _ => CallNextHookEx(None, n_code, w_param, l_param),
            }
        }
        GrabState::None => CallNextHookEx(None, n_code, w_param, l_param),
    }
}
