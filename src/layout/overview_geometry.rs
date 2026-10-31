//! niri's overview camera, as pure arithmetic.
//!
//! niri's overview is not a "fit everything on one screen" zoom: the desktop is an
//! infinite grid (x: each workspace's endless strip of columns, y: the workspaces
//! stacked vertically) and the overview is the *same* grid seen through a fixed
//! zoom.  The camera pans in x (each workspace keeps its own horizontal view
//! offset) and in y (the workspace stack follows the focused workspace).
//!
//! Pinned source (niri `e19df12cf81ffe805609fee497da1a097066ad2f`):
//! * `src/layout/mod.rs:5034-5044` `compute_overview_zoom`: the configured zoom is
//!   clamped to `0.0001..=0.75` and the open/close animation interpolates
//!   `1 - progress * (1 - zoom)`.
//! * `src/layout/monitor.rs:1329-1336` `Monitor::workspace_size` /
//!   `workspace_gap` / `workspace_size_with_gap`: workspace size is the view scaled
//!   by the zoom (ceiled to physical pixels) and the gap between workspaces is 10%
//!   of the view height, also scaled by the zoom (at least one pixel).
//! * `src/layout/monitor.rs:1467-1497` `Monitor::workspaces_render_geo`: every
//!   workspace gets `y = (view.h - ws.h)/2 - render_idx * (ws.h + gap) + idx *
//!   (ws.h + gap)`, i.e. the active workspace is centred vertically and its
//!   neighbours show above and below; `x` centres the workspace in the view.
//!   `render_idx` is `workspace_render_idx()` (`src/layout/monitor.rs:1410-1465`),
//!   the fractional index of the focused workspace, animated by the workspace
//!   switch, and empty workspaces are ordinary entries of that stack.
//! * `src/layout/monitor.rs:1735-1745` `Monitor::render_workspaces`: each workspace is
//!   relocated to its `workspaces_render_geo` rectangle and rescaled by the zoom, so
//!   a window is drawn at `zoom * tile size`.
//!
//! Windows cannot rescale a live HWND, so wiri draws the same rectangles out of
//! DWM thumbnails and crops each one to its workspace rectangle, exactly like the
//! monitor bounds crop already implemented in `overlay::thumbnail_overview`.

use crate::utils::{Point, Rect, Size};

/// Smallest zoom niri will use: `compute_overview_zoom` clamps to it so the
/// workspaces never collapse to nothing.
pub const MIN_ZOOM: f64 = 0.0001;
/// Largest zoom niri will use, even if `overview { zoom }` asks for more
/// (`src/layout/mod.rs:5036`).
pub const MAX_ZOOM: f64 = 0.75;
/// niri's default `overview { zoom }` (`niri-config/src/misc.rs:147`).
pub const DEFAULT_ZOOM: f64 = 0.5;

/// The configured zoom, clamped the way `compute_overview_zoom` clamps it.
pub fn configured_zoom(zoom: f64) -> f64 {
    if !zoom.is_finite() {
        return DEFAULT_ZOOM;
    }
    zoom.clamp(MIN_ZOOM, MAX_ZOOM)
}

/// The zoom actually drawn at `progress` (0 = normal view, 1 = fully in the
/// overview): `compute_overview_zoom`'s `1 - p * (1 - zoom)`.
pub fn zoom_for_progress(zoom: f64, progress: f64) -> f64 {
    let zoom = configured_zoom(zoom);
    let progress = if progress.is_finite() {
        progress.clamp(0.0, 1.0)
    } else {
        0.0
    };
    (1.0 - progress * (1.0 - zoom)).max(MIN_ZOOM)
}

/// `Monitor::workspace_size`: the view scaled by the zoom, ceiled like niri's
/// `to_physical_precise_ceil` at scale 1.
pub fn workspace_size(view: Rect, zoom: f64) -> Size {
    let zoom = if zoom.is_finite() {
        zoom.clamp(MIN_ZOOM, MAX_ZOOM)
    } else {
        1.0
    };
    Size::new(
        ((view.size.w as f64 * zoom).ceil() as u32).max(1),
        ((view.size.h as f64 * zoom).ceil() as u32).max(1),
    )
}

