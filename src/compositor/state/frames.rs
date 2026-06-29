use super::*;

impl CompositorState {
    pub(in crate::compositor) fn complete_frame_callbacks_now(&mut self, data: &SurfaceData) {
        let callbacks = data.take_frame_callbacks();
        self.complete_frame_callbacks(callbacks);
    }

    pub(in crate::compositor) fn complete_pending_frame_callbacks(&mut self) {
        let mut callbacks = std::mem::take(&mut self.pending_frame_callbacks);
        for surface in self.surface_resources.values() {
            if let Some(data) = surface.data::<SurfaceData>() {
                callbacks.extend(data.take_frame_callbacks());
            }
        }
        self.complete_frame_callbacks(callbacks);
    }

    pub(in crate::compositor) fn has_pending_frame_callbacks(&self) -> bool {
        !self.pending_frame_callbacks.is_empty()
            || self.pending_explicit_sync_commits.iter().any(|commit| {
                !self.external_acquire_readiness && !commit.frame_callbacks.is_empty()
            })
            || self
                .pending_surface_tree_transactions
                .iter()
                .flat_map(|transaction| &transaction.nodes)
                .any(|(_, commit)| !commit.frame_callbacks.is_empty())
            || self
                .surface_resources
                .values()
                .filter_map(Resource::data::<SurfaceData>)
                .any(SurfaceData::has_frame_callbacks)
    }

    pub(in crate::compositor) fn has_only_pending_surface_frame_callbacks(&self) -> bool {
        !self.pending_resize_configure_is_flushable()
            && self.pending_frame_callbacks.is_empty()
            && self.pending_explicit_sync_commits.is_empty()
            && self.pending_surface_tree_transactions.is_empty()
            && self.pending_presentation_feedbacks.is_empty()
            && self
                .surface_resources
                .values()
                .filter_map(Resource::data::<SurfaceData>)
                .any(SurfaceData::has_frame_callbacks)
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
    }

    pub(in crate::compositor) fn complete_pending_presentation_feedbacks(
        &mut self,
        presentation: FramePresentation,
    ) {
        let feedbacks = std::mem::take(&mut self.pending_presentation_feedbacks);
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
        }
    }

    pub(in crate::compositor) fn discard_pending_presentation_feedbacks_for_surface(
        &mut self,
        surface_id: u32,
    ) {
        let mut pending_feedbacks = Vec::new();
        for pending in std::mem::take(&mut self.pending_presentation_feedbacks) {
            if pending.surface_id == surface_id {
                pending.feedback.discarded();
            } else {
                pending_feedbacks.push(pending);
            }
        }
        self.pending_presentation_feedbacks = pending_feedbacks;
    }

    pub(in crate::compositor) fn discard_all_pending_presentation_feedbacks(&mut self) {
        for pending in std::mem::take(&mut self.pending_presentation_feedbacks) {
            pending.feedback.discarded();
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
        let time = self.frame_callback_time_ms();
        for callback in callbacks {
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
        if self
            .pending_explicit_sync_commits
            .iter_mut()
            .find(|commit| {
                commit.commit_id == commit_id
                    && commit.surface_id == surface_id
                    && commit.acquire == *acquire
            })
            .is_some_and(|commit| commit.acquire_state.mark_ready())
        {
            return true;
        }
        self.pending_surface_tree_transactions
            .iter_mut()
            .flat_map(|transaction| &mut transaction.dependencies)
            .find(|dependency| {
                dependency.commit_id == commit_id
                    && dependency.surface_id == surface_id
                    && dependency.acquire == *acquire
            })
            .is_some_and(|dependency| dependency.state.mark_ready())
    }

    pub(in crate::compositor) fn commit_ready_explicit_sync_buffers(&mut self) {
        let mut commits = std::mem::take(&mut self.pending_explicit_sync_commits);
        for commit in &mut commits {
            if !self.external_acquire_readiness && commit.acquire.is_signaled() {
                commit.acquire_state.mark_ready();
            }
        }
        let newest_ready = newest_ready_explicit_sync_commit_indices(
            commits.iter().enumerate().map(|(index, commit)| {
                (
                    index,
                    commit.surface_id,
                    commit.acquire_state == PendingAcquireState::Ready,
                )
            }),
        );

        let mut waiting = Vec::new();
        let mut ready = Vec::new();
        let mut superseded_callbacks: HashMap<u32, Vec<wl_callback::WlCallback>> = HashMap::new();
        let mut released_captures = Vec::new();
        for (index, commit) in commits.into_iter().enumerate() {
            let Some(&ready_index) = newest_ready.get(&commit.surface_id) else {
                waiting.push(commit);
                continue;
            };
            if index < ready_index {
                superseded_callbacks
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
            } else if index == ready_index {
                ready.push(commit);
            } else {
                waiting.push(commit);
            }
        }
        self.pending_explicit_sync_commits = waiting;
        for (surface_id, commit_sequence) in released_captures {
            self.release_resize_capture(surface_id, commit_sequence);
        }
        for mut commit in ready {
            let decision =
                self.surface_publication_decision(commit.surface_id, commit.commit_sequence);
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
            let mut callbacks = superseded_callbacks
                .remove(&commit.surface_id)
                .unwrap_or_default();
            callbacks.extend(commit.frame_callbacks);
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
        if !self.external_acquire_readiness {
            for transaction in &mut transactions {
                for dependency in &mut transaction.dependencies {
                    if dependency.acquire.is_signaled() {
                        dependency.state.mark_ready();
                    }
                }
            }
        }
        let newest_ready =
            newest_ready_explicit_sync_commit_indices(transactions.iter().enumerate().map(
                |(index, transaction)| (index, transaction.root_surface_id, transaction.is_ready()),
            ));
        let mut waiting = Vec::new();
        let mut ready = Vec::new();
        let mut superseded_callbacks: HashMap<u32, Vec<wl_callback::WlCallback>> = HashMap::new();
        let mut superseded_resize_commits: HashMap<u32, ResizeCommitSnapshot> = HashMap::new();
        for (index, transaction) in transactions.into_iter().enumerate() {
            let Some(&ready_index) = newest_ready.get(&transaction.root_surface_id) else {
                waiting.push(transaction);
                continue;
            };
            if index < ready_index {
                let root_id = transaction.root_surface_id;
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
            } else if index == ready_index {
                ready.push(transaction);
            } else {
                waiting.push(transaction);
            }
        }
        self.pending_surface_tree_transactions = waiting;
        for mut transaction in ready {
            if let Some((_, root)) = transaction.nodes.first_mut() {
                root.frame_callbacks.extend(
                    superseded_callbacks
                        .remove(&transaction.root_surface_id)
                        .unwrap_or_default(),
                );
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
