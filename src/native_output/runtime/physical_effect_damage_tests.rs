use super::*;
use crate::egl_renderer::{
    BufferAge, EglPartialRepaintCapabilities, OutputDamage, OutputRect, PartialRepaintPlanner,
};
use crate::native_output::output::{
    NativeCursorDamageBounds, NativeDamageKind, NativeDamageRect, NativeOutputDamage,
    NativePresentationEffectInfluenceSnapshot, NativeSceneSnapshot, NativeSceneSurfaceSnapshot,
    NativeSurfaceDamageEvidence, clip_damage_for_frame_snapshots,
};
use oblivion_one::compositor::{
    FullscreenRenderPlanMetrics, PresentationFrameSnapshot, PresentationRect,
    PresentationSampleTimeSource, PresentationSceneSample, PresentedWindowGeometry,
    ResolvedEffectScene,
};
use oblivion_one::core::SceneNodeId;
use oblivion_one::effects::{
    EffectFootprint, EffectInstanceId, EffectOutsets, EffectRect, EffectRegion, plan_effect_damage,
};
use oblivion_one::presentation_animation::{
    PresentationClip, PresentationClipRect, PresentationGroupClip,
};
use oblivion_one::window_lifecycle_animation::{LifecycleFrameSnapshot, LifecycleSceneSample};

fn native_damage_covers(damage: &NativeOutputDamage, expected: NativeDamageRect) -> bool {
    damage.kind == NativeDamageKind::FullOutput
        || damage.rects.iter().any(|actual| {
            actual.x <= expected.x
                && actual.y <= expected.y
                && i64::from(actual.x) + i64::from(actual.width)
                    >= i64::from(expected.x) + i64::from(expected.width)
                && i64::from(actual.y) + i64::from(actual.height)
                    >= i64::from(expected.y) + i64::from(expected.height)
        })
}

fn output_damage_covers(damage: &OutputDamage, expected: OutputRect) -> bool {
    match damage {
        OutputDamage::Empty => false,
        OutputDamage::Full => true,
        OutputDamage::Rects(rects) => rects.iter().any(|actual| {
            actual.x <= expected.x
                && actual.y <= expected.y
                && i64::from(actual.x) + i64::from(actual.width)
                    >= i64::from(expected.x) + i64::from(expected.width)
                && i64::from(actual.y) + i64::from(actual.height)
                    >= i64::from(expected.y) + i64::from(expected.height)
        }),
    }
}

fn snapshot_with_clip(frame_id: u64, clip: PresentationClip) -> NativeFrameSceneSnapshot {
    let output_id = OutputId::from_raw(1).expect("nonzero output id");
    let owner = SceneNodeId::from_raw(7).expect("WindowGroup scene node");
    let unrelated_owner = SceneNodeId::from_raw(8).expect("unrelated WindowGroup scene node");
    let root_surface_id = 70;
    let unrelated_root_surface_id = 80;
    let source = NativeDamageRect {
        x: 10,
        y: 10,
        width: 10,
        height: 10,
    };
    let foreign_effect_output = EffectRect::new(8, 8, 14, 14).expect("foreign effect output");
    let mut scene = NativeSceneSnapshot::default();
    scene.surfaces.push(NativeSceneSurfaceSnapshot {
        scene_node_id: owner,
        surface_id: root_surface_id,
        visual_root_surface_id: root_surface_id,
        presentation_owner_root_surface_id: root_surface_id,
        bounds: Some(source),
        damage: NativeSurfaceDamageEvidence::AuthoritativeEmpty,
        content_generation: 1,
        commit_sequence: 1,
    });
    scene.effect_damage = EffectRegion::from_rect(foreign_effect_output);
    scene.effect_identity_signature = 44;
    scene.presentation_effect_influences = vec![
        NativePresentationEffectInfluenceSnapshot {
            scene_node_id: owner,
            presentation_owner_root_surface_id: root_surface_id,
            region: EffectRegion::from_rect(EffectRect::new(10, 10, 10, 10).unwrap()),
        },
        NativePresentationEffectInfluenceSnapshot {
            scene_node_id: unrelated_owner,
            presentation_owner_root_surface_id: unrelated_root_surface_id,
            region: EffectRegion::from_rect(foreign_effect_output),
        },
    ];

    let mut sample = PresentationSceneSample::empty_for_output(
        output_id,
        oblivion_one::compositor::AnimationTime::from_nanos(frame_id),
        PresentationSampleTimeSource::ZeroFallback,
    );
    let presented_clip = match &clip {
        PresentationClip::Unbounded => None,
        PresentationClip::Rect(rect) => Some(*rect),
    };
    sample.clips.push(PresentationGroupClip::with_scene_node(
        owner,
        root_surface_id,
        clip,
        presented_clip,
        None,
    ));
    let presentation = PresentationFrameSnapshot::from_sample_with_presented_windows(
        &sample,
        vec![PresentedWindowGeometry::with_scene_node(
            owner,
            root_surface_id,
            PresentationRect::new(10.0, 10.0, 10.0, 10.0).expect("window geometry"),
        )],
    );

    scene.physical_effect_damage = NativeEffectDamageFrameSnapshot {
        registry_generation: 0,
        instances: vec![NativeEffectDamageInstanceSnapshot {
            backdrop_anchor: None,
            instance_id: EffectInstanceId::new(1).expect("foreign effect instance"),
            region: EffectRegion::from_rect(foreign_effect_output),
            aggregate_footprint: EffectFootprint::symmetric(2),
        }],
        frame_local_dirty: EffectRegion::empty(),
        conservative_full: false,
    };

    NativeFrameSceneSnapshot {
        output_id,
        frame_id,
        render_generation: frame_id,
        scene,
        cursor_damage: NativeCursorDamageBounds::default(),
        presentation,
        lifecycle: LifecycleFrameSnapshot::default(),
        canonical_scene_evidence: None,
        window_exit_evidence: Vec::new(),
    }
}

