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
                }
            }
        }
        let mut superseded_callbacks: HashMap<u32, Vec<wl_callback::WlCallback>> = HashMap::new();
        let mut superseded_resize_commits: HashMap<u32, ResizeCommitSnapshot> = HashMap::new();
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
                    selected = Some((index, false, None));
                    break;
                }
                if transaction.is_pacing_protected() {
                    continue;
                }
                let root_id = transaction.root_surface_id;
                let mut replacement = None;
                for candidate in &transactions[index + 1..] {
                    if candidate.root_surface_id != root_id {
                        continue;
                    }
                    let ready = self.transaction_is_ready(candidate);
                    if candidate.is_pacing_protected() {
                        if ready {
                            replacement =
                                candidate.nodes.first().map(|(_, commit)| commit.commit_id);
                        }
                        break;
                    }
                    if ready {
                        replacement = candidate.nodes.first().map(|(_, commit)| commit.commit_id);
                        break;
                    }
                }
                if replacement.is_some() {
                    selected = Some((index, true, replacement));
                    break;
                }
            }
            let Some((index, supersede, replacement)) = selected else {
                break;
            };
            let transaction = transactions.remove(index);
            pacing_deadline_changed |= transaction.commit_timing_readiness.is_some();
            if supersede {
                let root_id = transaction.root_surface_id;
                let replacement = replacement.expect("supersession has a replacement");
                let acquire_state = if transaction
                    .dependencies
                    .iter()
                    .all(|dependency| dependency.state == PendingAcquireState::Ready)
                {
                    PendingAcquireState::Ready
                } else {
                    PendingAcquireState::RegistrationPending
                };
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
                continue;
            }
            let mut transaction = transaction;
            if let Some(readiness) = transaction.commit_timing_readiness {
                for (_, commit) in &mut transaction.nodes {
                    if commit.pacing.commit_timing.is_some() {
                        commit.pacing.commit_timing_readiness = Some(readiness);
                    }
                }
            }
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
        self.pending_surface_tree_transactions = transactions;
        if pacing_deadline_changed {
            self.invalidate_surface_pacing_deadline_cache();
        }
        self.rebuild_scene_work_index();
        for (root_surface_id, resize_commit) in superseded_resize_commits {
            self.release_detached_resize_capture(root_surface_id, resize_commit);
        }
    }
}
