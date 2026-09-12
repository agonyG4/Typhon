use super::*;

impl CompositorState {
    pub(in crate::compositor) fn cache_synchronized_subsurface_commit(
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
        self.subsurface_transaction_metrics
            .synchronized_child_commits_cached = self
            .subsurface_transaction_metrics
            .synchronized_child_commits_cached
            .saturating_add(1);
        let already_exhausted = self
            .subsurface_transactions
            .client_id(surface_id)
            .is_some_and(|client_id| self.client_resource_exhaustion_pending(client_id));
        if already_exhausted {
            self.reject_synchronized_cache_commit(
                surface_id,
                commit,
                CacheAdmissionFailure::ClientAlreadyExhausted,
            );
            self.update_synchronized_cache_metrics();
            return;
        }
        match self
            .subsurface_transactions
            .cache_commit(surface_id, commit)
        {
            CacheCommitOutcome::Inserted => {
                self.subsurface_transaction_metrics.cached_commits_appended = self
                    .subsurface_transaction_metrics
                    .cached_commits_appended
                    .saturating_add(1);
            }
            CacheCommitOutcome::Merged { superseded_buffer } => {
                self.subsurface_transaction_metrics.cached_commits_merged = self
                    .subsurface_transaction_metrics
                    .cached_commits_merged
                    .saturating_add(1);
                if let Some(buffer) = superseded_buffer {
                    self.release_pending_surface_buffer(*buffer);
                }
            }
            CacheCommitOutcome::Rejected { commit, reason } => {
                self.reject_synchronized_cache_commit(surface_id, *commit, reason);
            }
        }
        self.update_synchronized_cache_metrics();
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: subsurface_tx surface={surface_id} parent={:?} requested_mode={:?} effective_mode=sync decision=cached buffer_id={buffer_id:?}",
                self.subsurface_transactions.parent(surface_id),
                self.subsurface_transactions.requested_mode(surface_id),
            );
        }
    }

    fn reject_synchronized_cache_commit(
        &mut self,
        surface_id: u32,
        commit: CachedSubsurfaceCommit,
        reason: CacheAdmissionFailure,
    ) {
        if commit.explicit_sync.is_some() {
            self.note_explicit_commit_destroyed(
                commit.commit_id,
                "synchronized_cache_admission_rejected",
            );
        }
        self.release_unpublished_surface_tree_nodes(vec![(surface_id, commit)]);
        self.subsurface_transaction_metrics.cached_commits_rejected = self
            .subsurface_transaction_metrics
            .cached_commits_rejected
            .saturating_add(1);
        match reason {
            CacheAdmissionFailure::PerSurfaceEntryLimit
            | CacheAdmissionFailure::PerSurfaceObligationLimit => {
                self.subsurface_transaction_metrics
                    .cache_per_surface_limit_hits = self
                    .subsurface_transaction_metrics
                    .cache_per_surface_limit_hits
                    .saturating_add(1);
            }
            CacheAdmissionFailure::PerClientEntryLimit
            | CacheAdmissionFailure::PerClientObligationLimit => {
                self.subsurface_transaction_metrics
                    .cache_per_client_limit_hits = self
                    .subsurface_transaction_metrics
                    .cache_per_client_limit_hits
                    .saturating_add(1);
            }
            CacheAdmissionFailure::TotalEntryLimit
            | CacheAdmissionFailure::TotalObligationLimit => {
                self.subsurface_transaction_metrics.cache_global_limit_hits = self
                    .subsurface_transaction_metrics
                    .cache_global_limit_hits
                    .saturating_add(1);
            }
            CacheAdmissionFailure::MissingRole
            | CacheAdmissionFailure::ClientAlreadyExhausted
            | CacheAdmissionFailure::AccountingInvariant => {}
        }
        if !matches!(
            reason,
            CacheAdmissionFailure::MissingRole | CacheAdmissionFailure::ClientAlreadyExhausted
        ) && self.request_client_resource_exhaustion(surface_id)
        {
            self.surface_pacing_metrics
                .queue_admission_resource_exhaustion = self
                .surface_pacing_metrics
                .queue_admission_resource_exhaustion
                .saturating_add(1);
        }
    }

    pub(in crate::compositor) fn update_synchronized_cache_metrics(&mut self) {
        self.subsurface_transaction_metrics.current_cached_entries =
            self.subsurface_transactions.cached_entry_count();
        self.subsurface_transaction_metrics.maximum_cached_entries =
            self.subsurface_transactions.maximum_cached_entries();
        self.subsurface_transaction_metrics
            .maximum_cached_entries_per_surface = self
            .subsurface_transactions
            .maximum_cached_entries_per_surface();
        self.subsurface_transaction_metrics
            .maximum_cached_entries_per_client = self
            .subsurface_transactions
            .maximum_cached_entries_per_client();
        self.subsurface_transaction_metrics
            .current_cached_obligations = self.subsurface_transactions.cached_obligation_count();
        self.subsurface_transaction_metrics
            .maximum_cached_obligations = self.subsurface_transactions.maximum_cached_obligations();
        self.subsurface_transaction_metrics
            .maximum_cached_obligations_per_surface = self
            .subsurface_transactions
            .maximum_cached_obligations_per_surface();
        self.subsurface_transaction_metrics
            .maximum_cached_obligations_per_client = self
            .subsurface_transactions
            .maximum_cached_obligations_per_client();
        self.subsurface_transaction_metrics.maximum_cached_nodes = self
            .subsurface_transaction_metrics
            .maximum_cached_nodes
            .max(self.subsurface_transactions.cached_node_count());
        self.subsurface_transaction_metrics.maximum_tree_depth = self
            .subsurface_transaction_metrics
            .maximum_tree_depth
            .max(self.subsurface_transactions.maximum_depth());
    }
}
