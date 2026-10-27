//! Event-driven effective layout views for named workspaces.
use super::*;

#[cfg(test)]
mod tests;

impl TilingEngine {
    pub(super) fn refresh_named_config(&mut self) {
        self.config_per_monitor.clear();
        self.config_per_workspace.clear();
        let Some(mut config) = self.full_config.clone() else {
            self.named_workspace_outputs.clear();
            return;
        };
        // ReserveArea updates the runtime global base; explicit output/workspace
        // overrides retain their normal precedence. Keep it across name/hotplug events.
        config.layout.reserve_top = self.config.reserve_top;
        config.layout.reserve_bottom = self.config.reserve_bottom;
        config.layout.reserve_left = self.config.reserve_left;
        config.layout.reserve_right = self.config.reserve_right;
        for output in &config.output {
            let Some(partial) = output.layout_override.as_ref() else {
                continue;
            };
            if output.name.is_empty() {
                continue;
            }
            let id = self
                .find_output_by_name(&output.name)
                .unwrap_or_else(|| OutputId::from_name(&output.name));
            let mut merged = config.clone();
            merged.layout = partial.apply_to(&config.layout);
            self.config_per_monitor
                .insert(id, LayoutConfig::from_config(&merged));
        }
        let outputs: Vec<_> = self.monitors.keys().copied().collect();
        let mut preferred = HashMap::new();
        for workspace in &config.workspace {
            let Some(id) = self.workspace_id_by_name(&workspace.name) else {
                continue;
            };
            let preferences: Vec<&str> = if workspace.open_on_outputs.is_empty() {
                if workspace.monitor.is_empty() {
                    Vec::new()
                } else {
                    vec![workspace.monitor.as_str()]
                }
            } else {
                workspace
                    .open_on_outputs
                    .iter()
                    .map(String::as_str)
                    .collect()
            };
            if !preferences.is_empty() {
                let owner = preferences
                    .iter()
                    .find_map(|name| self.find_output_by_name(name))
                    .or_else(|| {
                        self.named_workspace_outputs
                            .get(&id)
                            .copied()
                            .filter(|output| self.monitors.get(output).is_some())
                    })
                    .or_else(|| {
                        outputs.iter().copied().find(|output| {
                            self.monitors
                                .get(output)
                                .and_then(|monitor| monitor.workspaces.get(&id))
                                .is_some_and(|workspace| !workspace.columns.is_empty())
                                || self
                                    .float_homes
                                    .values()
                                    .any(|home| home.output == *output && home.workspace == id)
                                || self
                                    .minimized
                                    .values()
                                    .any(|home| home.output == *output && home.workspace == id)
                        })
                    })
                    .or_else(|| self.monitors.focused_id())
                    .or_else(|| outputs.first().copied());
                if let Some(owner) = owner {
                    preferred.insert(id, owner);
                }
            }
            let Some(partial) = workspace.layout_override.as_ref() else {
                continue;
            };
            for output in &outputs {
                let mut base = config.layout.clone();
                for candidate in &config.output {
                    if candidate.name.is_empty() {
                        continue;
                    }
                    let candidate_id = self
                        .find_output_by_name(&candidate.name)
                        .unwrap_or_else(|| OutputId::from_name(&candidate.name));
                    if candidate_id == *output {
                        if let Some(output_layout) = &candidate.layout_override {
                            base = output_layout.apply_to(&base);
                        }
                    }
                }
                let mut merged = config.clone();
                merged.layout = partial.apply_to(&base);
                self.config_per_workspace
                    .insert((*output, id), LayoutConfig::from_config(&merged));
            }
        }
        for (&workspace, &output) in &preferred {
            self.move_named_workspace_to(workspace, output);
        }
        self.named_workspace_outputs = preferred;
    }

