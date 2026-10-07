use super::*;
use crate::compositor::SubsurfaceTransactionMetrics;
use crate::compositor::subsurface::{
    CachedSubsurfaceCommit, CapturedSubsurfacePosition, CapturedSubsurfaceRelationship,
    CapturedSubsurfaceStackEntry, ContentUpdateRef, PointerConstraintHintCommit,
    SubsurfaceRelationshipId, SubsurfaceRelationshipPhase, SubsurfaceSyncMode,
};
use std::collections::{HashMap, VecDeque};

pub(in crate::compositor) const MAX_SYNCHRONIZED_CACHED_COMMITS_PER_SURFACE: usize = 8;
pub(in crate::compositor) const MAX_SYNCHRONIZED_CACHED_COMMITS_PER_CLIENT: usize = 256;
pub(in crate::compositor) const MAX_SYNCHRONIZED_CACHED_COMMITS_TOTAL: usize = 4096;
pub(in crate::compositor) const MAX_SYNCHRONIZED_CACHED_OBLIGATIONS_PER_SURFACE: usize = 1024;
pub(in crate::compositor) const MAX_SYNCHRONIZED_CACHED_OBLIGATIONS_PER_CLIENT: usize = 8192;
pub(in crate::compositor) const MAX_SYNCHRONIZED_CACHED_OBLIGATIONS_TOTAL: usize = 65536;

#[derive(Debug)]
struct SubsurfaceRoleState {
    relationship_id: SubsurfaceRelationshipId,
    parent_id: u32,
    client_id: Option<ClientId>,
    requested_mode: SubsurfaceSyncMode,
    relationship_phase: SubsurfaceRelationshipPhase,
    cached_commits: VecDeque<CachedSubsurfaceCommit>,
    pending_position: Option<(i32, i32)>,
}

#[derive(Debug)]
pub(in crate::compositor) struct DetachedSubsurfaceRole {
    pub(in crate::compositor) relationship: CapturedSubsurfaceRelationship,
    pub(in crate::compositor) client_id: Option<ClientId>,
    pub(in crate::compositor) cached_commits: Vec<CachedSubsurfaceCommit>,
}

#[derive(Debug, Default)]
pub(in crate::compositor) struct SurfaceTransactionState {
    roles: HashMap<u32, SubsurfaceRoleState>,
    next_relationship_id: u64,
    cached_entries_per_surface: HashMap<u32, usize>,
    cached_obligations_per_surface: HashMap<u32, usize>,
    cached_entries_per_client: HashMap<ClientId, usize>,
    cached_obligations_per_client: HashMap<ClientId, usize>,
    cached_entries_total: usize,
    cached_obligations_total: usize,
    cached_nodes: usize,
    maximum_cached_entries: usize,
    maximum_cached_entries_per_surface: usize,
    maximum_cached_entries_per_client: usize,
    maximum_cached_obligations: usize,
    maximum_cached_obligations_per_surface: usize,
    maximum_cached_obligations_per_client: usize,
    pub(super) pending_surface_tree_transactions: Vec<PendingSurfaceTreeTransaction>,
    pub(super) next_surface_tree_transaction_id: u64,
    acquire_commit_ids: AcquireCommitIdAllocator,
    pub(in crate::compositor) metrics: SubsurfaceTransactionMetrics,
}

impl SurfaceTransactionState {
    #[cfg(test)]
    pub(in crate::compositor) fn register(&mut self, surface_id: u32, parent_id: u32) -> bool {
        self.register_with_client(surface_id, parent_id, None)
    }

    pub(in crate::compositor) fn register_with_client(
        &mut self,
        surface_id: u32,
        parent_id: u32,
        client_id: Option<ClientId>,
    ) -> bool {
        if surface_id == parent_id || self.roles.contains_key(&surface_id) {
            return false;
        }
        let mut ancestor = Some(parent_id);
        while let Some(id) = ancestor {
            if id == surface_id {
                return false;
            }
            ancestor = self.roles.get(&id).map(|role| role.parent_id);
        }
        let Some(next_relationship_id) = self.next_relationship_id.checked_add(1) else {
            return false;
        };
        self.next_relationship_id = next_relationship_id;
        self.roles.insert(
            surface_id,
            SubsurfaceRoleState {
                relationship_id: SubsurfaceRelationshipId::new(next_relationship_id),
                parent_id,
                client_id,
                requested_mode: SubsurfaceSyncMode::Synchronized,
                relationship_phase: SubsurfaceRelationshipPhase::PendingParentCommit,
                cached_commits: VecDeque::new(),
                pending_position: None,
            },
        );
        true
    }

