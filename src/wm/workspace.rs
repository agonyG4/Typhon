use std::{fmt, num::NonZeroU32};

use super::SpecialWorkspaceId;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceManager {
    active_workspace: WorkspaceId,
    workspaces: Vec<WorkspaceId>,
    special_workspaces: Vec<SpecialWorkspaceId>,
    visible_special_workspace: Option<SpecialWorkspaceId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSwitchOutcome {
    Changed {
        previous: WorkspaceId,
        current: WorkspaceId,
    },
    NoChange,
    UnknownWorkspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialWorkspaceToggleOutcome {
    Opened { id: SpecialWorkspaceId },
    Closed { id: SpecialWorkspaceId },
    UnknownSpecial { id: SpecialWorkspaceId },
}

impl WorkspaceManager {
    pub fn new(workspace_count: u32) -> Option<Self> {
        let workspaces = (1..=workspace_count)
            .map(WorkspaceId::new)
            .collect::<Option<Vec<_>>>()?;
        let active_workspace = workspaces.first().copied()?;
        Some(Self {
            active_workspace,
            workspaces,
            special_workspaces: vec![SpecialWorkspaceId::DEFAULT],
            visible_special_workspace: None,
        })
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
            _ => {
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

    pub fn activate(&mut self, workspace: WorkspaceId) -> WorkspaceSwitchOutcome {
        if !self.contains(workspace) {
            return WorkspaceSwitchOutcome::UnknownWorkspace;
        }
        if self.active_workspace == workspace {
            return WorkspaceSwitchOutcome::NoChange;
        }
        let previous = self.active_workspace;
        self.active_workspace = workspace;
        WorkspaceSwitchOutcome::Changed {
            previous,
            current: workspace,
        }
    }

    pub fn workspaces(&self) -> impl ExactSizeIterator<Item = WorkspaceId> + '_ {
        self.workspaces.iter().copied()
    }

    pub const fn workspace_count(&self) -> u32 {
        self.workspaces.len() as u32
    }

    pub fn contains(&self, workspace: WorkspaceId) -> bool {
        self.workspaces.binary_search(&workspace).is_ok()
    }
}

impl Default for WorkspaceManager {
    fn default() -> Self {
        Self::new(DEFAULT_WORKSPACE_COUNT).expect("default workspace count is non-zero")
    }
}
