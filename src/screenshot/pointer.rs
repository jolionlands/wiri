//! Request-local cursor compositing. The cursor handle belongs to the OS; the
//! temporary DIB/DC and GetIconInfo bitmaps are always released.
use super::Image;
use anyhow::{ensure, Result};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub fn draw(image: &mut Image, bounds: RECT, receiving_window: Option<HWND>) -> Result<()> {
    unsafe {
        let mut cursor = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        GetCursorInfo(&mut cursor)?;
        if cursor.flags != CURSOR_SHOWING {
            return Ok(());
        }
        if let Some(hwnd) = receiving_window {
            // niri only draws a window's pointer while that window receives it.
            let receiver = GetAncestor(WindowFromPoint(cursor.ptScreenPos), GA_ROOT);
            if receiver != GetAncestor(hwnd, GA_ROOT) {
                return Ok(());
            }
        }
        let mut icon = ICONINFO::default();
        GetIconInfo(cursor.hCursor, &mut icon)?;
        let x = cursor.ptScreenPos.x - bounds.left - icon.xHotspot as i32;
        let y = cursor.ptScreenPos.y - bounds.top - icon.yHotspot as i32;
        if !icon.hbmMask.is_invalid() {
            let _ = DeleteObject(icon.hbmMask);
        }
        if !icon.hbmColor.is_invalid() {
            let _ = DeleteObject(icon.hbmColor);
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: image.width as i32,
                biHeight: -(image.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = CreateCompatibleDC(None);
        ensure!(!dc.is_invalid(), "cannot allocate cursor DC");
        let mut data = std::ptr::null_mut();
        let bitmap = match CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut data, None, 0) {
            Ok(bitmap) => bitmap,
            Err(error) => {
                let _ = DeleteDC(dc);
                return Err(error.into());
            }
        };
        let previous = SelectObject(dc, bitmap);
        std::ptr::copy_nonoverlapping(image.pixels.as_ptr(), data.cast::<u8>(), image.pixels.len());
        let result = DrawIconEx(dc, x, y, cursor.hCursor, 0, 0, 0, None, DI_NORMAL);
        let _ = GdiFlush();
        if result.is_ok() {
            std::ptr::copy_nonoverlapping(
                data.cast::<u8>(),
                image.pixels.as_mut_ptr(),
                image.pixels.len(),
            );
            for pixel in image.pixels.chunks_exact_mut(4) {
                pixel[3] = 255;
            }
        }
        SelectObject(dc, previous);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(dc);
        result?;
    }
    Ok(())
}
