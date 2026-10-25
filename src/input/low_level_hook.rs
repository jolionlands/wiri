//! Low-level mouse hook for interactive move/resize grabs.
//!
//! Uses `SetWindowsHookExW(WH_MOUSE_LL, ...)` to intercept all mouse input
//! globally. When a grab is active (user is dragging a window), the hook
//! consumes mouse events and dispatches them to the grab handler.
//!
//! WH_KEYBOARD_LL is the primary bind engine on the existing hotkey thread.
//! Only bound non-modifier keys are consumed, with RegisterHotKey as a fallback.

use parking_lot::Mutex;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use tracing::{info, warn};
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_USER,
};

use crate::backend::BackendHandle;
use crate::input::grab::{ColumnReorderGrab, MoveGrab, ResizeGrab, COLUMN_REORDER_HIT_ZONE_PX};
use crate::layout::snap::{build_candidates, find_snap, SnapTarget};
use crate::layout::TilingEngine;
use crate::utils::{Point, Rect, WindowId};

// WM_ mouse messages
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MOUSEWHEEL: u32 = 0x020A;

// WM_ keyboard messages used by the primary bind hook
const WM_KEYDOWN: u32 = 0x0100;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_KEYUP: u32 = 0x0101;
const WM_SYSKEYUP: u32 = 0x0105;

const VK_ESCAPE: u32 = 0x1B;

// Key auto-repeat tracking for binds with `repeat=false`.  The keyboard hook sees every key
// down and up before the system turns a registered chord into WM_HOTKEY, so by the time
// the hotkey arrives these say whether this press was the key repeating.
const KEY_FLAG_OFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static KEY_IS_DOWN: [std::sync::atomic::AtomicBool; 256] = [KEY_FLAG_OFF; 256];
static KEY_PRESS_WAS_REPEAT: [std::sync::atomic::AtomicBool; 256] = [KEY_FLAG_OFF; 256];

/// True when the last press of the key `vk` was auto-repeat: the key was already down.
/// False when the keyboard hook is not running (every press then counts as a first press).
pub fn last_press_was_repeat(vk: u32) -> bool {
    KEY_PRESS_WAS_REPEAT[(vk & 0xFF) as usize].load(std::sync::atomic::Ordering::Relaxed)
}

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
    fn default() -> Self {
        Self::None
    }
}

/// Wrapper to make HHOOK Send-safe (it's just a pointer we pass to Win32 API)
#[derive(Clone, Copy)]
struct SendHook(HHOOK);
unsafe impl Send for SendHook {}

#[derive(Clone, Copy)]
struct HookThread {
    hook: SendHook,
    thread_id: u32,
}
unsafe impl Send for HookThread {}

const WM_STOP_HOOK_LOOP: u32 = WM_USER + 301;

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

static HOOK_HANDLE: LazyLock<Mutex<Option<HookThread>>> = LazyLock::new(|| Mutex::new(None));

// ---------------------------------------------------------------------------
// Keyboard hook — double-Alt detection
// ---------------------------------------------------------------------------

/// Handle for the installed `WH_KEYBOARD_LL` hook.
static KB_HOOK_HANDLE: LazyLock<Mutex<Option<HookThread>>> = LazyLock::new(|| Mutex::new(None));

fn run_hook_message_loop(name: &str) {
    let mut msg = MSG::default();
    loop {
        let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if ret.0 == 0 || ret.0 == -1 || msg.message == WM_STOP_HOOK_LOOP {
            break;
        }
        let _ = unsafe { TranslateMessage(&msg) };
        unsafe {
            DispatchMessageW(&msg);
        }
    }
    info!("Low-level {} hook message loop exited", name);
}

fn take_hook_for_thread(
    hook_slot: &LazyLock<Mutex<Option<HookThread>>>,
    thread_id: u32,
) -> Option<HookThread> {
    let mut guard = hook_slot.lock();
    if guard.as_ref().map(|h| h.thread_id) == Some(thread_id) {
        guard.take()
    } else {
        None
    }
}

/// Start the low-level mouse hook on a dedicated thread with a message pump.
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

    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("wiri-mouse-hook".to_string())
        .spawn(move || {
            let thread_id = unsafe { GetCurrentThreadId() };
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
                    *HOOK_HANDLE.lock() = Some(HookThread {
                        hook: SendHook(h),
                        thread_id,
                    });
                    info!("Low-level mouse hook installed");
                    let _ = ready_tx.send(Ok(()));
                    run_hook_message_loop("mouse");
                    if let Some(hook_thread) = take_hook_for_thread(&HOOK_HANDLE, thread_id) {
                        unsafe {
                            let _ = UnhookWindowsHookEx(hook_thread.hook.0);
                        }
                        info!("Low-level mouse hook removed");
                    }
                }
                Err(e) => {
                    let err = std::io::Error::last_os_error();
                    let _ = ready_tx.send(Err(format!(
                        "SetWindowsHookExW(WH_MOUSE_LL) failed: {:?} os_err={}",
                        e, err
                    )));
                }
            }
        })
        .map_err(|e| format!("failed to spawn mouse hook thread: {}", e))?;

    ready_rx
        .recv_timeout(Duration::from_secs(2))
        .map_err(|e| format!("mouse hook did not start: {}", e))?
}

