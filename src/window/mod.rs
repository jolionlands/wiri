use crate::utils::{Rect, Size, WindowId};
use std::collections::HashMap;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextW, GetWindowThreadProcessId};

pub mod rules;
pub use rules::{resolve_window_rules, resolve_window_rules_legacy};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HwndWrapper(HWND);

impl HwndWrapper {
    pub fn new(hwnd: HWND) -> Self {
        Self(hwnd)
    }

    pub fn as_hwnd(self) -> HWND {
        self.0
    }

    pub fn to_raw(self) -> isize {
        self.0 .0 as isize
    }

    pub fn from_raw(raw: isize) -> Self {
        Self(HWND(raw as *mut std::ffi::c_void))
    }
}

impl std::hash::Hash for HwndWrapper {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.to_raw().hash(state);
    }
}

impl From<HWND> for WindowId {
    fn from(hwnd: HWND) -> Self {
        WindowId::new(hwnd.0 as isize)
    }
}

impl From<WindowId> for HWND {
    fn from(window_id: WindowId) -> Self {
        HWND(window_id.as_isize() as *mut std::ffi::c_void)
    }
}

impl From<HwndWrapper> for WindowId {
    fn from(hwnd: HwndWrapper) -> Self {
        WindowId::new(hwnd.to_raw())
    }
}

impl From<WindowId> for HwndWrapper {
    fn from(window_id: WindowId) -> Self {
        HwndWrapper::from_raw(window_id.as_isize())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowState {
    Normal,
    Floating,
    Maximized,
    Fullscreen,
    Minimized,
}

#[derive(Debug, Clone)]
pub struct WindowHandle {
    pub title: String,
    pub class_name: String,
    pub process_id: u32,
}

impl WindowHandle {
    pub fn new(title: String, class_name: String, process_id: u32) -> Self {
        Self {
            title,
            class_name,
            process_id,
        }
    }

    pub fn from_hwnd(hwnd: HWND) -> Self {
        let mut title_buf = [0u16; 256];
        let title_len = unsafe { GetWindowTextW(hwnd, &mut title_buf) };
        let title = String::from_utf16_lossy(&title_buf[..title_len as usize]);

        let mut class_name_buf = [0u16; 256];
        let class_len = unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetClassNameW(hwnd, &mut class_name_buf)
        };
        let class_name = String::from_utf16_lossy(&class_name_buf[..class_len as usize]);

        let mut process_id = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };

        Self {
            title,
            class_name,
            process_id,
        }
    }
}

/// What the window rules decide for one window: the properties of every rule that matches it,
/// a later rule winning over an earlier one for the same property.
///
/// niri applies the `open-*` and `default-*` properties when the window opens and no later;
/// the others (`opacity`, the size limits) follow the window while its title, its focus or
/// its state changes ([`ResolvedWindowRules::same_dynamic_part`] is how the engine notices).
#[derive(Debug, Clone)]
pub struct ResolvedWindowRules {
    pub float: bool,
    pub column: Option<usize>,
    pub follow_cursor: bool,
    pub border: bool,
    /// niri-parity blur backdrop hint.  When `true`, the engine applies the
    /// Aero-style `DwmEnableBlurBehindWindow` blur to the window's backdrop
    /// once (tracked in `TilingEngine.blur_applied`).  Cleared from the set
    /// when the window is removed so a re-add re-applies it.
    pub blur: bool,
    /// Per-window opacity override (0.0..=1.0). `None` means "no rule set;
    /// the engine should leave the window at fully opaque (1.0)".  `Some(0.0)`
    /// is a legal value meaning "explicitly fully transparent".
    pub opacity: Option<f32>,

