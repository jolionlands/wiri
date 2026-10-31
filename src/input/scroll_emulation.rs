//! Explicit mouse button scrolling. WH_MOUSE_LL cannot identify the device or
//! expose libinput's continuous axis source. This is a Windows approximation:
//! captured clicks are never replayed, and screen pixels replace device units.

use crate::config::{InputConfig, ScrollMethod};
use parking_lot::Mutex;
use std::sync::LazyLock;
use std::time::Instant;
use windows::Win32::Foundation::POINT;

/// Windows has 120 wheel units per notch. Use 15 screen pixels per notch;
/// neither a physical millimetre conversion nor niri's continuous-axis units.
const WHEEL_UNITS_PER_PIXEL: f64 = 120.0 / 15.0;
const AXIS_THRESHOLD_PX: f64 = 3.0;
/// libinput evdev.c SCROLL_BUTTON_LOCK_GRACE_TIMEOUT (500ms), as documented at
/// https://wayland.freedesktop.org/libinput/doc/latest/scrolling.html:
/// a held AND scrolled gesture past the grace period must not latch on release.
/// The separate 200ms click-replay delay is deliberately absent in this adapter.
const LOCK_GRACE_MS: u64 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Settings {
    button: u8,
    lock: bool,
    natural: bool,
}

fn map_button(code: u32) -> Option<u8> {
    match code {
        272..=276 => Some((code - 272) as u8),
        1..=5 => Some((code - 1) as u8), // decoder reports this compatibility alias
        _ => None,
    }
}

fn settings(input: &InputConfig) -> Option<Settings> {
    let config = &input.mouse_scroll;
    if input.mouse_off || config.method != Some(ScrollMethod::OnButtonDown) {
        return None;
    }
    Some(Settings {
        // No device default can be queried reliably from WH_MOUSE_LL: require
        // an explicit supported button, including when lock alone is configured.
        button: map_button(config.button?)?,
        lock: config.lock.unwrap_or(false),
        natural: config.natural,
    })
}

