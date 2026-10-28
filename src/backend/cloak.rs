//! Hiding a window without hiding it from the system: shell cloaking.
//!
//! `SW_HIDE` (what wiri has always done to the windows of inactive workspaces) takes a
//! window out of the taskbar and out of Alt+Tab.  The shell can instead *cloak* a
//! window: the window stays visible to the system and to its application (it keeps its
//! taskbar button and its place in Alt+Tab), but DWM does not draw it.  This is the way
//! Windows hides the windows of the other virtual desktops.
//!
//! `DwmSetWindowAttribute(DWMWA_CLOAK)` is refused with `E_ACCESSDENIED` for another
//! process's window.  The shell does it for itself through the undocumented
//! `IApplicationView::SetCloak`, which komorebi uses for its `cloak` hiding behaviour as
//! well; the interface definitions follow komorebi's, which took them (MIT) from Jari
//! Pennanen's AltTabAccessor.  They are private to the shell and have changed between
//! builds of Windows, so nothing here trusts them blindly: the view of a window is asked
//! for by the IID this code knows (`QueryInterface`) before its vtable is used, a build
//! that changed the interface answers `E_NOINTERFACE` instead of being called with the
//! wrong layout, and the first such refusal turns cloaking off for the rest of the run
//! (the caller then hides the window the plain way).
//!
//! Opt-in: `layout { hide-method "cloak" }`.  The default stays `hide`.

use parking_lot::Mutex;
use std::cell::Cell;
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::LazyLock;
use tracing::{debug, warn};
use windows::core::{Interface, GUID, HRESULT};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IServiceProvider, CLSCTX_ALL, COINIT_MULTITHREADED,
};

/// How the windows of inactive workspaces are hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HideMethod {
    /// `SW_HIDE`: the window leaves the taskbar and Alt+Tab while it is hidden.
    #[default]
    Hide,
    /// Shell cloaking: the window stays in the taskbar and in Alt+Tab, but is not drawn.
    Cloak,
}

impl HideMethod {
    /// The value of `hide-method` in the config; `None` for anything that is not a method.
    pub fn parse(text: &str) -> Option<HideMethod> {
        match text.trim().to_ascii_lowercase().as_str() {
            "hide" | "sw-hide" | "sw_hide" => Some(HideMethod::Hide),
            "cloak" | "cloaked" => Some(HideMethod::Cloak),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            HideMethod::Hide => "hide",
            HideMethod::Cloak => "cloak",
        }
    }
}

static METHOD: AtomicU8 = AtomicU8::new(0);

/// Set the method the backend hides windows with (from the config, at start and on every
/// reload).
pub fn set_hide_method(method: HideMethod) {
    METHOD.store(method as u8, Ordering::Relaxed);
}

pub fn hide_method() -> HideMethod {
    if METHOD.load(Ordering::Relaxed) == HideMethod::Cloak as u8 {
        HideMethod::Cloak
    } else {
        HideMethod::Hide
    }
}

/// Set once this build of Windows has refused the shell interfaces.
static UNSUPPORTED: AtomicBool = AtomicBool::new(false);

/// Has this build of Windows not refused cloaking so far?
pub fn available() -> bool {
    !UNSUPPORTED.load(Ordering::Relaxed)
}

/// Should the next window that is hidden be cloaked?  The method says so, and the shell has
/// not refused its interfaces.
pub fn wanted() -> bool {
    hide_method() == HideMethod::Cloak && available()
}

/// The windows this run has cloaked.
static CLOAKED: LazyLock<Mutex<HashSet<isize>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// `IApplicationView::SetCloak(type, flags)`: komorebi cloaks with `(1, 2)` and uncloaks
/// with `(1, 0)`.
const CLOAK_TYPE: u32 = 1;
const CLOAK_ON: i32 = 2;
const CLOAK_OFF: i32 = 0;

const CLSID_IMMERSIVE_SHELL: GUID = GUID::from_u128(0xC2F03A33_21F5_47FA_B4BB_156362A2F239);
const IID_APPLICATION_VIEW_COLLECTION: GUID =
    GUID::from_u128(0x1841C6D7_4F9D_42C0_AF41_8747538F10E5);
const IID_APPLICATION_VIEW: GUID = GUID::from_u128(0x372E1D3B_38D3_42E4_A15B_8AB2B178F513);

/// `E_NOINTERFACE` and `REGDB_E_CLASSNOTREG`: the shell does not have what this code asks
/// for.  Retrying never helps; anything else (a busy or restarting shell) may pass.
const E_NOINTERFACE: i32 = 0x8000_4002_u32 as i32;
const REGDB_E_CLASSNOTREG: i32 = 0x8004_0154_u32 as i32;

// The vtable structs only describe a layout: most of their entries are never called.
#[repr(C)]
#[allow(dead_code)]
struct UnknownVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: usize,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