fn snapshot_with_effect_recipe(
    frame_id: u64,
    source_rect: EffectRect,
    effect_rect: EffectRect,
    sample_radius: u32,
) -> NativeFrameSceneSnapshot {
    snapshot_with_effect_recipe_at_generation(frame_id, source_rect, effect_rect, sample_radius, 0)
}

fn snapshot_with_effect_recipe_at_generation(
    frame_id: u64,
    source_rect: EffectRect,
    effect_rect: EffectRect,
    sample_radius: u32,
    registry_generation: u64,
) -> NativeFrameSceneSnapshot {
    let mut snapshot = snapshot_with_clip(frame_id, PresentationClip::Unbounded);
    snapshot.scene.effect_damage = EffectRegion::from_rect(source_rect);
    snapshot.scene.effect_identity_signature = frame_id;
    snapshot.scene.physical_effect_damage = NativeEffectDamageFrameSnapshot {
        registry_generation,
        instances: vec![NativeEffectDamageInstanceSnapshot {
            backdrop_anchor: None,
            instance_id: EffectInstanceId::new(frame_id).expect("effect instance"),
            region: EffectRegion::from_rect(effect_rect),
            aggregate_footprint: EffectFootprint::symmetric(sample_radius),
        }],
        frame_local_dirty: EffectRegion::empty(),
        conservative_full: false,
    };
    snapshot
}

fn empty_resolved_scene_with_workspace_slide() -> ResolvedNativeFrameScene<'static> {
    let output_id = OutputId::from_raw(1).expect("test output id");
    let sampled_at = oblivion_one::compositor::AnimationTime::from_nanos(1);
    ResolvedNativeFrameScene {
        window_preview: None,
        surfaces: std::borrow::Cow::Borrowed(&[]),
        surface_scene_node_ids: std::borrow::Cow::Borrowed(&[]),
        presentation_owner_root_surface_ids: std::borrow::Cow::Borrowed(&[]),
        decorations: Vec::new(),
        popup_surface_ids: std::borrow::Cow::Borrowed(&[]),
        external_overlay_surface_ids: Vec::new(),
        special_workspace_dim: oblivion_one::compositor::SpecialWorkspaceDim::default(),
        wallpaper_echo: None,
        render_generation: 1,
        visibility: FullscreenRenderPlanMetrics::default(),
        snapshot: NativeSceneSnapshot::default(),
        canonical_scene_evidence: None,
        scene_identity_signature: 0,
        workspace_presentation_sample: Some(
            oblivion_one::compositor::WorkspacePresentationSample {
                transition_id: oblivion_one::compositor::WorkspacePresentationTransitionId::new(1),
                sampled_at,
                position: 0.5,
                velocity: 0.0,
                target_position: 1.0,
                target_workspace: oblivion_one::wm::WorkspaceId::new(2).expect("target workspace"),
                mathematically_settled: false,
                participants: Vec::new(),
                signature: 9,
            },
        ),
        special_workspace_presentation: None,
        effects: ResolvedEffectScene::default(),
        effect_scene_resolution_ns: None,
        effect_scene_visible_instances: 0,
        presentation: PresentationSceneSample::empty_for_output(
            output_id,
            sampled_at,
            PresentationSampleTimeSource::ZeroFallback,
        ),
        presentation_snapshot: PresentationFrameSnapshot::empty_for_output(output_id),
        lifecycle: LifecycleSceneSample {
            sampled_at,
            samples: Vec::new(),
            minimized_visual_resource_payload_ids: Vec::new(),
        },
        lifecycle_marker_insertions: Vec::new(),
        lifecycle_surfaces: Vec::new(),
        lifecycle_decorations: Vec::new(),
        lifecycle_snapshot: LifecycleFrameSnapshot::default(),
        window_exit_evidence: Vec::new(),
    }
}

#[test]
fn workspace_slide_damage_covers_the_full_output_even_without_surface_damage() {
    let resolved = empty_resolved_scene_with_workspace_slide();
    let damage = super::presentation_worker::native_scene_damage_for_resolved_scene(
        1920,
        1080,
        &NativeSceneSnapshot::default(),
        &resolved,
        NativeCursorDamageBounds::default(),
    );

    assert_eq!(damage.kind, NativeDamageKind::FullOutput);
    assert_eq!(damage.pixels, 1920 * 1080);
    assert_eq!(
        damage.rects,
        [NativeDamageRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }]
    );
}

#[test]
fn physical_pageflip_expands_clip_source_for_observing_foreign_effect() {
    let old = snapshot_with_clip(1, PresentationClip::Unbounded);
    let new = snapshot_with_clip(
        2,
        PresentationClip::Rect(
            PresentationClipRect::new(0.0, 0.0, 10.0, 10.0).expect("clip rectangle"),
        ),
    );
    let foreign_effect_corner = NativeDamageRect {
        x: 8,
        y: 8,
        width: 2,
        height: 2,
    };

    let owner_scoped = clip_damage_for_frame_snapshots(
        100,
        80,
        &old.presentation,
        &new.presentation,
        &old.scene,
        &new.scene,
    );
    assert!(native_damage_covers(
        &owner_scoped,
        NativeDamageRect {
            x: 10,
            y: 10,
            width: 10,
            height: 10,
        }
    ));
    assert!(!native_damage_covers(&owner_scoped, foreign_effect_corner));

    let mut history = NativeSceneHistory::new(old);
    history.replace_ready(new);
    assert!(history.queue_submission(20));
    let transition = history
        .prepare_pageflip_transition(20, 100, 80)
        .expect("submitted transition");

    assert!(output_damage_covers(
        &transition.damage,
        OutputRect::new(8, 8, 14, 14)
    ));
}

