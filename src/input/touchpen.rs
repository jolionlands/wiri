//! Promoted touch/pen mouse events only. Windows often exposes just the primary
//! contact; no overlay, global WM_POINTER hook or synthetic hardware claim.
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::{Mutex, RwLock};

use crate::backend::BackendHandle;
use crate::layout::TilingEngine;
use crate::utils::{Point, Rect, WindowId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Touch(u8),
    Pen(u8),
}
/// MI_WP_SIGNATURE with its touch bit and (where supplied) seven-bit contact id.
pub fn source(extra: usize) -> Option<Source> {
    let bits = extra as u32;
    if bits & 0xffff_ff00 != 0xff51_5700 {
        return None;
    }
    Some(if bits & 0x80 != 0 {
        Source::Touch((bits & 0x7f) as u8)
    } else {
        Source::Pen((bits & 0x7f) as u8)
    })
}

/// Which niri `TabletStylusButtonN` a promoted pen mouse event carries, if it is a pen event at
/// all.
///
/// Windows has no global pen-button stream without UIAccess, so wiri only sees what the system
/// promotes to mouse input: the pen tip arrives as the left button, the barrel button as the
/// right button, and a second barrel button or the eraser as the first X button.  Which
/// physical button lands on which promoted button is the driver's choice, so this is an
/// approximation of niri's own evdev stylus buttons, not a device-accurate mapping.
pub fn stylus_button(message: u32, extra: usize) -> Option<u8> {
    if !matches!(source(extra)?, Source::Pen(_)) {
        return None;
    }
    match message {
        // WM_LBUTTONDOWN / WM_LBUTTONUP: the pen tip.
        0x201 | 0x202 => Some(1),
        // WM_RBUTTONDOWN / WM_RBUTTONUP: the barrel button.
        0x204 | 0x205 => Some(2),
        // WM_XBUTTONDOWN / WM_XBUTTONUP: a second barrel button or the eraser.
        0x20B | 0x20C => Some(3),
        _ => None,
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    Begin,
    Motion,
    Toggle,
    End,
    Guard,
}
#[derive(Default)]
struct Contacts {
    primary: Option<Source>,
    secondary: BTreeSet<Source>,
}
impl Contacts {
    fn event(&mut self, message: u32, contact: Source, allowed: bool) -> Option<Event> {
        match message {
            0x201 if self.primary.is_none() && self.secondary.is_empty() && allowed => {
                self.primary = Some(contact);
                Some(Event::Begin)
            }
            0x201 if self.primary == Some(contact) || self.secondary.contains(&contact) => {
                Some(Event::Guard)
            }
            0x201 if self.primary.is_some() => {
                if let (Some(Source::Touch(_)), Source::Touch(_)) = (self.primary, contact) {
                    self.secondary.insert(contact); // at most 127 seven-bit ids
                    return Some(Event::Toggle);
                }
                None
            }
            0x200 if self.primary == Some(contact) => Some(Event::Motion),
            0x202 if self.primary == Some(contact) => {
                self.primary = None;
                Some(Event::End)
            }
            0x202 if self.secondary.remove(&contact) => Some(Event::Guard),
            _ => None,
        }
    }
}
#[derive(Clone)]
struct Service {
    engine: Arc<RwLock<TilingEngine>>,
    backend: BackendHandle,
    touch: bool,
    pen: bool,
    generation: u64,
}
struct State {
    service: Option<Service>,
    contacts: Contacts,
    generation: u64,
}
static STATE: Mutex<State> = Mutex::new(State {
    service: None,
    contacts: Contacts {
        primary: None,
        secondary: BTreeSet::new(),
    },
    generation: 0,
});
thread_local! { static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) }; }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Scroll,
    Move,
}
fn select_mode(floating: bool, dx: f64, dy: f64) -> Option<Mode> {
    (dx.hypot(dy) >= 8.0).then_some(if !floating && dx.abs() > dy.abs() {
        Mode::Scroll
    } else {
        Mode::Move
    })
}
struct Session {
    id: WindowId,
    start: Point,
    last: Point,
    rect: Rect,
    floating: bool,
    mode: Option<Mode>,
    clock: Instant,
}

