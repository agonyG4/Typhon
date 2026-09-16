use crate::core::{SceneNodeId, WindowId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SceneSource {
    Surface(u32),
    WindowGroup(WindowId),
    ServerDecoration(WindowId),
}

/// Lifetime authority for a canonical scene node. This is deliberately
/// separate from visual ancestry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SceneOwner {
    Surface(u32),
    Window(WindowId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SceneRole {
    UnassignedSurface,
    WindowGroup,
    ClientSurface,
    PopupSurface,
    Subsurface,
    LayerSurface,
    CursorSurface,
    DragIcon,
    ServerDecoration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SceneDomain {
    Desktop,
    Content,
    Chrome,
    Input,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SceneDomainAssignment {
    Explicit(SceneDomain),
    Inherit { fallback: SceneDomain },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SceneNodeMetadata {
    pub(crate) id: SceneNodeId,
    pub(crate) source: SceneSource,
    pub(crate) owner: SceneOwner,
    pub(crate) role: SceneRole,
    pub(crate) domain: SceneDomainAssignment,
    pub(crate) visual_parent: Option<SceneNodeId>,
}
