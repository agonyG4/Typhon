#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XwaylandSceneBatchToken {
    pub(super) epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XwaylandSceneBatchError {
    AlreadyActive,
    NotActive,
    InvalidToken,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct XwaylandSceneBatchMetrics {
    pub(crate) xwayland_scene_batches: u64,
    pub(crate) xwayland_scene_mutations: u64,
    pub(crate) pointer_refreshes_deferred: u64,
    pub(crate) pointer_refreshes_committed: u64,
    pub(crate) intermediate_pointer_targets_suppressed: u64,
    pub(crate) render_stack_reorders_coalesced: u64,
    pub(crate) client_list_syncs_coalesced: u64,
    pub(crate) override_redirect_stack_snapshots_applied: u64,
    pub(crate) override_redirect_stack_snapshots_rejected_stale: u64,
    pub(crate) override_redirect_stack_snapshots_rejected_generation: u64,
    pub(crate) override_redirect_restack_writebacks_prevented: u64,
    pub(crate) pre_admission_popup_cancellations: u64,
    pub(crate) popup_lifecycle_redundant_cleanup: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct XwaylandSceneMetricsSnapshot {
    pub xwayland_scene_batches: u64,
    pub xwayland_scene_mutations: u64,
    pub pointer_refreshes_deferred: u64,
    pub pointer_refreshes_committed: u64,
    pub intermediate_pointer_targets_suppressed: u64,
    pub render_stack_reorders_coalesced: u64,
    pub client_list_syncs_coalesced: u64,
    pub override_redirect_stack_snapshots_applied: u64,
    pub override_redirect_stack_snapshots_rejected_stale: u64,
    pub override_redirect_stack_snapshots_rejected_generation: u64,
    pub override_redirect_restack_writebacks_prevented: u64,
    pub pre_admission_popup_cancellations: u64,
    pub popup_lifecycle_redundant_cleanup: u64,
}

impl From<XwaylandSceneBatchMetrics> for XwaylandSceneMetricsSnapshot {
    fn from(metrics: XwaylandSceneBatchMetrics) -> Self {
        Self {
            xwayland_scene_batches: metrics.xwayland_scene_batches,
            xwayland_scene_mutations: metrics.xwayland_scene_mutations,
            pointer_refreshes_deferred: metrics.pointer_refreshes_deferred,
            pointer_refreshes_committed: metrics.pointer_refreshes_committed,
            intermediate_pointer_targets_suppressed: metrics
                .intermediate_pointer_targets_suppressed,
            render_stack_reorders_coalesced: metrics.render_stack_reorders_coalesced,
            client_list_syncs_coalesced: metrics.client_list_syncs_coalesced,
            override_redirect_stack_snapshots_applied: metrics
                .override_redirect_stack_snapshots_applied,
            override_redirect_stack_snapshots_rejected_stale: metrics
                .override_redirect_stack_snapshots_rejected_stale,
            override_redirect_stack_snapshots_rejected_generation: metrics
                .override_redirect_stack_snapshots_rejected_generation,
            override_redirect_restack_writebacks_prevented: metrics
                .override_redirect_restack_writebacks_prevented,
            pre_admission_popup_cancellations: metrics.pre_admission_popup_cancellations,
            popup_lifecycle_redundant_cleanup: metrics.popup_lifecycle_redundant_cleanup,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct XwaylandSceneBatchDirty {
    pub(crate) pointer_focus_dirty: bool,
    pub(crate) render_stack_dirty: bool,
    pub(crate) client_lists_dirty: bool,
    pub(crate) repaint_dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingOverrideRedirectStackSnapshot {
    pub(crate) generation: XwaylandGeneration,
    pub(crate) epoch: u64,
    pub(crate) bottom_to_top: Vec<X11WindowHandle>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct XwaylandSceneBatchState {
    active: Option<XwaylandSceneBatchToken>,
    next_epoch: u64,
    pub(crate) dirty: XwaylandSceneBatchDirty,
    pub(crate) pending_override_redirect_stack: Option<PendingOverrideRedirectStackSnapshot>,
    repaint_requested: bool,
    pub(crate) metrics: XwaylandSceneBatchMetrics,
}

use super::*;

impl CompositorState {
    pub(in crate::compositor) fn begin_xwayland_scene_batch(
        &mut self,
    ) -> Result<XwaylandSceneBatchToken, XwaylandSceneBatchError> {
        if self.xwayland_scene_batch.active.is_some() {
            return Err(XwaylandSceneBatchError::AlreadyActive);
        }
        self.xwayland_scene_batch.next_epoch = self
            .xwayland_scene_batch
            .next_epoch
            .saturating_add(1)
            .max(1);
        let token = XwaylandSceneBatchToken {
            epoch: self.xwayland_scene_batch.next_epoch,
        };
        self.xwayland_scene_batch.active = Some(token);
        self.xwayland_scene_batch.metrics.xwayland_scene_batches = self
            .xwayland_scene_batch
            .metrics
            .xwayland_scene_batches
            .saturating_add(1);
        Ok(token)
    }

    pub(in crate::compositor) fn commit_xwayland_scene_batch(
        &mut self,
        token: XwaylandSceneBatchToken,
    ) -> Result<XwaylandSceneBatchDirty, XwaylandSceneBatchError> {
        self.validate_xwayland_scene_batch_token(token)?;
        if let Some(snapshot) = self
            .xwayland_scene_batch
            .pending_override_redirect_stack
            .take()
        {
            let outcome = self.apply_override_redirect_stack_snapshot(
                snapshot.generation,
                snapshot.epoch,
                &snapshot.bottom_to_top,
            );
            if matches!(
                outcome,
                OverrideRedirectStackSnapshotResult::Applied {
                    logical_stack_changed: true,
                }
            ) {
                self.defer_pointer_focus_refresh();
            }
        }
        self.xwayland_scene_batch.active = None;
        let dirty = std::mem::take(&mut self.xwayland_scene_batch.dirty);
        self.xwayland_scene_batch.repaint_requested |= dirty.repaint_dirty;
        Ok(dirty)
    }

    pub(in crate::compositor) fn abort_xwayland_scene_batch(
        &mut self,
        token: XwaylandSceneBatchToken,
    ) -> Result<(), XwaylandSceneBatchError> {
        self.validate_xwayland_scene_batch_token(token)?;
        self.xwayland_scene_batch.active = None;
        Ok(())
    }

    fn validate_xwayland_scene_batch_token(
        &self,
        token: XwaylandSceneBatchToken,
    ) -> Result<(), XwaylandSceneBatchError> {
        let Some(active) = self.xwayland_scene_batch.active else {
            return Err(XwaylandSceneBatchError::NotActive);
        };
        (active == token)
            .then_some(())
            .ok_or(XwaylandSceneBatchError::InvalidToken)
    }

    pub(in crate::compositor) fn xwayland_scene_batch_active(&self) -> bool {
        self.xwayland_scene_batch.active.is_some()
    }

    pub(in crate::compositor) fn take_xwayland_scene_repaint_request(&mut self) -> bool {
        std::mem::take(&mut self.xwayland_scene_batch.repaint_requested)
    }

    pub(in crate::compositor) fn xwayland_scene_metrics(&self) -> XwaylandSceneMetricsSnapshot {
        self.xwayland_scene_batch.metrics.into()
    }

    pub(in crate::compositor) fn stage_override_redirect_stack_snapshot(
        &mut self,
        generation: XwaylandGeneration,
        epoch: u64,
        bottom_to_top: Vec<X11WindowHandle>,
    ) -> bool {
        if !self.validate_override_redirect_stack_snapshot_header(generation, epoch, &bottom_to_top)
        {
            return false;
        }
        if self
            .xwayland_scene_batch
            .pending_override_redirect_stack
            .as_ref()
            .is_some_and(|pending| pending.epoch >= epoch)
        {
            self.note_override_redirect_snapshot_rejected_stale();
            return false;
        }
        self.xwayland_scene_batch.pending_override_redirect_stack =
            Some(PendingOverrideRedirectStackSnapshot {
                generation,
                epoch,
                bottom_to_top,
            });
        true
    }

    pub(in crate::compositor) fn clear_xwayland_scene_snapshot_for_generation(
        &mut self,
        generation: XwaylandGeneration,
    ) {
        if self
            .xwayland_scene_batch
            .pending_override_redirect_stack
            .as_ref()
            .is_some_and(|pending| pending.generation == generation)
        {
            self.xwayland_scene_batch.pending_override_redirect_stack = None;
        }
    }

    pub(in crate::compositor) fn mark_xwayland_scene_repaint(&mut self) {
        if self.xwayland_scene_batch_active() {
            self.xwayland_scene_batch.dirty.repaint_dirty = true;
        } else {
            self.xwayland_scene_batch.repaint_requested = true;
        }
    }

    pub(in crate::compositor) fn note_xwayland_scene_mutation(&mut self) {
        if !self.xwayland_scene_batch_active() {
            return;
        }
        self.xwayland_scene_batch.metrics.xwayland_scene_mutations = self
            .xwayland_scene_batch
            .metrics
            .xwayland_scene_mutations
            .saturating_add(1);
    }

    pub(in crate::compositor) fn defer_pointer_focus_refresh(&mut self) -> bool {
        if !self.xwayland_scene_batch_active() {
            return false;
        }
        self.xwayland_scene_batch.dirty.pointer_focus_dirty = true;
        self.xwayland_scene_batch.metrics.pointer_refreshes_deferred = self
            .xwayland_scene_batch
            .metrics
            .pointer_refreshes_deferred
            .saturating_add(1);
        self.xwayland_scene_batch
            .metrics
            .intermediate_pointer_targets_suppressed = self
            .xwayland_scene_batch
            .metrics
            .intermediate_pointer_targets_suppressed
            .saturating_add(1);
        true
    }

    pub(in crate::compositor) fn defer_render_stack_reorder(&mut self) -> bool {
        if !self.xwayland_scene_batch_active() {
            return false;
        }
        if self.xwayland_scene_batch.dirty.render_stack_dirty {
            self.xwayland_scene_batch
                .metrics
                .render_stack_reorders_coalesced = self
                .xwayland_scene_batch
                .metrics
                .render_stack_reorders_coalesced
                .saturating_add(1);
        }
        self.xwayland_scene_batch.dirty.render_stack_dirty = true;
        true
    }

    pub(in crate::compositor) fn defer_client_list_sync(&mut self) -> bool {
        if !self.xwayland_scene_batch_active() {
            return false;
        }
        if self.xwayland_scene_batch.dirty.client_lists_dirty {
            self.xwayland_scene_batch
                .metrics
                .client_list_syncs_coalesced = self
                .xwayland_scene_batch
                .metrics
                .client_list_syncs_coalesced
                .saturating_add(1);
        }
        self.xwayland_scene_batch.dirty.client_lists_dirty = true;
        true
    }

    pub(in crate::compositor) fn note_committed_pointer_refresh(&mut self) {
        self.xwayland_scene_batch
            .metrics
            .pointer_refreshes_committed = self
            .xwayland_scene_batch
            .metrics
            .pointer_refreshes_committed
            .saturating_add(1);
    }

    pub(in crate::compositor) fn note_override_redirect_restack_writeback_prevented(&mut self) {
        self.xwayland_scene_batch
            .metrics
            .override_redirect_restack_writebacks_prevented = self
            .xwayland_scene_batch
            .metrics
            .override_redirect_restack_writebacks_prevented
            .saturating_add(1);
    }

    pub(in crate::compositor) fn note_override_redirect_snapshot_rejected_stale(&mut self) {
        self.xwayland_scene_batch
            .metrics
            .override_redirect_stack_snapshots_rejected_stale = self
            .xwayland_scene_batch
            .metrics
            .override_redirect_stack_snapshots_rejected_stale
            .saturating_add(1);
    }

    pub(in crate::compositor) fn note_override_redirect_snapshot_rejected_generation(&mut self) {
        self.xwayland_scene_batch
            .metrics
            .override_redirect_stack_snapshots_rejected_generation = self
            .xwayland_scene_batch
            .metrics
            .override_redirect_stack_snapshots_rejected_generation
            .saturating_add(1);
    }

    pub(in crate::compositor) fn note_pre_admission_popup_cancellation(&mut self) {
        self.xwayland_scene_batch
            .metrics
            .pre_admission_popup_cancellations = self
            .xwayland_scene_batch
            .metrics
            .pre_admission_popup_cancellations
            .saturating_add(1);
    }

    pub(in crate::compositor) fn note_popup_lifecycle_redundant_cleanup(&mut self) {
        self.xwayland_scene_batch
            .metrics
            .popup_lifecycle_redundant_cleanup = self
            .xwayland_scene_batch
            .metrics
            .popup_lifecycle_redundant_cleanup
            .saturating_add(1);
    }

    #[cfg(test)]
    pub(in crate::compositor) fn xwayland_scene_batch_dirty_for_test(&self) -> bool {
        let dirty = self.xwayland_scene_batch.dirty;
        dirty.pointer_focus_dirty
            || dirty.render_stack_dirty
            || dirty.client_lists_dirty
            || dirty.repaint_dirty
    }
}
