use std::collections::HashMap;

use oblivion_one::compositor::PresentationFrameSnapshot;

use super::{NativeDamageRect, NativeOutputDamage, NativeSceneSnapshot};

impl NativeSceneSnapshot {
    #[cfg(test)]
    pub(crate) fn from_surfaces(
        surfaces: &[oblivion_one::compositor::RenderableSurface],
        decorations: Vec<oblivion_one::compositor::DecorationSceneSnapshot>,
    ) -> Self {
        Self::from_surfaces_with_popup_ids(surfaces, decorations, &[])
    }

    #[cfg(test)]
    pub(crate) fn from_surfaces_with_popup_ids(
        surfaces: &[oblivion_one::compositor::RenderableSurface],
        decorations: Vec<oblivion_one::compositor::DecorationSceneSnapshot>,
        popup_surface_ids: &[u32],
    ) -> Self {
        let scene_node_ids = surfaces
            .iter()
            .enumerate()
            .map(|(index, _)| {
                oblivion_one::core::SceneNodeId::from_raw((index as u64).saturating_add(1))
                    .expect("test scene node id")
            })
            .collect::<Vec<_>>();
        Self::from_surfaces_with_scene_nodes(
            surfaces,
            &scene_node_ids,
            decorations,
            popup_surface_ids,
        )
    }

    #[allow(dead_code)]
    pub(crate) fn from_surfaces_with_scene_nodes(
        surfaces: &[oblivion_one::compositor::RenderableSurface],
        scene_node_ids: &[oblivion_one::core::SceneNodeId],
        decorations: Vec<oblivion_one::compositor::DecorationSceneSnapshot>,
        popup_surface_ids: &[u32],
    ) -> Self {
        let owner_roots =
            oblivion_one::compositor::visual_stack_groups(surfaces, popup_surface_ids)
                .into_iter()
                .flat_map(|group| {
                    group
                        .surface_indices()
                        .iter()
                        .map(|&index| (surfaces[index].surface_id, group.root_surface_id()))
                        .collect::<HashMap<_, _>>()
                })
                .collect::<HashMap<_, _>>();
        let owner_roots = surfaces
            .iter()
            .map(|surface| {
                owner_roots
                    .get(&surface.surface_id)
                    .copied()
                    .unwrap_or(surface.surface_id)
            })
            .collect::<Vec<_>>();
        Self::from_surfaces_with_scene_nodes_and_presentation_owners(
            surfaces,
            scene_node_ids,
            &owner_roots,
            decorations,
            popup_surface_ids,
        )
    }
}

/// Damage caused solely by a WindowGroup presentation opacity change.
///
/// The two frame snapshots are the physical evidence for this comparison.
/// Current compositor state is intentionally not consulted, so an older
/// submitted frame remains a valid historical source of its footprint.
pub(crate) fn opacity_damage_for_frame_snapshots(
    output_width: u32,
    output_height: u32,
    previous_presentation: &PresentationFrameSnapshot,
    current_presentation: &PresentationFrameSnapshot,
    previous_scene: &NativeSceneSnapshot,
    current_scene: &NativeSceneSnapshot,
) -> NativeOutputDamage {
    let previous_opacities = previous_presentation
        .opacities
        .iter()
        .map(|entry| (entry.scene_node_id, entry))
        .collect::<HashMap<_, _>>();
    let current_opacities = current_presentation
        .opacities
        .iter()
        .map(|entry| (entry.scene_node_id, entry))
        .collect::<HashMap<_, _>>();
    let mut changed_owners = previous_opacities
        .keys()
        .chain(current_opacities.keys())
        .copied()
        .collect::<Vec<_>>();
    changed_owners.sort_unstable();
    changed_owners.dedup();
    let changed_owners = changed_owners
        .into_iter()
        .filter(|owner| {
            previous_opacities.get(owner).map(|entry| &entry.opacity)
                != current_opacities.get(owner).map(|entry| &entry.opacity)
        })
        .collect::<Vec<_>>();

    let mut rects = Vec::new();
    for owner in changed_owners {
        for (presentation, scene) in [
            (previous_presentation, previous_scene),
            (current_presentation, current_scene),
        ] {
            let root_surface_id = presentation
                .opacities
                .iter()
                .find(|entry| entry.scene_node_id == owner)
                .map(|entry| entry.root_surface_id)
                .or_else(|| {
                    presentation
                        .presented_window_geometry_for_scene_node(owner)
                        .map(|window| window.root_surface_id())
                });
            if let Some(root_surface_id) = root_surface_id {
                for surface in scene
                    .surfaces
                    .iter()
                    .filter(|surface| surface.presentation_owner_root_surface_id == root_surface_id)
                {
                    push_rect(&mut rects, surface.bounds, output_width, output_height);
                }
                for decoration in scene
                    .decorations
                    .iter()
                    .filter(|decoration| decoration.identity().1 == root_surface_id)
                {
                    push_decoration_rect(&mut rects, decoration, output_width, output_height);
                }
            }
            for influence in scene
                .presentation_effect_influences
                .iter()
                .filter(|influence| influence.scene_node_id == owner)
            {
                push_effect_region(
                    &mut rects,
                    &influence.region,
                    None,
                    output_width,
                    output_height,
                );
            }
        }
    }
    NativeOutputDamage::surface_damage(rects)
}

