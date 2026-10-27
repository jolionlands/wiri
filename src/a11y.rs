//! Screen-reader announcements — wiri's counterpart of niri's `src/a11y.rs`.
//!
//! niri exposes an AccessKit tree and announces workspace switches, the overview, the
//! Alt-Tab selection, config errors and so on through it (wiki:Accessibility).  Windows
//! has no AccessKit, so wiri raises **UI Automation notification events** instead: a
//! hidden, never-activated tool window with a minimal UIA provider (`WM_GETOBJECT` +
//! `UiaReturnRawElementProvider`) raises `UiaRaiseNotificationEvent` for every
//! announcement.  Narrator and NVDA read those out; the window is never shown, never
//! activated and holds no input.
//!
//! [`announce`] is the only way an announcement reaches the screen reader, and it goes
//! through the installed [`AnnounceSink`].  Production installs the UIA sink; unit tests
//! install a recording sink and assert the exact text of every event.
//!
//! Nothing runs at rest.  There is no timer and no polling, and the provider window (and
//! its thread) only exists once something has actually been announced — and not at all
//! when `accessibility false` is configured.  When disabled, [`announce`] returns before
//! the provider is ever created.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::RwLock;

/// Announcements are on by default, as in niri (which has no config key for them).  wiri
/// accepts a top-level `accessibility false` (or `off`) to turn them off.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// `accessibility false` in the config: no announcements and no provider window.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Release);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

// ---------------------------------------------------------------------------------------
// The exact texts
//
// niri announces short phrases; the same phrases are asserted by the unit tests and by
// the `h77_*` live gates, so they live here rather than at the call sites.
// ---------------------------------------------------------------------------------------

pub const OVERVIEW_OPENED: &str = "Overview opened";
pub const OVERVIEW_CLOSED: &str = "Overview closed";
/// The exit confirmation (`MessageBoxW` "Quit wiri?") has no accessible children of its
/// own, so the text the dialog shows is what gets announced.
pub const QUIT_DIALOG: &str = "Quit wiri? Yes or No.";
pub const SCREENSHOT_UI: &str = "Screenshot: drag to select, Space to confirm, Escape to cancel.";
pub const HOTKEY_LIST: &str = "Important hotkeys shown";

/// niri announces "Workspace <name>", and "Workspace <n>" for an unnamed workspace.
pub fn workspace_text(name: Option<&str>, number: i32) -> String {
    match name.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => format!("Workspace {name}"),
        None => format!("Workspace {number}"),
    }
}

/// niri 25.11 announces the window the Alt-Tab selection moved to.
pub fn alt_tab_text(title: Option<&str>) -> String {
    match title.map(str::trim).filter(|title| !title.is_empty()) {
        Some(title) => format!("Selected window: {title}"),
        None => "Selected window: untitled".to_string(),
    }
}

pub fn config_error_text(summary: &str) -> String {
    format!("Config error: {}", summary.trim())
}

// ---------------------------------------------------------------------------------------
// The one announcement funnel
// ---------------------------------------------------------------------------------------

/// Where announcements go.  [`announce`] never blocks on the sink: the UIA sink only
/// queues the text and posts a message to its own thread.
pub trait AnnounceSink: Send + Sync {
    fn announce(&self, text: &str);
}

/// The sink when announcements are off or no provider is wanted (tests).
pub struct NullSink;

impl AnnounceSink for NullSink {
    fn announce(&self, _text: &str) {}
}

fn default_sink() -> Arc<dyn AnnounceSink> {
    #[cfg(test)]
    {
        // Unit tests must not create windows or threads implicitly; a test that wants to
        // see announcements installs `testing::RecordingSink` (or the UIA sink).
        Arc::new(NullSink)
    }
    #[cfg(not(test))]
    {
        Arc::new(uia::UiaSink)
    }
}

static SINK: OnceLock<RwLock<Arc<dyn AnnounceSink>>> = OnceLock::new();

fn sink() -> &'static RwLock<Arc<dyn AnnounceSink>> {
    SINK.get_or_init(|| RwLock::new(default_sink()))
}

