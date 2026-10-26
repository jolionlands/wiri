//! Bounded, non-messaging snapshot of a window's redirection DC. Unlike PrintWindow
//! or waiting for WGC's first frame this never calls the application. GPU-only,
//! protected, empty and black sources fail closed; do not delay close to get an image.
use crate::utils::Rect;
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowDisplayAffinity, GetWindowRect, IsWindowVisible,
};

pub struct Snapshot {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

/// Conservative black-frame rejection uses the interior, not a bright native caption.
fn has_content(width: u32, height: u32, pixels: &[u32]) -> bool {
    if width == 0 || height == 0 || pixels.len() != width as usize * height as usize {
        return false;
    }
    let margin_x = width / 8;
    let margin_y = height / 4;
    (margin_y..height.saturating_sub(margin_y))
        .step_by(8)
        .any(|y| {
            (margin_x..width.saturating_sub(margin_x))
                .step_by(8)
                .any(|x| {
                    let p = pixels[(y * width + x) as usize];
                    ((p >> 16) & 255).max((p >> 8) & 255).max(p & 255) > 3
                })
        })
}

pub fn frame(hwnd: isize) -> Option<Rect> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    let mut r = RECT::default();
    unsafe {
        if DwmGetWindowAttribute(
            HWND(hwnd as *mut _),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut _ as *mut _,
            std::mem::size_of_val(&r) as u32,
        )
        .is_err()
        {
            GetWindowRect(HWND(hwnd as *mut _), &mut r).ok()?;
        }
    }
    (r.right > r.left && r.bottom > r.top).then(|| {
        Rect::new(
            r.left,
            r.top,
            (r.right - r.left) as u32,
            (r.bottom - r.top) as u32,
        )
    })
}

/// Returned dimensions correspond to the full HWND, matching DWM's source coordinates.
pub fn capture(raw: isize) -> Option<Snapshot> {
    unsafe {
        let hwnd = HWND(raw as *mut _);
        if raw == 0 || !IsWindowVisible(hwnd).as_bool() {
            return None;
        }
        let mut affinity = 0;
        if GetWindowDisplayAffinity(hwnd, &mut affinity).is_ok() && affinity != 0 {
            return None;
        }
        let mut r = windows::Win32::Foundation::RECT::default();
        GetWindowRect(hwnd, &mut r).ok()?;
        let (w, h) = (r.right.checked_sub(r.left)?, r.bottom.checked_sub(r.top)?);
        if w <= 0 || h <= 0 || w > 8192 || h > 8192 || w as u64 * h as u64 > 8_388_608 {
            return None;
        }
        let source = GetWindowDC(hwnd);
        if source.is_invalid() {
            return None;
        }
        let dc = CreateCompatibleDC(source);
        if dc.is_invalid() {
            ReleaseDC(hwnd, source);
            return None;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0);
        let bitmap = match bitmap {
            Ok(bitmap) if !bits.is_null() => bitmap,
            _ => {
                let _ = DeleteDC(dc);
                ReleaseDC(hwnd, source);
                return None;
            }
        };
        let old = SelectObject(dc, bitmap);
        let ok = BitBlt(dc, 0, 0, w, h, source, 0, 0, SRCCOPY).is_ok();
        // Flush GDI's batched writes before reading DIB memory.
        let _ = GdiFlush();
        let mut pixels = if ok {
            std::slice::from_raw_parts(bits as *const u32, w as usize * h as usize).to_vec()
        } else {
            Vec::new()
        };
        SelectObject(dc, old);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(dc);
        ReleaseDC(hwnd, source);
        if !has_content(w as u32, h as u32, &pixels) {
            return None;
        }
        for p in &mut pixels {
            *p |= 0xff00_0000;
        }
        Some(Snapshot {
            width: w as u32,
            height: h as u32,
            pixels,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_empty_protected_black_and_caption_only_frames() {
        assert!(!has_content(0, 0, &[]));
        assert!(!has_content(10, 10, &[]));
        assert!(!has_content(32, 32, &[0xff00_0000; 1024]));
        let mut px = vec![0; 1024];
        px[..32].fill(0xffff_ffff);
        assert!(!has_content(32, 32, &px));
        px.fill(0xff20_2020);
        assert!(has_content(32, 32, &px));
        px.fill(0xff00_00ff);
        assert!(has_content(32, 32, &px));
    }
}
