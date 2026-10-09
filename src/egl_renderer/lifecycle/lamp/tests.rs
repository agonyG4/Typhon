use super::*;
use crate::egl_renderer::tests::{
    GlesEffectTestHarness, lifecycle_test_lamp_sample as lamp_test_sample,
    lifecycle_transfer_texture_key as lifecycle_transfer_test_texture_key,
    test_lifecycle_identities_for_windows, test_lifecycle_identity,
    test_lifecycle_identity_sequence,
};
use oblivion_one::presentation_animation::PresentationRect;

fn set_lamp_test_texture_filter(gl: &glow::Context, texture: glow::Texture, filter: u32) {
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter as i32);
        gl.bind_texture(glow::TEXTURE_2D, None);
    }
}

fn lamp_test_texture_filters(gl: &glow::Context, texture: glow::Texture) -> (i32, i32) {
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        let filters = (
            gl.get_tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER),
            gl.get_tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER),
        );
        gl.bind_texture(glow::TEXTURE_2D, None);
        filters
    }
}

fn prepare_two_window_lamp_draw(
    harness: &mut GlesEffectTestHarness,
    sampling: SurfaceSampling,
) -> [glow::Texture; 2] {
    let requested_filter = match sampling {
        SurfaceSampling::ExactNearest => glow::NEAREST,
        SurfaceSampling::ScaledLinear => glow::LINEAR,
    };
    let conflicting_filter = match requested_filter {
        glow::NEAREST => glow::LINEAR,
        _ => glow::NEAREST,
    };
    let mut lifecycle = lamp_test_sample(0.5);
    let mut second_sample = lifecycle.samples[0].clone();
    second_sample.window_id = compositor::WindowId::from_raw(902).expect("second test window");
    let identities = test_lifecycle_identities_for_windows(&[
        lifecycle.samples[0].window_id,
        second_sample.window_id,
    ]);
    assert_eq!(identities[0], lifecycle.samples[0].presentation_identity);
    let second_identity = identities[1];
    second_sample.presentation_identity = second_identity;
    second_sample.root_surface_id = 2;
    second_sample.payload_id =
        compositor::PresentationRetainedVisualPayloadId::from_origin_identity(second_identity);
    assert_ne!(second_sample.payload_id, lifecycle.samples[0].payload_id);
    second_sample.visual_source.window_id = second_sample.window_id;
    second_sample.visual_source.presentation_identity = second_identity;
    second_sample.visual_source.root_surface_id = second_sample.root_surface_id;
    second_sample.visual_source.payload_id = second_sample.payload_id;
    lifecycle.samples.push(second_sample);
    harness.renderer.lifecycle.begin_frame(&lifecycle);

    let texture_key = lifecycle_transfer_test_texture_key(4, 4);
    let mut texture_ids = Vec::new();
    for (index, sample) in lifecycle.samples.iter().enumerate() {
        let texture = harness
            .renderer
            .effect_runtime
            .effect_resources
            .acquire(&harness.gl, texture_key)
            .expect("Lamp lifecycle texture allocates");
        let texture_id = harness
            .renderer
            .effect_runtime
            .effect_resources
            .texture(&texture)
            .expect("Lamp lifecycle texture is available");
        set_lamp_test_texture_filter(
            &harness.gl,
            texture_id,
            if index == 0 {
                requested_filter
            } else {
                conflicting_filter
            },
        );
        texture_ids.push(texture_id);
        harness
            .renderer
            .lifecycle
            .visual_store
            .resolved_visual_resources
            .insert(
                sample.payload_id,
                LifecycleResolvedVisualResource {
                    texture,
                    source_signature: 0,
                },
            );
    }
    assert_eq!(
        lamp_test_texture_filters(&harness.gl, texture_ids[0]),
        (requested_filter as i32, requested_filter as i32)
    );
    assert_eq!(
        lamp_test_texture_filters(&harness.gl, texture_ids[1]),
        (conflicting_filter as i32, conflicting_filter as i32)
    );
    assert_ne!(
        texture_ids[0], texture_ids[1],
        "the two Lamp windows must use distinct GL texture objects"
    );
    harness.renderer.lifecycle.rebuild_lamp_commands(
        &lifecycle,
        &[],
        &[],
        1.0,
        (320, 200),
        OutputFramebufferOrigin::BottomLeft,
    );
    assert_eq!(
        harness.renderer.lifecycle.lamp.commands.len(),
        2,
        "each lifecycle window contributes one Lamp draw"
    );
    for command in &mut harness.renderer.lifecycle.lamp.commands {
        command.sampling = sampling;
    }
    [texture_ids[0], texture_ids[1]]
}

