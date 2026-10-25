//! Loaded keyboard layouts and an owned, event-driven change watcher.
//!
//! Queries deliberately do not cache: they work before `start()` and after its
//! guard is dropped. The watcher only wakes the IPC snapshot/diff machinery.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};

use serde_json::{json, Value};
use windows::core::{Error, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, WPARAM, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
    RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, GetKeyboardLayoutList, HKL};
use windows::Win32::UI::Shell::SHLoadIndirectString;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DeregisterShellHookWindow, DestroyWindow, DispatchMessageW,
    GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, PostMessageW, PostQuitMessage,
    PostThreadMessageW, RegisterClassW, RegisterShellHookWindow, RegisterWindowMessageW,
    TranslateMessage, UnregisterClassW, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE,
    WM_DESTROY, WM_SETTINGCHANGE, WNDCLASSW,
};

const MAX_LAYOUTS: usize = 256;
const LIST_ATTEMPTS: usize = 3;
const MAX_REGISTRY_LAYOUTS: u32 = 4096;
const STRING_CHARS: usize = 2048;
const LAYOUTS_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Keyboard Layouts";

/// Describe loaded layouts in OS enumeration order, using the foreground
/// window's thread (never this IPC thread) to select `current_idx`.
///
/// If enumeration races a switch, the foreground HKL is appended if necessary.
/// With no foreground window, index 0 is the numeric-schema fallback; an empty
/// enumeration is represented by `{"names":[],"current_idx":0}`.
pub fn keyboard_layouts() -> Value {
    let (layouts, current_idx) = order_and_index(loaded_layouts(), foreground_layout());
    let registry = RegistryKey::open();
    let special_ids = match registry.as_ref() {
        Some(key) if layouts.iter().any(|hkl| is_special(*hkl)) => special_layout_ids(key),
        _ => Vec::new(),
    };
    let names: Vec<String> = layouts
        .into_iter()
        .map(|hkl| layout_name(hkl, registry.as_ref(), &special_ids))
        .collect();
    json!({ "names": names, "current_idx": current_idx })
}

fn loaded_layouts() -> Vec<u32> {
    let mut layouts = Vec::new();
    // Both the allocation and retries are bounded even if layouts keep changing.
    for _ in 0..LIST_ATTEMPTS {
        let needed = unsafe { GetKeyboardLayoutList(None) };
        if needed <= 0 {
            break;
        }
        let capacity = (needed as usize).min(MAX_LAYOUTS);
        let mut buffer = vec![HKL::default(); capacity];
        let copied = unsafe { GetKeyboardLayoutList(Some(&mut buffer)) };
        if copied <= 0 {
            continue;
        }
        buffer.truncate((copied as usize).min(capacity));
        layouts = buffer.iter().map(|hkl| hkl.0 as usize as u32).collect();
        let latest_count = unsafe { GetKeyboardLayoutList(None) };
        if latest_count <= copied || capacity == MAX_LAYOUTS {
            break;
        }
    }
    layouts
}

fn foreground_layout() -> Option<u32> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    let thread_id = unsafe { GetWindowThreadProcessId(hwnd, None) };
    // Passing zero would silently ask for the caller's layout instead.
    if thread_id == 0 {
        return None;
    }
    let hkl = unsafe { GetKeyboardLayout(thread_id) }.0 as usize as u32;
    (hkl != 0).then_some(hkl)
}

fn order_and_index(layouts: Vec<u32>, current: Option<u32>) -> (Vec<u32>, usize) {
    let mut ordered = Vec::with_capacity(layouts.len() + 1);
    for hkl in layouts {
        if hkl != 0 && !ordered.contains(&hkl) {
            ordered.push(hkl);
        }
    }
    let index = match current.filter(|hkl| *hkl != 0) {
        Some(current) => match ordered.iter().position(|hkl| *hkl == current) {
            Some(index) => index,
            None => {
                let index = ordered.len();
                ordered.push(current);
                index
            }
        },
        None => 0,
    };
    (ordered, index)
}

