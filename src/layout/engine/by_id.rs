//! niri's `*ById` actions name a window and never change focus.
//!
//! Pin e19df12: `ConsumeOrExpelWindowLeft/RightById` (src/input/mod.rs:1110-1136),
//! `SwitchPresetWindowWidth[Back]ById` (1684-1697), `SwitchPresetWindowHeight[Back]ById`
//! (1704-1717), `CenterWindowById` (1728-1736), `ResetWindowHeightById` (2111-2117),
//! `ToggleWindowFloatingById`, `MoveWindowToFloatingById` and `MoveWindowToTilingById`
//! (2213-2250). Each resolves the id to the window's own workspace (any workspace of any
//! output; an unknown id does nothing) and acts there on that window in place:
//! layout/mod.rs:1930-1975, 2313-2330, 3042-3086, 3148-3170, 3178-3260 and the
//! workspace/scrolling layers beneath them. None of them selects the window, switches the
//! workspace or the output, or activates anything; a floating window is left alone by
//! consume/expel and reset-window-height, and is centred and preset-sized by the floating
//! layer (workspace.rs:1129-1147, 1190-1199, 1252-1280).
//!
//! Every route here runs under `without_activation`: layout passes inside the shared
//! float/unfloat/size code that would hand the foreground to the engine focus are skipped,
//! so native focus (no activation request) and managed focus stay exactly as they were.

use super::*;
use crate::layout::ColumnDisplay;