    // ── niri `open-*` placement properties (consumed when the window opens) ──────
    /// Open on the monitor with this device name (`\\.\DISPLAY1` or `DISPLAY1`); a name
    /// that no monitor has is ignored.
    pub open_on_output: Option<String>,
    /// Open on the workspace at this 1-based position (wiri's `open-in-workspace 2`, and
    /// `workspace "2"`).
    pub open_in_workspace: Option<i32>,
    /// Open on the workspace with this name (niri's `open-on-workspace "web"`, and
    /// `workspace "web"`); a name that no workspace has is ignored.
    pub open_on_workspace: Option<String>,
    /// Open fullscreen.  None = engine default.
    pub open_fullscreen: Option<bool>,
    /// Open floating.  None = engine default (also see `float`
    /// flag, which is the resolved `floating` property from any matching
    /// rule; `open_floating` is the explicit `open-floating "…"` directive).
    pub open_floating: Option<bool>,
    /// Open with its column as wide as the view.
    pub open_maximized: Option<bool>,
    /// Open maximized to the edges of the work area (niri 25.11).
    pub open_maximized_to_edges: Option<bool>,
    /// `Some(false)`: the window opens without taking the focus.
    pub open_focused: Option<bool>,
    /// Cap the initial physical size of the window to `(width_px, height_px)`.
    /// Either dimension of `0` means "no cap on that axis".
    pub open_max_bounds: Option<(u32, u32)>,

    // ── niri `default-*` size properties (the size the window opens with) ──────
    /// `default-column-width`: `Some(Some(size))`, or `Some(None)` for the empty block (the
    /// window opens with the width it has itself).
    pub default_column_width: Option<Option<crate::config::types::SizeSpec>>,
    /// `default-window-height`, the same forms.
    pub default_window_height: Option<Option<crate::config::types::SizeSpec>>,
    /// wiri's older `default-width 800`: the width of the column in pixels.
    pub default_width_px: Option<u32>,
    /// wiri's older `default-height 400`: the height of the tile in pixels.
    pub default_height_px: Option<u32>,

    // ── size limits in pixels (the size of the window, without its border) ──────
    pub min_width: Option<u32>,
    pub max_width: Option<u32>,
    pub min_height: Option<u32>,
    pub max_height: Option<u32>,
}

impl Default for ResolvedWindowRules {
    fn default() -> Self {
        Self {
            float: false,
            column: None,
            follow_cursor: true,
            border: true,
            blur: false,
            opacity: None,
            open_on_output: None,
            open_in_workspace: None,
            open_on_workspace: None,
            open_fullscreen: None,
            open_floating: None,
            open_maximized: None,
            open_maximized_to_edges: None,
            open_focused: None,
            open_max_bounds: None,
            default_column_width: None,
            default_window_height: None,
            default_width_px: None,
            default_height_px: None,
            min_width: None,
            max_width: None,
            min_height: None,
            max_height: None,
        }
    }
}

impl ResolvedWindowRules {
    /// True when `other` asks for the same of the properties that follow the window while
    /// it lives (opacity and the size limits).  The `open-*` and `default-*` properties
    /// are left out: they are applied once, when the window opens.
    pub fn same_dynamic_part(&self, other: &ResolvedWindowRules) -> bool {
        self.opacity == other.opacity
            && self.min_width == other.min_width
            && self.max_width == other.max_width
            && self.min_height == other.min_height
            && self.max_height == other.max_height
    }
}

#[derive(Debug, Clone)]
pub struct Mapped {
    pub id: WindowId,
    pub handle: WindowHandle,
    pub bounds: Rect,
    pub state: WindowState,
    pub rules: ResolvedWindowRules,
    pub activated: bool,
    pub decorated: bool,
}

impl Mapped {
    pub fn new(
        id: WindowId,
        handle: WindowHandle,
        bounds: Rect,
        state: WindowState,
        rules: ResolvedWindowRules,
    ) -> Self {
        Self {
            id,
            handle,
            bounds,
            state,
            rules,
            activated: false,
            decorated: true,
        }
    }

    pub fn request_size(&mut self, size: Size) {
        self.bounds.size = size;
    }

    pub fn set_activated(&mut self, activated: bool) {
        self.activated = activated;
    }

