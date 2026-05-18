use std::collections::HashMap;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextW, GetWindowThreadProcessId};
use crate::utils::{Rect, Size, WindowId};

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
        let class_len = unsafe { windows::Win32::UI::WindowsAndMessaging::GetClassNameW(hwnd, &mut class_name_buf) };
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

#[derive(Debug, Clone)]
pub struct ResolvedWindowRules {
    pub float: bool,
    pub workspace: Option<i32>,
    pub column: Option<usize>,
    pub follow_cursor: bool,
    pub border: bool,
    pub opacity: f32,
}

impl Default for ResolvedWindowRules {
    fn default() -> Self {
        Self {
            float: false,
            workspace: None,
            column: None,
            follow_cursor: true,
            border: true,
            opacity: 1.0,
        }
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
            .chain(self.unmapped.values().map(|u| WindowRef::Unmapped(u.clone())))
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

    pub fn promote_to_mapped(&mut self, window_id: WindowId, bounds: Rect, state: WindowState) -> Option<Mapped> {
        let unmapped = self.unmapped.remove(&window_id)?;
        let mapped = Mapped::new(window_id, unmapped.handle, bounds, state, unmapped.pending_rules);
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

pub fn should_skip_window(hwnd: HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::*;

    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
    let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;

    if style & WS_VISIBLE.0 == 0 {
        return true;
    }
    if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
        return true;
    }
    if ex_style & WS_EX_NOACTIVATE.0 != 0 {
        return true;
    }
    if ex_style & WS_EX_TOPMOST.0 != 0 {
        return true;
    }

    let mut class_name_buf = [0u16; 256];
    let class_len = unsafe { GetClassNameW(hwnd, &mut class_name_buf) };
    let class_name = String::from_utf16_lossy(&class_name_buf[..class_len as usize]);

    let skip_classes = [
        "Progman",
        "Shell_TrayWnd",
        "Windows.UI.Core.corewindow",
        "ApplicationFrameWindow",
        "Windows.UI.Input.InputSite.WindowClass",
    ];

    for skip in &skip_classes {
        if class_name.contains(skip) {
            return true;
        }
    }

    false
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
        let mapped1 = Mapped::new(id1, handle1, Rect::new(0, 0, 800, 600), WindowState::Normal, ResolvedWindowRules::default());
        set.insert_mapped(mapped1);
        assert_eq!(set.mapped_count(), 1);

        let id2 = WindowId::new(101);
        let handle2 = WindowHandle::new("Win2".to_string(), "Class2".to_string(), 1235);
        let mapped2 = Mapped::new(id2, handle2, Rect::new(0, 0, 400, 300), WindowState::Normal, ResolvedWindowRules::default());
        set.insert_mapped(mapped2);
        assert_eq!(set.mapped_count(), 2);
    }

    #[test]
    fn test_window_set_remove_and_get() {
        let mut set = WindowSet::new();
        let id = WindowId::new(100);
        let handle = WindowHandle::new("Test".to_string(), "TestClass".to_string(), 1234);
        let mapped = Mapped::new(id, handle, Rect::new(0, 0, 800, 600), WindowState::Normal, ResolvedWindowRules::default());
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
        let mapped = Mapped::new(id, handle, Rect::new(0, 0, 800, 600), WindowState::Normal, ResolvedWindowRules::default());
        set.insert_mapped(mapped);
        assert!(set.contains(id));
    }

    #[test]
    fn test_mapped_state_checks() {
        let id = WindowId::new(100);
        let handle = WindowHandle::new("Test".to_string(), "TestClass".to_string(), 1234);

        let mut mapped = Mapped::new(id, handle, Rect::new(0, 0, 800, 600), WindowState::Normal, ResolvedWindowRules::default());
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
        let mut mapped = Mapped::new(id, handle, Rect::new(0, 0, 800, 600), WindowState::Normal, ResolvedWindowRules::default());
        assert_eq!(mapped.bounds.size.w, 800);

        mapped.set_bounds(Rect::new(100, 100, 1024, 768));
        assert_eq!(mapped.bounds.loc.x, 100);
        assert_eq!(mapped.bounds.size.w, 1024);
    }

    #[test]
    fn test_resolved_window_rules_default() {
        let rules = ResolvedWindowRules::default();
        assert!(!rules.float);
        assert!(rules.workspace.is_none());
        assert!(rules.column.is_none());
        assert!(rules.follow_cursor);
        assert!(rules.border);
    }

}
