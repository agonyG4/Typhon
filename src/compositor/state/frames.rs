use std::num::NonZeroU64;

use super::*;
use crate::compositor::frame_batch::FrameCallbackPacingState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum DmabufReleaseCompletion {
    Completed,
    Discarded,
    DeferredCurrent,
    SignalRetry,
}

#[derive(Debug, Default)]
pub(in crate::compositor) struct ShutdownDmabufReleaseSet {
    obligations: Vec<DmabufReleaseObligation>,
}

fn select_callback_timing(
    timings: impl IntoIterator<Item = FrameCallbackTimingEvidence>,
) -> (Option<FrameCallbackTimingEvidence>, bool) {
    let mut selected = None;
    let mut selected_surface_id = None;
    let mut ambiguous = false;
    for timing in timings {
        match selected_surface_id {
            None => {
                selected_surface_id = Some(timing.surface_id);
                selected = Some(timing);
            }
            Some(surface_id) if surface_id == timing.surface_id => {
                if selected.is_none_or(|current| timing.commit_ns > current.commit_ns) {
                    selected = Some(timing);
                }
            }
            Some(_) => ambiguous = true,
        }
    }
    if ambiguous {
        (None, true)
    } else {
        (selected, false)
    }
}

impl ShutdownDmabufReleaseSet {
    pub(in crate::compositor) fn push(&mut self, obligation: DmabufReleaseObligation) {
        if self
            .obligations
            .iter()
            .any(|existing| existing.same_release_token(&obligation))
        {
            return;
        }
        self.obligations.push(obligation);
    }

    pub(in crate::compositor) fn complete(self, state: &mut CompositorState) {
        for obligation in self.obligations {
            state.complete_dmabuf_release(CompositorFrameBatchId::for_shutdown(), 0, obligation);
        }
    }
}

impl CompositorState {
    pub(in crate::compositor) const fn buffer_release_metrics(&self) -> BufferReleaseMetrics {
        self.buffer_release_metrics
    }

    pub(in crate::compositor) const fn shm_buffer_lifetime_metrics(
        &self,
    ) -> ShmBufferLifetimeMetrics {
        self.shm_buffer_lifetime_metrics
    }

    pub(in crate::compositor) fn record_surface_tree_merge_metrics(
        &mut self,
        stats: &SurfaceTreeMergeStats,
    ) {
        self.surface_transactions
            .metrics
            .bufferless_tree_commits_merged = self
            .surface_transactions
            .metrics
            .bufferless_tree_commits_merged
            .saturating_add((stats.bufferless_nodes == stats.incoming_nodes) as u64);
        self.surface_transactions.metrics.metadata_only_nodes_merged = self
            .surface_transactions
            .metrics
            .metadata_only_nodes_merged
            .saturating_add(stats.bufferless_nodes as u64);
        self.surface_transactions.metrics.attachments_replaced = self
            .surface_transactions
            .metrics
            .attachments_replaced
            .saturating_add(stats.attachments_replaced as u64);
        self.surface_transactions.metrics.explicit_detaches = self
            .surface_transactions
            .metrics
            .explicit_detaches
            .saturating_add(stats.explicit_detaches as u64);
        self.surface_transactions
            .metrics
            .acquire_dependencies_preserved = self
            .surface_transactions
            .metrics
            .acquire_dependencies_preserved
            .saturating_add(stats.dependencies_preserved as u64);
        self.surface_transactions
            .metrics
            .acquire_dependencies_replaced = self
            .surface_transactions
            .metrics
            .acquire_dependencies_replaced
            .saturating_add(stats.dependencies_replaced as u64);
        self.surface_transactions.metrics.callbacks_merged = self
            .surface_transactions
            .metrics
            .callbacks_merged
            .saturating_add(stats.callbacks_merged as u64);
        self.surface_transactions.metrics.feedbacks_merged = self
            .surface_transactions
            .metrics
            .feedbacks_merged
            .saturating_add(stats.feedbacks_merged as u64);
        self.surface_transactions.metrics.resize_snapshots_preserved = self
            .surface_transactions
            .metrics
            .resize_snapshots_preserved
            .saturating_add(stats.resize_snapshots_preserved as u64);
        self.surface_transactions.metrics.resize_snapshots_replaced = self
            .surface_transactions
            .metrics
            .resize_snapshots_replaced
            .saturating_add(stats.resize_snapshots_replaced as u64);
    }

    pub(in crate::compositor) fn mark_prepared_frame_submitted(&mut self) {
        assert!(
            self.legacy_submitted_frame_batch.is_none(),
            "a compositor output frame batch is already submitted"
        );
        self.legacy_submitted_frame_batch = Some(
            self.legacy_prepared_frame_batch
                .take()
                .expect("no prepared compositor frame batch exists"),
        );
    }

    pub(in crate::compositor) fn has_submitted_frame_batch(&self) -> bool {
        self.legacy_submitted_frame_batch.is_some()
    }

    pub(in crate::compositor) fn has_pending_frame_prepare_work(&self) -> bool {
        self.scene_work_index
            .has_visible_prepare_work(self.active_scene_selection())
            || self.pending_resize_configure_is_flushable()
            || !self.pending_color_info.is_empty()
    }

    pub(in crate::compositor) fn has_pending_interactive_visual_work(&self) -> bool {
        self.pending_tiled_resize.is_some() || self.has_pending_floating_interaction_geometry()
    }

    pub(in crate::compositor) fn record_interactive_render_admission(
        &mut self,
        render_ahead: bool,
    ) {
        self.resize_flow_metrics.interactive_render_admissions = self
            .resize_flow_metrics
            .interactive_render_admissions
            .saturating_add(1);
        if render_ahead {
            self.resize_flow_metrics.interactive_render_ahead_admissions = self
                .resize_flow_metrics
                .interactive_render_ahead_admissions
                .saturating_add(1);
        }
    }

    pub(in crate::compositor) fn record_interactive_scheduler_decision(&mut self) {
        self.resize_flow_metrics
            .interactive_scheduler_decisions_while_pending = self
            .resize_flow_metrics
            .interactive_scheduler_decisions_while_pending
            .saturating_add(1);
    }

    pub(in crate::compositor) fn has_unowned_frame_work(&mut self) -> bool {
        self.settle_lifecycle_no_visual_change();
        self.has_pending_frame_prepare_work()
            || self.has_pending_interactive_visual_work()
            || self.presentation_animation_has_pending_visible()
            || self.lifecycle_animation_has_pending_visible()
            || self.has_unowned_frame_callbacks()
            || self.has_frame_eligible_pending_presentation_feedbacks()
            // Deferred DMA-BUF releases are runtime retry debt.  They must
            // not manufacture a visual frame when no scene work exists.
            || !self.pending_dmabuf_buffer_releases.is_empty()
    }