    /// Preferences are reevaluated only on config/output/name events. Move whole
    /// columns (not re-added windows) so widths, tabs and scrolling survive. The
    /// existing per-monitor empty slots remain for numeric-workspace compatibility.
    fn move_named_workspace_to(&mut self, workspace: i32, target: OutputId) {
        let Some(target_work) = self.monitors.get(&target).map(|monitor| monitor.work_area) else {
            return;
        };
        let sources: Vec<_> = self
            .monitors
            .keys()
            .copied()
            .filter(|id| *id != target)
            .collect();
        for source in sources {
            let Some(source_monitor) = self.monitors.get_mut(&source) else {
                continue;
            };
            let source_work = source_monitor.work_area;
            let strip = source_monitor
                .workspaces
                .get_mut(&workspace)
                .map(|strip| std::mem::replace(strip, crate::layout::Workspace::new()));
            let mut windows: Vec<_> = strip
                .as_ref()
                .into_iter()
                .flat_map(|strip| &strip.columns)
                .flat_map(|column| &column.tiles)
                .map(|tile| tile.window_id)
                .collect();
            let focus = source_monitor.focus_window;
            let owns_workspace = self.named_workspace_outputs.get(&workspace) == Some(&source)
                || !windows.is_empty()
                || self
                    .float_homes
                    .values()
                    .any(|home| home.output == source && home.workspace == workspace)
                || self
                    .minimized
                    .values()
                    .any(|home| home.output == source && home.workspace == workspace);
            let offset = self
                .monitors
                .get(&target)
                .and_then(|monitor| monitor.workspaces.get(&workspace))
                .map_or(0, |strip| strip.columns.len());
            // Empty does not mean stateless: an all-minimized workspace still
            // owns its selected layout mode and scroll/focus memory.
            if let Some(mut strip) = strip.filter(|_| owns_workspace) {
                for tile in strip
                    .columns
                    .iter_mut()
                    .flat_map(|column| &mut column.tiles)
                {
                    tile.cached_bounds = Rect::default();
                }
                let destination = self
                    .monitors
                    .get_mut(&target)
                    .unwrap()
                    .workspaces
                    .entry(workspace)
                    .or_insert_with(crate::layout::Workspace::new);
                if destination.columns.is_empty() {
                    *destination = strip;
                } else {
                    destination.columns.append(&mut strip.columns);
                }
            }
            for (window, home) in &mut self.float_homes {
                if home.output == source && home.workspace == workspace {
                    home.output = target;
                    home.rect.loc.x += target_work.loc.x - source_work.loc.x;
                    home.rect.loc.y += target_work.loc.y - source_work.loc.y;
                    home.rect = Self::fit_rect_into(home.rect, target_work);
                    windows.push(*window);
                }
            }
            for (window, home) in &mut self.minimized {
                if home.output == source && home.workspace == workspace {
                    home.output = target;
                    if let Some(slot) = &mut home.slot {
                        slot.column = slot.column.saturating_add(offset);
                    }
                    windows.push(*window);
                }
            }
            if windows.is_empty() {
                continue;
            }
            if let Some(monitor) = self.monitors.get_mut(&source) {
                for window in &windows {
                    monitor.focus_ring.remove(*window);
                }
                if focus.is_some_and(|window| windows.contains(&window)) {
                    monitor.focus_window = None;
                    monitor.focus_column = None;
                }
            }
            let follow_focus = self.monitors.focused_id() == Some(source)
                && focus.is_some_and(|window| windows.contains(&window));
            if let Some(monitor) = self.monitors.get_mut(&target) {
                for window in &windows {
                    monitor.focus_ring.push_back(*window);
                }
                if follow_focus {
                    monitor.switch_workspace(workspace);
                }
                if monitor.active_workspace == workspace {
                    if let Some(window) = focus.filter(|window| windows.contains(window)) {
                        monitor.focus_window = Some(window);
                        monitor.focus_column = monitor
                            .workspace()
                            .and_then(|strip| strip.find_window_column(window));
                    }
                }
            }
            if follow_focus {
                self.monitors.set_focused(target);
            }
            for window in windows {
                self.applied_state.remove(&window);
            }
            self.maintain_empty_workspace(source);
        }
        self.maintain_empty_workspace(target);
    }

    pub(super) fn effective_config_for_workspace(
        &self,
        output: OutputId,
        workspace: i32,
    ) -> &LayoutConfig {
        self.config_per_workspace
            .get(&(output, workspace))
            .unwrap_or_else(|| self.effective_config(output))
    }

    pub(super) fn active_workspace_config(&self, output: OutputId) -> &LayoutConfig {
        self.monitors
            .get(&output)
            .map(|monitor| self.effective_config_for_workspace(output, monitor.active_workspace))
            .unwrap_or_else(|| self.effective_config(output))
    }

    pub(super) fn config_for_window(&self, window: WindowId, output: OutputId) -> &LayoutConfig {
        let place = self
            .locate_window(window)
            .map(|(output, workspace, _)| (output, workspace))
            .or_else(|| {
                self.float_homes
                    .get(&window)
                    .map(|home| (home.output, home.workspace))
            })
            .or_else(|| {
                self.minimized
                    .get(&window)
                    .map(|home| (home.output, home.workspace))
            });
        place
            .map(|(output, workspace)| self.effective_config_for_workspace(output, workspace))
            .unwrap_or_else(|| self.active_workspace_config(output))
    }

    pub(super) fn named_workspace_output(&self, workspace: i32) -> Option<OutputId> {
        self.named_workspace_outputs
            .get(&workspace)
            .copied()
            .filter(|output| self.monitors.get(output).is_some())
    }
}
