//! Low-level mouse hook for interactive move/resize grabs.
//!
//! Uses `SetWindowsHookExW(WH_MOUSE_LL, ...)` to intercept all mouse input
//! globally. When a grab is active (user is dragging a window), the hook
//! consumes mouse events and dispatches them to the grab handler.

use std::sync::{Arc, LazyLock};
use parking_lot::Mutex;
use tracing::{info, warn};
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx,
    WH_MOUSE_LL, MSLLHOOKSTRUCT, HHOOK,
};

use crate::input::grab::{MoveGrab, ResizeGrab};
use crate::utils::{Point, Rect, WindowId};
use crate::backend::BackendHandle;
use crate::layout::TilingEngine;

// WM_ mouse messages
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MOUSEWHEEL: u32 = 0x020A;

/// Global grab state — what kind of interactive operation is active
#[derive(Debug, Clone)]
pub enum GrabState {
    /// No grab active
    None,
    /// Moving a window by dragging
    Move(MoveGrab),
    /// Resizing a window by dragging an edge
    Resize(ResizeGrab),
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

    // --- WM_MOUSEWHEEL: scroll the workspace horizontally when the configured
    // modifier (Alt by default) is held. We let normal wheel events through to
    // the app under cursor by returning CallNextHookEx unmodified. The wheel
    // delta lives in the HIWORD of mouseData (signed); a positive delta means
    // the wheel rolled forward (away from user) which we map to scroll-right
    // when `natural_scroll = false`. We never intercept when no grab is active
    // AND no modifier is held to avoid breaking in-app scrolling.
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
                    let new_pos = grab.new_position(cursor);
                    let window_id = grab.window_id;
                    // Use the window's actual size captured at grab start to avoid
                    // resizing the window on every mouse-move event.
                    let grab_size = grab.initial_size;
                    let state = HOOK_STATE.lock();
                    if let Some(backend) = &state.backend {
                        let rect = Rect::new(new_pos.x, new_pos.y, grab_size.w, grab_size.h);
                        let _ = backend.set_window_position(
                            window_id.as_isize(), rect,
                            windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                                | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
                        );
                    }
                    LRESULT(1)
                }
                WM_LBUTTONUP => {
                    let window_id = grab.window_id;
                    info!("Move grab finished: window {:?}", window_id);

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
        GrabState::None => CallNextHookEx(None, n_code, w_param, l_param),
    }
}