/// Damage the old and new visible contribution for every WindowGroup whose
/// immutable physical Clip changed.
pub(crate) fn clip_damage_for_frame_snapshots(
    output_width: u32,
    output_height: u32,
    previous_presentation: &PresentationFrameSnapshot,
    current_presentation: &PresentationFrameSnapshot,
    previous_scene: &NativeSceneSnapshot,
    current_scene: &NativeSceneSnapshot,
) -> NativeOutputDamage {
    use oblivion_one::core::SceneNodeId;

    let previous_clips = previous_presentation
        .clips
        .iter()
        .filter_map(|entry| {
            entry.presented_clip.map(|clip| {
                (
                    entry.scene_node_id,
                    (entry.root_surface_id, presentation_clip_damage_rect(clip)),
                )
            })
        })
        .collect::<HashMap<SceneNodeId, _>>();
    let current_clips = current_presentation
        .clips
        .iter()
        .filter_map(|entry| {
            entry.presented_clip.map(|clip| {
                (
                    entry.scene_node_id,
                    (entry.root_surface_id, presentation_clip_damage_rect(clip)),
                )
            })
        })
        .collect::<HashMap<SceneNodeId, _>>();
    let mut changed_owners = previous_clips
        .keys()
        .chain(current_clips.keys())
        .copied()
        .collect::<Vec<_>>();
    changed_owners.sort_unstable();
    changed_owners.dedup();
    let changed_owners = changed_owners
        .into_iter()
        .filter(|owner| {
            previous_clips.get(owner).map(|(_, rect)| *rect)
                != current_clips.get(owner).map(|(_, rect)| *rect)
        })
        .collect::<Vec<_>>();

    let mut rects = Vec::new();
    for owner in changed_owners {
        for (presentation, scene, clips) in [
            (previous_presentation, previous_scene, &previous_clips),
            (current_presentation, current_scene, &current_clips),
        ] {
            let clip_record = clips.get(&owner).copied();
            let root = clip_record.map(|(root, _)| root).or_else(|| {
                presentation
                    .presented_window_geometry_for_scene_node(owner)
                    .map(|window| window.root_surface_id())
            });
            let mask = clip_record.map(|(_, rect)| rect);
            if let Some(root) = root {
                for surface in scene
                    .surfaces
                    .iter()
                    .filter(|surface| surface.presentation_owner_root_surface_id == root)
                {
                    push_clipped_rect(
                        &mut rects,
                        surface.bounds,
                        mask,
                        output_width,
                        output_height,
                    );
                }
                for decoration in scene
                    .decorations
                    .iter()
                    .filter(|decoration| decoration.identity().1 == root)
                {
                    let (x, y, width, height) = decoration.bounds();
                    push_clipped_rect(
                        &mut rects,
                        Some(NativeDamageRect {
                            x,
                            y,
                            width,
                            height,
                        }),
                        mask,
                        output_width,
                        output_height,
                    );
                }
            }
            for influence in scene
                .presentation_effect_influences
                .iter()
                .filter(|influence| influence.scene_node_id == owner)
            {
                push_effect_region(
                    &mut rects,
                    &influence.region,
                    mask,
                    output_width,
                    output_height,
                );
            }
        }
    }
    NativeOutputDamage::surface_damage(rects)
}

