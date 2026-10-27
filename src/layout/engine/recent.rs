//! Recent-windows integration. Preview navigation never changes focus or MRU.
use super::{BackendHandle, TilingEngine};
use crate::config::recent_windows_config::RecentWindowsConfig;
use crate::overlay::recent_windows::{
    RecentDebouncer, RecentHistory, RecentOutcome, RecentPreviewSink, RecentSession, RecentWindow,
};
use crate::utils::WindowId;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};

#[derive(Default)]
pub(super) struct RecentState {
    history: RecentHistory,
    session: Option<RecentSession>,
    debounce: RecentDebouncer,
    sink: Option<Arc<dyn RecentPreviewSink>>,
    engine: Weak<parking_lot::RwLock<TilingEngine>>,
    observed: Option<WindowId>,
    identities: HashMap<WindowId, String>,
    /// The selection last announced to the screen reader (niri 25.11 announces each move).
    announced: Option<WindowId>,
}

impl TilingEngine {
    pub fn install_recent_windows(
        &mut self,
        sink: Arc<dyn RecentPreviewSink>,
        engine: Weak<parking_lot::RwLock<Self>>,
    ) {
        self.recent_reset();
        self.recent.sink = Some(sink);
        self.recent.engine = engine;
    }

    fn recent_config(&self) -> RecentWindowsConfig {
        self.full_config
            .as_ref()
            .map(|c| c.recent_windows.clone())
            .unwrap_or_default()
    }

    /// Reload/disable/quit cancels the session and its one-shot deadline. No joins.
    pub(super) fn recent_reset(&mut self) {
        self.recent.debounce.cancel();
        self.recent.history.cancel_pending();
        self.recent.session = None;
        self.recent.observed = None;
        self.recent.announced = None;
        if let Some(sink) = &self.recent.sink {
            sink.hide();
        }
        crate::input::recent::set_active(false);
    }

    fn recent_windows(&mut self, backend: &BackendHandle) -> Vec<RecentWindow> {
        let mut windows = Vec::new();
        for (&id, info) in &self.tiled_windows {
            if self.minimized.contains_key(&id) {
                continue;
            }
            let location = self
                .locate_window(id)
                .map(|(o, w, _)| (o, w))
                .or_else(|| self.float_location(id));
            let Some((output, workspace)) = location else {
                continue;
            };
            // Cached process identity, never a title or a class-name heuristic.
            let app_id = self
                .recent
                .identities
                .entry(id)
                .or_insert_with(|| {
                    self.rule_state
                        .get(&id)
                        .and_then(|s| s.aumid.clone().or_else(|| s.process_name.clone()))
                        .or_else(|| backend.process_name_for_pid(info.process_id))
                        .unwrap_or_else(|| format!("pid:{}", info.process_id))
                        .to_ascii_lowercase()
                })
                .clone();
            windows.push(RecentWindow {
                id,
                output,
                workspace,
                app_id,
                width: info.bounds.size.w.max(1),
                height: info.bounds.size.h.max(1),
                urgent: self.urgent_windows.contains(&id),
            });
        }
        // HashMap order must not decide which new, as-yet-unfocused window cycles next.
        windows.sort_by_key(|w| w.id.as_isize());
        windows
    }

    pub(super) fn recent_foreground_changed(&mut self, id: WindowId, backend: &BackendHandle) {
        let config = self.recent_config();
        if !config.enabled {
            return;
        }
        // An external focus change ends the camera without stealing focus back.
        if self
            .recent
            .session
            .as_ref()
            .is_some_and(|s| s.origin() != id)
        {
            self.recent_reset();
        }
        if self.recent.session.is_some() || self.recent.observed == Some(id) {
            return;
        }
        self.recent.observed = Some(id);
        let windows = self.recent_windows(backend);
        self.recent.history.sync(&windows);
        if !windows.iter().any(|w| w.id == id) {
            self.recent.history.cancel_pending();
            self.recent.debounce.cancel();
            return;
        }
        let deadline = self.recent.history.focus(id, config.debounce_ms);
        if let Some(deadline) = deadline {
            let weak = self.recent.engine.clone();
            self.recent.debounce.schedule(deadline, move |generation| {
                if let Some(engine) = weak.upgrade() {
                    let mut engine = engine.write();
                    if engine.recent.session.is_none() {
                        engine.recent.history.expire(generation);
                    }
                }
            });
        } else if config.debounce_ms == 0 {
            self.recent.debounce.cancel();
        }
    }

