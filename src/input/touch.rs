//! Touch / pointer input scaffolding for wiri.
//!
//! Listens to WM_POINTERDOWN/WM_POINTERUP/WM_POINTERUPDATE (or WM_TOUCH) and
//! recognizes basic gestures: tap, swipe-left, swipe-right, swipe-up, swipe-down.
//! Two-finger gestures (pinch in/out) are not yet implemented because they
//! require multi-pointer tracking with frame coalescing.
//! Each completed gesture is mapped to an `Action` via TouchConfig and posted
//! to the global action queue read by `backend::message_loop`.

use crate::input::Action;
use parking_lot::Mutex;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, WH_GETMESSAGE,
};

// WM_POINTER* message constants (from Win32 SDK).
// We define them here because Win32_UI_Input_Pointer is not enabled as a cargo feature.
const WM_POINTERDOWN: u32 = 0x0246;
const WM_POINTERUP: u32 = 0x0247;
const WM_POINTERUPDATE: u32 = 0x0245;

// ---------------------------------------------------------------------------
// Public config types
// ---------------------------------------------------------------------------

/// Configuration for touch input.
#[derive(Debug, Clone)]
pub struct TouchConfig {
    pub enabled: bool,
    /// Minimum swipe distance in pixels to register as a swipe (vs a tap).
    pub swipe_threshold_px: u32,
    /// Maximum time for a swipe gesture in milliseconds.
    pub swipe_timeout_ms: u32,
    /// Map of gesture → Action.
    pub gestures: std::collections::HashMap<TouchGesture, Action>,
}

impl Default for TouchConfig {
    fn default() -> Self {
        let mut gestures = std::collections::HashMap::new();
        // Sensible niri-like defaults; user overrides via config.
        gestures.insert(TouchGesture::SwipeLeft, Action::FocusColumnRight);
        gestures.insert(TouchGesture::SwipeRight, Action::FocusColumnLeft);
        gestures.insert(TouchGesture::SwipeUp, Action::FocusUp);
        gestures.insert(TouchGesture::SwipeDown, Action::FocusDown);
        Self {
            enabled: false,
            swipe_threshold_px: 50,
            swipe_timeout_ms: 500,
            gestures,
        }
    }
}

/// Named touch gestures that can be bound to actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TouchGesture {
    Tap,
    SwipeLeft,
    SwipeRight,
    SwipeUp,
    SwipeDown,
    PinchIn,
    PinchOut,
}

// ---------------------------------------------------------------------------
// Gesture recognizer
// ---------------------------------------------------------------------------

/// Minimal gesture recognizer. Tracks one finger only.
#[derive(Debug, Default)]
pub struct GestureRecognizer {
    start: Option<(POINT, Instant)>,
    swipe_threshold_px: u32,
    swipe_timeout_ms: u32,
}

impl GestureRecognizer {
    pub fn new(config: &TouchConfig) -> Self {
        Self {
            start: None,
            swipe_threshold_px: config.swipe_threshold_px,
            swipe_timeout_ms: config.swipe_timeout_ms,
        }
    }

    pub fn pointer_down(&mut self, p: POINT) {
        self.start = Some((p, Instant::now()));
    }

    pub fn pointer_up(&mut self, p: POINT) -> Option<TouchGesture> {
        let (start_p, start_t) = self.start.take()?;
        let elapsed = start_t.elapsed();
        let dx = (p.x - start_p.x) as i64;
        let dy = (p.y - start_p.y) as i64;
        let dist_sq = dx * dx + dy * dy;
        let threshold_sq = (self.swipe_threshold_px as i64).pow(2);

        if elapsed > Duration::from_millis(self.swipe_timeout_ms as u64) {
            return None;
        }
        if dist_sq < threshold_sq {
            return Some(TouchGesture::Tap);
        }
        // Determine direction by dominant axis.
        if dx.abs() > dy.abs() {
            if dx > 0 {
                Some(TouchGesture::SwipeRight)
            } else {
                Some(TouchGesture::SwipeLeft)
            }
        } else if dy > 0 {
            Some(TouchGesture::SwipeDown)
        } else {
            Some(TouchGesture::SwipeUp)
        }
    }

    /// Test-only helper: inject a start point with an explicit timestamp so
    /// timeout scenarios can be exercised without sleeping.
    #[cfg(test)]
    pub fn start_at(&mut self, p: POINT, t: Instant) {
        self.start = Some((p, t));
    }
}

// ---------------------------------------------------------------------------
// Win32 pointer info — minimal inline definition
//
// GetPointerInfo lives in User32. We declare it ourselves because the
// Win32_UI_Input_Pointer crate feature is not enabled in Cargo.toml.
// ---------------------------------------------------------------------------

