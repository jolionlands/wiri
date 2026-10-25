use anyhow::Result;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::{mpsc, Arc};
use tracing::{debug, error, info, warn};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow,
    GetMessageW, PostQuitMessage, PostThreadMessageW, RegisterClassW, ShowWindow, TranslateMessage,
    SW_MAXIMIZE, SW_MINIMIZE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_DESTROY, WM_DISPLAYCHANGE, WM_USER,
    WNDCLASSW, WNDCLASS_STYLES,
};

use super::BackendHandle;
use crate::input::{Action, SizeChange};
use crate::layout::{ScrollDirection, TilingEngine};

const WM_STOP_MESSAGE_LOOP: u32 = WM_USER + 100;
/// Custom message sent to the hotkey thread to trigger re-registration
const WM_RELOAD_HOTKEYS: u32 = WM_USER + 102;

// Do NOT use MOD_NOREPEAT (0x4000) — it causes RegisterHotKey to fail on
// ARM64 Windows. Key repeat is debounced in software.
const MOD_ALT: u32 = 0x0001;
const MOD_CTRL: u32 = 0x0002;
const MOD_SHIFT: u32 = 0x0004;
const MOD_WIN: u32 = 0x0008;

// TODO(audit): per-monitor mod-key only takes effect when used in
// monitor-scoped IPC actions (e.g. focus-monitor-by-prefix). Global hotkeys
// remain unified across monitors because Windows registers hotkeys per-thread,
// not per-display.  The `OutputConfig.mod_key` field is populated by the parser
// and can be read via `engine.full_config().map(|c| &c.output[i].mod_key)` for
// future IPC dispatch.

// TODO(item3-wheel): the low-level mouse-wheel hook in `src/input/low_level_hook.rs`
// needs to call `engine.read().is_over_gap(cursor_x, cursor_y) -> bool` to decide
// whether a no-modifier scroll should be consumed as `Action::ScrollLeft/Right`
// (gap zone) or passed through to the window (content zone).  The engine helper
// `TilingEngine::is_over_gap` is implemented in `src/layout/engine.rs`.

/// Resolve a `mod-key` config string to a (prefix, prefix+shift) pair of Win32 modifier flags.
/// A value wiri does not know falls back to Ctrl+Alt (config loading already warned about it).
fn resolve_mod_prefix(name: &str) -> (u32, u32) {
    let prefix = crate::input::mod_key_flags(name).unwrap_or(crate::input::DEFAULT_MOD_KEY);
    (prefix, prefix | MOD_SHIFT)
}

/// Turn one config bind into a `(modifiers, vk, action)` hotkey row, or say why
/// it cannot become one.  `mod_key` is what `Mod` stands for.  A bind with a
/// modifier wiri does not understand is refused, never registered as the bare
/// key (which would steal that key from every application).
fn bind_to_hotkey(
    bind: &crate::config::HotkeyBinding,
    mod_key: u32,
) -> Result<(u32, u32, Action), String> {
    let mods = bind
        .resolve_modifiers(mod_key)
        .map_err(|e| format!("{}: {}", bind.chord_text(), e))?;
    let vk = bind
        .vk_code()
        .ok_or_else(|| format!("{}: unknown key '{}'", bind.chord_text(), bind.key))?;
    let action = bind
        .parse_action()
        .ok_or_else(|| {
            format!(
                "{}: unknown action or bad arguments '{} {}'",
                bind.chord_text(),
                bind.command,
                bind.args.join(" ")
            )
        })?;
    Ok((mods, vk, action))
}

