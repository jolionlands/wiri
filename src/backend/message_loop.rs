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
        // Bare Escape → OverviewToggle.  Conceptually wrong on its own
        // (we don't want Escape to enter overview), so the WM_HOTKEY
        // dispatcher in `run_message_loop` state-gates this binding: the
        // action only fires when overview is already active.  Registered
        // unconditionally so the OS knows the chord belongs to wiri while
        // overview is on.
        (0, 0x1B, Action::OverviewToggle),             // Escape (gated)
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
        (prefix, 0x52, Action::EnterResizeMode),          // R — niri-style interactive resize
        (shift_prefix, 0x52, Action::CenterColumn),       // Shift+R — center-on-screen
        (prefix, 0x57, Action::ColumnWidthPresetCycle),   // W
        (prefix, 0xBD, Action::ResizeColumnLeft),         // VK_OEM_MINUS
        (prefix, 0xBB, Action::ResizeColumnRight),        // VK_OEM_PLUS
        // NOTE: Shift+H / Shift+L are repurposed below to Shrink/GrowColumnWidth
        // for niri parity. MoveToMonitorLeft/Right remain available via the
        // KDL `binds {}` block (e.g. Ctrl+Alt+Comma+Shift) or wiri-ctl.
        // ((shift_prefix, 0x48, Action::MoveToMonitorLeft) replaced)
        // ((shift_prefix, 0x4C, Action::MoveToMonitorRight) replaced)
        // Niri-parity round-2: FocusPrevious (alt-tab MRU) and ToggleAlwaysOnTop
        // NOTE: if prefix is Ctrl+Alt, prefix+Tab conflicts with the Windows system
        // Ctrl+Alt+Tab switcher. RegisterHotKey will fail; that's OK — the warning
        // will be logged and the action remains available via custom KDL binds.
        (prefix, 0x09, Action::FocusPrevious),            // Tab
        (prefix, 0x50, Action::Screenshot),               // P (capture desktop to file)
        (prefix, 0x41, Action::ToggleAlwaysOnTop),        // A (always-on-top)
        // niri-parity Round 4: rearrange / sizing actions.
        //   Ctrl+Alt+Comma   → consume window into next column
        //   Ctrl+Alt+Period  → expel window into a new column to the right
        //   Ctrl+Alt+E       → expand column to fill leftover width
        //   Ctrl+Alt+Shift+F → maximize column (full work-area height)
        //   Ctrl+Alt+Shift+L → grow column width (5%)
        //   Ctrl+Alt+Shift+H → shrink column width (5%)
        //   Ctrl+Alt+Shift+K → grow tile height (5%)
        //   Ctrl+Alt+Shift+J → shrink tile height (5%)
        //   Ctrl+Alt+Shift+PgUp/PgDn → move column to monitor left/right
        (prefix, 0xBC, Action::ConsumeWindowIntoColumn),   // VK_OEM_COMMA  ","
        (prefix, 0xBE, Action::ExpelWindowFromColumn),     // VK_OEM_PERIOD "."
        (prefix, 0x45, Action::ExpandColumnToAvailable),   // E
        (shift_prefix, 0x46, Action::MaximizeColumn),      // Shift+F
        (shift_prefix, 0x4C, Action::GrowColumnWidth),     // Shift+L (overrides MoveToMonitorRight above)
        (shift_prefix, 0x48, Action::ShrinkColumnWidth),   // Shift+H (overrides MoveToMonitorLeft above)
        (shift_prefix, 0x4B, Action::GrowTileHeight),      // Shift+K
        (shift_prefix, 0x4A, Action::ShrinkTileHeight),    // Shift+J
        // Workspace swap (reorder the workspace stack) — niri-parity
        // `Mod+Ctrl+Page_Up/Down`.  Our default prefix is Ctrl+Alt, so the
        // shifted variant naturally lands on `Ctrl+Alt+Shift+Page_Up/Down`,
        // which is what we used to dedicate to move-column-to-monitor.  The
        // column-to-monitor chords move to Ctrl+Alt+Shift+Comma/Period below.
        (shift_prefix, 0x21, Action::MoveWorkspaceUp),    // Shift+PageUp
        (shift_prefix, 0x22, Action::MoveWorkspaceDown),  // Shift+PageDown
        // Niri parity: move focused column vertically between workspaces.
        // Ctrl+Alt+Shift+Up/Down (VK_UP=0x26, VK_DOWN=0x28).
        (shift_prefix, 0x26, Action::MoveColumnToWorkspaceUp),
        (shift_prefix, 0x28, Action::MoveColumnToWorkspaceDown),
        // Move column to monitor moves to Ctrl+Alt+Shift+Comma/Period
        // (VK_OEM_COMMA=0xBC, VK_OEM_PERIOD=0xBE) to free up Page_Up/Down
        // for workspace swap per the niri default.
        (shift_prefix, 0xBC, Action::MoveColumnToMonitorLeft),
        (shift_prefix, 0xBE, Action::MoveColumnToMonitorRight),
    ]
}

