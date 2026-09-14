#![allow(clippy::question_mark)]

use super::*;
use crate::compositor::subsurface::{
    CapturedContentUpdateLineage, CapturedSubsurfaceParentState, CapturedSubsurfaceStackEntry,
    CapturedSurfaceCommitContext, ContentUpdateRef,
};

#[derive(Debug)]
struct PreparedContentUpdateCandidate {
    root_surface_id: u32,
    nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    external_content_update_dependencies: Vec<ContentUpdateRef>,
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    fn test_cached_commit(sequence: u64) -> CachedSubsurfaceCommit {
        let mut commit = crate::compositor::state::empty_cached_subsurface_commit();
        commit.commit_id = SurfaceCommitId::for_tests(sequence);
        commit.commit_sequence = SurfaceCommitSequence(sequence);
        commit.pacing.fifo_set_barrier = true;
        commit
    }

    #[test]
    fn candidate_extraction_follows_exact_direct_child_edges() {
        let mut state = CompositorState::default();
        assert!(state.subsurface_transactions.register(2, 1));
        assert!(state.subsurface_transactions.register(3, 2));

        let grandchild = test_cached_commit(10);
        let grandchild_ref = grandchild.content_update_ref(3);
        assert!(matches!(
            state.subsurface_transactions.cache_commit(3, grandchild),
            CacheCommitOutcome::Inserted
        ));
        let dependencies = state
            .subsurface_transactions
            .capture_direct_child_dependencies(2);
        assert_eq!(dependencies, vec![grandchild_ref]);

        let later_grandchild = test_cached_commit(11);
        assert!(matches!(
            state
                .subsurface_transactions
                .cache_commit(3, later_grandchild),
            CacheCommitOutcome::Inserted
        ));

        let mut child = test_cached_commit(12);
        child.lineage.child_dependencies = dependencies;
        let candidate = state.extract_content_update_candidate(2, child);

        assert_eq!(
            candidate
                .nodes
                .iter()
                .map(|(_, commit)| commit.commit_sequence)
                .collect::<Vec<_>>(),
            vec![SurfaceCommitSequence(10), SurfaceCommitSequence(12)]
        );
        assert!(candidate.external_content_update_dependencies.is_empty());
        let remaining = state
            .subsurface_transactions
            .take_cached_commits_for_surface(3);
        assert_eq!(
            remaining
                .iter()
                .map(|commit| commit.commit_sequence)
                .collect::<Vec<_>>(),
            vec![SurfaceCommitSequence(11)]
        );
    }

    #[test]
    fn candidate_extraction_does_not_drain_orphan_grandchildren() {
        let mut state = CompositorState::default();
        assert!(state.subsurface_transactions.register(2, 1));
        assert!(state.subsurface_transactions.register(3, 2));
        assert!(matches!(
            state
                .subsurface_transactions
                .cache_commit(3, test_cached_commit(20)),
            CacheCommitOutcome::Inserted
        ));

        let candidate = state.extract_content_update_candidate(1, test_cached_commit(21));

        assert_eq!(candidate.nodes.len(), 1);
        assert_eq!(candidate.nodes[0].0, 1);
        assert_eq!(
            state
                .subsurface_transactions
                .take_cached_commits_for_surface(3)
                .len(),
            1
        );
    }

    #[test]
    fn relationship_detach_does_not_create_a_fake_parent_update_for_a_sync_grandchild() {
        let mut state = CompositorState::default();
        assert!(state.subsurface_transactions.register(2, 1));
        assert!(state.subsurface_transactions.register(3, 2));
        assert!(matches!(
            state
                .subsurface_transactions
                .cache_commit(3, test_cached_commit(25)),
            CacheCommitOutcome::Inserted
        ));

        state.destroy_subsurface_role(2);

        assert!(state.pending_surface_tree_transactions.is_empty());
        let retained = state
            .subsurface_transactions
            .take_cached_commits_for_surface(3);
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].commit_sequence, SurfaceCommitSequence(25));
    }

    #[test]
    fn cached_same_surface_prefix_is_emitted_in_predecessor_order() {
        let mut state = CompositorState::default();
        assert!(state.subsurface_transactions.register(2, 1));
        for sequence in 30..=32 {
            assert!(matches!(
                state
                    .subsurface_transactions
                    .cache_commit(2, test_cached_commit(sequence)),
                CacheCommitOutcome::Inserted
            ));
        }
        let reference = ContentUpdateRef {
            surface_id: 2,
            commit_id: SurfaceCommitId::for_tests(32),
            commit_sequence: SurfaceCommitSequence(32),
        };
        let mut dependent = test_cached_commit(33);
        dependent.lineage.child_dependencies = vec![reference];

        let candidate = state.extract_content_update_candidate(1, dependent);

        assert_eq!(
            candidate
                .nodes
                .iter()
                .map(|(_, commit)| commit.commit_sequence)
                .collect::<Vec<_>>(),
            vec![
                SurfaceCommitSequence(30),
                SurfaceCommitSequence(31),
                SurfaceCommitSequence(32),
                SurfaceCommitSequence(33),
            ]
        );
    }

    #[test]
    fn external_content_update_dependencies_wait_for_their_owner() {
        let mut state = CompositorState::default();
        let dependency = ContentUpdateRef {
            surface_id: 2,
            commit_id: SurfaceCommitId::for_tests(40),
            commit_sequence: SurfaceCommitSequence(40),
        };
        state
            .pending_surface_tree_transactions
            .push(PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(1),
                root_surface_id: 1,
                nodes: vec![(2, test_cached_commit(40))],
                publication_lifetimes: SurfaceTreeNodeLifetimes::Synthetic,
                dependencies: Vec::new(),
                external_content_update_dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            });
        let waiting = PendingSurfaceTreeTransaction {
            id: SurfaceTreeTransactionId::new(2),
            root_surface_id: 3,
            nodes: vec![(3, test_cached_commit(41))],
            publication_lifetimes: SurfaceTreeNodeLifetimes::Synthetic,
            dependencies: Vec::new(),
            external_content_update_dependencies: vec![dependency],
            commit_timing_readiness: None,
            received_at: Instant::now(),
        };

        assert!(!state.content_update_dependencies_ready(&waiting));
        state.pending_surface_tree_transactions.clear();
        state.surface_publications.insert(
            2,
            SurfacePublicationState {
                latest_published: Some(SurfaceCommitSequence(40)),
                ..SurfacePublicationState::default()
            },
        );
        assert!(state.content_update_dependencies_ready(&waiting));
    }
}

