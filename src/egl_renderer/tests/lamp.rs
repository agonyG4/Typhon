use super::*;

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
    let next_identity = test_lifecycle_identity(first.window_id, 2);
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
fn frozen_visual_signature_survives_reversal_but_changes_for_fresh_payload() {
    let first_sample = lamp_test_sample(0.5);
    let first_lamp = LifecycleFrameSnapshot::from_sample(&first_sample).samples[0];
    let first_source = &first_sample.samples[0].visual_source;
    let next_identity = test_lifecycle_identity(first_lamp.window_id, 2);
    let mut reversed_lamp = first_lamp;
    reversed_lamp.presentation_identity = next_identity;
    let reversed_source = LifecycleVisualSource {
        presentation_identity: next_identity,
        ..first_source.clone()
    };
    assert_eq!(
        lifecycle_visual_source_signature(&reversed_source, reversed_lamp, 1.0),
        lifecycle_visual_source_signature(first_source, first_lamp, 1.0),
        "owner revisions do not invalidate the same immutable payload"
    );

    let fresh_payload =
        compositor::PresentationRetainedVisualPayloadId::from_origin_identity(next_identity);
    let fresh_lamp = LifecycleFrameSample {
        presentation_identity: next_identity,
        payload_id: fresh_payload,
        ..first_lamp
    };
    let fresh_source = LifecycleVisualSource {
        presentation_identity: next_identity,
        payload_id: fresh_payload,
        ..first_source.clone()
    };
    assert_ne!(
        lifecycle_visual_source_signature(&fresh_source, fresh_lamp, 1.0),
        lifecycle_visual_source_signature(first_source, first_lamp, 1.0),
        "a new lifecycle payload cannot alias the previous resolved visual"
    );
}

#[test]
fn lamp_draw_commands_are_qualified_by_exact_presentation_identity() {
    let window_id = compositor::WindowId::from_raw(901).expect("test window id");
    let first = test_lifecycle_identity(window_id, 1);
    let second = test_lifecycle_identity(window_id, 2);
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
        let damage = lifecycle_damage_for_samples(&sample, 1920, 1080, 1.0);
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
