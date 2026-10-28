//! niri's pointing-device settings, and which of them Windows can actually be told about.
//!
//! niri configures every pointing device separately (`input { mouse/touchpad/trackpoint/trackball }`)
//! through libinput.  Windows has one system-wide pointer setup plus a handful of precision-touchpad
//! values, so the same config keys are handled in three ways:
//!
//! * **written to the OS** — `accel-profile` (Windows' "Enhance pointer precision"),
//!   `left-handed` (button swap), the touchpad's `dwt` (typing sensitivity) and
//!   `disabled-on-external-mouse` ("leave the touchpad on when a mouse is connected").  Only the
//!   options the config names are touched, the value the OS had before is remembered, and it is
//!   put back on a graceful quit or when a reload stops naming the option.
//! * **hook-only** — explicit mouse `on-button-down`/button/lock scrolling.
//! * **parsed and validated but not written** — unsupported scroll methods,
//!   device-scoped button scrolling, `middle-emulation`, touchpad `dwtp`, `drag`, `drag-lock`,
//!   `tap-button-map`, `click-method` and `off` for mouse/trackpoint/trackball: Windows exposes no
//!   public setting for them (they are reported at load time, never silently dropped).
//! * **handled by wiri itself** — touchpad `off` and the `toggle-touchpad` action stop wiri's own
//!   raw-input reading of the touchpad (`crate::input::touchpad`).
//!
//! Every OS write goes through [`PointerSettingsBackend`], one small trait: unit tests use a fake
//! and assert the exact writes and restores, and the live harness wraps the real backend in
//! [`Audited`], which records the writes it *would* make in
//! `WIRI_POINTER_SETTINGS_DRY_RUN=<file>` instead of making them.  There is no polling here: a
//! config load, a reload and a quit are the only callers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use tracing::{info, warn};

use crate::config::InputConfig;

/// The environment variable that turns the real backend into a recorder (the live harness sets
/// it; it names an append-only NDJSON file).
pub const DRY_RUN_ENV: &str = "WIRI_POINTER_SETTINGS_DRY_RUN";

/// Windows' default acceleration thresholds (`SPI_GETMOUSE`): two speed thresholds and the
/// acceleration flag.  What "Enhance pointer precision" on means.
pub const ADAPTIVE_THRESHOLDS: [i32; 3] = [6, 10, 1];
/// "Enhance pointer precision" off: no thresholds, no acceleration.
pub const FLAT_THRESHOLDS: [i32; 3] = [0, 0, 0];
/// The precision touchpad's `AAPThreshold` with the strongest palm/typing rejection ("Low
/// sensitivity" in Settings) — the nearest thing Windows has to niri's `dwt`.
pub const TOUCHPAD_TYPING_AGGRESSIVE: i32 = 3;
/// `AAPThreshold` with no extra rejection ("Most sensitive").
pub const TOUCHPAD_TYPING_OFF: i32 = 0;

/// One setting the OS keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Setting {
    /// `SPI_GETMOUSE`/`SPI_SETMOUSE`: the two speed thresholds and the acceleration flag.
    EnhancedPrecision,
    /// `SM_SWAPBUTTON`/`SwapMouseButton`: primary and secondary buttons exchanged.
    ButtonSwap,
    /// Precision touchpad `HKCU\...\PrecisionTouchPad\AAPThreshold` (typing sensitivity).
    TouchpadTypingSensitivity,
    /// Precision touchpad `HKCU\...\PrecisionTouchPad\LeaveOnWithMouse`.
    TouchpadLeaveOnWithMouse,
    /// `SPI_GETMOUSEVANISH`/`SPI_SETMOUSEVANISH`: the per-user "hide pointer while typing"
    /// flag (niri `cursor { hide-when-typing }`).
    HidePointerWhileTyping,
}

impl Setting {
    /// The name used in log lines and in the dry-run audit file.
    pub fn name(self) -> &'static str {
        match self {
            Self::EnhancedPrecision => "enhanced-pointer-precision",
            Self::ButtonSwap => "mouse-button-swap",
            Self::TouchpadTypingSensitivity => "touchpad-typing-sensitivity",
            Self::TouchpadLeaveOnWithMouse => "touchpad-leave-on-with-mouse",
            Self::HidePointerWhileTyping => "hide-pointer-while-typing",
        }
    }

    /// What the value means, for the log.
    pub fn describe(self, value: Value) -> String {
        match (self, value) {
            (Self::EnhancedPrecision, Value::Thresholds([_, _, acceleration])) => {
                if acceleration != 0 {
                    "\"adaptive\" (Enhance pointer precision on)".to_string()
                } else {
                    "\"flat\" (Enhance pointer precision off)".to_string()
                }
            }
            (Self::ButtonSwap, Value::Int(swapped)) => {
                if swapped != 0 {
                    "right-handed".to_string()
                } else {
                    "left-handed (buttons swapped)".to_string()
                }
            }
            (Self::TouchpadTypingSensitivity, Value::Int(level)) => {
                format!("typing sensitivity {}", level)
            }
            (Self::TouchpadLeaveOnWithMouse, Value::Int(leave)) => {
                if leave != 0 {
                    "touchpad stays on with a mouse".to_string()
                } else {
                    "touchpad off while a mouse is connected".to_string()
                }
            }
            (Self::HidePointerWhileTyping, Value::Int(vanish)) => {
                if vanish != 0 {
                    "the pointer hides while typing".to_string()
                } else {
                    "the pointer never hides while typing".to_string()
                }
            }
            (_, other) => format!("{other:?}"),
        }
    }
}

/// A setting's value as the OS stores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    /// One integer: a registry DWORD, a system metric, a flag as 0/1.
    Int(i32),
    /// The three integers of `SPI_GETMOUSE`.
    Thresholds([i32; 3]),
}

impl Value {
    /// The integers inside, for the audit file.
    pub fn numbers(self) -> Vec<i32> {
        match self {
            Self::Int(value) => vec![value],
            Self::Thresholds(values) => values.to_vec(),
        }
    }
}

/// One setting wiri wants the OS to have, and the config key that asked for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wanted {
    pub setting: Setting,
    pub value: Value,
    /// The niri option, for the log.
    pub option: &'static str,
}