struct ContentUpdateCandidateExtractor<'a> {
    state: &'a mut CompositorState,
    nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    external_content_update_dependencies: Vec<ContentUpdateRef>,
    seen: HashSet<ContentUpdateRef>,
    visiting: HashSet<ContentUpdateRef>,
}

impl ContentUpdateCandidateExtractor<'_> {
    fn visit_commit(&mut self, surface_id: u32, commit: CachedSubsurfaceCommit) {
        let reference = commit.content_update_ref(surface_id);
        if !self.seen.insert(reference) {
            return;
        }
        if !self.visiting.insert(reference) {
            debug_assert!(false, "content update dependency cycle detected");
            return;
        }
        let lineage = commit.lineage.clone();
        if let Some(predecessor) = lineage.predecessor {
            self.visit_reference(predecessor);
        }
        for dependency in lineage.child_dependencies {
            self.visit_reference(dependency);
        }
        self.visiting.remove(&reference);
        self.nodes.push((surface_id, commit));
    }

    fn visit_reference(&mut self, reference: ContentUpdateRef) {
        if self.visiting.contains(&reference) {
            debug_assert!(false, "content update dependency cycle detected");
            return;
        }
        if self.seen.contains(&reference) {
            return;
        }
        if let Some(commits) = self
            .state
            .subsurface_transactions
            .take_cached_commits_through(reference)
        {
            for commit in commits {
                self.visit_commit(reference.surface_id, commit);
            }
            return;
        }
        if self.state.content_update_ref_is_published(reference) {
            return;
        }
        if self.state.content_update_ref_is_terminal(reference) {
            return;
        }
        if !self
            .external_content_update_dependencies
            .contains(&reference)
        {
            self.external_content_update_dependencies.push(reference);
        }
    }
}

impl CompositorState {
    const MAX_SURFACE_TREE_TRANSACTIONS_PER_ROOT: usize = 8;

    fn content_update_ref_is_published(&self, reference: ContentUpdateRef) -> bool {
        self.surface_publications
            .get(&reference.surface_id)
            .and_then(|publication| publication.latest_published)
            .is_some_and(|published| published >= reference.commit_sequence)
    }

    fn content_update_ref_is_pending(&self, reference: ContentUpdateRef) -> bool {
        self.pending_surface_tree_transactions
            .iter()
            .any(|transaction| {
                transaction
                    .nodes
                    .iter()
                    .any(|(surface_id, commit)| commit.content_update_ref(*surface_id) == reference)
            })
    }

    fn content_update_ref_is_terminal(&self, reference: ContentUpdateRef) -> bool {
        !self.surface_resources.contains_key(&reference.surface_id)
            || self
                .surface_client_ids
                .get(&reference.surface_id)
                .is_some_and(|client_id| self.terminal_client_ids.contains(client_id))
    }

    pub(in crate::compositor) fn content_update_dependencies_ready(
        &self,
        transaction: &PendingSurfaceTreeTransaction,
    ) -> bool {
        transaction
            .external_content_update_dependencies
            .iter()
            .all(|dependency| {
                transaction.nodes.iter().any(|(surface_id, commit)| {
                    commit.content_update_ref(*surface_id) == *dependency
                }) || self.content_update_ref_is_published(*dependency)
                    || (!self.content_update_ref_is_pending(*dependency)
                        && self.content_update_ref_is_terminal(*dependency))
            })
    }

    fn extract_content_update_candidate(
        &mut self,
        root_surface_id: u32,
        root_commit: CachedSubsurfaceCommit,
    ) -> PreparedContentUpdateCandidate {
        let mut extractor = ContentUpdateCandidateExtractor {
            state: self,
            nodes: Vec::new(),
            external_content_update_dependencies: Vec::new(),
            seen: HashSet::new(),
            visiting: HashSet::new(),
        };
        extractor.visit_commit(root_surface_id, root_commit);
        PreparedContentUpdateCandidate {
            root_surface_id,
            nodes: extractor.nodes,
            external_content_update_dependencies: extractor.external_content_update_dependencies,
        }
    }

    pub(in crate::compositor) fn capture_content_update_lineage(
        &mut self,
        surface_id: u32,
        commit_id: SurfaceCommitId,
        commit_sequence: SurfaceCommitSequence,
    ) -> CapturedContentUpdateLineage {
        let predecessor = self
            .surface_publications
            .get(&surface_id)
            .map(|publication| publication.latest_received)
            .filter(|sequence| *sequence != SurfaceCommitSequence::initial())
            .map(|previous_sequence| ContentUpdateRef {
                surface_id,
                commit_id: SurfaceCommitId::from_sequence(previous_sequence),
                commit_sequence: previous_sequence,
            });
        let child_dependencies = self
            .subsurface_transactions
            .capture_direct_child_dependencies(surface_id);
        debug_assert!(child_dependencies.iter().all(|dependency| {
            dependency.commit_sequence < commit_sequence && dependency.commit_id != commit_id
        }));
        CapturedContentUpdateLineage {
            predecessor,
            child_dependencies,
            merge_frozen: false,
        }
    }

