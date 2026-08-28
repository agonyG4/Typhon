use std::collections::HashMap;

use wayland_server::Resource;

use crate::wm::WorkspaceLocation;

use super::{
    ActiveFifoBarrier, ActiveSceneSelection, CompositorState, PendingAcquireState, SceneWorkOwner,
    advance_nonzero_serial,
};

const EMPTY_COMMIT_TIMING_PLANNING_SIGNATURE: u64 = 0xcbf2_9ce4_8422_2325;

#[derive(Debug, Default)]
pub(in crate::compositor) struct SceneWorkIndex {
    prepare_by_owner: HashMap<SceneWorkOwner, usize>,
    callbacks_by_owner: HashMap<SceneWorkOwner, usize>,
    feedback_by_owner: HashMap<SceneWorkOwner, usize>,
    unowned_callbacks_by_owner: HashMap<SceneWorkOwner, usize>,
}

impl SceneWorkIndex {
    pub(in crate::compositor) fn clear(&mut self) {
        self.prepare_by_owner.clear();
        self.callbacks_by_owner.clear();
        self.feedback_by_owner.clear();
        self.unowned_callbacks_by_owner.clear();
    }

    pub(in crate::compositor) fn add_prepare_work(&mut self, owner: SceneWorkOwner) {
        *self.prepare_by_owner.entry(owner).or_default() += 1;
    }

    pub(in crate::compositor) fn add_callback(&mut self, owner: SceneWorkOwner) {
        *self.callbacks_by_owner.entry(owner).or_default() += 1;
    }

    pub(in crate::compositor) fn add_feedback(&mut self, owner: SceneWorkOwner) {
        *self.feedback_by_owner.entry(owner).or_default() += 1;
    }

    pub(in crate::compositor) fn add_unowned_callback(&mut self, owner: SceneWorkOwner) {
        *self.unowned_callbacks_by_owner.entry(owner).or_default() += 1;
    }

    pub(in crate::compositor) fn has_visible_unowned_callbacks(
        &self,
        selection: ActiveSceneSelection,
    ) -> bool {
        self.has_visible_owner(&self.unowned_callbacks_by_owner, selection)
    }

    fn has_visible_owner(
        &self,
        owners: &HashMap<SceneWorkOwner, usize>,
        selection: ActiveSceneSelection,
    ) -> bool {
        owners.iter().any(|(owner, count)| {
            *count > 0
                && match owner {
                    SceneWorkOwner::Global => true,
                    SceneWorkOwner::Location(crate::wm::WorkspaceLocation::Regular(workspace)) => {
                        *workspace == selection.regular
                    }
                    SceneWorkOwner::Location(crate::wm::WorkspaceLocation::Special(special)) => {
                        selection.special == Some(*special)
                    }
                }
        })
    }

    #[cfg(test)]
    pub(in crate::compositor) fn prepare_count(&self, owner: SceneWorkOwner) -> usize {
        self.prepare_by_owner
            .get(&owner)
            .copied()
            .unwrap_or_default()
    }

    pub(in crate::compositor) fn has_visible_prepare_work(
        &self,
        selection: ActiveSceneSelection,
    ) -> bool {
        self.prepare_by_owner.iter().any(|(owner, count)| {
            *count > 0
                && match owner {
                    SceneWorkOwner::Global => true,
                    SceneWorkOwner::Location(WorkspaceLocation::Regular(workspace)) => {
                        *workspace == selection.regular
                    }
                    SceneWorkOwner::Location(WorkspaceLocation::Special(special)) => {
                        selection.special == Some(*special)
                    }
                }
        })
    }
}

impl CompositorState {
    pub(in crate::compositor) fn has_pending_commit_timing_planning(&self) -> bool {
        self.commit_timing_planning_pending
    }

    fn fifo_barrier_frame_owned(&self, surface_id: u32, barrier: ActiveFifoBarrier) -> bool {
        self.frame_batches.values().any(|batch| {
            batch.fifo_barrier_claims.iter().any(|claim| {
                claim.surface_id == surface_id
                    && claim.surface_generation == barrier.surface_generation
                    && claim.fifo_barrier_generation == barrier.fifo_barrier_generation
                    && claim.commit_sequence == barrier.commit_sequence
            })
        })
    }

