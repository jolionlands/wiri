//! Interactive IPC picks share the existing input hooks and dispatch message thread.
//! Only the input state machine runs in hook callbacks; window and pixel queries do not.
use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use std::sync::{Arc, LazyLock};
use tokio::sync::oneshot;
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC};
use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, WindowFromPoint, GA_ROOT};

use crate::layout::TilingEngine;
use crate::utils::Point;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickKind {
    Window,
    Color,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    Sample { id: u64, kind: PickKind, at: Point },
    Cancel { id: u64 },
}
impl Effect {
    fn id(self) -> u64 {
        match self {
            Self::Sample { id, .. } | Self::Cancel { id } => id,
        }
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
struct Decision {
    consume: bool,
    effect: Option<Effect>,
}

#[derive(Clone, Copy)]
struct Session {
    id: u64,
    kind: PickKind,
    resolving: bool,
}

/// Captured releases outlive the request, including when its future is dropped.
/// Per-origin down state prevents stealing an already-held press, or letting an
/// injected release clear a physical guard (and vice versa).
#[derive(Default)]
struct PickState {
    next_id: u64,
    active: Option<Session>,
    left_down: [bool; 2],
    escape_down: [bool; 2],
    captured_left: Option<bool>,
    captured_escape: Option<bool>,
}
impl PickState {
    fn begin(&mut self, kind: PickKind) -> Result<u64, String> {
        if self.active.is_some() || self.captured_left.is_some() || self.captured_escape.is_some() {
            return Err("another pick is active or waiting for its captured input release".into());
        }
        self.next_id = self.next_id.wrapping_add(1);
        let id = self.next_id;
        self.active = Some(Session {
            id,
            kind,
            resolving: false,
        });
        Ok(id)
    }

    fn matches(&self, id: u64) -> bool {
        self.active.is_some_and(|session| session.id == id)
    }

    fn finish(&mut self, id: u64) -> bool {
        if !self.matches(id) {
            return false;
        }
        self.active = None;
        // Never clear captured_* here: the matching-origin up must still be eaten.
        true
    }

    fn left(&mut self, pressed: bool, at: Point, injected: bool) -> Decision {
        let was_down = std::mem::replace(&mut self.left_down[usize::from(injected)], pressed);
        if let Some(origin) = self.captured_left {
            if origin != injected {
                return Decision::default();
            }
            if !pressed {
                self.captured_left = None;
            }
            return Decision {
                consume: true,
                effect: None,
            };
        }
        if pressed && !was_down {
            if let Some(session) = self.active.as_mut().filter(|session| !session.resolving) {
                session.resolving = true;
                self.captured_left = Some(injected);
                return Decision {
                    consume: true,
                    effect: Some(Effect::Sample {
                        id: session.id,
                        kind: session.kind,
                        at,
                    }),
                };
            }
        }
        Decision::default()
    }

    fn escape(&mut self, pressed: bool, injected: bool) -> Decision {
        let was_down = std::mem::replace(&mut self.escape_down[usize::from(injected)], pressed);
        if let Some(origin) = self.captured_escape {
            if origin != injected {
                return Decision::default();
            }
            if !pressed {
                self.captured_escape = None;
            }
            return Decision {
                consume: true,
                effect: None,
            };
        }
        if pressed && !was_down {
            if let Some(session) = self.active.as_mut().filter(|session| !session.resolving) {
                session.resolving = true;
                self.captured_escape = Some(injected);
                return Decision {
                    consume: true,
                    effect: Some(Effect::Cancel { id: session.id }),
                };
            }
        }
        Decision::default()
    }
}

type Reply = oneshot::Sender<Result<Value, String>>;
struct Request {
    reply: Reply,
    engine: Arc<RwLock<TilingEngine>>,
}
#[derive(Default)]
struct Runtime {
    input: PickState,
    request: Option<Request>,
}
impl Runtime {
    fn finish(&mut self, id: u64) -> Option<Reply> {
        if self.input.finish(id) {
            self.request.take().map(|request| request.reply)
        } else {
            None
        }
    }
}
static PICK: LazyLock<Mutex<Runtime>> = LazyLock::new(|| Mutex::new(Runtime::default()));

/// Read-only modal guard for message-thread pointer consumers.
pub fn is_active() -> bool {
    PICK.lock().input.active.is_some()
}

struct RequestGuard(u64);
impl Drop for RequestGuard {
    fn drop(&mut self) {
        // Dropping a disconnected request cancels it, but not its release guards.
        let reply = PICK.lock().finish(self.0);
        drop(reply);
    }
}

/// Wait for the next left click, or Escape (`null`). Returns a managed
/// `WindowIpc` object or `{ "rgb": [r, g, b] }` with normalized sRGB channels.
/// Dropping this future cancels the request. The server must not apply its normal
/// response deadline while waiting, and should drop it on requester disconnect.
pub async fn pick(kind: PickKind) -> Result<Value, String> {
    let (reply, receive) = oneshot::channel();
    let id = crate::input::low_level_hook::with_pick_hooks(|engine| {
        let mut runtime = PICK.lock();
        let id = runtime.input.begin(kind)?;
        runtime.request = Some(Request { reply, engine });
        Ok::<_, String>(id)
    })??;
    let _guard = RequestGuard(id);
    receive
        .await
        .map_err(|_| "pick response channel closed".to_string())?
}

/// Called before binds or drag handling. Physical and injected input can pick,
/// but only the matching origin can release its captured press.
pub(crate) fn mouse_event(message: u32, at: Point, injected: bool) -> bool {
    let pressed = match message {
        0x0201 => true,  // WM_LBUTTONDOWN
        0x0202 => false, // WM_LBUTTONUP
        _ => return false,
    };
    let decision = PICK.lock().input.left(pressed, at, injected);
    dispatch(decision.effect);
    decision.consume
}

pub(crate) fn keyboard_event(vk: u32, pressed: bool, injected: bool) -> bool {
    if vk != 0x1B {
        return false;
    }
    let decision = PICK.lock().input.escape(pressed, injected);
    dispatch(decision.effect);
    decision.consume
}

pub(crate) fn hooks_unavailable() {
    let reply = {
        let mut runtime = PICK.lock();
        runtime
            .input
            .active
            .map(|session| session.id)
            .and_then(|id| runtime.finish(id))
    };
    if let Some(reply) = reply {
        let _ = reply.send(Err("input hook became unavailable during pick".into()));
    }
}

// A queued task may be discarded by dispatch shutdown or a failed wake-up. Its
// destructor completes the request instead of leaving the IPC caller hanging.
struct QueuedPick(u64);
impl Drop for QueuedPick {
    fn drop(&mut self) {
        let reply = PICK.lock().finish(self.0);
        if let Some(reply) = reply {
            let _ = reply.send(Err("input dispatcher unavailable during pick".into()));
        }
    }
}

fn dispatch(effect: Option<Effect>) {
    let Some(effect) = effect else { return };
    let queued = QueuedPick(effect.id());
    let _ = crate::input::dispatch::send_work(Box::new(move || {
        let _queued = queued;
        resolve(effect);
    }));
}

fn resolve(effect: Effect) {
    let engine = {
        let runtime = PICK.lock();
        if !runtime.input.matches(effect.id()) {
            return;
        }
        let Some(request) = runtime
            .request
            .as_ref()
            .filter(|request| !request.reply.is_closed())
        else {
            return;
        };
        Arc::clone(&request.engine)
    };
    let result = match effect {
        Effect::Cancel { .. } => Ok(Value::Null),
        Effect::Sample {
            kind: PickKind::Window,
            at,
            ..
        } => window_at(&engine, at),
        Effect::Sample {
            kind: PickKind::Color,
            at,
            ..
        } => color_at(at),
    };
    let reply = PICK.lock().finish(effect.id());
    if let Some(reply) = reply {
        let _ = reply.send(result);
    }
}

fn window_at(engine: &RwLock<TilingEngine>, at: Point) -> Result<Value, String> {
    // SAFETY: these Win32 queries accept screen coordinates and validate HWNDs;
    // no handle is dereferenced or retained. This runs on the dispatch thread.
    let root = unsafe {
        let hit = WindowFromPoint(POINT { x: at.x, y: at.y });
        GetAncestor(hit, GA_ROOT)
    };
    if root.0.is_null() {
        return Ok(Value::Null);
    }
    let id = root.0 as isize as u64;
    let snapshot = engine.read().ipc_snapshot();
    match snapshot.windows.into_iter().find(|window| window.id == id) {
        Some(window) => serde_json::to_value(window)
            .map_err(|error| format!("serialize picked window: {error}")),
        None => Ok(Value::Null),
    }
}

fn color_at(at: Point) -> Result<Value, String> {
    // SAFETY: the screen DC is acquired, used, and released on this dispatch
    // thread. GetPixel does not borrow buffers, and the DC is checked before use.
    let pixel = unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err("could not acquire screen DC for color pick".into());
        }
        let pixel = GetPixel(screen, at.x, at.y);
        let _ = ReleaseDC(None, screen);
        pixel
    };
    if pixel.0 == u32::MAX {
        return Err("could not read screen pixel for color pick".into());
    }
    Ok(serde_json::json!({ "rgb": colorref_to_srgb(pixel.0) }))
}