    pub(in crate::compositor) fn settle_no_visual_change_work(
        &mut self,
        surface_damage: Option<SurfaceDamagePresentation>,
        owns_frame_batch: bool,
    ) -> bool {
        let completed_work = owns_frame_batch || surface_damage.is_some();
        if owns_frame_batch {
            if self.legacy_prepared_frame_batch.is_none() {
                self.capture_frame_callbacks_for_render_with_presentation_samples(
                    std::iter::empty(),
                );
            }
            if let Some(surface_damage) = surface_damage {
                let batch_id = self
                    .legacy_prepared_frame_batch
                    .expect("no prepared frame batch for surface damage ownership");
                self.set_frame_batch_surface_damage(batch_id, surface_damage);
            }
            let batch_id = self
                .legacy_prepared_frame_batch
                .expect("no prepared frame batch for no-visual-change settlement");
            self.complete_no_visual_change_frame_batch(batch_id);
        } else {
            // The output-damage authority has proven this logical transition
            // needs no pixels repaired. Settle its surface lineage without
            // asserting a physical output presentation.
            if let Some(surface_damage) = surface_damage {
                self.commit_surface_damage_no_visual_change(surface_damage);
            }
        }
        completed_work
    }

    pub(in crate::compositor) fn complete_pending_presentation_feedbacks(
        &mut self,
        presentation: FramePresentation,
    ) {
        let batch_id = self
            .legacy_submitted_frame_batch
            .take()
            .or_else(|| self.legacy_prepared_frame_batch.take())
            .expect("no compositor frame batch exists for presentation");
        let frame_id = self
            .frame_batches
            .get(&batch_id)
            .expect("compositor frame batch registry lost an owned batch")
            .frame_id;
        self.complete_presented_frame_batch(frame_id, batch_id, presentation);
    }

    pub(in crate::compositor) fn complete_presentation_feedbacks(
        &mut self,
        feedbacks: Vec<PendingPresentationFeedback>,
        presentation: FramePresentation,
    ) {
        if feedbacks.is_empty() {
            return;
        }

        let timestamp = presentation.timestamp;
        let (tv_sec_hi, tv_sec_lo) = timestamp.protocol_seconds();
        let sequence = presentation.sequence;
        let mut flags = match presentation.kind {
            PresentationKind::Synchronized => wp_presentation_feedback::Kind::Vsync,
            PresentationKind::Tearing => wp_presentation_feedback::Kind::empty(),
            PresentationKind::Software => wp_presentation_feedback::Kind::empty(),
        };
        if presentation.zero_copy {
            flags |= wp_presentation_feedback::Kind::ZeroCopy;
        }
        for pending in feedbacks {
            if !pending.surface.is_alive() || presentation.clock != self.presentation_clock {
                client_pacing_log(
                    "presentation_feedback_completed",
                    &[
                        ("surface", pending.surface_id.to_string()),
                        ("feedback", format!("{:?}", pending.feedback.id())),
                        ("outcome", "discarded".to_string()),
                    ],
                );
                pending.feedback.discarded();
                continue;
            }
            for output in self.output_resources.iter().filter(|binding| {
                binding.resource.is_alive()
                    && resource_belongs_to_surface_client(&binding.resource, &pending.surface)
            }) {
                pending.feedback.sync_output(&output.resource);
            }
            pending.feedback.presented(
                tv_sec_hi,
                tv_sec_lo,
                timestamp.nanoseconds(),
                presentation.feedback_refresh_nsec(self.output_refresh.presentation_refresh_nsec()),
                (sequence >> 32) as u32,
                sequence as u32,
                flags,
            );
            client_pacing_log(
                "presentation_feedback_completed",
                &[
                    ("surface", pending.surface_id.to_string()),
                    (
                        "root",
                        self.root_surface_id_for_surface(pending.surface_id)
                            .to_string(),
                    ),
                    (
                        "client",
                        format!("{:?}", self.surface_client_ids.get(&pending.surface_id)),
                    ),
                    ("feedback", format!("{:?}", pending.feedback.id())),
                    ("outcome", "presented".to_string()),
                    ("sequence", sequence.to_string()),
                ],
            );
        }
    }

    pub(in crate::compositor) fn complete_presentation_feedback_batch(
        &mut self,
        batch_id: PresentationFeedbackBatchId,
        presentation: FramePresentation,
    ) {
        let Some(batch) = self.presentation_feedback_batches.remove(&batch_id) else {
            // Shutdown can discard feedback globally before a late physical
            // completion is observed.  That feedback already has its
            // terminal Discarded result; never manufacture Presented or
            // panic while retiring the transaction owner.
            return;
        };
        self.complete_presentation_feedbacks(batch.feedbacks, presentation);
    }

    pub(in crate::compositor) fn complete_direct_presentation_feedbacks(
        &mut self,
        feedbacks: Vec<PendingPresentationFeedback>,
        direct_surface_id: u32,
        direct_lineage: Option<(u64, SurfaceCommitSequence)>,
        presentation: FramePresentation,
    ) {
        let mut direct_feedbacks = Vec::new();
        for pending in feedbacks {
            if pending.surface_id == direct_surface_id
                && direct_lineage.is_none_or(|(generation, commit_sequence)| {
                    pending.surface_presentation_generation == generation
                        && pending.commit_sequence == commit_sequence
                })
            {
                direct_feedbacks.push(pending);
            } else {
                pending.feedback.discarded();
            }
        }
        self.complete_presentation_feedbacks(direct_feedbacks, presentation);
    }

    pub(in crate::compositor) fn take_frame_batch_for_render(
        &mut self,
        frame_id: u64,
    ) -> CompositorFrameBatchId {
        self.take_frame_batch_for_render_inner(frame_id, None)
    }

    pub(in crate::compositor) fn take_frame_batch_for_render_with_presentation_samples(
        &mut self,
        frame_id: u64,
        presentation_samples: impl IntoIterator<Item = SurfacePresentationCommitKey>,
    ) -> CompositorFrameBatchId {
        let presentation_samples = presentation_samples.into_iter().collect::<HashSet<_>>();
        self.take_frame_batch_for_render_inner(frame_id, Some(&presentation_samples))
    }

