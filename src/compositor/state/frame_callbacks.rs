use super::*;

impl CompositorState {
    pub(in crate::compositor) fn queue_frame_callbacks_for_surface(
        &mut self,
        surface_id: u32,
        callbacks: Vec<wl_callback::WlCallback>,
    ) {
        if self.surface_is_visible_in_active_workspace(surface_id) {
            self.visible_pending_frame_callback_count = self
                .visible_pending_frame_callback_count
                .saturating_add(callbacks.len());
        }
        for callback in callbacks {
            self.pending_frame_callback_surfaces
                .insert(callback.id(), surface_id);
            self.pending_frame_callbacks.push(callback);
        }
    }

    fn pending_frame_callback_is_visible(&self, callback: &wl_callback::WlCallback) -> bool {
        self.pending_frame_callback_surfaces
            .get(&callback.id())
            .is_none_or(|surface_id| self.surface_is_visible_in_active_workspace(*surface_id))
    }

    pub(in crate::compositor) fn discard_frame_callbacks_for_surface(&mut self, surface_id: u32) {
        let callback_ids = self
            .pending_frame_callback_surfaces
            .iter()
            .filter_map(|(callback_id, owner_surface_id)| {
                (*owner_surface_id == surface_id).then_some(callback_id.clone())
            })
            .collect::<HashSet<_>>();
        if callback_ids.is_empty() {
            return;
        }

        let pending_discarded = self
            .pending_frame_callbacks
            .iter()
            .filter(|callback| callback_ids.contains(&callback.id()))
            .count();
        self.pending_frame_callbacks
            .retain(|callback| !callback_ids.contains(&callback.id()));
        self.visible_pending_frame_callback_count = self
            .visible_pending_frame_callback_count
            .saturating_sub(pending_discarded);
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
        let pending = std::mem::take(&mut self.pending_frame_callbacks);
        let mut visible = Vec::with_capacity(pending.len());
        for callback in pending {
            if self.pending_frame_callback_is_visible(&callback) {
                visible.push(callback);
            } else {
                self.pending_frame_callbacks.push(callback);
            }
        }
        self.visible_pending_frame_callback_count = self
            .visible_pending_frame_callback_count
            .saturating_sub(visible.len());
        visible
    }

    pub(in crate::compositor) fn refresh_frame_work_visibility(&mut self) {
        self.visible_pending_frame_callback_count = self
            .pending_frame_callbacks
            .iter()
            .filter(|callback| self.pending_frame_callback_is_visible(callback))
            .count();
        self.visible_pending_presentation_feedback_count = self
            .pending_presentation_feedbacks
            .iter()
            .filter(|feedback| self.pending_presentation_feedback_is_visible(feedback))
            .count();
    }

    pub(in crate::compositor) fn queue_pending_presentation_feedbacks(
        &mut self,
        feedbacks: Vec<PendingPresentationFeedback>,
    ) {
        self.visible_pending_presentation_feedback_count = self
            .visible_pending_presentation_feedback_count
            .saturating_add(
                feedbacks
                    .iter()
                    .filter(|feedback| self.pending_presentation_feedback_is_visible(feedback))
                    .count(),
            );
        self.pending_presentation_feedbacks.extend(feedbacks);
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
            || self.pending_explicit_sync_commits.iter().any(|commit| {
                !self.external_acquire_readiness
                    && !commit.frame_callbacks.is_empty()
                    && self.surface_is_visible_in_active_workspace(commit.surface_id)
            })
            || self
                .pending_surface_tree_transactions
                .iter()
                .flat_map(|transaction| &transaction.nodes)
                .any(|(surface_id, commit)| {
                    !commit.frame_callbacks.is_empty()
                        && self.surface_is_visible_in_active_workspace(*surface_id)
                })
    }

    pub(in crate::compositor) fn has_only_pending_surface_frame_callbacks(&self) -> bool {
        if self.visible_pending_frame_callback_count == 0 {
            return false;
        }
        !self.pending_resize_configure_is_flushable()
            && self.pending_explicit_sync_commits.is_empty()
            && self.pending_surface_tree_transactions.is_empty()
            && self.pending_color_info.is_empty()
            && !self.has_visible_pending_presentation_feedbacks()
    }

    fn pending_presentation_feedback_is_visible(
        &self,
        feedback: &PendingPresentationFeedback,
    ) -> bool {
        self.surface_is_visible_in_active_workspace(feedback.surface_id)
    }

    pub(in crate::compositor) fn take_visible_pending_presentation_feedbacks(
        &mut self,
    ) -> Vec<PendingPresentationFeedback> {
        let pending = std::mem::take(&mut self.pending_presentation_feedbacks);
        let mut visible = Vec::with_capacity(pending.len());
        for feedback in pending {
            if self.pending_presentation_feedback_is_visible(&feedback) {
                visible.push(feedback);
            } else {
                self.pending_presentation_feedbacks.push(feedback);
            }
        }
        self.visible_pending_presentation_feedback_count = self
            .visible_pending_presentation_feedback_count
            .saturating_sub(visible.len());
        visible
    }

    pub(in crate::compositor) fn has_visible_pending_presentation_feedbacks(&self) -> bool {
        self.visible_pending_presentation_feedback_count > 0
    }

