//! Application wheel scaling. Snapshots arrive only on admission/rule/config changes;
//! the existing LL hook scales only an unbound event and queues a tagged reinjection.
//! SendInput runs after returning from the hook, on the existing input message pump.
//! No engine lock, timer, worker, or device/system setting is needed.
use std::collections::HashMap;
use std::sync::LazyLock;

use parking_lot::Mutex;
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_WHEEL, MOUSEINPUT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, WindowFromPoint, GA_ROOT};

use crate::config::InputConfig;
use crate::utils::WindowId;
use crate::window::rule_policy::ScrollAccumulator;

pub(super) const REINJECTED: usize = 0x5749_5257;

struct State {
    factors: HashMap<WindowId, f64>,
    device: (f64, f64),
    residual: ScrollAccumulator,
}
impl Default for State {
    fn default() -> Self {
        Self {
            factors: HashMap::new(),
            device: (1.0, 1.0),
            residual: ScrollAccumulator::default(),
        }
    }
}

impl State {
    fn configure(&mut self, input: &InputConfig) {
        let (horizontal, vertical) = input
            .mouse_scroll_factor
            .map(|factor| factor.h_v_factors())
            .unwrap_or((input.mouse_speed, input.mouse_speed));
        // libinput's mouse natural-scroll setting reverses both application
        // axes before device/rule factors. Keep the mouse origin: a later
        // touchpad block can overwrite the legacy merged natural_scroll field.
        let direction = if input.mouse_scroll.natural {
            -1.0
        } else {
            1.0
        };
        self.device = (horizontal * direction, vertical * direction);
        self.residual.reset();
    }

    fn factor(&self, window: WindowId, physical_wheel: bool, horizontal: bool) -> f64 {
        self.factors.get(&window).copied().unwrap_or(1.0)
            * if !physical_wheel {
                1.0
            } else if horizontal {
                self.device.0
            } else {
                self.device.1
            }
    }
}
static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

pub(crate) fn configure(input: &InputConfig) {
    STATE.lock().configure(input);
    if input.touchpad_scroll_factor.is_some() {
        tracing::info!("touchpad scroll-factor stored but unapplied: WH_MOUSE_LL cannot identify finger-source scrolling; mouse factors apply to the shared Windows wheel stream");
    }
}

pub(crate) fn update(window: WindowId, factor: Option<f64>) {
    let mut state = STATE.lock();
    let factor = factor.filter(|f| *f != 1.0);
    if state.factors.get(&window).copied() == factor {
        return;
    }
    match factor {
        Some(factor) => {
            state.factors.insert(window, factor);
        }
        None => {
            state.factors.remove(&window);
        }
    }
    state.residual.reset();
}

/// True means the original event was replaced (or deliberately suppressed).
/// The caller must first offer the event to pointer binds and skip our tagged events.
pub(super) fn wheel(point: POINT, horizontal: bool, delta: i32) -> bool {
    let hwnd = unsafe { GetAncestor(WindowFromPoint(point), GA_ROOT) };
    if hwnd.0.is_null() {
        return false;
    }
    let window = WindowId::new(hwnd.0 as isize);
    let output = {
        let mut state = STATE.lock();
        let factor = state.factor(window, true, horizontal);
        if !factor.is_finite() {
            state.residual.reset();
            return false;
        }
        let output = state.residual.scale(window, horizontal, delta, factor);
        if factor == 1.0 {
            return false;
        }
        output
    };
    if output == 0 {
        return true;
    }
    // Never call SendInput synchronously inside an LL hook: re-entrant delivery
    // can let both the original and replacement through. Consume only after the
    // existing message pump accepted the task, then inject after this callback.
    super::dispatch::send_work(Box::new(move || {
        if !inject_wheel(output, horizontal) && !inject_wheel(delta, horizontal) {
            // Async failure cannot pass the original through, so try one tagged
            // original-delta fallback. UIPI can reject both; do not retry/spin.
            tracing::warn!("Application wheel replacement and fallback were rejected");
        }
    }))
}

/// Continuous button motion bypasses the physical-wheel device multiplier.
/// Window rules still apply to the resulting application scroll stream.
pub(super) fn continuous_factor(point: POINT) -> f64 {
    let hwnd = unsafe { GetAncestor(WindowFromPoint(point), GA_ROOT) };
    STATE
        .lock()
        .factor(WindowId::new(hwnd.0 as isize), false, false)
}

pub(super) fn inject_wheel(delta: i32, horizontal: bool) -> bool {
    let events = wheel_inputs(delta, horizontal);
    // On partial delivery do not duplicate the delivered portion by also
    // injecting a fallback. A total failure permits one original-delta attempt.
    unsafe { SendInput(&events, std::mem::size_of::<INPUT>() as i32) > 0 }
}

