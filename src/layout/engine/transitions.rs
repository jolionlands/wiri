//! Admission/close/workspace/content transitions. Kept separate from strip/focus
//! actions: native resources live on the existing input pump, timelines here use
//! the existing active-only frame driver. Unsupported sources simply keep normal motion.
use super::{BackendHandle, TilingEngine};
use crate::layout::animation::{AnimationKind, AnimationTarget};
use crate::overlay::compositor::{self, Effect, Scene, Snapshot, Tile, Timeline};
use crate::utils::{OutputId, Rect, WindowId};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Default)]
pub(super) struct Transitions {
    opening: HashSet<WindowId>,
    active: BTreeMap<u64, Running>,
    closing: HashMap<WindowId, PendingClose>,
    retained: HashSet<WindowId>,
    next: u64,
    pub preparing: bool,
}
struct Running {
    timeline: Timeline,
    windows: Vec<WindowId>,
    output: OutputId,
    effect: Effect,
}
struct PendingClose {
    image: Snapshot,
    rect: Rect,
    output: OutputId,
    timeline: Timeline,
}

impl Transitions {
    pub fn active(&self) -> bool {
        !self.active.is_empty() || !self.closing.is_empty()
    }
    pub fn owns(&self, wid: WindowId) -> bool {
        self.active.values().any(|a| a.windows.contains(&wid))
    }
    pub fn retained(&self, wid: WindowId) -> bool {
        self.retained.contains(&wid)
    }
    pub fn workspace_active(&self, output: OutputId) -> bool {
        self.preparing
            || self
                .active
                .values()
                .any(|r| r.output == output && r.effect == Effect::Workspace)
    }
}

impl TilingEngine {
    fn compositing_enabled(&self, kind: AnimationKind) -> bool {
        compositor::ready()
            && !self.is_overview()
            && self.motion.enabled
            && !kind.is_off()
            && self.motion.slowdown > 0.0
    }
    fn wake_compositor(&self) {
        if let Some(wake) = &self.animation_wake {
            wake.notify_one();
        }
    }

    fn start_transition(
        &mut self,
        effect: Effect,
        output: OutputId,
        tiles: Vec<Tile>,
        kind: AnimationKind,
    ) -> bool {
        if tiles.is_empty() || !self.compositing_enabled(kind) {
            return false;
        }
        self.transitions.next = self.transitions.next.wrapping_add(1);
        let id = self.transitions.next;
        let windows = tiles.iter().map(|t| WindowId::new(t.source)).collect();
        if !compositor::start(Scene { id, effect, tiles }) {
            return false;
        }
        self.transitions.active.insert(
            id,
            Running {
                timeline: Timeline::new(kind, self.motion.slowdown),
                windows,
                output,
                effect,
            },
        );
        self.wake_compositor();
        true
    }

    pub(super) fn mark_window_opening(&mut self, wid: WindowId, at_startup: bool) {
        if !at_startup
            && !self.floating_windows.contains(&wid)
            && self.compositing_enabled(self.motion.open_kind())
        {
            self.transitions.opening.insert(wid);
        }
    }

    /// Called before normal placement. Open follows the actual source frame as
    /// the existing camera/neighbour driver slides the surrounding strip.
    pub(super) fn begin_window_open(
        &mut self,
        wid: WindowId,
        rect: Rect,
        output: OutputId,
    ) -> bool {
        if !self.transitions.opening.remove(&wid) || self.transitions.preparing {
            return false;
        }
        let Some(monitor) = self.monitors.get(&output) else {
            return false;
        };
        if !rect.intersects(monitor.bounds) {
            return false;
        }
        self.start_transition(
            Effect::Open,
            output,
            vec![Tile {
                source: wid.as_isize(),
                from: rect,
                to: rect,
                clip: monitor.bounds,
                outgoing: false,
                snapshot: None,
            }],
            self.motion.open_kind(),
        )
    }

    /// The old pixels are copied before SetWindowPos. The original independent
    /// resize/movement timeline continues to drive the real HWND under its cloak,
    /// preserving neighbour/camera motion and allowing the application to repaint.
    pub(super) fn begin_content_resize(&mut self, wid: WindowId, from: Rect, to: Rect) {
        if self.transitions.preparing
            || self.transitions.owns(wid)
            || from.size == to.size
            || !self.compositing_enabled(self.motion.resize_kind())
        {
            return;
        }
        let Some((output, ws, _)) = self.locate_window(wid) else {
            return;
        };
        let Some(monitor) = self.monitors.get(&output) else {
            return;
        };
        if monitor.active_workspace != ws
            || !to.intersects(monitor.bounds)
            || !from.intersects(monitor.bounds)
        {
            return;
        }
        let clip = monitor.bounds;
        let Some(image) = compositor::capture(wid.as_isize()) else {
            return;
        };
        self.start_transition(
            Effect::Resize,
            output,
            vec![Tile {
                source: wid.as_isize(),
                from,
                to,
                clip,
                outgoing: false,
                snapshot: Some(image),
            }],
            self.motion.resize_kind(),
        );
    }

