//! Explicit keyboard options with native and read/log-only Windows adapters.
//!
//! niri's repeat-delay is milliseconds and repeat-rate is repeats/second (zero
//! disables repeat). Windows exposes only four delay steps (250..=1000 ms) and
//! 32 speed steps (approximately 2.5..=30 repeats/second, hardware dependent).
//! We round/clamp requests; rate zero releases ownership instead of setting
//! Windows speed zero (which still repeats). InputConfig retains niri values.
//!
//! Only explicitly configured options acquire ownership. Native SPI changes
//! are non-persistent. Native NumLock enforcement is refused: GetKeyState is
//! queue-relative, so a background daemon cannot safely capture/restore its toggle.
//! WIRI_KEYBOARD_DRY_RUN=1 selects shadow writes and log-only layout requests:
//! no SPI setters, SendInput, or layout posts, including restoration/Drop.
//! The mode is sampled once, so cleanup cannot accidentally change backends.
//!
//! Keep one KeyboardConfigSession in main, apply(&config.input) before spawning
//! clients and in apply_loaded_config, then restore() on normal exit. Drop is a
//! fallback. External changes relinquish ownership. Layout requests use Lane H
//! and are foreground-thread-only, asynchronous, and best effort (apps may
//! reject them). No timers, polling, workers, layout installs, or registry writes.

use std::collections::BTreeMap;

use crate::config::InputConfig;
use crate::ipc::keyboard::{
    plan_configured_layout, request_configured_layout, ConfiguredLayoutPlan,
};
use tracing::{info, warn};
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETKEYBOARDDELAY, SPI_GETKEYBOARDSPEED, SPI_SETKEYBOARDDELAY,
    SPI_SETKEYBOARDSPEED, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

/// Lifetime of one startup/reload sequence. Construction does not query Windows.
pub struct KeyboardConfigSession {
    dry_run: bool,
    settings: ManagedSettings<WindowsSettings>,
    layout: ManagedLayout<WindowsLayout>,
}

impl KeyboardConfigSession {
    /// The literal value `1` selects the read/log-only backend for this lifetime.
    pub fn from_env() -> Self {
        let dry_run = dry_run_enabled(std::env::var_os("WIRI_KEYBOARD_DRY_RUN").as_deref());
        Self {
            dry_run,
            settings: ManagedSettings::new(if dry_run {
                WindowsSettings::DryRun(DryRunWindows::default())
            } else {
                WindowsSettings::Native(NativeWindows)
            }),
            layout: ManagedLayout::new(WindowsLayout { dry_run }),
        }
    }

    pub fn apply(&mut self, input: &InputConfig) {
        if input.repeat_delay_configured {
            info!(
                dry_run = self.dry_run,
                requested_ms = input.repeat_delay,
                windows_ms = (delay_step(input.repeat_delay) + 1) * 250,
                "Keyboard repeat-delay approximation"
            );
        }
        if input.repeat_rate_configured {
            match speed_step(input.repeat_rate) {
                Some(speed) => info!(
                    dry_run = self.dry_run,
                    requested_per_second = input.repeat_rate,
                    windows_speed = speed,
                    approximate_per_second = approximate_rate(speed),
                    "Keyboard repeat-rate approximation (hardware dependent)"
                ),
                None => warn!("repeat-rate 0 disables repeat in niri; Windows speed 0 still repeats, so native repeat is left unchanged or restored"),
            }
        }
        self.settings.apply(RequestedSettings::from_input(input));
        self.layout.apply(input);
    }

    /// Idempotent. Removed options are also restored by `apply` on reload.
    pub fn restore(&mut self) {
        self.layout.restore();
        self.settings.restore();
    }
}

// Both managed fields independently restore on Drop, including early returns.
trait LayoutBackend {
    fn plan(&mut self, name: &str) -> Result<ConfiguredLayoutPlan, String>;
    /// false means foreground/layout ownership was lost, without posting.
    fn request(&mut self, plan: &ConfiguredLayoutPlan, restore: bool) -> Result<bool, String>;
}

