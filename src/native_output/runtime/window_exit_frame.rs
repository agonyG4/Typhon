use super::*;
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
};

use oblivion_one::compositor::{
    DecorationRenderInstance, NativeFramePresentationTargets, PresentationSceneSample,
    PresentationWindowTarget, SceneNodeId, SurfacePresentationKey, WindowExitRenderGroup,
};
use oblivion_one::presentation_animation::AnimationTime;

pub(super) struct WindowExitFrameParts<'a> {
    pub(super) exits: Vec<WindowExitRenderGroup>,
    pub(super) surfaces: Cow<'a, [RenderableSurface]>,
    pub(super) scene_nodes: Cow<'a, [SceneNodeId]>,
    pub(super) owner_roots: Cow<'a, [u32]>,
    pub(super) presented_surfaces: Cow<'a, [RenderableSurface]>,
    pub(super) decorations: Vec<DecorationRenderInstance>,
    pub(super) targets: NativeFramePresentationTargets,
    pub(super) presentation: PresentationSceneSample,
    pub(super) canonical_surface_ids: HashSet<u32>,
    pub(super) canonical_key_by_surface: HashMap<u32, Option<SurfacePresentationKey>>,
}

pub(super) fn resolve_window_exit_frame_parts<'a>(
    server: &OwnCompositorServer,
    canonical_surfaces: &Cow<'a, [RenderableSurface]>,
    canonical_scene_nodes: &Cow<'a, [SceneNodeId]>,
    canonical_owner_roots: &Cow<'a, [u32]>,
    at: AnimationTime,
    sample_time_source: oblivion_one::compositor::PresentationSampleTimeSource,
) -> WindowExitFrameParts<'a> {
    let canonical_presentation_keys = canonical_surfaces
        .iter()
        .map(|surface| server.surface_presentation_key_for_surface(surface.surface_id))
        .collect::<Vec<Option<SurfacePresentationKey>>>();
    let exits = server.active_window_exit_render_groups();
    let (surfaces, scene_nodes, owner_roots) = server.merge_window_exit_surfaces(
        canonical_surfaces.clone(),
        canonical_scene_nodes.clone(),
        canonical_owner_roots.clone(),
        &exits,
    );
    let mut target_windows = server
        .native_frame_presentation_targets(canonical_surfaces.as_ref())
        .windows()
        .to_vec();
    for exit in &exits {
        target_windows.push(
            PresentationWindowTarget::with_scene_node(
                exit.content.scene_node_id,
                exit.content.root_surface_id,
                exit.content.canonical_rect,
            )
            .with_canonical_opacity(exit.content.source_presented_opacity)
            .with_canonical_clip(exit.content.source_presented_clip),
        );
    }
    let targets = NativeFramePresentationTargets::from_windows(target_windows);
    let presentation = server.presentation_scene_sample_for_targets_at_with_source(
        at,
        sample_time_source,
        &targets,
    );
    let mut decorations = server
        .native_decoration_render_instances_for_scale(canonical_surfaces.as_ref(), 1.0)
        .into_iter()
        .map(|decoration| {
            presentation
                .transform_for_root(decoration.root_surface_id())
                .and_then(|transform| decoration.with_presentation_transform(transform))
                .unwrap_or(decoration)
        })
        .collect::<Vec<_>>();
    for exit in &exits {
        if let Some(decoration) = exit.content.frozen_decoration.as_ref() {
            decorations.push(
                presentation
                    .transform_for_root(exit.content.root_surface_id)
                    .and_then(|transform| decoration.with_presentation_transform(transform))
                    .unwrap_or_else(|| decoration.clone()),
            );
        }
    }
    let mut decoration_order = HashMap::new();
    let mut canonical_decoration_roots = Vec::new();
    for root_surface_id in canonical_owner_roots.iter().copied() {
        if !canonical_decoration_roots.contains(&root_surface_id) {
            canonical_decoration_roots.push(root_surface_id);
        }
    }
    for (stable_order, root_surface_id) in canonical_decoration_roots.iter().copied().enumerate() {
        let (scene_band, layer_rank, stack_position, _) =
            server.native_scene_root_stack_key(root_surface_id, stable_order);
        decoration_order.insert(
            root_surface_id,
            oblivion_one::compositor::window_exit_stack_order_key(
                scene_band,
                layer_rank,
                stack_position,
                false,
                stable_order,
            ),
        );
    }
    for (stable_order, exit) in exits.iter().enumerate() {
        let painter = exit.content.painter_order;
        decoration_order.insert(
            exit.content.root_surface_id,
            oblivion_one::compositor::window_exit_stack_order_key(
                painter.scene_band,
                painter.layer_rank,
                painter.stack_position,
                true,
                stable_order,
            ),
        );
    }
    decorations.sort_by_key(|decoration| {
        decoration_order
            .get(&decoration.root_surface_id())
            .copied()
            .unwrap_or((u8::MAX, u8::MAX, u64::MAX, u8::MAX, usize::MAX))
    });
    let presented_surfaces = server.apply_presentation_to_native_frame_surfaces(
        surfaces.clone(),
        owner_roots.as_ref(),
        &presentation,
    );
    let canonical_surface_ids = canonical_surfaces
        .iter()
        .map(|surface| surface.surface_id)
        .collect::<HashSet<_>>();
    let canonical_key_by_surface = canonical_surfaces
        .iter()
        .zip(canonical_presentation_keys)
        .map(|(surface, key)| (surface.surface_id, key))
        .collect();

    WindowExitFrameParts {
        exits,
        surfaces,
        scene_nodes,
        owner_roots,
        presented_surfaces,
        decorations,
        targets,
        presentation,
        canonical_surface_ids,
        canonical_key_by_surface,
    }
}
