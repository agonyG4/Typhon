use std::{collections::BTreeMap, fmt, num::NonZeroU32};

use super::{SPECIAL_WORKSPACE_COUNT, SpecialWorkspaceId};
use crate::core::OutputId;

const DEFAULT_WORKSPACE_COUNT: u32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkspaceId(NonZeroU32);

impl WorkspaceId {
    pub const fn new(value: u32) -> Option<Self> {
        match NonZeroU32::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn get(self) -> u32 {
        self.0.get()
    }

    pub const fn raw(self) -> u32 {
        self.get()
    }

    pub const fn from_ewmh(index: u32) -> Option<Self> {
        match index.checked_add(1) {
            Some(value) => Self::new(value),
            None => None,
        }
    }

    pub const fn to_ewmh(self) -> u32 {
        self.get() - 1
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(formatter)
    }
}

/// Workspaces are placed on outputs, Hyprland style: every regular workspace
/// belongs to one output, each output shows one of its workspaces, and the
/// focused output's workspace is the active workspace.
///
/// Before any output is attached the manager behaves as a single implicit
/// output, so headless and test compositors keep the single-screen model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceManager {
    active_workspace: WorkspaceId,
    workspaces: Vec<WorkspaceId>,
    special_workspaces: Vec<SpecialWorkspaceId>,
    visible_special_workspace: Option<SpecialWorkspaceId>,
    focused_output: Option<OutputId>,
    output_workspaces: BTreeMap<OutputId, WorkspaceId>,
    workspace_outputs: BTreeMap<WorkspaceId, OutputId>,
}

/// Regular workspaces currently shown on some output, as a bit set over
/// workspace ids `1..=64`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct VisibleWorkspaces(u64);

impl VisibleWorkspaces {
    pub fn single(workspace: WorkspaceId) -> Self {
        let mut visible = Self::default();
        visible.insert(workspace);
        visible
    }

    fn insert(&mut self, workspace: WorkspaceId) {
        if let Some(bit) = workspace.get().checked_sub(1).filter(|bit| *bit < 64) {
            self.0 |= 1 << bit;
        }
    }