fn draw_lamp_sampling_pass(harness: &mut GlesEffectTestHarness) {
    let renderer = &mut harness.renderer;
    let mut context = LifecycleRenderContext::new(
        &harness.gl,
        &mut renderer.scene_state,
        &mut renderer.effect_runtime,
        renderer.resources.texture_view(),
    );
    renderer
        .lifecycle
        .draw_lamp_overlay(&mut context, None)
        .expect("Lamp sampling draw succeeds");
}

fn assert_lamp_textures_have_filter(
    gl: &glow::Context,
    textures: [glow::Texture; 2],
    expected_filter: i32,
) {
    for texture in textures {
        assert_eq!(
            lamp_test_texture_filters(gl, texture),
            (expected_filter, expected_filter),
            "each bound Lamp texture must carry the requested min/mag filter"
        );
    }
}

#[test]
fn lamp_sets_linear_filter_on_each_texture_for_two_windows() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    let textures = prepare_two_window_lamp_draw(&mut harness, SurfaceSampling::ScaledLinear);

    draw_lamp_sampling_pass(&mut harness);

    assert_eq!(
        harness.renderer.lifecycle.evidence().consumed.len(),
        2,
        "both Lamp windows must reach an executed GLES draw"
    );
    assert_lamp_textures_have_filter(&harness.gl, textures, glow::LINEAR as i32);
}

#[test]
fn lamp_sets_nearest_filter_on_each_texture_for_two_windows() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    let textures = prepare_two_window_lamp_draw(&mut harness, SurfaceSampling::ExactNearest);

    draw_lamp_sampling_pass(&mut harness);

    assert_eq!(
        harness.renderer.lifecycle.evidence().consumed.len(),
        2,
        "both Lamp windows must reach an executed GLES draw"
    );
    assert_lamp_textures_have_filter(&harness.gl, textures, glow::NEAREST as i32);
}

#[test]
fn lamp_tracks_texture_filter_transitions_across_commands_and_render_passes() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    let textures = prepare_two_window_lamp_draw(&mut harness, SurfaceSampling::ScaledLinear);

    let first = harness.renderer.lifecycle.lamp.commands[0];
    let second_window = harness.renderer.lifecycle.lamp.commands[1];
    let mut first_linear = first;
    first_linear.sampling = SurfaceSampling::ScaledLinear;
    let mut first_nearest = first;
    first_nearest.sampling = SurfaceSampling::ExactNearest;
    let mut second_linear = second_window;
    second_linear.sampling = SurfaceSampling::ScaledLinear;
    harness.renderer.lifecycle.lamp.commands = vec![
        first_nearest,
        first_linear,
        first_nearest,
        first_linear,
        second_linear,
    ];

    let draws_before = harness.renderer.scene_state.frame_stats.draw_calls;
    draw_lamp_sampling_pass(&mut harness);
    assert_eq!(
        harness.renderer.scene_state.frame_stats.draw_calls - draws_before,
        5,
        "alternating filter commands and both windows must execute in the pass"
    );
    assert_eq!(
        lamp_test_texture_filters(&harness.gl, textures[0]),
        (glow::LINEAR as i32, glow::LINEAR as i32)
    );
    assert_eq!(
        lamp_test_texture_filters(&harness.gl, textures[1]),
        (glow::LINEAR as i32, glow::LINEAR as i32)
    );

    // Model an ordinary scene pass changing these lifecycle textures between
    // retained-effect passes. The next Lamp pass must establish its own state.
    set_lamp_test_texture_filter(&harness.gl, textures[0], glow::LINEAR);
    set_lamp_test_texture_filter(&harness.gl, textures[1], glow::LINEAR);
    let transition_modes = [
        SurfaceSampling::ScaledLinear,
        SurfaceSampling::ExactNearest,
        SurfaceSampling::ScaledLinear,
        SurfaceSampling::ExactNearest,
    ];
    let mut first_window_command_index = 0;
    for command in &mut harness.renderer.lifecycle.lamp.commands {
        if command.presentation_identity == first.presentation_identity {
            command.sampling = transition_modes[first_window_command_index];
            first_window_command_index += 1;
        } else {
            command.sampling = SurfaceSampling::ExactNearest;
        }
    }
    let draws_before_second_pass = harness.renderer.scene_state.frame_stats.draw_calls;
    draw_lamp_sampling_pass(&mut harness);

    assert_eq!(
        harness.renderer.scene_state.frame_stats.draw_calls - draws_before_second_pass,
        5,
        "the second pass must execute after the ordinary pass mutates both textures"
    );
    assert_eq!(
        harness.renderer.lifecycle.evidence().consumed.len(),
        2,
        "both lifecycle windows must be drawn again in the second pass"
    );
    assert_eq!(
        lamp_test_texture_filters(&harness.gl, textures[0]),
        (glow::NEAREST as i32, glow::NEAREST as i32)
    );
    assert_eq!(
        lamp_test_texture_filters(&harness.gl, textures[1]),
        (glow::NEAREST as i32, glow::NEAREST as i32)
    );
}

