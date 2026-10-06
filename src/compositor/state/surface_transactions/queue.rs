use super::state::SurfaceTransactionState;
use super::*;
use crate::compositor::explicit_sync::{AcquireCommitId, PendingAcquireState};
use crate::compositor::subsurface::{ContentUpdateRef, PointerConstraintHintCommit};

pub(in crate::compositor) const MAX_SURFACE_TREE_TRANSACTIONS_PER_ROOT: usize = 8;

#[derive(Debug, Clone, Copy)]
pub(in crate::compositor) struct SurfaceTreeProgressAcquireReplacement {
    pub(in crate::compositor) surface_id: u32,
    pub(in crate::compositor) anchor_commit_id: SurfaceCommitId,
    pub(in crate::compositor) incoming_commit_id: SurfaceCommitId,
}

pub(in crate::compositor) fn surface_tree_merge_would_replace_progress_acquire(
    target: &PendingSurfaceTreeTransaction,
    incoming_nodes: &[(u32, CachedSubsurfaceCommit)],
    incoming_dependencies: &[SurfaceTreeAcquireDependency],
) -> Option<SurfaceTreeProgressAcquireReplacement> {
    target.dependencies.iter().find_map(|anchor_dependency| {
        if anchor_dependency.state == PendingAcquireState::Ready {
            return None;
        }
        let (surface_id, anchor_commit) = target.nodes.iter().find(|(surface_id, commit)| {
            *surface_id == anchor_dependency.surface_id
                && commit.commit_id == anchor_dependency.surface_commit_id
        })?;
        let Some(PendingSurfaceAttachment::Buffer(anchor_buffer)) =
            anchor_commit.attachment.as_ref()
        else {
            return None;
        };
        if anchor_buffer.resource.id().protocol_id() != anchor_dependency.buffer_id {
            return None;
        }
        let incoming_commit = incoming_nodes
            .iter()
            .find_map(|(incoming_surface_id, commit)| {
                (*incoming_surface_id == *surface_id).then_some(commit)
            })?;
        let Some(PendingSurfaceAttachment::Buffer(incoming_buffer)) =
            incoming_commit.attachment.as_ref()
        else {
            return None;
        };
        let incoming_buffer_id = incoming_buffer.resource.id().protocol_id();
        let incoming_has_unready_acquire = incoming_dependencies.iter().any(|dependency| {
            dependency.surface_id == *surface_id
                && dependency.surface_commit_id == incoming_commit.commit_id
                && dependency.buffer_id == incoming_buffer_id
                && dependency.state != PendingAcquireState::Ready
        });
        incoming_has_unready_acquire.then_some(SurfaceTreeProgressAcquireReplacement {
            surface_id: *surface_id,
            anchor_commit_id: anchor_commit.commit_id,
            incoming_commit_id: incoming_commit.commit_id,
        })
    })
}

pub(in crate::compositor) fn can_coalesce_pending_surface_tree_transaction(
    target: &PendingSurfaceTreeTransaction,
    incoming_nodes: &[(u32, CachedSubsurfaceCommit)],
) -> bool {
    if target.ordering() != TransactionOrdering::Coalescible
        || target
            .nodes
            .iter()
            .any(|(_, commit)| commit.lineage.merge_frozen)
        || incoming_nodes
            .iter()
            .any(|(_, commit)| commit.pacing.is_boundary() || commit.lineage.merge_frozen)
    {
        debug_assert_eq!(target.ordering(), TransactionOrdering::Coalescible);
        debug_assert!(
            incoming_nodes
                .iter()
                .all(|(_, commit)| !commit.pacing.is_boundary())
        );
        return false;
    }

    for (incoming_index, (surface_id, incoming)) in incoming_nodes.iter().enumerate() {
        if incoming_nodes[..incoming_index]
            .iter()
            .any(|(previous_surface_id, _)| previous_surface_id == surface_id)
        {
            debug_assert!(false, "coalescible candidate retains duplicate surfaces");
            return false;
        }
        let mut matching = target
            .nodes
            .iter()
            .filter(|(existing_surface_id, _)| existing_surface_id == surface_id);
        let Some((_, existing)) = matching.next() else {
            continue;
        };
        if matching.next().is_some() {
            debug_assert!(false, "coalescible transaction retains duplicate surfaces");
            return false;
        }
        if existing.commit_sequence >= incoming.commit_sequence {
            return false;
        }
        let Some(predecessor) = incoming.lineage.predecessor else {
            return false;
        };
        if predecessor.surface_id != *surface_id
            || predecessor.commit_sequence >= incoming.commit_sequence
            || !content_update_node_covers_ref(*surface_id, existing, predecessor)
        {
            return false;
        }
    }
    true
}

