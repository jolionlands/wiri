//! On-demand screenshots. No persistent windows, codecs, devices, workers or clock.
mod clipboard;
mod encode;
mod gdi;
pub mod path;
mod wgc;

use anyhow::{ensure, Context, Result};
use parking_lot::{Mutex, RwLock};
use std::path::Path;
use tracing::{debug, info, warn};
use windows::core::w;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetForegroundWindow, GetWindowThreadProcessId};

use crate::ipc::IpcEvent;
use path::ScreenshotPath;

static SETTING: RwLock<ScreenshotPath> = RwLock::new(ScreenshotPath::Pictures);
static EVENT_TX: Mutex<Option<tokio::sync::broadcast::Sender<IpcEvent>>> = Mutex::new(None);
// Serialize active requests, not desktop activity. In particular two captures cannot race
// EmptyClipboard/SetClipboardData or overwrite each other's one-second template path mid-write.
static CAPTURE: Mutex<()> = Mutex::new(());

pub fn configure(config: &crate::config::Config) { *SETTING.write() = config.screenshot_path.clone(); }
pub(crate) fn set_event_sender(tx: tokio::sync::broadcast::Sender<IpcEvent>) { *EVENT_TX.lock() = Some(tx); }

pub struct Image { pub width: u32, pub height: u32, pub pixels: Vec<u8> }

fn pixel_len(width: i32, height: i32) -> Result<usize> {
    ensure!(width > 0 && height > 0, "screenshot has nonpositive dimensions");
    let len = (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4)).context("screenshot too large")?;
    ensure!(len <= u32::MAX as usize, "screenshot exceeds WIC buffer limit");
    Ok(len)
}

/// None captures only the output containing the foreground window, never the virtual desktop.
/// An explicit path is used by the older CaptureWindow/ScreenshotWindow IPC requests.
pub fn capture(window: Option<isize>, explicit_path: Option<&Path>) -> Result<Option<String>> {
    let _capture = CAPTURE.lock();
    let _com = encode::ComScope::new()?;
    let image = match window {
        Some(raw) => {
            ensure!(raw != 0, "no focused window to capture");
            let hwnd = HWND(raw as *mut _);
            match wgc::capture(hwnd) {
                Ok(image) => { debug!("Screenshot window: Windows.Graphics.Capture"); image }
                Err(error) => { debug!("Windows.Graphics.Capture unavailable: {error:#}; using PrintWindow"); gdi::window(hwnd)? }
            }
        }
        None => {
            let rect = focused_output_rect()?;
            gdi::screen(rect)?
        }
    };
    let png = encode::png(&image)?;
    clipboard::copy(&image, &png).context("copying screenshot to clipboard")?;
    let path = match explicit_path {
        Some(path) => Some(path.to_path_buf()),
        None => path::resolve(&SETTING.read())?,
    };
    let saved = if let Some(path) = path {
        let result = (|| -> Result<()> {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) { std::fs::create_dir_all(parent)?; }
            std::fs::write(&path, &png).with_context(|| format!("saving {}", path.display()))?;
            Ok(())
        })();
        match result {
            Ok(()) => Some(path.to_string_lossy().into_owned()),
            Err(error) => {
                // A failed disk write never loses the successful clipboard capture.
                warn!("Screenshot copied but not saved: {error:#}");
                balloon("wiri: screenshot copied", &format!("Could not save: {error}"));
                emit(None);
                return Err(error);
            }
        }
    } else { None };
    info!("ScreenshotCaptured: {}x{} path={saved:?}", image.width, image.height);
    emit(saved.clone());
    balloon("wiri: screenshot captured", &saved.as_ref().map(|p| format!("Copied to clipboard and saved to {p}")).unwrap_or_else(|| "Copied to clipboard".into()));
    Ok(saved)
}

pub fn focused_window(explicit_path: Option<&Path>) -> Result<Option<String>> {
    let hwnd = unsafe { GetForegroundWindow().0 as isize };
    capture(Some(hwnd), explicit_path)
}

fn emit(path: Option<String>) {
    if let Some(tx) = EVENT_TX.lock().as_ref() { let _ = tx.send(IpcEvent::ScreenshotCaptured { path }); }
}

fn focused_output_rect() -> Result<RECT> {
    unsafe {
        let monitor = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        ensure!(GetMonitorInfoW(monitor, &mut info).as_bool(), "cannot find focused output");
        Ok(info.rcMonitor)
    }
}

fn balloon(title: &str, text: &str) {
    unsafe {
        // Reuse the existing tray icon; never create a second icon or resident window.
        let Ok(hwnd) = FindWindowW(w!("WiriTrayWnd"), None) else { return };
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != windows::Win32::System::Threading::GetCurrentProcessId() { return; }
        let mut nid = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd, uID: 1, uFlags: NIF_INFO, dwInfoFlags: NIIF_INFO, ..Default::default() };
        for (dst, src) in nid.szInfoTitle[..63].iter_mut().zip(title.encode_utf16()) { *dst = src; }
        for (dst, src) in nid.szInfo[..255].iter_mut().zip(text.encode_utf16()) { *dst = src; }
        if Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool() { debug!("Screenshot notification shown"); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn dimensions_are_checked_before_allocation() {
        assert!(pixel_len(0, 2).is_err()); assert!(pixel_len(-1, 2).is_err());
        assert!(pixel_len(i32::MAX, i32::MAX).is_err()); assert_eq!(pixel_len(2, 3).unwrap(), 24);
    }
    #[test] fn event_has_niri_path_and_null_for_clipboard_only() {
        let value = serde_json::to_value(IpcEvent::ScreenshotCaptured { path: None }).unwrap();
        assert_eq!(value, serde_json::json!({"type":"screenshot_captured","data":{"path":null}}));
    }
}