    pub fn set_bounds(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    pub fn set_state(&mut self, state: WindowState) {
        self.state = state;
    }

    pub fn is_floating(&self) -> bool {
        self.rules.float || self.state == WindowState::Floating
    }

    pub fn is_maximized(&self) -> bool {
        self.state == WindowState::Maximized
    }

    pub fn is_fullscreen(&self) -> bool {
        self.state == WindowState::Fullscreen
    }

    pub fn is_minimized(&self) -> bool {
        self.state == WindowState::Minimized
    }

    pub fn is_normal(&self) -> bool {
        self.state == WindowState::Normal
    }
}

#[derive(Debug, Clone)]
pub struct Unmapped {
    pub id: WindowId,
    pub handle: WindowHandle,
    pub pending_rules: ResolvedWindowRules,
}

impl Unmapped {
    pub fn new(id: WindowId, handle: WindowHandle, rules: ResolvedWindowRules) -> Self {
        Self {
            id,
            handle,
            pending_rules: rules,
        }
    }
}

#[derive(Debug, Clone)]
pub enum WindowRef {
    Mapped(Mapped),
    Unmapped(Unmapped),
}

impl WindowRef {
    pub fn id(&self) -> WindowId {
        match self {
            WindowRef::Mapped(m) => m.id,
            WindowRef::Unmapped(u) => u.id,
        }
    }

    pub fn handle(&self) -> &WindowHandle {
        match self {
            WindowRef::Mapped(m) => &m.handle,
            WindowRef::Unmapped(u) => &u.handle,
        }
    }

    pub fn is_mapped(&self) -> bool {
        matches!(self, WindowRef::Mapped(_))
    }

    pub fn is_unmapped(&self) -> bool {
        matches!(self, WindowRef::Unmapped(_))
    }

    pub fn as_mapped(&self) -> Option<&Mapped> {
        match self {
            WindowRef::Mapped(m) => Some(m),
            WindowRef::Unmapped(_) => None,
        }
    }

    pub fn as_mapped_mut(&mut self) -> Option<&mut Mapped> {
        match self {
            WindowRef::Mapped(m) => Some(m),
            WindowRef::Unmapped(_) => None,
        }
    }

    pub fn into_mapped(self) -> Option<Mapped> {
        match self {
            WindowRef::Mapped(m) => Some(m),
            WindowRef::Unmapped(_) => None,
        }
    }
}

pub struct WindowSet {
    mapped: HashMap<WindowId, Mapped>,
    unmapped: HashMap<WindowId, Unmapped>,
}

impl WindowSet {
    pub fn new() -> Self {
        Self {
            mapped: HashMap::new(),
            unmapped: HashMap::new(),
        }
    }

    pub fn insert_mapped(&mut self, window: Mapped) -> Option<Mapped> {
        self.mapped.insert(window.id, window)
    }

    pub fn insert_unmapped(&mut self, window: Unmapped) -> Option<Unmapped> {
        self.unmapped.insert(window.id, window)
    }

    pub fn remove(&mut self, window_id: WindowId) -> Option<WindowRef> {
        if let Some(mapped) = self.mapped.remove(&window_id) {
            Some(WindowRef::Mapped(mapped))
        } else if let Some(unmapped) = self.unmapped.remove(&window_id) {
            Some(WindowRef::Unmapped(unmapped))
        } else {
            None
        }
    }

    pub fn get(&self, window_id: WindowId) -> Option<WindowRef> {
        if let Some(mapped) = self.mapped.get(&window_id) {
            Some(WindowRef::Mapped(mapped.clone()))
        } else if let Some(unmapped) = self.unmapped.get(&window_id) {
            Some(WindowRef::Unmapped(unmapped.clone()))
        } else {
            None
        }
    }

    pub fn get_mapped(&self, window_id: WindowId) -> Option<&Mapped> {
        self.mapped.get(&window_id)
    }

    pub fn get_mapped_mut(&mut self, window_id: WindowId) -> Option<&mut Mapped> {
        self.mapped.get_mut(&window_id)
    }

    pub fn mapped_windows(&self) -> impl Iterator<Item = &Mapped> {
        self.mapped.values()
    }

    pub fn mapped_windows_mut(&mut self) -> impl Iterator<Item = &mut Mapped> {
        self.mapped.values_mut()
    }

    pub fn unmapped_windows(&self) -> impl Iterator<Item = &Unmapped> {
        self.unmapped.values()
    }

    pub fn unmapped_windows_mut(&mut self) -> impl Iterator<Item = &mut Unmapped> {
        self.unmapped.values_mut()
    }

