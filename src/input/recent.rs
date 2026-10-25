//! Event-driven keyboard input for the recent-windows switcher.
//!
//! Integration (the existing keyboard-action drainer remains the only worker):
//! - Publish an `InputSnapshot` on startup/reload, using the *effective* ordinary
//!   hotkey table, including defaults and overrides, not just config binds.
//! - Wire that drainer's `Sender<KeyboardEvent>` with `set_sender`. Legacy hook
//!   actions can use `send_action` instead of a separate `Sender<Action>`.
//! - Call `key_event` for every keyboard down/up, before legacy double-Alt /
//!   Escape handling; return a nonzero hook result when it returns `true`.
//!   Reset the legacy double-Alt chain on consumption; bypass its recognition
//!   while `is_active()` is true so switcher input cannot also toggle overview.
//! - Drain `Action` as before and forward `Recent` to the engine. The engine
//!   calls `set_active` on session changes, including failed opening attempts.
//!
//! Opening publishes active=true *before* queuing Cycle, so a modifier release
//! arriving before the drainer starts is still queued behind the opening event.
//! Repeated Cycle events belong to the existing frozen session; only the engine
//! decides whether to begin one. No engine access, OS queries, timers or threads
//! are needed here. Locks protect only small input-owned state/snapshots.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, LazyLock};

use parking_lot::{Mutex, RwLock};

use super::{Action, DEFAULT_MOD_KEY, MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN};
use crate::config::recent_windows_config::{RecentCycle, RecentScope, RecentWindowsConfig};
use crate::overlay::recent_windows::RecentCommand;
use crate::utils::WindowId;

const MODIFIERS: u32 = MOD_ALT | MOD_CTRL | MOD_SHIFT | MOD_WIN;
const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
const VK_MENU: u32 = 0x12;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
const VK_LSHIFT: u32 = 0xA0;
const VK_RSHIFT: u32 = 0xA1;
const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_LMENU: u32 = 0xA4;
const VK_RMENU: u32 = 0xA5;

/// Shared with the existing keyboard-action channel; no additional drainer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyboardEvent {
    Action(Action),
    Recent(RecentInput),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecentInput {
    /// Begin a session if absent, otherwise cycle its frozen list. Scope is an
    /// opening preselection only (as in `RecentSession::cycle`). The HWND is
    /// captured with the trigger, not sampled later by the drainer.
    Cycle(RecentCycle, isize),
    Command(RecentCommand),
    /// Current physical Alt/Ctrl/Shift/Win state, AFTER this modifier event.
    Modifiers(bool),
    /// Genuine pass-through application key-down, including auto-repeat. The
    /// engine should immediately promote this HWND, without focus debounce.
    ApplicationKey(WindowId),
}

/// Compiled off the hook thread by `publish_snapshot`.
#[derive(Debug, Clone)]
pub struct InputSnapshot {
    pub config: RecentWindowsConfig,
    /// Resolved Win32 MOD_* flags, not a virtual key.
    pub mod_key: u32,
    /// Effective ordinary `(modifiers, virtual_key, action)` rows. Ordinary
    /// bindings take precedence over colliding recent-windows bindings. During
    /// a session, navigation actions use their trigger keys without modifiers;
    /// built-in modal commands take precedence over those derived keys.
    pub hotkeys: Vec<(u32, u32, Action)>,
}