#[test]
fn expanded_physical_transition_enters_buffer_age_repair_history() {
    let old = snapshot_with_clip(1, PresentationClip::Unbounded);
    let new = snapshot_with_clip(
        2,
        PresentationClip::Rect(
            PresentationClipRect::new(0.0, 0.0, 10.0, 10.0).expect("clip rectangle"),
        ),
    );
    let mut history = NativeSceneHistory::new(old);
    history.replace_ready(new);
    assert!(history.queue_submission(20));
    let transition = history
        .prepare_pageflip_transition(20, 100, 80)
        .expect("submitted transition");

    let mut planner = PartialRepaintPlanner::new(
        (100, 80),
        EglPartialRepaintCapabilities {
            buffer_age: true,
            partial_render_repair: true,
            swap_buffers_with_damage: true,
        },
    );
    let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    planner.commit_presented_transition(first.render_damage);
    planner.commit_presented_transition(transition.damage.clone());

    let repair = planner.plan(
        OutputDamage::Rects(vec![OutputRect::new(1, 1, 1, 1)]),
        BufferAge::Value(2),
    );
    assert!(output_damage_covers(
        &repair.repair_damage,
        OutputRect::new(8, 8, 14, 14)
    ));
}

#[test]
fn physical_effect_expansion_keeps_old_and_new_influence_regions() {
    let output_bounds = EffectRect::new(0, 0, 100, 80).expect("output bounds");
    let old_instance = NativeEffectDamageInstanceSnapshot {
        backdrop_anchor: None,
        instance_id: EffectInstanceId::new(11).expect("old effect instance"),
        region: EffectRegion::from_rect(EffectRect::new(20, 20, 10, 10).unwrap()),
        aggregate_footprint: EffectFootprint::ZERO,
    };
    let new_instance = NativeEffectDamageInstanceSnapshot {
        backdrop_anchor: None,
        instance_id: EffectInstanceId::new(12).expect("new effect instance"),
        region: EffectRegion::from_rect(EffectRect::new(60, 20, 10, 10).unwrap()),
        aggregate_footprint: EffectFootprint::ZERO,
    };
    let previous = NativeEffectDamageFrameSnapshot {
        registry_generation: 0,
        instances: vec![old_instance],
        frame_local_dirty: EffectRegion::empty(),
        conservative_full: false,
    };
    let current = NativeEffectDamageFrameSnapshot {
        registry_generation: 0,
        instances: vec![new_instance],
        frame_local_dirty: EffectRegion::empty(),
        conservative_full: false,
    };
    let base = NativeOutputDamage::surface_damage(vec![NativeDamageRect {
        x: 20,
        y: 20,
        width: 50,
        height: 10,
    }]);
    let expanded = expand_physical_effect_damage(base, &previous, &current, output_bounds);

    assert!(native_damage_covers(
        &expanded,
        NativeDamageRect {
            x: 20,
            y: 20,
            width: 10,
            height: 10,
        }
    ));
    assert!(native_damage_covers(
        &expanded,
        NativeDamageRect {
            x: 60,
            y: 20,
            width: 10,
            height: 10,
        }
    ));
}

#[test]
fn removed_and_introduced_effects_cover_frozen_sample_and_output_footprints() {
    let output_bounds = EffectRect::new(0, 0, 100, 80).expect("output bounds");
    let footprint = EffectFootprint {
        sample_radius_x: 2,
        sample_radius_y: 2,
        output_outsets: EffectOutsets {
            top: 2,
            right: 2,
            bottom: 2,
            left: 2,
        },
        grid_alignment: 1,
    };
    let previous = NativeEffectDamageFrameSnapshot {
        registry_generation: 0,
        instances: vec![NativeEffectDamageInstanceSnapshot {
            backdrop_anchor: None,
            instance_id: EffectInstanceId::new(13).expect("removed effect instance"),
            region: EffectRegion::from_rect(EffectRect::new(18, 18, 14, 14).unwrap()),
            aggregate_footprint: footprint,
        }],
        frame_local_dirty: EffectRegion::empty(),
        conservative_full: false,
    };
    let old_output = expand_physical_effect_damage(
        NativeOutputDamage::surface_damage(vec![NativeDamageRect {
            x: 20,
            y: 20,
            width: 10,
            height: 10,
        }]),
        &previous,
        &NativeEffectDamageFrameSnapshot::default(),
        output_bounds,
    );
    assert!(native_damage_covers(
        &old_output,
        NativeDamageRect {
            x: 18,
            y: 18,
            width: 14,
            height: 14,
        }
    ));

    let current = NativeEffectDamageFrameSnapshot {
        registry_generation: 0,
        instances: vec![NativeEffectDamageInstanceSnapshot {
            backdrop_anchor: None,
            instance_id: EffectInstanceId::new(14).expect("introduced effect instance"),
            region: EffectRegion::from_rect(EffectRect::new(58, 18, 14, 14).unwrap()),
            aggregate_footprint: footprint,
        }],
        frame_local_dirty: EffectRegion::empty(),
        conservative_full: false,
    };
    let new_output = expand_physical_effect_damage(
        NativeOutputDamage::surface_damage(vec![NativeDamageRect {
            x: 60,
            y: 20,
            width: 10,
            height: 10,
        }]),
        &NativeEffectDamageFrameSnapshot::default(),
        &current,
        output_bounds,
    );
    assert!(native_damage_covers(
        &new_output,
        NativeDamageRect {
            x: 58,
            y: 18,
            width: 14,
            height: 14,
        }
    ));
}