pub(in crate::compositor) fn normalize_external_content_dependencies_for_nodes(
    nodes: &[(u32, CachedSubsurfaceCommit)],
    dependencies: &mut Vec<ContentUpdateRef>,
) {
    let input = std::mem::take(dependencies);
    let normalized = input
        .into_iter()
        .filter(|dependency| node_index_covering_content_update_ref(nodes, *dependency).is_none())
        .fold(Vec::new(), |mut normalized, dependency| {
            if !normalized.contains(&dependency) {
                normalized.push(dependency);
            }
            normalized
        });
    *dependencies = normalized;
    debug_assert!(dependencies.iter().all(|dependency| {
        node_index_covering_content_update_ref(nodes, *dependency).is_none()
    }));
}

pub(in crate::compositor) fn normalize_transaction_external_content_dependencies(
    transaction: &mut PendingSurfaceTreeTransaction,
) {
    let dependencies = std::mem::take(&mut transaction.external_content_update_dependencies);
    transaction.external_content_update_dependencies = dependencies
        .into_iter()
        .filter(|dependency| !transaction_covers_content_update_ref(transaction, *dependency))
        .fold(Vec::new(), |mut normalized, dependency| {
            if !normalized.contains(&dependency) {
                normalized.push(dependency);
            }
            normalized
        });
    debug_assert!(
        transaction
            .external_content_update_dependencies
            .iter()
            .all(|dependency| !transaction_covers_content_update_ref(transaction, *dependency))
    );
}

pub(in crate::compositor) fn normalize_surface_tree_node_order(
    transaction: &mut PendingSurfaceTreeTransaction,
) {
    let node_count = transaction.nodes.len();
    if node_count < 2 {
        return;
    }
    let mut indegree = vec![0usize; node_count];
    let mut successors = vec![Vec::<usize>::new(); node_count];
    for (dependent_index, ((_, commit), indegree_entry)) in transaction
        .nodes
        .iter()
        .zip(indegree.iter_mut())
        .enumerate()
    {
        let mut dependencies = Vec::with_capacity(
            commit.lineage.child_dependencies.len()
                + usize::from(commit.lineage.predecessor.is_some()),
        );
        if let Some(predecessor) = commit.lineage.predecessor {
            dependencies.push(predecessor);
        }
        dependencies.extend(commit.lineage.child_dependencies.iter().copied());
        for dependency in dependencies {
            let Some(dependency_index) =
                transaction_node_index_covering_content_update_ref(transaction, dependency)
            else {
                continue;
            };
            if dependency_index == dependent_index
                || successors[dependency_index].contains(&dependent_index)
            {
                continue;
            }
            successors[dependency_index].push(dependent_index);
            *indegree_entry = indegree_entry.saturating_add(1);
        }
    }

    let mut selected = vec![false; node_count];
    let mut order = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        let Some(next) = (0..node_count).find(|index| !selected[*index] && indegree[*index] == 0)
        else {
            debug_assert!(
                false,
                "surface-tree content update dependency cycle detected"
            );
            return;
        };
        selected[next] = true;
        order.push(next);
        for successor in &successors[next] {
            indegree[*successor] = indegree[*successor].saturating_sub(1);
        }
    }
    if order
        .iter()
        .enumerate()
        .all(|(index, original)| index == *original)
    {
        return;
    }

    let old_nodes = std::mem::take(&mut transaction.nodes);
    let old_lifetimes = std::mem::replace(
        &mut transaction.publication_lifetimes,
        SurfaceTreeNodeLifetimes::Captured(Vec::new()),
    );
    let lifetimes = old_lifetimes
        .captured()
        .expect("surface-tree transactions use captured publication lifetimes")
        .to_vec();
    debug_assert_eq!(old_nodes.len(), lifetimes.len());
    let mut node_slots = old_nodes.into_iter().map(Some).collect::<Vec<_>>();
    let mut lifetime_slots = lifetimes.into_iter().map(Some).collect::<Vec<_>>();
    let mut reordered_nodes = Vec::with_capacity(node_count);
    let mut reordered_lifetimes = Vec::with_capacity(node_count);
    for index in order {
        reordered_nodes.push(node_slots[index].take().expect("node order index"));
        reordered_lifetimes.push(
            lifetime_slots[index]
                .take()
                .expect("node lifetime order index"),
        );
    }
    transaction.nodes = reordered_nodes;
    transaction.publication_lifetimes = SurfaceTreeNodeLifetimes::Captured(reordered_lifetimes);
}

impl SurfaceTransactionState {
    pub(in crate::compositor) fn pending_tree_count_for_root(&self, root_surface_id: u32) -> usize {
        self.pending_surface_tree_transactions
            .iter()
            .filter(|transaction| transaction.root_surface_id == root_surface_id)
            .count()
    }

    pub(in crate::compositor) fn take_pending_tree_at(
        &mut self,
        index: usize,
    ) -> Option<PendingSurfaceTreeTransaction> {
        (index < self.pending_surface_tree_transactions.len())
            .then(|| self.pending_surface_tree_transactions.remove(index))
    }

    pub(in crate::compositor) fn take_pending_trees_for_root(
        &mut self,
        root_surface_id: u32,
    ) -> Vec<PendingSurfaceTreeTransaction> {
        self.take_pending_trees_matching(|transaction| {
            transaction.root_surface_id == root_surface_id
        })
    }

