#![allow(clippy::question_mark)]

use super::*;
use crate::compositor::subsurface::{
    CapturedContentUpdateLineage, CapturedSubsurfaceParentState, CapturedSubsurfaceStackEntry,
    CapturedSurfaceCommitContext, ContentUpdateRef,
};

#[derive(Debug)]
pub(super) struct PreparedContentUpdateCandidate {
    pub(in crate::compositor) root_surface_id: u32,
    pub(in crate::compositor) nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    pub(in crate::compositor) external_content_update_dependencies: Vec<ContentUpdateRef>,
}

#[derive(Debug)]
struct XdgTopologyMutationSnapshot {
    owner_root_surface_id: Option<u32>,
    scene_root_surface_id: u32,
    before_geometry: Option<EffectiveXdgWindowGeometry>,
    affected_surface_ids: Vec<u32>,
    affects_active_scene: bool,
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
            .surface_transactions
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
    fn content_update_ref_is_published(&self, reference: ContentUpdateRef) -> bool {
        self.surface_publications
            .get(&reference.surface_id)
            .and_then(|publication| publication.latest_published)
            .is_some_and(|published| published >= reference.commit_sequence)
    }

    pub(super) fn content_update_ref_is_pending(&self, reference: ContentUpdateRef) -> bool {
        self.surface_transactions.contains_content_update(reference)
    }

    pub(super) fn content_update_ref_is_terminal(&self, reference: ContentUpdateRef) -> bool {
        if !self.surface_resources.contains_key(&reference.surface_id)
            || self
                .surface_client_ids
                .get(&reference.surface_id)
                .is_some_and(|client_id| self.terminal_client_ids.contains(client_id))
        {
            return true;
        }
        self.surface_publications
            .get(&reference.surface_id)
            .and_then(|publication| publication.latest_terminal)
            .is_some_and(|terminal| {
                debug_assert!(reference.commit_sequence <= terminal);
                reference.commit_sequence <= terminal
            })
    }

    pub(in crate::compositor) fn content_update_dependencies_ready(
        &self,
        transaction: &PendingSurfaceTreeTransaction,
    ) -> bool {
        transaction
            .external_content_update_dependencies
            .iter()
            .all(|dependency| {
                transaction_covers_content_update_ref(transaction, *dependency)
                    || self.content_update_ref_is_published(*dependency)
                    || (!self.content_update_ref_is_pending(*dependency)
                        && self.content_update_ref_is_terminal(*dependency))
            })
    }

    pub(super) fn extract_content_update_candidate(
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
            .surface_transactions
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
        if !self
            .surface_transactions
            .register_with_client(surface_id, parent_id, Some(client_id))
        {
            return false;
        }
        self.add_subsurface_to_pending_stack(parent_id, surface_id);
        true
    }

    pub(in crate::compositor) fn is_effectively_synchronized_subsurface(
        &self,
        surface_id: u32,
    ) -> bool {
        self.surface_transactions
            .is_effectively_synchronized(surface_id)
    }

