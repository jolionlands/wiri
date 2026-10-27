//! What the overlay layer is asked to draw after a layout pass: the focus ring, the border
//! band and the shadow of every tile that is on a screen.
//!
//! The decisions are in [`frame_specs`], a pure function of the tiles and the style, so that
//! they are tested without a desktop; [`TilingEngine::refresh_decorations`] collects the
//! inputs from the engine and hands the result to the layer.
//!
//! niri's rules (docs/wiki/Configuration:-Layout.md): the focus ring is drawn around the
//! active window of each monitor only (in the inactive colour when that monitor is not the one
//! with the keyboard focus), the border around every window; an urgent window uses the urgent
//! colour; a fullscreen window and a window maximized to the edges have neither.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::{DrawMode, LayoutConfig, TilingEngine};
use crate::config::RingStyle;
use crate::overlay::decor::paint::{Paint, Rgba};
use crate::overlay::decor::{paint_of, DecorSink, DecorSpec, FrameSpec, ShadowSpec};
use crate::utils::{OutputId, Rect, WindowId};

/// What `frame_specs` needs to know about the monitor.
pub(super) struct FrameInputs<'a> {
    pub cfg: &'a LayoutConfig,
    /// This monitor has the keyboard focus.
    pub focus_monitor: bool,
    /// The active window of the monitor.
    pub active: Option<WindowId>,
    pub urgent: &'a HashSet<WindowId>,
    pub monitor_bounds: Rect,
    /// The area the strip is shown in (gradients can be relative to it).
    pub view: Rect,
    /// The Windows accent colour, for the colour name `accent`.
    pub accent: Option<[u8; 4]>,
    /// The corner radius of the windows themselves.
    pub radius: f32,
    /// The workspace holds one window alone (`smart-borders` leaves it bare).
    pub lone_tile: bool,
}

/// The paint a ring-like style gives a window in its present state.
fn paint_for(
    style: &RingStyle,
    urgent: bool,
    active: bool,
    accent: Option<[u8; 4]>,
) -> Option<Paint> {
    let which = if urgent {
        &style.urgent
    } else if active {
        &style.active
    } else {
        &style.inactive
    };
    paint_of(which, accent)
}

/// The decoration of each tile in `tiles` (the tile rectangle includes the border band).
/// Sorted by window, so that the same input gives the same list.
pub(super) fn frame_specs(
    inputs: &FrameInputs,
    tiles: &[(WindowId, Rect, DrawMode)],
) -> Vec<DecorSpec> {
    let fx = &inputs.cfg.effects;
    let band = inputs.cfg.border_inset();
    let mut out = Vec::new();
    if inputs.lone_tile && inputs.cfg.smart_borders {
        return out;
    }
    for &(wid, tile, mode) in tiles {
        if mode != DrawMode::Inset || tile.size.w == 0 || tile.size.h == 0 {
            continue;
        }
        // Scrolled out of view (parked below the screens): nothing to decorate.
        if !tile.intersects(inputs.monitor_bounds) {
            continue;
        }
        let is_active_window = inputs.active == Some(wid);
        let focused = is_active_window && inputs.focus_monitor;
        let urgent = inputs.urgent.contains(&wid);

        let border = if !fx.border.off && band > 0 {
            paint_for(&fx.border, urgent, focused, inputs.accent)
        } else {
            None
        };
        let ring_width = fx.focus_ring.width as i32;
        let ring = if !fx.focus_ring.off && ring_width > 0 && is_active_window {
            paint_for(&fx.focus_ring, urgent, inputs.focus_monitor, inputs.accent)
        } else {
            None
        };
        let shadow = if fx.shadow.on {
            shadow_of(&fx.shadow, focused)
        } else {
            None
        };
        if border.is_none() && ring.is_none() && shadow.is_none() {
            continue;
        }
        out.push(DecorSpec::frame(
            wid.as_isize(),
            FrameSpec {
                tile,
                band,
                radius: inputs.radius,
                corners: None,
                border,
                ring_width: if ring.is_some() { ring_width } else { 0 },
                ring,
                shadow,
                view: inputs.view,
            },
        ));
    }
    out.sort_by_key(|s| s.id.key);
    out
}

fn shadow_of(style: &crate::config::ShadowStyle, active: bool) -> Option<ShadowSpec> {
    let text = if active {
        &style.color
    } else {
        style.inactive_color.as_ref().unwrap_or(&style.color)
    };
    let mut color = Rgba::from_array(crate::config::parse_css_color(text)?);
    // niri: without an `inactive-color` the shadow of an inactive window is a quarter fainter.
    if !active && style.inactive_color.is_none() {
        color = color.faded(0.75);
    }
    Some(ShadowSpec {
        softness: style.softness as f32,
        spread: style.spread as f32,
        offset_x: style.offset_x as f32,
        offset_y: style.offset_y as f32,
        color,
        draw_behind_window: style.draw_behind_window,
    })
}