    fn take_frame_batch_for_render_inner(
        &mut self,
        frame_id: u64,
        presentation_samples: Option<&HashSet<SurfacePresentationCommitKey>>,
    ) -> CompositorFrameBatchId {
        assert!(
            self.frame_batches.len() < 2,
            "compositor frame batch registry exceeds pending plus ready capacity"
        );
        self.next_frame_batch_id = self
            .next_frame_batch_id
            .checked_add(1)
            .expect("compositor frame batch ID overflow");
        let batch_id = CompositorFrameBatchId::new(
            NonZeroU64::new(self.next_frame_batch_id)
                .expect("compositor frame batch IDs start at one"),
        );
        let deferred = std::mem::take(&mut self.deferred_dmabuf_buffer_releases);
        let mut dmabuf_releases_to_complete_on_present = Vec::with_capacity(deferred.len());
        let mut current_deferred = Vec::new();
        for obligation in deferred {
            if self.dmabuf_release_token_is_active(&obligation) {
                current_deferred.push(obligation);
            } else {
                dmabuf_releases_to_complete_on_present.push(obligation);
            }
        }
        self.deferred_dmabuf_buffer_releases = current_deferred;
        dmabuf_releases_to_complete_on_present.append(&mut self.pending_dmabuf_buffer_releases);
        let callbacks = self.take_visible_pending_frame_callbacks();
        let callback_count = callbacks.len();
        let (mut callback_timing, callback_timing_ambiguous) =
            select_callback_timing(callbacks.iter().filter_map(|callback| {
                self.pending_frame_callback_timing
                    .get(&callback.id())
                    .copied()
            }));
        if callback_timing_ambiguous {
            callback_timing = None;
            self.frame_callback_metrics
                .content_callback_attribution_ambiguous = self
                .frame_callback_metrics
                .content_callback_attribution_ambiguous
                .saturating_add(1);
        }
        let callback_commit_ns = callback_timing.map(|timing| timing.commit_ns);
        if callback_count > 0 {
            self.frame_callback_metrics.callbacks_captured = self
                .frame_callback_metrics
                .callbacks_captured
                .saturating_add(callback_count as u64);
            self.frame_callback_metrics.last_callback_capture_batch_id = Some(batch_id.get());
            client_pacing_log(
                "frame_callbacks_captured",
                &[
                    ("frame_batch_id", batch_id.get().to_string()),
                    ("frame_id", frame_id.to_string()),
                    ("count", callback_count.to_string()),
                    (
                        "callback_commit_ns",
                        callback_commit_ns.unwrap_or_default().to_string(),
                    ),
                ],
            );
        }
        let captured_releases = dmabuf_releases_to_complete_on_present.len();
        let active_scene_surface_ids = self
            .active_scene_surfaces()
            .iter()
            .map(|surface| surface.surface_id)
            .chain(self.client_cursor_surfaces.keys().copied())
            .collect::<Vec<_>>();
        let fifo_barrier_claims =
            self.fifo_claims_for_frame(active_scene_surface_ids.iter().copied());
        let commit_timing_target_claims =
            self.commit_timing_claims_for_frame(active_scene_surface_ids.iter().copied());
        let presentation_feedbacks = match presentation_samples {
            Some(samples) => self.take_presentation_feedbacks_for_samples(samples),
            None => self.take_frame_eligible_pending_presentation_feedbacks(),
        };
        self.buffer_release_metrics.buffer_releases_captured = self
            .buffer_release_metrics
            .buffer_releases_captured
            .saturating_add(captured_releases as u64);
        client_pacing_log(
            "buffer_releases_captured",
            &[
                ("frame_batch_id", batch_id.get().to_string()),
                ("frame_id", frame_id.to_string()),
                ("count", captured_releases.to_string()),
                (
                    "dmabuf_count",
                    dmabuf_releases_to_complete_on_present.len().to_string(),
                ),
            ],
        );
        let previous = self.frame_batches.insert(
            batch_id,
            CompositorFrameBatch {
                frame_id,
                callbacks,
                callback_timing,
                callback_commit_ns,
                callback_render_completed_ns: None,
                callback_admission_ns: None,
                callback_pacing_state: FrameCallbackPacingState::Captured,
                callback_settlement: FrameCallbackSettlement::new(callback_count),
                callback_terminal_ownership_checked: false,
                presentation_samples_bound: presentation_samples.is_some(),
                presentation_feedbacks,
                dmabuf_releases_to_complete_on_present,
                fifo_barrier_claims,
                commit_timing_target_claims,
                surface_damage: None,
            },
        );
        assert!(previous.is_none(), "compositor frame batch ID was reused");
        self.trace_surface_pipeline_active_surfaces(
            SurfacePipelineEvent::FrameBatchBuilt,
            Some(batch_id.get()),
            None,
            None,
            None,
        );
        self.rebuild_scene_work_index();
        batch_id
    }

    #[allow(dead_code)] // Called through the explicit output server API after runtime integration.
    pub(in crate::compositor) fn restore_frame_batch_after_render_failure(
        &mut self,
        batch_id: CompositorFrameBatchId,
    ) {
        let _ = self
            .prepare_terminal_callback_ownership(batch_id, TerminalCallbackDisposition::Retryable);
        let mut batch = self
            .frame_batches
            .remove(&batch_id)
            .expect("missing compositor frame batch on render failure");
        self.requeue_frame_callbacks_after_restore(batch.callbacks);
        self.requeue_presentation_feedbacks_after_restore(batch.presentation_feedbacks);
        let restored_dmabuf = batch.dmabuf_releases_to_complete_on_present.len();
        batch
            .dmabuf_releases_to_complete_on_present
            .append(&mut self.pending_dmabuf_buffer_releases);
        self.pending_dmabuf_buffer_releases = batch.dmabuf_releases_to_complete_on_present;
        self.note_buffer_releases_restored(batch_id, restored_dmabuf);
        self.clear_legacy_batch_reference(batch_id);
        self.rebuild_scene_work_index();
    }

    pub(in crate::compositor) fn frame_batch_dmabuf_release_count(
        &self,
        batch_id: CompositorFrameBatchId,
    ) -> usize {
        self.frame_batches.get(&batch_id).map_or(0, |batch| {
            batch.dmabuf_releases_to_complete_on_present.len()
        })
    }

    pub(in crate::compositor) fn pending_dmabuf_release_count(&self) -> usize {
        self.pending_dmabuf_buffer_releases.len() + self.deferred_dmabuf_buffer_releases.len()
    }

    pub(in crate::compositor) fn explicit_release_signal_retry_count(&self) -> usize {
        self.explicit_release_signal_retries.len()
    }

    pub(in crate::compositor) fn deferred_dmabuf_release_count(&self) -> usize {
        self.deferred_dmabuf_buffer_releases.len()
    }

    pub(in crate::compositor) fn retryable_deferred_dmabuf_release_count(&self) -> usize {
        self.deferred_dmabuf_buffer_releases
            .iter()
            .filter(|obligation| !self.dmabuf_release_token_is_active(obligation))
            .count()
    }

    pub(in crate::compositor) fn transfer_deferred_dmabuf_releases_to_gpu_lease(
        &mut self,
        lease_id: DmabufGpuReleaseLeaseId,
    ) -> usize {
        let deferred = std::mem::take(&mut self.deferred_dmabuf_buffer_releases);
        let mut obligations = Vec::with_capacity(deferred.len());
        let mut current = Vec::new();
        for obligation in deferred {
            if self.dmabuf_release_token_is_active(&obligation) {
                current.push(obligation);
            } else {
                obligations.push(obligation);
            }
        }
        self.deferred_dmabuf_buffer_releases = current;
        let count = obligations.len();
        if count > 0 {
            let previous = self.dmabuf_gpu_release_leases.insert(
                lease_id,
                DmabufGpuReleaseLease {
                    source_batch_id: None,
                    obligations,
                },
            );
            assert!(
                previous.is_none(),
                "DMA-BUF GPU release lease ID was reused"
            );
        }
        count
    }