    pub(in crate::compositor) fn subsurface_content_is_inactive(&self, surface_id: u32) -> bool {
        match self.surface_role(surface_id) {
            SurfaceRole::Subsurface { parent_id } => {
                !self
                    .surface_transactions
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
        self.surface_transactions
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
        for child_id in self.surface_transactions.applied_children_of(parent_id) {
            changed |= self.adopt_current_surface_content_for_role(child_id);
        }
        changed
    }

    pub(in crate::compositor) fn set_subsurface_sync_mode(
        &mut self,
        surface_id: u32,
        mode: SubsurfaceSyncMode,
    ) {
        if self.surface_transactions.requested_mode(surface_id) == Some(mode) {
            return;
        }
        let affected_surfaces = self.surface_transactions.subsurface_tree_ids(surface_id);
        let was_effectively_synchronized = affected_surfaces
            .iter()
            .map(|surface_id| {
                (
                    *surface_id,
                    self.is_effectively_synchronized_subsurface(*surface_id),
                )
            })
            .collect::<HashMap<_, _>>();
        if !self.surface_transactions.set_mode(surface_id, mode) {
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
                .surface_transactions
                .oldest_cached_content_update_ref(*surface_id)
            {
                let Some(mut commits) = self
                    .surface_transactions
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
        if crate::compositor::state::roles::surface_tree_debug_enabled()
            && let Some(relationship) = self.surface_transactions.captured_relationship(surface_id)
        {
            eprintln!(
                "event=subsurface_position_requested child={} parent={} relationship={} x={} y={}",
                relationship.surface_id,
                relationship.parent_id,
                relationship.relationship_id.get(),
                x,
                y,
            );
        }
        self.surface_transactions
            .set_pending_position(surface_id, x, y);
    }

    fn capture_surface_commit_context(
        &mut self,
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
    ) -> Result<CapturedSurfaceCommitContext, ()> {
        let layer_surface = self.capture_layer_surface_commit_state(surface_id)?;
        let xdg_decoration = self.capture_xdg_decoration_commit_state(surface_id, commit_sequence);
        let activations = self
            .surface_transactions
            .take_pending_relationship_activations_for_parent(surface_id);
        let positions = self
            .surface_transactions
            .take_pending_positions_for_parent(surface_id);
        let live_stack = self
            .surface_topology
            .take_pending_stack_for_capture(surface_id);
        let stack = live_stack.map(|stack| {
            self.surface_transactions
                .capture_subsurface_stack(surface_id, stack)
        });
        Ok(CapturedSurfaceCommitContext {
            subsurface_parent: CapturedSubsurfaceParentState {
                activations,
                positions,
                stack,
            },
            layer_surface,
            xdg_decoration,
        })
    }

    pub(in crate::compositor) fn commit_surface_tree_request(
        &mut self,
        surface_id: u32,
        mut commit: CachedSubsurfaceCommit,
    ) {
        let commit_context =
            match self.capture_surface_commit_context(surface_id, commit.commit_sequence) {
                Ok(context) => context,
                Err(()) => {
                    self.release_unpublished_surface_tree_nodes(vec![(surface_id, commit)]);
                    return;
                }
            };
        if crate::compositor::state::roles::surface_tree_debug_enabled()
            && (commit.window_geometry.is_some()
                || !commit_context.subsurface_parent.positions.is_empty())
        {
            let geometry = commit.window_geometry.map_or_else(
                || "none".to_string(),
                |geometry| {
                    format!(
                        "{},{},{},{}",
                        geometry.x, geometry.y, geometry.width, geometry.height
                    )
                },
            );
            let positions = commit_context
                .subsurface_parent
                .positions
                .iter()
                .map(|position| {
                    format!(
                        "child={} relationship={} x={} y={}",
                        position.relationship.surface_id,
                        position.relationship.relationship_id.get(),
                        position.x,
                        position.y,
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            eprintln!(
                "event=surface_parent_state_captured parent={} commit_sequence={} xdg_geometry={} positions=[{}]",
                surface_id,
                commit.commit_sequence.get(),
                geometry,
                positions,
            );
        }
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
        let synchronized = self.is_effectively_synchronized_subsurface(surface_id);
        let direct_mapping = if synchronized {
            None
        } else {
            match self.derive_surface_mapping_for_commit(surface_id, &commit) {
                Some(Ok(mapping)) => mapping,
                Some(Err((error, viewport_error_owner))) => {
                    self.post_surface_mapping_error(surface_id, error, viewport_error_owner);
                    self.release_unpublished_surface_tree_nodes(vec![(surface_id, commit)]);
                    return;
                }
                None => None,
            }
        };
        if synchronized {
            self.cache_synchronized_subsurface_commit(surface_id, commit);
            return;
        }
        match commit.attachment.as_mut() {
            Some(PendingSurfaceAttachment::Buffer(pending)) => {
                if let Some(mapping) = direct_mapping {
                    pending.apply_content_mapping(mapping);
                }
                self.finalize_pending_buffer_resize_capture(
                    surface_id,
                    pending,
                    commit.window_geometry,
                );
            }
            _ => {
                commit.resize_commit = self.capture_acked_resize_for_surface_commit(surface_id);
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

    pub(super) fn submit_surface_tree_nodes_with_kind(
        &mut self,
        surface_id: u32,
        mut nodes: Vec<(u32, CachedSubsurfaceCommit)>,
        external_content_update_dependencies: Vec<ContentUpdateRef>,
        submission_kind: SurfaceTreeSubmissionKind,
    ) {
        if let Err((error_surface_id, error, viewport_error_owner)) = self
            .validate_surface_tree_surface_state(
                surface_id,
                &nodes,
                &external_content_update_dependencies,
            )
        {
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: surface_commit validation failed surface={error_surface_id}"
                );
            }
            self.post_surface_mapping_error(error_surface_id, error, viewport_error_owner);
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        }
        if nodes
            .iter()
            .all(|(_, commit)| !commit.pacing.is_boundary() && !commit.lineage.merge_frozen)
        {
            nodes = self.canonicalize_coalescible_surface_tree_nodes(nodes);
        }
        if let Err((error_surface_id, error, viewport_error_owner)) = self
            .prepare_surface_tree_surface_state(
                surface_id,
                &mut nodes,
                &external_content_update_dependencies,
            )
        {
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: surface_commit validation failed surface={error_surface_id}"
                );
            }
            self.post_surface_mapping_error(error_surface_id, error, viewport_error_owner);
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        }
        let mut external_content_update_dependencies = external_content_update_dependencies;
        normalize_external_content_dependencies_for_nodes(
            &nodes,
            &mut external_content_update_dependencies,
        );
        let Some(dependencies) = self.prepare_surface_tree_acquires(&mut nodes) else {
            self.release_unpublished_surface_tree_nodes(nodes);
            return;
        };
        debug_assert!(
            nodes
                .iter()
                .all(|(_, commit)| commit.explicit_sync.is_none()),
            "materialized SurfaceTree transactions cannot retain captured explicit-sync state"
        );
        self.surface_transactions.metrics.tree_transactions_prepared = self
            .surface_transactions
            .metrics
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

    fn canonicalize_coalescible_surface_tree_nodes(
        &mut self,
        nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    ) -> Vec<(u32, CachedSubsurfaceCommit)> {
        debug_assert!(
            nodes.iter().all(|(_, commit)| {
                !commit.pacing.is_boundary() && !commit.lineage.merge_frozen
            })
        );
        let mut canonical = Vec::with_capacity(nodes.len());
        for (surface_id, newer) in nodes {
            let Some(existing_index) = canonical
                .iter()
                .position(|(existing_surface_id, _)| *existing_surface_id == surface_id)
            else {
                canonical.push((surface_id, newer));
                continue;
            };
            let existing = &mut canonical[existing_index].1;
            debug_assert!(existing.commit_sequence < newer.commit_sequence);
            if let Some(predecessor) = newer.lineage.predecessor {
                debug_assert_eq!(predecessor.surface_id, surface_id);
                debug_assert!(content_update_node_covers_ref(
                    surface_id,
                    existing,
                    predecessor
                ));
            }
            let attachment_changed = newer.attachment.is_some();
            let old_resize_commit = attachment_changed
                .then(|| pending_node_resize_commit(existing))
                .flatten();
            let previous_commit_id = existing.commit_id;
            let previous_callback_count = existing.frame_callbacks.len();
            let replacement_commit_id = newer.commit_id;
            if let Some(release) = existing.merge(newer) {
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
        }
        canonical
    }

    pub(super) fn capture_surface_tree_node_lifetimes(
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

    pub(in crate::compositor) fn apply_captured_subsurface_parent_state(
        &mut self,
        parent_id: u32,
        commit_sequence: SurfaceCommitSequence,
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
                    .surface_transactions
                    .apply_captured_relationship(relationship)
            {
                continue;
            }
            let captured_position = positions
                .iter()
                .position(|position| position.relationship == relationship)
                .map(|index| positions.remove(index))
                .map(|position| (position.x, position.y));
            let position = captured_position.unwrap_or((0, 0));
            let placement = SurfacePlacement::subsurface(parent_id, position.0, position.1);
            changed |= self.surface_placement(relationship.surface_id) != placement;
            self.set_surface_placement(relationship.surface_id, placement);
            if let Some((x, y)) = captured_position
                && crate::compositor::state::roles::surface_tree_debug_enabled()
            {
                eprintln!(
                    "event=subsurface_position_applied parent={} child={} relationship={} commit_sequence={} x={} y={}",
                    parent_id,
                    relationship.surface_id,
                    relationship.relationship_id.get(),
                    commit_sequence.get(),
                    x,
                    y,
                );
            }
        }
        for position in positions {
            if position.relationship.parent_id != parent_id
                || !self
                    .surface_transactions
                    .relationship_is_applied(position.relationship)
            {
                continue;
            }
            let placement = SurfacePlacement::subsurface(parent_id, position.x, position.y);
            changed |= self.surface_placement(position.relationship.surface_id) != placement;
            self.set_surface_placement(position.relationship.surface_id, placement);
            if crate::compositor::state::roles::surface_tree_debug_enabled() {
                eprintln!(
                    "event=subsurface_position_applied parent={} child={} relationship={} commit_sequence={} x={} y={}",
                    parent_id,
                    position.relationship.surface_id,
                    position.relationship.relationship_id.get(),
                    commit_sequence.get(),
                    position.x,
                    position.y,
                );
            }
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

    pub(in crate::compositor) fn add_subsurface_to_pending_stack(
        &mut self,
        parent_id: u32,
        surface_id: u32,
    ) {
        self.surface_topology
            .add_child_to_pending_stack(parent_id, surface_id);
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
                .surface_transactions
                .relationship_is_registered_child_of(reference_id, parent_id);
        if !valid_reference {
            return false;
        }
        self.surface_topology
            .restack_pending_child(surface_id, parent_id, reference_id, above)
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
                            .surface_transactions
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
            .surface_topology
            .commit_subsurface_stack(parent_id, applied_stack);
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
        let surface_resources = &self.surface_resources;
        self.surface_topology
            .cleanup_surface_stacks(surface_id, |parent_id| {
                surface_resources.contains_key(&parent_id)
            });
        self.reorder_renderable_surfaces_by_committed_stack();
    }

    fn detach_subsurface_from_parent_stack_lineage(&mut self, parent_id: u32, surface_id: u32) {
        self.surface_topology
            .detach_child_from_stack_lineage(parent_id, surface_id);
    }

    pub(in crate::compositor) fn destroy_subsurface_role(&mut self, surface_id: u32) {
        let affected_surface_ids = self.surface_transactions.subsurface_tree_ids(surface_id);
        let (owner_root_surface_id, before_geometry) = self
            .capture_xdg_geometry_for_topology_mutation(surface_id)
            .map_or((None, None), |(root_surface_id, before)| {
                (Some(root_surface_id), before)
            });
        let snapshot = XdgTopologyMutationSnapshot {
            owner_root_surface_id,
            scene_root_surface_id: self.root_surface_id_for_surface(surface_id),
            before_geometry,
            affects_active_scene: affected_surface_ids
                .iter()
                .any(|affected_id| self.surface_is_visible_in_active_scene(*affected_id)),
            affected_surface_ids,
        };
        let was_effectively_synchronized = snapshot
            .affected_surface_ids
            .iter()
            .map(|surface_id| {
                (
                    *surface_id,
                    self.is_effectively_synchronized_subsurface(*surface_id),
                )
            })
            .collect::<HashMap<_, _>>();
        let Some(detached) = self.surface_transactions.detach_role(surface_id) else {
            return;
        };
        let parent_id = detached.relationship.parent_id;
        let client_id = detached.client_id.clone();
        let promoted_refs = detached
            .cached_commits
            .iter()
            .map(|commit| commit.content_update_ref(surface_id))
            .collect::<Vec<_>>();
        if was_effectively_synchronized
            .get(&surface_id)
            .copied()
            .unwrap_or(false)
        {
            self.surface_transactions
                .remove_cached_parent_dependencies_to_child_commits(
                    parent_id,
                    surface_id,
                    &promoted_refs,
                );
        }
        self.update_synchronized_cache_metrics();
        self.cleanup_hidden_surface_ids(&snapshot.affected_surface_ids);
        if let Some(owner_root_surface_id) = snapshot.owner_root_surface_id {
            let affected_ids = snapshot
                .affected_surface_ids
                .iter()
                .copied()
                .collect::<HashSet<_>>();
            if let Some(window_id) = self.window_id_for_surface(owner_root_surface_id)
                && let Some(window) = self.window_mut(window_id)
            {
                window.state.remove_minimized_surface_ids(&affected_ids);
            }
        }
        self.deactivate_role_instance(surface_id);
        self.set_surface_placement(surface_id, SurfacePlacement::root());
        self.detach_subsurface_from_parent_stack_lineage(parent_id, surface_id);
        self.reorder_renderable_surfaces_by_committed_stack();
        self.reclassify_unreachable_synchronized_commits(
            &snapshot.affected_surface_ids,
            &was_effectively_synchronized,
            Some((surface_id, detached.cached_commits)),
        );

        let geometry_changed = snapshot
            .owner_root_surface_id
            .is_some_and(|root_surface_id| {
                self.publish_xdg_geometry_after_topology_mutation(
                    root_surface_id,
                    snapshot.before_geometry,
                )
            });
        if !geometry_changed {
            if snapshot.affects_active_scene {
                self.refresh_active_scene_surface_tree(snapshot.scene_root_surface_id);
            } else {
                self.refresh_active_scene_surface_order();
            }
            self.reconcile_surface_tree_output_memberships(snapshot.scene_root_surface_id);
            self.refresh_pointer_focus_at_last_position();
        }
        self.reconcile_hidden_surface_output_memberships(&snapshot.affected_surface_ids);
        self.advance_render_generation_with_scene_effect(
            RenderGenerationCause::SurfaceUnmap,
            snapshot.affects_active_scene,
        );

        if compositor_debug_surface_logging_enabled() {
            let after_geometry = snapshot
                .owner_root_surface_id
                .and_then(|root_surface_id| self.effective_xdg_window_geometry(root_surface_id));
            eprintln!(
                "oblivion-one compositor: event=surface_topology_mutation kind=subsurface_destroy root={:?} removed_ids={:?} geometry_before={:?} geometry_after={:?}",
                snapshot.owner_root_surface_id,
                snapshot.affected_surface_ids,
                snapshot.before_geometry.map(|geometry| geometry.geometry),
                after_geometry.map(|geometry| geometry.geometry),
            );
        }
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
            self.surface_topology.debug_assert_stack_invariants();
        }
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