/// What DWM does with the radius a user asks for: the corner preference to set, and the radius
/// the window then has.  DWM knows three: square, small (4 px) and round (8 px).
pub(super) fn dwm_corner_for(requested: u32) -> (i32, f32) {
    match requested {
        0 => (1, 0.0),     // DWMWCP_DONOTROUND
        1..=5 => (3, 4.0), // DWMWCP_ROUNDSMALL
        _ => (2, 8.0),     // DWMWCP_ROUND
    }
}

/// The corner radius of the windows on this machine: what the user asked for where DWM
/// can do it, otherwise what Windows gives (8 px on Windows 11, none before).
pub(super) fn window_radius(requested: Option<u32>, rounded_corners: bool) -> f32 {
    match (requested, rounded_corners) {
        (_, false) => 0.0,
        (Some(n), true) => dwm_corner_for(n).1,
        (None, true) => 8.0,
    }
}

/// The indicator belongs to one column. The selected tab chooses the active paint; all
/// tabs in a column without keyboard focus are dimmed. Urgency overrides the tab selection.
fn tab_specs(
    cfg: &LayoutConfig,
    workspace: &crate::layout::Workspace,
    positions: &HashMap<WindowId, Rect>,
    modes: &HashMap<WindowId, DrawMode>,
    urgent: &HashSet<WindowId>,
    focused_output: bool,
    focused_window: Option<WindowId>,
    view: Rect,
    accent: Option<[u8; 4]>,
    rule_states: Option<&HashMap<WindowId, super::rules::RuleState>>,
) -> Vec<DecorSpec> {
    use crate::layout::workspace::ColumnDisplay;
    use crate::overlay::decor::TabsSpec;
    let style = &cfg.effects.tab_indicator;
    if style.off {
        return Vec::new();
    }
    workspace
        .columns
        .iter()
        .filter_map(|col| {
            let ColumnDisplay::Tabbed { active_tab } = col.display else {
                return None;
            };
            if col.tiles.is_empty() || (style.hide_when_single_tab && col.tiles.len() == 1) {
                return None;
            }
            let active = col.tiles.get(active_tab)?.window_id;
            if col.tiles.iter().any(|tile| {
                modes
                    .get(&tile.window_id)
                    .map_or(false, |mode| *mode != DrawMode::Inset)
            }) {
                return None;
            }
            let area = *positions.get(&active)?;
            if !area.intersects(view) {
                return None;
            }
            // niri falls back to the enabled focus ring, otherwise to the border,
            // even when both frame effects are disabled.
            let fallback = if !cfg.effects.focus_ring.off {
                &cfg.effects.focus_ring
            } else {
                &cfg.effects.border
            };
            let column_is_active = focused_output
                && col
                    .tiles
                    .iter()
                    .any(|tile| Some(tile.window_id) == focused_window);
            let paints = col
                .tiles
                .iter()
                .enumerate()
                .map(|(i, tile)| {
                    let mut style = style.clone();
                    if let Some(state) = rule_states.and_then(|s| s.get(&tile.window_id)) {
                        state.resolved.effects.tab_indicator.apply(&mut style);
                    }
                    let is_urgent = urgent.contains(&tile.window_id);
                    let is_selected = i == active_tab;
                    let paint = if is_urgent {
                        style.urgent.as_ref()
                    } else if is_selected {
                        style.active.as_ref()
                    } else {
                        style.inactive.as_ref()
                    };
                    let paint = paint
                        .and_then(|s| paint_of(s, accent))
                        .or_else(|| paint_for(fallback, is_urgent, is_selected, accent))
                        .unwrap_or(Paint::Solid(Rgba::new(128, 200, 255, 255)));
                    if column_is_active {
                        paint
                    } else {
                        paint.faded(0.5)
                    }
                })
                .collect();
            Some(DecorSpec::tabs(
                col.tiles[0].window_id.as_isize(),
                active.as_isize(),
                TabsSpec {
                    area,
                    position: style.position,
                    width: style.width.round() as i32,
                    gap: style.gap,
                    gaps_between: style.gaps_between_tabs,
                    length: style.length,
                    corner_radius: style.corner_radius as f32,
                    tabs: paints,
                    windows: col.tiles.iter().map(|tile| tile.window_id).collect(),
                    view,
                },
            ))
        })
        .collect()
}

/// Visible column geometry shared by drop hit-testing and the insert hint. Hidden tabs can
/// retain stale bounds from a previous layout; they must not influence either destination.
pub(super) fn column_drop_bounds(column: &crate::layout::Column) -> Option<Rect> {
    let mut rects = column
        .visible_tile_indices()
        .into_iter()
        .map(|i| column.tiles[i].cached_bounds)
        .filter(|r| r.size.w > 0 && r.size.h > 0);
    let first = rects.next()?;
    let (mut l, mut t, mut r, mut b) = (first.loc.x, first.loc.y, first.right(), first.bottom());
    for rect in rects {
        l = l.min(rect.loc.x);
        t = t.min(rect.loc.y);
        r = r.max(rect.right());
        b = b.max(rect.bottom());
    }
    Some(Rect::new(l, t, (r - l) as u32, (b - t) as u32))
}

