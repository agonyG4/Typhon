use super::desktop_window_tests::{
    install_x11_scanout_surface, x11_output_snapshot, x11_scanout_surface,
};
use super::*;
use crate::effects::{EffectRect, EffectRegion};
use crate::presentation_animation::{
    AnimationCurve, AnimationTime, EasingCurve, PresentationClip, PresentationClipMutation,
    PresentationClipRect, PresentationGeometryMutation, PresentationGroupClip,
    PresentationGroupOpacity, PresentationGroupTransform, PresentationOpacity,
    PresentationOpacityMutation, PresentationRect, PresentationRevisionId,
    PresentationSampleTimeSource, PresentationTransactionId, PresentationTransactionRequest,
};
use crate::render_backend::buffer::DrmFormat;
use crate::xwayland::XwaylandGeneration;
use std::num::NonZeroU64;
use std::time::Duration;

fn install_probe_scanout_surface(
    state: &mut CompositorState,
    root_surface_id: u32,
    source_size: BufferSize,
) {
    let output_size = BufferSize::new(state.output_size.width, state.output_size.height)
        .expect("configured output size");
    install_probe_scanout_surface_with_metadata(
        state,
        root_surface_id,
        source_size,
        1,
        wayland_server::protocol::wl_output::Transform::Normal,
        None,
        Some(output_size),
    );
}

fn install_probe_scanout_surface_with_metadata(
    state: &mut CompositorState,
    root_surface_id: u32,
    source_size: BufferSize,
    buffer_scale: u32,
    buffer_transform: wayland_server::protocol::wl_output::Transform,
    viewport_source: Option<ViewportSourceRect>,
    viewport_destination: Option<BufferSize>,
) {
    let generation = XwaylandGeneration::new(
        NonZeroU64::new(u64::from(root_surface_id)).expect("nonzero test surface id"),
    );
    let mut surface = x11_scanout_surface(
        root_surface_id,
        source_size.width,
        source_size.height,
        SurfacePlacement::absolute_root_at(0, 0),
        DrmFormat::Xrgb8888,
    );
    surface.width = state.output_size.width;
    surface.height = state.output_size.height;
    surface.buffer_scale = buffer_scale;
    surface.buffer_transform = buffer_transform;
    surface.viewport_source = viewport_source;
    surface.viewport_destination = viewport_destination;
    install_x11_scanout_surface(
        state,
        surface,
        x11_output_snapshot(generation, root_surface_id, root_surface_id),
    );
}

