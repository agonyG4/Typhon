use std::time::Instant;

use wayland_protocols::wp::viewporter::server::wp_viewport;
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
use crate::compositor::decoration::types::CapturedXdgDecorationCommit;
use crate::compositor::layer_shell::CapturedLayerSurfaceCommitState;
use crate::effects::EffectCoverage;

// An obligation is one retained frame callback, presentation feedback, buffer
// ownership slot (plus one slot per validated DMA-BUF plane), or explicit-sync
// acquire/release point. This is a cardinality guard, not a byte or GPU-memory
// estimate. A merged entry is checked against the same limits because callbacks
// can accumulate even while the VecDeque length stays constant.

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

impl SubsurfaceRelationshipId {
    pub(super) const fn new(value: u64) -> Self {
        Self(value)
    }

    pub(super) const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ContentUpdateRef {
    pub(super) surface_id: u32,
    pub(super) commit_id: SurfaceCommitId,
    pub(super) commit_sequence: SurfaceCommitSequence,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CapturedContentUpdateLineage {
    pub(super) predecessor: Option<ContentUpdateRef>,
    pub(super) child_dependencies: Vec<ContentUpdateRef>,
    pub(super) merge_frozen: bool,
}

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
    pub(super) xdg_decoration: Option<CapturedXdgDecorationCommit>,
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
        // Decoration transitions are commit obligations. A later coalesced
        // commit with no transition leaves the older state in force; a newer
        // captured transition replaces it chronologically.
        if newer.xdg_decoration.is_some() {
            self.xdg_decoration = newer.xdg_decoration;
        }
    }
}

#[derive(Debug)]
pub(super) struct CachedSubsurfaceCommit {
    pub(super) commit_id: SurfaceCommitId,
    pub(super) commit_sequence: SurfaceCommitSequence,
    pub(super) lineage: CapturedContentUpdateLineage,
    pub(super) attachment: Option<PendingSurfaceAttachment>,
    pub(super) damage: Option<RenderableSurfaceDamage>,
    pub(super) frame_callbacks: Vec<wl_callback::WlCallback>,
    pub(super) explicit_sync: Option<CapturedExplicitSyncState>,
    pub(super) offset: Option<(i32, i32)>,
    pub(super) viewport_destination: PendingViewportChange,
    /// The resource that authored the effective source state represented by
    /// this cached commit. This is an attribution token, not viewport state:
    /// it may become dead before synchronized publication.
    pub(super) viewport_error_owner: Option<wp_viewport::WpViewport>,
    pub(super) buffer_scale: Option<u32>,
    pub(super) buffer_transform: Option<wl_output::Transform>,
    pub(super) opaque_region: Option<SurfaceInputRegion>,
    pub(super) input_region: Option<SurfaceInputRegion>,
    pub(super) background_effect: Option<BackgroundEffectRegion>,
    /// Outer `None` is no mutation; `Some(None)` queues a coverage clear.
    pub(super) background_effect_coverage: Option<Option<EffectCoverage>>,
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
    pub(super) fn content_update_ref(&self, surface_id: u32) -> ContentUpdateRef {
        ContentUpdateRef {
            surface_id,
            commit_id: self.commit_id,
            commit_sequence: self.commit_sequence,
        }
    }

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

