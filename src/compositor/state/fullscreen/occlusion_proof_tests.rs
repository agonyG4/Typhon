use super::*;
use crate::effects::{
    EFFECT_MANIFEST_VERSION, EffectAlphaMode, EffectDefinition, EffectFailurePolicy,
    EffectFrameDemand, EffectManifest, EffectNode, EffectNodeId, EffectOutsets, EffectProgram,
    EffectProgramId, EffectRect, EffectRegion, EffectSource, EffectWorkingSpace, MaskMode,
    MaskSpec,
};
use crate::presentation_animation::{
    AnimationTime, PresentationClip, PresentationClipRect, PresentationFrameSnapshot,
    PresentationGroupClip, PresentationGroupOpacity, PresentationGroupTransform,
    PresentationOpacity, PresentationRect, PresentationRevisionId, PresentationSampleTimeSource,
    PresentationSceneSample, PresentationTransactionId,
};
use crate::render_backend::buffer::{BufferSize, DrmFormat};
use std::collections::BTreeMap;

const UNDERLAY_ROOT: u32 = 941;
const OWNER_ROOT: u32 = 942;
const OWNER_CHILD: u32 = 943;

fn fullscreen_owner_fixture() -> (CompositorState, WindowId, SceneNodeId, PresentationRect) {
    let mut state = CompositorState::default();
    let output_id = state
        .ensure_native_output_id()
        .expect("test output identity");
    let output_size = BufferSize::new(state.output_size.width, state.output_size.height)
        .expect("test output size");
    let underlay_window = WindowId::from_raw(41).expect("underlay window id");
    let owner_window = WindowId::from_raw(42).expect("owner window id");
    let underlay = super::super::desktop_window_tests::x11_scanout_surface(
        UNDERLAY_ROOT,
        output_size.width,
        output_size.height,
        SurfacePlacement::root_at(0, 0),
        DrmFormat::Xrgb8888,
    );
    let owner = super::super::desktop_window_tests::x11_scanout_surface(
        OWNER_ROOT,
        output_size.width,
        output_size.height,
        SurfacePlacement::absolute_root_at(0, 0),
        DrmFormat::Xrgb8888,
    );
    state.install_native_frame_test_scene(
        vec![underlay, owner],
        &[(UNDERLAY_ROOT, underlay_window), (OWNER_ROOT, owner_window)],
        Some(OWNER_ROOT),
    );

    let presentation = PresentationSceneSample::empty_for_output(
        output_id,
        AnimationTime::from_nanos(0),
        PresentationSampleTimeSource::ZeroFallback,
    );
    let targets = state.native_frame_presentation_targets(state.active_scene_surfaces());
    let presented_windows = state.presented_window_geometries_for_targets(&presentation, &targets);
    let snapshot = PresentationFrameSnapshot::from_sample_with_presented_windows(
        &presentation,
        presented_windows,
    );
    state.publish_presented_presentation(1, &snapshot);

    let owner_scene_node = state
        .presentation_scene_node_id_for_root(OWNER_ROOT)
        .expect("fullscreen owner scene node");
    let output_rect = PresentationRect::new(
        0.0,
        0.0,
        f64::from(output_size.width),
        f64::from(output_size.height),
    )
    .expect("test output rect");
    (state, owner_window, owner_scene_node, output_rect)
}

fn register_mask_program(state: &CompositorState, alpha_mode: EffectAlphaMode) -> EffectProgramId {
    let program_id = EffectProgramId::new(77).expect("effect program ID");
    let content_node = EffectNodeId::new(1).expect("content node ID");
    let masked_node = EffectNodeId::new(2).expect("mask node ID");
    let program = EffectProgram {
        id: program_id,
        nodes: vec![
            EffectNode::source(content_node, EffectSource::TargetContent),
            EffectNode::mask(
                masked_node,
                content_node,
                MaskSpec {
                    mode: MaskMode::Alpha,
                },
            ),
        ],
        output: masked_node,
        working_space: EffectWorkingSpace::LinearSrgb,
        alpha_mode,
        outsets: EffectOutsets::ZERO,
        frame_demand: EffectFrameDemand::OnDamage,
        failure_policy: EffectFailurePolicy::Passthrough,
    };
    let effect_name = match alpha_mode {
        EffectAlphaMode::Opaque => "test.fullscreen-opaque-mask",
        EffectAlphaMode::Preserve => "test.fullscreen-preserve-mask",
    }
    .to_owned();
    let manifest = EffectManifest {
        version: EFFECT_MANIFEST_VERSION,
        effects: BTreeMap::from([(
            effect_name.clone(),
            EffectDefinition {
                name: effect_name,
                program,
                parameters: BTreeMap::new(),
                shader_assets: Vec::new(),
            },
        )]),
    };
    state
        .trusted_effect_registry()
        .reload(manifest, |_| Ok(()))
        .expect("preserve-alpha effect registry generation");
    program_id
}