#[test]
fn output_postprocess_and_continuous_dirty_use_frozen_recipe() {
    let output_bounds = EffectRect::new(0, 0, 100, 80).expect("output bounds");
    let postprocess = NativeEffectDamageInstanceSnapshot {
        backdrop_anchor: None,
        instance_id: EffectInstanceId::new(20).expect("postprocess instance"),
        region: EffectRegion::from_rect(EffectRect::new(38, 38, 4, 4).unwrap()),
        aggregate_footprint: EffectFootprint::symmetric(2),
    };
    let previous = NativeEffectDamageFrameSnapshot::default();
    let current = NativeEffectDamageFrameSnapshot {
        registry_generation: 0,
        instances: vec![postprocess],
        frame_local_dirty: EffectRegion::from_rect(EffectRect::new(40, 40, 1, 1).unwrap()),
        conservative_full: false,
    };
    let expanded = expand_physical_effect_damage(
        NativeOutputDamage::empty(),
        &previous,
        &current,
        output_bounds,
    );

    assert!(native_damage_covers(
        &expanded,
        NativeDamageRect {
            x: 40,
            y: 40,
            width: 1,
            height: 1,
        }
    ));
    assert!(native_damage_covers(
        &expanded,
        NativeDamageRect {
            x: 38,
            y: 38,
            width: 4,
            height: 4,
        }
    ));
}

#[test]
fn continuous_dirty_region_survives_physical_promotion_into_age_history() {
    let old = snapshot_with_clip(1, PresentationClip::Unbounded);
    let mut new = snapshot_with_clip(2, PresentationClip::Unbounded);
    new.scene.physical_effect_damage.frame_local_dirty =
        EffectRegion::from_rect(EffectRect::new(30, 30, 6, 6).unwrap());
    let mut history = NativeSceneHistory::new(old);
    history.replace_ready(new);
    assert!(history.queue_submission(20));
    let transition = history
        .prepare_pageflip_transition(20, 100, 80)
        .expect("continuous physical transition");
    assert!(output_damage_covers(
        &transition.damage,
        OutputRect::new(30, 30, 6, 6)
    ));

    let mut planner = PartialRepaintPlanner::new(
        (100, 80),
        EglPartialRepaintCapabilities {
            buffer_age: true,
            partial_render_repair: true,
            swap_buffers_with_damage: true,
        },
    );
    planner.commit_presented_transition(OutputDamage::Full);
    planner.commit_presented_transition(transition.damage);
    let repair = planner.plan(
        OutputDamage::Rects(vec![OutputRect::new(1, 1, 1, 1)]),
        BufferAge::Value(2),
    );
    assert!(output_damage_covers(
        &repair.repair_damage,
        OutputRect::new(30, 30, 6, 6)
    ));
}

#[test]
fn incomplete_recipe_uses_bounded_full_output_fallback() {
    let output_bounds = EffectRect::new(0, 0, 100, 80).expect("output bounds");
    let current = NativeEffectDamageFrameSnapshot {
        registry_generation: 0,
        instances: Vec::new(),
        frame_local_dirty: EffectRegion::empty(),
        conservative_full: true,
    };
    let base = NativeOutputDamage::surface_damage(vec![NativeDamageRect {
        x: 10,
        y: 10,
        width: 2,
        height: 2,
    }]);
    let expanded = expand_physical_effect_damage(
        base,
        &NativeEffectDamageFrameSnapshot::default(),
        &current,
        output_bounds,
    );
    assert_eq!(expanded.kind, NativeDamageKind::FullOutput);
}

#[test]
fn empty_and_full_physical_paths_remain_stable() {
    let output_bounds = EffectRect::new(0, 0, 100, 80).expect("output bounds");
    let empty = expand_physical_effect_damage(
        NativeOutputDamage::empty(),
        &NativeEffectDamageFrameSnapshot::default(),
        &NativeEffectDamageFrameSnapshot::default(),
        output_bounds,
    );
    assert_eq!(empty.kind, NativeDamageKind::Empty);

    let full = NativeOutputDamage::full_output(100, 80);
    let expanded = expand_physical_effect_damage(
        full.clone(),
        &NativeEffectDamageFrameSnapshot::default(),
        &NativeEffectDamageFrameSnapshot::default(),
        output_bounds,
    );
    assert_eq!(expanded, full);

    let mut history = NativeSceneHistory::new(snapshot_with_clip(1, PresentationClip::Unbounded));
    history.replace_ready(snapshot_with_clip(2, PresentationClip::Unbounded));
    assert!(history.queue_submission(2));
    let transition = history
        .prepare_pageflip_transition(2, 100, 80)
        .expect("empty physical transition");
    assert_eq!(transition.damage, OutputDamage::Empty);
}

