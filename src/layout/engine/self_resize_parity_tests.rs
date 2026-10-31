//! Windows keep the final say over their size; the layout adapts.
//!
//! Pin e19df12cf8.  niri reads the size a client committed as the truth and the column width
//! it asked for as a request:
//!
//! * `Column::width` (`src/layout/scrolling.rs:4860-4875`) is the widest size the column's
//!   windows committed, and `ScrollingSpace::update_window` (`scrolling.rs:1287-1305`) moves
//!   the columns to the right by the difference instead of re-dividing them.
//! * `Column::update_window` (`scrolling.rs:4480-4527`) moves the windows below a resized
//!   window of a stacked column in tandem with its height.
//! * `Column::update_tile_sizes` clamps the requested width to the windows' minimum and
//!   maximum sizes, where the minimum wins (`scrolling.rs:4622-4627`).
//! * The camera keeps the resized active column visible (`scrolling.rs:1381-1401`).
//!
//! Every test below drives the real engine with fake windows (negative HWNDs, no real
//! window, input pump, hook or sink), zero gaps and borders so that one pixel of tile space
//! is one pixel of window space.
use super::*;
use crate::layout::Column;

fn fixture(ids: &[isize]) -> (TilingEngine, BackendHandle, OutputId) {
    let mut cfg = LayoutConfig {
        outer_gaps: (0, 0, 0, 0),
        column_gap: 0,
        window_gap: 0,
        border_width: 0,
        reserve_top: 0,
        reserve_bottom: 0,
        reserve_left: 0,
        reserve_right: 0,
        // niri's scrolling layout shares the working area among the columns
        // (`ScrollingSpace::working_area` and the column proportions), so one column fills the
        // view and three split it equally.
        column_width_mode: ColumnWidthMode::Proportional,
        ..LayoutConfig::default()
    };
    cfg.effects.border.off = true;
    cfg.effects.focus_ring.off = true;
    cfg.effects.tab_indicator.off = true;
    let mut engine = TilingEngine::new(cfg);
    let output = OutputId::from_name("self-resize");
    engine.register_monitor(
        output,
        Rect::new(0, 0, 1200, 900),
        Rect::new(0, 0, 1200, 900),
    );
    let backend = BackendHandle::default_for_test();
    let monitor = engine.monitors.get_mut(&output).unwrap();
    for id in ids {
        let mut column = Column::new();
        column.add_tile(crate::layout::Tile::new(WindowId::new(*id)));
        monitor.workspace_mut().unwrap().columns.push(column);
    }
    monitor.focus_column = Some(0);
    monitor.focus_window = ids.first().map(|id| WindowId::new(*id));
    apply(&mut engine, &backend);
    (engine, backend, output)
}

/// One column of several stacked windows.
fn stacked_fixture(ids: &[isize]) -> (TilingEngine, BackendHandle, OutputId) {
    let mut cfg = LayoutConfig {
        outer_gaps: (0, 0, 0, 0),
        column_gap: 0,
        window_gap: 0,
        border_width: 0,
        reserve_top: 0,
        reserve_bottom: 0,
        reserve_left: 0,
        reserve_right: 0,
        column_width_mode: ColumnWidthMode::Proportional,
        ..LayoutConfig::default()
    };
    cfg.effects.border.off = true;
    cfg.effects.focus_ring.off = true;
    cfg.effects.tab_indicator.off = true;
    let mut engine = TilingEngine::new(cfg);
    let output = OutputId::from_name("self-resize-stacked");
    engine.register_monitor(
        output,
        Rect::new(0, 0, 1200, 900),
        Rect::new(0, 0, 1200, 900),
    );
    let backend = BackendHandle::default_for_test();
    let mut column = Column::new();
    for id in ids {
        column.add_tile(crate::layout::Tile::new(WindowId::new(*id)));
    }
    let monitor = engine.monitors.get_mut(&output).unwrap();
    monitor.workspace_mut().unwrap().columns.push(column);
    monitor.focus_column = Some(0);
    monitor.focus_window = ids.first().map(|id| WindowId::new(*id));
    apply(&mut engine, &backend);
    (engine, backend, output)
}