impl TilingEngine {
    /// Install the overlay layer: from now on the decorations of every monitor are described
    /// to `sink` after each layout pass.
    pub fn install_decor_sink(&mut self, sink: Arc<dyn DecorSink>) {
        self.decor = Some(sink);
    }

    /// True once an overlay layer is installed.
    pub fn has_decor_sink(&self) -> bool {
        self.decor.is_some()
    }

    /// Describe the decorations of `output_id` to the overlay layer.  `positions` are the tile
    /// rectangles of the pass (before the border inset), `draw_modes` how each tile is drawn,
    /// `view` the area the strip is shown in; `suppressed` clears the monitor's decorations
    /// (the overview is up).
    pub(super) fn refresh_decorations(
        &self,
        output_id: OutputId,
        positions: &HashMap<WindowId, Rect>,
        draw_modes: &HashMap<WindowId, DrawMode>,
        view: Rect,
        suppressed: bool,
    ) {
        let Some(sink) = &self.decor else {
            return;
        };
        let key = output_id.as_u64();
        let Some(monitor) = self.monitors.get(&output_id) else {
            sink.set_output(key, Vec::new());
            return;
        };
        if suppressed || self.effective_zoom() < 1.0 {
            sink.set_output(key, Vec::new());
            return;
        }
        let cfg = self.active_workspace_config(output_id);
        let lone_tile = monitor.workspace().map_or(false, |ws| {
            ws.columns.len() == 1 && ws.columns[0].tiles.len() == 1
        });
        let mut tiles: Vec<(WindowId, Rect, DrawMode)> = positions
            .iter()
            .filter(|(wid, _)| {
                !self.floating_windows.contains(wid) && !self.minimized.contains_key(wid)
            })
            .map(|(wid, rect)| {
                (
                    *wid,
                    *rect,
                    draw_modes.get(wid).copied().unwrap_or(DrawMode::Inset),
                )
            })
            .collect();
        let inputs = FrameInputs {
            cfg,
            focus_monitor: Some(output_id) == self.monitors.focused_id(),
            active: monitor.focus_window,
            urgent: &self.urgent_windows,
            monitor_bounds: monitor.bounds,
            view,
            accent: crate::backend::accent::current_windows_accent_rgba(),
            radius: window_radius(
                cfg.effects.corner_radius,
                crate::backend::osversion::supports_rounded_corners(),
            ),
            lone_tile,
        };
        // Floats keep their window size; their border is outside that rectangle.
        for wid in &self.floating_windows {
            if self.float_location(*wid) != Some((output_id, monitor.active_workspace))
                || self.minimized.contains_key(wid)
            {
                continue;
            }
            if let Some(rect) = self.float_rect_of(*wid) {
                let band = self.rule_border_inset(*wid, cfg);
                tiles.push((
                    *wid,
                    Rect::new(
                        rect.loc.x - band,
                        rect.loc.y - band,
                        rect.size.w + 2 * band as u32,
                        rect.size.h + 2 * band as u32,
                    ),
                    DrawMode::Inset,
                ));
            }
        }
        let mut specs = Vec::new();
        for tile in &tiles {
            let mut per_window = cfg.clone();
            if let Some(state) = self.rule_state.get(&tile.0) {
                per_window.effects = state.resolved.effects.apply_to(&cfg.effects);
            }
            per_window.border_width = self.rule_border_inset(tile.0, cfg);
            let radius = self
                .rule_state
                .get(&tile.0)
                .and_then(|s| s.resolved.effects.geometry_corner_radius)
                .map_or(inputs.radius, |r| r[0] * monitor.scale_factor as f32);
            let active = if self
                .os_foreground
                .map_or(false, |w| self.floating_windows.contains(&w))
            {
                self.os_foreground
            } else {
                inputs.active
            };
            let window_inputs = FrameInputs {
                cfg: &per_window,
                radius,
                active,
                ..inputs
            };
            let mut frames = frame_specs(&window_inputs, std::slice::from_ref(tile));
            if let Some(corners) = self
                .rule_state
                .get(&tile.0)
                .and_then(|s| s.resolved.effects.geometry_corner_radius)
            {
                for frame in &mut frames {
                    if let crate::overlay::decor::DecorContent::Frame(spec) = &mut frame.content {
                        spec.corners = Some(corners.map(|r| r * monitor.scale_factor as f32));
                    }
                }
            }
            specs.extend(frames);
        }
        if let Some(ws) = monitor.workspace() {
            specs.extend(tab_specs(
                cfg,
                ws,
                positions,
                draw_modes,
                &self.urgent_windows,
                inputs.focus_monitor,
                monitor.focus_window,
                view,
                inputs.accent,
                Some(&self.rule_state),
            ));
        }
        // niri `fullscreen-backdrop-color`: while a fullscreen window of this monitor is up,
        // the layer fills the tiling view with the configured colour behind it (before the
        // frames, so it is drawn below them).  A fullscreen window can leave a rim — gaps,
        // the taskbar area, DPI rounding — and that rim is the backdrop, not the desktop.
        if let Some(backdrop) = self.fullscreen_backdrop_spec(output_id, view) {
            specs.insert(0, backdrop);
        }
        sink.set_output(key, specs);
    }

