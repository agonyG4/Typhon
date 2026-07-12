use super::*;

impl CompositorState {
    pub(in crate::compositor) fn release_cached_resources_for_shutdown(&mut self) {
        let mut cached = self.subsurface_transactions.drain_cached_commits();
        for transaction in self.pending_surface_tree_transactions.drain(..) {
            cached.extend(transaction.nodes.into_iter().map(|(_, commit)| commit));
        }
        for commit in cached {
            if commit.explicit_sync.is_some() {
                self.note_explicit_commit_destroyed(commit.commit_id, "compositor_shutdown");
            }
            for feedback in commit.presentation_feedbacks {
                feedback.feedback.discarded();
            }
            if let Some(PendingSurfaceAttachment::Buffer(buffer)) = commit.attachment {
                buffer.release_target().release();
            }
        }
        for commit in std::mem::take(&mut self.pending_explicit_sync_commits) {
            self.note_explicit_commit_destroyed(commit.surface_commit_id, "compositor_shutdown");
            commit.pending.release_target().release();
        }
    }
}

pub(in crate::compositor) fn empty_cached_subsurface_commit() -> CachedSubsurfaceCommit {
    CachedSubsurfaceCommit {
        commit_id: SurfaceCommitId::for_tests(1),
        commit_sequence: SurfaceCommitSequence::initial(),
        attachment: None,
        damage: None,
        frame_callbacks: Vec::new(),
        explicit_sync: None,
        offset: None,
        viewport_destination: PendingViewportChange::default(),
        buffer_scale: None,
        input_region: None,
        presentation_feedbacks: Vec::new(),
        resize_commit: None,
        resize_capture_finalized: true,
        window_geometry: None,
        cached_at: Instant::now(),
    }
}

pub(in crate::compositor) fn take_tree_resize_commit(
    root_surface_id: u32,
    nodes: &mut [(u32, CachedSubsurfaceCommit)],
) -> Option<ResizeCommitSnapshot> {
    let (_, root) = nodes
        .iter_mut()
        .find(|(surface_id, _)| *surface_id == root_surface_id)?;
    match root.attachment.as_mut() {
        Some(PendingSurfaceAttachment::Buffer(buffer)) => {
            buffer.resize_commit.take().map(|resize| *resize)
        }
        _ => root.resize_commit.take(),
    }
}

pub(in crate::compositor) fn pending_node_resize_commit(
    commit: &CachedSubsurfaceCommit,
) -> Option<ResizeCommitSnapshot> {
    match commit.attachment.as_ref() {
        Some(PendingSurfaceAttachment::Buffer(buffer)) => buffer.resize_commit.as_deref().copied(),
        _ => commit.resize_commit,
    }
}

pub(in crate::compositor) fn pending_attachment_buffer_protocol_id(
    attachment: &PendingSurfaceAttachment,
) -> Option<u32> {
    match attachment {
        PendingSurfaceAttachment::Buffer(buffer) => Some(buffer.resource.id().protocol_id()),
        PendingSurfaceAttachment::RemoveContent => None,
    }
}

pub(in crate::compositor) fn remove_surface_tree_dependency(
    transaction: &mut PendingSurfaceTreeTransaction,
    surface_id: u32,
    buffer_id: u32,
) -> Option<SurfaceTreeAcquireDependency> {
    let index = transaction.dependencies.iter().position(|dependency| {
        dependency.surface_id == surface_id && dependency.buffer_id == buffer_id
    })?;
    Some(transaction.dependencies.remove(index))
}

pub(in crate::compositor) fn newest_ready_explicit_sync_commit_indices(
    commits: impl IntoIterator<Item = (usize, u32, bool)>,
) -> HashMap<u32, usize> {
    let mut newest_ready = HashMap::new();
    for (index, surface_id, ready) in commits {
        if ready {
            newest_ready.insert(surface_id, index);
        }
    }
    newest_ready
}

pub(in crate::compositor) fn damage_only_rendered_surface_size(
    existing: BufferSize,
    requested: BufferSize,
    resize_pending: bool,
) -> BufferSize {
    if resize_pending { existing } else { requested }
}

pub(in crate::compositor) fn resource_belongs_to_surface_client<R>(
    resource: &R,
    surface: &wl_surface::WlSurface,
) -> bool
where
    R: Resource,
{
    resource.id().same_client_as(&surface.id())
}

pub(in crate::compositor) fn same_wayland_resource<L, R>(left: &L, right: &R) -> bool
where
    L: Resource,
    R: Resource,
{
    left.id().protocol_id() == right.id().protocol_id() && left.id().same_client_as(&right.id())
}

#[cfg(test)]
mod explicit_sync_commit_accounting_tests {
    use super::*;

    #[test]
    fn ready_explicit_sync_commit_is_superseded_before_publication() {
        let newest = newest_ready_explicit_sync_commit_indices([(0, 7, true), (1, 7, true)]);
        assert_eq!(newest.get(&7), Some(&1));

        let mut metrics = ExplicitSyncCommitMetrics::default();
        let disposition = metrics.note_superseded(PendingAcquireState::Ready);
        assert_eq!(disposition, SurfaceCommitDisposition::SupersededWhileReady);
        assert_eq!(metrics.ready_commits_superseded, 1);
    }

    #[test]
    fn unready_explicit_sync_commit_can_be_superseded_by_newer_state() {
        let newest = newest_ready_explicit_sync_commit_indices([(0, 7, false), (1, 7, true)]);
        assert_eq!(newest.get(&7), Some(&1));

        let mut metrics = ExplicitSyncCommitMetrics::default();
        let disposition = metrics.note_superseded(PendingAcquireState::RegistrationPending);
        assert_eq!(
            disposition,
            SurfaceCommitDisposition::SupersededWhileUnready
        );
        assert_eq!(metrics.unready_commits_superseded, 1);
        assert_eq!(metrics.ready_commits_superseded, 0);
    }
}
