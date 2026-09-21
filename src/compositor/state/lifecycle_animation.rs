use super::*;
use crate::compositor::decoration::types::DecorationMode;
use crate::animation_control::{AnimationEffect, AnimationRuntimeCapabilities, AnimationSlot};
use crate::presentation_animation::{PresentationGroupTransform, TransitionId};
use crate::window_lifecycle_animation::{
    LifecycleDirection, LifecycleFrameSnapshot, LifecycleRenderFallbackEntry, LifecycleSceneSample,
    LifecycleTransitionRequest, LifecycleVisualGroup,
};
use std::num::NonZeroU64;

impl CompositorState {
    /// Freeze only the compositor-owned effect instances belonging to one
    /// lifecycle visual group before logical minimize removes that group from
    /// the canonical scene. Output-wide effects intentionally do not become a
    /// lifecycle source; they remain owned by the normal output composition.
    pub(in crate::compositor) fn resolved_effect_scene_for_lifecycle_root(
        &self,
        root_surface_id: u32,
    ) -> ResolvedEffectScene {
        let scene = self.resolved_effect_scene();
        let instances = scene
            .instances
            .into_iter()
            .filter(|instance| match instance.anchor {
                EffectAnchor::BeforeSurface(surface_id)
                | EffectAnchor::ReplaceSurface(surface_id)
                | EffectAnchor::AfterSurface(surface_id) => {
                    self.root_surface_id_for_surface(surface_id) == root_surface_id
                }
                EffectAnchor::OutputPostProcess => false,
            })
            .collect();
        ResolvedEffectScene::new(scene.generation, instances)
    }

    pub(in crate::compositor) fn map_effect_scene_to_presentation(
        scene: &ResolvedEffectScene,
        canonical_rect: PresentationRect,
        presented_rect: PresentationRect,
    ) -> ResolvedEffectScene {
        let transform = PresentationGroupTransform::with_scene_node(
            crate::core::SceneNodeId::from_raw(1).expect("effect scene node"),
            0,
            crate::presentation_animation::PresentationTransactionId::new(NonZeroU64::MIN),
            TransitionId::new(NonZeroU64::MIN),
            canonical_rect,
            presented_rect,
            false,
        );
        let instances = scene
            .instances
            .iter()
            .cloned()
            .map(|mut instance| {
                instance.region = super::super::effects::map_effect_region(
                    transform,
                    &instance.region,
                    instance.target_bounds,
                );
                if let Some(target_bounds) =
                    super::super::effects::map_effect_rect(transform, instance.target_bounds)
                {
                    instance.target_bounds = target_bounds;
                }
                instance.signature =
                    instance.signature.wrapping_mul(0x0000_0100_0000_01b3) ^ transform.signature();
                instance
            })
            .collect();
        ResolvedEffectScene::new(scene.generation, instances)
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use crate::compositor::DecorationRenderInstance;
    use crate::compositor::decoration::types::DecorationPreference;
    use crate::render_backend::buffer::{BufferIdAllocator, BufferSize, CommittedSurfaceBuffer};
    use crate::window_lifecycle_animation::{
        LampWindowSample, LifecycleFrameSnapshot, LifecycleRenderEvidence,
        LifecycleRenderEvidenceEntry, LifecycleRenderFallbackEntry, LifecycleRenderFallbackReason,
        LifecycleTransitionId, LifecycleTransitionRequest,
    };

    fn lifecycle_decoration(
        window_id: WindowId,
        root_surface_id: u32,
        visual_signature: u8,
    ) -> DecorationRenderInstance {
        DecorationRenderInstance::test_solid(
            window_id,
            root_surface_id,
            0,
            0,
            832,
            640,
            [visual_signature, 0, 0, 255],
        )
    }

    fn visual_group(visual_rect: PresentationRect) -> LifecycleVisualGroup {
        LifecycleVisualGroup::from_bounds(
            rect(400.0, 100.0, 800.0, 600.0),
            visual_rect,
            rect(200.0, 160.0, 960.0, 720.0),
            rect(1500.0, 500.0, 64.0, 64.0),
            1920,
            1080,
        )
        .expect("valid test visual group")
    }

    fn ssd_test_surface(surface_id: u32) -> RenderableSurface {
        let buffer_id = BufferIdAllocator::default()
            .allocate()
            .expect("test buffer identity");
        RenderableSurface {
            surface_id,
            x: 0,
            y: 0,
            width: 300,
            height: 200,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: CommittedSurfaceBuffer::shm_snapshot(
                buffer_id,
                BufferSize::new(300, 200).expect("test buffer size"),
                vec![0xff12_3456; 300 * 200],
            ),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
            damage: RenderableSurfaceDamage::Full,
        }
    }

    fn ssd_test_state(surface_id: u32) -> (CompositorState, WindowId) {
        let mut state = CompositorState::new(None);
        let window_id = state.allocate_window_id().expect("test window id");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
            .expect("test XDG window");
        let mut decoration_state = WindowDecorationState::new();
        decoration_state.set_preference(DecorationPreference::ServerSide);
        decoration_state.apply_configured_mode(DecorationMode::ServerSide);
        state
            .xdg_decoration_states
            .insert(surface_id, decoration_state);
        state.append_renderable_surface(ssd_test_surface(surface_id));
        state.rebuild_active_scene_view();
        (state, window_id)
    }

    fn lifecycle_decoration_signature(
        state: &CompositorState,
        root_surface_id: u32,
        surfaces: &[RenderableSurface],
        at: AnimationTime,
    ) -> Option<u64> {
        let sample = state.lifecycle_scene_sample_at(at);
        state
            .lifecycle_decoration_render_instances(&sample, surfaces)
            .into_iter()
            .find(|decoration| decoration.root_surface_id() == root_surface_id)
            .map(|decoration| decoration.scene_snapshot().visual_signature())
    }

    fn settle_lifecycle_transition(state: &mut CompositorState, window_id: WindowId) {
        let now = AnimationTime::monotonic_now().expect("monotonic test time");
        let active = state
            .window_lifecycle_animator
            .sample(window_id, now)
            .expect("active lifecycle transition");
        assert!(state.window_lifecycle_animator.snap_to_endpoint(
            window_id,
            active.transition_id,
            now,
        ));
        let sample = state.lifecycle_scene_sample_at(now);
        let evidence = LifecycleRenderEvidence::from_consumed([LifecycleRenderEvidenceEntry {
            window_id,
            root_surface_id: active.root_surface_id,
            transition_id: active.transition_id,
        }]);
        let snapshot = LifecycleFrameSnapshot::qualified_from_sample(&sample, &evidence);
        state.publish_presented_lifecycle(1, &snapshot);
    }

    fn lifecycle_request(
        window_id: WindowId,
        root_surface_id: u32,
        source_rect: PresentationRect,
        anchor_rect: PresentationRect,
        direction: LifecycleDirection,
    ) -> LifecycleTransitionRequest {
        LifecycleTransitionRequest {
            window_id,
            root_surface_id,
            visual_group: LifecycleVisualGroup::from_bounds(
                source_rect,
                source_rect,
                source_rect,
                anchor_rect,
                1920,
                1080,
            )
            .expect("valid test visual group"),
            direction,
            resolved_effect_scene: ResolvedEffectScene::default(),
        }
    }

    fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
        PresentationRect::new(x, y, width, height).expect("valid test rectangle")
    }

