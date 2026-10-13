use super::{CompositorState, PresentationFrameSnapshot};
use crate::window_lifecycle_animation::LifecycleFrameSnapshot;

use crate::compositor::surface::SurfaceCommitSequence;
use crate::compositor::{RenderableSurface, SurfacePresentationKey};
use crate::core::{OutputId, SceneNodeId};
use crate::presentation_animation::{
    AnimationTime, PresentationRetainedVisualIdentity, PresentationSampleTimeSource,
    PresentationSceneSample,
};
use crate::render_backend::buffer::BufferId;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedCanonicalSceneSnapshot {
    pub output_id: OutputId,
    pub render_generation: u64,
    pub effect_identity_signature: u64,
    pub surfaces: Vec<PresentedSurfaceContentEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PresentedSurfaceContentEvidence {
    pub key: SurfacePresentationKey,
    pub commit_sequence: SurfaceCommitSequence,
    pub buffer_id: BufferId,
    pub scene_node_id: SceneNodeId,
    pub visual_root_surface_id: u32,
    pub presentation_owner_root_surface_id: u32,
}

impl PresentedCanonicalSceneSnapshot {
    #[doc(hidden)]
    pub fn capture(
        output_id: OutputId,
        render_generation: u64,
        effect_identity_signature: u64,
        surfaces: &[RenderableSurface],
        scene_node_ids: &[SceneNodeId],
        presentation_owner_root_surface_ids: &[u32],
        presentation_keys: &[Option<SurfacePresentationKey>],
        visual_root_surface_ids: &[u32],
    ) -> Option<Self> {
        let len = surfaces.len();
        if scene_node_ids.len() != len
            || presentation_owner_root_surface_ids.len() != len
            || presentation_keys.len() != len
            || visual_root_surface_ids.len() != len
        {
            return None;
        }

        let mut seen_surface_ids = HashSet::with_capacity(len);
        let mut evidence = Vec::with_capacity(len);
        for ((((surface, scene_node_id), owner_root_surface_id), key), visual_root_surface_id) in
            surfaces
                .iter()
                .zip(scene_node_ids.iter().copied())
                .zip(presentation_owner_root_surface_ids.iter().copied())
                .zip(presentation_keys.iter().copied())
                .zip(visual_root_surface_ids.iter().copied())
        {
            let key = key?;
            if key.surface_id != surface.surface_id || !seen_surface_ids.insert(surface.surface_id)
            {
                return None;
            }
            evidence.push(PresentedSurfaceContentEvidence {
                key,
                commit_sequence: surface.commit_sequence,
                buffer_id: surface.buffer_id(),
                scene_node_id,
                visual_root_surface_id,
                presentation_owner_root_surface_id: owner_root_surface_id,
            });
        }
        evidence.sort_unstable_by_key(|surface| (surface.key.surface_id, surface.key.generation));

        Some(Self {
            output_id,
            render_generation,
            effect_identity_signature,
            surfaces: evidence,
        })
    }

    pub(crate) fn surfaces_for_owner(
        &self,
        owner_root_surface_id: u32,
    ) -> Vec<PresentedSurfaceContentEvidence> {
        self.surfaces
            .iter()
            .copied()
            .filter(|surface| surface.presentation_owner_root_surface_id == owner_root_surface_id)
            .collect()
    }
}

pub struct PresentedFramePublication<'a> {
    pub frame_id: u64,
    pub presentation: &'a PresentationFrameSnapshot,
    pub lifecycle: &'a LifecycleFrameSnapshot,
    pub lifecycle_scene: PresentedLifecycleScene<'a>,
    pub canonical_scene: Option<&'a PresentedCanonicalSceneSnapshot>,
    pub window_exits: &'a [WindowExitFrameEvidence],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowExitFrameEvidence {
    pub identity: PresentationRetainedVisualIdentity,
    pub payload_id: u64,
    pub root_surface_id: u32,
    pub scene_node_id: SceneNodeId,
}

pub enum PresentedLifecycleScene<'a> {
    Initial,
    RenderedSceneReplacement {
        canonical_root_surface_ids: &'a [u32],
    },
}

impl CompositorState {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn publish_direct_scanout_frame(
        &mut self,
        frame_id: u64,
        presented_at_ns: u64,
        output_id: OutputId,
        render_generation: u64,
        effect_identity_signature: u64,
        surface_id: u32,
        surface_presentation_generation: u64,
        commit_sequence: SurfaceCommitSequence,
        buffer_id: BufferId,
        surface_scene_node_id: SceneNodeId,
        window_scene_node_id: SceneNodeId,
        root_surface_id: u32,
        presented_window_rect: crate::presentation_animation::PresentationRect,
    ) {
        let sample = PresentationSceneSample::empty_for_output(
            output_id,
            AnimationTime::from_nanos(presented_at_ns),
            PresentationSampleTimeSource::MonotonicFallback,
        );
        let presentation = PresentationFrameSnapshot::from_sample_with_presented_windows(
            &sample,
            vec![
                crate::presentation_animation::PresentedWindowGeometry::with_scene_node(
                    window_scene_node_id,
                    root_surface_id,
                    presented_window_rect,
                ),
            ],
        );
        let canonical_scene = PresentedCanonicalSceneSnapshot {
            output_id,
            render_generation,
            effect_identity_signature,
            surfaces: vec![PresentedSurfaceContentEvidence {
                key: SurfacePresentationKey {
                    surface_id,
                    generation: surface_presentation_generation,
                },
                commit_sequence,
                buffer_id,
                scene_node_id: surface_scene_node_id,
                visual_root_surface_id: root_surface_id,
                presentation_owner_root_surface_id: root_surface_id,
            }],
        };
        let lifecycle = LifecycleFrameSnapshot::default();
        self.publish_presented_frame(PresentedFramePublication {
            frame_id,
            presentation: &presentation,
            lifecycle: &lifecycle,
            lifecycle_scene: PresentedLifecycleScene::RenderedSceneReplacement {
                canonical_root_surface_ids: &[root_surface_id],
            },
            canonical_scene: Some(&canonical_scene),
            window_exits: &[],
        });
    }