    /// No WGC wait or PrintWindow/WM_PRINT: the close request is posted immediately
    /// after this bounded redirection-DC copy. A vetoed/slow close expires with its
    /// configured timeline instead of keeping a snapshot or frame task at idle.
    pub fn prepare_close_transition(&mut self, wid: WindowId) {
        let kind = self.motion.close_kind();
        if !self.compositing_enabled(kind) {
            return;
        }
        let Some((output, ws, _)) = self.locate_window(wid) else {
            return;
        };
        let Some(monitor) = self.monitors.get(&output) else {
            return;
        };
        if monitor.active_workspace != ws {
            return;
        }
        let Some(rect) = crate::overlay::compositor::source_frame(wid.as_isize()) else {
            return;
        };
        if !rect.intersects(monitor.bounds) {
            return;
        }
        let Some(image) = compositor::capture(wid.as_isize()) else {
            return;
        };
        self.transitions.closing.insert(
            wid,
            PendingClose {
                image,
                rect,
                output,
                timeline: Timeline::new(kind, self.motion.slowdown),
            },
        );
        self.wake_compositor();
    }

    /// Only DESTROY, not the general remove/auto-float path, may start a close.
    /// Unsolicited destruction has no surviving DWM image: skip, never delay it.
    pub fn window_destroyed_transition(&mut self, wid: WindowId) {
        self.transitions.opening.remove(&wid);
        let Some(pending) = self.transitions.closing.remove(&wid) else {
            return;
        };
        let Some(monitor) = self.monitors.get(&pending.output) else {
            return;
        };
        self.start_transition(
            Effect::Close,
            pending.output,
            vec![Tile {
                source: wid.as_isize(),
                from: pending.rect,
                to: pending.rect,
                clip: monitor.bounds,
                outgoing: true,
                snapshot: Some(pending.image),
            }],
            self.motion.close_kind(),
        );
    }

    fn visible_workspace_tiles(&self, output: OutputId) -> Vec<(WindowId, Rect)> {
        let Some(monitor) = self.monitors.get(&output) else {
            return Vec::new();
        };
        let mut windows: Vec<_> = monitor
            .workspace()
            .into_iter()
            .flat_map(|ws| &ws.columns)
            .flat_map(|c| {
                c.visible_tile_indices()
                    .into_iter()
                    .map(|i| c.tiles[i].window_id)
            })
            .filter(|wid| {
                !self.sticky_windows.contains(wid) && !self.native_moves.contains_key(wid)
            })
            .filter_map(|wid| {
                let rect = self.applied_state.get(&wid).map(|s| s.rect)?;
                rect.intersects(monitor.bounds).then_some((wid, rect))
            })
            .collect();
        windows.extend(
            self.float_homes
                .iter()
                .filter(|(wid, h)| {
                    h.output == output
                        && h.workspace == monitor.active_workspace
                        && !self.minimized.contains_key(wid)
                        && !self.sticky_windows.contains(wid)
                })
                .map(|(wid, h)| (*wid, h.rect))
                .filter(|(_, r)| r.intersects(monitor.bounds)),
        );
        windows
    }

    /// Both workspace sets remain redirection sources until the same driver ends
    /// their slide. Retained sources bypass only inactive-workspace hiding, not
    /// membership, focus, monitor selection or the float stacking model.
    pub(super) fn switch_workspace_composited(
        &mut self,
        workspace: i32,
        backend: &BackendHandle,
    ) -> bool {
        let kind = self.motion.workspace_kind();
        if !self.compositing_enabled(kind) {
            return false;
        }
        let Some(output) = self.monitors.focused_id() else {
            return false;
        };
        let Some(monitor) = self.monitors.get(&output) else {
            return false;
        };
        if workspace == monitor.active_workspace {
            return false;
        }
        let (previous, bounds) = (monitor.active_workspace, monitor.bounds);
        self.cancel_composited_transitions(backend);
        let outgoing = self.visible_workspace_tiles(output);
        self.transitions
            .retained
            .extend(outgoing.iter().map(|(w, _)| *w));
        self.transitions.preparing = true;
        if let Some(monitor) = self.monitors.get_mut(&output) {
            monitor.switch_workspace(workspace);
        }
        self.maintain_empty_workspace(output);
        self.animation.cancel(&AnimationTarget::ScrollX(output));
        self.animation.cancel(&AnimationTarget::WorkspaceX(output));
        self.reveal_focused_column(output);
        self.apply_layout_for_monitor(output, backend);
        let incoming = self.visible_workspace_tiles(output);
        self.transitions.preparing = false;
        let dy = if workspace > previous {
            bounds.size.h as i32
        } else {
            -(bounds.size.h as i32)
        };
        let shifted =
            |r: Rect, dy: i32| Rect::new(r.loc.x, r.loc.y.saturating_add(dy), r.size.w, r.size.h);
        let mut tiles: Vec<_> = outgoing
            .into_iter()
            .map(|(wid, rect)| Tile {
                source: wid.as_isize(),
                from: rect,
                to: shifted(rect, -dy),
                clip: bounds,
                outgoing: true,
                snapshot: None,
            })
            .collect();
        tiles.extend(incoming.into_iter().map(|(wid, rect)| Tile {
            source: wid.as_isize(),
            from: shifted(rect, dy),
            to: rect,
            clip: bounds,
            outgoing: false,
            snapshot: None,
        }));
        // Focus before the pump cloaks: shell activation must not undo our transient cloak.
        if let Some(wid) = self.monitors.get(&output).and_then(|m| m.focus_window) {
            self.activate_now(wid, backend);
        }
        self.raise_floats(output);
        if !self.start_transition(Effect::Workspace, output, tiles, kind) {
            self.transitions.retained.clear();
            self.apply_layout_for_monitor(output, backend);
        }
        true
    }

