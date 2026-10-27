//! niri `cursor { hide-after-inactive-ms <ms> }`: the pointer disappears after the mouse has
//! been still for that long, and comes back the moment it moves or a button is pressed.
//!
//! Windows has no way to hide the global pointer (`ShowCursor` is per-thread), so the only
//! technique that reaches every application is swapping the system cursors: `SetSystemCursor`
//! replaces each of the fifteen `OCR_*` cursors with a fully transparent copy, and
//! `SystemParametersInfoW(SPI_SETCURSORS)` reloads the user's own scheme.  Both calls change
//! the machine, so the safety rules below are part of the feature, not a nicety:
//!
//! * **nothing at rest**: the state machine owns *one* one-shot `SetTimer` timer on the existing
//!   low-level mouse-hook thread.  It is re-armed by the hook on every pointer event, exists
//!   only while the pointer is visible and the option is set, and is consumed by firing.  No
//!   thread, poll or wake-up is added.
//! * **always restored**: the user's cursors are put back on a graceful quit, on a reload that
//!   drops the option, from the panic hook, and once at startup (in case a previous run died
//!   while hidden).
//! * **testable**: every OS call goes through [`CursorBackend`], so unit tests drive a fake and
//!   assert the exact hide, show and restore sequences, including the startup and panic ones.
//!   The real backend refuses to run under `cfg!(test)`.
//!
//! This is an approximation of niri's own cursor hiding: the Windows cursor is a system-wide
//! scheme, not a compositor surface, so a swap is visible to every application at once and a
//! cursor shape set by an application for a single window keeps Windows' own look until the
//! scheme is reloaded.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use tracing::{debug, info, warn};

use crate::config::CursorConfig;

/// One way to change the pointer on screen.
pub trait CursorBackend: Send + Sync {
    /// Replace every system cursor with a transparent one, so the pointer cannot be seen.
    fn hide(&self) -> Result<(), String>;
    /// Put the user's own cursors back (`SPI_SETCURSORS`).
    fn show(&self) -> Result<(), String>;
    /// The unconditional safety net: reload the user's cursor scheme whether or not wiri
    /// believes it is hidden.  Used at startup, from the panic hook and on quit.
    fn restore(&self) -> Result<(), String>;
}

impl<B: CursorBackend + ?Sized> CursorBackend for Arc<B> {
    fn hide(&self) -> Result<(), String> {
        (**self).hide()
    }

    fn show(&self) -> Result<(), String> {
        (**self).show()
    }

    fn restore(&self) -> Result<(), String> {
        (**self).restore()
    }
}

/// What a state transition does: the list is returned to the caller (and asserted by the unit
/// tests) as well as carried out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Swap in the transparent cursors.
    Hide,
    /// Put the user's cursors back, once.
    Show,
    /// Reload the user's cursor scheme unconditionally.
    Restore,
    /// Arm the single one-shot timer for this many milliseconds.
    Arm(u32),
    /// Kill the one-shot timer.
    Disarm,
}

/// The state machine.  It knows nothing about Windows: [`CursorBackend`] hides and shows, and
/// the free functions in this module wire it to the mouse hook and its timer.
pub struct CursorHider {
    backend: Arc<dyn CursorBackend>,
    timeout_ms: Option<u32>,
    hidden: bool,
    armed: bool,
}

impl CursorHider {
    pub fn new(backend: Arc<dyn CursorBackend>) -> Self {
        Self {
            backend,
            timeout_ms: None,
            hidden: false,
            armed: false,
        }
    }

    /// Apply one effect, updating the state and recording it.
    fn effect(&mut self, effect: Effect, out: &mut Vec<Effect>) {
        match effect {
            Effect::Hide => match self.backend.hide() {
                Ok(()) => {
                    self.hidden = true;
                    info!("cursor: pointer hidden");
                }
                Err(error) => warn!("cursor: could not hide the pointer: {}", error),
            },
            Effect::Show => match self.backend.show() {
                Ok(()) => {
                    self.hidden = false;
                    info!("cursor: pointer shown again");
                }
                Err(error) => warn!("cursor: could not show the pointer again: {}", error),
            },
            Effect::Restore => match self.backend.restore() {
                Ok(()) => {
                    self.hidden = false;
                    info!("cursor: Windows cursors reloaded");
                }
                Err(error) => warn!("cursor: could not reload the Windows cursors: {}", error),
            },
            Effect::Arm(ms) => {
                arm_timer(ms);
                self.armed = true;
            }
            Effect::Disarm => {
                // Only a timer of ours counts as an effect: a quit that had nothing armed
                // reports nothing.
                if !self.armed {
                    return;
                }
                cancel_timer();
                self.armed = false;
            }
        }
        out.push(effect);
    }

