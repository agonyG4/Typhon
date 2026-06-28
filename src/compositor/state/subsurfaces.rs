#![allow(clippy::question_mark)]

use super::*;

impl CompositorState {
    pub(crate) fn register_subsurface_relationship(
        &mut self,
        surface_id: u32,
        parent_id: u32,
    ) -> bool {
        if !self.subsurface_transactions.register(surface_id, parent_id) {
            return false;
        }
        self.committed_subsurface_stacks
            .entry(parent_id)
            .or_insert_with(|| vec![parent_id])
            .retain(|id| *id == parent_id || *id != surface_id);
        self.committed_subsurface_stacks
            .entry(parent_id)
            .or_insert_with(|| vec![parent_id])
            .push(surface_id);
        self.pending_subsurface_stacks.remove(&parent_id);
        self.reorder_renderable_surfaces_by_committed_stack();
        true
    }

    pub(crate) fn is_effectively_synchronized_subsurface(&self, surface_id: u32) -> bool {
        self.subsurface_transactions
            .is_effectively_synchronized(surface_id)
    }

    pub(crate) fn set_subsurface_sync_mode(&mut self, surface_id: u32, mode: SubsurfaceSyncMode) {
        if !self.subsurface_transactions.set_mode(surface_id, mode) {
            return;
        }
        if mode == SubsurfaceSyncMode::Desynchronized {
            let mut commits = self
                .subsurface_transactions
                .take_desynchronized_subtree_commits(surface_id);
            if !commits.is_empty() {
                if !commits
                    .iter()
                    .any(|(commit_surface_id, _)| *commit_surface_id == surface_id)
                {
                    commits.insert(0, (surface_id, empty_cached_subsurface_commit()));
                }
                self.submit_surface_tree_nodes(surface_id, commits);
            }
        }
    }

    pub(crate) fn set_pending_subsurface_position(&mut self, surface_id: u32, x: i32, y: i32) {
        self.subsurface_transactions
            .set_pending_position(surface_id, x, y);
    }

    pub(crate) fn commit_surface_tree_request(
        &mut self,
        surface_id: u32,
        mut commit: CachedSubsurfaceCommit,
    ) {
        if commit.attachment.is_some() {
            let mut superseded_callbacks = self.supersede_older_pending_attachments_for_surface(
                surface_id,
                commit.commit_sequence,
            );
            superseded_callbacks.extend(commit.frame_callbacks);
            commit.frame_callbacks = superseded_callbacks;
        }
        if self.is_effectively_synchronized_subsurface(surface_id) {
            self.cache_synchronized_subsurface_commit(surface_id, commit);
            return;
        }
        match commit.attachment.as_mut() {
            Some(PendingSurfaceAttachment::Buffer(pending)) => {
                if let Some(surface) = self.surface_resource_by_id(surface_id)
                    && let Some(data) = surface.data::<SurfaceData>()
                {
                    let viewport_destination =
                        data.viewport_destination_for_change(commit.viewport_destination);
                    let buffer_scale = data.buffer_scale_for_change(commit.buffer_scale);
                    if pending
                        .apply_committed_surface_state(viewport_destination, buffer_scale)
                        .is_err()
                    {
                        return;
                    }
                }
                self.finalize_pending_buffer_resize_capture(surface_id, pending);
            }
            _ => {
                commit.resize_commit = self.capture_acked_resize_for_surface_commit(surface_id);
                commit.resize_capture_finalized = true;
            }
        }
        let descendants = self
            .subsurface_transactions
            .take_latched_commits(surface_id);
        let mut nodes = Vec::with_capacity(descendants.len().saturating_add(1));
        nodes.push((surface_id, commit));
        nodes.extend(descendants);
        self.submit_surface_tree_nodes(surface_id, nodes);
    }

    pub(crate) fn submit_surface_tree_nodes(
        &mut self,
        surface_id: u32,
        mut nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    ) {
        if !self.prepare_surface_tree_surface_state(&mut nodes) {
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        }
        let Some(dependencies) = self.prepare_surface_tree_acquires(&mut nodes) else {
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        };
        self.subsurface_transaction_metrics
            .tree_transactions_prepared = self
            .subsurface_transaction_metrics
            .tree_transactions_prepared
            .saturating_add(1);
        self.merge_or_queue_surface_tree_transaction(surface_id, nodes, dependencies);
    }