/// `Monitor::workspace_gap`: 10% of the view height, scaled by the zoom, at least
/// one pixel (`round_logical_in_physical_max1`).
pub fn workspace_gap(view: Rect, zoom: f64) -> i32 {
    let zoom = if zoom.is_finite() {
        zoom.clamp(0.0, 1.0)
    } else {
        1.0
    };
    ((view.size.h as f64 * 0.1 * zoom).round() as i32).max(1)
}

/// Round half away from zero, like niri's `round_logical_in_physical` at scale 1.
fn round_logical(value: f64) -> i32 {
    if value >= 0.0 {
        (value + 0.5) as i32
    } else {
        -((-value + 0.5) as i32)
    }
}

/// `Monitor::workspaces_render_geo`, for `count` workspaces:
/// `y = static_offset.y + (idx - render_index) * (ws.h + gap)`,
/// `x = static_offset.x`, where `static_offset` centres the workspace in the view.
///
/// `render_index` is the fractional focused-workspace index, so a workspace switch
/// pans the camera in y instead of jumping.
pub fn workspace_rects(view: Rect, count: usize, render_index: f64, zoom: f64) -> Vec<Rect> {
    let size = workspace_size(view, zoom);
    let gap = workspace_gap(view, zoom);
    let stride = size.h as i32 + gap;
    let render_index = if render_index.is_finite() {
        render_index
    } else {
        0.0
    };
    let static_offset = Point::new(
        round_logical((view.size.w as f64 - size.w as f64) / 2.0),
        round_logical((view.size.h as f64 - size.h as f64) / 2.0),
    );
    let first_y = round_logical(-render_index * stride as f64);
    (0..count)
        .map(|idx| {
            Rect::new(
                view.loc.x + static_offset.x,
                view.loc.y + static_offset.y + first_y + idx as i32 * stride,
                size.w,
                size.h,
            )
        })
        .collect()
}

/// The one per-workspace horizontal camera.
///
/// Each workspace keeps its own endless strip and therefore its own view offset,
/// so the overview shows the columns that workspace currently shows (scaled), and
/// `focus-column-left/right` pans *that* strip.  niri has no unified camera
/// (`src/layout/monitor.rs:1720-1730` relocates every workspace by its own
/// `geo.loc` and lets the workspace scroll itself), so `monitor_camera` is `None`
/// today; the opt-in `layout { unified-grid }` mode fills it with the monitor's
/// camera so every row shares one horizontal offset.
pub fn workspace_camera(workspace_scroll: i32, monitor_camera: Option<i32>) -> i32 {
    monitor_camera.unwrap_or(workspace_scroll)
}

/// Intersect two screen rectangles; `None` when they do not overlap.
pub fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x = a.loc.x.max(b.loc.x);
    let y = a.loc.y.max(b.loc.y);
    let right = a.right().min(b.right());
    let bottom = a.bottom().min(b.bottom());
    if right <= x || bottom <= y {
        return None;
    }
    Some(Rect::new(
        x,
        y,
        (right - x) as u32,
        (bottom - y) as u32,
    ))
}

/// Where a tile drawn at `tile` (in the workspace's own, unscaled coordinate space,
/// already offset by the workspace's horizontal camera) lands on screen when the
/// workspace rectangle is `workspace`.  niri relocates to `geo.loc` and rescales by
/// the zoom (`src/layout/monitor.rs:1735-1745`), so the tile's offset inside the
/// workspace is multiplied by the zoom.
pub fn zoom_tile(tile: Rect, workspace: Rect, view: Rect, zoom: f64) -> Rect {
    let zoom = if zoom.is_finite() {
        zoom.clamp(MIN_ZOOM, MAX_ZOOM)
    } else {
        1.0
    };
    Rect::new(
        workspace.loc.x + round_logical((tile.loc.x - view.loc.x) as f64 * zoom),
        workspace.loc.y + round_logical((tile.loc.y - view.loc.y) as f64 * zoom),
        ((tile.size.w as f64 * zoom).round() as u32).max(1),
        ((tile.size.h as f64 * zoom).round() as u32).max(1),
    )
}

