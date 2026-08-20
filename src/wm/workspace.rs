use std::{fmt, num::NonZeroU32};

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

impl WorkspaceManager {
    pub fn new(workspace_count: u32) -> Option<Self> {
        let workspaces = (1..=workspace_count)
            .map(WorkspaceId::new)
            .collect::<Option<Vec<_>>>()?;
        let active_workspace = workspaces.first().copied()?;
        Some(Self {
            active_workspace,
            workspaces,
        })
    }

    pub const fn active_workspace(&self) -> WorkspaceId {
        self.active_workspace
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