    pub fn all_windows(&self) -> impl Iterator<Item = WindowRef> + '_ {
        self.mapped
            .values()
            .map(|m| WindowRef::Mapped(m.clone()))
            .chain(
                self.unmapped
                    .values()
                    .map(|u| WindowRef::Unmapped(u.clone())),
            )
    }

    pub fn mapped_count(&self) -> usize {
        self.mapped.len()
    }

    pub fn unmapped_count(&self) -> usize {
        self.unmapped.len()
    }

    pub fn total_count(&self) -> usize {
        self.mapped.len() + self.unmapped.len()
    }

    pub fn contains(&self, window_id: WindowId) -> bool {
        self.mapped.contains_key(&window_id) || self.unmapped.contains_key(&window_id)
    }

    pub fn is_mapped(&self, window_id: WindowId) -> bool {
        self.mapped.contains_key(&window_id)
    }

    pub fn is_unmapped(&self, window_id: WindowId) -> bool {
        self.unmapped.contains_key(&window_id)
    }

    pub fn promote_to_mapped(
        &mut self,
        window_id: WindowId,
        bounds: Rect,
        state: WindowState,
    ) -> Option<Mapped> {
        let unmapped = self.unmapped.remove(&window_id)?;
        let mapped = Mapped::new(
            window_id,
            unmapped.handle,
            bounds,
            state,
            unmapped.pending_rules,
        );
        self.mapped.insert(window_id, mapped.clone());
        Some(mapped)
    }

    pub fn demote_to_unmapped(&mut self, window_id: WindowId) -> Option<Unmapped> {
        let mapped = self.mapped.remove(&window_id)?;
        let unmapped = Unmapped::new(window_id, mapped.handle, mapped.rules);
        self.unmapped.insert(window_id, unmapped.clone());
        Some(unmapped)
    }
}

// ---------------------------------------------------------------------------
// Item 5 — FLASHWINFO / urgent-window helper
// ---------------------------------------------------------------------------

/// Check whether a window is currently flashing in the taskbar (urgent state).
///
/// Windows does not expose a public API to *query* the FlashWindow(Ex) state —
/// the call only initiates or stops flashing, it does not return current state.
/// We instead drive an in-process registry: the WinEvent hook in
/// `crate::backend::hooks` listens for `EVENT_SYSTEM_ALERT` (0x0002) and
/// stamps the HWND into a shared `HashSet`. The flag is cleared on
/// `EVENT_SYSTEM_FOREGROUND` (user looked at it) or `EVENT_OBJECT_DESTROY`
/// (window is gone).
///
/// This is the same model niri uses (`is_urgent` flag on the layout element
/// fed from the XDG-shell `set_urgent` request).
pub fn is_window_flashing(hwnd: windows::Win32::Foundation::HWND) -> bool {
    crate::backend::hooks::is_urgent_hwnd(hwnd.0 as isize)
}

impl Default for WindowSet {
    fn default() -> Self {
        Self::new()
    }
}

/// Everything `should_skip_window` needs to know about a window.  The Win32
/// calls live in [`gather_skip_facts`]; the decision itself is the pure
/// function [`classify_skip`] so it can be unit-tested without a desktop.
#[derive(Debug, Clone, Default)]
pub struct SkipFacts {
    pub visible: bool,
    pub tool_window: bool,
    pub no_activate: bool,
    pub topmost: bool,
    pub cloaked: bool,
    /// Owned top-level window without `WS_EX_APPWINDOW` (dialogs, popups).
    pub owned_popup: bool,
    /// `WS_CHILD`, or a parent other than the desktop: never a real top-level
    /// window.  Hosted UI (the XAML island inside every Windows Terminal
    /// window, WebView hosts, ...) shows up here and must never be tiled:
    /// `SetWindowPos` on a child takes parent-relative coordinates, so tiling
    /// it with screen coordinates mangles the host window.
    pub child: bool,
    /// The window belongs to wiri's own process (overlays, hotkey window).
    pub own_process: bool,
    /// The console window (or the terminal that hosts it) that wiri created
    /// for itself.
    pub own_console: bool,
    pub width: u32,
    pub height: u32,
    pub class_name: String,
}