    pub(in crate::compositor) fn transfer_frame_batch_dmabuf_releases_to_gpu_lease(
        &mut self,
        batch_id: CompositorFrameBatchId,
        lease_id: DmabufGpuReleaseLeaseId,
    ) -> usize {
        let batch = self
            .frame_batches
            .get_mut(&batch_id)
            .expect("missing compositor frame batch for DMA-BUF GPU release transfer");
        let obligations = std::mem::take(&mut batch.dmabuf_releases_to_complete_on_present);
        let count = obligations.len();
        if count > 0 {
            let previous = self.dmabuf_gpu_release_leases.insert(
                lease_id,
                DmabufGpuReleaseLease {
                    source_batch_id: Some(batch_id),
                    obligations,
                },
            );
            assert!(
                previous.is_none(),
                "DMA-BUF GPU release lease ID was reused"
            );
        }
        count
    }

    pub(in crate::compositor) fn transfer_frame_batch_dmabuf_releases_to_pending_gpu_lease(
        &mut self,
        batch_id: CompositorFrameBatchId,
        lease_id: DmabufGpuReleaseLeaseId,
    ) -> usize {
        let batch = self
            .frame_batches
            .get_mut(&batch_id)
            .expect("missing compositor frame batch for DMA-BUF release-only transfer");
        let obligations = std::mem::take(&mut batch.dmabuf_releases_to_complete_on_present);
        let count = obligations.len();
        if count > 0 {
            let previous = self.dmabuf_gpu_release_leases.insert(
                lease_id,
                DmabufGpuReleaseLease {
                    source_batch_id: None,
                    obligations,
                },
            );
            assert!(
                previous.is_none(),
                "DMA-BUF GPU release lease ID was reused"
            );
        }
        count
    }

    pub(in crate::compositor) fn requeue_dmabuf_gpu_release_lease(
        &mut self,
        lease_id: DmabufGpuReleaseLeaseId,
    ) -> usize {
        let Some(lease) = self.dmabuf_gpu_release_leases.remove(&lease_id) else {
            return 0;
        };
        let count = lease.obligations.len();
        if let Some(batch_id) = lease.source_batch_id
            && let Some(batch) = self.frame_batches.get_mut(&batch_id)
        {
            batch
                .dmabuf_releases_to_complete_on_present
                .extend(lease.obligations);
        } else {
            for obligation in lease.obligations {
                self.push_deferred_dmabuf_release(obligation);
            }
        }
        count
    }

    pub(in crate::compositor) fn complete_dmabuf_gpu_release_lease(
        &mut self,
        lease_id: DmabufGpuReleaseLeaseId,
    ) -> usize {
        let Some(lease) = self.dmabuf_gpu_release_leases.remove(&lease_id) else {
            return 0;
        };
        let mut completed = 0;
        for obligation in lease.obligations {
            if matches!(
                self.complete_dmabuf_release_if_inactive(
                    CompositorFrameBatchId::for_shutdown(),
                    0,
                    obligation,
                ),
                DmabufReleaseCompletion::Completed
            ) {
                completed += 1;
            }
        }
        completed
    }

    pub(in crate::compositor) fn defer_frame_batch_releases(
        &mut self,
        batch_id: CompositorFrameBatchId,
        mut batch: CompositorFrameBatch,
    ) -> CompositorFrameBatch {
        let deferred = std::mem::take(&mut batch.dmabuf_releases_to_complete_on_present);
        let deferred_count = deferred.len();
        if !deferred.is_empty() {
            for obligation in deferred {
                self.push_deferred_dmabuf_release(obligation);
            }
            client_pacing_log(
                "buffer_releases_deferred_without_gpu_proof",
                &[
                    ("frame_batch_id", batch_id.get().to_string()),
                    ("count", deferred_count.to_string()),
                ],
            );
        }
        batch
    }

    pub(in crate::compositor) fn discard_frame_batch(
        &mut self,
        batch_id: CompositorFrameBatchId,
        reason: FrameBatchDiscardReason,
    ) {
        let _ = self
            .prepare_terminal_callback_ownership(batch_id, TerminalCallbackDisposition::Cancelled);
        let batch = self
            .frame_batches
            .remove(&batch_id)
            .expect("missing compositor frame batch on discard");
        let mut batch = batch;
        batch.callback_pacing_state = FrameCallbackPacingState::Completed;
        for claim in &batch.commit_timing_target_claims {
            self.discard_commit_timing_claim(*claim);
        }
        let callback_count = batch.callbacks.len();
        if callback_count > 0 {
            self.frame_callback_metrics
                .callbacks_completed_after_abandonment = self
                .frame_callback_metrics
                .callbacks_completed_after_abandonment
                .saturating_add(callback_count as u64);
            if batch.callback_render_completed_ns.is_some() {
                self.frame_callback_metrics
                    .callbacks_in_discarded_rendered_batches = self
                    .frame_callback_metrics
                    .callbacks_in_discarded_rendered_batches
                    .saturating_add(callback_count as u64);
            }
        }
        for pending in std::mem::take(&mut batch.presentation_feedbacks) {
            pending.feedback.discarded();
        }
        self.complete_frame_callbacks(std::mem::take(&mut batch.callbacks));
        let frame_id = batch.frame_id;
        let release_count = batch.dmabuf_releases_to_complete_on_present.len();
        self.retired_frame_batches.insert(batch_id, batch);
        self.rebuild_scene_work_index();
        client_pacing_log(
            "buffer_releases_retired",
            &[
                ("frame_batch_id", batch_id.get().to_string()),
                ("frame_id", frame_id.to_string()),
                ("count", release_count.to_string()),
                ("reason", format!("{reason:?}")),
            ],
        );
        self.clear_legacy_batch_reference(batch_id);
    }