struct RegistryKey(HKEY);

impl RegistryKey {
    fn open() -> Option<Self> {
        let path = wide(LAYOUTS_KEY);
        let mut key = HKEY::default();
        let status = unsafe {
            RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(path.as_ptr()), 0, KEY_READ, &mut key)
        };
        if status.is_ok() {
            Some(Self(key))
        } else {
            None
        }
    }
}

impl Drop for RegistryKey {
    fn drop(&mut self) {
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_text(buffer: &[u16]) -> Option<String> {
    let end = buffer.iter().position(|ch| *ch == 0).unwrap_or(buffer.len());
    String::from_utf16(&buffer[..end]).ok()
}

fn registry_string(key: &RegistryKey, subkey: &str, value: &str) -> Option<String> {
    let subkey = wide(subkey);
    let value = wide(value);
    let mut buffer = [0u16; STRING_CHARS];
    let mut bytes = std::mem::size_of_val(&buffer) as u32;
    let status = unsafe {
        RegGetValueW(
            key.0,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    let chars = bytes as usize / 2;
    if status.is_err() || bytes % 2 != 0 || chars > buffer.len() {
        return None;
    }
    wide_text(&buffer[..chars])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LayoutId {
    klid: u32,
    id: u16,
}

fn parse_klid(text: &str) -> Option<u32> {
    if text.len() != 8 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(text, 16).ok()
}

fn special_layout_ids(key: &RegistryKey) -> Vec<LayoutId> {
    let mut entries = Vec::new();
    for index in 0..MAX_REGISTRY_LAYOUTS {
        let mut buffer = [0u16; 32];
        let mut len = buffer.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
                key.0,
                index,
                PWSTR(buffer.as_mut_ptr()),
                &mut len,
                None,
                PWSTR::null(),
                None,
                None,
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status == ERROR_MORE_DATA {
            continue;
        }
        if status.is_err() || len as usize > buffer.len() {
            break;
        }
        let Some(name) = wide_text(&buffer[..len as usize]) else {
            continue;
        };
        let Some(klid) = parse_klid(&name) else {
            continue;
        };
        let Some(id) = registry_string(key, &name, "Layout Id")
            .and_then(|id| u16::from_str_radix(id.trim(), 16).ok())
        else {
            continue;
        };
        entries.push(LayoutId { klid, id });
    }
    entries
}

fn is_special(hkl: u32) -> bool {
    hkl & 0xf000_0000 == 0xf000_0000
}

fn registry_klid(hkl: u32, special_ids: &[LayoutId]) -> Option<u32> {
    let high = hkl >> 16;
    if is_special(hkl) {
        // Fxxx HKLs encode a registry "Layout Id", NOT a KLID or LANGID.
        let candidates: Vec<u32> = special_ids
            .iter()
            .filter(|entry| u32::from(entry.id) == high & 0x0fff)
            .map(|entry| entry.klid)
            .collect();
        let same_language: Vec<u32> = candidates
            .iter()
            .copied()
            .filter(|klid| klid & 0xffff == hkl & 0xffff)
            .collect();
        return match same_language.as_slice() {
            [klid] => Some(*klid),
            [] => match candidates.as_slice() {
                [klid] => Some(*klid),
                _ => None,
            },
            _ => None, // Ambiguous: do not mislabel a variant as the base layout.
        };
    }
    if high & 0xf000 == 0xe000 {
        // Legacy IME KLIDs retain their E-prefixed high word.
        Some(hkl)
    } else if high != 0 {
        // The high word identifies the physical layout; the low word can be a
        // different input language (e.g. US keys with UK English).
        Some(high)
    } else {
        (hkl != 0).then_some(hkl)
    }
}

fn fallback_name(hkl: u32) -> String {
    format!("Keyboard layout (HKL {hkl:08X})")
}

fn readable_name(text: &str) -> Option<&str> {
    let text = text.trim();
    (!text.is_empty() && !text.starts_with('@')).then_some(text)
}

fn select_name(
    hkl: u32,
    display: Option<&str>,
    text: Option<&str>,
    resolve: impl FnOnce(&str) -> Option<String>,
) -> String {
    if let Some(display) = display.map(str::trim).filter(|name| !name.is_empty()) {
        if let Some(name) = readable_name(display) {
            return name.to_owned();
        }
        if let Some(resolved) = resolve(display) {
            if let Some(name) = readable_name(&resolved) {
                return name.to_owned();
            }
        }
    }
    text.and_then(readable_name)
        .map(str::to_owned)
        .unwrap_or_else(|| fallback_name(hkl))
}

fn indirect_name(source: &str) -> Option<String> {
    let source = wide(source);
    let mut buffer = [0u16; STRING_CHARS];
    unsafe { SHLoadIndirectString(PCWSTR(source.as_ptr()), &mut buffer, None) }.ok()?;
    wide_text(&buffer)
}

fn layout_name(hkl: u32, registry: Option<&RegistryKey>, special_ids: &[LayoutId]) -> String {
    let Some((key, klid)) = registry.and_then(|key| registry_klid(hkl, special_ids).map(|id| (key, id)))
    else {
        return fallback_name(hkl);
    };
    let subkey = format!("{klid:08X}");
    let display = registry_string(key, &subkey, "Layout Display Name");
    let text = registry_string(key, &subkey, "Layout Text");
    select_name(hkl, display.as_deref(), text.as_deref(), indirect_name)
}

// Win32 event codes and private watcher messages.
const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const HSHELL_LANGUAGE: usize = 8;
const WINEVENT_OUTOFCONTEXT: u32 = 0;
const WM_LAYOUT_CHANGED: u32 = WM_APP + 0x351;
const WM_STOP_WATCHER: u32 = WM_APP + 0x352;

thread_local! {
    // Out-of-context WinEvents run on the installing (pumping) thread, so no
    // process-global HWND/hook routing or cross-thread raw pointers are needed.
    static WATCHER_WINDOW: Cell<isize> = const { Cell::new(0) };
    static SHELL_MESSAGE: Cell<u32> = const { Cell::new(0) };
}

/// Owns the notification window, hook and pumping thread. Retain this guard for
/// as long as IPC subscriptions should receive layout/foreground changes.
#[must_use = "dropping the guard stops and joins the keyboard watcher"]
pub struct KeyboardWatcher {
    thread_id: u32,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Start a hidden top-level window and its own message pump. No polling timers,
/// keyboard/input hooks, or Tokio runtime are needed.
pub fn start() -> Result<KeyboardWatcher, String> {
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<u32, String>>(1);
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let thread = thread::Builder::new()
        .name("wiri-ipc-keyboard".to_owned())
        .spawn(move || match WatcherThread::create() {
            Ok(watcher) => {
                let thread_id = unsafe { GetCurrentThreadId() };
                if ready_tx.send(Ok(thread_id)).is_ok() {
                    crate::ipc::notify_state_changed();
                    watcher.run(&thread_stop);
                }
            }
            Err(error) => {
                let _ = ready_tx.send(Err(error));
            }
        })
        .map_err(|error| format!("Starting keyboard watcher thread: {error}"))?;

    match ready_rx.recv() {
        Ok(Ok(thread_id)) => Ok(KeyboardWatcher {
            thread_id,
            stop,
            thread: Some(thread),
        }),
        result => {
            let _ = thread.join();
            Err(match result {
                Ok(Err(error)) => error,
                _ => "Keyboard watcher exited before reporting readiness".to_owned(),
            })
        }
    }
}

impl Drop for KeyboardWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            if !thread.is_finished() {
                // The queue exists before readiness is sent. If it is full,
                // the stop flag still terminates the next pump iteration.
                let _ = unsafe {
                    PostThreadMessageW(self.thread_id, WM_STOP_WATCHER, WPARAM(0), LPARAM(0))
                };
            }
            let _ = thread.join();
        }
    }
}

struct WatcherThread {
    instance: HINSTANCE,
    class_name: Vec<u16>,
    hwnd: HWND,
    hook: HWINEVENTHOOK,
    shell_registered: bool,
}

impl WatcherThread {
    fn create() -> Result<Self, String> {
        let shell_name = wide("SHELLHOOK");
        let shell_message = unsafe { RegisterWindowMessageW(PCWSTR(shell_name.as_ptr())) };
        if shell_message == 0 {
            return Err(format!("RegisterWindowMessageW(SHELLHOOK): {}", Error::from_win32()));
        }
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }
            .map_err(|error| format!("GetModuleHandleW: {error}"))?
            .into();
        let class_name = wide(&format!("WiriIpcKeyboard-{}", unsafe { GetCurrentThreadId() }));
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err(format!("RegisterClassW(keyboard): {}", Error::from_win32()));
        }
        // From here every partial startup failure runs the same cleanup path.
        let mut owner = Self {
            instance,
            class_name,
            hwnd: HWND::default(),
            hook: HWINEVENTHOOK::default(),
            shell_registered: false,
        };
        SHELL_MESSAGE.with(|message| message.set(shell_message));
        owner.hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(owner.class_name.as_ptr()),
                PCWSTR(owner.class_name.as_ptr()),
                WINDOW_STYLE(0), // No WS_VISIBLE; parent=None means top-level, NOT HWND_MESSAGE.
                0,
                0,
                0,
                0,
                None,
                None,
                instance,
                None,
            )
        }
        .map_err(|error| format!("CreateWindowExW(keyboard): {error}"))?;
        WATCHER_WINDOW.with(|window| window.set(owner.hwnd.0 as isize));
        if !unsafe { RegisterShellHookWindow(owner.hwnd) }.as_bool() {
            return Err(format!("RegisterShellHookWindow: {}", Error::from_win32()));
        }
        owner.shell_registered = true;
        // Install on THIS pumping thread, not the IPC caller. Do not set
        // WINEVENT_SKIPOWNPROCESS: foreground changes to our own UI matter too.
        owner.hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                HMODULE::default(),
                Some(foreground_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if owner.hook.is_invalid() {
            return Err(format!("SetWinEventHook(foreground): {}", Error::from_win32()));
        }
        Ok(owner)
    }