/// Default hotkey bindings using the configured modifier prefix. The Mod+Enter
/// terminal-spawn binding is parameterised by `terminal` (see
/// [`resolve_terminal_action`]) so callers pick up the user's preferred terminal
/// instead of always launching the same one.
fn default_hotkeys(prefix: u32, shift_prefix: u32, terminal: Action) -> Vec<(u32, u32, Action)> {
    vec![
        (prefix, 0x25, Action::FocusColumnLeft),  // Left
        (prefix, 0x27, Action::FocusColumnRight), // Right
        (prefix, 0x26, Action::FocusUp),          // Up
        (prefix, 0x28, Action::FocusDown),        // Down
        (shift_prefix, 0x25, Action::MoveColumnLeft),
        (shift_prefix, 0x27, Action::MoveColumnRight),
        (prefix, 0x51, Action::CloseWindow), // Q
        (prefix, 0x0D, terminal), // Enter
        (prefix, 0x46, Action::ToggleFullscreen), // F
        (prefix, 0x54, Action::ToggleFloating), // T
        (prefix, 0x48, Action::ScrollLeft),  // H
        (prefix, 0x4C, Action::ScrollRight), // L
        (shift_prefix, 0x51, Action::Quit),  // Shift+Q
        (prefix, 0x20, Action::OverviewToggle), // Space
        (prefix, 0x4F, Action::OverviewSelect), // O
        (prefix, 0xDC, Action::ColumnToggleTabbed), // \
        (prefix, 0xDD, Action::TabNext),     // ]
        (prefix, 0xDB, Action::TabPrev),     // [
        (prefix, 0x31, Action::FocusWorkspace(1)), // 1
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
        (prefix, 0x21, Action::FocusWorkspacePrevious), // PageUp
        (prefix, 0x22, Action::FocusWorkspaceNext),     // PageDown
        (prefix, 0x52, Action::EnterResizeMode),        // R — niri-style interactive resize
        (shift_prefix, 0x52, Action::CenterColumn),     // Shift+R — center-on-screen
        (prefix, 0x57, Action::ColumnWidthPresetCycle), // W
        (prefix, 0xBD, Action::ResizeColumnLeft),       // VK_OEM_MINUS
        (prefix, 0xBB, Action::ResizeColumnRight),      // VK_OEM_PLUS
        // NOTE: Shift+H / Shift+L are repurposed below to Shrink/GrowColumnWidth
        // for niri parity. MoveToMonitorLeft/Right remain available via the
        // KDL `binds {}` block (e.g. Ctrl+Alt+Comma+Shift) or wiri-ctl.
        // ((shift_prefix, 0x48, Action::MoveToMonitorLeft) replaced)
        // ((shift_prefix, 0x4C, Action::MoveToMonitorRight) replaced)
        // Niri-parity round-2: FocusPrevious (alt-tab MRU) and ToggleAlwaysOnTop.
        // niri's own chord is Mod+Tab, but with the default Ctrl+Alt prefix that is
        // Windows' task switcher: RegisterHotKey refuses it ("already registered") and
        // used to log a warning at every start.  The key above Tab stands in for it.
        (prefix, 0xC0, Action::FocusPrevious),     // ` (grave), the key above Tab
        (prefix, 0x50, Action::Screenshot),        // P (capture desktop to file)
        (prefix, 0x41, Action::ToggleAlwaysOnTop), // A (always-on-top)
        // niri-parity Round 4: rearrange / sizing actions.
        //   Ctrl+Alt+Comma   → consume-window-into-column: the first window of the
        //                      column on the right moves under the focused one
        //   Ctrl+Alt+Period  → expel-window-from-column: the bottom window of the
        //                      focused column gets a column of its own on the right
        //   Ctrl+Alt+E       → expand column to fill leftover width
        //   Ctrl+Alt+Shift+F → maximize-column: the column is as wide as the view
        //   Ctrl+Alt+Shift+L → grow column width (5%)
        //   Ctrl+Alt+Shift+H → shrink column width (5%)
        //   Ctrl+Alt+Shift+K → grow tile height (5%)
        //   Ctrl+Alt+Shift+J → shrink tile height (5%)
        //   Ctrl+Alt+Shift+PgUp/PgDn → move column to monitor left/right
        (prefix, 0xBC, Action::ConsumeWindowIntoColumn), // VK_OEM_COMMA  ","
        (prefix, 0xBE, Action::ExpelWindowFromColumn),   // VK_OEM_PERIOD "."
        (prefix, 0x45, Action::ExpandColumnToAvailable), // E
        (shift_prefix, 0x46, Action::MaximizeColumn),    // Shift+F
        (shift_prefix, 0x4C, Action::GrowColumnWidth), // Shift+L (overrides MoveToMonitorRight above)
        (shift_prefix, 0x48, Action::ShrinkColumnWidth), // Shift+H (overrides MoveToMonitorLeft above)
        (shift_prefix, 0x4B, Action::GrowTileHeight),    // Shift+K
        (shift_prefix, 0x4A, Action::ShrinkTileHeight),  // Shift+J
        // Workspace swap (reorder the workspace stack) — niri-parity
        // `Mod+Ctrl+Page_Up/Down`.  Our default prefix is Ctrl+Alt, so the
        // shifted variant naturally lands on `Ctrl+Alt+Shift+Page_Up/Down`,
        // which is what we used to dedicate to move-column-to-monitor.  The
        // column-to-monitor chords move to Ctrl+Alt+Shift+Comma/Period below.
        (shift_prefix, 0x21, Action::MoveWorkspaceUp), // Shift+PageUp
        (shift_prefix, 0x22, Action::MoveWorkspaceDown), // Shift+PageDown
        // Niri parity: move focused column vertically between workspaces.
        // Ctrl+Alt+Shift+Up/Down (VK_UP=0x26, VK_DOWN=0x28).
        (shift_prefix, 0x26, Action::MoveColumnToWorkspaceUp),
        (shift_prefix, 0x28, Action::MoveColumnToWorkspaceDown),
        // Move column to monitor moves to Ctrl+Alt+Shift+Comma/Period
        // (VK_OEM_COMMA=0xBC, VK_OEM_PERIOD=0xBE) to free up Page_Up/Down
        // for workspace swap per the niri default.
        (shift_prefix, 0xBC, Action::MoveColumnToMonitorLeft),
        (shift_prefix, 0xBE, Action::MoveColumnToMonitorRight),
        // Round-4: Mod+S → ToggleSticky (pin window to all workspaces)
        (prefix, 0x53, Action::ToggleSticky),
        // Cheatsheet overlay: prefix+Shift+/ = prefix+?  (VK_OEM_2 = 0xBF)
        (shift_prefix, 0xBF, Action::ShowKeyBindings),
        // niri keymap: first / last column, column moves to the ends of the strip,
        // consume-or-expel and the window moves inside a column.
        (prefix, 0x24, Action::FocusColumnFirst),        // Home
        (prefix, 0x23, Action::FocusColumnLast),         // End
        (shift_prefix, 0x24, Action::MoveColumnToFirst), // Shift+Home
        (shift_prefix, 0x23, Action::MoveColumnToLast),  // Shift+End
        (shift_prefix, 0xDB, Action::ConsumeOrExpelWindowLeft), // Shift+[
        (shift_prefix, 0xDD, Action::ConsumeOrExpelWindowRight), // Shift+]
        (prefix, 0x4B, Action::MoveWindowUp),            // K
        (prefix, 0x4A, Action::MoveWindowDown),          // J
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

/// niri's per-bind options that change when a hotkey fires: `repeat=false` (holding the key
/// fires the action once) and `cooldown-ms=` (after a fire the bind is ignored for that long).
#[derive(Debug, Clone, Default)]
struct BindOptions {
    /// False: the key's auto-repeat does not fire the action again.
    no_repeat: bool,
    cooldown: Option<std::time::Duration>,
    /// `hotkey-overlay-title=`: `Some(Some(text))` replaces the action name in the key-bindings
    /// overlay, `Some(None)` (`null`) hides the bind from it.
    title: Option<Option<String>>,
}

/// Whether a hotkey press fires its action: `repeat` says the key is auto-repeating, `last`
/// is when the bind last fired.  Pure so the rules can be tested.
fn bind_may_fire(
    options: &BindOptions,
    is_repeat: bool,
    last: Option<std::time::Instant>,
    now: std::time::Instant,
) -> bool {
    if options.no_repeat && is_repeat {
        return false;
    }
    if let (Some(cooldown), Some(last)) = (options.cooldown, last) {
        if now.saturating_duration_since(last) < cooldown {
            return false;
        }
    }
    true
}

/// Build hotkey list from config binds, optionally extending the built-in
/// defaults so newly-shipped actions stay available to users with stale
/// configs.  The second value holds the options of the binds that have any.
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
fn build_hotkey_list(
    engine: &Arc<parking_lot::RwLock<TilingEngine>>,
) -> (Vec<(u32, u32, Action)>, HashMap<(u32, u32), BindOptions>) {
    let terminal = resolve_terminal_action(engine);
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
    let defaults = default_hotkeys(prefix, shift_prefix, terminal.clone());
    let default_count = defaults.len();

    if config_binds.is_empty() {
        info!(
            "Hotkeys: {} from defaults, 0 from config (0 overridden)",
            default_count
        );
        return (defaults, HashMap::new());
    }

    // Parse every config bind once.
    let mut parsed_config: Vec<(u32, u32, Action)> = Vec::with_capacity(config_binds.len());
    let mut options: HashMap<(u32, u32), BindOptions> = HashMap::new();
    for bind in config_binds {
        match bind_to_hotkey(bind, prefix) {
            Ok(row) => {
                if !bind.repeat || bind.cooldown_ms.is_some() || bind.hotkey_overlay_title.is_some()
                {
                    options.insert(
                        (row.0, row.1),
                        BindOptions {
                            no_repeat: !bind.repeat,
                            cooldown: bind
                                .cooldown_ms
                                .map(|ms| std::time::Duration::from_millis(ms as u64)),
                            title: bind.hotkey_overlay_title.clone(),
                        },
                    );
                }
                parsed_config.push(row)
            }
            Err(why) => warn!("Config bind: {}, skipping", why),
        }
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
        return (default_hotkeys(prefix, shift_prefix, terminal), HashMap::new());
    }
    (merged, options)
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

/// A bare `Escape` bound to the overview toggle.  The low-level keyboard hook
/// (`input::low_level_hook`) already delivers Escape to overview and resize
/// mode *without consuming it*; registering it with `RegisterHotKey` as well
/// would take Escape away from every application on the desktop (a registered
/// hotkey is delivered to wiri only, never to the focused window), and made
/// Escape unusable everywhere while wiri ran.
fn is_overview_escape_binding(mods: u32, vk: u32, action: &Action) -> bool {
    mods == 0 && vk == 0x1B && matches!(action, Action::OverviewToggle | Action::OverviewExit)
}

/// Register a list of hotkeys with Windows, returning the IDs and the action map
/// of the ones that Windows accepted.
fn register_hotkeys(hwnd: HWND, hotkeys: &[(u32, u32, Action)]) -> (Vec<i32>, HashMap<i32, Action>) {
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
        if is_overview_escape_binding(*mods, *vk, action) {
            info!(
                "SKIP hotkey id={} Escape {:?} — handled by the keyboard hook (a registered \
                 Escape would be swallowed in every application)",
                id, action,
            );
            continue;
        }
        let result = unsafe { RegisterHotKey(hwnd, id, HOT_KEY_MODIFIERS(*mods), *vk) };
        if result.is_ok() {
            registered_ids.push(id);
            info!(
                "OK hotkey id={} mods=0x{:X} vk=0x{:02X} {:?}",
                id, mods, vk, action
            );
        } else {
            let err = std::io::Error::last_os_error();
            warn!(
                "FAIL hotkey id={} mods=0x{:X} vk=0x{:02X} {:?} os_err={}",
                id, mods, vk, action, err
            );
        }
    }
    info!(
        "Hotkeys: {}/{} registered",
        registered_ids.len(),
        hotkeys.len()
    );

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
        unsafe {
            let _ = UnregisterHotKey(hwnd, *id);
        }
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
    /// niri's `hotkey-overlay-title=`: `Some(Some(text))` is shown in the key-bindings overlay
    /// instead of the action name, `Some(None)` hides the bind from it.
    pub title: Option<Option<String>>,
}

/// Refresh the global REGISTERED_BINDINGS snapshot after a (re-)registration
/// pass.  Stores only bindings that Windows actually accepted (i.e. whose ID
/// appears in `registered_ids`) so users get a true picture of what's live.
fn update_current_bindings_snapshot(
    hotkeys: &[(u32, u32, Action)],
    registered_ids: &[i32],
    options: &HashMap<(u32, u32), BindOptions>,
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
            title: options.get(&(*mods, *vk)).and_then(|o| o.title.clone()),
        });
    }
    *REGISTERED_BINDINGS.lock() = snap;
}

