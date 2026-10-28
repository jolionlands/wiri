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

use crate::utils::WindowId;
use crate::window::rule_policy::ScrollAccumulator;

pub(super) const REINJECTED: usize = 0x5749_5257;

struct State {
    factors: HashMap<WindowId, f64>,
    device: f64,
    residual: ScrollAccumulator,
}
impl Default for State {
    fn default() -> Self {
        Self {
            factors: HashMap::new(),
            device: 1.0,
            residual: ScrollAccumulator::default(),
        }
    }
}
static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

pub(crate) fn configure(device: f64) {
    let mut state = STATE.lock();
    state.device = device;
    state.residual.reset();
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
        let factor = state.factors.get(&window).copied().unwrap_or(1.0) * state.device;
        if !factor.is_finite() || factor < 0.0 {
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

fn inject_wheel(delta: i32, horizontal: bool) -> bool {
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
