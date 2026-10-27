//! niri's Latin-key fallback on Windows (wiki:Configuration:-Key-Bindings, tip).
//!
//! niri binds by keysym, so a bind written `Mod+T` also fires while a Cyrillic, Greek or
//! any other non-Latin layout is active: when the layout's own keysym does not match, niri
//! tries the Latin keysym of the same key code.
//!
//! wiri binds Windows virtual keys, and those are already layout-independent for the
//! letters on almost every layout — the Russian and Greek layouts, for instance, report
//! the same `VK_T` for the physical T key, so `Mod+T` fires there without any help.  For a
//! layout that *does* remap the virtual keys (a Dvorak-family or other character remap),
//! the keyboard engine asks the installed [`LatinFallback`] for the virtual key the same
//! physical key has in the Latin (US) layout and tries that bind instead.
//!
//! The mapping goes through the scan code, because the scan code is what the physical key
//! is: `MapVirtualKeyExW(vk, MAPVK_VK_TO_VSC_EX, <the active layout>)` and then
//! `MapVirtualKeyExW(scan, MAPVK_VSC_TO_VK_EX, <en-US>)`.  The en-US half uses an English
//! (United States) layout that is already loaded on the machine, and a built-in scan-code
//! table when there is none: wiri never loads, activates or changes a keyboard layout, and
//! never writes a system setting.
//!
//! Results are cached per virtual key, so a keystroke costs at most one map lookup, and
//! nothing runs at rest: [`system_fallback`] only reads the loaded layouts, which the
//! caller does once at startup and once per hotkey reload.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, GetKeyboardLayoutList, MapVirtualKeyExW, HKL, MAPVK_VK_TO_VSC_EX,
    MAPVK_VSC_TO_VK_EX,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// The Latin (US) virtual key for the same physical key.
///
/// The engine calls this only when the observed `(modifiers, virtual key)` does not name a
/// bind, so a layout that already reports Latin virtual keys costs one cache lookup and
/// nothing else.
pub trait LatinFallback: Send + Sync {
    /// The Latin (US) virtual key for `vk`, or `None` when the key has no Latin twin (or
    /// already is one).
    fn latin_key(&self, vk: u32) -> Option<u32>;
}

/// The Windows mapping for the layout that is active right now.
///
/// The layout handles are kept as plain integers so the fallback can be shared with the
/// hook thread (Windows' `HKL` is a raw pointer and therefore neither `Send` nor `Sync`;
/// a layout handle is only ever passed back to `MapVirtualKeyExW`).
pub struct LayoutKeys {
    /// The layout whose virtual keys the hook observed.
    active: isize,
    /// An English (United States) layout that is already loaded, when the machine has one.
    us: Option<isize>,
    /// `vk → Latin (US) vk`, computed once per key.
    cache: Mutex<HashMap<u32, Option<u32>>>,
}

impl LatinFallback for LayoutKeys {
    fn latin_key(&self, vk: u32) -> Option<u32> {
        if let Some(cached) = self.cache.lock().get(&vk) {
            return *cached;
        }
        let mapped = self.map(vk);
        self.cache.lock().insert(vk, mapped);
        mapped
    }
}

impl LayoutKeys {
    /// Read the active layout (the foreground thread's) and the loaded en-US layout.
    ///
    /// Called once at startup and once per hotkey reload — never per keystroke.
    pub fn current() -> Option<Self> {
        Some(Self {
            active: active_layout()?,
            us: us_layout(),
            cache: Mutex::new(HashMap::new()),
        })
    }

    fn map(&self, vk: u32) -> Option<u32> {
        let scan = unsafe { MapVirtualKeyExW(vk, MAPVK_VK_TO_VSC_EX, hkl(self.active)) };
        if scan == 0 {
            return None;
        }
        // `MAPVK_VK_TO_VSC_EX` sets 0xE000 for an extended key; a bind names the scan code
        // itself, so the flag goes.
        let scan = scan & 0xFF;
        if scan == 0 {
            return None;
        }
        let latin = match self.us {
            Some(us) => unsafe { MapVirtualKeyExW(scan, MAPVK_VSC_TO_VK_EX, hkl(us)) },
            None => u32::from(us_virtual_key(scan as u8)?),
        };
        (latin != 0 && latin != vk).then_some(latin)
    }
}

fn hkl(raw: isize) -> HKL {
    HKL(raw as *mut std::ffi::c_void)
}

/// The fallback for the layout that is active now; `None` when Windows cannot say which
/// layout that is (the engine then matches virtual keys as before).
pub fn system_fallback() -> Option<Arc<dyn LatinFallback>> {
    LayoutKeys::current().map(|keys| Arc::new(keys) as Arc<dyn LatinFallback>)
}