fn register_preserving_mask_program(state: &CompositorState) -> EffectProgramId {
    register_mask_program(state, EffectAlphaMode::Preserve)
}

fn owner_proves_full_occlusion(state: &CompositorState) -> bool {
    let eligibility = state.fullscreen_presentation_eligibility();
    state.fullscreen_owner_proves_full_occlusion(OWNER_ROOT, eligibility)
}

#[test]
fn fullscreen_occlusion_effect_proof_without_replacements_skips_scene_resolution() {
    let (state, _, _, _) = fullscreen_owner_fixture();
    let calls_before = crate::compositor::effects::resolved_effect_scene_call_count_for_test();

    assert!(state.fullscreen_owner_effects_preserve_full_occlusion(OWNER_ROOT));
    assert_eq!(
        crate::compositor::effects::resolved_effect_scene_call_count_for_test(),
        calls_before,
        "a no-replacement proof must not resolve the effect scene"
    );
}

#[test]
fn fullscreen_occlusion_effect_proof_ignores_before_surface_effect_without_scene_resolution() {
    let (mut state, _, _, _) = fullscreen_owner_fixture();
    let program = register_preserving_mask_program(&state);
    assert!(
        state.set_internal_surface_effect(
            UNDERLAY_ROOT,
            EffectAnchor::BeforeSurface(UNDERLAY_ROOT),
            program,
            EffectRegion::from_rect(
                EffectRect::new(0, 0, state.output_size.width, state.output_size.height)
                    .expect("background effect region"),
            ),
        )
    );
    let calls_before = crate::compositor::effects::resolved_effect_scene_call_count_for_test();

    assert!(state.fullscreen_owner_effects_preserve_full_occlusion(OWNER_ROOT));
    assert_eq!(
        crate::compositor::effects::resolved_effect_scene_call_count_for_test(),
        calls_before,
        "an unrelated BeforeSurface effect must not resolve the effect scene"
    );
}

#[test]
fn fullscreen_occlusion_proof_accepts_settled_xrgb_dmabuf_without_replacement_effect() {
    let (state, _, _, _) = fullscreen_owner_fixture();
    let eligibility = state.fullscreen_presentation_eligibility();

    assert!(
        eligibility.eligible,
        "fixture should prove XRGB/DMABUF opacity"
    );
    assert!(state.fullscreen_owner_proves_full_occlusion(OWNER_ROOT, eligibility));

    let plan = state.fullscreen_composition_plan();
    assert!(plan.owner_occludes_underlays);
    assert!(!plan.allows_composition_root(UNDERLAY_ROOT));
    assert!(!plan.allows_interaction_root(UNDERLAY_ROOT));
}

#[test]
fn fullscreen_occlusion_proof_rejects_non_opaque_canonical_window_opacity() {
    let (mut state, owner_window, _, _) = fullscreen_owner_fixture();
    state
        .set_window_canonical_opacity(
            owner_window,
            PresentationOpacity::new(0.5).expect("valid opacity"),
            None,
        )
        .expect("canonical opacity update");

    assert!(!owner_proves_full_occlusion(&state));
}

#[test]
fn fullscreen_occlusion_proof_rejects_bounded_canonical_clip() {
    let (mut state, owner_window, _, _) = fullscreen_owner_fixture();
    let clip = PresentationClipRect::new(0.0, 0.0, 640.0, 480.0).expect("bounded clip");
    state
        .set_window_canonical_clip(owner_window, PresentationClip::Rect(clip), None)
        .expect("canonical clip update");

    assert!(!owner_proves_full_occlusion(&state));
}

#[test]
fn fullscreen_occlusion_proof_rejects_non_identity_presented_opacity() {
    let (mut state, _, scene_node, _) = fullscreen_owner_fixture();
    let mut snapshot = state
        .presented_presentation
        .clone()
        .expect("published presentation snapshot");
    snapshot
        .opacities
        .push(PresentationGroupOpacity::with_scene_node(
            scene_node,
            OWNER_ROOT,
            PresentationOpacity::new(0.5).expect("valid opacity"),
            None,
        ));
    state.publish_presented_presentation(2, &snapshot);

    assert!(!owner_proves_full_occlusion(&state));
}

