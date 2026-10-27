//! The overview is a camera over a vertically stacked set of normal workspace views.
//! It never reconfigures applications to thumbnail-sized rectangles.
use super::{AppliedState, BackendHandle, OverviewState, TilingEngine};
use crate::overlay::thumbnail_overview::{OverviewCommand, OverviewContext, OverviewWorkspace};
use crate::utils::{OutputId, Point, Rect, WindowId};

use crate::layout::animation::Animation;

const WORKSPACE_GAP: i32 = 50;

#[derive(Default)]
pub(super) struct OverviewCamera {
    pub timeline: Option<Animation>,
    pub closing: bool,
    pub output: Option<OutputId>,
}
impl OverviewCamera {
    pub fn animating(&self) -> bool {
        self.timeline.as_ref().is_some_and(|a| !a.is_finished())
    }
}

/// The monitor's workspaces as one vertical stack in visible order, centred on the active
/// one. Pure: the host paints it, the pointer hit-tests it, drag-and-drop targets it and
/// the engine lays tiles out inside it, so the four never disagree about where a workspace
/// is. Rows carry the stable key, the IPC identity and the name (else "Workspace N").
pub(super) fn overview_workspace_stack(
    monitor: &crate::layout::Monitor,
    view: Rect,
    zoom: f64,
) -> Vec<OverviewWorkspace> {
    let ids = monitor.workspace_ids();
    let active = ids
        .iter()
        .position(|id| *id == monitor.active_workspace)
        .unwrap_or(0) as i32;
    let height = (view.size.h as f64 * zoom).round() as i32;
    let stride = height + (WORKSPACE_GAP as f64 * zoom).round() as i32;
    let center_y = view.loc.y + (view.size.h as i32 - height) / 2;
    ids.into_iter()
        .enumerate()
        .filter_map(|(index, id)| {
            let ws = monitor.workspaces.get(&id)?;
            let y = center_y + (index as i32 - active) * stride;
            Some(OverviewWorkspace {
                id,
                ipc_id: ws.ipc_id,
                label: ws
                    .name
                    .clone()
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("Workspace {}", index + 1)),
                active: id == monitor.active_workspace,
                rect: zoom_rect(view, view, y, zoom),
            })
        })
        .collect()
}

fn zoom_rect(rect: Rect, view: Rect, section_y: i32, zoom: f64) -> Rect {
    Rect::new(
        view.loc.x
            + (view.size.w as f64 * (1.0 - zoom) / 2.0).round() as i32
            + ((rect.loc.x - view.loc.x) as f64 * zoom).round() as i32,
        section_y + ((rect.loc.y - view.loc.y) as f64 * zoom).round() as i32,
        (rect.size.w as f64 * zoom).round().max(1.0) as u32,
        (rect.size.h as f64 * zoom).round().max(1.0) as u32,
    )
}