    fn normalize_explicit_sync_commit(&mut self, commit: &mut CachedSubsurfaceCommit) -> bool {
        let has_buffer = matches!(
            commit.attachment.as_ref(),
            Some(PendingSurfaceAttachment::Buffer(_))
        );
        let Some(explicit_sync) = commit.explicit_sync.as_ref() else {
            return true;
        };
        if has_buffer {
            return true;
        }
        if explicit_sync.has_points() {
            explicit_sync.state.post_error_with_metrics(
                &mut self.compliance_metrics,
                &mut self.protocol_error_trace,
                &mut self.terminal_client_ids,
                SYNCOBJ_SURFACE_ERROR_NO_BUFFER,
                "explicit sync points were set without an attached buffer",
            );
            return false;
        }
        commit.explicit_sync = None;
        true
    }

    pub(in crate::compositor) fn register_subsurface_relationship(
        &mut self,
        surface_id: u32,
        parent_id: u32,
        client_id: ClientId,
    ) -> bool {
        if !self.subsurface_transactions.register_with_client(
            surface_id,
            parent_id,
            Some(client_id),
        ) {
            return false;
        }
        self.add_subsurface_to_pending_stack(parent_id, surface_id);
        true
    }

    pub(in crate::compositor) fn is_effectively_synchronized_subsurface(
        &self,
        surface_id: u32,
    ) -> bool {
        self.subsurface_transactions
            .is_effectively_synchronized(surface_id)
    }

    pub(in crate::compositor) fn subsurface_content_is_inactive(&self, surface_id: u32) -> bool {
        match self.surface_role(surface_id) {
            SurfaceRole::Subsurface { parent_id } => {
                !self
                    .subsurface_transactions
                    .relationship_is_applied_child_of(surface_id, parent_id)
                    || !self.subsurface_parent_is_mapped(parent_id)
            }
            // Destroying wl_subsurface removes only the live relationship. The
            // permanent role remains, so dormant subsurface commits must retain
            // current content without becoming eligible for presentation.
            SurfaceRole::Unassigned
                if self.permanent_surface_role(surface_id)
                    == Some(PermanentSurfaceRole::Subsurface) =>
            {
                true
            }
            _ => false,
        }
    }

    pub(in crate::compositor) fn subsurface_parent_is_mapped(&self, parent_id: u32) -> bool {
        self.renderable_surface_index(parent_id).is_some()
    }

    pub(in crate::compositor) fn subsurface_can_map(&self, surface_id: u32) -> bool {
        let SurfaceRole::Subsurface { parent_id } = self.surface_role(surface_id) else {
            return false;
        };
        self.subsurface_transactions
            .relationship_is_applied_child_of(surface_id, parent_id)
            && self.subsurface_parent_is_mapped(parent_id)
            && self.current_surface_buffers.contains_key(&surface_id)
    }

    pub(in crate::compositor) fn reconcile_applied_subsurface_mapping(
        &mut self,
        parent_id: u32,
    ) -> bool {
        if !self.subsurface_parent_is_mapped(parent_id) {
            return false;
        }
        let mut changed = false;
        for child_id in self.subsurface_transactions.applied_children_of(parent_id) {
            changed |= self.adopt_current_surface_content_for_role(child_id);
        }
        changed
    }

    pub(in crate::compositor) fn set_subsurface_sync_mode(
        &mut self,
        surface_id: u32,
        mode: SubsurfaceSyncMode,
    ) {
        if self.subsurface_transactions.requested_mode(surface_id) == Some(mode) {
            return;
        }
        let affected_surfaces = self.subsurface_transactions.subsurface_tree_ids(surface_id);
        let was_effectively_synchronized = affected_surfaces
            .iter()
            .map(|surface_id| {
                (
                    *surface_id,
                    self.is_effectively_synchronized_subsurface(*surface_id),
                )
            })
            .collect::<HashMap<_, _>>();
        if !self.subsurface_transactions.set_mode(surface_id, mode) {
            return;
        }
        self.reclassify_unreachable_synchronized_commits(
            &affected_surfaces,
            &was_effectively_synchronized,
            None,
        );
    }

    fn reclassify_unreachable_synchronized_commits(
        &mut self,
        affected_surfaces: &[u32],
        was_effectively_synchronized: &HashMap<u32, bool>,
        detached_root_commits: Option<(u32, Vec<CachedSubsurfaceCommit>)>,
    ) {
        let mut detached_root_commits = detached_root_commits;
        for surface_id in affected_surfaces {
            let was_synchronized = was_effectively_synchronized
                .get(surface_id)
                .copied()
                .unwrap_or(false);
            if !was_synchronized || self.is_effectively_synchronized_subsurface(*surface_id) {
                continue;
            }
            let commits = detached_root_commits
                .take()
                .filter(|(detached_surface_id, _)| detached_surface_id == surface_id)
                .map(|(_, commits)| commits)
                .unwrap_or_default();
            if !commits.is_empty() {
                for commit in commits {
                    let candidate = self.extract_content_update_candidate(*surface_id, commit);
                    self.submit_content_update_candidate(
                        candidate,
                        SurfaceTreeSubmissionKind::InternalMigration,
                    );
                }
            }
            while let Some(reference) = self
                .subsurface_transactions
                .oldest_cached_content_update_ref(*surface_id)
            {
                let Some(mut commits) = self
                    .subsurface_transactions
                    .take_cached_commits_through(reference)
                else {
                    break;
                };
                for commit in commits.drain(..) {
                    let candidate = self.extract_content_update_candidate(*surface_id, commit);
                    self.submit_content_update_candidate(
                        candidate,
                        SurfaceTreeSubmissionKind::InternalMigration,
                    );
                }
            }
        }
        self.update_synchronized_cache_metrics();
        self.commit_ready_surface_tree_transactions();
    }

