use anyhow::Result;
use std::sync::Arc;
use tracing::{error, info, warn};
use windows::Win32::Foundation::{HWND, WPARAM, LPARAM, LRESULT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetMessageW, PostQuitMessage, PostThreadMessageW, RegisterClassW, TranslateMessage,
    WM_DESTROY, WM_USER, WM_DISPLAYCHANGE, WNDCLASSW, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASS_STYLES,
    ShowWindow, SetWindowPos, GetForegroundWindow, SW_MAXIMIZE, SW_MINIMIZE,
    HWND_TOP, SWP_NOZORDER, SWP_NOACTIVATE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, GetMonitorInfoW, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use parking_lot::Mutex;
use std::collections::HashMap;

use super::BackendHandle;
use crate::input::Action;
use crate::layout::{TilingEngine, ScrollDirection};

const WM_STOP_MESSAGE_LOOP: u32 = WM_USER + 100;
/// Custom message sent to the hotkey thread to trigger re-registration
const WM_RELOAD_HOTKEYS: u32 = WM_USER + 102;

// Do NOT use MOD_NOREPEAT (0x4000) — it causes RegisterHotKey to fail on
// ARM64 Windows. Key repeat is debounced in software.
const MOD_ALT: u32 = 0x0001;
const MOD_CTRL: u32 = 0x0002;
const MOD_SHIFT: u32 = 0x0004;
const MOD_WIN: u32 = 0x0008;

/// Resolve a `mod-key` config string to a (prefix, prefix+shift) pair of Win32 modifier flags.
fn resolve_mod_prefix(name: &str) -> (u32, u32) {
    let prefix = match name.trim().to_lowercase().as_str() {
        "alt" => MOD_ALT,
        "ctrl" | "control" => MOD_CTRL,
        "super" | "win" | "meta" => MOD_WIN,
        "win-alt" | "super-alt" | "meta-alt" => MOD_WIN | MOD_ALT,
        "ctrl-shift" => MOD_CTRL | MOD_SHIFT,
        _ => MOD_CTRL | MOD_ALT, // "ctrl-alt" or anything unrecognized
    };
    (prefix, prefix | MOD_SHIFT)
}

/// Default hotkey bindings using the configured modifier prefix. The Mod+Enter
/// terminal-spawn binding is parameterised by `terminal_cmd` so callers can
/// pick up the user's preferred shell instead of always launching cmd.exe.
fn default_hotkeys(prefix: u32, shift_prefix: u32, terminal_cmd: &str) -> Vec<(u32, u32, Action)> {
    vec![
        (prefix, 0x25, Action::FocusColumnLeft),       // Left
        (prefix, 0x27, Action::FocusColumnRight),      // Right
        (prefix, 0x26, Action::FocusUp),               // Up
        (prefix, 0x28, Action::FocusDown),             // Down
        (shift_prefix, 0x25, Action::MoveColumnLeft),
        (shift_prefix, 0x27, Action::MoveColumnRight),
        (prefix, 0x51, Action::CloseWindow),           // Q
        (prefix, 0x0D, Action::Spawn(terminal_cmd.to_string())), // Enter
        (prefix, 0x46, Action::ToggleFullscreen),      // F
        (prefix, 0x54, Action::ToggleFloating),        // T
        (prefix, 0x48, Action::ScrollLeft),            // H
        (prefix, 0x4C, Action::ScrollRight),           // L
        (shift_prefix, 0x51, Action::Quit),            // Shift+Q
        (prefix, 0x20, Action::OverviewToggle),        // Space
        (prefix, 0x4F, Action::OverviewSelect),        // O
        (prefix, 0xDC, Action::ColumnToggleTabbed),    // \
        (prefix, 0xDD, Action::TabNext),               // ]
        (prefix, 0xDB, Action::TabPrev),               // [
        (prefix, 0x31, Action::FocusWorkspace(1)),     // 1
        (prefix, 0x32, Action::FocusWorkspace(2)),
        (prefix, 0x33, Action::FocusWorkspace(3)),
        (prefix, 0x34, Action::FocusWorkspace(4)),
        (prefix, 0x35, Action::FocusWorkspace(5)),
        (prefix, 0x36, Action::FocusWorkspace(6)),
        (prefix, 0x37, Action::FocusWorkspace(7)),
        (prefix, 0x38, Action::FocusWorkspace(8)),
        (prefix, 0x39, Action::FocusWorkspace(9)),
        // Niri-style layout commands
        (shift_prefix, 0x31, Action::MoveToWorkspace(1)), // Shift+1..9 → MoveToWorkspace
        (shift_prefix, 0x32, Action::MoveToWorkspace(2)),
        (shift_prefix, 0x33, Action::MoveToWorkspace(3)),
        (shift_prefix, 0x34, Action::MoveToWorkspace(4)),
        (shift_prefix, 0x35, Action::MoveToWorkspace(5)),
        (shift_prefix, 0x36, Action::MoveToWorkspace(6)),
        (shift_prefix, 0x37, Action::MoveToWorkspace(7)),
        (shift_prefix, 0x38, Action::MoveToWorkspace(8)),
        (shift_prefix, 0x39, Action::MoveToWorkspace(9)),
        (prefix, 0x21, Action::FocusWorkspacePrevious),   // PageUp
        (prefix, 0x22, Action::FocusWorkspaceNext),       // PageDown
        (prefix, 0x52, Action::CenterColumn),             // R
        (prefix, 0x57, Action::ColumnWidthPresetCycle),   // W
        (prefix, 0xBD, Action::ResizeColumnLeft),         // VK_OEM_MINUS
        (prefix, 0xBB, Action::ResizeColumnRight),        // VK_OEM_PLUS
        (shift_prefix, 0x48, Action::MoveToMonitorLeft),  // Shift+H
        (shift_prefix, 0x4C, Action::MoveToMonitorRight), // Shift+L
        // Niri-parity round-2: FocusPrevious (alt-tab MRU) and ToggleAlwaysOnTop
        // NOTE: if prefix is Ctrl+Alt, prefix+Tab conflicts with the Windows system
        // Ctrl+Alt+Tab switcher. RegisterHotKey will fail; that's OK — the warning
        // will be logged and the action remains available via custom KDL binds.
        (prefix, 0x09, Action::FocusPrevious),            // Tab
        (prefix, 0x50, Action::ToggleAlwaysOnTop),        // P
    ]
}

/// Build hotkey list from config binds, falling back to defaults.
fn build_hotkey_list(engine: &Arc<parking_lot::RwLock<TilingEngine>>) -> Vec<(u32, u32, Action)> {
    let terminal_cmd = resolve_terminal_command(engine);
    let eng = engine.read();
    let config_binds = &eng.config_binds().hotkeys;
    let (prefix, shift_prefix) = eng
        .full_config()
        .map(|c| resolve_mod_prefix(&c.input.mod_key))
        .unwrap_or((MOD_CTRL | MOD_ALT, MOD_CTRL | MOD_ALT | MOD_SHIFT));

    if config_binds.is_empty() {
        return default_hotkeys(prefix, shift_prefix, &terminal_cmd);
    }

    let mut hotkeys = Vec::new();
    for bind in config_binds {
        let mods = bind.mod_flags();
        let vk = match bind.vk_code() {
            Some(v) => v,
            None => {
                warn!("Config bind: unknown key '{}', skipping", bind.key);
                continue;
            }
        };
        let action = match bind.parse_action() {
            Some(a) => a,
            None => {
                warn!("Config bind: unknown action '{}', skipping", bind.command);
                continue;
            }
        };
        hotkeys.push((mods, vk, action));
    }

    if hotkeys.is_empty() {
        warn!("Config had binds section but none parsed successfully, using defaults");
        return default_hotkeys(prefix, shift_prefix, &terminal_cmd);
    }

    info!("Using {} hotkeys from config", hotkeys.len());
    hotkeys
}

/// Register a list of hotkeys with Windows, returning the IDs and action map
fn register_hotkeys(
    hwnd: HWND,
    hotkeys: &[(u32, u32, Action)],
) -> (Vec<i32>, HashMap<i32, Action>) {
    let base_id = 32768i32;
    let mut registered_ids: Vec<i32> = Vec::new();

    for (i, (mods, vk, action)) in hotkeys.iter().enumerate() {
        let id = base_id + i as i32;
        let result = unsafe {
            RegisterHotKey(hwnd, id, HOT_KEY_MODIFIERS(*mods), *vk)
        };
        if result.is_ok() {
            registered_ids.push(id);
            info!("OK hotkey id={} mods=0x{:X} vk=0x{:02X} {:?}", id, mods, vk, action);
        } else {
            let err = std::io::Error::last_os_error();
            warn!("FAIL hotkey id={} mods=0x{:X} vk=0x{:02X} {:?} os_err={}", id, mods, vk, action, err);
        }
    }
    info!("Hotkeys: {}/{} registered", registered_ids.len(), hotkeys.len());

    let action_map: HashMap<i32, Action> = hotkeys
        .iter()
        .enumerate()
        .filter(|(i, _)| registered_ids.contains(&(base_id + *i as i32)))
        .map(|(i, (_, _, action))| (base_id + i as i32, action.clone()))
        .collect();

    (registered_ids, action_map)
}

/// Unregister all hotkeys by ID
fn unregister_hotkeys(hwnd: HWND, ids: &[i32]) {
    for id in ids {
        unsafe { let _ = UnregisterHotKey(hwnd, *id); }
    }
}

/// Wrapper to make HWND Send-safe
#[allow(dead_code)]
struct SendHwnd(HWND);
unsafe impl Send for SendHwnd {}

/// Shared state between the main thread and the hotkey thread
struct HotkeyThreadState {
    thread_id: u32,
    #[allow(dead_code)]
    hwnd: Option<SendHwnd>,
}

static HOTKEY_THREAD: Mutex<Option<HotkeyThreadState>> = Mutex::new(None);

pub struct MessageLoop {
    running: Arc<Mutex<bool>>,
}

impl MessageLoop {
    pub fn new() -> Result<Self> {
        let running = Arc::new(Mutex::new(false));
        Ok(Self { running })
    }

    pub fn start_hotkey_loop(
        &self,
        engine: Arc<parking_lot::RwLock<TilingEngine>>,
        backend_handle: BackendHandle,
    ) {
        let running = self.running.clone();
        std::thread::spawn(move || {
            Self::run_message_loop(running, engine, backend_handle);
        });
    }

    fn run_message_loop(
        running: Arc<Mutex<bool>>,
        engine: Arc<parking_lot::RwLock<TilingEngine>>,
        backend_handle: BackendHandle,
    ) {
        let thread_id = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };

        // Create a hidden window to own the hotkey registrations
        let class_name: Vec<u16> = "WiriHotkeyWnd\0".encode_utf16().collect();
        let wnd_class = WNDCLASSW {
            style: WNDCLASS_STYLES(0),
            lpfnWndProc: Some(Self::window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: unsafe { GetModuleHandleW(None) }.ok().map(|h| h.into()).unwrap_or_default(),
            hIcon: windows::Win32::UI::WindowsAndMessaging::HICON::default(),
            hCursor: windows::Win32::UI::WindowsAndMessaging::HCURSOR::default(),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH::default(),
            lpszMenuName: windows::core::PCWSTR::null(),
            lpszClassName: windows::core::PCWSTR(class_name.as_ptr()),
        };
        unsafe { let _ = RegisterClassW(&wnd_class); }

        let window_name: Vec<u16> = "WiriHotkeyWindow\0".encode_utf16().collect();
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                windows::core::PCWSTR(class_name.as_ptr()),
                windows::core::PCWSTR(window_name.as_ptr()),
                WINDOW_STYLE(0),
                0, 0, 0, 0,
                None, None,
                windows::Win32::Foundation::HMODULE::default(),
                None,
            )
        };
        let hwnd = match hwnd {
            Ok(w) => w,
            Err(e) => {
                error!("Failed to create hotkey window: {:?}", e);
                return;
            }
        };
        info!("Hotkey window created");

        // Store thread state for hot-reload support
        *HOTKEY_THREAD.lock() = Some(HotkeyThreadState {
            thread_id,
            hwnd: Some(SendHwnd(hwnd)),
        });

        // Register initial hotkeys
        let hotkeys = build_hotkey_list(&engine);
        let (mut registered_ids, mut action_map) = register_hotkeys(hwnd, &hotkeys);

        *running.lock() = true;
        info!("Hotkey message loop started on thread {:?}", std::thread::current().id());

        // Standard GetMessageW message loop
        unsafe {
            let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
            loop {
                let ret = GetMessageW(&mut msg, None, 0, 0);
                if ret.0 == 0 || ret.0 == -1 {
                    break;
                }
                if msg.message == WM_STOP_MESSAGE_LOOP {
                    break;
                }
                if msg.message == WM_RELOAD_HOTKEYS {
                    // Hot-reload: unregister old, re-read config, register new
                    info!("Hot-reloading keybindings...");
                    unregister_hotkeys(hwnd, &registered_ids);
                    let new_hotkeys = build_hotkey_list(&engine);
                    let (new_ids, new_map) = register_hotkeys(hwnd, &new_hotkeys);
                    registered_ids = new_ids;
                    action_map = new_map;
                    info!("Hotkey reload complete");
                    continue;
                }
                if msg.message == 0x0312 { // WM_HOTKEY
                    let hotkey_id = msg.wParam.0 as i32;
                    if let Some(action) = action_map.get(&hotkey_id) {
                        info!("HOTKEY id={} -> {:?}", hotkey_id, action);
                        execute_action(action, &engine, &backend_handle);
                    }
                } else {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                // Drain any pending touch-triggered actions. We piggy-back on
                // the message loop because touch gestures are produced from a
                // separate hook thread and need to dispatch under the same
                // engine lock semantics as keyboard hotkeys.
                for action in crate::input::touch::take_pending_actions() {
                    info!("TOUCH -> {:?}", action);
                    execute_action(&action, &engine, &backend_handle);
                }
            }
        }

        unregister_hotkeys(hwnd, &registered_ids);
        unsafe { let _ = DestroyWindow(hwnd); }
        *HOTKEY_THREAD.lock() = None;
        *running.lock() = false;
        info!("Hotkey message loop exited");
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_DESTROY => { PostQuitMessage(0); LRESULT(0) }
            WM_DISPLAYCHANGE => {
                // Monitor hot-plug: re-enumerate monitors and notify the backend
                // via the static `backend::hooks::set_backend` handle that the
                // WinEvent thread holds. We synthesise `MonitorConnected` events
                // for every currently-attached monitor; the main event handler
                // diffs against the engine's known set, registers new monitors,
                // and unregisters vanished ones (see `handle_backend_event`).
                info!("WM_DISPLAYCHANGE received — monitor configuration changed");
                crate::backend::hooks::notify_display_change();
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    /// Tell the hotkey thread to reload keybindings from the engine's config
    pub fn reload_hotkeys(&self) {
        let state = HOTKEY_THREAD.lock();
        if let Some(s) = &*state {
            unsafe {
                let _ = PostThreadMessageW(
                    s.thread_id,
                    WM_RELOAD_HOTKEYS,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
            info!("Sent WM_RELOAD_HOTKEYS to hotkey thread");
        }
    }

    pub fn stop(&self) {
        *self.running.lock() = false;
        let state = HOTKEY_THREAD.lock();
        if let Some(s) = &*state {
            unsafe {
                let _ = PostThreadMessageW(
                    s.thread_id,
                    WM_STOP_MESSAGE_LOOP,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }

    pub fn is_running(&self) -> bool { *self.running.lock() }

    pub async fn run(&mut self) -> Result<()> {
        // Set running=true synchronously so the polling loop does not return
        // immediately before the hotkey thread has had a chance to set it.
        *self.running.lock() = true;
        while self.is_running() {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        Ok(())
    }
}

impl Drop for MessageLoop {
    fn drop(&mut self) { self.stop(); }
}

impl Default for MessageLoop {
    fn default() -> Self { Self::new().expect("Failed to create message loop") }
}

/// Key-repeat debounce state. Allows max 8 actions/sec for held keys.
use std::sync::LazyLock;
static LAST_ACTION: LazyLock<parking_lot::Mutex<(Action, std::time::Instant)>> =
    LazyLock::new(|| {
        // Start 60s in the past so the first real Action::Quit (or any action)
        // within 125 ms of startup is never silently dropped.
        let past = std::time::Instant::now() - std::time::Duration::from_secs(60);
        parking_lot::Mutex::new((Action::Quit, past))
    });

fn execute_action(
    action: &Action,
    engine: &Arc<parking_lot::RwLock<TilingEngine>>,
    backend: &BackendHandle,
) {
    // Debounce key repeat: derive from config keyboard.repeat_rate (defaults to ~8/sec).
    {
        let debounce_ms = engine
            .read()
            .full_config()
            .map(|c| crate::input::repeat_debounce_ms(&c.input))
            .unwrap_or(125);
        let mut last = LAST_ACTION.lock();
        if last.0 == *action && last.1.elapsed() < std::time::Duration::from_millis(debounce_ms) {
            return;
        }
        *last = (action.clone(), std::time::Instant::now());
    }

    match action {
        Action::FocusColumnLeft => {
            engine.write().focus_left(backend);
        }
        Action::FocusColumnRight => {
            engine.write().focus_right(backend);
        }
        Action::FocusUp => {
            engine.write().focus_up(backend);
        }
        Action::FocusDown => {
            engine.write().focus_down(backend);
        }
        Action::MoveColumnLeft => {
            engine.write().move_column(ScrollDirection::Left, backend);
        }
        Action::MoveColumnRight => {
            engine.write().move_column(ScrollDirection::Right, backend);
        }
        Action::CloseWindow => {
            engine.write().close_focused_window(backend);
        }
        Action::Spawn(cmd) => {
            spawn_process(cmd);
        }
        Action::ToggleFullscreen => {
            engine.write().toggle_fullscreen(backend);
        }
        Action::Quit => {
            std::process::exit(0);
        }
        Action::FocusWorkspace(id) => {
            engine.write().switch_workspace(*id, backend);
        }
        Action::ScrollLeft => {
            engine.write().scroll(ScrollDirection::Left, backend);
        }
        Action::ScrollRight => {
            engine.write().scroll(ScrollDirection::Right, backend);
        }
        Action::ToggleFloating => {
            engine.write().toggle_floating(backend);
        }
        Action::OverviewToggle => {
            engine.write().toggle_overview(backend);
        }
        Action::OverviewLeft => {
            if engine.read().is_overview() {
                engine.write().overview_focus_left(backend);
            } else {
                engine.write().focus_left(backend);
            }
        }
        Action::OverviewRight => {
            if engine.read().is_overview() {
                engine.write().overview_focus_right(backend);
            } else {
                engine.write().focus_right(backend);
            }
        }
        Action::OverviewSelect => {
            engine.write().overview_select(backend);
        }
        Action::Maximize => {
            // Maximize the currently focused window
            let fg = unsafe { GetForegroundWindow() };
            if !fg.0.is_null() {
                unsafe { let _ = ShowWindow(fg, SW_MAXIMIZE); }
                info!("Maximize: hwnd={:?}", fg);
            } else {
                warn!("Maximize: no foreground window");
            }
        }
        Action::Minimize => {
            // Minimize the currently focused window
            let fg = unsafe { GetForegroundWindow() };
            if !fg.0.is_null() {
                unsafe { let _ = ShowWindow(fg, SW_MINIMIZE); }
                info!("Minimize: hwnd={:?}", fg);
            } else {
                warn!("Minimize: no foreground window");
            }
        }
        Action::CenterWindow => {
            // Center the focused window on its current monitor's work area
            let fg = unsafe { GetForegroundWindow() };
            if fg.0.is_null() {
                warn!("CenterWindow: no foreground window");
            } else {
                let hmon = unsafe { MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) };
                let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
                mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                if unsafe { GetMonitorInfoW(hmon, &mut mi).as_bool() } {
                    let wa = mi.rcWork;
                    let wa_w = wa.right - wa.left;
                    let wa_h = wa.bottom - wa.top;
                    // Place at 60% of work area, centered
                    let win_w = wa_w * 6 / 10;
                    let win_h = wa_h * 6 / 10;
                    let x = wa.left + (wa_w - win_w) / 2;
                    let y = wa.top + (wa_h - win_h) / 2;
                    unsafe {
                        let _ = SetWindowPos(
                            fg, HWND_TOP, x, y, win_w, win_h,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                    info!("CenterWindow: hwnd={:?} x={} y={} w={} h={}", fg, x, y, win_w, win_h);
                } else {
                    warn!("CenterWindow: GetMonitorInfoW failed");
                }
            }
        }
        Action::SwitchMonitor => {
            // Generic "switch monitor" — cycle focus to the next monitor in
            // x-order, wrapping around. Distinct from MoveToMonitorLeft/Right,
            // which move the focused window across monitors.
            let mut engine_w = engine.write();
            let order: Vec<_> = {
                let mut v: Vec<_> = engine_w.monitors().iter()
                    .map(|(oid, m)| (m.bounds.loc.x, *oid))
                    .collect();
                v.sort_by_key(|(x, _)| *x);
                v
            };
            if order.is_empty() {
                return;
            }
            let cur = engine_w.focused_output();
            let next_idx = match cur {
                Some(oid) => order.iter().position(|(_, o)| *o == oid)
                    .map(|i| (i + 1) % order.len())
                    .unwrap_or(0),
                None => 0,
            };
            let target = order[next_idx].1;
            engine_w.set_focused_output(target);
            engine_w.apply_all(backend);
            info!("SwitchMonitor: focused {:?}", target);
        }
        Action::MoveWorkspace(target_ws) => {
            // Move the focused window to workspace `target_ws` (no switch).
            engine.write().move_window_to_workspace(*target_ws, backend);
            engine.write().apply_all(backend);
        }
        Action::ColumnToggleTabbed => {
            engine.write().toggle_tabbed_for_focused_column(backend);
            engine.write().apply_all(backend);
        }
        Action::TabNext => {
            engine.write().focused_column_next_tab(backend);
            engine.write().apply_all(backend);
        }
        Action::TabPrev => {
            engine.write().focused_column_prev_tab(backend);
            engine.write().apply_all(backend);
        }
        Action::Refresh => {
            engine.write().apply_all(backend);
        }
        Action::MoveToWorkspace(id) => {
            engine.write().move_window_to_workspace(*id, backend);
            engine.write().apply_all(backend);
        }
        Action::MoveToMonitorLeft => {
            engine.write().move_window_to_monitor(crate::layout::ScrollDirection::Left, backend);
            engine.write().apply_all(backend);
        }
        Action::MoveToMonitorRight => {
            engine.write().move_window_to_monitor(crate::layout::ScrollDirection::Right, backend);
            engine.write().apply_all(backend);
        }
        Action::CenterColumn => {
            engine.write().center_focused_column(backend);
            engine.write().apply_all(backend);
        }
        Action::ColumnWidthPresetCycle => {
            engine.write().set_column_width_preset(crate::layout::ColumnWidthPreset::Cycle, backend);
            engine.write().apply_all(backend);
        }
        Action::ColumnWidthPresetHalf => {
            engine.write().set_column_width_preset(crate::layout::ColumnWidthPreset::Half, backend);
            engine.write().apply_all(backend);
        }
        Action::ColumnWidthPresetThird => {
            engine.write().set_column_width_preset(crate::layout::ColumnWidthPreset::OneThird, backend);
            engine.write().apply_all(backend);
        }
        Action::ColumnWidthPresetTwoThirds => {
            engine.write().set_column_width_preset(crate::layout::ColumnWidthPreset::TwoThirds, backend);
            engine.write().apply_all(backend);
        }
        Action::ColumnWidthPresetFull => {
            engine.write().set_column_width_preset(crate::layout::ColumnWidthPreset::Full, backend);
            engine.write().apply_all(backend);
        }
        Action::ResizeColumnLeft => {
            engine.write().resize_focused_column_by(-100, backend);
            engine.write().apply_all(backend);
        }
        Action::ResizeColumnRight => {
            engine.write().resize_focused_column_by(100, backend);
            engine.write().apply_all(backend);
        }
        Action::FocusWorkspaceNext => {
            engine.write().focus_workspace_relative(crate::layout::WorkspaceDirection::Next, backend);
            engine.write().apply_all(backend);
        }
        Action::FocusWorkspacePrevious => {
            engine.write().focus_workspace_relative(crate::layout::WorkspaceDirection::Previous, backend);
            engine.write().apply_all(backend);
        }
        Action::FocusPrevious => {
            engine.write().focus_previous_window(backend);
            engine.write().apply_all(backend);
        }
        Action::ToggleAlwaysOnTop => {
            engine.write().toggle_always_on_top_for_focused(backend);
        }
        Action::FocusWorkspaceNamed(name) => {
            engine.write().focus_workspace_named(name, backend);
            engine.write().apply_all(backend);
        }
        Action::SetAutoTileThreshold(t) => {
            engine.write().set_auto_tile_threshold(*t);
            engine.write().apply_all(backend);
        }
    }
}

fn spawn_process(cmd: &str) {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() { return; }
    match std::process::Command::new(parts[0]).args(&parts[1..]).spawn() {
        Ok(_) => info!("Spawned: {}", cmd),
        Err(e) => warn!("Spawn failed: {}", e),
    }
}

/// Resolve the user's preferred "terminal" command, falling back to `cmd.exe`.
///
/// Looked up from the engine's full config in this order:
/// 1. The first `Spawn(...)` entry in the configured binds (so a user bind
///    `Mod+Enter { spawn "wt.exe" }` overrides cmd.exe automatically).
/// 2. The first non-empty entry in `spawn_at_startup` (rare, but if a user
///    has only declared their terminal there we still find it).
/// 3. Literal `"cmd.exe"`.
fn resolve_terminal_command(engine: &Arc<parking_lot::RwLock<TilingEngine>>) -> String {
    let eng = engine.read();
    if let Some(cfg) = eng.full_config() {
        // 1. Look for a Spawn bind in the config.
        for bind in &cfg.binds.hotkeys {
            if let Some(Action::Spawn(cmd)) = bind.parse_action() {
                if !cmd.trim().is_empty() {
                    return cmd;
                }
            }
        }
        // 2. Look at spawn-at-startup entries.
        if let Some(entry) = cfg.spawn_at_startup.iter().find(|e| !e.is_empty()) {
            return entry.join(" ");
        }
    }
    "cmd.exe".to_string()
}