    pub(in crate::compositor) fn remove_cached_parent_dependencies_to_child_commits(
        &mut self,
        parent_id: u32,
        child_id: u32,
        promoted_refs: &[ContentUpdateRef],
    ) -> usize {
        debug_assert!(
            promoted_refs
                .iter()
                .all(|reference| reference.surface_id == child_id)
        );
        let Some(parent) = self.roles.get_mut(&parent_id) else {
            return 0;
        };
        let mut removed = 0usize;
        for commit in &mut parent.cached_commits {
            commit.lineage.child_dependencies.retain(|dependency| {
                let remove =
                    dependency.surface_id == child_id && promoted_refs.contains(dependency);
                if remove {
                    removed = removed.saturating_add(1);
                }
                !remove
            });
        }
        debug_assert!(parent.cached_commits.iter().all(|commit| {
            commit.lineage.child_dependencies.iter().all(|dependency| {
                dependency.surface_id != child_id || !promoted_refs.contains(dependency)
            })
        }));
        removed
    }

    pub(in crate::compositor) fn detach_role(
        &mut self,
        surface_id: u32,
    ) -> Option<DetachedSubsurfaceRole> {
        let role = self.roles.remove(&surface_id)?;
        let relationship = CapturedSubsurfaceRelationship {
            surface_id,
            parent_id: role.parent_id,
            relationship_id: role.relationship_id,
        };
        let client_id = role.client_id.clone();
        let cached_commits = role.cached_commits.into_iter().collect::<Vec<_>>();
        let cached_obligations = cached_commits
            .iter()
            .map(CachedSubsurfaceCommit::cached_obligation_count)
            .sum();
        self.replace_cached_accounting(
            surface_id,
            client_id.as_ref(),
            cached_commits.len(),
            0,
            cached_obligations,
            0,
        );
        debug_assert!(self.debug_accounting_is_consistent());
        Some(DetachedSubsurfaceRole {
            relationship,
            client_id,
            cached_commits,
        })
    }

    #[cfg(test)]
    pub(in crate::compositor) fn remove_role(
        &mut self,
        surface_id: u32,
    ) -> Vec<CachedSubsurfaceCommit> {
        self.detach_role(surface_id)
            .map(|detached| detached.cached_commits)
            .unwrap_or_default()
    }

    pub(in crate::compositor) fn remove_subtree(
        &mut self,
        surface_id: u32,
    ) -> Vec<CachedSubsurfaceCommit> {
        let mut removed = Vec::new();
        let mut pending = vec![surface_id];
        while let Some(id) = pending.pop() {
            pending.extend(
                self.roles
                    .iter()
                    .filter_map(|(child_id, role)| (role.parent_id == id).then_some(*child_id)),
            );
            if let Some(role) = self.roles.remove(&id) {
                let client_id = role.client_id.clone();
                let cached_commits = role.cached_commits.into_iter().collect::<Vec<_>>();
                let cached_obligations = cached_commits
                    .iter()
                    .map(CachedSubsurfaceCommit::cached_obligation_count)
                    .sum();
                self.replace_cached_accounting(
                    id,
                    client_id.as_ref(),
                    cached_commits.len(),
                    0,
                    cached_obligations,
                    0,
                );
                removed.extend(cached_commits);
            }
        }
        debug_assert!(self.debug_accounting_is_consistent());
        removed
    }

    pub(in crate::compositor) fn drain_cached_commits(&mut self) -> Vec<CachedSubsurfaceCommit> {
        let surface_ids = self.roles.keys().copied().collect::<Vec<_>>();
        let mut commits = Vec::new();
        for surface_id in surface_ids {
            commits.extend(self.take_cached_commits_for_surface(surface_id));
        }
        debug_assert!(self.debug_accounting_is_consistent());
        commits
    }

    pub(in crate::compositor) fn parent(&self, surface_id: u32) -> Option<u32> {
        self.roles.get(&surface_id).map(|role| role.parent_id)
    }

    pub(in crate::compositor) fn relationship_phase(
        &self,
        surface_id: u32,
    ) -> Option<SubsurfaceRelationshipPhase> {
        self.roles
            .get(&surface_id)
            .map(|role| role.relationship_phase)
    }

    pub(in crate::compositor) fn captured_relationship(
        &self,
        surface_id: u32,
    ) -> Option<CapturedSubsurfaceRelationship> {
        self.roles
            .get(&surface_id)
            .map(|role| CapturedSubsurfaceRelationship {
                surface_id,
                parent_id: role.parent_id,
                relationship_id: role.relationship_id,
            })
    }

