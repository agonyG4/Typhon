use super::*;

impl CompositorState {
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
        false
    }

    pub(in crate::compositor) fn has_pending_frame_prepare_work(&self) -> bool {
        self.pending_interactive_resize_update.is_some()
            || self.pending_resize_configure_is_flushable()
            || self.pending_explicit_sync_commits.iter().any(|commit| {
                !self.external_acquire_readiness
                    || commit.acquire_state == PendingAcquireState::Ready
            })
            || self
                .pending_surface_tree_transactions
                .iter()
                .any(|transaction| !self.external_acquire_readiness || transaction.is_ready())
            || !self.pending_color_info.is_empty()
    }

    pub(in crate::compositor) fn has_pending_explicit_sync_work(&self) -> bool {
        !self.pending_explicit_sync_commits.is_empty()
            || !self.pending_surface_tree_transactions.is_empty()
    }

    pub(in crate::compositor) fn has_pending_frame_work(&self) -> bool {
        self.pending_interactive_resize_update.is_some()
            || self.pending_resize_configure_is_flushable()
            || self.has_pending_frame_callbacks()
            || !self.pending_presentation_feedbacks.is_empty()
            || self
                .frame_batches
                .values()
                .any(|batch| !batch.presentation_feedbacks.is_empty())
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
        let flags = match presentation.kind {
            PresentationKind::Synchronized => wp_presentation_feedback::Kind::Vsync,
            PresentationKind::Software => wp_presentation_feedback::Kind::empty(),
        };
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
        let batch_id = CompositorFrameBatchId(
            NonZeroU64::new(self.next_frame_batch_id)
                .expect("compositor frame batch IDs start at one"),
        );
        let previous = self.frame_batches.insert(
            batch_id,
            CompositorFrameBatch {
                frame_id,
                callbacks: std::mem::take(&mut self.pending_frame_callbacks),
                presentation_feedbacks: std::mem::take(&mut self.pending_presentation_feedbacks),
            },
        );
        assert!(previous.is_none(), "compositor frame batch ID was reused");
        batch_id
    }

    #[allow(dead_code)] // Called through the explicit output server API after runtime integration.
    pub(in crate::compositor) fn restore_frame_batch_after_render_failure(
        &mut self,
        batch_id: CompositorFrameBatchId,
    ) {
        let mut batch = self
            .frame_batches
            .remove(&batch_id)
            .expect("missing compositor frame batch on render failure");
        batch.callbacks.append(&mut self.pending_frame_callbacks);
        self.pending_frame_callbacks = batch.callbacks;
        batch
            .presentation_feedbacks
            .append(&mut self.pending_presentation_feedbacks);
        self.pending_presentation_feedbacks = batch.presentation_feedbacks;
        self.clear_legacy_batch_reference(batch_id);
    }

    pub(in crate::compositor) fn discard_frame_batch(
        &mut self,
        batch_id: CompositorFrameBatchId,
        _reason: FrameBatchDiscardReason,
    ) {
        let batch = self
            .frame_batches
            .remove(&batch_id)
            .expect("missing compositor frame batch on discard");
        for pending in batch.presentation_feedbacks {
            pending.feedback.discarded();
        }
        self.complete_frame_callbacks(batch.callbacks);
        self.clear_legacy_batch_reference(batch_id);
    }

    pub(in crate::compositor) fn complete_presented_frame_batch(
        &mut self,
        frame_id: u64,
        batch_id: CompositorFrameBatchId,
        presentation: FramePresentation,
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
        let batch = self
            .frame_batches
            .remove(&batch_id)
            .expect("compositor frame batch disappeared during completion");
        self.clear_legacy_batch_reference(batch_id);
        self.complete_frame_callbacks(batch.callbacks);
        self.complete_presentation_feedbacks(batch.presentation_feedbacks, presentation);
    }

    fn clear_legacy_batch_reference(&mut self, batch_id: CompositorFrameBatchId) {
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
        discard_surface(&mut self.pending_presentation_feedbacks, surface_id);
        for batch in self.frame_batches.values_mut() {
            discard_surface(&mut batch.presentation_feedbacks, surface_id);
        }
    }

    pub(in crate::compositor) fn discard_all_pending_presentation_feedbacks(&mut self) {
        for pending in std::mem::take(&mut self.pending_presentation_feedbacks) {
            pending.feedback.discarded();
        }
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

    pub(in crate::compositor) fn release_pending_buffers(&mut self) {
        let buffers = std::mem::take(&mut self.pending_buffer_releases);
        for buffer in buffers {
            let _ = buffer.send_event(wl_buffer::Event::Release);
        }

        let dmabuf_releases = std::mem::replace(
            &mut self.deferred_dmabuf_buffer_releases,
            std::mem::take(&mut self.pending_dmabuf_buffer_releases),
        );
        for release in dmabuf_releases {
            release.release();
        }
    }

    pub(in crate::compositor) fn complete_frame_callbacks(
        &mut self,
        callbacks: Vec<wl_callback::WlCallback>,
    ) {
        let callbacks: Vec<_> = callbacks
            .into_iter()
            .filter(|callback| callback.is_alive())
            .collect();
        let time = self.frame_callback_time_ms();
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
        let ids = self
            .pending_explicit_sync_commits
            .iter()
            .filter(|commit| same_wayland_resource(&commit.pending.resource, buffer))
            .map(|commit| commit.surface_id)
            .collect::<Vec<_>>();
        for surface_id in ids {
            self.cancel_pending_acquire_commits_for_surface(surface_id, reason);
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
            self.complete_frame_callbacks(released.callbacks);
        }
    }

    pub(in crate::compositor) fn cancel_pending_acquire_commits_for_timeline(
        &mut self,
        timeline: &crate::syncobj::DrmSyncobjTimeline,
        reason: AcquireWatchCancelReason,
    ) {
        let mut retained = Vec::with_capacity(self.pending_explicit_sync_commits.len());
        let mut released_captures = Vec::new();
        for commit in std::mem::take(&mut self.pending_explicit_sync_commits) {
            let uses_timeline = commit.acquire.timeline.same_timeline(timeline)
                || commit
                    .pending
                    .explicit_release
                    .as_ref()
                    .is_some_and(|release| release.timeline.same_timeline(timeline));
            if uses_timeline {
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
        for (surface_id, commit_sequence) in released_captures {
            self.release_resize_capture(surface_id, commit_sequence);
        }
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

    pub(in crate::compositor) fn commit_ready_surface_tree_transactions(&mut self) {
        let mut transactions = std::mem::take(&mut self.pending_surface_tree_transactions);
        let mut newly_ready = Vec::new();
        if !self.external_acquire_readiness {
            for transaction in &mut transactions {
                for dependency in &mut transaction.dependencies {
                    if dependency.acquire.is_signaled() && dependency.state.mark_ready() {
                        newly_ready.push(dependency.surface_commit_id);
                    }
                }
            }
        }
        for commit_id in newly_ready {
            self.note_explicit_commit_ready(commit_id);
        }
        let prefix_end =
            ready_explicit_sync_prefix_end_indices(transactions.iter().enumerate().map(
                |(index, transaction)| (index, transaction.root_surface_id, transaction.is_ready()),
            ));
        let replacements = transactions
            .iter()
            .enumerate()
            .filter_map(|(index, transaction)| {
                let end = *prefix_end.get(&transaction.root_surface_id)?;
                (index <= end && !transaction.is_ready()).then(|| {
                    let replacement = transactions[index + 1..=end]
                        .iter()
                        .find(|candidate| {
                            candidate.root_surface_id == transaction.root_surface_id
                                && candidate.is_ready()
                        })
                        .and_then(|candidate| candidate.nodes.first())
                        .expect("ready tree prefix guarantees an ordered ready successor")
                        .1
                        .commit_id;
                    (index, replacement)
                })
            })
            .collect::<HashMap<_, _>>();
        let mut waiting = Vec::new();
        let mut ready = Vec::new();
        let mut superseded_callbacks: HashMap<u32, Vec<wl_callback::WlCallback>> = HashMap::new();
        let mut superseded_resize_commits: HashMap<u32, ResizeCommitSnapshot> = HashMap::new();
        for (index, transaction) in transactions.into_iter().enumerate() {
            let Some(&end_index) = prefix_end.get(&transaction.root_surface_id) else {
                waiting.push(transaction);
                continue;
            };
            if index > end_index {
                waiting.push(transaction);
            } else if !transaction.is_ready() {
                let root_id = transaction.root_surface_id;
                let acquire_state = if transaction.is_ready() {
                    PendingAcquireState::Ready
                } else {
                    PendingAcquireState::RegistrationPending
                };
                let replacement = replacements[&index];
                for (_, commit) in &transaction.nodes {
                    if commit.attachment.is_some() {
                        self.note_explicit_commit_superseded(
                            commit.commit_id,
                            acquire_state,
                            commit.frame_callbacks.len(),
                            replacement,
                            "unready_surface_tree_head_superseded",
                        );
                    }
                }
                let released = self.release_pending_surface_tree_transaction(
                    transaction,
                    AcquireWatchCancelReason::Superseded,
                );
                superseded_callbacks
                    .entry(root_id)
                    .or_default()
                    .extend(released.callbacks);
                if let Some(resize_commit) = released.resize_commit
                    && let Some(previous) = superseded_resize_commits.insert(root_id, resize_commit)
                {
                    self.release_detached_resize_capture(root_id, previous);
                }
                self.subsurface_transaction_metrics
                    .tree_transactions_superseded = self
                    .subsurface_transaction_metrics
                    .tree_transactions_superseded
                    .saturating_add(1);
            } else {
                let mut transaction = transaction;
                if let Some((_, root)) = transaction.nodes.first_mut() {
                    let mut callbacks = superseded_callbacks
                        .remove(&transaction.root_surface_id)
                        .unwrap_or_default();
                    callbacks.append(&mut root.frame_callbacks);
                    root.frame_callbacks = callbacks;
                }
                if let Some(resize_commit) =
                    superseded_resize_commits.remove(&transaction.root_surface_id)
                {
                    self.install_tree_resize_commit(
                        transaction.root_surface_id,
                        &mut transaction.nodes,
                        resize_commit,
                    );
                }
                ready.push(transaction);
            }
        }
        self.pending_surface_tree_transactions = waiting;
        for transaction in ready {
            let wait_ms =
                u64::try_from(transaction.received_at.elapsed().as_millis()).unwrap_or(u64::MAX);
            self.subsurface_transaction_metrics
                .maximum_transaction_wait_ms = self
                .subsurface_transaction_metrics
                .maximum_transaction_wait_ms
                .max(wait_ms);
            self.subsurface_transaction_metrics
                .waiting_transactions_published = self
                .subsurface_transaction_metrics
                .waiting_transactions_published
                .saturating_add(1);
            self.publish_surface_tree_nodes(transaction.root_surface_id, transaction.nodes);
        }
        for (root_surface_id, resize_commit) in superseded_resize_commits {
            self.release_detached_resize_capture(root_surface_id, resize_commit);
        }
    }
}

#[cfg(test)]
mod frame_consumption_tests {
    use super::*;

    #[test]
    fn empty_submitted_frame_batch_is_still_owned_until_completion() {
        let mut state = CompositorState::default();
        state.capture_frame_callbacks_for_render();
        state.mark_prepared_frame_submitted();

        assert!(state.has_submitted_frame_batch());
        state.complete_pending_presentation_feedbacks(
            FramePresentation::software_now(state.presentation_clock).unwrap(),
        );
        assert!(!state.has_submitted_frame_batch());
        assert!(state.frame_batches.is_empty());
    }

    #[test]
    fn prepare_publication_does_not_create_a_submitted_frame_batch() {
        let mut state = CompositorState::default();
        state.commit_ready_explicit_sync_buffers();
        assert!(!state.has_submitted_frame_batch());
    }

    #[test]
    fn empty_frame_batch_is_explicit_and_registry_is_bounded_to_two() {
        let mut state = CompositorState::default();
        let first = state.take_frame_batch_for_render(10);
        let second = state.take_frame_batch_for_render(11);
        assert_eq!(state.frame_batches.len(), 2);
        assert!(state.frame_batches[&first].callbacks.is_empty());
        assert!(
            state.frame_batches[&second]
                .presentation_feedbacks
                .is_empty()
        );

        let overflow = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.take_frame_batch_for_render(12)
        }));
        assert!(overflow.is_err());
        assert_eq!(state.frame_batches.len(), 2);
    }

    #[test]
    fn unrelated_completion_cannot_consume_ready_frame_batch() {
        let mut state = CompositorState::default();
        let submitted = state.take_frame_batch_for_render(20);
        let ready = state.take_frame_batch_for_render(21);
        let presentation = FramePresentation::software_now(state.presentation_clock).unwrap();

        state.complete_presented_frame_batch(20, submitted, presentation);

        assert!(!state.frame_batches.contains_key(&submitted));
        assert!(state.frame_batches.contains_key(&ready));
        state.restore_frame_batch_after_render_failure(ready);
        assert!(state.frame_batches.is_empty());
    }

    #[test]
    fn mismatched_frame_and_batch_identity_completes_nothing() {
        let mut state = CompositorState::default();
        let batch = state.take_frame_batch_for_render(30);
        let presentation = FramePresentation::software_now(state.presentation_clock).unwrap();

        let mismatch = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.complete_presented_frame_batch(31, batch, presentation)
        }));

        assert!(mismatch.is_err());
        assert!(state.frame_batches.contains_key(&batch));
    }
}
