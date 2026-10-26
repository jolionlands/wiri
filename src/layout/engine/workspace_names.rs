//! Workspace names belong to workspace identities, never to monitor-local keys.
use super::{BackendHandle, OutputId, TilingEngine, WorkspaceLayout};
use crate::input::WorkspaceRef;
use crate::layout::Workspace;
use std::collections::{HashMap, HashSet};

impl TilingEngine {
    pub fn workspace_name_on(&self, output: OutputId, id: i32) -> Option<String> {
        self.monitors
            .get(&output)?
            .workspaces
            .get(&id)?
            .name
            .as_ref()
            .filter(|name| !name.is_empty())
            .cloned()
    }

    /// Compatibility lookup: local workspace keys are meaningful on the focused output.
    pub fn workspace_name(&self, id: i32) -> Option<String> {
        self.workspace_name_on(self.monitors.focused_id()?, id)
    }

    /// Find a live workspace by its globally unique runtime name.
    pub fn workspace_id_by_name(&self, name: &str) -> Option<i32> {
        if name.is_empty() {
            return None;
        }
        for output in self.monitors.keys() {
            let Some(monitor) = self.monitors.get(output) else {
                continue;
            };
            for id in monitor.workspace_ids() {
                if monitor.workspaces.get(&id).and_then(|ws| ws.name.as_deref()) == Some(name) {
                    return Some(id);
                }
            }
        }
        None
    }

    pub fn workspace_label(&self, output: OutputId, id: i32) -> String {
        self.workspace_name_on(output, id)
            .unwrap_or_else(|| format!("Workspace {}", self.workspace_number_on(output, id)))
    }

    pub fn workspace_number_on(&self, output: OutputId, id: i32) -> i32 {
        self.monitors
            .get(&output)
            .and_then(|monitor| monitor.workspace_ids().iter().position(|candidate| *candidate == id))
            .map(|position| position as i32 + 1)
            .unwrap_or_else(|| crate::layout::workspace_number(id))
    }

    pub fn workspace_id_for_number(&self, number: i32) -> i32 {
        self.monitors
            .focused()
            .and_then(|monitor| {
                let ids = monitor.workspace_ids();
                let position = number.saturating_sub(1).max(0) as usize;
                ids.get(position).copied().or_else(|| ids.last().copied())
            })
            .unwrap_or_else(|| crate::layout::workspace_id_from_number(number))
    }

    /// Rename without changing focus or requiring a backend. Empty means unnamed.
    pub fn rename_workspace(&mut self, id: i32, new_name: &str) -> Result<(), String> {
        let output = self
            .monitors
            .focused_id()
            .ok_or_else(|| "no focused monitor".to_string())?;
        self.rename_workspace_on(output, id, new_name)
    }

    pub fn set_workspace_name(
        &mut self,
        name: &str,
        reference: Option<WorkspaceRef>,
        _backend: &BackendHandle,
    ) {
        let Some((output, id)) = self.workspace_action_target(reference.as_ref()) else {
            return;
        };
        if let Err(error) = self.rename_workspace_on(output, id, name) {
            tracing::warn!(%error, "could not set workspace name");
        }
    }

    pub fn unset_workspace_name(
        &mut self,
        reference: Option<WorkspaceRef>,
        _backend: &BackendHandle,
    ) {
        let Some((output, id)) = self.workspace_action_target(reference.as_ref()) else {
            return;
        };
        if let Err(error) = self.rename_workspace_on(output, id, "") {
            tracing::warn!(%error, "could not unset workspace name");
        }
    }

    fn rename_workspace_on(
        &mut self,
        output: OutputId,
        id: i32,
        new_name: &str,
    ) -> Result<(), String> {
        let workspace = self
            .monitors
            .get(&output)
            .and_then(|monitor| monitor.workspaces.get(&id))
            .ok_or_else(|| format!("workspace {} does not exist on monitor {:?}", id, output))?;
        let ipc_id = workspace.ipc_id;
        let old_name = workspace.name.clone();
        let name = (!new_name.is_empty()).then(|| new_name.to_string());

        // A hotplug snapshot can contain this same identity; it is not another
        // workspace. Every other identity, including parked ones, reserves its name.
        if let Some(name) = name.as_deref() {
            let conflict = self.monitors.keys().any(|output| {
                self.monitors.get(output).is_some_and(|monitor| {
                    monitor.workspaces.values().any(|workspace| {
                        workspace.ipc_id != ipc_id && workspace.name.as_deref() == Some(name)
                    })
                })
            }) || self.unplugged.values().any(|unplugged| {
                unplugged.monitor.workspaces.values().any(|workspace| {
                    workspace.ipc_id != ipc_id && workspace.name.as_deref() == Some(name)
                })
            });
            if conflict {
                return Err(format!("workspace name {:?} is already in use", name));
            }
        }

        // Validation above is read-only: a rejected rename cannot prune workspaces,
        // alter the configured-name history, or partially rename a hotplug snapshot.
        self.monitors
            .get_mut(&output)
            .and_then(|monitor| monitor.workspaces.get_mut(&id))
            .expect("validated workspace")
            .name = name.clone();
        for unplugged in self.unplugged.values_mut() {
            for workspace in unplugged.monitor.workspaces.values_mut() {
                if workspace.ipc_id == ipc_id {
                    workspace.name = name.clone();
                }
            }
        }

        // A runtime name matching a current declaration has already satisfied it,
        // even if the next reconciliation has not run yet. Never resurrect it later.
        if let Some(config) = self.full_config.as_ref() {
            for candidate in old_name.iter().chain(name.iter()) {
                if !candidate.is_empty()
                    && config.workspace.iter().any(|entry| entry.name == *candidate)
                {
                    self.configured_workspace_names.insert(candidate.clone());
                }
            }
        }
        self.maintain_empty_workspace(output);
        Ok(())
    }