    pub(in crate::compositor) fn rebuild_scene_work_index(&mut self) {
        let mut index = std::mem::take(&mut self.scene_work_index);
        index.clear();
        for (surface_id, barrier) in self.active_fifo_barriers.iter() {
            if !self.fifo_barrier_frame_owned(*surface_id, *barrier) {
                index.add_prepare_work(self.scene_work_owner_for_surface(*surface_id));
            }
        }
        for commit in &self.pending_explicit_sync_commits {
            let owner = self.scene_work_owner_for_surface(commit.surface_id);
            if !self.external_acquire_readiness
                || commit.acquire_state == PendingAcquireState::Ready
            {
                index.add_prepare_work(owner);
            }
            if !self.external_acquire_readiness && !commit.frame_callbacks.is_empty() {
                index.add_unowned_callback(owner);
            }
        }
        for transaction in &self.pending_surface_tree_transactions {
            let owner = self.scene_work_owner_for_surface(transaction.root_surface_id);
            let locally_signaled_acquire = !self.external_acquire_readiness
                && transaction
                    .dependencies
                    .iter()
                    .any(|dependency| dependency.acquire.is_signaled());
            if self.transaction_is_ready(transaction) || locally_signaled_acquire {
                index.add_prepare_work(owner);
            }
            for (surface_id, commit) in &transaction.nodes {
                if !commit.frame_callbacks.is_empty() && !self.external_acquire_readiness {
                    index.add_unowned_callback(self.scene_work_owner_for_surface(*surface_id));
                }
            }
        }
        for callback in self
            .pending_frame_callbacks
            .iter()
            .chain(self.visible_pending_frame_callbacks.iter())
        {
            if let Some(surface_id) = self.pending_frame_callback_surfaces.get(&callback.id()) {
                index.add_callback(self.scene_work_owner_for_surface(*surface_id));
            }
        }
        for feedback in self
            .pending_presentation_feedbacks
            .iter()
            .chain(self.visible_pending_presentation_feedbacks.iter())
        {
            index.add_feedback(self.scene_work_owner_for_surface(feedback.surface_id));
        }
        let mut commit_timing_planning_signature = EMPTY_COMMIT_TIMING_PLANNING_SIGNATURE;
        let mut commit_timing_planning_pending = false;
        for (index, transaction) in self.pending_surface_tree_transactions.iter().enumerate() {
            let Some(requested) = transaction.commit_timing_request() else {
                continue;
            };
            if transaction.commit_timing_readiness.is_some()
                || self.pending_surface_tree_transactions[..index]
                    .iter()
                    .any(|previous| previous.root_surface_id == transaction.root_surface_id)
            {
                continue;
            }
            commit_timing_planning_pending = true;
            commit_timing_planning_signature = commit_timing_planning_signature.rotate_left(7)
                ^ transaction.id.get().wrapping_mul(0x9e37_79b9_7f4a_7c15)
                ^ u64::from(transaction.root_surface_id);
            commit_timing_planning_signature = commit_timing_planning_signature
                .wrapping_mul(0x1000_0000_01b3)
                ^ requested.seconds
                ^ u64::from(requested.nanoseconds);
        }
        self.commit_timing_planning_pending = commit_timing_planning_pending;
        if commit_timing_planning_signature != self.commit_timing_planning_signature {
            self.commit_timing_planning_generation =
                advance_nonzero_serial(self.commit_timing_planning_generation);
            self.commit_timing_planning_signature = commit_timing_planning_signature;
        }
        self.scene_work_index = index;
    }

    #[cfg(test)]
    pub(in crate::compositor) fn scene_work_prepare_count(
        &self,
        location: WorkspaceLocation,
    ) -> usize {
        self.scene_work_index
            .prepare_count(SceneWorkOwner::Location(location))
    }
}