#[test]
fn fullscreen_occlusion_proof_rejects_bounded_presented_clip() {
    let (mut state, _, scene_node, _) = fullscreen_owner_fixture();
    let mut snapshot = state
        .presented_presentation
        .clone()
        .expect("published presentation snapshot");
    let clip = PresentationClipRect::new(0.0, 0.0, 640.0, 480.0).expect("bounded clip");
    snapshot.clips.push(PresentationGroupClip::with_scene_node(
        scene_node,
        OWNER_ROOT,
        PresentationClip::Rect(clip),
        Some(clip),
        None,
    ));
    state.publish_presented_presentation(2, &snapshot);

    assert!(!owner_proves_full_occlusion(&state));
}

#[test]
fn fullscreen_occlusion_proof_rejects_non_identity_presented_geometry() {
    let (mut state, _, scene_node, canonical_rect) = fullscreen_owner_fixture();
    let mut snapshot = state
        .presented_presentation
        .clone()
        .expect("published presentation snapshot");
    let presented_rect = PresentationRect::new(
        canonical_rect.x() + 1.0,
        canonical_rect.y(),
        canonical_rect.width(),
        canonical_rect.height(),
    )
    .expect("translated presentation rect");
    snapshot
        .transforms
        .push(PresentationGroupTransform::with_scene_node(
            scene_node,
            OWNER_ROOT,
            PresentationTransactionId::from_raw(1).expect("transaction ID"),
            PresentationRevisionId::from_raw(1).expect("revision ID"),
            canonical_rect,
            presented_rect,
            false,
        ));
    state.publish_presented_presentation(2, &snapshot);

    assert!(!owner_proves_full_occlusion(&state));
}

#[test]
fn fullscreen_occlusion_proof_rejects_missing_published_presentation_snapshot() {
    let (mut state, _, _, _) = fullscreen_owner_fixture();
    state.presented_presentation = None;

    assert!(!owner_proves_full_occlusion(&state));
}

#[test]
fn fullscreen_occlusion_proof_rejects_missing_replacement_program_metadata() {
    let (mut state, _, _, _) = fullscreen_owner_fixture();
    assert!(
        state.set_internal_surface_effect(
            OWNER_ROOT,
            EffectAnchor::ReplaceSurface(OWNER_ROOT),
            EffectProgramId::new(98).expect("unknown program ID"),
            EffectRegion::from_rect(
                EffectRect::new(0, 0, state.output_size.width, state.output_size.height)
                    .expect("replacement region"),
            ),
        )
    );

    assert!(!owner_proves_full_occlusion(&state));
}

#[test]
fn fullscreen_occlusion_proof_ignores_replacement_on_inactive_surface() {
    let (mut state, _, _, _) = fullscreen_owner_fixture();
    let inactive_surface = OWNER_ROOT + 100;
    assert!(
        state.set_internal_surface_effect(
            inactive_surface,
            EffectAnchor::ReplaceSurface(inactive_surface),
            EffectProgramId::new(98).expect("unknown program ID"),
            EffectRegion::from_rect(
                EffectRect::new(0, 0, state.output_size.width, state.output_size.height)
                    .expect("replacement region"),
            ),
        )
    );

    assert!(owner_proves_full_occlusion(&state));
}

#[test]
fn fullscreen_occlusion_proof_ignores_replacement_region_outside_output() {
    let (mut state, _, _, _) = fullscreen_owner_fixture();
    let output_right = i32::try_from(state.output_size.width).expect("output width") + 1;
    assert!(state.set_internal_surface_effect(
        OWNER_ROOT,
        EffectAnchor::ReplaceSurface(OWNER_ROOT),
        EffectProgramId::new(98).expect("unknown program ID"),
        EffectRegion::from_rect(
            EffectRect::new(output_right, 0, 10, 10).expect("off-output region"),
        ),
    ));

    assert!(owner_proves_full_occlusion(&state));
}