    pub(in crate::compositor) fn publish_presented_frame(
        &mut self,
        publication: PresentedFramePublication<'_>,
    ) {
        let expected_output_id = self
            .native_output_id()
            .expect("native presentation requires allocated logical OutputId");
        if publication.presentation.output_id != expected_output_id {
            return;
        }
        if publication
            .canonical_scene
            .is_some_and(|scene| scene.output_id != expected_output_id)
        {
            return;
        }

        self.presented_canonical_scene = publication.canonical_scene.cloned();
        self.publish_presented_presentation(publication.frame_id, publication.presentation);
        self.publish_presented_lifecycle_snapshot(
            publication.frame_id,
            publication.lifecycle,
            publication.lifecycle_scene,
        );
        self.settle_window_exit_physical(
            publication.frame_id,
            publication.presentation,
            publication.window_exits,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::compositor::PresentationRetainedVisualPayloadId;
    use crate::core::{OutputId, SceneNodeId, WindowId};
    use crate::presentation_animation::{
        AnimationCurve, AnimationTime, EasingCurve, PresentationClip, PresentationClipMutation,
        PresentationClipRect, PresentationEngine, PresentationFrameSnapshot,
        PresentationGeometryMutation, PresentationOpacity, PresentationOpacityMutation,
        PresentationRetainedVisualIdentity, PresentationRetainedVisualKind, PresentationRevisionId,
        PresentationSampleTimeSource, PresentationSceneSample, PresentationTransactionId,
        PresentationTransactionRequest, PresentationWindowTarget, PresentedWindowGeometry,
    };
    use crate::window_lifecycle_animation::{
        LifecycleDirection, LifecycleFrameSample, LifecycleFrameSnapshot, LifecycleMotionRequest,
        LifecycleVisualGroup,
    };
    use std::time::Duration;

    const PRESENTATION_NODE: u64 = 71;
    const PRESENTATION_ROOT: u32 = 71;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
        PresentationRect::new(x, y, width, height).expect("valid test rectangle")
    }

    fn physical_presentation_snapshot(
        output_id: OutputId,
        sampled_at: AnimationTime,
        presented_windows: Vec<PresentedWindowGeometry>,
    ) -> PresentationFrameSnapshot {
        let sample = PresentationSceneSample::empty_for_output(
            output_id,
            sampled_at,
            PresentationSampleTimeSource::MonotonicFallback,
        );
        PresentationFrameSnapshot::from_sample_with_presented_windows(&sample, presented_windows)
    }

    fn input_projection(presentation: &PresentationFrameSnapshot) -> Vec<(u32, PresentationRect)> {
        presentation
            .presented_windows
            .iter()
            .map(|window| (window.root_surface_id(), window.presented_rect()))
            .collect()
    }

    fn clip_rect(x: f64, y: f64, width: f64, height: f64) -> PresentationClipRect {
        PresentationClipRect::new(x, y, width, height).expect("valid test clip rectangle")
    }

    fn lifecycle_visual_group() -> LifecycleVisualGroup {
        LifecycleVisualGroup::from_bounds(
            rect(20.0, 20.0, 200.0, 150.0),
            rect(20.0, 20.0, 200.0, 150.0),
            rect(20.0, 20.0, 200.0, 150.0),
            rect(500.0, 500.0, 40.0, 40.0),
            1920,
            1080,
        )
        .expect("valid test lifecycle visual group")
    }

    fn lifecycle_snapshot(
        window_id: WindowId,
        root_surface_id: u32,
        presentation_identity: PresentationRetainedVisualIdentity,
        direction: LifecycleDirection,
    ) -> LifecycleFrameSnapshot {
        lifecycle_snapshot_with_settlement(
            window_id,
            root_surface_id,
            presentation_identity,
            direction,
            true,
        )
    }

    fn lifecycle_snapshot_with_settlement(
        window_id: WindowId,
        root_surface_id: u32,
        presentation_identity: PresentationRetainedVisualIdentity,
        direction: LifecycleDirection,
        mathematically_settled: bool,
    ) -> LifecycleFrameSnapshot {
        let mut snapshot = LifecycleFrameSnapshot {
            sampled_at: Some(AnimationTime::from_nanos(280_000_000)),
            samples: vec![LifecycleFrameSample {
                window_id,
                root_surface_id,
                presentation_identity,
                payload_id: PresentationRetainedVisualPayloadId::from_origin_identity(
                    presentation_identity,
                ),
                visual_group: lifecycle_visual_group(),
                effect: crate::window_lifecycle_animation::LifecycleEffectKind::Lamp,
                progress: if direction == LifecycleDirection::Restore {
                    0.0
                } else {
                    1.0
                },
                effect_opacity: 1.0,
                mathematically_settled,
                direction,
            }],
            signature: 0,
        };
        snapshot.refresh_signature();
        snapshot
    }

    fn lifecycle_request(
        presentation_engine: &mut PresentationEngine,
        payload_store: &mut super::super::state::lifecycle_retained::RetainedLifecyclePayloadStore,
        window_id: WindowId,
        _root_surface_id: u32,
        direction: LifecycleDirection,
    ) -> LifecycleMotionRequest {
        let scene_node_id = SceneNodeId::from_raw(window_id.get()).expect("test scene node");
        let previous_identity = presentation_engine.active_retained_visual(
            scene_node_id,
            PresentationRetainedVisualKind::WindowLifecycle,
        );
        let presentation_identity = presentation_engine
            .begin_retained_visual(
                scene_node_id,
                PresentationRetainedVisualKind::WindowLifecycle,
                AnimationTime::from_nanos(0),
            )
            .expect("test retained lifecycle identity");
        let activated_previous = presentation_engine
            .activate_retained_visual_exact(presentation_identity)
            .expect("activate lifecycle identity");
        assert_eq!(activated_previous, previous_identity);
        if let Some(previous_identity) = previous_identity {
            let payload = std::sync::Arc::clone(
                payload_store
                    .get_exact(previous_identity)
                    .expect("reversal keeps its exact retained payload"),
            );
            assert!(payload_store.transfer_exact(
                previous_identity,
                presentation_identity,
                &payload
            ));
        } else {
            let payload =
                super::super::state::lifecycle_retained::RetainedLifecyclePayload::capture(
                    presentation_identity,
                    window_id,
                    _root_surface_id,
                    lifecycle_visual_group(),
                    ResolvedEffectScene::default(),
                    None,
                    super::super::state::lifecycle_retained::RetainedLifecycleRenderSource::new(
                        super::super::state::RetainedSurfacePresentationSnapshot::test_root(
                            _root_surface_id,
                        ),
                        std::sync::Arc::new(
                            crate::compositor::FrozenLifecycleRenderLayout::empty_for_test(),
                        ),
                    ),
                )
                .expect("valid test lifecycle payload");
            assert!(
                payload_store
                    .publish_exact(
                        presentation_identity,
                        super::super::state::lifecycle_retained::RetainedLifecycleEntry {
                            payload,
                            pre_capture: None,
                        },
                    )
                    .is_ok()
            );
        }
        LifecycleMotionRequest {
            presentation_identity,
            direction,
            effect: crate::window_lifecycle_animation::LifecycleEffectKind::Lamp,
            canonical_opacity: 1.0,
        }
    }

    fn historical_identity(window_id: WindowId, raw: u64) -> PresentationRetainedVisualIdentity {
        PresentationRetainedVisualIdentity::new(
            SceneNodeId::from_raw(window_id.get()).expect("test scene node"),
            PresentationRetainedVisualKind::WindowLifecycle,
            PresentationTransactionId::from_raw(raw).expect("test transaction id"),
            PresentationRevisionId::from_raw(raw).expect("test revision id"),
        )
    }

    fn presentation_request(
        scene_node_id: SceneNodeId,
        started_at: AnimationTime,
        geometry: (PresentationRect, PresentationRect),
        opacity: (f64, f64),
        clip: (PresentationClip, PresentationClip),
    ) -> PresentationTransactionRequest {
        let curve = AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear);
        let (start_rect, target_rect) = geometry;
        let (start_opacity, target_opacity) = opacity;
        let (start_clip, target_clip) = clip;
        PresentationTransactionRequest::mixed_all(
            started_at,
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                start_rect,
                target_rect,
                curve,
            )],
            vec![PresentationOpacityMutation::new(
                scene_node_id,
                PresentationOpacity::new(start_opacity).expect("valid starting opacity"),
                PresentationOpacity::new(target_opacity).expect("valid target opacity"),
                curve,
            )],
            vec![PresentationClipMutation::new(
                scene_node_id,
                start_clip,
                target_clip,
                Some(clip_rect(0.0, 0.0, 200.0, 150.0)),
                curve,
            )],
        )
    }

    fn sample_presentation(
        state: &CompositorState,
        output_id: OutputId,
        at: AnimationTime,
        canonical_rect: PresentationRect,
    ) -> PresentationFrameSnapshot {
        let target_clip = PresentationClip::Rect(clip_rect(10.0, 10.0, 80.0, 60.0));
        state
            .presentation_animator
            .sample(
                output_id,
                at,
                PresentationSampleTimeSource::ScheduledTarget,
                &[PresentationWindowTarget::with_scene_node(
                    SceneNodeId::from_raw(PRESENTATION_NODE).expect("test scene node"),
                    PRESENTATION_ROOT,
                    canonical_rect,
                )
                .with_canonical_clip(target_clip)],
            )
            .frame_snapshot()
    }

    #[test]
    fn repeated_identical_physical_presentation_keeps_pointer_hit_generation() {
        let mut state = CompositorState::new(None);
        let output_id = state.native_output_id().expect("test output id");
        let root_surface_id = 71;
        let pointer_generation_before_authority = state.pointer_hit_generation;
        let scene_node_id = SceneNodeId::from_raw(PRESENTATION_NODE).expect("scene node");
        let presented_rect = rect(20.0, 30.0, 180.0, 120.0);
        let second_window = PresentedWindowGeometry::with_scene_node(
            SceneNodeId::from_raw(PRESENTATION_NODE + 1).expect("second scene node"),
            root_surface_id + 1,
            rect(230.0, 30.0, 100.0, 80.0),
        );
        let first_window = PresentedWindowGeometry::with_scene_node(
            scene_node_id,
            root_surface_id,
            presented_rect,
        );
        let presentation = physical_presentation_snapshot(
            output_id,
            AnimationTime::from_nanos(1_000_000),
            vec![second_window, first_window],
        );
        let reordered_presentation = physical_presentation_snapshot(
            output_id,
            AnimationTime::from_nanos(1_000_000),
            vec![first_window, second_window],
        );
        assert_eq!(
            input_projection(&presentation),
            input_projection(&reordered_presentation),
            "the canonical constructor makes producer input order irrelevant"
        );

        state.publish_presented_presentation(1, &presentation);
        let after_authority = state.pointer_hit_generation;
        assert_eq!(
            after_authority,
            pointer_generation_before_authority + 1,
            "the first authoritative publication changes input eligibility"
        );
        let _ = state.pointer_scene_hit_at(0.0, 0.0);
        let topology_after_authority = state.interaction_topology_generation;
        let metrics_before_repeats = state.pointer_input_metrics_snapshot();

        for frame_id in 2..=4 {
            let mut repeated_windows = vec![
                PresentedWindowGeometry::with_scene_node(
                    scene_node_id,
                    root_surface_id,
                    presented_rect,
                ),
                second_window,
            ];
            if frame_id == 3 {
                repeated_windows[0] = PresentedWindowGeometry::with_scene_node(
                    SceneNodeId::from_raw(PRESENTATION_NODE + 3).expect("replacement node"),
                    root_surface_id,
                    presented_rect,
                );
            }
            if frame_id == 4 {
                repeated_windows.reverse();
            }
            let mut repeated = physical_presentation_snapshot(
                output_id,
                AnimationTime::from_nanos(frame_id * 1_000_000),
                repeated_windows,
            );
            repeated.signature = frame_id;
            assert_eq!(input_projection(&presentation), input_projection(&repeated));
            state.publish_presented_frame(PresentedFramePublication {
                frame_id,
                presentation: &repeated,
                lifecycle: &LifecycleFrameSnapshot::default(),
                lifecycle_scene: PresentedLifecycleScene::Initial,
                canonical_scene: None,
                window_exits: &[],
            });
        }

        assert_eq!(
            state.pointer_hit_generation, after_authority,
            "frame IDs, sample timestamps, signatures, and scene-node IDs do not change pointer mapping"
        );
        assert_eq!(
            state.interaction_topology_generation,
            topology_after_authority
        );
        let metrics_after_repeats = state.pointer_input_metrics_snapshot();
        assert_eq!(
            metrics_after_repeats.pointer_hit_generation_invalidations,
            metrics_before_repeats.pointer_hit_generation_invalidations
        );
        assert_eq!(
            metrics_after_repeats.interaction_topology_invalidations,
            metrics_before_repeats.interaction_topology_invalidations
        );
        assert_eq!(
            metrics_after_repeats.visual_stack_groups_cache_rebuilds,
            metrics_before_repeats.visual_stack_groups_cache_rebuilds
        );
    }

    #[test]
    fn physical_projection_changes_invalidate_pointer_hits_without_changing_topology() {
        let mut state = CompositorState::new(None);
        let output_id = state.native_output_id().expect("test output id");
        let first_root = 71;
        let second_root = 72;
        let added_root = 73;
        let first_node = SceneNodeId::from_raw(PRESENTATION_NODE).expect("first scene node");
        let second_node = SceneNodeId::from_raw(PRESENTATION_NODE + 1).expect("second scene node");
        let first_rect = rect(20.0, 30.0, 180.0, 120.0);
        let second_rect = rect(230.0, 30.0, 100.0, 80.0);
        let topology_generation = state.interaction_topology_generation;

        let initial = physical_presentation_snapshot(
            output_id,
            AnimationTime::from_nanos(1_000_000),
            vec![
                PresentedWindowGeometry::with_scene_node(second_node, second_root, second_rect),
                PresentedWindowGeometry::with_scene_node(first_node, first_root, first_rect),
            ],
        );
        state.publish_presented_presentation(1, &initial);
        let mut pointer_generation = state.pointer_hit_generation;
        assert_eq!(state.interaction_topology_generation, topology_generation);

        let identical_projection = physical_presentation_snapshot(
            output_id,
            AnimationTime::from_nanos(2_000_000),
            vec![
                PresentedWindowGeometry::with_scene_node(
                    SceneNodeId::from_raw(PRESENTATION_NODE + 4).expect("replacement node"),
                    first_root,
                    first_rect,
                ),
                PresentedWindowGeometry::with_scene_node(second_node, second_root, second_rect),
            ],
        );
        state.publish_presented_presentation(2, &identical_projection);
        assert_eq!(state.pointer_hit_generation, pointer_generation);
        assert_eq!(state.interaction_topology_generation, topology_generation);

        let changed_rect = rect(240.0, 35.0, 100.0, 80.0);
        let rect_changed = physical_presentation_snapshot(
            output_id,
            AnimationTime::from_nanos(3_000_000),
            vec![
                PresentedWindowGeometry::with_scene_node(first_node, first_root, first_rect),
                PresentedWindowGeometry::with_scene_node(second_node, second_root, changed_rect),
            ],
        );
        state.publish_presented_presentation(3, &rect_changed);
        assert_eq!(state.pointer_hit_generation, pointer_generation + 1);
        pointer_generation = state.pointer_hit_generation;
        assert_eq!(state.interaction_topology_generation, topology_generation);

        let root_added = physical_presentation_snapshot(
            output_id,
            AnimationTime::from_nanos(4_000_000),
            vec![
                PresentedWindowGeometry::with_scene_node(
                    SceneNodeId::from_raw(PRESENTATION_NODE + 2).expect("added scene node"),
                    added_root,
                    rect(350.0, 30.0, 90.0, 70.0),
                ),
                PresentedWindowGeometry::with_scene_node(first_node, first_root, first_rect),
                PresentedWindowGeometry::with_scene_node(second_node, second_root, changed_rect),
            ],
        );
        state.publish_presented_presentation(4, &root_added);
        assert_eq!(state.pointer_hit_generation, pointer_generation + 1);
        pointer_generation = state.pointer_hit_generation;
        assert_eq!(state.interaction_topology_generation, topology_generation);

        let root_removed = physical_presentation_snapshot(
            output_id,
            AnimationTime::from_nanos(5_000_000),
            vec![
                PresentedWindowGeometry::with_scene_node(first_node, first_root, first_rect),
                PresentedWindowGeometry::with_scene_node(second_node, second_root, changed_rect),
            ],
        );
        state.publish_presented_presentation(5, &root_removed);
        assert_eq!(state.pointer_hit_generation, pointer_generation + 1);
        assert_eq!(state.interaction_topology_generation, topology_generation);
    }

    #[test]
    fn opacity_and_presentation_clip_only_do_not_invalidate_pointer_hits() {
        let mut state = CompositorState::new(None);
        let output_id = state.native_output_id().expect("test output id");
        let scene_node_id = SceneNodeId::from_raw(PRESENTATION_NODE).expect("scene node");
        let canonical_rect = rect(20.0, 30.0, 180.0, 120.0);
        let start_clip = PresentationClip::Rect(clip_rect(0.0, 0.0, 180.0, 120.0));
        let target_clip = PresentationClip::Rect(clip_rect(10.0, 10.0, 120.0, 90.0));
        state
            .presentation_animator
            .commit(presentation_request(
                scene_node_id,
                AnimationTime::from_nanos(0),
                (canonical_rect, canonical_rect),
                (1.0, 0.4),
                (start_clip, target_clip),
            ))
            .expect("opacity and clip animation");
        let physical_geometry = vec![PresentedWindowGeometry::with_scene_node(
            scene_node_id,
            PRESENTATION_ROOT,
            canonical_rect,
        )];
        let mut first = sample_presentation(
            &state,
            output_id,
            AnimationTime::from_nanos(2_000_000),
            canonical_rect,
        );
        first.presented_windows.clone_from(&physical_geometry);
        let mut second = sample_presentation(
            &state,
            output_id,
            AnimationTime::from_nanos(6_000_000),
            canonical_rect,
        );
        second.presented_windows.clone_from(&physical_geometry);
        assert_ne!(first.opacities, second.opacities);
        assert_ne!(first.clips, second.clips);

        state.publish_presented_presentation(1, &first);
        let pointer_generation = state.pointer_hit_generation;
        let topology_generation = state.interaction_topology_generation;
        state.publish_presented_presentation(2, &second);

        assert_eq!(state.pointer_hit_generation, pointer_generation);
        assert_eq!(state.interaction_topology_generation, topology_generation);
    }

    #[test]
    fn canonical_capture_rejects_misaligned_surface_identity_projections() {
        let output_id = OutputId::from_raw(1).expect("test output id");
        let scene_node_ids = [SceneNodeId::from_raw(1).expect("test scene node")];

        let snapshot = super::PresentedCanonicalSceneSnapshot::capture(
            output_id,
            1,
            0,
            &[],
            &scene_node_ids,
            &[],
            &[],
            &[],
        );

        assert!(snapshot.is_none());
    }

    #[test]
    fn mismatched_unified_publication_is_all_or_nothing() {
        let mut state = CompositorState::new(None);
        let output_id = state.native_output_id().expect("test output id");
        let initial_presentation = PresentationFrameSnapshot::empty_for_output(output_id);
        let existing_lifecycle = lifecycle_snapshot(
            WindowId::from_raw(700).expect("existing physical window id"),
            700,
            historical_identity(
                WindowId::from_raw(700).expect("existing physical window id"),
                70,
            ),
            LifecycleDirection::Minimize,
        );

        state.publish_presented_presentation(7, &initial_presentation);
        state.publish_presented_lifecycle(7, &existing_lifecycle);
        let previous_lifecycle = state
            .presented_lifecycle_physical
            .snapshot_for_test()
            .clone();

        let node = SceneNodeId::from_raw(PRESENTATION_NODE).expect("test scene node");
        state.presentation_animator.set_enabled(true);
        state
            .presentation_animator
            .commit(presentation_request(
                node,
                AnimationTime::from_nanos(0),
                (rect(0.0, 0.0, 100.0, 80.0), rect(10.0, 10.0, 110.0, 85.0)),
                (1.0, 0.5),
                (
                    PresentationClip::Rect(clip_rect(0.0, 0.0, 100.0, 80.0)),
                    PresentationClip::Rect(clip_rect(10.0, 10.0, 80.0, 60.0)),
                ),
            ))
            .expect("start all three presentation tracks");
        let settled_presentation = sample_presentation(
            &state,
            output_id,
            AnimationTime::from_nanos(10_000_000),
            rect(10.0, 10.0, 110.0, 85.0),
        );
        assert!(settled_presentation.transforms[0].mathematically_settled);
        assert!(
            settled_presentation.opacities[0]
                .transition
                .expect("active opacity evidence")
                .mathematically_settled
        );
        assert!(
            settled_presentation.clips[0]
                .transition
                .expect("active clip evidence")
                .mathematically_settled
        );

        let restore_window = WindowId::from_raw(501).expect("restore window id");
        let restore_root = 501;
        let restore_id = state
            .window_lifecycle_animator
            .start_or_reverse(
                lifecycle_request(
                    &mut state.presentation_animator,
                    &mut state.retained_lifecycle_payloads,
                    restore_window,
                    restore_root,
                    LifecycleDirection::Restore,
                ),
                None,
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("restore transition starts");
        let restore_evidence = lifecycle_snapshot(
            restore_window,
            restore_root,
            restore_id,
            LifecycleDirection::Restore,
        );

        let mismatched_output_id = OutputId::from_raw(output_id.get() + 1).expect("mismatch");
        let mut mismatched_presentation = settled_presentation.clone();
        mismatched_presentation.output_id = mismatched_output_id;
        let previous_pointer_hit_generation = state.pointer_hit_generation;

        state.publish_presented_frame(PresentedFramePublication {
            frame_id: 8,
            presentation: &mismatched_presentation,
            lifecycle: &restore_evidence,
            lifecycle_scene: PresentedLifecycleScene::RenderedSceneReplacement {
                canonical_root_surface_ids: &[restore_root],
            },
            canonical_scene: None,
            window_exits: &[],
        });

        assert_eq!(state.presented_presentation_frame_id(), 7);
        assert_eq!(state.presented_lifecycle_frame_id(), 7);
        assert_eq!(
            state.presented_presentation.as_ref(),
            Some(&initial_presentation)
        );
        assert_eq!(
            state.presented_lifecycle_physical.snapshot_for_test(),
            &previous_lifecycle
        );
        assert_eq!(state.presentation_animator.active_count(), 3);
        assert_eq!(state.presentation_animator.transaction_count(), 2);
        assert!(
            state
                .window_lifecycle_animator
                .sample(restore_id, AnimationTime::from_nanos(280_000_000))
                .is_some()
        );
        assert!(
            state
                .lifecycle_scene_sample_at(AnimationTime::from_nanos(0))
                .restore_suppresses_root(restore_root)
        );
        assert_eq!(
            state.pointer_hit_generation,
            previous_pointer_hit_generation
        );
    }

    #[test]
    fn initial_unified_publication_keeps_lifecycle_replacement_disabled() {
        let mut state = CompositorState::new(None);
        let output_id = state.native_output_id().expect("test output id");
        let presentation = PresentationFrameSnapshot::empty_for_output(output_id);
        let old_physical_lifecycle = lifecycle_snapshot_with_settlement(
            WindowId::from_raw(901).expect("old physical window id"),
            901,
            historical_identity(WindowId::from_raw(901).expect("old physical window id"), 91),
            LifecycleDirection::Minimize,
            false,
        );
        state.publish_presented_lifecycle(8, &old_physical_lifecycle);
        let lifecycle = LifecycleFrameSnapshot::default();
        let previous_pointer_hit_generation = state.pointer_hit_generation;

        state.publish_presented_frame(PresentedFramePublication {
            frame_id: 9,
            presentation: &presentation,
            lifecycle: &lifecycle,
            lifecycle_scene: PresentedLifecycleScene::Initial,
            canonical_scene: None,
            window_exits: &[],
        });

        assert_eq!(state.presented_presentation_frame_id(), 9);
        assert_eq!(state.presented_lifecycle_frame_id(), 9);
        assert_eq!(state.presented_presentation.as_ref(), Some(&presentation));
        assert_eq!(
            state
                .presented_lifecycle_physical
                .snapshot_for_test()
                .samples,
            old_physical_lifecycle.samples
        );
        assert_eq!(
            state
                .presented_lifecycle_physical
                .snapshot_for_test()
                .signature,
            old_physical_lifecycle.signature
        );
        assert_eq!(
            state.pointer_hit_generation,
            previous_pointer_hit_generation + 1,
            "only the first authoritative physical presentation changes input mapping"
        );
    }

    #[test]
    fn carried_physical_lamp_is_not_fresh_ack_evidence() {
        let mut state = CompositorState::new(None);
        let output_id = state.native_output_id().expect("test output id");
        let window_id = WindowId::from_raw(902).expect("lifecycle window id");
        let root_surface_id = 902;
        let identity = state
            .window_lifecycle_animator
            .start_or_reverse(
                lifecycle_request(
                    &mut state.presentation_animator,
                    &mut state.retained_lifecycle_payloads,
                    window_id,
                    root_surface_id,
                    LifecycleDirection::Minimize,
                ),
                None,
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("active lifecycle transition");
        let presentation = PresentationFrameSnapshot::empty_for_output(output_id);
        let submitted = LifecycleFrameSnapshot::from_sample(
            &state.lifecycle_scene_sample_at(AnimationTime::from_nanos(100_000_000)),
        );
        state.publish_presented_frame(PresentedFramePublication {
            frame_id: 1,
            presentation: &presentation,
            lifecycle: &submitted,
            lifecycle_scene: PresentedLifecycleScene::Initial,
            canonical_scene: None,
            window_exits: &[],
        });
        assert_eq!(state.window_lifecycle_animator.active_count(), 1);

        let empty_submission = LifecycleFrameSnapshot::default();
        state.publish_presented_frame(PresentedFramePublication {
            frame_id: 2,
            presentation: &presentation,
            lifecycle: &empty_submission,
            lifecycle_scene: PresentedLifecycleScene::Initial,
            canonical_scene: None,
            window_exits: &[],
        });

        let physical = state.presented_lifecycle_physical.snapshot_for_test();
        assert_eq!(physical.samples.len(), 1);
        assert_eq!(physical.samples[0].presentation_identity, identity);
        assert_eq!(state.presented_lifecycle_frame_id(), 2);
        assert_eq!(state.window_lifecycle_animator.active_count(), 1);
        assert_eq!(
            state.presentation_animator.active_retained_visual(
                identity.scene_node_id(),
                PresentationRetainedVisualKind::WindowLifecycle,
            ),
            Some(identity)
        );
        assert!(
            state
                .retained_lifecycle_payloads
                .get_exact(identity)
                .is_some()
        );
        assert!(
            state
                .presentation_animator
                .transaction_record(identity.transaction_id())
                .is_some()
        );
    }

    #[test]
    fn empty_and_minimize_lifecycle_publications_do_not_invalidate_pointer_hits() {
        let mut state = CompositorState::new(None);
        let output_id = state.native_output_id().expect("test output id");
        let presentation = PresentationFrameSnapshot::empty_for_output(output_id);
        state.publish_presented_presentation(1, &presentation);
        let pointer_generation = state.pointer_hit_generation;
        let topology_generation = state.interaction_topology_generation;

        state.publish_presented_lifecycle(2, &LifecycleFrameSnapshot::default());
        assert_eq!(state.pointer_hit_generation, pointer_generation);

        let window_id = WindowId::from_raw(903).expect("lifecycle window id");
        let root_surface_id = 903;
        let identity = state
            .window_lifecycle_animator
            .start_or_reverse(
                lifecycle_request(
                    &mut state.presentation_animator,
                    &mut state.retained_lifecycle_payloads,
                    window_id,
                    root_surface_id,
                    LifecycleDirection::Minimize,
                ),
                None,
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("minimize transition starts");
        let minimize = lifecycle_snapshot(
            window_id,
            root_surface_id,
            identity,
            LifecycleDirection::Minimize,
        );
        state.publish_presented_lifecycle(3, &minimize);

        assert_eq!(state.pointer_hit_generation, pointer_generation);
        assert_eq!(state.interaction_topology_generation, topology_generation);
    }

    #[test]
    fn unified_publication_acks_only_exact_settled_presentation_evidence() {
        let mut state = CompositorState::new(None);
        let output_id = state.native_output_id().expect("test output id");
        state.presentation_animator.set_enabled(true);
        let node = SceneNodeId::from_raw(PRESENTATION_NODE).expect("test scene node");
        let start = rect(0.0, 0.0, 100.0, 80.0);
        let first_target = rect(10.0, 10.0, 110.0, 85.0);
        let second_target = rect(20.0, 15.0, 120.0, 90.0);
        let start_clip = PresentationClip::Rect(clip_rect(0.0, 0.0, 100.0, 80.0));
        let first_clip = PresentationClip::Rect(clip_rect(10.0, 10.0, 80.0, 60.0));
        let second_clip = PresentationClip::Rect(clip_rect(20.0, 15.0, 90.0, 65.0));

        state
            .presentation_animator
            .commit(presentation_request(
                node,
                AnimationTime::from_nanos(0),
                (start, first_target),
                (1.0, 0.7),
                (start_clip, first_clip),
            ))
            .expect("first presentation transaction");
        let stale = sample_presentation(
            &state,
            output_id,
            AnimationTime::from_nanos(10_000_000),
            first_target,
        );
        state
            .presentation_animator
            .commit(presentation_request(
                node,
                AnimationTime::from_nanos(10_000_000),
                (first_target, second_target),
                (0.7, 0.4),
                (first_clip, second_clip),
            ))
            .expect("reversed presentation transaction");
        let settled = sample_presentation(
            &state,
            output_id,
            AnimationTime::from_nanos(20_000_000),
            second_target,
        );
        assert!(settled.transforms[0].mathematically_settled);
        assert!(
            settled.opacities[0]
                .transition
                .expect("active opacity evidence")
                .mathematically_settled
        );
        assert!(
            settled.clips[0]
                .transition
                .expect("active clip evidence")
                .mathematically_settled
        );

        state.publish_presented_frame(PresentedFramePublication {
            frame_id: 10,
            presentation: &stale,
            lifecycle: &LifecycleFrameSnapshot::default(),
            lifecycle_scene: PresentedLifecycleScene::Initial,
            canonical_scene: None,
            window_exits: &[],
        });
        assert_eq!(state.presentation_animator.active_count(), 3);
        assert_eq!(state.presentation_animator.transaction_count(), 1);

        state.publish_presented_frame(PresentedFramePublication {
            frame_id: 11,
            presentation: &settled,
            lifecycle: &LifecycleFrameSnapshot::default(),
            lifecycle_scene: PresentedLifecycleScene::Initial,
            canonical_scene: None,
            window_exits: &[],
        });
        assert_eq!(state.presentation_animator.active_count(), 0);
        assert_eq!(state.presentation_animator.transaction_count(), 0);
    }

    #[test]
    fn unified_publication_keeps_restore_suppressed_until_exact_physical_ack() {
        let mut state = CompositorState::new(None);
        let output_id = state.native_output_id().expect("test output id");
        let window_id = WindowId::from_raw(601).expect("restore window id");
        let root_surface_id = 601;
        let transition_id = state
            .window_lifecycle_animator
            .start_or_reverse(
                lifecycle_request(
                    &mut state.presentation_animator,
                    &mut state.retained_lifecycle_payloads,
                    window_id,
                    root_surface_id,
                    LifecycleDirection::Restore,
                ),
                None,
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("restore transition starts");
        let endpoint_time = AnimationTime::from_nanos(280_000_000);
        let endpoint = state
            .window_lifecycle_animator
            .sample(transition_id, endpoint_time)
            .expect("mathematical restore endpoint");
        assert!(endpoint.mathematically_settled);
        let lifecycle_sample = state.lifecycle_scene_sample_at(endpoint_time);
        assert!(lifecycle_sample.restore_suppresses_root(root_surface_id));
        assert_eq!(lifecycle_sample.samples.len(), 1);
        assert_eq!(
            lifecycle_sample.samples[0].payload_id,
            PresentationRetainedVisualPayloadId::from_origin_identity(transition_id)
        );
        let lifecycle = LifecycleFrameSnapshot::from_sample(&lifecycle_sample);
        assert!(lifecycle.samples[0].mathematically_settled);
        let presentation = PresentationFrameSnapshot::empty_for_output(output_id);
        state.publish_presented_presentation(11, &presentation);
        let pointer_generation_before_ack = state.pointer_hit_generation;
        let topology_generation_before_ack = state.interaction_topology_generation;
        let metrics_before_ack = state.pointer_input_metrics_snapshot();
        state.publish_presented_frame(PresentedFramePublication {
            frame_id: 12,
            presentation: &presentation,
            lifecycle: &lifecycle,
            lifecycle_scene: PresentedLifecycleScene::RenderedSceneReplacement {
                canonical_root_surface_ids: &[root_surface_id],
            },
            canonical_scene: None,
            window_exits: &[],
        });

        assert!(
            state
                .window_lifecycle_animator
                .sample(transition_id, endpoint_time)
                .is_none()
        );
        assert!(!state.lifecycle_root_restore_suppressed(root_surface_id));
        assert_eq!(state.presented_presentation_frame_id(), 12);
        assert_eq!(state.presented_lifecycle_frame_id(), 12);
        assert_eq!(
            state.pointer_hit_generation,
            pointer_generation_before_ack + 1
        );
        assert_eq!(
            state.interaction_topology_generation,
            topology_generation_before_ack
        );
        assert_eq!(
            state
                .pointer_input_metrics_snapshot()
                .pointer_hit_generation_invalidations,
            metrics_before_ack.pointer_hit_generation_invalidations + 1
        );
        assert_eq!(
            state
                .presented_lifecycle_physical
                .snapshot_for_test()
                .samples[0]
                .presentation_identity,
            transition_id
        );
    }

    #[test]
    fn unified_publication_rejects_stale_lifecycle_reversal_evidence() {
        let mut state = CompositorState::new(None);
        let output_id = state.native_output_id().expect("test output id");
        let window_id = WindowId::from_raw(801).expect("lifecycle window id");
        let root_surface_id = 801;
        let first = state
            .window_lifecycle_animator
            .start_or_reverse(
                lifecycle_request(
                    &mut state.presentation_animator,
                    &mut state.retained_lifecycle_payloads,
                    window_id,
                    root_surface_id,
                    LifecycleDirection::Minimize,
                ),
                None,
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("first lifecycle transition starts");
        let active = state
            .window_lifecycle_animator
            .start_or_reverse(
                lifecycle_request(
                    &mut state.presentation_animator,
                    &mut state.retained_lifecycle_payloads,
                    window_id,
                    root_surface_id,
                    LifecycleDirection::Restore,
                ),
                Some(first),
                AnimationTime::from_nanos(1),
                1.0,
            )
            .expect("reversal starts");
        assert_ne!(first, active);
        assert!(
            state
                .presentation_animator
                .retire_retained_visual_exact(first)
        );
        assert!(
            state
                .presentation_animator
                .transaction_record(first.transaction_id())
                .is_none()
        );
        assert!(
            state
                .presentation_animator
                .transaction_record(active.transaction_id())
                .is_some()
        );
        let presentation = PresentationFrameSnapshot::empty_for_output(output_id);
        let stale = lifecycle_snapshot(
            window_id,
            root_surface_id,
            first,
            LifecycleDirection::Minimize,
        );

        state.publish_presented_frame(PresentedFramePublication {
            frame_id: 13,
            presentation: &presentation,
            lifecycle: &stale,
            lifecycle_scene: PresentedLifecycleScene::RenderedSceneReplacement {
                canonical_root_surface_ids: &[root_surface_id],
            },
            canonical_scene: None,
            window_exits: &[],
        });
        assert_eq!(
            state
                .window_lifecycle_animator
                .sample(active, AnimationTime::from_nanos(280_000_000))
                .expect("new reversal remains active")
                .presentation_identity,
            active
        );
        assert!(
            state
                .presentation_animator
                .transaction_record(active.transaction_id())
                .is_some()
        );

        let mut exact = lifecycle_snapshot(
            window_id,
            root_surface_id,
            active,
            LifecycleDirection::Restore,
        );
        exact.samples[0].payload_id = state
            .retained_lifecycle_payloads
            .get_exact(active)
            .expect("reversal retains first payload")
            .payload_id;
        exact.refresh_signature();
        state.publish_presented_frame(PresentedFramePublication {
            frame_id: 14,
            presentation: &presentation,
            lifecycle: &exact,
            lifecycle_scene: PresentedLifecycleScene::RenderedSceneReplacement {
                canonical_root_surface_ids: &[root_surface_id],
            },
            canonical_scene: None,
            window_exits: &[],
        });
        assert!(
            state
                .window_lifecycle_animator
                .sample(active, AnimationTime::from_nanos(280_000_000))
                .is_none()
        );
        assert!(
            state
                .presentation_animator
                .transaction_record(active.transaction_id())
                .is_none()
        );
    }
}
