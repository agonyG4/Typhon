use std::{
    collections::{HashMap, VecDeque},
    time::Instant,
};

use wayland_server::protocol::wl_callback;
use wayland_server::protocol::wl_output;

use super::state::CapturedSurfacePacing;
use super::{
    CapturedSurfacePresentation, RenderableSurfaceDamage, SurfaceCommitId, SurfaceCommitSequence,
    SurfaceInputRegion,
    explicit_sync::{CapturedExplicitSyncState, PendingPresentationFeedback},
    state_data::{PendingSurfaceAttachment, PendingViewportChange, SurfaceBufferRelease},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum SubsurfaceSyncMode {
    #[default]
    Synchronized,
    Desynchronized,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum PointerConstraintLifecycleCommit {
    // Install/removal are synchronized with surface publication as Typhon
    // policy, inspired by current KWin behavior; this is not an explicit
    // pointer-constraints protocol requirement.
    #[default]
    NoChange,
    Install,
    Remove,
    Cancel,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) enum PointerConstraintRegionCommit {
    // set_region is protocol-defined double-buffered state.  The captured
    // mutation carries the producing constraint identity alongside it.
    #[default]
    NoChange,
    Set(SurfaceInputRegion),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) enum PointerConstraintHintCommit {
    // set_cursor_position_hint is protocol-defined double-buffered state.
    // The captured mutation carries the producing constraint identity.
    #[default]
    NoChange,
    Set((f64, f64)),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct CapturedPointerConstraintCommit {
    pub(super) constraint_id: u64,
    pub(super) lifecycle: PointerConstraintLifecycleCommit,
    pub(super) region: PointerConstraintRegionCommit,
    pub(super) cursor_position_hint: PointerConstraintHintCommit,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) enum CapturedPointerConstraintSurfaceState {
    #[default]
    NoChange,
    Mutation(CapturedPointerConstraintCommit),
}

impl CapturedPointerConstraintSurfaceState {
    pub(super) fn merge(self, newer: Self) -> Self {
        match (self, newer) {
            (Self::NoChange, newer) | (newer, Self::NoChange) => newer,
            (Self::Mutation(older), Self::Mutation(newer)) => {
                if older.constraint_id != newer.constraint_id {
                    return Self::Mutation(newer);
                }
                let lifecycle =
                    merge_pointer_constraint_lifecycle(older.lifecycle, newer.lifecycle);
                if lifecycle == PointerConstraintLifecycleCommit::Cancel {
                    return Self::Mutation(CapturedPointerConstraintCommit {
                        constraint_id: newer.constraint_id,
                        lifecycle,
                        region: PointerConstraintRegionCommit::NoChange,
                        cursor_position_hint: PointerConstraintHintCommit::NoChange,
                    });
                }
                Self::Mutation(CapturedPointerConstraintCommit {
                    constraint_id: newer.constraint_id,
                    lifecycle,
                    region: merge_pointer_constraint_region(older.region, newer.region),
                    cursor_position_hint: merge_pointer_constraint_hint(
                        older.cursor_position_hint,
                        newer.cursor_position_hint,
                    ),
                })
            }
        }
    }
}

fn merge_pointer_constraint_lifecycle(
    older: PointerConstraintLifecycleCommit,
    newer: PointerConstraintLifecycleCommit,
) -> PointerConstraintLifecycleCommit {
    match (older, newer) {
        (
            PointerConstraintLifecycleCommit::Install,
            PointerConstraintLifecycleCommit::Remove | PointerConstraintLifecycleCommit::Cancel,
        ) => PointerConstraintLifecycleCommit::Cancel,
        (PointerConstraintLifecycleCommit::Cancel, _) => PointerConstraintLifecycleCommit::Cancel,
        (_, PointerConstraintLifecycleCommit::Cancel) => PointerConstraintLifecycleCommit::Cancel,
        (older, PointerConstraintLifecycleCommit::NoChange) => older,
        (_, newer) => newer,
    }
}

fn merge_pointer_constraint_region(
    older: PointerConstraintRegionCommit,
    newer: PointerConstraintRegionCommit,
) -> PointerConstraintRegionCommit {
    match newer {
        PointerConstraintRegionCommit::NoChange => older,
        newer => newer,
    }
}

fn merge_pointer_constraint_hint(
    older: PointerConstraintHintCommit,
    newer: PointerConstraintHintCommit,
) -> PointerConstraintHintCommit {
    match newer {
        PointerConstraintHintCommit::NoChange => older,
        newer => newer,
    }
}

#[derive(Debug)]
pub(super) struct CachedSubsurfaceCommit {
    pub(super) commit_id: SurfaceCommitId,
    pub(super) commit_sequence: SurfaceCommitSequence,
    pub(super) attachment: Option<PendingSurfaceAttachment>,
    pub(super) damage: Option<RenderableSurfaceDamage>,
    pub(super) frame_callbacks: Vec<wl_callback::WlCallback>,
    pub(super) explicit_sync: Option<CapturedExplicitSyncState>,
    pub(super) offset: Option<(i32, i32)>,
    pub(super) viewport_destination: PendingViewportChange,
    pub(super) buffer_scale: Option<u32>,
    pub(super) buffer_transform: Option<wl_output::Transform>,
    pub(super) opaque_region: Option<SurfaceInputRegion>,
    pub(super) input_region: Option<SurfaceInputRegion>,
    pub(super) presentation_feedbacks: Vec<PendingPresentationFeedback>,
    pub(super) resize_commit: Option<super::ResizeCommitSnapshot>,
    pub(super) resize_capture_finalized: bool,
    pub(super) window_geometry: Option<super::XdgWindowGeometry>,
    pub(super) cached_at: Instant,
    pub(super) pacing: CapturedSurfacePacing,
    pub(super) presentation: CapturedSurfacePresentation,
    pub(super) pointer_constraint_state: CapturedPointerConstraintSurfaceState,
}

impl CachedSubsurfaceCommit {
    pub(super) fn merge(&mut self, newer: Self) -> Option<SurfaceBufferRelease> {
        let Self {
            commit_id,
            commit_sequence,
            attachment,
            damage,
            frame_callbacks,
            explicit_sync,
            offset,
            viewport_destination,
            buffer_scale,
            buffer_transform,
            opaque_region,
            input_region,
            presentation_feedbacks,
            resize_commit,
            resize_capture_finalized,
            window_geometry,
            cached_at: _,
            pacing,
            presentation,
            pointer_constraint_state,
        } = newer;
        // A pacing value is a commit boundary.  The caller must not merge a
        // later paced update into an older content update.
        debug_assert!(!pacing.is_boundary());
        self.commit_id = commit_id;
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
        if buffer_transform.is_some() {
            self.buffer_transform = buffer_transform;
        }
        if opaque_region.is_some() {
            self.opaque_region = opaque_region;
        }
        if input_region.is_some() {
            self.input_region = input_region;
        }
        self.presentation_feedbacks.extend(presentation_feedbacks);
        self.presentation = presentation;
        self.pointer_constraint_state = self
            .pointer_constraint_state
            .clone()
            .merge(pointer_constraint_state);
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
        (Some(RenderableSurfaceDamage::HistoryLost), _)
        | (_, Some(RenderableSurfaceDamage::HistoryLost)) => {
            Some(RenderableSurfaceDamage::HistoryLost)
        }
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
    use crate::compositor::state_data::{InputRegionOp, InputRegionRect};
    use crate::compositor::{
        SurfaceContentType, SurfacePresentationHint, SurfacePresentationMetadata,
        SurfacePresentationState, XdgWindowGeometry,
    };

    fn cached_commit_with_window_geometry(
        sequence: u64,
        window_geometry: XdgWindowGeometry,
    ) -> CachedSubsurfaceCommit {
        CachedSubsurfaceCommit {
            commit_id: SurfaceCommitId::for_tests(sequence),
            commit_sequence: SurfaceCommitSequence(sequence),
            attachment: None,
            damage: None,
            frame_callbacks: Vec::new(),
            explicit_sync: None,
            offset: None,
            viewport_destination: PendingViewportChange::default(),
            buffer_scale: None,
            buffer_transform: None,
            opaque_region: None,
            input_region: None,
            presentation_feedbacks: Vec::new(),
            resize_commit: None,
            resize_capture_finalized: true,
            window_geometry: Some(window_geometry),
            cached_at: Instant::now(),
            pacing: CapturedSurfacePacing::default(),
            presentation: CapturedSurfacePresentation::default(),
            pointer_constraint_state: CapturedPointerConstraintSurfaceState::default(),
        }
    }

    fn pointer_state(
        constraint_id: u64,
        lifecycle: PointerConstraintLifecycleCommit,
        region: PointerConstraintRegionCommit,
        cursor_position_hint: PointerConstraintHintCommit,
    ) -> CapturedPointerConstraintSurfaceState {
        CapturedPointerConstraintSurfaceState::Mutation(CapturedPointerConstraintCommit {
            constraint_id,
            lifecycle,
            region,
            cursor_position_hint,
        })
    }

    #[test]
    fn newer_region_replaces_older_region_but_no_change_preserves_it() {
        let mut cached = cached_commit_with_window_geometry(1, XdgWindowGeometry::new(1, 2, 3, 4));
        cached.pointer_constraint_state = pointer_state(
            1,
            PointerConstraintLifecycleCommit::NoChange,
            PointerConstraintRegionCommit::Set(SurfaceInputRegion::Custom(vec![
                InputRegionOp::Add(InputRegionRect::new(1, 2, 3, 4).unwrap()),
            ])),
            PointerConstraintHintCommit::NoChange,
        );
        let mut newer = cached_commit_with_window_geometry(2, XdgWindowGeometry::new(1, 2, 3, 4));
        newer.pointer_constraint_state = pointer_state(
            1,
            PointerConstraintLifecycleCommit::NoChange,
            PointerConstraintRegionCommit::Set(SurfaceInputRegion::Custom(vec![
                InputRegionOp::Add(InputRegionRect::new(5, 6, 7, 8).unwrap()),
            ])),
            PointerConstraintHintCommit::NoChange,
        );

        cached.merge(newer);

        assert_eq!(
            cached.pointer_constraint_state,
            pointer_state(
                1,
                PointerConstraintLifecycleCommit::NoChange,
                PointerConstraintRegionCommit::Set(SurfaceInputRegion::Custom(vec![
                    InputRegionOp::Add(InputRegionRect::new(5, 6, 7, 8).unwrap()),
                ])),
                PointerConstraintHintCommit::NoChange,
            )
        );
    }

    #[test]
    fn explicit_default_region_is_not_treated_as_no_change() {
        let mut cached = cached_commit_with_window_geometry(1, XdgWindowGeometry::new(1, 2, 3, 4));
        cached.pointer_constraint_state = pointer_state(
            1,
            PointerConstraintLifecycleCommit::NoChange,
            PointerConstraintRegionCommit::Set(SurfaceInputRegion::Custom(vec![
                InputRegionOp::Add(InputRegionRect::new(1, 2, 3, 4).unwrap()),
            ])),
            PointerConstraintHintCommit::NoChange,
        );
        let mut newer = cached_commit_with_window_geometry(2, XdgWindowGeometry::new(1, 2, 3, 4));
        newer.pointer_constraint_state = pointer_state(
            1,
            PointerConstraintLifecycleCommit::NoChange,
            PointerConstraintRegionCommit::Set(SurfaceInputRegion::Default),
            PointerConstraintHintCommit::NoChange,
        );

        cached.merge(newer);

        assert_eq!(
            cached.pointer_constraint_state,
            pointer_state(
                1,
                PointerConstraintLifecycleCommit::NoChange,
                PointerConstraintRegionCommit::Set(SurfaceInputRegion::Default),
                PointerConstraintHintCommit::NoChange,
            )
        );
    }

    #[test]
    fn install_then_remove_before_publication_collapses_without_activation() {
        let mut cached = cached_commit_with_window_geometry(1, XdgWindowGeometry::new(1, 2, 3, 4));
        cached.pointer_constraint_state = pointer_state(
            22,
            PointerConstraintLifecycleCommit::Install,
            PointerConstraintRegionCommit::Set(SurfaceInputRegion::Custom(vec![
                InputRegionOp::Add(InputRegionRect::new(11, 12, 13, 14).unwrap()),
            ])),
            PointerConstraintHintCommit::Set((15.0, 16.0)),
        );
        let mut newer = cached_commit_with_window_geometry(2, XdgWindowGeometry::new(1, 2, 3, 4));
        newer.pointer_constraint_state = pointer_state(
            22,
            PointerConstraintLifecycleCommit::Remove,
            PointerConstraintRegionCommit::NoChange,
            PointerConstraintHintCommit::NoChange,
        );

        cached.merge(newer);

        assert_eq!(
            cached.pointer_constraint_state,
            CapturedPointerConstraintSurfaceState::Mutation(CapturedPointerConstraintCommit {
                constraint_id: 22,
                lifecycle: PointerConstraintLifecycleCommit::Cancel,
                region: PointerConstraintRegionCommit::NoChange,
                cursor_position_hint: PointerConstraintHintCommit::NoChange,
            })
        );
    }

    #[test]
    fn current_constraint_removal_survives_cached_commit_merge() {
        let mut cached = cached_commit_with_window_geometry(1, XdgWindowGeometry::new(1, 2, 3, 4));
        cached.pointer_constraint_state = pointer_state(
            22,
            PointerConstraintLifecycleCommit::Remove,
            PointerConstraintRegionCommit::NoChange,
            PointerConstraintHintCommit::NoChange,
        );
        let newer = cached_commit_with_window_geometry(2, XdgWindowGeometry::new(1, 2, 3, 4));

        cached.merge(newer);

        assert_eq!(
            cached.pointer_constraint_state,
            CapturedPointerConstraintSurfaceState::Mutation(CapturedPointerConstraintCommit {
                constraint_id: 22,
                lifecycle: PointerConstraintLifecycleCommit::Remove,
                region: PointerConstraintRegionCommit::NoChange,
                cursor_position_hint: PointerConstraintHintCommit::NoChange,
            })
        );
    }

    #[test]
    fn no_change_hint_preserves_captured_hint_until_a_new_hint_is_captured() {
        let mut cached = cached_commit_with_window_geometry(1, XdgWindowGeometry::new(1, 2, 3, 4));
        cached.pointer_constraint_state = pointer_state(
            1,
            PointerConstraintLifecycleCommit::NoChange,
            PointerConstraintRegionCommit::NoChange,
            PointerConstraintHintCommit::Set((12.0, 18.0)),
        );
        let newer = cached_commit_with_window_geometry(2, XdgWindowGeometry::new(1, 2, 3, 4));

        cached.merge(newer);

        assert_eq!(
            cached.pointer_constraint_state,
            CapturedPointerConstraintSurfaceState::Mutation(CapturedPointerConstraintCommit {
                constraint_id: 1,
                lifecycle: PointerConstraintLifecycleCommit::NoChange,
                region: PointerConstraintRegionCommit::NoChange,
                cursor_position_hint: PointerConstraintHintCommit::Set((12.0, 18.0)),
            })
        );
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

    fn captured_presentation(
        hint: SurfacePresentationHint,
        content_type: SurfaceContentType,
    ) -> CapturedSurfacePresentation {
        let state = SurfacePresentationState::default()
            .set_pending_hint(hint)
            .set_pending_content_type(content_type);
        let (_, captured) = state.capture_pending_and_reset();
        captured
    }

    #[test]
    fn cached_commit_merge_keeps_the_newest_presentation_metadata() {
        let mut cached =
            cached_commit_with_window_geometry(1, XdgWindowGeometry::new(1, 2, 300, 200));
        cached.presentation =
            captured_presentation(SurfacePresentationHint::Async, SurfaceContentType::None);
        let mut newer =
            cached_commit_with_window_geometry(2, XdgWindowGeometry::new(1, 2, 300, 200));
        newer.presentation =
            captured_presentation(SurfacePresentationHint::Async, SurfaceContentType::Video);

        cached.merge(newer);

        assert_eq!(
            cached.presentation.metadata,
            SurfacePresentationMetadata {
                hint: SurfacePresentationHint::Async,
                content_type: SurfaceContentType::Video,
            }
        );
    }
}

#[derive(Debug)]
struct SubsurfaceRoleState {
    parent_id: u32,
    requested_mode: SubsurfaceSyncMode,
    cached_commits: VecDeque<CachedSubsurfaceCommit>,
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
                cached_commits: VecDeque::new(),
                pending_position: None,
            },
        );
        true
    }

    pub(super) fn remove_role(&mut self, surface_id: u32) -> Vec<CachedSubsurfaceCommit> {
        self.roles
            .remove(&surface_id)
            .map(|role| role.cached_commits.into_iter().collect())
            .unwrap_or_default()
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
            if let Some(role) = self.roles.remove(&id) {
                removed.extend(role.cached_commits);
            }
        }
        removed
    }

    pub(super) fn drain_cached_commits(&mut self) -> Vec<CachedSubsurfaceCommit> {
        self.roles
            .values_mut()
            .flat_map(|role| role.cached_commits.drain(..))
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
        if role.cached_commits.is_empty() {
            role.cached_commits.push_back(commit);
            return None;
        }
        if role.cached_commits.len() == 1
            && !role
                .cached_commits
                .front()
                .is_some_and(|cached| cached.pacing.is_boundary())
            && !commit.pacing.is_boundary()
        {
            role.cached_commits
                .front_mut()
                .expect("cached commit exists")
                .merge(commit)
        } else {
            role.cached_commits.push_back(commit);
            None
        }
    }

    pub(super) fn has_cached_commit(&self, surface_id: u32) -> bool {
        self.roles
            .get(&surface_id)
            .is_some_and(|role| !role.cached_commits.is_empty())
    }

    pub(super) fn cached_node_count(&self) -> usize {
        self.roles
            .values()
            .filter(|role| !role.cached_commits.is_empty())
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
        let mut commits = Vec::new();
        for surface_id in surface_ids {
            if let Some(role) = self.roles.get_mut(&surface_id) {
                commits.extend(
                    role.cached_commits
                        .drain(..)
                        .map(|commit| (surface_id, commit)),
                );
            }
        }
        commits
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
        let mut commits = Vec::new();
        for surface_id in eligible {
            if let Some(role) = self.roles.get_mut(&surface_id) {
                commits.extend(
                    role.cached_commits
                        .drain(..)
                        .map(|commit| (surface_id, commit)),
                );
            }
        }
        commits
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
        assert!(state.remove_role(2).is_empty());
        assert_eq!(state.parent(2), None);
        assert_eq!(state.parent(3), Some(2));

        assert!(state.register(4, 1));
        assert!(state.register(5, 4));
        assert!(state.remove_subtree(4).is_empty());
        assert_eq!(state.parent(4), None);
        assert_eq!(state.parent(5), None);
    }

    #[test]
    fn pacing_boundaries_are_never_merged_or_reordered() {
        let mut state = SubsurfaceTransactionState::default();
        assert!(state.register(2, 1));

        let mut first = crate::compositor::state::empty_cached_subsurface_commit();
        first.pacing = CapturedSurfacePacing {
            fifo_set_barrier: true,
            ..CapturedSurfacePacing::default()
        };
        let mut second = crate::compositor::state::empty_cached_subsurface_commit();
        second.pacing = CapturedSurfacePacing {
            fifo_wait_barrier: true,
            ..CapturedSurfacePacing::default()
        };

        assert!(state.cache_commit(2, first).is_none());
        assert!(state.cache_commit(2, second).is_none());
        assert_eq!(state.roles[&2].cached_commits.len(), 2);
        assert!(state.roles[&2].cached_commits[0].pacing.fifo_set_barrier);
        assert!(state.roles[&2].cached_commits[1].pacing.fifo_wait_barrier);
    }
}
