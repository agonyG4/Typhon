use super::{CompositorState, PresentationFrameSnapshot};
use crate::window_lifecycle_animation::LifecycleFrameSnapshot;

pub struct PresentedFramePublication<'a> {
    pub frame_id: u64,
    pub presentation: &'a PresentationFrameSnapshot,
    pub lifecycle: &'a LifecycleFrameSnapshot,
    pub lifecycle_scene: PresentedLifecycleScene<'a>,
}

pub enum PresentedLifecycleScene<'a> {
    Initial,
    RenderedSceneReplacement {
        canonical_root_surface_ids: &'a [u32],
    },
}

impl CompositorState {
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

        self.publish_presented_presentation(publication.frame_id, publication.presentation);
        match publication.lifecycle_scene {
            PresentedLifecycleScene::Initial => {
                self.publish_presented_lifecycle(publication.frame_id, publication.lifecycle);
            }
            PresentedLifecycleScene::RenderedSceneReplacement {
                canonical_root_surface_ids,
            } => {
                self.publish_presented_lifecycle_with_replacements(
                    publication.frame_id,
                    publication.lifecycle,
                    canonical_root_surface_ids,
                    true,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::compositor::DecorationRenderInstance;
    use crate::core::{OutputId, SceneNodeId, WindowId};
    use crate::presentation_animation::{
        AnimationCurve, AnimationTime, EasingCurve, PresentationClip, PresentationClipMutation,
        PresentationClipRect, PresentationFrameSnapshot, PresentationGeometryMutation,
        PresentationOpacity, PresentationOpacityMutation, PresentationSampleTimeSource,
        PresentationTransactionRequest, PresentationWindowTarget,
    };
    use crate::window_lifecycle_animation::{
        LifecycleDirection, LifecycleFrameLamp, LifecycleFrameSnapshot, LifecycleTransitionId,
        LifecycleTransitionRequest, LifecycleVisualGroup,
    };
    use std::time::Duration;

    const PRESENTATION_NODE: u64 = 71;
    const PRESENTATION_ROOT: u32 = 71;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
        PresentationRect::new(x, y, width, height).expect("valid test rectangle")
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
        transition_id: LifecycleTransitionId,
        direction: LifecycleDirection,
    ) -> LifecycleFrameSnapshot {
        lifecycle_snapshot_with_settlement(
            window_id,
            root_surface_id,
            transition_id,
            direction,
            true,
        )
    }

    fn lifecycle_snapshot_with_settlement(
        window_id: WindowId,
        root_surface_id: u32,
        transition_id: LifecycleTransitionId,
        direction: LifecycleDirection,
        mathematically_settled: bool,
    ) -> LifecycleFrameSnapshot {
        let mut snapshot = LifecycleFrameSnapshot {
            sampled_at: Some(AnimationTime::from_nanos(280_000_000)),
            lamps: vec![LifecycleFrameLamp {
                window_id,
                root_surface_id,
                transition_id,
                visual_group: lifecycle_visual_group(),
                progress: if direction == LifecycleDirection::Restore {
                    0.0
                } else {
                    1.0
                },
                opacity: 1.0,
                mathematically_settled,
                direction,
            }],
            signature: 0,
        };
        snapshot.refresh_signature();
        snapshot
    }

    fn lifecycle_request(
        window_id: WindowId,
        root_surface_id: u32,
        direction: LifecycleDirection,
    ) -> LifecycleTransitionRequest {
        LifecycleTransitionRequest {
            window_id,
            root_surface_id,
            visual_group: lifecycle_visual_group(),
            direction,
            resolved_effect_scene: ResolvedEffectScene::default(),
        }
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
    fn mismatched_unified_publication_is_all_or_nothing() {
        let mut state = CompositorState::new(None);
        let output_id = state.native_output_id().expect("test output id");
        let initial_presentation = PresentationFrameSnapshot::empty_for_output(output_id);
        let existing_lifecycle = lifecycle_snapshot(
            WindowId::from_raw(700).expect("existing physical window id"),
            700,
            LifecycleTransitionId::new(70),
            LifecycleDirection::Minimize,
        );

        state.publish_presented_presentation(7, &initial_presentation);
        state.publish_presented_lifecycle(7, &existing_lifecycle);
        let previous_lifecycle = state.presented_lifecycle.clone();

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
                lifecycle_request(restore_window, restore_root, LifecycleDirection::Restore),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("restore transition starts");
        state.lifecycle_render_suppressed_roots.insert(restore_root);
        state.lifecycle_decorations.insert(
            restore_root,
            DecorationRenderInstance::test_solid(
                restore_window,
                restore_root,
                0,
                0,
                200,
                150,
                [11, 22, 33, 255],
            ),
        );
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
        let previous_decoration = state
            .lifecycle_decorations
            .get(&restore_root)
            .expect("restore decoration") as *const _;

        state.publish_presented_frame(PresentedFramePublication {
            frame_id: 8,
            presentation: &mismatched_presentation,
            lifecycle: &restore_evidence,
            lifecycle_scene: PresentedLifecycleScene::RenderedSceneReplacement {
                canonical_root_surface_ids: &[restore_root],
            },
        });

        assert_eq!(state.presented_presentation_frame_id(), 7);
        assert_eq!(state.presented_lifecycle_frame_id(), 7);
        assert_eq!(
            state.presented_presentation.as_ref(),
            Some(&initial_presentation)
        );
        assert_eq!(state.presented_lifecycle, previous_lifecycle);
        assert_eq!(state.presentation_animator.active_count(), 3);
        assert_eq!(state.presentation_animator.transaction_count(), 1);
        assert!(
            state
                .window_lifecycle_animator
                .sample(restore_window, AnimationTime::from_nanos(280_000_000))
                .is_some()
        );
        assert!(
            state
                .lifecycle_render_suppressed_roots
                .contains(&restore_root)
        );
        assert!(std::ptr::eq(
            state
                .lifecycle_decorations
                .get(&restore_root)
                .expect("restore decoration remains"),
            previous_decoration,
        ));
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
            LifecycleTransitionId::new(91),
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
        });

        assert_eq!(state.presented_presentation_frame_id(), 9);
        assert_eq!(state.presented_lifecycle_frame_id(), 9);
        assert_eq!(state.presented_presentation.as_ref(), Some(&presentation));
        assert_eq!(
            state.presented_lifecycle.lamps,
            old_physical_lifecycle.lamps
        );
        assert_eq!(
            state.presented_lifecycle.signature,
            old_physical_lifecycle.signature
        );
        assert_eq!(
            state.pointer_hit_generation,
            previous_pointer_hit_generation + 2,
            "Presentation and Lifecycle each keep their pointer-hit invalidation"
        );
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
        });
        assert_eq!(state.presentation_animator.active_count(), 3);
        assert_eq!(state.presentation_animator.transaction_count(), 1);

