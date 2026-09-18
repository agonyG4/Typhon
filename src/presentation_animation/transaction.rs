use crate::core::SceneNodeId;

use super::{
    AnimationCurve, AnimationTime, PresentationPropertyKind, PresentationRect,
    PresentationRevisionId, PresentationTransactionId, PresentationVelocity,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationGeometryMutation {
    scene_node_id: Option<SceneNodeId>,
    pub start: PresentationRect,
    pub target: PresentationRect,
    pub curve: AnimationCurve,
}

impl PresentationGeometryMutation {
    pub const fn new(
        scene_node_id: SceneNodeId,
        start: PresentationRect,
        target: PresentationRect,
        curve: AnimationCurve,
    ) -> Self {
        Self {
            scene_node_id: Some(scene_node_id),
            start,
            target,
            curve,
        }
    }

    pub const fn without_owner(
        start: PresentationRect,
        target: PresentationRect,
        curve: AnimationCurve,
    ) -> Self {
        Self {
            scene_node_id: None,
            start,
            target,
            curve,
        }
    }

    pub const fn scene_node_id(self) -> Option<SceneNodeId> {
        self.scene_node_id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PresentationTransactionRequest {
    pub started_at: AnimationTime,
    pub geometry: Vec<PresentationGeometryMutation>,
}

impl PresentationTransactionRequest {
    pub fn geometry(
        started_at: AnimationTime,
        geometry: Vec<PresentationGeometryMutation>,
    ) -> Self {
        Self {
            started_at,
            geometry,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationTransactionError {
    Disabled,
    Empty,
    MissingPresentationOwner,
    DuplicateProperty,
    InvalidGeometry,
    TransactionIdExhausted,
    RevisionIdExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationTransactionMember {
    scene_node_id: SceneNodeId,
    property: PresentationPropertyKind,
    transaction_id: PresentationTransactionId,
    revision_id: PresentationRevisionId,
}

impl PresentationTransactionMember {
    pub(crate) const fn new(
        scene_node_id: SceneNodeId,
        property: PresentationPropertyKind,
        transaction_id: PresentationTransactionId,
        revision_id: PresentationRevisionId,
    ) -> Self {
        Self {
            scene_node_id,
            property,
            transaction_id,
            revision_id,
        }
    }

    pub const fn scene_node_id(self) -> SceneNodeId {
        self.scene_node_id
    }

    pub const fn property(self) -> PresentationPropertyKind {
        self.property
    }

    pub const fn transaction_id(self) -> PresentationTransactionId {
        self.transaction_id
    }

    pub const fn revision_id(self) -> PresentationRevisionId {
        self.revision_id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PresentationTransactionRecord {
    id: PresentationTransactionId,
    started_at: AnimationTime,
    members: Vec<PresentationTransactionMember>,
}

impl PresentationTransactionRecord {
    pub(crate) fn new(
        id: PresentationTransactionId,
        started_at: AnimationTime,
        members: Vec<PresentationTransactionMember>,
    ) -> Self {
        Self {
            id,
            started_at,
            members,
        }
    }

    pub const fn id(&self) -> PresentationTransactionId {
        self.id
    }

    pub const fn started_at(&self) -> AnimationTime {
        self.started_at
    }

    pub fn members(&self) -> &[PresentationTransactionMember] {
        &self.members
    }

    pub(crate) fn remove_member(&mut self, scene_node_id: SceneNodeId) {
        self.members
            .retain(|member| member.scene_node_id != scene_node_id);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PreparedGeometryMutation {
    pub scene_node_id: SceneNodeId,
    pub start: PresentationRect,
    pub target: PresentationRect,
    pub start_velocity: PresentationVelocity,
    pub curve: AnimationCurve,
    pub preserve_start_velocity: bool,
}