/// The visible part of a published thumbnail: the workspace crop first, so a
/// column that scrolls past the workspace edge is cut there, at any zoom.
pub fn visible_tile(tile: Rect, workspace: Rect) -> Option<Rect> {
    intersect(tile, workspace)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> Rect {
        Rect::new(0, 0, 1366, 768)
    }

    /// niri `compute_overview_zoom` (`src/layout/mod.rs:5034`): the config value is
    /// clamped to 0.0001..=0.75, and only the *animation* interpolates towards 1.
    #[test]
    fn zoom_comes_from_config_and_is_clamped_like_niri() {
        assert_eq!(configured_zoom(0.3), 0.3);
        assert_eq!(configured_zoom(0.5), DEFAULT_ZOOM);
        assert_eq!(configured_zoom(0.9), MAX_ZOOM);
        assert_eq!(configured_zoom(-2.0), MIN_ZOOM);
        assert_eq!(configured_zoom(0.0), MIN_ZOOM);
        assert_eq!(configured_zoom(f64::NAN), DEFAULT_ZOOM);
        // Open/close animation endpoints and middle.
        assert_eq!(zoom_for_progress(0.5, 0.0), 1.0);
        assert_eq!(zoom_for_progress(0.5, 1.0), 0.5);
        assert_eq!(zoom_for_progress(0.5, 0.5), 0.75);
        assert_eq!(zoom_for_progress(0.5, 2.0), 0.5);
    }

    /// A workspace is `zoom * view` and the gap is 10% of the view height, times the
    /// zoom (niri `src/layout/monitor.rs:1329-1341`).
    #[test]
    fn workspace_size_and_gap_follow_the_zoom() {
        let size = workspace_size(view(), 0.5);
        assert_eq!((size.w, size.h), (683, 384));
        assert_eq!(workspace_gap(view(), 0.5), 38); // 38.4 rounded
        assert_eq!(workspace_gap(view(), 1.0), 77); // 76.8 rounded
        assert_eq!(workspace_gap(view(), 0.01), 1); // never below one pixel
        // 0.3 of 1366 = 409.8, ceiled like niri's physical ceil.
        assert_eq!(workspace_size(view(), 0.3).w, 410);
    }

    /// niri `Monitor::workspaces_render_geo` (`src/layout/monitor.rs:1467`): the
    /// focused workspace is centred, neighbours above and below keep the 10% gap,
    /// and the stack is a *camera*, so off-screen workspaces have off-screen y.
    #[test]
    fn focused_workspace_is_centred_and_neighbours_are_visible() {
        let rects = workspace_rects(view(), 5, 2.0, 0.5);
        // (1366-683)/2 = 341.5 -> 342; (768-384)/2 = 192.
        assert_eq!(rects[2], Rect::new(342, 192, 683, 384));
        let stride = 384 + 38;
        assert_eq!(rects[1], Rect::new(342, 192 - stride, 683, 384));
        assert_eq!(rects[3], Rect::new(342, 192 + stride, 683, 384));
        // The neighbour above is partly visible: its bottom 192-38 pixels are on screen.
        assert!(rects[1].intersects(view()));
        assert_eq!(rects[1].bottom(), 192 - 38);
        // The second neighbour up is entirely off screen.
        assert!(!rects[0].intersects(view()));
        assert_eq!(rects.len(), 5);
    }

    /// The vertical camera follows the focused workspace, including mid-animation.
    #[test]
    fn camera_pans_vertically_with_the_focused_workspace() {
        let stride = (workspace_size(view(), 0.5).h + workspace_gap(view(), 0.5) as u32) as i32;
        let first = workspace_rects(view(), 3, 0.0, 0.5);
        let second = workspace_rects(view(), 3, 1.0, 0.5);
        assert_eq!(second[0].loc.y - first[0].loc.y, -stride);
        assert_eq!(second[1].loc.y, first[0].loc.y); // workspace 1 is now where 0 was
        // Halfway through the switch the stack sits half a stride up.
        let middle = workspace_rects(view(), 3, 0.5, 0.5);
        assert_eq!(first[0].loc.y - middle[0].loc.y, stride / 2);
    }

    /// A 3x10 grid: x is each workspace's own strip camera, y is the workspace stack.
    #[test]
    fn navigation_across_x_and_y_in_a_three_by_ten_grid() {
        let zoom = 0.5;
        // 10 workspaces, focused one in the middle: only a few are on screen.
        let rects = workspace_rects(view(), 10, 5.0, zoom);
        let on_screen: Vec<usize> = rects
            .iter()
            .enumerate()
            .filter(|(_, r)| r.intersects(view()))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(on_screen, vec![4, 5, 6]);
        // Each workspace's strip is panned by its own camera.
        assert_eq!(workspace_camera(400, None), 400);
        assert_eq!(workspace_camera(400, Some(120)), 120);
        // Column focus moves pan the strip; the tile's on-screen x follows it.
        let ws = rects[5];
        let tile = Rect::new(1000, 0, 500, 768);
        let before = zoom_tile(tile, ws, view(), zoom);
        let after = zoom_tile(
            Rect::new(1000 - 300, 0, 500, 768),
            ws,
            view(),
            zoom,
        );
        assert_eq!(before.loc.x - after.loc.x, 150);
    }

    /// Empty workspaces are ordinary entries of niri's stack, so they keep their
    /// slot and the neighbours do not jump.
    #[test]
    fn empty_workspaces_keep_their_slot_in_the_stack() {
        let rects = workspace_rects(view(), 4, 1.0, 0.5);
        let stride = 384 + 38;
        assert_eq!(rects[1].loc.y, 192);
        assert_eq!(rects[2].loc.y, 192 + stride);
        assert_eq!(rects[2], Rect::new(342, 192 + stride, 683, 384));
    }

    /// A thumbnail whose column runs past the workspace edge is cropped exactly
    /// there (`Monitor::render_workspaces` crops to the workspace height and the
    /// workspace's own render rect); a fully off-screen column is dropped.
    #[test]
    fn a_column_past_the_workspace_edge_is_cropped_and_one_beyond_is_dropped() {
        let ws = Rect::new(342, 192, 683, 384);
        // Half inside the workspace's right edge.
        let tile = Rect::new(900, 192, 400, 384);
        let visible = visible_tile(tile, ws).expect("edge column is visible");
        assert_eq!(visible, Rect::new(900, 192, 125, 384));
        assert_eq!(visible.right(), ws.right());
        // Not intersecting at all.
        assert_eq!(visible_tile(Rect::new(1100, 192, 100, 100), ws), None);
        assert_eq!(intersect(ws, Rect::new(0, 0, 10, 10)), None);
    }

    /// `zoom_tile` is the fixture the live gate reads: a full-height tile becomes
    /// exactly `zoom` times its size, not "fit to the screen".
    #[test]
    fn tile_size_is_the_zoom_times_the_tile_size() {
        let view = view();
        let ws = workspace_rects(view, 1, 0.0, 0.3)[0];
        let tile = Rect::new(0, 0, 1366, 768);
        let scaled = zoom_tile(tile, ws, view, 0.3);
        assert_eq!(scaled.size, Size::new(410, 230));
        assert_eq!((scaled.loc.x, scaled.loc.y), (478, 269));
    }
}
