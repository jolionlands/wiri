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

/// The indicator uses the active tab's area and belongs to that column, not to a screen-wide
/// HUD. Non-selected tabs are fainter; urgency overrides both active and inactive colours.
fn tab_specs(
    cfg: &LayoutConfig,
    workspace: &crate::layout::Workspace,
    positions: &HashMap<WindowId, Rect>,
    modes: &HashMap<WindowId, DrawMode>,
    urgent: &HashSet<WindowId>,
    focused_output: bool,
    view: Rect,
    accent: Option<[u8; 4]>,
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
            if modes.get(&active).map_or(false, |m| *m != DrawMode::Inset) {
                return None;
            }
            let area = *positions.get(&active)?;
            if !area.intersects(view) {
                return None;
            }
            let fallback = if !cfg.effects.border.off {
                &cfg.effects.border
            } else {
                &cfg.effects.focus_ring
            };
            let paints = col
                .tiles
                .iter()
                .enumerate()
                .map(|(i, tile)| {
                    let is_urgent = urgent.contains(&tile.window_id);
                    let paint = if is_urgent {
                        style.urgent.as_ref()
                    } else if focused_output {
                        style.active.as_ref()
                    } else {
                        style.inactive.as_ref()
                    };
                    let paint = paint
                        .and_then(|s| paint_of(s, accent))
                        .or_else(|| paint_for(fallback, is_urgent, focused_output, accent))
                        .unwrap_or(Paint::Solid(Rgba::new(128, 200, 255, 255)));
                    if i == active_tab {
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
                    gap: style.gap.round() as i32,
                    gaps_between: style.gaps_between_tabs.round() as i32,
                    length: style.length as f32,
                    corner_radius: style.corner_radius as f32,
                    tabs: paints,
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
        let tiles: Vec<(WindowId, Rect, DrawMode)> = positions
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
        let mut specs = frame_specs(&inputs, &tiles);
        if let Some(ws) = monitor.workspace() {
            specs.extend(tab_specs(
                cfg,
                ws,
                positions,
                draw_modes,
                &self.urgent_windows,
                inputs.focus_monitor,
                view,
                inputs.accent,
            ));
        }
        sink.set_output(key, specs);
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
            DecorContent::Frame(f) => f,
            other => panic!("expected a frame, got {:?}", other),
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
    fn fullscreen_and_maximized_windows_have_no_frame() {
        let cfg = cfg_with(Effects::default());
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