    /// Called once per existing frame. Input only cancels/snap; it is never queued
    /// behind an effect. Active resources and pending captures are all bounded.
    pub(super) fn tick_composited_transitions(&mut self, delta_ms: u32, backend: &BackendHandle) {
        if self
            .transitions
            .active
            .values()
            .any(|a| a.timeline.interrupted())
        {
            self.cancel_composited_transitions(backend);
            return;
        }
        self.transitions.closing.retain(|_, c| {
            c.timeline.tick(delta_ms);
            !c.timeline.finished()
        });
        let mut done = Vec::new();
        for (&id, active) in &mut self.transitions.active {
            let value = active.timeline.tick(delta_ms);
            if active.timeline.finished() {
                done.push(id);
            } else {
                compositor::frame(id, value);
            }
        }
        let mut outputs = Vec::new();
        for id in &done {
            if let Some(active) = self.transitions.active.remove(id) {
                if active.effect == Effect::Workspace {
                    for wid in active.windows {
                        self.transitions.retained.remove(&wid);
                    }
                    outputs.push(active.output);
                }
            }
        }
        // Real geometry/visibility is final before thumbnail removal. Do not
        // show an outgoing workspace during uncloaking, even with hide-method hide.
        outputs.sort_by_key(|o| o.as_u64());
        outputs.dedup();
        for output in outputs {
            self.apply_layout_for_monitor(output, backend);
        }
        for id in done {
            compositor::finish(id);
        }
    }

    /// Explicit action/reload/quit cancellation, also invoked on the next active
    /// frame after raw input. Snaps existing geometry so an invisible HWND cannot
    /// lag behind a cancelled thumbnail or intercept input at an old rectangle.
    pub fn cancel_composited_transitions(&mut self, backend: &BackendHandle) {
        self.transitions.opening.clear();
        self.transitions.closing.clear();
        if self.transitions.active.is_empty() {
            return;
        }
        let active = std::mem::take(&mut self.transitions.active);
        self.transitions.retained.clear();
        self.transitions.preparing = true;
        // Advance old geometry/camera timelines to their end before releasing the
        // cloak. The config caps both animation time and slowdown below this tick.
        self.tick_animations_with_backend(u32::MAX, backend);
        let mut outputs: Vec<_> = active.values().map(|a| a.output).collect();
        outputs.sort_by_key(|o| o.as_u64());
        outputs.dedup();
        for output in outputs {
            self.apply_layout_for_monitor(output, backend);
        }
        for id in active.keys() {
            compositor::finish(*id);
        }
        self.transitions.preparing = false;
    }

    pub(super) fn reset_composited_transitions(&mut self) {
        for id in self.transitions.active.keys() {
            compositor::finish(*id);
        }
        self.transitions.active.clear();
        self.transitions.closing.clear();
        self.transitions.opening.clear();
        self.transitions.retained.clear();
        self.transitions.preparing = false;
        // Force native reconciliation after a reload has discarded retained sources.
        for state in self.applied_state.values_mut() {
            state.rect = Rect::default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_native_resources_or_pending_work_without_the_input_pump() {
        let mut engine = TilingEngine::new(Default::default());
        assert!(!engine.transitions.active());
        engine.mark_window_opening(WindowId::new(0), false);
        assert!(engine.transitions.opening.is_empty());
        engine.prepare_close_transition(WindowId::new(0));
        engine.window_destroyed_transition(WindowId::new(0));
        engine.reset_composited_transitions();
        assert!(!engine.has_active_animations());
    }
    #[test]
    fn close_wait_expires_even_when_application_vetoes_close() {
        use crate::layout::animation::Easing;
        let mut engine = TilingEngine::new(Default::default());
        engine.transitions.closing.insert(
            WindowId::new(0),
            PendingClose {
                image: Snapshot {
                    width: 1,
                    height: 1,
                    pixels: vec![0xffff_ffff],
                },
                rect: Rect::new(0, 0, 1, 1),
                output: OutputId::from_name("test"),
                timeline: Timeline::new(
                    AnimationKind::Easing {
                        duration_ms: 150,
                        curve: Easing::Linear,
                    },
                    10.0,
                ),
            },
        );
        assert!(engine.transitions.active());
        engine.tick_composited_transitions(1499, &BackendHandle::default_for_test());
        assert!(engine.transitions.active());
        engine.tick_composited_transitions(1, &BackendHandle::default_for_test());
        assert!(!engine.transitions.active());
    }
}
