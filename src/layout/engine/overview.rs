//! The overview is a camera over a vertically stacked set of normal workspace views.
//! It never reconfigures applications to thumbnail-sized rectangles.
//!
//! The geometry is niri's: a fixed zoom from `overview { zoom }` (never a
//! fit-everything fit), every workspace of the monitor stacked vertically with a
//! 10%-of-height gap and the focused one centred, and each workspace drawn through
//! its own horizontal camera (see [`crate::layout::overview_geometry`]).
use super::{AppliedState, BackendHandle, OverviewState, TilingEngine};
use crate::layout::animation::Animation;
use crate::layout::overview_geometry;
use crate::overlay::thumbnail_overview::{OverviewCommand, OverviewContext, OverviewWorkspace};
use crate::utils::{OutputId, Point, Rect, WindowId};

#[derive(Default)]
pub(super) struct OverviewCamera {
    pub timeline: Option<Animation>,
    pub closing: bool,
    pub output: Option<OutputId>,
    /// niri's `workspace_render_idx`: the fractional index of the workspace the
    /// camera is centred on.  It pans (never jumps) when the focused workspace
    /// changes, exactly like niri's workspace-switch animation.
    pub render_index: f64,
    /// The in-flight vertical pan: the animation and the index it heads for.
    pub pan: Option<(Animation, f64)>,
}
impl OverviewCamera {
    pub fn animating(&self) -> bool {
        self.timeline.as_ref().is_some_and(|a| !a.is_finished())
            || self.pan.as_ref().is_some_and(|(a, _)| !a.is_finished())
    }
}

/// The monitor's workspaces as one vertical stack in visible order, centred on the
/// camera's fractional workspace index. Pure: the host paints it, the pointer
/// hit-tests it, drag-and-drop targets it and the engine lays tiles out inside it,
/// so the four never disagree about where a workspace is. Rows carry the stable
/// key, the IPC identity and the name (else "Workspace N").
///
/// Empty workspaces are ordinary rows: niri's stack has one entry per workspace
/// (`Monitor::workspaces_render_geo`, src/layout/monitor.rs:1467), so leaving them
/// out would shift every neighbour.
pub(super) fn overview_workspace_stack(
    monitor: &crate::layout::Monitor,
    view: Rect,
    zoom: f64,
    render_index: f64,
) -> Vec<OverviewWorkspace> {
    let ids = monitor.workspace_ids();
    let rects = overview_geometry::workspace_rects(view, ids.len(), render_index, zoom);
    ids.into_iter()
        .enumerate()
        .zip(rects)
        .filter_map(|((index, id), rect)| {
            let ws = monitor.workspaces.get(&id)?;
            Some(OverviewWorkspace {
                id,
                ipc_id: ws.ipc_id,
                label: ws
                    .name
                    .clone()
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("Workspace {}", index + 1)),
                active: id == monitor.active_workspace,
                rect,
            })
        })
        .collect()
}

impl TilingEngine {
    pub(super) fn overview_options(&self) -> crate::config::overview_config::OverviewConfig {
        let mut options = self
            .full_config
            .as_ref()
            .map(|c| c.overview)
            .unwrap_or_default();
        // niri `layout { background-color }` (where the deprecated per-output key maps) is
        // what the overview paints behind its workspaces unless `overview { backdrop-color }`
        // names a colour. The single overview host paints it; GDI-style alpha is ignored.
        if !options.backdrop_set {
            let layout = self
                .monitors
                .focused_id()
                .map_or(&self.config, |output| self.active_workspace_config(output));
            if let Some([r, g, b, _]) = layout.background_color {
                options.backdrop_color = [r, g, b, 255];
            }
        }
        options
    }

    fn start_overview_camera(&mut self, target: f64) {
        let from = self.overview_zoom();
        let kind = if self.animation_wake.is_some() {
            self.motion.overview_kind()
        } else {
            crate::layout::animation::AnimationKind::Off
        };
        if let Some(a) = &mut self.overview_camera.timeline {
            a.retarget(target, kind, self.motion.slowdown);
        } else {
            self.overview_camera.timeline = Some(Animation::with_kind(
                from,
                target,
                kind,
                self.motion.slowdown,
            ));
        }
        let a = self.overview_camera.timeline.as_ref().unwrap();
        if let Some(state) = &mut self.overview {
            state.zoom = a.current();
        }
        if let Some(wake) = &self.animation_wake {
            wake.notify_one();
        }
    }

    pub(super) fn reconfigure_overview_camera(&mut self) {
        if self.overview.is_some() && self.thumbnail_overview.is_some() {
            let target = if self.overview_camera.closing {
                1.0
            } else {
                overview_geometry::configured_zoom(self.overview_options().zoom)
            };
            self.start_overview_camera(target);
            // Even an Off retarget needs one frame to retire a closing host.
            if self.overview_camera.closing && !self.overview_camera.animating() {
                self.overview_camera.timeline =
                    Some(Animation::new(0.0, 1.0, 1, crate::layout::Easing::Linear));
            }
        }
    }

    pub(super) fn begin_overview_close(&mut self) -> bool {
        if self.overview.is_none() || self.thumbnail_overview.is_none() {
            return false;
        }
        if self.overview_camera.closing {
            return true;
        }
        self.overview_camera.closing = true;
        self.start_overview_camera(1.0);
        self.overview_camera.animating()
    }