/// The rows of the key-bindings overlay: the chord and what it does, the action's name unless
/// the bind has a `hotkey-overlay-title`, and no row for a bind whose title is `null`.
fn overlay_rows(bindings: &[RegisteredBinding]) -> Vec<(String, String)> {
    bindings
        .iter()
        .filter_map(|b| {
            let chord = crate::ipc::server::format_chord(b.modifiers, b.vk_code);
            match &b.title {
                Some(None) => None,
                Some(Some(text)) => Some((chord, text.clone())),
                None => Some((chord, b.action.clone())),
            }
        })
        .collect()
}

/// The options of the registered binds that have any, by hotkey id (with the virtual key
/// that auto-repeat is detected on).
fn options_by_id(
    hotkeys: &[(u32, u32, Action)],
    registered_ids: &[i32],
    options: &HashMap<(u32, u32), BindOptions>,
) -> HashMap<i32, (u32, BindOptions)> {
    let base_id = 32768i32;
    let mut by_id = HashMap::new();
    for (i, (mods, vk, _)) in hotkeys.iter().enumerate() {
        let id = base_id + i as i32;
        if !registered_ids.contains(&id) {
            continue;
        }
        if let Some(o) = options.get(&(*mods, *vk)) {
            by_id.insert(id, (*vk, o.clone()));
        }
    }
    by_id
}

/// Return a clone of every hotkey currently registered with Windows.
/// Empty when the hotkey thread hasn't started or registration failed.
pub fn current_bindings() -> Vec<RegisteredBinding> {
    REGISTERED_BINDINGS.lock().clone()
}

pub struct MessageLoop {
    running: Arc<Mutex<bool>>,
    quit_tx: Arc<Mutex<Option<mpsc::Sender<()>>>>,
}

impl MessageLoop {
    pub fn new() -> Result<Self> {
        let running = Arc::new(Mutex::new(false));
        Ok(Self {
            running,
            quit_tx: Arc::new(Mutex::new(None)),
        })
    }

    pub fn set_quit_sender(&self, tx: mpsc::Sender<()>) {
        *self.quit_tx.lock() = Some(tx);
    }

    pub fn start_hotkey_loop(
        &self,
        engine: Arc<parking_lot::RwLock<TilingEngine>>,
        backend_handle: BackendHandle,
    ) {
        let running = self.running.clone();
        let quit_tx = self.quit_tx.clone();
        let (keyboard_action_tx, keyboard_action_rx) = mpsc::channel();
        crate::input::set_keyboard_action_sender(keyboard_action_tx);
        if let Err(e) = crate::input::start_keyboard_hook() {
            warn!("Double-Alt keyboard hook disabled: {}", e);
        }
        {
            let engine = engine.clone();
            let backend_handle = backend_handle.clone();
            let quit_tx = quit_tx.clone();
            std::thread::spawn(move || {
                while let Ok(action) = keyboard_action_rx.recv() {
                    // Every Escape press in every application arrives here (the hook
                    // reports it without consuming it), so this is not worth an INFO line.
                    debug!("KEYHOOK -> {:?}", action);
                    execute_action(&action, &engine, &backend_handle, &quit_tx);
                }
            });
        }
        std::thread::spawn(move || {
            Self::run_message_loop(running, engine, backend_handle, quit_tx);
        });
    }

    fn run_message_loop(
        running: Arc<Mutex<bool>>,
        engine: Arc<parking_lot::RwLock<TilingEngine>>,
        backend_handle: BackendHandle,
        quit_tx: Arc<Mutex<Option<mpsc::Sender<()>>>>,
    ) {
        let thread_id = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };

        // Create a hidden window to own the hotkey registrations
        let class_name: Vec<u16> = "WiriHotkeyWnd\0".encode_utf16().collect();
        let wnd_class = WNDCLASSW {
            style: WNDCLASS_STYLES(0),
            lpfnWndProc: Some(Self::window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: unsafe { GetModuleHandleW(None) }
                .ok()
                .map(|h| h.into())
                .unwrap_or_default(),
            hIcon: windows::Win32::UI::WindowsAndMessaging::HICON::default(),
            hCursor: windows::Win32::UI::WindowsAndMessaging::HCURSOR::default(),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH::default(),
            lpszMenuName: windows::core::PCWSTR::null(),
            lpszClassName: windows::core::PCWSTR(class_name.as_ptr()),
        };
        unsafe {
            let _ = RegisterClassW(&wnd_class);
        }

        let window_name: Vec<u16> = "WiriHotkeyWindow\0".encode_utf16().collect();
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                windows::core::PCWSTR(class_name.as_ptr()),
                windows::core::PCWSTR(window_name.as_ptr()),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
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
        let (current_hotkeys, bind_options) = build_hotkey_list(&engine);
        let (mut registered_ids, mut action_map) = register_hotkeys(hwnd, &current_hotkeys);
        // Snapshot mirror for `current_bindings()` queries from IPC.
        update_current_bindings_snapshot(&current_hotkeys, &registered_ids, &bind_options);
        // niri's `repeat=false` / `cooldown-ms=` binds, by hotkey id, and when each last fired.
        let mut id_options = options_by_id(&current_hotkeys, &registered_ids, &bind_options);
        let mut last_fire: HashMap<i32, std::time::Instant> = HashMap::new();

        *running.lock() = true;
        info!(
            "Hotkey message loop started on thread {:?}",
            std::thread::current().id()
        );

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
                    let (new_hotkeys, new_options) = build_hotkey_list(&engine);
                    let (new_ids, new_map) = register_hotkeys(hwnd, &new_hotkeys);
                    registered_ids = new_ids;
                    action_map = new_map;
                    update_current_bindings_snapshot(&new_hotkeys, &registered_ids, &new_options);
                    id_options = options_by_id(&new_hotkeys, &registered_ids, &new_options);
                    last_fire.clear();
                    info!("Hotkey reload complete");
                    continue;
                }
                if msg.message == 0x0312 {
                    // WM_HOTKEY
                    let hotkey_id = msg.wParam.0 as i32;
                    if let Some(action) = action_map.get(&hotkey_id) {
                        if let Some((vk, options)) = id_options.get(&hotkey_id) {
                            let now = std::time::Instant::now();
                            let is_repeat = crate::input::last_press_was_repeat(*vk);
                            if !bind_may_fire(options, is_repeat, last_fire.get(&hotkey_id).copied(), now) {
                                debug!("HOTKEY id={} ignored (key repeat or cooldown)", hotkey_id);
                                continue;
                            }
                            last_fire.insert(hotkey_id, now);
                        }
                        info!("HOTKEY id={} -> {:?}", hotkey_id, action);
                        execute_action(action, &engine, &backend_handle, &quit_tx);
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
                    execute_action(&action, &engine, &backend_handle, &quit_tx);
                }
            }
        }

        unregister_hotkeys(hwnd, &registered_ids);
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        *HOTKEY_THREAD.lock() = None;
        *running.lock() = false;
        info!("Hotkey message loop exited");
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
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
                let _ = PostThreadMessageW(s.thread_id, WM_RELOAD_HOTKEYS, WPARAM(0), LPARAM(0));
            }
            info!("Sent WM_RELOAD_HOTKEYS to hotkey thread");
        }
    }

    pub fn stop(&self) {
        *self.running.lock() = false;
        crate::input::stop_keyboard_hook();
        crate::input::clear_keyboard_action_sender();
        let state = HOTKEY_THREAD.lock();
        if let Some(s) = &*state {
            unsafe {
                let _ = PostThreadMessageW(s.thread_id, WM_STOP_MESSAGE_LOOP, WPARAM(0), LPARAM(0));
            }
        }
    }

    pub fn is_running(&self) -> bool {
        *self.running.lock()
    }

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
    fn drop(&mut self) {
        self.stop();
    }
}

impl Default for MessageLoop {
    fn default() -> Self {
        Self::new().expect("Failed to create message loop")
    }
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
    quit_tx: &Arc<Mutex<Option<mpsc::Sender<()>>>>,
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
    perform_action(action, engine, backend, quit_tx);
}