fn wheel_inputs(delta: i32, horizontal: bool) -> Vec<INPUT> {
    // WM_MOUSE[H]WHEEL exposes a signed 16-bit delta. Preserve larger scaled
    // amounts in one SendInput batch instead of silently wrapping at the app.
    wheel_deltas(delta)
        .into_iter()
        .map(|part| INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    mouseData: part as u32,
                    dwFlags: if horizontal {
                        MOUSEEVENTF_HWHEEL
                    } else {
                        MOUSEEVENTF_WHEEL
                    },
                    dwExtraInfo: REINJECTED,
                    ..Default::default()
                },
            },
        })
        .collect()
}

fn wheel_deltas(mut delta: i32) -> Vec<i32> {
    let mut parts = Vec::new();
    while delta != 0 {
        let part = delta.clamp(i16::MIN as i32, i16::MAX as i32);
        parts.push(part);
        delta -= part;
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured_state(input_text: &str) -> State {
        let decoded = crate::config::decode::decode_text(input_text, "", None);
        assert!(decoded.issues.is_empty(), "{:?}", decoded.issues);
        let mut state = State::default();
        state.configure(&decoded.config.input);
        state
    }

    #[test]
    fn parity_axis_scroll_factor_consumer_composes_signed_axes_with_window_rule() {
        let mut state = configured_state("input { mouse { scroll-factor 2 vertical=-1 } }");
        let target = WindowId::new(123);
        state.factors.insert(target, 3.0);
        assert_eq!(state.factor(target, true, true), 6.0);
        assert_eq!(state.factor(target, true, false), -3.0);
        assert_eq!(state.factor(target, false, true), 3.0);
        assert_eq!(state.factor(target, false, false), 3.0);
        assert_eq!(state.factor(WindowId::new(456), true, true), 2.0);
        assert_eq!(state.factor(WindowId::new(456), true, false), -1.0);
    }

    #[test]
    fn parity_axis_scroll_factor_consumer_zero_axis_preserves_other_axis() {
        let mut state =
            configured_state("input { mouse { scroll-factor horizontal=0 vertical=0.25 } }");
        let target = WindowId::new(1);
        let h = state.factor(target, true, true);
        let v = state.factor(target, true, false);
        assert_eq!(state.residual.scale(target, true, 120, h), 0);
        assert_eq!(state.residual.scale(target, false, 120, v), 30);
    }

    #[test]
    fn parity_axis_scroll_factor_consumer_touchpad_only_leaves_shared_mouse_stream_unscaled() {
        let state = configured_state("input { touchpad { scroll-factor 4 horizontal=-3 } }");
        assert_eq!(state.factor(WindowId::new(1), true, true), 1.0);
        assert_eq!(state.factor(WindowId::new(1), true, false), 1.0);
    }

    #[test]
    fn parity_axis_scroll_factor_consumer_legacy_scalar_is_retained_without_niri_factor() {
        let state = configured_state("input { mouse { scroll-speed 2.5 } }");
        assert_eq!(state.factor(WindowId::new(1), true, true), 2.5);
        assert_eq!(state.factor(WindowId::new(1), true, false), 2.5);
    }

    #[test]
    fn parity_axis_scroll_factor_natural_reverses_both_application_axes() {
        let mut state =
            configured_state("input { mouse { natural-scroll true; scroll-factor 0.5; } }");
        let target = WindowId::new(1);
        for horizontal in [false, true] {
            let factor = state.factor(target, true, horizontal);
            assert_eq!(state.residual.scale(target, horizontal, 120, factor), -60);
            assert_eq!(state.residual.scale(target, horizontal, -120, factor), 60);
        }
    }

    #[test]
    fn parity_axis_scroll_factor_natural_composes_signed_zero_axes_and_window_rule() {
        for (axes, expected_h, expected_v) in [
            ("horizontal=-0.5 vertical=0.25", 180, -90),
            ("horizontal=-0.5 vertical=0", 180, 0),
            ("horizontal=0 vertical=-0.25", 0, 90),
        ] {
            let mut state = configured_state(&format!(
                "input {{ mouse {{ natural-scroll true; scroll-factor {axes}; }} }}"
            ));
            let target = WindowId::new(1);
            state.factors.insert(target, 3.0);
            let horizontal = state.factor(target, true, true);
            let vertical = state.factor(target, true, false);
            assert_eq!(
                state.residual.scale(target, true, 120, horizontal),
                expected_h,
                "{axes}"
            );
            assert_eq!(
                state.residual.scale(target, false, 120, vertical),
                expected_v,
                "{axes}"
            );
        }
    }

    #[test]
    fn parity_axis_scroll_factor_natural_device_composition_bypasses_button_scroll() {
        let mut state = configured_state(
            "input { mouse { natural-scroll true; scroll-factor horizontal=-0.5 vertical=0; } }",
        );
        let target = WindowId::new(1);
        state.factors.insert(target, 3.0);
        assert_eq!(state.factor(target, false, true), 3.0);
        assert_eq!(state.factor(target, false, false), 3.0);
        assert_eq!(state.factor(WindowId::new(2), false, true), 1.0);
        assert_eq!(state.factor(WindowId::new(2), false, false), 1.0);
    }

    #[test]
    fn parity_axis_scroll_factor_natural_uses_mouse_origin_in_both_block_orders() {
        for text in [
            "input { mouse { natural-scroll true; scroll-factor 0.5; }; touchpad { natural-scroll false; }; }",
            "input { touchpad { natural-scroll false; }; mouse { natural-scroll true; scroll-factor 0.5; }; }",
        ] {
            let state = configured_state(text);
            assert_eq!(state.factor(WindowId::new(1), true, true), -0.5);
            assert_eq!(state.factor(WindowId::new(1), true, false), -0.5);
        }
        for text in [
            "input { mouse { scroll-factor 0.5; }; touchpad { natural-scroll true; }; }",
            "input { touchpad { natural-scroll true; }; mouse { scroll-factor 0.5; }; }",
        ] {
            let state = configured_state(text);
            assert_eq!(state.factor(WindowId::new(1), true, true), 0.5);
            assert_eq!(state.factor(WindowId::new(1), true, false), 0.5);
        }
        let state = configured_state("input { touchpad { natural-scroll true; }; }");
        assert_eq!(state.factor(WindowId::new(1), true, true), 1.0);
        assert_eq!(state.factor(WindowId::new(1), true, false), 1.0);
    }

    #[test]
    fn parity_axis_scroll_factor_natural_composes_legacy_scalar_fallback() {
        let mut state =
            configured_state("input { mouse { natural-scroll true; scroll-speed 2.5; } }");
        let target = WindowId::new(1);
        for horizontal in [false, true] {
            let factor = state.factor(target, true, horizontal);
            assert_eq!(state.residual.scale(target, horizontal, 120, factor), -300);
        }
    }

    #[test]
    fn parity_axis_scroll_factor_negative_axis_reverses_application_wheel() {
        let mut scroll = ScrollAccumulator::default();
        assert_eq!(scroll.scale(WindowId::new(1), true, 120, -0.5), -60);
        assert_eq!(scroll.scale(WindowId::new(1), false, -120, -0.5), 60);
    }

    #[test]
    fn parity_axis_scroll_factor_unequal_axes_preserve_interleaved_fractions() {
        let mut scroll = ScrollAccumulator::default();
        // Four horizontal quarter-units and two vertical half-units each
        // independently make one wheel unit, even when their factors differ.
        let observed: Vec<_> = [
            (true, 0.25),
            (false, 0.5),
            (true, 0.25),
            (false, 0.5),
            (true, 0.25),
            (true, 0.25),
        ]
        .into_iter()
        .map(|(horizontal, factor)| scroll.scale(WindowId::new(1), horizontal, 1, factor))
        .collect();
        assert_eq!(observed, vec![0, 0, 0, 1, 0, 1]);
    }

    #[test]
    fn parity_button_scroll_continuous_factor_keeps_window_rule_and_bypasses_wheel_device_factor() {
        let mut state = State {
            device: (9.0, 9.0),
            ..State::default()
        };
        state.factors.insert(WindowId::new(123), 0.25);
        assert_eq!(state.factor(WindowId::new(123), false, false), 0.25);
        assert_eq!(state.factor(WindowId::new(123), true, false), 2.25);
        assert_eq!(state.factor(WindowId::new(456), false, false), 1.0);
    }

    #[test]
    fn replacement_batches_preserve_axes_signs_and_bypass_tag() {
        for horizontal in [false, true] {
            for delta in [0, 120, -240, 40_000, -40_000] {
                let inputs = wheel_inputs(delta, horizontal);
                let delivered: i32 = inputs
                    .iter()
                    .map(|input| {
                        assert_eq!(input.r#type, INPUT_MOUSE);
                        let mouse = unsafe { input.Anonymous.mi };
                        assert_eq!(mouse.dwExtraInfo, REINJECTED);
                        assert_eq!(mouse.dx, 0);
                        assert_eq!(mouse.dy, 0);
                        assert_eq!(
                            mouse.dwFlags,
                            if horizontal {
                                MOUSEEVENTF_HWHEEL
                            } else {
                                MOUSEEVENTF_WHEEL
                            }
                        );
                        mouse.mouseData as i16 as i32
                    })
                    .sum();
                assert_eq!(delivered, delta);
            }
        }
    }
    #[test]
    fn large_scaled_wheels_preserve_signed_sixteen_bit_delivery() {
        for delta in [0, 120, -120, 40_000, -40_000, 120_000, -120_000] {
            let parts = wheel_deltas(delta);
            assert_eq!(parts.iter().sum::<i32>(), delta);
            assert!(parts
                .iter()
                .all(|part| (i16::MIN as i32..=i16::MAX as i32).contains(part) && *part != 0));
        }
    }
}