    pub(super) fn tick_overview_camera(&mut self, delta_ms: u32, backend: &BackendHandle) {
        // niri animates the workspace stack's vertical camera with the workspace
        // switch and the zoom with the overview open/close animation; both are
        // ticked here, then one layout pass republishes the scene.
        let mut moved = false;
        let mut finished_pan = false;
        if let Some((a, target)) = &mut self.overview_camera.pan {
            if !a.is_finished() {
                self.overview_camera.render_index = a.tick(delta_ms);
                moved = true;
            }
            if a.is_finished() {
                self.overview_camera.render_index = *target;
                finished_pan = true;
            }
        }
        if finished_pan {
            self.overview_camera.pan = None;
        }
        if let Some(a) = &mut self.overview_camera.timeline {
            if !a.is_finished() {
                let zoom = a.tick(delta_ms).clamp(0.0, 1.0);
                if let Some(state) = &mut self.overview {
                    state.zoom = zoom;
                }
                moved = true;
                if a.is_finished() && self.overview_camera.closing {
                    self.finish_overview_close(backend);
                    return;
                }
            }
        }
        if moved {
            if let Some(output) = self.overview_camera.output {
                self.apply_layout_for_monitor(output, backend);
            }
        }
    }

    /// The vertical camera target: the fractional index of the monitor's focused
    /// workspace (niri's `workspace_render_idx`, src/layout/monitor.rs:1410).
    pub(super) fn overview_camera_target(&self, output: OutputId) -> Option<f64> {
        let monitor = self.monitors.get(&output)?;
        let ids = monitor.workspace_ids();
        ids.iter()
            .position(|id| *id == monitor.active_workspace)
            .map(|index| index as f64)
    }

    /// Pan the workspace stack towards the focused workspace instead of jumping,
    /// using the workspace-switch animation.  Called from the layout pass, so a
    /// keyboard or IPC workspace change inside the overview is animated exactly
    /// like niri's `workspace_render_idx`.
    pub(super) fn retarget_overview_vertical_camera(&mut self) {
        let Some(output) = self.overview_camera.output else {
            return;
        };
        let Some(target) = self.overview_camera_target(output) else {
            return;
        };
        let current = self.overview_camera.render_index;
        if (current - target).abs() < 0.0005 {
            self.overview_camera.render_index = target;
            self.overview_camera.pan = None;
            return;
        }
        if let Some((a, pan_target)) = &self.overview_camera.pan {
            if !a.is_finished() && (*pan_target - target).abs() < 0.0005 {
                return;
            }
        }
        let kind = if self.animation_wake.is_some() {
            self.motion.workspace_kind()
        } else {
            crate::layout::animation::AnimationKind::Off
        };
        let anim = Animation::with_kind(current, target, kind, self.motion.slowdown);
        self.overview_camera.render_index = anim.current();
        if anim.is_finished() {
            self.overview_camera.render_index = target;
            self.overview_camera.pan = None;
        } else {
            self.overview_camera.pan = Some((anim, target));
            if let Some(wake) = &self.animation_wake {
                wake.notify_one();
            }
        }
    }

    pub(super) fn enter_workspace_overview(&mut self, backend: &BackendHandle) {
        self.cancel_composited_transitions(backend);
        let Some(output) = self.monitors.focused_id() else {
            return;
        };
        let Some(sink) = self.thumbnail_overview.clone() else {
            return;
        };
        if self.overview.is_some() {
            if self.overview_camera.closing {
                self.overview_camera.closing = false;
                self.start_overview_camera(overview_geometry::configured_zoom(
                    self.overview_options().zoom,
                ));
                crate::a11y::announce_overview(true);
            }
            return;
        }
        // Stop in-flight geometry before opening the camera; source surfaces keep normal size.
        self.animation.cancel_all();
        for wid in self.animating_rects.keys() {
            if let Some(state) = self.applied_state.get_mut(wid) {
                state.rect = Rect::default();
            }
        }
        self.animating_rects.clear();
        self.overview = Some(OverviewState {
            zoom: 1.0,
            workspace_offsets: Vec::new(),
        });
        crate::a11y::announce_overview(true);
        self.overview_camera.output = Some(output);
        // Open centred on the focused workspace: niri computes the stack from the
        // focused index, so opening never pans.
        self.overview_camera.render_index = self.overview_camera_target(output).unwrap_or(0.0);
        self.overview_camera.pan = None;
        self.start_overview_camera(overview_geometry::configured_zoom(
            self.overview_options().zoom,
        ));
        sink.enter();
        let sources: Vec<_> = self
            .monitors
            .get(&output)
            .into_iter()
            .flat_map(|m| m.workspaces.values())
            .flat_map(|ws| &ws.columns)
            .flat_map(|col| {
                col.visible_tile_indices()
                    .into_iter()
                    .map(|i| col.tiles[i].window_id)
            })
            .collect();
        for wid in sources {
            sink.register(wid, wid.as_isize());
            // Hidden HWNDs yield stale/blank DWM surfaces. Let them render behind the opaque
            // topmost host. SW_SHOWNOACTIVATE never steals keyboard focus.
            let _ = backend.show_window(wid.as_isize(), true);
            self.applied_state
                .entry(wid)
                .or_insert_with(AppliedState::unset)
                .visible = true;
        }
        self.apply_layout_for_monitor(output, backend);
    }