/// Run one action against the engine.  Hotkeys reach it through
/// [`execute_action`] (which first drops key-repeat duplicates); the IPC
/// `run_action` request calls it directly, so `wiri-ctl action <name>` behaves
/// exactly like pressing the bound key.
pub(crate) fn perform_action(
    action: &Action,
    engine: &Arc<parking_lot::RwLock<TilingEngine>>,
    backend: &BackendHandle,
    quit_tx: &Arc<Mutex<Option<mpsc::Sender<()>>>>,
) {
    // The window the action acts on is the one the user is looking at: the OS foreground window
    // (or the application window that owns it), a floating window as well as a tile.  The
    // engine's own focus is only the fallback.  `close-window` refuses to guess when an
    // unmanaged window (the Start menu, an elevated program) has the focus.
    let target = match action {
        Action::Quit
        | Action::Spawn(_)
        | Action::SpawnCmd(_)
        | Action::Screenshot
        | Action::WindowScreenshot
        | Action::ScreenshotWindow
        | Action::ShowKeyBindings => None,
        Action::CloseWindow => engine.write().resolve_action_target_strict(backend),
        _ => engine.write().resolve_action_target(backend),
    };

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
                engine
                    .write()
                    .resize_focused_tile_height_by_percent(-5, backend);
                engine.write().apply_all(backend);
                return;
            }
            Action::FocusDown => {
                engine
                    .write()
                    .resize_focused_tile_height_by_percent(5, backend);
                engine.write().apply_all(backend);
                return;
            }
            Action::OverviewToggle | Action::OverviewExit => {
                // Escape is the canonical "exit resize mode" chord. Swallow
                // the action so overview doesn't open.
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
        Action::MoveColumnLeft => match float_of(target) {
            Some(window) => engine.write().float_move_by(window, -FLOAT_STEP_PX, 0, backend),
            None => engine.write().move_column(ScrollDirection::Left, backend),
        },
        Action::MoveColumnRight => match float_of(target) {
            Some(window) => engine.write().float_move_by(window, FLOAT_STEP_PX, 0, backend),
            None => engine.write().move_column(ScrollDirection::Right, backend),
        },
        Action::CloseWindow => match target {
            Some(t) => engine.write().close_target(t),
            None => info!("close-window: the focused window is not managed by wiri; nothing closed"),
        },
        Action::Spawn(argv) => {
            spawn_process(argv);
        }
        Action::SpawnCmd(cmd) => {
            // niri-style `spawn-cmd "<command>"` — route through the
            // Windows shell so cmd-builtins (`start`, `dir`, redirection,
            // env-var expansion) and PowerShell one-liners work as the
            // user typed them.  Equivalent to `cmd.exe /C "<command>"`.
            spawn_shell_command(cmd);
        }
        Action::ToggleFullscreen => {
            if let Some(t) = target {
                engine.write().toggle_fullscreen_for(t.window, backend);
            }
        }
        Action::Quit => {
            if let Some(tx) = quit_tx.lock().as_ref() {
                let _ = tx.send(());
            } else {
                warn!("Quit hotkey ignored: no quit channel configured");
            }
        }
        Action::FocusWorkspace(number) => {
            // `Ctrl+Alt+2` is the second workspace of the monitor (niri's index
            // addressing), whatever its id is.
            engine.write().focus_workspace_number(*number, backend);
        }
        Action::FocusWorkspaceLast => {
            engine.write().focus_workspace_last_visited(backend);
        }
        Action::ScrollLeft => {
            engine.write().scroll(ScrollDirection::Left, backend);
        }
        Action::ScrollRight => {
            engine.write().scroll(ScrollDirection::Right, backend);
        }
        Action::ToggleFloating => {
            if let Some(t) = target {
                engine.write().toggle_floating_for(t.window, backend);
            }
        }
        Action::OverviewToggle => {
            engine.write().toggle_overview(backend);
        }
        Action::OverviewExit => {
            if engine.read().is_overview() {
                engine.write().toggle_overview(backend);
            }
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
                unsafe {
                    let _ = ShowWindow(fg, SW_MAXIMIZE);
                }
                info!("Maximize: hwnd={:?}", fg);
            } else {
                warn!("Maximize: no foreground window");
            }
        }
        Action::Minimize => {
            // Minimize the currently focused window
            let fg = unsafe { GetForegroundWindow() };
            if !fg.0.is_null() {
                unsafe {
                    let _ = ShowWindow(fg, SW_MINIMIZE);
                }
                info!("Minimize: hwnd={:?}", fg);
            } else {
                warn!("Minimize: no foreground window");
            }
        }
        Action::CenterWindow => match float_of(target) {
            Some(window) => engine.write().float_center(window, backend),
            None => {
                engine.write().center_focused_column(backend);
                engine.write().apply_all(backend);
            }
        },
        Action::SwitchMonitor => {
            // Generic "switch monitor" — cycle focus to the next monitor in
            // x-order, wrapping around. Distinct from MoveToMonitorLeft/Right,
            // which move the focused window across monitors.
            let mut engine_w = engine.write();
            let order: Vec<_> = {
                let mut v: Vec<_> = engine_w
                    .monitors()
                    .iter()
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
                Some(oid) => order
                    .iter()
                    .position(|(_, o)| *o == oid)
                    .map(|i| (i + 1) % order.len())
                    .unwrap_or(0),
                None => 0,
            };
            let target = order[next_idx].1;
            engine_w.set_focused_output(target);
            engine_w.apply_all(backend);
            info!("SwitchMonitor: focused {:?}", target);
        }
        Action::MoveWorkspace(target_number) => {
            // Move the focused window to workspace number `target_number` (no switch).
            let workspace = engine.read().workspace_id_for_number(*target_number);
            match float_of(target) {
                Some(window) => engine.write().float_move_to_workspace(window, workspace, backend),
                None => engine.write().move_window_to_workspace(workspace, backend),
            }
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
        Action::MoveToWorkspace(number) => {
            let workspace = engine.read().workspace_id_for_number(*number);
            match float_of(target) {
                Some(window) => engine.write().float_move_to_workspace(window, workspace, backend),
                None => engine.write().move_window_to_workspace(workspace, backend),
            }
            engine.write().apply_all(backend);
        }
        Action::MoveToMonitorLeft => {
            engine
                .write()
                .move_window_to_monitor(crate::layout::ScrollDirection::Left, backend);
            engine.write().apply_all(backend);
        }
        Action::MoveToMonitorRight => {
            engine
                .write()
                .move_window_to_monitor(crate::layout::ScrollDirection::Right, backend);
            engine.write().apply_all(backend);
        }
        Action::CenterColumn => {
            match float_of(target) {
                Some(window) => engine.write().float_center(window, backend),
                None => engine.write().center_focused_column(backend),
            }
            engine.write().apply_all(backend);
        }
        Action::ColumnWidthPresetCycle | Action::ColumnWidthPresetCycleBack => {
            // niri's presets: a third, a half and two thirds of the view.
            let forwards = matches!(action, Action::ColumnWidthPresetCycle);
            match float_of(target) {
                Some(window) => engine.write().float_cycle_width(window, forwards, backend),
                None => engine.write().switch_preset_column_width(forwards, backend),
            }
        }
        // wiri's fractions are niri proportions of the view.
        Action::ColumnWidthPresetQuarter
        | Action::ColumnWidthPresetThird
        | Action::ColumnWidthPresetHalf
        | Action::ColumnWidthPresetTwoThirds
        | Action::ColumnWidthPresetThreeQuarters
        | Action::ColumnWidthPresetFull => {
            let milli_percent = match action {
                Action::ColumnWidthPresetQuarter => 25_000,
                Action::ColumnWidthPresetThird => 33_333,
                Action::ColumnWidthPresetHalf => 50_000,
                Action::ColumnWidthPresetTwoThirds => 66_667,
                Action::ColumnWidthPresetThreeQuarters => 75_000,
                _ => 100_000,
            };
            change_size(
                engine,
                backend,
                target,
                crate::layout::SizeAxis::Width,
                SizeChange::SetProportion(milli_percent),
            );
        }
        Action::SetColumnWidth(change) => {
            change_size(engine, backend, target, crate::layout::SizeAxis::Width, *change);
        }
        Action::SetWindowHeight(change) => {
            change_size(engine, backend, target, crate::layout::SizeAxis::Height, *change);
        }
        Action::ResetWindowHeight => {
            if float_of(target).is_none() {
                engine.write().reset_window_height(backend);
            }
        }
        Action::MoveWindowToFloating | Action::MoveWindowToTiling => {
            if let Some(t) = target {
                engine.write().set_window_floating(
                    t.window,
                    matches!(action, Action::MoveWindowToFloating),
                    backend,
                );
            }
        }
        Action::FocusMonitorLeft | Action::FocusMonitorRight => {
            let direction = if matches!(action, Action::FocusMonitorLeft) {
                ScrollDirection::Left
            } else {
                ScrollDirection::Right
            };
            engine.write().focus_monitor_toward(direction, backend);
        }
        Action::OverviewOpen => {
            if !engine.read().is_overview() {
                engine.write().toggle_overview(backend);
            }
        }
        Action::ResizeColumnLeft => {
            match float_of(target) {
                Some(window) => engine.write().float_resize_by(window, -100, 0, backend),
                None => engine.write().resize_focused_column_by(-100, backend),
            }
            engine.write().apply_all(backend);
        }
        Action::ResizeColumnRight => {
            match float_of(target) {
                Some(window) => engine.write().float_resize_by(window, 100, 0, backend),
                None => engine.write().resize_focused_column_by(100, backend),
            }
            engine.write().apply_all(backend);
        }
        // niri `focus-workspace-down` / `-up`: the workspace below / above, no wrapping.
        Action::FocusWorkspaceNext => {
            engine
                .write()
                .focus_workspace_relative(crate::layout::WorkspaceDirection::Next, backend);
        }
        Action::FocusWorkspacePrevious => {
            engine
                .write()
                .focus_workspace_relative(crate::layout::WorkspaceDirection::Previous, backend);
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
        }
        Action::SetAutoTileThreshold(t) => {
            engine.write().set_auto_tile_threshold(*t);
            engine.write().apply_all(backend);
        }
        Action::Screenshot => {
            take_screenshot();
        }
        // The column operations belong to the strip: a floating window has no column
        // (niri does nothing for it either).
        Action::ConsumeWindowIntoColumn => {
            if float_of(target).is_none() {
                engine.write().consume_window_into_column(backend);
            }
        }
        Action::ExpelWindowFromColumn => {
            if float_of(target).is_none() {
                engine.write().expel_window_from_column(backend);
            }
        }
        Action::ConsumeOrExpelWindowLeft => {
            if float_of(target).is_none() {
                engine.write().consume_or_expel_window_left(backend);
            }
        }
        Action::ConsumeOrExpelWindowRight => {
            if float_of(target).is_none() {
                engine.write().consume_or_expel_window_right(backend);
            }
        }
        Action::MoveWindowUp => match float_of(target) {
            Some(window) => engine.write().float_move_by(window, 0, -FLOAT_STEP_PX, backend),
            None => engine.write().move_window_in_column(-1, backend),
        },
        Action::MoveWindowDown => match float_of(target) {
            Some(window) => engine.write().float_move_by(window, 0, FLOAT_STEP_PX, backend),
            None => engine.write().move_window_in_column(1, backend),
        },
        Action::FocusColumnFirst => {
            engine.write().focus_column_first(backend);
        }
        Action::FocusColumnLast => {
            engine.write().focus_column_last(backend);
        }
        Action::MoveColumnToFirst => {
            if float_of(target).is_none() {
                engine.write().move_column_to_edge(false, backend);
            }
        }
        Action::MoveColumnToLast => {
            if float_of(target).is_none() {
                engine.write().move_column_to_edge(true, backend);
            }
        }
        Action::ExpandColumnToAvailable => {
            if float_of(target).is_none() {
                engine.write().expand_column_to_available(backend);
            }
        }
        Action::MaximizeColumn => {
            if float_of(target).is_none() {
                engine.write().toggle_maximize_focused_column(backend);
            }
        }
        Action::GrowColumnWidth => {
            match float_of(target) {
                Some(window) => engine.write().float_resize_percent(window, 5, 0, backend),
                None => engine.write().resize_focused_column_by_percent(5, backend),
            }
            engine.write().apply_all(backend);
        }
        Action::ShrinkColumnWidth => {
            match float_of(target) {
                Some(window) => engine.write().float_resize_percent(window, -5, 0, backend),
                None => engine.write().resize_focused_column_by_percent(-5, backend),
            }
            engine.write().apply_all(backend);
        }
        Action::GrowTileHeight => {
            match float_of(target) {
                Some(window) => engine.write().float_resize_percent(window, 0, 5, backend),
                None => engine
                    .write()
                    .resize_focused_tile_height_by_percent(5, backend),
            }
            engine.write().apply_all(backend);
        }
        Action::ShrinkTileHeight => {
            match float_of(target) {
                Some(window) => engine.write().float_resize_percent(window, 0, -5, backend),
                None => engine
                    .write()
                    .resize_focused_tile_height_by_percent(-5, backend),
            }
            engine.write().apply_all(backend);
        }
        Action::MoveColumnToMonitorLeft => {
            engine
                .write()
                .move_column_to_monitor(crate::layout::ScrollDirection::Left, backend);
            engine.write().apply_all(backend);
        }
        Action::MoveColumnToMonitorRight => {
            engine
                .write()
                .move_column_to_monitor(crate::layout::ScrollDirection::Right, backend);
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
        Action::ToggleSticky => {
            engine.write().toggle_sticky(backend);
            engine.write().apply_all(backend);
        }
        Action::SetWorkspaceLayout(mode) => {
            let mut eng = engine.write();
            match eng.set_workspace_layout(&mode) {
                Ok(()) => eng.apply_all(backend),
                Err(err) => warn!("SetWorkspaceLayout({}): {}", mode, err),
            }
        }
        Action::RenameWorkspace(name) => {
            // Rename the currently-focused workspace.
            let id = {
                let eng = engine.read();
                eng.focused_output()
                    .and_then(|oid| eng.monitors().get(&oid))
                    .map(|m| m.active_workspace_id())
                    .unwrap_or(0)
            };
            if let Err(e) = engine.write().rename_workspace(id, name.as_str()) {
                warn!("rename_workspace({:?}): {}", name, e);
            }
        }
        Action::ScreenshotWindow => {
            take_window_screenshot();
        }
        Action::ToggleBackdrop => {
            engine.write().toggle_backdrop_cycle(backend);
        }
        Action::ShowKeyBindings => {
            // Build a binding row list from the live REGISTERED_BINDINGS snapshot
            // (what Windows actually accepted) and toggle the cheatsheet overlay.
            if let Some(cs) = crate::overlay::bindings_cheatsheet::get_global_cheatsheet() {
                let raw = current_bindings();
                let rows: Vec<(String, String)> = if raw.is_empty() {
                    // Engine not yet registered — show hardcoded defaults.
                    let prefix_name = engine
                        .read()
                        .full_config()
                        .map(|c| mod_prefix_display_name(&c.input.mod_key))
                        .unwrap_or_else(|| "Ctrl+Alt".to_string());
                    crate::overlay::bindings_cheatsheet::default_bindings_table(&prefix_name)
                } else {
                    overlay_rows(&raw)
                };
                info!("ShowKeyBindings: toggling cheatsheet ({} rows)", rows.len());
                cs.toggle(rows);
            } else {
                info!("ShowKeyBindings: cheatsheet overlay not installed (main.rs wiring missing)");
            }
        }
    }
}