    /// Dispatch on the existing keyboard-action drainer, never inside WH_KEYBOARD_LL.
    /// Returns true only for the existing screenshot UI action.
    pub fn recent_input(
        &mut self,
        input: crate::input::recent::RecentInput,
        backend: &BackendHandle,
    ) -> bool {
        use crate::input::recent::RecentInput;
        if !self.recent_config().enabled {
            self.recent_reset();
            return false;
        }
        match input {
            RecentInput::ApplicationKey(id) => {
                if self.recent.session.is_none() {
                    if let Some(id) =
                        self.managed_window_for(id.as_isize(), super::root_owner_of(id.as_isize()))
                    {
                        // Input and WinEvents use different queues. An older key
                        // for A must not cancel B's newer focus deadline when it
                        // is finally drained; normalize owned dialogs as well.
                        let current = super::query_foreground()
                            .and_then(|fg| self.managed_window_for(fg.hwnd, fg.root_owner));
                        if current != Some(id) {
                            return false;
                        }
                        let windows = self.recent_windows(backend);
                        self.recent.history.sync(&windows);
                        self.recent.history.keyboard_input(id);
                        self.recent.debounce.cancel();
                        self.recent.observed = Some(id);
                    }
                }
            }
            RecentInput::Cycle(action, foreground) => {
                if let Some(session) = &mut self.recent.session {
                    session.cycle(action);
                } else {
                    self.recent.debounce.cancel();
                    let windows = self.recent_windows(backend);
                    // The hook captures actual foreground with the triggering event.
                    let focused =
                        self.managed_window_for(foreground, super::root_owner_of(foreground));
                    self.recent.session =
                        focused.and_then(|id| self.recent.history.begin(&windows, id, action));
                }
                // Acknowledge repeats too: a reset can race a queued trigger and
                // otherwise lose the last modifier-up while ACTIVE is false.
                crate::input::recent::set_active(self.recent.session.is_some());
                self.recent_render();
            }
            RecentInput::Command(command) => {
                let outcome = self.recent.session.as_mut().map(|s| s.command(command));
                if let Some(outcome) = outcome {
                    return self.recent_outcome(outcome, backend);
                }
            }
            RecentInput::Modifiers(held) => {
                let outcome = self
                    .recent
                    .session
                    .as_ref()
                    .map(|s| s.modifiers_changed(held));
                if let Some(outcome) = outcome {
                    return self.recent_outcome(outcome, backend);
                }
            }
        }
        false
    }

    /// Announce the Alt-Tab selection when it moved (niri 25.11 announces each move), and
    /// remember what was announced so a repeat of the same selection stays silent.
    fn announce_selection(&mut self, selected: Option<WindowId>) {
        if self.recent.announced == selected {
            return;
        }
        self.recent.announced = selected;
        if let Some(id) = selected {
            let title = self.tiled_windows.get(&id).map(|info| info.title.clone());
            crate::a11y::announce_alt_tab(title.as_deref());
        }
    }

    fn recent_render(&mut self) {
        // niri 25.11 announces the Alt-Tab selection whenever it moves, so a screen reader
        // user hears the window before committing to it.
        if self.recent.session.is_some() && self.recent.sink.is_some() {
            let selected = self.recent.session.as_ref().and_then(|s| s.selected());
            self.announce_selection(selected);
        }
        let (Some(session), Some(sink)) = (&self.recent.session, &self.recent.sink) else {
            return;
        };
        let output = self
            .locate_window(session.origin())
            .map(|(o, _, _)| o)
            .or_else(|| self.float_location(session.origin()).map(|(o, _)| o))
            .or_else(|| self.monitors.focused_id());
        let Some(monitor) = output.and_then(|o| self.monitors.get(&o)) else {
            return;
        };
        let config = self.recent_config();
        sink.show(
            session.frame(monitor.bounds, &config, monitor.scale_factor),
            config.open_delay_ms,
        );
    }

