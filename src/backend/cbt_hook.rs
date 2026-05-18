//! CBT (Computer-Based Training) Hook for intercepting window events before they happen.
//! 
//! Unlike WinEventHook which fires AFTER events, CBT hook fires BEFORE window creation,
//! destruction, activation, focus, and moves/resizes. This allows wiri to:
//! 
//! - Set initial window size/position before the window is shown
//! - Prevent unwanted windows from appearing
//! - Intercept focus changes for better control
//! 
//! Reference: <https://docs.microsoft.com/en-us/windows/win32/winmsg/using-hooks>

use anyhow::Result;
use parking_lot::Mutex;
use tracing::{debug, info, warn};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    SetWindowsHookExW, UnhookWindowsHookEx, CallNextHookEx,
    HHOOK, WH_CBT,
    HCBT_ACTIVATE, HCBT_CREATEWND, HCBT_DESTROYWND, HCBT_MOVESIZE, HCBT_SETFOCUS,
};

use crate::backend::BackendEvent;
use super::BackendHandle;

/// CBT Hook codes (for reference)
/// HCBT_CLICKSKIPPED = 4 - mouse click skipped
/// HCBT_KEYSTROKE = 6 - keystroke skipped
#[allow(dead_code)]
const HCBT_CLICKSKIPPED: i32 = 4;
#[allow(dead_code)]
const HCBT_KEYSTROKE: i32 = 6;

/// CBT Hook manager.
///
/// # Thread-scope note
/// The hook is installed with `dwThreadId = GetCurrentThreadId()`, meaning it
/// intercepts only wiri's own thread — NOT other processes. A system-wide CBT hook
/// would require an in-process DLL loaded into every target process, which is not
/// implemented here.
#[derive(Debug)]
pub struct CbtHook {
    hook_handle: Option<HHOOK>,
}

impl CbtHook {
    /// Create and install a thread-local CBT hook.
    ///
    /// The hook is scoped to the calling thread only. It intercepts wiri's own
    /// window operations; other processes are not affected.
    pub fn new(backend: BackendHandle) -> Result<Self> {
        // CBT hook is thread-local — intercepts wiri's own thread, not system-wide.
        // A system-wide hook would require a DLL.
        let current_thread_id = unsafe { GetCurrentThreadId() };
        let hook = unsafe {
            SetWindowsHookExW(WH_CBT, Some(cbt_hook_proc), None, current_thread_id)
        };

        match hook {
            Ok(handle) => {
                info!("CBT hook installed successfully: {:?}", handle);
                // Store the backend handle for the hook callback
                CBT_BACKEND.lock().replace(backend);
                Ok(Self { hook_handle: Some(handle) })
            }
            Err(e) => {
                warn!("Failed to install CBT hook: {:?}", e);
                // CBT hook is optional - don't fail initialization
                Ok(Self { hook_handle: None })
            }
        }
    }

    /// Check if the CBT hook is installed and active
    pub fn is_active(&self) -> bool {
        self.hook_handle.is_some()
    }
}

impl Drop for CbtHook {
    fn drop(&mut self) {
        if let Some(handle) = self.hook_handle {
            unsafe {
                let _ = UnhookWindowsHookEx(handle);
            }
            info!("CBT hook uninstalled");
        }
        *CBT_BACKEND.lock() = None;
    }
}

// Thread-safe storage for the backend handle
lazy_static::lazy_static! {
    static ref CBT_BACKEND: Mutex<Option<BackendHandle>> = Mutex::new(None);
}

/// CBT Hook procedure
/// 
/// This is called by Windows before:
/// - HCBT_CREATEWND: A window is about to be created
/// - HCBT_DESTROYWND: A window is about to be destroyed
/// - HCBT_ACTIVATE: A window is about to be activated
/// - HCBT_MOVESIZE: A window is about to be moved or sized
/// - HCBT_SETFOCUS: A window is about to receive focus
/// - HCBT_CLICKSKIPPED: A mouse click message is about to be removed from the queue
/// - HCBT_KEYSTROKE: A keystroke message is about to be removed from the queue
unsafe extern "system" fn cbt_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> windows::Win32::Foundation::LRESULT {
    if code < 0 {
        // Chain to next hook — must not hold CBT_BACKEND lock while calling
        return CallNextHookEx(HHOOK::default(), code, WPARAM(wparam.0), lparam);
    }

    // Clone the handle out of the lock before doing anything so we do not hold
    // the mutex across CallNextHookEx or send_event (both may re-enter).
    let backend = {
        let guard = CBT_BACKEND.lock();
        match guard.as_ref() {
            Some(b) => b.clone(),
            None => return CallNextHookEx(HHOOK::default(), code, WPARAM(wparam.0), lparam),
        }
    };

    // Cast code to u32 for comparison with Windows constants
    let code_u32 = code as u32;
    match code_u32 {
        HCBT_CREATEWND => {
            // A window is about to be created
            // wparam = handle to the window being created
            let hwnd = HWND(wparam.0 as *mut std::ffi::c_void);

            debug!("CBT: HCBT_CREATEWND for window {:p}", hwnd.0);

            // Note: At this point, we could modify CREATESTRUCT via lparam to set:
            // - Position (x, y)
            // - Size (cx, cy)
            // - Style flags
            // However, this requires the CREATESTRUCT pointer from lparam

            // Send event to backend for tracking
            backend.send_event(BackendEvent::CbtCreateWindow { hwnd: hwnd.0 as isize });
        }

        HCBT_DESTROYWND => {
            // A window is about to be destroyed
            let hwnd = HWND(wparam.0 as *mut std::ffi::c_void);
            debug!("CBT: HCBT_DESTROYWND for window {:p}", hwnd.0);
            backend.send_event(BackendEvent::CbtDestroyWindow { hwnd: hwnd.0 as isize });
        }

        HCBT_ACTIVATE => {
            // A window is about to be activated
            let hwnd = HWND(wparam.0 as *mut std::ffi::c_void);
            debug!("CBT: HCBT_ACTIVATE for window {:p}", hwnd.0);
            backend.send_event(BackendEvent::CbtActivate { hwnd: hwnd.0 as isize });
        }

        HCBT_MOVESIZE => {
            // A window is about to be moved or sized
            let hwnd = HWND(wparam.0 as *mut std::ffi::c_void);
            debug!("CBT: HCBT_MOVESIZE for window {:p}", hwnd.0);
            backend.send_event(BackendEvent::CbtMoveSize { hwnd: hwnd.0 as isize });
        }

        HCBT_SETFOCUS => {
            // A window is about to receive focus
            let hwnd = HWND(wparam.0 as *mut std::ffi::c_void);
            debug!("CBT: HCBT_SETFOCUS for window {:p}", hwnd.0);
            backend.send_event(BackendEvent::CbtSetFocus { hwnd: hwnd.0 as isize });
        }

        _ => {
            // Handle HCBT_CLICKSKIPPED (4) and HCBT_KEYSTROKE (6) if needed
            debug!("CBT: hook code {} (wparam={})", code, wparam.0);
        }
    }

    // Continue to the next hook in the chain (lock already released above)
    CallNextHookEx(HHOOK::default(), code, WPARAM(wparam.0), lparam)
}
