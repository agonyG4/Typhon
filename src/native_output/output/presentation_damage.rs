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
        AnimationTime, PresentationSampleTimeSource, PresentationSceneSample, SceneNodeId,
    };
    use oblivion_one::core::OutputId;
    use oblivion_one::presentation_animation::PresentationGroupOpacity;

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
}