/// The only way pointer settings reach the OS.  The real implementation is
/// [`SystemPointerSettings`]; tests use a fake, and [`Audited`] wraps any backend to record what
/// it would do without doing it.
pub trait PointerSettingsBackend: Send + Sync {
    /// The value the OS has now, or `None` when it has none (a registry value that is not set, or
    /// a setting this Windows build does not expose).
    fn read(&self, setting: Setting) -> Option<Value>;
    /// Store `value`.  The error text is logged; a failed write is not remembered, because
    /// nothing changed.
    fn write(&self, setting: Setting, value: Value) -> Result<(), String>;
    /// Remove the setting, so that it is "not there" again.
    fn remove(&self, setting: Setting) -> Result<(), String>;
}

/// A shared backend is a backend: the applier holds an `Arc`, and [`Audited`] can wrap one.
impl<B: PointerSettingsBackend + ?Sized> PointerSettingsBackend for Arc<B> {
    fn read(&self, setting: Setting) -> Option<Value> {
        (**self).read(setting)
    }

    fn write(&self, setting: Setting, value: Value) -> Result<(), String> {
        (**self).write(setting, value)
    }

    fn remove(&self, setting: Setting) -> Result<(), String> {
        (**self).remove(setting)
    }
}

/// What wiri has changed, and what the OS had before it did.
pub struct PointerSettings {
    backend: Arc<dyn PointerSettingsBackend>,
    /// One entry per setting wiri wrote: the OS value from before the first write
    /// (`None`: the setting was absent).
    originals: BTreeMap<Setting, Option<Value>>,
}

impl PointerSettings {
    pub fn new(backend: Arc<dyn PointerSettingsBackend>) -> Self {
        Self {
            backend,
            originals: BTreeMap::new(),
        }
    }

    /// Make the OS match `wanted`, touching only what it names.  A setting wiri changed earlier
    /// that `wanted` no longer names goes back to what it was: that is what a reload that removes
    /// an option does.
    ///
    /// Returns one line per change, for the log.
    pub fn apply(&mut self, wanted: &[Wanted]) -> Vec<String> {
        let mut lines = Vec::new();
        let desired: BTreeMap<Setting, Wanted> = wanted
            .iter()
            .map(|wanted| (wanted.setting, *wanted))
            .collect();
        // Options that are no longer configured: put the OS back.
        let dropped: Vec<Setting> = self
            .originals
            .keys()
            .copied()
            .filter(|setting| !desired.contains_key(setting))
            .collect();
        for setting in dropped {
            lines.extend(self.restore_setting(setting));
        }
        for (setting, wanted) in desired {
            let current = self.backend.read(setting);
            if current == Some(wanted.value) {
                // Already what the config asks for: nothing written, nothing to restore.
                continue;
            }
            match self.backend.write(setting, wanted.value) {
                Ok(()) => {
                    let original = *self.originals.entry(setting).or_insert(current);
                    lines.push(format!(
                        "pointer settings: {} = {} ({})",
                        setting.name(),
                        setting.describe(wanted.value),
                        wanted.option
                    ));
                    // The OS is back at the value it started with: no restore is owed.
                    if original == Some(wanted.value) {
                        self.originals.remove(&setting);
                    }
                }
                Err(error) => warn!(
                    "pointer settings: could not set {} ({}): {}",
                    setting.name(),
                    wanted.option,
                    error
                ),
            }
        }
        lines
    }

    /// Put everything wiri changed back, on a graceful quit.
    pub fn restore(&mut self) -> Vec<String> {
        let settings: Vec<Setting> = self.originals.keys().copied().collect();
        let mut lines = Vec::new();
        for setting in settings {
            lines.extend(self.restore_setting(setting));
        }
        lines
    }

    fn restore_setting(&mut self, setting: Setting) -> Vec<String> {
        let Some(original) = self.originals.remove(&setting) else {
            return Vec::new();
        };
        let result = match original {
            Some(value) => self.backend.write(setting, value),
            None => self.backend.remove(setting),
        };
        match result {
            Ok(()) => vec![format!(
                "pointer settings: {} restored to {}",
                setting.name(),
                original
                    .map(|value| setting.describe(value))
                    .unwrap_or_else(|| "unset".to_string())
            )],
            Err(error) => {
                warn!(
                    "pointer settings: could not restore {}: {}",
                    setting.name(),
                    error
                );
                Vec::new()
            }
        }
    }

    /// The settings wiri currently owns (unit tests and the log).
    pub fn changed(&self) -> Vec<Setting> {
        self.originals.keys().copied().collect()
    }
}

/// The options niri has that Windows gives no lever for.  They are validated when the config
/// loads and reported here once per load, so a user never wonders why nothing happened.
pub fn unapplied(input: &InputConfig) -> Vec<&'static str> {
    let mut options = Vec::new();
    let button_scroll_applied = super::scroll_emulation::applied(input);
    if input.scroll_method.is_some() && !button_scroll_applied {
        options.push("input scroll-method");
    }
    if input.scroll_button.is_some() && !button_scroll_applied {
        options.push("input scroll-button");
    }
    if input.scroll_button_lock.is_some() && !button_scroll_applied {
        options.push("input scroll-button-lock");
    }
    if input.non_mouse_scroll_configured {
        options.push("non-mouse device-scoped scrolling");
    }
    if input.middle_emulation.is_some() {
        options.push("input middle-emulation");
    }
    if input.touchpad_dwtp.is_some() {
        options.push("touchpad dwtp");
    }
    if input.touchpad_drag.is_some() {
        options.push("touchpad drag");
    }
    if input.touchpad_drag_lock.is_some() {
        options.push("touchpad drag-lock");
    }
    if input.touchpad_tap_button_map.is_some() {
        options.push("touchpad tap-button-map");
    }
    if input.touchpad_click_method.is_some() {
        options.push("touchpad click-method");
    }
    if input.mouse_off {
        options.push("input mouse off");
    }
    if input.trackpoint_off {
        options.push("input trackpoint off");
    }
    if input.trackball_off {
        options.push("input trackball off");
    }
    options
}

