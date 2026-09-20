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
        .map(|entry| (entry.root_surface_id, entry.opacity))
        .collect::<HashMap<_, _>>();
    let current_opacities = current_presentation
        .opacities
        .iter()
        .map(|entry| (entry.root_surface_id, entry.opacity))
        .collect::<HashMap<_, _>>();
    let mut changed_roots = previous_opacities
        .keys()
        .chain(current_opacities.keys())
        .copied()
        .collect::<Vec<_>>();
    changed_roots.sort_unstable();
    changed_roots.dedup();
    let changed_roots = changed_roots
        .into_iter()
        .filter(|root| previous_opacities.get(root) != current_opacities.get(root))
        .collect::<std::collections::HashSet<_>>();

    let mut rects = Vec::new();
    for surface in previous_scene
        .surfaces
        .iter()
        .filter(|surface| changed_roots.contains(&surface.presentation_owner_root_surface_id))
    {
        push_rect(&mut rects, surface.bounds, output_width, output_height);
    }
    for surface in current_scene
        .surfaces
        .iter()
        .filter(|surface| changed_roots.contains(&surface.presentation_owner_root_surface_id))
    {
        push_rect(&mut rects, surface.bounds, output_width, output_height);
    }
    for decoration in previous_scene
        .decorations
        .iter()
        .filter(|decoration| changed_roots.contains(&decoration.identity().1))
    {
        push_decoration_rect(&mut rects, decoration, output_width, output_height);
    }
    for decoration in current_scene
        .decorations
        .iter()
        .filter(|decoration| changed_roots.contains(&decoration.identity().1))
    {
        push_decoration_rect(&mut rects, decoration, output_width, output_height);
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
    let changed_owners = previous_clips
        .keys()
        .chain(current_clips.keys())
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .collect::<std::collections::HashSet<_>>()
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
            let Some(root) = root else {
                continue;
            };
            let mask = clip_record.map(|(_, rect)| rect);
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
        let output_id = OutputId::from_raw(1).expect("test output");
        let mut sample = PresentationSceneSample::empty_for_output(
            output_id,
            AnimationTime::from_nanos(1),
            PresentationSampleTimeSource::MonotonicFallback,
        );
        if let Some(rect) = clip {
            sample.clips.push(PresentationGroupClip::with_scene_node(
                SceneNodeId::from_raw(7).expect("test node"),
                7,
                PresentationClip::Rect(rect),
                Some(rect),
                None,
            ));
        }
        PresentationFrameSnapshot::from_sample_with_presented_windows(
            &sample,
            vec![PresentedWindowGeometry::with_scene_node(
                SceneNodeId::from_raw(7).expect("test node"),
                7,
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
}