    pub(in crate::compositor) fn relationship_matches(
        &self,
        relationship: CapturedSubsurfaceRelationship,
    ) -> bool {
        self.roles
            .get(&relationship.surface_id)
            .is_some_and(|role| {
                role.parent_id == relationship.parent_id
                    && role.relationship_id == relationship.relationship_id
            })
    }

    pub(in crate::compositor) fn relationship_is_applied(
        &self,
        relationship: CapturedSubsurfaceRelationship,
    ) -> bool {
        self.relationship_matches(relationship)
            && self.relationship_phase(relationship.surface_id)
                == Some(SubsurfaceRelationshipPhase::Applied)
    }

    pub(in crate::compositor) fn relationship_is_registered_child_of(
        &self,
        surface_id: u32,
        parent_id: u32,
    ) -> bool {
        self.roles
            .get(&surface_id)
            .is_some_and(|role| role.parent_id == parent_id)
    }

    pub(in crate::compositor) fn relationship_is_applied_child_of(
        &self,
        surface_id: u32,
        parent_id: u32,
    ) -> bool {
        self.roles.get(&surface_id).is_some_and(|role| {
            role.parent_id == parent_id
                && role.relationship_phase == SubsurfaceRelationshipPhase::Applied
        })
    }

    pub(in crate::compositor) fn take_pending_relationship_activations_for_parent(
        &mut self,
        parent_id: u32,
    ) -> Vec<CapturedSubsurfaceRelationship> {
        let mut activations = self
            .roles
            .iter_mut()
            .filter_map(|(surface_id, role)| {
                (role.parent_id == parent_id
                    && role.relationship_phase == SubsurfaceRelationshipPhase::PendingParentCommit)
                    .then(|| {
                        role.relationship_phase = SubsurfaceRelationshipPhase::Latched;
                        CapturedSubsurfaceRelationship {
                            surface_id: *surface_id,
                            parent_id,
                            relationship_id: role.relationship_id,
                        }
                    })
            })
            .collect::<Vec<_>>();
        activations.sort_by_key(|relationship| relationship.relationship_id);
        activations
    }

    pub(in crate::compositor) fn apply_captured_relationship(
        &mut self,
        relationship: CapturedSubsurfaceRelationship,
    ) -> bool {
        let Some(role) = self.roles.get_mut(&relationship.surface_id) else {
            return false;
        };
        if role.parent_id != relationship.parent_id
            || role.relationship_id != relationship.relationship_id
            || role.relationship_phase != SubsurfaceRelationshipPhase::Latched
        {
            return false;
        }
        role.relationship_phase = SubsurfaceRelationshipPhase::Applied;
        true
    }

    pub(in crate::compositor) fn applied_children_of(&self, parent_id: u32) -> Vec<u32> {
        self.roles
            .iter()
            .filter_map(|(surface_id, role)| {
                (role.parent_id == parent_id
                    && role.relationship_phase == SubsurfaceRelationshipPhase::Applied)
                    .then_some(*surface_id)
            })
            .collect()
    }

    pub(in crate::compositor) fn requested_mode(
        &self,
        surface_id: u32,
    ) -> Option<SubsurfaceSyncMode> {
        self.roles.get(&surface_id).map(|role| role.requested_mode)
    }

    pub(in crate::compositor) fn set_mode(
        &mut self,
        surface_id: u32,
        mode: SubsurfaceSyncMode,
    ) -> bool {
        let Some(role) = self.roles.get_mut(&surface_id) else {
            return false;
        };
        role.requested_mode = mode;
        true
    }

    pub(in crate::compositor) fn client_id(&self, surface_id: u32) -> Option<&ClientId> {
        self.roles
            .get(&surface_id)
            .and_then(|role| role.client_id.as_ref())
    }

    pub(in crate::compositor) fn capture_direct_child_dependencies(
        &mut self,
        parent_id: u32,
    ) -> Vec<ContentUpdateRef> {
        let mut child_ids = self
            .roles
            .iter()
            .filter_map(|(surface_id, role)| (role.parent_id == parent_id).then_some(*surface_id))
            .collect::<Vec<_>>();
        child_ids.sort_unstable();

        child_ids
            .into_iter()
            .filter_map(|child_id| {
                let role = self.roles.get_mut(&child_id)?;
                let commit = role.cached_commits.back_mut()?;
                if commit.lineage.merge_frozen {
                    return None;
                }
                commit.lineage.merge_frozen = true;
                Some(commit.content_update_ref(child_id))
            })
            .collect()
    }