/// Window classes that are never managed.  Compared case-insensitively.
const SKIP_CLASSES: &[&str] = &[
    "Progman",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Windows.UI.Core.CoreWindow",
    "Windows.UI.Input.InputSite.WindowClass",
    // XAML-island / WinUI bridge windows are children of a real top-level
    // window; skipping the class is a second line of defence next to `child`.
    "Windows.UI.Composition.DesktopWindowContentBridge",
    "Microsoft.UI.Content.DesktopChildSiteBridge",
    // Hidden helper window every console session owns.
    "PseudoConsoleWindow",
];

/// Decide whether a window must be left alone.  Returns the reason when it
/// must, `None` when it may be tiled.
pub fn classify_skip(f: &SkipFacts) -> Option<&'static str> {
    if f.own_process {
        return Some("belongs to wiri itself");
    }
    if f.own_console {
        return Some("wiri's own console window");
    }
    if f.child {
        return Some("child window (not a top-level window)");
    }
    if !f.visible {
        return Some("not visible");
    }
    if f.tool_window {
        return Some("tool window");
    }
    if f.no_activate {
        return Some("no-activate window");
    }
    if f.topmost {
        return Some("topmost window");
    }
    if f.cloaked {
        return Some("cloaked by DWM");
    }
    if f.width < 80 || f.height < 60 {
        return Some("too small");
    }
    let class = f.class_name.to_ascii_lowercase();
    if SKIP_CLASSES
        .iter()
        .any(|skip| class.contains(&skip.to_ascii_lowercase()))
    {
        return Some("system or hosted window class");
    }
    None
}

/// Dialog/parent hints correspond to niri's transient-parent auto-floating. An
/// ordinary top-level (including a borderless app) must not float just because
/// it lacks WS_THICKFRAME. Tool/child/no-activate windows are still excluded.
pub fn should_auto_float(facts: &SkipFacts) -> bool {
    classify_skip(facts).is_none()
        && (facts.owned_popup || facts.class_name.eq_ignore_ascii_case("#32770"))
}

/// Console windows that only wiri is attached to, recorded once at startup.
/// With the GUI-subsystem build this stays empty; it matters for a
/// console-subsystem build (debug builds, `cargo run`), where Windows opens a
/// console for wiri that must never end up as a tile.
static OWN_CONSOLE_WINDOWS: std::sync::OnceLock<Vec<isize>> = std::sync::OnceLock::new();

/// Record the console window wiri owns, if it owns one.  A console counts as
/// wiri's own when wiri is the only process attached to it; a console wiri
/// merely inherited from the shell that launched it belongs to the user.
/// Call once, after any `AttachConsole`, before the first window is tiled.
pub fn init_own_console() {
    use windows::Win32::System::Console::{GetConsoleProcessList, GetConsoleWindow};
    use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GetWindow, GA_ROOTOWNER, GW_OWNER};

    let mut own: Vec<isize> = Vec::new();
    unsafe {
        let console = GetConsoleWindow();
        if !console.0.is_null() {
            let mut pids = [0u32; 4];
            let attached = GetConsoleProcessList(&mut pids);
            if attached == 1 {
                own.push(console.0 as isize);
                // With Windows Terminal as the default terminal the console
                // window is an invisible helper owned by the visible window.
                if let Ok(owner) = GetWindow(console, GW_OWNER) {
                    if !owner.0.is_null() {
                        own.push(owner.0 as isize);
                    }
                }
                let root = GetAncestor(console, GA_ROOTOWNER);
                if !root.0.is_null() {
                    own.push(root.0 as isize);
                }
            }
        }
    }
    own.sort_unstable();
    own.dedup();
    let _ = OWN_CONSOLE_WINDOWS.set(own);
}

fn is_own_console_window(hwnd: HWND) -> bool {
    OWN_CONSOLE_WINDOWS
        .get()
        .map(|windows| windows.contains(&(hwnd.0 as isize)))
        .unwrap_or(false)
}