    /// The tiles of every workspace on `output` under the overview camera, as
    /// `(window, full screen rect)` plus the workspace rectangle each tile must be
    /// cropped to.
    ///
    /// niri draws each workspace at `zoom *` its normal size through the
    /// workspace's *own* horizontal camera and crops it to the workspace rect
    /// (`Monitor::render_workspaces`, src/layout/monitor.rs:1735-1745), so a column
    /// that is half past the edge is cut there and a fully scrolled-out one is not
    /// drawn at all.
    ///
    /// With the opt-in `layout { unified-grid }` on for this monitor every row is
    /// drawn through the monitor's *one* shared camera instead
    /// ([`TilingEngine::camera_offset`]), so the columns of different rows sit at the
    /// same on-screen x and the zoomed-out view reads as the same XY grid the normal
    /// view is. Empty slots stay empty: a row shorter than the camera offset simply
    /// has nothing there, exactly like rule 3's "past the row's ends".
    pub(super) fn workspace_overview_scene(
        &self,
        output: OutputId,
    ) -> (Vec<(WindowId, Rect)>, Vec<(WindowId, Rect)>) {
        let Some(monitor) = self.monitors.get(&output) else {
            return (Vec::new(), Vec::new());
        };
        // The camera spans the full work area; each workspace resolves its own
        // insets inside it (C2), rather than inheriting the active workspace's view.
        let view = monitor.work_area;
        let zoom = self.overview_zoom();
        // wiri `unified-grid`: one camera for every row of a unified monitor, so the
        // rows line up in x. `None` (mode off, the default) keeps niri's per-row camera.
        let monitor_camera = self
            .unified_grid_for(output)
            .then(|| self.camera_offset(output));
        let workspaces =
            overview_workspace_stack(monitor, view, zoom, self.overview_camera.render_index);
        let mut positions = Vec::new();
        let mut clips = Vec::new();
        for entry in &workspaces {
            let (id, workspace_rect) = (entry.id, entry.rect);
            let Some(ws) = monitor.workspaces.get(&id) else {
                continue;
            };
            // Each workspace lays out inside its own configured insets (C2), then the
            // camera transform places that inside the shared stack entry.
            let cfg = self.effective_config_for_workspace(output, id);
            let workspace_view = Self::precise_tiling_area(monitor, cfg);
            let view_rect = Rect::new(
                workspace_view.loc.x,
                workspace_view.loc.y,
                workspace_view.width as u32,
                workspace_view.height as u32,
            );
            // The horizontal camera: the layout already applies this workspace's own
            // scroll offset (niri's `render_scrolling`), so only the difference a shared
            // monitor camera needs is applied here.  With `unified-grid` on that
            // difference is `monitor camera - this row's own offset`, which makes the
            // drawn x of every tile the same `strip x - monitor camera` in every row.
            let camera =
                overview_geometry::workspace_camera_adjustment(ws.scroll_offset.x, monitor_camera);
            let normal = self.calculate_positions_at_zoom(
                ws,
                workspace_view,
                0,
                workspace_view.height,
                cfg,
                1.0,
            );
            let mut push = |wid: WindowId, rect: Rect, camera: i32| {
                let panned = Rect::new(rect.loc.x - camera, rect.loc.y, rect.size.w, rect.size.h);
                let screen = overview_geometry::zoom_tile(panned, workspace_rect, view_rect, zoom);
                positions.push((wid, screen));
                clips.push((wid, workspace_rect));
            };
            for (wid, rect) in normal {
                push(wid, rect, camera);
            }
            // Floating windows are not part of the grid (`docs/design/unified-grid.md`):
            // their rects are already screen positions in their own workspace, so the
            // shared camera never shifts them (and never shifts them with the mode off).
            for (wid, rect) in self.float_homes.iter().filter_map(|(wid, home)| {
                (home.output == output && home.workspace == id)
                    .then(|| self.float_rect_of(*wid))
                    .flatten()
                    .map(|rect| (*wid, rect))
            }) {
                push(wid, rect, 0);
            }
        }
        // One session host exists, on the output the camera opened over: another
        // output's layout pass must not republish its own geometry to that host.
        let hosted = self.overview_camera.output.map_or(true, |o| o == output);
        if let (true, Some(sink)) = (hosted, &self.thumbnail_overview) {
            sink.set_appearance(self.overview_options(), zoom);
            sink.set_context(OverviewContext {
                bounds: monitor.bounds,
                workspaces,
                focused: monitor.focus_window,
            });
        }
        (positions, clips)
    }

    /// The workspace the overview currently draws under `point` (screen pixels), found in
    /// the same stack the host paints, so a drag lands on what the user sees at any zoom.
    pub(crate) fn overview_workspace_at(&self, point: Point) -> Option<(OutputId, i32)> {
        let (&output, monitor) = self
            .monitors
            .iter()
            .find(|(_, m)| m.bounds.contains_point(point))?;
        // The same camera view `workspace_overview_scene` stacks the workspaces in.
        overview_workspace_stack(
            monitor,
            monitor.work_area,
            self.overview_zoom(),
            self.overview_camera.render_index,
        )
        .into_iter()
        .find(|ws| point.y >= ws.rect.loc.y && point.y < ws.rect.bottom())
        .map(|ws| (output, ws.id))
    }

