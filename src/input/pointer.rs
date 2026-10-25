//! Bindable pointer triggers. Windows reports precision-touchpad scrolling through the
//! same wheel stream, so TouchpadScroll names are aliases, not a device identity claim.
use super::{
    keyboard::{bind_may_fire, BindOptions},
    Action,
};
use crate::config::Config;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trigger {
    Button(u8),
    Wheel(bool, bool),
} // horizontal, positive
pub fn parse_trigger(name: &str) -> Option<Trigger> {
    Some(match name.to_ascii_lowercase().as_str() {
        "mouseleft" => Trigger::Button(0),
        "mouseright" => Trigger::Button(1),
        "mousemiddle" => Trigger::Button(2),
        "mouseback" => Trigger::Button(3),
        "mouseforward" => Trigger::Button(4),
        "wheelscrollup" | "touchpadscrollup" => Trigger::Wheel(false, true),
        "wheelscrolldown" | "touchpadscrolldown" => Trigger::Wheel(false, false),
        "wheelscrollleft" | "touchpadscrollleft" => Trigger::Wheel(true, false),
        "wheelscrollright" | "touchpadscrollright" => Trigger::Wheel(true, true),
        _ => return None,
    })
}
#[derive(Clone)]
struct Bind {
    action: Action,
    options: BindOptions,
    last: Option<Instant>,
}
impl Bind {
    fn fire(&mut self, now: Instant) -> Option<Action> {
        if !bind_may_fire(&self.options, false, self.last, now) {
            return None;
        }
        self.last = Some(now);
        Some(self.action.clone())
    }
}
#[derive(Default)]
pub struct PointerEngine {
    binds: HashMap<(u32, Trigger), Bind>,
    captured: HashMap<u8, ((u32, Trigger), Bind)>,
    down: [bool; 5],
    gesture: [bool; 5],
    residual: [i32; 2],
    residual_mods: Option<u32>,
    pub mod_key: u32,
    natural: bool,
}
impl PointerEngine {
    pub fn configure(&mut self, config: &Config) {
        self.mod_key =
            super::mod_key_flags(&config.input.mod_key).unwrap_or(super::DEFAULT_MOD_KEY);
        self.natural = config.input.natural_scroll;
        self.binds.clear();
        self.residual = [0; 2];
        self.residual_mods = None;
        for b in &config.binds.hotkeys {
            let (Some(trigger), Ok(mods), Some(action)) = (
                parse_trigger(&b.key),
                b.resolve_modifiers(self.mod_key),
                b.parse_action(),
            ) else {
                continue;
            };
            self.binds.insert(
                (mods, trigger),
                Bind {
                    action,
                    options: BindOptions {
                        no_repeat: !b.repeat,
                        release: b.release,
                        allow_inhibiting: b.allow_inhibiting,
                        cooldown: b.cooldown_ms.map(|ms| Duration::from_millis(ms as u64)),
                        title: b.hotkey_overlay_title.clone(),
                    },
                    last: None,
                },
            );
        }
    }
    pub fn capture_gesture(&mut self, button: u8) {
        self.gesture[button as usize] = true;
    }
    /// Paired suppression survives modifier-first release and reload.
    pub fn button(
        &mut self,
        mods: u32,
        button: u8,
        pressed: bool,
        now: Instant,
    ) -> (bool, Vec<Action>) {
        let Some(down) = self.down.get_mut(button as usize) else {
            return (false, vec![]);
        };
        let repeated = *down;
        *down = pressed;
        if !pressed {
            if std::mem::take(&mut self.gesture[button as usize]) {
                return (true, vec![]);
            }
            if let Some((key, mut b)) = self.captured.remove(&button) {
                let a = if b.options.release { b.fire(now) } else { None };
                if let Some(current) = self.binds.get_mut(&key) {
                    current.last = b.last;
                }
                return (true, a.into_iter().collect());
            }
        } else if self.captured.contains_key(&button) {
            return (true, vec![]);
        } else if !repeated {
            let key = (mods, Trigger::Button(button));
            if let Some(b) = self.binds.get_mut(&key) {
                let a = if b.options.release { None } else { b.fire(now) };
                self.captured.insert(button, (key, b.clone()));
                return (true, a.into_iter().collect());
            }
        }
        (false, vec![])
    }
    /// Consume both directions only on a bound axis, including sub-tick deltas.
    pub fn wheel(
        &mut self,
        mods: u32,
        horizontal: bool,
        delta: i32,
        now: Instant,
    ) -> (bool, Vec<Action>) {
        let axis = horizontal as usize;
        if self.residual_mods != Some(mods) {
            self.residual = [0; 2];
            self.residual_mods = Some(mods);
        }
        if ![true, false].iter().any(|p| {
            self.binds
                .contains_key(&(mods, Trigger::Wheel(horizontal, *p)))
        }) {
            self.residual[axis] = 0;
            return (false, vec![]);
        }
        let delta = if self.natural { -delta } else { delta };
        self.residual[axis] = self.residual[axis].saturating_add(delta);
        let ticks = self.residual[axis] / 120;
        self.residual[axis] %= 120;
        let mut actions = Vec::new();
        if let Some(b) = self
            .binds
            .get_mut(&(mods, Trigger::Wheel(horizontal, ticks > 0)))
        {
            // Physical wheel packets are bounded by the Win32 signed 16-bit delta.
            for _ in 0..ticks.unsigned_abs().min(273) {
                if let Some(a) = b.fire(now) {
                    actions.push(a);
                }
            }
        }
        (true, actions)
    }
}
pub static ENGINE: LazyLock<Mutex<PointerEngine>> =
    LazyLock::new(|| Mutex::new(PointerEngine::default()));