#[test]
fn registry_generation_transition_for_visible_effect_is_full_output_without_source_damage() {
    let output_bounds = EffectRect::new(0, 0, 100, 80).expect("output bounds");
    let previous = NativeEffectDamageFrameSnapshot {
        registry_generation: 7,
        instances: vec![NativeEffectDamageInstanceSnapshot {
            backdrop_anchor: None,
            instance_id: EffectInstanceId::new(40).expect("effect instance"),
            region: EffectRegion::from_rect(EffectRect::new(20, 20, 10, 10).unwrap()),
            aggregate_footprint: EffectFootprint::symmetric(2),
        }],
        frame_local_dirty: EffectRegion::empty(),
        conservative_full: false,
    };
    let current = NativeEffectDamageFrameSnapshot {
        registry_generation: 8,
        ..previous.clone()
    };

    let expanded = expand_physical_effect_damage(
        NativeOutputDamage::empty(),
        &previous,
        &current,
        output_bounds,
    );

    assert_eq!(expanded.kind, NativeDamageKind::FullOutput);
}

#[test]
fn same_registry_generation_with_visible_effect_preserves_empty_physical_damage() {
    let output_bounds = EffectRect::new(0, 0, 100, 80).expect("output bounds");
    let snapshot = NativeEffectDamageFrameSnapshot {
        registry_generation: 7,
        instances: vec![NativeEffectDamageInstanceSnapshot {
            backdrop_anchor: None,
            instance_id: EffectInstanceId::new(41).expect("effect instance"),
            region: EffectRegion::from_rect(EffectRect::new(20, 20, 10, 10).unwrap()),
            aggregate_footprint: EffectFootprint::symmetric(2),
        }],
        frame_local_dirty: EffectRegion::empty(),
        conservative_full: false,
    };

    let expanded = expand_physical_effect_damage(
        NativeOutputDamage::empty(),
        &snapshot,
        &snapshot,
        output_bounds,
    );

    assert_eq!(expanded.kind, NativeDamageKind::Empty);
}

#[test]
fn registry_generation_transition_without_visible_effects_stays_empty() {
    let output_bounds = EffectRect::new(0, 0, 100, 80).expect("output bounds");
    let previous = NativeEffectDamageFrameSnapshot {
        registry_generation: 7,
        ..NativeEffectDamageFrameSnapshot::default()
    };
    let current = NativeEffectDamageFrameSnapshot {
        registry_generation: 8,
        ..NativeEffectDamageFrameSnapshot::default()
    };

    let expanded = expand_physical_effect_damage(
        NativeOutputDamage::empty(),
        &previous,
        &current,
        output_bounds,
    );

    assert_eq!(expanded.kind, NativeDamageKind::Empty);
}

#[test]
fn removed_visible_effect_across_registry_reload_is_full_output() {
    let output_bounds = EffectRect::new(0, 0, 100, 80).expect("output bounds");
    let previous = snapshot_with_effect_recipe_at_generation(
        1,
        EffectRect::new(20, 20, 10, 10).unwrap(),
        EffectRect::new(20, 20, 10, 10).unwrap(),
        0,
        7,
    )
    .scene
    .physical_effect_damage;
    let current = NativeEffectDamageFrameSnapshot {
        registry_generation: 8,
        ..NativeEffectDamageFrameSnapshot::default()
    };

    let expanded = expand_physical_effect_damage(
        NativeOutputDamage::empty(),
        &previous,
        &current,
        output_bounds,
    );

    assert_eq!(expanded.kind, NativeDamageKind::FullOutput);
}

#[test]
fn introduced_visible_effect_across_registry_reload_is_full_output() {
    let output_bounds = EffectRect::new(0, 0, 100, 80).expect("output bounds");
    let previous = NativeEffectDamageFrameSnapshot {
        registry_generation: 7,
        ..NativeEffectDamageFrameSnapshot::default()
    };
    let current = snapshot_with_effect_recipe_at_generation(
        2,
        EffectRect::new(20, 20, 10, 10).unwrap(),
        EffectRect::new(20, 20, 10, 10).unwrap(),
        0,
        8,
    )
    .scene
    .physical_effect_damage;

    let expanded = expand_physical_effect_damage(
        NativeOutputDamage::empty(),
        &previous,
        &current,
        output_bounds,
    );

    assert_eq!(expanded.kind, NativeDamageKind::FullOutput);
}

#[test]
fn delayed_old_generation_submission_survives_registry_reload() {
    let mut presented = snapshot_with_clip(1, PresentationClip::Unbounded);
    presented.scene.physical_effect_damage.registry_generation = 1;
    let mut submitted = presented.clone();
    submitted.frame_id = 2;
    let mut ready_after_reload = submitted.clone();
    ready_after_reload.frame_id = 3;
    ready_after_reload
        .scene
        .physical_effect_damage
        .registry_generation = 2;

    let mut history = NativeSceneHistory::new(presented);
    history.replace_ready(submitted);
    assert!(history.queue_submission(20));
    history.replace_ready(ready_after_reload);

    let old_generation_transition = history
        .prepare_pageflip_transition(20, 100, 80)
        .expect("old-generation submission");
    assert_eq!(old_generation_transition.previous_frame_id, Some(1));
    assert_eq!(old_generation_transition.current_frame_id, 2);
    assert_ne!(old_generation_transition.damage, OutputDamage::Full);

    assert!(history.promote_pageflip(20));
    assert!(history.queue_submission(30));
    let reload_transition = history
        .prepare_pageflip_transition(30, 100, 80)
        .expect("post-reload submission");
    assert_eq!(reload_transition.previous_frame_id, Some(2));
    assert_eq!(reload_transition.current_frame_id, 3);
    assert_eq!(reload_transition.damage, OutputDamage::Full);
}