fn positions(engine: &TilingEngine, output: OutputId) -> Vec<(WindowId, Rect)> {
    let monitor = engine.monitors.get(&output).unwrap();
    let cfg = engine.active_workspace_config(output).clone();
    engine.calculate_positions(
        monitor.workspace().unwrap(),
        Rect::new(0, 0, 1200, 900),
        &cfg,
    )
}

/// The rectangle the window occupies after the last layout pass.
fn rect_of(engine: &TilingEngine, output: OutputId, id: WindowId) -> Rect {
    positions(engine, output)
        .into_iter()
        .find(|(window, _)| *window == id)
        .unwrap_or_else(|| panic!("{id:?} is not laid out"))
        .1
}

/// The rectangle wiri last handed to SetWindowPos for `id`.
fn applied_rect(engine: &TilingEngine, id: WindowId) -> Rect {
    engine.applied_state[&id].rect
}

fn observed(rect: Rect, width: u32, height: u32) -> Rect {
    Rect::new(rect.loc.x, rect.loc.y, width, height)
}

fn workspace<'a>(engine: &'a TilingEngine, output: OutputId) -> &'a crate::layout::Workspace {
    engine.monitors.get(&output).unwrap().workspace().unwrap()
}

/// A layout pass with the per-tile 60 fps configure throttle forgotten, so that "the next
/// pass asks for nothing" is a statement about the rectangles and not about the rate limiter.
fn apply(engine: &mut TilingEngine, backend: &BackendHandle) {
    for (_, monitor) in engine.monitors.iter_mut() {
        for workspace in monitor.workspaces.values_mut() {
            for column in &mut workspace.columns {
                for tile in &mut column.tiles {
                    tile.configure_throttle = crate::layout::ConfigureThrottle::default_60fps();
                }
            }
        }
    }
    engine.apply_all(backend);
}

#[test]
fn single_column_keeps_the_size_the_window_chose() {
    // niri: the column is as wide as its window committed (scrolling.rs:4860-4875).
    let (mut engine, backend, output) = fixture(&[-9401]);
    let id = WindowId::new(-9401);
    let before = applied_rect(&engine, id);
    assert_eq!(before.size.w, 1200, "one column fills the view");

    let chosen = observed(before, 700, 640);
    assert!(
        engine.adopt_self_resize(id, chosen),
        "the window's own size is adopted"
    );
    assert_eq!(
        rect_of(&engine, output, id).size.w,
        700,
        "the tile follows the window, not the request"
    );

    // The next pass asks for exactly what the window already is, so it is not asked again and
    // cannot be pulled back to the old width.
    apply(&mut engine, &backend);
    assert_eq!(applied_rect(&engine, id).size.w, 700);
    engine.win32_call_count = 0;
    apply(&mut engine, &backend);
    assert_eq!(
        engine.win32_call_count, 0,
        "a settled self-resize issues no further Win32 call"
    );
}

#[test]
fn the_windows_own_write_is_not_adopted_twice() {
    // Only a size that differs from the rectangle wiri applied is the window's doing.
    let (mut engine, _, _) = fixture(&[-9402]);
    let id = WindowId::new(-9402);
    let applied = applied_rect(&engine, id);
    assert!(!engine.adopt_self_resize(id, applied));
    let jitter = Rect::new(
        applied.loc.x + 1,
        applied.loc.y,
        applied.size.w + 1,
        applied.size.h,
    );
    assert!(
        !engine.adopt_self_resize(id, jitter),
        "one pixel of frame adjustment is still wiri's own write"
    );
}

#[test]
fn an_in_flight_batch_is_never_mistaken_for_a_self_resize() {
    // wiri's own SetWindowPos batch, and an animation frame, write the geometry themselves.
    let (mut engine, _, output) = fixture(&[-9403]);
    let id = WindowId::new(-9403);
    let before = applied_rect(&engine, id);
    let chosen = observed(before, 640, 500);

    engine.applying_batch = true;
    assert!(!engine.adopt_self_resize(id, chosen), "own batch in flight");
    engine.applying_batch = false;
    engine.frame_in_progress = true;
    assert!(!engine.adopt_self_resize(id, chosen), "own animation frame");
    engine.frame_in_progress = false;
    assert!(
        engine.adopt_self_resize(id, chosen),
        "the same event outside the batch is the window's"
    );
    assert_eq!(rect_of(&engine, output, id).size.w, 640);
}

