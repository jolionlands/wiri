//! Event-driven lid/posture observation on the existing hidden-window pump.
//! Config parsing and command execution belong to the parent. This module only
//! publishes edges; it never changes a power plan, display, or input setting.

use std::cell::RefCell;
use std::ffi::c_void;
use std::marker::PhantomData;
use std::rc::Rc;

use parking_lot::Mutex;
use tracing::warn;
use windows::core::GUID;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, PostMessageW, SM_CONVERTIBLESLATEMODE, WM_APP, WM_POWERBROADCAST,
    WM_SETTINGCHANGE,
};

use crate::backend::{BackendEvent, BackendHandle};

/// Payload of `BackendEvent::SwitchEvent`. Dispatch configured actions in the
/// parent, without routing through keyboard repeat/inhibition/lock filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchEvent {
    LidClose,
    LidOpen,
    TabletModeOn,
    TabletModeOff,
}

/// Subscribe to a switch family if either of its two edges has an action.
/// The parent selects actions for individual edges when receiving events.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SwitchEventSubscriptions {
    pub lid: bool,
    pub tablet_mode: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchSample {
    LidClosed(bool),
    TabletMode(bool),
}

/// Pure reducer: unknown -> known is a baseline, not an edge. Duplicate samples
/// are inert. Disabling/re-enabling a family starts a new baseline; unchanged
/// subscriptions (including action-only reloads) preserve the existing one.
#[derive(Debug, Default)]
pub struct SwitchState {
    subscriptions: SwitchEventSubscriptions,
    lid_closed: Option<bool>,
    tablet_mode: Option<bool>,
}

impl SwitchState {
    pub fn configure(&mut self, subscriptions: SwitchEventSubscriptions) {
        if self.subscriptions.lid != subscriptions.lid {
            self.lid_closed = None;
        }
        if self.subscriptions.tablet_mode != subscriptions.tablet_mode {
            self.tablet_mode = None;
        }
        self.subscriptions = subscriptions;
    }

    pub fn observe(&mut self, sample: SwitchSample) -> Option<SwitchEvent> {
        let (previous, value, on, off) = match sample {
            SwitchSample::LidClosed(value) if self.subscriptions.lid => (
                &mut self.lid_closed,
                value,
                SwitchEvent::LidClose,
                SwitchEvent::LidOpen,
            ),
            SwitchSample::TabletMode(value) if self.subscriptions.tablet_mode => (
                &mut self.tablet_mode,
                value,
                SwitchEvent::TabletModeOn,
                SwitchEvent::TabletModeOff,
            ),
            _ => return None,
        };
        match previous.replace(value) {
            Some(old) if old != value => Some(if value { on } else { off }),
            _ => None,
        }
    }
}

const WM_CONFIGURE_SWITCH_EVENTS: u32 = WM_APP + 0x357;
const PBT_POWERSETTINGCHANGE: usize = 0x8013;
// winnt.h: DWORD 0 = closed, 1 = open. This is not a power-plan setting.
const GUID_LIDSWITCH_STATE_CHANGE: GUID = GUID::from_u128(0xba3e0f4d_b817_4094_a2d1_d56379e6a0f3);

// Keep the dependency surface local: windows' System::Power feature is not
// otherwise needed. These user32 signatures use HWND, DWORD, HPOWERNOTIFY, BOOL.
#[link(name = "user32")]
extern "system" {
    fn RegisterPowerSettingNotification(
        recipient: *mut c_void,
        setting: *const GUID,
        flags: u32,
    ) -> *mut c_void;
    fn UnregisterPowerSettingNotification(handle: *mut c_void) -> i32;
}

struct PowerNotification(*mut c_void);

