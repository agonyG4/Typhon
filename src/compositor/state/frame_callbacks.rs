use super::*;
use crate::compositor::frame_batch::FrameCallbackPacingState;

impl CompositorState {
    pub(in crate::compositor) fn queue_frame_callbacks_for_surface(
        &mut self,
        surface_id: u32,
        callbacks: Vec<wl_callback::WlCallback>,
    ) {
        for callback in callbacks {
            self.pending_frame_callback_surfaces
                .insert(callback.id(), surface_id);
            if self.surface_is_visible_in_active_scene(surface_id) {
                self.visible_pending_frame_callbacks.push(callback);
                self.visible_pending_frame_callback_count =
                    self.visible_pending_frame_callback_count.saturating_add(1);
            } else {
                self.pending_frame_callbacks.push(callback);
            }
        }
    }

    pub(in crate::compositor) fn discard_frame_callbacks_for_surface(&mut self, surface_id: u32) {
        let callback_ids = self
            .pending_frame_callback_surfaces
            .iter()
            .filter_map(|(callback_id, owner_surface_id)| {
                (*owner_surface_id == surface_id).then_some(callback_id.clone())
            })
            .collect::<Vec<_>>();
        if callback_ids.is_empty() {
            return;
        }

        let visible_discarded = self
            .visible_pending_frame_callbacks
            .iter()
            .filter(|callback| callback_ids.contains(&callback.id()))
            .count();
        self.visible_pending_frame_callbacks
            .retain(|callback| !callback_ids.contains(&callback.id()));
        self.pending_frame_callbacks
            .retain(|callback| !callback_ids.contains(&callback.id()));
        self.visible_pending_frame_callback_count = self
            .visible_pending_frame_callback_count
            .saturating_sub(visible_discarded);
        for callback_id in &callback_ids {
            self.pending_frame_callback_surfaces.remove(callback_id);
        }
        for batch in self.frame_batches.values_mut() {
            if batch.callback_terminal_ownership_checked {
                continue;
            }
            let before = batch.callbacks.len();
            batch
                .callbacks
                .retain(|callback| !callback_ids.contains(&callback.id()));
            let discarded = before.saturating_sub(batch.callbacks.len());
            if discarded > 0 {
                batch.callback_settlement.cancel(discarded);
            }
        }
    }

    pub(in crate::compositor) fn take_visible_pending_frame_callbacks(
        &mut self,
    ) -> Vec<wl_callback::WlCallback> {
        self.visible_pending_frame_callback_count = 0;
        std::mem::take(&mut self.visible_pending_frame_callbacks)
    }

    pub(in crate::compositor) fn refresh_frame_work_visibility(&mut self) {
        let callbacks = std::mem::take(&mut self.pending_frame_callbacks);
        let visible_callbacks = std::mem::take(&mut self.visible_pending_frame_callbacks);
        self.pending_frame_callbacks.clear();
        self.visible_pending_frame_callbacks.clear();
        for callback in visible_callbacks.into_iter().chain(callbacks) {
            let surface_id = self
                .pending_frame_callback_surfaces
                .get(&callback.id())
                .copied();
            if surface_id
                .is_some_and(|surface_id| self.surface_is_visible_in_active_scene(surface_id))
            {
                self.visible_pending_frame_callbacks.push(callback);
            } else {
                self.pending_frame_callbacks.push(callback);
            }
        }
        self.visible_pending_frame_callback_count = self.visible_pending_frame_callbacks.len();

        let feedbacks = std::mem::take(&mut self.pending_presentation_feedbacks);
        let visible_feedbacks = std::mem::take(&mut self.visible_pending_presentation_feedbacks);
        self.pending_presentation_feedbacks.clear();
        self.visible_pending_presentation_feedbacks.clear();
        for feedback in visible_feedbacks.into_iter().chain(feedbacks) {
            if self.pending_presentation_feedback_is_visible(&feedback) {
                self.visible_pending_presentation_feedbacks.push(feedback);
            } else {
                self.pending_presentation_feedbacks.push(feedback);
            }
        }
        self.visible_pending_presentation_feedback_count =
            self.visible_pending_presentation_feedbacks.len();
    }