        state.publish_presented_frame(PresentedFramePublication {
            frame_id: 11,
            presentation: &settled,
            lifecycle: &LifecycleFrameSnapshot::default(),
            lifecycle_scene: PresentedLifecycleScene::Initial,
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
                lifecycle_request(window_id, root_surface_id, LifecycleDirection::Restore),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("restore transition starts");
        state
            .lifecycle_render_suppressed_roots
            .insert(root_surface_id);
        state.lifecycle_decorations.insert(
            root_surface_id,
            DecorationRenderInstance::test_solid(
                window_id,
                root_surface_id,
                0,
                0,
                200,
                150,
                [44, 55, 66, 255],
            ),
        );

        let endpoint_time = AnimationTime::from_nanos(280_000_000);
        let endpoint = state
            .window_lifecycle_animator
            .sample(window_id, endpoint_time)
            .expect("mathematical restore endpoint");
        assert!(endpoint.mathematically_settled);
        assert!(
            state
                .lifecycle_render_suppressed_roots
                .contains(&root_surface_id)
        );
        assert!(state.lifecycle_decorations.contains_key(&root_surface_id));

        let lifecycle_sample = state.window_lifecycle_animator.sample_scene(endpoint_time);
        let lifecycle = LifecycleFrameSnapshot::from_sample(&lifecycle_sample);
        assert!(lifecycle.lamps[0].mathematically_settled);
        let presentation = PresentationFrameSnapshot::empty_for_output(output_id);
        state.publish_presented_frame(PresentedFramePublication {
            frame_id: 12,
            presentation: &presentation,
            lifecycle: &lifecycle,
            lifecycle_scene: PresentedLifecycleScene::RenderedSceneReplacement {
                canonical_root_surface_ids: &[root_surface_id],
            },
        });

        assert!(
            state
                .window_lifecycle_animator
                .sample(window_id, endpoint_time)
                .is_none()
        );
        assert!(
            !state
                .lifecycle_render_suppressed_roots
                .contains(&root_surface_id)
        );
        assert!(!state.lifecycle_decorations.contains_key(&root_surface_id));
        assert_eq!(state.presented_presentation_frame_id(), 12);
        assert_eq!(state.presented_lifecycle_frame_id(), 12);
        assert_eq!(
            state.presented_lifecycle.lamps[0].transition_id,
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
                lifecycle_request(window_id, root_surface_id, LifecycleDirection::Minimize),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("first lifecycle transition starts");
        let active = state
            .window_lifecycle_animator
            .start_or_reverse(
                lifecycle_request(window_id, root_surface_id, LifecycleDirection::Restore),
                AnimationTime::from_nanos(1),
                1.0,
            )
            .expect("reversal starts");
        assert_ne!(first, active);
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
        });
        assert_eq!(
            state
                .window_lifecycle_animator
                .sample(window_id, AnimationTime::from_nanos(280_000_000))
                .expect("new reversal remains active")
                .transition_id,
            active
        );

        let exact = lifecycle_snapshot(
            window_id,
            root_surface_id,
            active,
            LifecycleDirection::Restore,
        );
        state.publish_presented_frame(PresentedFramePublication {
            frame_id: 14,
            presentation: &presentation,
            lifecycle: &exact,
            lifecycle_scene: PresentedLifecycleScene::RenderedSceneReplacement {
                canonical_root_surface_ids: &[root_surface_id],
            },
        });
        assert!(
            state
                .window_lifecycle_animator
                .sample(window_id, AnimationTime::from_nanos(280_000_000))
                .is_none()
        );
    }
}