fn presentation_clip_damage_rect(
    clip: oblivion_one::presentation_animation::PresentationClipRect,
) -> NativeDamageRect {
    let left = clip
        .x()
        .floor()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    let top = clip
        .y()
        .floor()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    let right = (clip.x() + clip.width())
        .ceil()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    let bottom = (clip.y() + clip.height())
        .ceil()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    NativeDamageRect {
        x: left,
        y: top,
        width: (i64::from(right) - i64::from(left)).clamp(0, i64::from(u32::MAX)) as u32,
        height: (i64::from(bottom) - i64::from(top)).clamp(0, i64::from(u32::MAX)) as u32,
    }
}

fn push_clipped_rect(
    rects: &mut Vec<NativeDamageRect>,
    rect: Option<NativeDamageRect>,
    mask: Option<NativeDamageRect>,
    output_width: u32,
    output_height: u32,
) {
    let rect =
        rect.and_then(|rect| mask.map_or(Some(rect), |mask| intersect_damage_rect(rect, mask)));
    push_rect(rects, rect, output_width, output_height);
}

fn intersect_damage_rect(
    left: NativeDamageRect,
    right: NativeDamageRect,
) -> Option<NativeDamageRect> {
    let x = left.left().max(right.left());
    let y = left.top().max(right.top());
    let right_edge = left.right().min(right.right());
    let bottom_edge = left.bottom().min(right.bottom());
    (right_edge > x && bottom_edge > y).then_some(NativeDamageRect {
        x: x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        y: y.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        width: (right_edge - x).clamp(0, i64::from(u32::MAX)) as u32,
        height: (bottom_edge - y).clamp(0, i64::from(u32::MAX)) as u32,
    })
}

fn push_rect(
    rects: &mut Vec<NativeDamageRect>,
    rect: Option<NativeDamageRect>,
    output_width: u32,
    output_height: u32,
) {
    if let Some(rect) = rect.and_then(|rect| rect.clipped_to_output(output_width, output_height)) {
        rects.push(rect);
    }
}

fn push_effect_region(
    rects: &mut Vec<NativeDamageRect>,
    region: &oblivion_one::effects::EffectRegion,
    mask: Option<NativeDamageRect>,
    output_width: u32,
    output_height: u32,
) {
    for rect in region.rects() {
        push_clipped_rect(
            rects,
            Some(NativeDamageRect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            }),
            mask,
            output_width,
            output_height,
        );
    }
}