    pub(in crate::compositor) fn is_effectively_synchronized(&self, surface_id: u32) -> bool {
        let mut current = Some(surface_id);
        while let Some(id) = current {
            let Some(role) = self.roles.get(&id) else {
                return false;
            };
            if role.requested_mode == SubsurfaceSyncMode::Synchronized {
                return true;
            }
            current = self
                .roles
                .contains_key(&role.parent_id)
                .then_some(role.parent_id);
        }
        false
    }

    pub(in crate::compositor) fn cache_commit(
        &mut self,
        surface_id: u32,
        commit: CachedSubsurfaceCommit,
    ) -> CacheCommitOutcome {
        debug_assert!(self.debug_accounting_is_consistent());
        debug_assert!(commit.lineage.predecessor.is_none_or(|predecessor| {
            predecessor.surface_id == surface_id
                && predecessor.commit_sequence < commit.commit_sequence
        }));
        debug_assert!(commit.lineage.child_dependencies.iter().all(|dependency| {
            dependency.commit_sequence < commit.commit_sequence
                && dependency.commit_sequence != commit.commit_sequence
        }));
        let Some(role) = self.roles.get(&surface_id) else {
            return CacheCommitOutcome::Rejected {
                commit: Box::new(commit),
                reason: CacheAdmissionFailure::MissingRole,
            };
        };
        let old_entries = role.cached_commits.len();
        let old_obligations = self
            .cached_obligations_per_surface
            .get(&surface_id)
            .copied()
            .unwrap_or_default();
        let can_merge = role.cached_commits.back().is_some_and(|tail| {
            !tail.lineage.merge_frozen && !tail.pacing.is_boundary() && !commit.pacing.is_boundary()
        });
        let new_entries = if can_merge {
            old_entries
        } else {
            let Some(new_entries) = old_entries.checked_add(1) else {
                return CacheCommitOutcome::Rejected {
                    commit: Box::new(commit),
                    reason: CacheAdmissionFailure::AccountingInvariant,
                };
            };
            new_entries
        };
        let new_obligations = if can_merge {
            role.cached_commits
                .back()
                .expect("merge target exists")
                .merged_cached_obligation_count(&commit)
        } else {
            let Some(new_obligations) =
                old_obligations.checked_add(commit.cached_obligation_count())
            else {
                return CacheCommitOutcome::Rejected {
                    commit: Box::new(commit),
                    reason: CacheAdmissionFailure::AccountingInvariant,
                };
            };
            new_obligations
        };
        let Some(new_total_entries) =
            checked_replace_cached_count(self.cached_entries_total, old_entries, new_entries)
        else {
            return CacheCommitOutcome::Rejected {
                commit: Box::new(commit),
                reason: CacheAdmissionFailure::AccountingInvariant,
            };
        };
        let Some(new_total_obligations) = checked_replace_cached_count(
            self.cached_obligations_total,
            old_obligations,
            new_obligations,
        ) else {
            return CacheCommitOutcome::Rejected {
                commit: Box::new(commit),
                reason: CacheAdmissionFailure::AccountingInvariant,
            };
        };
        let client_id = role.client_id.clone();
        let (new_client_entries, new_client_obligations) =
            if let Some(client_id) = client_id.as_ref() {
                let client_cached_entries = self
                    .cached_entries_per_client
                    .get(client_id)
                    .copied()
                    .unwrap_or_default();
                let client_cached_obligations = self
                    .cached_obligations_per_client
                    .get(client_id)
                    .copied()
                    .unwrap_or_default();
                let Some(new_client_entries) =
                    checked_replace_cached_count(client_cached_entries, old_entries, new_entries)
                else {
                    return CacheCommitOutcome::Rejected {
                        commit: Box::new(commit),
                        reason: CacheAdmissionFailure::AccountingInvariant,
                    };
                };
                let Some(new_client_obligations) = checked_replace_cached_count(
                    client_cached_obligations,
                    old_obligations,
                    new_obligations,
                ) else {
                    return CacheCommitOutcome::Rejected {
                        commit: Box::new(commit),
                        reason: CacheAdmissionFailure::AccountingInvariant,
                    };
                };
                (new_client_entries, new_client_obligations)
            } else {
                (0, 0)
            };

        let rejection = if new_entries > MAX_SYNCHRONIZED_CACHED_COMMITS_PER_SURFACE {
            Some(CacheAdmissionFailure::PerSurfaceEntryLimit)
        } else if new_obligations > MAX_SYNCHRONIZED_CACHED_OBLIGATIONS_PER_SURFACE {
            Some(CacheAdmissionFailure::PerSurfaceObligationLimit)
        } else if client_id.is_some()
            && new_client_entries > MAX_SYNCHRONIZED_CACHED_COMMITS_PER_CLIENT
        {
            Some(CacheAdmissionFailure::PerClientEntryLimit)
        } else if client_id.is_some()
            && new_client_obligations > MAX_SYNCHRONIZED_CACHED_OBLIGATIONS_PER_CLIENT
        {
            Some(CacheAdmissionFailure::PerClientObligationLimit)
        } else if new_total_entries > MAX_SYNCHRONIZED_CACHED_COMMITS_TOTAL {
            Some(CacheAdmissionFailure::TotalEntryLimit)
        } else if new_total_obligations > MAX_SYNCHRONIZED_CACHED_OBLIGATIONS_TOTAL {
            Some(CacheAdmissionFailure::TotalObligationLimit)
        } else {
            None
        };
        let Some(role) = self.roles.get_mut(&surface_id) else {
            return CacheCommitOutcome::Rejected {
                commit: Box::new(commit),
                reason: CacheAdmissionFailure::MissingRole,
            };
        };
        if let Some(reason) = rejection {
            return CacheCommitOutcome::Rejected {
                commit: Box::new(commit),
                reason,
            };
        }
        let outcome = if can_merge {
            CacheCommitOutcome::Merged {
                superseded_buffer: role
                    .cached_commits
                    .back_mut()
                    .expect("merge target exists")
                    .merge(commit)
                    .map(Box::new),
            }
        } else {
            role.cached_commits.push_back(commit);
            CacheCommitOutcome::Inserted
        };
        self.replace_cached_accounting(
            surface_id,
            client_id.as_ref(),
            old_entries,
            new_entries,
            old_obligations,
            new_obligations,
        );
        debug_assert!(self.debug_accounting_is_consistent());
        outcome
    }

