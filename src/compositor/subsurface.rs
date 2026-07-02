use std::{collections::HashMap, time::Instant};

use wayland_server::protocol::wl_callback;

use super::{
    RenderableSurfaceDamage, SurfaceCommitSequence, SurfaceInputRegion,
    explicit_sync::{CapturedExplicitSyncState, PendingPresentationFeedback},
    state_data::{PendingSurfaceAttachment, PendingViewportChange, SurfaceBufferRelease},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum SubsurfaceSyncMode {
    #[default]
    Synchronized,
    Desynchronized,
}

#[derive(Debug)]
pub(super) struct CachedSubsurfaceCommit {
    pub(super) commit_sequence: SurfaceCommitSequence,
    pub(super) attachment: Option<PendingSurfaceAttachment>,
    pub(super) damage: Option<RenderableSurfaceDamage>,
    pub(super) frame_callbacks: Vec<wl_callback::WlCallback>,
    pub(super) explicit_sync: Option<CapturedExplicitSyncState>,
    pub(super) offset: Option<(i32, i32)>,
    pub(super) viewport_destination: PendingViewportChange,
    pub(super) buffer_scale: Option<u32>,
    pub(super) input_region: Option<SurfaceInputRegion>,
    pub(super) presentation_feedbacks: Vec<PendingPresentationFeedback>,
    pub(super) resize_commit: Option<super::ResizeCommitSnapshot>,
    pub(super) resize_capture_finalized: bool,
    pub(super) window_geometry: Option<super::XdgWindowGeometry>,
    pub(super) cached_at: Instant,
}

impl CachedSubsurfaceCommit {
    pub(super) fn merge(&mut self, newer: Self) -> Option<SurfaceBufferRelease> {
        let Self {
            commit_sequence,
            attachment,
            damage,
            frame_callbacks,
            explicit_sync,
            offset,
            viewport_destination,
            buffer_scale,
            input_region,
            presentation_feedbacks,
            resize_commit,
            resize_capture_finalized,
            window_geometry,
            cached_at: _,
        } = newer;
        self.commit_sequence = commit_sequence;
        let attachment_changed = attachment.is_some();
        let superseded = attachment.and_then(|attachment| {
            self.attachment
                .replace(attachment)
                .and_then(|previous| match previous {
                    PendingSurfaceAttachment::Buffer(buffer) => Some(buffer.release_target()),
                    PendingSurfaceAttachment::RemoveContent => None,
                })
        });
        self.damage = merge_damage(self.damage.take(), damage);
        self.frame_callbacks.extend(frame_callbacks);
        if attachment_changed || explicit_sync.is_some() {
            self.explicit_sync = explicit_sync;
        }
        if offset.is_some() {
            self.offset = offset;
        }
        if viewport_destination.source.is_some() || viewport_destination.destination.is_some() {
            self.viewport_destination = viewport_destination;
        }
        if buffer_scale.is_some() {
            self.buffer_scale = buffer_scale;
        }
        if input_region.is_some() {
            self.input_region = input_region;
        }
        self.presentation_feedbacks.extend(presentation_feedbacks);
        if resize_capture_finalized {
            self.resize_commit = resize_commit;
            self.resize_capture_finalized = true;
        }
        if window_geometry.is_some() {
            self.window_geometry = window_geometry;
        }
        superseded
    }
}

fn merge_damage(
    older: Option<RenderableSurfaceDamage>,
    newer: Option<RenderableSurfaceDamage>,
) -> Option<RenderableSurfaceDamage> {
    match (older, newer) {
        (Some(RenderableSurfaceDamage::Full), _) | (_, Some(RenderableSurfaceDamage::Full)) => {
            Some(RenderableSurfaceDamage::Full)
        }
        (
            Some(RenderableSurfaceDamage::Partial(mut older)),
            Some(RenderableSurfaceDamage::Partial(newer)),
        ) => {
            older.extend(newer);
            Some(RenderableSurfaceDamage::Partial(older))
        }
        (Some(RenderableSurfaceDamage::Empty), Some(damage))
        | (Some(damage), Some(RenderableSurfaceDamage::Empty)) => Some(damage),
        (Some(damage), None) | (None, Some(damage)) => Some(damage),
        (None, None) => None,
    }
}

#[cfg(test)]
mod window_geometry_tests {
    use super::*;
    use crate::compositor::XdgWindowGeometry;

    fn cached_commit_with_window_geometry(
        sequence: u64,
        window_geometry: XdgWindowGeometry,
    ) -> CachedSubsurfaceCommit {
        CachedSubsurfaceCommit {
            commit_sequence: SurfaceCommitSequence(sequence),
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
            window_geometry: Some(window_geometry),
            cached_at: Instant::now(),
        }
    }

    #[test]
    fn cached_window_geometry_uses_latest_committed_value() {
        let mut cached =
            cached_commit_with_window_geometry(1, XdgWindowGeometry::new(1, 2, 300, 200));
        let newer = cached_commit_with_window_geometry(2, XdgWindowGeometry::new(8, 9, 320, 220));

        cached.merge(newer);

        assert_eq!(
            cached.window_geometry,
            Some(XdgWindowGeometry::new(8, 9, 320, 220))
        );
    }
}

#[derive(Debug)]
struct SubsurfaceRoleState {
    parent_id: u32,
    requested_mode: SubsurfaceSyncMode,
    cached_commit: Option<CachedSubsurfaceCommit>,
    pending_position: Option<(i32, i32)>,
}

#[derive(Debug, Default)]
pub(super) struct SubsurfaceTransactionState {
    roles: HashMap<u32, SubsurfaceRoleState>,
}

impl SubsurfaceTransactionState {
    pub(super) fn register(&mut self, surface_id: u32, parent_id: u32) -> bool {
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
        self.roles.insert(
            surface_id,
            SubsurfaceRoleState {
                parent_id,
                requested_mode: SubsurfaceSyncMode::Synchronized,
                cached_commit: None,
                pending_position: None,
            },
        );
        true
    }

    pub(super) fn remove_role(&mut self, surface_id: u32) -> Option<CachedSubsurfaceCommit> {
        self.roles
            .remove(&surface_id)
            .and_then(|role| role.cached_commit)
    }

    pub(super) fn remove_subtree(&mut self, surface_id: u32) -> Vec<CachedSubsurfaceCommit> {
        let mut removed = Vec::new();
        let mut pending = vec![surface_id];
        while let Some(id) = pending.pop() {
            pending.extend(
                self.roles
                    .iter()
                    .filter_map(|(child_id, role)| (role.parent_id == id).then_some(*child_id)),
            );
            if let Some(mut role) = self.roles.remove(&id)
                && let Some(commit) = role.cached_commit.take()
            {
                removed.push(commit);
            }
        }
        removed
    }

    pub(super) fn drain_cached_commits(&mut self) -> Vec<CachedSubsurfaceCommit> {
        self.roles
            .values_mut()
            .filter_map(|role| role.cached_commit.take())
            .collect()
    }

    pub(super) fn parent(&self, surface_id: u32) -> Option<u32> {
        self.roles.get(&surface_id).map(|role| role.parent_id)
    }

    pub(super) fn requested_mode(&self, surface_id: u32) -> Option<SubsurfaceSyncMode> {
        self.roles.get(&surface_id).map(|role| role.requested_mode)
    }

    pub(super) fn set_mode(&mut self, surface_id: u32, mode: SubsurfaceSyncMode) -> bool {
        let Some(role) = self.roles.get_mut(&surface_id) else {
            return false;
        };
        role.requested_mode = mode;
        true
    }

    pub(super) fn is_effectively_synchronized(&self, surface_id: u32) -> bool {
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

    pub(super) fn cache_commit(
        &mut self,
        surface_id: u32,
        commit: CachedSubsurfaceCommit,
    ) -> Option<SurfaceBufferRelease> {
        let role = self.roles.get_mut(&surface_id)?;
        if let Some(cached) = role.cached_commit.as_mut() {
            cached.merge(commit)
        } else {
            role.cached_commit = Some(commit);
            None
        }
    }

    pub(super) fn has_cached_commit(&self, surface_id: u32) -> bool {
        self.roles
            .get(&surface_id)
            .is_some_and(|role| role.cached_commit.is_some())
    }

    pub(super) fn cached_node_count(&self) -> usize {
        self.roles
            .values()
            .filter(|role| role.cached_commit.is_some())
            .count()
    }

    pub(super) fn maximum_depth(&self) -> usize {
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

    pub(super) fn set_pending_position(&mut self, surface_id: u32, x: i32, y: i32) -> bool {
        let Some(role) = self.roles.get_mut(&surface_id) else {
            return false;
        };
        role.pending_position = Some((x, y));
        true
    }

    pub(super) fn take_pending_positions_for_parent(
        &mut self,
        parent_id: u32,
    ) -> Vec<(u32, i32, i32)> {
        self.roles
            .iter_mut()
            .filter_map(|(surface_id, role)| {
                (role.parent_id == parent_id)
                    .then(|| {
                        role.pending_position
                            .take()
                            .map(|(x, y)| (*surface_id, x, y))
                    })
                    .flatten()
            })
            .collect()
    }

    pub(super) fn take_latched_commits(
        &mut self,
        parent_id: u32,
    ) -> Vec<(u32, CachedSubsurfaceCommit)> {
        let mut surface_ids = Vec::new();
        self.collect_effectively_synchronized_descendants(parent_id, &mut surface_ids);
        surface_ids
            .into_iter()
            .filter_map(|surface_id| {
                self.roles
                    .get_mut(&surface_id)
                    .and_then(|role| role.cached_commit.take())
                    .map(|commit| (surface_id, commit))
            })
            .collect()
    }

    pub(super) fn take_desynchronized_subtree_commits(
        &mut self,
        surface_id: u32,
    ) -> Vec<(u32, CachedSubsurfaceCommit)> {
        let mut surface_ids = vec![surface_id];
        self.collect_all_descendants(surface_id, &mut surface_ids);
        let eligible = surface_ids
            .into_iter()
            .filter(|surface_id| !self.is_effectively_synchronized(*surface_id))
            .collect::<Vec<_>>();
        eligible
            .into_iter()
            .filter_map(|surface_id| {
                self.roles
                    .get_mut(&surface_id)
                    .and_then(|role| role.cached_commit.take())
                    .map(|commit| (surface_id, commit))
            })
            .collect()
    }

    fn collect_effectively_synchronized_descendants(&self, parent_id: u32, output: &mut Vec<u32>) {
        let children = self
            .roles
            .iter()
            .filter_map(|(surface_id, role)| (role.parent_id == parent_id).then_some(*surface_id))
            .collect::<Vec<_>>();
        for child_id in children {
            if self.is_effectively_synchronized(child_id) {
                output.push(child_id);
                self.collect_effectively_synchronized_descendants(child_id, output);
            }
        }
    }

    fn collect_all_descendants(&self, parent_id: u32, output: &mut Vec<u32>) {
        let children = self
            .roles
            .iter()
            .filter_map(|(surface_id, role)| (role.parent_id == parent_id).then_some(*surface_id))
            .collect::<Vec<_>>();
        for child_id in children {
            output.push(child_id);
            self.collect_all_descendants(child_id, output);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_role_defaults_to_synchronized() {
        let mut state = SubsurfaceTransactionState::default();
        assert!(state.register(2, 1));
        assert_eq!(
            state.requested_mode(2),
            Some(SubsurfaceSyncMode::Synchronized)
        );
        assert!(state.is_effectively_synchronized(2));
    }

    #[test]
    fn set_sync_and_set_desync_record_requested_mode() {
        let mut state = SubsurfaceTransactionState::default();
        assert!(state.register(2, 1));
        assert!(state.set_mode(2, SubsurfaceSyncMode::Desynchronized));
        assert_eq!(
            state.requested_mode(2),
            Some(SubsurfaceSyncMode::Desynchronized)
        );
        assert!(state.set_mode(2, SubsurfaceSyncMode::Synchronized));
        assert_eq!(
            state.requested_mode(2),
            Some(SubsurfaceSyncMode::Synchronized)
        );
    }

    #[test]
    fn desynchronized_descendant_under_synchronized_ancestor_remains_effectively_sync() {
        let mut state = SubsurfaceTransactionState::default();
        assert!(state.register(2, 1));
        assert!(state.register(3, 2));
        assert!(state.set_mode(3, SubsurfaceSyncMode::Desynchronized));
        assert!(state.is_effectively_synchronized(3));
        assert!(state.set_mode(2, SubsurfaceSyncMode::Desynchronized));
        assert!(!state.is_effectively_synchronized(3));
    }

    #[test]
    fn role_registration_rejects_reuse_and_cycles() {
        let mut state = SubsurfaceTransactionState::default();
        assert!(state.register(2, 1));
        assert!(!state.register(2, 3));
        assert!(!state.register(1, 2));
    }

    #[test]
    fn role_destruction_removes_only_that_role_while_surface_teardown_removes_subtree() {
        let mut state = SubsurfaceTransactionState::default();
        assert!(state.register(2, 1));
        assert!(state.register(3, 2));
        assert!(state.remove_role(2).is_none());
        assert_eq!(state.parent(2), None);
        assert_eq!(state.parent(3), Some(2));

        assert!(state.register(4, 1));
        assert!(state.register(5, 4));
        assert!(state.remove_subtree(4).is_empty());
        assert_eq!(state.parent(4), None);
        assert_eq!(state.parent(5), None);
    }
}
