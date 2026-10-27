//! Recent-windows state and live DWM previews. No source HWND is resized or moved.
//!
//! The engine owns History + Session and forwards focus/input events. The sink
//! never calls the engine, and its message-pump thread exists only for a session.
//! Deadline generations reject late callbacks; no idle timer or polling is used.
use crate::config::recent_windows_config::{RecentCycle, RecentScope, RecentWindowsConfig};
use crate::utils::{OutputId, Rect, WindowId};
use parking_lot::{Condvar, Mutex};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentWindow {
    pub id: WindowId,
    pub output: OutputId,
    pub workspace: i32,
    /// Windows app identity, not the title (normally executable/app model ID).
    pub app_id: String,
    pub width: u32,
    pub height: u32,
    pub urgent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusDeadline {
    pub generation: u64,
    pub delay_ms: u32,
}

// ---------------------------------------------------------------------------------------------
// niri `recent-windows-close`: the fade-out of the overlay
// ---------------------------------------------------------------------------------------------

/// The fade of the recent-windows overlay when it closes, as pure data: the configured
/// duration and the curve of the animation timeline (`animations { recent-windows-close }`).
///
/// The host window only runs a timer while a fade lasts (it starts one when the overlay is
/// hidden and kills it when the alpha reaches zero), so nothing ticks at rest.  A spring has
/// no duration of its own; it is boxed to the time it takes to settle to `epsilon` — about
/// 190 ms for niri's default spring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloseFade {
    duration_ms: u32,
    curve: crate::layout::animation::Easing,
}

impl CloseFade {
    /// The fade of `kind`, or `None` when nothing is to be animated.
    pub fn of(kind: crate::layout::animation::AnimationKind) -> Option<Self> {
        use crate::layout::animation::{AnimationKind, Easing};
        match kind {
            AnimationKind::Off => None,
            AnimationKind::Easing { duration_ms, curve } => {
                if duration_ms == 0 || matches!(curve, Easing::None) {
                    None
                } else {
                    Some(Self { duration_ms, curve })
                }
            }
            AnimationKind::Spring(spring) => Some(Self {
                duration_ms: Self::spring_settle_ms(spring),
                // A spring moves fast at first, like an ease-out; the exact spring shape is
                // the layout lane's business (no frame clock in the overlay).
                curve: Easing::QuadOut,
            }),
        }
    }

    /// How long a spring takes to settle to its `epsilon`, in milliseconds (bounded).
    fn spring_settle_ms(spring: crate::layout::animation::Spring) -> u32 {
        let w = spring.stiffness.max(0.000_001).sqrt();
        let z = spring.damping_ratio.max(0.01);
        // A critically damped spring decays like exp(-w t); the overshoot factor gives the
        // rest a little more time.  Clamped to something a person can see.
        let seconds = (6.0 / (z * w)).max(0.05);
        (seconds * 1000.0).round().clamp(40.0, 1000.0) as u32
    }

    pub fn duration_ms(&self) -> u32 {
        self.duration_ms
    }

    /// The alpha of the overlay (0 to 255) `elapsed_ms` into the fade.
    pub fn alpha(&self, elapsed_ms: u32) -> u8 {
        let t = (elapsed_ms as f64 / self.duration_ms.max(1) as f64).clamp(0.0, 1.0);
        let eased = self.curve.apply(t);
        ((1.0 - eased).clamp(0.0, 1.0) * 255.0).round() as u8
    }

    /// True once the fade is over and the overlay can be destroyed.
    pub fn finished(&self, elapsed_ms: u32) -> bool {
        elapsed_ms >= self.duration_ms
    }
}