/// Minimal POINTER_INFO — only the fields we actually read.
#[repr(C)]
struct PointerInfo {
    pointer_type: u32,
    pointer_id: u32,
    frame_id: u32,
    pointer_flags: u32,
    source_device: isize,
    hwnd_target: isize,
    pt_pixel_location: POINT,
    // … additional fields exist in the real struct; we don't need them.
    _pad: [u8; 96],
}

impl PointerInfo {
    /// Zeroed PointerInfo — safe because all fields are POD (numeric / handle / array).
    fn zeroed() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

extern "system" {
    /// <https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getpointerinfo>
    fn GetPointerInfo(pointer_id: u32, pointer_info: *mut PointerInfo) -> i32;
}

/// Extract the pointer id from a WM_POINTER* wParam.
#[inline]
fn pointer_id_from_wparam(wparam: usize) -> u32 {
    (wparam & 0xFFFF) as u32
}

// ---------------------------------------------------------------------------
// Global hook state
// ---------------------------------------------------------------------------

/// Wrapper to make HHOOK Send-safe (it's an opaque pointer we pass to Win32).
struct SendHook(HHOOK);
unsafe impl Send for SendHook {}

struct TouchHookState {
    config: TouchConfig,
    recognizer: GestureRecognizer,
}

static TOUCH_HOOK: LazyLock<Mutex<Option<SendHook>>> =
    LazyLock::new(|| Mutex::new(None));

static TOUCH_STATE: LazyLock<Mutex<Option<TouchHookState>>> =
    LazyLock::new(|| Mutex::new(None));

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Install a `WH_GETMESSAGE` hook that intercepts WM_POINTER* messages and
/// feeds them into the gesture recognizer. If touch is disabled in `config`
/// or the hook cannot be installed, logs a warning and returns `Ok(())` —
/// touch is strictly optional.
///
/// Must be called from a thread that has a Windows message pump.
#[allow(dead_code)]
pub fn start_touch_hook(config: TouchConfig) -> Result<(), String> {
    if !config.enabled {
        info!("Touch input disabled in config — skipping hook installation");
        return Ok(());
    }

    // Guard against double-install.
    if TOUCH_HOOK.lock().is_some() {
        warn!("Touch hook already installed; skipping duplicate SetWindowsHookExW");
        return Ok(());
    }

    let recognizer = GestureRecognizer::new(&config);
    *TOUCH_STATE.lock() = Some(TouchHookState { config, recognizer });

    let hook = unsafe {
        SetWindowsHookExW(
            WH_GETMESSAGE,
            Some(touch_hook_callback),
            windows::Win32::Foundation::HMODULE::default(),
            0,
        )
    };

    match hook {
        Ok(h) => {
            *TOUCH_HOOK.lock() = Some(SendHook(h));
            info!("Touch (WH_GETMESSAGE) hook installed");
            Ok(())
        }
        Err(e) => {
            let os_err = std::io::Error::last_os_error();
            let msg = format!(
                "SetWindowsHookExW(WH_GETMESSAGE) failed: {:?} os_err={}",
                e, os_err
            );
            warn!("{}", msg);
            // Touch is optional — clear state and return Ok so callers don't
            // need to handle the error unless they care.
            *TOUCH_STATE.lock() = None;
            Ok(())
        }
    }
}

/// Remove the WH_GETMESSAGE touch hook, if installed.
#[allow(dead_code)]
pub fn stop_touch_hook() {
    if let Some(h) = TOUCH_HOOK.lock().take() {
        unsafe { let _ = UnhookWindowsHookEx(h.0); }
        info!("Touch hook removed");
    }
    *TOUCH_STATE.lock() = None;
}

// ---------------------------------------------------------------------------
// Hook callback
// ---------------------------------------------------------------------------

/// `WH_GETMESSAGE` callback — called for every message retrieved by
/// `GetMessageW` / `PeekMessageW` in any thread in the same process.
///
/// We peek at WM_POINTERDOWN / WM_POINTERUP. For each, we call
/// `GetPointerInfo` to obtain the screen-space pixel coordinate, then feed
/// it into the gesture recognizer. On a completed gesture we look up the
/// mapped action from the config and log it. Full dispatch (posting to the
/// hotkey channel) is a TODO once a shared action-sender is wired in.
unsafe extern "system" fn touch_hook_callback(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code < 0 {
        return CallNextHookEx(None, n_code, w_param, l_param);
    }

    // w_param for WH_GETMESSAGE: PM_NOREMOVE (0) or PM_REMOVE (1) — we
    // handle both so we see all messages regardless of how they were peeked.
    let msg_ptr = l_param.0 as *const windows::Win32::UI::WindowsAndMessaging::MSG;
    if msg_ptr.is_null() {
        return CallNextHookEx(None, n_code, w_param, l_param);
    }
    let msg = &*msg_ptr;

    match msg.message {
        WM_POINTERDOWN | WM_POINTERUP => {
            let pid = pointer_id_from_wparam(msg.wParam.0);
            let mut info = PointerInfo::zeroed();
            if GetPointerInfo(pid, &mut info) != 0 {
                let pt = info.pt_pixel_location;
                if let Some(state) = TOUCH_STATE.lock().as_mut() {
                    if msg.message == WM_POINTERDOWN {
                        debug!("touch: pointer_down ({}, {})", pt.x, pt.y);
                        state.recognizer.pointer_down(pt);
                    } else {
                        debug!("touch: pointer_up ({}, {})", pt.x, pt.y);
                        if let Some(gesture) = state.recognizer.pointer_up(pt) {
                            debug!("touch: gesture detected = {:?}", gesture);
                            if let Some(action) = state.config.gestures.get(&gesture) {
                                info!("touch gesture {:?} -> action {:?}", gesture, action);
                                push_pending_action(action.clone());
                            }
                        }
                    }
                }
            } else {
                warn!("touch: GetPointerInfo failed for pointer_id={}", pid);
            }
        }
        WM_POINTERUPDATE => {
            // Intentionally ignored for now — single-finger tracking only.
        }
        _ => {}
    }

    CallNextHookEx(None, n_code, w_param, l_param)
}

// ---------------------------------------------------------------------------
// Pending-action queue
// ---------------------------------------------------------------------------

/// Pending touch-triggered actions. Drained by `take_pending_actions()` which
/// is called once per tick from the message loop. We use a plain `Vec` behind
/// a `Mutex` (rather than a channel) so we can drop duplicates of the same
/// gesture in a single tick without re-implementing back-pressure.
static PENDING_ACTIONS: LazyLock<Mutex<Vec<Action>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Append an action to the pending queue. Called from the touch hook callback
/// when a recognized gesture has a config-mapped action.
pub fn push_pending_action(action: Action) {
    let mut q = PENDING_ACTIONS.lock();
    // Drop trivial repeats so a finger held against the screen doesn't fire
    // the same workspace switch 60 times a second.
    if q.last() != Some(&action) {
        q.push(action);
    }
}

/// Drain and return all currently queued actions. Called by the message loop
/// each tick to dispatch buffered gestures.
pub fn take_pending_actions() -> Vec<Action> {
    std::mem::take(&mut *PENDING_ACTIONS.lock())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_point(x: i32, y: i32) -> POINT {
        POINT { x, y }
    }

    fn default_recognizer() -> GestureRecognizer {
        GestureRecognizer::new(&TouchConfig::default())
    }

    #[test]
    fn test_recognizer_tap() {
        let mut r = default_recognizer();
        r.pointer_down(make_point(10, 10));
        // Move only 2–3 px — well below the 50 px threshold.
        let result = r.pointer_up(make_point(12, 11));
        assert_eq!(result, Some(TouchGesture::Tap));
    }

    #[test]
    fn test_recognizer_swipe_right() {
        let mut r = default_recognizer();
        // Inject a start in the recent past (within 200 ms).
        let start_t = Instant::now() - Duration::from_millis(50);
        r.start_at(make_point(10, 10), start_t);
        let result = r.pointer_up(make_point(200, 15));
        assert_eq!(result, Some(TouchGesture::SwipeRight));
    }

    #[test]
    fn test_recognizer_swipe_up() {
        let mut r = default_recognizer();
        let start_t = Instant::now() - Duration::from_millis(50);
        r.start_at(make_point(100, 200), start_t);
        // dy = 50 - 200 = -150 (upward); dx = 5; |dy| > |dx|; dy < 0 → SwipeUp.
        let result = r.pointer_up(make_point(105, 50));
        assert_eq!(result, Some(TouchGesture::SwipeUp));
    }

    #[test]
    fn test_recognizer_timeout_returns_none() {
        let mut r = default_recognizer();
        // Place start 2 seconds in the past — exceeds the 500 ms timeout.
        let past = Instant::now() - Duration::from_secs(2);
        r.start_at(make_point(10, 10), past);
        // Distance is large enough to be a swipe, but elapsed > timeout.
        let result = r.pointer_up(make_point(200, 15));
        assert_eq!(result, None);
    }
}
