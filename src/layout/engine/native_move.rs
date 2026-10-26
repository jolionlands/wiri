//! Native caption/edge loops are owned by the application until MOVESIZEEND.
//! Unlike compositor grabs, caption moves snap back to the existing tile slot.
use super::*;

impl TilingEngine {
    pub fn native_move_resize_start(&mut self, id: WindowId) {
        if !self.tiled_windows.contains_key(&id) || self.native_moves.contains_key(&id) {
            return;
        }
        let rect = Self::live_window_rect(id)
            .or_else(|| self.applied_state.get(&id).map(|s| s.rect))
            .or_else(|| self.tiled_windows.get(&id).map(|w| w.bounds));
        if let Some(rect) = rect {
            self.native_moves.insert(id, rect);
            // A queued frame must not take the window back from the native modal loop.
            self.animating_rects.remove(&id);
        }
    }

    pub fn native_move_resize_active(&self, id: WindowId) -> bool {
        self.native_moves.contains_key(&id)
    }

    pub fn native_move_resize_end(&mut self, id: WindowId, backend: &BackendHandle) {
        // Read the final frame unconditionally, not the throttled LOCATIONCHANGE cache.
        let rect = Self::live_window_rect(id);
        self.finish_native_move(id, rect);
        self.apply_all(backend);
    }

