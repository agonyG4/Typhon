use super::*;

impl CompositorState {
    pub(in crate::compositor) fn has_pending_acquire_watch_changes(&self) -> bool {
        !self.pending_acquire_watch_changes.is_empty()
    }

    pub(in crate::compositor) fn enable_external_acquire_readiness(&mut self) {
        if self.external_acquire_readiness {
            return;
        }
        self.external_acquire_readiness = true;
        for transaction in self.surface_transactions.pending_trees() {
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
        self.surface_transactions
            .acquire_dependency_mut(commit_id)
            .is_some_and(|dependency| dependency.state.mark_eventfd_backed())
    }

    pub(in crate::compositor) fn mark_acquire_commit_fallback_backed(
        &mut self,
        commit_id: AcquireCommitId,
    ) -> bool {
        self.surface_transactions
            .acquire_dependency_mut(commit_id)
            .is_some_and(|dependency| dependency.state.mark_fallback_backed())
    }

    pub(in crate::compositor) fn mark_acquire_commit_ready(
        &mut self,
        commit_id: AcquireCommitId,
        surface_id: u32,
        acquire: &ExplicitSyncPoint,
    ) -> bool {
        let surface_commit_id = self
            .surface_transactions
            .acquire_dependency(commit_id)
            .map(|dependency| dependency.surface_commit_id);
        let tree_dependency_lifetime_is_current = self
            .surface_transactions
            .acquire_dependency(commit_id)
            .map(|dependency| {
                let Some(owner_client_id) = dependency.owner_client_id.as_ref() else {
                    return true;
                };
                self.async_surface_lifecycle_rejection(dependency.surface_id, owner_client_id)
                    .is_none()
                    && dependency
                        .surface_presentation_generation
                        .is_none_or(|generation| {
                            self.surface_presentation_generations
                                .get(&dependency.surface_id)
                                .copied()
                                == Some(generation)
                        })
            })
            .unwrap_or(true);
        let ready = self
            .surface_transactions
            .acquire_dependency_mut(commit_id)
            .filter(|dependency| {
                dependency.surface_id == surface_id && dependency.acquire == *acquire
            })
            .is_some_and(|dependency| dependency.state.mark_ready());
        if ready {
            if !tree_dependency_lifetime_is_current {
                client_pacing_log(
                    "acquire_ready_stale_surface_lifetime",
                    &[
                        ("surface", surface_id.to_string()),
                        ("acquire_commit_id", commit_id.get().to_string()),
                    ],
                );
            }
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
}