impl TilingEngine {
    fn overview_options(&self) -> crate::config::overview_config::OverviewConfig {
        self.full_config
            .as_ref()
            .map(|c| c.overview)
            .unwrap_or_default()
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
                self.overview_options().zoom
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
        let Some(a) = &mut self.overview_camera.timeline else {
            return;
        };
        if a.is_finished() {
            return;
        }
        let zoom = a.tick(delta_ms).clamp(0.0, 1.0);
        let finished = a.is_finished();
        if let Some(state) = &mut self.overview {
            state.zoom = zoom;
        }
        if finished && self.overview_camera.closing {
            self.finish_overview_close(backend);
        } else if let Some(output) = self.overview_camera.output {
            self.apply_layout_for_monitor(output, backend);
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
                self.start_overview_camera(self.overview_options().zoom);
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
        self.overview_camera.output = Some(output);
        self.start_overview_camera(self.overview_options().zoom);
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

    pub(super) fn workspace_overview_scene(&self, output: OutputId) -> Vec<(WindowId, Rect)> {
        let Some(monitor) = self.monitors.get(&output) else {
            return Vec::new();
        };
        // The camera spans the full work area; each workspace resolves its own
        // insets inside it (C2), rather than inheriting the active workspace's view.
        let view = monitor.work_area;
        let zoom = self.overview_zoom();
        let workspaces = overview_workspace_stack(monitor, view, zoom);
        let mut positions = Vec::new();
        for entry in &workspaces {
            let (id, y) = (entry.id, entry.rect.loc.y);
            let Some(ws) = monitor.workspaces.get(&id) else {
                continue;
            };
            // Each workspace lays out inside its own configured insets (C2), then the
            // camera transform places that inside the shared stack entry.
            let cfg = self.effective_config_for_workspace(output, id);
            let workspace_view = Self::tiling_work_rect(monitor, cfg);
            let normal = self.calculate_positions_at_zoom(
                ws,
                workspace_view,
                0,
                workspace_view.size.h as i32,
                cfg,
                1.0,
            );
            positions.extend(
                normal
                    .into_iter()
                    .map(|(wid, rect)| (wid, zoom_rect(rect, view, y, zoom))),
            );
            positions.extend(self.float_homes.iter().filter_map(|(wid, home)| {
                (home.output == output && home.workspace == id)
                    .then(|| self.float_rect_of(*wid))
                    .flatten()
                    .map(|rect| (*wid, zoom_rect(rect, view, y, zoom)))
            }));
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
        positions
    }

    /// The workspace the overview currently draws under `point` (screen pixels), found in
    /// the same stack the host paints, so a drag lands on what the user sees at any zoom.
    pub(crate) fn overview_workspace_at(&self, point: Point) -> Option<(OutputId, i32)> {
        let (&output, monitor) = self
            .monitors
            .iter()
            .find(|(_, m)| m.bounds.contains_point(point))?;
        // The same camera view `workspace_overview_scene` stacks the workspaces in.
        overview_workspace_stack(monitor, monitor.work_area, self.overview_zoom())
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
            OverviewCommand::PreviousWorkspace | OverviewCommand::NextWorkspace => {
                let Some(output) = self.monitors.focused_id() else {
                    return;
                };
                if let Some(m) = self.monitors.get_mut(&output) {
                    let ids = m.workspace_ids();
                    let index = ids
                        .iter()
                        .position(|id| *id == m.active_workspace)
                        .unwrap_or(0);
                    let next = if command == OverviewCommand::NextWorkspace {
                        (index + 1).min(ids.len().saturating_sub(1))
                    } else {
                        index.saturating_sub(1)
                    };
                    if let Some(id) = ids.get(next) {
                        m.switch_workspace(*id);
                    }
                }
                self.apply_layout_for_monitor(output, backend);
            }
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
        let height = (view.size.h as f64 * zoom).round() as i32;
        let y = view.loc.y
            + (view.size.h as i32 - height) / 2
            + height
            + (WORKSPACE_GAP as f64 * zoom).round() as i32;
        let expected = zoom_rect(normal[0].1, view, y, zoom);
        let scene = engine.workspace_overview_scene(output);
        assert_eq!(
            scene.iter().find(|(wid, _)| *wid == id).unwrap().1,
            expected
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
        assert_ne!(zoom_rect(wrong[0].1, view, y, zoom), expected);
        // The inactive workspace's insets really differ from the view the camera spans.
        assert_ne!(inset, view);
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
        let stack = overview_workspace_stack(&m, view, 0.5);
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
        // 50 px niri gap scaled with the workspaces: 364 + 25.
        assert_eq!(stack[0].rect.loc.y, 182 - 389);
        assert_eq!(stack[2].rect.loc.y, 182 + 389);
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
        let stack = overview_workspace_stack(e.monitors.get(&output).unwrap(), view, 0.35);
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
    fn overview_scales_half_and_preserves_normal_aspect_and_negative_origins() {
        let view = Rect::new(-1366, 0, 1366, 728);
        let tile = Rect::new(-1358, 8, 500, 700);
        assert_eq!(
            zoom_rect(tile, view, 182, 0.5),
            Rect::new(-1020, 186, 250, 350)
        );
        assert_eq!(
            zoom_rect(view, view, 182, 0.5),
            Rect::new(-1024, 182, 683, 364)
        );
    }
}