    pub(crate) fn merge_or_queue_surface_tree_transaction(
        &mut self,
        root_surface_id: u32,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        dependencies: Vec<SurfaceTreeAcquireDependency>,
    ) {
        let incoming_has_unready_acquire = !dependencies.is_empty();
        let incoming_has_attachment_change =
            nodes.iter().any(|(_, commit)| commit.attachment.is_some());
        let matching = self
            .pending_surface_tree_transactions
            .iter()
            .enumerate()
            .filter_map(|(index, transaction)| {
                (transaction.root_surface_id == root_surface_id).then_some(index)
            })
            .collect::<Vec<_>>();
        let Some(&target_index) = matching.last() else {
            if incoming_has_unready_acquire {
                self.queue_waiting_surface_tree(root_surface_id, nodes, dependencies);
            } else {
                self.publish_surface_tree_nodes(root_surface_id, nodes);
            }
            return;
        };

        // Keep each root bounded to the presentable ready successor plus the newest
        // blocked successor. Metadata-only commits merge into the newest ordered
        // successor; a newer unready attachment never makes an existing ready
        // successor unpresentable.
        if incoming_has_unready_acquire
            && self.pending_surface_tree_transactions[target_index].is_ready()
        {
            self.subsurface_transaction_metrics
                .ready_transactions_preserved_from_newer_unready = self
                .subsurface_transaction_metrics
                .ready_transactions_preserved_from_newer_unready
                .saturating_add(1);
            self.queue_waiting_surface_tree(root_surface_id, nodes, dependencies);
            return;
        }

        let mut transaction = self.pending_surface_tree_transactions.remove(target_index);
        let stats = self.merge_surface_tree_nodes_into_transaction(
            root_surface_id,
            &mut transaction,
            nodes,
            dependencies,
        );
        let ready_after_merge = transaction.is_ready();
        self.record_surface_tree_merge_metrics(&stats);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: subsurface_tx root={root_surface_id} decision={} incoming_nodes={} existing_nodes={} bufferless_nodes={} attachments_replaced={} dependencies_preserved={} dependencies_replaced={} callbacks_merged={} feedbacks_merged={} resize_snapshot={} ready_after_merge={ready_after_merge}",
                if ready_after_merge {
                    "merged_ready"
                } else {
                    "merged_waiting"
                },
                stats.incoming_nodes,
                stats.existing_nodes,
                stats.bufferless_nodes,
                stats.attachments_replaced,
                stats.dependencies_preserved,
                stats.dependencies_replaced,
                stats.callbacks_merged,
                stats.feedbacks_merged,
                if stats.resize_snapshots_replaced > 0 {
                    "replaced"
                } else if stats.resize_snapshots_preserved > 0 {
                    "preserved"
                } else {
                    "none"
                },
            );
        }
        self.pending_surface_tree_transactions.push(transaction);
        self.update_surface_tree_slot_metrics(root_surface_id);
        if ready_after_merge || !incoming_has_attachment_change {
            self.commit_ready_surface_tree_transactions();
        }
    }

    pub(crate) fn merge_surface_tree_nodes_into_transaction(
        &mut self,
        root_surface_id: u32,
        transaction: &mut PendingSurfaceTreeTransaction,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        dependencies: Vec<SurfaceTreeAcquireDependency>,
    ) -> SurfaceTreeMergeStats {
        let mut stats = SurfaceTreeMergeStats {
            incoming_nodes: nodes.len(),
            existing_nodes: transaction.nodes.len(),
            ..SurfaceTreeMergeStats::default()
        };
        for (surface_id, incoming) in nodes {
            let attachment_changed = incoming.attachment.is_some();
            let callbacks = incoming.frame_callbacks.len();
            let feedbacks = incoming.presentation_feedbacks.len();
            if !attachment_changed {
                stats.bufferless_nodes = stats.bufferless_nodes.saturating_add(1);
            }
            if matches!(
                incoming.attachment,
                Some(PendingSurfaceAttachment::RemoveContent)
            ) {
                stats.explicit_detaches = stats.explicit_detaches.saturating_add(1);
            }
            let resize_replaced =
                incoming.resize_capture_finalized && incoming.resize_commit.is_some();
            let Some(existing_index) = transaction
                .nodes
                .iter()
                .position(|(node_surface_id, _)| *node_surface_id == surface_id)
            else {
                transaction.nodes.push((surface_id, incoming));
                continue;
            };
            let old_buffer_id = transaction.nodes[existing_index]
                .1
                .attachment
                .as_ref()
                .and_then(pending_attachment_buffer_protocol_id);
            let old_resize_commit = attachment_changed
                .then(|| pending_node_resize_commit(&transaction.nodes[existing_index].1))
                .flatten();
            let replaced_dependency = attachment_changed
                .then(|| {
                    old_buffer_id.and_then(|buffer_id| {
                        remove_surface_tree_dependency(transaction, surface_id, buffer_id)
                    })
                })
                .flatten();
            if let Some(dependency) = replaced_dependency {
                stats.dependencies_replaced = stats.dependencies_replaced.saturating_add(1);
                if self.external_acquire_readiness {
                    self.pending_acquire_watch_changes
                        .push(AcquireWatchChange::Cancel {
                            commit_id: dependency.commit_id,
                            reason: AcquireWatchCancelReason::Superseded,
                        });
                }
                if compositor_debug_surface_logging_enabled() {
                    eprintln!(
                        "oblivion-one compositor: subsurface_tx root={root_surface_id} decision=attachment_superseded surface={surface_id} old_buffer_id={} old_commit_id={}",
                        dependency.buffer_id,
                        dependency.commit_id.get(),
                    );
                }
            } else if !attachment_changed {
                stats.dependencies_preserved = stats.dependencies_preserved.saturating_add(
                    transaction
                        .dependencies
                        .iter()
                        .filter(|dependency| dependency.surface_id == surface_id)
                        .count(),
                );
            }
            let existing = &mut transaction.nodes[existing_index].1;
            if let Some(release) = existing.merge(incoming) {
                stats.attachments_replaced = stats.attachments_replaced.saturating_add(1);
                release.release();
            }
            if let Some(resize_commit) = old_resize_commit {
                self.release_detached_resize_capture(surface_id, resize_commit);
            }
            if !attachment_changed && existing.resize_commit.is_some() {
                stats.resize_snapshots_preserved =
                    stats.resize_snapshots_preserved.saturating_add(1);
            }
            if resize_replaced {
                stats.resize_snapshots_replaced = stats.resize_snapshots_replaced.saturating_add(1);
            }
            stats.callbacks_merged = stats.callbacks_merged.saturating_add(callbacks);
            stats.feedbacks_merged = stats.feedbacks_merged.saturating_add(feedbacks);
        }
        if self.external_acquire_readiness {
            for dependency in &dependencies {
                self.pending_acquire_watch_changes
                    .push(AcquireWatchChange::Register(AcquireWatchRequest {
                        commit_id: dependency.commit_id,
                        surface_id: dependency.surface_id,
                        buffer_id: dependency.buffer_id,
                        acquire: dependency.acquire.clone(),
                        received_at: Instant::now(),
                    }));
            }
        }
        transaction.dependencies.extend(dependencies);
        stats
    }

    pub(crate) fn record_surface_tree_merge_metrics(&mut self, stats: &SurfaceTreeMergeStats) {
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

    pub(crate) fn update_surface_tree_slot_metrics(&mut self, root_surface_id: u32) {
        let mut ready = 0usize;
        let mut waiting = 0usize;
        for transaction in self
            .pending_surface_tree_transactions
            .iter()
            .filter(|transaction| transaction.root_surface_id == root_surface_id)
        {
            if transaction.is_ready() {
                ready = ready.saturating_add(1);
            } else {
                waiting = waiting.saturating_add(1);
            }
        }
        self.subsurface_transaction_metrics
            .maximum_ready_slots_per_root = self
            .subsurface_transaction_metrics
            .maximum_ready_slots_per_root
            .max(ready);
        self.subsurface_transaction_metrics
            .maximum_waiting_slots_per_root = self
            .subsurface_transaction_metrics
            .maximum_waiting_slots_per_root
            .max(waiting);
    }

    pub(crate) fn prepare_surface_tree_surface_state(
        &self,
        nodes: &mut [(u32, CachedSubsurfaceCommit)],
    ) -> bool {
        for (surface_id, commit) in nodes {
            let Some(PendingSurfaceAttachment::Buffer(pending)) = commit.attachment.as_mut() else {
                continue;
            };
            let Some(surface) = self.surface_resource_by_id(*surface_id) else {
                return false;
            };
            let Some(data) = surface.data::<SurfaceData>() else {
                return false;
            };
            let viewport_destination =
                data.viewport_destination_for_change(commit.viewport_destination);
            let buffer_scale = data.buffer_scale_for_change(commit.buffer_scale);
            if pending
                .apply_committed_surface_state(viewport_destination, buffer_scale)
                .is_err()
            {
                return false;
            }
        }
        true
    }

    pub(crate) fn prepare_surface_tree_acquires(
        &mut self,
        nodes: &mut [(u32, CachedSubsurfaceCommit)],
    ) -> Option<Vec<SurfaceTreeAcquireDependency>> {
        let mut dependencies = Vec::new();
        for (surface_id, commit) in nodes {
            let Some(explicit_sync) = commit.explicit_sync.take() else {
                continue;
            };
            let CapturedExplicitSyncState {
                state,
                acquire,
                release,
            } = explicit_sync;
            let Some(PendingSurfaceAttachment::Buffer(pending)) = commit.attachment.as_mut() else {
                if acquire.is_some() || release.is_some() {
                    state.post_error(
                        SYNCOBJ_SURFACE_ERROR_NO_BUFFER,
                        "explicit sync points were set without an attached buffer",
                    );
                    return None;
                }
                continue;
            };
            if !pending.data.is_dmabuf() {
                state.post_error(
                    SYNCOBJ_SURFACE_ERROR_UNSUPPORTED_BUFFER,
                    "explicit sync is only supported for linux-dmabuf buffers",
                );
                return None;
            }
            let Some(acquire) = acquire else {
                state.post_error(
                    SYNCOBJ_SURFACE_ERROR_NO_ACQUIRE_POINT,
                    "dmabuf commit is missing an acquire timeline point",
                );
                return None;
            };
            let Some(release) = release else {
                state.post_error(
                    SYNCOBJ_SURFACE_ERROR_NO_RELEASE_POINT,
                    "dmabuf commit is missing a release timeline point",
                );
                return None;
            };
            if acquire.timeline.same_timeline(&release.timeline) && acquire.point >= release.point {
                state.post_error(
                    SYNCOBJ_SURFACE_ERROR_CONFLICTING_POINTS,
                    "acquire timeline point must be lower than release point on the same timeline",
                );
                return None;
            }
            pending.explicit_release = Some(release);
            if acquire.is_signaled() {
                continue;
            }
            let Some(commit_id) = self.acquire_commit_ids.allocate() else {
                state.post_error(
                    SYNCOBJ_SURFACE_ERROR_NO_ACQUIRE_POINT,
                    "explicit sync commit identity space exhausted",
                );
                return None;
            };
            dependencies.push(SurfaceTreeAcquireDependency {
                commit_id,
                surface_id: *surface_id,
                buffer_id: pending.resource.id().protocol_id(),
                acquire,
                state: PendingAcquireState::RegistrationPending,
            });
        }
        Some(dependencies)
    }

    pub(crate) fn queue_waiting_surface_tree(
        &mut self,
        root_surface_id: u32,
        mut nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        dependencies: Vec<SurfaceTreeAcquireDependency>,
    ) {
        let matching = self
            .pending_surface_tree_transactions
            .iter()
            .enumerate()
            .filter_map(|(index, transaction)| {
                (transaction.root_surface_id == root_surface_id).then_some(index)
            })
            .collect::<Vec<_>>();
        if matching.len() >= 2 {
            let remove_index = matching
                .iter()
                .rev()
                .copied()
                .find(|index| !self.pending_surface_tree_transactions[*index].is_ready())
                .unwrap_or(matching[0]);
            let superseded = self.pending_surface_tree_transactions.remove(remove_index);
            let released = self.release_pending_surface_tree_transaction(
                superseded,
                AcquireWatchCancelReason::Superseded,
            );
            if let Some((_, root)) = nodes.first_mut() {
                root.frame_callbacks.extend(released.callbacks);
            }
            if let Some(resize_commit) = released.resize_commit {
                self.install_tree_resize_commit(root_surface_id, &mut nodes, resize_commit);
            }
            self.subsurface_transaction_metrics
                .tree_transactions_superseded = self
                .subsurface_transaction_metrics
                .tree_transactions_superseded
                .saturating_add(1);
        }
        if self.external_acquire_readiness {
            for dependency in &dependencies {
                self.pending_acquire_watch_changes
                    .push(AcquireWatchChange::Register(AcquireWatchRequest {
                        commit_id: dependency.commit_id,
                        surface_id: dependency.surface_id,
                        buffer_id: dependency.buffer_id,
                        acquire: dependency.acquire.clone(),
                        received_at: Instant::now(),
                    }));
            }
        }
        self.subsurface_transaction_metrics
            .tree_transactions_waiting_on_acquire = self
            .subsurface_transaction_metrics
            .tree_transactions_waiting_on_acquire
            .saturating_add(1);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: subsurface_tx root={root_surface_id} decision=waiting_acquire cached_nodes={} waiting_acquires={} callbacks={} preview_active={}",
                nodes.len(),
                dependencies.len(),
                nodes
                    .iter()
                    .map(|(_, commit)| commit.frame_callbacks.len())
                    .sum::<usize>(),
                self.active_toplevel_resizes.contains_key(&root_surface_id),
            );
        }
        self.pending_surface_tree_transactions
            .push(PendingSurfaceTreeTransaction {
                root_surface_id,
                nodes,
                dependencies,
                received_at: Instant::now(),
            });
        self.update_surface_tree_slot_metrics(root_surface_id);
        let pending_acquires = self.pending_explicit_sync_commits.len().saturating_add(
            self.pending_surface_tree_transactions
                .iter()
                .map(|transaction| transaction.dependencies.len())
                .sum::<usize>(),
        );
        self.resize_flow_metrics.max_pending_explicit_sync_commits = self
            .resize_flow_metrics
            .max_pending_explicit_sync_commits
            .max(pending_acquires);
    }

    pub(crate) fn publish_surface_tree_nodes(
        &mut self,
        root_surface_id: u32,
        mut nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    ) {
        let stale_node = nodes.iter().find_map(|(surface_id, commit)| {
            if commit.attachment.is_none() {
                return None;
            }
            let decision = self.surface_publication_decision(*surface_id, commit.commit_sequence);
            (decision != SurfacePublicationDecision::Publish).then_some((
                *surface_id,
                commit.commit_sequence,
                commit
                    .attachment
                    .as_ref()
                    .and_then(|attachment| match attachment {
                        PendingSurfaceAttachment::Buffer(buffer) => Some(buffer.data.buffer_id()),
                        PendingSurfaceAttachment::RemoveContent => None,
                    }),
                decision,
            ))
        });
        if let Some((surface_id, commit_sequence, buffer_id, decision)) = stale_node {
            self.record_surface_publication_rejection(
                surface_id,
                commit_sequence,
                buffer_id,
                SurfacePublicationSource::SurfaceTree,
                decision,
            );
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        }
        let Some(root_index) = nodes
            .iter()
            .position(|(surface_id, _)| *surface_id == root_surface_id)
        else {
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        };
        let (_, root_commit) = nodes.remove(root_index);
        self.publish_surface_tree(root_surface_id, root_commit, nodes);
    }

    pub(crate) fn cancel_pending_surface_trees_for_root(
        &mut self,
        root_surface_id: u32,
        reason: AcquireWatchCancelReason,
    ) -> ReleasedSurfaceTreeState {
        let mut retained = Vec::new();
        let mut released = ReleasedSurfaceTreeState {
            callbacks: Vec::new(),
            resize_commit: None,
        };
        for transaction in std::mem::take(&mut self.pending_surface_tree_transactions) {
            if transaction.root_surface_id == root_surface_id {
                let transaction =
                    self.release_pending_surface_tree_transaction(transaction, reason);
                self.subsurface_transaction_metrics.root_wide_supersessions = self
                    .subsurface_transaction_metrics
                    .root_wide_supersessions
                    .saturating_add(1);
                released.callbacks.extend(transaction.callbacks);
                if released.resize_commit.is_none() {
                    released.resize_commit = transaction.resize_commit;
                } else if let Some(resize_commit) = transaction.resize_commit {
                    self.release_detached_resize_capture(root_surface_id, resize_commit);
                }
            } else {
                retained.push(transaction);
            }
        }
        self.pending_surface_tree_transactions = retained;
        released
    }

    pub(crate) fn cancel_pending_surface_trees_for_surface(
        &mut self,
        surface_id: u32,
        reason: AcquireWatchCancelReason,
    ) {
        let mut retained = Vec::new();
        let mut callbacks = Vec::new();
        for transaction in std::mem::take(&mut self.pending_surface_tree_transactions) {
            if transaction
                .nodes
                .iter()
                .any(|(node_surface_id, _)| *node_surface_id == surface_id)
            {
                let root_surface_id = transaction.root_surface_id;
                let released = self.release_pending_surface_tree_transaction(transaction, reason);
                callbacks.extend(released.callbacks);
                if let Some(resize_commit) = released.resize_commit {
                    self.release_detached_resize_capture(root_surface_id, resize_commit);
                }
            } else {
                retained.push(transaction);
            }
        }
        self.pending_surface_tree_transactions = retained;
        self.complete_frame_callbacks(callbacks);
    }

    pub(crate) fn release_pending_surface_tree_transaction(
        &mut self,
        mut transaction: PendingSurfaceTreeTransaction,
        reason: AcquireWatchCancelReason,
    ) -> ReleasedSurfaceTreeState {
        if self.external_acquire_readiness {
            for dependency in &transaction.dependencies {
                self.pending_acquire_watch_changes
                    .push(AcquireWatchChange::Cancel {
                        commit_id: dependency.commit_id,
                        reason,
                    });
            }
        }
        let resize_commit =
            take_tree_resize_commit(transaction.root_surface_id, &mut transaction.nodes);
        self.release_resize_captures_for_tree_nodes(&transaction.nodes);
        ReleasedSurfaceTreeState {
            callbacks: self.take_unpublished_surface_tree_callbacks(transaction.nodes),
            resize_commit,
        }
    }

    pub(crate) fn release_unpublished_surface_tree_nodes(
        &mut self,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    ) {
        self.release_resize_captures_for_tree_nodes(&nodes);
        let callbacks = self.take_unpublished_surface_tree_callbacks(nodes);
        self.complete_frame_callbacks(callbacks);
    }

    pub(crate) fn release_resize_captures_for_tree_nodes(
        &mut self,
        nodes: &[(u32, CachedSubsurfaceCommit)],
    ) {
        for (surface_id, commit) in nodes {
            let resize = match commit.attachment.as_ref() {
                Some(PendingSurfaceAttachment::Buffer(buffer)) => {
                    buffer.resize_commit.as_deref().copied()
                }
                _ => commit.resize_commit,
            };
            if let Some(resize) = resize {
                self.release_resize_capture(*surface_id, resize.commit_sequence);
            }
        }
    }

    pub(crate) fn install_tree_resize_commit(
        &self,
        root_surface_id: u32,
        nodes: &mut [(u32, CachedSubsurfaceCommit)],
        resize_commit: ResizeCommitSnapshot,
    ) {
        let Some((_, root)) = nodes
            .iter_mut()
            .find(|(surface_id, _)| *surface_id == root_surface_id)
        else {
            return;
        };
        if let Some(PendingSurfaceAttachment::Buffer(buffer)) = root.attachment.as_mut() {
            let resize_commit =
                self.snapshot_resize_commit_for_buffer(root_surface_id, resize_commit, buffer);
            buffer.resize_commit = Some(Box::new(resize_commit));
            buffer.resize_capture_finalized = true;
        } else {
            root.resize_commit = Some(resize_commit);
            root.resize_capture_finalized = true;
        }
    }

    pub(crate) fn release_detached_resize_capture(
        &mut self,
        surface_id: u32,
        resize_commit: ResizeCommitSnapshot,
    ) {
        self.release_resize_capture(surface_id, resize_commit.commit_sequence);
    }

    pub(crate) fn take_unpublished_surface_tree_callbacks(
        &mut self,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    ) -> Vec<wl_callback::WlCallback> {
        let mut callbacks = Vec::new();
        for (_, commit) in nodes {
            callbacks.extend(commit.frame_callbacks);
            for feedback in commit.presentation_feedbacks {
                feedback.feedback.discarded();
            }
            if let Some(PendingSurfaceAttachment::Buffer(buffer)) = commit.attachment {
                buffer.release_target().release();
            }
        }
        callbacks
    }

    pub(crate) fn cache_synchronized_subsurface_commit(
        &mut self,
        surface_id: u32,
        mut commit: CachedSubsurfaceCommit,
    ) {
        let buffer_id = commit
            .attachment
            .as_ref()
            .and_then(|attachment| match attachment {
                PendingSurfaceAttachment::Buffer(buffer) => Some(buffer.data.buffer_id().get()),
                PendingSurfaceAttachment::RemoveContent => None,
            });
        if let (
            Some(PendingSurfaceAttachment::Buffer(buffer)),
            Some(CapturedExplicitSyncState {
                release: Some(release),
                ..
            }),
        ) = (commit.attachment.as_mut(), commit.explicit_sync.as_ref())
        {
            buffer.explicit_release = Some(release.clone());
        }
        let merged = self.subsurface_transactions.has_cached_commit(surface_id);
        if let Some(release) = self
            .subsurface_transactions
            .cache_commit(surface_id, commit)
        {
            release.release();
        }
        self.subsurface_transaction_metrics
            .synchronized_child_commits_cached = self
            .subsurface_transaction_metrics
            .synchronized_child_commits_cached
            .saturating_add(1);
        if merged {
            self.subsurface_transaction_metrics.cached_commits_merged = self
                .subsurface_transaction_metrics
                .cached_commits_merged
                .saturating_add(1);
        }
        self.subsurface_transaction_metrics.maximum_cached_nodes = self
            .subsurface_transaction_metrics
            .maximum_cached_nodes
            .max(self.subsurface_transactions.cached_node_count());
        self.subsurface_transaction_metrics.maximum_tree_depth = self
            .subsurface_transaction_metrics
            .maximum_tree_depth
            .max(self.subsurface_transactions.maximum_depth());
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: subsurface_tx surface={surface_id} parent={:?} requested_mode={:?} effective_mode=sync decision=cached buffer_id={buffer_id:?}",
                self.subsurface_transactions.parent(surface_id),
                self.subsurface_transactions.requested_mode(surface_id),
            );
        }
    }

    pub(crate) fn apply_pending_subsurface_parent_state(&mut self, parent_id: u32) -> bool {
        let positions = self
            .subsurface_transactions
            .take_pending_positions_for_parent(parent_id);
        let mut changed = false;
        for (surface_id, x, y) in positions {
            let placement = SurfacePlacement::subsurface(parent_id, x, y);
            changed |= self.surface_placement(surface_id) != placement;
            self.set_surface_placement(surface_id, SurfacePlacement::subsurface(parent_id, x, y));
        }
        changed |= self.apply_pending_subsurface_stack_for_parent(parent_id);
        if changed {
            self.advance_render_generation(RenderGenerationCause::SurfaceCommit);
        }
        changed
    }

    pub(crate) fn publish_surface_tree(
        &mut self,
        root_id: u32,
        root_commit: CachedSubsurfaceCommit,
        commits: Vec<(u32, CachedSubsurfaceCommit)>,
    ) {
        let changed_nodes = commits.len().saturating_add(1);
        let maximum_wait_ms = std::iter::once(&root_commit)
            .chain(commits.iter().map(|(_, commit)| commit))
            .map(|commit| u64::try_from(commit.cached_at.elapsed().as_millis()).unwrap_or(u64::MAX))
            .max()
            .unwrap_or(0);
        self.subsurface_transaction_metrics
            .maximum_transaction_wait_ms = self
            .subsurface_transaction_metrics
            .maximum_transaction_wait_ms
            .max(maximum_wait_ms);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: subsurface_tx root={root_id} decision=prepared changed_nodes={changed_nodes}",
            );
        }
        self.begin_surface_tree_publication();
        self.apply_cached_subsurface_commit(root_id, root_commit);
        self.apply_pending_pointer_constraint_state_for_surface(root_id);
        self.apply_pending_subsurface_parent_state(root_id);
        for (surface_id, commit) in commits {
            self.apply_pending_subsurface_parent_state(surface_id);
            self.apply_cached_subsurface_commit(surface_id, commit);
        }
        self.finish_surface_tree_publication();
        self.debug_assert_surface_tree_invariants();
        self.subsurface_transaction_metrics
            .tree_transactions_published = self
            .subsurface_transaction_metrics
            .tree_transactions_published
            .saturating_add(1);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: subsurface_tx root={root_id} decision=published changed_nodes={} tree_generation={}",
                changed_nodes, self.render_generation,
            );
        }
    }

    pub(crate) fn apply_cached_subsurface_commit(
        &mut self,
        surface_id: u32,
        commit: CachedSubsurfaceCommit,
    ) {
        let CachedSubsurfaceCommit {
            commit_sequence,
            attachment,
            damage,
            frame_callbacks,
            explicit_sync,
            offset,
            viewport_destination,
            buffer_scale,
            input_region,
            presentation_feedbacks,
            resize_commit,
            resize_capture_finalized,
            window_geometry_changed,
            cached_at: _,
        } = commit;
        let Some(surface) = self.surface_resource_by_id(surface_id) else {
            return;
        };
        let Some(data) = surface.data::<SurfaceData>() else {
            return;
        };
        let surface_size = data.apply_viewport_change(viewport_destination);
        let committed_buffer_scale = data.apply_buffer_scale_change(buffer_scale);
        let input_region_changed = data.apply_input_region_change(input_region);
        let damage = damage.or(window_geometry_changed.then_some(RenderableSurfaceDamage::Full));
        match attachment {
            Some(PendingSurfaceAttachment::Buffer(mut pending)) => {
                if let Some((x, y)) = offset {
                    pending.x = x;
                    pending.y = y;
                }
                debug_assert!(pending.surface_size.is_some());
                self.commit_surface_request_with_captured_sync(
                    surface_id,
                    commit_sequence,
                    SurfacePublicationSource::SurfaceTree,
                    pending,
                    damage.unwrap_or_else(RenderableSurfaceDamage::full),
                    frame_callbacks,
                    explicit_sync,
                );
            }
            Some(PendingSurfaceAttachment::RemoveContent) => {
                if let Some(explicit_sync) = explicit_sync
                    && (explicit_sync.acquire.is_some() || explicit_sync.release.is_some())
                {
                    explicit_sync.state.post_error(
                        SYNCOBJ_SURFACE_ERROR_NO_BUFFER,
                        "explicit sync points were set without an attached buffer",
                    );
                    return;
                }
                if self.is_cursor_surface(surface_id) {
                    self.commit_cursor_surface_removal_request(surface_id, data, None);
                    self.complete_frame_callbacks(frame_callbacks);
                } else {
                    self.commit_surface_remove_content(
                        surface_id,
                        commit_sequence,
                        frame_callbacks,
                        SurfacePublicationSource::RemoveContent,
                    );
                }
            }
            None => {
                let explicit_sync = match explicit_sync {
                    Some(explicit_sync)
                        if explicit_sync.acquire.is_some() || explicit_sync.release.is_some() =>
                    {
                        explicit_sync.state.post_error(
                            SYNCOBJ_SURFACE_ERROR_NO_BUFFER,
                            "explicit sync points were set without an attached buffer",
                        );
                        return;
                    }
                    Some(explicit_sync) => Some(explicit_sync.state),
                    None => None,
                };
                self.commit_surface_without_buffer(
                    surface_id,
                    data,
                    BufferlessSurfaceCommitState {
                        commit_sequence,
                        damage,
                        explicit_sync,
                        surface_size,
                        buffer_scale: committed_buffer_scale,
                        resize_commit,
                        resize_capture_finalized,
                    },
                );
                if self
                    .renderable_surfaces
                    .iter()
                    .any(|surface| surface.surface_id == surface_id)
                {
                    self.pending_frame_callbacks.extend(frame_callbacks);
                } else {
                    self.complete_frame_callbacks(frame_callbacks);
                }
            }
        }
        if input_region_changed {
            self.refresh_pointer_focus_at_last_position();
        }
        self.pending_presentation_feedbacks
            .extend(presentation_feedbacks);
    }

    pub(crate) fn pending_stack_for_parent(&mut self, parent_id: u32) -> &mut Vec<u32> {
        self.pending_subsurface_stacks
            .entry(parent_id)
            .or_insert_with(|| {
                self.committed_subsurface_stacks
                    .get(&parent_id)
                    .cloned()
                    .unwrap_or_else(|| vec![parent_id])
            })
    }

    pub(crate) fn restack_subsurface(
        &mut self,
        surface_id: u32,
        parent_id: u32,
        reference_id: u32,
        above: bool,
    ) -> bool {
        if reference_id == surface_id {
            return false;
        }
        let valid_reference = reference_id == parent_id
            || self
                .surface_placements
                .get(&reference_id)
                .is_some_and(|placement| placement.parent_surface_id == Some(parent_id));
        if !valid_reference {
            return false;
        }

        let stack = self.pending_stack_for_parent(parent_id);
        stack.retain(|id| *id == parent_id || *id != surface_id);
        if !stack.contains(&parent_id) {
            stack.insert(0, parent_id);
        }
        let Some(reference_index) = stack.iter().position(|id| *id == reference_id) else {
            return false;
        };
        let insert_index = if above {
            reference_index + 1
        } else {
            reference_index
        };
        stack.insert(insert_index.min(stack.len()), surface_id);
        true
    }

    pub(crate) fn apply_pending_subsurface_stack_for_parent(&mut self, parent_id: u32) -> bool {
        let Some(mut stack) = self.pending_subsurface_stacks.remove(&parent_id) else {
            return false;
        };
        stack.retain(|id| {
            *id == parent_id
                || self
                    .surface_placements
                    .get(id)
                    .is_some_and(|placement| placement.parent_surface_id == Some(parent_id))
        });
        if !stack.contains(&parent_id) {
            stack.insert(0, parent_id);
        }
        stack.dedup();
        let changed = self
            .committed_subsurface_stacks
            .get(&parent_id)
            .is_none_or(|current| *current != stack);
        self.committed_subsurface_stacks.insert(parent_id, stack);
        if changed {
            self.reorder_renderable_surfaces_by_committed_stack();
            self.refresh_pointer_focus_at_last_position();
        }
        changed
    }

    pub(crate) fn cleanup_subsurface_stack_state_for_surface(&mut self, surface_id: u32) {
        self.committed_subsurface_stacks.remove(&surface_id);
        self.pending_subsurface_stacks.remove(&surface_id);
        for stack in self.committed_subsurface_stacks.values_mut() {
            stack.retain(|id| *id != surface_id);
            stack.dedup();
        }
        for stack in self.pending_subsurface_stacks.values_mut() {
            stack.retain(|id| *id != surface_id);
            stack.dedup();
        }
        self.committed_subsurface_stacks.retain(|parent_id, stack| {
            self.surface_resources.contains_key(parent_id) && stack.iter().any(|id| id != parent_id)
        });
        self.pending_subsurface_stacks.retain(|parent_id, stack| {
            self.surface_resources.contains_key(parent_id) && stack.iter().any(|id| id != parent_id)
        });
        self.reorder_renderable_surfaces_by_committed_stack();
    }

    pub(crate) fn destroy_subsurface_role(&mut self, surface_id: u32) {
        let parent_id = self.subsurface_transactions.parent(surface_id);
        if let Some(commit) = self.subsurface_transactions.remove_role(surface_id) {
            self.release_cached_subsurface_commits(vec![commit]);
        }
        self.unmap_surface_content(surface_id);
        self.set_surface_placement(surface_id, SurfacePlacement::root());
        for stack in self.committed_subsurface_stacks.values_mut() {
            stack.retain(|id| *id != surface_id);
        }
        for stack in self.pending_subsurface_stacks.values_mut() {
            stack.retain(|id| *id != surface_id);
        }
        self.reorder_renderable_surfaces_by_committed_stack();
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: subsurface_tx surface={surface_id} parent={parent_id:?} decision=destroyed reason=role_destroyed"
            );
        }
    }

    pub(crate) fn release_cached_subsurface_commits(
        &mut self,
        commits: Vec<CachedSubsurfaceCommit>,
    ) {
        for commit in commits {
            for feedback in commit.presentation_feedbacks {
                feedback.feedback.discarded();
            }
            if let Some(PendingSurfaceAttachment::Buffer(buffer)) = commit.attachment {
                buffer.release_target().release();
            }
        }
    }

    pub(crate) fn debug_assert_surface_tree_invariants(&self) {
        #[cfg(debug_assertions)]
        {
            let mut renderable_ids = HashSet::new();
            for surface in &self.renderable_surfaces {
                debug_assert!(renderable_ids.insert(surface.surface_id));
                if let Some(parent_id) = surface.placement.parent_surface_id {
                    debug_assert!(self.surface_resources.contains_key(&parent_id));
                }
            }
            for (parent_id, stack) in &self.committed_subsurface_stacks {
                let mut stack_ids = HashSet::new();
                debug_assert!(stack.iter().all(|surface_id| stack_ids.insert(*surface_id)));
                debug_assert!(stack.contains(parent_id));
            }
        }
    }

    pub(crate) fn take_surface_presentation_feedbacks(
        &mut self,
        surface_id: u32,
    ) -> Vec<PendingPresentationFeedback> {
        self.pending_surface_presentation_feedbacks
            .remove(&surface_id)
            .unwrap_or_default()
    }

    pub(crate) fn reorder_renderable_surfaces_by_committed_stack(&mut self) -> bool {
        if self.renderable_surfaces.len() <= 1 {
            return false;
        }

        let original_order = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let mut by_id = self
            .renderable_surfaces
            .drain(..)
            .map(|surface| (surface.surface_id, surface))
            .collect::<HashMap<_, _>>();
        let visible_ids = by_id.keys().copied().collect::<HashSet<_>>();
        let mut ordered_ids = Vec::new();
        let root_ids = original_order
            .iter()
            .copied()
            .filter(|surface_id| {
                self.surface_placements
                    .get(surface_id)
                    .and_then(|placement| placement.parent_surface_id)
                    .is_none_or(|parent_id| !visible_ids.contains(&parent_id))
            })
            .collect::<Vec<_>>();

        for root_id in root_ids {
            self.append_surface_tree_order(root_id, &visible_ids, &mut ordered_ids);
        }
        for surface_id in &original_order {
            if visible_ids.contains(surface_id) && !ordered_ids.contains(surface_id) {
                self.append_surface_tree_order(*surface_id, &visible_ids, &mut ordered_ids);
            }
        }

        self.renderable_surfaces = ordered_ids
            .into_iter()
            .filter_map(|surface_id| by_id.remove(&surface_id))
            .collect();
        let changed = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .ne(original_order);
        if changed {
            self.invalidate_surface_origin_cache();
        }
        changed
    }

    pub(crate) fn append_surface_tree_order(
        &self,
        surface_id: u32,
        visible_ids: &HashSet<u32>,
        ordered_ids: &mut Vec<u32>,
    ) {
        if !visible_ids.contains(&surface_id) || ordered_ids.contains(&surface_id) {
            return;
        }

        if let Some(stack) = self.committed_subsurface_stacks.get(&surface_id) {
            for stacked_id in stack {
                if *stacked_id == surface_id {
                    ordered_ids.push(surface_id);
                } else {
                    self.append_surface_tree_order(*stacked_id, visible_ids, ordered_ids);
                }
            }
        } else {
            ordered_ids.push(surface_id);
        }

        let children = self
            .surface_placements
            .iter()
            .filter_map(|(child_id, placement)| {
                (placement.parent_surface_id == Some(surface_id)
                    && visible_ids.contains(child_id)
                    && !ordered_ids.contains(child_id))
                .then_some(*child_id)
            })
            .collect::<Vec<_>>();
        for child_id in children {
            self.append_surface_tree_order(child_id, visible_ids, ordered_ids);
        }
    }

    pub(crate) fn set_surface_placement(
        &mut self,
        surface_id: u32,
        placement: SurfacePlacement,
    ) -> bool {
        self.set_surface_placement_with_cause(
            surface_id,
            placement,
            RenderGenerationCause::SurfacePlacement,
        )
    }

    pub(crate) fn set_surface_placement_with_cause(
        &mut self,
        surface_id: u32,
        placement: SurfacePlacement,
        cause: RenderGenerationCause,
    ) -> bool {
        if self.surface_placement(surface_id) == placement {
            return false;
        }

        self.store_surface_placement(surface_id, placement);
        if let Some(visual) = self.toplevel_visual_geometries.get_mut(&surface_id)
            && visual.active_resize.is_none()
        {
            visual.placement = placement;
        }

        if let Some(surface) = self
            .renderable_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == surface_id)
        {
            surface.placement = placement;
            let root_surface_id = self.root_surface_id_for_surface(surface_id);
            if self
                .toplevel_visual_geometries
                .contains_key(&root_surface_id)
            {
                self.update_toplevel_visual_render_assignment(root_surface_id);
            }
            self.advance_render_generation(cause);
            return true;
        }

        false
    }

    pub(crate) fn refresh_surface_origin_cache(&mut self) {
        if self.surface_origin_cache_generation != Some(self.render_generation)
            || self.surface_origin_cache.len() != self.renderable_surfaces.len()
        {
            self.surface_origin_cache = render::surface_origins(&self.renderable_surfaces);
            self.surface_origin_cache_generation = Some(self.render_generation);
        }
    }

    pub(crate) fn invalidate_surface_origin_cache(&mut self) {
        self.surface_origin_cache_generation = None;
    }

    pub(crate) fn raise_renderable_surface_tree(&mut self, surface_id: u32) -> bool {
        let tree_ids = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .filter(|candidate_id| self.surface_is_descendant_of(*candidate_id, surface_id))
            .collect::<HashSet<_>>();
        if tree_ids.is_empty() {
            return false;
        }

        let original_order = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let mut tree = Vec::new();
        let mut lower = Vec::with_capacity(self.renderable_surfaces.len());
        for surface in self.renderable_surfaces.drain(..) {
            if tree_ids.contains(&surface.surface_id) {
                tree.push(surface);
            } else {
                lower.push(surface);
            }
        }
        lower.extend(tree);
        let changed = lower
            .iter()
            .map(|surface| surface.surface_id)
            .ne(original_order);
        self.renderable_surfaces = lower;
        if changed {
            self.invalidate_surface_origin_cache();
        }
        changed
    }
}