struct WindowsLayout {
    dry_run: bool,
}

impl LayoutBackend for WindowsLayout {
    fn plan(&mut self, name: &str) -> Result<ConfiguredLayoutPlan, String> {
        plan_configured_layout(name)
    }

    fn request(&mut self, plan: &ConfiguredLayoutPlan, restore: bool) -> Result<bool, String> {
        if self.dry_run {
            info!(
                action = if restore { "restore" } else { "apply" },
                setting = "keyboard-layout",
                name = %plan.name,
                foreground_thread = plan.thread_id,
                original_hkl = format_args!("{:08X}", plan.original),
                selected_hkl = format_args!("{:08X}", plan.selected),
                "WIRI_KEYBOARD_DRY_RUN (request not posted)"
            );
            return Ok(true);
        }
        request_configured_layout(plan, restore)
    }
}

struct ManagedLayout<B: LayoutBackend> {
    backend: B,
    saved: Option<ConfiguredLayoutPlan>,
}

impl<B: LayoutBackend> ManagedLayout<B> {
    fn new(backend: B) -> Self {
        Self {
            backend,
            saved: None,
        }
    }

    fn apply(&mut self, input: &InputConfig) {
        if !input.keyboard_layout_configured {
            self.restore();
            return;
        }
        let mut next = match self.backend.plan(&input.keyboard_layout) {
            Ok(plan) => plan,
            Err(error) => {
                self.restore();
                warn!(%error, "Configured keyboard layout skipped");
                return;
            }
        };
        let mut original = next.original;
        if let Some(previous) = self.saved.as_ref() {
            if previous.window == next.window
                && previous.thread_id == next.thread_id
                && (next.original == previous.selected || next.original == previous.original)
            {
                // The original HKL can mean dry-run, a pending request, or a
                // rejected request. Never treat it as a new baseline.
                original = previous.original;
                if previous.selected == next.selected {
                    return;
                }
            } else {
                self.restore();
                if self.saved.is_some() {
                    return; // Retry failed restoration before owning another target.
                }
            }
        }
        if next.selected == original {
            self.restore();
            return;
        }
        // Apply is guarded against the observed HKL, not the first baseline.
        match self.backend.request(&next, false) {
            Ok(true) => {
                next.original = original;
                self.saved = Some(next);
            }
            Ok(false) => {
                self.saved = None;
                warn!("Keyboard layout target changed before request; relinquishing ownership");
            }
            Err(error) => warn!(%error, "Configured keyboard layout request failed"),
        }
    }

    fn restore(&mut self) {
        if let Some(previous) = self.saved.as_ref() {
            match self.backend.request(previous, true) {
                Ok(owned) => {
                    if !owned {
                        warn!("Keyboard layout foreground or HKL changed; relinquishing ownership without restoring");
                    }
                    self.saved = None;
                }
                Err(error) => warn!(%error, "Keyboard layout restore deferred"),
            }
        }
    }
}

impl<B: LayoutBackend> Drop for ManagedLayout<B> {
    fn drop(&mut self) {
        self.restore();
    }
}

fn dry_run_enabled(value: Option<&std::ffi::OsStr>) -> bool {
    value == Some(std::ffi::OsStr::new("1"))
}

/// Nearest Windows delay, with ties rounded up and endpoint saturation.
fn delay_step(milliseconds: u32) -> u32 {
    (milliseconds.saturating_sub(250).saturating_add(125) / 250).min(3)
}

/// A documented linear approximation of Windows' hardware-dependent speed.
/// None is intentionally NOT converted to speed 0 (2.5 repeats/second).
fn speed_step(repeats_per_second: u32) -> Option<u32> {
    if repeats_per_second == 0 {
        return None;
    }
    let twice_rate = u64::from(repeats_per_second) * 2;
    Some(((twice_rate.saturating_sub(5) * 31 + 27) / 55).min(31) as u32)
}

