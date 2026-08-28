use std::num::NonZeroU64;

use super::*;
use crate::compositor::frame_batch::FrameCallbackPacingState;

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
        self.subsurface_transaction_metrics
            .bufferless_tree_commits_merged = self
            .subsurface_transaction_metrics
            .bufferless_tree_commits_merged
            .saturating_add((stats.bufferless_nodes == stats.incoming_nodes) as u64);
        self.subsurface_transaction_metrics
            .metadata_only_nodes_merged = self
            .subsurface_transaction_metrics
            .metadata_only_nodes_merged
            .saturating_add(stats.bufferless_nodes as u64);
        self.subsurface_transaction_metrics.attachments_replaced = self
            .subsurface_transaction_metrics
            .attachments_replaced
            .saturating_add(stats.attachments_replaced as u64);
        self.subsurface_transaction_metrics.explicit_detaches = self
            .subsurface_transaction_metrics
            .explicit_detaches
            .saturating_add(stats.explicit_detaches as u64);
        self.subsurface_transaction_metrics
            .acquire_dependencies_preserved = self
            .subsurface_transaction_metrics
            .acquire_dependencies_preserved
            .saturating_add(stats.dependencies_preserved as u64);
        self.subsurface_transaction_metrics
            .acquire_dependencies_replaced = self
            .subsurface_transaction_metrics
            .acquire_dependencies_replaced
            .saturating_add(stats.dependencies_replaced as u64);
        self.subsurface_transaction_metrics.callbacks_merged = self
            .subsurface_transaction_metrics
            .callbacks_merged
            .saturating_add(stats.callbacks_merged as u64);
        self.subsurface_transaction_metrics.feedbacks_merged = self
            .subsurface_transaction_metrics
            .feedbacks_merged
            .saturating_add(stats.feedbacks_merged as u64);
        self.subsurface_transaction_metrics
            .resize_snapshots_preserved = self
            .subsurface_transaction_metrics
            .resize_snapshots_preserved
            .saturating_add(stats.resize_snapshots_preserved as u64);
        self.subsurface_transaction_metrics
            .resize_snapshots_replaced = self
            .subsurface_transaction_metrics
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

    pub(in crate::compositor) fn has_pending_acquire_watch_changes(&self) -> bool {
        !self.pending_acquire_watch_changes.is_empty()
    }

    pub(in crate::compositor) fn has_unowned_frame_work(&self) -> bool {
        self.has_pending_frame_prepare_work()
            || self.has_pending_interactive_visual_work()
            || self.has_unowned_frame_callbacks()
            || self.has_visible_pending_presentation_feedbacks()
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
                self.capture_frame_callbacks_for_render();
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
        } else if let Some(surface_damage) = surface_damage {
            self.commit_surface_damage_no_visual_change(surface_damage);
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

    fn complete_presentation_feedbacks(
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
            for output in self
                .output_resources
                .iter()
                .filter(|output| resource_belongs_to_surface_client(*output, &pending.surface))
            {
                pending.feedback.sync_output(output);
            }
            pending.feedback.presented(
                tv_sec_hi,
                tv_sec_lo,
                timestamp.nanoseconds(),
                self.output_refresh.presentation_refresh_nsec(),
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

    pub(in crate::compositor) fn complete_direct_presentation_feedbacks(
        &mut self,
        feedbacks: Vec<PendingPresentationFeedback>,
        direct_surface_id: u32,
        presentation: FramePresentation,
    ) {
        let mut direct_feedbacks = Vec::new();
        for pending in feedbacks {
            if pending.surface_id == direct_surface_id {
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
        let dmabuf_releases_to_complete_on_present =
            std::mem::take(&mut self.pending_dmabuf_buffer_releases);
        let callbacks = self.take_visible_pending_frame_callbacks();
        let callback_count = callbacks.len();
        let callback_commit_ns = (callback_count > 0).then_some(
            self.frame_callback_metrics
                .last_callback_commit_ns
                .unwrap_or_else(client_pacing_now_ns),
        );
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
        let presentation_feedbacks = self.take_visible_pending_presentation_feedbacks();
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
                callback_commit_ns,
                callback_render_completed_ns: None,
                callback_admission_ns: None,
                callback_pacing_state: FrameCallbackPacingState::Captured,
                callback_settlement: FrameCallbackSettlement::new(callback_count),
                callback_terminal_ownership_checked: false,
                presentation_feedbacks,
                dmabuf_releases_to_complete_on_present,
                fifo_barrier_claims,
                commit_timing_target_claims,
                surface_damage: None,
            },
        );
        assert!(previous.is_none(), "compositor frame batch ID was reused");
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
        let before = self.visible_pending_presentation_feedbacks.len();
        discard_surface(&mut self.visible_pending_presentation_feedbacks, surface_id);
        discard_surface(&mut self.pending_presentation_feedbacks, surface_id);
        self.visible_pending_presentation_feedback_count = self
            .visible_pending_presentation_feedback_count
            .saturating_sub(
                before.saturating_sub(self.visible_pending_presentation_feedbacks.len()),
            );
        for batch in self.frame_batches.values_mut() {
            discard_surface(&mut batch.presentation_feedbacks, surface_id);
        }
    }

    pub(in crate::compositor) fn discard_all_pending_presentation_feedbacks(&mut self) {
        for pending in std::mem::take(&mut self.visible_pending_presentation_feedbacks)
            .into_iter()
            .chain(std::mem::take(&mut self.pending_presentation_feedbacks))
        {
            pending.feedback.discarded();
        }
        self.visible_pending_presentation_feedback_count = 0;
        for batch in self.frame_batches.values_mut() {
            for pending in std::mem::take(&mut batch.presentation_feedbacks) {
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
        for release in dmabuf_releases {
            self.complete_dmabuf_release(batch_id, frame_id, release);
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

    fn complete_dmabuf_release(
        &mut self,
        batch_id: CompositorFrameBatchId,
        frame_id: u64,
        release: SurfaceBufferRelease,
    ) {
        release.release();
        self.buffer_release_metrics.buffer_releases_completed = self
            .buffer_release_metrics
            .buffer_releases_completed
            .saturating_add(1);
        client_pacing_log(
            "buffer_release_completed",
            &[
                ("frame_batch_id", batch_id.get().to_string()),
                ("frame_id", frame_id.to_string()),
                ("kind", "dmabuf".to_string()),
            ],
        );
    }

    pub(in crate::compositor) fn release_client_buffers_for_shutdown(&mut self) {
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
            self.complete_frame_batch_releases(batch_id, batch);
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
            self.complete_frame_batch_releases(batch_id, batch);
        }
        self.legacy_prepared_frame_batch = None;
        self.legacy_submitted_frame_batch = None;

        let pending_dmabuf = std::mem::take(&mut self.pending_dmabuf_buffer_releases);
        for release in pending_dmabuf {
            self.complete_dmabuf_release(CompositorFrameBatchId::for_shutdown(), 0, release);
        }

        let mut active_dmabuf = std::mem::take(&mut self.active_dmabuf_buffers);
        for (surface_id, current) in std::mem::take(&mut self.current_surface_buffers) {
            let CurrentSurfaceBuffer::Unmaterialized(pending) = current else {
                continue;
            };
            if pending.data.is_shm() {
                self.release_unmaterialized_pending_buffer(pending, false);
            } else if let Some(release) = active_dmabuf.remove(&surface_id) {
                self.complete_dmabuf_release(CompositorFrameBatchId::for_shutdown(), 0, release);
            } else {
                self.release_surface_buffer_direct(pending.release_target());
            }
        }
        for (_, release) in active_dmabuf {
            self.complete_dmabuf_release(CompositorFrameBatchId::for_shutdown(), 0, release);
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
        candidate: &SurfaceBufferRelease,
    ) -> bool {
        let same = |release: &SurfaceBufferRelease| release.same_release_token(candidate);
        self.pending_dmabuf_buffer_releases.iter().any(same)
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
        self.pending_dmabuf_buffer_releases.retain(|release| {
            let alive = match release {
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
                .retain(|release| {
                    let alive = match release {
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
                .retain(|release| {
                    let alive = match release {
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

    pub(in crate::compositor) fn cancel_pending_acquire_commits_for_surface(
        &mut self,
        surface_id: u32,
        reason: AcquireWatchCancelReason,
    ) -> Vec<wl_callback::WlCallback> {
        let mut retained = Vec::with_capacity(self.pending_explicit_sync_commits.len());
        let mut canceled_callbacks = Vec::new();
        let mut canceled_resize_captures = Vec::new();
        for commit in std::mem::take(&mut self.pending_explicit_sync_commits) {
            if commit.surface_id == surface_id {
                match reason {
                    AcquireWatchCancelReason::Superseded => self.note_explicit_commit_destroyed(
                        commit.surface_commit_id,
                        "superseded_without_replacement_identity",
                    ),
                    AcquireWatchCancelReason::Rejected => self.note_explicit_commit_rejected(
                        commit.surface_commit_id,
                        "acquire_commit_rejected",
                    ),
                    _ => self.note_explicit_commit_destroyed(
                        commit.surface_commit_id,
                        "surface_or_sync_owner_destroyed",
                    ),
                }
                commit.pending.release_target().release();
                canceled_callbacks.extend(commit.frame_callbacks);
                if let Some(resize) = commit.pending.resize_commit.as_deref() {
                    canceled_resize_captures.push(resize.commit_sequence);
                }
                if self.external_acquire_readiness {
                    self.pending_acquire_watch_changes
                        .push(AcquireWatchChange::Cancel {
                            commit_id: commit.commit_id,
                            reason,
                        });
                }
            } else {
                retained.push(commit);
            }
        }
        self.pending_explicit_sync_commits = retained;
        self.rebuild_scene_work_index();
        for commit_sequence in canceled_resize_captures {
            self.release_resize_capture(surface_id, commit_sequence);
        }
        canceled_callbacks
    }

    pub(in crate::compositor) fn retain_oldest_pending_acquire_for_surface(
        &mut self,
        surface_id: u32,
        replacement: SurfaceCommitId,
    ) -> Vec<wl_callback::WlCallback> {
        let mut retained = Vec::with_capacity(self.pending_explicit_sync_commits.len());
        let mut kept_oldest = false;
        let mut superseded_callbacks = Vec::new();
        let mut released_captures = Vec::new();
        for commit in std::mem::take(&mut self.pending_explicit_sync_commits) {
            if commit.surface_id != surface_id || !kept_oldest {
                kept_oldest |= commit.surface_id == surface_id;
                retained.push(commit);
                continue;
            }
            self.note_explicit_commit_superseded(
                commit.surface_commit_id,
                commit.acquire_state,
                commit.frame_callbacks.len(),
                replacement,
                "bounded_pending_acquire_retention",
            );
            superseded_callbacks.extend(commit.frame_callbacks);
            if let Some(resize) = commit.pending.resize_commit.as_deref() {
                released_captures.push(resize.commit_sequence);
            }
            if self.external_acquire_readiness {
                self.pending_acquire_watch_changes
                    .push(AcquireWatchChange::Cancel {
                        commit_id: commit.commit_id,
                        reason: AcquireWatchCancelReason::Superseded,
                    });
            }
        }
        self.pending_explicit_sync_commits = retained;
        self.rebuild_scene_work_index();
        for commit_sequence in released_captures {
            self.release_resize_capture(surface_id, commit_sequence);
        }
        superseded_callbacks
    }

    pub(in crate::compositor) fn cancel_pending_acquire_commits_for_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        reason: AcquireWatchCancelReason,
    ) {
        let mut callbacks = Vec::new();
        let ids = self
            .pending_explicit_sync_commits
            .iter()
            .filter(|commit| same_wayland_resource(&commit.pending.resource, buffer))
            .map(|commit| commit.surface_id)
            .collect::<Vec<_>>();
        for surface_id in ids {
            callbacks.extend(self.cancel_pending_acquire_commits_for_surface(surface_id, reason));
        }
        let tree_roots = self
            .pending_surface_tree_transactions
            .iter()
            .filter(|transaction| {
                transaction.nodes.iter().any(|(_, commit)| {
                    commit.attachment.as_ref().is_some_and(|attachment| {
                        matches!(attachment, PendingSurfaceAttachment::Buffer(pending) if same_wayland_resource(&pending.resource, buffer))
                    })
                })
            })
            .map(|transaction| transaction.root_surface_id)
            .collect::<Vec<_>>();
        for root_surface_id in tree_roots {
            let released = self.cancel_pending_surface_trees_for_root(root_surface_id, reason);
            if let Some(resize_commit) = released.resize_commit {
                self.release_detached_resize_capture(root_surface_id, resize_commit);
            }
            callbacks.extend(released.callbacks);
        }
        self.complete_frame_callbacks(callbacks);
    }

    pub(in crate::compositor) fn cancel_pending_acquire_commits_for_timeline(
        &mut self,
        timeline: &crate::syncobj::DrmSyncobjTimeline,
        reason: AcquireWatchCancelReason,
    ) {
        let mut retained = Vec::with_capacity(self.pending_explicit_sync_commits.len());
        let mut released_captures = Vec::new();
        let mut callbacks = Vec::new();
        for commit in std::mem::take(&mut self.pending_explicit_sync_commits) {
            let uses_timeline = commit.acquire.timeline.same_timeline(timeline)
                || commit
                    .pending
                    .explicit_release
                    .as_ref()
                    .is_some_and(|release| release.timeline.same_timeline(timeline));
            if uses_timeline {
                commit.pending.release_target().release();
                callbacks.extend(commit.frame_callbacks);
                if let Some(resize) = commit.pending.resize_commit.as_deref() {
                    released_captures.push((commit.surface_id, resize.commit_sequence));
                }
                if self.external_acquire_readiness {
                    self.pending_acquire_watch_changes
                        .push(AcquireWatchChange::Cancel {
                            commit_id: commit.commit_id,
                            reason,
                        });
                }
            } else {
                retained.push(commit);
            }
        }
        self.pending_explicit_sync_commits = retained;
        self.rebuild_scene_work_index();
        for (surface_id, commit_sequence) in released_captures {
            self.release_resize_capture(surface_id, commit_sequence);
        }
        self.complete_frame_callbacks(callbacks);
        let tree_roots = self
            .pending_surface_tree_transactions
            .iter()
            .filter(|transaction| {
                transaction.dependencies.iter().any(|dependency| {
                    dependency.acquire.timeline.same_timeline(timeline)
                }) || transaction.nodes.iter().any(|(_, commit)| {
                    commit.attachment.as_ref().is_some_and(|attachment| {
                        matches!(attachment, PendingSurfaceAttachment::Buffer(pending) if pending.explicit_release.as_ref().is_some_and(|release| release.timeline.same_timeline(timeline)))
                    })
                })
            })
            .map(|transaction| transaction.root_surface_id)
            .collect::<Vec<_>>();
        for root_surface_id in tree_roots {
            let released = self.cancel_pending_surface_trees_for_root(root_surface_id, reason);
            if let Some(resize_commit) = released.resize_commit {
                self.release_detached_resize_capture(root_surface_id, resize_commit);
            }
            self.complete_frame_callbacks(released.callbacks);
        }
    }

    pub(in crate::compositor) fn enable_external_acquire_readiness(&mut self) {
        if self.external_acquire_readiness {
            return;
        }
        self.external_acquire_readiness = true;
        for commit in &self.pending_explicit_sync_commits {
            if commit.acquire_state == PendingAcquireState::Ready {
                continue;
            }
            self.pending_acquire_watch_changes
                .push(AcquireWatchChange::Register(AcquireWatchRequest {
                    commit_id: commit.commit_id,
                    surface_id: commit.surface_id,
                    buffer_id: commit.pending.resource.id().protocol_id(),
                    acquire: commit.acquire.clone(),
                    received_at: Instant::now(),
                }));
        }
        for transaction in &self.pending_surface_tree_transactions {
            for dependency in &transaction.dependencies {
                if dependency.state == PendingAcquireState::Ready {
                    continue;
                }
                self.pending_acquire_watch_changes
                    .push(AcquireWatchChange::Register(AcquireWatchRequest {
                        commit_id: dependency.commit_id,
                        surface_id: dependency.surface_id,
                        buffer_id: dependency.buffer_id,
                        acquire: dependency.acquire.clone(),
                        received_at: transaction.received_at,
                    }));
            }
        }
        self.rebuild_scene_work_index();
    }

    pub(in crate::compositor) fn take_acquire_watch_changes(&mut self) -> Vec<AcquireWatchChange> {
        std::mem::take(&mut self.pending_acquire_watch_changes)
    }

    pub(in crate::compositor) fn mark_acquire_commit_eventfd_backed(
        &mut self,
        commit_id: AcquireCommitId,
    ) -> bool {
        if self
            .pending_explicit_sync_commits
            .iter_mut()
            .find(|commit| commit.commit_id == commit_id)
            .is_some_and(|commit| commit.acquire_state.mark_eventfd_backed())
        {
            return true;
        }
        self.pending_surface_tree_transactions
            .iter_mut()
            .flat_map(|transaction| &mut transaction.dependencies)
            .find(|dependency| dependency.commit_id == commit_id)
            .is_some_and(|dependency| dependency.state.mark_eventfd_backed())
    }

    pub(in crate::compositor) fn mark_acquire_commit_fallback_backed(
        &mut self,
        commit_id: AcquireCommitId,
    ) -> bool {
        if self
            .pending_explicit_sync_commits
            .iter_mut()
            .find(|commit| commit.commit_id == commit_id)
            .is_some_and(|commit| commit.acquire_state.mark_fallback_backed())
        {
            return true;
        }
        self.pending_surface_tree_transactions
            .iter_mut()
            .flat_map(|transaction| &mut transaction.dependencies)
            .find(|dependency| dependency.commit_id == commit_id)
            .is_some_and(|dependency| dependency.state.mark_fallback_backed())
    }

    pub(in crate::compositor) fn mark_acquire_commit_ready(
        &mut self,
        commit_id: AcquireCommitId,
        surface_id: u32,
        acquire: &ExplicitSyncPoint,
    ) -> bool {
        let surface_commit_id = self
            .pending_explicit_sync_commits
            .iter()
            .find(|commit| commit.commit_id == commit_id)
            .map(|commit| commit.surface_commit_id);
        let surface_commit_id = surface_commit_id.or_else(|| {
            self.pending_surface_tree_transactions
                .iter()
                .flat_map(|transaction| &transaction.dependencies)
                .find(|dependency| dependency.commit_id == commit_id)
                .map(|dependency| dependency.surface_commit_id)
        });
        let ready = if self
            .pending_explicit_sync_commits
            .iter_mut()
            .find(|commit| {
                commit.commit_id == commit_id
                    && commit.surface_id == surface_id
                    && commit.acquire == *acquire
            })
            .is_some_and(|commit| commit.acquire_state.mark_ready())
        {
            true
        } else {
            self.pending_surface_tree_transactions
                .iter_mut()
                .flat_map(|transaction| &mut transaction.dependencies)
                .find(|dependency| {
                    dependency.commit_id == commit_id
                        && dependency.surface_id == surface_id
                        && dependency.acquire == *acquire
                })
                .is_some_and(|dependency| dependency.state.mark_ready())
        };
        if ready {
            if let Some(surface_commit_id) = surface_commit_id {
                self.note_explicit_commit_ready(surface_commit_id);
            }
            client_pacing_log(
                "acquire_ready",
                &[
                    ("surface", surface_id.to_string()),
                    (
                        "root",
                        self.root_surface_id_for_surface(surface_id).to_string(),
                    ),
                    (
                        "client",
                        format!("{:?}", self.surface_client_ids.get(&surface_id)),
                    ),
                    ("acquire_commit_id", commit_id.get().to_string()),
                ],
            );
            self.rebuild_scene_work_index();
        }
        ready
    }

    pub(in crate::compositor) fn commit_ready_explicit_sync_buffers(&mut self) {
        let mut commits = std::mem::take(&mut self.pending_explicit_sync_commits);
        let mut newly_ready = Vec::new();
        for commit in &mut commits {
            if !self.external_acquire_readiness
                && commit.acquire.is_signaled()
                && commit.acquire_state.mark_ready()
            {
                newly_ready.push(commit.surface_commit_id);
            }
        }
        for commit_id in newly_ready {
            self.note_explicit_commit_ready(commit_id);
        }
        let prefix_end = ready_explicit_sync_prefix_end_indices(commits.iter().enumerate().map(
            |(index, commit)| {
                (
                    index,
                    commit.surface_id,
                    commit.acquire_state == PendingAcquireState::Ready,
                )
            },
        ));
        let replacements = commits
            .iter()
            .enumerate()
            .filter_map(|(index, commit)| {
                let end = *prefix_end.get(&commit.surface_id)?;
                (index <= end && commit.acquire_state != PendingAcquireState::Ready).then(|| {
                    let replacement = commits[index + 1..=end]
                        .iter()
                        .find(|candidate| {
                            candidate.surface_id == commit.surface_id
                                && candidate.acquire_state == PendingAcquireState::Ready
                        })
                        .expect("ready prefix end guarantees an ordered ready successor")
                        .surface_commit_id;
                    (index, replacement)
                })
            })
            .collect::<HashMap<_, _>>();
        let mut waiting = Vec::new();
        let mut ready = Vec::new();
        let mut carried_callbacks: HashMap<u32, Vec<wl_callback::WlCallback>> = HashMap::new();
        let mut released_captures = Vec::new();
        for (index, commit) in commits.into_iter().enumerate() {
            let Some(&end_index) = prefix_end.get(&commit.surface_id) else {
                waiting.push(commit);
                continue;
            };
            if index > end_index {
                waiting.push(commit);
            } else if commit.acquire_state != PendingAcquireState::Ready {
                self.note_explicit_commit_superseded(
                    commit.surface_commit_id,
                    commit.acquire_state,
                    commit.frame_callbacks.len(),
                    replacements[&index],
                    "unready_head_superseded",
                );
                carried_callbacks
                    .entry(commit.surface_id)
                    .or_default()
                    .extend(commit.frame_callbacks);
                if let Some(resize) = commit.pending.resize_commit.as_deref() {
                    released_captures.push((commit.surface_id, resize.commit_sequence));
                }
                if self.external_acquire_readiness {
                    self.pending_acquire_watch_changes
                        .push(AcquireWatchChange::Cancel {
                            commit_id: commit.commit_id,
                            reason: AcquireWatchCancelReason::Superseded,
                        });
                }
            } else {
                let mut commit = commit;
                let mut callbacks = carried_callbacks
                    .remove(&commit.surface_id)
                    .unwrap_or_default();
                callbacks.append(&mut commit.frame_callbacks);
                commit.frame_callbacks = callbacks;
                ready.push(commit);
            }
        }
        self.pending_explicit_sync_commits = waiting;
        for (surface_id, commit_sequence) in released_captures {
            self.release_resize_capture(surface_id, commit_sequence);
        }
        for mut commit in ready {
            let decision = self.surface_publication_decision(
                commit.surface_id,
                commit.commit_sequence,
                SurfacePublicationContext::OrderedExplicitSyncQueue,
            );
            if decision != SurfacePublicationDecision::Publish {
                self.record_surface_publication_rejection(
                    commit.surface_id,
                    commit.commit_sequence,
                    Some(commit.pending.data.buffer_id()),
                    SurfacePublicationSource::ExplicitSync,
                    decision,
                );
                if let Some(resize) = commit.pending.resize_commit.as_deref() {
                    self.release_resize_capture(commit.surface_id, resize.commit_sequence);
                }
                commit.pending.release_target().release();
                self.complete_frame_callbacks(commit.frame_callbacks);
                continue;
            }
            let callbacks = commit.frame_callbacks;
            if commit.pending.resize_commit.is_none() {
                commit.pending.resize_commit = self
                    .capture_acked_resize_for_surface_commit(commit.surface_id)
                    .map(|snapshot| {
                        self.snapshot_resize_commit_for_buffer(
                            commit.surface_id,
                            snapshot,
                            &commit.pending,
                            commit.window_geometry,
                        )
                    })
                    .map(Box::new);
            }
            self.commit_surface_buffer_by_role(
                commit.surface_id,
                commit.pending,
                commit.damage,
                callbacks,
                SurfacePublicationSource::ExplicitSync,
                commit.window_geometry,
            );
        }
        self.commit_ready_surface_tree_transactions();
    }
}