#[test]
fn fullscreen_occlusion_proof_includes_active_protocol_replacement_bindings() {
    let (mut state, _, _, _) = fullscreen_owner_fixture();
    let program = register_preserving_mask_program(&state);
    assert!(
        state.set_internal_surface_effect(
            OWNER_ROOT,
            EffectAnchor::ReplaceSurface(OWNER_ROOT),
            program,
            EffectRegion::from_rect(
                EffectRect::new(0, 0, state.output_size.width, state.output_size.height)
                    .expect("replacement region"),
            ),
        )
    );
    let instance = state
        .internal_surface_effects
        .remove(&OWNER_ROOT)
        .expect("created replacement instance");
    state.protocol_surface_effects.insert(
        crate::compositor::effects::SurfaceEffectBindingKey {
            surface_id: OWNER_ROOT,
            slot: crate::compositor::effects::SurfaceEffectSlot::Content,
        },
        crate::compositor::effects::ProtocolSurfaceEffectBinding {
            owner_id: 1,
            instance: Some(instance),
            program_name: None,
            program_generation: None,
            schema_signature: None,
        },
    );

    assert!(!owner_proves_full_occlusion(&state));
}

#[test]
fn fullscreen_occlusion_proof_accepts_trusted_opaque_replacement_effect() {
    let (mut state, _, _, _) = fullscreen_owner_fixture();
    let program = register_mask_program(&state, EffectAlphaMode::Opaque);
    let registry = state.trusted_effect_registry().current();
    let registered = registry
        .effect_for_program(program)
        .expect("trusted opaque effect");
    assert_eq!(
        registered.program.program.alpha_mode,
        EffectAlphaMode::Opaque
    );
    assert!(
        state.set_internal_surface_effect(
            OWNER_ROOT,
            EffectAnchor::ReplaceSurface(OWNER_ROOT),
            program,
            EffectRegion::from_rect(
                EffectRect::new(0, 0, state.output_size.width, state.output_size.height)
                    .expect("replacement region"),
            ),
        )
    );

    assert!(owner_proves_full_occlusion(&state));
}

#[test]
fn fullscreen_occlusion_proof_rejects_visual_group_replacement_in_owner_group() {
    let (mut state, owner_window, _, _) = fullscreen_owner_fixture();
    let output_size = state.output_size;
    let child = super::super::desktop_window_tests::x11_scanout_surface(
        OWNER_CHILD,
        output_size.width,
        output_size.height,
        SurfacePlacement {
            parent_surface_id: Some(OWNER_ROOT),
            local_x: 0,
            local_y: 0,
            root_mode: RootPlacementMode::CascadedWindow,
        },
        DrmFormat::Xrgb8888,
    );
    let mut surfaces = state.active_scene_surfaces().to_vec();
    surfaces.push(child);
    state.install_native_frame_test_scene(
        surfaces,
        &[
            (
                UNDERLAY_ROOT,
                WindowId::from_raw(41).expect("underlay window id"),
            ),
            (OWNER_ROOT, owner_window),
        ],
        Some(OWNER_ROOT),
    );

    assert!(
        state
            .active_scene_surfaces()
            .iter()
            .any(|surface| surface.surface_id == OWNER_CHILD)
    );
    assert_eq!(
        state.root_surface_id_for_surface(OWNER_CHILD),
        OWNER_ROOT,
        "the replacement target belongs to the fullscreen owner root"
    );
    assert_eq!(
        state.visual_group_for_surface(OWNER_CHILD),
        state.visual_group_for_surface(OWNER_ROOT)
    );

    let program = register_preserving_mask_program(&state);
    assert!(
        state.set_internal_surface_effect(
            OWNER_CHILD,
            EffectAnchor::ReplaceSurface(OWNER_CHILD),
            program,
            EffectRegion::from_rect(
                EffectRect::new(0, 0, output_size.width, output_size.height)
                    .expect("replacement region"),
            ),
        )
    );

    assert!(!state.fullscreen_owner_effects_preserve_full_occlusion(OWNER_ROOT));
}

#[test]
fn fullscreen_occlusion_proof_rejects_partial_output_opaque_replacement() {
    let (mut state, _, _, _) = fullscreen_owner_fixture();
    let program = register_mask_program(&state, EffectAlphaMode::Opaque);
    assert!(
        state.set_internal_surface_effect(
            OWNER_ROOT,
            EffectAnchor::ReplaceSurface(OWNER_ROOT),
            program,
            EffectRegion::from_rect(
                EffectRect::new(0, 0, state.output_size.width / 2, state.output_size.height,)
                    .expect("partial replacement region"),
            ),
        )
    );

    assert!(
        !owner_proves_full_occlusion(&state),
        "a replacement that skips composition must cover the fullscreen output"
    );
}