fn approximate_rate(speed: u32) -> f64 {
    2.5 + f64::from(speed) * 27.5 / 31.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Setting {
    Delay,
    Speed,
    NumLock,
}

impl Setting {
    const ALL: [Self; 3] = [Self::Delay, Self::Speed, Self::NumLock];

    fn name(self) -> &'static str {
        match self {
            Self::Delay => "repeat-delay",
            Self::Speed => "repeat-rate",
            Self::NumLock => "numlock",
        }
    }

    fn valid(self, value: u32) -> bool {
        value
            <= match self {
                Self::Delay => 3,
                Self::Speed => 31,
                Self::NumLock => 1,
            }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Apply,
    Restore,
}

#[derive(Default, Debug, PartialEq, Eq)]
struct RequestedSettings {
    delay: Option<u32>,
    speed: Option<u32>,
    numlock: Option<u32>,
}

impl RequestedSettings {
    fn from_input(input: &InputConfig) -> Self {
        Self {
            delay: input
                .repeat_delay_configured
                .then(|| delay_step(input.repeat_delay)),
            speed: input
                .repeat_rate_configured
                .then(|| speed_step(input.repeat_rate))
                .flatten(),
            numlock: input.numlock_configured.then_some(u32::from(input.numlock)),
        }
    }

    fn get(&self, setting: Setting) -> Option<u32> {
        match setting {
            Setting::Delay => self.delay,
            Setting::Speed => self.speed,
            Setting::NumLock => self.numlock,
        }
    }
}

// Private so tests can substitute a synthetic backend without invoking Windows.
trait SettingsBackend {
    fn read(&mut self, setting: Setting) -> Result<u32, String>;
    fn write(&mut self, setting: Setting, value: u32, phase: Phase) -> Result<(), String>;
}

#[derive(Clone, Copy, Debug)]
struct Saved {
    original: u32,
    applied: u32,
}

struct ManagedSettings<B: SettingsBackend> {
    backend: B,
    saved: BTreeMap<Setting, Saved>,
}

impl<B: SettingsBackend> ManagedSettings<B> {
    fn new(backend: B) -> Self {
        Self {
            backend,
            saved: BTreeMap::new(),
        }
    }

    fn apply(&mut self, request: RequestedSettings) {
        for setting in Setting::ALL {
            if let Err(error) = self.reconcile(setting, request.get(setting)) {
                warn!(option = setting.name(), %error, "Keyboard option skipped");
            }
        }
    }

    fn restore(&mut self) {
        for setting in Setting::ALL {
            if let Err(error) = self.reconcile(setting, None) {
                // Retain the snapshot so a later restore can retry.
                warn!(option = setting.name(), %error, "Keyboard restore deferred");
            }
        }
    }

    fn reconcile(&mut self, setting: Setting, desired: Option<u32>) -> Result<(), String> {
        if desired.is_none() && !self.saved.contains_key(&setting) {
            return Ok(()); // Defaults/unowned options must not even be queried.
        }
        let current = self.backend.read(setting)?;
        if !setting.valid(current) {
            return Err(format!(
                "invalid native keyboard value {current}; not changing it"
            ));
        }
        if self
            .saved
            .get(&setting)
            .is_some_and(|saved| saved.applied != current)
        {
            // Do not overwrite a user's change when removing/restoring options.
            // An explicit new request can acquire the new baseline below.
            self.saved.remove(&setting);
            warn!(
                option = setting.name(),
                "Keyboard value changed externally; relinquishing previous snapshot"
            );
        }
        match desired {
            Some(value) if value != current => {
                if !setting.valid(value) {
                    return Err(format!("invalid planned keyboard value {value}"));
                }
                let original = self
                    .saved
                    .get(&setting)
                    .map_or(current, |saved| saved.original);
                // A return to the baseline must also clear dry-run shadow state.
                let phase = if original == value {
                    Phase::Restore
                } else {
                    Phase::Apply
                };
                self.backend.write(setting, value, phase)?;
                if original == value {
                    self.saved.remove(&setting);
                } else {
                    self.saved.insert(
                        setting,
                        Saved {
                            original,
                            applied: value,
                        },
                    );
                }
            }
            None => {
                if let Some(saved) = self.saved.get(&setting) {
                    self.backend
                        .write(setting, saved.original, Phase::Restore)?;
                    self.saved.remove(&setting);
                }
            }
            _ => {} // An already-matching value does not create ownership.
        }
        Ok(())
    }
}

impl<B: SettingsBackend> Drop for ManagedSettings<B> {
    fn drop(&mut self) {
        self.restore();
    }
}

enum WindowsSettings {
    DryRun(DryRunWindows),
    Native(NativeWindows),
}

impl SettingsBackend for WindowsSettings {
    fn read(&mut self, setting: Setting) -> Result<u32, String> {
        match self {
            Self::DryRun(backend) => backend.read(setting),
            Self::Native(backend) => backend.read(setting),
        }
    }

    fn write(&mut self, setting: Setting, value: u32, phase: Phase) -> Result<(), String> {
        match self {
            Self::DryRun(backend) => backend.write(setting, value, phase),
            Self::Native(backend) => backend.write(setting, value, phase),
        }
    }
}

fn read_windows_setting(setting: Setting) -> Result<u32, String> {
    if setting == Setting::NumLock {
        return Err("NumLock skipped: Windows exposes only a queue-relative toggle state; a background daemon cannot safely capture and restore it".into());
    }
    let mut value = 0u32;
    let action = match setting {
        Setting::Delay => SPI_GETKEYBOARDDELAY,
        Setting::Speed => SPI_GETKEYBOARDSPEED,
        Setting::NumLock => unreachable!(),
    };
    unsafe {
        SystemParametersInfoW(
            action,
            0,
            Some((&mut value as *mut u32).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .map_err(|error| format!("could not read {}: {error}", setting.name()))?;
    Ok(value)
}

#[derive(Default)]
struct NativeWindows;

impl SettingsBackend for NativeWindows {
    fn read(&mut self, setting: Setting) -> Result<u32, String> {
        read_windows_setting(setting)
    }

    fn write(&mut self, setting: Setting, value: u32, phase: Phase) -> Result<(), String> {
        if !setting.valid(value) {
            return Err(format!("invalid native keyboard value {value}"));
        }
        let action = match setting {
            Setting::Delay => SPI_SETKEYBOARDDELAY,
            Setting::Speed => SPI_SETKEYBOARDSPEED,
            Setting::NumLock => return Err("native NumLock enforcement is unavailable without a trustworthy toggle-state baseline".into()),
        };
        // Both setters take uiParam and NULL pvParam. No profile persistence.
        unsafe {
            SystemParametersInfoW(action, value, None, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0))
        }
        .map_err(|error| format!("could not set {}: {error}", setting.name()))?;
        info!(action = ?phase, option = setting.name(), native_value = value, "Keyboard option changed (non-persistent, best-effort restoration)");
        Ok(())
    }
}

#[derive(Default)]
struct DryRunWindows {
    shadow: BTreeMap<Setting, u32>,
}

impl SettingsBackend for DryRunWindows {
    fn read(&mut self, setting: Setting) -> Result<u32, String> {
        if let Some(value) = self.shadow.get(&setting) {
            return Ok(*value);
        }
        // NumLock has no trustworthy native baseline on this background thread.
        // A fixed hypothetical baseline exercises planning only, never OS state.
        if setting == Setting::NumLock {
            return Ok(0);
        }
        read_windows_setting(setting)
    }

    fn write(&mut self, setting: Setting, value: u32, phase: Phase) -> Result<(), String> {
        info!(
            action = ?phase,
            option = setting.name(),
            native_value = value,
            "WIRI_KEYBOARD_DRY_RUN (system setting not changed)"
        );
        match phase {
            Phase::Apply => {
                self.shadow.insert(setting, value);
            }
            Phase::Restore => {
                self.shadow.remove(&setting);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::rc::Rc;

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Call {
        Read(Setting),
        Write(Setting, u32, Phase),
    }

    struct FakeState {
        values: BTreeMap<Setting, u32>,
        calls: Vec<Call>,
        fail_reads: BTreeSet<Setting>,
        fail_writes: BTreeSet<Setting>,
    }

    #[derive(Clone)]
    struct Fake(Rc<RefCell<FakeState>>);

    impl Fake {
        fn new() -> Self {
            Self(Rc::new(RefCell::new(FakeState {
                values: [
                    (Setting::Delay, 3),
                    (Setting::Speed, 10),
                    (Setting::NumLock, 0),
                ]
                .into(),
                calls: Vec::new(),
                fail_reads: BTreeSet::new(),
                fail_writes: BTreeSet::new(),
            })))
        }
        fn writes(&self) -> Vec<Call> {
            self.0
                .borrow()
                .calls
                .iter()
                .filter(|call| matches!(call, Call::Write(..)))
                .cloned()
                .collect()
        }
        fn value(&self, setting: Setting) -> u32 {
            self.0.borrow().values[&setting]
        }
    }

    impl SettingsBackend for Fake {
        fn read(&mut self, setting: Setting) -> Result<u32, String> {
            let mut state = self.0.borrow_mut();
            state.calls.push(Call::Read(setting));
            if state.fail_reads.contains(&setting) {
                return Err("synthetic read failure".into());
            }
            Ok(state.values[&setting])
        }
        fn write(&mut self, setting: Setting, value: u32, phase: Phase) -> Result<(), String> {
            let mut state = self.0.borrow_mut();
            state.calls.push(Call::Write(setting, value, phase));
            if state.fail_writes.contains(&setting) {
                return Err("synthetic write failure".into());
            }
            state.values.insert(setting, value);
            Ok(())
        }
    }

    fn delay(value: u32) -> RequestedSettings {
        RequestedSettings {
            delay: Some(value),
            ..Default::default()
        }
    }

    #[test]
    fn dry_run_gate_requires_literal_one() {
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("true"),
            Some("log.txt"),
        ] {
            assert!(!dry_run_enabled(value.map(std::ffi::OsStr::new)));
        }
        assert!(dry_run_enabled(Some(std::ffi::OsStr::new("1"))));
    }

    #[test]
    fn delay_keeps_milliseconds_and_rounds_with_saturation() {
        for (ms, step) in [
            (0, 0),
            (250, 0),
            (374, 0),
            (375, 1),
            (500, 1),
            (600, 1),
            (625, 2),
            (750, 2),
            (875, 3),
            (1000, 3),
            (u32::MAX, 3),
        ] {
            assert_eq!(delay_step(ms), step, "{ms} ms");
        }
    }

    #[test]
    fn rate_is_repeats_per_second_and_zero_is_not_windows_zero() {
        for (rate, step) in [
            (0, None),
            (1, Some(0)),
            (3, Some(1)),
            (25, Some(25)),
            (30, Some(31)),
            (40, Some(31)),
            (u32::MAX, Some(31)),
        ] {
            assert_eq!(speed_step(rate), step, "{rate} repeats/sec");
        }
        assert_eq!(approximate_rate(0), 2.5);
        assert_eq!(approximate_rate(31), 30.0);
    }

    #[test]
    fn primitive_defaults_are_not_explicit_configuration() {
        let mut input = InputConfig::default_config();
        input.repeat_delay = 500;
        input.repeat_rate = 30;
        input.numlock = true;
        assert_eq!(
            RequestedSettings::from_input(&input),
            RequestedSettings::default()
        );
        input.repeat_delay_configured = true;
        assert_eq!(RequestedSettings::from_input(&input), delay(1));
        input.repeat_rate_configured = true;
        input.numlock_configured = true;
        assert_eq!(
            RequestedSettings::from_input(&input),
            RequestedSettings {
                delay: Some(1),
                speed: Some(31),
                numlock: Some(1)
            }
        );
        input.numlock = false;
        input.repeat_rate = 0;
        assert_eq!(RequestedSettings::from_input(&input).numlock, Some(0));
        assert_eq!(RequestedSettings::from_input(&input).speed, None);
    }

    #[test]
    fn defaults_and_restore_before_apply_never_query_anything() {
        let fake = Fake::new();
        let mut session = ManagedSettings::new(fake.clone());
        session.apply(RequestedSettings::default());
        session.restore();
        drop(session);
        assert!(fake.0.borrow().calls.is_empty());
    }

    #[test]
    fn explicitly_configured_false_and_default_values_are_applied() {
        let fake = Fake::new();
        fake.0.borrow_mut().values.insert(Setting::NumLock, 1);
        let mut input = InputConfig::default_config();
        input.repeat_delay_configured = true;
        input.numlock_configured = true;
        let mut session = ManagedSettings::new(fake.clone());
        session.apply(RequestedSettings::from_input(&input));
        assert_eq!(
            fake.writes(),
            vec![
                Call::Write(Setting::Delay, 0, Phase::Apply),
                Call::Write(Setting::NumLock, 0, Phase::Apply)
            ]
        );
        assert!(!fake.0.borrow().calls.contains(&Call::Read(Setting::Speed)));
        session.restore();
        assert_eq!(fake.value(Setting::Delay), 3);
        assert_eq!(fake.value(Setting::NumLock), 1);
    }

    #[test]
    fn reload_preserves_first_baseline_and_identical_apply_is_idempotent() {
        let fake = Fake::new();
        let mut session = ManagedSettings::new(fake.clone());
        session.apply(delay(1));
        session.apply(delay(1));
        session.apply(delay(2));
        assert_eq!(fake.writes().len(), 2);
        session.restore();
        session.restore();
        assert_eq!(
            fake.writes(),
            vec![
                Call::Write(Setting::Delay, 1, Phase::Apply),
                Call::Write(Setting::Delay, 2, Phase::Apply),
                Call::Write(Setting::Delay, 3, Phase::Restore)
            ]
        );
    }

    #[test]
    fn removing_one_option_restores_only_that_option_then_drop_restores_rest() {
        let fake = Fake::new();
        {
            let mut session = ManagedSettings::new(fake.clone());
            session.apply(RequestedSettings {
                delay: Some(0),
                speed: Some(31),
                numlock: Some(1),
            });
            session.apply(RequestedSettings {
                speed: Some(31),
                ..Default::default()
            });
            assert_eq!(fake.value(Setting::Delay), 3);
            assert_eq!(fake.value(Setting::Speed), 31);
            assert_eq!(fake.value(Setting::NumLock), 0);
        }
        assert_eq!(fake.value(Setting::Speed), 10);
        assert_eq!(fake.writes().len(), 6);
    }

    #[test]
    fn rate_zero_releases_previously_managed_speed_without_disabling_native_repeat() {
        let fake = Fake::new();
        let mut session = ManagedSettings::new(fake.clone());
        let mut input = InputConfig::default_config();
        input.repeat_rate_configured = true;
        input.repeat_rate = 30;
        session.apply(RequestedSettings::from_input(&input));
        input.repeat_rate = 0;
        session.apply(RequestedSettings::from_input(&input));
        assert_eq!(
            fake.writes(),
            vec![
                Call::Write(Setting::Speed, 31, Phase::Apply),
                Call::Write(Setting::Speed, 10, Phase::Restore)
            ]
        );
    }

    #[test]
    fn already_matching_value_never_acquires_ownership() {
        let fake = Fake::new();
        let mut session = ManagedSettings::new(fake.clone());
        session.apply(delay(3));
        session.restore();
        assert!(session.saved.is_empty());
        assert!(fake.writes().is_empty());
    }

    #[test]
    fn failed_snapshot_and_invalid_snapshot_never_write_but_other_options_work() {
        let fake = Fake::new();
        fake.0.borrow_mut().fail_reads.insert(Setting::Delay);
        fake.0.borrow_mut().values.insert(Setting::Speed, 99);
        let mut session = ManagedSettings::new(fake.clone());
        session.apply(RequestedSettings {
            delay: Some(1),
            speed: Some(31),
            numlock: Some(1),
        });
        assert_eq!(
            fake.writes(),
            vec![Call::Write(Setting::NumLock, 1, Phase::Apply)]
        );
        session.restore();
        assert_eq!(fake.value(Setting::NumLock), 0);
    }

    #[test]
    fn failed_apply_does_not_claim_ownership_and_failed_restore_can_retry() {
        let fake = Fake::new();
        let mut session = ManagedSettings::new(fake.clone());
        fake.0.borrow_mut().fail_writes.insert(Setting::Delay);
        session.apply(delay(1));
        assert!(session.saved.is_empty());
        fake.0.borrow_mut().fail_writes.clear();
        session.apply(delay(1));
        fake.0.borrow_mut().fail_writes.insert(Setting::Delay);
        session.restore();
        assert!(session.saved.contains_key(&Setting::Delay));
        fake.0.borrow_mut().fail_writes.clear();
        session.restore();
        assert!(session.saved.is_empty());
        assert_eq!(fake.value(Setting::Delay), 3);
    }

    #[test]
    fn restore_does_not_clobber_external_changes() {
        let fake = Fake::new();
        let mut session = ManagedSettings::new(fake.clone());
        session.apply(delay(1));
        fake.0.borrow_mut().values.insert(Setting::Delay, 2);
        session.restore();
        assert_eq!(fake.value(Setting::Delay), 2);
        assert_eq!(fake.writes().len(), 1);
    }

    #[test]
    fn explicit_reload_after_external_change_acquires_a_new_baseline() {
        let fake = Fake::new();
        let mut session = ManagedSettings::new(fake.clone());
        session.apply(delay(1));
        fake.0.borrow_mut().values.insert(Setting::Delay, 2);
        session.apply(delay(0));
        session.restore();
        assert_eq!(fake.value(Setting::Delay), 2);
    }

    #[test]
    fn dry_run_return_to_baseline_clears_saved_and_shadow_without_os_reads() {
        // Seed so no native getter is called, even in this regression test.
        let mut backend = DryRunWindows::default();
        backend.shadow.insert(Setting::Delay, 3);
        let mut session = ManagedSettings::new(backend);
        session.apply(delay(0));
        session.apply(delay(3));
        assert!(session.saved.is_empty());
        assert!(session.backend.shadow.is_empty());
        session.restore();
    }

    #[derive(Clone)]
    struct FakeLayout(Rc<RefCell<FakeLayoutState>>);

    struct FakeLayoutState {
        plan: ConfiguredLayoutPlan,
        plans: usize,
        requests: Vec<(ConfiguredLayoutPlan, bool)>,
        fail: bool,
        owned: bool,
    }

    impl FakeLayout {
        fn new() -> Self {
            Self(Rc::new(RefCell::new(FakeLayoutState {
                plan: ConfiguredLayoutPlan {
                    window: 1,
                    thread_id: 2,
                    original: 10,
                    selected: 20,
                    name: "Synthetic".into(),
                },
                plans: 0,
                requests: Vec::new(),
                fail: false,
                owned: true,
            })))
        }
    }

    impl LayoutBackend for FakeLayout {
        fn plan(&mut self, _: &str) -> Result<ConfiguredLayoutPlan, String> {
            let mut state = self.0.borrow_mut();
            state.plans += 1;
            Ok(state.plan.clone())
        }
        fn request(&mut self, plan: &ConfiguredLayoutPlan, restore: bool) -> Result<bool, String> {
            let mut state = self.0.borrow_mut();
            state.requests.push((plan.clone(), restore));
            if state.fail {
                Err("synthetic post failure".into())
            } else {
                Ok(state.owned)
            }
        }
    }

    #[test]
    fn layout_defaults_do_not_query_and_reload_drop_preserve_first_baseline() {
        let fake = FakeLayout::new();
        let mut input = InputConfig::default_config();
        {
            let mut session = ManagedLayout::new(fake.clone());
            session.apply(&input);
            session.restore();
            assert_eq!(fake.0.borrow().plans, 0);
            input.keyboard_layout_configured = true;
            session.apply(&input);
            session.apply(&input); // Same pending/rejected or dry-run plan.
            assert_eq!(fake.0.borrow().requests.len(), 1);
            fake.0.borrow_mut().plan.original = 20;
            fake.0.borrow_mut().plan.selected = 30;
            session.apply(&input);
            assert_eq!(session.saved.as_ref().unwrap().original, 10);
        }
        let state = fake.0.borrow();
        assert_eq!(state.requests.len(), 3);
        assert_eq!(state.requests[2].0.original, 10);
        assert!(state.requests[2].1);
    }

    #[test]
    fn layout_restore_retries_errors_and_relinquishes_lost_foreground() {
        let fake = FakeLayout::new();
        let mut input = InputConfig::default_config();
        input.keyboard_layout_configured = true;
        let mut session = ManagedLayout::new(fake.clone());
        session.apply(&input);
        input.keyboard_layout_configured = false;
        fake.0.borrow_mut().fail = true;
        session.apply(&input);
        assert!(session.saved.is_some());
        fake.0.borrow_mut().fail = false;
        fake.0.borrow_mut().owned = false;
        session.restore();
        assert!(session.saved.is_none());
    }

    #[test]
    fn dry_run_layout_requests_do_not_query_or_post() {
        let mut backend = WindowsLayout { dry_run: true };
        let fake = FakeLayout::new();
        let plan = fake.0.borrow().plan.clone();
        assert_eq!(backend.request(&plan, false), Ok(true));
        assert_eq!(backend.request(&plan, true), Ok(true));
    }

    #[test]
    fn native_numlock_is_refused_without_query_or_injection() {
        let mut backend = NativeWindows;
        assert!(backend
            .read(Setting::NumLock)
            .unwrap_err()
            .contains("queue-relative"));
        assert!(backend.write(Setting::NumLock, 1, Phase::Apply).is_err());
        assert!(backend.write(Setting::NumLock, 0, Phase::Restore).is_err());
    }

    #[test]
    fn dry_run_numlock_uses_only_a_hypothetical_baseline_and_shadow() {
        let mut backend = DryRunWindows::default();
        assert_eq!(backend.read(Setting::NumLock), Ok(0));
        assert_eq!(backend.write(Setting::NumLock, 1, Phase::Apply), Ok(()));
        assert_eq!(backend.read(Setting::NumLock), Ok(1));
        assert_eq!(backend.write(Setting::NumLock, 0, Phase::Restore), Ok(()));
        assert_eq!(backend.read(Setting::NumLock), Ok(0));
    }

    #[test]
    fn reloading_the_original_value_releases_ownership() {
        let fake = Fake::new();
        let mut session = ManagedSettings::new(fake.clone());
        session.apply(delay(1));
        session.apply(delay(3));
        let count = fake.0.borrow().calls.len();
        session.restore();
        assert_eq!(fake.0.borrow().calls.len(), count);
        assert_eq!(fake.writes().len(), 2);
    }
}