#[test]
fn lamp_mesh_identity_ignores_progress() {
    assert_eq!(
        lamp_geometry_key(
            &lamp_test_sample(0.1),
            &[],
            &[],
            1.0,
            OutputFramebufferOrigin::BottomLeft,
        ),
        lamp_geometry_key(
            &lamp_test_sample(0.9),
            &[],
            &[],
            1.0,
            OutputFramebufferOrigin::BottomLeft,
        )
    );
}

#[test]
fn lamp_mesh_identity_includes_sink_geometry() {
    let baseline = lamp_test_sample(0.5);
    let mut changed = baseline.clone();
    changed.samples[0].visual_group.sink_rect = PresentationRect::new(
        changed.samples[0].visual_group.sink_rect.x(),
        changed.samples[0].visual_group.sink_rect.y() + 1.0,
        changed.samples[0].visual_group.sink_rect.width(),
        changed.samples[0].visual_group.sink_rect.height(),
    )
    .expect("valid changed sink rectangle");
    assert_ne!(
        lamp_geometry_key(
            &baseline,
            &[],
            &[],
            1.0,
            OutputFramebufferOrigin::BottomLeft,
        ),
        lamp_geometry_key(&changed, &[], &[], 1.0, OutputFramebufferOrigin::BottomLeft,)
    );
}

#[test]
fn lifecycle_source_requires_exact_owner_payload_and_root() {
    let sample = lamp_test_sample(0.5);
    let first = LifecycleFrameSnapshot::from_sample(&sample).samples[0];
    let next_identity = test_lifecycle_identity_sequence(first.window_id, 2)[1];
    let mut reversed = sample.clone();
    reversed.samples[0].presentation_identity = next_identity;
    reversed.samples[0].visual_source.presentation_identity = next_identity;
    let reversed_frame = LifecycleFrameSnapshot::from_sample(&reversed).samples[0];

    assert!(lifecycle_visual_source_for_lamp(&reversed, reversed_frame).is_some());

    let wrong_payload =
        compositor::PresentationRetainedVisualPayloadId::from_origin_identity(next_identity);
    assert_ne!(first.payload_id, wrong_payload);
    assert!(lifecycle_draw_layer_matches_payload(
        EglDrawLayer::LifecycleResolvedVisual(first.payload_id),
        first.payload_id,
        LifecycleVisualSourceKind::ResolvedOwnedEffects,
    ));
    assert!(!lifecycle_draw_layer_matches_payload(
        EglDrawLayer::LifecycleResolvedVisual(wrong_payload),
        first.payload_id,
        LifecycleVisualSourceKind::ResolvedOwnedEffects,
    ));
    reversed.samples[0].payload_id = wrong_payload;
    let reversed_frame = LifecycleFrameSnapshot::from_sample(&reversed).samples[0];
    assert!(lifecycle_visual_source_for_lamp(&reversed, reversed_frame).is_none());

    reversed.samples[0].payload_id = first.payload_id;
    reversed.samples[0].root_surface_id = 2;
    let reversed_frame = LifecycleFrameSnapshot::from_sample(&reversed).samples[0];
    assert!(lifecycle_visual_source_for_lamp(&reversed, reversed_frame).is_none());
}

#[test]
fn lamp_draw_commands_are_qualified_by_exact_presentation_identity() {
    let window_id = compositor::WindowId::from_raw(901).expect("test window id");
    let identities = test_lifecycle_identity_sequence(window_id, 2);
    let first = identities[0];
    let second = identities[1];
    assert_ne!(first, second);
    let mut vertices = Vec::new();
    let mut commands = Vec::new();
    for presentation_identity in [first, second] {
        assert!(append_lamp_grid(
            &mut vertices,
            &mut commands,
            LampGridSpec {
                layer: EglDrawLayer::Surface(7),
                presentation_identity,
                bounds: EglRect::new(0.0, 0.0, 64.0, 64.0),
                uv: EglUvRect::new(0.0, 0.0, 1.0, 1.0),
            },
        ));
    }
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].presentation_identity, first);
    assert_eq!(commands[1].presentation_identity, second);
}

#[test]
fn every_visible_lamp_progress_sample_has_lifecycle_damage() {
    for progress in [0.01, 0.5, 0.97] {
        let sample = lamp_test_sample(progress);
        let damage = super::super::frame_state::LifecycleFrameState::damage_for_snapshot(
            &sample,
            (1920, 1080),
            1.0,
        );
        assert!(
            !matches!(damage, OutputDamage::Empty),
            "progress {progress} must remain eligible for presentation"
        );
    }
}