/// Collect the Win32 facts about `hwnd` that [`classify_skip`] judges.
pub fn gather_skip_facts(hwnd: HWND) -> SkipFacts {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::*;

    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
    let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;

    let owner = unsafe { GetWindow(hwnd, GW_OWNER) };
    let owned_popup =
        owner.map(|owner| !owner.0.is_null()).unwrap_or(false) && ex_style & WS_EX_APPWINDOW.0 == 0;

    let child = style & WS_CHILD.0 != 0 || {
        let parent = unsafe { GetAncestor(hwnd, GA_PARENT) };
        !parent.0.is_null() && parent != unsafe { GetDesktopWindow() }
    };

    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let own_process = pid != 0 && pid == unsafe { GetCurrentProcessId() };

    let (width, height) = get_window_bounds(hwnd)
        .map(|b| (b.size.w, b.size.h))
        .unwrap_or((u32::MAX, u32::MAX));

    let mut class_name_buf = [0u16; 256];
    let class_len = unsafe { GetClassNameW(hwnd, &mut class_name_buf) };
    let class_name = String::from_utf16_lossy(&class_name_buf[..class_len as usize]);

    SkipFacts {
        visible: style & WS_VISIBLE.0 != 0,
        tool_window: ex_style & WS_EX_TOOLWINDOW.0 != 0,
        no_activate: ex_style & WS_EX_NOACTIVATE.0 != 0,
        topmost: ex_style & WS_EX_TOPMOST.0 != 0,
        cloaked: is_dwm_cloaked(hwnd),
        owned_popup,
        child,
        own_process,
        own_console: is_own_console_window(hwnd),
        width,
        height,
        class_name,
    }
}

/// Why `hwnd` must not be tiled, or `None` when it may be.
pub fn skip_reason(hwnd: HWND) -> Option<&'static str> {
    classify_skip(&gather_skip_facts(hwnd))
}

pub fn should_skip_window(hwnd: HWND) -> bool {
    skip_reason(hwnd).is_some()
}

fn is_dwm_cloaked(hwnd: HWND) -> bool {
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};

    let mut cloaked = 0u32;
    let result = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut _ as *mut _,
            std::mem::size_of::<u32>() as u32,
        )
    };

    result.is_ok() && cloaked != 0
}

pub fn filter_visible_windows(windows: &[WindowId]) -> Vec<WindowId> {
    windows
        .iter()
        .filter(|&&id| {
            let hwnd = HWND(id.as_isize() as *mut std::ffi::c_void);
            !should_skip_window(hwnd)
        })
        .copied()
        .collect()
}

pub fn get_window_bounds(hwnd: HWND) -> Option<Rect> {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    unsafe {
        let mut rect = windows::Win32::Foundation::RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_ok() {
            Some(Rect::new(
                rect.left,
                rect.top,
                (rect.right - rect.left) as u32,
                (rect.bottom - rect.top) as u32,
            ))
        } else {
            None
        }
    }
}

