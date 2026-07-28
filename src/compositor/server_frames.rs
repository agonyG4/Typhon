use super::*;

#[derive(Debug, Clone, Copy)]
pub struct PreparedDirectFrameBatch {
    pub(crate) frame_id: u64,
    pub(crate) batch_id: CompositorFrameBatchId,
    pub(crate) direct_surface_id: u32,
}

impl OwnCompositorServer {
    #[doc(hidden)]
    pub fn has_prepared_frame_batch(&self) -> bool {
        self.state.legacy_prepared_frame_batch.is_some()
    }

    #[doc(hidden)]
    pub fn prepared_frame_batch_id(&self) -> Option<CompositorFrameBatchId> {
        self.state.legacy_prepared_frame_batch
    }

    #[doc(hidden)]
    pub fn prepared_frame_id(&self) -> Option<u64> {
        self.state
            .legacy_prepared_frame_batch
            .and_then(|batch_id| self.state.frame_batches.get(&batch_id))
            .map(|batch| batch.frame_id)
    }

    #[doc(hidden)]
    pub fn frame_batch_count(&self) -> usize {
        self.state.frame_batches.len()
    }

    pub fn has_frame_batch(&self, batch_id: CompositorFrameBatchId) -> bool {
        self.state.frame_batches.contains_key(&batch_id)
    }

    pub fn direct_callback_owner_leaks(&self, batch_id: CompositorFrameBatchId) -> u64 {
        self.state.direct_callback_owner_leaks(batch_id)
    }

    pub fn prepare_direct_presented_frame_batch(
        &self,
        frame_id: u64,
        batch_id: CompositorFrameBatchId,
        direct_surface_id: u32,
    ) -> io::Result<PreparedDirectFrameBatch> {
        let batch = self
            .state
            .frame_batches
            .get(&batch_id)
            .ok_or_else(|| io::Error::other("direct pageflip frame batch is not owned"))?;
        if batch.frame_id != frame_id {
            return Err(io::Error::other(
                "direct pageflip frame batch frame identity does not match",
            ));
        }
        Ok(PreparedDirectFrameBatch {
            frame_id,
            batch_id,
            direct_surface_id,
        })
    }

    pub fn commit_prepared_direct_presented_frame_batch(
        &mut self,
        prepared: PreparedDirectFrameBatch,
        presentation: FramePresentation,
    ) -> u64 {
        let callback_owner_leaks = self.state.complete_direct_presented_frame_batch(
            prepared.frame_id,
            prepared.batch_id,
            prepared.direct_surface_id,
            presentation,
        );
        let _ = self.display.flush_clients();
        callback_owner_leaks
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

    pub fn finish_immediate_frame_batch(
        &mut self,
        batch_id: CompositorFrameBatchId,
    ) -> io::Result<()> {
        self.finish_immediate_frame_batch_with(batch_id, FramePresentation::software_now)
    }

    #[doc(hidden)]
    pub fn finish_immediate_frame_batch_with<F>(
        &mut self,
        batch_id: CompositorFrameBatchId,
        software_now: F,
    ) -> io::Result<()>
    where
        F: FnOnce(PresentationClock) -> io::Result<FramePresentation>,
    {
        if self.state.legacy_prepared_frame_batch != Some(batch_id) {
            return Err(io::Error::other(
                "immediate presentation batch is not the prepared output batch",
            ));
        }
        let presentation = match software_now(self.state.presentation_clock) {
            Ok(presentation) => presentation,
            Err(error) => {
                self.state.complete_frame_batch_after_safe_abandonment(
                    batch_id,
                    FrameBatchDiscardReason::OutputDestroyed,
                );
                let _ = self.display.flush_clients();
                return Err(error);
            }
        };
        self.state.complete_rendered_frame_callbacks(batch_id);
        let frame_id = self
            .state
            .frame_batches
            .get(&batch_id)
            .ok_or_else(|| io::Error::other("immediate presentation batch disappeared"))?
            .frame_id;
        self.state
            .complete_presented_frame_batch(frame_id, batch_id, presentation);
        let _ = self.display.flush_clients();
        Ok(())
    }

    pub fn finish_presented_frame_batch(
        &mut self,
        batch_id: CompositorFrameBatchId,
        presentation: FramePresentation,
    ) -> io::Result<()> {
        if self.state.legacy_prepared_frame_batch != Some(batch_id)
            && self.state.legacy_submitted_frame_batch != Some(batch_id)
        {
            return Err(io::Error::other(
                "presented batch is not owned by the compatibility output path",
            ));
        }
        let frame_id = self
            .state
            .frame_batches
            .get(&batch_id)
            .ok_or_else(|| io::Error::other("presented compositor frame batch disappeared"))?
            .frame_id;
        self.state
            .complete_presented_frame_batch(frame_id, batch_id, presentation);
        let _ = self.display.flush_clients();
        Ok(())
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
    pub fn complete_no_visual_change_frame_batch(&mut self, batch_id: CompositorFrameBatchId) {
        self.state.complete_no_visual_change_frame_batch(batch_id);
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