#[test]
fn rejected_registry_generation_candidate_does_not_become_physical_authority() {
    let mut presented = snapshot_with_clip(1, PresentationClip::Unbounded);
    presented.scene.physical_effect_damage.registry_generation = 1;
    let mut rejected = presented.clone();
    rejected.frame_id = 2;
    rejected.scene.physical_effect_damage.registry_generation = 2;
    let mut current = presented.clone();
    current.frame_id = 3;

    let mut history = NativeSceneHistory::new(presented);
    history.replace_ready(rejected);
    assert!(history.queue_submission(20));
    assert!(history.discard_submission(20));
    history.replace_ready(current);
    assert!(history.queue_submission(30));

    let transition = history
        .prepare_pageflip_transition(30, 100, 80)
        .expect("current generation-1 submission");
    assert_eq!(transition.previous_frame_id, Some(1));
    assert_eq!(transition.current_frame_id, 3);
    assert_eq!(transition.damage, OutputDamage::Empty);
}

#[test]
fn registry_reload_discontinuity_enters_physical_age_two_repair_history() {
    let mut presented = snapshot_with_clip(1, PresentationClip::Unbounded);
    presented.scene.physical_effect_damage.registry_generation = 1;
    let mut reloaded = presented.clone();
    reloaded.frame_id = 2;
    reloaded.scene.physical_effect_damage.registry_generation = 2;

    let mut history = NativeSceneHistory::new(presented);
    history.replace_ready(reloaded);
    assert!(history.queue_submission(20));
    let transition = history
        .prepare_pageflip_transition(20, 100, 80)
        .expect("registry reload transition");
    assert_eq!(transition.damage, OutputDamage::Full);

    let mut planner = PartialRepaintPlanner::new(
        (100, 80),
        EglPartialRepaintCapabilities {
            buffer_age: true,
            partial_render_repair: true,
            swap_buffers_with_damage: true,
        },
    );
    let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    planner.commit_presented_transition(first.render_damage);
    planner.commit_presented_transition(transition.damage);

    let repair = planner.plan(
        OutputDamage::Rects(vec![OutputRect::new(1, 1, 1, 1)]),
        BufferAge::Value(2),
    );
    assert_eq!(repair.repair_damage, OutputDamage::Full);
}

#[test]
fn integrated_registry_reload_transition_participates_in_physical_age_three_repair() {
    let mut presented = snapshot_with_clip(1, PresentationClip::Unbounded);
    presented.scene.physical_effect_damage.registry_generation = 1;
    let mut reloaded = presented.clone();
    reloaded.frame_id = 2;
    reloaded.scene.physical_effect_damage.registry_generation = 2;
    let mut later = reloaded.clone();
    later.frame_id = 3;
    later.scene.physical_effect_damage.frame_local_dirty =
        EffectRegion::from_rect(EffectRect::new(30, 30, 6, 6).unwrap());

    let mut history = NativeSceneHistory::new(presented);
    history.replace_ready(reloaded);
    assert!(history.queue_submission(20));
    let reload_transition = history
        .prepare_pageflip_transition(20, 100, 80)
        .expect("registry reload transition");
    assert_eq!(reload_transition.damage, OutputDamage::Full);
    assert!(history.promote_pageflip(20));

    history.replace_ready(later);
    assert!(history.queue_submission(30));
    let later_transition = history
        .prepare_pageflip_transition(30, 100, 80)
        .expect("later physical transition");
    assert!(later_transition.damage != OutputDamage::Empty);

    let mut planner = PartialRepaintPlanner::new(
        (100, 80),
        EglPartialRepaintCapabilities {
            buffer_age: true,
            partial_render_repair: true,
            swap_buffers_with_damage: true,
        },
    );
    let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    planner.commit_presented_transition(first.render_damage);
    planner.commit_presented_transition(reload_transition.damage);
    planner.commit_presented_transition(later_transition.damage);

    let repair = planner.plan(
        OutputDamage::Rects(vec![OutputRect::new(1, 1, 1, 1)]),
        BufferAge::Value(3),
    );
    assert_eq!(repair.repair_damage, OutputDamage::Full);
}

#[test]
fn render_ahead_transition_uses_a_to_c_recipe_and_predecessor() {
    let old = snapshot_with_effect_recipe(
        1,
        EffectRect::new(10, 10, 10, 10).unwrap(),
        EffectRect::new(10, 10, 10, 10).unwrap(),
        0,
    );
    let b = snapshot_with_effect_recipe(
        2,
        EffectRect::new(40, 10, 10, 10).unwrap(),
        EffectRect::new(40, 10, 10, 10).unwrap(),
        2,
    );
    let c = snapshot_with_effect_recipe(
        3,
        EffectRect::new(40, 10, 10, 10).unwrap(),
        EffectRect::new(70, 10, 10, 10).unwrap(),
        0,
    );
    let mut history = NativeSceneHistory::new(old);
    history.replace_ready(b);
    assert!(history.queue_submission(20));
    history.replace_ready(c);
    assert!(history.queue_submission(30));
    assert!(history.discard_submission(20));

    let transition = history
        .prepare_pageflip_transition(30, 100, 80)
        .expect("C transition");
    assert_eq!(transition.previous_frame_id, Some(1));
    assert_eq!(transition.current_frame_id, 3);
    assert!(!output_damage_covers(
        &transition.damage,
        OutputRect::new(38, 10, 2, 10)
    ));
}

