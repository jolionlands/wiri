//! Keyboard notifications on the existing hidden hotkey window: no timer,
//! polling loop, extra window or hook thread. Shell language notifications also
//! cover a layout switch in another process, unlike WM_INPUTLANGCHANGE alone.
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    DeregisterShellHookWindow, RegisterShellHookWindow, RegisterWindowMessageW,
};

static SHELL_MESSAGE: AtomicU32 = AtomicU32::new(0);

pub(crate) fn initialize(hwnd: HWND) {
    let message = unsafe { RegisterWindowMessageW(windows::core::w!("SHELLHOOK")) };
    if message != 0 && unsafe { RegisterShellHookWindow(hwnd) }.as_bool() {
        SHELL_MESSAGE.store(message, Ordering::Release);
    } else {
        tracing::warn!("Keyboard layout shell notification registration failed");
    }
}

pub(crate) fn shutdown(hwnd: HWND) {
    if SHELL_MESSAGE.swap(0, Ordering::AcqRel) != 0 {
        let _ = unsafe { DeregisterShellHookWindow(hwnd) };
    }
}

fn relevant(message: u32, code: usize, shell: u32) -> bool {
    // WM_INPUTLANGCHANGE / WM_SETTINGCHANGE and HSHELL_LANGUAGE.
    matches!(message, 0x0051 | 0x001A) || (shell != 0 && message == shell && code == 8)
}

pub(crate) fn observe_message(message: u32, code: usize) {
    if relevant(message, code, SHELL_MESSAGE.load(Ordering::Acquire)) {
        super::notify_state_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_language_or_settings_notifications_wake_the_state_task() {
        assert!(relevant(0x51, 0, 0));
        assert!(relevant(0x1A, 0, 0));
        assert!(relevant(0xC123, 8, 0xC123));
        assert!(!relevant(0xC123, 4, 0xC123));
        assert!(!relevant(0xC124, 8, 0xC123));
        assert!(!relevant(0, 8, 0));
    }
}
