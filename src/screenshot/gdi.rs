//! Focused-output BitBlt and PrintWindow fallback. All handles are scoped to one capture.
use super::Image;
use anyhow::{ensure, Result};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

struct Surface { dc: HDC, bitmap: HBITMAP, previous: HGDIOBJ }
impl Drop for Surface {
    fn drop(&mut self) { unsafe {
        SelectObject(self.dc, self.previous);
        let _ = DeleteObject(self.bitmap); let _ = DeleteDC(self.dc);
    } }
}

fn capture(rect: RECT, window: Option<HWND>) -> Result<Image> {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let len = super::pixel_len(width, height)?;
    unsafe {
        let screen = GetDC(None);
        ensure!(!screen.is_invalid(), "GetDC failed");
        let dc = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        ReleaseDC(None, screen);
        if dc.is_invalid() || bitmap.is_invalid() {
            if !bitmap.is_invalid() { let _ = DeleteObject(bitmap); }
            if !dc.is_invalid() { let _ = DeleteDC(dc); }
            anyhow::bail!("cannot allocate screenshot surface");
        }
        let surface = Surface { dc, bitmap, previous: SelectObject(dc, bitmap) };
        if let Some(hwnd) = window {
            ensure!(PrintWindow(hwnd, dc, PRINT_WINDOW_FLAGS(2)).as_bool()
                || PrintWindow(hwnd, dc, PRINT_WINDOW_FLAGS(0)).as_bool(), "PrintWindow failed");
        } else {
            let screen = GetDC(None);
            let result = BitBlt(dc, 0, 0, width, height, screen, rect.left, rect.top, SRCCOPY | CAPTUREBLT);
            ReleaseDC(None, screen);
            result?;
        }
        // GetDIBits requires that the bitmap is not selected into a DC.
        SelectObject(dc, surface.previous);
        let mut info = BITMAPINFO { bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width, biHeight: -height, biPlanes: 1, biBitCount: 32,
            ..Default::default()
        }, ..Default::default() };
        let mut pixels = vec![0; len];
        ensure!(GetDIBits(dc, bitmap, 0, height as u32, Some(pixels.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS) == height,
            "GetDIBits failed");
        for pixel in pixels.chunks_exact_mut(4) { pixel[3] = 255; }
        Ok(Image { width: width as u32, height: height as u32, pixels })
    }
}

pub fn screen(rect: RECT) -> Result<Image> { capture(rect, None) }

pub fn window(hwnd: HWND) -> Result<Image> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect)?; }
    capture(rect, Some(hwnd))
}