    pub(super) fn merged_cached_obligation_count(&self, newer: &Self) -> usize {
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
            lineage: newer_lineage,
            attachment,
            damage,
            frame_callbacks,
            explicit_sync,
            offset,
            viewport_destination,
            viewport_error_owner,
            buffer_scale,
            buffer_transform,
            opaque_region,
            input_region,
            background_effect,
            background_effect_coverage,
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
        let mut lineage = std::mem::take(&mut self.lineage);
        for dependency in newer_lineage.child_dependencies {
            if !lineage.child_dependencies.contains(&dependency) {
                lineage.child_dependencies.push(dependency);
            }
        }
        lineage.merge_frozen |= newer_lineage.merge_frozen;
        self.lineage = lineage;
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
        // Source is the only viewport field that can produce a mapping
        // protocol error. A destination-only delta therefore keeps the
        // source author's identity; a source delta (including an explicit
        // reset from viewport destruction) replaces it deterministically.
        let source_changed = viewport_destination.source.is_some();
        self.viewport_destination.merge(viewport_destination);
        if source_changed {
            self.viewport_error_owner = viewport_error_owner;
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
        if background_effect_coverage.is_some() {
            self.background_effect_coverage = background_effect_coverage;
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

#[cfg(test)]
mod commit_context_tests {
    use super::*;
    use crate::compositor::decoration::types::{
        CapturedXdgDecorationCommitState, ConfiguredXdgDecorationState, DecorationMode,
        DecorationObjectGeneration,
    };

    fn relationship(surface_id: u32, relationship_id: u64) -> CapturedSubsurfaceRelationship {
        CapturedSubsurfaceRelationship {
            surface_id,
            parent_id: 1,
            relationship_id: SubsurfaceRelationshipId(relationship_id),
        }
    }

    fn configured(generation: u64, mode: DecorationMode) -> CapturedXdgDecorationCommit {
        CapturedXdgDecorationCommit {
            state: CapturedXdgDecorationCommitState::Configured(ConfiguredXdgDecorationState {
                generation: DecorationObjectGeneration(generation),
                mode,
            }),
            commit_sequence: SurfaceCommitSequence(generation),
        }
    }

    #[test]
    fn captured_context_merge_preserves_older_decoration_when_newer_has_none() {
        let mut older = CapturedSurfaceCommitContext {
            xdg_decoration: Some(configured(1, DecorationMode::ServerSide)),
            ..CapturedSurfaceCommitContext::default()
        };
        older.merge(CapturedSurfaceCommitContext::default());
        assert_eq!(
            older.xdg_decoration,
            Some(configured(1, DecorationMode::ServerSide))
        );
    }

    #[test]
    fn captured_context_merge_uses_newer_decoration_transition() {
        let mut older = CapturedSurfaceCommitContext {
            xdg_decoration: Some(configured(1, DecorationMode::ServerSide)),
            ..CapturedSurfaceCommitContext::default()
        };
        older.merge(CapturedSurfaceCommitContext {
            xdg_decoration: Some(configured(2, DecorationMode::ClientSide)),
            ..CapturedSurfaceCommitContext::default()
        });
        assert_eq!(
            older.xdg_decoration,
            Some(configured(2, DecorationMode::ClientSide))
        );
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
            xdg_decoration: None,
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
            xdg_decoration: None,
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
            xdg_decoration: None,
        };
        let newer = CapturedSurfaceCommitContext {
            subsurface_parent: CapturedSubsurfaceParentState {
                activations: vec![relationship(10, 1), relationship(20, 2)],
                ..CapturedSubsurfaceParentState::default()
            },
            layer_surface: None,
            xdg_decoration: None,
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
            xdg_decoration: None,
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
            xdg_decoration: None,
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
mod pending_viewport_change_tests {
    use super::*;
    use crate::compositor::state_data::ViewportSourceRect;
    use crate::render_backend::buffer::BufferSize;

    fn source(x: f64, y: f64, width: f64, height: f64) -> ViewportSourceRect {
        ViewportSourceRect::new(x, y, width, height).expect("valid viewport source")
    }

    fn destination(width: u32, height: u32) -> BufferSize {
        BufferSize::new(width, height).expect("valid viewport destination")
    }

    fn merge_viewport_changes(
        mut older: PendingViewportChange,
        newer: PendingViewportChange,
    ) -> PendingViewportChange {
        older.merge(newer);
        older
    }

    #[test]
    fn older_source_and_newer_destination_preserve_both_changes() {
        let older = PendingViewportChange {
            source: Some(Some(source(1.0, 2.0, 3.0, 4.0))),
            destination: None,
        };
        let newer = PendingViewportChange {
            source: None,
            destination: Some(Some(destination(5, 6))),
        };

        assert_eq!(
            merge_viewport_changes(older, newer),
            PendingViewportChange {
                source: Some(Some(source(1.0, 2.0, 3.0, 4.0))),
                destination: Some(Some(destination(5, 6))),
            }
        );
    }

    #[test]
    fn older_destination_and_newer_source_preserve_both_changes() {
        let older = PendingViewportChange {
            source: None,
            destination: Some(Some(destination(5, 6))),
        };
        let newer = PendingViewportChange {
            source: Some(Some(source(1.0, 2.0, 3.0, 4.0))),
            destination: None,
        };

        assert_eq!(
            merge_viewport_changes(older, newer),
            PendingViewportChange {
                source: Some(Some(source(1.0, 2.0, 3.0, 4.0))),
                destination: Some(Some(destination(5, 6))),
            }
        );
    }

    #[test]
    fn newer_source_replaces_older_source_only() {
        let older = PendingViewportChange {
            source: Some(Some(source(1.0, 2.0, 3.0, 4.0))),
            destination: Some(Some(destination(5, 6))),
        };
        let newer = PendingViewportChange {
            source: Some(Some(source(7.0, 8.0, 9.0, 10.0))),
            destination: None,
        };

        assert_eq!(
            merge_viewport_changes(older, newer),
            PendingViewportChange {
                source: Some(Some(source(7.0, 8.0, 9.0, 10.0))),
                destination: Some(Some(destination(5, 6))),
            }
        );
    }

    #[test]
    fn newer_destination_replaces_older_destination_only() {
        let older = PendingViewportChange {
            source: Some(Some(source(1.0, 2.0, 3.0, 4.0))),
            destination: Some(Some(destination(5, 6))),
        };
        let newer = PendingViewportChange {
            source: None,
            destination: Some(Some(destination(7, 8))),
        };

        assert_eq!(
            merge_viewport_changes(older, newer),
            PendingViewportChange {
                source: Some(Some(source(1.0, 2.0, 3.0, 4.0))),
                destination: Some(Some(destination(7, 8))),
            }
        );
    }

    #[test]
    fn newer_source_reset_replaces_older_source_only() {
        let older = PendingViewportChange {
            source: Some(Some(source(1.0, 2.0, 3.0, 4.0))),
            destination: Some(Some(destination(5, 6))),
        };
        let newer = PendingViewportChange {
            source: Some(None),
            destination: None,
        };

        assert_eq!(
            merge_viewport_changes(older, newer),
            PendingViewportChange {
                source: Some(None),
                destination: Some(Some(destination(5, 6))),
            }
        );
    }

    #[test]
    fn newer_destination_reset_replaces_older_destination_only() {
        let older = PendingViewportChange {
            source: Some(Some(source(1.0, 2.0, 3.0, 4.0))),
            destination: Some(Some(destination(5, 6))),
        };
        let newer = PendingViewportChange {
            source: None,
            destination: Some(None),
        };

        assert_eq!(
            merge_viewport_changes(older, newer),
            PendingViewportChange {
                source: Some(Some(source(1.0, 2.0, 3.0, 4.0))),
                destination: Some(None),
            }
        );
    }

    #[test]
    fn older_source_reset_survives_newer_destination_change() {
        let older = PendingViewportChange {
            source: Some(None),
            destination: None,
        };
        let newer = PendingViewportChange {
            source: None,
            destination: Some(Some(destination(5, 6))),
        };

        assert_eq!(
            merge_viewport_changes(older, newer),
            PendingViewportChange {
                source: Some(None),
                destination: Some(Some(destination(5, 6))),
            }
        );
    }

    #[test]
    fn older_destination_reset_survives_newer_source_change() {
        let older = PendingViewportChange {
            source: None,
            destination: Some(None),
        };
        let newer = PendingViewportChange {
            source: Some(Some(source(1.0, 2.0, 3.0, 4.0))),
            destination: None,
        };

        assert_eq!(
            merge_viewport_changes(older, newer),
            PendingViewportChange {
                source: Some(Some(source(1.0, 2.0, 3.0, 4.0))),
                destination: Some(None),
            }
        );
    }

    #[test]
    fn newer_unchanged_viewport_preserves_older_changes() {
        let older = PendingViewportChange {
            source: Some(Some(source(1.0, 2.0, 3.0, 4.0))),
            destination: Some(Some(destination(5, 6))),
        };

        assert_eq!(
            merge_viewport_changes(older, PendingViewportChange::default()),
            older
        );
    }

    #[test]
    fn older_unchanged_viewport_accepts_newer_changes() {
        let newer = PendingViewportChange {
            source: Some(Some(source(1.0, 2.0, 3.0, 4.0))),
            destination: Some(Some(destination(5, 6))),
        };

        assert_eq!(
            merge_viewport_changes(PendingViewportChange::default(), newer),
            newer
        );
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
            lineage: CapturedContentUpdateLineage::default(),
            attachment: None,
            damage: None,
            frame_callbacks: Vec::new(),
            explicit_sync: None,
            offset: None,
            viewport_destination: PendingViewportChange::default(),
            viewport_error_owner: None,
            buffer_scale: None,
            buffer_transform: None,
            opaque_region: None,
            input_region: None,
            background_effect: None,
            background_effect_coverage: None,
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
    fn cached_coverage_merge_replaces_only_with_explicit_newer_change() {
        let shape = EffectCoverage {
            rounded_rect: Some(crate::effects::EffectCoverageRoundedRect {
                x: 1.25,
                y: 2.5,
                width: 72.0,
                height: 34.0,
                radius: 17.0,
            }),
            triangle: None,
        };
        let mut cached = cached_commit_with_window_geometry(1, XdgWindowGeometry::new(1, 2, 3, 4));
        cached.background_effect_coverage = Some(Some(shape.clone()));
        let unchanged = cached_commit_with_window_geometry(2, XdgWindowGeometry::new(1, 2, 3, 4));
        cached.merge(unchanged);
        assert_eq!(cached.background_effect_coverage, Some(Some(shape.clone())));

        let mut clear = cached_commit_with_window_geometry(3, XdgWindowGeometry::new(1, 2, 3, 4));
        clear.background_effect_coverage = Some(None);
        cached.merge(clear);
        assert_eq!(cached.background_effect_coverage, Some(None));
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