    /// The backdrop of a monitor with a fullscreen window, when
    /// `layout { fullscreen-backdrop-color }` names one.  `None` when no window of the
    /// monitor is fullscreen or the colour is unset (no spec, so nothing is drawn).
    fn fullscreen_backdrop_spec(
        &self,
        output_id: OutputId,
        view: Rect,
    ) -> Option<crate::overlay::decor::DecorSpec> {
        let cfg = self.effective_config(output_id);
        let color = cfg
            .fullscreen_backdrop_color
            .as_deref()
            .and_then(crate::config::parse_color)
            .map(crate::overlay::decor::paint::Rgba::from_array)?;        let fullscreen = self.fullscreen_windows.iter().any(|window| {
            self.locate_window(*window)
                .map(|(output, _, _)| output)
                .or_else(|| self.float_location(*window).map(|(output, _)| output))
                == Some(output_id)
        });
        if !fullscreen {
            return None;
        }
        Some(crate::overlay::decor::DecorSpec::backdrop(
            output_id.as_u64(),
            crate::overlay::decor::BackdropSpec::new(view, color),
        ))
    }

    /// Show the drop destination while a column is being dragged. The same column hit-test
    /// drives both the hint and the eventual swap, so the preview never promises another slot.
    pub fn update_insert_hint(&self, cursor_x: Option<i32>) {
        use crate::overlay::decor::{HintSpec, TRANSIENT_OUTPUT};
        let Some(sink) = &self.decor else { return };
        let spec = (|| {
            let x = cursor_x?;
            let output = self.monitors.focused_id()?;
            let monitor = self.monitors.get(&output)?;
            let col = self.column_at_x(x)?;
            let column = monitor.workspace()?.columns.get(col)?;
            let cfg = self.active_workspace_config(output);
            if cfg.effects.insert_hint.off {
                return None;
            }
            Some(DecorSpec::hint(HintSpec {
                rect: column_drop_bounds(column)?,
                radius: window_radius(
                    cfg.effects.corner_radius,
                    crate::backend::osversion::supports_rounded_corners(),
                ),
                paint: paint_of(
                    &cfg.effects.insert_hint.paint,
                    crate::backend::accent::current_windows_accent_rgba(),
                )?,
                view: monitor.work_area,
            }))
        })();
        sink.set_output(TRANSIENT_OUTPUT, spec.into_iter().collect());
    }

    /// Remove every decoration (a quit, or the overlay layer being turned off).
    pub(super) fn clear_decorations(&self) {
        if let Some(sink) = &self.decor {
            sink.shutdown();
        }
    }

    /// Ask DWM to round (or not) the corners of a window.
    pub(super) fn apply_corner_for_window(&self, hwnd: isize, preference: i32) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Dwm::{
            DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWM_WINDOW_CORNER_PREFERENCE,
        };
        let value = DWM_WINDOW_CORNER_PREFERENCE(preference);
        let _ = unsafe {
            DwmSetWindowAttribute(
                HWND(hwnd as *mut std::ffi::c_void),
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &value as *const DWM_WINDOW_CORNER_PREFERENCE as *const std::ffi::c_void,
                std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
            )
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Effects, PaintStyle};
    use crate::overlay::decor::DecorContent;

    fn cfg_with(fx: Effects) -> LayoutConfig {
        let mut c = LayoutConfig::default();
        c.effects = fx;
        c.border_width = if c.effects.border.off {
            0
        } else {
            c.effects.border.width as i32
        };
        c
    }

    fn wid(n: isize) -> WindowId {
        WindowId::new(n)
    }

    fn inputs<'a>(cfg: &'a LayoutConfig, urgent: &'a HashSet<WindowId>) -> FrameInputs<'a> {
        FrameInputs {
            cfg,
            focus_monitor: true,
            active: Some(wid(2)),
            urgent,
            monitor_bounds: Rect::new(0, 0, 1366, 768),
            view: Rect::new(8, 8, 1350, 752),
            accent: None,
            radius: 8.0,
            lone_tile: false,
        }
    }

    fn tile(n: isize, x: i32) -> (WindowId, Rect, DrawMode) {
        (wid(n), Rect::new(x, 8, 400, 752), DrawMode::Inset)
    }