/// Which OS settings the config asks for.  Only options the user wrote appear here.
pub fn wanted(input: &InputConfig) -> Vec<Wanted> {
    let mut wanted = Vec::new();
    if let Some(profile) = input.accel_profile {
        use crate::config::AccelProfile;
        wanted.push(Wanted {
            setting: Setting::EnhancedPrecision,
            value: Value::Thresholds(match profile {
                AccelProfile::Flat => FLAT_THRESHOLDS,
                AccelProfile::Adaptive => ADAPTIVE_THRESHOLDS,
            }),
            option: "accel-profile",
        });
    }
    if let Some(left_handed) = input.left_handed {
        wanted.push(Wanted {
            setting: Setting::ButtonSwap,
            value: Value::Int(i32::from(left_handed)),
            option: "left-handed",
        });
    }
    if let Some(dwt) = input.touchpad_dwt {
        wanted.push(Wanted {
            setting: Setting::TouchpadTypingSensitivity,
            value: Value::Int(if dwt {
                TOUCHPAD_TYPING_AGGRESSIVE
            } else {
                TOUCHPAD_TYPING_OFF
            }),
            option: "touchpad dwt",
        });
    }
    if let Some(disabled) = input.touchpad_disabled_on_external_mouse {
        // niri: `disabled-on-external-mouse` on means the touchpad stops working while a mouse is
        // connected; Windows calls the same switch "leave the touchpad on when a mouse is
        // connected", so the value is inverted.
        wanted.push(Wanted {
            setting: Setting::TouchpadLeaveOnWithMouse,
            value: Value::Int(if disabled { 0 } else { 1 }),
            option: "touchpad disabled-on-external-mouse",
        });
    }
    wanted
}

/// niri `cursor { hide-when-typing }` is Windows' own per-user "hide pointer while typing"
/// (`SPI_GETMOUSEVANISH`/`SPI_SETMOUSEVANISH`).  Everything else under `cursor` is not a
/// setting wiri owns (`xcursor-theme`/`xcursor-size` are the Windows pointer scheme, and
/// `hide-after-inactive-ms` is handled by `crate::input::cursor_hide`).
pub fn wanted_cursor(cursor: &crate::config::CursorConfig) -> Vec<Wanted> {
    let mut wanted = Vec::new();
    if let Some(hide) = cursor.hide_when_typing {
        wanted.push(Wanted {
            setting: Setting::HidePointerWhileTyping,
            value: Value::Int(i32::from(hide)),
            option: "cursor hide-when-typing",
        });
    }
    wanted
}

/// The `(uiParam, pvParam)` pair `SystemParametersInfoW(SPI_SETMOUSEVANISH, ..)` takes for
/// "hide pointer while typing" on or off.
///
/// Unlike the older BOOL setters (`SPI_SETBEEP`), this one carries the new state in `pvParam`
/// (`TRUE` / `FALSE` cast to a pointer) and requires `uiParam` to be 0: Windows answers
/// `ERROR_INVALID_PARAMETER` (87) to `uiParam = 1`, which is what the first version of the
/// real backend sent for "on" (and `pvParam = NULL` is `FALSE`, so "on" could never take effect
/// and a restore of the user's original "on" failed as well).  Measured on a Windows 11 machine
/// from a non-interactive session: `uiParam = 1` -> 87 whatever `pvParam` is, while `uiParam = 0`
/// passes the parameter check and only then stops at `ERROR_REQUIRES_INTERACTIVE_WINDOWSTATION`
/// (1459).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) const fn mouse_vanish_set_arguments(on: bool) -> (u32, usize) {
    (0, on as usize)
}

// ---------------------------------------------------------------------------------------
// the runtime state: one applier, plus the `toggle-touchpad` switch
// ---------------------------------------------------------------------------------------

static SETTINGS: Mutex<Option<PointerSettings>> = parking_lot::const_mutex(None);
/// `toggle-touchpad` flipped the touchpad off (niri: reset on restart, combined with `off`).
static TOUCHPAD_TOGGLED_OFF: AtomicBool = AtomicBool::new(false);
static EVENT_TX: Mutex<Option<tokio::sync::broadcast::Sender<crate::ipc::IpcEvent>>> =
    parking_lot::const_mutex(None);

/// The real backend, or the recorder when [`DRY_RUN_ENV`] names a file.
fn backend_from_env() -> Arc<dyn PointerSettingsBackend> {
    match std::env::var_os(DRY_RUN_ENV).filter(|path| !path.is_empty()) {
        Some(path) => {
            let path = PathBuf::from(path);
            info!(
                "pointer settings: dry run, writes are recorded in {}",
                path.display()
            );
            Arc::new(Audited::new(SystemPointerSettings, path))
        }
        None => Arc::new(SystemPointerSettings),
    }
}

/// Apply the pointer options of a loaded (or reloaded) config.  Options the config no longer
/// names are put back to what the OS had before wiri changed them.
pub fn apply_config(input: &InputConfig, cursor: &crate::config::CursorConfig) {
    super::scroll_emulation::configure(input);
    let mut guard = SETTINGS.lock();
    let settings = guard.get_or_insert_with(|| PointerSettings::new(backend_from_env()));
    let mut wanted = wanted(input);
    wanted.extend(wanted_cursor(cursor));
    for line in settings.apply(&wanted) {
        info!("{}", line);
    }
    let unapplied = unapplied(input);
    if !unapplied.is_empty() {
        info!(
            "pointer settings: {} configured without a Windows equivalent ({})",
            unapplied.len(),
            unapplied.join(", ")
        );
    }
}

/// Restore every setting wiri changed (graceful quit).
pub fn restore() {
    super::scroll_emulation::cancel();
    if let Some(settings) = SETTINGS.lock().as_mut() {
        for line in settings.restore() {
            info!("{}", line);
        }
    }
}

/// The touchpad switch `toggle-touchpad` flips: `configured` is `input { touchpad { } }` without
/// `off`.  Returns whether the touchpad is on afterwards.
pub fn toggle_touchpad(configured: bool) -> bool {
    let was_off = TOUCHPAD_TOGGLED_OFF.fetch_xor(true, Ordering::AcqRel);
    let on = configured && was_off;
    emit_touchpad_toggled(on);
    on
}