pub fn get_window_state(hwnd: HWND) -> WindowState {
    use windows::Win32::UI::WindowsAndMessaging::*;

    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;

    if style & WS_MAXIMIZE.0 != 0 {
        WindowState::Maximized
    } else if style & WS_MINIMIZE.0 != 0 {
        WindowState::Minimized
    } else {
        let placement = unsafe {
            let mut placement = WINDOWPLACEMENT::default();
            placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
            let _ = GetWindowPlacement(hwnd, &mut placement);
            placement
        };

        if placement.showCmd == SW_MINIMIZE.0 as u32 {
            WindowState::Minimized
        } else if placement.showCmd == SW_MAXIMIZE.0 as u32
            || placement.showCmd == SW_SHOWMAXIMIZED.0 as u32
        {
            WindowState::Maximized
        } else {
            WindowState::Normal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hwnd_wrapper_conversion() {
        let raw = 12345isize;
        let wrapper = HwndWrapper::from_raw(raw);
        assert_eq!(wrapper.to_raw(), raw);

        let window_id: WindowId = wrapper.into();
        assert_eq!(window_id.as_isize(), raw);

        let wrapper2: HwndWrapper = window_id.into();
        assert_eq!(wrapper2.to_raw(), raw);
    }

    #[test]
    fn test_window_set_operations() {
        let mut window_set = WindowSet::new();

        let id = WindowId::new(100);
        let handle = WindowHandle::new("Test".to_string(), "TestClass".to_string(), 1234);

        let mapped = Mapped::new(
            id,
            handle,
            Rect::new(0, 0, 800, 600),
            WindowState::Normal,
            ResolvedWindowRules::default(),
        );

        window_set.insert_mapped(mapped);
        assert!(window_set.contains(id));
        assert!(window_set.is_mapped(id));
        assert!(!window_set.is_unmapped(id));
        assert_eq!(window_set.mapped_count(), 1);

        let retrieved = window_set.get(id).unwrap();
        assert!(retrieved.is_mapped());

        window_set.remove(id);
        assert!(!window_set.contains(id));
    }

    #[test]
    fn test_window_set_mapped_count() {
        let mut set = WindowSet::new();
        assert_eq!(set.mapped_count(), 0);

        let id1 = WindowId::new(100);
        let handle1 = WindowHandle::new("Win1".to_string(), "Class1".to_string(), 1234);
        let mapped1 = Mapped::new(
            id1,
            handle1,
            Rect::new(0, 0, 800, 600),
            WindowState::Normal,
            ResolvedWindowRules::default(),
        );
        set.insert_mapped(mapped1);
        assert_eq!(set.mapped_count(), 1);

        let id2 = WindowId::new(101);
        let handle2 = WindowHandle::new("Win2".to_string(), "Class2".to_string(), 1235);
        let mapped2 = Mapped::new(
            id2,
            handle2,
            Rect::new(0, 0, 400, 300),
            WindowState::Normal,
            ResolvedWindowRules::default(),
        );
        set.insert_mapped(mapped2);
        assert_eq!(set.mapped_count(), 2);
    }

    #[test]
    fn test_window_set_remove_and_get() {
        let mut set = WindowSet::new();
        let id = WindowId::new(100);
        let handle = WindowHandle::new("Test".to_string(), "TestClass".to_string(), 1234);
        let mapped = Mapped::new(
            id,
            handle,
            Rect::new(0, 0, 800, 600),
            WindowState::Normal,
            ResolvedWindowRules::default(),
        );
        set.insert_mapped(mapped);

        assert!(set.get(id).is_some());
        let removed = set.remove(id);
        assert!(removed.is_some());
        assert!(set.get(id).is_none());
    }

    #[test]
    fn test_window_set_contains() {
        let mut set = WindowSet::new();
        let id = WindowId::new(100);
        assert!(!set.contains(id));

        let handle = WindowHandle::new("Test".to_string(), "TestClass".to_string(), 1234);
        let mapped = Mapped::new(
            id,
            handle,
            Rect::new(0, 0, 800, 600),
            WindowState::Normal,
            ResolvedWindowRules::default(),
        );
        set.insert_mapped(mapped);
        assert!(set.contains(id));
    }

    #[test]
    fn test_mapped_state_checks() {
        let id = WindowId::new(100);
        let handle = WindowHandle::new("Test".to_string(), "TestClass".to_string(), 1234);

        let mut mapped = Mapped::new(
            id,
            handle,
            Rect::new(0, 0, 800, 600),
            WindowState::Normal,
            ResolvedWindowRules::default(),
        );
        assert!(mapped.is_normal());
        assert!(!mapped.is_floating());
        assert!(!mapped.is_minimized());

        mapped.set_state(WindowState::Minimized);
        assert!(mapped.is_minimized());
        assert!(!mapped.is_normal());
    }

    #[test]
    fn test_mapped_bounds_update() {
        let id = WindowId::new(100);
        let handle = WindowHandle::new("Test".to_string(), "TestClass".to_string(), 1234);
        let mut mapped = Mapped::new(
            id,
            handle,
            Rect::new(0, 0, 800, 600),
            WindowState::Normal,
            ResolvedWindowRules::default(),
        );
        assert_eq!(mapped.bounds.size.w, 800);

        mapped.set_bounds(Rect::new(100, 100, 1024, 768));
        assert_eq!(mapped.bounds.loc.x, 100);
        assert_eq!(mapped.bounds.size.w, 1024);
    }

    #[test]
    fn test_resolved_window_rules_default() {
        let rules = ResolvedWindowRules::default();
        assert!(!rules.float);
        assert!(rules.open_in_workspace.is_none());
        assert!(rules.open_on_workspace.is_none());
        assert!(rules.column.is_none());
        assert!(rules.follow_cursor);
        assert!(rules.border);
    }

    #[test]
    fn only_the_properties_that_follow_the_window_count_as_its_dynamic_part() {
        let base = ResolvedWindowRules::default();
        let mut other = base.clone();
        // What is applied once, when the window opens, is not part of it.
        other.open_floating = Some(true);
        other.open_on_workspace = Some("web".to_string());
        other.default_width_px = Some(500);
        other.open_focused = Some(false);
        assert!(base.same_dynamic_part(&other));
        // Opacity and the size limits are.
        for change in [
            (|r: &mut ResolvedWindowRules| r.opacity = Some(0.5)) as fn(&mut ResolvedWindowRules),
            |r| r.min_width = Some(300),
            |r| r.max_width = Some(900),
            |r| r.min_height = Some(200),
            |r| r.max_height = Some(700),
        ] {
            let mut changed = base.clone();
            change(&mut changed);
            assert!(!base.same_dynamic_part(&changed));
        }
    }

    fn normal_window() -> SkipFacts {
        SkipFacts {
            visible: true,
            width: 800,
            height: 600,
            class_name: "Notepad".to_string(),
            ..SkipFacts::default()
        }
    }

    #[test]
    fn test_classify_skip_allows_an_ordinary_window() {
        assert_eq!(classify_skip(&normal_window()), None);
    }

    #[test]
    fn test_classify_skip_rejects_child_windows() {
        // The XAML island inside every Windows Terminal window: tiling it with
        // screen coordinates moved it inside its host and made wiri fight the
        // terminal for focus forever.
        let mut f = normal_window();
        f.child = true;
        f.class_name = "Windows.UI.Composition.DesktopWindowContentBridge".to_string();
        assert!(classify_skip(&f).is_some());
        // Even if the parent lookup were to fail, the class alone is enough.
        f.child = false;
        assert!(classify_skip(&f).is_some());
    }

    #[test]
    fn test_classify_skip_rejects_wiri_own_windows_and_console() {
        let mut f = normal_window();
        f.own_process = true;
        assert!(classify_skip(&f).is_some());

        let mut f = normal_window();
        f.own_console = true;
        f.class_name = "CASCADIA_HOSTING_WINDOW_CLASS".to_string();
        assert!(classify_skip(&f).is_some());
    }

    #[test]
    fn test_classify_skip_rejects_system_and_transient_windows() {
        for mutate in [
            (|f: &mut SkipFacts| f.visible = false) as fn(&mut SkipFacts),
            |f| f.tool_window = true,
            |f| f.no_activate = true,
            |f| f.topmost = true,
            |f| f.cloaked = true,
            |f| f.width = 79,
            |f| f.height = 59,
            |f| f.class_name = "Shell_TrayWnd".to_string(),
            |f| f.class_name = "windows.ui.core.corewindow".to_string(),
        ] {
            let mut f = normal_window();
            mutate(&mut f);
            assert!(classify_skip(&f).is_some(), "{:?} must be skipped", f);
        }
    }

    #[test]
    fn dialogs_float_but_normal_toplevels_and_helpers_do_not() {
        let mut facts = normal_window();
        assert!(!should_auto_float(&facts));
        facts.owned_popup = true;
        assert_eq!(
            classify_skip(&facts),
            None,
            "owned dialog is managed, not skipped"
        );
        assert!(should_auto_float(&facts));
        facts.owned_popup = false;
        facts.class_name = "#32770".into();
        assert!(should_auto_float(&facts));
        facts.child = true;
        assert!(!should_auto_float(&facts));
        facts.child = false;
        facts.tool_window = true;
        assert!(!should_auto_float(&facts));
    }

    #[test]
    fn test_classify_skip_keeps_terminal_hosting_window() {
        // A user's terminal window is a legitimate tile.
        let mut f = normal_window();
        f.class_name = "CASCADIA_HOSTING_WINDOW_CLASS".to_string();
        assert_eq!(classify_skip(&f), None);
    }
}