#[test]
fn a_native_caption_loop_owns_the_size_until_it_ends() {
    // native_move.rs already reconciles a drag when MOVESIZEEND arrives; adopting the
    // intermediate frames as well would fight the loop (niri: the grab owns the tile).
    let (mut engine, _, output) = fixture(&[-9404]);
    let id = WindowId::new(-9404);
    let before = applied_rect(&engine, id);
    engine.native_moves.insert(id, before);
    assert!(!engine.adopt_self_resize(id, observed(before, 500, 500)));
    assert_eq!(rect_of(&engine, output, id).size.w, before.size.w);
    engine.native_moves.remove(&id);
    assert!(engine.adopt_self_resize(id, observed(before, 500, 500)));
}

#[test]
fn a_floating_window_and_a_sized_tile_are_not_self_resized_tiles() {
    let (mut engine, _, _) = fixture(&[-9405]);
    let id = WindowId::new(-9405);
    let before = applied_rect(&engine, id);
    let chosen = observed(before, 500, 400);

    engine.floating_windows.insert(id);
    assert!(!engine.adopt_self_resize(id, chosen), "floating");
    engine.floating_windows.remove(&id);

    engine.window_sizing.insert(id, SizingMode::Fullscreen);
    assert!(!engine.adopt_self_resize(id, chosen), "fullscreen");
    engine.window_sizing.insert(id, SizingMode::Maximized);
    assert!(!engine.adopt_self_resize(id, chosen), "maximized");
    engine.window_sizing.remove(&id);

    // A window whose target was invalidated (a reload clears the cached rectangles) proves
    // nothing about what it was asked for.
    engine.applied_state.remove(&id);
    assert!(!engine.adopt_self_resize(id, chosen), "unknown target");
}

#[test]
fn a_proportional_column_keeps_its_proportion_intent() {
    // niri re-resolves the column from `ColumnWidth` on the next request
    // (scrolling.rs:4961-4972): the window's size is what the layout is, not what it asks for.
    let (mut engine, backend, output) = fixture(&[-9406]);
    let id = WindowId::new(-9406);
    engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .workspace_mut()
        .unwrap()
        .columns[0]
        .set_proportion(0.5, 1200.0, 0);
    apply(&mut engine, &backend);
    let before = applied_rect(&engine, id);
    assert!(engine.adopt_self_resize(id, observed(before, 900, 700)));
    let column = &workspace(&engine, output).columns[0];
    assert_eq!(
        column.width_proportion,
        Some(0.5),
        "the proportion is the layout's word for the next request"
    );
    assert_eq!(rect_of(&engine, output, id).size.w, 900);
}

#[test]
fn a_fixed_column_keeps_its_fixed_intent() {
    let (mut engine, backend, output) = fixture(&[-9407]);
    let id = WindowId::new(-9407);
    engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .workspace_mut()
        .unwrap()
        .columns[0]
        .set_fixed_logical(600.0, 1.0, 0);
    apply(&mut engine, &backend);
    let before = applied_rect(&engine, id);
    assert!(engine.adopt_self_resize(id, observed(before, 800, 700)));
    let column = &workspace(&engine, output).columns[0];
    assert_eq!(column.width_fixed_logical, Some(600.0));
    assert_eq!(rect_of(&engine, output, id).size.w, 800);
}