/// Whether wiri should read the touchpad: the config enables it and the toggle did not turn it off.
pub fn touchpad_active(configured: bool) -> bool {
    configured && !TOUCHPAD_TOGGLED_OFF.load(Ordering::Acquire)
}

/// The touchpad state after a reload (niri: the config's `off` is combined with the toggle).
pub fn touchpad_on_after_reload(configured: bool) -> bool {
    touchpad_active(configured)
}

pub(crate) fn set_event_sender(tx: tokio::sync::broadcast::Sender<crate::ipc::IpcEvent>) {
    *EVENT_TX.lock() = Some(tx);
}

fn emit_touchpad_toggled(on: bool) {
    if let Some(tx) = EVENT_TX.lock().as_ref() {
        // Nobody listening is not an error (niri sends the event to its subscribers).
        let _ = tx.send(crate::ipc::IpcEvent::TouchpadToggled { on });
    }
}

// ---------------------------------------------------------------------------------------
// the recorder used by the live harness
// ---------------------------------------------------------------------------------------

/// Wraps a backend and records the writes it is asked to make instead of making them.
///
/// Reads still go to the wrapped backend, so the applier compares against the real values; only
/// writes and removals are diverted.  This is how the live phase proves that a config naming
/// pointer options changes nothing on the test machine while the audit file shows exactly what a
/// real run would have written.
pub struct Audited<B: PointerSettingsBackend> {
    inner: B,
    path: PathBuf,
}