    pub(in crate::compositor) fn queue_pending_presentation_feedbacks(
        &mut self,
        feedbacks: Vec<PendingPresentationFeedback>,
    ) {
        for feedback in feedbacks {
            if self.pending_presentation_feedback_is_visible(&feedback) {
                self.visible_pending_presentation_feedbacks.push(feedback);
                self.visible_pending_presentation_feedback_count = self
                    .visible_pending_presentation_feedback_count
                    .saturating_add(1);
            } else {
                self.pending_presentation_feedbacks.push(feedback);
            }
        }
    }

    pub(in crate::compositor) fn capture_frame_callbacks_for_render(&mut self) {
        if self.legacy_prepared_frame_batch.is_some() {
            return;
        }
        self.next_legacy_output_frame_id = self
            .next_legacy_output_frame_id
            .checked_add(1)
            .expect("legacy output frame ID overflow");
        let frame_id = self.next_legacy_output_frame_id;
        self.legacy_prepared_frame_batch = Some(self.take_frame_batch_for_render(frame_id));
    }

    pub(in crate::compositor) fn has_pending_frame_callbacks(&self) -> bool {
        self.visible_pending_frame_callback_count > 0
            || self
                .frame_batches
                .values()
                .any(|batch| !batch.callbacks.is_empty())
            || self
                .scene_work_index
                .has_visible_unowned_callbacks(self.active_scene_selection())
    }

    pub(in crate::compositor) fn has_only_pending_surface_frame_callbacks(&self) -> bool {
        if self.visible_pending_frame_callback_count == 0 {
            return false;
        }
        !self.pending_resize_configure_is_flushable()
            && !self.has_pending_frame_prepare_work()
            && self.pending_color_info.is_empty()
            && !self.has_visible_pending_presentation_feedbacks()
    }

    fn pending_presentation_feedback_is_visible(
        &self,
        feedback: &PendingPresentationFeedback,
    ) -> bool {
        self.surface_is_visible_in_active_scene(feedback.surface_id)
    }

    pub(in crate::compositor) fn take_visible_pending_presentation_feedbacks(
        &mut self,
    ) -> Vec<PendingPresentationFeedback> {
        self.visible_pending_presentation_feedback_count = 0;
        std::mem::take(&mut self.visible_pending_presentation_feedbacks)
    }

    pub(in crate::compositor) fn has_visible_pending_presentation_feedbacks(&self) -> bool {
        self.visible_pending_presentation_feedback_count > 0
    }

    pub(in crate::compositor) fn has_unowned_frame_callbacks(&self) -> bool {
        self.visible_pending_frame_callback_count > 0
            || self
                .scene_work_index
                .has_visible_unowned_callbacks(self.active_scene_selection())
    }

    pub(in crate::compositor) fn requeue_frame_callbacks_after_restore(
        &mut self,
        callbacks: Vec<wl_callback::WlCallback>,
    ) {
        for callback in callbacks {
            let visible = self
                .pending_frame_callback_surfaces
                .get(&callback.id())
                .copied()
                .is_some_and(|surface_id| self.surface_is_visible_in_active_scene(surface_id));
            if visible {
                self.visible_pending_frame_callbacks.push(callback);
                self.visible_pending_frame_callback_count =
                    self.visible_pending_frame_callback_count.saturating_add(1);
            } else {
                self.pending_frame_callbacks.push(callback);
            }
        }
    }

    pub(in crate::compositor) fn requeue_presentation_feedbacks_after_restore(
        &mut self,
        feedbacks: Vec<PendingPresentationFeedback>,
    ) {
        for feedback in feedbacks {
            if self.pending_presentation_feedback_is_visible(&feedback) {
                self.visible_pending_presentation_feedbacks.push(feedback);
                self.visible_pending_presentation_feedback_count = self
                    .visible_pending_presentation_feedback_count
                    .saturating_add(1);
            } else {
                self.pending_presentation_feedbacks.push(feedback);
            }
        }
    }