fn colorref_to_srgb(pixel: u32) -> [f32; 3] {
    [pixel & 0xff, (pixel >> 8) & 0xff, (pixel >> 16) & 0xff].map(|channel| channel as f32 / 255.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    const AT: Point = Point { x: -120, y: 77 };

    #[test]
    fn click_selects_once_and_consumes_down_repeat_up() {
        let mut state = PickState::default();
        let id = state.begin(PickKind::Window).unwrap();
        assert_eq!(
            state.left(true, AT, false),
            Decision {
                consume: true,
                effect: Some(Effect::Sample {
                    id,
                    kind: PickKind::Window,
                    at: AT,
                }),
            }
        );
        assert_eq!(
            state.left(true, AT, false),
            Decision {
                consume: true,
                effect: None,
            }
        );
        assert!(state.left(false, AT, false).consume);
        assert!(!state.left(false, AT, false).consume);
    }

    #[test]
    fn only_one_request_including_while_resolving() {
        let mut state = PickState::default();
        state.begin(PickKind::Color).unwrap();
        assert!(state.begin(PickKind::Window).is_err());
        state.left(true, AT, false);
        state.left(false, AT, false);
        assert!(state.begin(PickKind::Window).is_err());
    }

    #[test]
    fn completion_before_up_keeps_release_guard_and_busy_state() {
        let mut state = PickState::default();
        let id = state.begin(PickKind::Color).unwrap();
        state.left(true, AT, false);
        assert!(state.finish(id));
        assert!(state.begin(PickKind::Window).is_err());
        assert!(state.left(false, AT, false).consume);
        assert!(state.begin(PickKind::Window).is_ok());
    }

    #[test]
    fn escape_cancels_and_consumes_repeats_and_release_after_completion() {
        let mut state = PickState::default();
        let id = state.begin(PickKind::Color).unwrap();
        assert_eq!(
            state.escape(true, false),
            Decision {
                consume: true,
                effect: Some(Effect::Cancel { id }),
            }
        );
        assert!(state.finish(id));
        assert!(state.begin(PickKind::Window).is_err());
        assert_eq!(
            state.escape(true, false),
            Decision {
                consume: true,
                effect: None,
            }
        );
        assert!(state.escape(false, false).consume);
        assert!(!state.escape(false, false).consume);
        assert!(state.begin(PickKind::Window).is_ok());
    }

    #[test]
    fn disconnect_before_press_does_not_capture_later_input() {
        let mut state = PickState::default();
        let id = state.begin(PickKind::Window).unwrap();
        assert!(state.finish(id));
        assert!(!state.left(true, AT, false).consume);
        assert!(!state.left(false, AT, false).consume);
        assert!(!state.escape(true, false).consume);
        assert!(!state.escape(false, false).consume);
    }

    #[test]
    fn disconnect_after_press_still_consumes_the_paired_release() {
        let mut state = PickState::default();
        let id = state.begin(PickKind::Window).unwrap();
        state.left(true, AT, false);
        state.finish(id);
        assert!(state.left(false, AT, false).consume);
        assert!(!state.left(true, AT, false).consume);
        assert!(!state.left(false, AT, false).consume);
    }

    #[test]
    fn stale_work_cannot_finish_a_new_request() {
        let mut state = PickState::default();
        let first = state.begin(PickKind::Window).unwrap();
        state.finish(first);
        let second = state.begin(PickKind::Color).unwrap();
        assert_ne!(first, second);
        assert!(!state.finish(first));
        assert!(state.matches(second));
    }

    #[test]
    fn button_held_before_start_is_not_stolen() {
        let mut state = PickState::default();
        assert!(!state.left(true, AT, false).consume);
        let id = state.begin(PickKind::Window).unwrap();
        assert!(!state.left(true, AT, false).consume);
        assert!(!state.left(false, AT, false).consume);
        assert_eq!(
            state.left(true, AT, false).effect,
            Some(Effect::Sample {
                id,
                kind: PickKind::Window,
                at: AT,
            })
        );
    }

    #[test]
    fn escape_held_before_start_is_not_stolen() {
        let mut state = PickState::default();
        assert!(!state.escape(true, false).consume);
        let id = state.begin(PickKind::Window).unwrap();
        assert!(!state.escape(true, false).consume);
        assert!(!state.escape(false, false).consume);
        assert_eq!(
            state.escape(true, false).effect,
            Some(Effect::Cancel { id })
        );
    }

    #[test]
    fn injected_clicks_and_escape_select_and_cancel_with_paired_guards() {
        let mut state = PickState::default();
        let id = state.begin(PickKind::Color).unwrap();
        assert!(state.left(true, AT, true).effect.is_some());
        assert!(state.finish(id));
        assert!(!state.left(false, AT, false).consume);
        assert!(state.left(false, AT, true).consume);
        let id = state.begin(PickKind::Color).unwrap();
        assert_eq!(state.escape(true, true).effect, Some(Effect::Cancel { id }));
        assert!(state.finish(id));
        assert!(!state.escape(false, false).consume);
        assert!(state.escape(false, true).consume);
    }

    #[test]
    fn injected_release_does_not_clear_physical_release_guard() {
        let mut state = PickState::default();
        let id = state.begin(PickKind::Color).unwrap();
        state.left(true, AT, false);
        state.finish(id);
        assert!(!state.left(false, AT, true).consume);
        assert!(state.left(false, AT, false).consume);
        let id = state.begin(PickKind::Color).unwrap();
        state.escape(true, false);
        state.finish(id);
        assert!(!state.escape(false, true).consume);
        assert!(state.escape(false, false).consume);
    }

    #[test]
    fn first_terminal_event_wins() {
        let mut state = PickState::default();
        state.begin(PickKind::Window).unwrap();
        state.left(true, AT, false);
        assert!(!state.escape(true, false).consume);
        assert!(state.escape(false, false).effect.is_none());
        let mut state = PickState::default();
        state.begin(PickKind::Window).unwrap();
        state.escape(true, false);
        assert!(!state.left(true, AT, false).consume);
        assert!(state.left(false, AT, false).effect.is_none());
    }

    #[test]
    fn colorref_is_normalized_srgb_in_rgb_order() {
        assert_eq!(colorref_to_srgb(0), [0.0, 0.0, 0.0]);
        assert_eq!(colorref_to_srgb(0x00ffffff), [1.0, 1.0, 1.0]);
        assert_eq!(
            colorref_to_srgb(0x008040ff),
            [1.0, 64.0 / 255.0, 128.0 / 255.0]
        );
    }
}