/// The fade the daemon installed from the config (`0` = no fade).  The overlay host thread
/// has no engine, so the setting lives here; a reload replaces it.
static CLOSE_FADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A stable code for a curve, so the setting can cross the thread boundary as one number
/// without depending on the enum's declaration order.
fn curve_code(curve: crate::layout::animation::Easing) -> u64 {
    use crate::layout::animation::Easing;
    match curve {
        Easing::None => 0,
        Easing::Linear => 1,
        Easing::EaseIn => 2,
        Easing::EaseOut => 3,
        Easing::EaseInOut => 4,
        Easing::CubicOut => 5,
        Easing::QuadOut => 6,
        Easing::ExpoOut => 7,
    }
}

fn curve_from_code(code: u64) -> crate::layout::animation::Easing {
    use crate::layout::animation::Easing;
    match code {
        0 => Easing::None,
        1 => Easing::Linear,
        2 => Easing::EaseIn,
        3 => Easing::EaseOut,
        4 => Easing::EaseInOut,
        5 => Easing::CubicOut,
        7 => Easing::ExpoOut,
        _ => Easing::QuadOut,
    }
}

/// Install `animations { recent-windows-close }` for the overlay host.
pub fn set_close_fade(kind: crate::layout::animation::AnimationKind) {
    use std::sync::atomic::Ordering;
    let encoded = CloseFade::of(kind).map_or(0, |fade| {
        // Lower 32 bits: milliseconds.  Upper byte: curve, so a reload can change the shape
        // without the host holding a lock.
        fade.duration_ms as u64 | (curve_code(fade.curve) << 32)
    });
    CLOSE_FADE.store(encoded, Ordering::Release);
}

/// The fade the overlay host should run when it closes, if any.
pub fn close_fade() -> Option<CloseFade> {
    use std::sync::atomic::Ordering;
    let encoded = CLOSE_FADE.load(Ordering::Acquire);
    if encoded == 0 {
        return None;
    }
    let duration_ms = (encoded & 0xFFFF_FFFF) as u32;
    (duration_ms > 0).then_some(CloseFade {
        duration_ms,
        curve: curve_from_code(encoded >> 32),
    })
}

#[derive(Debug, Default)]
pub struct RecentHistory {
    order: Vec<WindowId>,
    pending: Option<(WindowId, u64)>,
    generation: u64,
}