    pub(crate) fn prepare_terminal_callback_ownership(
        &mut self,
        batch_id: CompositorFrameBatchId,
        disposition: TerminalCallbackDisposition,
    ) -> TerminalCallbackOwnership {
        self.settle_terminal_callback_ownership(batch_id, disposition)
    }

    fn settle_terminal_callback_ownership(
        &mut self,
        batch_id: CompositorFrameBatchId,
        disposition: TerminalCallbackDisposition,
    ) -> TerminalCallbackOwnership {
        let Some(batch) = self.frame_batches.get_mut(&batch_id) else {
            return TerminalCallbackOwnership::Leaked {
                owner: batch_id,
                unresolved: 0,
                reason: TerminalCallbackLeakReason::MissingBatch,
            };
        };
        if batch.callback_terminal_ownership_checked {
            return TerminalCallbackOwnership::None;
        }
        batch.callback_terminal_ownership_checked = true;
        let live = batch
            .callbacks
            .iter()
            .filter(|callback| callback.is_alive())
            .count();
        let cancelled = batch.callbacks.len().saturating_sub(live);
        let settlement = &mut batch.callback_settlement;
        if settlement.count_mismatch || !settlement.is_reconciled() {
            return TerminalCallbackOwnership::Leaked {
                owner: batch_id,
                unresolved: settlement.unresolved,
                reason: TerminalCallbackLeakReason::CountMismatch,
            };
        }
        if settlement.originally_owned == 0 {
            return TerminalCallbackOwnership::None;
        }
        if settlement.unresolved == 0 {
            return if settlement.completed() == 0 && settlement.cancelled > 0 {
                TerminalCallbackOwnership::Cancelled {
                    callbacks: settlement.cancelled,
                }
            } else {
                TerminalCallbackOwnership::Resolved {
                    completed: settlement.completed(),
                }
            };
        }
        match disposition {
            TerminalCallbackDisposition::Presented => {
                settlement.complete_at_presentation_fallback(live);
                settlement.cancel(cancelled);
                if settlement.count_mismatch || !settlement.is_reconciled() {
                    return TerminalCallbackOwnership::Leaked {
                        owner: batch_id,
                        unresolved: settlement.unresolved,
                        reason: TerminalCallbackLeakReason::CountMismatch,
                    };
                }
                TerminalCallbackOwnership::Resolved {
                    completed: settlement.completed(),
                }
            }
            TerminalCallbackDisposition::NoVisualChange => {
                settlement.complete_without_visual(live);
                settlement.cancel(cancelled);
                if settlement.count_mismatch || !settlement.is_reconciled() {
                    return TerminalCallbackOwnership::Leaked {
                        owner: batch_id,
                        unresolved: settlement.unresolved,
                        reason: TerminalCallbackLeakReason::CountMismatch,
                    };
                }
                TerminalCallbackOwnership::Resolved {
                    completed: settlement.completed(),
                }
            }
            TerminalCallbackDisposition::Retryable | TerminalCallbackDisposition::Superseded => {
                settlement.transfer(live);
                settlement.cancel(cancelled);
                if settlement.count_mismatch || !settlement.is_reconciled() {
                    return TerminalCallbackOwnership::Leaked {
                        owner: batch_id,
                        unresolved: settlement.unresolved,
                        reason: TerminalCallbackLeakReason::CountMismatch,
                    };
                }
                if live == 0 {
                    TerminalCallbackOwnership::Cancelled {
                        callbacks: cancelled,
                    }
                } else {
                    TerminalCallbackOwnership::Transferred {
                        owner: batch_id,
                        callbacks: live,
                    }
                }
            }
            TerminalCallbackDisposition::Cancelled => {
                let count = live.saturating_add(cancelled);
                settlement.cancel(count);
                if settlement.count_mismatch || !settlement.is_reconciled() {
                    return TerminalCallbackOwnership::Leaked {
                        owner: batch_id,
                        unresolved: settlement.unresolved,
                        reason: TerminalCallbackLeakReason::CountMismatch,
                    };
                }
                TerminalCallbackOwnership::Cancelled { callbacks: count }
            }
        }
    }

