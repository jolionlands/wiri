//! Eager PNG + CF_DIB clipboard data. Windows owns successful HGLOBAL transfers after return.
use super::Image;
use anyhow::{ensure, Result};
use windows::core::w;
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub fn dib(image: &Image) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(40 + image.pixels.len());
    bytes.extend_from_slice(&40u32.to_le_bytes());
    bytes.extend_from_slice(&image.width.to_le_bytes());
    bytes.extend_from_slice(&(-(image.height as i32)).to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&32u16.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    bytes.extend_from_slice(&(image.pixels.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&[0; 16]);
    bytes.extend_from_slice(&image.pixels);
    bytes
}

struct Clipboard(HWND);
impl Drop for Clipboard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
            let _ = DestroyWindow(self.0);
        }
    }
}
struct Memory(HGLOBAL);
impl Drop for Memory {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::GlobalFree(self.0);
        }
    }
}

unsafe fn put(format: u32, bytes: &[u8]) -> Result<()> {
    let memory = Memory(GlobalAlloc(GMEM_MOVEABLE, bytes.len())?);
    let ptr = GlobalLock(memory.0);
    ensure!(!ptr.is_null(), "GlobalLock for clipboard failed");
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast(), bytes.len());
    let _ = GlobalUnlock(memory.0);
    SetClipboardData(format, HANDLE(memory.0 .0))?;
    std::mem::forget(memory); // ownership passed to the clipboard
    Ok(())
}

pub fn copy(image: &Image, png: &[u8]) -> Result<()> {
    unsafe {
        // A real owner is required after EmptyClipboard. This unshown stock-class window
        // exists for this call only, also when the tray is disabled. No message loop/thread.
        let owner = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("STATIC"),
            w!("wiri screenshot clipboard"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        )?;
        if let Err(error) = OpenClipboard(owner) {
            let _ = DestroyWindow(owner);
            return Err(error.into());
        }
        let _clipboard = Clipboard(owner);
        EmptyClipboard()?;
        let png_format = RegisterClipboardFormatW(w!("PNG"));
        ensure!(png_format != 0, "RegisterClipboardFormat PNG failed");
        put(8, &dib(image))?; // CF_DIB: compatible with WinForms, Paint and older Win32 apps
        put(png_format, png)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dib_is_top_down_opaque_bgra_without_bitmap_file_header() {
        let bytes = dib(&Image {
            width: 1,
            height: 2,
            pixels: vec![3, 2, 1, 255, 9, 8, 7, 255],
        });
        assert_eq!(bytes.len(), 48);
        assert_eq!(&bytes[..4], &40u32.to_le_bytes());
        assert_eq!(&bytes[8..12], &(-2i32).to_le_bytes());
        assert_eq!(&bytes[40..], &[3, 2, 1, 255, 9, 8, 7, 255]);
    }
}
