//! Sparse SceneNode-owned geometry presentation engine.

use std::{
    cell::Cell,
    collections::{BTreeMap, HashSet},
    num::NonZeroU64,
};

use crate::core::{OutputId, SceneNodeId};

use super::{
    AnimationCurve, AnimationTime, PresentationClip, PresentationClipRect,
    PresentationClipTransitionEvidence, PresentationGeometryTransform, PresentationGroupClip,
    PresentationGroupOpacity, PresentationGroupTransform, PresentationOpacity,
    PresentationOpacityTransitionEvidence, PresentationPropertyKind, PresentationRect,
    PresentationRevisionId, PresentationSampleTimeSource, PresentationSceneSample,
    PresentationTransactionError, PresentationTransactionId, PresentationTransactionMember,
    PresentationTransactionRecord, PresentationTransactionRequest, PresentationVelocity,
    PresentationWindowSample, PresentationWindowTarget, PresentedClipAck, PresentedGeometryAck,
    PresentedOpacityAck,
};

#[cfg(test)]
use super::PresentationGeometryMutation;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationTransition {
    scene_node_id: SceneNodeId,
    transaction_id: PresentationTransactionId,
    revision_id: PresentationRevisionId,
    start: PresentationRect,
    target: PresentationRect,
    start_velocity: PresentationVelocity,
    started_at: AnimationTime,
    curve: AnimationCurve,
    preserve_start_velocity: bool,
}

impl PresentationTransition {
    pub const fn new(
        start: PresentationRect,
        target: PresentationRect,
        started_at: AnimationTime,
        curve: AnimationCurve,
    ) -> Self {
        let mut transition = Self::new_with_velocity(
            start,
            target,
            PresentationVelocity::new(0.0, 0.0, 0.0, 0.0),
            started_at,
            curve,
        );
        transition.preserve_start_velocity = false;
        transition
    }

    pub const fn new_with_velocity(
        start: PresentationRect,
        target: PresentationRect,
        start_velocity: PresentationVelocity,
        started_at: AnimationTime,
        curve: AnimationCurve,
    ) -> Self {
        let identity = PresentationRevisionId::new(NonZeroU64::MIN);
        Self {
            scene_node_id: synthetic_scene_node_id(),
            transaction_id: PresentationTransactionId::new(NonZeroU64::MIN),
            revision_id: identity,
            start,
            target,
            start_velocity,
            started_at,
            curve,
            preserve_start_velocity: true,
        }
    }

    pub const fn target(self) -> PresentationRect {
        self.target
    }

    pub const fn curve(self) -> AnimationCurve {
        self.curve
    }

    pub const fn started_at(self) -> AnimationTime {
        self.started_at
    }