    fn frame_of(spec: &DecorSpec) -> &FrameSpec {
        match &spec.content {
            DecorContent::Frame(f) => f,            other => panic!("expected a frame, got {:?}", other),
        }
    }

    #[test]
    fn the_ring_is_for_the_active_window_only_and_the_border_for_every_window() {
        let cfg = cfg_with(Effects::default());
        let urgent = HashSet::new();
        let specs = frame_specs(
            &inputs(&cfg, &urgent),
            &[tile(1, 8), tile(2, 424), tile(3, 840)],
        );
        assert_eq!(specs.len(), 3);
        let by = |n: isize| frame_of(specs.iter().find(|s| s.id.key == n).unwrap());
        assert!(by(2).ring.is_some() && by(2).ring_width == 3);
        assert!(by(1).ring.is_none() && by(3).ring.is_none());
        assert!(by(1).border.is_some() && by(2).border.is_some() && by(3).border.is_some());
        assert_eq!(by(1).band, 4);
    }

    #[test]
    fn the_active_window_has_the_active_colour_and_the_others_the_inactive_one() {
        let cfg = cfg_with(Effects::default());
        let urgent = HashSet::new();
        let specs = frame_specs(&inputs(&cfg, &urgent), &[tile(1, 8), tile(2, 424)]);
        let active = paint_of(&cfg.effects.border.active, None);
        let inactive = paint_of(&cfg.effects.border.inactive, None);
        let by = |n: isize| frame_of(specs.iter().find(|s| s.id.key == n).unwrap());
        assert_eq!(by(2).border, active);
        assert_eq!(by(1).border, inactive);
        assert_ne!(active, inactive);
    }

    #[test]
    fn on_a_monitor_without_the_keyboard_focus_the_active_window_looks_inactive() {
        let cfg = cfg_with(Effects::default());
        let urgent = HashSet::new();
        let mut i = inputs(&cfg, &urgent);
        i.focus_monitor = false;
        let specs = frame_specs(&i, &[tile(2, 424)]);
        let f = frame_of(&specs[0]);
        assert_eq!(f.border, paint_of(&cfg.effects.border.inactive, None));
        assert_eq!(f.ring, paint_of(&cfg.effects.focus_ring.inactive, None));
    }

    #[test]
    fn an_urgent_window_uses_the_urgent_colour() {
        let cfg = cfg_with(Effects::default());
        let mut urgent = HashSet::new();
        urgent.insert(wid(1));
        let specs = frame_specs(&inputs(&cfg, &urgent), &[tile(1, 8), tile(2, 424)]);
        let f1 = frame_of(specs.iter().find(|s| s.id.key == 1).unwrap());
        assert_eq!(f1.border, paint_of(&cfg.effects.border.urgent, None));
    }

    #[test]
    fn a_border_that_is_off_paints_nothing_and_leaves_no_band() {
        let mut fx = Effects::default();
        fx.border.off = true;
        let cfg = cfg_with(fx);
        assert_eq!(cfg.border_inset(), 0);
        let urgent = HashSet::new();
        let specs = frame_specs(&inputs(&cfg, &urgent), &[tile(1, 8), tile(2, 424)]);
        // Only the ring of the active window is left.
        assert_eq!(specs.len(), 1);
        let f = frame_of(&specs[0]);
        assert_eq!(f.band, 0);
        assert!(f.border.is_none() && f.ring.is_some());
    }

    #[test]
    fn a_ring_that_is_off_and_a_border_that_is_off_draw_nothing() {
        let mut fx = Effects::default();
        fx.border.off = true;
        fx.focus_ring.off = true;
        let cfg = cfg_with(fx);
        let urgent = HashSet::new();
        assert!(frame_specs(&inputs(&cfg, &urgent), &[tile(1, 8), tile(2, 424)]).is_empty());
    }