impl PowerNotification {
    fn register(hwnd: HWND) -> windows::core::Result<Self> {
        // DEVICE_NOTIFY_WINDOW_HANDLE = 0. Windows supplies the first lid
        // sample after registration; do not guess its state from a power plan.
        let handle =
            unsafe { RegisterPowerSettingNotification(hwnd.0, &GUID_LIDSWITCH_STATE_CHANGE, 0) };
        if handle.is_null() {
            Err(windows::core::Error::from_win32())
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for PowerNotification {
    fn drop(&mut self) {
        if unsafe { UnregisterPowerSettingNotification(self.0) } == 0 {
            warn!(
                "UnregisterPowerSettingNotification(lid) failed: {}",
                windows::core::Error::from_win32()
            );
        }
    }
}

struct Control {
    requested: SwitchEventSubscriptions,
    window: Option<isize>,
}

static CONTROL: Mutex<Control> = Mutex::new(Control {
    requested: SwitchEventSubscriptions {
        lid: false,
        tablet_mode: false,
    },
    window: None,
});

/// Publish subscriptions after a successful initial config load and each
/// successful reload. Safe before pump startup: the latest request is retained.
/// Live changes are posted to the existing pump, never applied on this thread.
/// Native registration failures are logged on that pump; no fallback polling.
pub fn configure(subscriptions: SwitchEventSubscriptions) -> windows::core::Result<()> {
    let mut control = CONTROL.lock();
    control.requested = subscriptions;
    if let Some(window) = control.window {
        // Serialize posting with detach so the HWND cannot be recycled here.
        unsafe {
            PostMessageW(
                HWND(window as *mut c_void),
                WM_CONFIGURE_SWITCH_EVENTS,
                WPARAM(0),
                LPARAM(0),
            )?;
        }
    }
    Ok(())
}

struct PumpState {
    hwnd: HWND,
    backend: BackendHandle,
    reducer: SwitchState,
    lid_notification: Option<PowerNotification>,
}

thread_local! {
    static PUMP: RefCell<Option<PumpState>> = const { RefCell::new(None) };
}

/// Must be created and dropped on the window's message thread. A guard exists
/// even when disabled, but no registrations or state queries do.
pub(crate) struct MessagePump {
    hwnd: HWND,
    _not_send: PhantomData<Rc<()>>,
}

pub(crate) fn attach(hwnd: HWND, backend: BackendHandle) -> MessagePump {
    PUMP.with(|pump| {
        *pump.borrow_mut() = Some(PumpState {
            hwnd,
            backend,
            reducer: SwitchState::default(),
            lid_notification: None,
        });
    });
    CONTROL.lock().window = Some(hwnd.0 as isize);
    apply_configuration(hwnd);
    MessagePump {
        hwnd,
        _not_send: PhantomData,
    }
}

impl Drop for MessagePump {
    fn drop(&mut self) {
        {
            let mut control = CONTROL.lock();
            if control.window == Some(self.hwnd.0 as isize) {
                control.window = None;
            }
        }
        // Drop native registrations outside the RefCell borrow, before HWND
        // destruction. No callback can observe partially-dropped state.
        let old = PUMP.with(|pump| pump.borrow_mut().take());
        drop(old);
    }
}

fn apply_configuration(hwnd: HWND) {
    let requested = CONTROL.lock().requested;
    let changes = PUMP.with(|pump| {
        let mut pump = pump.borrow_mut();
        let state = pump.as_mut().filter(|state| state.hwnd == hwnd)?;
        let seed_tablet = requested.tablet_mode && !state.reducer.subscriptions.tablet_mode;
        state.reducer.configure(requested);
        let old = if !requested.lid {
            state.lid_notification.take()
        } else {
            None
        };
        Some((
            old,
            requested.lid && state.lid_notification.is_none(),
            seed_tablet,
        ))
    });
    let Some((old, register_lid, seed_tablet)) = changes else {
        return;
    };
    drop(old);
    if seed_tablet {
        // 0 means slate/tablet, nonzero means laptop. The existing hidden
        // top-level window receives ConvertibleSlateMode WM_SETTINGCHANGE;
        // no additional posture power registration is needed.
        observe(
            hwnd,
            tablet_sample(unsafe { GetSystemMetrics(SM_CONVERTIBLESLATEMODE) }),
        );
    }
    if register_lid {
        // Registration can synchronously send the initial power message. Never
        // hold a TLS borrow or CONTROL lock across this Win32 call.
        match PowerNotification::register(hwnd) {
            Ok(notification) => PUMP.with(|pump| {
                if let Some(state) = pump.borrow_mut().as_mut() {
                    state.lid_notification = Some(notification);
                }
            }),
            Err(error) => warn!("Lid switch notifications unavailable: {}", error),
        }
    }
}

fn observe(hwnd: HWND, sample: SwitchSample) {
    PUMP.with(|pump| {
        if let Some(state) = pump
            .borrow_mut()
            .as_mut()
            .filter(|state| state.hwnd == hwnd)
        {
            if let Some(event) = state.reducer.observe(sample) {
                state.backend.send_event(BackendEvent::SwitchEvent(event));
            }
        }
    });
}

fn enabled(hwnd: HWND, lid: bool) -> bool {
    PUMP.with(|pump| {
        pump.borrow()
            .as_ref()
            .filter(|state| state.hwnd == hwnd)
            .is_some_and(|state| {
                if lid {
                    state.reducer.subscriptions.lid
                } else {
                    state.reducer.subscriptions.tablet_mode
                }
            })
    })
}

fn lid_sample(value: u32) -> Option<SwitchSample> {
    match value {
        0 => Some(SwitchSample::LidClosed(true)),
        1 => Some(SwitchSample::LidClosed(false)),
        _ => None,
    }
}

fn tablet_sample(metric: i32) -> SwitchSample {
    SwitchSample::TabletMode(metric == 0)
}

// POWERBROADCAST_SETTING has a variable-length trailing BYTE Data[1]. Read
// only its fixed header first, not an entire Rust struct with a fake tail.
#[repr(C)]
#[derive(Clone, Copy)]
struct PowerSettingHeader {
    setting: GUID,
    data_length: u32,
}

fn decode_power_value(setting: GUID, length: u32, value: u32) -> Option<SwitchSample> {
    if setting == GUID_LIDSWITCH_STATE_CHANGE && length == 4 {
        lid_sample(value)
    } else {
        None
    }
}

/// # Safety
/// Pointer-bearing messages must have the Win32-defined, readable payload for
/// the duration of this call, just as required by the owning window procedure.
pub(crate) unsafe fn handle_message(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<LRESULT> {
    match message {
        WM_CONFIGURE_SWITCH_EVENTS => {
            apply_configuration(hwnd);
            Some(LRESULT(0))
        }
        WM_POWERBROADCAST if wparam.0 == PBT_POWERSETTINGCHANGE && enabled(hwnd, true) => {
            let data = lparam.0 as *const u8;
            if data.is_null() {
                return None;
            }
            let header = std::ptr::read_unaligned(data.cast::<PowerSettingHeader>());
            if header.setting != GUID_LIDSWITCH_STATE_CHANGE || header.data_length != 4 {
                return None;
            }
            let value = std::ptr::read_unaligned(
                data.add(std::mem::size_of::<PowerSettingHeader>())
                    .cast::<u32>(),
            );
            if let Some(sample) = decode_power_value(header.setting, header.data_length, value) {
                observe(hwnd, sample);
            }
            Some(LRESULT(1))
        }
        WM_SETTINGCHANGE if enabled(hwnd, false) && lparam.0 != 0 => {
            // Bounded, allocation-free comparison, including NUL; stop at the
            // first mismatch so shorter unrelated setting names are safe.
            let name = lparam.0 as *const u16;
            if "ConvertibleSlateMode"
                .encode_utf16()
                .chain(Some(0))
                .enumerate()
                .all(|(index, ch)| std::ptr::read_unaligned(name.add(index)) == ch)
            {
                observe(
                    hwnd,
                    tablet_sample(GetSystemMetrics(SM_CONVERTIBLESLATEMODE)),
                );
            }
            None // Let the normal window procedure handle other setting work.
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: SwitchEventSubscriptions = SwitchEventSubscriptions {
        lid: true,
        tablet_mode: true,
    };

    #[test]
    fn disabled_samples_do_not_seed_state_or_emit_events() {
        let mut state = SwitchState::default();
        assert_eq!(state.observe(SwitchSample::LidClosed(false)), None);
        assert_eq!(state.observe(SwitchSample::TabletMode(false)), None);
        state.configure(BOTH);
        assert_eq!(state.observe(SwitchSample::LidClosed(true)), None);
        assert_eq!(state.observe(SwitchSample::TabletMode(true)), None);
    }

    #[test]
    fn either_initial_state_is_only_a_baseline() {
        for initial in [false, true] {
            let mut state = SwitchState::default();
            state.configure(BOTH);
            assert_eq!(state.observe(SwitchSample::LidClosed(initial)), None);
            assert_eq!(state.observe(SwitchSample::TabletMode(initial)), None);
            assert_eq!(state.observe(SwitchSample::LidClosed(initial)), None);
            assert_eq!(state.observe(SwitchSample::TabletMode(initial)), None);
        }
    }

    #[test]
    fn each_transition_fires_once_and_switches_are_independent() {
        let mut state = SwitchState::default();
        state.configure(BOTH);
        assert_eq!(state.observe(SwitchSample::LidClosed(false)), None);
        assert_eq!(state.observe(SwitchSample::TabletMode(false)), None);
        assert_eq!(
            state.observe(SwitchSample::LidClosed(true)),
            Some(SwitchEvent::LidClose)
        );
        assert_eq!(state.observe(SwitchSample::LidClosed(true)), None);
        assert_eq!(
            state.observe(SwitchSample::TabletMode(true)),
            Some(SwitchEvent::TabletModeOn)
        );
        assert_eq!(state.observe(SwitchSample::TabletMode(true)), None);
        assert_eq!(
            state.observe(SwitchSample::LidClosed(false)),
            Some(SwitchEvent::LidOpen)
        );
        assert_eq!(
            state.observe(SwitchSample::TabletMode(false)),
            Some(SwitchEvent::TabletModeOff)
        );
    }

    #[test]
    fn action_only_reload_preserves_baselines() {
        let mut state = SwitchState::default();
        state.configure(BOTH);
        assert_eq!(state.observe(SwitchSample::LidClosed(true)), None);
        assert_eq!(state.observe(SwitchSample::TabletMode(false)), None);
        state.configure(BOTH);
        assert_eq!(
            state.observe(SwitchSample::LidClosed(false)),
            Some(SwitchEvent::LidOpen)
        );
        assert_eq!(
            state.observe(SwitchSample::TabletMode(true)),
            Some(SwitchEvent::TabletModeOn)
        );
    }

    #[test]
    fn disabling_and_reenabling_resets_only_that_family() {
        let mut state = SwitchState::default();
        state.configure(BOTH);
        assert_eq!(state.observe(SwitchSample::LidClosed(false)), None);
        assert_eq!(state.observe(SwitchSample::TabletMode(false)), None);
        state.configure(SwitchEventSubscriptions {
            lid: false,
            tablet_mode: true,
        });
        assert_eq!(state.observe(SwitchSample::LidClosed(true)), None);
        state.configure(BOTH);
        assert_eq!(state.observe(SwitchSample::LidClosed(true)), None);
        assert_eq!(
            state.observe(SwitchSample::TabletMode(true)),
            Some(SwitchEvent::TabletModeOn)
        );
        state.configure(SwitchEventSubscriptions::default());
        state.configure(BOTH);
        assert_eq!(state.observe(SwitchSample::LidClosed(false)), None);
        assert_eq!(state.observe(SwitchSample::TabletMode(false)), None);
    }

    #[test]
    fn native_values_and_payload_validation() {
        assert_eq!(std::mem::size_of::<PowerSettingHeader>(), 20);
        assert_eq!(
            decode_power_value(GUID_LIDSWITCH_STATE_CHANGE, 4, 0),
            Some(SwitchSample::LidClosed(true))
        );
        assert_eq!(
            decode_power_value(GUID_LIDSWITCH_STATE_CHANGE, 4, 1),
            Some(SwitchSample::LidClosed(false))
        );
        assert_eq!(decode_power_value(GUID_LIDSWITCH_STATE_CHANGE, 4, 2), None);
        for length in [0, 1, 3, 5, u32::MAX] {
            assert_eq!(
                decode_power_value(GUID_LIDSWITCH_STATE_CHANGE, length, 0),
                None
            );
        }
        assert_eq!(decode_power_value(GUID::from_u128(0), 4, 0), None);
        assert_eq!(tablet_sample(0), SwitchSample::TabletMode(true));
        assert_eq!(tablet_sample(1), SwitchSample::TabletMode(false));
        assert_eq!(tablet_sample(-1), SwitchSample::TabletMode(false));
    }
}
