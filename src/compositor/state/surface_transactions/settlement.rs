use super::*;
use crate::compositor::subsurface::ContentUpdateRef;

impl CompositorState {}

impl CompositorState {
    pub(in crate::compositor) fn discard_surface_tree_transaction_with_decision(
        &mut self,
        transaction: PendingSurfaceTreeTransaction,
        decision: SurfacePublicationDecision,
    ) {
        let root_surface_id = transaction.root_surface_id;
        let released = self.release_pending_surface_tree_transaction(
            transaction,
            AcquireWatchCancelReason::SurfaceDestroyed,
        );
        if let Some(resize_commit) = released.resize_commit {
            self.release_detached_resize_capture(root_surface_id, resize_commit);
        }
        if decision == SurfacePublicationDecision::TerminalClient {
            self.discard_frame_callbacks(released.callbacks);
        } else {
            self.complete_frame_callbacks(released.callbacks);
        }
    }

    pub(in crate::compositor) fn cancel_pending_surface_trees_for_root(
        &mut self,
        root_surface_id: u32,
        reason: AcquireWatchCancelReason,
    ) -> ReleasedSurfaceTreeState {
        let transactions = self
            .surface_transactions
            .take_pending_trees_for_root(root_surface_id);
        let mut canceled_refs = Vec::new();
        let mut pacing_deadline_changed = false;
        let mut released = ReleasedSurfaceTreeState {
            callbacks: Vec::new(),
            resize_commit: None,
        };
        for transaction in transactions {
            canceled_refs.extend(
                transaction
                    .nodes
                    .iter()
                    .map(|(surface_id, commit)| commit.content_update_ref(*surface_id)),
            );
            pacing_deadline_changed |= transaction.commit_timing_readiness.is_some();
            let transaction = self.release_pending_surface_tree_transaction(transaction, reason);
            self.surface_transactions.metrics.root_wide_supersessions = self
                .surface_transactions
                .metrics
                .root_wide_supersessions
                .saturating_add(1);
            released.callbacks.extend(transaction.callbacks);
            if released.resize_commit.is_none() {
                released.resize_commit = transaction.resize_commit;
            } else {
                if let Some(resize_commit) = transaction.resize_commit {
                    self.release_detached_resize_capture(root_surface_id, resize_commit);
                }
            }
        }
        let (callbacks, dependent_pacing_deadline_changed) =
            self.cancel_pending_surface_tree_dependents(canceled_refs, reason);
        pacing_deadline_changed |= dependent_pacing_deadline_changed;
        released.callbacks.extend(callbacks);
        if pacing_deadline_changed {
            self.invalidate_surface_pacing_deadline_cache();
        }
        self.rebuild_scene_work_index();
        released
    }

    pub(in crate::compositor) fn cancel_pending_surface_trees_for_surface(
        &mut self,
        surface_id: u32,
        reason: AcquireWatchCancelReason,
    ) {
        let transactions = self
            .surface_transactions
            .take_pending_trees_for_surface(surface_id);
        let mut canceled_refs = Vec::new();
        let mut pacing_deadline_changed = false;
        let mut callbacks = Vec::new();
        for transaction in transactions {
            canceled_refs.extend(
                transaction
                    .nodes
                    .iter()
                    .map(|(node_surface_id, commit)| commit.content_update_ref(*node_surface_id)),
            );
            pacing_deadline_changed |= transaction.commit_timing_readiness.is_some();
            let root_surface_id = transaction.root_surface_id;
            let released = self.release_pending_surface_tree_transaction(transaction, reason);
            callbacks.extend(released.callbacks);
            if let Some(resize_commit) = released.resize_commit {
                self.release_detached_resize_capture(root_surface_id, resize_commit);
            }
        }
        let (dependent_callbacks, dependent_pacing_deadline_changed) =
            self.cancel_pending_surface_tree_dependents(canceled_refs, reason);
        pacing_deadline_changed |= dependent_pacing_deadline_changed;
        callbacks.extend(dependent_callbacks);
        if pacing_deadline_changed {
            self.invalidate_surface_pacing_deadline_cache();
        }
        self.rebuild_scene_work_index();
        self.complete_frame_callbacks(callbacks);
    }

