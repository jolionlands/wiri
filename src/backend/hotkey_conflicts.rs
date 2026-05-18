//! Startup probe for known global hotkey hijackers.
//!
//! Some GPU control panels install services that capture chords like
//! `Ctrl+Alt+Arrows` (Intel HD Graphics, AMD Radeon) and route them to
//! screen rotation instead of the foreground window.  Once the driver
//! claims the chord no user-space `RegisterHotKey` call can take it back —
//! the only fix is to disable the chord in the GPU control panel or to
//! rebind in `config.kdl`.
//!
//! Detection is best-effort: we check a couple of well-known registry
//! keys and a couple of well-known service names.  If anything matches we
//! emit a startup WARN log so the user knows why their arrow-key
//! navigation isn't firing.  No probe runs on `aarch64-pc-windows-msvc`
//! (Snapdragon ARM64 doesn't ship the Intel or AMD discrete-GPU stacks).
//!
//! Returns a `Vec<String>` of detected-conflict labels so the caller can
//! log them all at once (and tests can assert against the list without
//! parsing log output).

#[cfg(target_arch = "x86_64")]
use windows::core::PCWSTR;

/// Run every detection heuristic and return one human-readable label per
/// confirmed conflict.  Empty when nothing is detected (the common case on
/// modern Win11 boxes without GPU OEM utilities).
#[cfg(target_arch = "x86_64")]
pub fn detect() -> Vec<String> {
    let mut hits: Vec<String> = Vec::new();

    if has_intel_hotkey_service() {
        hits.push(
            "Intel HotKey Service detected — Ctrl+Alt+Arrows are likely \
             captured by the graphics driver and will rotate the screen \
             instead of firing wiri bindings. Disable in Intel Graphics \
             Control Panel → Options → Hot Keys, or rebind in config.kdl."
                .to_string(),
        );
    } else if has_intel_graphics_registry() {
        // Registry-only detection: weaker signal, mention it as advisory.
        hits.push(
            "Intel graphics registry keys present — if Ctrl+Alt+Arrow \
             bindings don't fire, check Intel Graphics Control Panel → \
             Options → Hot Keys (or rebind in config.kdl)."
                .to_string(),
        );
    }

    if has_amd_radeon_registry() {
        hits.push(
            "AMD Radeon software detected — Radeon Settings can capture \
             Ctrl+Alt+Arrows for screen rotation. If wiri bindings don't \
             fire, disable rotation hotkeys in Radeon Settings or rebind \
             in config.kdl."
                .to_string(),
        );
    }

    hits
}

/// aarch64 builds (Snapdragon X) get an empty probe — no Intel/AMD GPU
/// software ships there, so we'd just be wasting startup time.
#[cfg(not(target_arch = "x86_64"))]
pub fn detect() -> Vec<String> {
    Vec::new()
}

/// True when the Windows service `Intel(R) Hotkey Service` (or one of the
/// alternative names Intel has used over the years) is installed.  Uses
/// `OpenServiceW` because that doesn't require admin rights to query
/// installation, only to start/stop.
#[cfg(target_arch = "x86_64")]
fn has_intel_hotkey_service() -> bool {
    use windows::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW,
        SC_MANAGER_CONNECT, SERVICE_QUERY_STATUS,
    };

    let scm = unsafe {
        OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT)
    };
    let scm = match scm {
        Ok(h) => h,
        Err(_) => return false,
    };

    // Try several historical service names Intel has shipped under.
    let candidates: [&str; 3] = [
        "ihsmsvc",            // Intel Hotkey Service (recent driver bundles)
        "igfxext",            // Intel Graphics Control Panel extension
        "IntelHotkeyService", // alt label seen on some OEM images
    ];
    let mut found = false;
    for name in candidates {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let res = unsafe {
            OpenServiceW(scm, PCWSTR::from_raw(wide.as_ptr()), SERVICE_QUERY_STATUS)
        };
        if let Ok(svc) = res {
            unsafe { let _ = CloseServiceHandle(svc); }
            found = true;
            break;
        }
    }
    unsafe { let _ = CloseServiceHandle(scm); }
    found
}

/// True when the registry contains a known Intel Graphics control-panel
/// key.  Weaker signal than the service check — many machines have these
/// keys from old driver installs without the active hotkey service — so
/// callers should treat the warning as "may" not "will".
#[cfg(target_arch = "x86_64")]
fn has_intel_graphics_registry() -> bool {
    use windows::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW,
    };

    // Try a couple of well-known subkeys.
    let candidates: [&str; 2] = [
        r"SOFTWARE\Intel\Display\igfxext",
        r"SOFTWARE\Intel\Display",
    ];
    for path in candidates {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut hkey = windows::Win32::System::Registry::HKEY::default();
        let res = unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR::from_raw(wide.as_ptr()),
                0,
                KEY_READ,
                &mut hkey,
            )
        };
        if res.is_ok() {
            unsafe { let _ = RegCloseKey(hkey); }
            return true;
        }
    }
    false
}

/// True when AMD's Radeon software registry tree is present.
#[cfg(target_arch = "x86_64")]
fn has_amd_radeon_registry() -> bool {
    use windows::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW,
    };

    let candidates: [&str; 2] = [
        r"SOFTWARE\AMD\CN",
        r"SOFTWARE\AMD",
    ];
    for path in candidates {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut hkey = windows::Win32::System::Registry::HKEY::default();
        let res = unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR::from_raw(wide.as_ptr()),
                0,
                KEY_READ,
                &mut hkey,
            )
        };
        if res.is_ok() {
            unsafe { let _ = RegCloseKey(hkey); }
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `detect()` must never panic regardless of host config.  This is the
    /// only general-purpose test we can write — the actual return value
    /// depends on whether the test runner is on a box with Intel/AMD
    /// software installed, and we want CI to be neutral.
    #[test]
    fn test_detect_never_panics() {
        let _ = detect();
    }

    /// aarch64 must always return an empty vec because the entire detection
    /// path is `#[cfg(target_arch = "x86_64")]`.  The check below compiles
    /// on every target and behaves correctly on each: on aarch64 the
    /// `detect()` body returns an empty vec; on x86_64 the result may be
    /// non-empty depending on the host, so the assertion is loose.
    #[test]
    fn test_aarch64_returns_empty() {
        let v = detect();
        if cfg!(not(target_arch = "x86_64")) {
            assert!(v.is_empty(), "non-x86_64 must skip the probe entirely");
        }
    }
}