/// Apply a niri size change to the window an action targets: a floating window is
/// resized (proportions are of its monitor's work area), a tile changes the width of its
/// column or its own height inside the column.
fn change_size(
    engine: &Arc<parking_lot::RwLock<TilingEngine>>,
    backend: &BackendHandle,
    target: Option<crate::layout::ActionTarget>,
    axis: crate::layout::SizeAxis,
    change: SizeChange,
) {
    match (float_of(target), axis) {
        (Some(window), axis) => {
            engine
                .write()
                .float_apply_size_change(window, axis, change, backend)
        }
        (None, crate::layout::SizeAxis::Width) => engine.write().set_column_width_by(change, backend),
        (None, crate::layout::SizeAxis::Height) => {
            engine.write().set_window_height_by(change, backend)
        }
    }
}

/// How far a floating window moves for one `move-column-left` / `move-column-right` /
/// `move-window-up` / `move-window-down` (niri nudges by 50).
const FLOAT_STEP_PX: i32 = 50;

/// The floating window an action targets, if it targets one.
fn float_of(target: Option<crate::layout::ActionTarget>) -> Option<crate::utils::WindowId> {
    target.filter(|t| t.floating).map(|t| t.window)
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

/// Run a program with its arguments intact (`Action::Spawn`).  Goes through the
/// one shared spawner, so bind, `spawn-at-startup` and IPC `spawn` all quote and
/// search the PATH the same way.
fn spawn_process(argv: &[String]) {
    match crate::hooks::spawner::spawn_argv(argv) {
        Ok(pid) => info!("Spawned {:?} (pid {})", argv, pid),
        Err(e) => warn!("Spawn failed for {:?}: {}", argv, e),
    }
}

/// Launch a single command line through `cmd.exe /d /s /c "<command>"`.
///
/// Used by `Action::SpawnCmd`.  The shell expands env vars (`%USERPROFILE%`,
/// etc.), recognises built-ins (`start`, redirection, `&&`) and resolves PATH
/// lookups for callers that want those.  The shell itself runs without a
/// console window; whatever it starts shows its own.
fn spawn_shell_command(cmd: &str) {
    if cmd.trim().is_empty() {
        return;
    }
    match crate::hooks::spawner::spawn_shell(cmd) {
        Ok(pid) => info!("SpawnCmd (cmd.exe /c): {} (pid {})", cmd, pid),
        Err(e) => warn!("SpawnCmd failed for {:?}: {}", cmd, e),
    }
}

/// Convert a `mod-key` config string to a human-readable prefix label such as
/// `"Ctrl+Alt"`.  Used by `ShowKeyBindings` to build the fallback default table
/// when no live registered-bindings snapshot is available.
fn mod_prefix_display_name(name: &str) -> String {
    match name.trim().to_lowercase().as_str() {
        "alt" => "Alt".to_string(),
        "ctrl" | "control" => "Ctrl".to_string(),
        "super" | "win" | "meta" => "Win".to_string(),
        "win-alt" | "super-alt" | "meta-alt" => "Win+Alt".to_string(),
        "ctrl-shift" => "Ctrl+Shift".to_string(),
        _ => "Ctrl+Alt".to_string(),
    }
}

/// What `Mod+Enter` launches when the config does not say: Windows Terminal,
/// falling back to PowerShell, so the user is never dropped into a bare cmd.exe.
const DEFAULT_TERMINAL_COMMAND: &str = "start wt.exe || start powershell.exe";

/// Resolve the user's preferred "terminal" launch action.
///
/// Looked up from the engine's full config in this order:
/// 1. The first `spawn` / `spawn-cmd` bind (so a user bind
///    `Mod+Enter { spawn "wt.exe" }` is also what the built-in default runs).
/// 2. The first non-empty entry in `spawn_at_startup` (rare, but if a user
///    has only declared their terminal there we still find it).
/// 3. [`DEFAULT_TERMINAL_COMMAND`].
fn resolve_terminal_action(engine: &Arc<parking_lot::RwLock<TilingEngine>>) -> Action {
    let eng = engine.read();
    terminal_action_from_config(eng.full_config())
}

/// Pure core of [`resolve_terminal_action`].
fn terminal_action_from_config(cfg: Option<&crate::config::Config>) -> Action {
    if let Some(cfg) = cfg {
        for bind in &cfg.binds.hotkeys {
            match bind.parse_action() {
                Some(Action::Spawn(argv)) if !argv.is_empty() => return Action::Spawn(argv),
                Some(Action::SpawnCmd(cmd)) if !cmd.trim().is_empty() => {
                    return Action::SpawnCmd(cmd)
                }
                _ => {}
            }
        }
        if let Some(entry) = cfg.spawn_at_startup.iter().find(|e| !e.is_empty()) {
            return Action::Spawn(entry.clone());
        }
    }
    Action::SpawnCmd(DEFAULT_TERMINAL_COMMAND.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(mods: u32, vk: u32, label: &str) -> (u32, u32, Action) {
        (mods, vk, Action::Spawn(vec![label.to_string()]))
    }

    fn spawn_of(label: &str) -> Action {
        Action::Spawn(vec![label.to_string()])
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
        assert_eq!(merged[0].2, spawn_of("default-left"));
        assert_eq!(merged[1].2, spawn_of("default-right"));
        assert_eq!(merged[2].2, spawn_of("config-alt-p"));
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
        assert_eq!(merged[0].2, spawn_of("config-left"));
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
        assert_eq!(merged[0].2, spawn_of("config-alt-p"));
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

    #[test]
    fn bare_escape_for_overview_is_left_to_the_keyboard_hook() {
        // The shipped config binds `Escape` to the overview toggle.  Registering it
        // with RegisterHotKey makes Escape unusable in every other application.
        assert!(is_overview_escape_binding(0, 0x1B, &Action::OverviewToggle));
        assert!(is_overview_escape_binding(0, 0x1B, &Action::OverviewExit));
        // A modified Escape (Ctrl+Alt+Escape) does not steal the plain key.
        assert!(!is_overview_escape_binding(MOD_CTRL | MOD_ALT, 0x1B, &Action::OverviewToggle));
        // Other actions on a bare Escape are the user's explicit choice.
        assert!(!is_overview_escape_binding(0, 0x1B, &Action::CloseWindow));
        // Another bare key bound to the overview toggle is not Escape.
        assert!(!is_overview_escape_binding(0, 0x20, &Action::OverviewToggle));
    }

    /// Chords Windows keeps for itself.  `RegisterHotKey` refuses them (os error 1409) or the
    /// system takes them first, so a default bind on one logged a warning at every start and
    /// never fired.
    const RESERVED_BY_WINDOWS: &[(u32, u32, &str)] = &[
        (MOD_CTRL | MOD_ALT, 0x09, "Ctrl+Alt+Tab (task switcher)"),
        (MOD_CTRL | MOD_ALT, 0x2E, "Ctrl+Alt+Delete"),
        (MOD_CTRL | MOD_SHIFT, 0x1B, "Ctrl+Shift+Esc (task manager)"),
        (MOD_ALT, 0x09, "Alt+Tab"),
        (MOD_ALT, 0x73, "Alt+F4"),
        (MOD_WIN, 0x4C, "Win+L"),
    ];

    #[test]
    fn no_default_hotkey_is_a_chord_windows_keeps_for_itself() {
        let terminal = Action::SpawnCmd("t".to_string());
        for (prefix, shift_prefix) in [
            (MOD_CTRL | MOD_ALT, MOD_CTRL | MOD_ALT | MOD_SHIFT),
            (MOD_WIN | MOD_ALT, MOD_WIN | MOD_ALT | MOD_SHIFT),
            (MOD_ALT, MOD_ALT | MOD_SHIFT),
        ] {
            for (mods, vk, action) in default_hotkeys(prefix, shift_prefix, terminal.clone()) {
                for (reserved_mods, reserved_vk, label) in RESERVED_BY_WINDOWS {
                    assert!(
                        !(mods == *reserved_mods && vk == *reserved_vk),
                        "the default bind for {:?} is {}, which Windows keeps for itself",
                        action,
                        label
                    );
                }
            }
        }
    }

    #[test]
    fn the_default_hotkeys_do_not_share_a_chord() {
        // Two binds on one chord: the second registration fails at every start.
        let terminal = Action::SpawnCmd("t".to_string());
        let defaults = default_hotkeys(
            MOD_CTRL | MOD_ALT,
            MOD_CTRL | MOD_ALT | MOD_SHIFT,
            terminal,
        );
        let mut seen = std::collections::HashSet::new();
        for (mods, vk, action) in &defaults {
            assert!(
                seen.insert((*mods, *vk)),
                "{:?} shares its chord (mods {:#x}, vk {:#x}) with another default bind",
                action,
                mods,
                vk
            );
        }
    }

    #[test]
    fn focus_previous_has_a_default_chord_that_registers() {
        // niri's Mod+Tab is Windows' own task switcher with the Ctrl+Alt prefix: the default
        // is the key above Tab (grave), and every other prefix gets it as well.
        let terminal = Action::SpawnCmd("t".to_string());
        let defaults = default_hotkeys(
            MOD_CTRL | MOD_ALT,
            MOD_CTRL | MOD_ALT | MOD_SHIFT,
            terminal,
        );
        let chords: Vec<(u32, u32)> = defaults
            .iter()
            .filter(|(_, _, a)| *a == Action::FocusPrevious)
            .map(|(m, v, _)| (*m, *v))
            .collect();
        assert_eq!(chords, vec![(MOD_CTRL | MOD_ALT, 0xC0)]);
    }

    fn bind(chord: &str, command: &str, args: &[&str]) -> crate::config::HotkeyBinding {
        let mut parts: Vec<String> = chord.split('+').map(|s| s.to_string()).collect();
        let key = parts.pop().unwrap_or_default();
        crate::config::HotkeyBinding {
            modifiers: parts,
            key,
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            ..crate::config::HotkeyBinding::default()
        }
    }

    #[test]
    fn repeat_false_and_cooldown_decide_whether_a_press_fires() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let no_repeat = BindOptions {
            no_repeat: true,
            ..BindOptions::default()
        };
        assert!(bind_may_fire(&no_repeat, false, None, t0), "the first press fires");
        assert!(!bind_may_fire(&no_repeat, true, Some(t0), t0), "auto-repeat does not");
        let repeating = BindOptions::default();
        assert!(bind_may_fire(&repeating, true, Some(t0), t0), "binds repeat by default");
        let cooldown = BindOptions {
            cooldown: Some(Duration::from_millis(150)),
            ..BindOptions::default()
        };
        assert!(bind_may_fire(&cooldown, false, None, t0));
        assert!(!bind_may_fire(&cooldown, false, Some(t0), t0 + Duration::from_millis(100)));
        assert!(bind_may_fire(&cooldown, false, Some(t0), t0 + Duration::from_millis(150)));
        // The options of a config bind are found again by the hotkey id.
        let hotkeys = vec![
            (MOD_CTRL | MOD_ALT, 0x54u32, Action::ToggleFloating),
            (MOD_CTRL | MOD_ALT, 0x51u32, Action::CloseWindow),
        ];
        let mut options = HashMap::new();
        options.insert((MOD_CTRL | MOD_ALT, 0x51u32), cooldown.clone());
        let by_id = options_by_id(&hotkeys, &[32768, 32769], &options);
        assert_eq!(by_id.len(), 1);
        assert_eq!(by_id.get(&32769).map(|(vk, _)| *vk), Some(0x51));
        // A bind Windows refused to register has no options to look up.
        assert!(options_by_id(&hotkeys, &[32768], &options).is_empty());
    }

    #[test]
    fn the_overlay_shows_titles_and_hides_null_titles() {
        let row = |vk: u32, action: &str, title: Option<Option<&str>>| RegisteredBinding {
            id: 32768,
            modifiers: MOD_CTRL | MOD_ALT,
            vk_code: vk,
            action: action.to_string(),
            title: title.map(|t| t.map(|s| s.to_string())),
        };
        let rows = overlay_rows(&[
            row(0x54, "SpawnCmd(\"wt\")", Some(Some("Open a Terminal"))),
            row(0x51, "CloseWindow", None),
            row(0x58, "Quit", Some(None)),
        ]);
        assert_eq!(rows.len(), 2, "the null title hides its bind: {:?}", rows);
        assert_eq!(rows[0].1, "Open a Terminal");
        assert_eq!(rows[1].1, "CloseWindow");
    }

    #[test]
    fn a_bind_with_an_unknown_modifier_never_becomes_a_hotkey() {
        let mod_key = MOD_CTRL | MOD_ALT;
        // `Hyper+T` must not turn into a bare `T`.
        let err = bind_to_hotkey(&bind("Hyper+T", "close-window", &[]), mod_key).unwrap_err();
        assert!(err.contains("Hyper"), "{}", err);
        // `Mod` is the configured prefix; `Mod+Shift` adds Shift.
        assert_eq!(
            bind_to_hotkey(&bind("Mod+T", "toggle-floating", &[]), mod_key),
            Ok((MOD_CTRL | MOD_ALT, 0x54, Action::ToggleFloating))
        );
        assert_eq!(
            bind_to_hotkey(&bind("Mod+Shift+T", "toggle-floating", &[]), MOD_WIN),
            Ok((MOD_WIN | MOD_SHIFT, 0x54, Action::ToggleFloating))
        );
        // A bare key stays a bare key: that is a legitimate bind (the shipped Escape).
        assert_eq!(
            bind_to_hotkey(&bind("Escape", "overview-toggle", &[]), mod_key),
            Ok((0, 0x1B, Action::OverviewToggle))
        );
        // Unknown key / action are still refused.
        assert!(bind_to_hotkey(&bind("Mod+Nonsense", "quit", &[]), mod_key).is_err());
        assert!(bind_to_hotkey(&bind("Mod+T", "no-such-action", &[]), mod_key).is_err());
    }

    #[test]
    fn the_default_terminal_follows_the_config_and_never_is_a_bare_cmd() {
        // No config at all: Windows Terminal, then PowerShell.
        assert_eq!(
            terminal_action_from_config(None),
            Action::SpawnCmd(DEFAULT_TERMINAL_COMMAND.to_string())
        );
        let mut cfg = crate::config::Config::default();
        assert_eq!(
            terminal_action_from_config(Some(&cfg)),
            Action::SpawnCmd(DEFAULT_TERMINAL_COMMAND.to_string())
        );
        // A `spawn` bind with arguments is used as the terminal, arguments intact.
        cfg.binds
            .hotkeys
            .push(bind("Ctrl+Alt+Return", "spawn", &["wt.exe", "-d", "C:\\My Projects"]));
        assert_eq!(
            terminal_action_from_config(Some(&cfg)),
            Action::Spawn(vec![
                "wt.exe".to_string(),
                "-d".to_string(),
                "C:\\My Projects".to_string()
            ])
        );
        // spawn-at-startup is the fallback when no bind spawns anything.
        let mut cfg = crate::config::Config::default();
        cfg.spawn_at_startup
            .push(vec!["alacritty.exe".to_string(), "--hold".to_string()]);
        assert_eq!(
            terminal_action_from_config(Some(&cfg)),
            Action::Spawn(vec!["alacritty.exe".to_string(), "--hold".to_string()])
        );
    }
}
