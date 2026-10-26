//! Frozen-output screenshot UI. One on-demand HWND/message thread, destroyed on
//! save/copy/cancel. No timer, animation clock, resident worker, or shell snipper.
use super::screenshot_selection::{crop, Region, Selection};
use anyhow::{Context, Result};
use parking_lot::Mutex;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;

const KEY: u32 = WM_APP + 146;
static OPEN: AtomicBool = AtomicBool::new(false);
static HOST: AtomicIsize = AtomicIsize::new(0);
static LAST: Mutex<Option<([i32; 4], Region)>> = Mutex::new(None);
// Preserve captured releases even when Enter/Escape already destroyed the host.
// Physical and injected edges cannot retire one another's pair.
static KEYS: Mutex<[[bool; 256]; 2]> = Mutex::new([[false; 256]; 2]);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Options {
    pub show_pointer: bool,
    pub path: Option<String>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            show_pointer: true,
            path: None,
        }
    }
}
impl Options {
    pub fn parse(words: &[String]) -> Option<Self> {
        let mut out = Self::default();
        let mut pointer_seen = false;
        let mut i = 0;
        while i < words.len() {
            let word = &words[i];
            let (key, inline) = word
                .split_once('=')
                .map_or((word.as_str(), None), |(k, v)| (k, Some(v)));
            match key {
                "--show-pointer" | "-p" | "show-pointer" => {
                    if pointer_seen {
                        return None;
                    }
                    pointer_seen = true;
                    i += 1;
                    let value = inline.or_else(|| words.get(i).map(String::as_str))?;
                    out.show_pointer = match value {
                        "true" => true,
                        "false" => false,
                        _ => return None,
                    };
                    if inline.is_some() {
                        i -= 1;
                    }
                }
                "--path" | "path" => {
                    if out.path.is_some() {
                        return None;
                    }
                    i += 1;
                    out.path = Some(
                        inline
                            .or_else(|| words.get(i).map(String::as_str))?
                            .to_owned(),
                    );
                    if inline.is_some() {
                        i -= 1;
                    }
                }
                _ if !word.starts_with('-') && out.path.is_none() => out.path = Some(word.clone()),
                _ => return None,
            }
            i += 1;
        }
        if let Some(path) = &out.path {
            if !Path::new(path).is_absolute() || path.contains('\0') {
                return None;
            }
        }
        Some(out)
    }
}

pub fn is_active() -> bool {
    OPEN.load(Ordering::Acquire)
}

/// None leaves ordinary dispatch alone. Some(false) advances modal physical
/// state but passes modifiers; Some(true) consumes a paired modal key.
pub fn key_event(vk: u32, pressed: bool, injected: bool, modifiers: u32) -> Option<bool> {
    let hwnd = HOST.load(Ordering::Acquire);
    let slot = vk as usize;
    if slot >= 256 {
        return None;
    }
    let mut keys = KEYS.lock();
    let pair = &mut keys[usize::from(injected)][slot];
    let captured = *pair;
    if !pressed {
        *pair = false;
    }
    if hwnd == 0 {
        return captured.then_some(true);
    }
    if crate::input::keyboard::system_critical(modifiers, vk) {
        return None;
    }
    if crate::input::keyboard::modifier(vk) != 0 {
        return Some(false);
    }
    if pressed {
        *pair = true;
    }
    if pressed || captured {
        let flags = usize::from(pressed)
            | ((modifiers as usize) << 1)
            | (usize::from(captured && pressed) << 8);
        let _ = unsafe {
            PostMessageW(
                HWND(hwnd as *mut _),
                KEY,
                WPARAM(vk as usize),
                LPARAM(flags as isize),
            )
        };
    }
    Some(pressed || captured)
}