    /// A new (or reloaded) config.  Turning the option off shows the pointer again and puts the
    /// user's cursors back; turning it on does nothing until the pointer moves.
    pub fn configure(&mut self, timeout_ms: Option<u32>) -> Vec<Effect> {
        let mut out = Vec::new();
        if self.timeout_ms == timeout_ms {
            return out;
        }
        self.timeout_ms = timeout_ms;
        if timeout_ms.is_none() {
            // niri's `hide-after-inactive-ms` is gone: never hide again, and undo what is
            // hidden right now (a reload must not leave the pointer invisible).
            self.effect(Effect::Disarm, &mut out);
            if self.hidden {
                self.effect(Effect::Show, &mut out);
            }
            self.effect(Effect::Restore, &mut out);
        }
        out
    }

    /// Any pointer movement or click: show the pointer again and re-arm the timer.
    pub fn activity(&mut self) -> Vec<Effect> {
        let mut out = Vec::new();
        let Some(ms) = self.timeout_ms else {
            return out;
        };
        if self.hidden {
            self.effect(Effect::Show, &mut out);
        }
        self.effect(Effect::Arm(ms), &mut out);
        out
    }

    /// The one-shot timer fired: the pointer has been still for the configured time.
    pub fn timer(&mut self) -> Vec<Effect> {
        let mut out = Vec::new();
        // One shot: a fired timer is gone, whatever happens next.
        self.armed = false;
        if self.timeout_ms.is_none() || self.hidden {
            return out;
        }
        self.effect(Effect::Hide, &mut out);
        out
    }

    /// Graceful quit: no timer, no hidden pointer, the user's cursors back.
    pub fn shutdown(&mut self) -> Vec<Effect> {
        let mut out = Vec::new();
        self.timeout_ms = None;
        self.effect(Effect::Disarm, &mut out);
        if self.hidden {
            self.effect(Effect::Show, &mut out);
        }
        self.effect(Effect::Restore, &mut out);
        out
    }

    /// The panic hook: nothing fancy, no state beyond "we are not hidden any more".
    pub fn panic_restore(&mut self) -> Vec<Effect> {
        let mut out = Vec::new();
        self.timeout_ms = None;
        self.effect(Effect::Disarm, &mut out);
        self.effect(Effect::Restore, &mut out);
        out
    }

    /// Startup: a previous run may have died while the pointer was hidden.
    pub fn startup_restore(&mut self) -> Vec<Effect> {
        let mut out = Vec::new();
        self.effect(Effect::Restore, &mut out);
        out
    }

    pub fn hidden(&self) -> bool {
        self.hidden
    }
}

// ---------------------------------------------------------------------------------------
// the runtime state
// ---------------------------------------------------------------------------------------

static HIDER: Mutex<Option<CursorHider>> = parking_lot::const_mutex(None);
/// Set while `hide-after-inactive-ms` is configured: the mouse hook reads it on every event
/// before taking the state lock, so an unconfigured wiri costs one atomic load per event.
static ENABLED: AtomicBool = AtomicBool::new(false);

fn with_hider(f: impl FnOnce(&mut CursorHider) -> Vec<Effect>) -> Vec<Effect> {
    let mut guard = HIDER.lock();
    let hider = guard
        .get_or_insert_with(|| CursorHider::new(Arc::new(SystemCursors)));
    f(hider)
}

/// A loaded (or reloaded) config: `cursor { hide-after-inactive-ms <ms> }`.
pub fn configure(cursor: &CursorConfig) {
    let timeout = cursor.hide_after_inactive_ms;
    ENABLED.store(timeout.is_some(), Ordering::Relaxed);
    let effects = with_hider(|hider| hider.configure(timeout));
    debug!("cursor: configure {:?} -> {:?}", timeout, effects);
    match timeout {
        Some(ms) => info!(
            "cursor: hiding the pointer after {} ms without pointer movement",
            ms
        ),
        None => info!("cursor: the pointer is never hidden"),
    }
}