    fn submit_content_update_candidate(
        &mut self,
        candidate: PreparedContentUpdateCandidate,
        submission_kind: SurfaceTreeSubmissionKind,
    ) {
        self.submit_surface_tree_nodes_with_kind(
            candidate.root_surface_id,
            candidate.nodes,
            candidate.external_content_update_dependencies,
            submission_kind,
        );
    }

    pub(in crate::compositor) fn set_pending_subsurface_position(
        &mut self,
        surface_id: u32,
        x: i32,
        y: i32,
    ) {
        self.subsurface_transactions
            .set_pending_position(surface_id, x, y);
    }

    fn capture_surface_commit_context(
        &mut self,
        surface_id: u32,
    ) -> Result<CapturedSurfaceCommitContext, ()> {
        let layer_surface = self.capture_layer_surface_commit_state(surface_id)?;
        let activations = self
            .subsurface_transactions
            .take_pending_relationship_activations_for_parent(surface_id);
        let positions = self
            .subsurface_transactions
            .take_pending_positions_for_parent(surface_id);
        let live_stack = self.pending_subsurface_stacks.remove(&surface_id);
        if let Some(stack) = &live_stack {
            self.latched_subsurface_stacks
                .insert(surface_id, stack.clone());
        }
        let stack = live_stack.map(|stack| {
            self.subsurface_transactions
                .capture_subsurface_stack(surface_id, stack)
        });
        Ok(CapturedSurfaceCommitContext {
            subsurface_parent: CapturedSubsurfaceParentState {
                activations,
                positions,
                stack,
            },
            layer_surface,
        })
    }

    pub(in crate::compositor) fn commit_surface_tree_request(
        &mut self,
        surface_id: u32,
        mut commit: CachedSubsurfaceCommit,
    ) {
        let commit_context = match self.capture_surface_commit_context(surface_id) {
            Ok(context) => context,
            Err(()) => {
                self.release_unpublished_surface_tree_nodes(vec![(surface_id, commit)]);
                return;
            }
        };
        commit.commit_context = commit_context;
        if !self.normalize_explicit_sync_commit(&mut commit) {
            self.release_unpublished_surface_tree_nodes(vec![(surface_id, commit)]);
            return;
        }
        if self.xdg_surface_is_constructed(surface_id) {
            match commit.attachment.as_ref() {
                Some(PendingSurfaceAttachment::Buffer(_))
                    if !self.xdg_surface_is_configured(surface_id) =>
                {
                    if let Some(surface) = self.surface_resource_by_id(surface_id)
                        && let Some(client) = surface.client()
                        && let Some(xdg_surface) =
                            self.xdg_surface_resources.get(&surface_id).cloned()
                    {
                        self.post_protocol_error(
                            &client,
                            &xdg_surface,
                            xdg_surface::Error::UnconfiguredBuffer,
                            "xdg_surface buffer commit was not preceded by an acknowledged configure"
                                .to_string(),
                        );
                    }
                    self.release_unpublished_surface_tree_nodes(vec![(surface_id, commit)]);
                    return;
                }
                Some(PendingSurfaceAttachment::RemoveContent) => {
                    self.begin_xdg_empty_or_unmap_commit(surface_id);
                    self.configure_xdg_surface_if_needed(surface_id);
                }
                None => {
                    if self.mark_xdg_empty_commit(surface_id) {
                        self.configure_xdg_surface_if_needed(surface_id);
                    }
                }
                _ => {}
            }
        }
        if self.is_effectively_synchronized_subsurface(surface_id) {
            self.cache_synchronized_subsurface_commit(surface_id, commit);
            return;
        }
        if commit.attachment.is_some() {
            let mut superseded_callbacks = self.supersede_older_pending_attachments_for_surface(
                surface_id,
                commit.commit_sequence,
            );
            superseded_callbacks.extend(commit.frame_callbacks);
            commit.frame_callbacks = superseded_callbacks;
        }
        match commit.attachment.as_mut() {
            Some(PendingSurfaceAttachment::Buffer(pending)) => {
                if let Some(surface) = self.surface_resource_by_id(surface_id)
                    && let Some(data) = surface.data::<SurfaceData>()
                {
                    let viewport = data.viewport_for_change(commit.viewport_destination);
                    let buffer_scale = data.buffer_scale_for_change(commit.buffer_scale);
                    let buffer_transform =
                        data.buffer_transform_for_change(commit.buffer_transform);
                    if pending
                        .apply_committed_surface_state(viewport, buffer_scale, buffer_transform)
                        .is_err()
                    {
                        if let Some(client) = surface.client() {
                            self.post_protocol_error(
                                &client,
                                &surface,
                                wl_surface::Error::InvalidSize,
                                "buffer dimensions are not integral after transform and scale"
                                    .to_string(),
                            );
                        }
                        self.release_pending_surface_buffer(pending.clone());
                        self.complete_frame_callbacks(std::mem::take(&mut commit.frame_callbacks));
                        return;
                    }
                }
                self.finalize_pending_buffer_resize_capture(
                    surface_id,
                    pending,
                    commit.window_geometry,
                );
            }
            _ => {
                commit.resize_commit = self
                    .capture_acked_resize_for_surface_commit(surface_id)
                    .map(|snapshot| {
                        commit.window_geometry.map_or(snapshot, |window_geometry| {
                            snapshot.with_committed_window_geometry(window_geometry)
                        })
                    });
                commit.resize_capture_finalized = true;
            }
        }
        let candidate = self.extract_content_update_candidate(surface_id, commit);
        self.update_synchronized_cache_metrics();
        self.submit_surface_tree_nodes_with_kind(
            candidate.root_surface_id,
            candidate.nodes,
            candidate.external_content_update_dependencies,
            SurfaceTreeSubmissionKind::ClientAdmission,
        );
    }

