use super::*;

impl OwnCompositorServer {
    #[doc(hidden)]
    pub fn has_prepared_frame_batch(&self) -> bool {
        self.state.legacy_prepared_frame_batch.is_some()
    }

    #[doc(hidden)]
    pub fn frame_batch_count(&self) -> usize {
        self.state.frame_batches.len()
    }

    #[doc(hidden)]
    pub fn has_submitted_frame_batch(&self) -> bool {
        self.state.has_submitted_frame_batch()
    }

    /// Settle the batch captured immediately before a legacy paint/present
    /// terminal path. This deliberately does not consume an older submitted
    /// batch that is still owned by a pageflip.
    pub fn finish_prepared_frame(&mut self) {
        let batch_id = self
            .state
            .legacy_prepared_frame_batch
            .expect("no prepared compositor frame batch exists");
        let Ok(presentation) = FramePresentation::software_now(self.state.presentation_clock)
        else {
            self.state.complete_frame_batch_after_safe_abandonment(
                batch_id,
                FrameBatchDiscardReason::OutputDestroyed,
            );
            let _ = self.display.flush_clients();
            return;
        };
        self.state.complete_rendered_frame_callbacks(batch_id);
        let frame_id = self
            .state
            .frame_batches
            .get(&batch_id)
            .expect("prepared compositor frame batch disappeared")
            .frame_id;
        self.state
            .complete_presented_frame_batch(frame_id, batch_id, presentation);
        let _ = self.display.flush_clients();
    }

    pub fn capture_frame_callbacks_for_render(&mut self) {
        self.state.capture_frame_callbacks_for_render();
    }

    #[doc(hidden)]
    pub fn complete_rendered_frame_callbacks(&mut self, batch_id: CompositorFrameBatchId) {
        self.state.complete_rendered_frame_callbacks(batch_id);
        let _ = self.display.flush_clients();
    }

    #[doc(hidden)]
    pub fn complete_rendered_frame_callbacks_for_prepared(&mut self) {
        let batch_id = self
            .state
            .legacy_prepared_frame_batch
            .expect("no prepared compositor frame batch exists");
        self.complete_rendered_frame_callbacks(batch_id);
    }

    #[doc(hidden)]
    pub fn restore_prepared_frame_batch_after_render_failure(&mut self) {
        let batch_id = self
            .state
            .legacy_prepared_frame_batch
            .expect("no prepared compositor frame batch exists");
        self.restore_frame_batch_after_render_failure(batch_id);
    }

    #[doc(hidden)]
    pub fn take_frame_batch_for_render(&mut self, frame_id: u64) -> CompositorFrameBatchId {
        self.state.take_frame_batch_for_render(frame_id)
    }
}