    pub(in crate::compositor) fn complete_frame_batch_after_safe_abandonment(
        &mut self,
        batch_id: CompositorFrameBatchId,
        reason: FrameBatchDiscardReason,
    ) {
        let _ = self
            .prepare_terminal_callback_ownership(batch_id, TerminalCallbackDisposition::Cancelled);
        let batch = self
            .frame_batches
            .remove(&batch_id)
            .or_else(|| self.retired_frame_batches.remove(&batch_id))
            .expect("missing compositor frame batch after safe abandonment");
        let mut batch = batch;
        batch.callback_pacing_state = FrameCallbackPacingState::Completed;
        for claim in &batch.commit_timing_target_claims {
            self.discard_commit_timing_claim(*claim);
        }
        let frame_id = batch.frame_id;
        let callback_count = batch.callbacks.len();
        if callback_count > 0 {
            self.frame_callback_metrics
                .callbacks_completed_after_abandonment = self
                .frame_callback_metrics
                .callbacks_completed_after_abandonment
                .saturating_add(callback_count as u64);
        }
        let batch = self.complete_frame_batch_releases(batch_id, batch);
        for pending in batch.presentation_feedbacks {
            pending.feedback.discarded();
        }
        self.complete_frame_callbacks(batch.callbacks);
        self.clear_legacy_batch_reference(batch_id);
        self.rebuild_scene_work_index();
        client_pacing_log(
            "buffer_releases_completed_after_abandonment",
            &[
                ("frame_batch_id", batch_id.get().to_string()),
                ("frame_id", frame_id.to_string()),
                ("reason", format!("{reason:?}")),
            ],
        );
    }

    pub(in crate::compositor) fn complete_presented_frame_batch(
        &mut self,
        frame_id: u64,
        batch_id: CompositorFrameBatchId,
        presentation: FramePresentation,
    ) {
        self.assert_frame_batch_identity(frame_id, batch_id);
        let (render_completed_ns, callbacks_remaining) = self
            .frame_batches
            .get(&batch_id)
            .map(|batch| (batch.callback_render_completed_ns, batch.callbacks.len()))
            .expect("missing compositor frame batch at presentation");
        self.note_frame_callbacks_at_pageflip(batch_id, render_completed_ns, callbacks_remaining);
        self.complete_frame_callbacks_at_presentation_fallback(batch_id);
        let batch = self.take_presented_frame_batch(frame_id, batch_id);
        if !matches!(presentation.kind, PresentationKind::Tearing) {
            for claim in &batch.fifo_barrier_claims {
                self.clear_fifo_barrier_claim(*claim, FifoBarrierClearReason::Presented);
            }
        }
        for claim in &batch.commit_timing_target_claims {
            self.complete_commit_timing_claim(*claim, presentation);
        }
        let surface_damage = batch.surface_damage.clone();
        let batch = self.complete_frame_batch_releases(batch_id, batch);
        if let Some(surface_damage) = surface_damage {
            self.commit_surface_damage_presented(surface_damage);
        }
        self.clear_legacy_batch_reference(batch_id);
        self.complete_presentation_feedbacks(batch.presentation_feedbacks, presentation);
    }

    pub(in crate::compositor) fn set_frame_batch_surface_damage(
        &mut self,
        batch_id: CompositorFrameBatchId,
        surface_damage: SurfaceDamagePresentation,
    ) {
        let batch = self
            .frame_batches
            .get_mut(&batch_id)
            .expect("missing compositor frame batch for surface damage ownership");
        assert!(
            batch.surface_damage.is_none(),
            "compositor frame batch surface damage ownership was replaced"
        );
        batch.surface_damage = Some(surface_damage);
    }

    pub(in crate::compositor) fn assert_frame_batch_identity(
        &self,
        frame_id: u64,
        batch_id: CompositorFrameBatchId,
    ) {
        let registered_frame_id = self
            .frame_batches
            .get(&batch_id)
            .expect("missing compositor frame batch on presentation")
            .frame_id;
        assert_eq!(
            registered_frame_id, frame_id,
            "pageflip frame ID does not own the compositor frame batch"
        );
    }