    pub const fn contains(self, workspace: WorkspaceId) -> bool {
        match workspace.get().checked_sub(1) {
            Some(bit) if bit < 64 => self.0 & (1 << bit) != 0,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSwitchOutcome {
    Changed {
        previous: WorkspaceId,
        current: WorkspaceId,
    },
    /// The workspace was already shown on another output; only output focus
    /// moved there. Nothing became visible or hidden.
    FocusedOutput {
        previous: WorkspaceId,
        current: WorkspaceId,
    },
    NoChange,
    UnknownWorkspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialWorkspaceToggleOutcome {
    Opened {
        id: SpecialWorkspaceId,
    },
    Closed {
        id: SpecialWorkspaceId,
    },
    Switched {
        previous: SpecialWorkspaceId,
        current: SpecialWorkspaceId,
    },
    UnknownSpecial {
        id: SpecialWorkspaceId,
    },
}

impl WorkspaceManager {
    pub fn new(workspace_count: u32) -> Option<Self> {
        let workspaces = (1..=workspace_count)
            .map(WorkspaceId::new)
            .collect::<Option<Vec<_>>>()?;
        let active_workspace = workspaces.first().copied()?;
        let special_workspaces = (1..=SPECIAL_WORKSPACE_COUNT)
            .map(SpecialWorkspaceId::new)
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            active_workspace,
            workspaces,
            special_workspaces,
            visible_special_workspace: None,
            focused_output: None,
            output_workspaces: BTreeMap::new(),
            workspace_outputs: BTreeMap::new(),
        })
    }

    /// Places an output in the workspace model and returns the workspace it
    /// shows. The first output takes every workspace and keeps the active one;
    /// later outputs take the lowest workspace not shown anywhere.
    pub fn attach_output(&mut self, output: OutputId) -> Option<WorkspaceId> {
        if let Some(shown) = self.output_workspaces.get(&output) {
            return Some(*shown);
        }
        let Some(focused) = self.focused_output else {
            self.focused_output = Some(output);
            self.output_workspaces.insert(output, self.active_workspace);
            for workspace in &self.workspaces {
                self.workspace_outputs.insert(*workspace, output);
            }
            return Some(self.active_workspace);
        };
        let visible = self.visible_workspaces();
        let workspace = self
            .workspaces
            .iter()
            .copied()
            .find(|workspace| !visible.contains(*workspace))?;
        debug_assert!(self.output_workspaces.contains_key(&focused));
        self.output_workspaces.insert(output, workspace);
        self.workspace_outputs.insert(workspace, output);
        Some(workspace)
    }

    /// Removes an output; its workspaces move to the focused output (or the
    /// first remaining one when the focused output is the one leaving).
    pub fn detach_output(&mut self, output: OutputId) {
        if self.output_workspaces.remove(&output).is_none() {
            return;
        }
        let Some(fallback) = self
            .focused_output
            .filter(|focused| *focused != output)
            .or_else(|| self.output_workspaces.keys().next().copied())
        else {
            // The last output left: fall back to the implicit single output.
            self.focused_output = None;
            self.workspace_outputs.clear();
            return;
        };
        for owner in self.workspace_outputs.values_mut() {
            if *owner == output {
                *owner = fallback;
            }
        }
        if self.focused_output == Some(output) {
            self.focused_output = Some(fallback);
            self.active_workspace = self.output_workspaces[&fallback];
        }
    }

    pub const fn focused_output(&self) -> Option<OutputId> {
        self.focused_output
    }

    /// Moves output focus; the active workspace becomes the one it shows.
    pub fn focus_output(&mut self, output: OutputId) -> bool {
        let Some(shown) = self.output_workspaces.get(&output).copied() else {
            return false;
        };
        if self.focused_output == Some(output) {
            return false;
        }
        self.focused_output = Some(output);
        self.active_workspace = shown;
        true
    }

    pub fn workspace_output(&self, workspace: WorkspaceId) -> Option<OutputId> {
        self.workspace_outputs.get(&workspace).copied()
    }

    pub fn output_workspace(&self, output: OutputId) -> Option<WorkspaceId> {
        self.output_workspaces.get(&output).copied()
    }

    pub fn outputs(&self) -> impl Iterator<Item = (OutputId, WorkspaceId)> + '_ {
        self.output_workspaces
            .iter()
            .map(|(output, workspace)| (*output, *workspace))
    }

    pub fn visible_workspaces(&self) -> VisibleWorkspaces {
        let mut visible = VisibleWorkspaces::single(self.active_workspace);
        for workspace in self.output_workspaces.values() {
            visible.insert(*workspace);
        }
        visible
    }

    pub fn is_visible(&self, workspace: WorkspaceId) -> bool {
        self.visible_workspaces().contains(workspace)
    }

    pub const fn active_workspace(&self) -> WorkspaceId {
        self.active_workspace
    }

    pub const fn visible_special_workspace(&self) -> Option<SpecialWorkspaceId> {
        self.visible_special_workspace
    }

    pub fn contains_special_workspace(&self, id: SpecialWorkspaceId) -> bool {
        self.special_workspaces.contains(&id)
    }

    pub fn toggle_special_workspace(
        &mut self,
        id: SpecialWorkspaceId,
    ) -> SpecialWorkspaceToggleOutcome {
        if !self.contains_special_workspace(id) {
            return SpecialWorkspaceToggleOutcome::UnknownSpecial { id };
        }
        match self.visible_special_workspace {
            Some(visible) if visible == id => {
                self.visible_special_workspace = None;
                SpecialWorkspaceToggleOutcome::Closed { id }
            }
            Some(previous) => {
                self.visible_special_workspace = Some(id);
                SpecialWorkspaceToggleOutcome::Switched {
                    previous,
                    current: id,
                }
            }
            None => {
                self.visible_special_workspace = Some(id);
                SpecialWorkspaceToggleOutcome::Opened { id }
            }
        }
    }

    pub fn set_visible_special_workspace(&mut self, id: Option<SpecialWorkspaceId>) -> bool {
        if id.is_some_and(|id| !self.contains_special_workspace(id)) {
            return false;
        }
        self.visible_special_workspace = id;
        true
    }

    /// Shows `workspace` on the output that owns it and focuses that output.
    /// `Changed.previous` is the workspace that output stopped showing, which
    /// differs from the old active workspace when focus crosses outputs.
    pub fn activate(&mut self, workspace: WorkspaceId) -> WorkspaceSwitchOutcome {
        if !self.contains(workspace) {
            return WorkspaceSwitchOutcome::UnknownWorkspace;
        }
        if self.active_workspace == workspace {
            return WorkspaceSwitchOutcome::NoChange;
        }
        let Some(output) = self.workspace_output(workspace) else {
            let previous = self.active_workspace;
            self.active_workspace = workspace;
            return WorkspaceSwitchOutcome::Changed {
                previous,
                current: workspace,
            };
        };
        let focused_previous = self.active_workspace;
        let shown = self.output_workspaces[&output];
        self.focused_output = Some(output);
        self.active_workspace = workspace;
        if shown == workspace {
            return WorkspaceSwitchOutcome::FocusedOutput {
                previous: focused_previous,
                current: workspace,
            };
        }
        self.output_workspaces.insert(output, workspace);
        WorkspaceSwitchOutcome::Changed {
            previous: shown,
            current: workspace,
        }
    }

    pub fn workspaces(&self) -> impl ExactSizeIterator<Item = WorkspaceId> + '_ {
        self.workspaces.iter().copied()
    }

    pub fn workspace_axis_position(&self, workspace: WorkspaceId) -> Option<f64> {
        self.workspaces()
            .position(|candidate| candidate == workspace)
            .map(|index| index as f64)
    }