pub fn cancel() {
    let hwnd = HOST.load(Ordering::Acquire);
    if hwnd != 0 {
        let _ = unsafe { PostMessageW(HWND(hwnd as *mut _), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    }
}

struct SessionGuard;
impl Drop for SessionGuard {
    fn drop(&mut self) {
        HOST.store(0, Ordering::Release);
        OPEN.store(false, Ordering::Release);
    }
}

pub fn open(options: Options) -> Result<()> {
    if OPEN
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok(());
    }
    let guard = SessionGuard;
    // Freeze before any UI HWND exists. Saving never recaptures the changing desktop.
    let (bounds, clean, pointed) = crate::screenshot::interactive::freeze()?;
    let previous = unsafe { GetForegroundWindow().0 as isize };
    std::thread::Builder::new()
        .name("wiri-screenshot-ui".into())
        .spawn(move || {
            let _guard = guard;
            if let Err(error) = run(bounds, clean, pointed, previous, options) {
                tracing::warn!("Screenshot UI: {error:#}");
            }
        })
        .context("starting screenshot UI")?;
    Ok(())
}

struct State {
    clean: crate::screenshot::Image,
    pointed: crate::screenshot::Image,
    selection: Selection,
    bounds: [i32; 4],
    show_pointer: bool,
    toolbar_down: bool,
    outcome: Option<bool>,
}
impl State {
    fn save_button(&self) -> RECT {
        let w = self.clean.width as i32;
        let h = self.clean.height as i32;
        let left = ((w - 680) / 2).max(8);
        RECT {
            left,
            top: (h - 78).max(0),
            right: (left + 78).min(w),
            bottom: (h - 16).max(1),
        }
    }
}
fn inside(rect: RECT, point: (i32, i32)) -> bool {
    point.0 >= rect.left && point.0 < rect.right && point.1 >= rect.top && point.1 < rect.bottom
}
fn point(lp: LPARAM) -> (i32, i32) {
    (
        (lp.0 as u16 as i16) as i32,
        ((lp.0 >> 16) as u16 as i16) as i32,
    )
}

fn run(
    bounds: RECT,
    clean: crate::screenshot::Image,
    pointed: crate::screenshot::Image,
    previous: isize,
    options: Options,
) -> Result<()> {
    let key = [bounds.left, bounds.top, bounds.right, bounds.bottom];
    let remembered = LAST.lock().filter(|(b, _)| *b == key).map(|(_, r)| r);
    let mut state = Box::new(State {
        selection: Selection::new(clean.width as i32, clean.height as i32, remembered),
        bounds: key,
        clean,
        pointed,
        show_pointer: options.show_pointer,
        toolbar_down: false,
        outcome: None,
    });
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: w!("WiriScreenshotUI"),
            hCursor: LoadCursorW(None, IDC_CROSS)?,
            ..Default::default()
        };
        // A process-wide class is tiny and holds no HWND/GDI resources. Reopening
        // uses the same procedure; a failed creation below still releases the frame.
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("WiriScreenshotUI"),
            w!("wiri screenshot"),
            WS_POPUP,
            bounds.left,
            bounds.top,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            None,
            None,
            instance,
            Some((&mut *state as *mut State).cast()),
        )?;
        HOST.store(hwnd.0 as isize, Ordering::Release);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = InvalidateRect(hwnd, None, false);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if IsWindow(hwnd).as_bool() {
            let _ = DestroyWindow(hwnd);
        }
        if GetForegroundWindow() == hwnd
            && previous != 0
            && IsWindow(HWND(previous as *mut _)).as_bool()
        {
            let _ = SetForegroundWindow(HWND(previous as *mut _));
        }
    }
    if let Some(write_to_disk) = state.outcome {
        let source = if state.show_pointer {
            &state.pointed
        } else {
            &state.clean
        };
        let image = crop(source, state.selection.region())?;
        crate::screenshot::save_image(
            &image,
            write_to_disk,
            options.path.as_deref().map(Path::new),
        )?;
    }
    Ok(())
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lp.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let state = &mut *ptr;
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_MOUSEACTIVATE => {
            let _ = SetForegroundWindow(hwnd);
            return LRESULT(MA_NOACTIVATE as isize);
        }
        WM_PAINT => {
            paint(hwnd, state);
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            SetCapture(hwnd);
            let pos = point(lp);
            state.toolbar_down = inside(state.save_button(), pos);
            if !state.toolbar_down {
                state.selection.begin(pos);
            }
        }
        WM_MOUSEMOVE => {
            if state.toolbar_down || !state.selection.dragging {
                return LRESULT(0);
            }
            state.selection.motion(point(lp));
        }
        WM_LBUTTONUP => {
            let _ = ReleaseCapture();
            if state.toolbar_down {
                state.toolbar_down = false;
                if inside(state.save_button(), point(lp)) {
                    state.outcome = Some(true);
                    let _ = DestroyWindow(hwnd);
                }
            } else {
                state.selection.motion(point(lp));
                state.selection.end();
            }
        }
        WM_CAPTURECHANGED => state.selection.dragging = false,
        KEY => {
            let down = lp.0 & 1 != 0;
            let mods = (lp.0 as u32 >> 1) & 15;
            let repeat = lp.0 & 256 != 0;
            let vk = wp.0 as u32;
            if vk == 0x20 {
                state.selection.space(down);
            }
            if down {
                if vk == 0x1B {
                    let _ = DestroyWindow(hwnd);
                } else if mods & 5 == 0 {
                    match vk {
                        0x20 if mods & 2 == 0 && !state.selection.dragging => {
                            state.outcome = Some(true);
                            let _ = DestroyWindow(hwnd);
                        }
                        0x0D if mods & 2 == 0 => {
                            state.outcome = Some(true);
                            let _ = DestroyWindow(hwnd);
                        }
                        0x43 if mods & 2 != 0 => {
                            state.outcome = Some(false);
                            let _ = DestroyWindow(hwnd);
                        }
                        0x50 if mods & 2 == 0 && !repeat => {
                            state.show_pointer = !state.show_pointer
                        }
                        0x25 => state.selection.translate(-50, 0),
                        0x27 => state.selection.translate(50, 0),
                        0x26 => state.selection.translate(0, -50),
                        0x28 => state.selection.translate(0, 50),
                        _ => {}
                    }
                } else if mods == 4 {
                    match vk {
                        0x25 => state.selection.resize(-10, 0),
                        0x27 => state.selection.resize(10, 0),
                        0x26 => state.selection.resize(0, -10),
                        0x28 => state.selection.resize(0, 10),
                        _ => {}
                    }
                }
            }
        }
        WM_CLOSE | WM_DISPLAYCHANGE | WM_DPICHANGED => {
            let _ = DestroyWindow(hwnd);
        }
        WM_DESTROY => {
            *LAST.lock() = Some((state.bounds, state.selection.region()));
            HOST.store(0, Ordering::Release);
            PostQuitMessage(0);
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    let _ = InvalidateRect(hwnd, None, false);
    LRESULT(0)
}

