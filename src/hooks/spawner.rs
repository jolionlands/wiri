use anyhow::{Context, Result};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{BOOL, CloseHandle, HWND};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
    HGDIOBJ, SRCCOPY,
};
use windows::Win32::System::Threading::{
    CreateProcessW, GetProcessId, CREATE_UNICODE_ENVIRONMENT, DETACHED_PROCESS,
    STARTUPINFOW, PROCESS_INFORMATION,
};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

#[derive(Debug, Clone)]
pub struct Notification {
    pub title: String,
    pub message: String,
    pub icon_type: NotificationIcon,
}

#[derive(Debug, Clone, Copy)]
pub enum NotificationIcon {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct ScreenshotEvent {
    pub monitor_id: Option<u64>,
    pub save_path: Option<String>,
}

pub struct Spawner {
    env_vars: HashMap<String, String>,
}

impl Spawner {
    pub fn new() -> Self {
        let env_vars: HashMap<String, String> = std::env::vars().collect();
        Self { env_vars }
    }

    pub fn spawn(&self, program: &Path, args: &[&str], detached: bool) -> Result<u32> {
        let mut cmd_line = program.to_string_lossy().into_owned();
        for arg in args {
            cmd_line.push(' ');
            cmd_line.push_str(arg);
        }

        unsafe {
            let mut si = STARTUPINFOW::default();
            si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            let mut pi = PROCESS_INFORMATION::default();

            let creation_flags = if detached {
                DETACHED_PROCESS | CREATE_UNICODE_ENVIRONMENT
            } else {
                CREATE_UNICODE_ENVIRONMENT
            };

            let program_wide: Vec<u16> = OsStr::new(program)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let cmd_wide: Vec<u16> = OsStr::new(&cmd_line)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let env_block = self.get_environment_block()?;

            let result = CreateProcessW(
                PCWSTR::from_raw(program_wide.as_ptr()),
                PWSTR::from_raw(cmd_wide.as_ptr() as *mut u16),
                None,
                None,
                BOOL::from(false),
                creation_flags,
                Some(env_block.as_ptr() as *mut _),
                None,
                &mut si,
                &mut pi,
            );

            result.context("CreateProcessW failed")?;
            let pid = pi.dwProcessId;
            // Close handles immediately — we only need the PID
            let _ = CloseHandle(pi.hProcess);
            let _ = CloseHandle(pi.hThread);
            Ok(pid)
        }
    }

    pub fn spawn_with_env(
        &self,
        program: &Path,
        args: &[&str],
        env: &[(String, String)],
        detached: bool,
    ) -> Result<u32> {
        let mut cmd_line = program.to_string_lossy().into_owned();
        for arg in args {
            cmd_line.push(' ');
            cmd_line.push_str(arg);
        }

        unsafe {
            let mut si = STARTUPINFOW::default();
            si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            let mut pi = PROCESS_INFORMATION::default();

            let creation_flags = if detached {
                DETACHED_PROCESS | CREATE_UNICODE_ENVIRONMENT
            } else {
                CREATE_UNICODE_ENVIRONMENT
            };

            let program_wide: Vec<u16> = OsStr::new(program)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let cmd_wide: Vec<u16> = OsStr::new(&cmd_line)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let mut full_env = self.env_vars.clone();
            for (key, value) in env {
                full_env.insert(key.clone(), value.clone());
            }

            let env_block = self.env_to_block(&full_env)?;

            let result = CreateProcessW(
                PCWSTR::from_raw(program_wide.as_ptr()),
                PWSTR::from_raw(cmd_wide.as_ptr() as *mut u16),
                None,
                None,
                BOOL::from(false),
                creation_flags,
                Some(env_block.as_ptr() as *mut _),
                None,
                &mut si,
                &mut pi,
            );

            result.context("CreateProcessW failed")?;
            let pid = pi.dwProcessId;
            // Close handles immediately — we only need the PID
            let _ = CloseHandle(pi.hProcess);
            let _ = CloseHandle(pi.hThread);
            Ok(pid)
        }
    }

    unsafe fn get_environment_block(&self) -> Result<Box<[u16]>> {
        let mut result: Vec<u16> = Vec::new();
        for (key, value) in &self.env_vars {
            // Proper UTF-16 encoding for Windows environment block
            result.extend(key.encode_utf16());
            result.push('=' as u16);
            result.extend(value.encode_utf16());
            result.push(0); // Null terminator for each VAR=VALUE entry
        }
        result.push(0); // Final null terminator
        Ok(result.into_boxed_slice())
    }

    unsafe fn env_to_block(&self, env: &HashMap<String, String>) -> Result<Box<[u16]>> {
        let mut result: Vec<u16> = Vec::new();
        for (key, value) in env.iter() {
            // Proper UTF-16 encoding for Windows environment block
            result.extend(key.encode_utf16());
            result.push('=' as u16);
            result.extend(value.encode_utf16());
            result.push(0); // Null terminator for each VAR=VALUE entry
        }
        result.push(0); // Final null terminator
        Ok(result.into_boxed_slice())
    }

