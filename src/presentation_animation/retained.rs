use crate::core::SceneNodeId;
use std::collections::BTreeMap;

use super::engine::PresentationEngine;
use super::transaction::{PresentationTransactionMember, PresentationTransactionRecord};
use super::{
    AnimationTime, PresentationPropertyKind, PresentationRevisionId, PresentationTransactionError,
    PresentationTransactionId,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationRetainedVisualActivationError {
    MissingTransactionMember,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PresentationRetainedVisualKey {
    scene_node_id: SceneNodeId,
    kind: PresentationRetainedVisualKind,
}

impl From<PresentationRetainedVisualIdentity> for PresentationRetainedVisualKey {
    fn from(identity: PresentationRetainedVisualIdentity) -> Self {
        Self {
            scene_node_id: identity.scene_node_id(),
            kind: identity.kind(),
        }
    }
}

/// Current retained presentation ownership, separate from the transaction
/// ledger that records every allocated member until its exact retirement.
#[derive(Debug, Default)]
pub(crate) struct PresentationRetainedVisualRegistry {
    active: BTreeMap<PresentationRetainedVisualKey, PresentationRetainedVisualIdentity>,
}

impl PresentationRetainedVisualRegistry {
    pub(crate) fn active(
        &self,
        scene_node_id: SceneNodeId,
        kind: PresentationRetainedVisualKind,
    ) -> Option<PresentationRetainedVisualIdentity> {
        self.active
            .get(&PresentationRetainedVisualKey {
                scene_node_id,
                kind,
            })
            .copied()
    }

    pub(crate) fn active_of_kind(
        &self,
        kind: PresentationRetainedVisualKind,
    ) -> Vec<PresentationRetainedVisualIdentity> {
        self.active
            .values()
            .copied()
            .filter(|identity| identity.kind() == kind)
            .collect()
    }

    pub(crate) fn activate(
        &mut self,
        identity: PresentationRetainedVisualIdentity,
    ) -> Option<PresentationRetainedVisualIdentity> {
        self.active.insert(identity.into(), identity)
    }

    pub(crate) fn restore(
        &mut self,
        expected_current: PresentationRetainedVisualIdentity,
        previous: Option<PresentationRetainedVisualIdentity>,
    ) -> bool {
        let key = expected_current.into();
        if self.active.get(&key).copied() != Some(expected_current)
            || previous.is_some_and(|identity| PresentationRetainedVisualKey::from(identity) != key)
        {
            return false;
        }
        if let Some(previous) = previous {
            self.active.insert(key, previous);
        } else {
            self.active.remove(&key);
        }
        true
    }

    pub(crate) fn retire_exact(&mut self, identity: PresentationRetainedVisualIdentity) -> bool {
        let key = PresentationRetainedVisualKey::from(identity);
        if self.active.get(&key).copied() != Some(identity) {
            return false;
        }
        self.active.remove(&key);
        true
    }
}

impl PresentationEngine {
    /// Reserve one retained visual identity in the shared transaction and
    /// revision namespace. The reservation is not active ownership.
    pub fn begin_retained_visual(
        &mut self,
        scene_node_id: SceneNodeId,
        kind: PresentationRetainedVisualKind,
        started_at: AnimationTime,
    ) -> Result<PresentationRetainedVisualIdentity, PresentationTransactionError> {
        let (transaction_id, mut revision_ids) = self.allocate_transaction_and_revisions(1)?;
        let revision_id = revision_ids.pop().expect("one revision was reserved");
        let identity = PresentationRetainedVisualIdentity::new(
            scene_node_id,
            kind,
            transaction_id,
            revision_id,
        );
        let record = PresentationTransactionRecord::new(
            transaction_id,
            started_at,
            vec![PresentationTransactionMember::retained_visual(identity)],
        );
        self.transactions.insert(transaction_id, record);
        Ok(identity)
    }

    /// Return the exact retained visual currently owning this logical scene
    /// node and retained kind. Ledger reservations are not owners.
    pub fn active_retained_visual(
        &self,
        scene_node_id: SceneNodeId,
        kind: PresentationRetainedVisualKind,
    ) -> Option<PresentationRetainedVisualIdentity> {
        self.retained_visuals.active(scene_node_id, kind)
    }

    /// List current owners of one retained visual kind.
    pub fn active_retained_visuals(
        &self,
        kind: PresentationRetainedVisualKind,
    ) -> Vec<PresentationRetainedVisualIdentity> {
        self.retained_visuals.active_of_kind(kind)
    }

    /// Make an exact ledger reservation the current retained owner, returning
    /// the owner it superseded.
    pub fn activate_retained_visual_exact(
        &mut self,
        identity: PresentationRetainedVisualIdentity,
    ) -> Result<Option<PresentationRetainedVisualIdentity>, PresentationRetainedVisualActivationError>
    {
        if !self.has_retained_visual_member(identity) {
            return Err(PresentationRetainedVisualActivationError::MissingTransactionMember);
        }
        Ok(self.retained_visuals.activate(identity))
    }

    /// Restore the previous exact owner after a failed coordinator handoff.
    /// The previous owner must still be present in the transaction ledger.
    pub fn restore_retained_visual_owner_exact(
        &mut self,
        expected_current: PresentationRetainedVisualIdentity,
        previous: Option<PresentationRetainedVisualIdentity>,
    ) -> bool {
        if previous.is_some_and(|identity| !self.has_retained_visual_member(identity)) {
            return false;
        }
        self.retained_visuals.restore(expected_current, previous)
    }

    /// Retire a retained visual only when it is still the exact active owner.
    pub fn retire_active_retained_visual_exact(
        &mut self,
        identity: PresentationRetainedVisualIdentity,
    ) -> bool {
        if self.active_retained_visual(identity.scene_node_id(), identity.kind()) != Some(identity)
        {
            return false;
        }
        self.retire_retained_visual_exact(identity)
    }

    /// Retire one exact retained member without affecting a newer identity
    /// for the same SceneNode.
    pub fn retire_retained_visual_exact(
        &mut self,
        identity: PresentationRetainedVisualIdentity,
    ) -> bool {
        let transaction_id = identity.transaction_id();
        let Some(record) = self.transactions.get_mut(&transaction_id) else {
            return false;
        };
        let removed = record.remove_member_exact(
            identity.scene_node_id(),
            PresentationTransactionMemberKind::RetainedVisual(identity.kind()),
            identity.revision_id(),
        );
        let is_empty = record.members().is_empty();
        if is_empty {
            self.transactions.remove(&transaction_id);
        }
        if removed {
            self.retained_visuals.retire_exact(identity);
        }
        removed
    }

    fn has_retained_visual_member(&self, identity: PresentationRetainedVisualIdentity) -> bool {
        self.transactions
            .get(&identity.transaction_id())
            .is_some_and(|record| {
                record
                    .members()
                    .iter()
                    .any(|member| member.retained_visual_identity() == Some(identity))
            })
    }
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
