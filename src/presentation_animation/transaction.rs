use crate::core::SceneNodeId;

use super::{
    AnimationCurve, AnimationTime, PresentationClip, PresentationClipRect, PresentationOpacity,
    PresentationPropertyKind, PresentationRect, PresentationRetainedVisualIdentity,
    PresentationRevisionId, PresentationTransactionId, PresentationTransactionMemberKind,
    PresentationVelocity,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationGeometryMutation {
    scene_node_id: Option<SceneNodeId>,
    pub start: PresentationRect,
    pub target: PresentationRect,
    pub curve: AnimationCurve,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationOpacityMutation {
    scene_node_id: Option<SceneNodeId>,
    pub start: PresentationOpacity,
    pub target: PresentationOpacity,
    pub curve: AnimationCurve,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationClipMutation {
    scene_node_id: Option<SceneNodeId>,
    pub start: PresentationClip,
    pub target: PresentationClip,
    pub frozen_identity_envelope: Option<PresentationClipRect>,
    pub curve: AnimationCurve,
}

impl PresentationClipMutation {
    pub const fn new(
        scene_node_id: SceneNodeId,
        start: PresentationClip,
        target: PresentationClip,
        frozen_identity_envelope: Option<PresentationClipRect>,
        curve: AnimationCurve,
    ) -> Self {
        Self {
            scene_node_id: Some(scene_node_id),
            start,
            target,
            frozen_identity_envelope,
            curve,
        }
    }

    pub const fn without_owner(
        start: PresentationClip,
        target: PresentationClip,
        frozen_identity_envelope: Option<PresentationClipRect>,
        curve: AnimationCurve,
    ) -> Self {
        Self {
            scene_node_id: None,
            start,
            target,
            frozen_identity_envelope,
            curve,
        }
    }

    pub const fn scene_node_id(self) -> Option<SceneNodeId> {
        self.scene_node_id
    }
}

impl PresentationOpacityMutation {
    pub const fn new(
        scene_node_id: SceneNodeId,
        start: PresentationOpacity,
        target: PresentationOpacity,
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
        start: PresentationOpacity,
        target: PresentationOpacity,
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
    pub opacity: Vec<PresentationOpacityMutation>,
    pub clip: Vec<PresentationClipMutation>,
}

impl PresentationTransactionRequest {
    pub fn geometry(
        started_at: AnimationTime,
        geometry: Vec<PresentationGeometryMutation>,
    ) -> Self {
        Self {
            started_at,
            geometry,
            opacity: Vec::new(),
            clip: Vec::new(),
        }
    }

    pub fn opacity(started_at: AnimationTime, opacity: Vec<PresentationOpacityMutation>) -> Self {
        Self {
            started_at,
            geometry: Vec::new(),
            opacity,
            clip: Vec::new(),
        }
    }

    pub fn clip(started_at: AnimationTime, clip: Vec<PresentationClipMutation>) -> Self {
        Self {
            started_at,
            geometry: Vec::new(),
            opacity: Vec::new(),
            clip,
        }
    }

    pub fn with_clip(mut self, clip: Vec<PresentationClipMutation>) -> Self {
        self.clip = clip;
        self
    }

    pub fn mixed_all(
        started_at: AnimationTime,
        geometry: Vec<PresentationGeometryMutation>,
        opacity: Vec<PresentationOpacityMutation>,
        clip: Vec<PresentationClipMutation>,
    ) -> Self {
        Self {
            started_at,
            geometry,
            opacity,
            clip,
        }
    }

    pub fn mixed(
        started_at: AnimationTime,
        geometry: Vec<PresentationGeometryMutation>,
        opacity: Vec<PresentationOpacityMutation>,
    ) -> Self {
        Self {
            started_at,
            geometry,
            opacity,
            clip: Vec::new(),
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
    InvalidClip,
    MissingClipEnvelope,
    TransactionIdExhausted,
    RevisionIdExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationTransactionMember {
    scene_node_id: SceneNodeId,
    kind: PresentationTransactionMemberKind,
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
            kind: PresentationTransactionMemberKind::Property(property),
            transaction_id,
            revision_id,
        }
    }

    pub(crate) const fn retained_visual(identity: PresentationRetainedVisualIdentity) -> Self {
        Self {
            scene_node_id: identity.scene_node_id(),
            kind: PresentationTransactionMemberKind::RetainedVisual(identity.kind()),
            transaction_id: identity.transaction_id(),
            revision_id: identity.revision_id(),
        }
    }

    pub const fn scene_node_id(self) -> SceneNodeId {
        self.scene_node_id
    }

    pub const fn kind(self) -> PresentationTransactionMemberKind {
        self.kind
    }

    pub const fn property(self) -> Option<PresentationPropertyKind> {
        match self.kind {
            PresentationTransactionMemberKind::Property(property) => Some(property),
            PresentationTransactionMemberKind::RetainedVisual(_) => None,
        }
    }

    pub const fn retained_visual_identity(self) -> Option<PresentationRetainedVisualIdentity> {
        match self.kind {
            PresentationTransactionMemberKind::Property(_) => None,
            PresentationTransactionMemberKind::RetainedVisual(kind) => {
                Some(PresentationRetainedVisualIdentity::new(
                    self.scene_node_id,
                    kind,
                    self.transaction_id,
                    self.revision_id,
                ))
            }
        }
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

    pub(crate) fn remove_member_exact(
        &mut self,
        scene_node_id: SceneNodeId,
        kind: PresentationTransactionMemberKind,
        revision_id: PresentationRevisionId,
    ) -> bool {
        let original_len = self.members.len();
        self.members.retain(|member| {
            !(member.scene_node_id == scene_node_id
                && member.kind == kind
                && member.revision_id == revision_id)
        });
        self.members.len() != original_len
    }

    pub(crate) fn retain_members(
        &mut self,
        mut predicate: impl FnMut(&PresentationTransactionMember) -> bool,
    ) {
        self.members.retain(|member| predicate(member));
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PreparedOpacityMutation {
    pub scene_node_id: SceneNodeId,
    pub start: PresentationOpacity,
    pub target: PresentationOpacity,
    pub start_velocity: f64,
    pub curve: AnimationCurve,
    pub preserve_start_velocity: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PreparedClipMutation {
    pub scene_node_id: SceneNodeId,
    pub start: PresentationClip,
    pub target: PresentationClip,
    pub start_rect: PresentationClipRect,
    pub target_rect: PresentationClipRect,
    pub start_velocity: PresentationVelocity,
    pub curve: AnimationCurve,
    pub preserve_start_velocity: bool,
}
