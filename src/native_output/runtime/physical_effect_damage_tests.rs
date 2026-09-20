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
    PresentationFrameSnapshot, PresentationRect, PresentationSampleTimeSource,
    PresentationSceneSample, PresentedWindowGeometry,
};
use oblivion_one::core::SceneNodeId;
use oblivion_one::effects::{
    EffectFootprint, EffectInstanceId, EffectOutsets, EffectRect, EffectRegion, plan_effect_damage,
};
use oblivion_one::presentation_animation::{
    PresentationClip, PresentationClipRect, PresentationGroupClip,
};
use oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot;

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
        instances: vec![NativeEffectDamageInstanceSnapshot {
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
    }
}

fn snapshot_with_effect_recipe(
    frame_id: u64,
    source_rect: EffectRect,
    effect_rect: EffectRect,
    sample_radius: u32,
) -> NativeFrameSceneSnapshot {
    let mut snapshot = snapshot_with_clip(frame_id, PresentationClip::Unbounded);
    snapshot.scene.effect_damage = EffectRegion::from_rect(source_rect);
    snapshot.scene.effect_identity_signature = frame_id;
    snapshot.scene.physical_effect_damage = NativeEffectDamageFrameSnapshot {
        instances: vec![NativeEffectDamageInstanceSnapshot {
            instance_id: EffectInstanceId::new(frame_id).expect("effect instance"),
            region: EffectRegion::from_rect(effect_rect),
            aggregate_footprint: EffectFootprint::symmetric(sample_radius),
        }],
        frame_local_dirty: EffectRegion::empty(),
        conservative_full: false,
    };
    snapshot
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
        instance_id: EffectInstanceId::new(11).expect("old effect instance"),
        region: EffectRegion::from_rect(EffectRect::new(20, 20, 10, 10).unwrap()),
        aggregate_footprint: EffectFootprint::ZERO,
    };
    let new_instance = NativeEffectDamageInstanceSnapshot {
        instance_id: EffectInstanceId::new(12).expect("new effect instance"),
        region: EffectRegion::from_rect(EffectRect::new(60, 20, 10, 10).unwrap()),
        aggregate_footprint: EffectFootprint::ZERO,
    };
    let previous = NativeEffectDamageFrameSnapshot {
        instances: vec![old_instance],
        frame_local_dirty: EffectRegion::empty(),
        conservative_full: false,
    };
    let current = NativeEffectDamageFrameSnapshot {
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
    };
    let previous = NativeEffectDamageFrameSnapshot {
        instances: vec![NativeEffectDamageInstanceSnapshot {
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
        instances: vec![NativeEffectDamageInstanceSnapshot {
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
        instance_id: EffectInstanceId::new(20).expect("postprocess instance"),
        region: EffectRegion::from_rect(EffectRect::new(38, 38, 4, 4).unwrap()),
        aggregate_footprint: EffectFootprint::symmetric(2),
    };
    let previous = NativeEffectDamageFrameSnapshot::default();
    let current = NativeEffectDamageFrameSnapshot {
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
        instances: vec![NativeEffectDamageInstanceSnapshot {
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
        instances: vec![NativeEffectDamageInstanceSnapshot {
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
