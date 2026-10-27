//! Event-driven effective layout views and output preferences for named workspaces.
//!
//! Workspaces keep their stable local keys and identities (Lane F); this module never
//! creates or renumbers them. A workspace layout partial is found by the workspace's
//! *name*, so it follows the workspace across renames, reordering and moves between
//! outputs. Output preferences are reevaluated only on config, output and name events.
use super::*;
use crate::input::WorkspaceRef;

#[cfg(test)]
mod tests;

/// Resolved global -> output -> workspace layouts: workspace name, then connected output.
pub(super) type WorkspaceLayouts = HashMap<String, HashMap<OutputId, LayoutConfig>>;

impl TilingEngine {
    /// Rebuild the cached layout views and reevaluate the output preferences.
    pub(super) fn refresh_named_config(&mut self) {
        self.rebuild_layout_views();
        self.reconcile_named_workspace_homes();
    }

    fn rebuild_layout_views(&mut self) {
        self.config_per_monitor.clear();
        self.config_per_workspace.clear();
        let Some(mut config) = self.full_config.clone() else {
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
        let mut views = WorkspaceLayouts::new();
        let mut merged = config.clone();
        for workspace in &config.workspace {
            // As for placement, only the first declaration of a name counts.
            if workspace.name.is_empty() || views.contains_key(&workspace.name) {
                continue;
            }
            let Some(partial) = workspace.layout_override.as_ref() else {
                continue;
            };
            let mut per_output = HashMap::new();
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
                merged.layout = partial.apply_to(&base);
                per_output.insert(*output, LayoutConfig::from_config(&merged));
            }
            views.insert(workspace.name.clone(), per_output);
        }
        self.config_per_workspace = views;
    }

    /// The ordered output preferences of one `workspace` declaration: its repeated
    /// `open-on-output` lines, then the legacy `monitor` as the last fallback (the same
    /// list that decides where the workspace is first created).
    pub(super) fn output_preferences(
        workspace: &crate::config::types::WorkspaceConfig,
    ) -> Vec<&str> {
        let mut outputs: Vec<&str> = workspace
            .open_on_outputs
            .iter()
            .map(String::as_str)
            .collect();
        if !workspace.monitor.is_empty() && !outputs.contains(&workspace.monitor.as_str()) {
            outputs.push(workspace.monitor.as_str());
        }
        outputs
    }

    /// Preferences are reevaluated only on config/output/name events. A live named
    /// workspace moves, as one object, to the first connected output of its list; when
    /// none of them is connected it stays where it is (a connected owner is retained,
    /// and a disconnected one is already handled by the hotplug host rules). Whole
    /// workspaces move so widths, tabs, scrolling, floats and minimized homes survive.
    fn reconcile_named_workspace_homes(&mut self) {
        let Some(config) = self.full_config.as_ref() else {
            return;
        };
        let mut seen = HashSet::new();
        let mut wanted = Vec::new();
        for workspace in &config.workspace {
            if workspace.name.is_empty() || !seen.insert(workspace.name.as_str()) {
                continue;
            }
            let preferences = Self::output_preferences(workspace);
            if let Some(target) = preferences
                .iter()
                .find_map(|name| self.find_output_by_name(name))
            {
                wanted.push((workspace.name.clone(), target));
            }
        }
        for (name, target) in wanted {
            let Some((source, key)) =
                self.resolve_workspace_reference(target, &WorkspaceRef::Name(name))
            else {
                continue; // Not realized yet: ensure_named_workspaces places it.
            };
            if source != target {
                self.relocate_workspace(source, key, target, None);
            }
        }
    }

    /// The cached layout view of a workspace, found by name for the output it is on.
    pub(super) fn workspace_layout_in<'a>(
        views: &'a WorkspaceLayouts,
        output: OutputId,
        workspace: &crate::layout::Workspace,
    ) -> Option<&'a LayoutConfig> {
        views.get(workspace.name.as_deref()?)?.get(&output)
    }

    pub(super) fn effective_config_for_workspace(
        &self,
        output: OutputId,
        workspace: i32,
    ) -> &LayoutConfig {
        self.monitors
            .get(&output)
            .and_then(|monitor| monitor.workspaces.get(&workspace))
            .and_then(|ws| Self::workspace_layout_in(&self.config_per_workspace, output, ws))
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
}