    pub const fn workspace_count(&self) -> u32 {
        self.workspaces.len() as u32
    }

    pub fn contains(&self, workspace: WorkspaceId) -> bool {
        self.workspaces.binary_search(&workspace).is_ok()
    }
}

impl WorkspaceSwitchOutcome {
    pub const fn is_changed(self) -> bool {
        matches!(self, Self::Changed { .. })
    }
}

impl Default for WorkspaceManager {
    fn default() -> Self {
        Self::new(DEFAULT_WORKSPACE_COUNT).expect("default workspace count is non-zero")
    }
}

#[cfg(test)]
mod output_placement_tests {
    use super::*;

    fn output(raw: u64) -> OutputId {
        OutputId::from_raw(raw).unwrap()
    }

    fn workspace(raw: u32) -> WorkspaceId {
        WorkspaceId::new(raw).unwrap()
    }

    #[test]
    fn first_output_owns_every_workspace_and_keeps_the_active_one() {
        let mut manager = WorkspaceManager::new(4).unwrap();
        assert!(manager.activate(workspace(3)).is_changed());
        assert_eq!(manager.attach_output(output(1)), Some(workspace(3)));
        assert_eq!(manager.focused_output(), Some(output(1)));
        assert!(
            manager
                .workspaces()
                .all(|id| manager.workspace_output(id) == Some(output(1)))
        );
    }

    #[test]
    fn later_outputs_show_the_lowest_hidden_workspace() {
        let mut manager = WorkspaceManager::new(4).unwrap();
        manager.attach_output(output(1));
        assert_eq!(manager.attach_output(output(2)), Some(workspace(2)));
        assert_eq!(manager.workspace_output(workspace(2)), Some(output(2)));
        let visible = manager.visible_workspaces();
        assert!(visible.contains(workspace(1)) && visible.contains(workspace(2)));
        assert!(!visible.contains(workspace(3)));
        assert_eq!(manager.active_workspace(), workspace(1));
    }

    #[test]
    fn activating_a_workspace_shown_elsewhere_only_moves_focus() {
        let mut manager = WorkspaceManager::new(4).unwrap();
        manager.attach_output(output(1));
        manager.attach_output(output(2));
        assert_eq!(
            manager.activate(workspace(2)),
            WorkspaceSwitchOutcome::FocusedOutput {
                previous: workspace(1),
                current: workspace(2),
            }
        );
        assert_eq!(manager.focused_output(), Some(output(2)));
        assert_eq!(manager.output_workspace(output(1)), Some(workspace(1)));
    }

    #[test]
    fn activating_a_hidden_workspace_switches_its_owning_output() {
        let mut manager = WorkspaceManager::new(4).unwrap();
        manager.attach_output(output(1));
        manager.attach_output(output(2));
        // Workspace 3 is owned by the first output; output 2 keeps showing 2.
        assert_eq!(
            manager.activate(workspace(3)),
            WorkspaceSwitchOutcome::Changed {
                previous: workspace(1),
                current: workspace(3),
            }
        );
        assert_eq!(manager.output_workspace(output(1)), Some(workspace(3)));
        assert_eq!(manager.output_workspace(output(2)), Some(workspace(2)));
        manager.focus_output(output(2));
        assert_eq!(
            manager.activate(workspace(1)),
            WorkspaceSwitchOutcome::Changed {
                previous: workspace(3),
                current: workspace(1),
            }
        );
        assert_eq!(manager.focused_output(), Some(output(1)));
    }

    #[test]
    fn detaching_an_output_moves_its_workspaces_to_the_focused_output() {
        let mut manager = WorkspaceManager::new(4).unwrap();
        manager.attach_output(output(1));
        manager.attach_output(output(2));
        manager.activate(workspace(2));
        manager.detach_output(output(2));
        assert_eq!(manager.focused_output(), Some(output(1)));
        assert_eq!(manager.active_workspace(), workspace(1));
        assert_eq!(manager.workspace_output(workspace(2)), Some(output(1)));
        assert!(!manager.is_visible(workspace(2)));
        manager.detach_output(output(1));
        assert_eq!(manager.focused_output(), None);
        assert_eq!(manager.workspace_output(workspace(1)), None);
    }
}

#[cfg(test)]
mod workspace_axis_tests {
    use super::*;

    #[test]
    fn workspace_axis_position_uses_regular_workspace_order() {
        let manager = WorkspaceManager::new(4).unwrap();
        let workspaces = manager.workspaces().collect::<Vec<_>>();

        assert_eq!(manager.workspace_axis_position(workspaces[0]), Some(0.0));
        assert_eq!(manager.workspace_axis_position(workspaces[1]), Some(1.0));
        assert_eq!(manager.workspace_axis_position(workspaces[3]), Some(3.0));
        assert_eq!(
            manager.workspace_axis_position(WorkspaceId::new(99).unwrap()),
            None
        );
    }
}
