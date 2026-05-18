use anyhow::Result;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use tracing::warn;
use windows::core::PCWSTR;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_WRITE, KEY_READ, REG_SZ,
};

const RUN_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

pub struct StartupManager {
    app_name: String,
    exe_path: String,
}

impl StartupManager {
    pub fn new() -> Self {
        let exe_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        Self {
            app_name: "wiri".to_string(),
            exe_path,
        }
    }

    pub fn with_name(mut self, name: &str) -> Self {
        self.app_name = name.to_string();
        self
    }

    pub fn is_registered(&self) -> bool {
        self.get_registered_path().is_some()
    }

    pub fn get_registered_path(&self) -> Option<String> {
        unsafe {
            let mut hkey: HKEY = HKEY::default();
            let key_path: Vec<u16> = OsStr::new(RUN_KEY_PATH)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let result = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR::from_raw(key_path.as_ptr()),
                0,
                KEY_READ,
                &mut hkey,
            );

            if result.is_err() {
                return None;
            }

            let value_name: Vec<u16> = OsStr::new(&self.app_name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let mut data: [u8; 512] = [0; 512];
            let mut data_size = data.len() as u32;

            let result = RegQueryValueExW(
                hkey,
                PCWSTR::from_raw(value_name.as_ptr()),
                None,
                None,
                Some(data.as_mut_ptr()),
                Some(&mut data_size),
            );

            if let Err(e) = RegCloseKey(hkey).ok() {
                warn!("StartupManager::get_registered_path: RegCloseKey failed: {:?}", e);
            }

            if result.is_ok() {
                let wide_chars: Vec<u16> = data
                    .chunks_exact(2)
                    .take((data_size as usize / 2).saturating_sub(1))
                    .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
                    .collect();
                Some(String::from_utf16_lossy(&wide_chars))
            } else {
                None
            }
        }
    }

    pub fn register(&self) -> Result<()> {
        unsafe {
            let mut hkey: HKEY = HKEY::default();
            let key_path: Vec<u16> = OsStr::new(RUN_KEY_PATH)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let result = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR::from_raw(key_path.as_ptr()),
                0,
                KEY_WRITE,
                &mut hkey,
            );
            if result.is_err() {
                return Err(anyhow::anyhow!("Failed to open Run registry key: {:?}", result));
            }

            let value_name: Vec<u16> = OsStr::new(&self.app_name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let path_bytes: Vec<u8> = OsStr::new(&self.exe_path)
                .encode_wide()
                .chain(std::iter::once(0))
                .flat_map(|c| c.to_le_bytes())
                .collect();

            let result = RegSetValueExW(
                hkey,
                PCWSTR::from_raw(value_name.as_ptr()),
                0,
                REG_SZ,
                Some(&path_bytes),
            );
            if result.is_err() {
                let _ = RegCloseKey(hkey).ok();
                return Err(anyhow::anyhow!("Failed to set registry value: {:?}", result));
            }

            let result = RegCloseKey(hkey);
            if result.is_err() {
                return Err(anyhow::anyhow!("Failed to close registry key: {:?}", result));
            }
        }

        Ok(())
    }

    pub fn unregister(&self) -> Result<()> {
        unsafe {
            let mut hkey: HKEY = HKEY::default();
            let key_path: Vec<u16> = OsStr::new(RUN_KEY_PATH)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let result = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR::from_raw(key_path.as_ptr()),
                0,
                KEY_WRITE,
                &mut hkey,
            );
            if result.is_err() {
                return Err(anyhow::anyhow!("Failed to open Run registry key: {:?}", result));
            }

            let value_name: Vec<u16> = OsStr::new(&self.app_name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let result = RegDeleteValueW(hkey, PCWSTR::from_raw(value_name.as_ptr()));
            if result.is_err() {
                let _ = RegCloseKey(hkey).ok();
                return Err(anyhow::anyhow!("Failed to delete registry value: {:?}", result));
            }

            let result = RegCloseKey(hkey);
            if result.is_err() {
                return Err(anyhow::anyhow!("Failed to close registry key: {:?}", result));
            }
        }

        Ok(())
    }
}

impl Default for StartupManager {
    fn default() -> Self {
        Self::new()
    }
}
