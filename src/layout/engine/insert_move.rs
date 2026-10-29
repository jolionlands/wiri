//! Interactive Mod+LMB insertion, separate from the Ctrl+Alt column-swap grab.
//!
//! Like niri e19df12cf8 scrolling.rs:836-901, compare the nearest column boundary
//! with the nearest vertical tile slot. Both preview and release recompute the same
//! pure destination; no HWND hit-testing, timers, or remembered preview index.

use super::{BackendHandle, TilingEngine};
use crate::layout::{Column, ColumnDisplay, Tile, Workspace};
use crate::overlay::decor::{paint_of, DecorSpec, HintSpec, TRANSIENT_OUTPUT};
use crate::utils::{OutputId, Point, Rect, WindowId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InsertPosition {
    NewColumn(usize),
    InColumn(usize, usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ColumnGeometry {
    bounds: Rect,
    /// Logical tile insertion indices, not indices into the visible-tile list.
    slots: Vec<(usize, i32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InsertDestination {
    position: InsertPosition,
    hint: Rect,
}

struct WindowInsertDestination {
    output: OutputId,
    workspace: i32,
    destination: InsertDestination,
}

fn midpoint(a: i32, b: i32) -> i32 {
    ((a as i64 + b as i64) / 2) as i32
}

/// Work on a copy: the live source slot remains intact until release, so cancellation
/// needs no structural rollback. Destination indices always describe the source removed.
fn without_window(workspace: &Workspace, window: WindowId) -> Workspace {
    let mut workspace = workspace.clone();
    workspace.remove_window(window);
    if workspace.columns.is_empty() {
        // An empty workspace must not inherit an old strip's camera offset.
        workspace.scroll_offset = Point::default();
    }
    workspace
}

/// Use the logical strip, never cached HWND rectangles: fully off-screen windows are
/// parked beside the monitor and hidden tabs can retain old geometry. Only calculated
/// visible-tile bounds preserve monotonically ordered column boundaries after removal.
fn column_geometry(
    workspace: &Workspace,
    positions: &[(WindowId, Rect)],
) -> Option<Vec<ColumnGeometry>> {
    workspace
        .columns
        .iter()
        .map(|column| {
            let visible = column.visible_tile_indices();
            let rects: Vec<_> = visible
                .into_iter()
                .map(|index| {
                    let tile = &column.tiles[index];
                    let rect = positions.iter().find(|(w, _)| *w == tile.window_id)?.1;
                    (rect.size.w > 0 && rect.size.h > 0).then_some((index, rect))
                })
                .collect::<Option<_>>()?;
            let first = rects.first()?.1;
            let last = rects.last()?.1;
            let left = rects.iter().map(|(_, r)| r.loc.x).min()?;
            let right = rects.iter().map(|(_, r)| r.right()).max()?;
            let mut slots = vec![(rects[0].0, first.loc.y)];
            for pair in rects.windows(2) {
                slots.push((pair[1].0, midpoint(pair[0].1.bottom(), pair[1].1.loc.y)));
            }
            // Tabbed columns expose just the active tile: insert above/below that tab,
            // never use the stale rectangles of hidden tabs.
            let end = if matches!(column.display, ColumnDisplay::Tabbed { .. }) {
                rects.last()?.0 + 1
            } else {
                column.tiles.len()
            };
            slots.push((end, last.bottom()));
            Some(ColumnGeometry {
                bounds: Rect::new(
                    left,
                    first.loc.y,
                    (right - left) as u32,
                    (last.bottom() - first.loc.y).max(0) as u32,
                ),
                slots,
            })
        })
        .collect()
}

fn bounded_hint(view: Rect, x: i32, y: i32, width: u32, height: u32) -> Rect {
    let width = width.min(view.size.w).max(1);
    let height = height.min(view.size.h).max(1);
    Rect::new(
        x.clamp(view.loc.x, view.right() - width as i32),
        y.clamp(view.loc.y, view.bottom() - height as i32),
        width,
        height,
    )
}

/// Pure shared hit-test and hint description. Ties choose a new column, as in niri.
/// Half-open monitor containment is checked by the caller, allowing outer-gap/edge drops
/// even when the pointer is outside the inset tiling work rectangle.
fn insertion_destination(
    columns: &[ColumnGeometry],
    view: Rect,
    point: Point,
    column_gap: i32,
    hint_width: u32,
) -> Option<InsertDestination> {
    if view.size.w == 0 || view.size.h == 0 {
        return None;
    }
    let half_gap = column_gap.max(0) / 2;
    let (position, anchor) = if columns.is_empty() {
        (InsertPosition::NewColumn(0), view.loc.x)
    } else {
        let mut boundaries = vec![columns[0].bounds.loc.x.saturating_sub(half_gap)];
        boundaries.extend(
            columns
                .windows(2)
                .map(|pair| midpoint(pair[0].bounds.right(), pair[1].bounds.loc.x)),
        );
        boundaries.push(columns.last()?.bounds.right().saturating_add(half_gap));
        let (column_slot, &boundary) = boundaries
            .iter()
            .enumerate()
            .min_by_key(|(_, x)| (**x as i64 - point.x as i64).abs())?;
        let containing = boundaries.iter().take_while(|x| **x <= point.x).count();
        if containing == 0 || containing > columns.len() {
            (InsertPosition::NewColumn(column_slot), boundary)
        } else {
            let column = containing - 1;
            let &(slot, y) = columns[column]
                .slots
                .iter()
                .min_by_key(|(_, y)| (*y as i64 - point.y as i64).abs())?;
            let dx = (boundary as i64 - point.x as i64).abs();
            let dy = (y as i64 - point.y as i64).abs();
            if dx <= dy {
                (InsertPosition::NewColumn(column_slot), boundary)
            } else {
                (InsertPosition::InColumn(column, slot), y)
            }
        }
    };
    // This is an insertion band, not a second hit-test (niri similarly draws a
    // 300-unit new-column/interior band, 150 units at vertical edges).
    let width = hint_width.max(1);
    let hint = match position {
        InsertPosition::NewColumn(_) => bounded_hint(
            view,
            anchor.saturating_sub(width as i32 / 2),
            view.loc.y,
            width,
            view.size.h,
        ),
        InsertPosition::InColumn(column, slot) => {
            let geometry = &columns[column];
            let edge = geometry.slots.first()?.0 == slot || geometry.slots.last()?.0 == slot;
            let height = if edge { (width / 2).max(1) } else { width };
            bounded_hint(
                view,
                geometry.bounds.loc.x,
                anchor.saturating_sub(height as i32 / 2),
                geometry.bounds.size.w,
                height,
            )
        }
    };
    Some(InsertDestination { position, hint })
}

/// The one mutation primitive, also exercised without a backend by the tests.
fn insert_at(
    workspace: &mut Workspace,
    position: InsertPosition,
    mut tile: Tile,
    mut new_column: Column,
) -> Option<usize> {
    match position {
        InsertPosition::NewColumn(index) if index > workspace.columns.len() => return None,
        InsertPosition::InColumn(index, slot)
            if workspace
                .columns
                .get(index)
                .map_or(true, |c| slot > c.tiles.len()) =>
        {
            return None
        }
        _ => {}
    }
    let window = tile.window_id;
    tile.cached_bounds = Rect::default();
    tile.configure_throttle = Tile::new(window).configure_throttle;
    let column = match position {
        InsertPosition::NewColumn(index) => {
            let was_empty = workspace.columns.is_empty();
            new_column.tiles = vec![tile];
            new_column.select_window(window);
            workspace.columns.insert(index, new_column);
            if was_empty {
                workspace.scroll_offset = Point::default();
            }
            index
        }
        InsertPosition::InColumn(index, slot) => {
            let column = &mut workspace.columns[index];
            column.tiles.insert(slot, tile);
            column.select_window(window);
            index
        }
    };
    workspace.active_window = Some(window);
    Some(column)
}

impl TilingEngine {
    /// Forget a preview without touching the HWND (destroy/minimize/stale identity).
    pub(crate) fn clear_window_insert(&mut self, window: WindowId) {
        self.update_window_insert_hint(window, None);
        self.native_moves.remove(&window);
        self.animating_rects.remove(&window);
        self.applied_state.remove(&window);
    }

    /// End an abandoned Move preview without moving a tile or changing focus/workspaces.
    pub(crate) fn cancel_window_insert(&mut self, window: WindowId, backend: &BackendHandle) {
        self.clear_window_insert(window);
        if self.minimized.contains_key(&window) || !self.tiled_windows.contains_key(&window) {
            return;
        }
        // A restore acknowledgment can have been suppressed during preview, or still
        // be in flight. Settle only the former; the latter must keep its pending token
        // so the eventual normal-size event invalidates placement and applies the state.
        if self.unzoom_pending.contains_key(&window) {
            let zoomed = unsafe {
                windows::Win32::UI::WindowsAndMessaging::IsZoomed(windows::Win32::Foundation::HWND(
                    window.as_isize() as *mut std::ffi::c_void,
                ))
                .as_bool()
            };
            self.settle_cancelled_sizing(window, zoomed, backend);
        }
        if let Some(home) = self.float_homes.get(&window) {
            let rect = home.rect;
            self.place_float_from_pointer(window, rect, backend);
        } else if let Some((output, workspace, _)) = self.locate_window(window) {
            if let Some(ws) = self
                .monitors
                .get_mut(&output)
                .and_then(|m| m.workspaces.get_mut(&workspace))
            {
                for tile in ws.columns.iter_mut().flat_map(|c| &mut c.tiles) {
                    if tile.window_id == window {
                        tile.configure_throttle = Tile::new(window).configure_throttle;
                    }
                }
            }
            self.apply_layout_for_monitor(output, backend);
        }
    }

    fn normalize_insert_sizing(
        &mut self,
        window: WindowId,
        native_zoomed: bool,
        backend: &BackendHandle,
    ) {
        // Discard the old toggle intent. If the application is still natively zoomed,
        // retain only a normal-state acknowledgment so its async restore is reapplied.
        self.unzoom_pending.remove(&window);
        self.window_sizing.insert(window, super::SizingMode::Normal);
        if native_zoomed {
            self.unzoom_pending.insert(window, false);
            backend.restore_window_async(window.as_isize());
        }
    }

    fn settle_cancelled_sizing(
        &mut self,
        window: WindowId,
        native_zoomed: bool,
        backend: &BackendHandle,
    ) {
        if !native_zoomed && self.unzoom_pending.contains_key(&window) {
            self.window_size_class_changed(window, false, false, backend);
        }
    }

    fn insertion_workspace(&self, workspace: &Workspace, window: WindowId) -> Workspace {
        let mut copy = workspace.clone();
        // Size-state overrides belong to the old slot, not the moved tile. Restore
        // the source column's ordinary width in the planned post-removal workspace.
        if self.slot_forced.contains(&window) {
            if let Some(index) = copy.find_window_column(window) {
                let column = &mut copy.columns[index];
                if column.maximized {
                    column.toggle_maximized(0);
                }
            }
        }
        without_window(&copy, window)
    }

    fn window_insert_destination(
        &self,
        window: WindowId,
        point: Point,
    ) -> Option<WindowInsertDestination> {
        if self.is_overview()
            || !self.tiled_windows.contains_key(&window)
            || self.minimized.contains_key(&window)
            || self.sticky_windows.contains(&window)
            || (!self.floating_windows.contains(&window) && self.locate_window(window).is_none())
        {
            return None;
        }
        let (&output, monitor) = self
            .monitors
            .iter()
            .find(|(_, monitor)| monitor.bounds.contains_point(point))?;
        let cfg = self.effective_config(output);
        let view = Self::tiling_work_rect(monitor, cfg);
        let workspace = self.insertion_workspace(monitor.workspace()?, window);
        if workspace.layout_mode != crate::layout::workspace::WorkspaceLayout::Scrolling {
            return None;
        }
        let fallback =
            self.calculate_positions(&workspace, Self::precise_tiling_area(monitor, cfg), cfg);
        let columns = column_geometry(&workspace, &fallback)?;
        let destination = insertion_destination(
            &columns,
            view,
            point,
            cfg.column_gap,
            (300.0 * monitor.scale_factor)
                .round()
                .clamp(1.0, i32::MAX as f64) as u32,
        )?;
        Some(WindowInsertDestination {
            output,
            workspace: monitor.active_workspace,
            destination,
        })
    }

    /// General Move-grab hint. `None` always clears it, including after leaving all
    /// monitors, Escape, Mod release, hook shutdown, and unsuccessful drops.
    /// The older `update_insert_hint(Option<i32>)` remains column-reorder-only.
    pub fn update_window_insert_hint(&self, window: WindowId, point: Option<Point>) {
        let Some(sink) = &self.decor else { return };
        let spec = (|| {
            let target = self.window_insert_destination(window, point?)?;
            let cfg = self.effective_config(target.output);
            if cfg.effects.insert_hint.off {
                return None;
            }
            Some(DecorSpec::hint(HintSpec {
                rect: target.destination.hint,
                radius: super::decor::window_radius(
                    cfg.effects.corner_radius,
                    crate::backend::osversion::supports_rounded_corners(),
                ),
                paint: paint_of(
                    &cfg.effects.insert_hint.paint,
                    crate::backend::accent::current_windows_accent_rgba(),
                )?,
                view: self.monitors.get(&target.output)?.work_area,
            }))
        })();
        sink.set_output(TRANSIENT_OUTPUT, spec.into_iter().collect());
    }

    /// Recompute the preview's pure destination, then move exactly one tile there.
    /// Returns false for an invalid/outside drop; the input path restores its source.
    pub fn finish_window_insert(
        &mut self,
        window: WindowId,
        point: Point,
        backend: &BackendHandle,
    ) -> bool {
        self.update_window_insert_hint(window, None);
        let Some(target) = self.window_insert_destination(window, point) else {
            return false;
        };
        let source = self.locate_window(window);
        let slot = source.and_then(|(output, ws, _)| {
            self.monitors
                .get(&output)?
                .workspaces
                .get(&ws)?
                .slot_of(window)
        });
        let was_floating = self.floating_windows.contains(&window);
        let float_output = was_floating
            .then(|| self.float_location(window))
            .flatten()
            .map(|h| h.0);
        let mut column = Column::new();
        if let Some(slot) = &slot {
            column.width = if self.slot_forced.contains(&window) {
                slot.column_pre_max_width.unwrap_or(slot.column_width)
            } else {
                slot.column_width
            };
            column.width_proportion = slot.column_width_proportion;
            column.width_fixed_logical = slot.column_width_fixed_logical;
            column.width_fixed_window = slot.column_width_fixed_window;
        } else {
            column.width = self
                .float_homes
                .get(&window)
                .map(|h| h.rect.size.w)
                .or_else(|| self.tiled_windows.get(&window).map(|w| w.bounds.size.w));
        }
        let tile = slot.map_or_else(|| Tile::new(window), |slot| slot.tile);
        // Prepare the target transaction before changing any live bookkeeping. This uses
        // the same source-removed workspace/index convention as the hint's hit-test.
        let mut workspace = self.insertion_workspace(
            &self.monitors.get(&target.output).unwrap().workspaces[&target.workspace],
            window,
        );
        if insert_at(&mut workspace, target.destination.position, tile, column).is_none() {
            return false;
        }
        self.leave_sizing_states(window, false);
        let native_zoomed = unsafe {
            windows::Win32::UI::WindowsAndMessaging::IsZoomed(windows::Win32::Foundation::HWND(
                window.as_isize() as *mut std::ffi::c_void,
            ))
            .as_bool()
        };
        self.normalize_insert_sizing(window, native_zoomed, backend);
        self.native_moves.remove(&window);
        self.animating_rects.remove(&window);
        // The HWND was positioned directly while dragging; even an unchanged slot
        // must be configured again instead of trusting the pre-grab target cache.
        self.applied_state.remove(&window);
        if let Some((output, _, _)) = source {
            self.monitors
                .get_mut(&output)
                .unwrap()
                .remove_window(window);
        }
        if was_floating {
            self.floating_windows.remove(&window);
            if let Some(home) = self.float_homes.remove(&window) {
                let rect = Self::live_window_rect(window).unwrap_or(home.rect);
                self.float_memory.insert(window, rect);
            }
            self.failed_position.remove(&window);
        }
        let monitor = self.monitors.get_mut(&target.output).unwrap();
        monitor.workspaces.insert(target.workspace, workspace);
        monitor.set_focus(window);
        self.monitors.set_focused(target.output);
        if matches!(target.destination.position, InsertPosition::NewColumn(_)) {
            self.apply_new_column_policy(window);
        }
        self.sync_tile_limits(false);
        let mut outputs = vec![target.output];
        if let Some((output, _, _)) = source {
            outputs.push(output);
        }
        if let Some(output) = float_output {
            outputs.push(output);
        }
        outputs.sort_by_key(|o| o.as_u64());
        outputs.dedup();
        for &output in &outputs {
            self.maintain_empty_workspace(output);
            self.refresh_strip_widths(output);
            // Cross-output removal may shorten the source strip too. Reveal its
            // replacement focus instead of leaving it beyond the old camera offset.
            self.reveal_focused_column(output);
        }
        for output in outputs {
            self.apply_layout_for_monitor(output, backend);
        }
        if was_floating {
            self.rules_after_float_change(window, backend);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(x: i32, ids: &[isize]) -> Column {
        let mut column = Column::new();
        for (i, id) in ids.iter().enumerate() {
            let mut tile = Tile::new(WindowId::new(*id));
            tile.cached_bounds = Rect::new(x, i as i32 * 300, 400, 280);
            column.tiles.push(tile);
        }
        column
    }

    fn target(workspace: &Workspace, point: Point) -> InsertDestination {
        // Pure fixtures stand in for calculate_positions's logical strip rectangles.
        let positions: Vec<_> = workspace
            .columns
            .iter()
            .flat_map(|c| c.tiles.iter().map(|t| (t.window_id, t.cached_bounds)))
            .collect();
        let geometry = column_geometry(workspace, &positions).unwrap();
        insertion_destination(&geometry, Rect::new(0, 0, 1200, 600), point, 20, 300).unwrap()
    }

    fn ids(workspace: &Workspace) -> Vec<Vec<WindowId>> {
        workspace
            .columns
            .iter()
            .map(|c| c.tiles.iter().map(|t| t.window_id).collect())
            .collect()
    }

    #[test]
    fn parked_hwnds_cannot_change_logical_column_indices() {
        let mut ws = Workspace::new();
        ws.columns = vec![
            column(-400, &[1]),
            column(-400, &[2]),
            column(20, &[3]),
            column(440, &[4]),
        ];
        let logical = [
            (WindowId::new(1), Rect::new(-820, 0, 400, 600)),
            (WindowId::new(2), Rect::new(-400, 0, 400, 600)),
            (WindowId::new(3), Rect::new(20, 0, 400, 600)),
            (WindowId::new(4), Rect::new(440, 0, 400, 600)),
        ];
        let geometry = column_geometry(&ws, &logical).unwrap();
        let destination = insertion_destination(
            &geometry,
            Rect::new(0, 0, 1200, 600),
            Point::new(220, 0),
            20,
            300,
        )
        .unwrap();
        assert_eq!(destination.position, InsertPosition::InColumn(2, 0));
        assert!(column_geometry(&ws, &logical[..3]).is_none());
    }

    #[test]
    fn removing_active_tab_uses_fallback_not_hidden_tab_cache() {
        let mut workspace = Workspace::new();
        let mut tabs = column(0, &[1, 2, 3]);
        tabs.display = ColumnDisplay::Tabbed { active_tab: 1 };
        workspace.columns.push(tabs);
        let removed = without_window(&workspace, WindowId::new(2));
        let fallback = Rect::new(0, 0, 400, 600);
        let geometry = column_geometry(&removed, &[(WindowId::new(3), fallback)]).unwrap();
        assert_eq!(geometry[0].bounds, fallback);
        assert_eq!(geometry[0].slots, vec![(1, 0), (2, 600)]);
        assert_eq!(workspace.columns[0].tiles.len(), 3);
    }

    #[test]
    fn lane_t_drop_uses_actual_variable_width_scrolled_destinations() {
        // Adapt T's nearest-column regression to W's shared hint/drop selector.
        let columns: Vec<_> = [
            Rect::new(-100, 0, 250, 700),
            Rect::new(170, 0, 650, 700),
            Rect::new(840, 0, 300, 700),
        ]
        .into_iter()
        .map(|bounds| ColumnGeometry {
            bounds,
            slots: vec![(0, 0), (1, 700)],
        })
        .collect();
        let view = Rect::new(-100, 0, 1240, 700);
        let target = |point| {
            insertion_destination(&columns, view, point, 20, 300)
                .unwrap()
                .position
        };
        assert_eq!(target(Point::new(500, 0)), InsertPosition::InColumn(1, 0));
        assert_eq!(target(Point::new(835, 300)), InsertPosition::NewColumn(2));
        assert_eq!(target(Point::new(-900, 300)), InsertPosition::NewColumn(0));
        assert_eq!(
            insertion_destination(&[], view, Point::new(10, 10), 20, 300)
                .unwrap()
                .position,
            InsertPosition::NewColumn(0)
        );
    }

    #[test]
    fn edges_make_columns_and_vertical_gaps_offer_every_slot() {
        let mut ws = Workspace::new();
        ws.columns = vec![column(20, &[1, 2]), column(440, &[3])];
        assert_eq!(
            target(&ws, Point::new(0, 150)).position,
            InsertPosition::NewColumn(0)
        );
        assert_eq!(
            target(&ws, Point::new(430, 150)).position,
            InsertPosition::NewColumn(1)
        );
        assert_eq!(
            target(&ws, Point::new(1199, 150)).position,
            InsertPosition::NewColumn(2)
        );
        for (y, slot) in [(0, 0), (290, 1), (599, 2)] {
            assert_eq!(
                target(&ws, Point::new(220, y)).position,
                InsertPosition::InColumn(0, slot)
            );
        }
        // Equidistant from column boundary x=10 and tile boundary y=0.
        assert_eq!(
            target(&ws, Point::new(110, 100)).position,
            InsertPosition::NewColumn(0)
        );
    }

    #[test]
    fn preview_and_drop_share_normalized_indices_after_removing_the_source() {
        let mut original = Workspace::new();
        original.columns = vec![column(20, &[1]), column(440, &[2, 3])];
        for point in [
            Point::new(0, 140),
            Point::new(640, 0),
            Point::new(640, 290),
            Point::new(1199, 140),
        ] {
            let mut projected = without_window(&original, WindowId::new(1));
            let hint = target(&projected, point);
            let drop = target(&without_window(&original, WindowId::new(1)), point);
            assert_eq!(hint, drop);
            let index = insert_at(
                &mut projected,
                drop.position,
                Tile::new(WindowId::new(1)),
                Column::new(),
            )
            .unwrap();
            let expected = match hint.position {
                InsertPosition::NewColumn(c) | InsertPosition::InColumn(c, _) => c,
            };
            assert_eq!(index, expected);
            assert_eq!(
                projected.find_window_column(WindowId::new(1)),
                Some(expected)
            );
            assert_eq!(
                ids(&projected)
                    .iter()
                    .flatten()
                    .filter(|w| **w == WindowId::new(1))
                    .count(),
                1
            );
        }
        // Planning does not remove anything from the live source (cancel is structural no-op).
        assert_eq!(original.columns[0].tiles[0].window_id, WindowId::new(1));
    }

    #[test]
    fn moving_within_a_stack_preserves_tile_policy_and_does_not_duplicate() {
        let mut original = Workspace::new();
        original.columns = vec![column(20, &[1, 2, 3])];
        original.columns[0].tiles[1].height_weight = 2.5;
        let tile = original.slot_of(WindowId::new(2)).unwrap().tile;
        let mut ws = without_window(&original, WindowId::new(2));
        let destination = target(&ws, Point::new(220, 0));
        assert_eq!(destination.position, InsertPosition::InColumn(0, 0));
        insert_at(&mut ws, destination.position, tile, Column::new()).unwrap();
        assert_eq!(
            ids(&ws)[0],
            vec![WindowId::new(2), WindowId::new(1), WindowId::new(3)]
        );
        assert_eq!(ws.columns[0].tiles[0].height_weight, 2.5);
    }

    #[test]
    fn float_or_cross_workspace_insert_can_stack_or_make_a_column() {
        for (point, expected) in [
            (Point::new(220, 290), InsertPosition::InColumn(0, 1)),
            (Point::new(1199, 150), InsertPosition::NewColumn(1)),
        ] {
            let mut ws = Workspace::new();
            ws.columns = vec![column(20, &[1, 2])];
            let destination = target(&ws, point);
            assert_eq!(destination.position, expected);
            insert_at(
                &mut ws,
                destination.position,
                Tile::new(WindowId::new(99)),
                Column::new(),
            )
            .unwrap();
            assert!(ws.find_window_column(WindowId::new(99)).is_some());
            assert_eq!(ws.active_window, Some(WindowId::new(99)));
        }
    }

    #[test]
    fn hidden_tabs_do_not_supply_stale_geometry_or_slots() {
        let mut ws = Workspace::new();
        ws.columns = vec![column(20, &[1, 2, 3])];
        ws.columns[0].display = ColumnDisplay::Tabbed { active_tab: 1 };
        ws.columns[0].tiles[0].cached_bounds = Rect::new(-5000, -5000, 9999, 9999);
        ws.columns[0].tiles[1].cached_bounds = Rect::new(20, 0, 400, 600);
        assert_eq!(
            target(&ws, Point::new(220, 0)).position,
            InsertPosition::InColumn(0, 1)
        );
        assert_eq!(
            target(&ws, Point::new(220, 599)).position,
            InsertPosition::InColumn(0, 2)
        );
    }

    #[test]
    fn empty_workspace_and_only_window_drop_reset_scroll_and_keep_hints_visible() {
        let view = Rect::new(-1600, -200, 1200, 800);
        for point in [Point::new(-1600, -200), Point::new(-401, 599)] {
            let destination = insertion_destination(&[], view, point, 20, 300).unwrap();
            assert_eq!(destination.position, InsertPosition::NewColumn(0));
            assert!(destination.hint.intersects(view));
            let mut ws = Workspace::new();
            ws.scroll_offset.x = 900;
            insert_at(
                &mut ws,
                destination.position,
                Tile::new(WindowId::new(1)),
                Column::new(),
            )
            .unwrap();
            assert_eq!(ws.scroll_offset, Point::default());
        }
        assert!(insertion_destination(&[], Rect::default(), Point::default(), 0, 300).is_none());
        let mut ws = Workspace::new();
        ws.columns = vec![column(20, &[1])];
        ws.scroll_offset.x = 400;
        assert_eq!(
            without_window(&ws, WindowId::new(1)).scroll_offset,
            Point::default()
        );
    }

    fn engine_fixture() -> (TilingEngine, BackendHandle, OutputId, WindowId) {
        let mut engine = TilingEngine::new(Default::default());
        let backend = BackendHandle::default_for_test();
        let output = OutputId::from_name("insert-test");
        engine.register_monitor(
            output,
            Rect::new(0, 0, 1200, 800),
            Rect::new(0, 0, 1200, 760),
        );
        let window = WindowId::new(716_101);
        engine.add_window(
            crate::backend::WindowInfo {
                hwnd: window.as_isize(),
                title: "Insertion fixture".into(),
                class_name: "TestClass".into(),
                process_id: 1000,
                bounds: Rect::new(100, 100, 400, 500),
                state: crate::backend::WindowState::Normal,
                is_visible: true,
            },
            &backend,
        );
        (engine, backend, output, window)
    }

    #[test]
    fn new_column_drop_uses_creation_policy_and_clears_pending_sizing() {
        let (mut engine, backend, output, window) = engine_fixture();
        engine.config.default_tabbed = true;
        engine.unzoom_pending.insert(window, true);
        assert!(engine.finish_window_insert(window, Point::new(0, 300), &backend));
        let (_, workspace, column) = engine.locate_window(window).unwrap();
        assert!(matches!(
            engine.monitors.get(&output).unwrap().workspaces[&workspace].columns[column].display,
            ColumnDisplay::Tabbed { .. }
        ));
        assert!(!engine.unzoom_pending.contains_key(&window));
        assert_eq!(
            engine.window_sizing[&window],
            super::super::SizingMode::Normal
        );
    }

    #[test]
    fn pending_restore_cancellation_waits_for_ack_but_drop_discards_old_toggle_intent() {
        for wanted in [false, true] {
            let (mut engine, backend, _, window) = engine_fixture();
            if !wanted {
                engine.set_maximized_to_edges(window, true, &backend);
            }
            engine.unzoom_pending.insert(window, wanted);
            engine.settle_cancelled_sizing(window, true, &backend);
            assert_eq!(engine.unzoom_pending.get(&window), Some(&wanted));
            assert_eq!(engine.is_maximized_to_edges(window), !wanted);
            engine.settle_cancelled_sizing(window, false, &backend);
            assert!(!engine.unzoom_pending.contains_key(&window));
            assert_eq!(engine.is_maximized_to_edges(window), wanted);
        }
        let (mut engine, backend, _, window) = engine_fixture();
        engine.unzoom_pending.insert(window, true);
        engine.normalize_insert_sizing(window, true, &backend);
        assert_eq!(engine.unzoom_pending.get(&window), Some(&false));
        engine.window_size_class_changed(window, false, false, &backend);
        assert!(!engine.unzoom_pending.contains_key(&window));
        assert!(!engine.is_maximized_to_edges(window));
        assert!(engine.applied_state.contains_key(&window));
    }

    #[test]
    fn non_scrolling_workspace_has_no_scrolling_insert_target() {
        let (mut engine, _, output, window) = engine_fixture();
        for mode in [
            crate::layout::workspace::WorkspaceLayout::BStack,
            crate::layout::workspace::WorkspaceLayout::Spiral,
        ] {
            engine
                .monitors
                .get_mut(&output)
                .unwrap()
                .workspace_mut()
                .unwrap()
                .layout_mode = mode;
            assert!(engine
                .window_insert_destination(window, Point::new(300, 300))
                .is_none());
        }
    }

    #[test]
    fn cross_output_drop_reveals_the_shortened_source_strip() {
        let (mut engine, backend, output, window) = engine_fixture();
        let replacement = WindowId::new(716_102);
        engine.add_window(
            crate::backend::WindowInfo {
                hwnd: replacement.as_isize(),
                title: "Source replacement".into(),
                class_name: "TestClass".into(),
                process_id: 1000,
                bounds: Rect::new(100, 100, 400, 500),
                state: crate::backend::WindowState::Normal,
                is_visible: true,
            },
            &backend,
        );
        let target = OutputId::from_name("insert-target");
        engine.register_monitor(
            target,
            Rect::new(1200, 0, 1200, 800),
            Rect::new(1200, 0, 1200, 760),
        );
        {
            let workspace = engine
                .monitors
                .get_mut(&output)
                .unwrap()
                .workspace_mut()
                .unwrap();
            workspace.scroll_offset.x = 10_000;
            for tile in workspace.columns.iter_mut().flat_map(|c| &mut c.tiles) {
                tile.preferred_height = Some(if tile.window_id == window { 231 } else { 437 });
            }
        }
        assert!(engine.finish_window_insert(window, Point::new(1200, 300), &backend));
        assert_eq!(engine.locate_window(window).unwrap().0, target);
        let monitor = engine.monitors.get(&output).unwrap();
        let workspace = monitor.workspace().unwrap();
        let cfg = engine.effective_config(output);
        let view = TilingEngine::tiling_work_rect(monitor, cfg);
        let positions = engine.calculate_positions(workspace, view, cfg);
        assert!(positions
            .iter()
            .any(|(id, rect)| *id == replacement && rect.intersects(view)));
        assert_eq!(
            workspace
                .slot_of(replacement)
                .unwrap()
                .tile
                .preferred_height,
            Some(437)
        );
        let (_, workspace, _) = engine.locate_window(window).unwrap();
        assert_eq!(
            engine.monitors.get(&target).unwrap().workspaces[&workspace]
                .slot_of(window)
                .unwrap()
                .tile
                .preferred_height,
            Some(231)
        );
    }

    #[test]
    fn cancellation_settles_pending_maximize_without_moving_the_source_slot() {
        let (mut engine, backend, _, window) = engine_fixture();
        let original = engine.locate_window(window);
        engine
            .native_moves
            .insert(window, Rect::new(100, 100, 400, 500));
        engine.unzoom_pending.insert(window, true);
        engine.cancel_window_insert(window, &backend);
        assert_eq!(engine.locate_window(window), original);
        assert!(engine.is_maximized_to_edges(window));
        assert!(!engine.native_moves.contains_key(&window));
        assert!(!engine.unzoom_pending.contains_key(&window));
    }

    #[test]
    fn invalid_destination_is_non_mutating_and_hidden_tab_fallback_is_pure() {
        let mut ws = Workspace::new();
        ws.columns = vec![column(20, &[1])];
        let before = ids(&ws);
        assert!(insert_at(
            &mut ws,
            InsertPosition::InColumn(0, 2),
            Tile::new(WindowId::new(2)),
            Column::new()
        )
        .is_none());
        assert_eq!(ids(&ws), before);
        ws.columns[0].tiles[0].cached_bounds = Rect::default();
        let fallback = [(WindowId::new(1), Rect::new(-400, 20, 400, 500))];
        assert_eq!(
            column_geometry(&ws, &fallback).unwrap()[0].bounds,
            fallback[0].1
        );
        assert_eq!(ws.columns[0].tiles[0].cached_bounds, Rect::default());
    }
}