pub(super) fn applied(input: &InputConfig) -> bool {
    settings(input).is_some()
        || (input.mouse_scroll.method == Some(ScrollMethod::OnButtonDown)
            && input.mouse_scroll.button == Some(0))
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Output {
    consume: bool,
    horizontal: i32,
    vertical: i32,
}

#[derive(Debug, Default)]
struct Reducer {
    config: Option<Settings>,
    captured: [bool; 5],
    anchor: Option<(i32, i32)>,
    down_at: u64,
    latched: bool,
    unlocking: bool,
    scrolled: bool,
    axis_active: [bool; 2],
    pending: [f64; 2],
    residual: [f64; 2],
    generation: u64,
}

impl Reducer {
    fn clear_motion(&mut self) {
        // Queued wheel work belongs to this gesture's frozen recipient. Once
        // the hold ends or lock releases the real pointer can leave it, so no
        // queued wheel from that gesture may survive into another recipient.
        self.generation = self.generation.wrapping_add(1);
        self.anchor = None;
        self.latched = false;
        self.unlocking = false;
        self.scrolled = false;
        self.axis_active = [false; 2];
        self.pending = [0.0; 2];
        self.residual = [0.0; 2];
    }

    fn configure(&mut self, config: Option<Settings>) {
        self.clear_motion();
        self.config = config;
        // Preserve captured releases across reload/cancellation. A consumed down
        // must never leak an unmatched up to an application or a reloaded bind.
    }

    fn button(&mut self, button: u8, down: bool, point: (i32, i32), now: u64) -> bool {
        let Some(captured) = self.captured.get_mut(button as usize) else {
            return false;
        };
        if !down {
            if !std::mem::take(captured) {
                return false;
            }
            if self.config.is_some_and(|c| c.button == button) && self.anchor.is_some() {
                let lock = self.config.is_some_and(|c| c.lock);
                if lock
                    && !self.unlocking
                    && !(self.scrolled && now.saturating_sub(self.down_at) >= LOCK_GRACE_MS)
                {
                    self.latched = true;
                } else {
                    self.clear_motion();
                }
            }
            return true;
        }
        if *captured {
            return true;
        }
        if !self.config.is_some_and(|c| c.button == button) {
            return false;
        }
        *captured = true;
        if self.latched {
            self.unlocking = true;
        } else {
            self.clear_motion();
            self.anchor = Some(point);
            self.down_at = now;
        }
        true
    }

    fn motion(&mut self, point: (i32, i32), factor: f64) -> Output {
        let (Some(anchor), Some(config)) = (self.anchor, self.config) else {
            return Output::default();
        };
        let mut output = Output {
            consume: true,
            ..Output::default()
        };
        if !factor.is_finite() || factor < 0.0 {
            return output;
        }
        // Windows has not updated the cursor yet. We consume motion so the real
        // cursor stays at anchor; each point is a fresh delta from that anchor,
        // NOT a delta from the previous intercepted point.
        let delta = [
            point.0 as f64 - anchor.0 as f64,
            point.1 as f64 - anchor.1 as f64,
        ];
        let mut units = [0; 2];
        for axis in 0..2 {
            let mut movement = delta[axis];
            if !self.axis_active[axis] {
                self.pending[axis] += movement;
                if self.pending[axis].abs() < AXIS_THRESHOLD_PX {
                    continue;
                }
                self.axis_active[axis] = true;
                movement = std::mem::take(&mut self.pending[axis]);
            }
            if movement != 0.0 {
                self.scrolled = true;
            }
            let sign = if (axis == 1) ^ config.natural {
                -1.0
            } else {
                1.0
            };
            self.residual[axis] += movement * WHEEL_UNITS_PER_PIXEL * factor * sign;
            let whole = self.residual[axis]
                .trunc()
                .clamp(i32::MIN as f64, i32::MAX as f64) as i32;
            units[axis] = whole;
            self.residual[axis] -= whole as f64;
        }
        output.horizontal = units[0];
        output.vertical = units[1];
        output
    }
}

static STATE: LazyLock<Mutex<Reducer>> = LazyLock::new(|| Mutex::new(Reducer::default()));
static START: LazyLock<Instant> = LazyLock::new(Instant::now);

pub(super) fn configure(input: &InputConfig) {
    STATE.lock().configure(settings(input));
    if settings(input).is_some() {
        tracing::info!("Button scrolling: mouse hook enabled (cross-device screen-pixel approximation; no click replay; 500ms lock grace)");
    }
}

pub(super) fn cancel() {
    let mut state = STATE.lock();
    let config = state.config;
    state.configure(config);
}

/// Handle a captured edge before modal/bind dispatch, but do not acquire a new
/// gesture there. Fresh presses are offered only after all existing owners.
pub(super) fn captured_button(button: u8, down: bool, point: POINT) -> bool {
    let mut state = STATE.lock();
    if !state
        .captured
        .get(button as usize)
        .copied()
        .unwrap_or(false)
    {
        return false;
    }
    state.button(
        button,
        down,
        (point.x, point.y),
        START.elapsed().as_millis() as u64,
    )
}

pub(super) fn begin(button: u8, point: POINT) -> bool {
    STATE.lock().button(
        button,
        true,
        (point.x, point.y),
        START.elapsed().as_millis() as u64,
    )
}

pub(super) fn motion(point: POINT) -> bool {
    let Some((x, y)) = STATE.lock().anchor else {
        return false;
    };
    // The real cursor and wheel recipient stay at the anchor. Intercepted
    // candidate positions may cross another window, but must not change its rule.
    let factor = super::rule_scroll::continuous_factor(POINT { x, y });
    let (output, generation) = {
        let mut state = STATE.lock();
        (state.motion((point.x, point.y), factor), state.generation)
    };
    if output.horizontal != 0 || output.vertical != 0 {
        let queued = super::dispatch::send_work(Box::new(move || {
            if STATE.lock().generation != generation {
                return;
            }
            for (delta, horizontal) in [(output.horizontal, true), (output.vertical, false)] {
                if delta != 0 && !super::rule_scroll::inject_wheel(delta, horizontal) {
                    tracing::warn!("Button-scroll wheel injection rejected");
                }
            }
        }));
        if !queued {
            cancel();
            tracing::warn!("Button-scroll dispatch unavailable; gesture cancelled");
        }
    }
    output.consume
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled(lock: bool) -> Reducer {
        let mut state = Reducer::default();
        state.configure(Some(Settings {
            button: 2,
            lock,
            natural: false,
        }));
        state
    }

    #[test]
    fn parity_button_scroll_default_zero_and_unsupported_do_nothing() {
        let mut state = Reducer::default();
        assert!(!state.button(2, true, (20, 30), 0));
        assert!(!state.motion((25, 35), 1.0).consume);
        let mut input = crate::config::InputConfig::default_config();
        input.mouse_scroll.method = Some(crate::config::ScrollMethod::OnButtonDown);
        for button in [None, Some(0), Some(99), Some(277), Some(u32::MAX)] {
            input.mouse_scroll.button = button;
            assert_eq!(settings(&input), None);
        }
    }

    #[test]
    fn parity_button_scroll_maps_evdev_and_warned_legacy_buttons() {
        for (code, expected) in [
            (272, 0),
            (273, 1),
            (274, 2),
            (275, 3),
            (276, 4),
            (1, 0),
            (2, 1),
            (3, 2),
            (4, 3),
            (5, 4),
        ] {
            assert_eq!(map_button(code), Some(expected));
        }
    }

    #[test]
    fn parity_button_scroll_hold_consumes_both_edges_without_click_replay() {
        let mut state = enabled(false);
        assert!(
            !state.button(2, false, (20, 30), 0),
            "orphan release passes"
        );
        assert!(state.button(2, true, (20, 30), 1));
        assert!(!state.button(1, true, (20, 30), 2));
        let output = state.motion((24, 36), 1.0);
        assert_eq!(
            (output.consume, output.horizontal, output.vertical),
            (true, 32, -48)
        );
        assert!(state.button(2, false, (20, 30), 3));
        assert!(!state.motion((24, 36), 1.0).consume);
        assert!(state.button(2, true, (20, 30), 4));
        assert!(state.button(2, false, (20, 30), 5));
    }

    #[test]
    fn parity_button_scroll_frozen_cursor_uses_anchor_for_every_motion() {
        let mut state = enabled(false);
        state.button(2, true, (100, 100), 0);
        assert_eq!(state.motion((104, 100), 1.0).horizontal, 32);
        assert_eq!(state.motion((104, 100), 1.0).horizontal, 32);
        assert_eq!(state.motion((96, 100), 1.0).horizontal, -32);
    }

    #[test]
    fn parity_button_scroll_axis_threshold_and_fractional_wheel_units_accumulate() {
        let mut state = enabled(false);
        state.button(2, true, (0, 0), 0);
        assert_eq!(state.motion((1, 0), 0.0625).horizontal, 0);
        assert_eq!(state.motion((1, 0), 0.0625).horizontal, 0);
        assert_eq!(state.motion((1, 0), 0.0625).horizontal, 1);
        assert_eq!(state.motion((1, 0), 0.0625).horizontal, 1);
        assert_eq!(state.motion((0, 1), 1.0).vertical, 0);
        assert_eq!(state.motion((0, 2), 1.0).vertical, -24);
    }

    #[test]
    fn parity_button_scroll_lock_unlock_and_long_scrolled_hold_grace() {
        let mut state = enabled(true);
        state.button(2, true, (0, 0), 0);
        state.button(2, false, (0, 0), 5);
        assert!(state.motion((4, 0), 1.0).consume);
        state.button(2, true, (0, 0), 10);
        assert!(
            state.motion((4, 0), 1.0).consume,
            "second down keeps logical hold until up"
        );
        state.button(2, false, (0, 0), 11);
        assert!(!state.motion((4, 0), 1.0).consume);
        state.button(2, true, (0, 0), 100);
        state.motion((4, 0), 1.0);
        state.button(2, false, (0, 0), 600);
        assert!(
            !state.motion((4, 0), 1.0).consume,
            "scrolled hold >=500ms must not latch"
        );
        state.button(2, true, (0, 0), 700);
        state.button(2, false, (0, 0), 1500);
        assert!(
            state.motion((4, 0), 1.0).consume,
            "long hold without scrolling still latches"
        );
    }

    #[test]
    fn parity_button_scroll_released_hold_and_unlocked_latch_reject_queued_wheels() {
        let mut hold = enabled(false);
        hold.button(2, true, (10, 20), 0);
        assert_eq!(hold.motion((14, 20), 1.0).horizontal, 32);
        let queued_hold_generation = hold.generation;
        hold.button(2, false, (10, 20), 1);
        assert_ne!(
            hold.generation, queued_hold_generation,
            "queued wheel must become stale when its held recipient is released"
        );

        let mut lock = enabled(true);
        lock.button(2, true, (10, 20), 0);
        lock.button(2, false, (10, 20), 1);
        assert_eq!(lock.motion((14, 20), 1.0).horizontal, 32);
        let queued_lock_generation = lock.generation;
        lock.button(2, true, (10, 20), 2);
        lock.button(2, false, (10, 20), 3);
        assert_ne!(
            lock.generation, queued_lock_generation,
            "queued wheel must become stale after unlocking"
        );
    }

    #[test]
    fn parity_button_scroll_reload_resets_latch_residual_anchor_but_pairs_old_up() {
        let mut state = enabled(true);
        state.button(2, true, (10, 20), 0);
        state.motion((14, 20), 0.01);
        state.configure(None);
        assert!(!state.motion((14, 20), 1.0).consume);
        assert!(state.button(2, false, (10, 20), 1));
        assert!(!state.button(2, false, (10, 20), 2));
        state.configure(Some(Settings {
            button: 2,
            lock: true,
            natural: true,
        }));
        state.button(2, true, (100, 100), 3);
        let output = state.motion((104, 106), 1.0);
        assert_eq!((output.horizontal, output.vertical), (-32, 48));
    }
}
