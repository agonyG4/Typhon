use super::*;

impl CompositorState {
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