#[test]
fn fullscreen_canonical_scene_keeps_subsurface_scanout_identity_aligned_after_filtering() {
    let mut state = CompositorState::new(None);
    let output_size = BufferSize::new(state.output_size.width, state.output_size.height)
        .expect("configured output size");
    let fullscreen_root = x11_scanout_surface(
        361,
        output_size.width,
        output_size.height,
        SurfacePlacement::absolute_root_at(0, 0),
        DrmFormat::Xrgb8888,
    );
    let scanout_child = x11_scanout_surface(
        362,
        output_size.width,
        output_size.height,
        SurfacePlacement::subsurface(361, 0, 0),
        DrmFormat::Xrgb8888,
    );
    state.install_native_frame_test_scene(
        vec![
            super::desktop_window_tests::x11_shm_surface(
                360,
                32,
                32,
                SurfacePlacement::absolute_root_at(0, 0),
            ),
            fullscreen_root,
            scanout_child,
        ],
        &[
            (360, WindowId::from_raw(71).expect("culled window id")),
            (361, WindowId::from_raw(72).expect("fullscreen window id")),
        ],
        Some(361),
    );
    for surface_id in [360, 361, 362] {
        state.surface_presentation_generations.insert(surface_id, 1);
    }
    let blur_program = crate::effects::builtin_background_blur_program_id();
    let blur_region = EffectRegion::from_rect(EffectRect::new(0, 0, 32, 32).unwrap());
    assert!(state.set_internal_surface_effect(
        360,
        EffectAnchor::BeforeSurface(360),
        blur_program,
        blur_region.clone(),
    ));
    assert!(state.set_internal_surface_effect(
        362,
        EffectAnchor::BeforeSurface(362),
        blur_program,
        blur_region,
    ));

    let raw_ids = state
        .active_scene_surfaces()
        .iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    let scene = state.canonical_presentation_scene();
    let canonical_ids = scene
        .surfaces
        .iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    let analysis = state.direct_scanout_scene_analysis();
    let candidate = analysis
        .candidate
        .as_ref()
        .expect("fullscreen subsurface should remain a direct candidate");
    let sample_time = AnimationTime::monotonic_now().expect("monotonic sample time");
    let targets = state.native_frame_presentation_targets_for_scene(&scene);
    let presentation = state.presentation_scene_sample_for_targets_at_with_source(
        sample_time,
        PresentationSampleTimeSource::MonotonicFallback,
        &targets,
    );
    let lifecycle = state.lifecycle_scene_sample_at(sample_time);
    let canonical_effects =
        state.direct_scanout_effect_identity_scene(&scene, &presentation, &lifecycle);
    let canonical_group_order = scene.visual_group_orders()[1].expect("surviving group order");

    assert_eq!(raw_ids, [360, 361, 362]);
    assert_eq!(canonical_ids, [361, 362]);
    assert_eq!(scene.surface_index(362), Some(1));
    assert_eq!(scene.owner_root_for_surface(362), Some(361));
    assert_eq!(
        scene.scene_node_for_surface(362),
        state.scene_node_id_for_surface(362)
    );
    assert_eq!(candidate.surface_id, 362);
    assert_eq!(candidate.root_surface_id, 361);
    assert_eq!(
        candidate.surface_scene_node_id,
        scene.scene_node_for_surface(362).expect("child SceneNode")
    );
    assert_eq!(
        candidate.window_scene_node_id,
        state
            .presentation_scene_node_id_for_root(361)
            .expect("fullscreen root SceneNode")
    );
    assert_eq!(candidate.surface_presentation_generation, 1);
    assert!(canonical_effects.instances.iter().all(|instance| {
        !matches!(
            instance.anchor,
            EffectAnchor::BeforeSurface(360)
                | EffectAnchor::ReplaceSurface(360)
                | EffectAnchor::AfterSurface(360)
        )
    }));
    let surviving_effect = canonical_effects
        .instances
        .iter()
        .find(|instance| instance.anchor == EffectAnchor::BeforeSurface(362))
        .expect("effect on the surviving child");
    assert_eq!(
        surviving_effect.scene_order.group_order,
        canonical_group_order
    );
    assert_eq!(
        surviving_effect.scene_order.surface_order,
        match surviving_effect.anchor_scope {
            crate::compositor::EffectAnchorScope::Surface => 1,
            crate::compositor::EffectAnchorScope::VisualGroup => 0,
        }
    );
    assert_eq!(
        candidate.effect_identity_signature,
        canonical_effects.signature
    );
    assert!(
        canonical_effects
            .instances
            .iter()
            .all(|effect| effect.anchor != EffectAnchor::BeforeSurface(360))
    );
    assert!(analysis.effects.instances.iter().any(|effect| {
        effect.anchor == EffectAnchor::BeforeSurface(360)
            && effect.disposition == DirectScanoutEffectDisposition::PresentationCulled
    }));
    assert!(analysis.effects.instances.iter().any(|effect| {
        effect.anchor == EffectAnchor::BeforeSurface(362)
            && effect.disposition == DirectScanoutEffectDisposition::OccludedByOpaqueScanoutSource
    }));
    assert!(
        !analysis
            .effects
            .instances
            .iter()
            .any(|effect| effect.disposition == DirectScanoutEffectDisposition::UnknownOrder)
    );
    assert!(!analysis.effects.requires_composition);
}

#[test]
fn transitioning_fullscreen_keeps_raw_scene_content_and_does_not_gain_scanout() {
    let mut state = CompositorState::new(None);
    let output_size = BufferSize::new(state.output_size.width, state.output_size.height)
        .expect("configured output size");
    let fullscreen_root = x11_scanout_surface(
        370,
        output_size.width,
        output_size.height,
        SurfacePlacement::absolute_root_at(0, 0),
        DrmFormat::Xrgb8888,
    );
    let visible_above = super::desktop_window_tests::x11_shm_surface(
        371,
        64,
        64,
        SurfacePlacement::absolute_root_at(0, 0),
    );
    state.install_native_frame_test_scene(
        vec![fullscreen_root, visible_above],
        &[
            (370, WindowId::from_raw(73).expect("fullscreen window id")),
            (371, WindowId::from_raw(74).expect("visible window id")),
        ],
        Some(370),
    );
    state.surface_presentation_generations.insert(370, 1);
    state.surface_presentation_generations.insert(371, 1);
    state.start_test_presentation_transition(
        370,
        PresentationRect::new(100.0, 100.0, 320.0, 200.0).expect("transition start"),
        PresentationRect::new(0.0, 0.0, 1280.0, 800.0).expect("transition target"),
        AnimationTime::from_nanos(0),
    );

    let raw_ids = state
        .active_scene_surfaces()
        .iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    let scene = state.canonical_presentation_scene();
    let canonical_ids = scene
        .surfaces
        .iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    let analysis = state.direct_scanout_scene_analysis();

    assert_eq!(
        scene.fullscreen_plan.mode,
        FullscreenCompositionMode::Transitioning
    );
    assert_eq!(raw_ids, [370, 371]);
    assert_eq!(canonical_ids, raw_ids);
    assert!(
        analysis
            .coverage
            .visible_content_above
            .iter()
            .any(|content| content.root_surface_id == 371)
    );
    assert!(analysis.candidate.is_none());
    assert!(
        analysis
            .blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::ApplicationContentAbove)
    );
}