impl Default for InputSnapshot {
    fn default() -> Self {
        Self {
            config: RecentWindowsConfig::default(),
            mod_key: DEFAULT_MOD_KEY,
            hotkeys: Vec::new(),
        }
    }
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
static SNAPSHOT: LazyLock<RwLock<Arc<CompiledSnapshot>>> =
    LazyLock::new(|| RwLock::new(Arc::new(CompiledSnapshot::default())));
static KEYS: LazyLock<Mutex<KeyState>> = LazyLock::new(|| Mutex::new(KeyState::default()));
static SENDER: LazyLock<RwLock<Option<Sender<KeyboardEvent>>>> =
    LazyLock::new(|| RwLock::new(None));

/// Publish configuration + the resolved normal hotkeys together, on startup and
/// every reload. Until the first publication, recent input is disabled.
/// Disabling/closing a session in the engine must also call `set_active(false)`.
pub fn publish_snapshot(snapshot: InputSnapshot) {
    let compiled = Arc::new(CompiledSnapshot::new(snapshot));
    *SNAPSHOT.write() = compiled;
}

pub fn set_sender(sender: Sender<KeyboardEvent>) {
    *SENDER.write() = Some(sender);
}

/// Call after stopping the keyboard hook. Reset physical state for a subsequent
/// hook installation; ordinary session closure should use `set_active(false)`.
pub fn clear_sender() {
    *SENDER.write() = None;
    ACTIVE.store(false, Ordering::Release);
    *KEYS.lock() = KeyState::default();
}

/// Bridge legacy low-level hook actions into the same ordered channel.
/// Returns false when unwired or when the receiver has gone away.
pub fn send_action(action: Action) -> bool {
    let sender = SENDER.read().clone();
    sender.is_some_and(|tx| tx.send(KeyboardEvent::Action(action)).is_ok())
}

/// Acknowledge every drained Cycle, including repeats. A reset may have cleared
/// ACTIVE after the hook queued that Cycle but before the final modifier-up.
/// Reconcile under KEYS and queue the missing release behind existing input.
/// Never clear suppression: swallowed downs still own their matching ups.
pub fn set_active(active: bool) {
    let sender = SENDER.read().clone();
    let keys = KEYS.lock();
    ACTIVE.store(active, Ordering::Release);
    if active && keys.modifiers() == 0 {
        if let Some(tx) = sender {
            if tx.send(KeyboardEvent::Recent(RecentInput::Modifiers(false))).is_err() {
                ACTIVE.store(false, Ordering::Release);
            }
        }
    }
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Called from WH_KEYBOARD_LL, before legacy keyboard handling. `vk` is the
/// unmodified KBDLLHOOKSTRUCT virtual key (preserve left/right modifier codes).
/// `foreground` is the current HWND, or zero when unavailable.
///
/// Only switcher downs and their matching ups are consumed. All physical
/// modifiers are tracked even while unwired/disabled. Never use async key-state
/// polling here: on key-up it can still include the very key being released.
pub fn key_event(vk: u32, down: bool, up: bool, foreground: isize) -> bool {
    let snapshot = SNAPSHOT.read().clone();
    let sender = SENDER.read().clone();
    let mut keys = KEYS.lock();
    let was_suppressed = keys
        .suppressed
        .get(vk as usize)
        .copied()
        .unwrap_or(false);
    let decision = keys.event(
        &snapshot,
        &ACTIVE,
        sender.is_some(),
        vk,
        down,
        up,
        foreground,
    );

    // event() marks opening active synchronously, before this send. Holding only
    // the input-state mutex here also preserves event order if callers overlap.
    if let (Some(input), Some(tx)) = (decision.input, sender) {
        if tx.send(KeyboardEvent::Recent(input)).is_err() {
            ACTIVE.store(false, Ordering::Release);
            // Fail open on a NEW press when there is no consumer. An already
            // swallowed press must still keep its repeats/up suppressed.
            if down && decision.consume && !was_suppressed {
                keys.suppressed[vk as usize] = false;
                return false;
            }
        }
    }
    decision.consume
}

/// A higher-priority modal UI swallowed this event. Still track physical
/// modifiers (and complete any previously suppressed key-up), but never open
/// the switcher or treat a dismissed-overlay key as application input.
pub fn consumed_key_event(vk: u32, down: bool, up: bool) {
    let snapshot = SNAPSHOT.read().clone();
    let sender = SENDER.read().clone();
    let mut keys = KEYS.lock();
    let decision = keys.consumed_event(&snapshot, &ACTIVE, sender.is_some(), vk, down, up);
    if let (Some(input), Some(tx)) = (decision.input, sender) {
        if tx.send(KeyboardEvent::Recent(input)).is_err() {
            ACTIVE.store(false, Ordering::Release);
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Trigger {
    Key(u32),
    /// Generic Alt/Ctrl/Shift/Win matches either physical side (and VK_MENU etc.).
    Modifier(u32),
}

impl Trigger {
    fn matches(self, vk: u32) -> bool {
        match self {
            Self::Key(key) => key == vk,
            Self::Modifier(flag) => modifier_flag(vk) == flag,
        }
    }

    fn modifier(self) -> u32 {
        match self {
            Self::Key(key) => modifier_flag(key),
            Self::Modifier(flag) => flag,
        }
    }
}

#[derive(Debug)]
struct RecentHotkey {
    modifiers: u32,
    trigger: Trigger,
    cycle: RecentCycle,
}

impl RecentHotkey {
    fn matches(&self, vk: u32, held: u32) -> bool {
        self.trigger.matches(vk) && held == (self.modifiers | self.trigger.modifier())
    }
}

/// No implicit default compilation on the hook's first event.
#[derive(Debug, Default)]
struct CompiledSnapshot {
    enabled: bool,
    recent: Vec<RecentHotkey>,
    hotkeys: Vec<(u32, u32, Action)>,
    navigation: Vec<(u32, RecentCommand)>,
}

impl CompiledSnapshot {
    fn new(snapshot: InputSnapshot) -> Self {
        let recent = snapshot
            .config
            .binds
            .iter()
            .filter_map(|bind| {
                compile_chord(&bind.chord, snapshot.mod_key).map(|(modifiers, trigger)| {
                    RecentHotkey {
                        modifiers,
                        trigger,
                        cycle: bind.action,
                    }
                })
            })
            .collect();
        // Navigation is modal: use ordinary trigger keys even when the session
        // was opened with a different modifier prefix. Keep table order for
        // multiple ordinary navigation actions sharing a trigger key.
        let navigation = snapshot
            .hotkeys
            .iter()
            .filter_map(|(_, key, action)| action_command(action).map(|command| (*key, command)))
            .collect();
        Self {
            enabled: snapshot.config.enabled,
            recent,
            hotkeys: snapshot.hotkeys,
            navigation,
        }
    }
}

fn modifier_flag(vk: u32) -> u32 {
    match vk {
        VK_MENU | VK_LMENU | VK_RMENU => MOD_ALT,
        VK_CONTROL | VK_LCONTROL | VK_RCONTROL => MOD_CTRL,
        VK_SHIFT | VK_LSHIFT | VK_RSHIFT => MOD_SHIFT,
        VK_LWIN | VK_RWIN => MOD_WIN,
        _ => 0,
    }
}

fn modifier_name(name: &str, mod_key: u32) -> Option<u32> {
    match name {
        "mod" if mod_key != 0 && mod_key & !MODIFIERS == 0 => Some(mod_key),
        "alt" | "altl" | "altr" | "lalt" | "ralt" => Some(MOD_ALT),
        "ctrl" | "control" | "controll" | "controlr" | "lctrl" | "rctrl" => Some(MOD_CTRL),
        "shift" | "shiftl" | "shiftr" | "lshift" | "rshift" => Some(MOD_SHIFT),
        "win" | "super" | "meta" | "superl" | "superr" | "lwin" | "rwin" => Some(MOD_WIN),
        _ => None,
    }
}

fn canonical_name(name: &str) -> String {
    name.trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| !matches!(ch, '_' | '-' | ' '))
        .collect()
}

fn parse_trigger(name: &str, mod_key: u32) -> Option<Trigger> {
    let canonical = canonical_name(name);
    let side = match canonical.as_str() {
        "altl" | "lalt" => Some(VK_LMENU),
        "altr" | "ralt" => Some(VK_RMENU),
        "controll" | "lctrl" => Some(VK_LCONTROL),
        "controlr" | "rctrl" => Some(VK_RCONTROL),
        "shiftl" | "lshift" => Some(VK_LSHIFT),
        "shiftr" | "rshift" => Some(VK_RSHIFT),
        "superl" | "lwin" => Some(VK_LWIN),
        "superr" | "rwin" => Some(VK_RWIN),
        _ => None,
    };
    if let Some(vk) = side {
        return Some(Trigger::Key(vk));
    }
    if let Some(flag) = modifier_name(&canonical, mod_key) {
        // A composite Mod cannot be a single physical trigger key.
        return (flag.count_ones() == 1).then_some(Trigger::Modifier(flag));
    }
    // The shared parser includes grave/backtick/quoteleft (VK_OEM_3, 0xC0).
    super::parse_key_name(name).map(Trigger::Key)
}

fn compile_chord(chord: &str, mod_key: u32) -> Option<(u32, Trigger)> {
    let parts: Vec<_> = chord.split('+').collect();
    if parts.len() < 2 || parts.iter().any(|part| part.trim().is_empty()) {
        return None;
    }
    let mut modifiers = 0;
    for part in &parts[..parts.len() - 1] {
        modifiers |= modifier_name(&canonical_name(part), mod_key)?;
    }
    let trigger = parse_trigger(parts[parts.len() - 1], mod_key)?;
    // Require a REAL modifier distinct from the trigger after resolving Mod.
    // Alt+Alt_L and Mod+Alt with Mod=Alt cannot keep a session open.
    if modifiers & !trigger.modifier() == 0 {
        return None;
    }
    Some((modifiers, trigger))
}

fn ordinary_matches(modifiers: u32, key: u32, vk: u32, held: u32) -> bool {
    let trigger = match key {
        VK_MENU | VK_CONTROL | VK_SHIFT => Trigger::Modifier(modifier_flag(key)),
        _ => Trigger::Key(key),
    };
    trigger.matches(vk) && held == ((modifiers & MODIFIERS) | trigger.modifier())
}

fn action_command(action: &Action) -> Option<RecentCommand> {
    Some(match action {
        Action::FocusColumnLeft => RecentCommand::Previous,
        Action::FocusColumnRight => RecentCommand::Next,
        Action::FocusColumnFirst => RecentCommand::First,
        Action::FocusColumnLast => RecentCommand::Last,
        Action::CloseWindow => RecentCommand::Close,
        Action::Screenshot | Action::WindowScreenshot | Action::ScreenshotWindow => {
            RecentCommand::Screenshot
        }
        _ => return None,
    })
}

fn session_command(vk: u32) -> Option<RecentCommand> {
    Some(match vk {
        0x1B => RecentCommand::Cancel,                       // Escape
        0x0D => RecentCommand::Confirm,                      // Enter
        0x41 => RecentCommand::Scope(RecentScope::All),       // A
        0x57 => RecentCommand::Scope(RecentScope::Workspace), // W
        0x4F => RecentCommand::Scope(RecentScope::Output),    // O
        0x53 => RecentCommand::CycleScope,                   // S
        0x25 => RecentCommand::Previous,                     // Left
        0x27 => RecentCommand::Next,                         // Right
        0x24 => RecentCommand::First,                        // Home
        0x23 => RecentCommand::Last,                         // End
        _ => return None,
    })
}

struct KeyState {
    physical: [bool; 256],
    suppressed: [bool; 256],
}

impl Default for KeyState {
    fn default() -> Self {
        Self {
            physical: [false; 256],
            suppressed: [false; 256],
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Decision {
    consume: bool,
    input: Option<RecentInput>,
}

impl KeyState {
    fn modifiers(&self) -> u32 {
        // Track every reported modifier, not just the configured opening chord.
        // In particular either Win key, or the other Alt/Shift side, keeps a
        // session alive after the original modifier was released.
        [
            VK_MENU, VK_LMENU, VK_RMENU, VK_CONTROL, VK_LCONTROL, VK_RCONTROL,
            VK_SHIFT, VK_LSHIFT, VK_RSHIFT, VK_LWIN, VK_RWIN,
        ]
        .iter()
        .filter(|vk| self.physical[**vk as usize])
        .fold(0, |flags, vk| flags | modifier_flag(*vk))
    }

    fn consume(&mut self, slot: usize, input: RecentInput) -> Decision {
        self.suppressed[slot] = true;
        Decision {
            consume: true,
            input: Some(input),
        }
    }

    fn consumed_event(
        &mut self, snapshot: &CompiledSnapshot, active: &AtomicBool,
        connected: bool, vk: u32, down: bool, up: bool,
    ) -> Decision {
        if vk == 0 || vk >= 256 || down == up { return Decision::default(); }
        self.physical[vk as usize] = down;
        if up { self.suppressed[vk as usize] = false; }
        Decision {
            consume: false,
            input: (connected && snapshot.enabled && active.load(Ordering::Acquire)
                && modifier_flag(vk) != 0)
                .then_some(RecentInput::Modifiers(self.modifiers() != 0)),
        }
    }

    /// Deterministic state machine; tests use a local active flag, no globals,
    /// engine, channel, desktop, or timing dependencies.
    #[allow(clippy::too_many_arguments)]
    fn event(
        &mut self,
        snapshot: &CompiledSnapshot,
        active: &AtomicBool,
        connected: bool,
        vk: u32,
        down: bool,
        up: bool,
        foreground: isize,
    ) -> Decision {
        if vk == 0 || vk >= 256 || down == up {
            return Decision::default();
        }
        let slot = vk as usize;
        // Update BEFORE calculating held modifiers, especially on key-up.
        self.physical[slot] = down;
        let held = self.modifiers();
        let modifier = modifier_flag(vk) != 0;
        let running = active.load(Ordering::Acquire);
        let suppressed = self.suppressed[slot];

        if up {
            self.suppressed[slot] = false;
            return Decision {
                consume: suppressed,
                input: (connected && snapshot.enabled && running && modifier)
                    .then_some(RecentInput::Modifiers(held != 0)),
            };
        }
        if !connected || !snapshot.enabled || (suppressed && !running) {
            return Decision {
                consume: suppressed,
                input: None,
            };
        }

        let ordinary = snapshot.hotkeys.iter()
            .find(|(modifiers, key, _)| ordinary_matches(*modifiers, *key, vk, held));
        // Normal hotkeys own collisions, even when the recent chord uses a
        // modifier as its trigger. They stay on WM_HOTKEY outside a session.
        if ordinary.is_none() {
            if let Some(binding) = snapshot.recent.iter().find(|binding| binding.matches(vk, held)) {
                // Do not wait for the drainer to open: its next event may be
                // the final modifier-up. Repeats merely send another Cycle.
                active.store(true, Ordering::Release);
                return self.consume(slot, RecentInput::Cycle(binding.cycle, foreground));
            }
        }
        if running {
            // Relevant ordinary trigger keys become commands regardless of
            // their modifiers, never layout actions. Built-ins win over derived
            // navigation, including Enter/A/W/O/S with opening modifiers held.
            // An explicit recent chord still keeps cycling on auto-repeat.
            let command = session_command(vk).or_else(|| {
                snapshot.navigation.iter()
                    .find(|(key, _)| *key == vk)
                    .map(|(_, command)| *command)
            });
            if let Some(command) = command {
                return self.consume(slot, RecentInput::Command(command));
            }
            if modifier {
                return Decision { consume: suppressed, input: Some(RecentInput::Modifiers(held != 0)) };
            }
        }
        if suppressed || modifier || ordinary.is_some() {
            return Decision {
                consume: suppressed,
                input: None,
            };
        }
        Decision {
            consume: false,
            input: (foreground != 0).then_some(RecentInput::ApplicationKey(WindowId::new(foreground))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::recent_windows_config::RecentBind;

    struct Hook {
        keys: KeyState,
        active: AtomicBool,
        snapshot: CompiledSnapshot,
    }

    impl Hook {
        fn new(snapshot: InputSnapshot) -> Self {
            Self { keys: KeyState::default(), active: AtomicBool::new(false),
                snapshot: CompiledSnapshot::new(snapshot) }
        }
        fn press(&mut self, vk: u32) -> Decision { self.event(vk, true, false, 42) }
        fn release(&mut self, vk: u32) -> Decision { self.event(vk, false, true, 42) }
        fn event(&mut self, vk: u32, down: bool, up: bool, foreground: isize) -> Decision {
            self.keys.event(&self.snapshot, &self.active, true, vk, down, up, foreground)
        }
        fn running(&self) -> bool { self.active.load(Ordering::Acquire) }
    }

    fn custom(chord: &str, cycle: RecentCycle) -> InputSnapshot {
        let mut snapshot = InputSnapshot::default();
        snapshot.config.binds = vec![RecentBind { chord: chord.into(), action: cycle }];
        snapshot
    }

    fn input(input: RecentInput, consume: bool) -> Decision {
        Decision { consume, input: Some(input) }
    }

    #[test]
    fn ordinary_bind_wins_opening_collision_and_maps_inside_session() {
        let mut snapshot = InputSnapshot::default();
        snapshot.hotkeys.push((MOD_ALT, 0x09, Action::FocusColumnRight));
        let mut hook = Hook::new(snapshot);
        hook.press(VK_LMENU);
        assert_eq!(hook.press(0x09), Decision::default());
        assert!(!hook.running());
        assert_eq!(hook.release(0x09), Decision::default());
        hook.active.store(true, Ordering::Release);
        assert_eq!(hook.press(0x09), input(RecentInput::Command(RecentCommand::Next), true));
    }

    #[test]
    fn effective_defaults_beat_mod_grave_but_not_alt_grave() {
        let mut snapshot = InputSnapshot::default();
        snapshot.hotkeys.push((MOD_CTRL | MOD_ALT, 0xC0, Action::FocusPrevious));
        let mut hook = Hook::new(snapshot);
        hook.press(VK_CONTROL); hook.press(VK_MENU);
        assert_eq!(hook.press(0xC0), Decision::default());
        assert!(!hook.running());
        hook.release(0xC0); hook.release(VK_CONTROL);
        assert_eq!(hook.press(0xC0), input(RecentInput::Cycle(RecentCycle {
            filter_app_id: true, ..RecentCycle::default()
        }, 42), true));
    }

    #[test]
    fn opening_is_active_before_drainer_and_quick_release_is_not_lost() {
        let mut hook = Hook::new(InputSnapshot::default());
        hook.press(VK_MENU);
        let opening = hook.press(0x09);
        // No event has been drained, nor has the engine acknowledged opening.
        assert!(hook.running());
        let release = hook.release(VK_MENU);
        assert_eq!(opening, input(RecentInput::Cycle(RecentCycle::default(), 42), true));
        assert_eq!(release, input(RecentInput::Modifiers(false), false));
    }

    #[test]
    fn repeats_cycle_without_reopening_or_app_input_and_keep_opening_scope() {
        let cycle = RecentCycle { backwards: true, filter_app_id: true, scope: RecentScope::Workspace };
        let mut hook = Hook::new(custom("Alt+Tab", cycle));
        hook.press(VK_RMENU);
        for _ in 0..5 {
            assert_eq!(hook.press(0x09), input(RecentInput::Cycle(cycle, 42), true));
            assert!(hook.running());
        }
        assert_eq!(hook.release(0x09), Decision { consume: true, input: None });
        assert_eq!(hook.release(0x09), Decision::default());
    }

    #[test]
    fn consumed_keyups_and_repeats_stay_suppressed_after_engine_closes() {
        let mut hook = Hook::new(InputSnapshot::default());
        hook.press(VK_LMENU); hook.press(0x09);
        hook.active.store(false, Ordering::Release);
        assert_eq!(hook.press(0x09), Decision { consume: true, input: None });
        assert!(!hook.running());
        assert_eq!(hook.release(0x09), Decision { consume: true, input: None });
        // A fresh press may open the next session.
        assert!(hook.press(0x09).consume);
        assert!(hook.running());
    }

    #[test]
    fn every_physical_modifier_keeps_session_until_the_last_release() {
        let mut hook = Hook::new(custom("Alt+Ctrl+Shift+Win+Tab", RecentCycle::default()));
        for vk in [VK_LMENU, VK_RCONTROL, VK_LSHIFT, VK_RWIN] { hook.press(vk); }
        assert!(hook.press(0x09).consume);
        for vk in [VK_LMENU, VK_RCONTROL, VK_LSHIFT] {
            assert_eq!(hook.release(vk), input(RecentInput::Modifiers(true), false));
        }
        assert_eq!(hook.release(VK_RWIN), input(RecentInput::Modifiers(false), false));
    }

    #[test]
    fn generic_modifiers_and_both_sides_are_tracked_without_polling() {
        for (generic, left, right) in [(VK_MENU, VK_LMENU, VK_RMENU),
            (VK_CONTROL, VK_LCONTROL, VK_RCONTROL), (VK_SHIFT, VK_LSHIFT, VK_RSHIFT),
            (VK_LWIN, VK_LWIN, VK_RWIN)]
        {
            let mut hook = Hook::new(InputSnapshot::default());
            hook.active.store(true, Ordering::Release);
            assert_eq!(hook.press(generic), input(RecentInput::Modifiers(true), false));
            assert_eq!(hook.release(generic), input(RecentInput::Modifiers(false), false));
            hook.press(left); hook.press(right);
            assert_eq!(hook.release(left), input(RecentInput::Modifiers(true), false));
            assert_eq!(hook.release(right), input(RecentInput::Modifiers(false), false));
        }
    }

    #[test]
    fn higher_priority_modal_consumption_still_tracks_modifiers_without_dispatch() {
        let mut hook = Hook::new(custom("Ctrl+Alt", RecentCycle::default()));
        assert_eq!(hook.keys.consumed_event(&hook.snapshot, &hook.active, true,
            VK_LCONTROL, true, false), Decision::default());
        // A swallowed trigger must not open recent windows or promote MRU.
        assert_eq!(hook.keys.consumed_event(&hook.snapshot, &hook.active, true,
            VK_RMENU, true, false), Decision::default());
        assert!(!hook.running());
        hook.keys.consumed_event(&hook.snapshot, &hook.active, true, VK_RMENU, false, true);
        assert!(hook.press(VK_RMENU).consume);
        assert_eq!(hook.keys.consumed_event(&hook.snapshot, &hook.active, true,
            VK_RMENU, false, true), input(RecentInput::Modifiers(true), false));
        assert!(!hook.keys.suppressed[VK_RMENU as usize]);
        assert_eq!(hook.keys.consumed_event(&hook.snapshot, &hook.active, true,
            VK_LCONTROL, false, true), input(RecentInput::Modifiers(false), false));
        assert_eq!(hook.keys.modifiers(), 0);
        assert_eq!(hook.keys.consumed_event(&hook.snapshot, &hook.active, true,
            0x53, true, false), Decision::default());
    }

    #[test]
    fn modifier_trigger_works_repeats_and_suppresses_its_up() {
        let mut hook = Hook::new(custom("Ctrl+Alt", RecentCycle::default()));
        hook.press(VK_LCONTROL);
        for _ in 0..2 {
            assert_eq!(hook.press(VK_RMENU), input(RecentInput::Cycle(RecentCycle::default(), 42), true));
        }
        assert_eq!(hook.release(VK_RMENU), input(RecentInput::Modifiers(true), true));
        assert_eq!(hook.release(VK_LCONTROL), input(RecentInput::Modifiers(false), false));
    }

    #[test]
    fn modifier_trigger_aliases_and_normal_precedence() {
        let mut hook = Hook::new(custom("Control_L+Alt_R", RecentCycle::default()));
        hook.press(VK_LCONTROL);
        assert!(!hook.press(VK_LMENU).consume);
        hook.release(VK_LMENU);
        assert!(hook.press(VK_RMENU).consume);

        let mut snapshot = custom("Ctrl+Alt", RecentCycle::default());
        snapshot.hotkeys.push((MOD_CTRL, VK_MENU, Action::Quit));
        let mut hook = Hook::new(snapshot);
        hook.press(VK_RCONTROL);
        assert_eq!(hook.press(VK_LMENU), Decision::default());
        assert!(!hook.running());
    }

    #[test]
    fn no_zero_modifiers_or_modifier_only_self_chords_after_mod_resolution() {
        for (chord, mod_key) in [("Tab", MOD_ALT), ("Alt+Alt_L", MOD_ALT),
            ("Mod+Alt", MOD_ALT), ("Mod+Tab", 0), ("Ctrl++Tab", MOD_ALT),
            ("Foo+Tab", MOD_ALT), ("Alt+NotAKey", MOD_ALT), ("Alt+Mod", DEFAULT_MOD_KEY)]
        {
            let mut snapshot = custom(chord, RecentCycle::default());
            snapshot.mod_key = mod_key;
            assert!(CompiledSnapshot::new(snapshot).recent.is_empty(), "{chord}");
        }
    }

    #[test]
    fn mod_resolution_scope_and_backwards_flags_are_preserved() {
        for scope in [RecentScope::Output, RecentScope::Workspace] {
            let cycle = RecentCycle { backwards: true, filter_app_id: true, scope };
            let mut snapshot = custom("Mod+Shift+grave", cycle);
            snapshot.mod_key = MOD_WIN;
            let mut hook = Hook::new(snapshot);
            hook.press(VK_RWIN); hook.press(VK_RSHIFT);
            assert_eq!(hook.press(0xC0), input(RecentInput::Cycle(cycle, 42), true));
        }
    }

    #[test]
    fn modal_keys_and_their_ups_are_consumed_even_with_opening_modifiers() {
        let mut snapshot = InputSnapshot::default();
        snapshot.hotkeys.push((MOD_ALT, 0x0D, Action::Spawn(vec!["terminal".into()])));
        snapshot.hotkeys.push((MOD_CTRL, 0x57, Action::CloseWindow));
        snapshot.hotkeys.push((MOD_CTRL, 0x53, Action::Screenshot));
        snapshot.hotkeys.push((MOD_CTRL, 0x25, Action::FocusColumnRight));
        let mut hook = Hook::new(snapshot);
        hook.press(VK_MENU); hook.press(0x09); hook.release(0x09);
        for (vk, command) in [(0x1B, RecentCommand::Cancel), (0x0D, RecentCommand::Confirm),
            (0x41, RecentCommand::Scope(RecentScope::All)),
            (0x57, RecentCommand::Scope(RecentScope::Workspace)),
            (0x4F, RecentCommand::Scope(RecentScope::Output)), (0x53, RecentCommand::CycleScope),
            (0x25, RecentCommand::Previous), (0x27, RecentCommand::Next),
            (0x24, RecentCommand::First), (0x23, RecentCommand::Last)]
        {
            assert_eq!(hook.press(vk), input(RecentInput::Command(command), true));
            assert_eq!(hook.release(vk), Decision { consume: true, input: None });
        }
    }

    #[test]
    fn ordinary_navigation_keys_ignore_modifiers_and_do_not_promote_app_input() {
        for (action, command) in [(Action::FocusColumnLeft, RecentCommand::Previous),
            (Action::FocusColumnRight, RecentCommand::Next), (Action::FocusColumnFirst, RecentCommand::First),
            (Action::FocusColumnLast, RecentCommand::Last), (Action::CloseWindow, RecentCommand::Close),
            (Action::Screenshot, RecentCommand::Screenshot), (Action::WindowScreenshot, RecentCommand::Screenshot),
            (Action::ScreenshotWindow, RecentCommand::Screenshot)]
        {
            let mut snapshot = InputSnapshot::default();
            snapshot.hotkeys.push((MOD_CTRL | MOD_WIN, 0x4A, action));
            let mut hook = Hook::new(snapshot);
            hook.press(VK_MENU); hook.press(0x09);
            assert_eq!(hook.press(0x4A), input(RecentInput::Command(command), true));
            hook.active.store(false, Ordering::Release);
            assert!(hook.release(0x4A).consume);
        }
    }

    #[test]
    fn vertical_focus_keys_are_not_recent_navigation() {
        let mut snapshot = InputSnapshot::default();
        snapshot.hotkeys.push((MOD_CTRL, 0x4A, Action::FocusUp));
        snapshot.hotkeys.push((MOD_CTRL, 0x4B, Action::FocusDown));
        let mut hook = Hook::new(snapshot);
        hook.press(VK_MENU);
        hook.press(0x09);
        hook.release(0x09);
        for vk in [0x26, 0x28, 0x4A, 0x4B] {
            assert_eq!(
                hook.press(vk),
                input(RecentInput::ApplicationKey(WindowId::new(42)), false)
            );
            assert_eq!(hook.release(vk), Decision::default());
        }
    }

    #[test]
    fn custom_recent_chord_on_modal_key_keeps_cycling() {
        let mut hook = Hook::new(custom("Alt+W", RecentCycle::default()));
        hook.press(VK_MENU);
        for _ in 0..3 {
            assert_eq!(hook.press(0x57), input(RecentInput::Cycle(RecentCycle::default(), 42), true));
        }
    }

    #[test]
    fn application_keys_are_immediate_pass_through_not_modifiers_or_keyups() {
        let mut hook = Hook::new(InputSnapshot::default());
        for _ in 0..2 {
            assert_eq!(hook.press(0x58), input(RecentInput::ApplicationKey(WindowId::new(42)), false));
        }
        assert_eq!(hook.release(0x58), Decision::default());
        assert_eq!(hook.event(0x58, true, false, 0), Decision::default());
        assert_eq!(hook.press(VK_MENU), Decision::default());
        // An application key stays pass-through even while a session exists.
        hook.press(0x09);
        assert_eq!(hook.press(0x58), input(RecentInput::ApplicationKey(WindowId::new(42)), false));
    }

    #[test]
    fn explicit_empty_binds_do_not_reinstall_recent_defaults() {
        let mut snapshot = InputSnapshot::default();
        snapshot.config.binds.clear();
        let mut hook = Hook::new(snapshot);
        hook.press(VK_MENU);
        assert_eq!(
            hook.press(0x09),
            input(RecentInput::ApplicationKey(WindowId::new(42)), false)
        );
        assert!(!hook.running());
    }

    #[test]
    fn default_shift_cycles_backwards_but_chords_require_exact_modifiers() {
        let mut snapshot = InputSnapshot::default();
        snapshot.mod_key = MOD_WIN;
        let mut hook = Hook::new(snapshot.clone());
        hook.press(VK_LMENU);
        hook.press(VK_RSHIFT);
        assert_eq!(
            hook.press(0x09),
            input(RecentInput::Cycle(RecentCycle {
                backwards: true,
                ..RecentCycle::default()
            }, 42), true)
        );

        let mut hook = Hook::new(snapshot);
        hook.press(VK_MENU);
        hook.press(VK_CONTROL); // Extra Ctrl is not an Alt+Tab match.
        assert_eq!(
            hook.press(0x09),
            input(RecentInput::ApplicationKey(WindowId::new(42)), false)
        );
        assert!(!hook.running());
    }

    #[test]
    fn disabled_unwired_and_invalid_events_do_not_steal_new_presses() {
        let mut snapshot = InputSnapshot::default();
        snapshot.config.enabled = false;
        let mut hook = Hook::new(snapshot);
        hook.press(VK_MENU);
        assert_eq!(hook.press(0x09), Decision::default());
        assert!(!hook.running());

        let snapshot = CompiledSnapshot::new(InputSnapshot::default());
        let active = AtomicBool::new(false);
        let mut keys = KeyState::default();
        assert_eq!(keys.event(&snapshot, &active, false, VK_MENU, true, false, 42), Decision::default());
        assert_eq!(keys.event(&snapshot, &active, false, 0x09, true, false, 42), Decision::default());
        assert!(!active.load(Ordering::Acquire));
        // Physical tracking was not disabled along with the sender.
        assert!(keys.event(&snapshot, &active, true, 0x09, true, false, 42).consume);
        for (vk, down, up) in [(0, true, false), (256, true, false),
            (u32::MAX, false, true), (0x09, false, false), (0x09, true, true)]
        {
            assert_eq!(keys.event(&snapshot, &active, true, vk, down, up, 42), Decision::default());
        }
    }
}
