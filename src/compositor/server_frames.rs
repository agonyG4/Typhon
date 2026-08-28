use super::*;
use crate::native::presentation_deadline::MonotonicTimestampNs;
use wayland_server::{Resource, backend::protocol::ProtocolError};

#[derive(Debug, Clone, Copy)]
pub struct PreparedDirectFrameBatch {
    pub(crate) frame_id: u64,
    pub(crate) batch_id: CompositorFrameBatchId,
    pub(crate) direct_surface_id: u32,
}

impl OwnCompositorServer {
    pub fn popup_surface_ids(&self) -> &[u32] {
        self.state.active_scene_popup_surface_ids()
    }

    pub(super) fn kill_pending_resource_exhaustion_clients(&mut self) {
        for surface_id in self.state.take_client_resource_exhaustions() {
            let handle = self.display.handle();
            if let Some(surface) = self.state.surface_resource_by_id(surface_id)
                && let Ok(client) = handle.get_client(surface.id())
            {
                client.kill(
                    &handle,
                    ProtocolError {
                        code: 2,
                        object_id: 1,
                        object_interface: "wl_display".to_string(),
                        message: "surface tree pacing transaction queue exhausted".to_string(),
                    },
                );
            }
        }
    }

    /// Earliest monotonic wake at which an ordered surface transaction can
    /// change pacing readiness.  Native output folds this into its existing
    /// deadline arbitration.
    #[doc(hidden)]
    pub fn has_surface_pacing_work(&self) -> bool {
        self.state.has_surface_pacing_work()
    }

    #[doc(hidden)]
    pub fn next_surface_pacing_deadline_ns(&self) -> Option<u64> {
        self.state.next_surface_pacing_deadline_ns()
    }

    #[doc(hidden)]
    pub fn next_commit_timing_planning_deadline_ns(&self) -> Option<u64> {
        self.state.next_commit_timing_planning_deadline_ns()
    }

    #[doc(hidden)]
    pub fn has_surface_pacing_readiness_pending(&self) -> bool {
        self.state.surface_pacing_readiness_pending()
    }

    #[doc(hidden)]
    pub fn surface_pacing_readiness_generation(&self) -> u64 {
        self.state.surface_pacing_readiness_generation()
    }

    #[doc(hidden)]
    pub fn has_pending_commit_timing_planning(&self) -> bool {
        self.state.has_pending_commit_timing_planning()
    }

    #[doc(hidden)]
    pub fn commit_timing_planning_generation(&self) -> u64 {
        self.state.commit_timing_planning_generation()
    }

    /// Earliest monotonic Commit Timing lower bound still held by an ordered
    /// surface transaction.  Native output uses this to select a refresh
    /// target before the transaction becomes publishable.
    #[doc(hidden)]
    pub fn next_commit_timing_deadline_ns(&self) -> Option<u64> {
        self.state.next_commit_timing_deadline_ns()
    }

    #[doc(hidden)]
    pub fn commit_timing_planning_candidates(&mut self) -> Vec<CommitTimingPlanningCandidate> {
        self.state.commit_timing_planning_candidates()
    }

    #[doc(hidden)]
    pub fn has_pending_commit_timing(&self) -> bool {
        self.state.has_pending_commit_timing()
    }

    #[doc(hidden)]
    pub fn arm_commit_timing_target(
        &mut self,
        candidate: CommitTimingPlanningCandidate,
        selected_monotonic_presentation_time: MonotonicTimestampNs,
        release_for_render_at: MonotonicTimestampNs,
        selected_sequence: u64,
        clock_generation: u64,
    ) -> bool {
        self.state.arm_commit_timing_target(CommitTimingReadiness {
            transaction_id: candidate.transaction_id,
            requested_not_before: candidate.requested_not_before,
            selected_monotonic_presentation_time,
            release_for_render_at,
            selected_sequence,
            clock_generation,
            clock_mapping: candidate.clock_mapping,
        })
    }

    #[doc(hidden)]
    pub fn invalidate_commit_timing_targets(&mut self) {
        self.state.invalidate_pending_commit_timing_targets();
    }

    #[doc(hidden)]
    pub fn commit_timing_submission_is_safe_for_batch(
        &mut self,
        batch_id: CompositorFrameBatchId,
        planned_monotonic_presentation_time: MonotonicTimestampNs,
        clock_generation: u64,
    ) -> bool {
        self.state.commit_timing_submission_is_safe_for_batch(
            batch_id,
            planned_monotonic_presentation_time,
            clock_generation,
        )
    }