    fn finish_native_move(&mut self, id: WindowId, final_rect: Option<Rect>) {
        let Some(before) = self.native_moves.remove(&id) else {
            return;
        };
        // Even a move-only drop must be sent again: the old target cache is not reality.
        self.applied_state.remove(&id);
        self.animating_rects.remove(&id);
        let Some(rect) = final_rect else { return };
        if let Some(home) = self.float_homes.get_mut(&id) {
            if home.rect.size.w != rect.size.w {
                home.width_preset_idx = None;
            }
            home.rect = rect;
            self.float_memory.insert(id, rect);
            return;
        }
        let Some((output, workspace, col)) = self.locate_window(id) else {
            return;
        };
        if self
            .window_sizing
            .get(&id)
            .is_some_and(|s| *s != SizingMode::Normal)
        {
            return;
        }
        let cfg = self.effective_config(output).clone();
        let Some(monitor) = self.monitors.get_mut(&output) else {
            return;
        };
        let work = Self::tiling_work_rect(monitor, &cfg);
        let scale = monitor.scale_factor;
        let Some(column) = monitor
            .workspaces
            .get_mut(&workspace)
            .and_then(|w| w.columns.get_mut(col))
        else {
            return;
        };
        let extra = 2 * cfg.border_width.max(1);
        if rect.size.w.abs_diff(before.size.w) > 1 {
            // Store logical client width so a later DPI/work-area change does not forget it.
            let width =
                (rect.size.w as i32 + extra).clamp(Self::MIN_COLUMN_PX, Self::MAX_COLUMN_PX);
            column.set_fixed_logical((width - extra) as f64 / scale, scale, extra);
        }
        if rect.size.h.abs_diff(before.size.h) > 1 && column.visible_tile_indices().len() > 1 {
            let available =
                (work.size.h as i32 - cfg.window_gap * (column.tiles.len() as i32 - 1)).max(1);
            if let Some(tile) = column.tiles.iter_mut().find(|t| t.window_id == id) {
                // The existing height allocator gives the remaining space to the siblings.
                tile.preferred_height =
                    Some((rect.size.h as i32 + extra).clamp(1, available) as u32);
                self.rule_heights.remove(&id);
            }
        }
        self.reveal_focused_column(output);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (TilingEngine, OutputId, WindowId, BackendHandle) {
        let backend = BackendHandle::default_for_test();
        let mut engine = TilingEngine::new(LayoutConfig::default());
        let output = OutputId::from_name("native-loop");
        engine.register_monitor(
            output,
            Rect::new(0, 0, 1200, 800),
            Rect::new(0, 0, 1200, 760),
        );
        let id = WindowId::new(901);
        engine.add_window(
            WindowInfo {
                hwnd: id.as_isize(),
                title: "native".into(),
                class_name: "test".into(),
                process_id: 0,
                bounds: Rect::new(100, 100, 400, 300),
                state: crate::backend::WindowState::Normal,
                is_visible: true,
            },
            &backend,
        );
        engine.apply_all(&backend);
        (engine, output, id, backend)
    }

    #[test]
    fn native_loop_cancels_frames_and_excludes_layout_writes_until_end() {
        let (mut engine, _, id, backend) = setup();
        let prior = engine.applied_state.get(&id).unwrap().rect;
        engine.animating_rects.insert(
            id,
            crate::layout::animation::RectAnimation::new(
                prior,
                prior,
                crate::layout::animation::AnimationKind::Easing {
                    duration_ms: 1000,
                    curve: crate::layout::animation::Easing::Linear,
                },
                1.0,
            ),
        );
        engine.native_move_resize_start(id);
        assert!(!engine.animating_rects.contains_key(&id));
        engine.applied_state.remove(&id);
        engine.win32_call_count = 0;
        engine.apply_all(&backend);
        assert_eq!(
            engine.win32_call_count, 0,
            "the native modal loop owns its HWND"
        );
        assert!(engine.native_move_resize_active(id));
        engine.finish_native_move(id, Some(prior));
        assert!(!engine.native_move_resize_active(id));
        engine.apply_all(&backend);
        assert!(engine.win32_call_count > 0);
    }

    #[test]
    fn caption_move_invalidates_target_but_does_not_change_column_or_slot() {
        let (mut engine, output, id, _) = setup();
        let before = engine.applied_state.get(&id).unwrap().rect;
        let width = engine
            .monitors
            .get(&output)
            .unwrap()
            .workspace()
            .unwrap()
            .columns[0]
            .width;
        engine.native_move_resize_start(id);
        engine.finish_native_move(id, Some(Rect::new(300, 200, before.size.w, before.size.h)));
        assert!(!engine.applied_state.contains_key(&id));
        assert_eq!(engine.locate_window(id), Some((output, 0, 0)));
        assert_eq!(
            engine
                .monitors
                .get(&output)
                .unwrap()
                .workspace()
                .unwrap()
                .columns[0]
                .width,
            width
        );
    }

    #[test]
    fn edge_resize_updates_column_and_only_the_resized_stacked_height() {
        let (mut engine, output, id, backend) = setup();
        engine
            .monitors
            .get_mut(&output)
            .unwrap()
            .workspace_mut()
            .unwrap()
            .add_window_to_column(0, WindowId::new(902));
        engine.apply_all(&backend);
        engine.native_move_resize_start(id);
        engine.finish_native_move(id, Some(Rect::new(10, 10, 600, 200)));
        let ws = engine.monitors.get(&output).unwrap().workspace().unwrap();
        assert_eq!(ws.columns[0].width, Some(608));
        assert_eq!(ws.columns[0].width_fixed_logical, Some(600.0));
        assert_eq!(ws.columns[0].tiles[0].preferred_height, Some(208));
        assert_eq!(ws.columns[0].tiles[1].preferred_height, None);
    }

    #[test]
    fn floating_native_drop_remembers_rect_and_destroy_clears_loop() {
        let (mut engine, _, id, backend) = setup();
        engine.toggle_floating_for(id, &backend);
        engine.native_move_resize_start(id);
        let rect = Rect::new(150, 170, 430, 240);
        engine.finish_native_move(id, Some(rect));
        assert_eq!(engine.float_memory.get(&id), Some(&rect));
        assert_eq!(engine.float_homes.get(&id).unwrap().rect, rect);
        engine.native_move_resize_start(id);
        engine.remove_window(id, &backend);
        assert!(!engine.native_move_resize_active(id));
        engine.finish_native_move(id, Some(rect));
        assert!(!engine.float_memory.contains_key(&id));
    }
}