impl TilingEngine {
    /// Run `f` without any native activation request. The engine's own focus selection is
    /// not touched either; a later layout pass or focus action reasserts it as usual.
    ///
    /// The flag is put back by a drop guard, not by an assignment after the call: these
    /// are IPC-dispatched actions, and one that unwinds would otherwise leave activation
    /// suppressed for the rest of the process (`backend::activation_attempt_test_support`
    /// restores its recorder the same way).
    fn without_activation<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        struct Restore<'a> {
            engine: &'a mut TilingEngine,
            outer: bool,
        }
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.engine.activation_suppressed = self.outer;
            }
        }

        let outer = std::mem::replace(&mut self.activation_suppressed, true);
        let restore = Restore {
            engine: self,
            outer,
        };
        f(&mut *restore.engine)
    }

    /// niri's `active_window()` of the workspace the window is on: the tile when the
    /// workspace's tiling layer is the active one, the float when the floating layer is.
    /// A tiled focus clears `Workspace::active_float`, so the float layer is active exactly
    /// while that marker is set.
    fn is_active_in_its_workspace(&self, window: WindowId) -> bool {
        let strip_of = |output: OutputId, workspace: i32| {
            self.monitors
                .get(&output)
                .and_then(|monitor| monitor.workspaces.get(&workspace))
        };
        if self.floating_windows.contains(&window) {
            return self
                .float_location(window)
                .and_then(|(output, workspace)| strip_of(output, workspace))
                .is_some_and(|strip| strip.active_float == Some(window));
        }
        self.locate_window(window)
            .and_then(|(output, workspace, _)| strip_of(output, workspace))
            .is_some_and(|strip| {
                strip.active_float.is_none() && strip.active_window == Some(window)
            })
    }

    /// After a column operation on a named tile: keep the monitor's focus column on its
    /// focused window (its index may have shifted) and lay out without activating.
    fn finish_by_id_column_op(
        &mut self,
        output: OutputId,
        workspace: i32,
        backend: &BackendHandle,
    ) {
        if let Some(monitor) = self.monitors.get_mut(&output) {
            if monitor.active_workspace == workspace {
                let column = monitor.focus_window.and_then(|window| {
                    monitor
                        .workspace()
                        .and_then(|w| w.find_window_column(window))
                });
                if column.is_some() {
                    monitor.focus_column = column;
                }
            }
        }
        self.reveal_focused_column(output);
        self.apply_layout_for_monitor_with_activation(output, backend, false);
    }

    /// `consume-or-expel-window-left` / `-right` with an id: the named tile joins the
    /// neighbouring column or leaves its own, wherever it is. A floating window is left
    /// alone. The tile becomes its column's active one only when it already was the
    /// workspace's active window (niri `source_tile_was_active`).
    pub fn consume_or_expel_window_for(
        &mut self,
        window: WindowId,
        right: bool,
        backend: &BackendHandle,
    ) {
        if self.floating_windows.contains(&window) {
            return;
        }
        let Some((output, workspace, column)) = self.locate_window(window) else {
            return;
        };
        let was_active = self.is_active_in_its_workspace(window);
        let from_multi = self
            .monitors
            .get(&output)
            .and_then(|m| m.workspaces.get(&workspace))
            .and_then(|w| w.columns.get(column))
            .is_some_and(|c| c.tiles.len() > 1);
        let moved = {
            let Some(strip) = self
                .monitors
                .get_mut(&output)
                .and_then(|m| m.workspaces.get_mut(&workspace))
            else {
                return;
            };
            let moved = if right {
                strip.consume_or_expel_right(window)
            } else {
                strip.consume_or_expel_left(window)
            };
            if let (Some(index), true) = (moved, was_active) {
                if let Some(column) = strip.columns.get_mut(index) {
                    column.select_window(window);
                }
            }
            moved
        };
        if moved.is_none() {
            return;
        }
        self.without_activation(|engine| {
            if from_multi {
                engine.apply_new_column_policy(window);
            }
            engine.finish_by_id_column_op(output, workspace, backend);
        });
    }

    /// `center-window` with an id: a floating window is centred in its work area; a tile is
    /// centred only when its column is its workspace's active column (niri
    /// scrolling.rs:2243-2262: "we can reasonably center only the active column").
    pub fn center_window_for(&mut self, window: WindowId, backend: &BackendHandle) {
        if self.floating_windows.contains(&window) {
            self.float_center(window, backend);
            return;
        }
        let Some((output, workspace, column)) = self.locate_window(window) else {
            return;
        };
        let active_column = self.monitors.get(&output).and_then(|monitor| {
            let strip = monitor.workspaces.get(&workspace)?;
            if monitor.active_workspace == workspace && monitor.focus_window.is_some() {
                monitor.focus_column
            } else {
                strip
                    .active_window
                    .and_then(|w| strip.find_window_column(w))
            }
        });
        if active_column != Some(column) {
            return;
        }
        self.refresh_strip_widths(output);
        let cfg = self
            .effective_config_for_workspace(output, workspace)
            .clone();
        let metrics = self.monitors.get(&output).and_then(|monitor| {
            let view_width = Self::view_width_for_monitor(monitor, &cfg);
            let strip = monitor.workspaces.get(&workspace)?;
            Some((
                view_width,
                Self::workspace_default_width(strip, view_width, &cfg),
            ))
        });
        let Some((view_width, column_width)) = metrics else {
            return;
        };
        if let Some(strip) = self
            .monitors
            .get_mut(&output)
            .and_then(|m| m.workspaces.get_mut(&workspace))
        {
            strip.reveal_with_policy(
                column,
                column_width,
                cfg.column_gap,
                view_width,
                CenterFocusedColumn::Always,
                false,
            );
        }
        self.without_activation(|engine| {
            engine.apply_layout_for_monitor_with_activation(output, backend, false);
        });
    }

    /// `reset-window-height` with an id: a tabbed column resets all its tiles, a stacked one
    /// only the named tile; a floating window has nothing to reset (workspace.rs:1252-1259).
    pub fn reset_window_height_for(&mut self, window: WindowId, backend: &BackendHandle) {
        if self.floating_windows.contains(&window) {
            return;
        }
        let Some((output, workspace, column)) = self.locate_window(window) else {
            return;
        };
        let Some(column) = self
            .monitors
            .get_mut(&output)
            .and_then(|m| m.workspaces.get_mut(&workspace))
            .and_then(|w| w.columns.get_mut(column))
        else {
            return;
        };
        let tabbed = matches!(column.display, ColumnDisplay::Tabbed { .. });
        let mut reset = Vec::new();
        for tile in &mut column.tiles {
            if tabbed || tile.window_id == window {
                tile.height_weight = 1.0;
                tile.preferred_height = None;
                tile.height_proportion = None;
                tile.height_preset_idx = None;
                reset.push(tile.window_id);
            }
        }
        if reset.is_empty() {
            return;
        }
        for window in reset {
            self.rule_heights.remove(&window);
        }
        self.without_activation(|engine| {
            engine.apply_layout_for_monitor_with_activation(output, backend, false);
        });
    }

    /// `switch-preset-window-width[-back]` with an id: in the scrolling layout a window fills
    /// its column horizontally, so a tile cycles its column's width presets; a floating
    /// window cycles its own width presets.
    pub fn switch_preset_window_width_for(
        &mut self,
        window: WindowId,
        forwards: bool,
        backend: &BackendHandle,
    ) {
        if self.floating_windows.contains(&window) {
            self.float_cycle_width(window, forwards, backend);
            return;
        }
        let Some((output, workspace, column)) = self.locate_window(window) else {
            return;
        };
        if self.apply_column_width_preset(output, workspace, column, forwards) {
            self.without_activation(|engine| {
                engine.reveal_focused_column(output);
                engine.apply_layout_for_monitor_with_activation(output, backend, false);
            });
        }
    }

    /// `toggle-window-floating` with an id.
    pub fn toggle_floating_by_id(&mut self, window: WindowId, backend: &BackendHandle) {
        self.set_floating_by_id(window, None, backend);
    }

    /// `move-window-to-floating` / `move-window-to-tiling` with an id (`Some(floating)`:
    /// nothing happens when the window already is there) and `toggle-window-floating`
    /// (`None`). The moved window becomes active in its workspace only when it already was
    /// the workspace's active window (niri workspace.rs:1418-1445 `target_is_active`);
    /// otherwise the workspace's selection, the focused output and native focus stay put.
    pub fn set_floating_by_id(
        &mut self,
        window: WindowId,
        floating: Option<bool>,
        backend: &BackendHandle,
    ) {
        if !self.tiled_windows.contains_key(&window) {
            return;
        }
        let is_floating = self.floating_windows.contains(&window);
        if floating == Some(is_floating) {
            return;
        }
        let target_is_active = self.is_active_in_its_workspace(window);
        self.without_activation(|engine| {
            if is_floating {
                engine.unfloat_window_with(window, target_is_active, true, backend);
            } else {
                engine.float_window(window, backend);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The actions here are dispatched from IPC, so a panic in one unwinds through this
    /// guard. Activation must not stay suppressed for the rest of the process afterwards.
    #[test]
    fn a_panicking_action_puts_activation_back_the_way_it_was() {
        let mut engine = TilingEngine::new(Default::default());
        assert!(!engine.activation_suppressed, "nothing is suppressed yet");

        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            engine
                .without_activation(|_| -> () { panic!("an IPC-dispatched by-id action unwound") });
        }));
        assert!(unwound.is_err(), "the panic reaches the caller");
        assert!(
            !engine.activation_suppressed,
            "a panic must not leave activation suppressed for the process"
        );

        // The flag is still set *inside*, so the actions stay non-activating.
        assert!(engine.without_activation(|inner| inner.activation_suppressed));
        assert!(!engine.activation_suppressed);
    }
}