/// Pure helper: merge `parsed_config` with `defaults` according to
/// `extend_defaults`.  Returns the final (mods, vk, Action) table plus the
/// number of (default, config) collisions overridden by the config side.
///
/// When `extend_defaults` is true:
///   - Start from defaults.
///   - For every (mods, vk) collision, the config bind wins.
///   - Non-colliding config binds are appended.
///
/// When `extend_defaults` is false:
///   - Returns `parsed_config` verbatim (override count is the input length
///     so the caller can log it; the defaults are discarded).
///   - If `parsed_config` is empty, returns `defaults` so the daemon never
///     ships zero hotkeys after a bad reload.
fn merge_hotkeys(
    defaults: Vec<(u32, u32, Action)>,
    parsed_config: Vec<(u32, u32, Action)>,
    extend_defaults: bool,
) -> (Vec<(u32, u32, Action)>, usize) {
    use std::collections::{HashMap, HashSet};
    if !extend_defaults {
        if parsed_config.is_empty() {
            return (defaults, 0);
        }
        let n = parsed_config.len();
        return (parsed_config, n);
    }
    if parsed_config.is_empty() {
        return (defaults, 0);
    }
    let default_count = defaults.len();
    let cfg_index: HashMap<(u32, u32), Action> = parsed_config
        .iter()
        .map(|(m, v, a)| ((*m, *v), a.clone()))
        .collect();
    let mut merged: Vec<(u32, u32, Action)> =
        Vec::with_capacity(default_count + parsed_config.len());
    let mut overridden = 0usize;
    let mut covered_in_defaults: HashSet<(u32, u32)> = HashSet::new();
    for (m, v, default_action) in &defaults {
        match cfg_index.get(&(*m, *v)) {
            Some(replacement) => {
                merged.push((*m, *v, replacement.clone()));
                overridden += 1;
                covered_in_defaults.insert((*m, *v));
            }
            None => merged.push((*m, *v, default_action.clone())),
        }
    }
    for (m, v, a) in parsed_config.into_iter() {
        if !covered_in_defaults.contains(&(m, v)) {
            merged.push((m, v, a));
        }
    }
    (merged, overridden)
}

