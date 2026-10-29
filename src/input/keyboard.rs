//! Primary keyboard binding state machine, independent of Win32. Modifiers always pass
//! through. A bound key's down/repeats/up are consumed as a pair, even across reloads,
//! inhibition, or releasing its modifiers first. Unbound downs are never captured later.
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use super::Action;

#[derive(Debug, Clone)]
pub struct BindOptions {
    pub no_repeat: bool,
    pub release: bool,
    pub allow_inhibiting: bool,
    pub cooldown: Option<Duration>,
    pub title: Option<Option<String>>,
    /// The config chord contained literal `Mod`, before resolving its Win32 mask.
    pub overlay_uses_compositor_mod: bool,
}
impl Default for BindOptions {
    fn default() -> Self {
        Self {
            no_repeat: false,
            release: false,
            allow_inhibiting: true,
            cooldown: None,
            title: None,
            overlay_uses_compositor_mod: false,
        }
    }
}
pub fn bind_may_fire(
    options: &BindOptions,
    repeat: bool,
    last: Option<Instant>,
    now: Instant,
) -> bool {
    !(options.no_repeat && repeat)
        && !matches!((options.cooldown, last), (Some(d), Some(t)) if now.saturating_duration_since(t) < d)
}

#[derive(Clone)]
struct Binding {
    action: Action,
    options: BindOptions,
    last_fire: Option<Instant>,
}
impl Binding {
    fn fire(&mut self, repeat: bool, now: Instant) -> Option<Action> {
        if !bind_may_fire(&self.options, repeat, self.last_fire, now) {
            return None;
        }
        self.last_fire = Some(now);
        Some(self.action.clone())
    }
}
#[derive(Default)]
pub struct Decision {
    pub consume: bool,
    pub mask_menu: bool,
    pub action: Option<Action>,
}

pub fn modifier(vk: u32) -> u32 {
    match vk {
        0x12 | 0xA4 | 0xA5 => 1,
        0x11 | 0xA2 | 0xA3 => 2,
        0x10 | 0xA0 | 0xA1 => 4,
        0x5B | 0x5C => 8,
        _ => 0,
    }
}
/// Security/system chords stay with Windows in both the primary hook and fallback.
pub fn system_critical(mods: u32, vk: u32) -> bool {
    matches!((mods, vk), (8, 0x4C | 0x44) | (12, 0x53))
}

pub struct KeyboardEngine {
    bindings: HashMap<(u32, u32), Binding>,
    down: [bool; 256],
    // Snapshot on the first down: release binds don't depend on modifiers still held.
    captured: HashMap<u32, ((u32, u32), Binding)>,
    inhibited: bool,
    /// niri's Latin-key fallback, consulted only when the observed key names no bind.
    latin: Option<std::sync::Arc<dyn super::keylayout::LatinFallback>>,
}
impl Default for KeyboardEngine {
    fn default() -> Self {
        Self {
            bindings: HashMap::new(),
            down: [false; 256],
            captured: HashMap::new(),
            inhibited: false,
            latin: None,
        }
    }
}
impl KeyboardEngine {
    /// Install (or clear) the Latin-key fallback.  Called at startup and on every hotkey
    /// reload; never per keystroke.
    pub fn set_latin_fallback(
        &mut self,
        fallback: Option<std::sync::Arc<dyn super::keylayout::LatinFallback>>,
    ) {
        self.latin = fallback;
    }

