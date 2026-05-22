//! Touch / pointer input scaffolding for wiri.
//!
//! Listens to WM_POINTERDOWN/WM_POINTERUP/WM_POINTERUPDATE (or WM_TOUCH) and
//! recognizes basic gestures: tap, swipe-left, swipe-right, swipe-up,
//! swipe-down, and two-finger pinch-in / pinch-out.
//!
//! Pinch detection coalesces WM_POINTER events by pointer id: when two
//! pointers are down simultaneously the recogniser records their initial
//! distance and emits a `PinchIn` / `PinchOut` gesture once the distance
//! changes by more than `pinch_threshold_px` between frames.  We deliberately
//! do not call `RegisterTouchWindow` / handle `WM_GESTURE` — Win32's
//! `GestureRecognizer` would re-implement the same logic at a different layer
//! and is a much larger surface to maintain.
//!
//! Each completed gesture is mapped to an `Action` via `TouchConfig` and
//! pushed to a global queue that `backend::message_loop` drains every tick.

use crate::input::Action;
use parking_lot::Mutex;
use std::collections::HashMap;
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
    /// Minimum pinch-distance change (px) between two pointers before a
    /// `PinchIn` / `PinchOut` gesture fires. Smaller values feel snappier but
    /// produce more spurious gestures on noisy hardware.
    pub pinch_threshold_px: u32,
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
        // Pinch defaults: pinch-out (spread two fingers) opens the overview;
        // pinch-in dismisses it. Both map to the same toggle so the gesture
        // round-trips even if the user's first fingers were already spread.
        gestures.insert(TouchGesture::PinchOut, Action::OverviewToggle);
        gestures.insert(TouchGesture::PinchIn, Action::OverviewToggle);
        Self {
            enabled: false,
            swipe_threshold_px: 50,
            swipe_timeout_ms: 500,
            pinch_threshold_px: 8,
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

/// Per-pointer tracking record. We retain the original-down location for the
/// primary pointer (used by tap / swipe recognition) and the most recent
/// location for every active pointer (used by pinch recognition).
#[derive(Debug, Clone, Copy)]
struct TrackedPointer {
    /// First location observed for this pointer (set on WM_POINTERDOWN).
    start_pos: POINT,
    /// Most recent location observed (updated on WM_POINTERUPDATE).
    last_pos: POINT,
    /// Timestamp of the down event — only meaningful for the primary pointer.
    start_t: Instant,
}

/// Gesture recogniser with single-finger (tap / swipe) and two-finger (pinch)
/// support.  Multi-pointer state is keyed by Win32 pointer id; pointers are
/// tracked in insertion order (so the first finger down is the "primary"
/// pointer that drives tap/swipe).
#[derive(Debug)]
pub struct GestureRecognizer {
    /// Active pointers keyed by Win32 pointer id.  We use a `Vec` rather than
    /// a `HashMap` because real workloads see <= 2 pointers and ordering
    /// matters (first-down is the primary tap/swipe pointer).
    pointers: Vec<(u32, TrackedPointer)>,
    /// Distance between the two pointers at the moment the second one went
    /// down.  Used as the reference for pinch-direction detection.  `None`
    /// when fewer than two pointers are active or after a pinch fires.
    pinch_reference_distance: Option<f32>,
    swipe_threshold_px: u32,
    swipe_timeout_ms: u32,
    pinch_threshold_px: u32,
}

impl Default for GestureRecognizer {
    fn default() -> Self {
        Self {
            pointers: Vec::new(),
            pinch_reference_distance: None,
            swipe_threshold_px: 50,
            swipe_timeout_ms: 500,
            pinch_threshold_px: 8,
        }
    }
}

impl GestureRecognizer {
    pub fn new(config: &TouchConfig) -> Self {
        Self {
            pointers: Vec::new(),
            pinch_reference_distance: None,
            swipe_threshold_px: config.swipe_threshold_px,
            swipe_timeout_ms: config.swipe_timeout_ms,
            pinch_threshold_px: config.pinch_threshold_px,
        }
    }

    /// Record a pointer-down event.  Single-finger callers (most of the
    /// existing test suite) can use `pointer_down(p)` which auto-assigns
    /// id 0; multi-pointer callers should use `pointer_down_id`.
    pub fn pointer_down(&mut self, p: POINT) {
        self.pointer_down_id(0, p);
    }

    /// Record a pointer-down event keyed by an explicit pointer id.
    pub fn pointer_down_id(&mut self, id: u32, p: POINT) {
        // Replace any stale entry for this id (defensive — Windows occasionally
        // re-uses pointer ids across short-lived gestures).
        self.pointers.retain(|(pid, _)| *pid != id);
        self.pointers.push((
            id,
            TrackedPointer {
                start_pos: p,
                last_pos: p,
                start_t: Instant::now(),
            },
        ));
        // If we just transitioned from one to two pointers, freeze the
        // reference distance so subsequent updates can detect pinch direction.
        if self.pointers.len() == 2 {
            self.pinch_reference_distance = Some(self.current_pointer_distance());
        }
    }

    /// Record a pointer-move event.  Returns a `PinchIn` / `PinchOut` gesture
    /// when two pointers are active and the inter-pointer distance has shifted
    /// by more than `pinch_threshold_px` from the last reference.  After
    /// emitting a pinch the reference distance is reset to the current
    /// distance so a continuous spread fires repeatedly (one event per
    /// threshold-crossing).
    pub fn pointer_update_id(&mut self, id: u32, p: POINT) -> Option<TouchGesture> {
        // Update last_pos for this pointer; bail if the id is unknown.
        let mut updated = false;
        for entry in self.pointers.iter_mut() {
            if entry.0 == id {
                entry.1.last_pos = p;
                updated = true;
                break;
            }
        }
        if !updated {
            return None;
        }

        if self.pointers.len() != 2 {
            return None;
        }
        let reference = self.pinch_reference_distance?;
        let current = self.current_pointer_distance();
        let delta = current - reference;
        if delta.abs() < self.pinch_threshold_px as f32 {
            return None;
        }
        // Reset the reference so the user can pinch continuously.
        self.pinch_reference_distance = Some(current);
        if delta > 0.0 {
            Some(TouchGesture::PinchOut)
        } else {
            Some(TouchGesture::PinchIn)
        }
    }

    /// Record a pointer-up event for the primary pointer (id 0 / first down).
    /// Same signature as the legacy single-touch API.
    pub fn pointer_up(&mut self, p: POINT) -> Option<TouchGesture> {
        let id = self.pointers.first().map(|(pid, _)| *pid).unwrap_or(0);
        self.pointer_up_id(id, p)
    }

    /// Record a pointer-up event for an explicit pointer id.  When the
    /// released pointer was the primary single-finger pointer this returns
    /// the resolved tap / swipe gesture; when releasing the second of a pair
    /// it returns `None` (the pinch has already fired during updates).
    pub fn pointer_up_id(&mut self, id: u32, p: POINT) -> Option<TouchGesture> {
        // Pop the entry — defensively handle a missing id (Windows can issue
        // a stray UP without a matching DOWN).
        let entry_idx = self.pointers.iter().position(|(pid, _)| *pid == id);
        let entry = entry_idx.map(|i| self.pointers.remove(i));

        // Releasing one of the two pointers ends the pinch session; drop the
        // reference distance so a fresh second-down restarts it.
        if self.pointers.len() < 2 {
            self.pinch_reference_distance = None;
        }

        let TrackedPointer { start_pos, start_t, .. } = match entry {
            Some((_, t)) => t,
            None => return None,
        };

        // Only the primary pointer (the first one that was tracked at the
        // time of the up event, i.e. the only remaining one if there was a
        // pair) resolves to tap / swipe.  If there are still pointers down
        // after this release we're mid-multi-touch and skip tap/swipe.
        if !self.pointers.is_empty() {
            return None;
        }

        let elapsed = start_t.elapsed();
        let dx = (p.x - start_pos.x) as i64;
        let dy = (p.y - start_pos.y) as i64;
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

    /// Distance between the two currently-tracked pointers.  Panics if there
    /// are fewer than two pointers — callers guard with `pointers.len() == 2`.
    fn current_pointer_distance(&self) -> f32 {
        let a = self.pointers[0].1.last_pos;
        let b = self.pointers[1].1.last_pos;
        let dx = (b.x - a.x) as f32;
        let dy = (b.y - a.y) as f32;
        (dx * dx + dy * dy).sqrt()
    }

    /// Test-only helper: inject a start point with an explicit timestamp so
    /// timeout scenarios can be exercised without sleeping.
    #[cfg(test)]
    pub fn start_at(&mut self, p: POINT, t: Instant) {
        self.pointers.clear();
        self.pointers.push((
            0,
            TrackedPointer {
                start_pos: p,
                last_pos: p,
                start_t: t,
            },
        ));
        self.pinch_reference_distance = None;
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
// Three-finger swipe — parallel pointer accumulator
// ---------------------------------------------------------------------------

/// Per-pointer record for the 3-finger swipe accumulator. Independent of
/// `GestureRecognizer` so the two-finger pinch path and the three-finger
/// swipe path do not interfere.
#[derive(Debug, Clone)]
struct ActivePointer {
    /// Mirrors the HashMap key; retained here so the struct is self-contained
    /// and logging doesn't require the caller to thread the key through.
    #[allow(dead_code)]
    pointer_id: u32,
    /// Screen-space pixel location at WM_POINTERDOWN time.
    start: POINT,
    /// Wall-clock time of the down event. Used to enforce the 300 ms lift
    /// window: if the oldest pointer's down happened more than 300 ms before
    /// the last lift, the gesture is discarded.
    start_time: Instant,
}

/// All pointers currently in the down state, keyed by pointer id. The hook
/// callback updates this on WM_POINTERDOWN and WM_POINTERUP. When the count
/// transitions from 3 → 0 (i.e. the third finger lifts) we evaluate the
/// aggregate displacement and possibly emit a 3-finger swipe gesture.
static ACTIVE_POINTERS: LazyLock<Mutex<HashMap<u32, ActivePointer>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Scratch accumulator for the displacement of pointers that lifted in the
/// current gesture cycle: `(start_point, end_point, down_time)`.
/// Cleared when a new down event arrives while the active map is empty
/// (start of a fresh gesture).
static LIFTED_POINTERS: LazyLock<Mutex<Vec<(POINT, POINT, Instant)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

// ---------------------------------------------------------------------------
// Optional action sender (for dispatch beyond the pending-action queue)
// ---------------------------------------------------------------------------

/// Wrapper so `tokio::sync::mpsc::UnboundedSender<Action>` is `Sync`.
struct SendSender(tokio::sync::mpsc::UnboundedSender<Action>);
// SAFETY: UnboundedSender is already Send; we only ever access it under the
// Mutex so Sync is fine here.
unsafe impl Sync for SendSender {}

static ACTION_SENDER: LazyLock<Mutex<Option<SendSender>>> =
    LazyLock::new(|| Mutex::new(None));

/// Wire a tokio mpsc sender so the hook can dispatch gestures to the main
/// event loop directly. Optional — if not set, gestures are still queued via
/// `push_pending_action` and available via `take_pending_actions`.
#[allow(dead_code)]
pub fn set_action_sender(tx: tokio::sync::mpsc::UnboundedSender<Action>) {
    *ACTION_SENDER.lock() = Some(SendSender(tx));
}

/// Dispatch `action` to the wired sender if available, otherwise fall back
/// to the pending-action queue. This function is safe to call from the hook
/// callback thread (the Mutex ensures mutual exclusion, and the sender is
/// `Send`).
fn dispatch_action(action: Action) {
    // Try the mpsc sender first so the engine receives it without polling delay.
    let sent = {
        let guard = ACTION_SENDER.lock();
        if let Some(SendSender(ref tx)) = *guard {
            tx.send(action.clone()).is_ok()
        } else {
            false
        }
    };
    if !sent {
        push_pending_action(action);
    }
}

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
        WM_POINTERDOWN | WM_POINTERUP | WM_POINTERUPDATE => {
            let pid = pointer_id_from_wparam(msg.wParam.0);
            let mut info = PointerInfo::zeroed();
            if GetPointerInfo(pid, &mut info) != 0 {
                let pt = info.pt_pixel_location;

                // ── Three-finger swipe accumulator ──────────────────────────
                // Maintained in parallel with GestureRecognizer (which handles
                // 1-finger swipe / 2-finger pinch) so the two paths don't
                // interfere.
                match msg.message {
                    WM_POINTERDOWN => {
                        let mut active = ACTIVE_POINTERS.lock();
                        // Start of a completely new gesture: clear the lifted
                        // accumulator so displacement from old fingers doesn't
                        // pollute the new gesture.
                        if active.is_empty() {
                            LIFTED_POINTERS.lock().clear();
                        }
                        active.insert(pid, ActivePointer {
                            pointer_id: pid,
                            start: pt,
                            start_time: Instant::now(),
                        });
                    }
                    WM_POINTERUP => {
                        let removed = ACTIVE_POINTERS.lock().remove(&pid);
                        if let Some(ap) = removed {
                            // Record (start, end, down_time) triple for swipe
                            // measurement and gesture-window enforcement.
                            LIFTED_POINTERS.lock().push((ap.start, pt, ap.start_time));
                        }

                        // Evaluate 3-finger swipe when all pointers have lifted
                        // and exactly 3 were involved.
                        let active_empty = ACTIVE_POINTERS.lock().is_empty();
                        if active_empty {
                            let lifted = {
                                let mut l = LIFTED_POINTERS.lock();
                                std::mem::take(&mut *l)
                            };
                            if lifted.len() == 3 {
                                // Enforce the 300 ms gesture window: all fingers
                                // must have come down within THREE_FINGER_WINDOW_MS
                                // of each other (oldest → youngest down time).
                                let now = Instant::now();
                                let oldest = lifted
                                    .iter()
                                    .map(|(_, _, t)| *t)
                                    .min()
                                    .unwrap_or(now);
                                let window_ok = now
                                    .duration_since(oldest)
                                    .as_millis()
                                    <= THREE_FINGER_WINDOW_MS as u128;
                                if window_ok {
                                    // Strip the timestamp for evaluate_three_finger_swipe.
                                    let pairs: Vec<(POINT, POINT)> = lifted
                                        .iter()
                                        .map(|(s, e, _)| (*s, *e))
                                        .collect();
                                    if let Some(gesture) = evaluate_three_finger_swipe(&pairs) {
                                        info!("touch: 3-finger gesture detected = {:?}", gesture);
                                        let action = TOUCH_STATE
                                            .lock()
                                            .as_ref()
                                            .and_then(|s| s.config.gestures.get(&gesture).cloned());
                                        if let Some(action) = action {
                                            info!("touch: 3-finger {:?} -> action {:?}", gesture, action);
                                            dispatch_action(action);
                                        } else {
                                            info!("touch: 3-finger {:?} (no action bound)", gesture);
                                        }
                                    }
                                } else {
                                    debug!("touch: 3-finger gesture discarded (outside {}ms window)", THREE_FINGER_WINDOW_MS);
                                }
                            }
                        }
                    }
                    _ => {}
                }

                // ── GestureRecognizer (1-finger swipe + 2-finger pinch) ──────
                if let Some(state) = TOUCH_STATE.lock().as_mut() {
                    let gesture: Option<TouchGesture> = match msg.message {
                        WM_POINTERDOWN => {
                            debug!("touch: pointer_down id={} ({}, {})", pid, pt.x, pt.y);
                            state.recognizer.pointer_down_id(pid, pt);
                            None
                        }
                        WM_POINTERUPDATE => {
                            // Only emit log lines on update if a pinch fires —
                            // every contact frame produces an update event, so
                            // logging unconditionally would flood the trace.
                            state.recognizer.pointer_update_id(pid, pt)
                        }
                        WM_POINTERUP => {
                            debug!("touch: pointer_up id={} ({}, {})", pid, pt.x, pt.y);
                            state.recognizer.pointer_up_id(pid, pt)
                        }
                        _ => unreachable!(),
                    };
                    if let Some(g) = gesture {
                        debug!("touch: gesture detected = {:?}", g);
                        if let Some(action) = state.config.gestures.get(&g) {
                            info!("touch gesture {:?} -> action {:?}", g, action);
                            dispatch_action(action.clone());
                        }
                    }
                }
            } else if msg.message != WM_POINTERUPDATE {
                // Failing to resolve a pointer-up / down is worth a warning;
                // failing on every coalesced update would spam the log.
                warn!("touch: GetPointerInfo failed for pointer_id={}", pid);
            }
        }
        _ => {}
    }

    CallNextHookEx(None, n_code, w_param, l_param)
}

// ---------------------------------------------------------------------------
// Three-finger swipe evaluation
// ---------------------------------------------------------------------------

/// Maximum time between the oldest down event and the last lift for the
/// group to qualify as a single gesture.
const THREE_FINGER_WINDOW_MS: u64 = 300;

/// Evaluate a completed 3-finger lift to determine if it constitutes a swipe.
///
/// `lifted` is a slice of `(start_point, end_point)` pairs, one per pointer,
/// ordered by lift time. Returns `None` if the average displacement is below
/// `SWIPE_THRESHOLD_PX` or if the direction is ambiguous.
///
/// This function is pure (no global state reads) so it can be exercised in
/// unit tests without Win32.
fn evaluate_three_finger_swipe(lifted: &[(POINT, POINT)]) -> Option<TouchGesture> {
    debug_assert_eq!(lifted.len(), 3, "caller must ensure exactly 3 lifted pointers");

    // Compute average displacement across the three fingers.
    let mut sum_dx: i64 = 0;
    let mut sum_dy: i64 = 0;
    for (start, end) in lifted {
        sum_dx += (end.x - start.x) as i64;
        sum_dy += (end.y - start.y) as i64;
    }
    let avg_dx = sum_dx / 3;
    let avg_dy = sum_dy / 3;

    // Use a fixed threshold of 50 px (matching the default swipe_threshold_px).
    // Callers that want a configurable threshold can wrap this at the call site.
    const THRESHOLD: i64 = 50;

    let abs_dx = avg_dx.abs();
    let abs_dy = avg_dy.abs();

    if abs_dx < THRESHOLD && abs_dy < THRESHOLD {
        // Displacement too small — not a deliberate swipe.
        return None;
    }

    // Require the dominant axis to be at least 2× the minor axis to avoid
    // diagionals being mis-classified.
    if abs_dx > abs_dy {
        // Horizontal swipe.
        if avg_dx > 0 {
            Some(TouchGesture::SwipeRight)
        } else {
            Some(TouchGesture::SwipeLeft)
        }
    } else {
        // Vertical swipe.
        if avg_dy > 0 {
            Some(TouchGesture::SwipeDown)
        } else {
            Some(TouchGesture::SwipeUp)
        }
    }
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

    // ── Multi-pointer pinch tests ───────────────────────────────────────────

    /// Two pointers move apart by more than the threshold -> PinchOut fires.
    #[test]
    fn test_recognizer_pinch_out() {
        let mut r = default_recognizer();
        r.pointer_down_id(1, make_point(100, 100));
        r.pointer_down_id(2, make_point(140, 100)); // reference distance = 40

        // First update inside threshold -> no gesture yet.
        assert_eq!(r.pointer_update_id(2, make_point(145, 100)), None);
        // Subsequent update pushes total distance past 8 px threshold.
        let g = r.pointer_update_id(2, make_point(160, 100));
        assert_eq!(g, Some(TouchGesture::PinchOut));
    }

    /// Two pointers move together by more than the threshold -> PinchIn fires.
    #[test]
    fn test_recognizer_pinch_in() {
        let mut r = default_recognizer();
        r.pointer_down_id(1, make_point(100, 100));
        r.pointer_down_id(2, make_point(200, 100)); // reference distance = 100
        let g = r.pointer_update_id(2, make_point(180, 100)); // delta = -20
        assert_eq!(g, Some(TouchGesture::PinchIn));
    }

    /// Tiny pointer movement below the pinch threshold is ignored.
    #[test]
    fn test_recognizer_pinch_below_threshold_is_silent() {
        let mut r = default_recognizer();
        r.pointer_down_id(1, make_point(100, 100));
        r.pointer_down_id(2, make_point(140, 100));
        // Only 3 px of additional spread -> still under the 8 px threshold.
        assert_eq!(r.pointer_update_id(2, make_point(143, 100)), None);
    }

    /// Single-pointer updates do not produce a pinch gesture.
    #[test]
    fn test_recognizer_single_pointer_update_no_pinch() {
        let mut r = default_recognizer();
        r.pointer_down_id(1, make_point(100, 100));
        // Move the lone pointer a long way — must not produce a pinch.
        assert_eq!(r.pointer_update_id(1, make_point(500, 500)), None);
    }

    /// Releasing one pointer ends the pinch session; a fresh second down
    /// re-establishes the reference distance.
    #[test]
    fn test_recognizer_pinch_resets_after_release() {
        let mut r = default_recognizer();
        r.pointer_down_id(1, make_point(100, 100));
        r.pointer_down_id(2, make_point(140, 100));
        assert_eq!(
            r.pointer_update_id(2, make_point(160, 100)),
            Some(TouchGesture::PinchOut)
        );
        // Release one pointer. With only one pointer left, no pinch is possible.
        let _ = r.pointer_up_id(2, make_point(160, 100));
        assert_eq!(r.pointer_update_id(1, make_point(50, 100)), None);
        // Second pointer comes back down — new reference distance applies.
        r.pointer_down_id(2, make_point(60, 100)); // distance now ~10
        // Tiny update -> still under threshold.
        assert_eq!(r.pointer_update_id(2, make_point(63, 100)), None);
        // Big spread -> PinchOut from the new reference.
        assert_eq!(
            r.pointer_update_id(2, make_point(80, 100)),
            Some(TouchGesture::PinchOut)
        );
    }

    /// Pinch gesture is mapped to the `OverviewToggle` action by default.
    #[test]
    fn test_default_config_maps_pinch_to_overview_toggle() {
        let cfg = TouchConfig::default();
        assert_eq!(
            cfg.gestures.get(&TouchGesture::PinchOut),
            Some(&Action::OverviewToggle)
        );
        assert_eq!(
            cfg.gestures.get(&TouchGesture::PinchIn),
            Some(&Action::OverviewToggle)
        );
    }

    // ── Three-finger swipe tests ────────────────────────────────────────────
    // These exercise `evaluate_three_finger_swipe` directly; no Win32 needed.

    /// Three fingers swiping right → SwipeRight.
    #[test]
    fn test_three_finger_swipe_right() {
        // Each finger starts at x=10 and ends at x=100; all move 90 px right.
        // avg_dx = 90, avg_dy = 0 — well above the 50 px threshold.
        let lifted = vec![
            (make_point(10, 100), make_point(100, 102)),
            (make_point(10, 200), make_point(100, 201)),
            (make_point(10, 300), make_point(100, 299)),
        ];
        assert_eq!(
            evaluate_three_finger_swipe(&lifted),
            Some(TouchGesture::SwipeRight),
            "three fingers moving right should produce SwipeRight"
        );
    }

    /// Displacement below the threshold → no gesture.
    #[test]
    fn test_three_finger_swipe_too_short_returns_none() {
        // Each finger moves only 10 px right — below the 50 px threshold.
        let lifted = vec![
            (make_point(50, 100), make_point(60, 100)),
            (make_point(50, 200), make_point(60, 200)),
            (make_point(50, 300), make_point(60, 300)),
        ];
        assert_eq!(
            evaluate_three_finger_swipe(&lifted),
            None,
            "displacement below threshold must not produce a gesture"
        );
    }

    /// Three fingers swiping up → SwipeUp.
    #[test]
    fn test_three_finger_swipe_up() {
        // Each finger moves 80 px upward (negative dy).
        let lifted = vec![
            (make_point(100, 300), make_point(102, 220)),
            (make_point(200, 300), make_point(201, 220)),
            (make_point(300, 300), make_point(299, 220)),
        ];
        assert_eq!(
            evaluate_three_finger_swipe(&lifted),
            Some(TouchGesture::SwipeUp),
        );
    }

    /// Three fingers swiping down → SwipeDown.
    #[test]
    fn test_three_finger_swipe_down() {
        let lifted = vec![
            (make_point(100, 100), make_point(100, 200)),
            (make_point(200, 100), make_point(200, 200)),
            (make_point(300, 100), make_point(300, 200)),
        ];
        assert_eq!(
            evaluate_three_finger_swipe(&lifted),
            Some(TouchGesture::SwipeDown),
        );
    }

    /// Three fingers swiping left → SwipeLeft.
    #[test]
    fn test_three_finger_swipe_left() {
        let lifted = vec![
            (make_point(200, 100), make_point(100, 100)),
            (make_point(200, 200), make_point(100, 200)),
            (make_point(200, 300), make_point(100, 300)),
        ];
        assert_eq!(
            evaluate_three_finger_swipe(&lifted),
            Some(TouchGesture::SwipeLeft),
        );
    }
}