    pub(in crate::compositor) fn take_pending_trees_for_surface(
        &mut self,
        surface_id: u32,
    ) -> Vec<PendingSurfaceTreeTransaction> {
        self.take_pending_trees_matching(|transaction| {
            transaction
                .nodes
                .iter()
                .any(|(node_surface_id, _)| *node_surface_id == surface_id)
        })
    }

    pub(in crate::compositor) fn take_pending_tree_dependents(
        &mut self,
        references: &[ContentUpdateRef],
    ) -> Vec<PendingSurfaceTreeTransaction> {
        self.take_pending_trees_matching(|transaction| {
            transaction_references_any_content_update(transaction, references)
        })
    }

    fn take_pending_trees_matching(
        &mut self,
        mut predicate: impl FnMut(&PendingSurfaceTreeTransaction) -> bool,
    ) -> Vec<PendingSurfaceTreeTransaction> {
        let mut retained = Vec::with_capacity(self.pending_surface_tree_transactions.len());
        let mut taken = Vec::new();
        for transaction in std::mem::take(&mut self.pending_surface_tree_transactions) {
            if predicate(&transaction) {
                taken.push(transaction);
            } else {
                retained.push(transaction);
            }
        }
        self.pending_surface_tree_transactions = retained;
        taken
    }

    pub(in crate::compositor) fn acquire_dependency(
        &self,
        commit_id: AcquireCommitId,
    ) -> Option<&SurfaceTreeAcquireDependency> {
        self.pending_surface_tree_transactions
            .iter()
            .flat_map(|transaction| &transaction.dependencies)
            .find(|dependency| dependency.commit_id == commit_id)
    }

    pub(in crate::compositor) fn acquire_dependency_mut(
        &mut self,
        commit_id: AcquireCommitId,
    ) -> Option<&mut SurfaceTreeAcquireDependency> {
        self.pending_surface_tree_transactions
            .iter_mut()
            .flat_map(|transaction| &mut transaction.dependencies)
            .find(|dependency| dependency.commit_id == commit_id)
    }

    pub(in crate::compositor) fn set_commit_timing_readiness(
        &mut self,
        transaction_id: SurfaceTreeTransactionId,
        readiness: Option<CommitTimingReadiness>,
    ) -> bool {
        let Some(transaction) = self.transaction_by_id_mut(transaction_id) else {
            return false;
        };
        transaction.commit_timing_readiness = readiness;
        true
    }

    pub(in crate::compositor) fn clear_resize_state_for_surfaces(&mut self, surface_ids: &[u32]) {
        for transaction in &mut self.pending_surface_tree_transactions {
            for (surface_id, commit) in &mut transaction.nodes {
                if !surface_ids.contains(surface_id) {
                    continue;
                }
                commit.resize_commit = None;
                if let Some(PendingSurfaceAttachment::Buffer(buffer)) = commit.attachment.as_mut() {
                    buffer.resize_commit = None;
                }
            }
        }
    }

    pub(in crate::compositor) fn drain_unpublished_commits(
        &mut self,
    ) -> Vec<CachedSubsurfaceCommit> {
        let mut commits = self.drain_cached_commits();
        for transaction in self.pending_surface_tree_transactions.drain(..) {
            commits.extend(transaction.nodes.into_iter().map(|(_, commit)| commit));
        }
        commits
    }

    pub(in crate::compositor) fn pending_pointer_constraint_hint(
        &self,
        surface_id: u32,
        constraint_id: u64,
    ) -> Option<(f64, f64)> {
        self.pending_surface_tree_transactions
            .iter()
            .rev()
            .flat_map(|transaction| transaction.nodes.iter().rev())
            .filter(|(node_surface_id, _)| *node_surface_id == surface_id)
            .find_map(|(_, commit)| match &commit.pointer_constraint_state {
                CapturedPointerConstraintSurfaceState::Mutation(captured)
                    if captured.constraint_id == constraint_id =>
                {
                    match &captured.cursor_position_hint {
                        PointerConstraintHintCommit::Set(hint) => Some(*hint),
                        PointerConstraintHintCommit::NoChange => None,
                    }
                }
                CapturedPointerConstraintSurfaceState::Transition(transition) => transition
                    .install_or_update
                    .as_ref()
                    .filter(|captured| captured.constraint_id == constraint_id)
                    .and_then(|captured| match &captured.cursor_position_hint {
                        PointerConstraintHintCommit::Set(hint) => Some(*hint),
                        PointerConstraintHintCommit::NoChange => None,
                    }),
                _ => None,
            })
            .or_else(|| self.cached_pointer_constraint_hint(surface_id, constraint_id))
    }

    #[cfg(test)]
    pub(in crate::compositor) fn set_node_commit_timing_for_test(
        &mut self,
        transaction_index: usize,
        node_index: usize,
        timing: Option<CommitTimingConstraint>,
    ) -> bool {
        let Some((_, commit)) = self
            .pending_surface_tree_transactions
            .get_mut(transaction_index)
            .and_then(|transaction| transaction.nodes.get_mut(node_index))
        else {
            return false;
        };
        commit.pacing.commit_timing = timing;
        true
    }