#[test]
fn a_stacked_column_gives_the_resized_window_its_height() {
    // niri Column::update_window moves the windows below in tandem (scrolling.rs:4480-4527);
    // here the sibling keeps its share of what is left.
    let (mut engine, backend, output) = stacked_fixture(&[-9408, -9409]);
    let top = WindowId::new(-9408);
    let bottom = WindowId::new(-9409);
    assert_eq!(rect_of(&engine, output, top).size.h, 450);
    assert_eq!(rect_of(&engine, output, bottom).size.h, 450);

    let before = applied_rect(&engine, top);
    assert!(engine.adopt_self_resize(top, observed(before, 700, 600)));
    assert_eq!(
        workspace(&engine, output).columns[0].tiles[0].preferred_height,
        Some(600),
        "the window's height becomes the tile's"
    );
    apply(&mut engine, &backend);

    assert_eq!(rect_of(&engine, output, top).size.w, 700, "column width");
    assert_eq!(rect_of(&engine, output, top).size.h, 600);
    assert_eq!(
        rect_of(&engine, output, bottom).size.h,
        300,
        "the sibling shares what is left"
    );
    assert_eq!(
        rect_of(&engine, output, bottom).loc.y,
        600,
        "the sibling moves down in tandem"
    );
    // Settled: the window is at the size it chose and the layout agrees.
    assert_eq!(applied_rect(&engine, top).size.h, 600);
}

#[test]
fn a_tabbed_column_takes_no_height_from_the_window() {
    let (mut engine, backend, output) = stacked_fixture(&[-9410, -9411]);
    let id = WindowId::new(-9410);
    engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .workspace_mut()
        .unwrap()
        .columns[0]
        .switch_to_tabbed();
    apply(&mut engine, &backend);
    let before = applied_rect(&engine, id);
    assert!(engine.adopt_self_resize(id, observed(before, 700, 300)));
    apply(&mut engine, &backend);
    assert_eq!(rect_of(&engine, output, id).size.w, 700);
    assert_eq!(
        rect_of(&engine, output, id).size.h,
        900,
        "the visible tab still fills its column"
    );
}

#[test]
fn neighbours_keep_their_width_and_move_with_the_resized_column() {
    // niri moves the columns to the right by the difference and does not re-divide them
    // (ScrollingSpace::update_window, scrolling.rs:1287-1305).
    let (mut engine, backend, output) = fixture(&[-9412, -9413, -9414]);
    let first = WindowId::new(-9412);
    let later: Vec<Rect> = [-9413, -9414]
        .iter()
        .map(|id| rect_of(&engine, output, WindowId::new(*id)))
        .collect();
    assert_eq!(later[0].size.w, later[1].size.w, "three equal columns");

    let before = applied_rect(&engine, first);
    assert!(engine.adopt_self_resize(first, observed(before, 500, 700)));
    apply(&mut engine, &backend);

    let after: Vec<Rect> = [-9413, -9414]
        .iter()
        .map(|id| rect_of(&engine, output, WindowId::new(*id)))
        .collect();
    assert_eq!(rect_of(&engine, output, first).size.w, 500);
    assert_eq!(
        after[0].size.w, later[0].size.w,
        "a neighbour keeps its width"
    );
    assert_eq!(
        after[0].loc.x - later[0].loc.x,
        500 - later[0].size.w as i32,
        "and moves by the width difference"
    );
    assert!(
        after[0].loc.x >= rect_of(&engine, output, first).loc.x + 500,
        "no overlap with the resized column"
    );
}

#[test]
fn a_grid_snapping_window_settles_without_a_ping_pong() {
    // A terminal snaps its width to a character cell: asked for 400 it commits 452, asked for
    // 452 it commits 448, and the layout must follow each answer instead of pulling it back.
    let (mut engine, backend, output) = fixture(&[-9415]);
    let id = WindowId::new(-9415);
    let mut rect = applied_rect(&engine, id);

    for cell_width in [452u32, 448, 452] {
        let snapped = observed(rect, cell_width, 700);
        assert!(engine.adopt_self_resize(id, snapped));
        apply(&mut engine, &backend);
        assert_eq!(rect_of(&engine, output, id).size.w, cell_width);
        // The window is already there: the next pass writes nothing at all.
        engine.win32_call_count = 0;
        apply(&mut engine, &backend);
        assert_eq!(
            engine.win32_call_count, 0,
            "the snapped width {cell_width} is asked for once and then agreed with"
        );
        // The snapped window's own rectangle is what the layout now expects, so the same
        // report again is not a new self-resize.
        assert!(!engine.adopt_self_resize(id, snapped));
        rect = snapped;
    }
}