/// Called on the dispatcher thread, including reload and shutdown.
pub fn configure(engine: Arc<RwLock<TilingEngine>>, backend: BackendHandle) {
    shutdown();
    let (touch, pen) = engine
        .read()
        .full_config()
        .map(|c| (c.input.touch_enabled, c.input.tablet_enabled))
        .unwrap_or_default();
    if touch || pen {
        let mut state = STATE.lock();
        let generation = state.generation;
        state.service = Some(Service {
            engine,
            backend,
            touch,
            pen,
            generation,
        });
    }
}
pub fn is_active() -> bool {
    STATE.lock().contacts.primary.is_some()
}
pub fn shutdown() {
    let service = {
        let mut state = STATE.lock();
        state.generation = state.generation.wrapping_add(1);
        // Keep claimed contact releases guarded across reload/disable, like Lane B.
        state.service.take()
    };
    if let Some(s) = service {
        finish(&s, true);
    }
}
/// None is ordinary mouse input or a disabled/unclaimed promoted sequence.
pub fn handle(message: u32, point: Point, extra: usize, mods: u32) -> Option<bool> {
    let contact = source(extra)?;
    let busy = !is_active() && super::low_level_hook::is_grab_active();
    let mut state = STATE.lock();
    let service = state.service.clone();
    let mut begin = None;
    if !busy
        && state.contacts.primary.is_none()
        && message == 0x201
        && mods == super::pointer::ENGINE.lock().mod_key
    {
        if let Some(s) = service.as_ref().filter(|s| match contact {
            Source::Touch(_) => s.touch,
            Source::Pen(_) => s.pen,
        }) {
            use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, WindowFromPoint, GA_ROOT};
            let root = unsafe {
                GetAncestor(
                    WindowFromPoint(windows::Win32::Foundation::POINT {
                        x: point.x,
                        y: point.y,
                    }),
                    GA_ROOT,
                )
            };
            let id = WindowId::new(root.0 as isize);
            let e = s.engine.try_read()?;
            let info = e.tiled_windows().get(&id)?;
            if e.is_fullscreen(id) {
                return None;
            }
            let rect = s
                .backend
                .get_window(id.as_isize())
                .and_then(|w| w.get_dwm_bounds())
                .unwrap_or(info.bounds);
            begin = Some((id, rect, e.is_floating(id)));
        }
    }
    let event = state.contacts.event(message, contact, begin.is_some())?;
    drop(state);
    if event == Event::End {
        super::pointer::ENGINE
            .lock()
            .button(0, 0, false, Instant::now());
    }
    if event != Event::Guard {
        if let Some(s) = service {
            let queued = super::dispatch::send_work(Box::new(move || {
                if STATE.lock().generation != s.generation {
                    return;
                }
                match event {
                    Event::Begin => {
                        if let Some((id, rect, floating)) = begin {
                            SESSION.with(|slot| {
                                *slot.borrow_mut() = Some(Session {
                                    id,
                                    start: point,
                                    last: point,
                                    rect,
                                    floating,
                                    mode: None,
                                    clock: Instant::now(),
                                })
                            });
                        }
                    }
                    Event::Motion => motion(&s, point),
                    Event::Toggle => toggle(&s),
                    Event::End => {
                        motion(&s, point);
                        finish(&s, false);
                    }
                    Event::Guard => {}
                }
            }));
            if event == Event::Begin {
                if !queued {
                    STATE.lock().contacts.primary = None;
                    return None;
                }
                super::pointer::ENGINE.lock().capture_gesture(0);
            }
        }
    }
    // Motion must pass so Windows advances the cursor; only claimed button edges vanish.
    Some(message != 0x200)
}
fn motion(s: &Service, point: Point) {
    SESSION.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(g) = slot.as_mut() else {
            return;
        };
        let dx = (point.x - g.start.x) as f64;
        let dy = (point.y - g.start.y) as f64;
        if g.mode.is_none() {
            g.mode = select_mode(g.floating, dx, dy);
            match g.mode {
                Some(Mode::Scroll) => {
                    super::gestures::external_begin_at(&s.engine, &s.backend, g.start)
                }
                Some(Mode::Move) => s.engine.write().native_move_resize_start(g.id),
                None => {}
            }
        }
        match g.mode {
            Some(Mode::Scroll) => super::gestures::external_update_pixels(
                &s.engine,
                &s.backend,
                (point.x - g.last.x) as f64,
                0.0,
                g.clock.elapsed(),
            ),
            Some(Mode::Move) => super::low_level_hook::preview_rect(
                g.id,
                Rect::new(
                    g.rect.loc.x + dx as i32,
                    g.rect.loc.y + dy as i32,
                    g.rect.size.w,
                    g.rect.size.h,
                ),
                &s.backend,
            ),
            None => {}
        }
        // Retain the starting coordinate until axis lock; don't lose the first pixels.
        if g.mode.is_some() {
            g.last = point;
        }
    });
}
fn toggle(s: &Service) {
    SESSION.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(g) = slot.as_mut() else {
            return;
        };
        if matches!(g.mode, Some(Mode::Scroll)) {
            super::gestures::external_end(&s.engine, &s.backend, g.clock.elapsed(), true);
        }
        let mut e = s.engine.write();
        e.toggle_floating_for(g.id, &s.backend);
        g.floating = e.is_floating(g.id);
        e.native_move_resize_start(g.id);
        g.mode = Some(Mode::Move);
    });
}
fn finish(s: &Service, cancel: bool) {
    if let Some(g) = SESSION.with(|slot| slot.borrow_mut().take()) {
        match g.mode {
            Some(Mode::Scroll) => {
                super::gestures::external_end(&s.engine, &s.backend, g.clock.elapsed(), cancel)
            }
            Some(Mode::Move) => {
                let mut e = s.engine.write();
                if cancel {
                    e.pointer_resize_end(g.id, g.rect, &s.backend);
                } else {
                    e.pointer_move_end(
                        g.id,
                        g.last,
                        Rect::new(
                            g.rect.loc.x + g.last.x - g.start.x,
                            g.rect.loc.y + g.last.y - g.start.y,
                            g.rect.size.w,
                            g.rect.size.h,
                        ),
                        &s.backend,
                    );
                }
            }
            None => {}
        }
        crate::ipc::notify_state_changed();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signature_distinguishes_touch_pen_and_ordinary_mouse() {
        assert_eq!(source(0xff515785), Some(Source::Touch(5)));
        assert_eq!(source(0xff515705), Some(Source::Pen(5)));
        assert_eq!(source(0), None);
        assert_eq!(source(0x57495249), None);
    }
    #[test]
    fn stylus_buttons_follow_only_promoted_pen_button_events() {
        let pen = 0xff515705usize;
        let touch = 0xff515785usize;
        // The pen tip, the barrel button and a second barrel button (or the eraser).
        assert_eq!(stylus_button(0x201, pen), Some(1));
        assert_eq!(stylus_button(0x202, pen), Some(1));
        assert_eq!(stylus_button(0x204, pen), Some(2));
        assert_eq!(stylus_button(0x205, pen), Some(2));
        assert_eq!(stylus_button(0x20B, pen), Some(3));
        assert_eq!(stylus_button(0x20C, pen), Some(3));
        // Motion and wheel are not buttons; a finger is not a pen; a mouse is not a pen.
        assert_eq!(stylus_button(0x200, pen), None);
        assert_eq!(stylus_button(0x20A, pen), None);
        assert_eq!(stylus_button(0x201, touch), None);
        assert_eq!(stylus_button(0x201, 0), None);
    }
    #[test]
    fn mod_claim_secondary_taps_and_paired_releases_even_after_primary_ends() {
        let mut c = Contacts::default();
        let first = Source::Touch(1);
        let second = Source::Touch(2);
        assert_eq!(c.event(0x201, first, false), None);
        assert_eq!(c.event(0x201, first, true), Some(Event::Begin));
        assert_eq!(c.event(0x200, first, false), Some(Event::Motion));
        assert_eq!(c.event(0x201, second, false), Some(Event::Toggle));
        assert_eq!(c.event(0x201, second, false), Some(Event::Guard));
        assert_eq!(c.event(0x202, second, false), Some(Event::Guard));
        assert_eq!(c.event(0x201, second, false), Some(Event::Toggle));
        assert_eq!(c.event(0x202, first, false), Some(Event::End));
        assert_eq!(c.event(0x202, second, false), Some(Event::Guard));
        assert_eq!(c.event(0x200, first, false), None);
        assert_eq!(c.event(0x202, Source::Touch(3), false), None);
    }
    #[test]
    fn pen_does_not_invent_secondary_touch_support() {
        let mut c = Contacts::default();
        c.event(0x201, Source::Pen(1), true);
        assert_eq!(c.event(0x201, Source::Touch(2), true), None);
    }
    #[test]
    fn tiled_horizontal_scrolls_but_vertical_and_floats_move() {
        assert_eq!(select_mode(false, 3.0, 2.0), None);
        assert_eq!(select_mode(false, 8.0, 1.0), Some(Mode::Scroll));
        assert_eq!(select_mode(false, 1.0, 8.0), Some(Mode::Move));
        assert_eq!(select_mode(true, 8.0, 1.0), Some(Mode::Move));
    }
    #[test]
    fn touch_and_tablet_are_independent_opt_ins_with_niri_off() {
        for (text, expected) in [
            ("", (false, false)),
            ("input { touch {}; }", (true, false)),
            ("input { tablet {}; }", (false, true)),
            ("input { touch { off; }; tablet { off; }; }", (false, false)),
        ] {
            let d = crate::config::decode::decode_text(text, "", None);
            assert!(!d.has_errors(), "{:?}", d.issues);
            assert_eq!(
                (d.config.input.touch_enabled, d.config.input.tablet_enabled),
                expected
            );
        }
    }
}