/// Build hotkey list from config binds, optionally extending the built-in
/// defaults so newly-shipped actions stay available to users with stale
/// configs.
///
/// Resolution order:
///   1. Read `binds.extend_defaults` (default `true`).
///   2. When `true`: build the default table, then walk config binds.
///      Each successfully-parsed config bind WINS on a `(mods, vk)`
///      collision with the defaults; otherwise the config bind is
///      appended.  Bindings the user could not parse (unknown key /
///      unknown action name) are logged and skipped.
///   3. When `false`: behaviour matches the pre-2026-05-18 path —
///      config binds replace defaults entirely (with the same skip-and-log
///      semantics), and an empty surviving table falls back to defaults
///      so the daemon never ships zero hotkeys after a bad reload.
fn build_hotkey_list(engine: &Arc<parking_lot::RwLock<TilingEngine>>) -> Vec<(u32, u32, Action)> {
    let terminal_cmd = resolve_terminal_command(engine);
    let eng = engine.read();
    let binds = eng.config_binds();
    let config_binds = &binds.hotkeys;
    let extend_defaults = binds.extend_defaults;
    let (prefix, shift_prefix) = eng
        .full_config()
        .map(|c| resolve_mod_prefix(&c.input.mod_key))
        .unwrap_or((MOD_CTRL | MOD_ALT, MOD_CTRL | MOD_ALT | MOD_SHIFT));

    // Always materialise the defaults first so we can both A) return them
    // verbatim when the user's binds block is missing, and B) merge with
    // user binds when extend_defaults=true.
    let defaults = default_hotkeys(prefix, shift_prefix, &terminal_cmd);
    let default_count = defaults.len();

    if config_binds.is_empty() {
        info!("Hotkeys: {} from defaults, 0 from config (0 overridden)", default_count);
        return defaults;
    }

    // Parse every config bind once.
    let mut parsed_config: Vec<(u32, u32, Action)> = Vec::with_capacity(config_binds.len());
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
        parsed_config.push((mods, vk, action));
    }

    let config_count = parsed_config.len();
    let (merged, overridden) = merge_hotkeys(defaults, parsed_config, extend_defaults);
    if extend_defaults {
        info!(
            "Hotkeys: {} from defaults, {} from config ({} overridden)",
            default_count.saturating_sub(overridden),
            config_count,
            overridden,
        );
    } else {
        info!(
            "Hotkeys: 0 from defaults (extend-defaults=false), {} from config",
            config_count,
        );
    }
    if merged.is_empty() {
        warn!("Hotkey table empty after merge; defaults reinstated");
        return default_hotkeys(prefix, shift_prefix, &terminal_cmd);
    }
    merged
}

/// Chords that wiri must NEVER attempt to register.  These are reserved by
/// Windows for security- or system-critical functions and stealing them at
/// the WM layer causes very bad user-visible behaviour (e.g. binding
/// `MOD_WIN+L` would lock out the user's lock-screen shortcut).  Even when
/// `RegisterHotKey` would happily accept the registration, the operating
/// system intercepts the chord before our process sees it — so the bind is
/// at best a no-op and at worst a footgun.  Format: `(modifiers, vk, label)`.
const SYSTEM_CRITICAL_HOTKEYS: &[(u32, u32, &str)] = &[
    (MOD_WIN, 0x4C, "Win+L (lock screen)"),
    (MOD_WIN | MOD_SHIFT, 0x53, "Win+Shift+S (snipping tool)"),
    (MOD_WIN, 0x44, "Win+D (show desktop)"),
];

/// Returns Some(label) when the (mods, vk) pair is in the
/// [`SYSTEM_CRITICAL_HOTKEYS`] denylist.  Pure helper extracted so the test
/// suite can exercise the filter without touching `RegisterHotKey`.
fn is_system_critical_hotkey(mods: u32, vk: u32) -> Option<&'static str> {
    SYSTEM_CRITICAL_HOTKEYS
        .iter()
        .find(|(m, v, _)| *m == mods && *v == vk)
        .map(|(_, _, label)| *label)
}

