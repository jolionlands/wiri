//! The AppUserModelID of a window.
//!
//! A packaged (UWP / MSIX) app, and a desktop app that names itself, carries an
//! AppUserModelID such as `Microsoft.WindowsTerminal_8wekyb3d8bbwe!App`; the shell keeps it
//! in the window's property store.  Windows has no `app-id`, so `app-id` in a window rule
//! matches the process's name and this.
//!
//! One call to the shell per window, made when the window opens and only when a rule has
//! an `app-id` condition.  It reads what the shell stores for the window; it does not wait
//! for the application (a hung application answers like any other).

use std::cell::Cell;
use std::ffi::c_void;
use windows::core::GUID;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::PropertiesSystem::{
    IPropertyStore, SHGetPropertyStoreForWindow, PROPERTYKEY,
};

/// `PKEY_AppUserModel_ID`: {9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}, 5.
const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};

thread_local! {
    static COM_READY: Cell<bool> = Cell::new(false);
}

/// The property store is a COM object: the thread joins the multithreaded apartment first
/// (a thread that is a single-threaded apartment already keeps what it has).
fn ensure_com() {
    COM_READY.with(|ready| {
        if !ready.get() {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            ready.set(true);
        }
    });
}

/// Text of a property value without the blanks around it; `None` for an empty one.
fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// The AppUserModelID of `hwnd`, or `None` when the window has none (most desktop windows
/// have not) or the shell does not say.
pub fn of_window(hwnd: isize) -> Option<String> {
    ensure_com();
    unsafe {
        let store: IPropertyStore =
            SHGetPropertyStoreForWindow(HWND(hwnd as *mut c_void)).ok()?;
        let value = store.GetValue(&PKEY_APP_USER_MODEL_ID).ok()?;
        non_empty(&value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_id_is_no_id() {
        assert_eq!(non_empty(""), None);
        assert_eq!(non_empty("  \t"), None);
        assert_eq!(
            non_empty(" Microsoft.WindowsTerminal_8wekyb3d8bbwe!App "),
            Some("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App".to_string())
        );
    }

}