/// Stop the low-level mouse hook
pub fn stop_mouse_hook() {
    if let Some(hook_thread) = HOOK_HANDLE.lock().take() {
        unsafe {
            let _ = UnhookWindowsHookEx(hook_thread.hook.0);
            let _ = PostThreadMessageW(
                hook_thread.thread_id,
                WM_STOP_HOOK_LOOP,
                WPARAM(0),
                LPARAM(0),
            );
        }
        info!("Low-level mouse hook removed");
    }
}

// ---------------------------------------------------------------------------
// Keyboard hook — double-Alt detection
// ---------------------------------------------------------------------------

/// Install WH_KEYBOARD_LL on the existing hotkey message thread. The callback only
/// evaluates a small state machine and posts actions; no new input thread is created.
pub fn start_keyboard_hook() -> Result<(), String> {
    if KB_HOOK_HANDLE.lock().is_some() { return Ok(()); }
    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_callback), windows::Win32::Foundation::HMODULE::default(), 0) }
        .map_err(|e| format!("SetWindowsHookExW(WH_KEYBOARD_LL) failed: {e}"))?;
    *KB_HOOK_HANDLE.lock() = Some(HookThread { hook: SendHook(hook), thread_id: unsafe { GetCurrentThreadId() } });
    info!("Low-level keyboard bind hook installed on hotkey thread");
    Ok(())
}

pub fn stop_keyboard_hook() {
    if let Some(h) = KB_HOOK_HANDLE.lock().take() {
        unsafe { let _ = UnhookWindowsHookEx(h.hook.0); }
    }
}

// A tagged, unassigned virtual key makes the shell see a chord rather than a lone
// Win/Alt tap, without faking a modifier up (which would break a physically held key).
const MENU_MASK_TAG: usize = 0x57495249;
fn mask_shell_menu() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY};
    let event = |flags| INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0xE8), dwFlags: flags, dwExtraInfo: MENU_MASK_TAG, ..Default::default() } } };
    let events = [event(Default::default()), event(KEYEVENTF_KEYUP)];
    unsafe { let _ = SendInput(&events, std::mem::size_of::<INPUT>() as i32); }
}

