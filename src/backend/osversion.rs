//! Which Windows this is: the build number, read once from the registry.
//!
//! Windows 11 (build 22000 and later) rounds the corners of top-level windows and lets a
//! program choose how much (`DWMWA_WINDOW_CORNER_PREFERENCE`); Windows 10 does neither.

use std::sync::OnceLock;

/// The first build of Windows 11.
pub const WINDOWS_11_BUILD: u32 = 22000;

/// The number in the text of `CurrentBuildNumber` (`"26200"`); 0 when it is not a number.
pub fn parse_build_number(text: &str) -> u32 {
    text.trim().trim_end_matches('\0').parse().unwrap_or(0)
}

/// The build number of the running Windows; 0 when it cannot be read.
pub fn windows_build() -> u32 {
    static BUILD: OnceLock<u32> = OnceLock::new();
    *BUILD.get_or_init(read_build_number)
}

/// True on Windows 11 and later: windows have rounded corners there, and DWM can be asked for
/// a different rounding.
pub fn supports_rounded_corners() -> bool {
    windows_build() >= WINDOWS_11_BUILD
}

fn read_build_number() -> u32 {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
    };

    unsafe {
        let key_path: Vec<u16> = OsStr::new(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut hkey: HKEY = HKEY::default();
        let open = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR::from_raw(key_path.as_ptr()),
            0,
            KEY_READ,
            &mut hkey,
        );
        if open.is_err() {
            return 0;
        }
        let value_name: Vec<u16> = OsStr::new("CurrentBuildNumber")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut buf = [0u16; 32];
        let mut size: u32 = (buf.len() * 2) as u32;
        let q = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(value_name.as_ptr()),
            None,
            None,
            Some(buf.as_mut_ptr() as *mut u8),
            Some(&mut size),
        );
        let _ = RegCloseKey(hkey);
        if q.is_err() {
            return 0;
        }
        let chars = (size as usize / 2).min(buf.len());
        parse_build_number(&String::from_utf16_lossy(&buf[..chars]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_number_is_read_from_its_text() {
        assert_eq!(parse_build_number("26200"), 26200);
        assert_eq!(parse_build_number(" 22631\0"), 22631);
        assert_eq!(parse_build_number("19045"), 19045);
        assert_eq!(parse_build_number(""), 0);
        assert_eq!(parse_build_number("Windows"), 0);
    }

    #[test]
    fn windows_11_starts_at_build_22000() {
        assert!(22000 >= WINDOWS_11_BUILD);
        assert!(19045 < WINDOWS_11_BUILD);
    }
}