#[test]
fn delayed_pageflip_uses_submitted_recipe_over_newer_ready_recipe() {
    let old = snapshot_with_effect_recipe(
        1,
        EffectRect::new(10, 10, 10, 10).unwrap(),
        EffectRect::new(10, 10, 10, 10).unwrap(),
        0,
    );
    let submitted = snapshot_with_effect_recipe(
        2,
        EffectRect::new(40, 10, 10, 10).unwrap(),
        EffectRect::new(38, 8, 14, 14).unwrap(),
        2,
    );
    let newer_ready = snapshot_with_effect_recipe(
        3,
        EffectRect::new(40, 10, 10, 10).unwrap(),
        EffectRect::new(70, 10, 10, 10).unwrap(),
        0,
    );
    let mut history = NativeSceneHistory::new(old);
    history.replace_ready(submitted);
    assert!(history.queue_submission(20));
    history.replace_ready(newer_ready);

    let transition = history
        .prepare_pageflip_transition(20, 100, 80)
        .expect("submitted B transition");
    assert_eq!(transition.previous_frame_id, Some(1));
    assert_eq!(transition.current_frame_id, 2);
    assert!(output_damage_covers(
        &transition.damage,
        OutputRect::new(38, 10, 2, 10)
    ));
}

#[test]
fn physical_expansion_matches_renderer_plan_for_frozen_footprint() {
    let output_bounds = EffectRect::new(0, 0, 100, 80).expect("output bounds");
    let footprint = EffectFootprint::symmetric(2);
    let visible_region = EffectRegion::from_rect(EffectRect::new(8, 8, 14, 14).unwrap());
    let source_damage = EffectRegion::from_rect(EffectRect::new(10, 10, 10, 10).unwrap());
    let renderer_plan =
        plan_effect_damage(footprint, &visible_region, &source_damage, output_bounds);
    let base = NativeOutputDamage::surface_damage(vec![NativeDamageRect {
        x: 10,
        y: 10,
        width: 10,
        height: 10,
    }]);
    let snapshot = NativeEffectDamageFrameSnapshot {
        registry_generation: 0,
        instances: vec![NativeEffectDamageInstanceSnapshot {
            backdrop_anchor: None,
            instance_id: EffectInstanceId::new(30).expect("renderer parity instance"),
            region: visible_region,
            aggregate_footprint: footprint,
        }],
        frame_local_dirty: EffectRegion::empty(),
        conservative_full: false,
    };
    let physical = expand_physical_effect_damage(
        base.clone(),
        &NativeEffectDamageFrameSnapshot::default(),
        &snapshot,
        output_bounds,
    );
    let expected = base.union_effect_region(&renderer_plan.output_damage, 100, 80);
    assert_eq!(physical, expected);
}

#[test]
fn unrelated_effect_domain_stays_out_of_physical_expansion() {
    let output_bounds = EffectRect::new(0, 0, 100, 80).expect("output bounds");
    let current = NativeEffectDamageFrameSnapshot {
        registry_generation: 0,
        instances: vec![NativeEffectDamageInstanceSnapshot {
            backdrop_anchor: None,
            instance_id: EffectInstanceId::new(31).expect("unrelated effect instance"),
            region: EffectRegion::from_rect(EffectRect::new(60, 10, 10, 10).unwrap()),
            aggregate_footprint: EffectFootprint::symmetric(2),
        }],
        frame_local_dirty: EffectRegion::empty(),
        conservative_full: false,
    };
    let expanded = expand_physical_effect_damage(
        NativeOutputDamage::surface_damage(vec![NativeDamageRect {
            x: 10,
            y: 10,
            width: 10,
            height: 10,
        }]),
        &NativeEffectDamageFrameSnapshot::default(),
        &current,
        output_bounds,
    );
    assert!(!native_damage_covers(
        &expanded,
        NativeDamageRect {
            x: 58,
            y: 10,
            width: 2,
            height: 10,
        }
    ));
}

fn ordered_backdrop_frame(frame_id: u64, changed_surface: Option<u32>) -> NativeFrameSceneSnapshot {
    use oblivion_one::compositor::{
        EffectAnchor, EffectAnchorScope, EffectSceneOrder, ResolvedEffectInstance,
    };
    use oblivion_one::effects::{
        EffectFrameDemand, EffectParameterBlock, EffectRegistryGeneration,
    };
    let bounds = EffectRect::new(0, 0, 1000, 800).unwrap();
    let mut frame = snapshot_with_clip(frame_id, PresentationClip::Unbounded);
    frame.presentation = PresentationFrameSnapshot::empty_for_output(frame.output_id);
    frame.scene = NativeSceneSnapshot::default();
    for id in [10, 21, 20, 30] {
        frame.scene.surfaces.push(NativeSceneSurfaceSnapshot {
            scene_node_id: SceneNodeId::from_raw(u64::from(id)).unwrap(),
            surface_id: id,
            visual_root_surface_id: if id == 21 { 20 } else { id },
            presentation_owner_root_surface_id: if id == 21 { 20 } else { id },
            bounds: Some(NativeDamageRect {
                x: 0,
                y: 0,
                width: 1000,
                height: 800,
            }),
            damage: if changed_surface == Some(id) {
                NativeSurfaceDamageEvidence::Known(vec![NativeDamageRect {
                    x: 500,
                    y: 400,
                    width: 2,
                    height: 2,
                }])
            } else {
                NativeSurfaceDamageEvidence::AuthoritativeEmpty
            },
            content_generation: 1,
            commit_sequence: frame_id,
        });
    }
    let anchor = EffectAnchor::BeforeSurface(20);
    let effects = ResolvedEffectScene::new(
        1,
        vec![ResolvedEffectInstance {
            id: EffectInstanceId::new(1).unwrap(),
            program: oblivion_one::effects::builtin_background_blur_program_id(),
            anchor,
            region: EffectRegion::from_rect(bounds),
            target_bounds: bounds,
            parameter_block: EffectParameterBlock::default(),
            coverage: None,
            signature: 1,
            frame_demand: EffectFrameDemand::OnDamage,
            visual_group: None,
            anchor_scope: EffectAnchorScope::VisualGroup,
            backdrop_reuse: oblivion_one::compositor::EffectBackdropReuse::Exact,
            scene_order: EffectSceneOrder::for_anchor(anchor),
            composite_opacity: None,
        }],
    );
    frame.scene.effect_identity_signature = 1;
    frame.scene.effect_damage = EffectRegion::from_rect(bounds);
    frame.scene.physical_effect_damage = freeze_native_effect_damage(
        &effects,
        &EffectRegistryGeneration::with_builtin_background_blur(),
        bounds,
    );
    frame
}