    #[test]
    fn a_fullscreen_window_gets_the_configured_backdrop_and_nothing_else_does() {
        use crate::backend::{BackendHandle, WindowInfo, WindowState};
        use crate::overlay::decor::paint::{Paint, Rgba};
        use crate::overlay::decor::RecordingSink;

        let sink = RecordingSink::new();
        let be = BackendHandle::default_for_test();
        let mut engine = TilingEngine::new(LayoutConfig {
            fullscreen_backdrop_color: Some("#101010".into()),
            ..LayoutConfig::default()
        });
        engine.install_decor_sink(sink.clone());
        let output = OutputId::from_name("backdrop");
        engine.register_monitor(
            output,
            Rect::new(0, 0, 1200, 800),
            Rect::new(0, 0, 1200, 760),
        );
        engine.add_window(
            WindowInfo {
                hwnd: 1,
                title: "player".into(),
                class_name: "backdrop".into(),
                process_id: 1,
                bounds: Rect::new(0, 0, 400, 400),
                state: WindowState::Normal,
                is_visible: true,
            },
            &be,
        );
        let view = Rect::new(8, 8, 1184, 744);
        let empty: HashMap<WindowId, Rect> = HashMap::new();
        let modes: HashMap<WindowId, DrawMode> = HashMap::new();
        engine.refresh_decorations(output, &empty, &modes, view, false);
        assert!(
            !sink
                .of_output(output.as_u64())
                .iter()
                .any(|spec| matches!(spec.content, DecorContent::Backdrop(_))),
            "no fullscreen window, no backdrop"
        );
        engine.fullscreen_windows.insert(WindowId::new(1));
        engine.refresh_decorations(output, &empty, &modes, view, false);
        let specs = sink.of_output(output.as_u64());
        let backdrop = specs
            .iter()
            .find(|spec| matches!(spec.content, DecorContent::Backdrop(_)))
            .expect("a fullscreen window gets the backdrop");
        match &backdrop.content {
            DecorContent::Backdrop(spec) => {
                assert_eq!(spec.rect, view, "the tiling view of the monitor");
                assert_eq!(spec.paint, Paint::Solid(Rgba::new(0x10, 0x10, 0x10, 255)));
            }
            other => panic!("expected a backdrop, got {other:?}"),
        }
        assert_eq!(
            specs.first().map(|spec| spec.id.kind),
            Some(crate::overlay::decor::DecorKind::Backdrop),
            "the backdrop is drawn below the frames"
        );
    }

    #[test]
    fn a_monitor_without_the_colour_draws_no_backdrop() {
        use crate::backend::BackendHandle;
        use crate::overlay::decor::RecordingSink;
        let sink = RecordingSink::new();
        let be = BackendHandle::default_for_test();
        let mut engine = TilingEngine::new(LayoutConfig::default());
        engine.install_decor_sink(sink.clone());
        let output = OutputId::from_name("no-backdrop");
        engine.register_monitor(
            output,
            Rect::new(0, 0, 1200, 800),
            Rect::new(0, 0, 1200, 760),
        );
        let _ = be;
        engine.fullscreen_windows.insert(WindowId::new(1));
        let empty: HashMap<WindowId, Rect> = HashMap::new();
        let modes: HashMap<WindowId, DrawMode> = HashMap::new();
        engine.refresh_decorations(output, &empty, &modes, Rect::new(8, 8, 1184, 744), false);
        assert!(sink
            .of_output(output.as_u64())
            .iter()
            .all(|spec| !matches!(spec.content, DecorContent::Backdrop(_))));
    }