impl RecentHistory {
    /// Preserve unfocused new windows at the end; discard dead windows.
    pub fn sync(&mut self, windows: &[RecentWindow]) {
        self.order.retain(|id| windows.iter().any(|w| w.id == *id));
        for w in windows {
            if !self.order.contains(&w.id) {
                self.order.push(w.id);
            }
        }
        if self
            .pending
            .is_some_and(|(id, _)| !self.order.contains(&id))
        {
            self.pending = None;
        }
    }
    pub fn order(&self) -> &[WindowId] {
        &self.order
    }
    /// Invalidate callbacks that have already left the cancellable worker.
    pub fn cancel_pending(&mut self) {
        self.pending = None;
    }
    fn promote(&mut self, id: WindowId) {
        self.order.retain(|w| *w != id);
        self.order.insert(0, id);
    }
    /// Duplicate focus notifications must not keep restarting the deadline.
    pub fn focus(&mut self, id: WindowId, debounce_ms: u32) -> Option<FocusDeadline> {
        if self.pending.is_some_and(|(w, _)| w == id) {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
        if debounce_ms == 0 {
            self.promote(id);
            return None;
        }
        self.pending = Some((id, self.generation));
        Some(FocusDeadline {
            generation: self.generation,
            delay_ms: debounce_ms,
        })
    }
    pub fn expire(&mut self, generation: u64) -> bool {
        match self.pending {
            Some((id, g)) if g == generation => {
                self.pending = None;
                self.promote(id);
                true
            }
            _ => false,
        }
    }
    /// Call only for genuine application input, never switcher navigation.
    pub fn keyboard_input(&mut self, focused: WindowId) {
        self.pending = None;
        self.promote(focused);
    }
    pub fn begin(
        &mut self,
        windows: &[RecentWindow],
        focused: WindowId,
        action: RecentCycle,
    ) -> Option<RecentSession> {
        self.sync(windows);
        let origin = windows.iter().find(|w| w.id == focused)?;
        // Put the actual focus first in the frozen view, without committing it
        // into history. A focus still inside debounce must cycle back to MRU,
        // not to the item that happened to follow it in the old list.
        let mut all = vec![origin.clone()];
        all.extend(
            self.order
                .iter()
                .filter(|id| **id != focused)
                .filter_map(|id| windows.iter().find(|w| w.id == *id).cloned()),
        );
        // Do not let an outstanding ordinary-focus debounce mutate MRU mid-session.
        self.pending = None;
        let mut session = RecentSession {
            all,
            origin: focused,
            output: origin.output,
            workspace: origin.workspace,
            scope: action.scope,
            filter: None,
            selected: Some(focused),
        };
        session.cycle(action);
        Some(session)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecentCommand {
    Next,
    Previous,
    First,
    Last,
    Scope(RecentScope),
    CycleScope,
    Cancel,
    Confirm,
    Close,
    Screenshot,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecentOutcome {
    Continue,
    Cancel,
    Commit(WindowId),
    Close(WindowId),
    Screenshot,
}

#[derive(Debug)]
pub struct RecentSession {
    all: Vec<RecentWindow>,
    origin: WindowId,
    output: OutputId,
    workspace: i32,
    scope: RecentScope,
    filter: Option<String>,
    selected: Option<WindowId>,
}
impl RecentSession {
    pub fn origin(&self) -> WindowId {
        self.origin
    }
    pub fn selected(&self) -> Option<WindowId> {
        self.selected
    }
    pub fn scope(&self) -> RecentScope {
        self.scope
    }
    pub fn visible(&self) -> Vec<&RecentWindow> {
        self.all
            .iter()
            .filter(|w| {
                (self.scope == RecentScope::All
                    || (w.output == self.output
                        && (self.scope != RecentScope::Workspace || w.workspace == self.workspace)))
                    && self.filter.as_ref().is_none_or(|app| &w.app_id == app)
            })
            .collect()
    }
    /// Repeated actions cycle the frozen list, never MRU-order the selection.
    /// Scope is an opening preselection only; filter uses the selected app.
    pub fn cycle(&mut self, action: RecentCycle) {
        self.filter = if action.filter_app_id {
            self.all
                .iter()
                .find(|w| Some(w.id) == self.selected)
                .map(|w| w.app_id.clone())
        } else {
            None
        };
        self.step(action.backwards);
    }
    fn step(&mut self, backwards: bool) {
        let ids: Vec<_> = self.visible().iter().map(|w| w.id).collect();
        self.selected = if ids.is_empty() {
            None
        } else {
            let i = ids.iter().position(|w| Some(*w) == self.selected);
            Some(
                ids[match i {
                    Some(i) if backwards => (i + ids.len() - 1) % ids.len(),
                    Some(i) => (i + 1) % ids.len(),
                    None if backwards => ids.len() - 1,
                    None => 0,
                }],
            )
        };
    }
    pub fn command(&mut self, cmd: RecentCommand) -> RecentOutcome {
        match cmd {
            RecentCommand::Next => self.step(false),
            RecentCommand::Previous => self.step(true),
            RecentCommand::First => self.selected = self.visible().first().map(|w| w.id),
            RecentCommand::Last => self.selected = self.visible().last().map(|w| w.id),
            RecentCommand::Scope(scope) => {
                self.scope = scope;
                self.repair_selection();
            }
            RecentCommand::CycleScope => {
                self.scope = self.scope.next();
                self.repair_selection();
            }
            RecentCommand::Cancel => return RecentOutcome::Cancel,
            RecentCommand::Confirm => return self.confirm(),
            RecentCommand::Close => {
                return self
                    .selected
                    .map_or(RecentOutcome::Cancel, RecentOutcome::Close)
            }
            RecentCommand::Screenshot => return RecentOutcome::Screenshot,
        }
        if self.selected.is_none() {
            RecentOutcome::Cancel
        } else {
            RecentOutcome::Continue
        }
    }
    fn confirm(&self) -> RecentOutcome {
        self.selected
            .map_or(RecentOutcome::Cancel, RecentOutcome::Commit)
    }
    /// Pass the *current physical* Alt/Ctrl/Shift/Win state, after this key-up.
    /// Releasing Alt while Shift remains held must not commit prematurely.
    pub fn modifiers_changed(&self, any_modifier_held: bool) -> RecentOutcome {
        if any_modifier_held {
            RecentOutcome::Continue
        } else {
            self.confirm()
        }
    }
    fn repair_selection(&mut self) {
        if !self.visible().iter().any(|w| Some(w.id) == self.selected) {
            self.selected = self.visible().first().map(|w| w.id);
        }
    }
    pub fn prune(&mut self, alive: &HashSet<WindowId>) -> RecentOutcome {
        self.all.retain(|w| alive.contains(&w.id));
        self.repair_selection();
        if self.selected.is_none() {
            RecentOutcome::Cancel
        } else {
            RecentOutcome::Continue
        }
    }
    pub fn frame(
        &self,
        monitor: Rect,
        config: &RecentWindowsConfig,
        scale: f64,
    ) -> RecentPreviewFrame {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let pad = (config.highlight.padding * scale).round().max(0.0) as i32;
        let visible = self.visible();
        let height_cap = (config.previews.max_height * scale)
            .min(monitor.size.h as f64 * config.previews.max_scale)
            .min((monitor.size.h as f64 - (2 * pad + 70) as f64).max(1.0));
        let mut x = pad;
        let mut previews = Vec::new();
        for w in visible {
            let factor = config
                .previews
                .max_scale
                .min(height_cap / w.height.max(1) as f64)
                .min((monitor.size.w as f64 - (2 * pad) as f64).max(1.0) / w.width.max(1) as f64);
            if factor <= 0.0 {
                continue;
            }
            let width = (w.width as f64 * factor).floor().max(1.0) as u32;
            let height = (w.height as f64 * factor).floor().max(1.0) as u32;
            previews.push(RecentPreview {
                id: w.id,
                rect: Rect::new(
                    x,
                    (monitor.size.h as i32 - height as i32) / 2,
                    width,
                    height,
                ),
                selected: Some(w.id) == self.selected,
                urgent: w.urgent,
            });
            x += width as i32 + 2 * pad + 12;
        }
        // A long MRU list scrolls around the selection instead of shrinking all
        // previews to illegibility. DWM clips off-screen destinations naturally.
        let center = previews
            .iter()
            .find(|p| p.selected)
            .map(|p| p.rect.center().x)
            .unwrap_or(x / 2);
        let shift = monitor.size.w as i32 / 2 - center;
        for p in &mut previews {
            p.rect.loc.x += shift;
        }
        let viewport = Rect::new(0, 0, monitor.size.w, monitor.size.h);
        previews.retain(|p| p.rect.intersects(viewport));
        RecentPreviewFrame {
            monitor,
            previews,
            scope: self.scope,
            highlight: config.highlight.clone(),
            scale,
        }
    }
}

/// Crop a partially off-screen thumbnail without stretching the visible part.
/// All coordinates are host-local; the source rectangle is in source pixels.
pub fn clip_preview(
    rect: Rect,
    viewport: Rect,
    source_width: u32,
    source_height: u32,
) -> Option<(Rect, Rect)> {
    if rect.size.w == 0 || rect.size.h == 0 || source_width == 0 || source_height == 0 {
        return None;
    }
    let left = rect.loc.x.max(viewport.loc.x);
    let top = rect.loc.y.max(viewport.loc.y);
    let right = rect.right().min(viewport.right());
    let bottom = rect.bottom().min(viewport.bottom());
    if left >= right || top >= bottom {
        return None;
    }
    let sx = |x: i32| ((x - rect.loc.x) as i64 * source_width as i64 / rect.size.w as i64) as i32;
    let sy = |y: i32| ((y - rect.loc.y) as i64 * source_height as i64 / rect.size.h as i64) as i32;
    Some((
        Rect::new(left, top, (right - left) as u32, (bottom - top) as u32),
        Rect::new(
            sx(left),
            sy(top),
            (sx(right) - sx(left)).max(1) as u32,
            (sy(bottom) - sy(top)).max(1) as u32,
        ),
    ))
}

#[derive(Debug, Clone)]
pub struct RecentPreview {
    pub id: WindowId,
    pub rect: Rect,
    pub selected: bool,
    pub urgent: bool,
}
#[derive(Debug, Clone)]
pub struct RecentPreviewFrame {
    pub monitor: Rect,
    /// Destinations are host-client coordinates, not screen coordinates.
    pub previews: Vec<RecentPreview>,
    pub scope: RecentScope,
    pub highlight: crate::config::recent_windows_config::RecentHighlight,
    pub scale: f64,
}

/// All operations are asynchronous: safe while the engine mutex is held.
pub trait RecentPreviewSink: Send + Sync {
    fn show(&self, frame: RecentPreviewFrame, open_delay_ms: u32);
    fn hide(&self);
    fn registered_count(&self) -> usize;
}

#[derive(Default)]
pub struct MockRecentWindows {
    pub frame: Mutex<Option<RecentPreviewFrame>>,
}
impl RecentPreviewSink for MockRecentWindows {
    fn show(&self, frame: RecentPreviewFrame, _: u32) {
        *self.frame.lock() = Some(frame);
    }
    fn hide(&self) {
        *self.frame.lock() = None;
    }
    fn registered_count(&self) -> usize {
        self.frame.lock().as_ref().map_or(0, |f| f.previews.len())
    }
}

/// One cancellable deadline worker; created only while a debounce is pending.
/// Rescheduling wakes it, it never polls, and it exits after firing/cancellation.
/// Callback should post to the backend or use a Weak engine handle.
#[derive(Default)]
pub struct RecentDebouncer {
    inner: Arc<DebounceInner>,
}
#[derive(Default)]
struct DebounceInner {
    state: Mutex<DebounceState>,
    wake: Condvar,
}
#[derive(Default)]
struct DebounceState {
    running: bool,
    job: Option<DebounceJob>,
}
struct DebounceJob {
    at: Instant,
    generation: u64,
    callback: Box<dyn FnOnce(u64) + Send>,
}
impl RecentDebouncer {
    pub fn schedule(&self, deadline: FocusDeadline, callback: impl FnOnce(u64) + Send + 'static) {
        let mut state = self.inner.state.lock();
        state.job = Some(DebounceJob {
            at: Instant::now() + Duration::from_millis(deadline.delay_ms as u64),
            generation: deadline.generation,
            callback: Box::new(callback),
        });
        if state.running {
            self.inner.wake.notify_one();
            return;
        }
        state.running = true;
        let inner = self.inner.clone();
        if std::thread::Builder::new()
            .name("wiri-recent-debounce".into())
            .spawn(move || {
                let job = {
                    let mut state = inner.state.lock();
                    loop {
                        let Some(job) = state.job.as_ref() else {
                            state.running = false;
                            return;
                        };
                        let remaining = job.at.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            state.running = false;
                            break state.job.take().unwrap();
                        }
                        inner.wake.wait_for(&mut state, remaining);
                    }
                };
                (job.callback)(job.generation);
            })
            .is_err()
        {
            state.running = false;
            state.job = None;
        }
    }
    pub fn cancel(&self) {
        self.inner.state.lock().job = None;
        self.inner.wake.notify_one();
    }
}
impl Drop for RecentDebouncer {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(target_os = "windows")]
#[path = "recent_windows_win.rs"]
mod win;
#[cfg(target_os = "windows")]
pub use win::DwmRecentWindows;
#[cfg(not(target_os = "windows"))]
pub type DwmRecentWindows = MockRecentWindows;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::animation::{AnimationKind, Easing, Spring};

    #[test]
    fn the_close_fade_follows_the_configured_curve() {
        let fade = CloseFade::of(AnimationKind::Easing {
            duration_ms: 200,
            curve: Easing::Linear,
        })
        .expect("a fade");
        assert_eq!(fade.duration_ms(), 200);
        assert_eq!(fade.alpha(0), 255);
        assert_eq!(fade.alpha(100), 128);
        assert_eq!(fade.alpha(200), 0);
        assert!(!fade.finished(199));
        assert!(fade.finished(200));
        assert!(fade.finished(1000), "a late tick still ends the fade");
    }

    #[test]
    fn an_off_animation_fades_nothing() {
        assert!(CloseFade::of(AnimationKind::Off).is_none());
        assert!(CloseFade::of(AnimationKind::Easing {
            duration_ms: 0,
            curve: Easing::Linear
        })
        .is_none());
        assert!(CloseFade::of(AnimationKind::Easing {
            duration_ms: 200,
            curve: Easing::None
        })
        .is_none());
    }

    #[test]
    fn a_spring_is_boxed_to_a_visible_duration() {
        let fade = CloseFade::of(AnimationKind::Spring(Spring::default())).expect("a fade");
        assert!(
            (40..=1000).contains(&fade.duration_ms()),
            "{}",
            fade.duration_ms()
        );
        assert_eq!(fade.alpha(0), 255);
        assert_eq!(fade.alpha(fade.duration_ms()), 0);
    }

    #[test]
    fn the_installed_fade_round_trips_through_the_published_setting() {
        // Nothing installed: no fade (a daemon with animations off stays as it was).
        set_close_fade(AnimationKind::Off);
        assert!(close_fade().is_none());
        set_close_fade(AnimationKind::Easing {
            duration_ms: 150,
            curve: Easing::ExpoOut,
        });
        let fade = close_fade().expect("a fade");
        assert_eq!(fade.duration_ms(), 150);
        assert_eq!(fade.curve, Easing::ExpoOut);
        set_close_fade(AnimationKind::Off);
        assert!(close_fade().is_none());
    }

    fn windows() -> Vec<RecentWindow> {        (1..=4)
            .map(|i| RecentWindow {
                id: WindowId::new(i),
                output: OutputId::from_name(if i == 4 { "B" } else { "A" }),
                workspace: if i == 3 { 2 } else { 1 },
                app_id: if i % 2 == 0 { "editor" } else { "term" }.into(),
                width: 1600,
                height: 1000,
                urgent: i == 3,
            })
            .collect()
    }
    fn next() -> RecentCycle {
        RecentCycle::default()
    }
    #[test]
    fn debounce_ignores_intermediate_and_stale_focus() {
        let mut h = RecentHistory::default();
        h.sync(&windows());
        let a = h.focus(WindowId::new(2), 750).unwrap();
        assert!(h.focus(WindowId::new(2), 750).is_none());
        let b = h.focus(WindowId::new(3), 750).unwrap();
        assert!(!h.expire(a.generation));
        assert_eq!(h.order()[0], WindowId::new(1));
        h.keyboard_input(WindowId::new(3));
        assert!(!h.expire(b.generation));
        assert_eq!(h.order()[0], WindowId::new(3));
        h.focus(WindowId::new(4), 0);
        assert_eq!(h.order()[0], WindowId::new(4));
    }
    #[test]
    fn repeated_cycle_is_frozen_and_only_release_of_all_modifiers_commits() {
        let mut h = RecentHistory::default();
        let mut s = h.begin(&windows(), WindowId::new(1), next()).unwrap();
        assert_eq!(s.selected(), Some(WindowId::new(2)));
        s.cycle(next());
        assert_eq!(s.selected(), Some(WindowId::new(3)));
        s.cycle(RecentCycle {
            backwards: true,
            ..next()
        });
        assert_eq!(s.selected(), Some(WindowId::new(2)));
        assert_eq!(s.modifiers_changed(true), RecentOutcome::Continue);
        assert_eq!(
            s.modifiers_changed(false),
            RecentOutcome::Commit(WindowId::new(2))
        );
        assert_eq!(s.command(RecentCommand::Cancel), RecentOutcome::Cancel);
        assert_eq!(s.origin(), WindowId::new(1));
        assert_eq!(h.order()[0], WindowId::new(1));
    }
    #[test]
    fn app_id_scopes_and_preselection() {
        let mut h = RecentHistory::default();
        let mut s = h
            .begin(
                &windows(),
                WindowId::new(1),
                RecentCycle {
                    filter_app_id: true,
                    ..next()
                },
            )
            .unwrap();
        assert_eq!(s.selected(), Some(WindowId::new(3)));
        s.command(RecentCommand::Scope(RecentScope::Workspace));
        assert_eq!(s.visible().len(), 1);
        s.cycle(RecentCycle {
            scope: RecentScope::All,
            ..next()
        });
        assert_eq!(s.scope(), RecentScope::Workspace);
        assert_eq!(s.visible().len(), 2);
        s.command(RecentCommand::Scope(RecentScope::Output));
        assert_eq!(s.visible().len(), 3);
        s.command(RecentCommand::Scope(RecentScope::All));
        assert_eq!(s.visible().len(), 4);
        s.command(RecentCommand::First);
        assert_eq!(s.selected(), Some(WindowId::new(1)));
        s.command(RecentCommand::Last);
        assert_eq!(s.selected(), Some(WindowId::new(4)));
        assert_eq!(s.prune(&HashSet::new()), RecentOutcome::Cancel);
    }
    #[test]
    fn intermediate_focus_opens_back_to_mru_without_committing_or_late_expiry() {
        let mut h = RecentHistory::default();
        h.sync(&windows());
        let deadline = h.focus(WindowId::new(2), 750).unwrap();
        let s = h.begin(&windows(), WindowId::new(2), next()).unwrap();
        assert_eq!(s.selected(), Some(WindowId::new(1)));
        assert_eq!(h.order()[0], WindowId::new(1));
        assert!(!h.expire(deadline.generation));
    }
    #[test]
    fn offscreen_previews_are_cropped_not_squeezed() {
        let (dest, source) = clip_preview(
            Rect::new(-100, 50, 400, 200),
            Rect::new(0, 0, 600, 400),
            1600,
            800,
        )
        .unwrap();
        assert_eq!(dest, Rect::new(0, 50, 300, 200));
        assert_eq!(source, Rect::new(400, 0, 1200, 800));
        assert!(clip_preview(
            Rect::new(900, 0, 400, 200),
            Rect::new(0, 0, 600, 400),
            1600,
            800
        )
        .is_none());
    }
    #[test]
    fn preview_limits_source_and_monitor_and_mock_cleans_up() {
        let mut h = RecentHistory::default();
        let s = h.begin(&windows(), WindowId::new(1), next()).unwrap();
        let c = RecentWindowsConfig::default();
        let frame = s.frame(Rect::new(-1280, 100, 1280, 720), &c, 1.0);
        for p in &frame.previews {
            assert!(p.rect.size.h <= 360);
            assert!(p.rect.size.w <= 800);
        }
        let sink = MockRecentWindows::default();
        sink.show(frame, 0);
        assert!(sink.registered_count() > 0);
        sink.hide();
        assert_eq!(sink.registered_count(), 0);
    }
}