    pub(in crate::compositor) fn cancel_pending_surface_trees_for_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        reason: AcquireWatchCancelReason,
    ) {
        let tree_roots = self
            .surface_transactions
            .pending_trees()
            .filter(|transaction| {
                transaction.nodes.iter().any(|(_, commit)| {
                    commit.attachment.as_ref().is_some_and(|attachment| {
                        matches!(
                            attachment,
                            PendingSurfaceAttachment::Buffer(pending)
                                if same_wayland_resource(&pending.resource, buffer)
                        )
                    })
                })
            })
            .map(|transaction| transaction.root_surface_id)
            .collect::<Vec<_>>();
        let mut callbacks = Vec::new();
        for root_surface_id in tree_roots {
            let released = self.cancel_pending_surface_trees_for_root(root_surface_id, reason);
            if let Some(resize_commit) = released.resize_commit {
                self.release_detached_resize_capture(root_surface_id, resize_commit);
            }
            callbacks.extend(released.callbacks);
        }
        self.complete_frame_callbacks(callbacks);
    }

    fn cancel_pending_surface_tree_dependents(
        &mut self,
        canceled_refs: Vec<ContentUpdateRef>,
        reason: AcquireWatchCancelReason,
    ) -> (Vec<wl_callback::WlCallback>, bool) {
        let mut canceled_refs = canceled_refs;
        let mut callbacks = Vec::new();
        let mut pacing_deadline_changed = false;
        loop {
            let dependents = self
                .surface_transactions
                .take_pending_tree_dependents(&canceled_refs);
            let mut newly_canceled_refs = Vec::new();
            for transaction in dependents {
                newly_canceled_refs.extend(
                    transaction
                        .nodes
                        .iter()
                        .map(|(surface_id, commit)| commit.content_update_ref(*surface_id)),
                );
                pacing_deadline_changed |= transaction.commit_timing_readiness.is_some();
                let root_surface_id = transaction.root_surface_id;
                let released = self.release_pending_surface_tree_transaction(transaction, reason);
                callbacks.extend(released.callbacks);
                if let Some(resize_commit) = released.resize_commit {
                    self.release_detached_resize_capture(root_surface_id, resize_commit);
                }
            }
            if newly_canceled_refs.is_empty() {
                break;
            }
            canceled_refs.extend(newly_canceled_refs);
        }
        (callbacks, pacing_deadline_changed)
    }

    pub(in crate::compositor) fn discard_surface_tree_dependents_from_queue(
        &mut self,
        transactions: &mut Vec<PendingSurfaceTreeTransaction>,
        canceled_root_surface_id: u32,
        canceled_refs: Vec<ContentUpdateRef>,
        decision: SurfacePublicationDecision,
    ) -> bool {
        let mut canceled_roots = vec![canceled_root_surface_id];
        let mut canceled_refs = canceled_refs;
        let mut pacing_deadline_changed = false;
        loop {
            let mut retained = Vec::new();
            let mut newly_canceled_roots = Vec::new();
            let mut newly_canceled_refs = Vec::new();
            for transaction in std::mem::take(transactions) {
                if canceled_roots.contains(&transaction.root_surface_id)
                    || transaction_references_any_content_update(&transaction, &canceled_refs)
                {
                    newly_canceled_roots.push(transaction.root_surface_id);
                    newly_canceled_refs.extend(
                        transaction
                            .nodes
                            .iter()
                            .map(|(surface_id, commit)| commit.content_update_ref(*surface_id)),
                    );
                    pacing_deadline_changed |= transaction.commit_timing_readiness.is_some();
                    let root_surface_id = transaction.root_surface_id;
                    let released = self.release_pending_surface_tree_transaction(
                        transaction,
                        AcquireWatchCancelReason::SurfaceDestroyed,
                    );
                    if let Some(resize_commit) = released.resize_commit {
                        self.release_detached_resize_capture(root_surface_id, resize_commit);
                    }
                    if decision == SurfacePublicationDecision::TerminalClient {
                        self.discard_frame_callbacks(released.callbacks);
                    } else {
                        self.complete_frame_callbacks(released.callbacks);
                    }
                } else {
                    retained.push(transaction);
                }
            }
            *transactions = retained;
            if newly_canceled_roots.is_empty() && newly_canceled_refs.is_empty() {
                break;
            }
            canceled_roots.extend(newly_canceled_roots);
            canceled_refs.extend(newly_canceled_refs);
        }
        pacing_deadline_changed
    }

    pub(in crate::compositor) fn release_pending_surface_tree_transaction(
        &mut self,
        mut transaction: PendingSurfaceTreeTransaction,
        reason: AcquireWatchCancelReason,
    ) -> ReleasedSurfaceTreeState {
        for (surface_id, commit) in &transaction.nodes {
            self.trace_surface_pipeline_event(
                SurfacePipelineEvent::TransactionAbandoned,
                *surface_id,
                commit.commit_sequence,
                commit
                    .attachment
                    .as_ref()
                    .and_then(|attachment| match attachment {
                        PendingSurfaceAttachment::Buffer(buffer) => {
                            Some(buffer.data.buffer_id().get())
                        }
                        PendingSurfaceAttachment::RemoveContent => None,
                    }),
                None,
                Some(transaction.id.get()),
                None,
                None,
                None,
            );
        }
        if self.external_acquire_readiness {
            for dependency in &transaction.dependencies {
                if dependency.state == PendingAcquireState::Ready {
                    continue;
                }
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

    pub(in crate::compositor) fn release_unpublished_surface_tree_nodes(
        &mut self,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    ) {
        self.release_resize_captures_for_tree_nodes(&nodes);
        let callbacks = self.take_unpublished_surface_tree_callbacks(nodes);
        self.complete_frame_callbacks(callbacks);
    }

    pub(in crate::compositor) fn release_resize_captures_for_tree_nodes(
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

    pub(in crate::compositor) fn release_detached_resize_capture(
        &mut self,
        surface_id: u32,
        resize_commit: ResizeCommitSnapshot,
    ) {
        self.release_resize_capture(surface_id, resize_commit.commit_sequence);
    }

    pub(in crate::compositor) fn take_unpublished_surface_tree_callbacks(
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
                self.release_pending_surface_buffer(buffer);
            }
        }
        callbacks
    }
}