/// Register a list of hotkeys with Windows, returning the IDs, action map,
/// and the subset of hotkey ids that must be state-gated on overview mode
/// (i.e. bare Escape → OverviewToggle, which should only fire when overview
/// is already active so the user can press Esc to exit).
fn register_hotkeys(
    hwnd: HWND,
    hotkeys: &[(u32, u32, Action)],
) -> (Vec<i32>, HashMap<i32, Action>, std::collections::HashSet<i32>) {
    let base_id = 32768i32;
    let mut registered_ids: Vec<i32> = Vec::new();

    for (i, (mods, vk, action)) in hotkeys.iter().enumerate() {
        let id = base_id + i as i32;
        // System-critical chords (Win+L, Win+Shift+S, Win+D) must never be
        // registered — even if RegisterHotKey accepts them, the OS intercepts
        // first and the user loses the original behaviour.  Skip silently
        // with a WARN so the audit log reflects the intent.
        if let Some(label) = is_system_critical_hotkey(*mods, *vk) {
            warn!(
                "SKIP hotkey id={} mods=0x{:X} vk=0x{:02X} {:?} — reserved by Windows ({})",
                id, mods, vk, action, label,
            );
            continue;
        }
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

    // Detect bare-Escape → OverviewToggle bindings (mods=0, vk=0x1B) — those
    // must be gated on `engine.is_overview()` so Esc remains usable outside
    // overview mode.
    let overview_exit_only: std::collections::HashSet<i32> = hotkeys
        .iter()
        .enumerate()
        .filter(|(i, (mods, vk, action))| {
            *mods == 0
                && *vk == 0x1B
                && matches!(action, Action::OverviewToggle)
                && registered_ids.contains(&(base_id + *i as i32))
        })
        .map(|(i, _)| base_id + i as i32)
        .collect();

    (registered_ids, action_map, overview_exit_only)
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

/// Snapshot of the currently registered hotkey bindings, refreshed by the
/// hotkey thread on every `register_hotkeys` call (initial + hot-reload).
/// Exposed via `MessageLoop::current_bindings()` so the IPC `ListBindings`
/// handler can return what the daemon really registered (vs what was in the
/// config) — useful for diagnosing why a chord isn't firing.
static REGISTERED_BINDINGS: Mutex<Vec<RegisteredBinding>> = Mutex::new(Vec::new());

/// One registered hotkey binding, used by `MessageLoop::current_bindings`.
#[derive(Debug, Clone)]
pub struct RegisteredBinding {
    /// Win32 hotkey id returned by `RegisterHotKey`.
    pub id: i32,
    /// Win32 MOD_* bitmask (1=Alt, 2=Ctrl, 4=Shift, 8=Win).
    pub modifiers: u32,
    /// Win32 VK_* virtual key code.
    pub vk_code: u32,
    /// Action this binding fires.  Held as a debug-formatted string so the
    /// IPC layer (which doesn't depend on `Action`'s serde impl) can ship it.
    pub action: String,
}

/// Refresh the global REGISTERED_BINDINGS snapshot after a (re-)registration
/// pass.  Stores only bindings that Windows actually accepted (i.e. whose ID
/// appears in `registered_ids`) so users get a true picture of what's live.
fn update_current_bindings_snapshot(
    hotkeys: &[(u32, u32, Action)],
    registered_ids: &[i32],
) {
    let base_id = 32768i32;
    let mut snap: Vec<RegisteredBinding> = Vec::with_capacity(hotkeys.len());
    for (i, (mods, vk, action)) in hotkeys.iter().enumerate() {
        let id = base_id + i as i32;
        if !registered_ids.contains(&id) {
            continue;
        }
        snap.push(RegisteredBinding {
            id,
            modifiers: *mods,
            vk_code: *vk,
            action: format!("{:?}", action),
        });
    }
    *REGISTERED_BINDINGS.lock() = snap;
}

/// Return a clone of every hotkey currently registered with Windows.
/// Empty when the hotkey thread hasn't started or registration failed.
pub fn current_bindings() -> Vec<RegisteredBinding> {
    REGISTERED_BINDINGS.lock().clone()
}

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

        // Register initial hotkeys.  Keep `current_hotkeys` so the snapshot
        // and hot-reload paths share a single source of truth.
        let current_hotkeys = build_hotkey_list(&engine);
        let (mut registered_ids, mut action_map, mut overview_exit_only) =
            register_hotkeys(hwnd, &current_hotkeys);
        // Snapshot mirror for `current_bindings()` queries from IPC.
        update_current_bindings_snapshot(&current_hotkeys, &registered_ids);

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
                    let (new_ids, new_map, new_exit_only) =
                        register_hotkeys(hwnd, &new_hotkeys);
                    registered_ids = new_ids;
                    action_map = new_map;
                    overview_exit_only = new_exit_only;
                    update_current_bindings_snapshot(&new_hotkeys, &registered_ids);
                    info!("Hotkey reload complete");
                    continue;
                }
                if msg.message == 0x0312 { // WM_HOTKEY
                    let hotkey_id = msg.wParam.0 as i32;
                    if let Some(action) = action_map.get(&hotkey_id) {
                        // Esc → OverviewToggle is registered globally but only
                        // dispatched when overview is currently active OR
                        // interactive resize-mode is engaged (Esc is the
                        // canonical "exit resize mode" key).  Outside both
                        // states the keypress is swallowed so Esc stays
                        // available to other apps.
                        if overview_exit_only.contains(&hotkey_id) {
                            let eng = engine.read();
                            let allow = eng.is_overview() || eng.is_resize_mode();
                            drop(eng);
                            if !allow {
                                continue;
                            }
                        }
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

    /// Diagnostic accessor: return a clone of the currently registered hotkey
    /// table (after the most recent `register_hotkeys` pass).  Used by the
    /// `ListBindings` IPC handler so `wiri-ctl test-bindings` can tell users
    /// what the daemon really registered vs what their config asked for.
    pub fn current_bindings(&self) -> Vec<RegisteredBinding> {
        current_bindings()
    }

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

    // niri-style interactive-resize mode: while engaged, the arrow-focus
    // chords resize the focused column / tile instead of moving focus.
    // Escape exits the mode (Esc-handling is gated on overview elsewhere,
    // but resize-mode also intercepts it via OverviewToggle below).
    let resize_active = engine.read().is_resize_mode();
    if resize_active {
        match action {
            Action::FocusColumnLeft => {
                engine.write().resize_focused_column_by_percent(-5, backend);
                engine.write().apply_all(backend);
                return;
            }
            Action::FocusColumnRight => {
                engine.write().resize_focused_column_by_percent(5, backend);
                engine.write().apply_all(backend);
                return;
            }
            Action::FocusUp => {
                engine.write().resize_focused_tile_height_by_percent(-5, backend);
                engine.write().apply_all(backend);
                return;
            }
            Action::FocusDown => {
                engine.write().resize_focused_tile_height_by_percent(5, backend);
                engine.write().apply_all(backend);
                return;
            }
            Action::OverviewToggle => {
                // Bare Escape (which is registered as OverviewToggle and
                // state-gated below) is the canonical "exit resize mode"
                // chord.  Swallow the action so overview doesn't open.
                engine.write().exit_resize_mode();
                return;
            }
            Action::EnterResizeMode => {
                // Pressing the toggle again exits.
                engine.write().exit_resize_mode();
                return;
            }
            _ => {}
        }
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
        Action::SpawnCmd(cmd) => {
            // niri-style `spawn-cmd "<command>"` — route through the
            // Windows shell so cmd-builtins (`start`, `dir`, redirection,
            // env-var expansion) and PowerShell one-liners work as the
            // user typed them.  Equivalent to `cmd.exe /C "<command>"`.
            spawn_shell_command(cmd);
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
        Action::Screenshot => {
            take_screenshot();
        }
        Action::ColumnWidthPresetQuarter => {
            engine.write().set_column_width_preset(crate::layout::ColumnWidthPreset::OneQuarter, backend);
            engine.write().apply_all(backend);
        }
        Action::ColumnWidthPresetThreeQuarters => {
            engine.write().set_column_width_preset(crate::layout::ColumnWidthPreset::ThreeQuarters, backend);
            engine.write().apply_all(backend);
        }
        Action::ConsumeWindowIntoColumn => {
            engine.write().consume_window_into_column(backend);
            engine.write().apply_all(backend);
        }
        Action::ExpelWindowFromColumn => {
            engine.write().expel_window_from_column(backend);
            engine.write().apply_all(backend);
        }
        Action::ExpandColumnToAvailable => {
            engine.write().expand_column_to_available(backend);
            engine.write().apply_all(backend);
        }
        Action::MaximizeColumn => {
            engine.write().toggle_maximize_focused_column(backend);
            engine.write().apply_all(backend);
        }
        Action::GrowColumnWidth => {
            engine.write().resize_focused_column_by_percent(5, backend);
            engine.write().apply_all(backend);
        }
        Action::ShrinkColumnWidth => {
            engine.write().resize_focused_column_by_percent(-5, backend);
            engine.write().apply_all(backend);
        }
        Action::GrowTileHeight => {
            engine.write().resize_focused_tile_height_by_percent(5, backend);
            engine.write().apply_all(backend);
        }
        Action::ShrinkTileHeight => {
            engine.write().resize_focused_tile_height_by_percent(-5, backend);
            engine.write().apply_all(backend);
        }
        Action::MoveColumnToMonitorLeft => {
            engine.write().move_column_to_monitor(crate::layout::ScrollDirection::Left, backend);
            engine.write().apply_all(backend);
        }
        Action::MoveColumnToMonitorRight => {
            engine.write().move_column_to_monitor(crate::layout::ScrollDirection::Right, backend);
            engine.write().apply_all(backend);
        }
        Action::WindowScreenshot => {
            // Capture the foreground window via PrintWindow. Resolves the
            // HWND through `GetForegroundWindow()` so this works for any
            // tile the user has focused (engine MRU isn't consulted —
            // intent matches what's on screen at hotkey time).
            take_window_screenshot();
        }
        Action::EnterResizeMode => {
            // Enter or exit resize mode.  When entering, the dispatcher
            // re-routes the Mod+Arrow chords to grow/shrink the focused
            // column / tile until the user presses Esc or Mod+R again.
            engine.write().toggle_resize_mode();
        }
        Action::MoveColumnToWorkspaceUp => {
            engine.write().move_focused_column_to_workspace(-1, backend);
            engine.write().apply_all(backend);
        }
        Action::MoveColumnToWorkspaceDown => {
            engine.write().move_focused_column_to_workspace(1, backend);
            engine.write().apply_all(backend);
        }
        Action::MoveWorkspaceUp => {
            engine.write().move_active_workspace(-1, backend);
            engine.write().apply_all(backend);
        }
        Action::MoveWorkspaceDown => {
            engine.write().move_active_workspace(1, backend);
            engine.write().apply_all(backend);
        }
    }
}

/// Capture the foreground window's bounding rect via the shared helper in
/// `crate::hooks::capture_focused_window_to_pictures` so the hotkey path,
/// the tray menu, and the IPC `CaptureWindow` request all obey identical
/// destination-resolution rules.
fn take_window_screenshot() {
    let hwnd: isize = unsafe { GetForegroundWindow().0 as isize };
    if hwnd == 0 {
        warn!("WindowScreenshot: no foreground window to capture");
        return;
    }
    match crate::hooks::capture_focused_window_to_pictures(hwnd) {
        Ok(path) => info!("Window screenshot saved to {}", path),
        Err(e) => warn!("Window screenshot failed: {}", e),
    }
}

/// Item 3: capture the full virtual desktop to a BMP file via the shared
/// helper in `crate::hooks::capture_screenshot_to_pictures` so the hotkey
/// path and the tray-menu path obey identical destination-resolution rules.
fn take_screenshot() {
    match crate::hooks::capture_screenshot_to_pictures() {
        Ok(path) => info!("Screenshot saved to {}", path),
        Err(e) => warn!("Screenshot failed: {}", e),
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

/// Launch a single command line through `cmd.exe /C "<command>"`.
///
/// Used by `Action::SpawnCmd` and the IPC `SpawnCommand` shell path.  The
/// shell expands env vars (`%USERPROFILE%`, etc.), recognises built-ins
/// (`start`, redirection, `&&`), and resolves PATH lookups for callers
/// that don't want to deal with `std::process::Command`'s argv parsing.
fn spawn_shell_command(cmd: &str) {
    if cmd.trim().is_empty() {
        return;
    }
    match std::process::Command::new("cmd.exe")
        .args(["/C", cmd])
        .spawn()
    {
        Ok(_) => info!("SpawnCmd (cmd.exe /C): {}", cmd),
        Err(e) => warn!("SpawnCmd failed: {}", e),
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
        // 1. Look for a Spawn / SpawnCmd bind in the config — either flavour
        // is treated as the user's terminal-launch preference for the
        // Mod+Enter default.
        for bind in &cfg.binds.hotkeys {
            match bind.parse_action() {
                Some(Action::Spawn(cmd)) | Some(Action::SpawnCmd(cmd)) => {
                    if !cmd.trim().is_empty() {
                        return cmd;
                    }
                }
                _ => {}
            }
        }
        // 2. Look at spawn-at-startup entries.
        if let Some(entry) = cfg.spawn_at_startup.iter().find(|e| !e.is_empty()) {
            return entry.join(" ");
        }
    }
    "cmd.exe".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(mods: u32, vk: u32, label: &str) -> (u32, u32, Action) {
        (mods, vk, Action::Spawn(label.to_string()))
    }

    /// extend_defaults=true preserves every default chord AND appends the
    /// non-colliding config chord; override count is zero.
    #[test]
    fn merge_extends_keeps_defaults_and_adds_new_config_chord() {
        let defaults = vec![
            d(0x0002, 0x25, "default-left"),
            d(0x0002, 0x27, "default-right"),
        ];
        let config = vec![d(0x0001, 0x50, "config-alt-p")];
        let (merged, overridden) = merge_hotkeys(defaults, config, true);
        assert_eq!(overridden, 0);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].2, Action::Spawn("default-left".to_string()));
        assert_eq!(merged[1].2, Action::Spawn("default-right".to_string()));
        assert_eq!(merged[2].2, Action::Spawn("config-alt-p".to_string()));
    }

    /// extend_defaults=true: config wins on a (mods, vk) collision; the
    /// override count is incremented and the original default action is
    /// replaced in-place.
    #[test]
    fn merge_extends_config_wins_on_collision() {
        let defaults = vec![d(0x0002, 0x25, "default-left")];
        let config = vec![d(0x0002, 0x25, "config-left")];
        let (merged, overridden) = merge_hotkeys(defaults, config, true);
        assert_eq!(overridden, 1);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].2, Action::Spawn("config-left".to_string()));
    }

    /// extend_defaults=false drops every default and uses the config table
    /// verbatim, including non-colliding chords.
    #[test]
    fn merge_no_extend_replaces_defaults_entirely() {
        let defaults = vec![
            d(0x0002, 0x25, "default-left"),
            d(0x0002, 0x27, "default-right"),
        ];
        let config = vec![d(0x0001, 0x50, "config-alt-p")];
        let (merged, _overridden) = merge_hotkeys(defaults, config, false);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].2, Action::Spawn("config-alt-p".to_string()));
    }

    /// Empty config + extend=true returns defaults untouched (a fresh user
    /// install or a config with only `binds { }` should still get every
    /// shipped default).
    #[test]
    fn merge_empty_config_returns_defaults() {
        let defaults = vec![d(0x0002, 0x25, "default-left")];
        let (merged, overridden) = merge_hotkeys(defaults.clone(), vec![], true);
        assert_eq!(overridden, 0);
        assert_eq!(merged, defaults);
    }

    /// System-critical chords (Win+L, Win+Shift+S, Win+D) must be flagged by
    /// `is_system_critical_hotkey` so `register_hotkeys` skips them silently
    /// before ever calling into `RegisterHotKey`.  Non-listed chords must
    /// pass through unaltered.
    #[test]
    fn system_critical_hotkey_filter_catches_win_l_and_friends() {
        // Win+L (lock screen) — VK_L = 0x4C, MOD_WIN = 0x0008.
        assert!(
            is_system_critical_hotkey(MOD_WIN, 0x4C).is_some(),
            "Win+L must be flagged as system-critical"
        );
        // Win+Shift+S (snipping tool) — VK_S = 0x53.
        assert!(
            is_system_critical_hotkey(MOD_WIN | MOD_SHIFT, 0x53).is_some(),
            "Win+Shift+S must be flagged as system-critical"
        );
        // Win+D (show desktop) — VK_D = 0x44.
        assert!(
            is_system_critical_hotkey(MOD_WIN, 0x44).is_some(),
            "Win+D must be flagged as system-critical"
        );

        // Non-system chord (Ctrl+Alt+Q, our quit binding) must pass through.
        assert!(
            is_system_critical_hotkey(MOD_CTRL | MOD_ALT, 0x51).is_none(),
            "Ctrl+Alt+Q is a normal binding, must NOT be filtered"
        );
        // Bare L (no modifiers) is not a system chord even though VK matches.
        assert!(
            is_system_critical_hotkey(0, 0x4C).is_none(),
            "Bare L without Win is not a system-critical hotkey"
        );
    }
}