#[test]
fn same_size_scene_stays_on_the_accepted_direct_candidate_path() {
    let mut state = CompositorState::new(None);
    let output_size = BufferSize::new(state.output_size.width, state.output_size.height)
        .expect("configured output size");
    install_probe_scanout_surface(&mut state, 301, output_size);

    let direct_candidate = state
        .direct_scanout_scene_candidate()
        .expect("same-size scene remains an accepted direct candidate");

    assert_eq!(direct_candidate.buffer_size, output_size);
    assert_eq!(direct_candidate.output_size, output_size);
    assert!(
        state
            .direct_scanout_probe_scene_analysis()
            .probe_candidate
            .is_none()
    );
}

#[test]
fn simple_size_mismatch_is_a_probe_candidate_with_distinct_source_and_output_sizes() {
    let mut state = CompositorState::new(None);
    let output_size = BufferSize::new(state.output_size.width, state.output_size.height)
        .expect("configured output size");
    let source_size = BufferSize::new(1600, 900).expect("source size");
    install_probe_scanout_surface(&mut state, 302, source_size);

    assert_eq!(
        state.direct_scanout_scene_candidate().unwrap_err(),
        DirectScanoutSceneRejection::BufferSizeMismatch
    );
    let analysis = state.direct_scanout_probe_scene_analysis();
    assert!(analysis.candidate.is_none());
    let probe = analysis.probe_candidate.unwrap_or_else(|| {
        panic!(
            "size mismatch is eligible for diagnostics; blockers={:?} coverage={:?}",
            analysis.blockers.reasons(),
            analysis.coverage.covering_application_group
        )
    });

    assert_eq!(probe.buffer_size, source_size);
    assert_eq!(probe.buffer.size(), source_size);
    assert_eq!(probe.output_size, output_size);
    assert_ne!(probe.buffer_size, probe.output_size);
}

#[test]
fn probe_rejects_non_unit_scale_non_normal_transform_and_non_identity_viewport() {
    let output_size = BufferSize::new(1280, 800).unwrap();
    let source_size = output_size;

    let mut scaled = CompositorState::new(None);
    install_probe_scanout_surface_with_metadata(
        &mut scaled,
        303,
        source_size,
        2,
        wayland_server::protocol::wl_output::Transform::Normal,
        None,
        Some(output_size),
    );
    assert!(
        scaled
            .direct_scanout_probe_scene_analysis()
            .probe_candidate
            .is_none()
    );
    let scaled_blockers = scaled.direct_scanout_scene_blockers();
    assert!(
        scaled_blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::BufferScaleUnsupported),
        "unexpected blockers: {:?}",
        scaled_blockers.reasons()
    );

    let mut transformed = CompositorState::new(None);
    install_probe_scanout_surface_with_metadata(
        &mut transformed,
        304,
        source_size,
        1,
        wayland_server::protocol::wl_output::Transform::_90,
        None,
        Some(output_size),
    );
    assert!(
        transformed
            .direct_scanout_probe_scene_analysis()
            .probe_candidate
            .is_none()
    );
    assert!(
        transformed
            .direct_scanout_scene_blockers()
            .reasons()
            .contains(&DirectScanoutSceneRejection::BufferTransformUnsupported)
    );

    let mut cropped = CompositorState::new(None);
    install_probe_scanout_surface_with_metadata(
        &mut cropped,
        305,
        source_size,
        1,
        wayland_server::protocol::wl_output::Transform::Normal,
        Some(ViewportSourceRect {
            x: 0.0,
            y: 0.0,
            width: 1599.0,
            height: 900.0,
        }),
        Some(output_size),
    );
    assert!(
        cropped
            .direct_scanout_probe_scene_analysis()
            .probe_candidate
            .is_none()
    );
    assert!(
        cropped
            .direct_scanout_scene_blockers()
            .reasons()
            .contains(&DirectScanoutSceneRejection::ViewportSourceNonIdentity)
    );

    let mut non_full_destination = CompositorState::new(None);
    install_probe_scanout_surface_with_metadata(
        &mut non_full_destination,
        306,
        source_size,
        1,
        wayland_server::protocol::wl_output::Transform::Normal,
        None,
        Some(BufferSize::new(1800, 1000).unwrap()),
    );
    assert!(
        non_full_destination
            .direct_scanout_probe_scene_analysis()
            .probe_candidate
            .is_none()
    );
    assert!(
        non_full_destination
            .direct_scanout_scene_blockers()
            .reasons()
            .contains(&DirectScanoutSceneRejection::ViewportDestinationNonIdentity)
    );
}