    fn recent_outcome(&mut self, outcome: RecentOutcome, backend: &BackendHandle) -> bool {
        match outcome {
            RecentOutcome::Continue => self.recent_render(),
            RecentOutcome::Cancel => {
                // Navigation has never activated a selection, so origin still owns focus.
                self.recent_reset();
            }
            RecentOutcome::Commit(id) => {
                self.recent_reset();
                if self.tiled_windows.contains_key(&id) {
                    let previous_output = self.monitors.focused_id();
                    let floating = self.focus_float(id, backend);
                    if !floating {
                        if let Some((output, workspace, _)) = self.locate_window(id) {
                            self.monitors.set_focused(output);
                            let needs_switch = self
                                .monitors
                                .get(&output)
                                .is_some_and(|m| m.active_workspace != workspace);
                            if needs_switch {
                                // Use the engine path: it hides the old workspace,
                                // shows the new one and updates visibility bookkeeping.
                                self.switch_workspace(workspace, backend);
                            }
                            if let Some(monitor) = self.monitors.get_mut(&output) {
                                monitor.set_focus(id);
                            }
                            self.reveal_focused_column(output);
                            self.apply_layout_for_monitor(output, backend);
                            self.activate_now(id, backend);
                        }
                    }
                    let focused_output = self.monitors.focused_id();
                    if focused_output != previous_output {
                        // Focus borders are per-output snapshots. Refresh the old
                        // output too; float focus does not lay out its destination.
                        if let Some(output) = previous_output {
                            self.apply_layout_for_monitor(output, backend);
                        }
                        if floating {
                            if let Some(output) = focused_output {
                                self.apply_layout_for_monitor(output, backend);
                            }
                        }
                    }
                    self.recent.history.keyboard_input(id);
                    self.recent.observed = Some(id);
                }
            }
            RecentOutcome::Close(id) => {
                self.close_window(id.as_isize());
                // Destruction arrives asynchronously; prune when remove_window is called.
                self.recent_render();
            }
            RecentOutcome::Screenshot => {
                self.recent_reset();
                return true;
            }
        }
        false
    }

    pub(super) fn recent_window_removed(&mut self, id: WindowId, backend: &BackendHandle) {
        self.recent.identities.remove(&id);
        if self.recent.observed == Some(id) {
            self.recent.observed = None;
        }
        let windows = self.recent_windows(backend);
        self.recent.history.sync(&windows);
        let alive: HashSet<_> = windows.iter().map(|w| w.id).collect();
        if let Some(session) = &mut self.recent.session {
            let outcome = session.prune(&alive);
            self.recent_outcome(outcome, backend);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{WindowInfo, WindowState};
    use crate::utils::Rect;

    fn window(id: isize, title: &str) -> (WindowId, WindowInfo) {
        (
            WindowId::new(id),
            WindowInfo {
                hwnd: id,
                title: title.to_string(),
                class_name: "WindowClass".to_string(),
                process_id: 42,
                bounds: Rect::default(),
                state: WindowState::Normal,
                is_visible: true,
            },
        )
    }

    #[test]
    fn the_alt_tab_selection_is_announced_once_per_move() {
        use crate::a11y::testing::contains_in_order;
        let (sink, _guard) = crate::a11y::testing::record();
        let mut engine = TilingEngine::new(Default::default());
        let (firefox, firefox_info) = window(1, "Firefox — Mozilla");
        let (terminal, terminal_info) = window(2, "Windows Terminal");
        engine.tiled_windows.insert(firefox, firefox_info);
        engine.tiled_windows.insert(terminal, terminal_info);

        // The first render of a session announces what is selected.
        engine.announce_selection(Some(firefox));
        // A repeat of the same selection stays silent.
        engine.announce_selection(Some(firefox));
        // A move announces the new window.
        engine.announce_selection(Some(terminal));
        // A window wiri knows nothing about still gets an announcement.
        engine.announce_selection(Some(WindowId::new(99)));
        // A session that ends (no selection) says nothing; the next one starts clean.
        engine.announce_selection(None);
        engine.announce_selection(Some(firefox));

        assert!(
            contains_in_order(
                &sink.texts(),
                &[
                    "Selected window: Firefox — Mozilla",
                    "Selected window: Windows Terminal",
                    "Selected window: untitled",
                    "Selected window: Firefox — Mozilla",
                ]
            ),
            "{:?}",
            sink.texts()
        );
    }

    #[test]
    fn a_reset_forgets_the_announced_selection() {
        use crate::a11y::testing::contains_in_order;
        let (sink, _guard) = crate::a11y::testing::record();
        let mut engine = TilingEngine::new(Default::default());
        let (firefox, info) = window(1, "Firefox");
        engine.tiled_windows.insert(firefox, info);
        engine.announce_selection(Some(firefox));
        assert_eq!(engine.recent.announced, Some(firefox));
        engine.recent_reset();
        assert_eq!(engine.recent.announced, None);
        engine.announce_selection(Some(firefox));
        // A new session announces the same window again: two identical texts in order.
        assert!(
            contains_in_order(
                &sink.texts(),
                &["Selected window: Firefox", "Selected window: Firefox"]
            ),
            "{:?}",
            sink.texts()
        );
    }
}