    /// The bind an observed `(modifiers, virtual key)` names.
    ///
    /// Windows virtual keys are layout-independent for the letters on almost every layout,
    /// so the key itself usually matches.  When it does not, the same physical key's Latin
    /// (US) virtual key is tried — niri's Latin-key fallback (wiki:Configuration:-Key-Bindings).
    fn bound_key(&self, mods: u32, vk: u32) -> Option<(u32, u32)> {
        if self.bindings.contains_key(&(mods, vk)) {
            return Some((mods, vk));
        }
        let latin = self.latin.as_ref()?.latin_key(vk)?;
        (latin != vk && self.bindings.contains_key(&(mods, latin))).then_some((mods, latin))
    }
    pub fn configure(
        &mut self,
        rows: &[(u32, u32, Action)],
        options: &HashMap<(u32, u32), BindOptions>,
    ) {
        self.bindings = rows
            .iter()
            .filter(|(m, v, a)| {
                !system_critical(*m, *v)
                    && !(*m == 0
                        && *v == 0x1B
                        && matches!(a, Action::OverviewToggle | Action::OverviewExit))
            })
            .map(|(m, v, a)| {
                (
                    (*m, *v),
                    Binding {
                        action: a.clone(),
                        options: options.get(&(*m, *v)).cloned().unwrap_or_default(),
                        last_fire: None,
                    },
                )
            })
            .collect();
        // Keep down/captured: reloading with a held key must never leak an unmatched up.
    }
    pub fn modifiers(&self) -> u32 {
        super::keynames::modifiers_from_pressed(&self.down)
    }
    pub fn inhibited(&self) -> bool {
        self.inhibited
    }
    pub fn toggle_inhibit(&mut self) {
        self.inhibited = !self.inhibited;
    }
    pub fn active(&self, key: (u32, u32)) -> bool {
        self.bindings.get(&key).is_some_and(|b| {
            !self.inhibited
                || !b.options.allow_inhibiting
                || b.action == Action::ToggleKeyboardShortcutsInhibit
        })
    }
    pub fn fallback_allowed(&self, key: (u32, u32)) -> bool {
        super::keynames::register_hotkey_modifiers(key.0).is_some()
            && self.active(key)
            && self.bindings.get(&key).is_some_and(|b| !b.options.release)
    }
    /// A higher-priority modal surface owns dispatch, but physical state must
    /// still advance. Retire captured releases without firing release binds.
    /// Returns whether this engine already owns the key's down/up pair.
    pub fn modal_event(&mut self, vk: u32, pressed: bool) -> bool {
        let Some(slot) = self.down.get_mut(vk as usize) else {
            return false;
        };
        *slot = pressed;
        if pressed {
            self.captured.contains_key(&vk)
        } else {
            self.captured.remove(&vk).is_some()
        }
    }
    pub fn event(&mut self, vk: u32, pressed: bool, now: Instant) -> Decision {
        let Some(slot) = self.down.get_mut(vk as usize) else {
            return Decision::default();
        };
        let repeat = *slot;
        *slot = pressed;
        if modifier(vk) != 0 {
            return Decision::default();
        }
        if !pressed {
            if let Some((key, mut b)) = self.captured.remove(&vk) {
                let action = if b.options.release
                    && (!self.inhibited
                        || !b.options.allow_inhibiting
                        || b.action == Action::ToggleKeyboardShortcutsInhibit)
                {
                    b.fire(false, now)
                } else {
                    None
                };
                if let Some(current) = self.bindings.get_mut(&key) {
                    current.last_fire = b.last_fire;
                }
                return Decision {
                    consume: true,
                    action,
                    mask_menu: false,
                };
            }
            return Decision::default();
        }
        if let Some((_, b)) = self.captured.get_mut(&vk) {
            let action = if !b.options.release
                && (!self.inhibited
                    || !b.options.allow_inhibiting
                    || b.action == Action::ToggleKeyboardShortcutsInhibit)
            {
                b.fire(true, now)
            } else {
                None
            };
            return Decision {
                consume: true,
                action,
                mask_menu: false,
            };
        }
        // Do not steal an auto-repeat whose first down reached the application.
        if repeat {
            return Decision::default();
        }
        let mods = self.modifiers();
        let Some(key) = self.bound_key(mods, vk) else {
            return Decision::default();
        };
        if !self.active(key) {
            return Decision::default();
        }
        let Some(b) = self.bindings.get_mut(&key) else {
            return Decision::default();
        };
        let action = if b.options.release {
            None
        } else {
            b.fire(false, now)
        };
        self.captured.insert(vk, (key, b.clone()));
        Decision {
            consume: true,
            action,
            mask_menu: mods & (1 | 8) != 0,
        }
    }
    /// RegisterHotKey is only a down fallback when the LL hook cannot observe input.
    pub fn fallback(&mut self, key: (u32, u32), repeat: bool, now: Instant) -> Option<Action> {
        if !self.fallback_allowed(key) {
            return None;
        }
        self.bindings.get_mut(&key)?.fire(repeat, now)
    }
}
pub static ENGINE: LazyLock<Mutex<KeyboardEngine>> =
    LazyLock::new(|| Mutex::new(KeyboardEngine::default()));

/// Install the Latin-key fallback for the layout that is active now.
///
/// Called at startup and on every hotkey reload — never per keystroke — so the hook only
/// ever sees a cached virtual-key mapping.
pub fn configure_latin_fallback() {
    ENGINE
        .lock()
        .set_latin_fallback(super::keylayout::system_fallback());
}