    pub(crate) fn mark_frame_callbacks_rendered(&mut self, batch_id: CompositorFrameBatchId) {
        let rendered_ns = client_pacing_now_ns();
        let (callback_count, callback_commit_ns) = {
            let batch = self
                .frame_batches
                .get_mut(&batch_id)
                .expect("missing compositor frame batch at render completion");
            if batch.callback_pacing_state != FrameCallbackPacingState::Captured {
                return;
            }
            if batch.callbacks.is_empty() {
                batch.callback_pacing_state = FrameCallbackPacingState::RenderedAwaitingAdmission;
                return;
            }
            batch.callback_pacing_state = FrameCallbackPacingState::RenderedAwaitingAdmission;
            batch.callback_render_completed_ns = Some(rendered_ns);
            (batch.callbacks.len(), batch.callback_commit_ns)
        };
        self.frame_callback_metrics.callbacks_marked_rendered = self
            .frame_callback_metrics
            .callbacks_marked_rendered
            .saturating_add(callback_count as u64);
        self.frame_callback_metrics
            .last_callback_render_completed_ns = Some(rendered_ns);
        self.frame_callback_metrics
            .last_callback_commit_to_render_ns = callback_commit_ns
            .filter(|commit_ns| rendered_ns >= *commit_ns)
            .map(|commit_ns| rendered_ns.saturating_sub(commit_ns));
        client_pacing_log(
            "frame_callbacks_rendered",
            &[
                ("frame_batch_id", batch_id.get().to_string()),
                ("count", callback_count.to_string()),
                ("render_completed_ns", rendered_ns.to_string()),
            ],
        );
    }

    pub(crate) fn note_frame_callback_admission_failure(
        &mut self,
        batch_id: CompositorFrameBatchId,
    ) {
        let Some(batch) = self.frame_batches.get(&batch_id) else {
            return;
        };
        if batch.callback_pacing_state != FrameCallbackPacingState::RenderedAwaitingAdmission
            || batch.callbacks.is_empty()
        {
            return;
        }
        self.frame_callback_metrics
            .callbacks_retained_after_failed_admission = self
            .frame_callback_metrics
            .callbacks_retained_after_failed_admission
            .saturating_add(batch.callbacks.len() as u64);
        client_pacing_log(
            "frame_callbacks_admission_failed",
            &[
                ("frame_batch_id", batch_id.get().to_string()),
                ("callbacks_retained", batch.callbacks.len().to_string()),
            ],
        );
    }

    pub(crate) fn note_frame_callbacks_deferred_ready(&mut self, batch_id: CompositorFrameBatchId) {
        let Some(batch) = self.frame_batches.get(&batch_id) else {
            return;
        };
        if batch.callback_pacing_state != FrameCallbackPacingState::RenderedAwaitingAdmission
            || batch.callbacks.is_empty()
        {
            return;
        }
        self.frame_callback_metrics.callbacks_deferred_ready = self
            .frame_callback_metrics
            .callbacks_deferred_ready
            .saturating_add(batch.callbacks.len() as u64);
        client_pacing_log(
            "frame_callbacks_deferred_ready",
            &[
                ("frame_batch_id", batch_id.get().to_string()),
                ("callbacks_deferred", batch.callbacks.len().to_string()),
            ],
        );
    }

    pub(crate) fn complete_frame_callbacks_after_admission(
        &mut self,
        batch_id: CompositorFrameBatchId,
        admission: FrameCallbackAdmission,
    ) {
        self.complete_frame_callbacks_after_admission_internal(batch_id, admission, true);
    }

    pub(crate) fn complete_direct_frame_callbacks_after_admission(
        &mut self,
        batch_id: CompositorFrameBatchId,
    ) {
        self.complete_frame_callbacks_after_admission_internal(
            batch_id,
            FrameCallbackAdmission::Direct,
            false,
        );
    }