/// Called by the low-level mouse hook for every pointer event (movement and buttons).
pub fn pointer_activity() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let effects = with_hider(CursorHider::activity);
    debug!("cursor: activity -> {:?}", effects);
}

/// The one-shot timer armed on the mouse-hook thread fired.
pub fn on_timer() {
    let effects = with_hider(CursorHider::timer);
    debug!("cursor: timer -> {:?}", effects);
}

/// Graceful quit: never leave the pointer hidden.
pub fn shutdown() {
    ENABLED.store(false, Ordering::Relaxed);
    let effects = with_hider(CursorHider::shutdown);
    debug!("cursor: shutdown -> {:?}", effects);
}

/// Panic hook: the machine must not be left with transparent cursors because wiri crashed.
pub fn panic_restore() {
    ENABLED.store(false, Ordering::Relaxed);
    // A panic inside the state machine (the mouse hook thread) still holds its lock; a panic
    // hook that waited for it would deadlock before unwinding even started.
    if let Some(mut guard) = HIDER.try_lock() {
        if let Some(hider) = guard.as_mut() {
            hider.panic_restore();
            return;
        }
    }
    // `restore` is stateless, so this is the same call the state machine would make.
    let _ = SystemCursors.restore();
}

/// Startup: put the user's cursors back in case a previous run died while hidden.
pub fn restore_at_startup() {
    let effects = with_hider(CursorHider::startup_restore);
    debug!("cursor: startup restore -> {:?}", effects);
}

// ---------------------------------------------------------------------------------------
// the timer: one one-shot timer on the mouse-hook thread
// ---------------------------------------------------------------------------------------

/// Posted (by `KillTimer`-ing the wrong thread's timer is not possible) to the hook thread when
/// another thread drops the option; `wParam` carries the timer id to kill.
pub const CANCEL_MESSAGE: u32 = 0x8000 + 0x5749;

fn arm_timer(ms: u32) {
    #[cfg(target_os = "windows")]
    if !cfg!(test) {
        return windows_timer::arm(ms);
    }
    let _ = ms;
}

fn cancel_timer() {
    #[cfg(target_os = "windows")]
    if !cfg!(test) {
        return windows_timer::disarm();
    }
}

/// Called by the hook message loop for each `WM_TIMER`: true when it is wiri's own timer, which
/// is then killed (a one-shot timer).
pub fn timer_fired(id: usize) -> bool {
    #[cfg(target_os = "windows")]
    if !cfg!(test) {
        return windows_timer::fired(id);
    }
    let _ = id;
    false
}

/// Called by the hook message loop when another thread asked for the timer to be killed.
pub fn cancel_timer_message(id: usize) {
    #[cfg(target_os = "windows")]
    if !cfg!(test) {
        return windows_timer::cancel(id);
    }
    let _ = id;
}

#[cfg(target_os = "windows")]
mod windows_timer {
    use parking_lot::Mutex;
    use tracing::warn;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{KillTimer, PostThreadMessageW, SetTimer};

    struct Armed {
        id: usize,
        thread: u32,
    }

    static TIMER: Mutex<Armed> = parking_lot::const_mutex(Armed { id: 0, thread: 0 });

    /// Arm the single one-shot timer.  Only the thread that pumps messages can own a timer and
    /// the low-level mouse hook runs its callback on this thread, so this is always called from
    /// the hook thread.
    pub fn arm(ms: u32) {
        let thread = unsafe { GetCurrentThreadId() };
        let mut armed = TIMER.lock();
        if armed.id != 0 && armed.thread == thread {
            unsafe {
                let _ = KillTimer(HWND::default(), armed.id);
            }
            armed.id = 0;
        }
        let id = unsafe { SetTimer(HWND::default(), 0, ms, None) };
        if id == 0 {
            warn!(
                "cursor: SetTimer({} ms) failed; the pointer will stay visible",
                ms
            );
            return;
        }
        armed.id = id;
        armed.thread = thread;
    }