#[test]
fn lamp_mesh_topology_upload_count_is_independent_of_progress_frames() {
    let early = lamp_test_sample(0.1);
    let middle = lamp_test_sample(0.5);
    let late = lamp_test_sample(0.9);
    let early_key = lamp_geometry_key(&early, &[], &[], 1.0, OutputFramebufferOrigin::BottomLeft);
    assert_eq!(
        early_key,
        lamp_geometry_key(&middle, &[], &[], 1.0, OutputFramebufferOrigin::BottomLeft,)
    );
    assert_eq!(
        early_key,
        lamp_geometry_key(&late, &[], &[], 1.0, OutputFramebufferOrigin::BottomLeft,)
    );
}

#[test]
fn lamp_mesh_reaches_preferred_cell_size_before_global_budget() {
    let (columns, rows) = lamp_grid_subdivisions(1920.0, 1080.0, MAX_LAMP_VERTICES)
        .expect("preferred Lamp grid fits in global budget");
    assert_eq!(columns, 60);
    assert_eq!(rows, 34);
    assert!(1920.0 / columns as f32 <= 36.0);
    assert!(1080.0 / rows as f32 <= 36.0);
}

#[test]
fn lamp_mesh_coarsens_deterministically_under_global_vertex_budget() {
    let available = 60 * 20 * 6;
    let first = lamp_grid_subdivisions(1920.0, 1080.0, available).expect("reduced Lamp grid fits");
    let second = lamp_grid_subdivisions(1920.0, 1080.0, available).expect("reduced Lamp grid fits");
    assert_eq!(first, second);
    assert!(first.0.saturating_mul(first.1).saturating_mul(6) <= available);
    assert!(first.0 > 1 || first.1 > 1);
}

#[test]
fn lamp_mesh_vertex_budget_is_bounded_for_concurrent_windows() {
    let window_id = oblivion_one::compositor::WindowId::from_raw(1).expect("valid window id");
    let presentation_identity = test_lifecycle_identity(window_id, 1);
    let mut vertices = Vec::new();
    let mut commands = Vec::new();
    for _ in 0..32 {
        append_lamp_grid(
            &mut vertices,
            &mut commands,
            LampGridSpec {
                layer: EglDrawLayer::Surface(1),
                presentation_identity,
                bounds: EglRect::new(0.0, 0.0, 10_000.0, 10_000.0),
                uv: EglUvRect::new(0.0, 0.0, 1.0, 1.0),
            },
        );
    }
    assert!(vertices.len() <= MAX_LAMP_VERTICES);
    assert!(
        commands
            .iter()
            .all(|command| command.vertex_count <= MAX_LAMP_VERTICES as u32)
    );
}

#[test]
fn lamp_grid_admission_is_atomic_when_the_minimum_grid_does_not_fit() {
    let window_id = oblivion_one::compositor::WindowId::from_raw(2).expect("valid window id");
    let presentation_identity = test_lifecycle_identity(window_id, 2);
    let vertex = EglLampVertex {
        position: [0.0, 0.0],
        uv: [0.0, 0.0],
    };
    let mut vertices = vec![vertex; MAX_LAMP_VERTICES - 5];
    let mut commands = Vec::new();
    assert!(!append_lamp_grid(
        &mut vertices,
        &mut commands,
        LampGridSpec {
            layer: EglDrawLayer::Surface(2),
            presentation_identity,
            bounds: EglRect::new(10_000.0, 10_000.0, 10_000.0, 10_000.0),
            uv: EglUvRect::new(0.0, 0.0, 1.0, 1.0),
        },
    ));
    assert_eq!(vertices.len(), MAX_LAMP_VERTICES - 5);
    assert!(commands.is_empty());
}

#[test]
fn legacy_scissored_lifecycle_phases_prepare_once_after_all_repairs() {
    assert_eq!(
        legacy_scene_scissored_phase_plan(2).collect::<Vec<_>>(),
        vec![
            LegacySceneScissoredPhase::BaseRepair(0),
            LegacySceneScissoredPhase::BaseRepair(1),
            LegacySceneScissoredPhase::PrepareLifecycleSources,
            LegacySceneScissoredPhase::RestoreRepairScissor(0),
            LegacySceneScissoredPhase::Lamp(0),
            LegacySceneScissoredPhase::Squash(0),
            LegacySceneScissoredPhase::ExternalOverlays(0),
            LegacySceneScissoredPhase::RestoreRepairScissor(1),
            LegacySceneScissoredPhase::Lamp(1),
            LegacySceneScissoredPhase::Squash(1),
            LegacySceneScissoredPhase::ExternalOverlays(1),
        ]
    );
}