    #[test]
    fn fullscreen_and_maximized_windows_have_no_frame() {        let cfg = cfg_with(Effects::default());
        let urgent = HashSet::new();
        let tiles = [
            (wid(1), Rect::new(0, 0, 1366, 768), DrawMode::Exact),
            (wid(2), Rect::new(8, 8, 1350, 752), DrawMode::Edges),
            tile(3, 8),
        ];
        let specs = frame_specs(&inputs(&cfg, &urgent), &tiles);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].id.key, 3);
    }

    #[test]
    fn tiles_scrolled_out_of_view_are_not_decorated() {
        let cfg = cfg_with(Effects::default());
        let urgent = HashSet::new();
        // Parked below the screen.
        let parked = (wid(1), Rect::new(8, 900, 400, 752), DrawMode::Inset);
        let specs = frame_specs(&inputs(&cfg, &urgent), &[parked, tile(2, 424)]);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].id.key, 2);
    }

    #[test]
    fn smart_borders_leave_a_lone_window_bare() {
        let mut cfg = cfg_with(Effects::default());
        cfg.smart_borders = true;
        let urgent = HashSet::new();
        let mut i = inputs(&cfg, &urgent);
        i.lone_tile = true;
        assert!(frame_specs(&i, &[tile(2, 8)]).is_empty());
        i.lone_tile = false;
        assert_eq!(frame_specs(&i, &[tile(2, 8)]).len(), 1);
    }

    #[test]
    fn the_shadow_is_fainter_for_a_window_that_is_not_active() {
        let mut fx = Effects::default();
        fx.shadow.on = true;
        let cfg = cfg_with(fx);
        let urgent = HashSet::new();
        let specs = frame_specs(&inputs(&cfg, &urgent), &[tile(1, 8), tile(2, 424)]);
        let a1 = frame_of(specs.iter().find(|s| s.id.key == 1).unwrap())
            .shadow
            .unwrap()
            .color
            .a;
        let a2 = frame_of(specs.iter().find(|s| s.id.key == 2).unwrap())
            .shadow
            .unwrap()
            .color
            .a;
        assert!(a1 < a2, "{} < {}", a1, a2);
        // 0x77 = 119 at full strength, three quarters of it otherwise.
        assert_eq!(a2, 119);
        assert_eq!(a1, 89);
    }

    #[test]
    fn a_gradient_ring_is_passed_on() {
        let mut fx = Effects::default();
        fx.focus_ring.active = PaintStyle::Gradient(crate::config::GradientStyle {
            from: "#80c8ff".to_string(),
            to: "#bbddff".to_string(),
            angle: 45.0,
            relative_to_view: true,
            space: crate::config::GradientSpace::Srgb,
            hue: crate::config::effects::GradientHue::Shorter,
        });
        let cfg = cfg_with(fx);
        let urgent = HashSet::new();
        let specs = frame_specs(&inputs(&cfg, &urgent), &[tile(2, 424)]);
        match frame_of(&specs[0]).ring {
            Some(Paint::Gradient(g)) => {
                assert_eq!(g.angle, 45.0);
                assert!(g.relative_to_view);
            }
            other => panic!("expected a gradient ring, got {:?}", other),
        }
    }

    #[test]
    fn the_output_is_the_same_whatever_order_the_tiles_come_in() {
        let cfg = cfg_with(Effects::default());
        let urgent = HashSet::new();
        let a = frame_specs(
            &inputs(&cfg, &urgent),
            &[tile(3, 840), tile(1, 8), tile(2, 424)],
        );
        let b = frame_specs(
            &inputs(&cfg, &urgent),
            &[tile(1, 8), tile(2, 424), tile(3, 840)],
        );
        assert_eq!(a, b);
        let keys: Vec<isize> = a.iter().map(|s| s.id.key).collect();
        assert_eq!(keys, vec![1, 2, 3]);
    }

    #[test]
    fn column_drop_geometry_ignores_hidden_tabs_and_unlaid_out_tiles() {
        use crate::layout::workspace::ColumnDisplay;
        use crate::layout::{Column, Tile};
        let mut column = Column::new();
        for id in 1..=3 {
            column.add_tile(Tile::new(wid(id)));
        }
        column.tiles[0].cached_bounds = Rect::new(-500, 700, 200, 400);
        column.tiles[1].cached_bounds = Rect::new(420, 8, 350, 712);
        column.display = ColumnDisplay::Tabbed { active_tab: 1 };
        assert_eq!(
            column_drop_bounds(&column),
            Some(Rect::new(420, 8, 350, 712))
        );
        column.display = ColumnDisplay::Stacked;
        column.tiles[0].cached_bounds = Rect::new(420, 8, 350, 300);
        column.tiles[1].cached_bounds = Rect::new(420, 316, 350, 404);
        assert_eq!(
            column_drop_bounds(&column),
            Some(Rect::new(420, 8, 350, 712))
        );
        assert_eq!(column_drop_bounds(&Column::new()), None);
    }

    fn tab_workspace() -> crate::layout::Workspace {
        use crate::layout::{Column, Tile, Workspace};
        let mut column = Column::new();
        for n in 1..=3 {
            column.add_tile(Tile::new(wid(n)));
        }
        column.switch_to_tabbed();
        column.select_window(wid(2));
        let mut workspace = Workspace::new();
        workspace.columns.push(column);
        workspace
    }

    fn tabs_for_test(
        cfg: &LayoutConfig,
        workspace: &crate::layout::Workspace,
        modes: &HashMap<WindowId, DrawMode>,
        urgent: &HashSet<WindowId>,
        focused_output: bool,
        focused_window: Option<WindowId>,
    ) -> Vec<crate::overlay::decor::TabsSpec> {
        let area = Rect::new(30, 40, 400, 600);
        let positions = (1..=3).map(|n| (wid(n), area)).collect();
        tab_specs(
            cfg,
            workspace,
            &positions,
            modes,
            urgent,
            focused_output,
            focused_window,
            Rect::new(0, 0, 1366, 768),
            None,
            None,
        )
        .into_iter()
        .map(|spec| match spec.content {
            DecorContent::Tabs(tabs) => tabs,
            other => panic!("expected tabs, got {:?}", other),
        })
        .collect()
    }

    #[test]
    fn tabs_fall_back_to_enabled_focus_ring_otherwise_border_even_when_both_are_off() {
        let mut cfg = cfg_with(Effects::default());
        cfg.effects.focus_ring.active = PaintStyle::color("#102030");
        cfg.effects.focus_ring.inactive = PaintStyle::color("#405060");
        cfg.effects.focus_ring.urgent = PaintStyle::color("#708090");
        cfg.effects.border.active = PaintStyle::color("#a02030");
        cfg.effects.border.inactive = PaintStyle::color("#b05060");
        cfg.effects.border.urgent = PaintStyle::color("#c08090");
        let workspace = tab_workspace();
        let urgent = HashSet::from([wid(3)]);
        for (ring_off, border_off) in [(false, false), (true, false), (true, true)] {
            cfg.effects.focus_ring.off = ring_off;
            cfg.effects.border.off = border_off;
            let tabs = tabs_for_test(
                &cfg,
                &workspace,
                &HashMap::new(),
                &urgent,
                true,
                Some(wid(2)),
            );
            let fallback = if ring_off {
                &cfg.effects.border
            } else {
                &cfg.effects.focus_ring
            };
            assert_eq!(
                tabs[0].tabs,
                vec![
                    paint_of(&fallback.inactive, None).unwrap(),
                    paint_of(&fallback.active, None).unwrap(),
                    paint_of(&fallback.urgent, None).unwrap(),
                ]
            );
            assert_eq!(tabs[0].windows, vec![wid(1), wid(2), wid(3)]);
        }
    }

    #[test]
    fn explicit_tab_paints_use_selected_tab_urgency_and_whole_column_focus() {
        let mut cfg = cfg_with(Effects::default());
        let style = &mut cfg.effects.tab_indicator;
        style.active = Some(PaintStyle::color("#102030"));
        style.inactive = Some(PaintStyle::color("#405060"));
        style.urgent = Some(PaintStyle::color("#708090"));
        let workspace = tab_workspace();
        let modes = HashMap::new();
        let urgent = HashSet::from([wid(2), wid(3)]);
        let active = paint_of(cfg.effects.tab_indicator.active.as_ref().unwrap(), None).unwrap();
        let inactive =
            paint_of(cfg.effects.tab_indicator.inactive.as_ref().unwrap(), None).unwrap();
        let urgent_paint =
            paint_of(cfg.effects.tab_indicator.urgent.as_ref().unwrap(), None).unwrap();
        let focused = tabs_for_test(&cfg, &workspace, &modes, &urgent, true, Some(wid(2)));
        assert_eq!(
            focused[0].tabs,
            vec![inactive.clone(), urgent_paint.clone(), urgent_paint.clone()]
        );
        let normal = tabs_for_test(
            &cfg,
            &workspace,
            &modes,
            &HashSet::new(),
            true,
            Some(wid(2)),
        );
        assert_eq!(normal[0].tabs[1], active);
        for (output, focus) in [(false, Some(wid(2))), (true, Some(wid(99))), (true, None)] {
            let unfocused = tabs_for_test(&cfg, &workspace, &modes, &urgent, output, focus);
            assert_eq!(
                unfocused[0].tabs,
                focused[0]
                    .tabs
                    .iter()
                    .map(|p| p.faded(0.5))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn fullscreen_and_edges_on_any_tab_suppress_rendering_after_every_selection() {
        let cfg = cfg_with(Effects::default());
        let mut workspace = tab_workspace();
        for mode in [DrawMode::Exact, DrawMode::Edges] {
            let modes = HashMap::from([(wid(1), mode)]);
            for n in 1..=3 {
                workspace.columns[0].select_window(wid(n));
                assert!(tabs_for_test(
                    &cfg,
                    &workspace,
                    &modes,
                    &HashSet::new(),
                    true,
                    Some(wid(n))
                )
                .is_empty());
            }
        }
    }

    #[test]
    fn maximize_column_keeps_tabs_and_reservation_and_preserves_raw_geometry_values() {
        let mut cfg = cfg_with(Effects::default());
        cfg.effects.tab_indicator.place_within_column = true;
        cfg.effects.tab_indicator.gap = -0.25;
        cfg.effects.tab_indicator.gaps_between_tabs = 0.25;
        cfg.effects.tab_indicator.length = f64::MAX;
        let mut workspace = tab_workspace();
        workspace.columns[0].maximized = true;
        let tabs = tabs_for_test(
            &cfg,
            &workspace,
            &HashMap::new(),
            &HashSet::new(),
            true,
            Some(wid(2)),
        );
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].gap, -0.25);
        assert_eq!(tabs[0].gaps_between, 0.25);
        assert_eq!(tabs[0].length, f64::MAX);
        let area = Rect::new(0, 0, 600, 400);
        assert_ne!(
            super::super::tabs::reserve_indicator(
                area,
                &workspace.columns[0],
                &cfg.effects.tab_indicator,
                1.0,
                DrawMode::Inset,
            ),
            area
        );
    }

    #[test]
    fn dwm_knows_three_roundings() {
        assert_eq!(dwm_corner_for(0), (1, 0.0));
        assert_eq!(dwm_corner_for(3), (3, 4.0));
        assert_eq!(dwm_corner_for(5), (3, 4.0));
        assert_eq!(dwm_corner_for(6), (2, 8.0));
        assert_eq!(dwm_corner_for(40), (2, 8.0));
    }

    #[test]
    fn the_window_radius_is_what_dwm_will_make_of_the_request() {
        assert_eq!(window_radius(None, true), 8.0);
        assert_eq!(window_radius(None, false), 0.0);
        assert_eq!(window_radius(Some(0), true), 0.0);
        assert_eq!(window_radius(Some(4), true), 4.0);
        assert_eq!(window_radius(Some(12), true), 8.0);
        assert_eq!(window_radius(Some(12), false), 0.0);
    }
}