fn push_decoration_rect(
    rects: &mut Vec<NativeDamageRect>,
    decoration: &oblivion_one::compositor::DecorationSceneSnapshot,
    output_width: u32,
    output_height: u32,
) {
    let (x, y, width, height) = decoration.bounds();
    push_rect(
        rects,
        Some(NativeDamageRect {
            x,
            y,
            width,
            height,
        }),
        output_width,
        output_height,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use oblivion_one::compositor::{
        AnimationTime, DecorationSceneSnapshot, PresentationSampleTimeSource,
        PresentationSceneSample, PresentedWindowGeometry, SceneNodeId, WindowId,
    };
    use oblivion_one::core::OutputId;
    use oblivion_one::effects::{EffectRect, EffectRegion};
    use oblivion_one::presentation_animation::{
        PresentationClip, PresentationClipRect, PresentationGroupClip, PresentationGroupOpacity,
        PresentationRect,
    };

    fn frame_snapshot(opacities: &[(u32, f64)]) -> PresentationFrameSnapshot {
        let output_id = OutputId::from_raw(1).expect("test output");
        let mut sample = PresentationSceneSample::empty_for_output(
            output_id,
            AnimationTime::from_nanos(1),
            PresentationSampleTimeSource::MonotonicFallback,
        );
        sample.opacities = opacities
            .iter()
            .map(|(root, opacity)| {
                PresentationGroupOpacity::with_scene_node(
                    SceneNodeId::from_raw(u64::from(*root)).expect("test node"),
                    *root,
                    oblivion_one::presentation_animation::PresentationOpacity::new(*opacity)
                        .expect("valid opacity"),
                    None,
                )
            })
            .collect();
        PresentationFrameSnapshot::from_sample(&sample)
    }

    fn frame_snapshot_for_owner(
        scene_node_id: SceneNodeId,
        root_surface_id: u32,
        opacity: f64,
    ) -> PresentationFrameSnapshot {
        let output_id = OutputId::from_raw(1).expect("test output");
        let mut sample = PresentationSceneSample::empty_for_output(
            output_id,
            AnimationTime::from_nanos(1),
            PresentationSampleTimeSource::MonotonicFallback,
        );
        sample
            .opacities
            .push(PresentationGroupOpacity::with_scene_node(
                scene_node_id,
                root_surface_id,
                oblivion_one::presentation_animation::PresentationOpacity::new(opacity)
                    .expect("valid opacity"),
                None,
            ));
        PresentationFrameSnapshot::from_sample(&sample)
    }

    fn scene() -> NativeSceneSnapshot {
        NativeSceneSnapshot {
            surfaces: vec![
                super::super::NativeSceneSurfaceSnapshot {
                    scene_node_id: SceneNodeId::from_raw(7).expect("test node"),
                    surface_id: 70,
                    visual_root_surface_id: 70,
                    presentation_owner_root_surface_id: 7,
                    bounds: Some(NativeDamageRect {
                        x: 10,
                        y: 20,
                        width: 100,
                        height: 80,
                    }),
                    damage: super::super::NativeSurfaceDamageEvidence::AuthoritativeEmpty,
                    content_generation: 1,
                    commit_sequence: 1,
                },
                super::super::NativeSceneSurfaceSnapshot {
                    scene_node_id: SceneNodeId::from_raw(8).expect("test node"),
                    surface_id: 80,
                    visual_root_surface_id: 80,
                    presentation_owner_root_surface_id: 7,
                    bounds: Some(NativeDamageRect {
                        x: 100,
                        y: 40,
                        width: 60,
                        height: 50,
                    }),
                    damage: super::super::NativeSurfaceDamageEvidence::AuthoritativeEmpty,
                    content_generation: 1,
                    commit_sequence: 1,
                },
            ],
            ..NativeSceneSnapshot::default()
        }
    }

    fn clip_frame_snapshot(clip: Option<PresentationClipRect>) -> PresentationFrameSnapshot {
        clip_frame_snapshot_for_owner(SceneNodeId::from_raw(7).expect("test node"), 7, clip)
    }

    fn clip_frame_snapshot_for_owner(
        scene_node_id: SceneNodeId,
        root_surface_id: u32,
        clip: Option<PresentationClipRect>,
    ) -> PresentationFrameSnapshot {
        let output_id = OutputId::from_raw(1).expect("test output");
        let mut sample = PresentationSceneSample::empty_for_output(
            output_id,
            AnimationTime::from_nanos(1),
            PresentationSampleTimeSource::MonotonicFallback,
        );
        if let Some(rect) = clip {
            sample.clips.push(PresentationGroupClip::with_scene_node(
                scene_node_id,
                root_surface_id,
                PresentationClip::Rect(rect),
                Some(rect),
                None,
            ));
        }
        PresentationFrameSnapshot::from_sample_with_presented_windows(
            &sample,
            vec![PresentedWindowGeometry::with_scene_node(
                scene_node_id,
                root_surface_id,
                PresentationRect::new(0.0, 0.0, 400.0, 300.0).expect("window geometry"),
            )],
        )
    }

    fn clip_scene() -> NativeSceneSnapshot {
        let mut scene = NativeSceneSnapshot {
            surfaces: vec![
                super::super::NativeSceneSurfaceSnapshot {
                    scene_node_id: SceneNodeId::from_raw(7).expect("test node"),
                    surface_id: 70,
                    visual_root_surface_id: 70,
                    presentation_owner_root_surface_id: 7,
                    bounds: Some(NativeDamageRect {
                        x: 10,
                        y: 20,
                        width: 80,
                        height: 80,
                    }),
                    damage: super::super::NativeSurfaceDamageEvidence::AuthoritativeEmpty,
                    content_generation: 1,
                    commit_sequence: 1,
                },
                super::super::NativeSceneSurfaceSnapshot {
                    scene_node_id: SceneNodeId::from_raw(8).expect("test node"),
                    surface_id: 80,
                    visual_root_surface_id: 80,
                    presentation_owner_root_surface_id: 7,
                    bounds: Some(NativeDamageRect {
                        x: 100,
                        y: 40,
                        width: 60,
                        height: 50,
                    }),
                    damage: super::super::NativeSurfaceDamageEvidence::AuthoritativeEmpty,
                    content_generation: 1,
                    commit_sequence: 1,
                },
                super::super::NativeSceneSurfaceSnapshot {
                    scene_node_id: SceneNodeId::from_raw(9).expect("test node"),
                    surface_id: 90,
                    visual_root_surface_id: 90,
                    presentation_owner_root_surface_id: 9,
                    bounds: Some(NativeDamageRect {
                        x: 300,
                        y: 30,
                        width: 50,
                        height: 50,
                    }),
                    damage: super::super::NativeSurfaceDamageEvidence::AuthoritativeEmpty,
                    content_generation: 1,
                    commit_sequence: 1,
                },
            ],
            decorations: vec![DecorationSceneSnapshot::from_bounds_with_scene_node(
                SceneNodeId::from_raw(7).expect("test node"),
                WindowId::from_raw(7).expect("test window"),
                7,
                20,
                10,
                60,
                10,
                1,
            )],
            ..NativeSceneSnapshot::default()
        };
        scene
            .surfaces
            .sort_unstable_by_key(|surface| surface.scene_node_id);
        scene
    }

    fn effect_halo_scene() -> NativeSceneSnapshot {
        NativeSceneSnapshot {
            surfaces: vec![super::super::NativeSceneSurfaceSnapshot {
                scene_node_id: SceneNodeId::from_raw(7).expect("test node"),
                surface_id: 70,
                visual_root_surface_id: 70,
                presentation_owner_root_surface_id: 7,
                bounds: Some(NativeDamageRect {
                    x: 100,
                    y: 100,
                    width: 100,
                    height: 100,
                }),
                damage: super::super::NativeSurfaceDamageEvidence::AuthoritativeEmpty,
                content_generation: 1,
                commit_sequence: 1,
            }],
            presentation_effect_influences: vec![
                super::super::NativePresentationEffectInfluenceSnapshot {
                    scene_node_id: SceneNodeId::from_raw(7).expect("test node"),
                    presentation_owner_root_surface_id: 7,
                    region: EffectRegion::from_rect(
                        EffectRect::new(80, 80, 140, 140).expect("effect halo"),
                    ),
                },
            ],
            ..NativeSceneSnapshot::default()
        }
    }

    fn effect_scene_for_owner(
        scene_node_id: SceneNodeId,
        root_surface_id: u32,
        rect: NativeDamageRect,
    ) -> NativeSceneSnapshot {
        NativeSceneSnapshot {
            presentation_effect_influences: vec![
                super::super::NativePresentationEffectInfluenceSnapshot {
                    scene_node_id,
                    presentation_owner_root_surface_id: root_surface_id,
                    region: EffectRegion::from_rect(
                        EffectRect::new(rect.x, rect.y, rect.width, rect.height)
                            .expect("effect influence"),
                    ),
                },
            ],
            ..NativeSceneSnapshot::default()
        }
    }

    fn assert_damage_covers(damage: &NativeOutputDamage, expected: NativeDamageRect) {
        assert!(
            damage.rects.iter().any(|actual| {
                actual.left() <= expected.left()
                    && actual.top() <= expected.top()
                    && actual.right() >= expected.right()
                    && actual.bottom() >= expected.bottom()
            }),
            "missing damage {expected:?} in {:?}",
            damage.rects
        );
    }

    #[test]
    fn opacity_change_damages_owner_footprint_but_equal_opacity_does_not() {
        let previous = frame_snapshot(&[(7, 1.0)]);
        let current = frame_snapshot(&[(7, 0.5)]);
        let scene = scene();
        let damage =
            opacity_damage_for_frame_snapshots(400, 300, &previous, &current, &scene, &scene);
        assert_eq!(damage.rects.len(), 2);

        let equal = frame_snapshot(&[(7, 1.0)]);
        assert!(
            opacity_damage_for_frame_snapshots(400, 300, &previous, &equal, &scene, &scene)
                .is_empty()
        );
    }

    #[test]
    fn opacity_damages_effect_halo() {
        let previous = frame_snapshot(&[(7, 1.0)]);
        let current = frame_snapshot(&[(7, 0.5)]);
        let scene = effect_halo_scene();
        let damage =
            opacity_damage_for_frame_snapshots(400, 300, &previous, &current, &scene, &scene);

        for expected in [
            NativeDamageRect {
                x: 80,
                y: 100,
                width: 20,
                height: 100,
            },
            NativeDamageRect {
                x: 200,
                y: 100,
                width: 20,
                height: 100,
            },
            NativeDamageRect {
                x: 100,
                y: 80,
                width: 100,
                height: 20,
            },
            NativeDamageRect {
                x: 100,
                y: 200,
                width: 100,
                height: 20,
            },
        ] {
            assert_damage_covers(&damage, expected);
        }
    }

    #[test]
    fn unrelated_owner_is_not_damaged() {
        let previous = frame_snapshot(&[(7, 0.5), (9, 1.0)]);
        let current = frame_snapshot(&[(7, 0.5), (9, 0.25)]);
        let scene = scene();
        let damage =
            opacity_damage_for_frame_snapshots(400, 300, &previous, &current, &scene, &scene);
        assert!(damage.is_empty());
    }

    #[test]
    fn clip_change_damages_previous_and_current_visible_group_contribution() {
        let previous = clip_frame_snapshot(Some(
            PresentationClipRect::new(10.0, 10.0, 40.0, 40.0).expect("previous clip"),
        ));
        let current = clip_frame_snapshot(Some(
            PresentationClipRect::new(50.0, 10.0, 60.0, 50.0).expect("current clip"),
        ));
        let scene = clip_scene();
        let damage = clip_damage_for_frame_snapshots(400, 300, &previous, &current, &scene, &scene);

        for expected in [
            NativeDamageRect {
                x: 10,
                y: 20,
                width: 40,
                height: 30,
            },
            NativeDamageRect {
                x: 50,
                y: 20,
                width: 40,
                height: 40,
            },
            NativeDamageRect {
                x: 100,
                y: 40,
                width: 10,
                height: 20,
            },
            NativeDamageRect {
                x: 20,
                y: 10,
                width: 30,
                height: 10,
            },
            NativeDamageRect {
                x: 50,
                y: 10,
                width: 30,
                height: 10,
            },
        ] {
            assert!(
                damage.rects.iter().any(|actual| {
                    actual.left() <= expected.left()
                        && actual.top() <= expected.top()
                        && actual.right() >= expected.right()
                        && actual.bottom() >= expected.bottom()
                }),
                "missing owner damage {expected:?} in {:?}",
                damage.rects
            );
        }
        assert!(damage.rects.iter().all(|rect| rect.right() <= 160));
    }

    #[test]
    fn identity_clip_transitions_damage_hidden_and_revealed_content() {
        let identity = clip_frame_snapshot(None);
        let bounded = clip_frame_snapshot(Some(
            PresentationClipRect::new(50.0, 10.0, 20.0, 20.0).expect("bounded clip"),
        ));
        let scene = clip_scene();

        let hidden = clip_damage_for_frame_snapshots(400, 300, &identity, &bounded, &scene, &scene);
        assert!(hidden.rects.iter().any(|rect| {
            rect.left() <= 10 && rect.top() <= 20 && rect.right() >= 90 && rect.bottom() >= 100
        }));

        let revealed =
            clip_damage_for_frame_snapshots(400, 300, &bounded, &identity, &scene, &scene);
        assert!(revealed.rects.iter().any(|rect| {
            rect.left() <= 100 && rect.top() <= 40 && rect.right() >= 160 && rect.bottom() >= 90
        }));
        assert!(
            clip_damage_for_frame_snapshots(400, 300, &identity, &identity, &scene, &scene)
                .is_empty()
        );
    }

    #[test]
    fn clip_hides_old_effect_halo() {
        let unbounded = clip_frame_snapshot(None);
        let clipped = clip_frame_snapshot(Some(
            PresentationClipRect::new(100.0, 100.0, 100.0, 100.0).expect("surface clip"),
        ));
        let scene = effect_halo_scene();
        let damage =
            clip_damage_for_frame_snapshots(400, 300, &unbounded, &clipped, &scene, &scene);

        for expected in [
            NativeDamageRect {
                x: 80,
                y: 100,
                width: 20,
                height: 100,
            },
            NativeDamageRect {
                x: 200,
                y: 100,
                width: 20,
                height: 100,
            },
            NativeDamageRect {
                x: 100,
                y: 80,
                width: 100,
                height: 20,
            },
            NativeDamageRect {
                x: 100,
                y: 200,
                width: 100,
                height: 20,
            },
        ] {
            assert_damage_covers(&damage, expected);
        }
    }

    #[test]
    fn clip_reveals_new_effect_halo() {
        let unbounded = clip_frame_snapshot(None);
        let clipped = clip_frame_snapshot(Some(
            PresentationClipRect::new(100.0, 100.0, 100.0, 100.0).expect("surface clip"),
        ));
        let scene = effect_halo_scene();
        let damage =
            clip_damage_for_frame_snapshots(400, 300, &clipped, &unbounded, &scene, &scene);

        for expected in [
            NativeDamageRect {
                x: 80,
                y: 100,
                width: 20,
                height: 100,
            },
            NativeDamageRect {
                x: 200,
                y: 100,
                width: 20,
                height: 100,
            },
            NativeDamageRect {
                x: 100,
                y: 80,
                width: 100,
                height: 20,
            },
            NativeDamageRect {
                x: 100,
                y: 200,
                width: 100,
                height: 20,
            },
        ] {
            assert_damage_covers(&damage, expected);
        }
    }

    #[test]
    fn clip_rect_change_damages_only_old_and_new_effect_intersections() {
        let owner = SceneNodeId::from_raw(7).expect("test node");
        let previous = clip_frame_snapshot(Some(
            PresentationClipRect::new(90.0, 95.0, 20.0, 15.0).expect("previous clip"),
        ));
        let current = clip_frame_snapshot(Some(
            PresentationClipRect::new(180.0, 180.0, 30.0, 25.0).expect("current clip"),
        ));
        let scene = effect_scene_for_owner(
            owner,
            7,
            NativeDamageRect {
                x: 80,
                y: 80,
                width: 140,
                height: 140,
            },
        );
        let damage = clip_damage_for_frame_snapshots(400, 300, &previous, &current, &scene, &scene);
        let expected = [
            NativeDamageRect {
                x: 90,
                y: 95,
                width: 20,
                height: 15,
            },
            NativeDamageRect {
                x: 180,
                y: 180,
                width: 30,
                height: 25,
            },
        ];

        for rect in expected {
            assert_damage_covers(&damage, rect);
        }
        assert!(damage.rects.iter().all(|actual| {
            expected.iter().any(|allowed| {
                actual.left() >= allowed.left()
                    && actual.top() >= allowed.top()
                    && actual.right() <= allowed.right()
                    && actual.bottom() <= allowed.bottom()
            })
        }));
    }

    #[test]
    fn zero_area_clip_damages_only_the_previous_effect_contribution() {
        let previous = clip_frame_snapshot(Some(
            PresentationClipRect::new(120.0, 130.0, 30.0, 20.0).expect("previous clip"),
        ));
        let empty = clip_frame_snapshot(Some(
            PresentationClipRect::new(250.0, 250.0, 0.0, 30.0).expect("zero-area clip"),
        ));
        let scene = effect_halo_scene();

        let damage = clip_damage_for_frame_snapshots(400, 300, &previous, &empty, &scene, &scene);

        assert_damage_covers(
            &damage,
            NativeDamageRect {
                x: 120,
                y: 130,
                width: 30,
                height: 20,
            },
        );
        assert!(damage.rects.iter().all(|rect| {
            rect.left() >= 120 && rect.top() >= 130 && rect.right() <= 150 && rect.bottom() <= 150
        }));
    }

    #[test]
    fn opacity_effect_damage_uses_stable_owner_across_root_replacement() {
        let owner = SceneNodeId::from_raw(77).expect("WindowGroup node");
        let previous = frame_snapshot_for_owner(owner, 70, 1.0);
        let current = frame_snapshot_for_owner(owner, 80, 0.5);
        let old_scene = effect_scene_for_owner(
            owner,
            70,
            NativeDamageRect {
                x: 20,
                y: 30,
                width: 40,
                height: 50,
            },
        );
        let new_scene = effect_scene_for_owner(
            owner,
            80,
            NativeDamageRect {
                x: 210,
                y: 110,
                width: 60,
                height: 70,
            },
        );

        let damage = opacity_damage_for_frame_snapshots(
            400, 300, &previous, &current, &old_scene, &new_scene,
        );

        assert_damage_covers(
            &damage,
            NativeDamageRect {
                x: 20,
                y: 30,
                width: 40,
                height: 50,
            },
        );
        assert_damage_covers(
            &damage,
            NativeDamageRect {
                x: 210,
                y: 110,
                width: 60,
                height: 70,
            },
        );
        assert_eq!(
            old_scene.presentation_effect_influences[0].presentation_owner_root_surface_id,
            70
        );
        assert_eq!(
            new_scene.presentation_effect_influences[0].presentation_owner_root_surface_id,
            80
        );
    }

    #[test]
    fn clip_effect_damage_uses_stable_owner_across_root_replacement() {
        let owner = SceneNodeId::from_raw(77).expect("WindowGroup node");
        let previous = clip_frame_snapshot_for_owner(
            owner,
            70,
            Some(PresentationClipRect::new(20.0, 30.0, 40.0, 50.0).expect("old clip")),
        );
        let current = clip_frame_snapshot_for_owner(
            owner,
            80,
            Some(PresentationClipRect::new(210.0, 110.0, 60.0, 70.0).expect("new clip")),
        );
        let old_scene = effect_scene_for_owner(
            owner,
            70,
            NativeDamageRect {
                x: 10,
                y: 20,
                width: 80,
                height: 80,
            },
        );
        let new_scene = effect_scene_for_owner(
            owner,
            80,
            NativeDamageRect {
                x: 200,
                y: 100,
                width: 80,
                height: 90,
            },
        );

        let damage =
            clip_damage_for_frame_snapshots(400, 300, &previous, &current, &old_scene, &new_scene);

        assert_damage_covers(
            &damage,
            NativeDamageRect {
                x: 20,
                y: 30,
                width: 40,
                height: 50,
            },
        );
        assert_damage_covers(
            &damage,
            NativeDamageRect {
                x: 210,
                y: 110,
                width: 60,
                height: 70,
            },
        );
        assert_eq!(
            old_scene.presentation_effect_influences[0].scene_node_id,
            owner
        );
        assert_eq!(
            new_scene.presentation_effect_influences[0].scene_node_id,
            owner
        );
    }

    #[test]
    fn presentation_effect_damage_does_not_include_unrelated_owners() {
        let previous_opacity = frame_snapshot(&[(7, 1.0)]);
        let current_opacity = frame_snapshot(&[(7, 0.5)]);
        let previous_clip = clip_frame_snapshot(None);
        let current_clip = clip_frame_snapshot(Some(
            PresentationClipRect::new(100.0, 100.0, 100.0, 100.0).expect("owner clip"),
        ));
        let mut scene = effect_halo_scene();
        scene.presentation_effect_influences.push(
            super::super::NativePresentationEffectInfluenceSnapshot {
                scene_node_id: SceneNodeId::from_raw(9).expect("unrelated owner"),
                presentation_owner_root_surface_id: 9,
                region: EffectRegion::from_rect(
                    EffectRect::new(300, 20, 60, 60).expect("unrelated effect"),
                ),
            },
        );

        let opacity_damage = opacity_damage_for_frame_snapshots(
            400,
            300,
            &previous_opacity,
            &current_opacity,
            &scene,
            &scene,
        );
        let clip_damage = clip_damage_for_frame_snapshots(
            400,
            300,
            &previous_clip,
            &current_clip,
            &scene,
            &scene,
        );

        assert!(opacity_damage.rects.iter().all(|rect| rect.right() <= 220));
        assert!(clip_damage.rects.iter().all(|rect| rect.right() <= 220));
    }
}