    fn workspace_name_exists(&self, name: &str) -> bool {
        self.monitors.keys().any(|output| {
            self.monitors.get(output).is_some_and(|monitor| {
                monitor.workspaces.values().any(|workspace| workspace.name.as_deref() == Some(name))
            })
        }) || self.unplugged.values().any(|unplugged| {
            unplugged
                .monitor
                .workspaces
                .values()
                .any(|workspace| workspace.name.as_deref() == Some(name))
        })
    }

    fn workspace_empty_for_naming(&self, output: OutputId, id: i32) -> bool {
        self.monitors
            .get(&output)
            .and_then(|monitor| monitor.workspaces.get(&id))
            .is_some_and(|workspace| {
                workspace.columns.is_empty()
                    && workspace.name.as_deref().unwrap_or("").is_empty()
                    && !self.float_homes.values().any(|home| {
                        home.output == output && home.workspace == id
                    })
                    && !self.minimized.values().any(|home| {
                        home.output == output && home.workspace == id
                    })
            })
    }

    /// Apply each configured name once per session, in declaration order. Missing
    /// outputs remain pending, and config reloads never undo runtime renames/unsets.
    pub(super) fn ensure_named_workspaces(&mut self) {
        let declarations: Vec<_> = self
            .full_config
            .as_ref()
            .map(|config| {
                config
                    .workspace
                    .iter()
                    .map(|workspace| {
                        let mut outputs = workspace.open_on_outputs.clone();
                        if !workspace.monitor.is_empty() && !outputs.contains(&workspace.monitor) {
                            outputs.push(workspace.monitor.clone());
                        }
                        (workspace.name.clone(), outputs, workspace.layout.clone())
                    })
                    .collect()
            })
            .unwrap_or_default();
        let first_output = self.monitors.keys().next().copied();
        let mut seen = HashSet::new();
        let mut touched = Vec::new();
        let mut inserted: HashMap<OutputId, usize> = HashMap::new();

        for (name, outputs, layout) in declarations {
            // Only the first declaration of a name determines placement/layout,
            // including when that declaration's intended output is not available.
            if name.is_empty()
                || !seen.insert(name.clone())
                || self.configured_workspace_names.contains(&name)
            {
                continue;
            }
            if self.workspace_name_exists(&name) {
                self.configured_workspace_names.insert(name);
                continue;
            }
            let output = if outputs.is_empty() {
                first_output
            } else {
                outputs.iter().find_map(|name| self.find_output_by_name(name))
            };
            let Some(output) = output else {
                continue;
            };
            let Some(monitor) = self.monitors.get(&output) else {
                continue;
            };
            let ids = monitor.workspace_ids();
            let reusable = ids
                .iter()
                .copied()
                .find(|id| self.workspace_empty_for_naming(output, *id));
            let layout_mode = WorkspaceLayout::from_str(&layout);
            let monitor = self.monitors.get_mut(&output).expect("available output");
            let id = if let Some(id) = reusable {
                let workspace = monitor.workspaces.get_mut(&id).expect("ordered workspace");
                workspace.name = Some(name.clone());
                workspace.layout_mode = layout_mode;
                id
            } else {
                let id = monitor.next_workspace_id();
                let mut workspace = Workspace::new();
                workspace.name = Some(name.clone());
                workspace.layout_mode = layout_mode;
                monitor.insert_workspace(id, workspace);
                id
            };
            // Newly declared names precede existing workspaces, in declaration
            // order. Previously realized declarations never reorder runtime state.
            let position = inserted.entry(output).or_default();
            monitor.move_workspace_to_position(id, *position);
            *position += 1;
            self.configured_workspace_names.insert(name);
            if !touched.contains(&output) {
                touched.push(output);
            }
        }
        for output in touched {
            self.maintain_empty_workspace(output);
        }
    }
}