    #[cfg(test)]
    pub(in crate::compositor) fn test_frame_batch_presentation_surface_ids(
        &self,
        batch_id: CompositorFrameBatchId,
    ) -> Vec<u32> {
        self.frame_batches
            .get(&batch_id)
            .map(|batch| {
                batch
                    .presentation_feedbacks
                    .iter()
                    .map(|feedback| feedback.surface_id)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(in crate::compositor) fn take_presented_frame_batch(
        &mut self,
        frame_id: u64,
        batch_id: CompositorFrameBatchId,
    ) -> CompositorFrameBatch {
        self.assert_frame_batch_identity(frame_id, batch_id);
        self.frame_batches
            .remove(&batch_id)
            .expect("compositor frame batch disappeared during completion")
    }

    pub(in crate::compositor) fn frame_callback_timing_for_batch(
        &self,
        batch_id: CompositorFrameBatchId,
    ) -> Option<FrameCallbackTimingEvidence> {
        self.frame_batches
            .get(&batch_id)
            .and_then(|batch| batch.callback_timing)
    }

    pub(in crate::compositor) fn clear_legacy_batch_reference(
        &mut self,
        batch_id: CompositorFrameBatchId,
    ) {
        if self.legacy_prepared_frame_batch == Some(batch_id) {
            self.legacy_prepared_frame_batch = None;
        }
        if self.legacy_submitted_frame_batch == Some(batch_id) {
            self.legacy_submitted_frame_batch = None;
        }
    }

    pub(in crate::compositor) fn discard_pending_presentation_feedbacks_for_surface(
        &mut self,
        surface_id: u32,
    ) {
        fn discard_surface(feedbacks: &mut Vec<PendingPresentationFeedback>, surface_id: u32) {
            feedbacks.retain(|pending| {
                if pending.surface_id == surface_id {
                    pending.feedback.discarded();
                    false
                } else {
                    true
                }
            });
        }
        let before = self.frame_eligible_pending_presentation_feedbacks.len();
        discard_surface(
            &mut self.frame_eligible_pending_presentation_feedbacks,
            surface_id,
        );
        discard_surface(&mut self.pending_presentation_feedbacks, surface_id);
        self.frame_eligible_pending_presentation_feedback_count = self
            .frame_eligible_pending_presentation_feedback_count
            .saturating_sub(
                before.saturating_sub(self.frame_eligible_pending_presentation_feedbacks.len()),
            );
        for batch in self.frame_batches.values_mut() {
            discard_surface(&mut batch.presentation_feedbacks, surface_id);
        }
        for batch in self.presentation_feedback_batches.values_mut() {
            discard_surface(&mut batch.feedbacks, surface_id);
        }
    }

    pub(in crate::compositor) fn discard_all_pending_presentation_feedbacks(&mut self) {
        for pending in std::mem::take(&mut self.frame_eligible_pending_presentation_feedbacks)
            .into_iter()
            .chain(std::mem::take(&mut self.pending_presentation_feedbacks))
        {
            pending.feedback.discarded();
        }
        self.frame_eligible_pending_presentation_feedback_count = 0;
        for batch in self.frame_batches.values_mut() {
            for pending in std::mem::take(&mut batch.presentation_feedbacks) {
                pending.feedback.discarded();
            }
        }
        for batch in std::mem::take(&mut self.presentation_feedback_batches).into_values() {
            for pending in batch.feedbacks {
                pending.feedback.discarded();
            }
        }
        for feedbacks in
            std::mem::take(&mut self.pending_surface_presentation_feedbacks).into_values()
        {
            for pending in feedbacks {
                pending.feedback.discarded();
            }
        }
    }

    pub(in crate::compositor) fn complete_frame_batch_releases(
        &mut self,
        batch_id: CompositorFrameBatchId,
        mut batch: CompositorFrameBatch,
    ) -> CompositorFrameBatch {
        let frame_id = batch.frame_id;
        let dmabuf_releases = std::mem::take(&mut batch.dmabuf_releases_to_complete_on_present);
        client_pacing_log(
            "buffer_releases_completed_on_present",
            &[
                ("frame_batch_id", batch_id.get().to_string()),
                ("frame_id", frame_id.to_string()),
                ("count", dmabuf_releases.len().to_string()),
            ],
        );
        for obligation in dmabuf_releases {
            let _ = self.complete_dmabuf_release_if_inactive(batch_id, frame_id, obligation);
        }
        batch
    }

    pub(in crate::compositor) fn complete_materialized_shm_release(
        &mut self,
        release: SafeShmRelease,
    ) {
        let buffer = release.into_buffer();
        if !buffer.is_alive() {
            self.buffer_release_metrics.buffer_releases_discarded = self
                .buffer_release_metrics
                .buffer_releases_discarded
                .saturating_add(1);
            client_pacing_log(
                "buffer_release_scrubbed",
                &[
                    ("buffer", format!("{:?}", buffer.id())),
                    ("outcome", "dead_resource".to_string()),
                ],
            );
            return;
        }
        match buffer.send_event(wl_buffer::Event::Release) {
            Ok(()) => {
                self.buffer_release_metrics.buffer_releases_completed = self
                    .buffer_release_metrics
                    .buffer_releases_completed
                    .saturating_add(1);
                client_pacing_log(
                    "buffer_release_completed",
                    &[
                        ("buffer", format!("{:?}", buffer.id())),
                        ("kind", "shm".to_string()),
                    ],
                );
            }
            Err(_) => {
                self.buffer_release_metrics.buffer_releases_discarded = self
                    .buffer_release_metrics
                    .buffer_releases_discarded
                    .saturating_add(1);
                client_pacing_log(
                    "buffer_release_scrubbed",
                    &[
                        ("buffer", format!("{:?}", buffer.id())),
                        ("outcome", "send_failed".to_string()),
                    ],
                );
            }
        }
    }

    pub(in crate::compositor) fn complete_dmabuf_release(
        &mut self,
        batch_id: CompositorFrameBatchId,
        frame_id: u64,
        obligation: DmabufReleaseObligation,
    ) -> DmabufReleaseCompletion {
        let buffer_id = obligation.buffer_id;
        match obligation.release.release() {
            SurfaceBufferReleaseOutcome::Completed => {
                self.buffer_release_metrics.buffer_releases_completed = self
                    .buffer_release_metrics
                    .buffer_releases_completed
                    .saturating_add(1);
                self.trace_surface_pipeline_buffer_release(
                    buffer_id.get(),
                    Some(batch_id.get()),
                    Some(frame_id),
                );
                client_pacing_log(
                    "buffer_release_completed",
                    &[
                        ("frame_batch_id", batch_id.get().to_string()),
                        ("frame_id", frame_id.to_string()),
                        ("buffer_id", buffer_id.get().to_string()),
                        ("kind", "dmabuf".to_string()),
                    ],
                );
                DmabufReleaseCompletion::Completed
            }
            SurfaceBufferReleaseOutcome::Discarded => {
                self.buffer_release_metrics.buffer_releases_discarded = self
                    .buffer_release_metrics
                    .buffer_releases_discarded
                    .saturating_add(1);
                client_pacing_log(
                    "buffer_release_scrubbed",
                    &[
                        ("frame_batch_id", batch_id.get().to_string()),
                        ("frame_id", frame_id.to_string()),
                        ("buffer_id", buffer_id.get().to_string()),
                        ("kind", "dmabuf".to_string()),
                        ("outcome", "terminal_resource_failure".to_string()),
                    ],
                );
                DmabufReleaseCompletion::Discarded
            }
            SurfaceBufferReleaseOutcome::ExplicitSyncFailed(point) => {
                self.buffer_release_metrics.explicit_release_signal_failures = self
                    .buffer_release_metrics
                    .explicit_release_signal_failures
                    .saturating_add(1);
                self.queue_explicit_release_signal_retry(DmabufReleaseObligation {
                    buffer_id,
                    release: SurfaceBufferRelease::ExplicitSync(point),
                });
                client_pacing_log(
                    "buffer_release_signal_failed",
                    &[
                        ("frame_batch_id", batch_id.get().to_string()),
                        ("frame_id", frame_id.to_string()),
                        ("buffer_id", buffer_id.get().to_string()),
                        ("kind", "dmabuf".to_string()),
                    ],
                );
                DmabufReleaseCompletion::SignalRetry
            }
        }
    }

    pub(in crate::compositor) fn dmabuf_release_token_is_active(
        &self,
        obligation: &DmabufReleaseObligation,
    ) -> bool {
        self.active_dmabuf_buffers
            .values()
            .any(|active| active.same_release_token(obligation))
    }

    pub(in crate::compositor) fn complete_dmabuf_release_if_inactive(
        &mut self,
        batch_id: CompositorFrameBatchId,
        frame_id: u64,
        obligation: DmabufReleaseObligation,
    ) -> DmabufReleaseCompletion {
        self.buffer_release_metrics
            .dmabuf_release_terminal_revalidated = self
            .buffer_release_metrics
            .dmabuf_release_terminal_revalidated
            .saturating_add(1);
        if self.dmabuf_release_token_is_active(&obligation) {
            self.push_deferred_dmabuf_release(obligation);
            self.buffer_release_metrics
                .dmabuf_release_terminal_requeued_current = self
                .buffer_release_metrics
                .dmabuf_release_terminal_requeued_current
                .saturating_add(1);
            DmabufReleaseCompletion::DeferredCurrent
        } else {
            self.complete_dmabuf_release(batch_id, frame_id, obligation)
        }
    }

    fn push_deferred_dmabuf_release(&mut self, obligation: DmabufReleaseObligation) {
        if self
            .deferred_dmabuf_buffer_releases
            .iter()
            .any(|existing| existing.same_release_token(&obligation))
        {
            self.note_buffer_release_duplicate_attempt();
            return;
        }
        self.deferred_dmabuf_buffer_releases.push(obligation);
    }

    fn queue_explicit_release_signal_retry(&mut self, obligation: DmabufReleaseObligation) {
        debug_assert!(matches!(
            obligation.release,
            SurfaceBufferRelease::ExplicitSync(_)
        ));
        if self
            .explicit_release_signal_retries
            .iter()
            .any(|existing| existing.same_release_token(&obligation))
        {
            self.note_buffer_release_duplicate_attempt();
            return;
        }
        self.explicit_release_signal_retries.push(obligation);
        self.buffer_release_metrics.explicit_release_signal_retries = self
            .buffer_release_metrics
            .explicit_release_signal_retries
            .saturating_add(1);
    }

    pub(in crate::compositor) fn reclassify_reactivated_dmabuf_release(
        &mut self,
        obligation: &DmabufReleaseObligation,
    ) -> bool {
        let Some(index) = self
            .explicit_release_signal_retries
            .iter()
            .position(|existing| existing.same_release_token(obligation))
        else {
            return false;
        };
        self.explicit_release_signal_retries.remove(index);
        self.push_deferred_dmabuf_release(obligation.clone());
        self.buffer_release_metrics
            .explicit_release_signal_retry_requeued_current = self
            .buffer_release_metrics
            .explicit_release_signal_retry_requeued_current
            .saturating_add(1);
        true
    }

    pub(in crate::compositor) fn service_explicit_release_signal_retries(
        &mut self,
    ) -> ExplicitReleaseSignalRetryResult {
        let retries = std::mem::take(&mut self.explicit_release_signal_retries);
        let mut result = ExplicitReleaseSignalRetryResult::default();
        for obligation in retries {
            if self.dmabuf_release_token_is_active(&obligation) {
                self.push_deferred_dmabuf_release(obligation);
                result.requeued_current = result.requeued_current.saturating_add(1);
                self.buffer_release_metrics
                    .explicit_release_signal_retry_requeued_current = self
                    .buffer_release_metrics
                    .explicit_release_signal_retry_requeued_current
                    .saturating_add(1);
                continue;
            }
            result.attempted = result.attempted.saturating_add(1);
            match self.complete_dmabuf_release(
                CompositorFrameBatchId::for_shutdown(),
                0,
                obligation,
            ) {
                DmabufReleaseCompletion::Completed => {
                    result.completed = result.completed.saturating_add(1);
                    self.buffer_release_metrics
                        .explicit_release_signal_retry_successes = self
                        .buffer_release_metrics
                        .explicit_release_signal_retry_successes
                        .saturating_add(1);
                }
                DmabufReleaseCompletion::SignalRetry => {
                    result.failed = result.failed.saturating_add(1);
                }
                DmabufReleaseCompletion::Discarded | DmabufReleaseCompletion::DeferredCurrent => {}
            }
        }
        result.remaining = self.explicit_release_signal_retries.len();
        result
    }

    #[cfg(test)]
    pub(in crate::compositor) fn release_client_buffers_for_shutdown(&mut self) {
        let mut releases = ShutdownDmabufReleaseSet::default();
        self.release_client_buffers_for_shutdown_with(&mut releases);
        releases.complete(self);
    }

    pub(in crate::compositor) fn release_client_buffers_for_shutdown_with(
        &mut self,
        releases: &mut ShutdownDmabufReleaseSet,
    ) {
        for batch_id in self.frame_batches.keys().copied().collect::<Vec<_>>() {
            let mut batch = self
                .frame_batches
                .remove(&batch_id)
                .expect("frame batch disappeared during shutdown release");
            for claim in &batch.commit_timing_target_claims {
                self.discard_commit_timing_claim(*claim);
            }
            for pending in std::mem::take(&mut batch.presentation_feedbacks) {
                pending.feedback.discarded();
            }
            self.complete_frame_callbacks(std::mem::take(&mut batch.callbacks));
            let dmabuf_releases = std::mem::take(&mut batch.dmabuf_releases_to_complete_on_present);
            for obligation in dmabuf_releases {
                releases.push(obligation);
            }
        }
        for batch_id in self
            .retired_frame_batches
            .keys()
            .copied()
            .collect::<Vec<_>>()
        {
            let mut batch = self
                .retired_frame_batches
                .remove(&batch_id)
                .expect("retired frame batch disappeared during shutdown release");
            for claim in &batch.commit_timing_target_claims {
                self.discard_commit_timing_claim(*claim);
            }
            for pending in std::mem::take(&mut batch.presentation_feedbacks) {
                pending.feedback.discarded();
            }
            self.complete_frame_callbacks(std::mem::take(&mut batch.callbacks));
            let dmabuf_releases = std::mem::take(&mut batch.dmabuf_releases_to_complete_on_present);
            for obligation in dmabuf_releases {
                releases.push(obligation);
            }
        }
        self.legacy_prepared_frame_batch = None;
        self.legacy_submitted_frame_batch = None;

        let deferred_dmabuf = std::mem::take(&mut self.deferred_dmabuf_buffer_releases);
        for obligation in deferred_dmabuf {
            releases.push(obligation);
        }
        let signal_retries = std::mem::take(&mut self.explicit_release_signal_retries);
        for obligation in signal_retries {
            releases.push(obligation);
        }
        let pending_dmabuf = std::mem::take(&mut self.pending_dmabuf_buffer_releases);
        for obligation in pending_dmabuf {
            releases.push(obligation);
        }

        let gpu_leases = std::mem::take(&mut self.dmabuf_gpu_release_leases);
        for (_, lease) in gpu_leases {
            for obligation in lease.obligations {
                releases.push(obligation);
            }
        }

        let mut active_dmabuf = std::mem::take(&mut self.active_dmabuf_buffers);
        for (surface_id, current) in std::mem::take(&mut self.current_surface_buffers) {
            let CurrentSurfaceBuffer::Unmaterialized(pending) = current else {
                continue;
            };
            if pending.data.is_shm() {
                self.release_unmaterialized_pending_buffer(pending, false);
            } else if let Some(release) = active_dmabuf.remove(&surface_id) {
                releases.push(release);
            } else {
                releases.push(DmabufReleaseObligation {
                    buffer_id: pending.data.buffer_id(),
                    release: pending.release_target(),
                });
            }
        }
        for (_, obligation) in active_dmabuf {
            releases.push(obligation);
        }

        let (window_exits, prepared_window_exits) = self.window_exit_payloads.drain_all();
        for (identity, mut payload) in window_exits {
            if let Some(revisions) = payload.property_revisions {
                let cancelled = self.presentation_animator.cancel_property_pair_exact(
                    identity.scene_node_id(),
                    revisions.transaction_id,
                    revisions.geometry_revision_id,
                    revisions.opacity_revision_id,
                );
                if !cancelled {
                    self.presentation_animator.cancel_transaction_exact(
                        identity.scene_node_id(),
                        revisions.transaction_id,
                    );
                }
            }
            let _ = self
                .presentation_animator
                .retire_active_retained_visual_exact(identity);
            let _ = self
                .presentation_animator
                .retire_retained_visual_exact(identity);
            for held in payload.take_held_release_obligations() {
                releases.push(held.obligation);
            }
        }
        for prepared in prepared_window_exits {
            for held in prepared.held_release_obligations {
                releases.push(held.obligation);
            }
        }
    }

    pub(in crate::compositor) fn note_buffer_releases_restored(
        &mut self,
        batch_id: CompositorFrameBatchId,
        count: usize,
    ) {
        self.buffer_release_metrics.buffer_releases_restored = self
            .buffer_release_metrics
            .buffer_releases_restored
            .saturating_add(count as u64);
        client_pacing_log(
            "buffer_releases_restored",
            &[
                ("frame_batch_id", batch_id.get().to_string()),
                ("count", count.to_string()),
            ],
        );
    }

    pub(in crate::compositor) fn buffer_release_is_owned(
        &self,
        candidate: &DmabufReleaseObligation,
    ) -> bool {
        let same = |obligation: &DmabufReleaseObligation| obligation.same_release_token(candidate);
        self.pending_dmabuf_buffer_releases.iter().any(same)
            || self.deferred_dmabuf_buffer_releases.iter().any(same)
            || self.explicit_release_signal_retries.iter().any(same)
            || self.frame_batches.values().any(|batch| {
                batch
                    .dmabuf_releases_to_complete_on_present
                    .iter()
                    .any(same)
            })
            || self.retired_frame_batches.values().any(|batch| {
                batch
                    .dmabuf_releases_to_complete_on_present
                    .iter()
                    .any(same)
            })
            || self
                .dmabuf_gpu_release_leases
                .values()
                .any(|lease| lease.obligations.iter().any(same))
            || self
                .window_exit_payloads
                .held_release_obligations()
                .any(same)
    }

    pub(in crate::compositor) fn note_buffer_release_duplicate_attempt(&mut self) {
        self.buffer_release_metrics
            .buffer_release_duplicate_attempts = self
            .buffer_release_metrics
            .buffer_release_duplicate_attempts
            .saturating_add(1);
        client_pacing_log(
            "buffer_release_duplicate_attempt",
            &[("count", "1".to_string())],
        );
    }

    pub(in crate::compositor) fn scrub_dead_buffer_releases(&mut self) {
        let mut discarded = 0u64;
        self.pending_dmabuf_buffer_releases.retain(|obligation| {
            let alive = match &obligation.release {
                SurfaceBufferRelease::WlBuffer(buffer) => buffer.is_alive(),
                SurfaceBufferRelease::ExplicitSync(_) => true,
            };
            if !alive {
                discarded = discarded.saturating_add(1);
            }
            alive
        });
        self.deferred_dmabuf_buffer_releases.retain(|obligation| {
            let alive = match &obligation.release {
                SurfaceBufferRelease::WlBuffer(buffer) => buffer.is_alive(),
                SurfaceBufferRelease::ExplicitSync(_) => true,
            };
            if !alive {
                discarded = discarded.saturating_add(1);
            }
            alive
        });
        self.explicit_release_signal_retries.retain(|obligation| {
            let alive = match &obligation.release {
                SurfaceBufferRelease::WlBuffer(buffer) => buffer.is_alive(),
                SurfaceBufferRelease::ExplicitSync(_) => true,
            };
            if !alive {
                discarded = discarded.saturating_add(1);
            }
            alive
        });
        for batch in self.frame_batches.values_mut() {
            batch
                .dmabuf_releases_to_complete_on_present
                .retain(|obligation| {
                    let alive = match &obligation.release {
                        SurfaceBufferRelease::WlBuffer(buffer) => buffer.is_alive(),
                        SurfaceBufferRelease::ExplicitSync(_) => true,
                    };
                    if !alive {
                        discarded = discarded.saturating_add(1);
                    }
                    alive
                });
        }
        for batch in self.retired_frame_batches.values_mut() {
            batch
                .dmabuf_releases_to_complete_on_present
                .retain(|obligation| {
                    let alive = match &obligation.release {
                        SurfaceBufferRelease::WlBuffer(buffer) => buffer.is_alive(),
                        SurfaceBufferRelease::ExplicitSync(_) => true,
                    };
                    if !alive {
                        discarded = discarded.saturating_add(1);
                    }
                    alive
                });
        }
        for lease in self.dmabuf_gpu_release_leases.values_mut() {
            lease.obligations.retain(|obligation| {
                let alive = match &obligation.release {
                    SurfaceBufferRelease::WlBuffer(buffer) => buffer.is_alive(),
                    SurfaceBufferRelease::ExplicitSync(_) => true,
                };
                if !alive {
                    discarded = discarded.saturating_add(1);
                }
                alive
            });
        }
        self.buffer_release_metrics.buffer_releases_discarded = self
            .buffer_release_metrics
            .buffer_releases_discarded
            .saturating_add(discarded);
        if discarded > 0 {
            client_pacing_log(
                "buffer_releases_scrubbed",
                &[("count", discarded.to_string())],
            );
        }
    }

    pub(in crate::compositor) fn complete_frame_callbacks(
        &mut self,
        callbacks: Vec<wl_callback::WlCallback>,
    ) {
        for callback in &callbacks {
            self.pending_frame_callback_surfaces.remove(&callback.id());
            self.pending_frame_callback_timing.remove(&callback.id());
        }
        let callbacks: Vec<_> = callbacks
            .into_iter()
            .filter(|callback| callback.is_alive())
            .collect();
        let time = self.frame_callback_time_ms();
        self.complete_frame_callbacks_at_time(callbacks, time);
    }

    pub(in crate::compositor) fn complete_frame_callbacks_at_time(
        &mut self,
        callbacks: Vec<wl_callback::WlCallback>,
        time: u32,
    ) {
        for callback in &callbacks {
            self.pending_frame_callback_surfaces.remove(&callback.id());
            self.pending_frame_callback_timing.remove(&callback.id());
        }
        self.note_callbacks_completed(&callbacks);
        for callback in callbacks {
            client_pacing_log(
                "frame_callback_sent",
                &[
                    ("callback", format!("{:?}", callback.id())),
                    ("callback_data_ms", time.to_string()),
                ],
            );
            let _ = callback.send_event(wl_callback::Event::Done {
                callback_data: time,
            });
        }
    }
}

#[cfg(test)]
mod callback_attribution_tests {
    use super::*;

    fn timing(surface_id: u32, commit_ns: u64) -> FrameCallbackTimingEvidence {
        FrameCallbackTimingEvidence {
            surface_id,
            commit_ns,
            admission_ns: Some(commit_ns.saturating_sub(10)),
            reaction_ns: Some(10),
        }
    }

    #[test]
    fn one_surface_uses_the_latest_commit_for_that_surface() {
        let (selected, ambiguous) = select_callback_timing([timing(7, 100), timing(7, 120)]);

        assert_eq!(selected, Some(timing(7, 120)));
        assert!(!ambiguous);
    }

    #[test]
    fn multiple_surfaces_are_not_attributed_to_an_arbitrary_callback() {
        let (selected, ambiguous) = select_callback_timing([timing(7, 100), timing(8, 120)]);

        assert_eq!(selected, None);
        assert!(ambiguous);
    }
}