unsafe extern "system" fn keyboard_hook_callback(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code < 0 { return CallNextHookEx(None, n_code, w_param, l_param); }
    let kb = &*(l_param.0 as *const KBDLLHOOKSTRUCT);
    if kb.dwExtraInfo == MENU_MASK_TAG { return CallNextHookEx(None, n_code, w_param, l_param); }
    let msg = w_param.0 as u32;
    let pressed = match msg { WM_KEYDOWN | WM_SYSKEYDOWN => true, WM_KEYUP | WM_SYSKEYUP => false, _ => return CallNextHookEx(None,n_code,w_param,l_param) };
    let vk = kb.vkCode;
    {
        use std::sync::atomic::Ordering;
        let slot = (vk & 255) as usize;
        let old = KEY_IS_DOWN[slot].swap(pressed, Ordering::Relaxed);
        KEY_PRESS_WAS_REPEAT[slot].store(pressed && old, Ordering::Relaxed);
    }
    let decision = crate::input::keyboard::ENGINE.lock().event(vk, pressed, Instant::now());
    if decision.mask_menu { mask_shell_menu(); }
    if let Some(action) = decision.action { crate::input::dispatch::send(action); }
    // Escape remains available to applications, but also exits the manager's modes.
    if pressed && vk == VK_ESCAPE && !decision.consume { crate::input::dispatch::send(crate::input::Action::OverviewExit); }
    if decision.consume { LRESULT(1) } else { CallNextHookEx(None, n_code, w_param, l_param) }
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

/// Read modifiers from the OS as well when the keyboard hook is unavailable.
fn pointer_modifiers() -> u32 {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    [(0x12,1),(0x11,2),(0x10,4),(0x5B,8),(0x5C,8)].iter().fold(0, |mods,(vk,flag)| {
        if unsafe { GetAsyncKeyState(*vk) } as u16 & 0x8000 != 0 { mods | flag } else { mods }
    })
}

unsafe extern "system" fn mouse_hook_callback(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code < 0 { return CallNextHookEx(None,n_code,w_param,l_param); }
    let msg = w_param.0 as u32;
    let event = &*(l_param.0 as *const MSLLHOOKSTRUCT);
    let cursor = Point::new(event.pt.x,event.pt.y);
    // This mutex protects only a snapshot; no layout work holds it. Do not let
    // contention leak the release of a button whose down was consumed.
    let grab = HOOK_STATE.lock().grab.clone();
    if !matches!(grab, GrabState::None) {
        let handled = match grab { GrabState::Move(_) | GrabState::ColumnReorder(_) => matches!(msg, WM_MOUSEMOVE | WM_LBUTTONUP), GrabState::Resize(_) => matches!(msg, WM_MOUSEMOVE | WM_RBUTTONUP), GrabState::None => false };
        if handled {
            let (engine,backend) = { let s=HOOK_STATE.lock(); (s.engine.clone(),s.backend.clone()) };
            if msg != WM_MOUSEMOVE {
                HOOK_STATE.lock().grab = GrabState::None;
                crate::input::pointer::ENGINE.lock().button(0,if msg==WM_LBUTTONUP {0} else {1},false,Instant::now());
            }
            if let (Some(engine),Some(backend)) = (engine,backend) {
                crate::input::dispatch::send_work(Box::new(move || process_grab(grab,msg,cursor,&engine,&backend)));
            }
            return LRESULT(1);
        }
    }
    if msg == WM_MOUSEMOVE {
        // Focus tracking is event-driven; the integration below adds its notification.
        return CallNextHookEx(None,n_code,w_param,l_param);
    }
    let mods=pointer_modifiers();
    let button = match msg { 0x201|0x202 => Some((0,msg==0x201)), 0x204|0x205 => Some((1,msg==0x204)), 0x207|0x208 => Some((2,msg==0x207)), 0x20B|0x20C => Some((if event.mouseData >> 16 == 1 {3} else {4},msg==0x20B)), _=>None };
    let (consume,actions) = if let Some((b,down))=button {
        crate::input::pointer::ENGINE.lock().button(mods,b,down,Instant::now())
    } else if msg==WM_MOUSEWHEEL || msg==0x20E {
        crate::input::pointer::ENGINE.lock().wheel(mods,msg==0x20E,(event.mouseData >> 16) as i16 as i32,Instant::now())
    } else { return CallNextHookEx(None,n_code,w_param,l_param); };
    if consume {
        for a in actions { crate::input::dispatch::send(a); }
        if mods & (1|8) != 0 { mask_shell_menu(); }
        return LRESULT(1);
    }
    // An explicit click bind wins over the corresponding built-in gesture.
    if matches!(button,Some((0|1,true))) {
        let mod_key=crate::input::pointer::ENGINE.lock().mod_key;
        let reorder=mods==mod_key|2 && mod_key&2==0;
        if mods==mod_key || reorder {
            let child=windows::Win32::UI::WindowsAndMessaging::WindowFromPoint(POINT {x:cursor.x,y:cursor.y});
            let root=windows::Win32::UI::WindowsAndMessaging::GetAncestor(child,windows::Win32::UI::WindowsAndMessaging::GA_ROOT);
            let wid=WindowId::new(root.0 as isize);
            let (engine,backend)={ let s=HOOK_STATE.lock(); (s.engine.clone(),s.backend.clone()) };
            if let (Some(engine),Some(backend))=(engine,backend) {
                if let Some(eng)=engine.try_read() {
                    if let Some(info)=eng.tiled_windows().get(&wid) {
                        if eng.is_fullscreen(wid) { return CallNextHookEx(None,n_code,w_param,l_param); }
                        let bounds=backend.get_window(wid.as_isize()).and_then(|w| w.get_dwm_bounds()).unwrap_or(info.bounds);
                        let col=eng.monitors().values().find_map(|m| m.workspace().and_then(|w| w.find_window_column(wid))).unwrap_or(0);
                        let next=if msg==WM_LBUTTONDOWN {
                            if reorder && !eng.is_floating(wid) && cursor.y-bounds.loc.y <= COLUMN_REORDER_HIT_ZONE_PX {
                                GrabState::ColumnReorder(ColumnReorderGrab::new(wid,col,cursor))
                            } else { GrabState::Move(MoveGrab::new(wid,cursor,bounds.loc,bounds.size,col)) }
                        } else if let Some(edge)=crate::input::grab::resize_edge_or_quadrant(cursor,bounds,eng.config().border_width) {
                            GrabState::Resize(ResizeGrab::new(wid,edge,cursor,bounds))
                        } else { return CallNextHookEx(None,n_code,w_param,l_param); };
                        drop(eng); HOOK_STATE.lock().grab=next;
                        crate::input::pointer::ENGINE.lock().capture_gesture(if msg==WM_LBUTTONDOWN {0} else {1});
                        if mods & (1|8) != 0 { mask_shell_menu(); }
                        return LRESULT(1);
                    }
                }
            }
        }
    }
    CallNextHookEx(None,n_code,w_param,l_param)
}

fn preview_rect(wid: WindowId, rect: Rect, backend: &BackendHandle) {
    let _=backend.set_window_position(wid.as_isize(),rect,windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE);
}

/// Slow layout/backend work never runs inside the global low-level callback.
fn process_grab(grab: GrabState, msg: u32, cursor: Point, engine: &Arc<parking_lot::RwLock<TilingEngine>>, backend: &BackendHandle) {
    match grab {
        GrabState::Move(g) => {
            let mut pos=g.new_position(cursor);
            if msg==WM_MOUSEMOVE {
                let cfg=crate::input::snap_guide::snap_config();
                let mut overlay=None;
                if cfg.enabled {
                    let eng=engine.read();
                    if let Some(m)=eng.focused_output().and_then(|o| eng.monitors().get(&o)) {
                        let mut lefts=vec![]; let mut rights=vec![];
                        if let Some(w)=m.workspace() { for col in &w.columns {
                            if col.tiles.iter().any(|t| t.window_id==g.window_id) { continue; }
                            for t in &col.tiles { if let Some(i)=eng.tiled_windows().get(&t.window_id) { lefts.push(i.bounds.loc.x); rights.push(i.bounds.right()); } }
                        } }
                        let wa=m.work_area; let candidates=build_candidates(&lefts,&rights,wa.loc.x,wa.size.w as i32);
                        if let Some(SnapTarget {x,..})=find_snap(pos.x,&candidates,cfg.threshold_px) { pos.x=x; overlay=Some((x,wa.loc.y,wa.size.h as i32)); }
                    }
                }
                preview_rect(g.window_id,Rect::new(pos.x,pos.y,g.initial_size.w,g.initial_size.h),backend);
                if crate::input::snap_guide::note_snap_x(overlay.map(|(x,_,_)| x)) {
                    match overlay { Some((x,y,h))=>crate::input::snap_guide::guide().show_at(x,y,h), None=>crate::input::snap_guide::guide().hide() }
                }
            } else {
                crate::input::snap_guide::note_snap_x(None);
                // Do not create the guide just to hide it on a drag with no snap.
                crate::input::snap_guide::hide_if_created();
                let mut eng=engine.write();
                if eng.is_floating(g.window_id) {
                    eng.place_float_from_pointer(g.window_id,Rect::new(pos.x,pos.y,g.initial_size.w,g.initial_size.h),backend);
                } else {
                    let oid=eng.monitors().iter().find_map(|(o,m)| m.workspace().and_then(|w| w.find_window_column(g.window_id)).map(|_| *o));
                    if let Some(o)=oid {
                        let gap=eng.config().column_gap;
                        if let Some(w)=eng.monitors_mut().get_mut(&o).and_then(|m| m.workspace_mut()) {
                            let target=g.target_column(cursor,g.initial_size.w as i32,gap,w.columns.len().max(1));
                            let _=w.move_window(g.window_id,target,0);
                        }
                        eng.apply_layout_for_monitor(o,backend);
                    }
                }
                crate::ipc::notify_state_changed();
            }
        }
        GrabState::Resize(g) => {
            let rect=g.new_rect(cursor,(100,100));
            if msg==WM_MOUSEMOVE { preview_rect(g.window_id,rect,backend); }
            else {
                let mut eng=engine.write();
                if eng.is_floating(g.window_id) {
                    eng.place_float_from_pointer(g.window_id,rect,backend);
                } else {
                    let target=eng.monitors().iter().find_map(|(o,m)| m.workspace().and_then(|w| w.find_window_column(g.window_id)).map(|c| (*o,c)));
                    if let Some((o,c))=target {
                        eng.set_focused_output(o);
                        if let Some(m)=eng.monitors_mut().get_mut(&o) { m.focus_column=Some(c); m.focus_window=Some(g.window_id); }
                        // Use the engine's existing setters: the width/height becomes layout
                        // state (and observes its limits), not a transient SetWindowPos.
                        eng.set_column_width_by(crate::input::SizeChange::SetFixed(rect.size.w as i32),backend);
                        eng.set_window_height_by(crate::input::SizeChange::SetFixed(rect.size.h as i32),backend);
                    }
                }
                crate::ipc::notify_state_changed();
            }
        }
        GrabState::ColumnReorder(g) => if msg==WM_LBUTTONUP {
            let mut eng=engine.write();
            if let Some(dst)=eng.column_at_x(cursor.x) { eng.swap_columns(g.source_col,dst,backend); }
            crate::ipc::notify_state_changed();
        },
        GrabState::None => {}
    }
}