    /// Spawn a process elevated (UAC "Run as administrator") via ShellExecuteExW.
    /// Returns the new process's PID on success.
    pub fn spawn_elevated(cmd: &str, args: &[&str]) -> Result<u32> {
        let verb: Vec<u16> = "runas\0".encode_utf16().collect();
        let file: Vec<u16> = OsStr::new(cmd)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let params_str = args.join(" ");
        let params: Vec<u16> = OsStr::new(&params_str)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        unsafe {
            let mut sei = SHELLEXECUTEINFOW {
                cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_NOCLOSEPROCESS,
                lpVerb: PCWSTR::from_raw(verb.as_ptr()),
                lpFile: PCWSTR::from_raw(file.as_ptr()),
                lpParameters: PCWSTR::from_raw(params.as_ptr()),
                nShow: windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL.0 as i32,
                ..Default::default()
            };

            ShellExecuteExW(&mut sei).context("ShellExecuteExW failed")?;

            let pid = if !sei.hProcess.is_invalid() {
                let pid = GetProcessId(sei.hProcess);
                let _ = CloseHandle(sei.hProcess);
                pid
            } else {
                0
            };
            Ok(pid)
        }
    }

    /// Capture the full virtual desktop and save it as a BMP file at `path`.
    /// Returns a [`ScreenshotEvent`] with `save_path` set to the canonical path.
    pub fn capture_screenshot_to_file<P: AsRef<Path>>(
        monitor_id: Option<u64>,
        path: P,
    ) -> Result<ScreenshotEvent> {
        let path = path.as_ref();
        capture_desktop_to_bmp(path)?;
        Ok(ScreenshotEvent {
            monitor_id,
            save_path: Some(path.to_string_lossy().into_owned()),
        })
    }

    /// Capture the full virtual desktop in-memory and save it to a temporary BMP file.
    /// Returns a [`ScreenshotEvent`] with `save_path` set to the temp file path.
    pub fn capture_screenshot(monitor_id: Option<u64>) -> Result<ScreenshotEvent> {
        // Build a temp path: <TEMP>\wiri_screenshot_<timestamp_ns>.bmp
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp_path = std::env::temp_dir().join(format!("wiri_screenshot_{}.bmp", ts));
        capture_desktop_to_bmp(&tmp_path)?;
        Ok(ScreenshotEvent {
            monitor_id,
            save_path: Some(tmp_path.to_string_lossy().into_owned()),
        })
    }
}

/// Capture the full virtual desktop (all monitors) and write a BMP file to `path`.
///
/// Uses pure GDI: `GetDC(NULL)` → `CreateCompatibleDC` → `CreateCompatibleBitmap` →
/// `BitBlt` → `GetDIBits` → manual BITMAPFILEHEADER + BITMAPINFOHEADER + pixel bytes.
///
/// All GDI handles are released on every exit path.
fn capture_desktop_to_bmp(path: &Path) -> Result<()> {
    unsafe {
        // --- Acquire desktop DC ---
        let desktop_dc = GetDC(HWND(std::ptr::null_mut()));
        if desktop_dc.is_invalid() {
            anyhow::bail!("GetDC(NULL) failed — no display available");
        }

        // Helper closure to release DC and bail on errors.
        // We manage cleanup manually before each early-return.

        // --- Virtual desktop dimensions ---
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);

        if vw <= 0 || vh <= 0 {
            let _ = ReleaseDC(HWND(std::ptr::null_mut()), desktop_dc);
            anyhow::bail!("GetSystemMetrics returned zero virtual desktop size");
        }

        let width = vw as u32;
        let height = vh as u32;

        // --- Create memory DC + compatible bitmap ---
        let mem_dc = CreateCompatibleDC(desktop_dc);
        if mem_dc.is_invalid() {
            let _ = ReleaseDC(HWND(std::ptr::null_mut()), desktop_dc);
            anyhow::bail!("CreateCompatibleDC failed");
        }

        let bitmap = CreateCompatibleBitmap(desktop_dc, vw, vh);
        if bitmap.is_invalid() {
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(HWND(std::ptr::null_mut()), desktop_dc);
            anyhow::bail!("CreateCompatibleBitmap failed");
        }

        // Select bitmap into memory DC; save the old one so we can restore.
        let bitmap_obj: HGDIOBJ = HGDIOBJ::from(bitmap);
        let old_obj = SelectObject(mem_dc, bitmap_obj);

        // --- BitBlt: copy virtual desktop into the memory DC ---
        let blt_ok = BitBlt(mem_dc, 0, 0, vw, vh, desktop_dc, vx, vy, SRCCOPY);
        if blt_ok.is_err() {
            let _ = SelectObject(mem_dc, old_obj);
            let _ = DeleteObject(bitmap_obj);
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(HWND(std::ptr::null_mut()), desktop_dc);
            anyhow::bail!("BitBlt failed");
        }

        // --- Extract pixel data via GetDIBits ---
        // 32-bit BGRA — bottom-up (standard BMP orientation).
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: vw,
                biHeight: vh,   // positive = bottom-up
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [Default::default(); 1],
        };