/// Install the sink that receives every announcement (the UIA sink in production, a
/// recording sink in tests).
pub fn install_sink(sink: Arc<dyn AnnounceSink>) {
    *self::sink().write() = sink;
}

/// Drop back to the null sink.
pub fn remove_sink() {
    install_sink(Arc::new(NullSink));
}

/// Announce `text` to the screen reader.  The single entry point for every event.
pub fn announce(text: &str) {
    if !enabled() {
        return;
    }
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    // Clone the Arc and release the lock before calling out: a sink may do I/O.
    let sink = sink().read().clone();
    sink.announce(text);
}

pub fn announce_workspace(name: Option<&str>, number: i32) {
    announce(&workspace_text(name, number));
}

pub fn announce_overview(opened: bool) {
    announce(if opened { OVERVIEW_OPENED } else { OVERVIEW_CLOSED });
}

pub fn announce_alt_tab(title: Option<&str>) {
    announce(&alt_tab_text(title));
}

pub fn announce_quit_dialog() {
    announce(QUIT_DIALOG);
}

pub fn announce_screenshot_ui() {
    announce(SCREENSHOT_UI);
}

pub fn announce_config_error(summary: &str) {
    announce(&config_error_text(summary));
}

pub fn announce_hotkey_list() {
    announce(HOTKEY_LIST);
}

// ---------------------------------------------------------------------------------------
// The Windows UIA provider
// ---------------------------------------------------------------------------------------

/// The Windows UIA provider.  In a test build the default sink is the null sink, so none of
/// this is reachable from a test — it is still compiled and type-checked there.
#[cfg_attr(test, allow(dead_code))]
mod uia {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
    use std::sync::{Arc, OnceLock};

    use parking_lot::Mutex;
    use windows::core::{BSTR, PCWSTR};
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Accessibility::{
        UiaHostProviderFromHwnd, UiaRaiseNotificationEvent, UiaReturnRawElementProvider,
        NotificationKind_ActionCompleted, NotificationProcessing_ImportantMostRecent,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostMessageW,
        RegisterClassExW, SetLayeredWindowAttributes, SetWindowTextW, ShowWindow, TranslateMessage,
        LWA_ALPHA, MSG, SW_SHOWNOACTIVATE, WM_APP, WM_GETOBJECT, WNDCLASSEXW, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
    };

    use super::AnnounceSink;

    /// The provider window's class name.  The `h77_*` live gates find the window by it.
    pub const PROVIDER_CLASS: &str = "WiriAnnounce";

    /// Wake the provider thread: one message per queued announcement.
    const WM_ANNOUNCE: u32 = WM_APP + 1;

    /// Everything the provider thread owns: the window handle, the queue of texts that
    /// have not been raised yet, and the activity-id counter.
    pub(super) struct Provider {
        hwnd: AtomicIsize,
        queue: Mutex<VecDeque<String>>,
        activity: AtomicU64,
        unavailable: AtomicBool,
    }

    static PROVIDER: OnceLock<Arc<Provider>> = OnceLock::new();

    pub struct UiaSink;

    impl AnnounceSink for UiaSink {
        fn announce(&self, text: &str) {
            provider().post(text);
        }
    }