    /// Kill the timer, on its own thread or (through [`super::CANCEL_MESSAGE`]) by asking the
    /// hook thread to do it.
    pub fn disarm() {
        let thread = unsafe { GetCurrentThreadId() };
        let mut armed = TIMER.lock();
        if armed.id == 0 {
            return;
        }
        let id = armed.id;
        if armed.thread == thread {
            unsafe {
                let _ = KillTimer(HWND::default(), id);
            }
        } else {
            unsafe {
                let _ =
                    PostThreadMessageW(armed.thread, super::CANCEL_MESSAGE, WPARAM(id), LPARAM(0));
            }
        }
        armed.id = 0;
    }

    /// `WM_TIMER` with this id: ours, and now consumed (one shot).
    pub fn fired(id: usize) -> bool {
        let mut armed = TIMER.lock();
        if id == 0 || armed.id != id {
            return false;
        }
        unsafe {
            let _ = KillTimer(HWND::default(), id);
        }
        armed.id = 0;
        true
    }

    /// The hook thread killed a timer another thread asked it to.
    pub fn cancel(id: usize) {
        TIMER.lock().id = 0;
        if id != 0 {
            unsafe {
                let _ = KillTimer(HWND::default(), id);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// the real backend
// ---------------------------------------------------------------------------------------

/// The real cursor swap: `SetSystemCursor` for every system cursor, `SPI_SETCURSORS` to undo.
pub struct SystemCursors;

impl CursorBackend for SystemCursors {
    fn hide(&self) -> Result<(), String> {
        // A unit test must never touch the machine's real cursors, even by accident.
        if cfg!(test) {
            return Ok(());
        }
        #[cfg(target_os = "windows")]
        {
            windows_cursors::hide()
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err("cursor hiding is only implemented on Windows".into())
        }
    }

    fn show(&self) -> Result<(), String> {
        if cfg!(test) {
            return Ok(());
        }
        self.restore()
    }

    fn restore(&self) -> Result<(), String> {
        if cfg!(test) {
            return Ok(());
        }
        #[cfg(target_os = "windows")]
        {
            windows_cursors::restore()
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err("cursor hiding is only implemented on Windows".into())
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_cursors {
    use windows::Win32::UI::WindowsAndMessaging::HCURSOR;
    use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject, HGDIOBJ};
    use windows::Win32::UI::WindowsAndMessaging::{
        CopyIcon, CreateIconIndirect, DestroyCursor, SetSystemCursor, SystemParametersInfoW,
        ICONINFO, OCR_APPSTARTING, OCR_CROSS, OCR_HAND, OCR_HELP, OCR_IBEAM, OCR_NO, OCR_NORMAL,
        OCR_SIZEALL, OCR_SIZENS, OCR_SIZENESW, OCR_SIZENWSE, OCR_SIZEWE, OCR_UP, OCR_WAIT,
        SPI_SETCURSORS, SYSTEM_CURSOR_ID, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };

    /// Every cursor a window can show: the arrow, the text caret, the busy/working ones, the
    /// resize arrows, the "no" sign and the hand.
    const SYSTEM_CURSORS: [SYSTEM_CURSOR_ID; 14] = [
        OCR_NORMAL, OCR_IBEAM, OCR_WAIT, OCR_CROSS, OCR_UP, OCR_SIZENWSE, OCR_SIZENESW, OCR_SIZEWE,
        OCR_SIZENS, OCR_SIZEALL, OCR_NO, OCR_HAND, OCR_APPSTARTING, OCR_HELP,
    ];

    /// A 32x32 transparent cursor: the AND mask is all ones ("leave the screen alone") and the
    /// XOR mask is all zeros ("draw nothing").
    const CURSOR_SIZE: i32 = 32;
    /// One bit per pixel, padded to whole bytes (`32 / 8`) per row.
    const MASK_STRIDE: usize = (CURSOR_SIZE / 8) as usize;

    fn blank_icon() -> Result<HCURSOR, String> {
        let and_bits = vec![0xFFu8; MASK_STRIDE * CURSOR_SIZE as usize];
        let xor_bits = vec![0x00u8; MASK_STRIDE * CURSOR_SIZE as usize];
        let (and, xor) = unsafe {
            let and = CreateBitmap(
                CURSOR_SIZE,
                CURSOR_SIZE,
                1,
                1,
                Some(and_bits.as_ptr() as *const _),
            );
            let xor = CreateBitmap(
                CURSOR_SIZE,
                CURSOR_SIZE,
                1,
                1,
                Some(xor_bits.as_ptr() as *const _),
            );
            (and, xor)
        };
        if and.0.is_null() || xor.0.is_null() {
            return Err("CreateBitmap for the blank cursor failed".into());
        }
        let info = ICONINFO {
            fIcon: windows::Win32::Foundation::BOOL(0),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: and,
            hbmColor: xor,
        };
        let icon = unsafe { CreateIconIndirect(&info) };
        unsafe {
            let _ = DeleteObject(HGDIOBJ(and.0));
            let _ = DeleteObject(HGDIOBJ(xor.0));
        }
        icon.map(|icon| HCURSOR(icon.0))
            .map_err(|error| format!("CreateIconIndirect for the blank cursor failed: {error}"))
    }

    pub fn hide() -> Result<(), String> {
        let blank = blank_icon()?;
        let mut failures = Vec::new();
        for id in SYSTEM_CURSORS {
            // `SetSystemCursor` takes ownership of the handle it is given, so every cursor
            // needs its own copy: the template stays valid for the next one.
            match unsafe { CopyIcon(blank) } {
                Ok(copy) => {
                    let copy = HCURSOR(copy.0);
                    if let Err(error) = unsafe { SetSystemCursor(copy, id) } {
                        failures.push(format!("{id:?}: {error}"));
                        unsafe {
                            let _ = DestroyCursor(copy);
                        }
                    }
                }
                Err(error) => failures.push(format!("{id:?}: {error}")),
            }
        }
        unsafe {
            let _ = DestroyCursor(blank);
        }
        if failures.is_empty() {
            Ok(())
        } else {
            // Whatever did get swapped is the user's own scheme again after a restore.
            Err(format!(
                "SetSystemCursor failed for {} cursor(s): {}",
                failures.len(),
                failures.join(", ")
            ))
        }
    }

    /// Reload the user's cursor scheme (`SPI_SETCURSORS`), which both undoes a hide and is the
    /// safety net when wiri does not know whether it hid anything.
    pub fn restore() -> Result<(), String> {
        unsafe {
            SystemParametersInfoW(
                SPI_SETCURSORS,
                0,
                None,
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        }
        .map_err(|error| format!("SystemParametersInfoW(SPI_SETCURSORS) failed: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex as ParkingMutex;

    /// A backend that records what it was asked to do.  `fail` makes every call fail, so the
    /// error paths are covered too.
    #[derive(Default)]
    struct Fake {
        calls: ParkingMutex<Vec<&'static str>>,
        fail: bool,
    }

    impl Fake {
        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().clone()
        }
    }

    impl CursorBackend for Fake {
        fn hide(&self) -> Result<(), String> {
            self.calls.lock().push("hide");
            if self.fail {
                return Err("the OS said no".into());
            }
            Ok(())
        }

        fn show(&self) -> Result<(), String> {
            self.calls.lock().push("show");
            if self.fail {
                return Err("the OS said no".into());
            }
            Ok(())
        }

        fn restore(&self) -> Result<(), String> {
            self.calls.lock().push("restore");
            if self.fail {
                return Err("the OS said no".into());
            }
            Ok(())
        }
    }

    fn hider_for(fake: &Arc<Fake>) -> CursorHider {
        CursorHider::new(fake.clone())
    }

    #[test]
    fn a_movement_arms_one_timer_and_firing_it_hides_the_pointer() {
        let fake = Arc::new(Fake::default());
        let mut hider = hider_for(&fake);
        assert!(hider.configure(Some(400)).is_empty(), "nothing until movement");
        assert_eq!(hider.activity(), vec![Effect::Arm(400)]);
        assert_eq!(fake.calls(), Vec::<&'static str>::new(), "only arming");
        assert_eq!(hider.timer(), vec![Effect::Hide]);
        assert_eq!(fake.calls(), vec!["hide"]);
        assert!(hider.hidden());
        // The timer was one shot: a second firing without movement does nothing.
        assert!(hider.timer().is_empty());
        assert_eq!(fake.calls(), vec!["hide"]);
    }

    #[test]
    fn movement_shows_the_pointer_again_and_rearms_the_one_timer() {
        let fake = Arc::new(Fake::default());
        let mut hider = hider_for(&fake);
        hider.configure(Some(400));
        hider.activity();
        hider.timer();
        assert_eq!(hider.activity(), vec![Effect::Show, Effect::Arm(400)]);
        assert_eq!(fake.calls(), vec!["hide", "show"]);
        assert!(!hider.hidden());
        // Every later hide is a fresh hide/show pair, never two hides in a row.
        hider.timer();
        assert_eq!(fake.calls(), vec!["hide", "show", "hide"]);
    }

    #[test]
    fn an_unconfigured_hider_never_hides_or_calls_the_backend() {
        let fake = Arc::new(Fake::default());
        let mut hider = hider_for(&fake);
        assert!(hider.activity().is_empty());
        assert!(hider.timer().is_empty());
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn a_graceful_quit_shows_then_restores_and_disarms() {
        let fake = Arc::new(Fake::default());
        let mut hider = hider_for(&fake);
        hider.configure(Some(250));
        hider.activity();
        hider.timer();
        assert_eq!(
            hider.shutdown(),
            // The timer was consumed by firing when it hid the pointer, so there is nothing to
            // disarm: only "show" and the unconditional reload.
            vec![Effect::Show, Effect::Restore]
        );
        assert_eq!(fake.calls(), vec!["hide", "show", "restore"]);
        assert!(!hider.hidden());
        // Nothing is left armed or configured; a second quit only reloads the cursors again
        // (the safety net is unconditional).
        assert_eq!(hider.shutdown(), vec![Effect::Restore]);
    }

    #[test]
    fn a_quit_with_a_visible_pointer_only_restores() {
        let fake = Arc::new(Fake::default());
        let mut hider = hider_for(&fake);
        hider.configure(Some(5000));
        hider.activity();
        assert_eq!(hider.shutdown(), vec![Effect::Disarm, Effect::Restore]);
        assert_eq!(fake.calls(), vec!["restore"]);
    }

    #[test]
    fn a_reload_that_drops_the_option_shows_the_pointer_and_restores() {
        let fake = Arc::new(Fake::default());
        let mut hider = hider_for(&fake);
        hider.configure(Some(400));
        hider.activity();
        hider.timer();
        assert!(hider.hidden());
        assert_eq!(
            hider.configure(None),
            vec![Effect::Show, Effect::Restore]
        );
        assert_eq!(fake.calls(), vec!["hide", "show", "restore"]);
        assert!(!hider.hidden());
        // A reload that keeps the option changes nothing.
        assert!(hider.configure(None).is_empty());
    }

    #[test]
    fn the_panic_restore_never_leaves_the_pointer_hidden() {
        let fake = Arc::new(Fake::default());
        let mut hider = hider_for(&fake);
        hider.configure(Some(400));
        hider.activity();
        hider.timer();
        assert_eq!(hider.panic_restore(), vec![Effect::Restore]);
        assert_eq!(fake.calls(), vec!["hide", "restore"]);
        assert!(!hider.hidden());
        // Even if the OS refuses, the state says "not hidden" so the next activity shows it.
        let failing = Arc::new(Fake {
            calls: ParkingMutex::new(Vec::new()),
            fail: true,
        });
        let mut refusing = hider_for(&failing);
        refusing.configure(Some(400));
        refusing.activity();
        refusing.timer();
        // The failed hide is reported but does not claim to be hidden.
        assert!(!refusing.hidden());
        assert_eq!(failing.calls(), vec!["hide"]);
        assert!(refusing.activity().iter().all(|e| *e != Effect::Show));
    }

    #[test]
    fn the_startup_restore_runs_once_without_being_asked() {
        let fake = Arc::new(Fake::default());
        let mut hider = hider_for(&fake);
        // Nothing configured yet: the startup restore still reloads the user's cursors.
        assert_eq!(hider.startup_restore(), vec![Effect::Restore]);
        assert_eq!(fake.calls(), vec!["restore"]);
        assert!(!hider.hidden());
    }

    #[test]
    fn zero_milliseconds_would_hide_as_soon_as_the_pointer_stops() {
        let fake = Arc::new(Fake::default());
        let mut hider = hider_for(&fake);
        assert!(hider.configure(Some(0)).is_empty());
        assert_eq!(hider.activity(), vec![Effect::Arm(0)]);
        assert_eq!(hider.timer(), vec![Effect::Hide]);
    }
}
