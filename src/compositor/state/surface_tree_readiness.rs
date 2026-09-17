use super::*;

impl CompositorState {
    pub(in crate::compositor) fn commit_ready_surface_tree_transactions(&mut self) {
        self.revalidate_pending_commit_timing_targets();
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
        for transaction in &transactions {
            for (surface_id, commit) in &transaction.nodes {
                if commit.pacing.fifo_wait_barrier {
                    if commit.pacing.fifo_wait_ignored_for_synchronized_subsurface {
                        self.surface_pacing_metrics
                            .waits_ignored_for_synchronized_subsurfaces = self
                            .surface_pacing_metrics
                            .waits_ignored_for_synchronized_subsurfaces
                            .saturating_add(1);
                    } else if self.active_fifo_barriers.contains_key(surface_id) {
                        self.surface_pacing_metrics.waits_blocked =
                            self.surface_pacing_metrics.waits_blocked.saturating_add(1);
                        self.trace_surface_pipeline_event(
                            SurfacePipelineEvent::FifoWaitBlocked,
                            *surface_id,
                            commit.commit_sequence,
                            None,
                            None,
                            Some(transaction.id.get()),
                            None,
                            None,
                            None,
                        );
                    }
                }
                if commit
                    .pacing
                    .commit_timing
                    .is_some_and(|timing| !timing.is_due(self.presentation_clock))
                {
                    self.surface_pacing_metrics.transactions_blocked_by_timing = self
                        .surface_pacing_metrics
                        .transactions_blocked_by_timing
                        .saturating_add(1);
                    self.trace_surface_pipeline_event(
                        SurfacePipelineEvent::CommitTimingBlocked,
                        *surface_id,
                        commit.commit_sequence,
                        None,
                        None,
                        Some(transaction.id.get()),
                        None,
                        None,
                        None,
                    );
                }
            }
        }
        let mut pacing_deadline_changed = false;
        loop {
            let root_heads = transactions
                .iter()
                .enumerate()
                .filter_map(|(index, transaction)| {
                    (!transactions[..index]
                        .iter()
                        .any(|previous| previous.root_surface_id == transaction.root_surface_id))
                    .then_some(index)
                })
                .collect::<Vec<_>>();
            let mut selected = None;
            for index in root_heads {
                let transaction = &transactions[index];
                if self.transaction_is_ready(transaction) {
                    selected = Some(index);
                    break;
                }
            }
            let Some(index) = selected else {
                break;
            };
            let transaction = transactions.remove(index);
            if let Some((surface_id, decision)) =
                self.surface_tree_async_publication_rejection(&transaction)
            {
                let canceled_root_surface_id = transaction.root_surface_id;
                let canceled_refs = transaction
                    .nodes
                    .iter()
                    .map(|(surface_id, commit)| commit.content_update_ref(*surface_id))
                    .collect();
                let (commit_sequence, buffer_id) = transaction
                    .nodes
                    .iter()
                    .filter(|(node_surface_id, _)| *node_surface_id == surface_id)
                    .max_by_key(|(_, commit)| commit.commit_sequence)
                    .map_or((SurfaceCommitSequence::initial(), None), |(_, commit)| {
                        (
                            commit.commit_sequence,
                            commit
                                .attachment
                                .as_ref()
                                .and_then(|attachment| match attachment {
                                    PendingSurfaceAttachment::Buffer(buffer) => {
                                        Some(buffer.data.buffer_id())
                                    }
                                    PendingSurfaceAttachment::RemoveContent => None,
                                }),
                        )
                    });
                if matches!(
                    decision,
                    SurfacePublicationDecision::SurfaceGone
                        | SurfacePublicationDecision::OwnerGone
                        | SurfacePublicationDecision::TerminalClient
                        | SurfacePublicationDecision::StaleSurfaceGeneration
                ) {
                    let has_node = transaction
                        .nodes
                        .iter()
                        .any(|(node_surface_id, _)| *node_surface_id == surface_id);
                    if has_node {
                        self.trace_surface_pipeline_event_with_reason(
                            SurfacePipelineEvent::AcquireReadyDiscarded,
                            surface_id,
                            commit_sequence,
                            buffer_id.map(BufferId::get),
                            None,
                            Some(transaction.id.get()),
                            None,
                            None,
                            None,
                            decision.pipeline_rejection_reason(),
                        );
                    }
                }
                self.record_surface_publication_rejection(
                    surface_id,
                    commit_sequence,
                    buffer_id,
                    SurfacePublicationSource::SurfaceTree,
                    decision,
                );
                self.discard_surface_tree_transaction_with_decision(transaction, decision);
                pacing_deadline_changed |= self.discard_surface_tree_dependents_from_queue(
                    &mut transactions,
                    canceled_root_surface_id,
                    canceled_refs,
                    decision,
                );
                continue;
            }
            pacing_deadline_changed |= transaction.commit_timing_readiness.is_some();
            let mut transaction = transaction;
            if let Some(readiness) = transaction.commit_timing_readiness {
                for (_, commit) in &mut transaction.nodes {
                    if commit.pacing.commit_timing.is_some() {
                        commit.pacing.commit_timing_readiness = Some(readiness);
                    }
                }
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
            for (surface_id, commit) in &transaction.nodes {
                if commit.pacing.fifo_wait_barrier
                    && !commit.pacing.fifo_wait_ignored_for_synchronized_subsurface
                {
                    self.trace_surface_pipeline_event(
                        SurfacePipelineEvent::FifoWaitReleased,
                        *surface_id,
                        commit.commit_sequence,
                        None,
                        None,
                        Some(transaction.id.get()),
                        None,
                        None,
                        None,
                    );
                }
                if commit.pacing.commit_timing.is_some() {
                    self.trace_surface_pipeline_event(
                        SurfacePipelineEvent::CommitTimingReleased,
                        *surface_id,
                        commit.commit_sequence,
                        None,
                        None,
                        Some(transaction.id.get()),
                        None,
                        None,
                        None,
                    );
                }
                self.trace_surface_pipeline_event(
                    SurfacePipelineEvent::TransactionPromoted,
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
            self.publish_surface_tree_nodes(transaction);
        }
        self.pending_surface_tree_transactions = transactions;
        if pacing_deadline_changed {
            self.invalidate_surface_pacing_deadline_cache();
        }
        self.rebuild_scene_work_index();
    }
}