/// The layout of the thread that owns the foreground window, else this thread's own.
fn active_layout() -> Option<isize> {
    unsafe {
        let foreground = GetForegroundWindow();
        if !foreground.0.is_null() {
            let thread = GetWindowThreadProcessId(foreground, None);
            if thread != 0 {
                let hkl = GetKeyboardLayout(thread);
                if !hkl.0.is_null() {
                    return Some(hkl.0 as isize);
                }
            }
        }
        let own = GetKeyboardLayout(0);
        (!own.0.is_null()).then_some(own.0 as isize)
    }
}

/// An already-loaded English (United States) layout.  Nothing is loaded or activated: when
/// the machine has none, the built-in table stands in for it.
fn us_layout() -> Option<isize> {
    unsafe {
        let count = GetKeyboardLayoutList(None);
        if count <= 0 {
            return None;
        }
        let mut layouts = vec![HKL::default(); count as usize];
        let written = GetKeyboardLayoutList(Some(&mut layouts));
        if written <= 0 {
            return None;
        }
        layouts.truncate(written as usize);
        layouts
            .into_iter()
            .find(|hkl| language_id(hkl.0 as isize) == 0x0409)
            .map(|hkl| hkl.0 as isize)
    }
}

/// The low word of a layout handle is its LANGID.
fn language_id(layout: isize) -> u16 {
    (layout as usize as u32 & 0xFFFF) as u16
}

/// The Latin (US) virtual key of a scan code (set 1), for a machine without a loaded en-US
/// layout.  Covers every key a bind can usefully name: letters, the number row and the
/// letter-row punctuation, Space and F1–F12.
fn us_virtual_key(scan: u8) -> Option<u8> {
    Some(match scan {
        0x02..=0x0A => b'1' + (scan - 0x02), // 1 … 9
        0x0B => 0x30,                        // 0
        0x0C => 0xBD,                        // -
        0x0D => 0xBB,                        // =
        0x10 => 0x51,                        // Q
        0x11 => 0x57,                        // W
        0x12 => 0x45,                        // E
        0x13 => 0x52,                        // R
        0x14 => 0x54,                        // T
        0x15 => 0x59,                        // Y
        0x16 => 0x55,                        // U
        0x17 => 0x49,                        // I
        0x18 => 0x4F,                        // O
        0x19 => 0x50,                        // P
        0x1A => 0xDB,                        // [
        0x1B => 0xDD,                        // ]
        0x1E => 0x41,                        // A
        0x1F => 0x53,                        // S
        0x20 => 0x44,                        // D
        0x21 => 0x46,                        // F
        0x22 => 0x47,                        // G
        0x23 => 0x48,                        // H
        0x24 => 0x4A,                        // J
        0x25 => 0x4B,                        // K
        0x26 => 0x4C,                        // L
        0x27 => 0xBA,                        // ;
        0x28 => 0xDE,                        // '
        0x29 => 0xC0,                        // `
        0x2B => 0xDC,                        // backslash
        0x2C => 0x5A,                        // Z
        0x2D => 0x58,                        // X
        0x2E => 0x43,                        // C
        0x2F => 0x56,                        // V
        0x30 => 0x42,                        // B
        0x31 => 0x4E,                        // N
        0x32 => 0x4D,                        // M
        0x33 => 0xBC,                        // ,
        0x34 => 0xBE,                        // .
        0x35 => 0xBF,                        // /
        0x39 => 0x20,                        // space
        0x3B..=0x44 => 0x70 + (scan - 0x3B), // F1 … F10
        0x57 => 0x7B,                        // F11
        0x58 => 0x7C,                        // F12
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_table_is_the_latin_us_keyboard() {
        // The physical keys niri's examples use.
        assert_eq!(us_virtual_key(0x14), Some(0x54)); // T
        assert_eq!(us_virtual_key(0x10), Some(0x51)); // Q
        assert_eq!(us_virtual_key(0x24), Some(0x4A)); // J
        assert_eq!(us_virtual_key(0x25), Some(0x4B)); // K
        assert_eq!(us_virtual_key(0x02), Some(0x31)); // 1
        assert_eq!(us_virtual_key(0x0B), Some(0x30)); // 0
        assert_eq!(us_virtual_key(0x1A), Some(0xDB)); // [
        assert_eq!(us_virtual_key(0x39), Some(0x20)); // space
        assert_eq!(us_virtual_key(0x3B), Some(0x70)); // F1
        assert_eq!(us_virtual_key(0x58), Some(0x7C)); // F12
                                                      // Nothing a bind can name.
        assert_eq!(us_virtual_key(0x00), None);
        assert_eq!(us_virtual_key(0x60), None);
    }

    #[test]
    fn the_language_id_comes_from_the_low_word_of_the_layout_handle() {
        // A layout handle's low word is its LANGID: 0x0409 English (US), 0x0419 Russian;
        // the bits above it are the layout's device handle.
        assert_eq!(language_id(0x0409_0409), 0x0409);
        assert_eq!(language_id(0x0419_0419), 0x0419);
        assert_eq!(language_id(0), 0);
    }
}