#[cfg(test)]
mod tests {
    use super::super::MOD_CTRL;
    use super::*;
    fn engine(options: BindOptions) -> KeyboardEngine {
        let mut e = KeyboardEngine::default();
        e.configure(
            &[(8, 0x54, Action::Refresh)],
            &HashMap::from([((8, 0x54), options)]),
        );
        e
    }
    #[test]
    fn altgr_matches_ctrl_right_alt_but_not_ctrl_left_alt_or_right_alt_alone() {
        use super::super::{keynames::MOD_ALTGR, MOD_ALT, MOD_CTRL};
        let t = Instant::now();
        for (ctrl, alt, expected_mods, expected_action) in [
            (Some(0xA2), 0xA5, MOD_ALTGR, Some(Action::Refresh)),
            (Some(0xA3), 0xA5, MOD_ALTGR, Some(Action::Refresh)),
            (Some(0xA2), 0xA4, MOD_CTRL | MOD_ALT, Some(Action::Quit)),
            (None, 0xA5, MOD_ALT, None),
        ] {
            let mut e = KeyboardEngine::default();
            e.configure(
                &[
                    (MOD_ALTGR, 0x54, Action::Refresh),
                    (MOD_CTRL | MOD_ALT, 0x54, Action::Quit),
                ],
                &HashMap::new(),
            );
            if let Some(ctrl) = ctrl {
                assert!(!e.event(ctrl, true, t).consume);
            }
            assert!(!e.event(alt, true, t).consume);
            assert_eq!(e.modifiers(), expected_mods);
            let d = e.event(0x54, true, t);
            assert_eq!(d.consume, expected_action.is_some());
            assert_eq!(d.action, expected_action);
            assert!(!e.event(alt, false, t).consume);
            if let Some(ctrl) = ctrl {
                assert!(!e.event(ctrl, false, t).consume);
            }
            assert_eq!(e.event(0x54, false, t).consume, expected_action.is_some());
            assert_eq!(e.modifiers(), 0);
        }
    }
    #[test]
    fn altgr_capture_survives_modifier_release_reload_and_inhibition() {
        use super::super::keynames::MOD_ALTGR;
        let t = Instant::now();
        for release in [false, true] {
            let mut e = KeyboardEngine::default();
            e.configure(
                &[(MOD_ALTGR, 0x54, Action::Refresh)],
                &HashMap::from([(
                    (MOD_ALTGR, 0x54),
                    BindOptions {
                        release,
                        ..Default::default()
                    },
                )]),
            );
            e.event(0xA2, true, t);
            e.event(0xA5, true, t);
            let d = e.event(0x54, true, t);
            assert!(d.consume);
            assert_eq!(d.action, (!release).then_some(Action::Refresh));
            e.event(0xA2, false, t);
            e.event(0xA5, false, t);
            e.configure(&[], &HashMap::new());
            let d = e.event(0x54, true, t);
            assert!(d.consume);
            assert_eq!(d.action, (!release).then_some(Action::Refresh));
            e.toggle_inhibit();
            let d = e.event(0x54, true, t);
            assert!(d.consume && d.action.is_none());
            e.toggle_inhibit();
            let d = e.event(0x54, false, t);
            assert!(d.consume);
            assert_eq!(d.action, release.then_some(Action::Refresh));
            assert!(!e.event(0x54, false, t).consume);
        }
    }
    #[test]
    fn hook_only_and_unknown_modifiers_never_reach_hotkey_fallback() {
        use super::super::{keynames::MOD_ALTGR, MOD_ALT, MOD_CTRL, MOD_SHIFT};
        let t = Instant::now();
        let mut e = KeyboardEngine::default();
        for mods in [MOD_ALTGR, MOD_ALTGR | MOD_SHIFT, 0x100, MOD_CTRL | 0x100] {
            e.configure(&[(mods, 0x54, Action::Refresh)], &HashMap::new());
            assert!(e.active((mods, 0x54)));
            assert!(!e.fallback_allowed((mods, 0x54)));
            assert!(e.fallback((mods, 0x54), false, t).is_none());
        }
        let mods = MOD_CTRL | MOD_ALT;
        e.configure(&[(mods, 0x54, Action::Refresh)], &HashMap::new());
        assert!(e.fallback_allowed((mods, 0x54)));
        assert_eq!(e.fallback((mods, 0x54), false, t), Some(Action::Refresh));
    }
    #[test]
    fn modal_input_tracks_modifiers_without_firing_release_bindings() {
        let mut e = engine(BindOptions {
            release: true,
            ..Default::default()
        });
        let t = Instant::now();
        e.event(0x5B, true, t);
        assert!(e.event(0x54, true, t).consume);
        assert!(!e.modal_event(0x5B, false));
        assert_eq!(e.modifiers(), 0);
        assert!(e.modal_event(0x54, false));
        assert!(!e.event(0x54, false, t).consume);
        assert!(!e.modal_event(0xA0, true));
        assert_eq!(e.modifiers(), 4);
        e.modal_event(0xA0, false);
        assert_eq!(e.modifiers(), 0);
    }
    #[test]
    fn modal_unbound_down_cannot_turn_into_a_binding_on_repeat() {
        let mut e = engine(BindOptions::default());
        let t = Instant::now();
        e.modal_event(0x54, true);
        e.event(0x5B, true, t);
        assert!(e.event(0x54, true, t).action.is_none());
        e.event(0x54, false, t);
        assert_eq!(e.event(0x54, true, t).action, Some(Action::Refresh));
    }
    #[test]
    fn unbound_keys_and_every_modifier_pass_in_both_directions() {
        for vk in [
            0x10, 0x11, 0x12, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0x5B, 0x5C, 0x41, 0x1B, 0x70,
        ] {
            let mut e = engine(BindOptions::default());
            assert!(!e.event(vk, true, Instant::now()).consume);
            assert!(!e.event(vk, false, Instant::now()).consume);
            assert_eq!(e.modifiers(), 0);
        }
    }
    #[test]
    fn win_chord_consumes_only_key_and_pairs_up_after_modifier_release() {
        let mut e = engine(BindOptions::default());
        let t = Instant::now();
        assert!(!e.event(0x5B, true, t).consume);
        let d = e.event(0x54, true, t);
        assert!(d.consume && d.mask_menu);
        assert_eq!(d.action, Some(Action::Refresh));
        assert!(!e.event(0x5B, false, t).consume);
        assert!(e.event(0x54, false, t).consume);
        assert_eq!(e.modifiers(), 0);
    }
    #[test]
    fn repeat_false_consumes_repeats_but_fires_once_per_press() {
        let mut e = engine(BindOptions {
            no_repeat: true,
            ..Default::default()
        });
        let t = Instant::now();
        e.event(0x5B, true, t);
        assert!(e.event(0x54, true, t).action.is_some());
        let d = e.event(0x54, true, t);
        assert!(d.consume);
        assert!(d.action.is_none());
        e.event(0x54, false, t);
        assert!(e.event(0x54, true, t).action.is_some());
    }
    #[test]
    fn cooldown_is_per_bind_and_covers_separate_presses() {
        let mut e = engine(BindOptions {
            cooldown: Some(Duration::from_millis(500)),
            ..Default::default()
        });
        let t = Instant::now();
        e.event(0x5B, true, t);
        assert!(e.event(0x54, true, t).action.is_some());
        e.event(0x54, false, t);
        let d = e.event(0x54, true, t + Duration::from_millis(499));
        assert!(d.consume);
        assert!(d.action.is_none());
        e.event(0x54, false, t);
        assert!(e
            .event(0x54, true, t + Duration::from_millis(500))
            .action
            .is_some());
    }
    #[test]
    fn inhibition_passes_bind_and_escape_hatch_always_works() {
        let mut e = engine(BindOptions::default());
        let t = Instant::now();
        e.toggle_inhibit();
        e.event(0x5B, true, t);
        assert!(!e.event(0x54, true, t).consume);
        e.event(0x54, false, t);
        e.configure(
            &[(8, 0x54, Action::ToggleKeyboardShortcutsInhibit)],
            &HashMap::new(),
        );
        assert!(e.event(0x54, true, t).action.is_some());
    }
    #[test]
    fn allow_inhibiting_false_still_fires() {
        let mut e = engine(BindOptions {
            allow_inhibiting: false,
            ..Default::default()
        });
        let t = Instant::now();
        e.toggle_inhibit();
        e.event(0x5B, true, t);
        assert!(e.event(0x54, true, t).action.is_some());
    }
    #[test]
    fn release_binds_capture_down_and_fire_on_up_without_modifier() {
        let mut e = engine(BindOptions {
            release: true,
            ..Default::default()
        });
        let t = Instant::now();
        e.event(0x5B, true, t);
        let d = e.event(0x54, true, t);
        assert!(d.consume && d.action.is_none());
        e.event(0x5B, false, t);
        assert_eq!(e.event(0x54, false, t).action, Some(Action::Refresh));
    }
    #[test]
    fn reload_keeps_consumed_key_pair() {
        let mut e = engine(BindOptions::default());
        let t = Instant::now();
        e.event(0x5B, true, t);
        e.event(0x54, true, t);
        e.configure(&[], &HashMap::new());
        assert!(e.event(0x54, false, t).consume);
    }
    #[test]
    fn unbound_first_down_is_not_stolen_after_reload_or_modifier_change() {
        let mut e = engine(BindOptions::default());
        let t = Instant::now();
        assert!(!e.event(0x54, true, t).consume);
        e.event(0x5B, true, t);
        assert!(!e.event(0x54, true, t).consume);
        assert!(!e.event(0x54, false, t).consume);
    }
    #[test]
    fn all_denylisted_chords_are_unbound_even_in_hook() {
        for (m, v) in [(8, 0x4C), (8, 0x44), (12, 0x53)] {
            let mut e = KeyboardEngine::default();
            e.configure(&[(m, v, Action::Quit)], &HashMap::new());
            assert!(!e.active((m, v)));
        }
    }
    #[test]
    fn release_and_inhibit_config_reaches_the_binding() {
        let decoded = crate::config::decode::decode_text("binds { Super+T on-release=true allow-inhibiting=false { toggle-keyboard-shortcuts-inhibit; } }", "", None);
        assert!(!decoded.has_errors(), "{:?}", decoded.issues);
        assert!(decoded.issues.is_empty(), "{:?}", decoded.issues);
        let bind = &decoded.config.binds.hotkeys[0];
        assert!(bind.release);
        assert!(!bind.allow_inhibiting);
        assert_eq!(
            bind.parse_action(),
            Some(Action::ToggleKeyboardShortcutsInhibit)
        );
    }
    #[test]
    fn bare_overview_escape_is_never_swallowed() {
        let mut e = KeyboardEngine::default();
        let t = Instant::now();
        e.configure(&[(0, 0x1B, Action::OverviewToggle)], &HashMap::new());
        assert!(!e.event(0x1B, true, t).consume);
        assert!(!e.event(0x1B, false, t).consume);
    }
    #[test]
    fn fallback_obeys_cooldown_and_never_fires_release_bind() {
        let t = Instant::now();
        let mut e = engine(BindOptions {
            cooldown: Some(Duration::from_millis(500)),
            ..Default::default()
        });
        assert!(e.fallback((8, 0x54), false, t).is_some());
        assert!(e.fallback((8, 0x54), false, t).is_none());
        let mut e = engine(BindOptions {
            release: true,
            ..Default::default()
        });
        assert!(e.fallback((8, 0x54), false, t).is_none());
    }