    #[test]
    fn unconsumed_endpoint_pageflip_does_not_retire_lifecycle_transition() {
        let window_id = WindowId::from_raw(301).expect("valid window ID");
        let mut state = CompositorState {
            lifecycle_animation_renderer_available: Some(true),
            ..Default::default()
        };
        let source = rect(20.0, 20.0, 200.0, 150.0);
        let anchor = rect(500.0, 500.0, 40.0, 40.0);
        let transition_id = state
            .window_lifecycle_animator
            .start_or_reverse(
                lifecycle_request(window_id, 301, source, anchor, LifecycleDirection::Minimize),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("Lamp transition starts");
        let endpoint = state
            .window_lifecycle_animator
            .sample_scene(AnimationTime::from_nanos(280_000_000));

        state.publish_presented_lifecycle(1, &LifecycleFrameSnapshot::default());
        assert_eq!(state.window_lifecycle_animator.active_count(), 1);

        let evidence = LifecycleRenderEvidence::from_consumed([LifecycleRenderEvidenceEntry {
            window_id,
            root_surface_id: 301,
            transition_id,
        }]);
        let qualified = LifecycleFrameSnapshot::qualified_from_sample(&endpoint, &evidence);
        state.publish_presented_lifecycle(2, &qualified);
        assert_eq!(state.window_lifecycle_animator.active_count(), 0);
    }

    #[test]
    fn off_output_transition_settles_without_pageflip_and_does_not_block_scheduler() {
        let window_id = WindowId::from_raw(302).expect("valid window ID");
        let mut state = CompositorState {
            output_size: OutputSize::new(100, 100),
            lifecycle_animation_renderer_available: Some(true),
            ..Default::default()
        };
        let transition_id = state
            .window_lifecycle_animator
            .start_or_reverse(
                lifecycle_request(
                    window_id,
                    302,
                    rect(300.0, 300.0, 100.0, 100.0),
                    rect(500.0, 500.0, 20.0, 20.0),
                    LifecycleDirection::Restore,
                ),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("Lamp transition starts");
        state.lifecycle_render_suppressed_roots.insert(302);
        assert!(!state.lifecycle_animation_has_pending_visible());
        assert!(state.settle_lifecycle_no_visual_change());
        assert_eq!(state.window_lifecycle_animator.active_count(), 0);
        assert!(!state.lifecycle_render_suppressed_roots.contains(&302));
        assert!(!state.lifecycle_animation_has_pending_visible());
        assert!(
            !state
                .window_lifecycle_animator
                .acknowledge(window_id, transition_id, true)
        );
    }

    #[test]
    fn exact_invisible_lamp_endpoint_can_settle_without_visual_change() {
        let window_id = WindowId::from_raw(309).expect("valid window ID");
        let mut state = CompositorState {
            output_size: OutputSize::new(100, 100),
            lifecycle_animation_renderer_available: Some(true),
            ..Default::default()
        };
        let transition_id = state
            .window_lifecycle_animator
            .start_or_reverse(
                lifecycle_request(
                    window_id,
                    309,
                    rect(10.0, 10.0, 60.0, 60.0),
                    rect(20.0, 20.0, 20.0, 20.0),
                    LifecycleDirection::Minimize,
                ),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("Lamp transition starts");
        assert!(state.window_lifecycle_animator.snap_to_endpoint(
            window_id,
            transition_id,
            AnimationTime::monotonic_now().expect("monotonic time"),
        ));

        assert!(!state.lifecycle_animation_has_pending_visible());
        assert!(state.settle_lifecycle_no_visual_change());
        assert_eq!(state.window_lifecycle_animator.active_count(), 0);
    }

    #[test]
    fn old_visible_physical_lamp_prevents_no_visual_settlement() {
        let window_id = WindowId::from_raw(303).expect("valid window ID");
        let mut state = CompositorState {
            output_size: OutputSize::new(100, 100),
            lifecycle_animation_renderer_available: Some(true),
            ..Default::default()
        };
        state
            .window_lifecycle_animator
            .start_or_reverse(
                lifecycle_request(
                    window_id,
                    303,
                    rect(300.0, 300.0, 100.0, 100.0),
                    rect(500.0, 500.0, 20.0, 20.0),
                    LifecycleDirection::Minimize,
                ),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("Lamp transition starts");
        let old = LifecycleSceneSample {
            sampled_at: AnimationTime::from_nanos(0),
            lamps: vec![LampWindowSample {
                window_id,
                root_surface_id: 303,
                transition_id: LifecycleTransitionId::new(99),
                visual_group: LifecycleVisualGroup::from_bounds(
                    rect(0.0, 0.0, 80.0, 80.0),
                    rect(0.0, 0.0, 80.0, 80.0),
                    rect(0.0, 0.0, 80.0, 80.0),
                    rect(20.0, 20.0, 20.0, 20.0),
                    100,
                    100,
                )
                .expect("valid visual group"),
                progress: 0.5,
                opacity: 1.0,
                direction: LifecycleDirection::Minimize,
                mathematically_settled: false,
            }],
            visual_sources: Vec::new(),
        };
        state.presented_lifecycle = LifecycleFrameSnapshot::from_sample(&old);
        assert!(!state.settle_lifecycle_no_visual_change());
        assert!(!state.settle_lifecycle_no_visual_change());
        assert_eq!(state.window_lifecycle_animator.active_count(), 1);
        assert!(state.lifecycle_animation_has_pending_visible());
        assert_eq!(
            state.direct_scanout_scene_candidate().unwrap_err(),
            DirectScanoutSceneRejection::LifecycleAnimation
        );
    }

    #[test]
    fn lifecycle_render_fallback_preserves_confirmed_physical_lamp_until_replacement() {
        let window_id = WindowId::from_raw(308).expect("valid window ID");
        let root_surface_id = 308;
        let mut state = CompositorState {
            output_size: OutputSize::new(100, 100),
            lifecycle_animation_renderer_available: Some(true),
            ..Default::default()
        };
        let transition_id = state
            .window_lifecycle_animator
            .start_or_reverse(
                lifecycle_request(
                    window_id,
                    root_surface_id,
                    rect(0.0, 0.0, 80.0, 80.0),
                    rect(20.0, 20.0, 20.0, 20.0),
                    LifecycleDirection::Minimize,
                ),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("Lamp transition starts");
        let physical = LifecycleSceneSample {
            sampled_at: AnimationTime::from_nanos(100_000_000),
            lamps: vec![LampWindowSample {
                window_id,
                root_surface_id,
                transition_id,
                visual_group: LifecycleVisualGroup::from_bounds(
                    rect(0.0, 0.0, 80.0, 80.0),
                    rect(0.0, 0.0, 80.0, 80.0),
                    rect(0.0, 0.0, 80.0, 80.0),
                    rect(20.0, 20.0, 20.0, 20.0),
                    100,
                    100,
                )
                .expect("valid visual group"),
                progress: 0.5,
                opacity: 1.0,
                direction: LifecycleDirection::Minimize,
                mathematically_settled: false,
            }],
            visual_sources: Vec::new(),
        };
        state.publish_presented_lifecycle(1, &LifecycleFrameSnapshot::from_sample(&physical));
        let confirmed = state.presented_lifecycle.clone();
        state
            .lifecycle_render_suppressed_roots
            .insert(root_surface_id);

        assert!(
            state.apply_lifecycle_render_fallback(LifecycleRenderFallbackEntry {
                window_id,
                root_surface_id,
                transition_id,
                reason: LifecycleRenderFallbackReason::LampProgramUnavailable,
            })
        );
        assert_eq!(state.window_lifecycle_animator.active_count(), 0);
        assert_eq!(state.presented_lifecycle, confirmed);
        assert_eq!(state.presented_lifecycle_frame_id, 1);
        assert!(state.lifecycle_animation_has_pending_visible());
        assert!(state.has_unowned_frame_work());

        state.publish_presented_lifecycle_with_replacements(
            2,
            &LifecycleFrameSnapshot::default(),
            &[root_surface_id],
            true,
        );
        assert!(state.presented_lifecycle.lamps.is_empty());
        assert_eq!(state.presented_lifecycle_frame_id, 2);
        assert!(!state.lifecycle_animation_has_pending_visible());
    }

    #[test]
    fn canonical_presentation_replaces_old_physical_lamp_after_logical_cancel() {
        let window_id = WindowId::from_raw(304).expect("valid window ID");
        let mut state = CompositorState {
            output_size: OutputSize::new(100, 100),
            lifecycle_animation_renderer_available: Some(true),
            ..Default::default()
        };
        let old = LifecycleSceneSample {
            sampled_at: AnimationTime::from_nanos(0),
            lamps: vec![LampWindowSample {
                window_id,
                root_surface_id: 304,
                transition_id: LifecycleTransitionId::new(100),
                visual_group: LifecycleVisualGroup::from_bounds(
                    rect(0.0, 0.0, 80.0, 80.0),
                    rect(0.0, 0.0, 80.0, 80.0),
                    rect(0.0, 0.0, 80.0, 80.0),
                    rect(20.0, 20.0, 20.0, 20.0),
                    100,
                    100,
                )
                .expect("valid visual group"),
                progress: 0.5,
                opacity: 1.0,
                direction: LifecycleDirection::Minimize,
                mathematically_settled: false,
            }],
            visual_sources: Vec::new(),
        };
        state.presented_lifecycle = LifecycleFrameSnapshot::from_sample(&old);
        state.publish_presented_lifecycle_with_replacements(2, &Default::default(), &[304], true);
        assert!(state.presented_lifecycle.lamps.is_empty());
    }

    #[test]
    fn rendered_replacement_clears_absent_physical_lamp_without_acknowledging_active_transition() {
        let window_id = WindowId::from_raw(305).expect("valid window ID");
        let mut state = CompositorState {
            output_size: OutputSize::new(100, 100),
            lifecycle_animation_renderer_available: Some(true),
            ..Default::default()
        };
        state
            .window_lifecycle_animator
            .start_or_reverse(
                lifecycle_request(
                    window_id,
                    305,
                    rect(0.0, 0.0, 80.0, 80.0),
                    rect(20.0, 20.0, 20.0, 20.0),
                    LifecycleDirection::Minimize,
                ),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("Lamp transition starts");
        let physical = state
            .window_lifecycle_animator
            .sample_scene(AnimationTime::from_nanos(100_000_000));
        state.presented_lifecycle = LifecycleFrameSnapshot::from_sample(&physical);
        state.publish_presented_lifecycle_with_replacements(2, &Default::default(), &[], true);
        assert!(state.presented_lifecycle.lamps.is_empty());
        assert_eq!(state.window_lifecycle_animator.active_count(), 1);
        assert_eq!(
            state
                .window_lifecycle_animator
                .sample_scene(AnimationTime::from_nanos(100_000_000))
                .lamps
                .len(),
            1
        );
    }

    #[test]
    fn runtime_slot_change_snaps_minimize_and_restore_but_retains_physical_ownership() {
        let directory = std::env::temp_dir().join(format!(
            "typhon-lifecycle-policy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        std::fs::create_dir(&directory).expect("create animation configuration directory");
        let mut state = CompositorState {
            lifecycle_animation_renderer_available: Some(true),
            ..Default::default()
        };
        state.animation_control = crate::animation_control::AnimationControlState::from_store(
            crate::animation_control::AnimationConfigurationStore::new(directory.clone())
                .expect("create animation configuration store"),
        );
        let window_id = WindowId::from_raw(306).expect("valid window ID");
        let minimize_id = state
            .window_lifecycle_animator
            .start_or_reverse(
                lifecycle_request(
                    window_id,
                    306,
                    rect(0.0, 0.0, 80.0, 80.0),
                    rect(200.0, 200.0, 20.0, 20.0),
                    LifecycleDirection::Minimize,
                ),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("minimize starts");
        let mut candidate = state.animation_control.configuration().clone();
        candidate
            .overrides
            .insert(AnimationSlot::WindowMinimize, AnimationEffect::None);
        state
            .set_animation_configuration(candidate)
            .expect("runtime policy mutation persists");
        let minimize = state
            .window_lifecycle_animator
            .sample(window_id, AnimationTime::from_nanos(0))
            .expect("snapped minimize remains owned");
        assert_eq!(minimize.transition_id, minimize_id);
        assert_eq!(minimize.progress, 1.0);
        assert!(minimize.mathematically_settled);
        let minimize_sample = state.lifecycle_scene_sample_at(AnimationTime::from_nanos(0));
        let minimize_frame = LifecycleFrameSnapshot::qualified_from_sample(
            &minimize_sample,
            &LifecycleRenderEvidence::from_consumed([LifecycleRenderEvidenceEntry {
                window_id,
                root_surface_id: 306,
                transition_id: minimize_id,
            }]),
        );
        state.publish_presented_lifecycle(1, &minimize_frame);
        assert_eq!(state.window_lifecycle_animator.active_count(), 0);

        let restore_id = state
            .window_lifecycle_animator
            .start_or_reverse(
                lifecycle_request(
                    window_id,
                    306,
                    rect(0.0, 0.0, 80.0, 80.0),
                    rect(200.0, 200.0, 20.0, 20.0),
                    LifecycleDirection::Restore,
                ),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("restore starts");
        state.lifecycle_render_suppressed_roots.insert(306);
        let mut candidate = state.animation_control.configuration().clone();
        candidate
            .overrides
            .insert(AnimationSlot::WindowRestore, AnimationEffect::None);
        state
            .set_animation_configuration(candidate)
            .expect("restore policy mutation persists");
        let restore = state
            .window_lifecycle_animator
            .sample(window_id, AnimationTime::from_nanos(0))
            .expect("snapped restore remains owned");
        assert_eq!(restore.transition_id, restore_id);
        assert_eq!(restore.progress, 0.0);
        assert!(state.lifecycle_render_suppressed_roots.contains(&306));
        let restore_sample = state.lifecycle_scene_sample_at(AnimationTime::from_nanos(0));
        let restore_frame = LifecycleFrameSnapshot::qualified_from_sample(
            &restore_sample,
            &LifecycleRenderEvidence::from_consumed([LifecycleRenderEvidenceEntry {
                window_id,
                root_surface_id: 306,
                transition_id: restore_id,
            }]),
        );
        state.publish_presented_lifecycle(2, &restore_frame);
        assert_eq!(state.window_lifecycle_animator.active_count(), 0);
        assert!(!state.lifecycle_render_suppressed_roots.contains(&306));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn stale_lifecycle_render_fallback_cannot_cancel_a_reversal() {
        let window_id = WindowId::from_raw(307).expect("valid window ID");
        let mut state = CompositorState {
            lifecycle_animation_renderer_available: Some(true),
            ..Default::default()
        };
        let request = |direction| {
            lifecycle_request(
                window_id,
                307,
                rect(0.0, 0.0, 80.0, 80.0),
                rect(200.0, 200.0, 20.0, 20.0),
                direction,
            )
        };
        let old_id = state
            .window_lifecycle_animator
            .start_or_reverse(
                request(LifecycleDirection::Minimize),
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("minimize starts");
        let new_id = state
            .window_lifecycle_animator
            .start_or_reverse(
                request(LifecycleDirection::Restore),
                AnimationTime::from_nanos(100_000_000),
                1.0,
            )
            .expect("restore reverses");
        assert_ne!(old_id, new_id);

        assert!(
            !state.apply_lifecycle_render_fallback(LifecycleRenderFallbackEntry {
                window_id,
                root_surface_id: 307,
                transition_id: old_id,
                reason: LifecycleRenderFallbackReason::LampProgramUnavailable,
            })
        );
        assert_eq!(
            state
                .window_lifecycle_animator
                .sample(window_id, AnimationTime::from_nanos(100_000_000))
                .expect("new transition survives")
                .transition_id,
            new_id
        );
    }

    #[test]
    fn fresh_restore_freezes_ssd_until_physical_settlement() {
        let (mut state, window_id) = ssd_test_state(401);
        state.lifecycle_animation_renderer_available = Some(true);
        let root_surface_id = 401;
        let group = visual_group(rect(384.0, 60.0, 832.0, 640.0));
        let decoration_a = state
            .native_decoration_render_instances_for_scale(&state.renderable_surfaces, 1.0)
            .into_iter()
            .next()
            .expect("authoritative SSD snapshot A");

        state.begin_lifecycle_restore(
            window_id,
            root_surface_id,
            Some(group),
            ResolvedEffectScene::default(),
            vec![decoration_a.clone()],
        );

        assert_eq!(
            lifecycle_decoration_signature(
                &state,
                root_surface_id,
                &state.renderable_surfaces,
                AnimationTime::monotonic_now().unwrap(),
            ),
            Some(decoration_a.scene_snapshot().visual_signature())
        );

        state.focused_window_id = Some(window_id);
        let decoration_b = state
            .native_decoration_render_instances_for_scale(&state.renderable_surfaces, 1.0)
            .into_iter()
            .next()
            .expect("live SSD snapshot B");
        assert_ne!(
            decoration_a.scene_snapshot().visual_signature(),
            decoration_b.scene_snapshot().visual_signature()
        );
        assert_eq!(
            lifecycle_decoration_signature(
                &state,
                root_surface_id,
                &state.renderable_surfaces,
                AnimationTime::monotonic_now().unwrap(),
            ),
            Some(decoration_a.scene_snapshot().visual_signature())
        );

        settle_lifecycle_transition(&mut state, window_id);
        assert!(!state.lifecycle_decorations.contains_key(&root_surface_id));
    }

    #[test]
    fn reversal_preserves_existing_frozen_ssd_snapshot() {
        let window_id = WindowId::from_raw(402).expect("window id");
        let root_surface_id = window_id.get() as u32;
        let mut state = CompositorState {
            lifecycle_animation_renderer_available: Some(true),
            ..Default::default()
        };
        let group_a = visual_group(rect(384.0, 60.0, 832.0, 640.0));
        let group_b = visual_group(rect(384.0, 20.0, 832.0, 680.0));
        let decoration_a = lifecycle_decoration(window_id, root_surface_id, 0x31);
        let decoration_b = lifecycle_decoration(window_id, root_surface_id, 0x32);

        state
            .window_lifecycle_animator
            .start_or_reverse(
                LifecycleTransitionRequest {
                    window_id,
                    root_surface_id,
                    visual_group: group_a,
                    direction: LifecycleDirection::Minimize,
                    resolved_effect_scene: ResolvedEffectScene::default(),
                },
                AnimationTime::monotonic_now().unwrap(),
                1.0,
            )
            .expect("minimize starts");
        state
            .lifecycle_decorations
            .insert(root_surface_id, decoration_a.clone());
        state.begin_lifecycle_restore(
            window_id,
            root_surface_id,
            Some(group_b),
            ResolvedEffectScene::default(),
            vec![decoration_b],
        );

        assert_eq!(
            lifecycle_decoration_signature(
                &state,
                root_surface_id,
                &[],
                AnimationTime::monotonic_now().unwrap(),
            ),
            Some(decoration_a.scene_snapshot().visual_signature())
        );
        assert_eq!(
            state
                .window_lifecycle_animator
                .visual_group(window_id)
                .expect("reversed transition")
                .canonical_visual_rect,
            group_a.canonical_visual_rect
        );
    }

    #[test]
    fn later_independent_restore_replaces_settled_ssd_snapshot() {
        let window_id = WindowId::from_raw(403).expect("window id");
        let root_surface_id = window_id.get() as u32;
        let mut state = CompositorState {
            lifecycle_animation_renderer_available: Some(true),
            ..Default::default()
        };
        let group = visual_group(rect(384.0, 60.0, 832.0, 640.0));
        let decoration_a = lifecycle_decoration(window_id, root_surface_id, 0x41);
        let decoration_b = lifecycle_decoration(window_id, root_surface_id, 0x42);

        state.begin_lifecycle_restore(
            window_id,
            root_surface_id,
            Some(group),
            ResolvedEffectScene::default(),
            vec![decoration_a],
        );
        settle_lifecycle_transition(&mut state, window_id);
        assert!(!state.lifecycle_decorations.contains_key(&root_surface_id));

        state.begin_lifecycle_restore(
            window_id,
            root_surface_id,
            Some(group),
            ResolvedEffectScene::default(),
            vec![decoration_b.clone()],
        );
        assert_eq!(
            lifecycle_decoration_signature(
                &state,
                root_surface_id,
                &[],
                AnimationTime::monotonic_now().unwrap(),
            ),
            Some(decoration_b.scene_snapshot().visual_signature())
        );
    }

    #[test]
    fn fresh_csd_restore_does_not_synthesize_frozen_ssd() {
        let window_id = WindowId::from_raw(404).expect("window id");
        let root_surface_id = window_id.get() as u32;
        let mut state = CompositorState {
            lifecycle_animation_renderer_available: Some(true),
            ..Default::default()
        };
        let stale_ssd = lifecycle_decoration(window_id, root_surface_id, 0x51);
        state
            .lifecycle_decorations
            .insert(root_surface_id, stale_ssd);

        state.begin_lifecycle_restore(
            window_id,
            root_surface_id,
            Some(visual_group(rect(400.0, 100.0, 800.0, 600.0))),
            ResolvedEffectScene::default(),
            Vec::new(),
        );

        assert!(!state.lifecycle_decorations.contains_key(&root_surface_id));
        let sample = state.lifecycle_scene_sample_at(AnimationTime::monotonic_now().unwrap());
        assert!(
            state
                .lifecycle_decoration_render_instances(&sample, &[])
                .is_empty()
        );
    }
}

impl CompositorState {
    pub(in crate::compositor) fn lifecycle_effect(
        &self,
        direction: LifecycleDirection,
    ) -> AnimationEffect {
        let slot = match direction {
            LifecycleDirection::Minimize => AnimationSlot::WindowMinimize,
            LifecycleDirection::Restore => AnimationSlot::WindowRestore,
        };
        self.animation_control
            .effective_effect(slot, self.animation_runtime_capabilities())
    }

    pub(in crate::compositor) fn animation_runtime_capabilities(
        &self,
    ) -> AnimationRuntimeCapabilities {
        AnimationRuntimeCapabilities {
            lamp_renderer: self.lifecycle_animation_renderer_available == Some(true),
        }
    }

    pub(in crate::compositor) fn lifecycle_minimize_source_rect(
        &self,
        root_surface_id: u32,
    ) -> Option<PresentationRect> {
        self.presented_window_geometry(root_surface_id)
            .map(PresentedWindowGeometry::presented_rect)
            .or_else(|| self.current_presentation_rect_for_root(root_surface_id))
    }

    pub(in crate::compositor) fn lifecycle_anchor_rect(
        &self,
        window_id: WindowId,
    ) -> Option<PresentationRect> {
        let anchor = self.astrea_toplevel_publisher.minimize_anchor(window_id)?;
        PresentationRect::new(
            f64::from(anchor.x),
            f64::from(anchor.y),
            f64::from(anchor.width),
            f64::from(anchor.height),
        )
    }

    pub(in crate::compositor) fn lifecycle_window_rect(
        &self,
        root_surface_id: u32,
    ) -> Option<PresentationRect> {
        let geometry = self
            .current_visual_root_window_geometry(root_surface_id)
            .or_else(|| self.current_root_window_geometry(root_surface_id))?;
        self.presentation_rect_for_geometry(root_surface_id, geometry)
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::compositor) fn begin_lifecycle_minimize(
        &mut self,
        window_id: WindowId,
        root_surface_id: u32,
        presented_source_client_rect: Option<PresentationRect>,
        canonical_client_rect: Option<PresentationRect>,
        visual_group: Option<LifecycleVisualGroup>,
        resolved_effect_scene: ResolvedEffectScene,
        lifecycle_decorations: Vec<DecorationRenderInstance>,
    ) {
        if self.lifecycle_effect(LifecycleDirection::Minimize) != AnimationEffect::MinimizeLamp {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            self.lifecycle_decorations.remove(&root_surface_id);
            return;
        }
        let Some(presented_source_client_rect) = presented_source_client_rect
            .or_else(|| self.lifecycle_minimize_source_rect(root_surface_id))
        else {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            self.lifecycle_decorations.remove(&root_surface_id);
            return;
        };
        let Some(canonical_client_rect) =
            canonical_client_rect.or_else(|| self.lifecycle_window_rect(root_surface_id))
        else {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            self.lifecycle_decorations.remove(&root_surface_id);
            return;
        };
        let Some(anchor_rect) = self.lifecycle_anchor_rect(window_id) else {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            self.lifecycle_decorations.remove(&root_surface_id);
            return;
        };
        let visual_group = visual_group.or_else(|| {
            LifecycleVisualGroup::from_bounds(
                canonical_client_rect,
                canonical_client_rect,
                presented_source_client_rect,
                anchor_rect,
                self.output_size.width,
                self.output_size.height,
            )
        });
        let Some(visual_group) = visual_group else {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            self.lifecycle_decorations.remove(&root_surface_id);
            return;
        };
        if !crate::window_lifecycle_animation::lamp_footprint_intersects_output(
            visual_group,
            self.output_size.width,
            self.output_size.height,
        ) {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            self.lifecycle_decorations.remove(&root_surface_id);
            return;
        }
        let Some(now) = AnimationTime::monotonic_now() else {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            self.lifecycle_decorations.remove(&root_surface_id);
            return;
        };
        let speed = self.animation_control.configuration().speed;
        let has_active_transition = self
            .window_lifecycle_animator
            .visual_group(window_id)
            .is_some();
        // Lamp takes over the root's presentation pixels, but the last
        // pageflip-confirmed geometry remains authoritative for source
        // continuity and direct-scanout safety.
        self.cancel_presentation_geometry_for_root(root_surface_id);
        self.lifecycle_render_suppressed_roots
            .remove(&root_surface_id);
        let started = self.window_lifecycle_animator.start_or_reverse(
            LifecycleTransitionRequest {
                window_id,
                root_surface_id,
                visual_group,
                direction: LifecycleDirection::Minimize,
                resolved_effect_scene,
            },
            now,
            speed,
        );
        if started.is_some() {
            if !has_active_transition {
                self.replace_lifecycle_decoration_snapshot(root_surface_id, lifecycle_decorations);
            }
        } else {
            self.lifecycle_decorations.remove(&root_surface_id);
        }
    }

    fn replace_lifecycle_decoration_snapshot(
        &mut self,
        root_surface_id: u32,
        decorations: Vec<DecorationRenderInstance>,
    ) {
        self.lifecycle_decorations.remove(&root_surface_id);
        if let Some(decoration) = decorations
            .into_iter()
            .find(|decoration| decoration.root_surface_id() == root_surface_id)
        {
            self.lifecycle_decorations
                .insert(root_surface_id, decoration);
        }
    }

    pub(in crate::compositor) fn begin_lifecycle_restore(
        &mut self,
        window_id: WindowId,
        root_surface_id: u32,
        visual_group: Option<LifecycleVisualGroup>,
        resolved_effect_scene: ResolvedEffectScene,
        lifecycle_decorations: Vec<DecorationRenderInstance>,
    ) {
        if self.lifecycle_effect(LifecycleDirection::Restore) != AnimationEffect::MinimizeLamp {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            self.lifecycle_decorations.remove(&root_surface_id);
            return;
        }
        let visual_group = self
            .window_lifecycle_animator
            .visual_group(window_id)
            .or(visual_group)
            .or_else(|| {
                let full_window_rect = self.lifecycle_window_rect(root_surface_id)?;
                let anchor_rect = self.lifecycle_anchor_rect(window_id)?;
                LifecycleVisualGroup::from_bounds(
                    full_window_rect,
                    full_window_rect,
                    full_window_rect,
                    anchor_rect,
                    self.output_size.width,
                    self.output_size.height,
                )
            });
        let Some(visual_group) = visual_group else {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            self.lifecycle_decorations.remove(&root_surface_id);
            return;
        };
        if !crate::window_lifecycle_animation::lamp_footprint_intersects_output(
            visual_group,
            self.output_size.width,
            self.output_size.height,
        ) {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            self.lifecycle_decorations.remove(&root_surface_id);
            return;
        }
        let Some(now) = AnimationTime::monotonic_now() else {
            self.window_lifecycle_animator.cancel(window_id);
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            self.lifecycle_decorations.remove(&root_surface_id);
            return;
        };
        let speed = self.animation_control.configuration().speed;
        let has_active_transition = self
            .window_lifecycle_animator
            .visual_group(window_id)
            .is_some();
        if self
            .window_lifecycle_animator
            .start_or_reverse(
                LifecycleTransitionRequest {
                    window_id,
                    root_surface_id,
                    visual_group,
                    direction: LifecycleDirection::Restore,
                    resolved_effect_scene,
                },
                now,
                speed,
            )
            .is_some()
        {
            if !has_active_transition {
                self.replace_lifecycle_decoration_snapshot(root_surface_id, lifecycle_decorations);
            }
            self.lifecycle_render_suppressed_roots
                .insert(root_surface_id);
        } else {
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
            self.lifecycle_decorations.remove(&root_surface_id);
        }
    }

    pub(in crate::compositor) fn lifecycle_scene_sample_at(
        &self,
        at: AnimationTime,
    ) -> LifecycleSceneSample {
        self.window_lifecycle_animator.sample_scene(at)
    }

    pub(in crate::compositor) fn lifecycle_renderable_surfaces(
        &self,
        sample: &LifecycleSceneSample,
    ) -> Vec<RenderableSurface> {
        let roots = sample
            .lamps
            .iter()
            .map(|lamp| lamp.root_surface_id)
            .collect::<HashSet<_>>();
        if roots.is_empty() {
            return Vec::new();
        }
        let mut surfaces = self
            .renderable_surfaces
            .iter()
            .filter(|surface| roots.contains(&self.root_surface_id_for_surface(surface.surface_id)))
            .cloned()
            .collect::<Vec<_>>();
        for lamp in &sample.lamps {
            if let Some(window) = self.window(lamp.window_id)
                && window.state.is_minimized()
            {
                surfaces.extend(window.state.minimized_surfaces().iter().cloned());
            }
        }
        let mut seen = HashSet::new();
        surfaces.retain(|surface| seen.insert(surface.surface_id));
        surfaces
    }

    pub(in crate::compositor) fn lifecycle_decoration_render_instances(
        &self,
        sample: &LifecycleSceneSample,
        surfaces: &[RenderableSurface],
    ) -> Vec<DecorationRenderInstance> {
        let roots = sample
            .lamps
            .iter()
            .map(|lamp| lamp.root_surface_id)
            .collect::<HashSet<_>>();
        let mut frozen = Vec::new();
        let mut frozen_roots = HashSet::new();
        for root_surface_id in roots.iter().copied() {
            if let Some(decoration) = self.lifecycle_decorations.get(&root_surface_id) {
                frozen.push(decoration.clone());
                frozen_roots.insert(root_surface_id);
            }
        }
        frozen.extend(
            self.native_decoration_render_instances_for_scale(surfaces, 1.0)
                .into_iter()
                .filter(|decoration| {
                    roots.contains(&decoration.root_surface_id())
                        && !frozen_roots.contains(&decoration.root_surface_id())
                }),
        );
        frozen
    }

    pub(in crate::compositor) fn lifecycle_surface_is_suppressed(&self, surface_id: u32) -> bool {
        self.lifecycle_render_suppressed_roots
            .contains(&self.root_surface_id_for_surface(surface_id))
    }

    pub(in crate::compositor) fn lifecycle_frame_snapshot_at(
        &self,
        at: AnimationTime,
    ) -> LifecycleFrameSnapshot {
        LifecycleFrameSnapshot::from_sample(&self.lifecycle_scene_sample_at(at))
    }

    fn lifecycle_lamp_intersects_output(
        &self,
        lamp: &crate::window_lifecycle_animation::LampWindowSample,
    ) -> bool {
        crate::window_lifecycle_animation::lamp_footprint_intersects_output(
            lamp.visual_group,
            self.output_size.width,
            self.output_size.height,
        )
    }

    fn lifecycle_lamp_has_visible_pixels(
        &self,
        lamp: &crate::window_lifecycle_animation::LampWindowSample,
    ) -> bool {
        lamp.opacity > f64::EPSILON && self.lifecycle_lamp_intersects_output(lamp)
    }

    pub(in crate::compositor) fn settle_lifecycle_no_visual_change(&mut self) -> bool {
        let now = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        let candidates = self
            .window_lifecycle_animator
            .sample_scene(now)
            .lamps
            .into_iter()
            .filter(|lamp| !self.lifecycle_lamp_has_visible_pixels(lamp))
            .filter(|lamp| {
                !self.presented_lifecycle.lamps.iter().any(|presented| {
                    presented.window_id == lamp.window_id
                        && !presented.mathematically_settled
                        && presented.opacity > f64::EPSILON
                        && crate::window_lifecycle_animation::lamp_footprint_intersects_output(
                            presented.visual_group,
                            self.output_size.width,
                            self.output_size.height,
                        )
                })
            })
            .map(|lamp| (lamp.window_id, lamp.root_surface_id, lamp.transition_id))
            .collect::<Vec<_>>();
        let mut settled = false;
        for (window_id, root_surface_id, transition_id) in candidates {
            if self
                .window_lifecycle_animator
                .settle_no_visual_change(window_id, transition_id)
            {
                self.lifecycle_render_suppressed_roots
                    .remove(&root_surface_id);
                self.lifecycle_decorations.remove(&root_surface_id);
                settled = true;
            }
        }
        settled
    }

    pub(in crate::compositor) fn lifecycle_animation_has_pending_visible(&self) -> bool {
        let active_intersects = self
            .window_lifecycle_animator
            .sample_scene(AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0)))
            .lamps
            .iter()
            .any(|lamp| self.lifecycle_lamp_has_visible_pixels(lamp));
        let presented_intersects = self.presented_lifecycle.lamps.iter().any(|lamp| {
            !lamp.mathematically_settled
                && lamp.opacity > f64::EPSILON
                && crate::window_lifecycle_animation::lamp_footprint_intersects_output(
                    lamp.visual_group,
                    self.output_size.width,
                    self.output_size.height,
                )
        });
        active_intersects || presented_intersects
    }

    pub(in crate::compositor) fn lifecycle_render_suppressed_roots(&self) -> &HashSet<u32> {
        &self.lifecycle_render_suppressed_roots
    }

    pub(in crate::compositor) fn publish_presented_lifecycle(
        &mut self,
        frame_id: u64,
        snapshot: &LifecycleFrameSnapshot,
    ) {
        self.publish_presented_lifecycle_with_replacements(frame_id, snapshot, &[], false);
    }

    pub(in crate::compositor) fn publish_presented_lifecycle_with_replacements(
        &mut self,
        frame_id: u64,
        snapshot: &LifecycleFrameSnapshot,
        canonical_root_surface_ids: &[u32],
        rendered_scene_replacement: bool,
    ) {
        self.presented_lifecycle_frame_id = frame_id;
        let mut qualified = snapshot.clone();
        for old in &self.presented_lifecycle.lamps {
            let replaced = snapshot
                .lamps
                .iter()
                .any(|lamp| lamp.root_surface_id == old.root_surface_id);
            let canonical_replaced = canonical_root_surface_ids.contains(&old.root_surface_id);
            let rendered_replaced = rendered_scene_replacement && !replaced;
            let still_visible = !old.mathematically_settled
                && old.opacity > f64::EPSILON
                && crate::window_lifecycle_animation::lamp_footprint_intersects_output(
                    old.visual_group,
                    self.output_size.width,
                    self.output_size.height,
                );
            if !replaced && !canonical_replaced && !rendered_replaced && still_visible {
                qualified.lamps.push(*old);
            }
        }
        qualified.refresh_signature();
        self.presented_lifecycle = qualified;
        for lamp in &snapshot.lamps {
            let acknowledged = self.window_lifecycle_animator.acknowledge(
                lamp.window_id,
                lamp.transition_id,
                lamp.mathematically_settled,
            );
            if acknowledged && matches!(lamp.direction, LifecycleDirection::Restore) {
                self.lifecycle_render_suppressed_roots
                    .remove(&lamp.root_surface_id);
            }
            if acknowledged {
                self.lifecycle_decorations.remove(&lamp.root_surface_id);
            }
        }
        self.advance_pointer_hit_generation();
    }

    pub(in crate::compositor) fn set_lifecycle_animation_enabled(&mut self, enabled: bool) {
        let now = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        self.window_lifecycle_animator.set_enabled(enabled, now);
    }

    pub(in crate::compositor) fn reconcile_lifecycle_animation_policy(&mut self) {
        let now = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        let active = self.window_lifecycle_animator.sample_scene(now).lamps;
        for lamp in active {
            if self.lifecycle_effect(lamp.direction) != AnimationEffect::MinimizeLamp {
                self.window_lifecycle_animator.snap_to_endpoint(
                    lamp.window_id,
                    lamp.transition_id,
                    now,
                );
            }
        }
    }

    pub(in crate::compositor) fn apply_lifecycle_render_fallback(
        &mut self,
        fallback: LifecycleRenderFallbackEntry,
    ) -> bool {
        let now = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        let Some(current) = self
            .window_lifecycle_animator
            .sample(fallback.window_id, now)
        else {
            return false;
        };
        if current.root_surface_id != fallback.root_surface_id
            || current.transition_id != fallback.transition_id
        {
            return false;
        }
        if !self
            .window_lifecycle_animator
            .retire_render_fallback(fallback.window_id, fallback.transition_id)
        {
            return false;
        }
        self.lifecycle_render_suppressed_roots
            .remove(&fallback.root_surface_id);
        self.lifecycle_decorations.remove(&fallback.root_surface_id);
        true
    }

    pub(in crate::compositor) fn set_lifecycle_animation_renderer_available(
        &mut self,
        available: bool,
    ) {
        self.lifecycle_animation_renderer_available = Some(available);
        if !available {
            self.window_lifecycle_animator.cancel_all();
            self.lifecycle_render_suppressed_roots.clear();
            self.lifecycle_decorations.clear();
        }
    }

    pub(in crate::compositor) fn lifecycle_cancel_window(&mut self, window_id: WindowId) {
        let root_surface_id = self.window(window_id).map(|window| window.root_surface_id);
        if let Some(root_surface_id) = root_surface_id {
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
        }
        self.window_lifecycle_animator.cancel(window_id);
        if let Some(root_surface_id) = root_surface_id {
            self.lifecycle_decorations.remove(&root_surface_id);
        }
    }

    pub(in crate::compositor) fn lifecycle_teardown_window(&mut self, window_id: WindowId) {
        let root_surface_id = self.window(window_id).map(|window| window.root_surface_id);
        self.lifecycle_cancel_window(window_id);
        self.presented_lifecycle
            .lamps
            .retain(|lamp| lamp.window_id != window_id);
        self.presented_lifecycle.refresh_signature();
        if let Some(root_surface_id) = root_surface_id {
            self.lifecycle_render_suppressed_roots
                .remove(&root_surface_id);
        }
    }

    pub(in crate::compositor) const fn presented_lifecycle_frame_id(&self) -> u64 {
        self.presented_lifecycle_frame_id
    }
}
