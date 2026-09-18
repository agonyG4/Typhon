//! Sparse SceneNode-owned geometry presentation engine.

use std::{
    cell::Cell,
    collections::{BTreeMap, HashSet},
    num::NonZeroU64,
};

use crate::core::{OutputId, SceneNodeId};

use super::{
    AnimationCurve, AnimationTime, PresentationGroupTransform, PresentationPropertyKind,
    PresentationRect, PresentationRevisionId, PresentationSampleTimeSource,
    PresentationSceneSample, PresentationTransactionError, PresentationTransactionId,
    PresentationTransactionMember, PresentationTransactionRecord, PresentationTransactionRequest,
    PresentationVelocity, PresentationWindowSample, PresentationWindowTarget,
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

#[derive(Debug)]
pub struct PresentationEngine {
    enabled: bool,
    geometry_tracks: BTreeMap<SceneNodeId, GeometryTrack>,
    transactions: BTreeMap<PresentationTransactionId, PresentationTransactionRecord>,
    next_transaction_id: NonZeroU64,
    next_revision_id: NonZeroU64,
    transaction_ids_exhausted: bool,
    revision_ids_exhausted: bool,
    metrics: PresentationAnimationMetrics,
    sampled_windows: Cell<u64>,
    sampled_output: Cell<Option<OutputId>>,
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
            transactions: BTreeMap::new(),
            next_transaction_id: NonZeroU64::MIN,
            next_revision_id: NonZeroU64::MIN,
            transaction_ids_exhausted: false,
            revision_ids_exhausted: false,
            metrics: PresentationAnimationMetrics::default(),
            sampled_windows: Cell::new(0),
            sampled_output: Cell::new(None),
            sample_metrics: Cell::new(PresentationAnimationMetrics::default()),
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            let removed = self.geometry_tracks.len() as u64;
            self.metrics.transitions_cancelled =
                self.metrics.transitions_cancelled.saturating_add(removed);
            self.metrics.geometry_cancels = self.metrics.geometry_cancels.saturating_add(removed);
            self.geometry_tracks.clear();
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
        if request.geometry.is_empty() {
            return Err(PresentationTransactionError::Empty);
        }

        let mut seen = HashSet::with_capacity(request.geometry.len());
        let mut prepared = Vec::with_capacity(request.geometry.len());
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
            let (start, start_velocity, preserve_start_velocity) = self
                .geometry_tracks
                .get(&scene_node_id)
                .map(|track| {
                    let sample = track.transition.sample(request.started_at);
                    (sample.rect, sample.velocity, true)
                })
                .unwrap_or((mutation.start, PresentationVelocity::default(), false));
            prepared.push(super::PreparedGeometryMutation {
                scene_node_id,
                start,
                target: mutation.target,
                start_velocity,
                curve: mutation.curve,
                preserve_start_velocity,
            });
        }

        let transaction_id = self.peek_transaction_id()?;
        let revision_ids = self.peek_revision_ids(prepared.len())?;
        let members = prepared
            .iter()
            .zip(revision_ids.iter().copied())
            .map(|(mutation, revision_id)| {
                PresentationTransactionMember::new(
                    mutation.scene_node_id,
                    PresentationPropertyKind::Geometry,
                    transaction_id,
                    revision_id,
                )
            })
            .collect::<Vec<_>>();
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

        for (mutation, revision_id) in prepared.into_iter().zip(revision_ids) {
            if let Some(old) = self.geometry_tracks.get(&mutation.scene_node_id) {
                self.remove_transaction_member(
                    old.transition.transaction_id,
                    mutation.scene_node_id,
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
        self.transactions.insert(transaction_id, record.clone());
        self.metrics.transactions_committed = self.metrics.transactions_committed.saturating_add(1);
        self.metrics.transaction_members = self
            .metrics
            .transaction_members
            .saturating_add(record.members().len() as u64);
        self.metrics.active_tracks = self.geometry_tracks.len() as u64;
        Ok(record)
    }

    pub(crate) fn cancel<K: PresentationOwnerKey>(&mut self, key: K) {
        let scene_node_id = key.presentation_scene_node_id();
        let Some(track) = self.geometry_tracks.remove(&scene_node_id) else {
            return;
        };
        self.remove_transaction_member(track.transition.transaction_id, scene_node_id);
        self.metrics.transitions_cancelled = self.metrics.transitions_cancelled.saturating_add(1);
        self.metrics.geometry_cancels = self.metrics.geometry_cancels.saturating_add(1);
        self.metrics.active_tracks = self.geometry_tracks.len() as u64;
    }

    pub fn active_count(&self) -> usize {
        self.geometry_tracks.len()
    }

    pub fn has_track(&self, scene_node_id: SceneNodeId) -> bool {
        self.geometry_tracks.contains_key(&scene_node_id)
    }

    pub fn has_pending_visible(&self, visible_scene_nodes: &[SceneNodeId]) -> bool {
        visible_scene_nodes
            .iter()
            .any(|scene_node_id| self.geometry_tracks.contains_key(scene_node_id))
    }

    pub fn sample(
        &self,
        output_id: OutputId,
        target_presentation_time: AnimationTime,
        time_source: PresentationSampleTimeSource,
        targets: &[PresentationWindowTarget],
    ) -> PresentationSceneSample {
        self.sampled_output.set(Some(output_id));
        let mut sampled = Vec::new();
        let mut transforms = Vec::new();
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
            active_transitions: self.active_count(),
            sampled_windows,
        }
    }

    pub fn acknowledge_presented_geometry(
        &mut self,
        output_id: OutputId,
        scene_node_id: SceneNodeId,
        revision_id: PresentationRevisionId,
        presented_rect: PresentationRect,
        transaction_id: Option<PresentationTransactionId>,
    ) -> bool {
        if self
            .sampled_output
            .get()
            .is_some_and(|sampled| sampled != output_id)
        {
            self.metrics.wrong_output_acks = self.metrics.wrong_output_acks.saturating_add(1);
            return false;
        }
        let Some(current) = self.geometry_tracks.get(&scene_node_id) else {
            return false;
        };
        let sample = current
            .transition
            .sample(AnimationTime::from_nanos(u64::MAX));
        if current.transition.revision_id != revision_id
            || transaction_id.is_some_and(|id| id != current.transition.transaction_id)
            || !sample.mathematically_settled
            || sample.rect != presented_rect
        {
            self.metrics.stale_acknowledgements =
                self.metrics.stale_acknowledgements.saturating_add(1);
            self.metrics.stale_revision_acks = self.metrics.stale_revision_acks.saturating_add(1);
            return false;
        }
        let transaction = current.transition.transaction_id;
        self.geometry_tracks.remove(&scene_node_id);
        self.remove_transaction_member(transaction, scene_node_id);
        self.metrics.transitions_acknowledged =
            self.metrics.transitions_acknowledged.saturating_add(1);
        self.metrics.physical_revision_acks = self.metrics.physical_revision_acks.saturating_add(1);
        self.metrics.active_tracks = self.geometry_tracks.len() as u64;
        true
    }

    pub fn metrics(&self) -> PresentationAnimationMetrics {
        let sample_metrics = self.sample_metrics.get();
        PresentationAnimationMetrics {
            sampled_windows: self.sampled_windows.get(),
            active_tracks: self.geometry_tracks.len() as u64,
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
    ) {
        if let Some(record) = self.transactions.get_mut(&transaction_id) {
            record.remove_member(scene_node_id);
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

pub(crate) trait PresentationOwnerKey {
    fn presentation_scene_node_id(self) -> SceneNodeId;
}

impl PresentationOwnerKey for SceneNodeId {
    fn presentation_scene_node_id(self) -> SceneNodeId {
        self
    }
}

#[cfg(test)]
impl PresentationOwnerKey for u32 {
    fn presentation_scene_node_id(self) -> SceneNodeId {
        synthetic_scene_node_id_from_key(self)
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

const fn synthetic_scene_node_id() -> SceneNodeId {
    SceneNodeId::from_raw(1).expect("synthetic presentation transition identity is nonzero")
}

#[cfg(test)]
const fn synthetic_scene_node_id_from_key(key: u32) -> SceneNodeId {
    SceneNodeId::from_raw(if key == 0 { 1 } else { key as u64 })
        .expect("synthetic test scene node identity is nonzero")
}
