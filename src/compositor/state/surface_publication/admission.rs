use super::*;

impl CompositorState {
    // Keep the captured-sync admission entry point for direct admissions that
    // do not arrive through the materialized SurfaceTree transaction queue.
    // Prepared tree nodes must use the non-reentrant apply leaf instead.
    #[allow(dead_code)]
    pub(in crate::compositor) fn commit_surface_request_with_captured_sync(
        &mut self,
        surface_id: u32,
        surface_commit_id: SurfaceCommitId,
        commit_sequence: SurfaceCommitSequence,
        source: SurfacePublicationSource,
        mut pending: PendingSurfaceBuffer,
        damage: RenderableSurfaceDamage,
        frame_callbacks: Vec<wl_callback::WlCallback>,
        presentation_feedbacks: Vec<PendingPresentationFeedback>,
        explicit_sync: Option<CapturedExplicitSyncState>,
        window_geometry: Option<XdgWindowGeometry>,
        layer_surface: Option<CapturedLayerSurfaceCommitState>,
    ) -> bool {
        pending.commit_sequence = commit_sequence;
        if !self.surface_buffer_publication_preconditions(surface_id, &pending, layer_surface) {
            self.release_pending_surface_buffer(pending);
            self.complete_frame_callbacks(frame_callbacks);
            self.discard_presentation_feedbacks(presentation_feedbacks);
            return false;
        }
        let Some(CapturedExplicitSyncState {
            state: sync_state,
            acquire,
            release,
        }) = explicit_sync
        else {
            let callbacks = self.settle_direct_surface_buffer_before_publication(
                surface_id,
                commit_sequence,
                &mut pending,
                frame_callbacks,
                window_geometry,
            );
            return self.publish_admitted_surface_buffer(
                surface_id,
                pending,
                damage,
                callbacks,
                presentation_feedbacks,
                source,
                window_geometry,
            );
        };

        if !pending.data.is_dmabuf() {
            sync_state.post_error_with_metrics(
                &mut self.compliance_metrics,
                &mut self.protocol_error_trace,
                &mut self.terminal_client_ids,
                SYNCOBJ_SURFACE_ERROR_UNSUPPORTED_BUFFER,
                "explicit sync is only supported for linux-dmabuf buffers",
            );
            self.discard_presentation_feedbacks(presentation_feedbacks);
            return false;
        }

        let Some(acquire) = acquire else {
            sync_state.post_error_with_metrics(
                &mut self.compliance_metrics,
                &mut self.protocol_error_trace,
                &mut self.terminal_client_ids,
                SYNCOBJ_SURFACE_ERROR_NO_ACQUIRE_POINT,
                "dmabuf commit is missing an acquire timeline point",
            );
            self.discard_presentation_feedbacks(presentation_feedbacks);
            return false;
        };
        let Some(release) = release else {
            sync_state.post_error_with_metrics(
                &mut self.compliance_metrics,
                &mut self.protocol_error_trace,
                &mut self.terminal_client_ids,
                SYNCOBJ_SURFACE_ERROR_NO_RELEASE_POINT,
                "dmabuf commit is missing a release timeline point",
            );
            self.discard_presentation_feedbacks(presentation_feedbacks);
            return false;
        };

        if acquire.timeline.same_timeline(&release.timeline) && acquire.point >= release.point {
            sync_state.post_error_with_metrics(
                &mut self.compliance_metrics,
                &mut self.protocol_error_trace,
                &mut self.terminal_client_ids,
                SYNCOBJ_SURFACE_ERROR_CONFLICTING_POINTS,
                "acquire timeline point must be lower than release point on the same timeline",
            );
            self.discard_presentation_feedbacks(presentation_feedbacks);
            return false;
        }

        pending.explicit_release = Some(release);
        let acquire_ready = acquire.is_signaled();
        if acquire_ready {
            self.note_explicit_commit_ready(surface_commit_id);
            let older_ready_is_queued = self.pending_explicit_sync_commits.iter().any(|commit| {
                commit.surface_id == surface_id
                    && commit.commit_sequence < commit_sequence
                    && commit.acquire_state == PendingAcquireState::Ready
            });
            if older_ready_is_queued {
                let Some(commit_id) = self.acquire_commit_ids.allocate() else {
                    sync_state.post_error_with_metrics(
                        &mut self.compliance_metrics,
                        &mut self.protocol_error_trace,
                        &mut self.terminal_client_ids,
                        SYNCOBJ_SURFACE_ERROR_NO_ACQUIRE_POINT,
                        "explicit sync commit identity space exhausted",
                    );
                    self.discard_presentation_feedbacks(presentation_feedbacks);
                    return false;
                };
                let Some((owner_client_id, surface_presentation_generation)) =
                    self.capture_surface_publication_lifetime(surface_id)
                else {
                    self.release_pending_surface_buffer(pending);
                    self.complete_frame_callbacks(frame_callbacks);
                    self.discard_presentation_feedbacks(presentation_feedbacks);
                    return false;
                };
                self.finalize_pending_buffer_resize_capture(
                    surface_id,
                    &mut pending,
                    window_geometry,
                );
                self.pending_explicit_sync_commits
                    .push(PendingExplicitSyncCommit {
                        surface_commit_id,
                        commit_id,
                        surface_id,
                        owner_client_id,
                        surface_presentation_generation,
                        commit_sequence,
                        pending,
                        damage,
                        window_geometry,
                        frame_callbacks,
                        presentation_feedbacks,
                        acquire,
                        acquire_state: PendingAcquireState::Ready,
                    });
                debug_assert!(
                    self.pending_explicit_sync_commits
                        .iter()
                        .filter(|commit| commit.surface_id == surface_id)
                        .count()
                        <= 3
                );
                self.commit_ready_explicit_sync_buffers();
                return true;
            }
            let callbacks = self.settle_direct_surface_buffer_before_publication(
                surface_id,
                commit_sequence,
                &mut pending,
                frame_callbacks,
                window_geometry,
            );
            return self.publish_admitted_surface_buffer(
                surface_id,
                pending,
                damage,
                callbacks,
                presentation_feedbacks,
                source,
                window_geometry,
            );
        }

        let Some(commit_id) = self.acquire_commit_ids.allocate() else {
            sync_state.post_error_with_metrics(
                &mut self.compliance_metrics,
                &mut self.protocol_error_trace,
                &mut self.terminal_client_ids,
                SYNCOBJ_SURFACE_ERROR_NO_ACQUIRE_POINT,
                "explicit sync commit identity space exhausted",
            );
            self.discard_presentation_feedbacks(presentation_feedbacks);
            return false;
        };
        let mut callbacks =
            self.retain_oldest_pending_acquire_for_surface(surface_id, surface_commit_id);
        callbacks.extend(frame_callbacks);
        let Some((owner_client_id, surface_presentation_generation)) =
            self.capture_surface_publication_lifetime(surface_id)
        else {
            self.release_pending_surface_buffer(pending);
            self.complete_frame_callbacks(callbacks);
            self.discard_presentation_feedbacks(presentation_feedbacks);
            return false;
        };
        self.finalize_pending_buffer_resize_capture(surface_id, &mut pending, window_geometry);
        let buffer_id = pending.resource.id().protocol_id();
        let received_at = Instant::now();
        client_pacing_log(
            "acquire_wait_queued",
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
                ("commit_sequence", commit_sequence.0.to_string()),
                ("acquire_commit_id", commit_id.get().to_string()),
                ("buffer", buffer_id.to_string()),
            ],
        );
        let callback_count = callbacks.len();
        self.pending_explicit_sync_commits
            .push(PendingExplicitSyncCommit {
                surface_commit_id,
                commit_id,
                surface_id,
                owner_client_id,
                surface_presentation_generation,
                commit_sequence,
                pending,
                damage,
                window_geometry,
                frame_callbacks: callbacks,
                presentation_feedbacks,
                acquire: acquire.clone(),
                acquire_state: PendingAcquireState::RegistrationPending,
            });
        self.rebuild_scene_work_index();
        debug_assert!(
            self.pending_explicit_sync_commits
                .iter()
                .filter(|commit| commit.surface_id == surface_id)
                .count()
                <= 3
        );
        self.note_explicit_commit_acquire_wait(surface_commit_id, callback_count);
        self.resize_flow_metrics.commits_delayed_by_explicit_sync = self
            .resize_flow_metrics
            .commits_delayed_by_explicit_sync
            .saturating_add(1);
        self.resize_flow_metrics.max_pending_explicit_sync_commits = self
            .resize_flow_metrics
            .max_pending_explicit_sync_commits
            .max(self.pending_explicit_sync_commits.len());
        if compositor_debug_surface_logging_enabled() {
            let pending = self
                .pending_explicit_sync_commits
                .last()
                .expect("explicit-sync commit was just queued");
            eprintln!(
                "oblivion-one compositor: resize_flow surface={surface_id} decision=captured commit_generation={} commit_has_buffer=true explicit_sync=waiting acked_serial={:?} pending_explicit_sync={}",
                pending
                    .pending
                    .resize_commit
                    .as_deref()
                    .map_or(0, |snapshot| snapshot.commit_sequence),
                pending
                    .pending
                    .resize_commit
                    .as_deref()
                    .map(|snapshot| snapshot.serial),
                self.pending_explicit_sync_commits.len(),
            );
        }
        if self.external_acquire_readiness {
            self.pending_acquire_watch_changes
                .push(AcquireWatchChange::Register(AcquireWatchRequest {
                    commit_id,
                    surface_id,
                    buffer_id,
                    acquire,
                    received_at,
                }));
        }
        true
    }
}