    fn complete_frame_callbacks_after_admission_internal(
        &mut self,
        batch_id: CompositorFrameBatchId,
        admission: FrameCallbackAdmission,
        require_rendered: bool,
    ) {
        let admission_ns = client_pacing_now_ns();
        let (callbacks, callback_commit_ns, callback_render_completed_ns) = {
            let batch = self
                .frame_batches
                .get_mut(&batch_id)
                .expect("missing compositor frame batch at output admission");
            if batch.callbacks.is_empty()
                || batch.callback_pacing_state == FrameCallbackPacingState::Completed
            {
                return;
            }
            if require_rendered {
                assert_eq!(
                    batch.callback_pacing_state,
                    FrameCallbackPacingState::RenderedAwaitingAdmission,
                    "callbacks cannot complete before successful rendering"
                );
            }
            let callbacks = std::mem::take(&mut batch.callbacks);
            let live = callbacks
                .iter()
                .filter(|callback| callback.is_alive())
                .count();
            let cancelled = callbacks.len().saturating_sub(live);
            batch.callback_settlement.complete_after_admission(live);
            batch.callback_settlement.cancel(cancelled);
            debug_assert!(
                !batch.callback_settlement.count_mismatch
                    && batch.callback_settlement.is_reconciled(),
                "output admission must reconcile every callback terminal"
            );
            batch.callback_admission_ns = Some(admission_ns);
            batch.callback_pacing_state = FrameCallbackPacingState::Completed;
            batch.callback_terminal_ownership_checked = true;
            (
                callbacks,
                batch.callback_commit_ns,
                batch.callback_render_completed_ns,
            )
        };
        if callbacks.is_empty() {
            return;
        }
        self.record_callback_admission_metrics(
            batch_id,
            admission,
            callbacks.len(),
            callback_commit_ns,
            callback_render_completed_ns,
            admission_ns,
        );
        self.complete_frame_callbacks(callbacks);
    }

    pub(crate) fn complete_frame_callbacks_at_presentation_fallback(
        &mut self,
        batch_id: CompositorFrameBatchId,
    ) {
        let (callbacks, callback_render_completed_ns) = {
            let batch = self
                .frame_batches
                .get_mut(&batch_id)
                .expect("missing compositor frame batch at presentation fallback");
            if batch.callbacks.is_empty()
                || batch.callback_pacing_state == FrameCallbackPacingState::Completed
            {
                return;
            }
            let callbacks = std::mem::take(&mut batch.callbacks);
            let live = callbacks
                .iter()
                .filter(|callback| callback.is_alive())
                .count();
            let cancelled = callbacks.len().saturating_sub(live);
            if batch.callback_settlement.unresolved > 0 {
                batch
                    .callback_settlement
                    .complete_at_presentation_fallback(live);
                batch.callback_settlement.cancel(cancelled);
            }
            batch.callback_pacing_state = FrameCallbackPacingState::Completed;
            batch.callback_terminal_ownership_checked = true;
            (callbacks, batch.callback_render_completed_ns)
        };
        if callbacks.is_empty() {
            return;
        }
        self.frame_callback_metrics
            .callbacks_completed_at_presentation_fallback = self
            .frame_callback_metrics
            .callbacks_completed_at_presentation_fallback
            .saturating_add(callbacks.len() as u64);
        if let Some(render_completed_ns) = callback_render_completed_ns {
            let fallback_ns = client_pacing_now_ns();
            self.frame_callback_metrics
                .last_callback_render_to_admission_ns =
                Some(fallback_ns.saturating_sub(render_completed_ns));
        }
        self.complete_frame_callbacks(callbacks);
    }