    /// Re-evaluate ordered FIFO/timing constraints after a native wake.  This
    /// publishes work through the normal surface-tree path; it does not render
    /// or submit anything itself.  The return value reports an active-scene
    /// visual handoff for the native scheduler.
    #[doc(hidden)]
    pub fn progress_surface_pacing(&mut self, now_ns: u64) -> Result<bool, CompositorError> {
        let visual_work = self.state.progress_surface_pacing(now_ns);
        self.flush_wayland_clients()?;
        Ok(visual_work)
    }

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

    pub fn prepare_terminal_callback_ownership(
        &mut self,
        batch_id: CompositorFrameBatchId,
        disposition: TerminalCallbackDisposition,
    ) -> TerminalCallbackOwnership {
        self.state
            .prepare_terminal_callback_ownership(batch_id, disposition)
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
    ) {
        self.state.complete_direct_presented_frame_batch(
            prepared.frame_id,
            prepared.batch_id,
            prepared.direct_surface_id,
            presentation,
        );
        let _ = self.display.flush_clients();
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
        self.state.mark_frame_callbacks_rendered(batch_id);
        self.state
            .complete_frame_callbacks_after_admission(batch_id, FrameCallbackAdmission::Immediate);
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
        self.state.mark_frame_callbacks_rendered(batch_id);
        self.state
            .complete_frame_callbacks_after_admission(batch_id, FrameCallbackAdmission::Immediate);
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
    pub fn mark_frame_callbacks_rendered(&mut self, batch_id: CompositorFrameBatchId) {
        self.state.mark_frame_callbacks_rendered(batch_id);
        let _ = self.display.flush_clients();
    }

    #[doc(hidden)]
    pub fn complete_frame_callbacks_after_admission(
        &mut self,
        batch_id: CompositorFrameBatchId,
        admission: FrameCallbackAdmission,
    ) {
        self.state
            .complete_frame_callbacks_after_admission(batch_id, admission);
        let _ = self.display.flush_clients();
    }

    #[doc(hidden)]
    pub fn note_frame_callback_admission_failure(&mut self, batch_id: CompositorFrameBatchId) {
        self.state.note_frame_callback_admission_failure(batch_id);
    }

    pub fn note_frame_callbacks_deferred_ready(&mut self, batch_id: CompositorFrameBatchId) {
        self.state.note_frame_callbacks_deferred_ready(batch_id);
    }

    #[doc(hidden)]
    pub fn complete_direct_frame_callbacks_after_admission(
        &mut self,
        batch_id: CompositorFrameBatchId,
    ) {
        self.state
            .complete_direct_frame_callbacks_after_admission(batch_id);
        let _ = self.display.flush_clients();
    }

    #[doc(hidden)]
    pub fn complete_no_visual_change_frame_batch(&mut self, batch_id: CompositorFrameBatchId) {
        self.state.complete_no_visual_change_frame_batch(batch_id);
        let _ = self.display.flush_clients();
    }

    pub fn settle_no_visual_change_work(
        &mut self,
        surface_damage: Option<SurfaceDamagePresentation>,
        owns_frame_batch: bool,
    ) -> bool {
        let completed = self
            .state
            .settle_no_visual_change_work(surface_damage, owns_frame_batch);
        if completed {
            let _ = self.display.flush_clients();
        }
        completed
    }

    #[doc(hidden)]
    pub fn mark_frame_callbacks_rendered_for_prepared(&mut self) {
        let batch_id = self
            .state
            .legacy_prepared_frame_batch
            .expect("no prepared compositor frame batch exists");
        self.mark_frame_callbacks_rendered(batch_id);
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

    #[cfg(test)]
    pub(crate) fn test_frame_callback_pacing_is_completed(
        &self,
        batch_id: CompositorFrameBatchId,
    ) -> bool {
        self.state
            .frame_batches
            .get(&batch_id)
            .is_some_and(|batch| {
                matches!(
                    batch.callback_pacing_state,
                    crate::compositor::frame_batch::FrameCallbackPacingState::Completed
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_pacing_service_flushes_at_its_write_side_boundary() {
        let mut server = OwnCompositorServer::bind(format!(
            "typhon-surface-pacing-flush-{}",
            std::process::id()
        ))
        .expect("surface pacing test Wayland socket");
        let before = server.wayland_flush_count_for_tests();

        assert!(!server.progress_surface_pacing(0).unwrap());

        assert_eq!(server.wayland_flush_count_for_tests(), before + 1);
    }
}