fn ordered_backdrop_transition(
    old: NativeFrameSceneSnapshot,
    new: NativeFrameSceneSnapshot,
) -> OutputDamage {
    let mut history = NativeSceneHistory::new(old);
    history.replace_ready(new);
    assert!(history.queue_submission(20));
    history
        .prepare_pageflip_transition(20, 1000, 800)
        .unwrap()
        .damage
}

#[test]
fn ordered_backdrop_foreground_scroll_does_not_invalidate_stationary_blur() {
    for foreground in [20, 21, 30] {
        let damage = ordered_backdrop_transition(
            ordered_backdrop_frame(1, None),
            ordered_backdrop_frame(2, Some(foreground)),
        );
        assert_eq!(
            damage,
            OutputDamage::Rects(vec![OutputRect::new(500, 400, 2, 2)]),
            "own/group/foreground damage must not feed the backdrop: surface {foreground}"
        );
    }
}

#[test]
fn ordered_backdrop_lower_source_still_expands_through_the_frozen_footprint() {
    let old = ordered_backdrop_frame(1, None);
    let radius = old.scene.physical_effect_damage.instances[0]
        .aggregate_footprint
        .sample_radius_x;
    let damage = ordered_backdrop_transition(old, ordered_backdrop_frame(2, Some(10)));
    assert!(output_damage_covers(
        &damage,
        OutputRect::new(
            500 - radius as i32,
            400 - radius as i32,
            2 + 2 * radius,
            2 + 2 * radius
        )
    ));
}

#[test]
fn ordered_backdrop_preserves_cascading_lower_blur_influence() {
    use oblivion_one::compositor::EffectAnchorScope;
    let mut old = ordered_backdrop_frame(1, None);
    let mut upper = old.scene.physical_effect_damage.instances[0].clone();
    upper.instance_id = EffectInstanceId::new(2).unwrap();
    upper.backdrop_anchor = Some((30, EffectAnchorScope::Surface));
    old.scene.physical_effect_damage.instances.push(upper);
    let mut new = old.clone();
    new.frame_id = 2;
    new.scene.surfaces[0].damage = NativeSurfaceDamageEvidence::Known(vec![NativeDamageRect {
        x: 500,
        y: 400,
        width: 2,
        height: 2,
    }]);
    let radius = old.scene.physical_effect_damage.instances[0]
        .aggregate_footprint
        .sample_radius_x;
    let damage = ordered_backdrop_transition(old, new);
    // Two cascaded footprints around the 2x2 source, clipped to the 1000x800 output.
    let reach = 2 * radius as i32;
    let left = (500 - reach).max(0);
    let top = (400 - reach).max(0);
    let right = (502 + reach).min(1000);
    let bottom = (402 + reach).min(800);
    assert!(output_damage_covers(
        &damage,
        OutputRect::new(left, top, (right - left) as u32, (bottom - top) as u32)
    ));
}

#[test]
fn ordered_backdrop_unproven_or_changed_scene_keeps_conservative_expansion() {
    let old = ordered_backdrop_frame(1, None);
    let baseline = ordered_backdrop_frame(2, Some(30));
    let bounds = EffectRect::new(0, 0, 1000, 800).unwrap();
    let base = NativeOutputDamage::surface_damage(vec![NativeDamageRect {
        x: 500,
        y: 400,
        width: 2,
        height: 2,
    }]);
    for case in 0..7 {
        let mut new = baseline.clone();
        match case {
            0 => new.scene.surfaces.swap(0, 3),
            1 => new.scene.surfaces[1].bounds.as_mut().unwrap().x += 1,
            2 => new.scene.physical_effect_damage.instances[0].backdrop_anchor = None,
            3 => new.scene.effect_identity_signature += 1,
            4 => new.scene.physical_effect_damage.registry_generation += 1,
            5 => {
                new.scene.physical_effect_damage.frame_local_dirty = EffectRegion::from_rect(bounds)
            }
            _ => new.scene.visibility_signature += 1,
        }
        assert!(
            expand_ordered_backdrop_content_damage(&base, &old.scene, &new.scene, bounds).is_none(),
            "unproven transition {case} must retain fallback"
        );
        let damage = ordered_backdrop_transition(old.clone(), new);
        assert!(
            output_damage_covers(&damage, OutputRect::new(499, 399, 4, 4)),
            "fallback {case} must preserve the expanded region"
        );
    }
}