    // ── niri's Latin-key fallback, with fake layouts ──────────────────────────────────

    /// A layout whose observed virtual keys are not the Latin ones: `key` is what the hook
    /// reports, `latin` the virtual key the same physical key has in the Latin (US) layout.
    struct FakeLayout {
        pairs: Vec<(u32, Option<u32>)>,
    }

    impl FakeLayout {
        fn new(
            pairs: &[(u32, Option<u32>)],
        ) -> std::sync::Arc<dyn super::super::keylayout::LatinFallback> {
            std::sync::Arc::new(Self {
                pairs: pairs.to_vec(),
            })
        }
    }

    impl super::super::keylayout::LatinFallback for FakeLayout {
        fn latin_key(&self, vk: u32) -> Option<u32> {
            self.pairs
                .iter()
                .find(|(observed, _)| *observed == vk)
                .and_then(|(_, latin)| *latin)
        }
    }

    #[test]
    fn a_latin_bind_fires_on_a_layout_that_reports_another_virtual_key() {
        // The fake "Cyrillic" layout reports the extra key 0xE2 for the physical T key;
        // en-US would say 0x54.
        let t = Instant::now();
        let mut e = KeyboardEngine::default();
        e.configure(&[(MOD_CTRL, 0x54, Action::Refresh)], &HashMap::new());
        e.set_latin_fallback(Some(FakeLayout::new(&[(0xE2, Some(0x54))])));
        e.event(0x11, true, t); // Ctrl
        let d = e.event(0xE2, true, t);
        assert!(d.consume);
        assert_eq!(d.action, Some(Action::Refresh));
        // The pair still owns the key-up, so nothing leaks to the application.
        assert!(e.event(0xE2, false, t).consume);
        assert!(!e.event(0x11, false, t).consume);

        // Without the fallback that layout's key names no bind at all.
        let mut plain = KeyboardEngine::default();
        plain.configure(&[(MOD_CTRL, 0x54, Action::Refresh)], &HashMap::new());
        plain.event(0x11, true, t);
        assert!(!plain.event(0xE2, true, t).consume);
        assert!(!plain.event(0xE2, false, t).consume);
    }