/// `IApplicationViewCollection`: only `GetViewForHwnd` (the fourth method after
/// `IUnknown`'s three) is called.
#[repr(C)]
#[allow(dead_code)]
struct ApplicationViewCollectionVtbl {
    base: UnknownVtbl,
    get_views: usize,
    get_views_by_zorder: usize,
    get_views_by_app_user_model_id: usize,
    get_view_for_hwnd: unsafe extern "system" fn(*mut c_void, HWND, *mut *mut c_void) -> HRESULT,
}

/// `IApplicationView`: only `SetCloak` is called.  Before it come `IUnknown`'s three
/// methods, `IInspectable`'s three and six of the interface's own.
#[repr(C)]
#[allow(dead_code)]
struct ApplicationViewVtbl {
    base: UnknownVtbl,
    get_iids: usize,
    get_runtime_class_name: usize,
    get_trust_level: usize,
    set_focus: usize,
    switch_to: usize,
    try_invoke_back: usize,
    get_thumbnail_window: usize,
    get_monitor: usize,
    get_visibility: usize,
    set_cloak: unsafe extern "system" fn(*mut c_void, u32, i32) -> HRESULT,
}

#[derive(Debug, PartialEq, Eq)]
enum CloakError {
    /// This build of Windows does not offer the interfaces this code knows.
    Unsupported,
    /// The call failed (for this window, or for now).
    Failed(i32),
}

thread_local! {
    static COM_READY: Cell<bool> = Cell::new(false);
}

/// The shell's interfaces are COM: every thread that cloaks a window joins the
/// multithreaded apartment first (a thread that is a single-threaded apartment already
/// keeps what it has).  Never undone: the threads that call this live as long as wiri.
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

unsafe fn release(object: *mut c_void) {
    let vtbl = *(object as *const *const UnknownVtbl);
    ((*vtbl).release)(object);
}

fn permanent(hr: HRESULT) -> bool {
    hr.0 == E_NOINTERFACE || hr.0 == REGDB_E_CLASSNOTREG
}

/// Ask the shell to cloak (`flags` = 2) or uncloak (0) the window.
fn set_cloak_through_the_shell(hwnd: isize, flags: i32) -> Result<(), CloakError> {
    ensure_com();
    let window = HWND(hwnd as *mut c_void);
    unsafe {
        let provider: IServiceProvider =
            match CoCreateInstance(&CLSID_IMMERSIVE_SHELL, None, CLSCTX_ALL) {
                Ok(provider) => provider,
                Err(e) => {
                    return Err(if permanent(e.code()) {
                        CloakError::Unsupported
                    } else {
                        CloakError::Failed(e.code().0)
                    })
                }
            };
        let mut collection: *mut c_void = std::ptr::null_mut();
        let hr = (provider.vtable().QueryService)(
            provider.as_raw(),
            &IID_APPLICATION_VIEW_COLLECTION,
            &IID_APPLICATION_VIEW_COLLECTION,
            &mut collection,
        );
        if hr.is_err() || collection.is_null() {
            return Err(if permanent(hr) {
                CloakError::Unsupported
            } else {
                CloakError::Failed(hr.0)
            });
        }

        // The shell's view of this window.
        let collection_vtbl = *(collection as *const *const ApplicationViewCollectionVtbl);
        let mut view: *mut c_void = std::ptr::null_mut();
        let hr = ((*collection_vtbl).get_view_for_hwnd)(collection, window, &mut view);
        release(collection);
        if hr.is_err() || view.is_null() {
            // The shell has no view of this window (it is not an application window).
            return Err(CloakError::Failed(hr.0));
        }

        // Ask for the interface this code knows by its IID: the layout of the vtable is
        // tied to it.  A build that changed the interface refuses here.
        let view_unknown = *(view as *const *const UnknownVtbl);
        let mut known: *mut c_void = std::ptr::null_mut();
        let hr = ((*view_unknown).query_interface)(view, &IID_APPLICATION_VIEW, &mut known);
        release(view);
        if hr.is_err() || known.is_null() {
            return Err(if permanent(hr) {
                CloakError::Unsupported
            } else {
                CloakError::Failed(hr.0)
            });
        }
        let known_vtbl = *(known as *const *const ApplicationViewVtbl);
        let hr = ((*known_vtbl).set_cloak)(known, CLOAK_TYPE, flags);
        release(known);
        if hr.is_err() {
            return Err(CloakError::Failed(hr.0));
        }
    }
    Ok(())
}

/// Does DWM draw `hwnd`?  `true` when something has cloaked it (the application, the shell
/// or its owner).
pub fn is_cloaked(hwnd: isize) -> bool {
    let mut flags: u32 = 0;
    let hr = unsafe {
        DwmGetWindowAttribute(
            HWND(hwnd as *mut c_void),
            DWMWA_CLOAKED,
            &mut flags as *mut u32 as *mut c_void,
            std::mem::size_of::<u32>() as u32,
        )
    };
    hr.is_ok() && flags != 0
}