    fn record_callback_admission_metrics(
        &mut self,
        batch_id: CompositorFrameBatchId,
        admission: FrameCallbackAdmission,
        callback_count: usize,
        callback_commit_ns: Option<u64>,
        callback_render_completed_ns: Option<u64>,
        admission_ns: u64,
    ) {
        match admission {
            FrameCallbackAdmission::Immediate => {
                self.frame_callback_metrics
                    .callbacks_completed_after_immediate_admission = self
                    .frame_callback_metrics
                    .callbacks_completed_after_immediate_admission
                    .saturating_add(callback_count as u64);
            }
            FrameCallbackAdmission::Ready => {
                self.frame_callback_metrics
                    .callbacks_completed_after_ready_admission = self
                    .frame_callback_metrics
                    .callbacks_completed_after_ready_admission
                    .saturating_add(callback_count as u64);
            }
            FrameCallbackAdmission::Direct => {}
        }
        self.frame_callback_metrics.last_callback_admission_ns = Some(admission_ns);
        self.frame_callback_metrics
            .last_callback_render_to_admission_ns = callback_render_completed_ns
            .filter(|render_ns| admission_ns >= *render_ns)
            .map(|render_ns| admission_ns.saturating_sub(render_ns));
        self.frame_callback_metrics
            .last_callback_commit_to_admission_ns = callback_commit_ns
            .filter(|commit_ns| admission_ns >= *commit_ns)
            .map(|commit_ns| admission_ns.saturating_sub(commit_ns));
        if let Some(duration_ns) = self
            .frame_callback_metrics
            .last_callback_render_to_admission_ns
        {
            self.frame_callback_metrics.callback_render_to_admission_us = self
                .frame_callback_metrics
                .callback_render_to_admission_us
                .saturating_add(duration_ns / 1_000);
        }
        if let Some(duration_ns) = self
            .frame_callback_metrics
            .last_callback_commit_to_admission_ns
        {
            self.frame_callback_metrics.callback_commit_to_admission_us = self
                .frame_callback_metrics
                .callback_commit_to_admission_us
                .saturating_add(duration_ns / 1_000);
        }
        client_pacing_log(
            "frame_callbacks_admitted",
            &[
                ("frame_batch_id", batch_id.get().to_string()),
                ("count", callback_count.to_string()),
                ("admission", format!("{admission:?}")),
                ("admission_ns", admission_ns.to_string()),
            ],
        );
    }

    pub(crate) fn complete_direct_presented_frame_batch(
        &mut self,
        frame_id: u64,
        batch_id: CompositorFrameBatchId,
        direct_surface_id: u32,
        presentation: FramePresentation,
    ) {
        self.assert_frame_batch_identity(frame_id, batch_id);
        let (render_completed_ns, callbacks_remaining) = self
            .frame_batches
            .get(&batch_id)
            .map(|batch| (batch.callback_render_completed_ns, batch.callbacks.len()))
            .expect("missing compositor frame batch at direct presentation");
        self.note_frame_callbacks_at_pageflip(batch_id, render_completed_ns, callbacks_remaining);
        self.complete_frame_callbacks_at_presentation_fallback(batch_id);
        let mut batch = self.take_presented_frame_batch(frame_id, batch_id);
        if !matches!(presentation.kind, PresentationKind::Tearing) {
            for claim in &batch.fifo_barrier_claims {
                if claim.surface_id == direct_surface_id {
                    self.clear_fifo_barrier_claim(*claim, FifoBarrierClearReason::Presented);
                }
            }
        }
        for claim in &batch.commit_timing_target_claims {
            if claim.surface_id == direct_surface_id {
                self.complete_commit_timing_claim(*claim, presentation);
            } else {
                self.discard_commit_timing_claim(*claim);
            }
        }
        let feedbacks = std::mem::take(&mut batch.presentation_feedbacks);
        self.clear_legacy_batch_reference(batch_id);
        self.complete_direct_presentation_feedbacks(feedbacks, direct_surface_id, presentation);
        let _ = self.complete_frame_batch_releases(batch_id, batch);
    }

    pub(crate) fn complete_no_visual_change_frame_batch(
        &mut self,
        batch_id: CompositorFrameBatchId,
    ) {
        let _ = self.settle_terminal_callback_ownership(
            batch_id,
            TerminalCallbackDisposition::NoVisualChange,
        );
        let mut batch = self
            .frame_batches
            .remove(&batch_id)
            .expect("missing compositor frame batch for no-visual-change settlement");
        batch.callback_pacing_state = FrameCallbackPacingState::Completed;
        // A no-visual-change result is not a content-latching event.  In
        // particular, a direct same-buffer attempt must not satisfy FIFO
        // merely because the buffer identity did not change; forward
        // progress is handled by the barrier's explicit fallback deadline.
        for claim in &batch.commit_timing_target_claims {
            self.discard_commit_timing_claim(*claim);
        }
        for pending in std::mem::take(&mut batch.presentation_feedbacks) {
            pending.feedback.discarded();
        }
        let surface_damage = batch.surface_damage.take();
        let callbacks = batch
            .callbacks
            .drain(..)
            .filter(|callback| callback.is_alive())
            .collect();
        let callback_time = self.frame_callback_time_ms();
        self.complete_frame_callbacks_at_time(callbacks, callback_time);
        let _ = self.complete_frame_batch_releases(batch_id, batch);
        // A proven no-visual-change terminal advances only the logical
        // surface-damage baseline. It does not advance any physical output
        // presentation authority.
        if let Some(surface_damage) = surface_damage {
            self.commit_surface_damage_no_visual_change(surface_damage);
        }
        self.clear_legacy_batch_reference(batch_id);
    }

