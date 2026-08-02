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
        self.xwayland_scene_batch.active = None;
        Ok(std::mem::take(&mut self.xwayland_scene_batch.dirty))
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

    pub(in crate::compositor) fn note_xwayland_scene_mutation(&mut self) {
        if !self.xwayland_scene_batch_active() {
            return;
        }
        self.xwayland_scene_batch.metrics.xwayland_scene_mutations = self
            .xwayland_scene_batch
            .metrics
            .xwayland_scene_mutations
            .saturating_add(1);
        self.xwayland_scene_batch.dirty.repaint_dirty = true;
    }

    pub(in crate::compositor) fn defer_pointer_focus_refresh(&mut self) -> bool {
        if !self.xwayland_scene_batch_active() {
            return false;
        }
        self.xwayland_scene_batch.dirty.pointer_focus_dirty = true;
        self.xwayland_scene_batch.dirty.repaint_dirty = true;
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
        self.xwayland_scene_batch.dirty.repaint_dirty = true;
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

    #[cfg(test)]
    pub(in crate::compositor) fn xwayland_scene_batch_dirty_for_test(&self) -> bool {
        let dirty = self.xwayland_scene_batch.dirty;
        dirty.pointer_focus_dirty
            || dirty.render_stack_dirty
            || dirty.client_lists_dirty
            || dirty.repaint_dirty
    }

    #[cfg(test)]
    pub(in crate::compositor) fn xwayland_scene_batch_metrics_for_test(
        &self,
    ) -> XwaylandSceneBatchMetrics {
        self.xwayland_scene_batch.metrics
    }
}