fn install_off_output_xdg_window(state: &mut CompositorState, root_surface_id: u32) -> SceneNodeId {
    let window_id = state.allocate_window_id().expect("off-output window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_id, root_surface_id))
        .expect("off-output window");
    state.append_renderable_surface(super::desktop_window_tests::x11_shm_surface(
        root_surface_id,
        16,
        16,
        SurfacePlacement::absolute_root_at(2_000, 0),
    ));
    state
        .surface_presentation_generations
        .insert(root_surface_id, 1);
    state.rebuild_active_scene_view();
    state
        .scene_node_id_for_window_group(window_id)
        .expect("off-output WindowGroup scene node")
}

fn install_xwayland_backing_replacement_candidate(
    state: &mut CompositorState,
    root_a: u32,
    root_b: u32,
    generation_id: u64,
) -> (SceneNodeId, WindowId) {
    let (width, height) = (state.output_size.width, state.output_size.height);
    let generation = XwaylandGeneration::new(NonZeroU64::new(generation_id).expect("generation"));
    let snapshot = x11_output_snapshot(generation, generation_id as u32 * 10, root_a);
    let handle = snapshot.handle;
    install_x11_scanout_surface(
        state,
        x11_scanout_surface(
            root_a,
            width,
            height,
            SurfacePlacement::absolute_root_at(0, 0),
            DrmFormat::Xrgb8888,
        ),
        snapshot,
    );
    let window_id = state
        .window_id_for_surface(root_a)
        .expect("candidate window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("candidate WindowGroup scene node");

    state.retire_xwayland_attachment(root_a);
    assert_eq!(state.attach_x11_surface(handle, root_b), Ok(Some(root_a)));
    state.append_renderable_surface(x11_scanout_surface(
        root_b,
        width,
        height,
        SurfacePlacement::absolute_root_at(0, 0),
        DrmFormat::Xrgb8888,
    ));
    state.surface_presentation_generations.insert(root_b, 1);
    state.rebuild_active_scene_view();

    (scene_node_id, window_id)
}

fn publish_physical_presentation(
    state: &mut CompositorState,
    output_id: OutputId,
    scene_node_id: SceneNodeId,
    root_surface_id: u32,
    transform: Option<(PresentationRect, PresentationRect)>,
    opacity: Option<PresentationOpacity>,
    frame_id: u64,
) {
    let mut sample = PresentationSceneSample::empty_for_output(
        output_id,
        AnimationTime::from_nanos(frame_id),
        PresentationSampleTimeSource::ZeroFallback,
    );
    if let Some((canonical_rect, presented_rect)) = transform {
        sample
            .transforms
            .push(PresentationGroupTransform::with_scene_node(
                scene_node_id,
                root_surface_id,
                PresentationTransactionId::new(NonZeroU64::new(77).expect("transaction")),
                PresentationRevisionId::new(NonZeroU64::new(88).expect("revision")),
                canonical_rect,
                presented_rect,
                false,
            ));
    }
    if let Some(opacity) = opacity {
        sample
            .opacities
            .push(PresentationGroupOpacity::with_scene_node(
                scene_node_id,
                root_surface_id,
                opacity,
                None,
            ));
    }
    state.publish_presented_presentation(frame_id, &sample.frame_snapshot());
}

fn publish_physical_clip_presentation(
    state: &mut CompositorState,
    output_id: OutputId,
    scene_node_id: SceneNodeId,
    root_surface_id: u32,
    clip: PresentationClip,
    presented_clip: Option<PresentationClipRect>,
    frame_id: u64,
) {
    let mut sample = PresentationSceneSample::empty_for_output(
        output_id,
        AnimationTime::from_nanos(frame_id),
        PresentationSampleTimeSource::ZeroFallback,
    );
    if !clip.is_unbounded() {
        sample.clips.push(PresentationGroupClip::with_scene_node(
            scene_node_id,
            root_surface_id,
            clip,
            presented_clip,
            None,
        ));
    }
    state.publish_presented_presentation(frame_id, &sample.frame_snapshot());
}

#[test]
fn candidate_opacity_track_blocks_direct_scanout() {
    let mut state = CompositorState::new(None);
    let (width, height) = (state.output_size.width, state.output_size.height);
    let generation = XwaylandGeneration::new(NonZeroU64::new(35).expect("generation"));
    install_x11_scanout_surface(
        &mut state,
        x11_scanout_surface(
            351,
            width,
            height,
            SurfacePlacement::absolute_root_at(0, 0),
            DrmFormat::Xrgb8888,
        ),
        x11_output_snapshot(generation, 351, 351),
    );
    let window_id = state.window_id_for_surface(351).expect("visible window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("visible WindowGroup scene node");
    state.presentation_animator.set_enabled(true);
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::opacity(
            AnimationTime::from_nanos(0),
            vec![PresentationOpacityMutation::new(
                scene_node_id,
                PresentationOpacity::OPAQUE,
                PresentationOpacity::new(0.5).expect("half opacity"),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("visible opacity transaction");

    assert!(
        state
            .direct_scanout_scene_blockers()
            .reasons()
            .contains(&DirectScanoutSceneRejection::PresentationOpacity)
    );
}

#[test]
fn unrelated_off_output_opacity_track_does_not_block_direct_scanout() {
    let mut state = CompositorState::new(None);
    let (width, height) = (state.output_size.width, state.output_size.height);
    let generation = XwaylandGeneration::new(NonZeroU64::new(352).expect("generation"));
    install_x11_scanout_surface(
        &mut state,
        x11_scanout_surface(
            352,
            width,
            height,
            SurfacePlacement::absolute_root_at(0, 0),
            DrmFormat::Xrgb8888,
        ),
        x11_output_snapshot(generation, 352, 352),
    );
    let unrelated_node = install_off_output_xdg_window(&mut state, 353);
    state.presentation_animator.set_enabled(true);
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::opacity(
            AnimationTime::from_nanos(0),
            vec![PresentationOpacityMutation::new(
                unrelated_node,
                PresentationOpacity::OPAQUE,
                PresentationOpacity::new(0.5).expect("half opacity"),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("off-output opacity transaction");

    let blockers = state.direct_scanout_scene_blockers();
    assert!(
        !blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::PresentationOpacity)
    );
    assert!(state.direct_scanout_scene_candidate().is_ok());
}

#[test]
fn candidate_geometry_track_blocks_direct_scanout() {
    let mut state = CompositorState::new(None);
    let (width, height) = (state.output_size.width, state.output_size.height);
    let generation = XwaylandGeneration::new(NonZeroU64::new(354).expect("generation"));
    install_x11_scanout_surface(
        &mut state,
        x11_scanout_surface(
            354,
            width,
            height,
            SurfacePlacement::absolute_root_at(0, 0),
            DrmFormat::Xrgb8888,
        ),
        x11_output_snapshot(generation, 354, 354),
    );
    let window_id = state.window_id_for_surface(354).expect("candidate window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("candidate WindowGroup scene node");
    state.presentation_animator.set_enabled(true);
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                PresentationRect::new(0.0, 0.0, 100.0, 100.0).expect("geometry start"),
                PresentationRect::new(1.0, 0.0, 100.0, 100.0).expect("geometry target"),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("candidate geometry transaction");

    assert!(
        state
            .direct_scanout_scene_blockers()
            .reasons()
            .contains(&DirectScanoutSceneRejection::AnimationTransform)
    );
}

#[test]
fn unrelated_off_output_geometry_track_does_not_block_direct_scanout() {
    let mut state = CompositorState::new(None);
    let (width, height) = (state.output_size.width, state.output_size.height);
    let generation = XwaylandGeneration::new(NonZeroU64::new(355).expect("generation"));
    install_x11_scanout_surface(
        &mut state,
        x11_scanout_surface(
            355,
            width,
            height,
            SurfacePlacement::absolute_root_at(0, 0),
            DrmFormat::Xrgb8888,
        ),
        x11_output_snapshot(generation, 355, 355),
    );
    let unrelated_node = install_off_output_xdg_window(&mut state, 356);
    state.presentation_animator.set_enabled(true);
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                unrelated_node,
                PresentationRect::new(0.0, 0.0, 16.0, 16.0).expect("geometry start"),
                PresentationRect::new(1.0, 0.0, 16.0, 16.0).expect("geometry target"),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("off-output geometry transaction");

    let blockers = state.direct_scanout_scene_blockers();
    assert!(
        !blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::AnimationTransform)
    );
    assert!(state.direct_scanout_scene_candidate().is_ok());
}

#[test]
fn unrelated_track_without_output_candidate_does_not_report_animation_blockers() {
    let mut state = CompositorState::new(None);
    let unrelated_node = install_off_output_xdg_window(&mut state, 357);
    state.presentation_animator.set_enabled(true);
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::mixed(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                unrelated_node,
                PresentationRect::new(0.0, 0.0, 16.0, 16.0).expect("geometry start"),
                PresentationRect::new(1.0, 0.0, 16.0, 16.0).expect("geometry target"),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
            vec![PresentationOpacityMutation::new(
                unrelated_node,
                PresentationOpacity::OPAQUE,
                PresentationOpacity::new(0.5).expect("half opacity"),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("off-output mixed transaction");

    let blockers = state.direct_scanout_scene_blockers();
    assert!(
        blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::NoOutputCoveringApplication)
    );
    assert!(
        !blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::AnimationTransform)
    );
    assert!(
        !blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::PresentationOpacity)
    );
}

#[test]
fn xwayland_backing_replacement_preserves_candidate_presentation_track_blockers() {
    let mut state = CompositorState::new(None);
    let (width, height) = (state.output_size.width, state.output_size.height);
    let generation = XwaylandGeneration::new(NonZeroU64::new(358).expect("generation"));
    let root_a = 358;
    let root_b = 359;
    let snapshot = x11_output_snapshot(generation, 3_580, root_a);
    let handle = snapshot.handle;
    install_x11_scanout_surface(
        &mut state,
        x11_scanout_surface(
            root_a,
            width,
            height,
            SurfacePlacement::absolute_root_at(0, 0),
            DrmFormat::Xrgb8888,
        ),
        snapshot,
    );
    let window_id = state
        .window_id_for_surface(root_a)
        .expect("candidate window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("candidate WindowGroup scene node");
    state.presentation_animator.set_enabled(true);
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::mixed(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                PresentationRect::new(0.0, 0.0, 100.0, 100.0).expect("geometry start"),
                PresentationRect::new(1.0, 0.0, 100.0, 100.0).expect("geometry target"),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
            vec![PresentationOpacityMutation::new(
                scene_node_id,
                PresentationOpacity::OPAQUE,
                PresentationOpacity::new(0.5).expect("half opacity"),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("candidate presentation transaction");

    state.retire_xwayland_attachment(root_a);
    assert_eq!(state.attach_x11_surface(handle, root_b), Ok(Some(root_a)));
    state.append_renderable_surface(x11_scanout_surface(
        root_b,
        width,
        height,
        SurfacePlacement::absolute_root_at(0, 0),
        DrmFormat::Xrgb8888,
    ));
    state.surface_presentation_generations.insert(root_b, 1);
    state.rebuild_active_scene_view();

    assert_eq!(
        state.presentation_scene_node_id_for_root(root_b),
        Some(scene_node_id)
    );
    let blockers = state.direct_scanout_scene_blockers();
    assert!(
        blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::AnimationTransform)
    );
    assert!(
        blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::PresentationOpacity)
    );
}

#[test]
fn physically_presented_geometry_remains_a_direct_scanout_blocker_after_track_retirement() {
    let mut state = CompositorState::new(None);
    let (width, height) = (state.output_size.width, state.output_size.height);
    let root_surface_id = 365;
    let generation = XwaylandGeneration::new(NonZeroU64::new(365).expect("generation"));
    install_x11_scanout_surface(
        &mut state,
        x11_scanout_surface(
            root_surface_id,
            width,
            height,
            SurfacePlacement::absolute_root_at(0, 0),
            DrmFormat::Xrgb8888,
        ),
        x11_output_snapshot(generation, root_surface_id, root_surface_id),
    );
    let window_id = state
        .window_id_for_surface(root_surface_id)
        .expect("candidate window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("candidate WindowGroup scene node");
    let output_id = state.ensure_native_output_id().expect("output identity");
    let canonical_rect = PresentationRect::new(0.0, 0.0, f64::from(width), f64::from(height))
        .expect("canonical candidate rect");
    let presented_rect = PresentationRect::new(8.0, 0.0, f64::from(width), f64::from(height))
        .expect("nonidentity presented rect");
    let mut sample = PresentationSceneSample::empty_for_output(
        output_id,
        AnimationTime::from_nanos(1),
        PresentationSampleTimeSource::ZeroFallback,
    );
    sample
        .transforms
        .push(PresentationGroupTransform::with_scene_node(
            scene_node_id,
            root_surface_id,
            PresentationTransactionId::new(NonZeroU64::new(77).expect("transaction")),
            PresentationRevisionId::new(NonZeroU64::new(88).expect("revision")),
            canonical_rect,
            presented_rect,
            false,
        ));
    state.publish_presented_presentation(1, &sample.frame_snapshot());

    assert!(
        !state
            .presentation_animator
            .has_geometry_track(scene_node_id)
    );
    assert!(
        state
            .direct_scanout_scene_blockers()
            .reasons()
            .contains(&DirectScanoutSceneRejection::AnimationTransform)
    );
}

#[test]
fn physical_presentation_owner_survives_xwayland_backing_replacement() {
    let mut state = CompositorState::new(None);
    let root_a = 366;
    let root_b = 367;
    let (scene_node_id, window_id) =
        install_xwayland_backing_replacement_candidate(&mut state, root_a, root_b, 366);
    let output_id = state.ensure_native_output_id().expect("output identity");
    let (width, height) = (state.output_size.width, state.output_size.height);
    let canonical_rect = PresentationRect::new(0.0, 0.0, f64::from(width), f64::from(height))
        .expect("canonical candidate rect");
    let presented_rect = PresentationRect::new(8.0, 0.0, f64::from(width), f64::from(height))
        .expect("nonidentity presented rect");
    publish_physical_presentation(
        &mut state,
        output_id,
        scene_node_id,
        root_a,
        Some((canonical_rect, presented_rect)),
        Some(PresentationOpacity::new(0.5).expect("half opacity")),
        1,
    );

    assert!(
        !state
            .presentation_animator
            .has_geometry_track(scene_node_id)
    );
    assert!(!state.presentation_animator.has_opacity_track(scene_node_id));
    assert!(
        state
            .window(window_id)
            .expect("candidate window")
            .canonical_opacity()
            .is_opaque()
    );
    assert_eq!(
        state.presentation_scene_node_id_for_root(root_b),
        Some(scene_node_id)
    );

    let analysis = state.direct_scanout_scene_analysis();
    assert_eq!(
        analysis
            .coverage
            .covering_application_group
            .as_ref()
            .map(|group| group.root_surface_id),
        Some(root_b)
    );
    let blockers = state.direct_scanout_scene_blockers();
    assert!(
        blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::AnimationTransform)
    );
    assert!(
        blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::PresentationOpacity)
    );

    publish_physical_presentation(&mut state, output_id, scene_node_id, root_b, None, None, 2);
    let recovered = state.direct_scanout_scene_blockers();
    assert!(
        !recovered
            .reasons()
            .contains(&DirectScanoutSceneRejection::AnimationTransform)
    );
    assert!(
        !recovered
            .reasons()
            .contains(&DirectScanoutSceneRejection::PresentationOpacity)
    );
    assert_eq!(
        state
            .direct_scanout_scene_candidate()
            .expect("identity physical frame restores scanout")
            .root_surface_id,
        root_b
    );
}

#[test]
fn unrelated_physical_presentation_owner_does_not_block_scanout_candidate() {
    let mut state = CompositorState::new(None);
    let (width, height) = (state.output_size.width, state.output_size.height);
    let generation = XwaylandGeneration::new(NonZeroU64::new(368).expect("generation"));
    install_x11_scanout_surface(
        &mut state,
        x11_scanout_surface(
            368,
            width,
            height,
            SurfacePlacement::absolute_root_at(0, 0),
            DrmFormat::Xrgb8888,
        ),
        x11_output_snapshot(generation, 368, 368),
    );
    let candidate_window = state.window_id_for_surface(368).expect("candidate window");
    let candidate_node = state
        .scene_node_id_for_window_group(candidate_window)
        .expect("candidate WindowGroup scene node");
    let unrelated_node = install_off_output_xdg_window(&mut state, 369);
    let output_id = state.ensure_native_output_id().expect("output identity");
    let canonical_rect = PresentationRect::new(0.0, 0.0, 16.0, 16.0).expect("unrelated rect");
    let presented_rect =
        PresentationRect::new(4.0, 0.0, 16.0, 16.0).expect("unrelated nonidentity rect");
    publish_physical_presentation(
        &mut state,
        output_id,
        unrelated_node,
        369,
        Some((canonical_rect, presented_rect)),
        Some(PresentationOpacity::new(0.5).expect("half opacity")),
        1,
    );

    assert_eq!(
        state.presentation_scene_node_id_for_root(368),
        Some(candidate_node)
    );
    let blockers = state.direct_scanout_scene_blockers();
    assert!(
        !blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::AnimationTransform)
    );
    assert!(
        !blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::PresentationOpacity)
    );
    assert!(state.direct_scanout_scene_candidate().is_ok());
}

#[test]
fn candidate_clip_track_blocks_direct_scanout_but_unrelated_track_does_not() {
    let mut state = CompositorState::new(None);
    let (width, height) = (state.output_size.width, state.output_size.height);
    let candidate_root = 370;
    let generation = XwaylandGeneration::new(NonZeroU64::new(370).expect("generation"));
    install_x11_scanout_surface(
        &mut state,
        x11_scanout_surface(
            candidate_root,
            width,
            height,
            SurfacePlacement::absolute_root_at(0, 0),
            DrmFormat::Xrgb8888,
        ),
        x11_output_snapshot(generation, candidate_root, candidate_root),
    );
    let candidate_window = state
        .window_id_for_surface(candidate_root)
        .expect("candidate window");
    let candidate_node = state
        .scene_node_id_for_window_group(candidate_window)
        .expect("candidate WindowGroup");
    state.presentation_animator.set_enabled(true);
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::clip(
            AnimationTime::from_nanos(0),
            vec![PresentationClipMutation::new(
                candidate_node,
                PresentationClip::Rect(
                    PresentationClipRect::new(0.0, 0.0, 10.0, 10.0).expect("clip start"),
                ),
                PresentationClip::Rect(
                    PresentationClipRect::new(1.0, 0.0, 8.0, 10.0).expect("clip target"),
                ),
                None,
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("candidate clip transaction");
    assert!(
        state
            .direct_scanout_scene_blockers()
            .reasons()
            .contains(&DirectScanoutSceneRejection::PresentationClip)
    );

    state.presentation_animator.cancel_clip(candidate_node);
    let unrelated_node = install_off_output_xdg_window(&mut state, 371);
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::clip(
            AnimationTime::from_nanos(0),
            vec![PresentationClipMutation::new(
                unrelated_node,
                PresentationClip::Rect(
                    PresentationClipRect::new(0.0, 0.0, 10.0, 10.0).expect("clip start"),
                ),
                PresentationClip::Rect(
                    PresentationClipRect::new(1.0, 0.0, 8.0, 10.0).expect("clip target"),
                ),
                None,
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("off-output clip transaction");
    let blockers = state.direct_scanout_scene_blockers();
    assert!(
        !blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::PresentationClip)
    );
    assert!(state.direct_scanout_scene_candidate().is_ok());
}

#[test]
fn canonical_presentation_clip_blocks_direct_scanout() {
    let mut state = CompositorState::new(None);
    let (width, height) = (state.output_size.width, state.output_size.height);
    let root_surface_id = 372;
    let generation = XwaylandGeneration::new(NonZeroU64::new(372).expect("generation"));
    install_x11_scanout_surface(
        &mut state,
        x11_scanout_surface(
            root_surface_id,
            width,
            height,
            SurfacePlacement::absolute_root_at(0, 0),
            DrmFormat::Xrgb8888,
        ),
        x11_output_snapshot(generation, root_surface_id, root_surface_id),
    );
    let window_id = state
        .window_id_for_surface(root_surface_id)
        .expect("candidate window");
    state
        .window_mut(window_id)
        .expect("candidate DesktopWindow")
        .set_canonical_clip(PresentationClip::Rect(
            PresentationClipRect::new(0.0, 0.0, 64.0, 64.0).expect("canonical clip"),
        ));

    let blockers = state.direct_scanout_scene_blockers();
    assert!(
        blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::PresentationClip)
    );
    assert_eq!(
        DirectScanoutSceneRejection::PresentationClip.as_str(),
        "presentation_clip"
    );
}

#[test]
fn xwayland_clip_track_and_physical_clip_follow_window_group_across_backing_replacement() {
    let mut state = CompositorState::new(None);
    let (width, height) = (state.output_size.width, state.output_size.height);
    let root_a = 373;
    let root_b = 374;
    let generation = XwaylandGeneration::new(NonZeroU64::new(373).expect("generation"));
    let snapshot = x11_output_snapshot(generation, 3_730, root_a);
    let handle = snapshot.handle;
    install_x11_scanout_surface(
        &mut state,
        x11_scanout_surface(
            root_a,
            width,
            height,
            SurfacePlacement::absolute_root_at(0, 0),
            DrmFormat::Xrgb8888,
        ),
        snapshot,
    );
    let window_id = state
        .window_id_for_surface(root_a)
        .expect("candidate window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("candidate WindowGroup");
    let target_clip = PresentationClip::Rect(
        PresentationClipRect::new(5.0, 6.0, 70.0, 80.0).expect("canonical Clip"),
    );
    state
        .window_mut(window_id)
        .expect("candidate DesktopWindow")
        .set_canonical_clip(target_clip);
    state.presentation_animator.set_enabled(true);
    let transaction = state
        .presentation_animator
        .commit(PresentationTransactionRequest::clip(
            AnimationTime::from_nanos(0),
            vec![PresentationClipMutation::new(
                scene_node_id,
                PresentationClip::Rect(
                    PresentationClipRect::new(0.0, 0.0, 100.0, 100.0).expect("clip start"),
                ),
                target_clip,
                None,
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("active Clip revision");
    let revision_id = transaction.members()[0].revision_id();

    state.retire_xwayland_attachment(root_a);
    assert_eq!(state.attach_x11_surface(handle, root_b), Ok(Some(root_a)));
    state.append_renderable_surface(x11_scanout_surface(
        root_b,
        width,
        height,
        SurfacePlacement::absolute_root_at(0, 0),
        DrmFormat::Xrgb8888,
    ));
    state.surface_presentation_generations.insert(root_b, 1);
    state.rebuild_active_scene_view();
    assert_eq!(state.window_id_for_surface(root_b), Some(window_id));
    assert_eq!(
        state.presentation_scene_node_id_for_root(root_b),
        Some(scene_node_id)
    );
    assert_eq!(
        state
            .window(window_id)
            .expect("same logical DesktopWindow")
            .canonical_clip(),
        target_clip
    );
    assert_eq!(
        state
            .presentation_animator
            .clip_track_revision(scene_node_id),
        Some(revision_id)
    );

    state.presentation_animator.cancel_clip(scene_node_id);
    state
        .window_mut(window_id)
        .expect("candidate DesktopWindow")
        .set_canonical_clip(PresentationClip::Unbounded);
    let output_id = state.ensure_native_output_id().expect("output identity");
    let physical_rect =
        PresentationClipRect::new(10.0, 12.0, 80.0, 60.0).expect("physical nonidentity clip");
    // This represents a submitted frame frozen with root A that promotes after B
    // has become the current render adapter.
    publish_physical_clip_presentation(
        &mut state,
        output_id,
        scene_node_id,
        root_a,
        PresentationClip::Rect(physical_rect),
        Some(physical_rect),
        1,
    );
    assert!(
        state
            .direct_scanout_scene_blockers()
            .reasons()
            .contains(&DirectScanoutSceneRejection::PresentationClip)
    );

    publish_physical_clip_presentation(
        &mut state,
        output_id,
        scene_node_id,
        root_b,
        PresentationClip::Unbounded,
        None,
        2,
    );
    let recovered = state.direct_scanout_scene_blockers();
    assert!(
        !recovered
            .reasons()
            .contains(&DirectScanoutSceneRejection::PresentationClip)
    );
    assert!(state.direct_scanout_scene_candidate().is_ok());
}