#[test]
fn fullscreen_occlusion_proof_ignores_replacement_effect_in_other_visual_group() {
    let (mut state, _, _, _) = fullscreen_owner_fixture();
    let program = register_preserving_mask_program(&state);
    assert!(
        state.set_internal_surface_effect(
            UNDERLAY_ROOT,
            EffectAnchor::ReplaceSurface(UNDERLAY_ROOT),
            program,
            EffectRegion::from_rect(
                EffectRect::new(0, 0, state.output_size.width, state.output_size.height)
                    .expect("underlay replacement region"),
            ),
        )
    );

    assert!(owner_proves_full_occlusion(&state));
    let plan = state.fullscreen_composition_plan();
    assert!(plan.owner_occludes_underlays);
    assert!(!plan.allows_composition_root(UNDERLAY_ROOT));
}

#[test]
fn fullscreen_occlusion_proof_rejects_ambiguous_visual_group_identity() {
    let (mut state, _, _, _) = fullscreen_owner_fixture();
    let program = register_preserving_mask_program(&state);
    assert!(
        state.set_internal_surface_effect(
            UNDERLAY_ROOT,
            EffectAnchor::ReplaceSurface(UNDERLAY_ROOT),
            program,
            EffectRegion::from_rect(
                EffectRect::new(0, 0, state.output_size.width, state.output_size.height)
                    .expect("underlay replacement region"),
            ),
        )
    );
    state
        .internal_surface_effects
        .get_mut(&UNDERLAY_ROOT)
        .expect("underlay replacement")
        .visual_group = Some(
        crate::compositor::render::VisualGroupId::new(u32::MAX).expect("nonzero group identity"),
    );

    assert!(
        !owner_proves_full_occlusion(&state),
        "a replacement with stale group identity cannot prove occlusion"
    );
}

#[test]
fn fullscreen_occlusion_proof_ignores_before_after_and_output_postprocess_effects() {
    for anchor in [
        EffectAnchor::BeforeSurface(OWNER_ROOT),
        EffectAnchor::AfterSurface(OWNER_ROOT),
        EffectAnchor::OutputPostProcess,
    ] {
        let (mut state, _, _, _) = fullscreen_owner_fixture();
        let program = register_preserving_mask_program(&state);
        assert!(
            state.set_internal_surface_effect(
                OWNER_ROOT,
                anchor,
                program,
                EffectRegion::from_rect(
                    EffectRect::new(0, 0, state.output_size.width, state.output_size.height)
                        .expect("effect region"),
                ),
            )
        );

        assert!(owner_proves_full_occlusion(&state));
    }
}

#[test]
fn fullscreen_occlusion_proof_rejects_preserve_alpha_replacement_effect() {
    let (mut state, _, _, _) = fullscreen_owner_fixture();
    let program = register_preserving_mask_program(&state);
    let registry = state.trusted_effect_registry().current();
    let registered = registry
        .effect_for_program(program)
        .expect("trusted preserve-alpha effect");
    assert_eq!(
        registered.program.program.alpha_mode,
        EffectAlphaMode::Preserve
    );
    assert!(
        state.set_internal_surface_effect(
            OWNER_ROOT,
            EffectAnchor::ReplaceSurface(OWNER_ROOT),
            program,
            EffectRegion::from_rect(
                EffectRect::new(0, 0, state.output_size.width, state.output_size.height)
                    .expect("replacement region"),
            ),
        )
    );

    let eligibility = state.fullscreen_presentation_eligibility();
    assert!(eligibility.fully_opaque);
    assert!(eligibility.exactly_covers_output);
    assert!(
        !state.fullscreen_owner_proves_full_occlusion(OWNER_ROOT, eligibility),
        "an alpha-preserving replacement can introduce transparent output"
    );

    let plan = state.fullscreen_composition_plan();
    assert!(!plan.owner_occludes_underlays);
    assert!(plan.allows_composition_root(UNDERLAY_ROOT));
    assert!(!plan.allows_interaction_root(UNDERLAY_ROOT));
    let scanout = state.direct_scanout_scene_analysis();
    assert!(scanout.candidate.is_none());
    assert!(scanout.blockers.reasons().contains(
        &crate::compositor::direct_scanout::DirectScanoutSceneRejection::FullscreenUnderlayVisible
    ));
}