    #[cfg(test)]
    pub(in crate::compositor) fn install_pending_trees_for_test(
        &mut self,
        transactions: impl IntoIterator<Item = PendingSurfaceTreeTransaction>,
    ) {
        self.pending_surface_tree_transactions.extend(transactions);
    }

    #[cfg(test)]
    pub(in crate::compositor) fn pending_tree_at_for_test(
        &self,
        index: usize,
    ) -> Option<&PendingSurfaceTreeTransaction> {
        self.pending_surface_tree_transactions.get(index)
    }
}

impl CompositorState {
    pub(in crate::compositor) fn merge_or_queue_surface_tree_transaction(
        &mut self,
        root_surface_id: u32,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        dependencies: Vec<SurfaceTreeAcquireDependency>,
        external_content_update_dependencies: Vec<ContentUpdateRef>,
        submission_kind: SurfaceTreeSubmissionKind,
    ) {
        let Some(publication_lifetimes) = self.capture_surface_tree_node_lifetimes(&nodes) else {
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        };
        let incoming_has_unready_acquire = !dependencies.is_empty();
        let incoming_has_attachment_change =
            nodes.iter().any(|(_, commit)| commit.attachment.is_some());
        let incoming_is_pacing_protected = nodes
            .iter()
            .any(|(_, commit)| commit.pacing.is_boundary() || commit.lineage.merge_frozen);
        let matching = self
            .surface_transactions
            .pending_trees()
            .enumerate()
            .filter_map(|(index, transaction)| {
                (transaction.root_surface_id == root_surface_id).then_some(index)
            })
            .collect::<Vec<_>>();
        let Some(&target_index) = matching.last() else {
            let transaction = self.build_surface_tree_transaction(
                root_surface_id,
                nodes,
                publication_lifetimes,
                dependencies,
                external_content_update_dependencies,
            );
            if self.transaction_is_ready(&transaction) {
                self.publish_surface_tree_nodes(transaction);
            } else {
                self.queue_waiting_surface_tree_transaction(transaction, submission_kind);
            }
            return;
        };
        let target = self
            .surface_transactions
            .pending_tree_at(target_index)
            .expect("selected transaction remains queued");
        let target_is_ready = self.transaction_is_ready(target);
        let target_is_pacing_protected = target.is_pacing_protected();
        if target_is_pacing_protected || incoming_is_pacing_protected {
            if target_is_pacing_protected {
                self.queue_waiting_surface_tree_with_lifetimes(
                    root_surface_id,
                    nodes,
                    publication_lifetimes.clone(),
                    dependencies,
                    external_content_update_dependencies.clone(),
                    submission_kind,
                );
                self.commit_ready_surface_tree_transactions();
                return;
            }
            if incoming_has_attachment_change && target_is_ready {
                self.queue_waiting_surface_tree_with_lifetimes(
                    root_surface_id,
                    nodes,
                    publication_lifetimes.clone(),
                    dependencies,
                    external_content_update_dependencies.clone(),
                    submission_kind,
                );
                self.commit_ready_surface_tree_transactions();
                return;
            }
            if incoming_is_pacing_protected {
                self.queue_waiting_surface_tree_with_lifetimes(
                    root_surface_id,
                    nodes,
                    publication_lifetimes.clone(),
                    dependencies,
                    external_content_update_dependencies.clone(),
                    submission_kind,
                );
                self.commit_ready_surface_tree_transactions();
                return;
            }
        }
        if incoming_has_attachment_change && target_is_ready {
            if incoming_has_unready_acquire {
                self.surface_transactions
                    .metrics
                    .ready_transactions_preserved_from_newer_unready = self
                    .surface_transactions
                    .metrics
                    .ready_transactions_preserved_from_newer_unready
                    .saturating_add(1);
            } else {
                self.surface_transactions
                    .metrics
                    .ready_transactions_preserved_from_newer_ready = self
                    .surface_transactions
                    .metrics
                    .ready_transactions_preserved_from_newer_ready
                    .saturating_add(1);
            }
            self.queue_waiting_surface_tree_with_lifetimes(
                root_surface_id,
                nodes,
                publication_lifetimes.clone(),
                dependencies,
                external_content_update_dependencies.clone(),
                submission_kind,
            );
            self.commit_ready_surface_tree_transactions();
            return;
        }

        if !can_coalesce_pending_surface_tree_transaction(
            self.surface_transactions
                .pending_tree_at(target_index)
                .expect("selected transaction remains queued"),
            &nodes,
        ) {
            self.queue_waiting_surface_tree_with_lifetimes(
                root_surface_id,
                nodes,
                publication_lifetimes,
                dependencies,
                external_content_update_dependencies,
                submission_kind,
            );
            self.commit_ready_surface_tree_transactions();
            return;
        }

        let progress_anchor_replacement = if matching.first() == Some(&target_index) {
            surface_tree_merge_would_replace_progress_acquire(
                self.surface_transactions
                    .pending_tree_at(target_index)
                    .expect("selected transaction remains queued"),
                &nodes,
                &dependencies,
            )
        } else {
            None
        };
        if let Some(replacement) = progress_anchor_replacement {
            self.surface_transactions
                .metrics
                .unready_progress_anchors_preserved_from_newer_unready = self
                .surface_transactions
                .metrics
                .unready_progress_anchors_preserved_from_newer_unready
                .saturating_add(1);
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: subsurface_tx root={root_surface_id} decision=preserve_unready_progress_anchor surface={} anchor_commit_id={} incoming_commit_id={}",
                    replacement.surface_id,
                    replacement.anchor_commit_id.get(),
                    replacement.incoming_commit_id.get(),
                );
            }
            self.queue_waiting_surface_tree_with_lifetimes(
                root_surface_id,
                nodes,
                publication_lifetimes,
                dependencies,
                external_content_update_dependencies,
                submission_kind,
            );
            self.commit_ready_surface_tree_transactions();
            return;
        }

        let Some(mut transaction) = self.surface_transactions.take_pending_tree_at(target_index)
        else {
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        };
        let pacing_deadline_changed = transaction.commit_timing_readiness.is_some();
        let stats = self.merge_surface_tree_nodes_into_transaction(
            root_surface_id,
            &mut transaction,
            nodes,
            publication_lifetimes,
            dependencies,
            external_content_update_dependencies,
        );
        if let Err((error_surface_id, error, viewport_error_owner)) = self
            .prepare_surface_tree_surface_state(
                transaction.root_surface_id,
                &mut transaction.nodes,
                &transaction.external_content_update_dependencies,
            )
        {
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: merged surface-tree validation failed surface={error_surface_id}"
                );
            }
            self.post_surface_mapping_error(error_surface_id, error, viewport_error_owner);
            let root_surface_id = transaction.root_surface_id;
            let released = self.release_pending_surface_tree_transaction(
                transaction,
                AcquireWatchCancelReason::Rejected,
            );
            if let Some(resize_commit) = released.resize_commit {
                self.release_detached_resize_capture(root_surface_id, resize_commit);
            }
            self.complete_frame_callbacks(released.callbacks);
            self.rebuild_scene_work_index();
            self.update_surface_tree_slot_metrics(root_surface_id);
            return;
        }
        let ready_after_merge = self.transaction_is_ready(&transaction);
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
        self.surface_transactions.push_pending_tree(transaction);
        if pacing_deadline_changed {
            self.invalidate_surface_pacing_deadline_cache();
        }
        self.rebuild_scene_work_index();
        self.update_surface_tree_slot_metrics(root_surface_id);
        if ready_after_merge || !incoming_has_attachment_change {
            self.commit_ready_surface_tree_transactions();
        }
    }

    pub(in crate::compositor) fn merge_surface_tree_nodes_into_transaction(
        &mut self,
        root_surface_id: u32,
        transaction: &mut PendingSurfaceTreeTransaction,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        publication_lifetimes: SurfaceTreeNodeLifetimes,
        dependencies: Vec<SurfaceTreeAcquireDependency>,
        external_content_update_dependencies: Vec<ContentUpdateRef>,
    ) -> SurfaceTreeMergeStats {
        let Some(publication_lifetimes) = publication_lifetimes.captured() else {
            return SurfaceTreeMergeStats::default();
        };
        debug_assert_eq!(transaction.ordering(), TransactionOrdering::Coalescible);
        debug_assert!(
            nodes.iter().all(|(_, commit)| {
                !commit.pacing.is_boundary() && !commit.lineage.merge_frozen
            })
        );
        if publication_lifetimes.len() != nodes.len() {
            return SurfaceTreeMergeStats::default();
        }
        let mut stats = SurfaceTreeMergeStats {
            incoming_nodes: nodes.len(),
            existing_nodes: transaction.nodes.len(),
            ..SurfaceTreeMergeStats::default()
        };
        let incoming_lifetimes = publication_lifetimes.to_vec();
        let incoming_surface_ids = nodes
            .iter()
            .map(|(surface_id, _)| *surface_id)
            .collect::<Vec<_>>();
        debug_assert!(
            incoming_surface_ids
                .iter()
                .enumerate()
                .all(|(index, surface_id)| !incoming_surface_ids[..index].contains(surface_id))
        );
        let Some(existing_lifetimes) = transaction
            .publication_lifetimes
            .captured()
            .filter(|lifetimes| lifetimes.len() == transaction.nodes.len())
            .map(<[SurfaceTreeNodeLifetime]>::to_vec)
        else {
            return SurfaceTreeMergeStats::default();
        };
        let mut existing_nodes = std::mem::take(&mut transaction.nodes)
            .into_iter()
            .zip(existing_lifetimes)
            .map(|((surface_id, commit), lifetime)| Some((surface_id, commit, lifetime)))
            .collect::<Vec<_>>();
        let mut merged_nodes = Vec::with_capacity(stats.existing_nodes.saturating_add(nodes.len()));
        let mut merged_lifetimes = Vec::with_capacity(merged_nodes.capacity());
        for existing in &mut existing_nodes {
            let Some((surface_id, commit, lifetime)) = existing.take() else {
                continue;
            };
            if incoming_surface_ids.contains(&surface_id) {
                *existing = Some((surface_id, commit, lifetime));
            } else {
                merged_nodes.push((surface_id, commit));
                merged_lifetimes.push(lifetime);
            }
        }
        for ((surface_id, incoming), incoming_lifetime) in nodes.into_iter().zip(incoming_lifetimes)
        {
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
            let Some(existing_index) = existing_nodes.iter().position(|entry| {
                entry
                    .as_ref()
                    .is_some_and(|(node_surface_id, _, _)| *node_surface_id == surface_id)
            }) else {
                merged_nodes.push((surface_id, incoming));
                merged_lifetimes.push(incoming_lifetime);
                continue;
            };
            let (existing_surface_id, mut existing, _existing_lifetime) = existing_nodes
                [existing_index]
                .take()
                .expect("existing surface-tree node");
            debug_assert_eq!(existing_surface_id, surface_id);
            debug_assert!(existing.commit_sequence < incoming.commit_sequence);
            let old_buffer_id = existing
                .attachment
                .as_ref()
                .and_then(pending_attachment_buffer_protocol_id);
            let old_resize_commit = attachment_changed
                .then(|| pending_node_resize_commit(&existing))
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
            let previous_commit_id = existing.commit_id;
            let previous_callback_count = existing.frame_callbacks.len();
            let replacement_commit_id = incoming.commit_id;
            if let Some(release) = existing.merge(incoming) {
                stats.attachments_replaced = stats.attachments_replaced.saturating_add(1);
                self.release_pending_surface_buffer(release);
            }
            if previous_commit_id != replacement_commit_id {
                self.note_explicit_commit_merged(
                    previous_commit_id,
                    replacement_commit_id,
                    previous_callback_count,
                );
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
            merged_nodes.push((surface_id, existing));
            merged_lifetimes.push(incoming_lifetime);
        }
        debug_assert!(existing_nodes.iter().all(Option::is_none));
        transaction.nodes = merged_nodes;
        transaction.publication_lifetimes = SurfaceTreeNodeLifetimes::Captured(merged_lifetimes);
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
        for dependency in external_content_update_dependencies {
            if !transaction
                .external_content_update_dependencies
                .contains(&dependency)
            {
                transaction
                    .external_content_update_dependencies
                    .push(dependency);
            }
        }
        normalize_transaction_external_content_dependencies(transaction);
        normalize_surface_tree_node_order(transaction);
        debug_assert_surface_tree_content_update_invariants(transaction);
        stats
    }

    pub(in crate::compositor) fn update_surface_tree_slot_metrics(&mut self, root_surface_id: u32) {
        let mut ready = 0usize;
        let mut waiting = 0usize;
        for transaction in self
            .surface_transactions
            .pending_trees()
            .filter(|transaction| transaction.root_surface_id == root_surface_id)
        {
            if self.transaction_is_ready(transaction) {
                ready = ready.saturating_add(1);
            } else {
                waiting = waiting.saturating_add(1);
            }
        }
        self.surface_transactions
            .metrics
            .maximum_ready_slots_per_root = self
            .surface_transactions
            .metrics
            .maximum_ready_slots_per_root
            .max(ready);
        self.surface_transactions
            .metrics
            .maximum_waiting_slots_per_root = self
            .surface_transactions
            .metrics
            .maximum_waiting_slots_per_root
            .max(waiting);
        self.surface_transactions
            .metrics
            .maximum_explicit_sync_queue_depth = self
            .surface_transactions
            .metrics
            .maximum_explicit_sync_queue_depth
            .max(ready.saturating_add(waiting));
    }

    pub(in crate::compositor) fn prepare_surface_tree_acquires(
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
                    state.post_error_with_metrics(
                        &mut self.compliance_metrics,
                        &mut self.protocol_error_trace,
                        &mut self.terminal_client_ids,
                        SYNCOBJ_SURFACE_ERROR_NO_BUFFER,
                        "explicit sync points were set without an attached buffer",
                    );
                    return None;
                }
                continue;
            };
            if !pending.data.is_dmabuf() {
                state.post_error_with_metrics(
                    &mut self.compliance_metrics,
                    &mut self.protocol_error_trace,
                    &mut self.terminal_client_ids,
                    SYNCOBJ_SURFACE_ERROR_UNSUPPORTED_BUFFER,
                    "explicit sync is only supported for linux-dmabuf buffers",
                );
                return None;
            }
            let Some(acquire) = acquire else {
                state.post_error_with_metrics(
                    &mut self.compliance_metrics,
                    &mut self.protocol_error_trace,
                    &mut self.terminal_client_ids,
                    SYNCOBJ_SURFACE_ERROR_NO_ACQUIRE_POINT,
                    "dmabuf commit is missing an acquire timeline point",
                );
                return None;
            };
            let Some(release) = release else {
                state.post_error_with_metrics(
                    &mut self.compliance_metrics,
                    &mut self.protocol_error_trace,
                    &mut self.terminal_client_ids,
                    SYNCOBJ_SURFACE_ERROR_NO_RELEASE_POINT,
                    "dmabuf commit is missing a release timeline point",
                );
                return None;
            };
            if acquire.timeline.same_timeline(&release.timeline) && acquire.point >= release.point {
                state.post_error_with_metrics(
                    &mut self.compliance_metrics,
                    &mut self.protocol_error_trace,
                    &mut self.terminal_client_ids,
                    SYNCOBJ_SURFACE_ERROR_CONFLICTING_POINTS,
                    "acquire timeline point must be lower than release point on the same timeline",
                );
                return None;
            }
            pending.explicit_release = Some(release);
            if acquire.is_signaled() {
                self.note_explicit_commit_ready(commit.commit_id);
                continue;
            }
            let Some(commit_id) = self.acquire_commit_ids.allocate() else {
                state.post_error_with_metrics(
                    &mut self.compliance_metrics,
                    &mut self.protocol_error_trace,
                    &mut self.terminal_client_ids,
                    SYNCOBJ_SURFACE_ERROR_NO_ACQUIRE_POINT,
                    "explicit sync commit identity space exhausted",
                );
                return None;
            };
            let (owner_client_id, surface_presentation_generation) =
                self.capture_surface_publication_lifetime(*surface_id)?;
            client_pacing_log(
                "acquire_wait_queued",
                &[
                    ("surface", surface_id.to_string()),
                    (
                        "root",
                        self.root_surface_id_for_surface(*surface_id).to_string(),
                    ),
                    (
                        "client",
                        format!("{:?}", self.surface_client_ids.get(surface_id)),
                    ),
                    ("commit_sequence", commit.commit_sequence.0.to_string()),
                    ("acquire_commit_id", commit_id.get().to_string()),
                    ("buffer", pending.resource.id().protocol_id().to_string()),
                ],
            );
            dependencies.push(SurfaceTreeAcquireDependency {
                surface_commit_id: commit.commit_id,
                commit_id,
                surface_id: *surface_id,
                owner_client_id: Some(owner_client_id),
                surface_presentation_generation: Some(surface_presentation_generation),
                buffer_id: pending.resource.id().protocol_id(),
                acquire,
                state: PendingAcquireState::RegistrationPending,
            });
            self.trace_surface_pipeline_event(
                SurfacePipelineEvent::AcquirePending,
                *surface_id,
                commit.commit_sequence,
                Some(pending.resource.id().protocol_id().into()),
                None,
                None,
                None,
                None,
                None,
            );
            self.note_explicit_commit_acquire_wait(commit.commit_id, commit.frame_callbacks.len());
        }
        Some(dependencies)
    }

    fn build_surface_tree_transaction(
        &mut self,
        root_surface_id: u32,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        publication_lifetimes: SurfaceTreeNodeLifetimes,
        dependencies: Vec<SurfaceTreeAcquireDependency>,
        external_content_update_dependencies: Vec<ContentUpdateRef>,
    ) -> PendingSurfaceTreeTransaction {
        let mut transaction = PendingSurfaceTreeTransaction {
            id: self
                .surface_transactions
                .allocate_surface_tree_transaction_id(),
            root_surface_id,
            nodes,
            publication_lifetimes,
            dependencies,
            external_content_update_dependencies,
            commit_timing_readiness: None,
            received_at: Instant::now(),
        };
        normalize_transaction_external_content_dependencies(&mut transaction);
        normalize_surface_tree_node_order(&mut transaction);
        debug_assert_surface_tree_content_update_invariants(&transaction);
        transaction
    }

    fn queue_waiting_surface_tree_transaction(
        &mut self,
        transaction: PendingSurfaceTreeTransaction,
        submission_kind: SurfaceTreeSubmissionKind,
    ) {
        let PendingSurfaceTreeTransaction {
            id: transaction_id,
            root_surface_id,
            nodes,
            publication_lifetimes,
            dependencies,
            external_content_update_dependencies,
            commit_timing_readiness,
            received_at,
        } = transaction;
        self.queue_waiting_surface_tree_parts(
            root_surface_id,
            transaction_id,
            nodes,
            publication_lifetimes,
            dependencies,
            external_content_update_dependencies,
            commit_timing_readiness,
            received_at,
            submission_kind,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn queue_waiting_surface_tree_parts(
        &mut self,
        root_surface_id: u32,
        transaction_id: SurfaceTreeTransactionId,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        publication_lifetimes: SurfaceTreeNodeLifetimes,
        dependencies: Vec<SurfaceTreeAcquireDependency>,
        external_content_update_dependencies: Vec<ContentUpdateRef>,
        commit_timing_readiness: Option<CommitTimingReadiness>,
        received_at: Instant,
        submission_kind: SurfaceTreeSubmissionKind,
    ) {
        let mut transaction = PendingSurfaceTreeTransaction {
            id: transaction_id,
            root_surface_id,
            nodes: Vec::new(),
            publication_lifetimes,
            dependencies: Vec::new(),
            external_content_update_dependencies: Vec::new(),
            commit_timing_readiness,
            received_at,
        };
        if submission_kind == SurfaceTreeSubmissionKind::ClientAdmission {
            let mut matching = self
                .surface_transactions
                .pending_trees()
                .enumerate()
                .filter_map(|(index, transaction)| {
                    (transaction.root_surface_id == root_surface_id).then_some(index)
                })
                .collect::<Vec<_>>();
            let at_capacity_with_only_ready = matching.len()
                >= MAX_SURFACE_TREE_TRANSACTIONS_PER_ROOT
                && matching.iter().all(|index| {
                    self.transaction_is_ready(
                        self.surface_transactions
                            .pending_tree_at(*index)
                            .expect("matching tree index"),
                    )
                });
            if at_capacity_with_only_ready {
                self.surface_transactions.metrics.all_ready_queue_pressure = self
                    .surface_transactions
                    .metrics
                    .all_ready_queue_pressure
                    .saturating_add(1);
                self.commit_ready_surface_tree_transactions();
                matching.clear();
                matching.extend(
                    self.surface_transactions
                        .pending_trees()
                        .enumerate()
                        .filter_map(|(index, transaction)| {
                            (transaction.root_surface_id == root_surface_id).then_some(index)
                        }),
                );
            }
            if matching.len() >= MAX_SURFACE_TREE_TRANSACTIONS_PER_ROOT {
                self.surface_transactions
                    .metrics
                    .explicit_sync_queue_overflow = self
                    .surface_transactions
                    .metrics
                    .explicit_sync_queue_overflow
                    .saturating_add(1);
                self.commit_ready_surface_tree_transactions();
                let still_at_capacity = self
                    .surface_transactions
                    .pending_tree_count_for_root(root_surface_id)
                    >= MAX_SURFACE_TREE_TRANSACTIONS_PER_ROOT;
                if still_at_capacity {
                    if self.request_client_resource_exhaustion(root_surface_id) {
                        self.surface_pacing_metrics
                            .queue_admission_resource_exhaustion = self
                            .surface_pacing_metrics
                            .queue_admission_resource_exhaustion
                            .saturating_add(1);
                    }
                    self.release_unpublished_surface_tree_nodes(nodes);
                    return;
                }
            }
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
        self.surface_transactions
            .metrics
            .tree_transactions_waiting_on_acquire = self
            .surface_transactions
            .metrics
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
        for (surface_id, commit) in &nodes {
            self.trace_surface_pipeline_event(
                SurfacePipelineEvent::TransactionQueued,
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
                Some(transaction_id.get()),
                None,
                None,
                None,
            );
        }
        transaction.nodes = nodes;
        transaction.dependencies = dependencies;
        transaction.external_content_update_dependencies = external_content_update_dependencies;
        normalize_transaction_external_content_dependencies(&mut transaction);
        normalize_surface_tree_node_order(&mut transaction);
        debug_assert_surface_tree_content_update_invariants(&transaction);
        self.surface_transactions.push_pending_tree(transaction);
        self.rebuild_scene_work_index();
        if submission_kind == SurfaceTreeSubmissionKind::ClientAdmission {
            debug_assert!(
                self.surface_transactions
                    .pending_trees()
                    .filter(|transaction| transaction.root_surface_id == root_surface_id)
                    .count()
                    <= MAX_SURFACE_TREE_TRANSACTIONS_PER_ROOT
            );
        }
        self.update_surface_tree_slot_metrics(root_surface_id);
        let pending_acquires = self.pending_explicit_sync_commits.len().saturating_add(
            self.surface_transactions
                .pending_trees()
                .map(|transaction| transaction.dependencies.len())
                .sum::<usize>(),
        );
        self.resize_flow_metrics.max_pending_explicit_sync_commits = self
            .resize_flow_metrics
            .max_pending_explicit_sync_commits
            .max(pending_acquires);
    }

    #[cfg(test)]
    pub(in crate::compositor) fn queue_waiting_surface_tree(
        &mut self,
        root_surface_id: u32,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        dependencies: Vec<SurfaceTreeAcquireDependency>,
    ) {
        let Some(publication_lifetimes) = self.capture_surface_tree_node_lifetimes(&nodes) else {
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        };
        self.queue_waiting_surface_tree_with_lifetimes(
            root_surface_id,
            nodes,
            publication_lifetimes,
            dependencies,
            Vec::new(),
            SurfaceTreeSubmissionKind::ClientAdmission,
        );
    }

    pub(super) fn queue_waiting_surface_tree_with_lifetimes(
        &mut self,
        root_surface_id: u32,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        publication_lifetimes: SurfaceTreeNodeLifetimes,
        dependencies: Vec<SurfaceTreeAcquireDependency>,
        external_content_update_dependencies: Vec<ContentUpdateRef>,
        submission_kind: SurfaceTreeSubmissionKind,
    ) {
        let transaction = self.build_surface_tree_transaction(
            root_surface_id,
            nodes,
            publication_lifetimes,
            dependencies,
            external_content_update_dependencies,
        );
        self.queue_waiting_surface_tree_transaction(transaction, submission_kind);
    }
}