#[cfg(test)]
mod tests {
    use super::*;
    fn engine(text: &str) -> PointerEngine {
        let d = crate::config::decode::decode_text(text, "", None);
        assert!(!d.has_errors(), "{:?}", d.issues);
        let mut e = PointerEngine::default();
        e.configure(&d.config);
        e
    }
    #[test]
    fn unbound_mod_wheel_always_passes() {
        let mut e = engine("");
        let t = Instant::now();
        for mods in [0, 1, 3, 8] {
            for axis in [false, true] {
                assert_eq!(e.wheel(mods, axis, 120, t), (false, vec![]));
            }
        }
    }
    #[test]
    fn gesture_up_stays_captured_after_cancel_reload_and_mod_release() {
        let mut e = engine("");
        let t = Instant::now();
        e.button(3, 0, true, t);
        e.capture_gesture(0);
        e.configure(&Config::default());
        assert_eq!(e.button(0, 0, false, t), (true, vec![]));
        assert_eq!(e.button(0, 0, true, t), (false, vec![]));
        assert_eq!(e.button(0, 0, false, t), (false, vec![]));
    }
    #[test]
    fn all_trigger_names_and_touchpad_aliases() {
        for name in [
            "MouseLeft",
            "MouseRight",
            "MouseMiddle",
            "MouseBack",
            "MouseForward",
            "WheelScrollUp",
            "WheelScrollDown",
            "WheelScrollLeft",
            "WheelScrollRight",
            "TouchpadScrollUp",
            "TouchpadScrollDown",
            "TouchpadScrollLeft",
            "TouchpadScrollRight",
        ] {
            assert!(parse_trigger(name).is_some(), "{name}");
        }
        assert_eq!(parse_trigger("F9"), None);
    }
    #[test]
    fn buttons_override_gestures_and_pair_release_after_modifier_up() {
        let mut e = engine("binds { Mod+MouseLeft { refresh; } }");
        let t = Instant::now();
        assert_eq!(e.button(3, 0, true, t), (true, vec![Action::Refresh]));
        assert_eq!(e.button(0, 0, false, t), (true, vec![]));
        assert_eq!(e.button(0, 0, true, t), (false, vec![]));
        assert_eq!(e.button(0, 0, false, t), (false, vec![]));
    }
    #[test]
    fn scroll_axis_consumption_and_fractional_accumulation() {
        let mut e = engine("binds { extend-defaults false; Ctrl+WheelScrollUp { refresh; } }");
        let t = Instant::now();
        assert_eq!(e.wheel(2, true, 120, t), (false, vec![]));
        assert_eq!(e.wheel(2, false, 40, t), (true, vec![]));
        assert_eq!(e.wheel(2, false, 40, t), (true, vec![]));
        assert_eq!(e.wheel(2, false, 40, t), (true, vec![Action::Refresh]));
        assert_eq!(e.wheel(2, false, -120, t), (true, vec![]));
        assert_eq!(e.wheel(0, false, 120, t), (false, vec![]));
    }
    #[test]
    fn wheel_cooldown_natural_direction_and_mod_reset() {
        let mut e=engine("input { mouse { natural-scroll; }; }\nbinds { extend-defaults false; Ctrl+WheelScrollUp cooldown-ms=500 { refresh; } }");
        let t = Instant::now();
        assert_eq!(e.wheel(2, false, -120, t), (true, vec![Action::Refresh]));
        assert_eq!(e.wheel(2, false, -120, t), (true, vec![]));
        assert_eq!(
            e.wheel(2, false, -120, t + Duration::from_millis(500)),
            (true, vec![Action::Refresh])
        );
        e.wheel(2, false, -60, t);
        e.wheel(0, false, -60, t);
        assert!(e.wheel(2, false, -60, t).1.is_empty());
    }
    #[test]
    fn button_release_binding_and_reload_pairing() {
        let mut e = engine("binds { Ctrl+MouseMiddle on-release=true { refresh; } }");
        let t = Instant::now();
        assert_eq!(e.button(2, 2, true, t), (true, vec![]));
        e.configure(&Config::default());
        assert_eq!(e.button(0, 2, false, t), (true, vec![Action::Refresh]));
    }
}