        let row_bytes = width * 4; // 32 bpp, no padding needed (width * 4 always DWORD-aligned for any w)
        let pixel_data_size = (row_bytes * height) as usize;
        let mut pixels: Vec<u8> = vec![0u8; pixel_data_size];

        let scan_lines = GetDIBits(
            mem_dc,
            bitmap,
            0,
            height,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        // --- Cleanup GDI resources ---
        let _ = SelectObject(mem_dc, old_obj);
        let _ = DeleteObject(bitmap_obj);
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(HWND(std::ptr::null_mut()), desktop_dc);

        if scan_lines == 0 {
            anyhow::bail!("GetDIBits failed (returned 0 scan lines)");
        }

        // --- Write BMP file ---
        // BITMAPFILEHEADER: 14 bytes
        // BITMAPINFOHEADER: 40 bytes
        // Pixel data: row_bytes * height bytes
        let file_header_size: u32 = 14;
        let info_header_size: u32 = 40;
        let pixel_offset: u32 = file_header_size + info_header_size;
        let file_size: u32 = pixel_offset + pixel_data_size as u32;

        let mut bmp: Vec<u8> = Vec::with_capacity(file_size as usize);

        // BITMAPFILEHEADER
        bmp.extend_from_slice(b"BM");                         // bfType
        bmp.extend_from_slice(&file_size.to_le_bytes());      // bfSize
        bmp.extend_from_slice(&0u16.to_le_bytes());           // bfReserved1
        bmp.extend_from_slice(&0u16.to_le_bytes());           // bfReserved2
        bmp.extend_from_slice(&pixel_offset.to_le_bytes());   // bfOffBits

        // BITMAPINFOHEADER (40 bytes, BI_RGB)
        bmp.extend_from_slice(&info_header_size.to_le_bytes());  // biSize
        bmp.extend_from_slice(&(width as i32).to_le_bytes());    // biWidth
        bmp.extend_from_slice(&(height as i32).to_le_bytes());   // biHeight (positive = bottom-up)
        bmp.extend_from_slice(&1u16.to_le_bytes());              // biPlanes
        bmp.extend_from_slice(&32u16.to_le_bytes());             // biBitCount
        bmp.extend_from_slice(&0u32.to_le_bytes());              // biCompression (BI_RGB)
        bmp.extend_from_slice(&(pixel_data_size as u32).to_le_bytes()); // biSizeImage
        bmp.extend_from_slice(&0i32.to_le_bytes());              // biXPelsPerMeter
        bmp.extend_from_slice(&0i32.to_le_bytes());              // biYPelsPerMeter
        bmp.extend_from_slice(&0u32.to_le_bytes());              // biClrUsed
        bmp.extend_from_slice(&0u32.to_le_bytes());              // biClrImportant

        // Pixel data
        bmp.extend_from_slice(&pixels);

        std::fs::write(path, &bmp)
            .with_context(|| format!("failed to write BMP to {}", path.display()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capture_screenshot_to_file_bmp_signature() {
        let tmp_path = std::env::temp_dir().join("wiri_test_screenshot.bmp");

        match Spawner::capture_screenshot_to_file(None, &tmp_path) {
            Ok(event) => {
                // Verify the BMP signature
                let bytes = std::fs::read(&tmp_path)
                    .expect("BMP file should be readable after capture");
                assert!(
                    bytes.len() >= 2,
                    "BMP file must be at least 2 bytes"
                );
                assert_eq!(
                    &bytes[0..2],
                    b"BM",
                    "BMP file must start with 'BM' signature"
                );
                assert!(
                    event.save_path.is_some(),
                    "ScreenshotEvent.save_path should be populated"
                );
                // Clean up
                let _ = std::fs::remove_file(&tmp_path);
            }
            Err(e) => {
                // In CI environments without a display, GetDC(NULL) may fail.
                // Skip gracefully rather than failing the test.
                let msg = e.to_string();
                if msg.contains("GetDC(NULL) failed")
                    || msg.contains("zero virtual desktop size")
                {
                    eprintln!("Skipping screenshot test — no display: {}", msg);
                } else {
                    panic!("capture_screenshot_to_file failed unexpectedly: {}", e);
                }
            }
        }
    }
}

impl Default for Spawner {
    fn default() -> Self {
        Self::new()
    }
}