    pub fn sample(self, now: AnimationTime) -> PresentationWindowSample {
        let elapsed = now.elapsed_seconds(self.started_at);
        let starts = [
            self.start.x(),
            self.start.y(),
            self.start.width(),
            self.start.height(),
        ];
        let targets = [
            self.target.x(),
            self.target.y(),
            self.target.width(),
            self.target.height(),
        ];
        let mut values = [0.0; 4];
        let mut velocities = [0.0; 4];
        let mut settled = true;
        for index in 0..4 {
            let (value, velocity, component_settled) = self.curve.sample_scalar(
                starts[index],
                targets[index],
                self.start_velocity.component(index),
                elapsed,
                self.preserve_start_velocity,
            );
            values[index] = value;
            velocities[index] = velocity;
            settled &= component_settled;
        }
        PresentationWindowSample {
            scene_node_id: self.scene_node_id,
            key: 0,
            rect: PresentationRect::new(
                values[0],
                values[1],
                values[2].max(f64::MIN_POSITIVE),
                values[3].max(f64::MIN_POSITIVE),
            )
            .expect("analytical geometry sample remains finite and positive"),
            velocity: PresentationVelocity::new(
                velocities[0],
                velocities[1],
                velocities[2],
                velocities[3],
            ),
            transaction_id: self.transaction_id,
            revision_id: self.revision_id,
            transition_id: self.revision_id,
            mathematically_settled: settled,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PresentationAnimationMetrics {
    pub transitions_started: u64,
    pub transitions_retargeted: u64,
    pub transitions_cancelled: u64,
    pub transitions_acknowledged: u64,
    pub stale_acknowledgements: u64,
    pub sampled_windows: u64,
    pub transactions_committed: u64,
    pub transaction_members: u64,
    pub geometry_starts: u64,
    pub geometry_retargets: u64,
    pub geometry_cancels: u64,
    pub opacity_starts: u64,
    pub opacity_retargets: u64,
    pub opacity_cancels: u64,
    pub clip_starts: u64,
    pub clip_retargets: u64,
    pub clip_cancels: u64,
    pub active_tracks: u64,
    pub mathematical_settlements: u64,
    pub physical_revision_acks: u64,
    pub stale_revision_acks: u64,
    pub wrong_output_acks: u64,
    pub frame_samples: u64,
    pub scheduled_target_samples: u64,
    pub monotonic_fallback_samples: u64,
    pub zero_fallback_samples: u64,
}

#[derive(Debug)]
struct GeometryTrack {
    transition: PresentationTransition,
}

#[derive(Debug, Clone, Copy)]
struct OpacityTrack {
    scene_node_id: SceneNodeId,
    transaction_id: PresentationTransactionId,
    revision_id: PresentationRevisionId,
    start: PresentationOpacity,
    target: PresentationOpacity,
    start_velocity: f64,
    started_at: AnimationTime,
    curve: AnimationCurve,
    preserve_start_velocity: bool,
}

#[derive(Debug, Clone, Copy)]
struct ClipTrack {
    scene_node_id: SceneNodeId,
    transaction_id: PresentationTransactionId,
    revision_id: PresentationRevisionId,
    start: PresentationClip,
    target: PresentationClip,
    start_rect: PresentationClipRect,
    target_rect: PresentationClipRect,
    start_velocity: PresentationVelocity,
    started_at: AnimationTime,
    curve: AnimationCurve,
    preserve_start_velocity: bool,
}

#[derive(Debug, Clone, Copy)]
struct PresentationClipSample {
    clip: PresentationClip,
    rect: PresentationClipRect,
    velocity: PresentationVelocity,
    mathematically_settled: bool,
}

impl ClipTrack {
    fn sample(self, now: AnimationTime) -> PresentationClipSample {
        if now <= self.started_at {
            return PresentationClipSample {
                clip: self.start,
                rect: self.start_rect,
                velocity: self.start_velocity,
                mathematically_settled: false,
            };
        }
        let elapsed = now.elapsed_seconds(self.started_at);
        let starts = [
            self.start_rect.x(),
            self.start_rect.y(),
            self.start_rect.width(),
            self.start_rect.height(),
        ];
        let targets = [
            self.target_rect.x(),
            self.target_rect.y(),
            self.target_rect.width(),
            self.target_rect.height(),
        ];
        let mut values = [0.0; 4];
        let mut velocities = [0.0; 4];
        let mut settled = true;
        for index in 0..4 {
            let (raw, velocity, component_settled) = self.curve.sample_scalar(
                starts[index],
                targets[index],
                self.start_velocity.component(index),
                elapsed,
                self.preserve_start_velocity,
            );
            let lower_bounded = index >= 2 && raw < 0.0;
            values[index] = if lower_bounded { 0.0 } else { raw };
            velocities[index] = if lower_bounded { 0.0 } else { velocity };
            settled &= component_settled;
        }
        let rect = PresentationClipRect::new(values[0], values[1], values[2], values[3])
            .expect("finite analytical Clip sample remains valid");
        PresentationClipSample {
            clip: if settled {
                self.target
            } else {
                PresentationClip::Rect(rect)
            },
            rect,
            velocity: PresentationVelocity::new(
                velocities[0],
                velocities[1],
                velocities[2],
                velocities[3],
            ),
            mathematically_settled: settled,
        }
    }
}

impl OpacityTrack {
    fn sample(self, now: AnimationTime) -> (PresentationOpacity, f64, bool) {
        let elapsed = now.elapsed_seconds(self.started_at);
        let (raw, raw_velocity, settled) = self.curve.sample_scalar(
            self.start.get(),
            self.target.get(),
            self.start_velocity,
            elapsed,
            self.preserve_start_velocity,
        );
        let visible = raw.clamp(0.0, 1.0);
        let visible_velocity = if raw == visible { raw_velocity } else { 0.0 };
        (
            PresentationOpacity::new(visible).expect("opacity samples are clamped and finite"),
            visible_velocity,
            settled,
        )
    }
}

#[derive(Debug)]
pub struct PresentationEngine {
    enabled: bool,
    geometry_tracks: BTreeMap<SceneNodeId, GeometryTrack>,
    opacity_tracks: BTreeMap<SceneNodeId, OpacityTrack>,
    clip_tracks: BTreeMap<SceneNodeId, ClipTrack>,
    transactions: BTreeMap<PresentationTransactionId, PresentationTransactionRecord>,
    next_transaction_id: NonZeroU64,
    next_revision_id: NonZeroU64,
    transaction_ids_exhausted: bool,
    revision_ids_exhausted: bool,
    metrics: PresentationAnimationMetrics,
    sampled_windows: Cell<u64>,
    sample_metrics: Cell<PresentationAnimationMetrics>,
}

impl Default for PresentationEngine {
    fn default() -> Self {
        if animation_policy_enabled(std::env::var("OBLIVION_ONE_ANIMATIONS").ok().as_deref()) {
            Self::enabled()
        } else {
            Self::disabled()
        }
    }
}

pub fn animation_policy_enabled(value: Option<&str>) -> bool {
    match value {
        None | Some("on") => true,
        Some("off") => false,
        Some(value) => {
            eprintln!(
                "oblivion-one presentation animation: unknown OBLIVION_ONE_ANIMATIONS={value:?}; using normal policy"
            );
            true
        }
    }
}

impl PresentationEngine {
    pub fn enabled() -> Self {
        Self::new(true)
    }

    pub fn disabled() -> Self {
        Self::new(false)
    }

    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            geometry_tracks: BTreeMap::new(),
            opacity_tracks: BTreeMap::new(),
            clip_tracks: BTreeMap::new(),
            transactions: BTreeMap::new(),
            next_transaction_id: NonZeroU64::MIN,
            next_revision_id: NonZeroU64::MIN,
            transaction_ids_exhausted: false,
            revision_ids_exhausted: false,
            metrics: PresentationAnimationMetrics::default(),
            sampled_windows: Cell::new(0),
            sample_metrics: Cell::new(PresentationAnimationMetrics::default()),
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            let removed = self.active_count() as u64;
            self.metrics.transitions_cancelled =
                self.metrics.transitions_cancelled.saturating_add(removed);
            self.metrics.geometry_cancels = self
                .metrics
                .geometry_cancels
                .saturating_add(self.geometry_tracks.len() as u64);
            self.metrics.opacity_cancels = self
                .metrics
                .opacity_cancels
                .saturating_add(self.opacity_tracks.len() as u64);
            self.metrics.clip_cancels = self
                .metrics
                .clip_cancels
                .saturating_add(self.clip_tracks.len() as u64);
            self.geometry_tracks.clear();
            self.opacity_tracks.clear();
            self.clip_tracks.clear();
            self.transactions.clear();
        }
    }

    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn commit(
        &mut self,
        request: PresentationTransactionRequest,
    ) -> Result<PresentationTransactionRecord, PresentationTransactionError> {
        if !self.enabled {
            return Err(PresentationTransactionError::Disabled);
        }
        if request.geometry.is_empty() && request.opacity.is_empty() && request.clip.is_empty() {
            return Err(PresentationTransactionError::Empty);
        }

        let mut seen = HashSet::with_capacity(
            request.geometry.len() + request.opacity.len() + request.clip.len(),
        );
        let mut prepared_geometry = Vec::with_capacity(request.geometry.len());
        for mutation in request.geometry {
            let Some(scene_node_id) = mutation.scene_node_id() else {
                return Err(PresentationTransactionError::MissingPresentationOwner);
            };
            if !seen.insert((scene_node_id, PresentationPropertyKind::Geometry)) {
                return Err(PresentationTransactionError::DuplicateProperty);
            }
            if !valid_rect(mutation.start) || !valid_rect(mutation.target) {
                return Err(PresentationTransactionError::InvalidGeometry);
            }
            let (start, start_velocity, preserve_start_velocity) = if let Some(track) =
                self.geometry_tracks.get(&scene_node_id)
            {
                let sample = track.transition.sample(request.started_at);
                if sample.mathematically_settled && sample.rect.is_identity_with(mutation.target) {
                    continue;
                }
                (sample.rect, sample.velocity, true)
            } else {
                if mutation.start.is_identity_with(mutation.target) {
                    continue;
                }
                (mutation.start, PresentationVelocity::default(), false)
            };
            prepared_geometry.push(super::PreparedGeometryMutation {
                scene_node_id,
                start,
                target: mutation.target,
                start_velocity,
                curve: mutation.curve,
                preserve_start_velocity,
            });
        }

        let mut prepared_opacity = Vec::with_capacity(request.opacity.len());
        for mutation in request.opacity {
            let Some(scene_node_id) = mutation.scene_node_id() else {
                return Err(PresentationTransactionError::MissingPresentationOwner);
            };
            if !seen.insert((scene_node_id, PresentationPropertyKind::Opacity)) {
                return Err(PresentationTransactionError::DuplicateProperty);
            }
            let (start, start_velocity, preserve_start_velocity) = if let Some(track) =
                self.opacity_tracks.get(&scene_node_id)
            {
                let (sample, velocity, mathematically_settled) = track.sample(request.started_at);
                if mathematically_settled && sample == mutation.target {
                    continue;
                }
                (sample, velocity, true)
            } else {
                if mutation.start == mutation.target {
                    continue;
                }
                (mutation.start, 0.0, false)
            };
            prepared_opacity.push(super::PreparedOpacityMutation {
                scene_node_id,
                start,
                target: mutation.target,
                start_velocity,
                curve: mutation.curve,
                preserve_start_velocity,
            });
        }

        let mut prepared_clip = Vec::with_capacity(request.clip.len());
        for mutation in request.clip {
            let Some(scene_node_id) = mutation.scene_node_id() else {
                return Err(PresentationTransactionError::MissingPresentationOwner);
            };
            if !seen.insert((scene_node_id, PresentationPropertyKind::Clip)) {
                return Err(PresentationTransactionError::DuplicateProperty);
            }
            if !valid_clip(mutation.start) || !valid_clip(mutation.target) {
                return Err(PresentationTransactionError::InvalidClip);
            }
            if mutation.start == mutation.target && !self.clip_tracks.contains_key(&scene_node_id) {
                continue;
            }
            let (start, start_rect, start_velocity, preserve_start_velocity) =
                if let Some(track) = self.clip_tracks.get(&scene_node_id).copied() {
                    let sample = track.sample(request.started_at);
                    if sample.mathematically_settled && sample.clip == mutation.target {
                        continue;
                    }
                    (sample.clip, sample.rect, sample.velocity, true)
                } else {
                    let start_rect =
                        resolve_clip_rect(mutation.start, mutation.frozen_identity_envelope)?;
                    (
                        mutation.start,
                        start_rect,
                        PresentationVelocity::default(),
                        false,
                    )
                };
            let target_rect =
                resolve_clip_rect(mutation.target, mutation.frozen_identity_envelope)?;
            prepared_clip.push(super::PreparedClipMutation {
                scene_node_id,
                start,
                target: mutation.target,
                start_rect,
                target_rect,
                start_velocity,
                curve: mutation.curve,
                preserve_start_velocity,
            });
        }

        if prepared_geometry.is_empty() && prepared_opacity.is_empty() && prepared_clip.is_empty() {
            return Err(PresentationTransactionError::Empty);
        }

        let transaction_id = self.peek_transaction_id()?;
        let geometry_count = prepared_geometry.len();
        let opacity_count = prepared_opacity.len();
        let member_count = geometry_count + opacity_count + prepared_clip.len();
        let revision_ids = self.peek_revision_ids(member_count)?;
        let mut revision_ids_iter = revision_ids.iter().copied();
        let mut members = Vec::with_capacity(member_count);
        for mutation in &prepared_geometry {
            members.push(PresentationTransactionMember::new(
                mutation.scene_node_id,
                PresentationPropertyKind::Geometry,
                transaction_id,
                revision_ids_iter.next().expect("revision count matches"),
            ));
        }
        for mutation in &prepared_opacity {
            members.push(PresentationTransactionMember::new(
                mutation.scene_node_id,
                PresentationPropertyKind::Opacity,
                transaction_id,
                revision_ids_iter.next().expect("revision count matches"),
            ));
        }
        for mutation in &prepared_clip {
            members.push(PresentationTransactionMember::new(
                mutation.scene_node_id,
                PresentationPropertyKind::Clip,
                transaction_id,
                revision_ids_iter.next().expect("revision count matches"),
            ));
        }
        let record =
            PresentationTransactionRecord::new(transaction_id, request.started_at, members);

        if transaction_id.get() == u64::MAX {
            self.transaction_ids_exhausted = true;
        } else {
            self.next_transaction_id =
                NonZeroU64::new(transaction_id.get() + 1).expect("increment remains nonzero");
        }
        let last_revision = revision_ids
            .last()
            .expect("prepared transaction is nonempty")
            .get();
        if last_revision == u64::MAX {
            self.revision_ids_exhausted = true;
        } else {
            self.next_revision_id =
                NonZeroU64::new(last_revision + 1).expect("increment remains nonzero");
        }

        for (mutation, revision_id) in prepared_geometry
            .into_iter()
            .zip(revision_ids.iter().copied())
        {
            if let Some(old) = self.geometry_tracks.get(&mutation.scene_node_id) {
                self.remove_transaction_member(
                    old.transition.transaction_id,
                    mutation.scene_node_id,
                    PresentationPropertyKind::Geometry,
                    old.transition.revision_id,
                );
            }
            let transition = PresentationTransition {
                scene_node_id: mutation.scene_node_id,
                transaction_id,
                revision_id,
                start: mutation.start,
                target: mutation.target,
                start_velocity: mutation.start_velocity,
                started_at: request.started_at,
                curve: mutation.curve,
                preserve_start_velocity: mutation.preserve_start_velocity,
            };
            let was_active = self
                .geometry_tracks
                .insert(mutation.scene_node_id, GeometryTrack { transition });
            if was_active.is_some() {
                self.metrics.transitions_retargeted =
                    self.metrics.transitions_retargeted.saturating_add(1);
                self.metrics.geometry_retargets = self.metrics.geometry_retargets.saturating_add(1);
            } else {
                self.metrics.transitions_started =
                    self.metrics.transitions_started.saturating_add(1);
                self.metrics.geometry_starts = self.metrics.geometry_starts.saturating_add(1);
            }
        }
        for (mutation, revision_id) in prepared_opacity
            .into_iter()
            .zip(revision_ids.iter().copied().skip(geometry_count))
        {
            if let Some(old) = self.opacity_tracks.get(&mutation.scene_node_id) {
                self.remove_transaction_member(
                    old.transaction_id,
                    mutation.scene_node_id,
                    PresentationPropertyKind::Opacity,
                    old.revision_id,
                );
            }
            let track = OpacityTrack {
                scene_node_id: mutation.scene_node_id,
                transaction_id,
                revision_id,
                start: mutation.start,
                target: mutation.target,
                start_velocity: mutation.start_velocity,
                started_at: request.started_at,
                curve: mutation.curve,
                preserve_start_velocity: mutation.preserve_start_velocity,
            };
            if self
                .opacity_tracks
                .insert(mutation.scene_node_id, track)
                .is_some()
            {
                self.metrics.transitions_retargeted =
                    self.metrics.transitions_retargeted.saturating_add(1);
                self.metrics.opacity_retargets = self.metrics.opacity_retargets.saturating_add(1);
            } else {
                self.metrics.transitions_started =
                    self.metrics.transitions_started.saturating_add(1);
                self.metrics.opacity_starts = self.metrics.opacity_starts.saturating_add(1);
            }
        }
        for (mutation, revision_id) in prepared_clip.into_iter().zip(
            revision_ids
                .iter()
                .copied()
                .skip(geometry_count + opacity_count),
        ) {
            if let Some(old) = self.clip_tracks.get(&mutation.scene_node_id) {
                self.remove_transaction_member(
                    old.transaction_id,
                    mutation.scene_node_id,
                    PresentationPropertyKind::Clip,
                    old.revision_id,
                );
            }
            let track = ClipTrack {
                scene_node_id: mutation.scene_node_id,
                transaction_id,
                revision_id,
                start: mutation.start,
                target: mutation.target,
                start_rect: mutation.start_rect,
                target_rect: mutation.target_rect,
                start_velocity: mutation.start_velocity,
                started_at: request.started_at,
                curve: mutation.curve,
                preserve_start_velocity: mutation.preserve_start_velocity,
            };
            if self
                .clip_tracks
                .insert(mutation.scene_node_id, track)
                .is_some()
            {
                self.metrics.transitions_retargeted =
                    self.metrics.transitions_retargeted.saturating_add(1);
                self.metrics.clip_retargets = self.metrics.clip_retargets.saturating_add(1);
            } else {
                self.metrics.transitions_started =
                    self.metrics.transitions_started.saturating_add(1);
                self.metrics.clip_starts = self.metrics.clip_starts.saturating_add(1);
            }
        }
        self.transactions.insert(transaction_id, record.clone());
        self.metrics.transactions_committed = self.metrics.transactions_committed.saturating_add(1);
        self.metrics.transaction_members = self
            .metrics
            .transaction_members
            .saturating_add(record.members().len() as u64);
        self.metrics.active_tracks = self.active_count() as u64;
        Ok(record)
    }

    pub(crate) fn cancel_geometry(&mut self, scene_node_id: SceneNodeId) {
        let Some(track) = self.geometry_tracks.remove(&scene_node_id) else {
            return;
        };
        self.remove_transaction_member(
            track.transition.transaction_id,
            scene_node_id,
            PresentationPropertyKind::Geometry,
            track.transition.revision_id,
        );
        self.metrics.transitions_cancelled = self.metrics.transitions_cancelled.saturating_add(1);
        self.metrics.geometry_cancels = self.metrics.geometry_cancels.saturating_add(1);
        self.metrics.active_tracks = self.active_count() as u64;
    }

    pub(crate) fn cancel_opacity(&mut self, scene_node_id: SceneNodeId) {
        let Some(track) = self.opacity_tracks.remove(&scene_node_id) else {
            return;
        };
        self.remove_transaction_member(
            track.transaction_id,
            scene_node_id,
            PresentationPropertyKind::Opacity,
            track.revision_id,
        );
        self.metrics.transitions_cancelled = self.metrics.transitions_cancelled.saturating_add(1);
        self.metrics.opacity_cancels = self.metrics.opacity_cancels.saturating_add(1);
        self.metrics.active_tracks = self.active_count() as u64;
    }

    pub(crate) fn cancel_clip(&mut self, scene_node_id: SceneNodeId) {
        let Some(track) = self.clip_tracks.remove(&scene_node_id) else {
            return;
        };
        self.remove_transaction_member(
            track.transaction_id,
            scene_node_id,
            PresentationPropertyKind::Clip,
            track.revision_id,
        );
        self.metrics.transitions_cancelled = self.metrics.transitions_cancelled.saturating_add(1);
        self.metrics.clip_cancels = self.metrics.clip_cancels.saturating_add(1);
        self.metrics.active_tracks = self.active_count() as u64;
    }

    pub(crate) fn cancel_all(&mut self, scene_node_id: SceneNodeId) {
        self.cancel_geometry(scene_node_id);
        self.cancel_opacity(scene_node_id);
        self.cancel_clip(scene_node_id);
    }

    pub fn active_count(&self) -> usize {
        self.geometry_tracks.len() + self.opacity_tracks.len() + self.clip_tracks.len()
    }

    pub fn has_track(&self, scene_node_id: SceneNodeId) -> bool {
        self.geometry_tracks.contains_key(&scene_node_id)
            || self.opacity_tracks.contains_key(&scene_node_id)
            || self.clip_tracks.contains_key(&scene_node_id)
    }

    pub fn has_geometry_track(&self, scene_node_id: SceneNodeId) -> bool {
        self.geometry_tracks.contains_key(&scene_node_id)
    }

    pub fn has_opacity_track(&self, scene_node_id: SceneNodeId) -> bool {
        self.opacity_tracks.contains_key(&scene_node_id)
    }

    pub fn has_clip_track(&self, scene_node_id: SceneNodeId) -> bool {
        self.clip_tracks.contains_key(&scene_node_id)
    }

    pub fn has_pending_visible(&self, visible_scene_nodes: &[SceneNodeId]) -> bool {
        visible_scene_nodes.iter().any(|scene_node_id| {
            self.geometry_tracks.contains_key(scene_node_id)
                || self.opacity_tracks.contains_key(scene_node_id)
                || self.clip_tracks.contains_key(scene_node_id)
        })
    }

    pub fn has_pending_visible_geometry(&self, visible_scene_nodes: &[SceneNodeId]) -> bool {
        visible_scene_nodes
            .iter()
            .any(|scene_node_id| self.geometry_tracks.contains_key(scene_node_id))
    }

    pub fn has_pending_visible_opacity(&self, visible_scene_nodes: &[SceneNodeId]) -> bool {
        visible_scene_nodes
            .iter()
            .any(|scene_node_id| self.opacity_tracks.contains_key(scene_node_id))
    }

    pub fn has_pending_visible_clip(&self, visible_scene_nodes: &[SceneNodeId]) -> bool {
        visible_scene_nodes
            .iter()
            .any(|scene_node_id| self.clip_tracks.contains_key(scene_node_id))
    }

    pub fn sample(
        &self,
        output_id: OutputId,
        target_presentation_time: AnimationTime,
        time_source: PresentationSampleTimeSource,
        targets: &[PresentationWindowTarget],
    ) -> PresentationSceneSample {
        let mut sampled = Vec::new();
        let mut transforms = Vec::new();
        let mut opacities = Vec::new();
        let mut clips = Vec::new();
        let mut sample_metrics = self.sample_metrics.get();
        sample_metrics.frame_samples = sample_metrics.frame_samples.saturating_add(1);
        match time_source {
            PresentationSampleTimeSource::ScheduledTarget => {
                sample_metrics.scheduled_target_samples =
                    sample_metrics.scheduled_target_samples.saturating_add(1);
            }
            PresentationSampleTimeSource::MonotonicFallback => {
                sample_metrics.monotonic_fallback_samples =
                    sample_metrics.monotonic_fallback_samples.saturating_add(1);
            }
            PresentationSampleTimeSource::ZeroFallback => {
                sample_metrics.zero_fallback_samples =
                    sample_metrics.zero_fallback_samples.saturating_add(1);
            }
        }
        for target in targets {
            let scene_node_id = target.window_group_scene_node_id();
            let Some(track) = self.geometry_tracks.get(&scene_node_id) else {
                continue;
            };
            let mut sample = track.transition.sample(target_presentation_time);
            sample.key = target.root_surface_id();
            let canonical_rect = target.canonical_rect();
            if sample.mathematically_settled {
                sample_metrics.mathematical_settlements =
                    sample_metrics.mathematical_settlements.saturating_add(1);
            }
            transforms.push(PresentationGroupTransform::with_scene_node(
                scene_node_id,
                target.root_surface_id(),
                sample.transaction_id,
                sample.revision_id,
                canonical_rect,
                sample.rect,
                sample.mathematically_settled,
            ));
            sampled.push(sample);
        }
        for target in targets {
            let scene_node_id = target.window_group_scene_node_id();
            if let Some(track) = self.opacity_tracks.get(&scene_node_id).copied() {
                debug_assert_eq!(track.scene_node_id, scene_node_id);
                let (opacity, _velocity, mathematically_settled) =
                    track.sample(target_presentation_time);
                opacities.push(PresentationGroupOpacity::with_scene_node(
                    scene_node_id,
                    target.root_surface_id(),
                    opacity,
                    Some(PresentationOpacityTransitionEvidence {
                        transaction_id: track.transaction_id,
                        revision_id: track.revision_id,
                        mathematically_settled,
                    }),
                ));
                if mathematically_settled {
                    sample_metrics.mathematical_settlements =
                        sample_metrics.mathematical_settlements.saturating_add(1);
                }
            } else if !target.canonical_opacity().is_opaque() {
                opacities.push(PresentationGroupOpacity::with_scene_node(
                    scene_node_id,
                    target.root_surface_id(),
                    target.canonical_opacity(),
                    None,
                ));
            }
        }
        for target in targets {
            let scene_node_id = target.window_group_scene_node_id();
            let (clip, transition) =
                if let Some(track) = self.clip_tracks.get(&scene_node_id).copied() {
                    debug_assert_eq!(track.scene_node_id, scene_node_id);
                    let sample = track.sample(target_presentation_time);
                    if sample.mathematically_settled {
                        sample_metrics.mathematical_settlements =
                            sample_metrics.mathematical_settlements.saturating_add(1);
                    }
                    (
                        sample.clip,
                        Some(PresentationClipTransitionEvidence {
                            transaction_id: track.transaction_id,
                            revision_id: track.revision_id,
                            mathematically_settled: sample.mathematically_settled,
                        }),
                    )
                } else {
                    (target.canonical_clip(), None)
                };
            if clip.is_unbounded() && transition.is_none() {
                continue;
            }
            let presented_rect = transforms
                .iter()
                .find(|transform: &&PresentationGroupTransform| {
                    transform.scene_node_id == scene_node_id
                })
                .map_or(target.canonical_rect(), |transform| {
                    transform.presented_rect
                });
            let geometry =
                PresentationGeometryTransform::new(target.canonical_rect(), presented_rect);
            let presented_clip = clip.rect().and_then(|rect| geometry.map_clip_rect(rect));
            clips.push(PresentationGroupClip::with_scene_node(
                scene_node_id,
                target.root_surface_id(),
                clip,
                presented_clip,
                transition,
            ));
        }
        opacities.sort_unstable_by_key(|opacity| opacity.root_surface_id);
        clips.sort_unstable_by_key(|clip| clip.root_surface_id);
        let sampled_windows = sampled.len();
        self.sampled_windows.set(
            self.sampled_windows
                .get()
                .saturating_add(sampled_windows as u64),
        );
        self.sample_metrics.set(sample_metrics);
        PresentationSceneSample {
            output_id,
            sampled_at: target_presentation_time,
            sample_time_source: time_source,
            windows: sampled,
            transforms,
            opacities,
            clips,
            active_transitions: self.active_count(),
            sampled_windows,
        }
    }

    pub fn acknowledge_presented_geometry(
        &mut self,
        expected_output_id: OutputId,
        ack: PresentedGeometryAck,
    ) -> bool {
        if ack.output_id != expected_output_id {
            self.metrics.wrong_output_acks = self.metrics.wrong_output_acks.saturating_add(1);
            return false;
        }
        let scene_node_id = ack.scene_node_id;
        let Some(current) = self.geometry_tracks.get(&scene_node_id) else {
            return false;
        };
        let sample = current
            .transition
            .sample(AnimationTime::from_nanos(u64::MAX));
        if ack.property != PresentationPropertyKind::Geometry
            || current.transition.revision_id != ack.revision_id
            || current.transition.transaction_id != ack.transaction_id
            || !sample.mathematically_settled
            || sample.rect != ack.presented_rect
        {
            self.metrics.stale_acknowledgements =
                self.metrics.stale_acknowledgements.saturating_add(1);
            self.metrics.stale_revision_acks = self.metrics.stale_revision_acks.saturating_add(1);
            return false;
        }
        let transaction = current.transition.transaction_id;
        let revision = current.transition.revision_id;
        self.geometry_tracks.remove(&scene_node_id);
        self.remove_transaction_member(
            transaction,
            scene_node_id,
            PresentationPropertyKind::Geometry,
            revision,
        );
        self.metrics.transitions_acknowledged =
            self.metrics.transitions_acknowledged.saturating_add(1);
        self.metrics.physical_revision_acks = self.metrics.physical_revision_acks.saturating_add(1);
        self.metrics.active_tracks = self.active_count() as u64;
        true
    }

    pub fn acknowledge_presented_opacity(
        &mut self,
        expected_output_id: OutputId,
        ack: PresentedOpacityAck,
    ) -> bool {
        if ack.output_id != expected_output_id {
            self.metrics.wrong_output_acks = self.metrics.wrong_output_acks.saturating_add(1);
            return false;
        }
        let scene_node_id = ack.scene_node_id;
        let Some(current) = self.opacity_tracks.get(&scene_node_id).copied() else {
            return false;
        };
        let (settled_opacity, _velocity, mathematically_settled) =
            current.sample(AnimationTime::from_nanos(u64::MAX));
        if ack.property != PresentationPropertyKind::Opacity
            || current.revision_id != ack.revision_id
            || current.transaction_id != ack.transaction_id
            || !mathematically_settled
            || settled_opacity != ack.presented_opacity
            || settled_opacity != current.target
        {
            self.metrics.stale_acknowledgements =
                self.metrics.stale_acknowledgements.saturating_add(1);
            self.metrics.stale_revision_acks = self.metrics.stale_revision_acks.saturating_add(1);
            return false;
        }
        self.opacity_tracks.remove(&scene_node_id);
        self.remove_transaction_member(
            current.transaction_id,
            scene_node_id,
            PresentationPropertyKind::Opacity,
            current.revision_id,
        );
        self.metrics.transitions_acknowledged =
            self.metrics.transitions_acknowledged.saturating_add(1);
        self.metrics.physical_revision_acks = self.metrics.physical_revision_acks.saturating_add(1);
        self.metrics.active_tracks = self.active_count() as u64;
        true
    }

    pub fn acknowledge_presented_clip(
        &mut self,
        expected_output_id: OutputId,
        ack: PresentedClipAck,
    ) -> bool {
        if ack.output_id != expected_output_id {
            self.metrics.wrong_output_acks = self.metrics.wrong_output_acks.saturating_add(1);
            return false;
        }
        let scene_node_id = ack.scene_node_id;
        let Some(current) = self.clip_tracks.get(&scene_node_id).copied() else {
            return false;
        };
        let sample = current.sample(AnimationTime::from_nanos(u64::MAX));
        if ack.property != PresentationPropertyKind::Clip
            || current.revision_id != ack.revision_id
            || current.transaction_id != ack.transaction_id
            || !sample.mathematically_settled
            || sample.clip != ack.presented_clip
            || sample.clip != current.target
        {
            self.metrics.stale_acknowledgements =
                self.metrics.stale_acknowledgements.saturating_add(1);
            self.metrics.stale_revision_acks = self.metrics.stale_revision_acks.saturating_add(1);
            return false;
        }
        self.clip_tracks.remove(&scene_node_id);
        self.remove_transaction_member(
            current.transaction_id,
            scene_node_id,
            PresentationPropertyKind::Clip,
            current.revision_id,
        );
        self.metrics.transitions_acknowledged =
            self.metrics.transitions_acknowledged.saturating_add(1);
        self.metrics.physical_revision_acks = self.metrics.physical_revision_acks.saturating_add(1);
        self.metrics.active_tracks = self.active_count() as u64;
        true
    }

    pub fn metrics(&self) -> PresentationAnimationMetrics {
        let sample_metrics = self.sample_metrics.get();
        PresentationAnimationMetrics {
            sampled_windows: self.sampled_windows.get(),
            active_tracks: self.active_count() as u64,
            mathematical_settlements: sample_metrics.mathematical_settlements,
            frame_samples: sample_metrics.frame_samples,
            scheduled_target_samples: sample_metrics.scheduled_target_samples,
            monotonic_fallback_samples: sample_metrics.monotonic_fallback_samples,
            zero_fallback_samples: sample_metrics.zero_fallback_samples,
            ..self.metrics
        }
    }

    #[cfg(test)]
    pub(crate) fn transaction_count(&self) -> usize {
        self.transactions.len()
    }

    #[cfg(test)]
    pub(crate) fn track_revision(
        &self,
        scene_node_id: SceneNodeId,
    ) -> Option<PresentationRevisionId> {
        self.geometry_tracks
            .get(&scene_node_id)
            .map(|track| track.transition.revision_id)
    }

    #[cfg(test)]
    pub(crate) fn track_transaction(
        &self,
        scene_node_id: SceneNodeId,
    ) -> Option<PresentationTransactionId> {
        self.geometry_tracks
            .get(&scene_node_id)
            .map(|track| track.transition.transaction_id)
    }

    #[cfg(test)]
    pub(crate) fn track_curve(&self, scene_node_id: SceneNodeId) -> Option<AnimationCurve> {
        self.geometry_tracks
            .get(&scene_node_id)
            .map(|track| track.transition.curve())
    }

    #[cfg(test)]
    pub(crate) fn track_started_at_for_scene_node(
        &self,
        scene_node_id: SceneNodeId,
    ) -> Option<AnimationTime> {
        self.geometry_tracks
            .get(&scene_node_id)
            .map(|track| track.transition.started_at())
    }

    #[cfg(test)]
    pub(crate) fn sample_for_scene_node(
        &self,
        scene_node_id: SceneNodeId,
        now: AnimationTime,
    ) -> Option<PresentationWindowSample> {
        self.geometry_tracks
            .get(&scene_node_id)
            .map(|track| track.transition.sample(now))
    }

    #[cfg(test)]
    pub(crate) fn opacity_track_revision(
        &self,
        scene_node_id: SceneNodeId,
    ) -> Option<PresentationRevisionId> {
        self.opacity_tracks
            .get(&scene_node_id)
            .map(|track| track.revision_id)
    }

    #[cfg(test)]
    pub(crate) fn opacity_track_transaction(
        &self,
        scene_node_id: SceneNodeId,
    ) -> Option<PresentationTransactionId> {
        self.opacity_tracks
            .get(&scene_node_id)
            .map(|track| track.transaction_id)
    }

    #[cfg(test)]
    pub(crate) fn clip_track_revision(
        &self,
        scene_node_id: SceneNodeId,
    ) -> Option<PresentationRevisionId> {
        self.clip_tracks
            .get(&scene_node_id)
            .map(|track| track.revision_id)
    }

    #[cfg(test)]
    pub(crate) fn clip_track_transaction(
        &self,
        scene_node_id: SceneNodeId,
    ) -> Option<PresentationTransactionId> {
        self.clip_tracks
            .get(&scene_node_id)
            .map(|track| track.transaction_id)
    }

    #[cfg(test)]
    pub(crate) fn sample_clip_for_scene_node(
        &self,
        scene_node_id: SceneNodeId,
        now: AnimationTime,
    ) -> Option<(PresentationClip, PresentationVelocity, bool)> {
        self.clip_tracks.get(&scene_node_id).copied().map(|track| {
            let sample = track.sample(now);
            (sample.clip, sample.velocity, sample.mathematically_settled)
        })
    }

    #[cfg(test)]
    pub(crate) fn sample_opacity_for_scene_node(
        &self,
        scene_node_id: SceneNodeId,
        now: AnimationTime,
    ) -> Option<(PresentationOpacity, f64, bool)> {
        self.opacity_tracks
            .get(&scene_node_id)
            .copied()
            .map(|track| track.sample(now))
    }

    #[cfg(test)]
    pub(crate) fn transition_started_at_for_scene_node(
        &self,
        scene_node_id: SceneNodeId,
    ) -> Option<AnimationTime> {
        self.geometry_tracks
            .get(&scene_node_id)
            .map(|track| track.transition.started_at())
    }

    #[cfg(test)]
    pub(crate) fn set_next_ids_for_test(
        &mut self,
        next_transaction_id: NonZeroU64,
        next_revision_id: NonZeroU64,
    ) {
        self.next_transaction_id = next_transaction_id;
        self.next_revision_id = next_revision_id;
        self.transaction_ids_exhausted = false;
        self.revision_ids_exhausted = false;
    }

    fn peek_transaction_id(
        &self,
    ) -> Result<PresentationTransactionId, PresentationTransactionError> {
        if self.transaction_ids_exhausted {
            return Err(PresentationTransactionError::TransactionIdExhausted);
        }
        Ok(PresentationTransactionId::new(self.next_transaction_id))
    }

    fn peek_revision_ids(
        &self,
        count: usize,
    ) -> Result<Vec<PresentationRevisionId>, PresentationTransactionError> {
        if self.revision_ids_exhausted {
            return Err(PresentationTransactionError::RevisionIdExhausted);
        }
        let mut next = self.next_revision_id.get();
        let mut revisions = Vec::with_capacity(count);
        for _ in 0..count {
            let id = PresentationRevisionId::from_raw(next)
                .ok_or(PresentationTransactionError::RevisionIdExhausted)?;
            revisions.push(id);
            if next == u64::MAX && revisions.len() < count {
                return Err(PresentationTransactionError::RevisionIdExhausted);
            }
            next = next.saturating_add(1);
        }
        Ok(revisions)
    }

    fn remove_transaction_member(
        &mut self,
        transaction_id: PresentationTransactionId,
        scene_node_id: SceneNodeId,
        property: PresentationPropertyKind,
        revision_id: PresentationRevisionId,
    ) {
        if let Some(record) = self.transactions.get_mut(&transaction_id) {
            record.remove_member_exact(scene_node_id, property, revision_id);
            if record.members().is_empty() {
                self.transactions.remove(&transaction_id);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn sample_scene(
        &self,
        at: AnimationTime,
        targets: &[PresentationWindowTarget],
    ) -> PresentationSceneSample {
        self.sample(
            OutputId::from_raw(1).expect("single native output identity is nonzero"),
            at,
            PresentationSampleTimeSource::ZeroFallback,
            targets,
        )
    }

    #[cfg(test)]
    pub(crate) fn start(
        &mut self,
        key: u32,
        start: PresentationRect,
        target: PresentationRect,
        now: AnimationTime,
        curve: AnimationCurve,
    ) -> Option<PresentationRevisionId> {
        let node = synthetic_scene_node_id_from_key(key);
        self.commit(PresentationTransactionRequest::geometry(
            now,
            vec![PresentationGeometryMutation::new(
                node, start, target, curve,
            )],
        ))
        .ok()
        .and_then(|record| record.members().first().map(|member| member.revision_id()))
    }

    #[cfg(test)]
    pub(crate) fn retarget(
        &mut self,
        key: u32,
        target: PresentationRect,
        now: AnimationTime,
        curve: AnimationCurve,
    ) -> Option<PresentationRevisionId> {
        let node = synthetic_scene_node_id_from_key(key);
        let start = self.geometry_tracks.get(&node)?.transition.sample(now).rect;
        self.commit(PresentationTransactionRequest::geometry(
            now,
            vec![PresentationGeometryMutation::new(
                node, start, target, curve,
            )],
        ))
        .ok()
        .and_then(|record| record.members().first().map(|member| member.revision_id()))
    }

    #[cfg(test)]
    pub(crate) fn sample_compat(
        &self,
        key: u32,
        now: AnimationTime,
    ) -> Option<PresentationWindowSample> {
        self.geometry_tracks
            .get(&synthetic_scene_node_id_from_key(key))
            .map(|track| {
                let mut sample = track.transition.sample(now);
                sample.key = key;
                sample
            })
    }

    #[cfg(test)]
    pub(crate) fn transition_curve(&self, key: u32) -> Option<AnimationCurve> {
        self.track_curve(synthetic_scene_node_id_from_key(key))
    }

    #[cfg(test)]
    pub(crate) fn sample_at_transition_start_for_scene_node(
        &self,
        scene_node_id: SceneNodeId,
    ) -> Option<PresentationWindowSample> {
        self.geometry_tracks
            .get(&scene_node_id)
            .map(|track| track.transition.sample(track.transition.started_at()))
    }
}

fn valid_rect(rect: PresentationRect) -> bool {
    rect.x().is_finite()
        && rect.y().is_finite()
        && rect.width().is_finite()
        && rect.height().is_finite()
        && rect.width() > 0.0
        && rect.height() > 0.0
}

fn valid_clip(clip: PresentationClip) -> bool {
    clip.rect().is_none_or(PresentationClipRect::is_finite)
}

fn resolve_clip_rect(
    clip: PresentationClip,
    frozen_identity_envelope: Option<PresentationClipRect>,
) -> Result<PresentationClipRect, PresentationTransactionError> {
    match clip {
        PresentationClip::Rect(rect) if rect.is_finite() => Ok(rect),
        PresentationClip::Rect(_) => Err(PresentationTransactionError::InvalidClip),
        PresentationClip::Unbounded => frozen_identity_envelope
            .filter(|rect| rect.is_finite())
            .ok_or(PresentationTransactionError::MissingClipEnvelope),
    }
}

const fn synthetic_scene_node_id() -> SceneNodeId {
    SceneNodeId::from_raw(1).expect("synthetic presentation transition identity is nonzero")
}

#[cfg(test)]
const fn synthetic_scene_node_id_from_key(key: u32) -> SceneNodeId {
    SceneNodeId::from_raw(if key == 0 { 1 } else { key as u64 })
        .expect("synthetic test scene node identity is nonzero")
}