    fn submit_surface_tree_nodes_with_kind(
        &mut self,
        surface_id: u32,
        mut nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        external_content_update_dependencies: Vec<ContentUpdateRef>,
        submission_kind: SurfaceTreeSubmissionKind,
    ) {
        if !self.prepare_surface_tree_surface_state(&mut nodes) {
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: surface_commit validation failed surface={surface_id}"
                );
            }
            if let Some(surface) = self.surface_resource_by_id(surface_id)
                && let Some(client) = surface.client()
            {
                self.post_protocol_error(
                    &client,
                    &surface,
                    wl_surface::Error::InvalidSize,
                    "buffer dimensions are not integral after transform and scale".to_string(),
                );
            }
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
        self.merge_or_queue_surface_tree_transaction(
            surface_id,
            nodes,
            dependencies,
            external_content_update_dependencies,
            submission_kind,
        );
    }

    fn capture_surface_tree_node_lifetimes(
        &self,
        nodes: &[(u32, CachedSubsurfaceCommit)],
    ) -> Option<SurfaceTreeNodeLifetimes> {
        let mut lifetimes = Vec::with_capacity(nodes.len());
        for (surface_id, _) in nodes {
            let (owner_client_id, surface_presentation_generation) =
                self.capture_surface_publication_lifetime(*surface_id)?;
            lifetimes.push(SurfaceTreeNodeLifetime {
                surface_id: *surface_id,
                owner_client_id,
                surface_presentation_generation,
            });
        }
        Some(SurfaceTreeNodeLifetimes::Captured(lifetimes))
    }

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
        let incoming_is_pacing_protected =
            nodes.iter().any(|(_, commit)| commit.pacing.is_boundary());
        let matching = self
            .pending_surface_tree_transactions
            .iter()
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
        let target_is_ready =
            self.transaction_is_ready(&self.pending_surface_tree_transactions[target_index]);
        let target_is_pacing_protected =
            self.pending_surface_tree_transactions[target_index].is_pacing_protected();
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
                self.subsurface_transaction_metrics
                    .ready_transactions_preserved_from_newer_unready = self
                    .subsurface_transaction_metrics
                    .ready_transactions_preserved_from_newer_unready
                    .saturating_add(1);
            } else {
                self.subsurface_transaction_metrics
                    .ready_transactions_preserved_from_newer_ready = self
                    .subsurface_transaction_metrics
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

        let mut transaction = self.pending_surface_tree_transactions.remove(target_index);
        let pacing_deadline_changed = transaction.commit_timing_readiness.is_some();
        let stats = self.merge_surface_tree_nodes_into_transaction(
            root_surface_id,
            &mut transaction,
            nodes,
            publication_lifetimes,
            dependencies,
            external_content_update_dependencies,
        );
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
        self.pending_surface_tree_transactions.push(transaction);
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
        if publication_lifetimes.len() != nodes.len() {
            return SurfaceTreeMergeStats::default();
        }
        let mut stats = SurfaceTreeMergeStats {
            incoming_nodes: nodes.len(),
            existing_nodes: transaction.nodes.len(),
            ..SurfaceTreeMergeStats::default()
        };
        for (node_index, (surface_id, incoming)) in nodes.into_iter().enumerate() {
            let incoming_lifetime = &publication_lifetimes[node_index];
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
                if let Some(lifetimes) = transaction.publication_lifetimes.captured_mut() {
                    lifetimes.push(incoming_lifetime.clone());
                }
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
            if let Some(lifetime) = transaction
                .publication_lifetimes
                .captured_mut()
                .and_then(|lifetimes| lifetimes.get_mut(existing_index))
            {
                *lifetime = incoming_lifetime.clone();
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
        stats
    }

    pub(in crate::compositor) fn update_surface_tree_slot_metrics(&mut self, root_surface_id: u32) {
        let mut ready = 0usize;
        let mut waiting = 0usize;
        for transaction in self
            .pending_surface_tree_transactions
            .iter()
            .filter(|transaction| transaction.root_surface_id == root_surface_id)
        {
            if self.transaction_is_ready(transaction) {
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
        self.subsurface_transaction_metrics
            .maximum_explicit_sync_queue_depth = self
            .subsurface_transaction_metrics
            .maximum_explicit_sync_queue_depth
            .max(ready.saturating_add(waiting));
    }

    pub(in crate::compositor) fn prepare_surface_tree_surface_state(
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
            let viewport = data.viewport_for_change(commit.viewport_destination);
            let buffer_scale = data.buffer_scale_for_change(commit.buffer_scale);
            let buffer_transform = data.buffer_transform_for_change(commit.buffer_transform);
            if pending
                .apply_committed_surface_state(viewport, buffer_scale, buffer_transform)
                .is_err()
            {
                return false;
            }
        }
        true
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
            let Some((owner_client_id, surface_presentation_generation)) =
                self.capture_surface_publication_lifetime(*surface_id)
            else {
                return None;
            };
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
        PendingSurfaceTreeTransaction {
            id: self.allocate_surface_tree_transaction_id(),
            root_surface_id,
            nodes,
            publication_lifetimes,
            dependencies,
            external_content_update_dependencies,
            commit_timing_readiness: None,
            received_at: Instant::now(),
        }
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
                .pending_surface_tree_transactions
                .iter()
                .enumerate()
                .filter_map(|(index, transaction)| {
                    (transaction.root_surface_id == root_surface_id).then_some(index)
                })
                .collect::<Vec<_>>();
            let at_capacity_with_only_ready = matching.len()
                >= Self::MAX_SURFACE_TREE_TRANSACTIONS_PER_ROOT
                && matching.iter().all(|index| {
                    self.transaction_is_ready(&self.pending_surface_tree_transactions[*index])
                });
            if at_capacity_with_only_ready {
                self.subsurface_transaction_metrics.all_ready_queue_pressure = self
                    .subsurface_transaction_metrics
                    .all_ready_queue_pressure
                    .saturating_add(1);
                self.commit_ready_surface_tree_transactions();
                matching.clear();
                matching.extend(
                    self.pending_surface_tree_transactions
                        .iter()
                        .enumerate()
                        .filter_map(|(index, transaction)| {
                            (transaction.root_surface_id == root_surface_id).then_some(index)
                        }),
                );
            }
            if matching.len() >= Self::MAX_SURFACE_TREE_TRANSACTIONS_PER_ROOT {
                self.subsurface_transaction_metrics
                    .explicit_sync_queue_overflow = self
                    .subsurface_transaction_metrics
                    .explicit_sync_queue_overflow
                    .saturating_add(1);
                self.commit_ready_surface_tree_transactions();
                let still_at_capacity = self
                    .pending_surface_tree_transactions
                    .iter()
                    .filter(|transaction| transaction.root_surface_id == root_surface_id)
                    .count()
                    >= Self::MAX_SURFACE_TREE_TRANSACTIONS_PER_ROOT;
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
        self.pending_surface_tree_transactions.push(transaction);
        self.rebuild_scene_work_index();
        if submission_kind == SurfaceTreeSubmissionKind::ClientAdmission {
            debug_assert!(
                self.pending_surface_tree_transactions
                    .iter()
                    .filter(|transaction| transaction.root_surface_id == root_surface_id)
                    .count()
                    <= Self::MAX_SURFACE_TREE_TRANSACTIONS_PER_ROOT
            );
        }
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

    fn queue_waiting_surface_tree_with_lifetimes(
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

    pub(in crate::compositor) fn publish_surface_tree_nodes(
        &mut self,
        transaction: PendingSurfaceTreeTransaction,
    ) {
        if let Some((surface_id, decision)) =
            self.surface_tree_async_publication_rejection(&transaction)
        {
            let (commit_sequence, buffer_id) = transaction
                .nodes
                .iter()
                .find(|(node_surface_id, _)| *node_surface_id == surface_id)
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
            return;
        }
        let PendingSurfaceTreeTransaction {
            root_surface_id,
            nodes,
            ..
        } = transaction;
        let stale_node = nodes.iter().find_map(|(surface_id, commit)| {
            if commit.attachment.is_none() {
                return None;
            }
            let decision = self.surface_publication_decision(
                *surface_id,
                commit.commit_sequence,
                SurfacePublicationContext::OrderedExplicitSyncQueue,
            );
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
        if !nodes
            .iter()
            .any(|(surface_id, _)| *surface_id == root_surface_id)
        {
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        }
        self.publish_surface_tree(root_surface_id, nodes);
    }

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
        let mut retained = Vec::new();
        let mut pacing_deadline_changed = false;
        let mut released = ReleasedSurfaceTreeState {
            callbacks: Vec::new(),
            resize_commit: None,
        };
        for transaction in std::mem::take(&mut self.pending_surface_tree_transactions) {
            if transaction.root_surface_id == root_surface_id {
                pacing_deadline_changed |= transaction.commit_timing_readiness.is_some();
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
        let mut retained = Vec::new();
        let mut pacing_deadline_changed = false;
        let mut callbacks = Vec::new();
        for transaction in std::mem::take(&mut self.pending_surface_tree_transactions) {
            if transaction
                .nodes
                .iter()
                .any(|(node_surface_id, _)| *node_surface_id == surface_id)
            {
                pacing_deadline_changed |= transaction.commit_timing_readiness.is_some();
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
        if pacing_deadline_changed {
            self.invalidate_surface_pacing_deadline_cache();
        }
        self.rebuild_scene_work_index();
        self.complete_frame_callbacks(callbacks);
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

    pub(in crate::compositor) fn apply_captured_subsurface_parent_state(
        &mut self,
        parent_id: u32,
        captured: CapturedSubsurfaceParentState,
    ) -> bool {
        let CapturedSubsurfaceParentState {
            activations,
            mut positions,
            stack,
        } = captured;
        let mut changed = false;
        for relationship in activations {
            if relationship.parent_id != parent_id
                || !self
                    .surface_resources
                    .contains_key(&relationship.surface_id)
                || !self.surface_resources.contains_key(&parent_id)
                || !self
                    .subsurface_transactions
                    .apply_captured_relationship(relationship)
            {
                continue;
            }
            let position = positions
                .iter()
                .position(|position| position.relationship == relationship)
                .map(|index| positions.remove(index))
                .map(|position| (position.x, position.y))
                .unwrap_or((0, 0));
            let placement = SurfacePlacement::subsurface(parent_id, position.0, position.1);
            changed |= self.surface_placement(relationship.surface_id) != placement;
            self.set_surface_placement(relationship.surface_id, placement);
        }
        for position in positions {
            if position.relationship.parent_id != parent_id
                || !self
                    .subsurface_transactions
                    .relationship_is_applied(position.relationship)
            {
                continue;
            }
            let placement = SurfacePlacement::subsurface(parent_id, position.x, position.y);
            changed |= self.surface_placement(position.relationship.surface_id) != placement;
            self.set_surface_placement(position.relationship.surface_id, placement);
        }
        if let Some(stack) = stack {
            changed |= self.apply_captured_subsurface_stack_for_parent(parent_id, stack);
        }
        self.reconcile_applied_subsurface_mapping(parent_id);
        if changed {
            self.advance_render_generation_with_scene_effect(
                RenderGenerationCause::SurfaceCommit,
                self.surface_is_visible_in_active_scene(parent_id),
            );
        }
        changed
    }

    pub(in crate::compositor) fn publish_surface_tree(
        &mut self,
        root_id: u32,
        commits: Vec<(u32, CachedSubsurfaceCommit)>,
    ) {
        let changed_nodes = commits.len();
        let maximum_wait_ms = commits
            .iter()
            .map(|(_, commit)| commit)
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
        for (surface_id, commit) in commits {
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

    pub(in crate::compositor) fn pending_stack_for_parent(
        &mut self,
        parent_id: u32,
    ) -> &mut Vec<u32> {
        self.pending_subsurface_stacks
            .entry(parent_id)
            .or_insert_with(|| {
                self.latched_subsurface_stacks
                    .get(&parent_id)
                    .cloned()
                    .or_else(|| self.committed_subsurface_stacks.get(&parent_id).cloned())
                    .unwrap_or_else(|| vec![parent_id])
            })
    }

    pub(in crate::compositor) fn add_subsurface_to_pending_stack(
        &mut self,
        parent_id: u32,
        surface_id: u32,
    ) {
        let stack = self.pending_stack_for_parent(parent_id);
        stack.retain(|id| *id == parent_id || *id != surface_id);
        if !stack.contains(&parent_id) {
            stack.insert(0, parent_id);
        }
        stack.push(surface_id);
    }

    pub(in crate::compositor) fn restack_subsurface(
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
                .subsurface_transactions
                .relationship_is_registered_child_of(reference_id, parent_id);
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

    fn apply_captured_subsurface_stack_for_parent(
        &mut self,
        parent_id: u32,
        stack: Vec<CapturedSubsurfaceStackEntry>,
    ) -> bool {
        let mut applied_stack = stack
            .into_iter()
            .filter_map(|entry| match entry {
                CapturedSubsurfaceStackEntry::Parent => Some(parent_id),
                CapturedSubsurfaceStackEntry::Child(relationship)
                    if relationship.parent_id == parent_id
                        && self
                            .subsurface_transactions
                            .relationship_is_applied(relationship) =>
                {
                    Some(relationship.surface_id)
                }
                CapturedSubsurfaceStackEntry::Child(_) => None,
            })
            .collect::<Vec<_>>();
        if !applied_stack.contains(&parent_id) {
            applied_stack.insert(0, parent_id);
        }
        applied_stack.dedup();
        let changed = self
            .committed_subsurface_stacks
            .get(&parent_id)
            .is_none_or(|current| *current != applied_stack);
        self.committed_subsurface_stacks
            .insert(parent_id, applied_stack);
        if changed {
            self.reorder_renderable_surfaces_by_committed_stack();
            self.refresh_pointer_focus_at_last_position();
        }
        changed
    }

    pub(in crate::compositor) fn cleanup_subsurface_stack_state_for_surface(
        &mut self,
        surface_id: u32,
    ) {
        self.committed_subsurface_stacks.remove(&surface_id);
        self.latched_subsurface_stacks.remove(&surface_id);
        self.pending_subsurface_stacks.remove(&surface_id);
        for stack in self.committed_subsurface_stacks.values_mut() {
            stack.retain(|id| *id != surface_id);
            stack.dedup();
        }
        for stack in self.pending_subsurface_stacks.values_mut() {
            stack.retain(|id| *id != surface_id);
            stack.dedup();
        }
        for stack in self.latched_subsurface_stacks.values_mut() {
            stack.retain(|id| *id != surface_id);
            stack.dedup();
        }
        self.committed_subsurface_stacks.retain(|parent_id, stack| {
            self.surface_resources.contains_key(parent_id) && stack.iter().any(|id| id != parent_id)
        });
        self.pending_subsurface_stacks.retain(|parent_id, stack| {
            self.surface_resources.contains_key(parent_id) && stack.iter().any(|id| id != parent_id)
        });
        self.latched_subsurface_stacks.retain(|parent_id, stack| {
            self.surface_resources.contains_key(parent_id) && stack.iter().any(|id| id != parent_id)
        });
        self.reorder_renderable_surfaces_by_committed_stack();
    }

    fn detach_subsurface_from_parent_stack_lineage(&mut self, parent_id: u32, surface_id: u32) {
        fn remove_from_stack(stacks: &mut HashMap<u32, Vec<u32>>, parent_id: u32, surface_id: u32) {
            let Some(stack) = stacks.get_mut(&parent_id) else {
                return;
            };
            stack.retain(|id| *id != surface_id);
            stack.dedup();
            if stack.len() <= 1 && stack.first().copied() == Some(parent_id) {
                stacks.remove(&parent_id);
            }
        }

        remove_from_stack(&mut self.committed_subsurface_stacks, parent_id, surface_id);
        remove_from_stack(&mut self.latched_subsurface_stacks, parent_id, surface_id);
        remove_from_stack(&mut self.pending_subsurface_stacks, parent_id, surface_id);
    }

    pub(in crate::compositor) fn destroy_subsurface_role(&mut self, surface_id: u32) {
        let affected_surfaces = self.subsurface_transactions.subsurface_tree_ids(surface_id);
        let was_effectively_synchronized = affected_surfaces
            .iter()
            .map(|surface_id| {
                (
                    *surface_id,
                    self.is_effectively_synchronized_subsurface(*surface_id),
                )
            })
            .collect::<HashMap<_, _>>();
        let Some(detached) = self.subsurface_transactions.detach_role(surface_id) else {
            return;
        };
        let parent_id = detached.relationship.parent_id;
        let client_id = detached.client_id.clone();
        self.update_synchronized_cache_metrics();
        self.hide_renderable_surface_subtree(surface_id);
        self.deactivate_role_instance(surface_id);
        self.set_surface_placement(surface_id, SurfacePlacement::root());
        self.detach_subsurface_from_parent_stack_lineage(parent_id, surface_id);
        self.reorder_renderable_surfaces_by_committed_stack();
        self.reclassify_unreachable_synchronized_commits(
            &affected_surfaces,
            &was_effectively_synchronized,
            Some((surface_id, detached.cached_commits)),
        );
        self.debug_assert_surface_tree_invariants();
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: subsurface_tx surface={surface_id} parent={parent_id:?} client={client_id:?} decision=destroyed reason=role_destroyed"
            );
        }
    }

    pub(in crate::compositor) fn release_cached_subsurface_commits(
        &mut self,
        commits: Vec<CachedSubsurfaceCommit>,
    ) {
        for commit in commits {
            for feedback in commit.presentation_feedbacks {
                feedback.feedback.discarded();
            }
            if let Some(PendingSurfaceAttachment::Buffer(buffer)) = commit.attachment {
                self.release_pending_surface_buffer(buffer);
            }
        }
    }

    pub(in crate::compositor) fn debug_assert_surface_tree_invariants(&self) {
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
                Self::debug_assert_subsurface_stack_invariant(*parent_id, stack);
            }
            for (parent_id, stack) in &self.latched_subsurface_stacks {
                Self::debug_assert_subsurface_stack_invariant(*parent_id, stack);
            }
            for (parent_id, stack) in &self.pending_subsurface_stacks {
                Self::debug_assert_subsurface_stack_invariant(*parent_id, stack);
            }
        }
    }

    #[cfg(debug_assertions)]
    fn debug_assert_subsurface_stack_invariant(parent_id: u32, stack: &[u32]) {
        let mut stack_ids = HashSet::new();
        debug_assert!(stack.iter().all(|surface_id| stack_ids.insert(*surface_id)));
        debug_assert_eq!(stack.iter().filter(|id| **id == parent_id).count(), 1);
    }

    pub(in crate::compositor) fn take_and_bind_surface_presentation_feedbacks(
        &mut self,
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
    ) -> Vec<PendingPresentationFeedback> {
        let Some(surface_generation) = self
            .surface_presentation_generations
            .get(&surface_id)
            .copied()
        else {
            for feedback in self
                .pending_surface_presentation_feedbacks
                .remove(&surface_id)
                .unwrap_or_default()
            {
                feedback.feedback.discarded();
            }
            return Vec::new();
        };
        self.pending_surface_presentation_feedbacks
            .remove(&surface_id)
            .unwrap_or_default()
            .into_iter()
            .map(|feedback| PendingPresentationFeedback {
                surface_id,
                surface_presentation_generation: surface_generation,
                commit_sequence,
                surface: feedback.surface,
                feedback: feedback.feedback,
            })
            .collect()
    }

    pub(in crate::compositor) fn set_surface_placement(
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

    pub(in crate::compositor) fn set_surface_placement_with_cause(
        &mut self,
        surface_id: u32,
        placement: SurfacePlacement,
        cause: RenderGenerationCause,
    ) -> bool {
        if self.surface_placement(surface_id) == placement {
            return false;
        }

        self.store_surface_placement(surface_id, placement);
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        if let Some(visual) = self.toplevel_visual_geometries.get_mut(&surface_id) {
            visual.placement = placement;
        }

        if let Some(surface) = self
            .renderable_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == surface_id)
        {
            surface.placement = placement;
            let refreshed_by_visual_assignment = self
                .toplevel_visual_geometries
                .contains_key(&root_surface_id)
                || self.toplevel_surfaces.contains_key(&root_surface_id);
            if refreshed_by_visual_assignment {
                self.update_toplevel_visual_render_assignment(root_surface_id);
            } else {
                self.refresh_active_scene_surface_tree(root_surface_id);
            }
            if refreshed_by_visual_assignment {
                self.compliance_metrics.prevented_duplicate_root_refreshes = self
                    .compliance_metrics
                    .prevented_duplicate_root_refreshes
                    .saturating_add(1);
            }
            self.advance_render_generation_with_scene_effect(
                cause,
                self.surface_is_visible_in_active_scene(root_surface_id),
            );
            return true;
        }

        false
    }

    pub(in crate::compositor) fn refresh_surface_origin_cache(&mut self) {
        if self.surface_origin_cache_generation != Some(self.render_generation)
            || self.surface_origin_cache.len() != self.renderable_surfaces.len()
        {
            self.surface_origin_cache = render::surface_origins(&self.renderable_surfaces);
            self.surface_origin_cache_generation = Some(self.render_generation);
            self.pointer_hit_metrics.global_origin_cache_recomputes = self
                .pointer_hit_metrics
                .global_origin_cache_recomputes
                .saturating_add(1);
        }
    }

    pub(in crate::compositor) fn invalidate_surface_origin_cache(&mut self) {
        self.surface_origin_cache_generation = None;
        self.visual_stack_groups_cache_generation = None;
        self.advance_pointer_hit_generation();
    }

    pub(in crate::compositor) fn raise_renderable_surface_tree(&mut self, surface_id: u32) -> bool {
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
        self.rebuild_renderable_surface_index();
        if changed {
            self.invalidate_surface_origin_cache();
            self.refresh_active_scene_surface_order();
        }
        changed
    }
}