    pub(in crate::compositor) const fn frame_callback_metrics(&self) -> FrameCallbackMetrics {
        self.frame_callback_metrics
    }

    pub(in crate::compositor) fn note_frame_callbacks_committed(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        let commit_ns = client_pacing_now_ns();
        self.frame_callback_metrics.callbacks_requested = self
            .frame_callback_metrics
            .callbacks_requested
            .saturating_add(count as u64);
        self.frame_callback_metrics
            .last_callback_admission_to_next_commit_ns = self
            .frame_callback_metrics
            .last_callback_admission_ns
            .filter(|admission_ns| commit_ns >= *admission_ns)
            .map(|admission_ns| commit_ns.saturating_sub(admission_ns));
        if let Some(duration_ns) = self
            .frame_callback_metrics
            .last_callback_admission_to_next_commit_ns
        {
            self.frame_callback_metrics
                .callback_admission_to_next_commit_us = self
                .frame_callback_metrics
                .callback_admission_to_next_commit_us
                .saturating_add(duration_ns / 1_000);
        }
        self.frame_callback_metrics.last_callback_commit_ns = Some(commit_ns);
    }

    pub(in crate::compositor) fn complete_protocol_only_frame_tick(
        &mut self,
        output_time: FrameCallbackTime,
    ) -> ProtocolOnlyCompletion {
        let callbacks = self
            .take_visible_pending_frame_callbacks()
            .into_iter()
            .filter(|callback| callback.is_alive())
            .collect::<Vec<_>>();
        if !self.pending_dmabuf_buffer_releases.is_empty() {
            self.settle_no_visual_change_work(None, true);
        }
        if callbacks.is_empty() {
            return ProtocolOnlyCompletion::NoCallbacks;
        }
        let callback_count = callbacks.len();
        self.complete_frame_callbacks_at_time(callbacks, output_time.milliseconds());
        ProtocolOnlyCompletion::Completed { callback_count }
    }

    pub(in crate::compositor) fn note_frame_callbacks_at_pageflip(
        &mut self,
        batch_id: CompositorFrameBatchId,
        render_completed_ns: Option<u64>,
        callbacks_remaining: usize,
    ) {
        let Some(render_completed_ns) = render_completed_ns else {
            return;
        };
        let pageflip_ns = client_pacing_now_ns();
        self.frame_callback_metrics.last_callback_pageflip_ns = Some(pageflip_ns);
        self.frame_callback_metrics
            .last_callback_render_to_pageflip_ns =
            Some(pageflip_ns.saturating_sub(render_completed_ns));
        if callbacks_remaining > 0 {
            self.frame_callback_metrics.callbacks_found_at_pageflip = self
                .frame_callback_metrics
                .callbacks_found_at_pageflip
                .saturating_add(callbacks_remaining as u64);
        }
        client_pacing_log(
            "frame_callbacks_pageflip_correlation",
            &[
                ("frame_batch_id", batch_id.get().to_string()),
                ("callback_pageflip_ns", pageflip_ns.to_string()),
                (
                    "render_to_pageflip_ns",
                    self.frame_callback_metrics
                        .last_callback_render_to_pageflip_ns
                        .unwrap_or_default()
                        .to_string(),
                ),
                ("callbacks_remaining", callbacks_remaining.to_string()),
            ],
        );
    }
}
