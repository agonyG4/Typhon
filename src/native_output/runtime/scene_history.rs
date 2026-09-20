use super::*;
use oblivion_one::compositor::PresentationFrameSnapshot;
use oblivion_one::window_lifecycle_animation::{LifecycleFrameSnapshot, lamp_footprint};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NativeFrameSceneSnapshot {
    pub(crate) output_id: OutputId,
    pub(crate) frame_id: u64,
    pub(crate) render_generation: u64,
    pub(crate) scene: NativeSceneSnapshot,
    pub(crate) cursor_damage: NativeCursorDamageBounds,
    pub(crate) presentation: PresentationFrameSnapshot,
    pub(crate) lifecycle: LifecycleFrameSnapshot,
}

impl NativeFrameSceneSnapshot {
    pub(crate) fn from_resolved_frame_scene(
        output_id: OutputId,
        frame_id: u64,
        resolved: &ResolvedNativeFrameScene<'_>,
        cursor_damage: NativeCursorDamageBounds,
    ) -> Self {
        Self {
            output_id,
            frame_id,
            render_generation: resolved.render_generation,
            scene: resolved.snapshot_owned(),
            cursor_damage,
            presentation: resolved.presentation_snapshot.clone(),
            lifecycle: resolved.lifecycle_snapshot.clone(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct NativeSceneHistory {
    output_id: OutputId,
    presented: Option<NativeFrameSceneSnapshot>,
    ready: Option<NativeFrameSceneSnapshot>,
    submitted: VecDeque<(u64, NativeFrameSceneSnapshot)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedNativePresentationTransition {
    pub(crate) token: u64,
    pub(crate) previous_frame_id: Option<u64>,
    pub(crate) current_frame_id: u64,
    pub(crate) damage: OutputDamage,
}

impl NativeSceneHistory {
    const MAX_SUBMITTED_SCENES: usize = 3;

    pub(crate) fn new(presented: NativeFrameSceneSnapshot) -> Self {
        let output_id = presented.output_id;
        Self {
            output_id,
            presented: Some(presented),
            ready: None,
            submitted: VecDeque::new(),
        }
    }

    pub(crate) const fn output_id(&self) -> OutputId {
        self.output_id
    }

    #[cfg(test)]
    pub(crate) fn presented_scene(&self) -> &NativeSceneSnapshot {
        &self
            .presented
            .as_ref()
            .expect("caller must establish a presented scene before using this accessor")
            .scene
    }

    pub(crate) fn presented_scene_if_any(&self) -> Option<&NativeSceneSnapshot> {
        self.presented.as_ref().map(|snapshot| &snapshot.scene)
    }

    pub(crate) fn presented_presentation_if_any(
        &self,
    ) -> Option<&oblivion_one::compositor::PresentationFrameSnapshot> {
        self.presented
            .as_ref()
            .map(|snapshot| &snapshot.presentation)
    }

    pub(crate) fn presented_frame_id(&self) -> Option<u64> {
        self.presented.as_ref().map(|snapshot| snapshot.frame_id)
    }

    #[cfg(test)]
    pub(crate) fn presented_scene_node_for_surface(&self, surface_id: u32) -> Option<SceneNodeId> {
        self.presented.as_ref().and_then(|snapshot| {
            snapshot
                .scene
                .surfaces
                .iter()
                .find(|surface| surface.surface_id == surface_id)
                .map(|surface| surface.scene_node_id)
        })
    }

    #[cfg(test)]
    pub(crate) fn presented_decoration_scene_node(
        &self,
        window_id: WindowId,
    ) -> Option<SceneNodeId> {
        self.presented.as_ref().and_then(|snapshot| {
            snapshot
                .scene
                .decorations
                .iter()
                .find(|decoration| decoration.identity().0 == window_id)
                .map(DecorationSceneSnapshot::scene_node_id)
        })
    }

    #[cfg(test)]
    pub(crate) fn presented_client_cursor_scene_node(&self) -> Option<SceneNodeId> {
        self.presented
            .as_ref()
            .and_then(|snapshot| snapshot.cursor_damage.client)
            .map(|cursor| cursor.scene_node_id)
    }

    #[cfg(test)]
    pub(crate) fn submitted_scene_node_for_surface(
        &self,
        token: u64,
        surface_id: u32,
    ) -> Option<SceneNodeId> {
        self.submitted
            .iter()
            .find(|(submitted_token, _)| *submitted_token == token)
            .and_then(|(_, snapshot)| {
                snapshot
                    .scene
                    .surfaces
                    .iter()
                    .find(|surface| surface.surface_id == surface_id)
                    .map(|surface| surface.scene_node_id)
            })
    }

    pub(crate) fn submitted_frame_id(&self, token: u64) -> Option<u64> {
        self.submitted
            .iter()
            .find(|(submitted_token, _)| *submitted_token == token)
            .map(|(_, snapshot)| snapshot.frame_id)
    }

    pub(crate) fn presented_snapshot(&self) -> Option<&NativeFrameSceneSnapshot> {
        self.presented.as_ref()
    }

    pub(crate) fn presented_cursor_damage(&self) -> NativeCursorDamageBounds {
        let Some(cursor_damage) = self
            .presented
            .as_ref()
            .map(|snapshot| snapshot.cursor_damage)
        else {
            return NativeCursorDamageBounds::default();
        };
        NativeCursorDamageBounds {
            previous_client: cursor_damage.client,
            client: cursor_damage.client,
            previous_software: cursor_damage.software,
            software: cursor_damage.software,
        }
    }

    pub(crate) fn cursor_damage(
        &self,
        (client, software): (
            Option<NativeClientCursorDamageState>,
            Option<NativeDamageRect>,
        ),
    ) -> NativeCursorDamageBounds {
        let presented = self.presented_cursor_damage();
        NativeCursorDamageBounds {
            previous_client: presented.client,
            client,
            previous_software: presented.software,
            software,
        }
    }

    pub(crate) fn invalidate_presented_damage_history(&mut self) {
        self.presented = None;
        self.ready = None;
        self.submitted.clear();
    }

    pub(crate) fn replace_ready(&mut self, snapshot: NativeFrameSceneSnapshot) -> bool {
        if snapshot.output_id != self.output_id {
            return false;
        }
        self.ready = Some(snapshot);
        true
    }

    pub(crate) fn discard_ready(&mut self) {
        self.ready = None;
    }

    pub(crate) fn queue_submission(&mut self, token: u64) -> bool {
        if self.submitted.len() >= Self::MAX_SUBMITTED_SCENES {
            return false;
        }
        if self
            .ready
            .as_ref()
            .is_some_and(|snapshot| snapshot.output_id != self.output_id)
        {
            return false;
        }
        let Some(snapshot) = self.ready.take() else {
            return false;
        };
        self.submitted.push_back((token, snapshot));
        true
    }

    pub(crate) fn queue_submission_or_error(&mut self, token: u64) -> NativeResult<()> {
        self.queue_submission(token).then_some(()).ok_or_else(|| {
            io::Error::other("native submission has no rendered scene snapshot").into()
        })
    }

    pub(crate) fn prepare_pageflip_transition(
        &self,
        token: u64,
        output_width: u32,
        output_height: u32,
    ) -> Option<PreparedNativePresentationTransition> {
        let (_, current) = self
            .submitted
            .iter()
            .find(|(submitted_token, _)| *submitted_token == token)?;
        if current.output_id != self.output_id {
            return None;
        }
        let previous_frame_id = self.presented.as_ref().map(|snapshot| snapshot.frame_id);
        if previous_frame_id.is_some_and(|frame_id| current.frame_id <= frame_id) {
            return None;
        }
        let damage = match self.presented.as_ref() {
            Some(previous) => native_output_damage_for_scene_snapshots(
                output_width,
                output_height,
                &previous.scene,
                &current.scene,
                NativeCursorDamageBounds {
                    previous_client: previous.cursor_damage.client,
                    client: current.cursor_damage.client,
                    previous_software: previous.cursor_damage.software,
                    software: current.cursor_damage.software,
                },
            )
            .union_surface_rects(
                opacity_damage_for_frame_snapshots(
                    output_width,
                    output_height,
                    &previous.presentation,
                    &current.presentation,
                    &previous.scene,
                    &current.scene,
                )
                .rects,
            )
            .union_surface_rects(
                clip_damage_for_frame_snapshots(
                    output_width,
                    output_height,
                    &previous.presentation,
                    &current.presentation,
                    &previous.scene,
                    &current.scene,
                )
                .rects,
            )
            .union_surface_rects(
                lifecycle_damage_rects(&previous.lifecycle, output_width, output_height)
                    .into_iter()
                    .chain(lifecycle_damage_rects(
                        &current.lifecycle,
                        output_width,
                        output_height,
                    )),
            )
            .as_renderer_damage(output_width, output_height),
            None => NativeOutputDamage::full_output(output_width, output_height)
                .as_renderer_damage(output_width, output_height),
        };
        Some(PreparedNativePresentationTransition {
            token,
            previous_frame_id,
            current_frame_id: current.frame_id,
            damage,
        })
    }

    pub(crate) fn promote_immediate(&mut self) -> bool {
        if self
            .ready
            .as_ref()
            .is_some_and(|snapshot| snapshot.output_id != self.output_id)
        {
            self.ready = None;
            return false;
        }
        let Some(snapshot) = self.ready.take() else {
            return false;
        };
        self.presented = Some(snapshot);
        true
    }

    #[allow(dead_code)]
    pub(crate) fn promote_immediate_or_error(&mut self) -> NativeResult<()> {
        self.promote_immediate().then_some(()).ok_or_else(|| {
            io::Error::other("immediate presentation has no rendered scene snapshot").into()
        })
    }

    pub(crate) fn promote_pageflip(&mut self, token: u64) -> bool {
        let Some(index) = self
            .submitted
            .iter()
            .position(|(submitted_token, _)| *submitted_token == token)
        else {
            return false;
        };
        let (_, snapshot) = self
            .submitted
            .remove(index)
            .expect("position was returned from submitted scene history");
        if snapshot.output_id != self.output_id {
            return false;
        }
        if self
            .presented
            .as_ref()
            .is_some_and(|presented| snapshot.frame_id <= presented.frame_id)
        {
            return false;
        }
        self.presented = Some(snapshot);
        true
    }

    pub(crate) fn discard_submission(&mut self, token: u64) -> bool {
        let Some(index) = self
            .submitted
            .iter()
            .position(|(submitted_token, _)| *submitted_token == token)
        else {
            return false;
        };
        self.submitted.remove(index).is_some()
    }

    pub(crate) fn discard_unpresented(&mut self) {
        self.ready = None;
        self.submitted.clear();
    }
}

fn lifecycle_damage_rects(
    snapshot: &LifecycleFrameSnapshot,
    output_width: u32,
    output_height: u32,
) -> Vec<NativeDamageRect> {
    snapshot
        .lamps
        .iter()
        .filter_map(|lamp| {
            let footprint = lamp_footprint(lamp.visual_group)?;
            let left = footprint.x();
            let top = footprint.y();
            let right = footprint.x() + footprint.width();
            let bottom = footprint.y() + footprint.height();
            NativeDamageRect {
                x: left.floor() as i32,
                y: top.floor() as i32,
                width: (right - left).ceil().max(1.0) as u32,
                height: (bottom - top).ceil().max(1.0) as u32,
            }
            .clipped_to_output(output_width, output_height)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oblivion_one::window_lifecycle_animation::{
        LampWindowSample, LifecycleDirection, LifecycleSceneSample, LifecycleTransitionId,
        LifecycleVisualGroup,
    };

    fn snapshot(frame_id: u64) -> NativeFrameSceneSnapshot {
        let output_id = OutputId::from_raw(1).expect("nonzero output id");
        NativeFrameSceneSnapshot {
            output_id,
            frame_id,
            render_generation: frame_id,
            scene: NativeSceneSnapshot::default(),
            cursor_damage: NativeCursorDamageBounds::default(),
            presentation: PresentationFrameSnapshot::empty_for_output(output_id),
            lifecycle: LifecycleFrameSnapshot::default(),
        }
    }

    fn output_damage_covers(damage: &OutputDamage, expected: NativeDamageRect) -> bool {
        match damage {
            OutputDamage::Full => true,
            OutputDamage::Empty => false,
            OutputDamage::Rects(rects) => rects.iter().any(|actual| {
                i64::from(actual.x) <= expected.left()
                    && i64::from(actual.y) <= expected.top()
                    && i64::from(actual.x) + i64::from(actual.width) >= expected.right()
                    && i64::from(actual.y) + i64::from(actual.height) >= expected.bottom()
            }),
        }
    }

    fn snapshot_with_root(frame_id: u64, x: f64) -> NativeFrameSceneSnapshot {
        let output_id = OutputId::from_raw(1).expect("nonzero output id");
        let sample = oblivion_one::compositor::PresentationSceneSample::empty_for_output(
            output_id,
            oblivion_one::compositor::AnimationTime::from_nanos(frame_id),
            oblivion_one::compositor::PresentationSampleTimeSource::ZeroFallback,
        );
        let presentation = PresentationFrameSnapshot::from_sample_with_presented_windows(
            &sample,
            vec![
                oblivion_one::compositor::PresentedWindowGeometry::with_scene_node(
                    oblivion_one::core::SceneNodeId::from_raw(7).expect("scene node"),
                    7,
                    oblivion_one::compositor::PresentationRect::new(x, 0.0, 100.0, 80.0)
                        .expect("valid window rect"),
                ),
            ],
        );
        NativeFrameSceneSnapshot {
            output_id,
            frame_id,
            render_generation: frame_id,
            scene: NativeSceneSnapshot::default(),
            cursor_damage: NativeCursorDamageBounds::default(),
            presentation,
            lifecycle: LifecycleFrameSnapshot::default(),
        }
    }

    fn snapshot_with_group_clip(
        frame_id: u64,
        root_surface_id: u32,
        clip: oblivion_one::presentation_animation::PresentationClip,
        presented_clip: Option<oblivion_one::presentation_animation::PresentationClipRect>,
        transition: Option<
            oblivion_one::presentation_animation::PresentationClipTransitionEvidence,
        >,
    ) -> NativeFrameSceneSnapshot {
        let output_id = OutputId::from_raw(1).expect("nonzero output id");
        let scene_node_id =
            oblivion_one::core::SceneNodeId::from_raw(7).expect("WindowGroup scene node");
        let mut sample = oblivion_one::compositor::PresentationSceneSample::empty_for_output(
            output_id,
            oblivion_one::compositor::AnimationTime::from_nanos(frame_id),
            oblivion_one::compositor::PresentationSampleTimeSource::ZeroFallback,
        );
        sample.clips.push(
            oblivion_one::presentation_animation::PresentationGroupClip::with_scene_node(
                scene_node_id,
                root_surface_id,
                clip,
                presented_clip,
                transition,
            ),
        );
        let presentation = PresentationFrameSnapshot::from_sample_with_presented_windows(
            &sample,
            vec![
                oblivion_one::compositor::PresentedWindowGeometry::with_scene_node(
                    scene_node_id,
                    root_surface_id,
                    oblivion_one::compositor::PresentationRect::new(0.0, 0.0, 100.0, 80.0)
                        .expect("valid frame geometry"),
                ),
            ],
        );
        NativeFrameSceneSnapshot {
            output_id,
            frame_id,
            render_generation: frame_id,
            scene: NativeSceneSnapshot::default(),
            cursor_damage: NativeCursorDamageBounds::default(),
            presentation,
            lifecycle: LifecycleFrameSnapshot::default(),
        }
    }

    fn lifecycle_snapshot(progress: f64) -> LifecycleFrameSnapshot {
        let source = oblivion_one::compositor::PresentationRect::new(80.0, 60.0, 640.0, 480.0)
            .expect("valid source rectangle");
        let anchor = oblivion_one::compositor::PresentationRect::new(1200.0, 800.0, 48.0, 48.0)
            .expect("valid anchor rectangle");
        LifecycleFrameSnapshot::from_sample(&LifecycleSceneSample {
            sampled_at: oblivion_one::compositor::AnimationTime::from_nanos(1),
            lamps: vec![LampWindowSample {
                window_id: oblivion_one::compositor::WindowId::from_raw(7)
                    .expect("valid window id"),
                root_surface_id: 7,
                transition_id: LifecycleTransitionId::new(1),
                visual_group: LifecycleVisualGroup::from_bounds(
                    source, source, source, anchor, 1920, 1080,
                )
                .expect("valid visual group"),
                progress,
                opacity: if progress >= 1.0 { 0.0 } else { 1.0 },
                mathematically_settled: progress >= 1.0,
                direction: LifecycleDirection::Minimize,
            }],
            visual_sources: Vec::new(),
        })
    }

    #[test]
    fn lifecycle_damage_and_metadata_follow_physical_scene_promotion() {
        let mut initial = snapshot(1);
        initial.lifecycle = lifecycle_snapshot(0.5);
        let mut next = snapshot(2);
        next.lifecycle = lifecycle_snapshot(1.0);
        let mut history = NativeSceneHistory::new(initial);
        history.replace_ready(next.clone());
        assert!(history.queue_submission(20));

        let transition = history
            .prepare_pageflip_transition(20, 1920, 1080)
            .expect("lifecycle scene transition must be prepared");
        assert!(!matches!(transition.damage, OutputDamage::Empty));
        assert_eq!(
            history
                .presented_snapshot()
                .expect("initial scene is presented")
                .lifecycle
                .lamps[0]
                .progress,
            0.5
        );
        assert!(history.promote_pageflip(20));
        assert!(
            history
                .presented_snapshot()
                .expect("promoted scene is presented")
                .lifecycle
                .lamps[0]
                .mathematically_settled
        );
    }

    #[test]
    fn lifecycle_damage_repairs_ssd_titlebar_above_client() {
        let client = oblivion_one::compositor::PresentationRect::new(400.0, 100.0, 800.0, 600.0)
            .expect("valid client rectangle");
        let ssd_outer = oblivion_one::compositor::PresentationRect::new(384.0, 60.0, 832.0, 640.0)
            .expect("valid SSD rectangle");
        let anchor = oblivion_one::compositor::PresentationRect::new(1200.0, 900.0, 64.0, 64.0)
            .expect("valid anchor rectangle");
        let group =
            LifecycleVisualGroup::from_bounds(client, ssd_outer, client, anchor, 1920, 1080)
                .expect("valid visual group");
        let snapshot = LifecycleFrameSnapshot::from_sample(&LifecycleSceneSample {
            sampled_at: oblivion_one::compositor::AnimationTime::from_nanos(1),
            lamps: vec![LampWindowSample {
                window_id: oblivion_one::compositor::WindowId::from_raw(8)
                    .expect("valid window id"),
                root_surface_id: 8,
                transition_id: LifecycleTransitionId::new(8),
                visual_group: group,
                progress: 0.5,
                opacity: 1.0,
                mathematically_settled: false,
                direction: LifecycleDirection::Minimize,
            }],
            visual_sources: Vec::new(),
        });

        let damage = lifecycle_damage_rects(&snapshot, 1920, 1080);

        assert_eq!(damage.len(), 1);
        assert!(damage[0].y <= 60);
        assert!(damage[0].y + i32::try_from(damage[0].height).expect("damage fits") >= 964);
    }

    #[test]
    fn rendered_snapshot_advances_presented_history_only_on_matching_pageflip() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        history.replace_ready(snapshot(2));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(1)
        );
        assert!(!history.promote_pageflip(77));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(1)
        );
        assert!(history.queue_submission(77));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(1)
        );
        assert!(history.promote_pageflip(77));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(2)
        );
    }

    #[test]
    fn scene_history_rejects_a_snapshot_from_another_logical_output() {
        let first = OutputId::from_raw(1).expect("nonzero output id");
        let second = OutputId::from_raw(2).expect("nonzero output id");
        let mut history = NativeSceneHistory::new(snapshot(1));
        let mut foreign = snapshot(2);
        foreign.output_id = second;

        assert_eq!(history.output_id(), first);
        assert!(!history.replace_ready(foreign));
        assert!(history.ready.is_none());
    }

    #[test]
    fn pageflip_promotion_rejects_a_foreign_output_snapshot() {
        let first = OutputId::from_raw(1).expect("nonzero output id");
        let second = OutputId::from_raw(2).expect("nonzero output id");
        let mut history = NativeSceneHistory::new(snapshot(1));
        let mut foreign = snapshot(2);
        foreign.output_id = second;
        history.submitted.push_back((77, foreign));

        assert!(!history.promote_pageflip(77));
        assert_eq!(
            history
                .presented_snapshot()
                .map(|snapshot| snapshot.output_id),
            Some(first)
        );
    }

    #[test]
    fn presented_window_projection_advances_only_on_physical_promotion() {
        let mut history = NativeSceneHistory::new(snapshot_with_root(1, 100.0));
        history.replace_ready(snapshot_with_root(2, 200.0));
        assert_eq!(
            history
                .presented_snapshot()
                .and_then(|snapshot| snapshot.presentation.presented_window_geometry(7))
                .map(|root| root.presented_rect().x()),
            Some(100.0)
        );
        assert!(history.queue_submission(20));
        assert_eq!(
            history
                .presented_snapshot()
                .and_then(|snapshot| snapshot.presentation.presented_window_geometry(7))
                .map(|root| root.presented_rect().x()),
            Some(100.0)
        );
        assert!(history.promote_pageflip(20));
        assert_eq!(
            history
                .presented_snapshot()
                .and_then(|snapshot| snapshot.presentation.presented_window_geometry(7))
                .map(|root| root.presented_rect().x()),
            Some(200.0)
        );
    }

    #[test]
    fn pageflip_transition_uses_the_actual_presented_predecessor() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        history.replace_ready(snapshot(2));
        assert!(history.queue_submission(20));
        history.replace_ready(snapshot(3));
        assert!(history.queue_submission(30));

        let b_transition = history
            .prepare_pageflip_transition(20, 100, 80)
            .expect("B transition must be prepared");
        assert_eq!(b_transition.previous_frame_id, Some(1));
        assert_eq!(b_transition.current_frame_id, 2);
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(1)
        );
        assert!(history.promote_pageflip(20));

        let c_transition = history
            .prepare_pageflip_transition(30, 100, 80)
            .expect("C transition must be prepared");
        assert_eq!(c_transition.previous_frame_id, Some(2));
        assert_eq!(c_transition.current_frame_id, 3);
    }