unsafe fn paint(hwnd: HWND, state: &State) {
    let mut ps = PAINTSTRUCT::default();
    let dc = BeginPaint(hwnd, &mut ps);
    let image = if state.show_pointer {
        &state.pointed
    } else {
        &state.clean
    };
    let r = state.selection.region();
    let mut pixels = image.pixels.clone();
    for y in 0..image.height as i32 {
        for x in 0..image.width as i32 {
            if x < r.x || x >= r.x + r.width || y < r.y || y >= r.y + r.height {
                let pos = (y as usize * image.width as usize + x as usize) * 4;
                for c in &mut pixels[pos..pos + 3] {
                    *c /= 2;
                }
            }
        }
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
    StretchDIBits(
        dc,
        0,
        0,
        image.width as i32,
        image.height as i32,
        0,
        0,
        image.width as i32,
        image.height as i32,
        Some(pixels.as_ptr().cast()),
        &info,
        DIB_RGB_COLORS,
        SRCCOPY,
    );
    let border = RECT {
        left: r.x - 1,
        top: r.y - 1,
        right: r.x + r.width + 1,
        bottom: r.y + r.height + 1,
    };
    FrameRect(dc, &border, HBRUSH(GetStockObject(WHITE_BRUSH).0));
    let button = state.save_button();
    let panel = RECT {
        left: button.left,
        top: button.top,
        right: (button.left + 680).min(image.width as i32),
        bottom: button.bottom,
    };
    FillRect(dc, &panel, HBRUSH(GetStockObject(BLACK_BRUSH).0));
    FrameRect(dc, &button, HBRUSH(GetStockObject(WHITE_BRUSH).0));
    let _ = SetTextColor(dc, windows::Win32::Foundation::COLORREF(0x00ffffff));
    SetBkMode(dc, TRANSPARENT);
    let mut save = " Save ".encode_utf16().collect::<Vec<_>>();
    let mut br = button;
    DrawTextW(
        dc,
        &mut save,
        &mut br,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );
    let text=format!("Enter/Space: save    Ctrl+C: copy    Esc: cancel\nP: pointer {}   Drag: select   Space-drag: move   {} x {}",
        if state.show_pointer {"on"} else {"off"},r.width,r.height);
    let mut text = text.encode_utf16().collect::<Vec<_>>();
    let mut tr = RECT {
        left: button.right + 10,
        top: button.top + 10,
        right: panel.right,
        bottom: panel.bottom,
    };
    DrawTextW(dc, &mut text, &mut tr, DT_LEFT | DT_NOPREFIX);
    let _ = EndPaint(hwnd, &ps);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn words(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn ui_options_have_niri_defaults_and_absolute_paths() {
        assert_eq!(Options::parse(&[]), Some(Options::default()));
        assert_eq!(
            Options::parse(&words(&[
                "--show-pointer=false",
                "--path",
                r"C:\shots\s.png"
            ])),
            Some(Options {
                show_pointer: false,
                path: Some(r"C:\shots\s.png".into())
            })
        );
        for bad in [
            &["--show-pointer", "yes"][..],
            &["relative.png"],
            &["--write-to-disk", "false"],
            &["--path"],
        ] {
            assert!(Options::parse(&words(bad)).is_none());
        }
    }
    #[test]
    fn capture_button_hit_test_has_exclusive_right_bottom_edges() {
        let r = RECT {
            left: 1,
            top: 2,
            right: 10,
            bottom: 20,
        };
        assert!(inside(r, (1, 2)));
        assert!(!inside(r, (10, 2)));
        assert!(!inside(r, (2, 20)));
    }
}
