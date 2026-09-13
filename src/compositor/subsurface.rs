use std::{
    collections::{HashMap, VecDeque},
    time::Instant,
};

use wayland_server::backend::ClientId;
use wayland_server::protocol::wl_callback;
use wayland_server::protocol::wl_output;

use super::state::CapturedSurfacePacing;
use super::{
    CapturedSurfacePresentation, RenderableSurfaceDamage, SurfaceCommitId, SurfaceCommitSequence,
    SurfaceInputRegion,
    explicit_sync::{CapturedExplicitSyncState, PendingPresentationFeedback},
    state_data::{
        BackgroundEffectRegion, PendingSurfaceAttachment, PendingSurfaceBuffer,
        PendingViewportChange,
    },
};
use crate::compositor::layer_shell::CapturedLayerSurfaceCommitState;

pub(super) const MAX_SYNCHRONIZED_CACHED_COMMITS_PER_SURFACE: usize = 8;
pub(super) const MAX_SYNCHRONIZED_CACHED_COMMITS_PER_CLIENT: usize = 256;
pub(super) const MAX_SYNCHRONIZED_CACHED_COMMITS_TOTAL: usize = 4096;
// An obligation is one retained frame callback, presentation feedback, buffer
// ownership slot (plus one slot per validated DMA-BUF plane), or explicit-sync
// acquire/release point. This is a cardinality guard, not a byte or GPU-memory
// estimate. A merged entry is checked against the same limits because callbacks
// can accumulate even while the VecDeque length stays constant.
pub(super) const MAX_SYNCHRONIZED_CACHED_OBLIGATIONS_PER_SURFACE: usize = 1024;
pub(super) const MAX_SYNCHRONIZED_CACHED_OBLIGATIONS_PER_CLIENT: usize = 8192;
pub(super) const MAX_SYNCHRONIZED_CACHED_OBLIGATIONS_TOTAL: usize = 65536;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum SubsurfaceSyncMode {
    #[default]
    Synchronized,
    Desynchronized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SubsurfaceRelationshipPhase {
    PendingParentCommit,
    Latched,
    Applied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct SubsurfaceRelationshipId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CapturedSubsurfaceRelationship {
    pub(super) surface_id: u32,
    pub(super) parent_id: u32,
    pub(super) relationship_id: SubsurfaceRelationshipId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CapturedSubsurfacePosition {
    pub(super) relationship: CapturedSubsurfaceRelationship,
    pub(super) x: i32,
    pub(super) y: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CapturedSubsurfaceStackEntry {
    Parent,
    Child(CapturedSubsurfaceRelationship),
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
pub(super) struct CapturedPointerConstraintSurfaceTransition {
    // The old effective constraint and the new requested mutation are
    // independent: a replacement must carry both through one surface commit.
    pub(super) retire: Option<CapturedPointerConstraintCommit>,
    pub(super) install_or_update: Option<CapturedPointerConstraintCommit>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) enum CapturedPointerConstraintSurfaceState {
    #[default]
    NoChange,
    Mutation(CapturedPointerConstraintCommit),
    Transition(CapturedPointerConstraintSurfaceTransition),
}

impl CapturedPointerConstraintSurfaceState {
    pub(super) fn merge(self, newer: Self) -> Self {
        let Some(older) = self.into_transition() else {
            return newer;
        };
        let Some(newer) = newer.into_transition() else {
            return Self::from_transition(older);
        };
        Self::from_transition(merge_pointer_constraint_transition(older, newer))
    }

    fn from_transition(transition: CapturedPointerConstraintSurfaceTransition) -> Self {
        match (transition.retire, transition.install_or_update) {
            (None, None) => Self::NoChange,
            (Some(retire), None) => Self::Mutation(retire),
            (None, Some(install)) => Self::Mutation(install),
            (Some(retire), Some(install)) => {
                Self::Transition(CapturedPointerConstraintSurfaceTransition {
                    retire: Some(retire),
                    install_or_update: Some(install),
                })
            }
        }
    }

    pub(super) fn into_transition(self) -> Option<CapturedPointerConstraintSurfaceTransition> {
        match self {
            Self::NoChange => None,
            Self::Mutation(mutation) => Some(match mutation.lifecycle {
                PointerConstraintLifecycleCommit::Remove => {
                    CapturedPointerConstraintSurfaceTransition {
                        retire: Some(mutation),
                        install_or_update: None,
                    }
                }
                PointerConstraintLifecycleCommit::Install
                | PointerConstraintLifecycleCommit::NoChange
                | PointerConstraintLifecycleCommit::Cancel => {
                    CapturedPointerConstraintSurfaceTransition {
                        retire: None,
                        install_or_update: Some(mutation),
                    }
                }
            }),
            Self::Transition(transition) => Some(transition),
        }
    }
}

fn merge_pointer_constraint_transition(
    mut older: CapturedPointerConstraintSurfaceTransition,
    newer: CapturedPointerConstraintSurfaceTransition,
) -> CapturedPointerConstraintSurfaceTransition {
    if let Some(newer_retire) = newer.retire {
        let retire_id = newer_retire.constraint_id;
        let same_identity_install = older
            .install_or_update
            .as_ref()
            .filter(|install| install.constraint_id == retire_id)
            .cloned();
        match same_identity_install {
            Some(install) if install.lifecycle == PointerConstraintLifecycleCommit::Install => {
                // The request was canceled before it became effective. Its
                // protocol object is removed immediately, so there is no
                // surface transition left for this install.
                older.install_or_update = None;
            }
            Some(install) if install.lifecycle == PointerConstraintLifecycleCommit::NoChange => {
                // A region/hint update belongs to the already-effective
                // constraint, so its later destruction is a real retirement.
                // Preserve those fields for retirement-only behavior (notably
                // the oneshot cursor-hint warp), without carrying them into a
                // replacement install.
                let retirement = merge_pointer_constraint_mutation(install, newer_retire);
                older.install_or_update = None;
                if let Some(existing) = older.retire.as_mut() {
                    if existing.constraint_id == retirement.constraint_id {
                        *existing = merge_pointer_constraint_mutation(existing.clone(), retirement);
                    } else {
                        debug_assert_eq!(existing.constraint_id, retirement.constraint_id);
                    }
                } else {
                    older.retire = Some(retirement);
                }
            }
            Some(install) if install.lifecycle == PointerConstraintLifecycleCommit::Cancel => {
                // A canceled request never owned effective surface state.
                older.install_or_update = None;
            }
            Some(_) | None => {
                if let Some(existing) = older.retire.as_mut() {
                    if existing.constraint_id == retire_id {
                        *existing =
                            merge_pointer_constraint_mutation(existing.clone(), newer_retire);
                    } else {
                        // One effective constraint exists for a surface/seat
                        // relationship. Keep the first retirement if malformed
                        // input retires two IDs.
                        debug_assert_eq!(existing.constraint_id, retire_id);
                    }
                } else {
                    older.retire = Some(newer_retire);
                }
            }
        }
    }
    if let Some(newer_install) = newer.install_or_update {
        if newer_install.lifecycle == PointerConstraintLifecycleCommit::Cancel
            && older
                .install_or_update
                .as_ref()
                .is_some_and(|install| install.constraint_id == newer_install.constraint_id)
        {
            older.install_or_update = None;
        } else {
            older.install_or_update = Some(match older.install_or_update.take() {
                Some(older_install)
                    if older_install.constraint_id == newer_install.constraint_id =>
                {
                    merge_pointer_constraint_mutation(older_install, newer_install)
                }
                _ => newer_install,
            });
        }
    }
    older
}

fn merge_pointer_constraint_mutation(
    older: CapturedPointerConstraintCommit,
    newer: CapturedPointerConstraintCommit,
) -> CapturedPointerConstraintCommit {
    let lifecycle = merge_pointer_constraint_lifecycle(older.lifecycle, newer.lifecycle);
    if lifecycle == PointerConstraintLifecycleCommit::Cancel {
        return CapturedPointerConstraintCommit {
            constraint_id: newer.constraint_id,
            lifecycle,
            region: PointerConstraintRegionCommit::NoChange,
            cursor_position_hint: PointerConstraintHintCommit::NoChange,
        };
    }
    CapturedPointerConstraintCommit {
        constraint_id: newer.constraint_id,
        lifecycle,
        region: merge_pointer_constraint_region(older.region, newer.region),
        cursor_position_hint: merge_pointer_constraint_hint(
            older.cursor_position_hint,
            newer.cursor_position_hint,
        ),
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

#[derive(Debug, Default)]
pub(super) struct CapturedSubsurfaceParentState {
    pub(super) activations: Vec<CapturedSubsurfaceRelationship>,
    pub(super) positions: Vec<CapturedSubsurfacePosition>,
    pub(super) stack: Option<Vec<CapturedSubsurfaceStackEntry>>,
}

#[derive(Debug, Default)]
pub(super) struct CapturedSurfaceCommitContext {
    pub(super) subsurface_parent: CapturedSubsurfaceParentState,
    pub(super) layer_surface: Option<CapturedLayerSurfaceCommitState>,
}

impl CapturedSurfaceCommitContext {
    pub(super) fn merge(&mut self, newer: Self) {
        for relationship in newer.subsurface_parent.activations {
            if !self.subsurface_parent.activations.contains(&relationship) {
                self.subsurface_parent.activations.push(relationship);
            }
        }
        for position in newer.subsurface_parent.positions {
            if let Some(current) = self
                .subsurface_parent
                .positions
                .iter_mut()
                .find(|current| current.relationship == position.relationship)
            {
                *current = position;
            } else {
                self.subsurface_parent.positions.push(position);
            }
        }
        if newer.subsurface_parent.stack.is_some() {
            self.subsurface_parent.stack = newer.subsurface_parent.stack;
        }
        self.layer_surface = match (self.layer_surface, newer.layer_surface) {
            (Some(older), Some(mut newer)) => {
                // Acknowledgements are chronological commit obligations. If
                // the newer coalesced update has no replacement, keep the
                // acknowledgement captured by the older update.
                if newer.acknowledged_configure.is_none() {
                    newer.acknowledged_configure = older.acknowledged_configure;
                }
                Some(newer)
            }
            (None, Some(newer)) => Some(newer),
            (Some(older), None) => Some(older),
            (None, None) => None,
        };
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
    pub(super) background_effect: Option<BackgroundEffectRegion>,
    pub(super) presentation_feedbacks: Vec<PendingPresentationFeedback>,
    pub(super) resize_commit: Option<super::ResizeCommitSnapshot>,
    pub(super) resize_capture_finalized: bool,
    pub(super) window_geometry: Option<super::XdgWindowGeometry>,
    pub(super) cached_at: Instant,
    pub(super) pacing: CapturedSurfacePacing,
    pub(super) presentation: CapturedSurfacePresentation,
    pub(super) pointer_constraint_state: CapturedPointerConstraintSurfaceState,
    pub(super) commit_context: CapturedSurfaceCommitContext,
}

impl CachedSubsurfaceCommit {
    pub(super) fn cached_obligation_count(&self) -> usize {
        self.frame_callbacks
            .len()
            .checked_add(self.presentation_feedbacks.len())
            .and_then(|count| {
                count.checked_add(cached_attachment_obligation_count(self.attachment.as_ref()))
            })
            .and_then(|count| {
                count.checked_add(cached_explicit_sync_obligation_count(
                    self.explicit_sync.as_ref(),
                ))
            })
            .expect("synchronized cache obligation count overflow")
    }

    fn merged_cached_obligation_count(&self, newer: &Self) -> usize {
        let attachment = newer.attachment.as_ref().or(self.attachment.as_ref());
        let explicit_sync = if newer.attachment.is_some() || newer.explicit_sync.is_some() {
            newer.explicit_sync.as_ref()
        } else {
            self.explicit_sync.as_ref()
        };
        newer
            .frame_callbacks
            .len()
            .checked_add(self.frame_callbacks.len())
            .and_then(|count| count.checked_add(newer.presentation_feedbacks.len()))
            .and_then(|count| count.checked_add(cached_attachment_obligation_count(attachment)))
            .and_then(|count| {
                count.checked_add(cached_explicit_sync_obligation_count(explicit_sync))
            })
            .expect("synchronized cache obligation count overflow")
    }

    pub(super) fn merge(&mut self, newer: Self) -> Option<PendingSurfaceBuffer> {
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
            background_effect,
            presentation_feedbacks,
            resize_commit,
            resize_capture_finalized,
            window_geometry,
            cached_at: _,
            pacing,
            presentation,
            pointer_constraint_state,
            commit_context,
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
                    PendingSurfaceAttachment::Buffer(buffer) => Some(buffer),
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
        if background_effect.is_some() {
            self.background_effect = background_effect;
        }
        // A cached merge eliminates the older Content Update. Presentation
        // feedback is bound to that exact commit and must not follow the
        // frame-callback carry-forward rules into the replacement commit.
        for feedback in self.presentation_feedbacks.drain(..) {
            feedback.feedback.discarded();
        }
        self.presentation_feedbacks = presentation_feedbacks;
        self.presentation = presentation;
        self.pointer_constraint_state = self
            .pointer_constraint_state
            .clone()
            .merge(pointer_constraint_state);
        self.commit_context.merge(commit_context);
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

fn cached_attachment_obligation_count(attachment: Option<&PendingSurfaceAttachment>) -> usize {
    match attachment {
        Some(PendingSurfaceAttachment::Buffer(buffer)) => 1usize
            .checked_add(
                buffer
                    .data
                    .dmabuf_handle()
                    .map_or(0, |handle| handle.planes().len()),
            )
            .expect("synchronized cache attachment obligation count overflow"),
        Some(PendingSurfaceAttachment::RemoveContent) | None => 0,
    }
}

fn cached_explicit_sync_obligation_count(
    explicit_sync: Option<&CapturedExplicitSyncState>,
) -> usize {
    explicit_sync.map_or(0, |state| {
        usize::from(state.acquire.is_some()) + usize::from(state.release.is_some())
    })
}

#[derive(Debug)]
pub(super) enum CacheCommitOutcome {
    Inserted,
    Merged {
        superseded_buffer: Option<Box<PendingSurfaceBuffer>>,
    },
    Rejected {
        commit: Box<CachedSubsurfaceCommit>,
        reason: CacheAdmissionFailure,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CacheAdmissionFailure {
    MissingRole,
    ClientAlreadyExhausted,
    PerSurfaceEntryLimit,
    PerClientEntryLimit,
    TotalEntryLimit,
    PerSurfaceObligationLimit,
    PerClientObligationLimit,
    TotalObligationLimit,
    AccountingInvariant,
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
mod commit_context_tests {
    use super::*;

    fn relationship(surface_id: u32, relationship_id: u64) -> CapturedSubsurfaceRelationship {
        CapturedSubsurfaceRelationship {
            surface_id,
            parent_id: 1,
            relationship_id: SubsurfaceRelationshipId(relationship_id),
        }
    }

    #[test]
    fn captured_context_merges_parent_state_chronologically() {
        let mut older = CapturedSurfaceCommitContext {
            subsurface_parent: CapturedSubsurfaceParentState {
                activations: vec![relationship(10, 1)],
                positions: vec![
                    CapturedSubsurfacePosition {
                        relationship: relationship(10, 1),
                        x: 10,
                        y: 10,
                    },
                    CapturedSubsurfacePosition {
                        relationship: relationship(20, 2),
                        x: 20,
                        y: 20,
                    },
                ],
                stack: Some(vec![
                    CapturedSubsurfaceStackEntry::Parent,
                    CapturedSubsurfaceStackEntry::Child(relationship(10, 1)),
                    CapturedSubsurfaceStackEntry::Child(relationship(20, 2)),
                ]),
            },
            layer_surface: None,
        };
        let newer = CapturedSurfaceCommitContext {
            subsurface_parent: CapturedSubsurfaceParentState {
                activations: vec![relationship(20, 2)],
                positions: vec![CapturedSubsurfacePosition {
                    relationship: relationship(10, 1),
                    x: 30,
                    y: 30,
                }],
                stack: Some(vec![
                    CapturedSubsurfaceStackEntry::Parent,
                    CapturedSubsurfaceStackEntry::Child(relationship(20, 2)),
                    CapturedSubsurfaceStackEntry::Child(relationship(10, 1)),
                ]),
            },
            layer_surface: None,
        };

        older.merge(newer);

        assert_eq!(
            older.subsurface_parent.positions,
            vec![
                CapturedSubsurfacePosition {
                    relationship: relationship(10, 1),
                    x: 30,
                    y: 30,
                },
                CapturedSubsurfacePosition {
                    relationship: relationship(20, 2),
                    x: 20,
                    y: 20,
                },
            ]
        );
        assert_eq!(
            older.subsurface_parent.stack,
            Some(vec![
                CapturedSubsurfaceStackEntry::Parent,
                CapturedSubsurfaceStackEntry::Child(relationship(20, 2)),
                CapturedSubsurfaceStackEntry::Child(relationship(10, 1)),
            ])
        );
        assert_eq!(
            older.subsurface_parent.activations,
            vec![relationship(10, 1), relationship(20, 2)]
        );
    }

    #[test]
    fn captured_context_merge_preserves_older_activations_without_duplicates() {
        let mut older = CapturedSurfaceCommitContext {
            subsurface_parent: CapturedSubsurfaceParentState {
                activations: vec![relationship(10, 1)],
                ..CapturedSubsurfaceParentState::default()
            },
            layer_surface: None,
        };
        let newer = CapturedSurfaceCommitContext {
            subsurface_parent: CapturedSubsurfaceParentState {
                activations: vec![relationship(10, 1), relationship(20, 2)],
                ..CapturedSubsurfaceParentState::default()
            },
            layer_surface: None,
        };

        older.merge(newer);

        assert_eq!(
            older.subsurface_parent.activations,
            vec![relationship(10, 1), relationship(20, 2)]
        );
    }

    #[test]
    fn captured_context_merge_keeps_recreated_surface_relationships_distinct() {
        let mut older = CapturedSurfaceCommitContext {
            subsurface_parent: CapturedSubsurfaceParentState {
                activations: vec![relationship(10, 1)],
                positions: vec![CapturedSubsurfacePosition {
                    relationship: relationship(10, 1),
                    x: 1,
                    y: 2,
                }],
                stack: Some(vec![
                    CapturedSubsurfaceStackEntry::Parent,
                    CapturedSubsurfaceStackEntry::Child(relationship(10, 1)),
                ]),
            },
            layer_surface: None,
        };
        let newer = CapturedSurfaceCommitContext {
            subsurface_parent: CapturedSubsurfaceParentState {
                activations: vec![relationship(10, 2)],
                positions: vec![CapturedSubsurfacePosition {
                    relationship: relationship(10, 2),
                    x: 3,
                    y: 4,
                }],
                stack: Some(vec![
                    CapturedSubsurfaceStackEntry::Parent,
                    CapturedSubsurfaceStackEntry::Child(relationship(10, 2)),
                ]),
            },
            layer_surface: None,
        };

        older.merge(newer);

        assert_eq!(
            older.subsurface_parent.activations,
            vec![relationship(10, 1), relationship(10, 2)]
        );
        assert_eq!(older.subsurface_parent.positions.len(), 2);
        assert_eq!(
            older.subsurface_parent.stack,
            Some(vec![
                CapturedSubsurfaceStackEntry::Parent,
                CapturedSubsurfaceStackEntry::Child(relationship(10, 2)),
            ])
        );
    }
}

#[cfg(test)]
mod relationship_phase_tests {
    use super::*;

    #[test]
    fn relationship_activation_follows_pending_latched_applied_lifecycle() {
        let mut transactions = SubsurfaceTransactionState::default();

        assert!(transactions.register(2, 1));
        assert_eq!(
            transactions.relationship_phase(2),
            Some(SubsurfaceRelationshipPhase::PendingParentCommit)
        );
        assert!(transactions.relationship_is_registered_child_of(2, 1));
        assert!(!transactions.relationship_is_applied_child_of(2, 1));

        let captured = transactions.captured_relationship(2).unwrap();
        assert_eq!(
            transactions.take_pending_relationship_activations_for_parent(1),
            vec![captured]
        );
        assert_eq!(
            transactions.relationship_phase(2),
            Some(SubsurfaceRelationshipPhase::Latched)
        );
        assert!(
            transactions
                .take_pending_relationship_activations_for_parent(1)
                .is_empty()
        );

        assert!(transactions.apply_captured_relationship(captured));
        assert_eq!(
            transactions.relationship_phase(2),
            Some(SubsurfaceRelationshipPhase::Applied)
        );
        assert!(transactions.relationship_is_applied_child_of(2, 1));
    }

    #[test]
    fn relationship_id_allocation_fails_without_wrapping() {
        let mut transactions = SubsurfaceTransactionState {
            next_relationship_id: u64::MAX,
            ..SubsurfaceTransactionState::default()
        };

        assert!(!transactions.register(2, 1));
        assert_eq!(transactions.relationship_phase(2), None);
    }

    #[test]
    fn nested_relationship_activations_are_captured_at_each_parent_boundary() {
        let mut transactions = SubsurfaceTransactionState::default();
        assert!(transactions.register(2, 1));
        assert!(transactions.register(3, 2));

        let child = transactions.captured_relationship(2).unwrap();
        let grandchild = transactions.captured_relationship(3).unwrap();
        assert_eq!(
            transactions.take_pending_relationship_activations_for_parent(1),
            vec![child]
        );
        assert_eq!(
            transactions.take_pending_relationship_activations_for_parent(2),
            vec![grandchild]
        );
    }

    #[test]
    fn destroying_each_relationship_phase_removes_its_live_identity() {
        let mut transactions = SubsurfaceTransactionState::default();

        assert!(transactions.register(2, 1));
        assert!(transactions.remove_role(2).is_empty());
        assert_eq!(transactions.relationship_phase(2), None);

        assert!(transactions.register(3, 1));
        let captured = transactions.captured_relationship(3).unwrap();
        assert_eq!(
            transactions.take_pending_relationship_activations_for_parent(1),
            vec![captured]
        );
        assert!(transactions.remove_role(3).is_empty());
        assert_eq!(transactions.relationship_phase(3), None);
        assert!(!transactions.relationship_matches(captured));

        assert!(transactions.register(4, 1));
        let captured = transactions.captured_relationship(4).unwrap();
        assert_eq!(
            transactions.take_pending_relationship_activations_for_parent(1),
            vec![captured]
        );
        assert!(transactions.apply_captured_relationship(captured));
        assert!(transactions.remove_role(4).is_empty());
        assert_eq!(transactions.relationship_phase(4), None);
    }

    #[test]
    fn relationship_ids_are_monotonic_and_stale_captures_are_rejected() {
        let mut transactions = SubsurfaceTransactionState::default();

        assert!(transactions.register(2, 1));
        let first = transactions.captured_relationship(2).unwrap();
        assert_eq!(
            transactions.take_pending_relationship_activations_for_parent(1),
            vec![first]
        );
        assert!(transactions.remove_role(2).is_empty());

        assert!(transactions.register(2, 1));
        let second = transactions.captured_relationship(2).unwrap();
        assert_ne!(first.relationship_id, second.relationship_id);
        assert!(!transactions.apply_captured_relationship(first));
        assert_eq!(
            transactions.relationship_phase(2),
            Some(SubsurfaceRelationshipPhase::PendingParentCommit)
        );
        assert_eq!(
            transactions.take_pending_relationship_activations_for_parent(1),
            vec![second]
        );
        assert!(transactions.apply_captured_relationship(second));
    }
}

#[cfg(test)]
mod window_geometry_tests {
    use super::*;
    use crate::compositor::state_data::{BackgroundEffectRegion, InputRegionOp, InputRegionRect};
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
            background_effect: None,
            presentation_feedbacks: Vec::new(),
            resize_commit: None,
            resize_capture_finalized: true,
            window_geometry: Some(window_geometry),
            cached_at: Instant::now(),
            pacing: CapturedSurfacePacing::default(),
            presentation: CapturedSurfacePresentation::default(),
            pointer_constraint_state: CapturedPointerConstraintSurfaceState::default(),
            commit_context: CapturedSurfaceCommitContext::default(),
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
    fn remove_and_install_different_constraints_preserve_both_sides() {
        let mut cached = cached_commit_with_window_geometry(1, XdgWindowGeometry::new(1, 2, 3, 4));
        cached.pointer_constraint_state = pointer_state(
            22,
            PointerConstraintLifecycleCommit::Remove,
            PointerConstraintRegionCommit::NoChange,
            PointerConstraintHintCommit::NoChange,
        );
        let mut newer = cached_commit_with_window_geometry(2, XdgWindowGeometry::new(1, 2, 3, 4));
        newer.pointer_constraint_state = pointer_state(
            23,
            PointerConstraintLifecycleCommit::Install,
            PointerConstraintRegionCommit::Set(SurfaceInputRegion::Custom(vec![
                InputRegionOp::Add(InputRegionRect::new(5, 6, 7, 8).unwrap()),
            ])),
            PointerConstraintHintCommit::Set((9.0, 10.0)),
        );

        cached.merge(newer);

        assert_eq!(
            cached.pointer_constraint_state,
            CapturedPointerConstraintSurfaceState::Transition(
                CapturedPointerConstraintSurfaceTransition {
                    retire: Some(CapturedPointerConstraintCommit {
                        constraint_id: 22,
                        lifecycle: PointerConstraintLifecycleCommit::Remove,
                        region: PointerConstraintRegionCommit::NoChange,
                        cursor_position_hint: PointerConstraintHintCommit::NoChange,
                    }),
                    install_or_update: Some(CapturedPointerConstraintCommit {
                        constraint_id: 23,
                        lifecycle: PointerConstraintLifecycleCommit::Install,
                        region: PointerConstraintRegionCommit::Set(SurfaceInputRegion::Custom(
                            vec![InputRegionOp::Add(
                                InputRegionRect::new(5, 6, 7, 8).unwrap()
                            )],
                        )),
                        cursor_position_hint: PointerConstraintHintCommit::Set((9.0, 10.0)),
                    }),
                },
            )
        );
    }

    #[test]
    fn canceled_replacement_does_not_lose_old_retirement() {
        let mut cached = cached_commit_with_window_geometry(1, XdgWindowGeometry::new(1, 2, 3, 4));
        cached.pointer_constraint_state = pointer_state(
            22,
            PointerConstraintLifecycleCommit::Remove,
            PointerConstraintRegionCommit::NoChange,
            PointerConstraintHintCommit::NoChange,
        );
        let mut install = cached_commit_with_window_geometry(2, XdgWindowGeometry::new(1, 2, 3, 4));
        install.pointer_constraint_state = pointer_state(
            23,
            PointerConstraintLifecycleCommit::Install,
            PointerConstraintRegionCommit::Set(SurfaceInputRegion::Default),
            PointerConstraintHintCommit::Set((9.0, 10.0)),
        );
        cached.merge(install);
        let mut cancel = cached_commit_with_window_geometry(3, XdgWindowGeometry::new(1, 2, 3, 4));
        cancel.pointer_constraint_state = pointer_state(
            23,
            PointerConstraintLifecycleCommit::Cancel,
            PointerConstraintRegionCommit::NoChange,
            PointerConstraintHintCommit::NoChange,
        );

        cached.merge(cancel);

        assert_eq!(
            cached.pointer_constraint_state.into_transition(),
            Some(CapturedPointerConstraintSurfaceTransition {
                retire: Some(CapturedPointerConstraintCommit {
                    constraint_id: 22,
                    lifecycle: PointerConstraintLifecycleCommit::Remove,
                    region: PointerConstraintRegionCommit::NoChange,
                    cursor_position_hint: PointerConstraintHintCommit::NoChange,
                }),
                install_or_update: None,
            })
        );
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
    fn newer_pending_background_effect_replaces_older_pending_effect() {
        let mut cached = cached_commit_with_window_geometry(1, XdgWindowGeometry::new(1, 2, 3, 4));
        let first =
            BackgroundEffectRegion::from_surface_input_region(SurfaceInputRegion::Custom(vec![
                InputRegionOp::Add(InputRegionRect::new(1, 2, 3, 4).unwrap()),
            ]));
        let second =
            BackgroundEffectRegion::from_surface_input_region(SurfaceInputRegion::Custom(vec![
                InputRegionOp::Add(InputRegionRect::new(5, 6, 7, 8).unwrap()),
            ]));
        cached.background_effect = Some(first);
        let mut newer = cached_commit_with_window_geometry(2, XdgWindowGeometry::new(1, 2, 3, 4));
        newer.background_effect = Some(second.clone());

        cached.merge(newer);

        assert_eq!(cached.background_effect, Some(second));
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
            CapturedPointerConstraintSurfaceState::NoChange
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
    relationship_id: SubsurfaceRelationshipId,
    parent_id: u32,
    client_id: Option<ClientId>,
    requested_mode: SubsurfaceSyncMode,
    relationship_phase: SubsurfaceRelationshipPhase,
    cached_commits: VecDeque<CachedSubsurfaceCommit>,
    pending_position: Option<(i32, i32)>,
}

#[derive(Debug, Default)]
pub(super) struct SubsurfaceTransactionState {
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
}

impl SubsurfaceTransactionState {
    #[cfg(test)]
    pub(super) fn register(&mut self, surface_id: u32, parent_id: u32) -> bool {
        self.register_with_client(surface_id, parent_id, None)
    }

    pub(super) fn register_with_client(
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
                relationship_id: SubsurfaceRelationshipId(next_relationship_id),
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

    pub(super) fn remove_role(&mut self, surface_id: u32) -> Vec<CachedSubsurfaceCommit> {
        let Some(role) = self.roles.remove(&surface_id) else {
            return Vec::new();
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
        cached_commits
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

    pub(super) fn drain_cached_commits(&mut self) -> Vec<CachedSubsurfaceCommit> {
        let surface_ids = self.roles.keys().copied().collect::<Vec<_>>();
        let mut commits = Vec::new();
        for surface_id in surface_ids {
            commits.extend(self.take_cached_commits_for_surface(surface_id));
        }
        debug_assert!(self.debug_accounting_is_consistent());
        commits
    }

    pub(super) fn parent(&self, surface_id: u32) -> Option<u32> {
        self.roles.get(&surface_id).map(|role| role.parent_id)
    }

    pub(super) fn relationship_phase(
        &self,
        surface_id: u32,
    ) -> Option<SubsurfaceRelationshipPhase> {
        self.roles
            .get(&surface_id)
            .map(|role| role.relationship_phase)
    }

    pub(super) fn captured_relationship(
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

    pub(super) fn relationship_matches(
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

    pub(super) fn relationship_is_applied(
        &self,
        relationship: CapturedSubsurfaceRelationship,
    ) -> bool {
        self.relationship_matches(relationship)
            && self.relationship_phase(relationship.surface_id)
                == Some(SubsurfaceRelationshipPhase::Applied)
    }

    pub(super) fn relationship_is_registered_child_of(
        &self,
        surface_id: u32,
        parent_id: u32,
    ) -> bool {
        self.roles
            .get(&surface_id)
            .is_some_and(|role| role.parent_id == parent_id)
    }

    pub(super) fn relationship_is_applied_child_of(&self, surface_id: u32, parent_id: u32) -> bool {
        self.roles.get(&surface_id).is_some_and(|role| {
            role.parent_id == parent_id
                && role.relationship_phase == SubsurfaceRelationshipPhase::Applied
        })
    }

    pub(super) fn take_pending_relationship_activations_for_parent(
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

    pub(super) fn apply_captured_relationship(
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

    pub(super) fn applied_children_of(&self, parent_id: u32) -> Vec<u32> {
        self.roles
            .iter()
            .filter_map(|(surface_id, role)| {
                (role.parent_id == parent_id
                    && role.relationship_phase == SubsurfaceRelationshipPhase::Applied)
                    .then_some(*surface_id)
            })
            .collect()
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

    pub(super) fn client_id(&self, surface_id: u32) -> Option<&ClientId> {
        self.roles
            .get(&surface_id)
            .and_then(|role| role.client_id.as_ref())
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
    ) -> CacheCommitOutcome {
        debug_assert!(self.debug_accounting_is_consistent());
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
        let can_merge = role
            .cached_commits
            .back()
            .is_some_and(|tail| !tail.pacing.is_boundary() && !commit.pacing.is_boundary());
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

    pub(super) fn cached_pointer_constraint_hint(
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

    pub(super) fn cached_node_count(&self) -> usize {
        self.cached_nodes
    }

    pub(super) fn cached_entry_count(&self) -> usize {
        self.cached_entries_total
    }

    pub(super) fn cached_obligation_count(&self) -> usize {
        self.cached_obligations_total
    }

    pub(super) fn maximum_cached_entries(&self) -> usize {
        self.maximum_cached_entries
    }

    pub(super) fn maximum_cached_entries_per_surface(&self) -> usize {
        self.maximum_cached_entries_per_surface
    }

    pub(super) fn maximum_cached_entries_per_client(&self) -> usize {
        self.maximum_cached_entries_per_client
    }

    pub(super) fn maximum_cached_obligations(&self) -> usize {
        self.maximum_cached_obligations
    }

    pub(super) fn maximum_cached_obligations_per_surface(&self) -> usize {
        self.maximum_cached_obligations_per_surface
    }

    pub(super) fn maximum_cached_obligations_per_client(&self) -> usize {
        self.maximum_cached_obligations_per_client
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

    pub(super) fn capture_subsurface_stack(
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

    pub(super) fn take_latched_commits(
        &mut self,
        parent_id: u32,
    ) -> Vec<(u32, CachedSubsurfaceCommit)> {
        let mut surface_ids = Vec::new();
        self.collect_effectively_synchronized_descendants(parent_id, &mut surface_ids);
        let mut commits = Vec::new();
        for surface_id in surface_ids {
            commits.extend(
                self.take_cached_commits_for_surface(surface_id)
                    .into_iter()
                    .map(|commit| (surface_id, commit)),
            );
        }
        debug_assert!(self.debug_accounting_is_consistent());
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
            commits.extend(
                self.take_cached_commits_for_surface(surface_id)
                    .into_iter()
                    .map(|commit| (surface_id, commit)),
            );
        }
        debug_assert!(self.debug_accounting_is_consistent());
        commits
    }

    fn take_cached_commits_for_surface(&mut self, surface_id: u32) -> Vec<CachedSubsurfaceCommit> {
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

        assert!(matches!(
            state.cache_commit(2, first),
            CacheCommitOutcome::Inserted
        ));
        assert!(matches!(
            state.cache_commit(2, second),
            CacheCommitOutcome::Inserted
        ));
        assert_eq!(state.roles[&2].cached_commits.len(), 2);
        assert!(state.roles[&2].cached_commits[0].pacing.fifo_set_barrier);
        assert!(state.roles[&2].cached_commits[1].pacing.fifo_wait_barrier);
    }
}

#[cfg(test)]
#[path = "subsurface_cache_tests.rs"]
mod cache_limit_tests;