    /// The process-wide provider, created — window, thread and all — on the first
    /// announcement.  Nothing exists before that.
    fn provider() -> &'static Arc<Provider> {
        PROVIDER.get_or_init(|| {
            let provider = Arc::new(Provider {
                hwnd: AtomicIsize::new(0),
                queue: Mutex::new(VecDeque::new()),
                activity: AtomicU64::new(0),
                unavailable: AtomicBool::new(false),
            });
            provider.start();
            provider
        })
    }

    impl Provider {
        fn start(self: &Arc<Self>) {
            let thread_provider = Arc::clone(self);
            let spawned = std::thread::Builder::new()
                .name("wiri-a11y".into())
                .spawn(move || run(thread_provider));
            if let Err(error) = spawned {
                tracing::warn!(%error, "screen-reader announcements unavailable");
                self.unavailable.store(true, Ordering::Release);
            }
        }

        fn post(&self, text: &str) {
            if self.unavailable.load(Ordering::Acquire) {
                return;
            }
            self.queue.lock().push_back(text.to_string());
            let hwnd = self.hwnd.load(Ordering::Acquire);
            if hwnd != 0 {
                unsafe {
                    let _ = PostMessageW(HWND(hwnd as *mut _), WM_ANNOUNCE, WPARAM(0), LPARAM(0));
                }
            }
        }

        fn drain(&self) -> Vec<String> {
            self.queue.lock().drain(..).collect()
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The provider thread: create the hidden window, then pump messages until it closes.
    fn run(provider: Arc<Provider>) {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let instance = match GetModuleHandleW(None) {
                Ok(instance) => instance,
                Err(error) => {
                    tracing::warn!(%error, "screen-reader announcements unavailable");
                    provider.unavailable.store(true, Ordering::Release);
                    return;
                }
            };
            let class = wide(PROVIDER_CLASS);
            let window_class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                lpszClassName: PCWSTR(class.as_ptr()),
                ..Default::default()
            };
            let _ = RegisterClassExW(&window_class);
            let title = wide("wiri");
            let module = windows::Win32::Foundation::HINSTANCE::from(instance);
            // Never activated, never in Alt+Tab, fully transparent, one pixel in the dead
            // corner of the primary monitor: it exists only for its UIA element, and a
            // client that asks for the element or listens for notifications finds it.
            let created = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_POPUP,
                -8,
                -8,
                1,
                1,
                None,
                None,
                Some(&module),
                None,
            );
            let hwnd = match created {
                Ok(hwnd) => hwnd,
                Err(error) => {
                    tracing::warn!(%error, "screen-reader announcements unavailable");
                    provider.unavailable.store(true, Ordering::Release);
                    return;
                }
            };
            provider.hwnd.store(hwnd.0 as isize, Ordering::Release);
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            if !provider.queue.lock().is_empty() {
                let _ = PostMessageW(hwnd, WM_ANNOUNCE, WPARAM(0), LPARAM(0));
            }
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            // A UIA client (Narrator, NVDA, the `h77_*` gates) asks the window for its
            // element: hand it the HWND's provider.  Its Name is the window text, which
            // is the last announcement.
            WM_GETOBJECT => match UiaHostProviderFromHwnd(hwnd) {
                Ok(element) => UiaReturnRawElementProvider(hwnd, wparam, lparam, &element),
                Err(_) => DefWindowProcW(hwnd, message, wparam, lparam),
            },
            WM_ANNOUNCE => {
                raise(hwnd);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }

    /// Raise one notification per queued text, newest state on the window's element.
    unsafe fn raise(hwnd: HWND) {
        let Some(provider) = PROVIDER.get() else {
            return;
        };
        let texts = provider.drain();
        if texts.is_empty() {
            return;
        }
        let Ok(element) = UiaHostProviderFromHwnd(hwnd) else {
            return;
        };
        for text in texts {
            let title = wide(&text);
            let _ = SetWindowTextW(hwnd, PCWSTR(title.as_ptr()));
            let activity = BSTR::from(format!(
                "wiri.announce.{}",
                provider.activity.fetch_add(1, Ordering::Relaxed)
            ));
            let display = BSTR::from(text.as_str());
            let _ = UiaRaiseNotificationEvent(
                &element,
                NotificationKind_ActionCompleted,
                NotificationProcessing_ImportantMostRecent,
                &display,
                &activity,
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// Test support
// ---------------------------------------------------------------------------------------

/// A sink that records the announcements, for unit tests in any module.
#[cfg(test)]
pub mod testing {
    use std::sync::Arc;

    use parking_lot::{Mutex, MutexGuard};

    use super::{install_sink, AnnounceSink};

    /// Records every text in order.
    #[derive(Debug, Default)]
    pub struct RecordingSink {
        texts: Mutex<Vec<String>>,
    }

    impl RecordingSink {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        pub fn texts(&self) -> Vec<String> {
            self.texts.lock().clone()
        }

        pub fn clear(&self) {
            self.texts.lock().clear();
        }
    }

    impl AnnounceSink for RecordingSink {
        fn announce(&self, text: &str) {
            self.texts.lock().push(text.to_string());
        }
    }

    static SERIAL: Mutex<()> = Mutex::new(());

    /// True when `expected` appears in `texts` in that order.
    ///
    /// The sink is process-wide, and the engine tests that open and close the overview run
    /// in parallel with these tests, so a test asserts the subsequence it caused rather
    /// than the whole list.
    pub fn contains_in_order(texts: &[String], expected: &[&str]) -> bool {
        let mut next = 0;
        for text in texts {
            if next < expected.len() && text == expected[next] {
                next += 1;
            }
        }
        next == expected.len()
    }

    /// Install a recording sink for one test.  The returned guard serialises the tests
    /// that share the process-wide sink and restores the null sink when it is dropped.
    pub fn record() -> (Arc<RecordingSink>, SinkGuard) {
        let guard = SERIAL.lock();
        let sink = RecordingSink::new();
        install_sink(sink.clone() as Arc<dyn AnnounceSink>);
        (sink, SinkGuard(guard))
    }

    pub struct SinkGuard(#[allow(dead_code)] MutexGuard<'static, ()>);

    impl Drop for SinkGuard {
        fn drop(&mut self) {
            super::remove_sink();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::record;
    use super::*;

    #[test]
    fn workspace_names_and_numbers_match_niri() {
        assert_eq!(workspace_text(Some("web"), 3), "Workspace web");
        assert_eq!(workspace_text(Some("   "), 2), "Workspace 2");
        assert_eq!(workspace_text(None, 2), "Workspace 2");
        assert_eq!(workspace_text(None, 10), "Workspace 10");
    }

    #[test]
    fn alt_tab_text_is_the_window_title_or_untitled() {
        assert_eq!(
            alt_tab_text(Some("Firefox — Mozilla")),
            "Selected window: Firefox — Mozilla"
        );
        assert_eq!(alt_tab_text(Some("  ")), "Selected window: untitled");
        assert_eq!(alt_tab_text(None), "Selected window: untitled");
    }

    #[test]
    fn config_error_text_prefixes_the_summary() {
        assert_eq!(
            config_error_text("config.kdl:3:1: unknown option 'foo'"),
            "Config error: config.kdl:3:1: unknown option 'foo'"
        );
    }

    #[test]
    fn every_event_announces_its_exact_text() {
        use super::testing::contains_in_order;
        let (sink, _guard) = record();
        announce_workspace(Some("web"), 3);
        announce_workspace(None, 2);
        announce_overview(true);
        announce_overview(false);
        announce_alt_tab(Some("Firefox"));
        announce_alt_tab(None);
        announce_quit_dialog();
        announce_screenshot_ui();
        announce_config_error("config.kdl:3:1: unknown option 'foo'");
        announce_hotkey_list();
        assert!(
            contains_in_order(
                &sink.texts(),
                &[
                    "Workspace web",
                    "Workspace 2",
                    OVERVIEW_OPENED,
                    OVERVIEW_CLOSED,
                    "Selected window: Firefox",
                    "Selected window: untitled",
                    QUIT_DIALOG,
                    SCREENSHOT_UI,
                    "Config error: config.kdl:3:1: unknown option 'foo'",
                    HOTKEY_LIST,
                ]
            ),
            "{:?}",
            sink.texts()
        );
    }

    #[test]
    fn disabled_accessibility_announces_nothing_and_creates_no_provider() {
        let (sink, _guard) = record();
        set_enabled(false);
        announce_overview(true);
        announce_workspace(Some("web"), 1);
        assert!(sink.texts().is_empty());
        set_enabled(true);
        announce_overview(true);
        assert_eq!(sink.texts(), vec![OVERVIEW_OPENED.to_string()]);
    }

    #[test]
    fn blank_text_is_not_announced_and_a_sink_that_is_removed_silences_it() {
        let (sink, _guard) = record();
        announce("   ");
        announce("");
        assert!(sink.texts().is_empty());
        remove_sink();
        announce(OVERVIEW_CLOSED);
        assert!(sink.texts().is_empty());
    }
}