    /// Called by the session-local host, never while holding a host mutex.
    pub fn overview_command(&mut self, command: OverviewCommand, backend: &BackendHandle) {
        if !self.is_overview() {
            return;
        }
        match command {
            OverviewCommand::SelectWindow(wid) => {
                if self.floating_windows.contains(&wid) {
                    self.focus_float(wid, backend);
                    self.exit_overview(backend);
                } else if let Some((output, ws, _)) = self.locate_window(wid) {
                    self.monitors.set_focused(output);
                    if let Some(m) = self.monitors.get_mut(&output) {
                        m.switch_workspace(ws);
                        m.set_focus(wid);
                    }
                    self.exit_overview(backend);
                }
            }
            OverviewCommand::SelectWorkspace(ws) => {
                if let Some(m) = self.monitors.focused_mut() {
                    m.switch_workspace(ws);
                }
                self.exit_overview(backend);
            }
            OverviewCommand::Close => self.exit_overview(backend),
            OverviewCommand::ActivateWorkspace(ws) => {
                self.switch_workspace_in_overview(ws, backend);
            }
            OverviewCommand::DropWindow { window, target } => {
                self.overview_drop_window(window, target, backend);
            }
            OverviewCommand::ScrollView(delta) => self.overview_scroll_view(delta, backend),
            OverviewCommand::PreviousWorkspace => self.overview_step_row(false, backend),
            OverviewCommand::NextWorkspace => self.overview_step_row(true, backend),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inactive_thumbnail_uses_its_workspace_insets_before_camera_transform() {
        use crate::layout::{LayoutConfig, Workspace};
        let output = OutputId::from_name("overview-insets");
        let view = Rect::new(-1000, 20, 1000, 760);
        let mut engine = TilingEngine::new(LayoutConfig {
            outer_gaps: (0, 0, 0, 0),
            ..LayoutConfig::default()
        });
        engine.register_monitor(output, view, view);
        let id = WindowId::new(901);
        let mut workspace = Workspace::new();
        workspace.name = Some("insets".to_string());
        workspace.add_window_to_new_column_with_width(id, Some(300));
        engine
            .monitors
            .get_mut(&output)
            .unwrap()
            .workspaces
            .insert(1, workspace);
        let cfg = LayoutConfig {
            outer_gaps: (20, 30, 40, 50),
            reserve_left: 80,
            reserve_top: 40,
            ..engine.config.clone()
        };
        engine.config_per_workspace.insert(
            "insets".to_string(),
            std::collections::HashMap::from([(output, cfg.clone())]),
        );
        // The camera zoom is state now, not a constant: pin it at niri's default.
        let zoom = 0.5;
        engine.overview = Some(OverviewState {
            zoom,
            workspace_offsets: Vec::new(),
        });
        let monitor = engine.monitors.get(&output).unwrap();
        let inset = TilingEngine::tiling_work_rect(monitor, &cfg);
        let normal = engine.calculate_positions_at_zoom(
            &monitor.workspaces[&1],
            inset,
            0,
            inset.size.h as i32,
            &cfg,
            1.0,
        );
        // niri's stack over this view at the default half zoom: two entries, the
        // second one is workspace key 1 (src/layout/monitor.rs:1467).
        let rects = overview_geometry::workspace_rects(view, 2, 0.0, zoom);
        let camera = overview_geometry::workspace_camera_adjustment(
            monitor.workspaces[&1].scroll_offset.x,
            None,
        );
        let panned = Rect::new(
            normal[0].1.loc.x - camera,
            normal[0].1.loc.y,
            normal[0].1.size.w,
            normal[0].1.size.h,
        );
        let expected = overview_geometry::zoom_tile(panned, rects[1], inset, zoom);
        let (scene, clips) = engine.workspace_overview_scene(output);
        assert_eq!(
            scene.iter().find(|(wid, _)| *wid == id).unwrap().1,
            expected
        );
        // The tile is cropped to its own workspace rectangle.
        assert_eq!(
            clips.iter().find(|(wid, _)| *wid == id).unwrap().1,
            rects[1]
        );
        // Prove the fixture distinguishes the old active-workspace rectangle.
        let wrong = engine.calculate_positions_at_zoom(
            &monitor.workspaces[&1],
            view,
            0,
            view.size.h as i32,
            &cfg,
            1.0,
        );
        assert_ne!(
            overview_geometry::zoom_tile(wrong[0].1, rects[1], inset, zoom),
            expected
        );
        // The inactive workspace's insets really differ from the view the camera spans.
        assert_ne!(inset, view);
    }

    /// The workspace's own horizontal camera must be applied exactly once: the layout
    /// already subtracts `scroll_offset.x`, so the overview only rescales and relocates
    /// (niri's `render_workspaces`, src/layout/monitor.rs:1735).  Applying it twice put
    /// the focused column off the workspace (observed on the desktop probe).
    #[test]
    fn the_workspace_camera_is_applied_exactly_once() {
        use crate::layout::{LayoutConfig, Workspace};
        let output = OutputId::from_name("overview-camera-once");
        let view = Rect::new(0, 0, 1366, 728);
        let mut engine = TilingEngine::new(LayoutConfig::default());
        engine.register_monitor(output, view, view);
        let id = WindowId::new(902);
        let mut workspace = Workspace::new();
        workspace.add_window_to_new_column_with_width(id, Some(300));
        // As if the strip had been panned 700 px to the right.
        workspace.scroll_offset.x = 700;
        engine
            .monitors
            .get_mut(&output)
            .unwrap()
            .workspaces
            .insert(0, workspace);
        engine.overview = Some(OverviewState {
            zoom: 0.5,
            workspace_offsets: Vec::new(),
        });
        let (positions, clips) = engine.workspace_overview_scene(output);
        let ws = clips.iter().find(|(w, _)| *w == id).unwrap().1;
        let scaled = positions.iter().find(|(w, _)| *w == id).unwrap().1;
        let monitor = engine.monitors.get(&output).unwrap();
        let cfg = engine.effective_config_for_workspace(output, 0);
        let inset = TilingEngine::precise_tiling_area(monitor, cfg);
        let normal = engine.calculate_positions_at_zoom(
            &monitor.workspaces[&0],
            inset,
            0,
            inset.height,
            cfg,
            1.0,
        );
        let view_rect = Rect::new(
            inset.loc.x,
            inset.loc.y,
            inset.width as u32,
            inset.height as u32,
        );
        let expected = overview_geometry::zoom_tile(normal[0].1, ws, view_rect, 0.5);
        assert_eq!(scaled, expected);
        // 700 px of camera at half zoom is 350 px: the column is scrolled out of the
        // workspace and therefore cropped away, not drawn inside it.
        assert!(
            overview_geometry::visible_tile(scaled, ws).is_none(),
            "{scaled:?} should be left of {ws:?}"
        );
    }

    #[test]
    fn overview_camera_uses_shared_spring_and_retires_after_close() {
        use crate::layout::{Column, Monitor, Tile};
        use crate::overlay::MockThumbnailOverview;
        use std::sync::Arc;
        let output = OutputId::from_name("camera");
        let mut e = TilingEngine::new(Default::default());
        let mut m = Monitor::new(
            output,
            Rect::new(0, 0, 1366, 768),
            Rect::new(0, 0, 1366, 728),
        );
        let mut col = Column::new();
        col.add_tile(Tile::new(WindowId::new(0)));
        m.workspace_mut().unwrap().columns.push(col);
        e.monitors.insert(output, m);
        e.monitors.set_focused(output);
        let sink = MockThumbnailOverview::new();
        e.thumbnail_overview = Some(sink.clone());
        e.set_animation_notifier(Arc::new(tokio::sync::Notify::new()));
        let backend = BackendHandle::default_for_test();
        e.enter_overview(&backend);
        assert_eq!(e.overview_zoom(), 1.0);
        assert!(e.has_active_animations());
        e.tick_animations_with_backend(50, &backend);
        assert!(e.overview_zoom() > 0.5 && e.overview_zoom() < 1.0);
        let zoom = e.overview_zoom();
        e.exit_overview(&backend);
        // Reversing preserves the displayed position and reuses the host/session.
        assert_eq!(e.overview_zoom(), zoom);
        e.tick_animations_with_backend(30, &backend);
        let reversing = e.overview_zoom();
        e.toggle_overview(&backend);
        assert_eq!(e.overview_zoom(), reversing);
        assert!(!e.overview_camera.closing);
        assert_eq!(sink.enter_calls(), 1);
        e.tick_animations_with_backend(60_000, &backend);
        assert!(e.is_overview());
        assert!(!e.has_active_animations());
        e.exit_overview(&backend);
        e.tick_animations_with_backend(60_000, &backend);
        assert!(!e.has_active_animations());
        assert!(!e.is_overview());
        assert_eq!(sink.exit_calls(), 1);
    }

    fn two_workspace_engine(zoom: f64) -> (TilingEngine, OutputId) {
        use crate::layout::{Monitor, Workspace};
        let output = OutputId::from_name("stack");
        let mut e = TilingEngine::new(Default::default());
        let mut m = Monitor::new(
            output,
            Rect::new(0, 0, 1366, 768),
            Rect::new(0, 0, 1366, 728),
        );
        m.insert_workspace(1, Workspace::new());
        e.monitors.insert(output, m);
        e.monitors.set_focused(output);
        let mut config = crate::config::Config::default();
        config.overview.zoom = zoom;
        e.set_full_config(config);
        (e, output)
    }

    #[test]
    fn workspace_stack_follows_visible_order_names_and_ipc_identity() {
        use crate::layout::{Monitor, Workspace};
        let output = OutputId::from_name("order");
        let mut m = Monitor::new(
            output,
            Rect::new(-1366, 0, 1366, 768),
            Rect::new(-1366, 0, 1366, 728),
        );
        m.insert_workspace(5, Workspace::new());
        m.insert_workspace(2, Workspace::new());
        m.move_workspace_to_position(5, 0);
        m.workspaces.get_mut(&0).unwrap().name = Some("web".into());
        m.workspaces.get_mut(&2).unwrap().name = Some(String::new());
        let view = Rect::new(-1366, 0, 1366, 728);
        // niri computes the stack from the focused workspace's index (its
        // `workspace_render_idx`, src/layout/monitor.rs:1467); key 0 is the second row.
        let stack = overview_workspace_stack(&m, view, 0.5, 1.0);
        // Visible order [5, 0, 2]; the active one (key 0) is centred on the view.
        assert_eq!(
            stack.iter().map(|w| w.id).collect::<Vec<_>>(),
            vec![5, 0, 2]
        );
        assert_eq!(
            stack.iter().map(|w| w.label.as_str()).collect::<Vec<_>>(),
            vec!["Workspace 1", "web", "Workspace 3"]
        );
        for w in &stack {
            assert_eq!(w.ipc_id, m.workspaces[&w.id].ipc_id);
            assert_eq!(w.active, w.id == m.active_workspace);
        }
        assert_eq!(stack[1].rect, Rect::new(-1024, 182, 683, 364));
        // niri's gap between workspaces is 10% of the view height, scaled by the
        // zoom (src/layout/monitor.rs:1335): round(728 * 0.1 * 0.5) = 36.
        assert_eq!(stack[0].rect.loc.y, 182 - 400);
        assert_eq!(stack[2].rect.loc.y, 182 + 400);
    }

    #[test]
    fn drag_targets_use_the_configured_zoom_not_a_fixed_half_scale() {
        let (mut e, output) = two_workspace_engine(0.35);
        let sink = crate::overlay::MockThumbnailOverview::new();
        e.thumbnail_overview = Some(sink.clone());
        e.set_animation_notifier(std::sync::Arc::new(tokio::sync::Notify::new()));
        let backend = BackendHandle::default_for_test();
        e.enter_overview(&backend);
        e.tick_animations_with_backend(60_000, &backend);
        assert_eq!(e.overview_zoom(), 0.35);

        // The camera spans the monitor's whole work area (C2): a workspace's own insets
        // move its tiles inside its stack entry, never the entry itself.
        let view = e.monitors.get(&output).unwrap().work_area;
        let stack = overview_workspace_stack(e.monitors.get(&output).unwrap(), view, 0.35, 0.0);
        assert_eq!(stack.len(), 2);
        // The host was handed exactly this stack, so paint, pointer and DnD agree.
        assert_eq!(sink.last_context().unwrap().workspaces, stack);
        e.monitors
            .get_mut(&output)
            .unwrap()
            .workspaces
            .get_mut(&1)
            .unwrap()
            .name = Some("inset".to_string());
        let inset = crate::layout::LayoutConfig {
            outer_gaps: (20, 30, 40, 50),
            reserve_left: 80,
            ..e.config.clone()
        };
        e.config_per_workspace.insert(
            "inset".to_string(),
            std::collections::HashMap::from([(output, inset)]),
        );
        e.apply_layout_for_monitor(output, &backend);
        let renamed: Vec<_> = sink.last_context().unwrap().workspaces;
        assert_eq!(
            renamed.iter().map(|w| w.rect).collect::<Vec<_>>(),
            stack.iter().map(|w| w.rect).collect::<Vec<_>>(),
            "insets do not move the stack"
        );
        assert_eq!(renamed[1].label, "inset");
        let (first, second) = (&stack[0], &stack[1]);
        assert!(first.rect.size.h < 300, "0.35 is not the half-scale 364");
        let mid = |r: Rect| Point::new(r.loc.x + 10, r.loc.y + r.size.h as i32 / 2);
        assert_eq!(e.overview_workspace_at(mid(first.rect)), Some((output, 0)));
        assert_eq!(e.overview_workspace_at(mid(second.rect)), Some((output, 1)));
        // The gap between two workspaces, and the area outside every monitor, hit nothing.
        let gap = Point::new(10, first.rect.bottom() + 1);
        assert!(gap.y < second.rect.loc.y);
        assert_eq!(e.overview_workspace_at(gap), None);
        assert_eq!(e.overview_workspace_at(Point::new(-5000, 10)), None);
        // The same lookup backs the native drag-and-drop adapter.
        assert!(matches!(
            e.dnd_target(mid(second.rect)),
            Some(crate::input::dnd::Target::Workspace(o, 1)) if o == output
        ));
    }

    #[test]
    fn only_the_hosted_output_publishes_overview_geometry() {
        let (mut e, output) = two_workspace_engine(0.5);
        let other = OutputId::from_name("other");
        e.monitors.insert(
            other,
            crate::layout::Monitor::new(
                other,
                Rect::new(1366, 0, 800, 600),
                Rect::new(1366, 0, 800, 560),
            ),
        );
        let sink = crate::overlay::MockThumbnailOverview::new();
        e.thumbnail_overview = Some(sink.clone());
        e.set_animation_notifier(std::sync::Arc::new(tokio::sync::Notify::new()));
        let backend = BackendHandle::default_for_test();
        e.enter_overview(&backend);
        let hosted = sink.last_context().unwrap().bounds;
        // A layout pass for the other output must not replace the host's geometry.
        e.apply_layout_for_monitor(other, &backend);
        e.apply_layout_for_monitor(output, &backend);
        assert_eq!(sink.last_context().unwrap().bounds, hosted);
        assert_eq!(hosted, Rect::new(0, 0, 1366, 768));
    }

    #[test]
    fn the_overview_opening_and_closing_are_announced_once_each() {
        use crate::a11y::testing::contains_in_order;
        let (sink, _guard) = crate::a11y::testing::record();
        let (mut e, _output) = two_workspace_engine(0.35);
        e.thumbnail_overview = Some(crate::overlay::MockThumbnailOverview::new());
        e.set_animation_notifier(std::sync::Arc::new(tokio::sync::Notify::new()));
        let backend = BackendHandle::default_for_test();
        e.enter_overview(&backend);
        assert!(
            contains_in_order(&sink.texts(), &[crate::a11y::OVERVIEW_OPENED]),
            "{:?}",
            sink.texts()
        );
        // A second enter while open is not a new announcement.
        let openings_before = sink
            .texts()
            .iter()
            .filter(|text| text.as_str() == crate::a11y::OVERVIEW_OPENED)
            .count();
        e.enter_overview(&backend);
        let openings_after = sink
            .texts()
            .iter()
            .filter(|text| text.as_str() == crate::a11y::OVERVIEW_OPENED)
            .count();
        assert_eq!(openings_after, openings_before);
        // The close is announced when the camera has finished closing, as niri does.
        e.exit_overview(&backend);
        e.tick_animations_with_backend(60_000, &backend);
        assert!(
            contains_in_order(
                &sink.texts(),
                &[crate::a11y::OVERVIEW_OPENED, crate::a11y::OVERVIEW_CLOSED]
            ),
            "{:?}",
            sink.texts()
        );
    }

    #[test]
    fn overview_scales_half_and_preserves_normal_aspect_and_negative_origins() {
        let view = Rect::new(-1366, 0, 1366, 728);
        let tile = Rect::new(-1358, 8, 500, 700);
        // niri relocates every tile into its workspace rectangle and rescales by the
        // zoom (src/layout/monitor.rs:1735-1745).
        let ws = Rect::new(-1024, 182, 683, 364);
        assert_eq!(
            overview_geometry::zoom_tile(tile, ws, view, 0.5),
            Rect::new(-1020, 186, 250, 350)
        );
        assert_eq!(
            overview_geometry::zoom_tile(view, ws, view, 0.5),
            Rect::new(-1024, 182, 683, 364)
        );
    }

    /// Three rows of different column counts (4, 2 and 3) of 400 px columns 16 px apart on
    /// a 1000 px wide view, each row with its own remembered offset: the fixture for the
    /// unified camera and the diagonal actions in the overview.
    fn grid_overview_engine(unified: bool) -> (TilingEngine, BackendHandle, OutputId) {
        use crate::layout::{ColumnWidthMode, Monitor, Workspace};
        let backend = BackendHandle::default_for_test();
        let output = OutputId::from_name("overview-grid");
        let mut e = TilingEngine::new(crate::layout::LayoutConfig {
            unified_grid: unified,
            column_width: 400,
            column_width_mode: ColumnWidthMode::Fixed,
            column_gap: 16,
            outer_gaps: (0, 0, 0, 0),
            ..crate::layout::LayoutConfig::default()
        });
        let mut m = Monitor::new(
            output,
            Rect::new(0, 0, 1000, 800),
            Rect::new(0, 0, 1000, 760),
        );
        // Row 0: 1000..1004, row 1: 1100..1102, row 2: 1200..1203. Each row's own
        // remembered offset is a valid one for its own strip and different from the others.
        for (workspace, count, first, offset) in
            [(0, 4, 1000, 100), (1, 2, 1100, 0), (2, 3, 1200, 200)]
        {
            let mut ws = Workspace::new();
            for index in 0..count {
                ws.add_window_to_new_column_with_width(WindowId::new(first + index), Some(400));
            }
            ws.scroll_offset.x = offset;
            m.insert_workspace(workspace, ws);
        }
        e.monitors.insert(output, m);
        e.monitors.set_focused(output);
        (e, backend, output)
    }

    /// Open the camera state without a host, for tests that only read the scene or move the
    /// camera: `enter_overview` needs a DWM sink, these do not.
    fn overview_at_half(engine: &mut TilingEngine) {
        engine.overview = Some(OverviewState {
            zoom: 0.5,
            workspace_offsets: Vec::new(),
        });
    }

    /// The drawn x of every tile of the scene, keyed by window.
    fn scene_x(
        engine: &TilingEngine,
        output: OutputId,
    ) -> std::collections::HashMap<WindowId, i32> {
        engine
            .workspace_overview_scene(output)
            .0
            .into_iter()
            .map(|(id, rect)| (id, rect.loc.x))
            .collect()
    }

    /// wiri `unified-grid` in the overview: one camera for every row, so column k of three
    /// rows with different column counts is drawn at the same screen x. With the mode off
    /// every row keeps its own camera (niri's behaviour, unchanged).
    #[test]
    fn unified_grid_draws_every_row_with_the_monitor_camera() {
        let (mut e, _backend, output) = grid_overview_engine(true);
        overview_at_half(&mut e);
        // The monitor's camera is the active row's offset; pin it to a known x.
        e.monitors.get_mut(&output).unwrap().active_workspace = 0;
        e.set_camera_offset(output, 316);
        let x = scene_x(&e, output);
        // Columns 0 and 1 exist in every row (4, 2 and 3 columns).
        for (row0, row1, row2) in [(1000, 1100, 1200), (1001, 1101, 1201)] {
            assert_eq!(x[&WindowId::new(row0)], x[&WindowId::new(row1)]);
            assert_eq!(x[&WindowId::new(row1)], x[&WindowId::new(row2)]);
        }
        // The rows still hold their own tile counts: an empty slot stays empty.
        assert_eq!(x.len(), 9);
        // The camera really is the monitor's: panning it moves every row by the same x.
        let before = x[&WindowId::new(1200)];
        e.set_camera_offset(output, 516);
        let after = scene_x(&e, output)[&WindowId::new(1200)];
        // 200 strip px at the half zoom.
        assert_eq!(before - after, 100);

        // Mode off: niri's per-row camera, so the same columns sit at different x.
        let (mut e, _backend, output) = grid_overview_engine(false);
        overview_at_half(&mut e);
        let x = scene_x(&e, output);
        assert_ne!(x[&WindowId::new(1000)], x[&WindowId::new(1100)]);
        assert_ne!(x[&WindowId::new(1100)], x[&WindowId::new(1200)]);
    }

    /// wiri `unified-grid` rule 4 in the overview: `focus-up-right` resolves the row and the
    /// column before anything moves, so the zoomed-out camera pans in x and y as one motion
    /// and the focus lands on the window diagonally up-and-right.
    #[test]
    fn a_diagonal_focus_in_the_overview_pans_the_camera_and_lands_on_the_window() {
        use crate::input::GridAction;
        use crate::overlay::MockThumbnailOverview;
        let (mut e, backend, output) = grid_overview_engine(true);
        // niri's motion on, so both legs of the diagonal run as animations rather than
        // snapping: this is what "one camera motion" means for the assertion below.
        e.configure_motion(&crate::config::AnimationsConfig {
            enabled: true,
            legacy_easing: true,
            duration: 100,
            easing: "linear".into(),
            ..Default::default()
        });
        e.install_thumbnail_overview(MockThumbnailOverview::new());
        e.set_animation_notifier(std::sync::Arc::new(tokio::sync::Notify::new()));
        // Row 1 (two columns) with its second column focused, at the monitor camera 0.
        {
            let monitor = e.monitors.get_mut(&output).unwrap();
            monitor.switch_workspace(1);
            monitor.focus_column = Some(1);
            monitor.focus_window = Some(WindowId::new(1101));
            monitor.workspaces.get_mut(&1).unwrap().scroll_offset.x = 0;
        }
        e.enter_overview(&backend);
        e.tick_animations_with_backend(60_000, &backend);
        assert!(e.has_thumbnail_overview());
        assert_eq!(e.overview_zoom(), 0.5);
        assert_eq!(e.camera_offset(output), 0);
        // The camera opened centred on the focused row (row 1 of 0,1,2).
        assert_eq!(e.overview_camera.render_index, 1.0);

        // The anchor is column 1's centre (616); row 0 matches column 1, one column right
        // is column 2 (window 1002), whose right edge is past the 1000 px view.
        e.apply_grid_action(GridAction::new(true, true, false), false, &backend);
        let monitor = e.monitors.get(&output).unwrap();
        assert_eq!(monitor.active_workspace, 0);
        assert_eq!(monitor.focus_window, Some(WindowId::new(1002)));
        // Both legs are in flight from the one action: the stack slides up to row 0 and the
        // x camera is retargeted towards column 2 (the x animation starts at the offset the
        // camera had, so nothing has jumped yet).
        assert_eq!(
            e.overview_camera_target(output),
            Some(0.0),
            "y did not move"
        );
        assert!(
            e.overview_camera.animating(),
            "one motion should be running"
        );
        assert_eq!(e.camera_offset(output), 0, "x jumped instead of animating");
        // One tick pair moves both: 232 strip px is what brings column 2's right edge
        // (1232) onto the 1000 px view's edge, and the stack arrives at row 0.
        e.tick_animations_with_backend(60_000, &backend);
        assert_eq!(e.camera_offset(output), 232, "x did not arrive");
        assert_eq!(e.overview_camera.render_index, 0.0, "y did not arrive");
        // A zoomed-out row of 1000 px now shows column 2 completely.
        assert!(e.camera_offset(output) + 1000 >= 832 + 400);
        // Nothing else moved focus: the window is focused on its own row.
        assert_eq!(e.monitors.get(&output).unwrap().focus_column, Some(2));
    }

    /// wiri `unified-grid` rule 6 in the overview: the vertical axis of a two-finger scroll
    /// steps the row and carries the one camera, then the horizontal axis pans that same
    /// camera, so every row moves with it instead of only the active one.
    #[test]
    fn a_two_axis_scroll_in_the_overview_pans_the_one_shared_camera() {
        let (mut e, backend, output) = grid_overview_engine(true);
        overview_at_half(&mut e);
        {
            let monitor = e.monitors.get_mut(&output).unwrap();
            monitor.switch_workspace(1);
            monitor.workspaces.get_mut(&1).unwrap().scroll_offset.x = 0;
        }
        // The vertical axis: one row down (niri's `NextWorkspace`), camera carried, not
        // row 2's own remembered offset.
        e.overview_command(super::OverviewCommand::NextWorkspace, &backend);
        assert_eq!(e.monitors.get(&output).unwrap().active_workspace, 2);
        assert_eq!(e.camera_offset(output), 0);
        // The horizontal axis: 60 screen px at the half zoom is 120 strip px of the one
        // camera, so the drawn x of every row follows.
        e.overview_scroll_view(60, &backend);
        assert_eq!(e.camera_offset(output), 120);
        let x = scene_x(&e, output);
        assert_eq!(x[&WindowId::new(1000)], x[&WindowId::new(1200)]);
        // Mode off: the vertical step lands on the row's own camera, and the horizontal
        // scroll moves only that row, which is niri's per-row behaviour.
        let (mut e, backend, output) = grid_overview_engine(false);
        overview_at_half(&mut e);
        {
            let monitor = e.monitors.get_mut(&output).unwrap();
            monitor.switch_workspace(1);
        }
        e.overview_command(super::OverviewCommand::NextWorkspace, &backend);
        assert_eq!(e.camera_offset(output), 200);
        e.overview_scroll_view(60, &backend);
        assert_eq!(e.camera_offset(output), 320);
        let x = scene_x(&e, output);
        assert_ne!(x[&WindowId::new(1000)], x[&WindowId::new(1200)]);
    }
}
