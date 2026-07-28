use super::*;

impl CompositorState {
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
        !self.pending_frame_callbacks.is_empty()
            || self
                .frame_batches
                .values()
                .any(|batch| !batch.callbacks.is_empty())
            || self.pending_explicit_sync_commits.iter().any(|commit| {
                !self.external_acquire_readiness && !commit.frame_callbacks.is_empty()
            })
            || self
                .pending_surface_tree_transactions
                .iter()
                .flat_map(|transaction| &transaction.nodes)
                .any(|(_, commit)| !commit.frame_callbacks.is_empty())
    }

    pub(in crate::compositor) fn has_only_pending_surface_frame_callbacks(&self) -> bool {
        if self.pending_frame_callbacks.is_empty() {
            return false;
        }
        self.pending_interactive_resize_update.is_none()
            && !self.pending_resize_configure_is_flushable()
            && self.pending_explicit_sync_commits.is_empty()
            && self.pending_surface_tree_transactions.is_empty()
            && self.pending_color_info.is_empty()
            && self.pending_presentation_feedbacks.is_empty()
    }

    pub(in crate::compositor) fn has_unowned_frame_callbacks(&self) -> bool {
        !self.pending_frame_callbacks.is_empty()
            || self.pending_explicit_sync_commits.iter().any(|commit| {
                !self.external_acquire_readiness && !commit.frame_callbacks.is_empty()
            })
            || self
                .pending_surface_tree_transactions
                .iter()
                .flat_map(|transaction| &transaction.nodes)
                .any(|(_, commit)| !commit.frame_callbacks.is_empty())
    }

    pub(crate) fn prepare_terminal_callback_ownership(
        &self,
        batch_id: CompositorFrameBatchId,
        disposition: TerminalCallbackDisposition,
    ) -> TerminalCallbackOwnership {
        let Some(batch) = self.frame_batches.get(&batch_id) else {
            return TerminalCallbackOwnership::Leaked {
                owner: batch_id,
                pending: 0,
            };
        };
        let pending = batch
            .callbacks
            .iter()
            .filter(|callback| callback.is_alive())
            .count();
        if pending == 0 {
            return TerminalCallbackOwnership::None;
        }
        match disposition {
            TerminalCallbackDisposition::Presented
                if batch.callback_render_completed_ns.is_some() =>
            {
                TerminalCallbackOwnership::Leaked {
                    owner: batch_id,
                    pending,
                }
            }
            TerminalCallbackDisposition::Retryable => {
                TerminalCallbackOwnership::Transferred(batch_id)
            }
            TerminalCallbackDisposition::Cancelled => TerminalCallbackOwnership::Cancelled,
            TerminalCallbackDisposition::Superseded
            | TerminalCallbackDisposition::Presented
            | TerminalCallbackDisposition::NoVisualChange => TerminalCallbackOwnership::Resolved,
        }
    }

    pub(crate) fn complete_direct_presented_frame_batch(
        &mut self,
        frame_id: u64,
        batch_id: CompositorFrameBatchId,
        direct_surface_id: u32,
        presentation: FramePresentation,
    ) {
        let mut batch = self.take_presented_frame_batch(frame_id, batch_id);
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
        let mut batch = self
            .frame_batches
            .remove(&batch_id)
            .expect("missing compositor frame batch for no-visual-change settlement");
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
            batch.callback_render_completed_ns = (!callbacks.is_empty()).then_some(completed_ns);
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
        let callbacks = std::mem::take(&mut self.pending_frame_callbacks)
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