    #[test]
    fn the_latin_fallback_is_installed_and_cleared_like_a_reload() {
        let t = Instant::now();
        let mut e = KeyboardEngine::default();
        e.configure(&[(MOD_CTRL, 0x54, Action::Refresh)], &HashMap::new());
        e.set_latin_fallback(Some(FakeLayout::new(&[(0xE2, Some(0x54))])));
        e.event(0x11, true, t);
        assert_eq!(e.event(0xE2, true, t).action, Some(Action::Refresh));
        e.event(0xE2, false, t);
        // A reload that installs no fallback (Windows could not name the layout) matches
        // the plain virtual keys only.
        e.set_latin_fallback(None);
        assert!(e.event(0xE2, true, t).action.is_none());
        e.event(0xE2, false, t);
        assert_eq!(e.event(0x54, true, t).action, Some(Action::Refresh));
    }

    #[test]
    fn the_observed_key_wins_over_its_latin_twin_and_unmapped_keys_never_match() {
        let t = Instant::now();
        let mut e = KeyboardEngine::default();
        // The "layout" maps the observed key onto a *different* bind than the key itself.
        e.configure(
            &[
                (MOD_CTRL, 0xE2, Action::Quit),
                (MOD_CTRL, 0x54, Action::Refresh),
            ],
            &HashMap::new(),
        );
        e.set_latin_fallback(Some(FakeLayout::new(&[(0xE2, Some(0x54)), (0xE3, None)])));
        e.event(0x11, true, t);
        // The layout-specific bind is what fires; the Latin twin is only a fallback.
        assert_eq!(e.event(0xE2, true, t).action, Some(Action::Quit));
        e.event(0xE2, false, t);
        // A key the layout does not map, and a Latin twin that no bind names.
        assert!(e.event(0xE3, true, t).action.is_none());
        assert!(!e.event(0xE3, true, t).consume);
        assert!(e.event(0xE4, true, t).action.is_none());
        assert!(!e.event(0xE4, true, t).consume);
    }