/// Cloak `hwnd`.  Returns true when the window is cloaked afterwards (also when it was
/// already); false when it could not be, so that the caller hides it the plain way.
pub fn cloak(hwnd: isize) -> bool {
    if !available() {
        return false;
    }
    if CLOAKED.lock().contains(&hwnd) {
        // Cheap: this runs for every window of every inactive workspace on every layout
        // pass.  Whether the shell still has it cloaked is a call to DWM, not to the shell.
        if is_cloaked(hwnd) {
            return true;
        }
        CLOAKED.lock().remove(&hwnd);
    }
    match set_cloak_through_the_shell(hwnd, CLOAK_ON) {
        Ok(()) => {
            CLOAKED.lock().insert(hwnd);
            debug!("cloak: hwnd={} cloaked", hwnd);
            true
        }
        Err(CloakError::Unsupported) => {
            if !UNSUPPORTED.swap(true, Ordering::Relaxed) {
                warn!(
                    "hide-method cloak: this build of Windows does not offer the shell \
                     interfaces cloaking needs; windows are hidden the plain way"
                );
            }
            false
        }
        Err(CloakError::Failed(code)) => {
            debug!("cloak: hwnd={} could not be cloaked (hr {:#x})", hwnd, code);
            false
        }
    }
}

/// Uncloak `hwnd` if this run cloaked it.  A window the shell would not let go of stays on
/// the list, so that the next show tries again instead of leaving it invisible for good.
pub fn uncloak(hwnd: isize) {
    if !CLOAKED.lock().contains(&hwnd) {
        return;
    }
    match set_cloak_through_the_shell(hwnd, CLOAK_OFF) {
        Ok(()) => {
            CLOAKED.lock().remove(&hwnd);
            debug!("uncloak: hwnd={} uncloaked", hwnd);
        }
        Err(e) => debug!(
            "uncloak: hwnd={} failed: {:?} (tried again at the next show)",
            hwnd, e
        ),
    }
}

/// Uncloak `hwnd` whether or not this run cloaked it: the wiri that did may have died.
/// Used on start (from `hidden.json`), on quit and from the panic hook.
pub fn uncloak_forced(hwnd: isize) {
    CLOAKED.lock().remove(&hwnd);
    match set_cloak_through_the_shell(hwnd, CLOAK_OFF) {
        Ok(()) => debug!(
            "uncloak: hwnd={} uncloaked (record from an earlier run)",
            hwnd
        ),
        Err(e) => debug!("uncloak: hwnd={} failed: {:?}", hwnd, e),
    }
}

/// The window is gone: forget that it was cloaked.
pub fn forget(hwnd: isize) {
    CLOAKED.lock().remove(&hwnd);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hide_method_names() {
        assert_eq!(HideMethod::parse("hide"), Some(HideMethod::Hide));
        assert_eq!(HideMethod::parse(" Cloak "), Some(HideMethod::Cloak));
        assert_eq!(HideMethod::parse("CLOAKED"), Some(HideMethod::Cloak));
        assert_eq!(HideMethod::parse("minimize"), None);
        assert_eq!(HideMethod::parse(""), None);
        assert_eq!(HideMethod::default(), HideMethod::Hide);
        assert_eq!(HideMethod::Cloak.name(), "cloak");
        assert_eq!(HideMethod::Hide.name(), "hide");
    }

    #[test]
    fn test_only_a_missing_interface_is_permanent() {
        assert!(permanent(HRESULT(E_NOINTERFACE)));
        assert!(permanent(HRESULT(REGDB_E_CLASSNOTREG)));
        // E_ACCESSDENIED, RPC_E_CHANGED_MODE, E_INVALIDARG: this window, or this moment.
        assert!(!permanent(HRESULT(0x8007_0005_u32 as i32)));
        assert!(!permanent(HRESULT(0x8001_0106_u32 as i32)));
        assert!(!permanent(HRESULT(0x8007_0057_u32 as i32)));
    }

    #[test]
    fn test_the_shell_vtables_have_the_documented_layout() {
        // IApplicationView::SetCloak is entry 12 of the vtable: three of IUnknown, three of
        // IInspectable, then SetFocus, SwitchTo, TryInvokeBack, GetThumbnailWindow,
        // GetMonitor and GetVisibility.
        let word = std::mem::size_of::<usize>();
        assert_eq!(
            std::mem::offset_of!(ApplicationViewVtbl, set_cloak),
            12 * word
        );
        // IApplicationViewCollection::GetViewForHwnd is index 6.
        assert_eq!(
            std::mem::offset_of!(ApplicationViewCollectionVtbl, get_view_for_hwnd),
            6 * word
        );
    }

    #[test]
    fn test_a_window_that_is_not_there_is_not_cloaked_and_cannot_be() {
        // Never a window: DWM says nothing, so it counts as not cloaked.
        assert!(!is_cloaked(0));
        assert!(!is_cloaked(0x0DEA_D002));
    }
}