    #[test]
    fn rejected_render_ahead_candidate_does_not_change_next_transition_predecessor() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        history.replace_ready(snapshot(2));
        assert!(history.queue_submission(20));
        history.replace_ready(snapshot(3));
        assert!(history.queue_submission(30));
        assert!(history.discard_submission(20));

        let transition = history
            .prepare_pageflip_transition(30, 100, 80)
            .expect("C transition must be prepared");
        assert_eq!(transition.previous_frame_id, Some(1));
        assert_eq!(transition.current_frame_id, 3);
    }

    #[test]
    fn delayed_pageflip_uses_submitted_c_not_mutable_ready_d() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        history.replace_ready(snapshot(2));
        assert!(history.queue_submission(20));
        assert!(history.promote_pageflip(20));
        history.replace_ready(snapshot(3));
        assert!(history.queue_submission(30));
        history.replace_ready(snapshot(4));

        let transition = history
            .prepare_pageflip_transition(30, 100, 80)
            .expect("C transition must remain available");
        assert_eq!(transition.previous_frame_id, Some(2));
        assert_eq!(transition.current_frame_id, 3);
    }

    #[test]
    fn submitted_xwayland_clip_evidence_survives_backing_replacement_unchanged() {
        use oblivion_one::presentation_animation::{
            PresentationClip, PresentationClipRect, PresentationClipTransitionEvidence,
        };

        let scene_node_id =
            oblivion_one::core::SceneNodeId::from_raw(7).expect("WindowGroup scene node");
        let output_id = OutputId::from_raw(1).expect("output");
        let transaction_id =
            oblivion_one::presentation_animation::PresentationTransactionId::from_raw(55)
                .expect("transaction");
        let revision_id =
            oblivion_one::presentation_animation::PresentationRevisionId::from_raw(66)
                .expect("revision");
        let clip_a = PresentationClipRect::new(100.0, 0.0, 100.0, 80.0).expect("root A clip");
        let clip_b = PresentationClipRect::new(200.0, 100.0, 100.0, 80.0).expect("root B clip");
        let mut frozen_a = snapshot_with_group_clip(
            1,
            70,
            PresentationClip::Rect(clip_a),
            Some(clip_a),
            Some(PresentationClipTransitionEvidence {
                transaction_id,
                revision_id,
                mathematically_settled: true,
            }),
        );
        let region_a = oblivion_one::effects::EffectRegion::from_rect(
            oblivion_one::effects::EffectRect::new(120, 10, 60, 40).expect("frame A influence"),
        );
        frozen_a.scene.presentation_effect_influences.push(
            super::super::output::NativePresentationEffectInfluenceSnapshot {
                scene_node_id,
                presentation_owner_root_surface_id: 70,
                region: region_a.clone(),
            },
        );
        let mut root_b_frame =
            snapshot_with_group_clip(2, 80, PresentationClip::Rect(clip_b), Some(clip_b), None);
        let region_b = oblivion_one::effects::EffectRegion::from_rect(
            oblivion_one::effects::EffectRect::new(220, 110, 60, 40).expect("frame B influence"),
        );
        root_b_frame.scene.presentation_effect_influences.push(
            super::super::output::NativePresentationEffectInfluenceSnapshot {
                scene_node_id,
                presentation_owner_root_surface_id: 80,
                region: region_b.clone(),
            },
        );
        let mut history = NativeSceneHistory::new(snapshot(0));
        assert!(history.replace_ready(frozen_a));
        assert!(history.queue_submission(20));

        // The next frame uses root B, but the submitted root A snapshot stays frozen.
        assert!(history.replace_ready(root_b_frame));
        assert!(history.queue_submission(30));
        let (_, submitted_a) = history
            .submitted
            .iter()
            .find(|(token, _)| *token == 20)
            .expect("root A frame remains submitted");
        let clip = submitted_a
            .presentation
            .clips
            .first()
            .expect("submitted Clip evidence");
        assert_eq!(clip.scene_node_id, scene_node_id);
        assert_eq!(clip.root_surface_id, 70);
        assert_eq!(clip.clip, PresentationClip::Rect(clip_a));
        let transition = clip.transition.expect("submitted exact Clip revision");
        assert_eq!(transition.transaction_id, transaction_id);
        assert_eq!(transition.revision_id, revision_id);
        assert_eq!(submitted_a.presentation.output_id, output_id);
        assert_eq!(
            submitted_a.scene.presentation_effect_influences[0].region,
            region_a
        );
        assert_eq!(
            submitted_a.scene.presentation_effect_influences[0].presentation_owner_root_surface_id,
            70
        );

        let prepared_a = history
            .prepare_pageflip_transition(20, 400, 300)
            .expect("submitted A transition remains frozen");
        assert!(output_damage_covers(
            &prepared_a.damage,
            NativeDamageRect {
                x: 120,
                y: 10,
                width: 60,
                height: 40,
            }
        ));

        assert!(history.promote_pageflip(20));
        let promoted_a = history
            .presented_snapshot()
            .expect("root A frame physically promoted");
        assert_eq!(promoted_a.presentation.clips[0].root_surface_id, 70);
        assert_eq!(
            promoted_a.presentation.clips[0].clip,
            PresentationClip::Rect(clip_a)
        );

        let prepared_b = history
            .prepare_pageflip_transition(30, 400, 300)
            .expect("submitted B transition compares the frozen A and B scenes");
        for expected in [
            NativeDamageRect {
                x: 120,
                y: 10,
                width: 60,
                height: 40,
            },
            NativeDamageRect {
                x: 220,
                y: 110,
                width: 60,
                height: 40,
            },
        ] {
            assert!(output_damage_covers(&prepared_b.damage, expected));
        }

        assert!(history.promote_pageflip(30));
        let promoted_b = history
            .presented_snapshot()
            .expect("root B frame physically promoted");
        assert_eq!(
            promoted_b.scene.presentation_effect_influences[0].region,
            region_b
        );
        assert_eq!(
            promoted_b.scene.presentation_effect_influences[0].presentation_owner_root_surface_id,
            80
        );
        assert_eq!(promoted_b.presentation.clips[0].root_surface_id, 80);
        assert_eq!(
            promoted_b.presentation.clips[0].clip,
            PresentationClip::Rect(clip_b)
        );
    }

    #[test]
    fn rejected_c_is_not_inserted_between_b_and_d() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        history.replace_ready(snapshot(2));
        assert!(history.queue_submission(20));
        assert!(history.promote_pageflip(20));
        history.replace_ready(snapshot(3));
        assert!(history.queue_submission(30));
        assert!(history.discard_submission(30));
        history.replace_ready(snapshot(4));
        assert!(history.queue_submission(40));

        let transition = history
            .prepare_pageflip_transition(40, 100, 80)
            .expect("D transition must be prepared");
        assert_eq!(transition.previous_frame_id, Some(2));
        assert_eq!(transition.current_frame_id, 4);
    }

    #[test]
    fn replacing_ready_and_discarding_submission_never_promotes_stale_frame() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        history.replace_ready(snapshot(2));
        assert!(history.queue_submission(20));
        history.replace_ready(snapshot(3));
        assert!(history.discard_submission(20));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(1)
        );
        assert!(history.promote_immediate());
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(3)
        );
    }

    #[test]
    fn stale_pageflip_token_cannot_regress_newer_presented_scene() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        history.replace_ready(snapshot(2));
        assert!(history.queue_submission(20));
        history.replace_ready(snapshot(3));
        assert!(history.queue_submission(30));
        assert!(history.promote_pageflip(30));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(3)
        );
        assert!(!history.promote_pageflip(20));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(3)
        );
    }

    #[test]
    fn multiple_rejected_frames_leave_presented_scene_unchanged_until_d_pageflip() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        for (frame_id, token) in [(2, 20), (3, 30)] {
            history.replace_ready(snapshot(frame_id));
            assert!(history.queue_submission(token));
            assert!(history.discard_submission(token));
            assert_eq!(
                history.presented.as_ref().map(|frame| frame.frame_id),
                Some(1)
            );
        }

        history.replace_ready(snapshot(4));
        assert!(history.queue_submission(40));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(1)
        );
        assert!(history.promote_pageflip(40));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(4)
        );
    }

    #[test]
    fn direct_primary_boundary_has_no_composited_scene_until_return() {
        let mut history = NativeSceneHistory::new(snapshot(1));
        history.replace_ready(snapshot(2));
        assert!(history.queue_submission(20));

        history.invalidate_presented_damage_history();

        assert!(history.presented_scene_if_any().is_none());
        assert!(history.ready.is_none());
        assert!(history.submitted.is_empty());
        assert_eq!(
            history.cursor_damage((None, None)),
            NativeCursorDamageBounds::default()
        );

        history.replace_ready(snapshot(3));
        assert!(history.queue_submission(30));
        assert!(history.promote_pageflip(30));
        assert_eq!(
            history.presented.as_ref().map(|frame| frame.frame_id),
            Some(3)
        );
    }
}
