//! Freeze exactly the focused output before the screenshot HWND exists.
use super::{gdi, pointer, Image};
use anyhow::Result;
use windows::Win32::Foundation::RECT;

pub fn freeze() -> Result<(RECT, Image, Image)> {
    let bounds = super::focused_output_rect()?;
    let clean = gdi::screen(bounds)?;
    let mut pointed = clean.clone();
    pointer::draw(&mut pointed, bounds, None)?;
    Ok((bounds, clean, pointed))
}