    #[test]
    fn a_latin_twin_that_is_the_same_key_is_not_tried_twice() {
        let t = Instant::now();
        let mut e = KeyboardEngine::default();
        e.configure(&[(MOD_CTRL, 0x54, Action::Refresh)], &HashMap::new());
        // Every Latin layout maps the key onto itself; the fallback must not change
        // anything (and, in particular, must not fire a bind twice).
        e.set_latin_fallback(Some(FakeLayout::new(&[(0x54, Some(0x54))])));
        e.event(0x11, true, t);
        let d = e.event(0x54, true, t);
        assert!(d.consume);
        assert_eq!(d.action, Some(Action::Refresh));
        // A key that no bind names stays unbound.
        assert!(!e.event(0x41, true, t).consume);
    }

    #[test]
    fn a_release_bind_fires_through_the_latin_fallback_too() {
        let t = Instant::now();
        let mut e = KeyboardEngine::default();
        e.configure(
            &[(MOD_CTRL, 0x54, Action::Refresh)],
            &HashMap::from([(
                (MOD_CTRL, 0x54),
                BindOptions {
                    release: true,
                    ..Default::default()
                },
            )]),
        );
        e.set_latin_fallback(Some(FakeLayout::new(&[(0xE2, Some(0x54))])));
        e.event(0x11, true, t);
        let down = e.event(0xE2, true, t);
        assert!(down.consume && down.action.is_none());
        e.event(0x11, false, t);
        assert_eq!(e.event(0xE2, false, t).action, Some(Action::Refresh));
    }
}