    fn run(&self, stop: &AtomicBool) {
        let mut message = MSG::default();
        while !stop.load(Ordering::Acquire) {
            let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
            if result.0 == -1 {
                tracing::warn!("Keyboard watcher GetMessageW failed: {}", Error::from_win32());
                break;
            }
            if result.0 == 0 || message.message == WM_STOP_WATCHER {
                break;
            }
            let _ = unsafe { TranslateMessage(&message) };
            let _ = unsafe { DispatchMessageW(&message) };
        }
    }
}

impl Drop for WatcherThread {
    fn drop(&mut self) {
        WATCHER_WINDOW.with(|window| window.set(0));
        if !self.hook.is_invalid() {
            let _ = unsafe { UnhookWinEvent(self.hook) };
        }
        if self.shell_registered {
            let _ = unsafe { DeregisterShellHookWindow(self.hwnd) };
        }
        if !self.hwnd.0.is_null() {
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
        let _ = unsafe { UnregisterClassW(PCWSTR(self.class_name.as_ptr()), self.instance) };
        SHELL_MESSAGE.with(|message| message.set(0));
    }
}

unsafe extern "system" fn foreground_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    _hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    if event == EVENT_SYSTEM_FOREGROUND {
        WATCHER_WINDOW.with(|window| {
            let raw = window.get();
            if raw != 0 {
                // Never run IPC work re-entrantly inside a WinEvent callback.
                let _ = unsafe {
                    PostMessageW(HWND(raw as *mut _), WM_LAYOUT_CHANGED, WPARAM(0), LPARAM(0))
                };
            }
        });
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let language_changed = SHELL_MESSAGE.with(|shell| {
        shell.get() != 0 && message == shell.get() && wparam.0 == HSHELL_LANGUAGE
    });
    if language_changed || message == WM_LAYOUT_CHANGED || message == WM_SETTINGCHANGE {
        // There is no cache to invalidate; the parent resamples and diffs.
        crate::ipc::notify_state_changed();
        return LRESULT(0);
    }
    if message == WM_CLOSE || message == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localized_display_name_precedes_layout_text() {
        let name = select_name(0x0409_0409, Some("@input.dll,-1"), Some("US"), |source| {
            assert_eq!(source, "@input.dll,-1");
            Some("  Localized US  ".to_owned())
        });
        assert_eq!(name, "Localized US");
        assert_eq!(
            select_name(1, Some("Literal display"), Some("Text"), |_| panic!("not indirect")),
            "Literal display"
        );
    }

    #[test]
    fn unresolved_or_empty_display_uses_text_then_an_honest_fallback() {
        for resolved in [None, Some(String::new()), Some("@unresolved,-1".to_owned())] {
            assert_eq!(
                select_name(0xf002_0409, Some("@missing,-1"), Some(" Dvorak "), |_| resolved),
                "Dvorak"
            );
        }
        assert_eq!(select_name(1, Some("  "), Some("Text"), |_| None), "Text");
        assert_eq!(
            select_name(0xf002_0409, None, Some("@not-a-name"), |_| None),
            "Keyboard layout (HKL F0020409)"
        );
    }

    #[test]
    fn selection_uses_exact_hkl_not_just_the_language() {
        let layouts = vec![0x0409_0409, 0xf002_0409, 0xf001_0409];
        assert_eq!(order_and_index(layouts.clone(), Some(0xf001_0409)), (layouts, 2));
        assert_eq!(order_and_index(vec![1, 2], Some(3)), (vec![1, 2, 3], 2));
        assert_eq!(order_and_index(vec![0, 2, 2, 1], None), (vec![2, 1], 0));
        assert_eq!(order_and_index(vec![], Some(0)), (vec![], 0));
    }

    #[test]
    fn special_layout_ids_resolve_variants_without_guessing_base_layouts() {
        let ids = [
            LayoutId { klid: 0x0001_0409, id: 2 },
            LayoutId { klid: 0x0002_0409, id: 1 },
            LayoutId { klid: 0x0001_0405, id: 1 },
        ];
        assert_eq!(registry_klid(0xf002_0409, &ids), Some(0x0001_0409));
        assert_eq!(registry_klid(0xf001_0409, &ids), Some(0x0002_0409));
        assert_eq!(registry_klid(0xf001_0405, &ids), Some(0x0001_0405));
        assert_eq!(registry_klid(0xf001_0419, &ids), None); // Ambiguous.
        assert_eq!(registry_klid(0xf003_0409, &ids), None); // Unknown, not US.
        assert_eq!(registry_klid(0xf002_0809, &ids), Some(0x0001_0409));
    }

    #[test]
    fn normal_cross_language_and_ime_handles_have_distinct_registry_rules() {
        assert_eq!(registry_klid(0x0409_0409, &[]), Some(0x0409));
        assert_eq!(registry_klid(0x0409_0809, &[]), Some(0x0409));
        assert_eq!(registry_klid(0x0000_0409, &[]), Some(0x0409));
        assert_eq!(registry_klid(0xe001_0411, &[]), Some(0xe001_0411));
        assert_eq!(registry_klid(0, &[]), None);
    }

    #[test]
    fn registry_key_parsing_and_utf16_are_bounded_and_strict() {
        assert_eq!(parse_klid("00010409"), Some(0x0001_0409));
        assert_eq!(parse_klid("e0010411"), Some(0xe001_0411));
        for invalid in ["409", "000004090", "00000Z09", "-0000409", " 0000409"] {
            assert_eq!(parse_klid(invalid), None);
        }
        assert_eq!(wide_text(&[65, 0, 66]), Some("A".to_owned()));
        assert_eq!(wide_text(&[65, 66]), Some("AB".to_owned()));
        assert_eq!(wide_text(&[0xd800]), None);
    }
}