impl<B: PointerSettingsBackend> Audited<B> {
    pub fn new(inner: B, path: impl Into<PathBuf>) -> Self {
        Self {
            inner,
            path: path.into(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn record(&self, action: &str, setting: Setting, value: Option<Value>) -> Result<(), String> {
        let line = serde_json::json!({
            "action": action,
            "setting": setting.name(),
            "value": value.map(Value::numbers),
        });
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| format!("cannot open {}: {}", self.path.display(), error))?;
        writeln!(file, "{}", line).map_err(|error| format!("cannot write audit line: {}", error))
    }
}

impl<B: PointerSettingsBackend> PointerSettingsBackend for Audited<B> {
    fn read(&self, setting: Setting) -> Option<Value> {
        self.inner.read(setting)
    }

    fn write(&self, setting: Setting, value: Value) -> Result<(), String> {
        self.record("write", setting, Some(value))
    }

    fn remove(&self, setting: Setting) -> Result<(), String> {
        self.record("remove", setting, None)
    }
}

// ---------------------------------------------------------------------------------------
// the real backend
// ---------------------------------------------------------------------------------------

/// The precision touchpad's registry key (the documented place for these two values).
#[cfg(any(target_os = "windows", test))]
const TOUCHPAD_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\PrecisionTouchPad";

/// The real settings: `SystemParametersInfo`, `SwapMouseButton` and the precision-touchpad
/// registry values.
pub struct SystemPointerSettings;

impl PointerSettingsBackend for SystemPointerSettings {
    fn read(&self, setting: Setting) -> Option<Value> {
        #[cfg(target_os = "windows")]
        {
            system::read(setting)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = setting;
            None
        }
    }

    fn write(&self, setting: Setting, value: Value) -> Result<(), String> {
        // A unit test must never touch the machine's real pointer settings, even by accident:
        // tests use a fake backend, and this guard makes that unconditional.
        if cfg!(test) {
            return Ok(());
        }
        #[cfg(target_os = "windows")]
        {
            system::write(setting, value)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (setting, value);
            Err("pointer settings are only implemented on Windows".into())
        }
    }

    fn remove(&self, setting: Setting) -> Result<(), String> {
        if cfg!(test) {
            return Ok(());
        }
        #[cfg(target_os = "windows")]
        {
            system::remove(setting)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = setting;
            Err("pointer settings are only implemented on Windows".into())
        }
    }
}

#[cfg(target_os = "windows")]
mod system {
    use super::{Setting, Value, TOUCHPAD_KEY};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_DWORD,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::SwapMouseButton;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SendMessageTimeoutW, SystemParametersInfoW, HWND_BROADCAST,
        SMTO_ABORTIFHUNG, SPI_GETMOUSE, SPI_GETMOUSEVANISH, SPI_SETMOUSE, SPI_SETMOUSEVANISH,
        SYSTEM_METRICS_INDEX, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WM_SETTINGCHANGE,
    };

    /// `SM_SWAPBUTTON`: nonzero when the primary and secondary buttons are exchanged.
    const SM_SWAPBUTTON: i32 = 23;

    pub(super) fn read(setting: Setting) -> Option<Value> {
        match setting {
            Setting::EnhancedPrecision => {
                let mut thresholds = [0i32; 3];
                unsafe {
                    SystemParametersInfoW(
                        SPI_GETMOUSE,
                        0,
                        Some(thresholds.as_mut_ptr().cast()),
                        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                    )
                    .ok()?;
                }
                Some(Value::Thresholds(thresholds))
            }
            Setting::ButtonSwap => {
                let swapped = unsafe { GetSystemMetrics(SYSTEM_METRICS_INDEX(SM_SWAPBUTTON)) };
                Some(Value::Int(swapped))
            }
            Setting::TouchpadTypingSensitivity => read_touchpad("AAPThreshold").map(Value::Int),
            Setting::TouchpadLeaveOnWithMouse => read_touchpad("LeaveOnWithMouse").map(Value::Int),
            Setting::HidePointerWhileTyping => {
                let mut vanish = windows::Win32::Foundation::BOOL(0);
                unsafe {
                    SystemParametersInfoW(
                        SPI_GETMOUSEVANISH,
                        0,
                        Some(&mut vanish as *mut _ as *mut _),
                        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                    )
                    .ok()?;
                }
                Some(Value::Int(i32::from(vanish.as_bool())))
            }
        }
    }

    pub(super) fn write(setting: Setting, value: Value) -> Result<(), String> {
        match (setting, value) {
            (Setting::EnhancedPrecision, Value::Thresholds(thresholds)) => {
                let result = unsafe {
                    SystemParametersInfoW(
                        SPI_SETMOUSE,
                        0,
                        Some(thresholds.as_ptr() as *mut _),
                        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                    )
                };
                result.map_err(|error| error.to_string())
            }
            (Setting::ButtonSwap, Value::Int(swapped)) => {
                let previous =
                    unsafe { SwapMouseButton(windows::Win32::Foundation::BOOL(swapped)) };
                tracing::debug!(
                    "pointer settings: SwapMouseButton({}) returned {:?}",
                    swapped,
                    previous
                );
                Ok(())
            }
            (Setting::TouchpadTypingSensitivity, Value::Int(level)) => {
                write_touchpad("AAPThreshold", level)
            }
            (Setting::TouchpadLeaveOnWithMouse, Value::Int(leave)) => {
                write_touchpad("LeaveOnWithMouse", leave)
            }
            (Setting::HidePointerWhileTyping, Value::Int(vanish)) => {
                // The same switch as Settings > Accessibility > Mouse pointer > "hide pointer
                // while typing".  The new state travels in `pvParam`, not `uiParam` (see
                // `mouse_vanish_set_arguments`).
                let (ui_param, pv_param) = super::mouse_vanish_set_arguments(vanish != 0);
                let result = unsafe {
                    SystemParametersInfoW(
                        SPI_SETMOUSEVANISH,
                        ui_param,
                        Some(pv_param as *mut core::ffi::c_void),
                        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                    )
                };
                result.map_err(|error| error.to_string())
            }
            (setting, value) => Err(format!("{} does not take {:?}", setting.name(), value)),
        }
    }

    pub(super) fn remove(setting: Setting) -> Result<(), String> {
        match setting {
            Setting::TouchpadTypingSensitivity => remove_touchpad("AAPThreshold"),
            Setting::TouchpadLeaveOnWithMouse => remove_touchpad("LeaveOnWithMouse"),
            // The system-wide settings always have a value: "removing" one means nothing.
            other => Err(format!("{} cannot be removed", other.name())),
        }
    }

    /// The precision-touchpad key, opened for reading.  `HKCU` always exists; the subkey may not
    /// on a machine without a precision touchpad.
    fn touchpad_key_read() -> Option<HKEY> {
        let mut key = HKEY::default();
        let path: Vec<u16> = TOUCHPAD_KEY.encode_utf16().chain(Some(0)).collect();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(path.as_ptr()),
                0,
                KEY_QUERY_VALUE,
                &mut key,
            )
        };
        status.is_ok().then_some(key)
    }

    /// The same key, opened for changing a value.  `KEY_SET_VALUE` alone cannot read, and the
    /// write path never needs to.
    fn touchpad_key_write() -> Option<HKEY> {
        let mut key = HKEY::default();
        let path: Vec<u16> = TOUCHPAD_KEY.encode_utf16().chain(Some(0)).collect();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(path.as_ptr()),
                0,
                KEY_SET_VALUE,
                &mut key,
            )
        };
        status.is_ok().then_some(key)
    }

    fn read_touchpad(name: &str) -> Option<i32> {
        use windows::Win32::System::Registry::REG_VALUE_TYPE;
        let key = touchpad_key_read()?;
        let mut data = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let mut kind = REG_VALUE_TYPE::default();
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let status = unsafe {
            RegQueryValueExW(
                key,
                PCWSTR(wide.as_ptr()),
                None,
                Some(&mut kind),
                Some(&mut data as *mut u32 as *mut u8),
                Some(&mut size),
            )
        };
        unsafe {
            let _ = RegCloseKey(key);
        }
        (status.is_ok() && kind == REG_DWORD && size == std::mem::size_of::<u32>() as u32)
            .then_some(data as i32)
    }

    fn write_touchpad(name: &str, value: i32) -> Result<(), String> {
        let key = touchpad_key_write().ok_or("the precision touchpad registry key is not there")?;
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let data = value as u32;
        let status = unsafe {
            RegSetValueExW(
                key,
                PCWSTR(wide.as_ptr()),
                0,
                REG_DWORD,
                Some(&data.to_ne_bytes()),
            )
        };
        unsafe {
            let _ = RegCloseKey(key);
        }
        if !status.is_ok() {
            return Err(format!("RegSetValueExW({name}) failed: {:?}", status));
        }
        // The touchpad stack re-reads its values on a settings change; without this the write is
        // only seen at the next sign-in (approximation: the value name in lParam is not
        // documented, and a driver that ignores it keeps the old setting until then).
        notify_touchpad_stack();
        Ok(())
    }

    fn remove_touchpad(name: &str) -> Result<(), String> {
        let key = touchpad_key_write().ok_or("the precision touchpad registry key is not there")?;
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let status = unsafe { RegDeleteValueW(key, PCWSTR(wide.as_ptr())) };
        unsafe {
            let _ = RegCloseKey(key);
        }
        if !status.is_ok() {
            return Err(format!("RegDeleteValueW({name}) failed: {:?}", status));
        }
        notify_touchpad_stack();
        Ok(())
    }

    fn notify_touchpad_stack() {
        let section: Vec<u16> = "PrecisionTouchPad".encode_utf16().chain(Some(0)).collect();
        unsafe {
            let _ = SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                WPARAM(0),
                LPARAM(section.as_ptr() as isize),
                SMTO_ABORTIFHUNG,
                100,
                None,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex as ParkingMutex;

    /// A backend that records what it was asked to do and keeps the values it was given.
    #[derive(Default)]
    struct Fake {
        values: ParkingMutex<BTreeMap<Setting, Value>>,
        writes: ParkingMutex<Vec<(Setting, Value)>>,
        removes: ParkingMutex<Vec<Setting>>,
    }

    impl Fake {
        fn with(values: &[(Setting, Value)]) -> Self {
            let fake = Self::default();
            for (setting, value) in values {
                fake.values.lock().insert(*setting, *value);
            }
            fake
        }

        fn writes(&self) -> Vec<(Setting, Value)> {
            self.writes.lock().clone()
        }

        fn removes(&self) -> Vec<Setting> {
            self.removes.lock().clone()
        }

        fn values(&self) -> BTreeMap<Setting, Value> {
            self.values.lock().clone()
        }
    }

    impl PointerSettingsBackend for Fake {
        fn read(&self, setting: Setting) -> Option<Value> {
            self.values.lock().get(&setting).copied()
        }

        fn write(&self, setting: Setting, value: Value) -> Result<(), String> {
            self.writes.lock().push((setting, value));
            self.values.lock().insert(setting, value);
            Ok(())
        }

        fn remove(&self, setting: Setting) -> Result<(), String> {
            self.removes.lock().push(setting);
            self.values.lock().remove(&setting);
            Ok(())
        }
    }

    fn input(text: &str) -> InputConfig {
        let decoded = crate::config::decode::decode_text(text, "", None);
        assert!(
            decoded.issues.is_empty(),
            "unexpected issues: {:?}",
            decoded.issues
        );
        decoded.config.input
    }

    fn cursor(text: &str) -> crate::config::CursorConfig {
        let decoded = crate::config::decode::decode_text(text, "", None);
        assert!(
            decoded.issues.is_empty(),
            "unexpected issues: {:?}",
            decoded.issues
        );
        decoded.config.cursor
    }

    #[test]
    fn hide_when_typing_writes_spi_mousevanish_and_restores_the_original() {
        let fake = Arc::new(Fake::with(&[(
            Setting::HidePointerWhileTyping,
            Value::Int(0),
        )]));
        let mut settings = PointerSettings::new(fake.clone());
        let lines = settings.apply(&wanted_cursor(&cursor(
            "cursor {\n    hide-when-typing\n}\n",
        )));
        // Exactly one write: the Windows flag turned on, nothing else.
        assert_eq!(
            fake.writes(),
            vec![(Setting::HidePointerWhileTyping, Value::Int(1))]
        );
        assert!(fake.removes().is_empty());
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("cursor hide-when-typing"), "{}", lines[0]);
        assert_eq!(settings.changed(), vec![Setting::HidePointerWhileTyping]);
        // A graceful quit writes back the value the machine had (0), not a guessed default.
        settings.restore();
        assert_eq!(
            fake.writes(),
            vec![
                (Setting::HidePointerWhileTyping, Value::Int(1)),
                (Setting::HidePointerWhileTyping, Value::Int(0))
            ]
        );
        assert_eq!(
            fake.values().get(&Setting::HidePointerWhileTyping),
            Some(&Value::Int(0))
        );
        assert!(settings.changed().is_empty());
    }

    #[test]
    fn hide_when_typing_that_the_os_already_has_is_neither_written_nor_restored() {
        let fake = Arc::new(Fake::with(&[(
            Setting::HidePointerWhileTyping,
            Value::Int(1),
        )]));
        let mut settings = PointerSettings::new(fake.clone());
        let lines = settings.apply(&wanted_cursor(&cursor("cursor { hide-when-typing; }")));
        assert!(fake.writes().is_empty(), "{:?}", fake.writes());
        assert!(lines.is_empty());
        assert!(settings.restore().is_empty());
        assert!(fake.writes().is_empty());
    }

    #[test]
    fn a_reload_that_drops_hide_when_typing_puts_the_os_flag_back() {
        let fake = Arc::new(Fake::with(&[(
            Setting::HidePointerWhileTyping,
            Value::Int(0),
        )]));
        let mut settings = PointerSettings::new(fake.clone());
        settings.apply(&wanted_cursor(&cursor("cursor { hide-when-typing; }")));
        assert_eq!(
            fake.values().get(&Setting::HidePointerWhileTyping),
            Some(&Value::Int(1))
        );
        // The new config says nothing about the cursor: the reload restores the old value.
        let lines = settings.apply(&wanted_cursor(&cursor("")));
        assert_eq!(
            fake.values().get(&Setting::HidePointerWhileTyping),
            Some(&Value::Int(0))
        );
        assert!(!lines.is_empty());
        assert!(settings.changed().is_empty());
    }

    #[test]
    fn hide_when_typing_false_is_an_explicit_zero() {
        let fake = Arc::new(Fake::with(&[(
            Setting::HidePointerWhileTyping,
            Value::Int(1),
        )]));
        let mut settings = PointerSettings::new(fake.clone());
        settings.apply(&wanted_cursor(&cursor(
            "cursor {\n    hide-when-typing false\n}\n",
        )));
        assert_eq!(
            fake.writes(),
            vec![(Setting::HidePointerWhileTyping, Value::Int(0))]
        );
        settings.restore();
        assert_eq!(
            fake.values().get(&Setting::HidePointerWhileTyping),
            Some(&Value::Int(1))
        );
    }

    #[test]
    fn the_real_mouse_vanish_write_sends_the_state_in_pv_param_and_zero_in_ui_param() {
        // Windows rejects `uiParam = 1` for SPI_SETMOUSEVANISH with ERROR_INVALID_PARAMETER, and
        // `pvParam = NULL` is FALSE: the first real backend sent `(1, NULL)` for "on", so "hide
        // pointer while typing" could never be switched on and the user's own "on" was not put
        // back on quit.  The audited backend never reaches this call, so a dry run cannot catch
        // it; the arguments are therefore pinned here.
        assert_eq!(mouse_vanish_set_arguments(true), (0, 1));
        assert_eq!(mouse_vanish_set_arguments(false), (0, 0));
        // The mapping from the stored value is the same for both directions of a restore.
        for stored in [0, 1] {
            assert_eq!(
                mouse_vanish_set_arguments(stored != 0),
                (0, stored as usize)
            );
        }
    }

    #[test]
    fn only_configured_options_are_written() {
        let fake = Arc::new(Fake::with(&[
            (
                Setting::EnhancedPrecision,
                Value::Thresholds(ADAPTIVE_THRESHOLDS),
            ),
            (Setting::ButtonSwap, Value::Int(0)),
        ]));
        let mut settings = PointerSettings::new(fake.clone());
        let lines = settings.apply(&wanted(&input(
            "input { mouse { accel-profile \"flat\" } }",
        )));
        assert_eq!(
            fake.writes(),
            vec![(
                Setting::EnhancedPrecision,
                Value::Thresholds(FLAT_THRESHOLDS)
            )]
        );
        assert!(fake.removes().is_empty());
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("accel-profile"), "{}", lines[0]);
        assert_eq!(settings.changed(), vec![Setting::EnhancedPrecision]);
    }

    #[test]
    fn an_option_the_os_already_has_is_not_written() {
        let fake = Arc::new(Fake::with(&[(
            Setting::EnhancedPrecision,
            Value::Thresholds(FLAT_THRESHOLDS),
        )]));
        let mut settings = PointerSettings::new(fake.clone());
        let lines = settings.apply(&wanted(&input(
            "input { mouse { accel-profile \"flat\" } }",
        )));
        assert!(fake.writes().is_empty(), "{:?}", fake.writes());
        assert!(lines.is_empty());
        // Nothing was changed, so a quit has nothing to restore.
        assert!(settings.restore().is_empty());
        assert!(fake.writes().is_empty());
    }

    #[test]
    fn every_configured_option_maps_to_its_windows_setting() {
        let fake = Arc::new(Fake::with(&[]));
        let mut settings = PointerSettings::new(fake.clone());
        settings.apply(&wanted(&input(
            "input {\n    mouse { accel-profile \"adaptive\"; left-handed }\n    touchpad { dwt; disabled-on-external-mouse }\n}",
        )));
        assert_eq!(
            fake.writes(),
            vec![
                (
                    Setting::EnhancedPrecision,
                    Value::Thresholds(ADAPTIVE_THRESHOLDS)
                ),
                (Setting::ButtonSwap, Value::Int(1)),
                (Setting::TouchpadTypingSensitivity, Value::Int(3)),
                (
                    Setting::TouchpadLeaveOnWithMouse,
                    // `disabled-on-external-mouse`: Windows' "leave it on" is inverted.
                    Value::Int(0)
                ),
            ]
        );
        // A value the OS did not have before is removed again on restore.
        assert!(fake.values().contains_key(&Setting::ButtonSwap));
        settings.restore();
        assert_eq!(
            fake.removes(),
            vec![
                Setting::EnhancedPrecision,
                Setting::ButtonSwap,
                Setting::TouchpadTypingSensitivity,
                Setting::TouchpadLeaveOnWithMouse
            ]
        );
        assert!(fake.values().is_empty(), "{:?}", fake.values());
    }

    #[test]
    fn a_graceful_quit_restores_the_exact_original_values() {
        let fake = Arc::new(Fake::with(&[
            (Setting::EnhancedPrecision, Value::Thresholds([4, 8, 1])),
            (Setting::ButtonSwap, Value::Int(0)),
            (Setting::TouchpadLeaveOnWithMouse, Value::Int(1)),
        ]));
        let mut settings = PointerSettings::new(fake.clone());
        settings.apply(&wanted(&input(
            "input {\n    mouse { accel-profile \"flat\"; left-handed }\n    touchpad { disabled-on-external-mouse }\n}",
        )));
        assert_eq!(
            fake.values().get(&Setting::EnhancedPrecision),
            Some(&Value::Thresholds(FLAT_THRESHOLDS))
        );
        settings.restore();
        // The original triple comes back, not Windows' defaults.
        assert_eq!(
            fake.values().get(&Setting::EnhancedPrecision),
            Some(&Value::Thresholds([4, 8, 1]))
        );
        assert_eq!(
            fake.values().get(&Setting::ButtonSwap),
            Some(&Value::Int(0))
        );
        assert_eq!(
            fake.values().get(&Setting::TouchpadLeaveOnWithMouse),
            Some(&Value::Int(1))
        );
        assert!(fake.removes().is_empty());
        assert!(settings.changed().is_empty());
    }

    #[test]
    fn a_setting_that_did_not_exist_is_removed_on_restore() {
        let fake = Arc::new(Fake::with(&[]));
        let mut settings = PointerSettings::new(fake.clone());
        settings.apply(&wanted(&input("input { touchpad { dwt } }")));
        assert_eq!(
            fake.writes(),
            vec![(Setting::TouchpadTypingSensitivity, Value::Int(3))]
        );
        settings.restore();
        assert_eq!(fake.removes(), vec![Setting::TouchpadTypingSensitivity]);
    }

    #[test]
    fn a_reload_that_drops_the_option_puts_the_os_back() {
        let fake = Arc::new(Fake::with(&[(Setting::ButtonSwap, Value::Int(0))]));
        let mut settings = PointerSettings::new(fake.clone());
        settings.apply(&wanted(&input("input { mouse { left-handed } }")));
        assert_eq!(
            fake.values().get(&Setting::ButtonSwap),
            Some(&Value::Int(1))
        );
        // The new config says nothing about the mouse: the reload restores the old value.
        let lines = settings.apply(&wanted(&input("input { mouse { natural-scroll } }")));
        assert_eq!(
            fake.values().get(&Setting::ButtonSwap),
            Some(&Value::Int(0))
        );
        assert!(!lines.is_empty());
        assert!(settings.changed().is_empty());
    }

    #[test]
    fn changing_an_option_keeps_the_first_original_for_the_restore() {
        let fake = Arc::new(Fake::with(&[
            (
                Setting::EnhancedPrecision,
                Value::Thresholds(ADAPTIVE_THRESHOLDS),
            ),
            (Setting::ButtonSwap, Value::Int(0)),
        ]));
        let mut settings = PointerSettings::new(fake.clone());
        settings.apply(&wanted(&input(
            "input { mouse { accel-profile \"flat\" } }",
        )));
        // Reload: adaptive again.  That is what the OS had at the start, so nothing is owed.
        settings.apply(&wanted(&input(
            "input { mouse { accel-profile \"adaptive\" } }",
        )));
        assert_eq!(
            fake.values().get(&Setting::EnhancedPrecision),
            Some(&Value::Thresholds(ADAPTIVE_THRESHOLDS))
        );
        assert!(settings.changed().is_empty(), "{:?}", settings.changed());
        assert!(settings.restore().is_empty());
    }

    #[test]
    fn a_failed_write_is_not_remembered_for_a_restore() {
        struct Failing;
        impl PointerSettingsBackend for Failing {
            fn read(&self, _: Setting) -> Option<Value> {
                Some(Value::Int(0))
            }
            fn write(&self, _: Setting, _: Value) -> Result<(), String> {
                Err("the OS said no".into())
            }
            fn remove(&self, _: Setting) -> Result<(), String> {
                Ok(())
            }
        }
        let mut settings = PointerSettings::new(Arc::new(Failing));
        settings.apply(&wanted(&input("input { mouse { left-handed } }")));
        assert!(settings.changed().is_empty());
        assert!(settings.restore().is_empty());
    }

    #[test]
    fn the_audit_backend_records_instead_of_writing() {
        let path = std::env::temp_dir().join(format!(
            "wiri-pointer-audit-{}-{:?}.ndjson",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        let fake = Arc::new(Fake::with(&[(Setting::ButtonSwap, Value::Int(0))]));
        let mut settings = PointerSettings::new(Arc::new(Audited::new(fake.clone(), path.clone())));
        settings.apply(&wanted(&input(
            "input {\n    mouse { left-handed; accel-profile \"flat\" }\n    touchpad { dwt; disabled-on-external-mouse }\n}",
        )));
        settings.restore();
        // The real backend was only read, never written.
        assert!(fake.writes().is_empty(), "{:?}", fake.writes());
        assert!(fake.removes().is_empty());
        assert_eq!(
            fake.values().get(&Setting::ButtonSwap),
            Some(&Value::Int(0))
        );
        let audit = std::fs::read_to_string(&path).expect("audit file");
        let lines: Vec<serde_json::Value> = audit
            .lines()
            .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
            .collect();
        assert_eq!(lines.len(), 8, "four writes and four restores: {:?}", lines);
        assert_eq!(lines[0]["action"], "write");
        assert_eq!(lines[0]["setting"], "enhanced-pointer-precision");
        assert_eq!(lines[0]["value"], serde_json::json!([0, 0, 0]));
        assert!(lines
            .iter()
            .any(|line| line["setting"] == "touchpad-typing-sensitivity"
                && line["value"] == serde_json::json!([3])));
        // Restores name their original (ButtonSwap was 0, the rest did not exist).
        assert!(lines.iter().any(|line| line["action"] == "remove"));
        assert!(lines.iter().any(|line| line["action"] == "write"
            && line["setting"] == "mouse-button-swap"
            && line["value"] == serde_json::json!([0])));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn options_without_a_windows_lever_are_reported() {
        let configured = input(
            "input {\n    mouse { middle-emulation; scroll-method \"edge\"; scroll-button 274; scroll-button-lock; off }\n    touchpad { dwtp; drag; drag-lock; click-method \"clickfinger\"; tap-button-map { left 1 } }\n}",
        );
        let reported = unapplied(&configured);
        assert_eq!(
            reported,
            vec![
                "input scroll-method",
                "input scroll-button",
                "input scroll-button-lock",
                "input middle-emulation",
                "touchpad dwtp",
                "touchpad drag",
                "touchpad drag-lock",
                "touchpad tap-button-map",
                "touchpad click-method",
                "input mouse off",
            ]
        );
        // Nothing that Windows can be told about is reported.
        assert!(unapplied(&input(
            "input { mouse { accel-profile \"flat\"; left-handed } }"
        ))
        .is_empty());
    }

    #[test]
    fn parity_button_scroll_is_hook_only_and_retains_unsupported_device_diagnostics() {
        let configured = input("input { mouse { scroll-method \"on-button-down\"; scroll-button 274; scroll-button-lock } }");
        assert!(
            wanted(&configured).is_empty(),
            "button scrolling never writes global pointer settings"
        );
        assert!(unapplied(&configured).is_empty());
        let disabled =
            input("input { mouse { scroll-method \"on-button-down\"; scroll-button 0 } }");
        assert!(wanted(&disabled).is_empty());
        assert!(unapplied(&disabled).is_empty());
        let mixed = input("input { mouse { scroll-method \"on-button-down\"; scroll-button 274 }; touchpad { scroll-button 276 } }");
        assert_eq!(unapplied(&mixed), vec!["non-mouse device-scoped scrolling"]);
        let unsupported = crate::config::decode::decode_text(
            "input { mouse { scroll-method \"on-button-down\"; scroll-button 99 } }",
            "",
            None,
        );
        assert!(!unsupported.has_errors());
        assert!(unapplied(&unsupported.config.input).contains(&"input scroll-button"));
    }

    #[test]
    fn the_touchpad_toggle_is_combined_with_the_config_and_resets_on_restart() {
        // Not configured: toggling cannot turn on a touchpad the config does not enable.
        assert!(!toggle_touchpad(false));
        assert!(!toggle_touchpad(false));
        // Configured: the first toggle turns it off, the second back on.
        assert!(!toggle_touchpad(true));
        assert!(toggle_touchpad(true));
        assert!(touchpad_active(true));
        assert!(!touchpad_active(false));
        // A reload keeps the runtime state (niri: only a restart resets it).
        assert!(touchpad_on_after_reload(true));
        assert!(!toggle_touchpad(true));
        assert!(!touchpad_on_after_reload(true));
        // Leave the shared static as it was for the other tests.
        assert!(toggle_touchpad(true));
    }

    #[test]
    fn settings_have_stable_names_for_the_audit_file() {
        assert_eq!(
            Setting::EnhancedPrecision.name(),
            "enhanced-pointer-precision"
        );
        assert_eq!(Setting::ButtonSwap.name(), "mouse-button-swap");
        assert_eq!(
            Setting::TouchpadTypingSensitivity.name(),
            "touchpad-typing-sensitivity"
        );
        assert_eq!(
            Setting::TouchpadLeaveOnWithMouse.name(),
            "touchpad-leave-on-with-mouse"
        );
        // The live phase matches this exact name in the audit file (`h78_hide_when_typing_dry_run`).
        assert_eq!(
            Setting::HidePointerWhileTyping.name(),
            "hide-pointer-while-typing"
        );
        assert_eq!(Value::Thresholds([0, 0, 0]).numbers(), vec![0, 0, 0]);
        assert_eq!(Value::Int(3).numbers(), vec![3]);
    }
}