    pub(in crate::compositor) fn cached_pointer_constraint_hint(
        &self,
        surface_id: u32,
        constraint_id: u64,
    ) -> Option<(f64, f64)> {
        self.roles
            .get(&surface_id)
            .into_iter()
            .flat_map(|role| role.cached_commits.iter().rev())
            .find_map(|commit| match &commit.pointer_constraint_state {
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
    }

    pub(in crate::compositor) fn cached_node_count(&self) -> usize {
        self.cached_nodes
    }

    pub(in crate::compositor) fn cached_entry_count(&self) -> usize {
        self.cached_entries_total
    }

    pub(in crate::compositor) fn cached_obligation_count(&self) -> usize {
        self.cached_obligations_total
    }

    pub(in crate::compositor) fn maximum_cached_entries(&self) -> usize {
        self.maximum_cached_entries
    }

    pub(in crate::compositor) fn maximum_cached_entries_per_surface(&self) -> usize {
        self.maximum_cached_entries_per_surface
    }

    pub(in crate::compositor) fn maximum_cached_entries_per_client(&self) -> usize {
        self.maximum_cached_entries_per_client
    }

    pub(in crate::compositor) fn maximum_cached_obligations(&self) -> usize {
        self.maximum_cached_obligations
    }

    pub(in crate::compositor) fn maximum_cached_obligations_per_surface(&self) -> usize {
        self.maximum_cached_obligations_per_surface
    }

    pub(in crate::compositor) fn maximum_cached_obligations_per_client(&self) -> usize {
        self.maximum_cached_obligations_per_client
    }

    pub(in crate::compositor) fn maximum_depth(&self) -> usize {
        self.roles
            .keys()
            .map(|surface_id| {
                let mut depth = 1;
                let mut current = *surface_id;
                while let Some(role) = self.roles.get(&current) {
                    if !self.roles.contains_key(&role.parent_id) {
                        break;
                    }
                    depth += 1;
                    current = role.parent_id;
                }
                depth
            })
            .max()
            .unwrap_or(0)
    }

    pub(in crate::compositor) fn set_pending_position(
        &mut self,
        surface_id: u32,
        x: i32,
        y: i32,
    ) -> bool {
        let Some(role) = self.roles.get_mut(&surface_id) else {
            return false;
        };
        role.pending_position = Some((x, y));
        true
    }

    pub(in crate::compositor) fn take_pending_positions_for_parent(
        &mut self,
        parent_id: u32,
    ) -> Vec<CapturedSubsurfacePosition> {
        let mut positions = self
            .roles
            .iter_mut()
            .filter_map(|(surface_id, role)| {
                (role.parent_id == parent_id)
                    .then(|| {
                        role.pending_position
                            .take()
                            .map(|(x, y)| CapturedSubsurfacePosition {
                                relationship: CapturedSubsurfaceRelationship {
                                    surface_id: *surface_id,
                                    parent_id,
                                    relationship_id: role.relationship_id,
                                },
                                x,
                                y,
                            })
                    })
                    .flatten()
            })
            .collect::<Vec<_>>();
        positions.sort_by_key(|position| position.relationship.relationship_id);
        positions
    }

    pub(in crate::compositor) fn capture_subsurface_stack(
        &self,
        parent_id: u32,
        stack: Vec<u32>,
    ) -> Vec<CapturedSubsurfaceStackEntry> {
        stack
            .into_iter()
            .filter_map(|surface_id| {
                if surface_id == parent_id {
                    Some(CapturedSubsurfaceStackEntry::Parent)
                } else {
                    self.captured_relationship(surface_id)
                        .filter(|relationship| relationship.parent_id == parent_id)
                        .map(CapturedSubsurfaceStackEntry::Child)
                }
            })
            .collect()
    }

    pub(in crate::compositor) fn take_cached_commits_for_surface(
        &mut self,
        surface_id: u32,
    ) -> Vec<CachedSubsurfaceCommit> {
        let (client_id, old_entries, old_obligations, commits) = {
            let Some(role) = self.roles.get_mut(&surface_id) else {
                return Vec::new();
            };
            let client_id = role.client_id.clone();
            let old_entries = role.cached_commits.len();
            let old_obligations = self
                .cached_obligations_per_surface
                .get(&surface_id)
                .copied()
                .unwrap_or_default();
            let commits = role.cached_commits.drain(..).collect::<Vec<_>>();
            (client_id, old_entries, old_obligations, commits)
        };
        self.replace_cached_accounting(
            surface_id,
            client_id.as_ref(),
            old_entries,
            0,
            old_obligations,
            0,
        );
        commits
    }

    pub(in crate::compositor) fn take_cached_commits_through(
        &mut self,
        reference: ContentUpdateRef,
    ) -> Option<Vec<CachedSubsurfaceCommit>> {
        let (client_id, old_entries, old_obligations, selected, new_entries, new_obligations) = {
            let role = self.roles.get_mut(&reference.surface_id)?;
            let target_index = role
                .cached_commits
                .iter()
                .position(|commit| commit.content_update_ref(reference.surface_id) == reference)?;
            let old_entries = role.cached_commits.len();
            let old_obligations = self
                .cached_obligations_per_surface
                .get(&reference.surface_id)
                .copied()
                .unwrap_or_default();
            let remaining = role
                .cached_commits
                .split_off(target_index.saturating_add(1));
            let client_id = role.client_id.clone();
            let selected = std::mem::replace(&mut role.cached_commits, remaining);
            let new_entries = role.cached_commits.len();
            let new_obligations = role
                .cached_commits
                .iter()
                .map(CachedSubsurfaceCommit::cached_obligation_count)
                .sum();
            (
                client_id,
                old_entries,
                old_obligations,
                selected.into_iter().collect::<Vec<_>>(),
                new_entries,
                new_obligations,
            )
        };
        self.replace_cached_accounting(
            reference.surface_id,
            client_id.as_ref(),
            old_entries,
            new_entries,
            old_obligations,
            new_obligations,
        );
        debug_assert!(self.debug_accounting_is_consistent());
        Some(selected)
    }

    pub(in crate::compositor) fn oldest_cached_content_update_ref(
        &self,
        surface_id: u32,
    ) -> Option<ContentUpdateRef> {
        self.roles
            .get(&surface_id)
            .and_then(|role| role.cached_commits.front())
            .map(|commit| commit.content_update_ref(surface_id))
    }

    pub(in crate::compositor) fn subsurface_tree_ids(&self, root_surface_id: u32) -> Vec<u32> {
        let mut tree = vec![root_surface_id];
        let mut index = 0;
        while let Some(parent_id) = tree.get(index).copied() {
            let mut children = self
                .roles
                .iter()
                .filter_map(|(surface_id, role)| {
                    (role.parent_id == parent_id).then_some(*surface_id)
                })
                .collect::<Vec<_>>();
            children.sort_unstable();
            tree.extend(children);
            index = index.saturating_add(1);
        }
        tree
    }

    fn replace_cached_accounting(
        &mut self,
        surface_id: u32,
        client_id: Option<&ClientId>,
        old_entries: usize,
        new_entries: usize,
        old_obligations: usize,
        new_obligations: usize,
    ) {
        self.cached_entries_total =
            replace_cached_count(self.cached_entries_total, old_entries, new_entries);
        self.cached_obligations_total = replace_cached_count(
            self.cached_obligations_total,
            old_obligations,
            new_obligations,
        );
        replace_cached_map_count(
            &mut self.cached_entries_per_surface,
            surface_id,
            old_entries,
            new_entries,
        );
        replace_cached_map_count(
            &mut self.cached_obligations_per_surface,
            surface_id,
            old_obligations,
            new_obligations,
        );
        if let Some(client_id) = client_id {
            replace_cached_map_count(
                &mut self.cached_entries_per_client,
                client_id.clone(),
                old_entries,
                new_entries,
            );
            replace_cached_map_count(
                &mut self.cached_obligations_per_client,
                client_id.clone(),
                old_obligations,
                new_obligations,
            );
        }
        match (old_entries == 0, new_entries == 0) {
            (true, false) => {
                self.cached_nodes = self
                    .cached_nodes
                    .checked_add(1)
                    .expect("synchronized cache node count overflow");
            }
            (false, true) => {
                self.cached_nodes = self
                    .cached_nodes
                    .checked_sub(1)
                    .expect("synchronized cache node count underflow");
            }
            _ => {}
        }
        self.maximum_cached_entries = self.maximum_cached_entries.max(self.cached_entries_total);
        self.maximum_cached_entries_per_surface =
            self.maximum_cached_entries_per_surface.max(new_entries);
        self.maximum_cached_obligations = self
            .maximum_cached_obligations
            .max(self.cached_obligations_total);
        self.maximum_cached_obligations_per_surface = self
            .maximum_cached_obligations_per_surface
            .max(new_obligations);
        if let Some(client_id) = client_id {
            let entries = self
                .cached_entries_per_client
                .get(client_id)
                .copied()
                .unwrap_or_default();
            let obligations = self
                .cached_obligations_per_client
                .get(client_id)
                .copied()
                .unwrap_or_default();
            self.maximum_cached_entries_per_client =
                self.maximum_cached_entries_per_client.max(entries);
            self.maximum_cached_obligations_per_client =
                self.maximum_cached_obligations_per_client.max(obligations);
        }
    }

    #[allow(clippy::mutable_key_type)]
    fn debug_accounting_is_consistent(&self) -> bool {
        let mut entries_total = 0usize;
        let mut obligations_total = 0usize;
        let mut nodes = 0usize;
        let mut entries_per_surface = HashMap::new();
        let mut obligations_per_surface = HashMap::new();
        let mut entries_per_client = HashMap::new();
        let mut obligations_per_client = HashMap::new();
        for (surface_id, role) in &self.roles {
            let entries = role.cached_commits.len();
            let obligations = role
                .cached_commits
                .iter()
                .map(CachedSubsurfaceCommit::cached_obligation_count)
                .sum::<usize>();
            entries_total += entries;
            obligations_total += obligations;
            if entries != 0 {
                nodes += 1;
                entries_per_surface.insert(*surface_id, entries);
            }
            if obligations != 0 {
                obligations_per_surface.insert(*surface_id, obligations);
            }
            if let Some(client_id) = role.client_id.as_ref() {
                if entries != 0 {
                    *entries_per_client.entry(client_id.clone()).or_insert(0) += entries;
                }
                if obligations != 0 {
                    *obligations_per_client.entry(client_id.clone()).or_insert(0) += obligations;
                }
            }
        }
        entries_total == self.cached_entries_total
            && obligations_total == self.cached_obligations_total
            && nodes == self.cached_nodes
            && entries_per_surface == self.cached_entries_per_surface
            && obligations_per_surface == self.cached_obligations_per_surface
            && entries_per_client == self.cached_entries_per_client
            && obligations_per_client == self.cached_obligations_per_client
    }
}

impl SurfaceTransactionState {
    pub(in crate::compositor) const fn metrics(&self) -> SubsurfaceTransactionMetrics {
        self.metrics
    }

    pub(in crate::compositor) fn pending_tree_count(&self) -> usize {
        self.pending_surface_tree_transactions.len()
    }

    pub(in crate::compositor) fn has_pending_trees(&self) -> bool {
        !self.pending_surface_tree_transactions.is_empty()
    }

    pub(in crate::compositor) fn pending_trees(
        &self,
    ) -> impl Iterator<Item = &PendingSurfaceTreeTransaction> {
        self.pending_surface_tree_transactions.iter()
    }

    pub(in crate::compositor) fn pending_tree_at(
        &self,
        index: usize,
    ) -> Option<&PendingSurfaceTreeTransaction> {
        self.pending_surface_tree_transactions.get(index)
    }

    pub(in crate::compositor) fn contains_content_update(
        &self,
        reference: ContentUpdateRef,
    ) -> bool {
        self.pending_surface_tree_transactions
            .iter()
            .any(|transaction| transaction_covers_content_update_ref(transaction, reference))
    }

    pub(in crate::compositor) fn pending_root_heads(
        &self,
    ) -> impl Iterator<Item = &PendingSurfaceTreeTransaction> {
        self.pending_surface_tree_transactions
            .iter()
            .enumerate()
            .filter_map(|(index, transaction)| {
                (!self.pending_surface_tree_transactions[..index]
                    .iter()
                    .any(|previous| previous.root_surface_id == transaction.root_surface_id))
                .then_some(transaction)
            })
    }

    pub(in crate::compositor) fn transaction_by_id(
        &self,
        id: SurfaceTreeTransactionId,
    ) -> Option<&PendingSurfaceTreeTransaction> {
        self.pending_surface_tree_transactions
            .iter()
            .find(|transaction| transaction.id == id)
    }

    pub(in crate::compositor) fn transaction_by_id_mut(
        &mut self,
        id: SurfaceTreeTransactionId,
    ) -> Option<&mut PendingSurfaceTreeTransaction> {
        self.pending_surface_tree_transactions
            .iter_mut()
            .find(|transaction| transaction.id == id)
    }

    pub(in crate::compositor) fn is_root_head(&self, id: SurfaceTreeTransactionId) -> bool {
        let Some(index) = self
            .pending_surface_tree_transactions
            .iter()
            .position(|transaction| transaction.id == id)
        else {
            return false;
        };
        let root_surface_id = self.pending_surface_tree_transactions[index].root_surface_id;
        !self.pending_surface_tree_transactions[..index]
            .iter()
            .any(|transaction| transaction.root_surface_id == root_surface_id)
    }

    pub(in crate::compositor) fn contains_pending_surface(&self, surface_id: u32) -> bool {
        self.pending_surface_tree_transactions
            .iter()
            .any(|transaction| transaction.nodes.iter().any(|(id, _)| *id == surface_id))
    }

    pub(in crate::compositor) fn take_pending_trees(
        &mut self,
    ) -> Vec<PendingSurfaceTreeTransaction> {
        std::mem::take(&mut self.pending_surface_tree_transactions)
    }

    pub(in crate::compositor) fn replace_pending_trees(
        &mut self,
        transactions: Vec<PendingSurfaceTreeTransaction>,
    ) {
        self.pending_surface_tree_transactions = transactions;
    }

    pub(in crate::compositor) fn push_pending_tree(
        &mut self,
        transaction: PendingSurfaceTreeTransaction,
    ) {
        self.pending_surface_tree_transactions.push(transaction);
    }

    pub(in crate::compositor) fn allocate_surface_tree_transaction_id(
        &mut self,
    ) -> SurfaceTreeTransactionId {
        self.next_surface_tree_transaction_id = self
            .next_surface_tree_transaction_id
            .checked_add(1)
            .expect("surface tree transaction ID overflow");
        SurfaceTreeTransactionId::new(self.next_surface_tree_transaction_id)
    }

    pub(super) fn allocate_acquire_commit_id(&mut self) -> Option<AcquireCommitId> {
        self.acquire_commit_ids.allocate()
    }
}

#[cfg(test)]
#[path = "tests/cache.rs"]
mod cache_tests;
#[cfg(test)]
#[path = "tests/promotion.rs"]
mod promotion_tests;
#[cfg(test)]
#[path = "tests/relationship.rs"]
mod relationship_tests;
#[cfg(test)]
#[path = "tests/mod.rs"]
mod surface_tree_tests;

fn replace_cached_count(current: usize, old: usize, new: usize) -> usize {
    current
        .checked_sub(old)
        .and_then(|count| count.checked_add(new))
        .expect("synchronized cache accounting drift")
}

fn checked_replace_cached_count(current: usize, old: usize, new: usize) -> Option<usize> {
    current.checked_sub(old)?.checked_add(new)
}

fn replace_cached_map_count<K>(counts: &mut HashMap<K, usize>, key: K, old: usize, new: usize)
where
    K: Eq + std::hash::Hash,
{
    let current = counts.get(&key).copied().unwrap_or_default();
    let updated = replace_cached_count(current, old, new);
    if updated == 0 {
        counts.remove(&key);
    } else {
        counts.insert(key, updated);
    }
}

#[cfg(test)]
#[path = "tests/roles.rs"]
mod role_tests;