#[test]
fn a_window_growing_past_the_work_area_pushes_the_strip() {
    let (mut engine, backend, output) = fixture(&[-9416, -9417]);
    let wide = WindowId::new(-9416);
    let neighbour = WindowId::new(-9417);
    let before = applied_rect(&engine, wide);
    assert!(engine.adopt_self_resize(wide, observed(before, 1600, 800)));
    apply(&mut engine, &backend);

    assert_eq!(rect_of(&engine, output, wide).size.w, 1600);
    assert_eq!(
        rect_of(&engine, output, neighbour).loc.x,
        1600,
        "the neighbour starts after the grown column"
    );
    let strip = workspace(&engine, output);
    assert!(
        strip.total_width(400, 0) >= 1600,
        "the strip is at least as wide as the column"
    );
    assert!(
        strip.scroll_offset.x >= 0,
        "the camera stays inside the strip"
    );
    // The window itself is not asked again: its width is the layout's width now.
    engine.win32_call_count = 0;
    apply(&mut engine, &backend);
    assert_eq!(engine.win32_call_count, 0);
}

#[test]
fn repeated_self_resize_never_oscillates() {
    let (mut engine, backend, output) = fixture(&[-9418]);
    let id = WindowId::new(-9418);
    let mut rect = applied_rect(&engine, id);
    for width in [500u32, 700, 500, 640] {
        let chosen = observed(rect, width, rect.size.h);
        assert!(engine.adopt_self_resize(id, chosen));
        apply(&mut engine, &backend);
        assert_eq!(rect_of(&engine, output, id).size.w, width);
        engine.win32_call_count = 0;
        apply(&mut engine, &backend);
        assert_eq!(
            engine.win32_call_count, 0,
            "each size is the last word until the window changes it again"
        );
        rect = chosen;
    }
    // A request the window cannot honour (a rule maximum) is the layout's word again on the
    // next pass (niri clamps the request to min/max, scrolling.rs:4622-4627).
    engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .workspace_mut()
        .unwrap()
        .columns[0]
        .tiles[0]
        .max_width = 600;
    apply(&mut engine, &backend);
    assert_eq!(rect_of(&engine, output, id).size.w, 600);
    assert_eq!(
        workspace(&engine, output).columns[0].tiles[0].observed_width,
        None,
        "a narrower request ends the window's word on its column"
    );
}

#[test]
fn an_explicit_width_is_the_layouts_word_again() {
    let (mut engine, backend, output) = fixture(&[-9419]);
    let id = WindowId::new(-9419);
    let before = applied_rect(&engine, id);
    assert!(engine.adopt_self_resize(id, observed(before, 700, 700)));
    let column = &mut engine
        .monitors
        .get_mut(&output)
        .unwrap()
        .workspace_mut()
        .unwrap()
        .columns[0];
    column.set_width(Some(480));
    assert_eq!(column.tiles[0].observed_width, None);
    apply(&mut engine, &backend);
    assert_eq!(rect_of(&engine, output, id).size.w, 480);
}

#[test]
fn the_camera_follows_the_focused_column_that_resized() {
    // scrolling.rs:1381-1401 keeps the resized active column visible.
    let (mut engine, backend, output) = fixture(&[-9420, -9421]);
    let first = WindowId::new(-9420);
    let second = WindowId::new(-9421);
    // Focus the second column and scroll it off to the right.
    {
        let monitor = engine.monitors.get_mut(&output).unwrap();
        monitor.focus_column = Some(1);
        monitor.focus_window = Some(second);
        monitor.workspace_mut().unwrap().scroll_offset.x = 1200;
    }
    let before = applied_rect(&engine, second);
    assert!(engine.adopt_self_resize(second, observed(before, 900, before.size.h)));
    apply(&mut engine, &backend);
    let strip = workspace(&engine, output);
    assert!(
        strip.scroll_offset.x <= 600,
        "the camera pulls the widened focused column back into view (offset {})",
        strip.scroll_offset.x
    );
    assert_eq!(rect_of(&engine, output, first).size.w, before.size.w);
}
