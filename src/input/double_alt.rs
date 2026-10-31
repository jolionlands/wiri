//! Legacy double-Alt overview gesture; evaluated on input only, never by a timer.
use std::time::{Duration, Instant};

use super::keyboard::Decision;
use super::{Action, MOD_ALT};

#[derive(Default)]
pub struct DoubleAlt {
    first: Option<Instant>,
    held: u8,
    swallowed: u8,
}
impl DoubleAlt {
    pub const fn new() -> Self {
        Self {
            first: None,
            held: 0,
            swallowed: 0,
        }
    }
    pub fn event(
        &mut self,
        vk: u32,
        down: bool,
        mods: u32,
        now: Instant,
        allowed: bool,
    ) -> Decision {
        let side = match vk {
            0x12 => 1,
            0xA4 => 2,
            0xA5 => 4,
            _ => 0,
        };
        if !allowed || (down && side == 0) || mods & !MOD_ALT != 0 {
            self.first = None;
        }
        if side == 0 {
            return Decision::default();
        }
        let was_held = self.held != 0;
        if down {
            self.held |= side;
        } else {
            self.held &= !side;
        }
        if self.swallowed & side != 0 {
            if !down {
                self.swallowed &= !side;
            }
            return Decision {
                consume: true,
                ..Default::default()
            };
        }
        if !down || was_held || !allowed || mods & !MOD_ALT != 0 {
            return Decision::default();
        }
        if self
            .first
            .take()
            .is_some_and(|first| now.saturating_duration_since(first) < Duration::from_millis(250))
        {
            self.swallowed |= side;
            Decision {
                consume: true,
                mask_menu: true,
                action: Some(Action::OverviewToggle),
                chorded: false,
            }
        } else {
            self.first = Some(now);
            Decision::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn second_tap_repeat_and_up_are_paired() {
        let mut e = DoubleAlt::default();
        let t = Instant::now();
        assert!(!e.event(0xA4, true, MOD_ALT, t, true).consume);
        assert!(!e.event(0xA4, false, 0, t, true).consume);
        let second = e.event(0xA4, true, MOD_ALT, t + Duration::from_millis(100), true);
        assert_eq!(second.action, Some(Action::OverviewToggle));
        assert!(second.mask_menu && second.consume);
        assert_eq!(e.event(0xA4, true, MOD_ALT, t, true).action, None);
        assert!(e.event(0xA4, false, 0, t, true).consume);
        assert!(!e.event(0xA4, false, 0, t, true).consume);
    }
    #[test]
    fn holds_chords_modal_input_and_timeout_break_chain() {
        for mode in 0..4 {
            let mut e = DoubleAlt::default();
            let t = Instant::now();
            e.event(0xA4, true, MOD_ALT, t, true);
            if mode == 0 {
                assert!(e.event(0xA4, true, MOD_ALT, t, true).action.is_none());
            }
            e.event(0xA4, false, 0, t, true);
            match mode {
                0 => {
                    e.event(0x41, true, 0, t, true);
                }
                1 => {
                    e.event(0xA4, true, MOD_ALT | 2, t, true);
                }
                2 => {
                    e.event(0xA4, true, MOD_ALT, t, false);
                }
                _ => {}
            }
            assert!(e
                .event(0xA5, true, MOD_ALT, t + Duration::from_millis(300), true)
                .action
                .is_none());
        }
    }
}
