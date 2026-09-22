use crate::core::SceneNodeId;

use super::{PresentationPropertyKind, PresentationRevisionId, PresentationTransactionId};

/// A retained visual participates in a presentation transaction without
/// becoming an interpolated property track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PresentationRetainedVisualKind {
    WindowLifecycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PresentationTransactionMemberKind {
    Property(PresentationPropertyKind),
    RetainedVisual(PresentationRetainedVisualKind),
}

/// Exact identity evidence for one retained visual member in the Presentation
/// transaction ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PresentationRetainedVisualIdentity {
    scene_node_id: SceneNodeId,
    kind: PresentationRetainedVisualKind,
    transaction_id: PresentationTransactionId,
    revision_id: PresentationRevisionId,
}

impl PresentationRetainedVisualIdentity {
    pub(crate) const fn new(
        scene_node_id: SceneNodeId,
        kind: PresentationRetainedVisualKind,
        transaction_id: PresentationTransactionId,
        revision_id: PresentationRevisionId,
    ) -> Self {
        Self {
            scene_node_id,
            kind,
            transaction_id,
            revision_id,
        }
    }

    pub const fn scene_node_id(self) -> SceneNodeId {
        self.scene_node_id
    }

    pub const fn kind(self) -> PresentationRetainedVisualKind {
        self.kind
    }

    pub const fn transaction_id(self) -> PresentationTransactionId {
        self.transaction_id
    }

    pub const fn revision_id(self) -> PresentationRevisionId {
        self.revision_id
    }
}