    pub(in crate::compositor) fn has_unowned_frame_callbacks(&self) -> bool {
        self.visible_pending_frame_callback_count > 0
            || self.pending_explicit_sync_commits.iter().any(|commit| {
                !self.external_acquire_readiness
                    && !commit.frame_callbacks.is_empty()
                    && self.surface_is_visible_in_active_workspace(commit.surface_id)
            })
            || self
                .pending_surface_tree_transactions
                .iter()
                .flat_map(|transaction| &transaction.nodes)
                .any(|(surface_id, commit)| {
                    !commit.frame_callbacks.is_empty()
                        && self.surface_is_visible_in_active_workspace(*surface_id)
                })
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
            return if settlement.completed_after_render == 0 && settlement.cancelled > 0 {
                TerminalCallbackOwnership::Cancelled {
                    callbacks: settlement.cancelled,
                }
            } else {
                TerminalCallbackOwnership::Resolved {
                    completed: settlement.completed_after_render,
                }
            };
        }
        match disposition {
            TerminalCallbackDisposition::Presented
            | TerminalCallbackDisposition::NoVisualChange => {
                settlement.complete(live);
                settlement.cancel(cancelled);
                if settlement.count_mismatch || !settlement.is_reconciled() {
                    return TerminalCallbackOwnership::Leaked {
                        owner: batch_id,
                        unresolved: settlement.unresolved,
                        reason: TerminalCallbackLeakReason::CountMismatch,
                    };
                }
                TerminalCallbackOwnership::Resolved {
                    completed: settlement.completed_after_render,
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

    pub(crate) fn complete_direct_presented_frame_batch(
        &mut self,
        frame_id: u64,
        batch_id: CompositorFrameBatchId,
        direct_surface_id: u32,
        presentation: FramePresentation,
    ) {
        self.assert_frame_batch_identity(frame_id, batch_id);
        let _ = self
            .settle_terminal_callback_ownership(batch_id, TerminalCallbackDisposition::Presented);
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
        self.note_frame_callbacks_at_pageflip(batch_id, &batch);
        let callbacks = std::mem::take(&mut batch.callbacks);
        self.complete_frame_callbacks(callbacks);
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
        let callbacks = batch
            .callbacks
            .drain(..)
            .filter(|callback| callback.is_alive())
            .collect();
        let callback_time = self.frame_callback_time_ms();
        self.complete_frame_callbacks_at_time(callbacks, callback_time);
        let _ = self.complete_frame_batch_releases(batch_id, batch);
        self.clear_legacy_batch_reference(batch_id);
    }

    pub(in crate::compositor) const fn frame_callback_metrics(&self) -> FrameCallbackMetrics {
        self.frame_callback_metrics
    }

    pub(in crate::compositor) fn note_frame_callbacks_committed(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        self.frame_callback_metrics.callbacks_requested = self
            .frame_callback_metrics
            .callbacks_requested
            .saturating_add(count as u64);
        self.frame_callback_metrics.last_callback_commit_ns = Some(client_pacing_now_ns());
    }

    pub(in crate::compositor) fn complete_rendered_frame_callbacks(
        &mut self,
        batch_id: CompositorFrameBatchId,
    ) {
        let completed_ns = client_pacing_now_ns();
        let (callbacks, callback_commit_ns) = {
            let batch = self
                .frame_batches
                .get_mut(&batch_id)
                .expect("missing compositor frame batch at render completion");
            let callbacks = batch.callbacks.drain(..).collect::<Vec<_>>();
            let completed = callbacks
                .iter()
                .filter(|callback| callback.is_alive())
                .count();
            let cancelled = callbacks.len().saturating_sub(completed);
            batch.callback_settlement.complete(completed);
            batch.callback_settlement.cancel(cancelled);
            if !callbacks.is_empty() {
                batch.callback_render_completed_ns = Some(completed_ns);
            }
            (callbacks, batch.callback_commit_ns)
        };
        if callbacks.is_empty() {
            return;
        }
        self.frame_callback_metrics.callbacks_completed_after_render = self
            .frame_callback_metrics
            .callbacks_completed_after_render
            .saturating_add(callbacks.len() as u64);
        self.frame_callback_metrics
            .last_callback_render_completed_ns = Some(completed_ns);
        self.frame_callback_metrics
            .last_callback_commit_to_render_ns = callback_commit_ns
            .filter(|commit_ns| completed_ns >= *commit_ns)
            .map(|commit_ns| completed_ns.saturating_sub(commit_ns));
        client_pacing_log(
            "frame_callbacks_render_completed",
            &[
                ("frame_batch_id", batch_id.get().to_string()),
                ("count", callbacks.len().to_string()),
                ("render_completed_ns", completed_ns.to_string()),
                (
                    "commit_to_render_ns",
                    self.frame_callback_metrics
                        .last_callback_commit_to_render_ns
                        .unwrap_or_default()
                        .to_string(),
                ),
            ],
        );
        self.complete_frame_callbacks(callbacks);
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
        batch: &CompositorFrameBatch,
    ) {
        let Some(render_completed_ns) = batch.callback_render_completed_ns else {
            return;
        };
        let pageflip_ns = client_pacing_now_ns();
        self.frame_callback_metrics.last_callback_pageflip_ns = Some(pageflip_ns);
        self.frame_callback_metrics
            .last_callback_render_to_pageflip_ns =
            Some(pageflip_ns.saturating_sub(render_completed_ns));
        if !batch.callbacks.is_empty() {
            self.frame_callback_metrics.callbacks_found_at_pageflip = self
                .frame_callback_metrics
                .callbacks_found_at_pageflip
                .saturating_add(batch.callbacks.len() as u64);
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
                ("callbacks_remaining", batch.callbacks.len().to_string()),
            ],
        );
    }
}
