use super::*;
use crate::presentation_animation::{
    AnimationCurve, AnimationTime, PresentationClip, PresentationOpacity,
    PresentationRetainedVisualIdentity, PresentationRetainedVisualKind, PresentationRevisionId,
    PresentationTransactionId,
};
use std::collections::BTreeMap;
use std::sync::Arc;

pub(crate) const MAX_RETAINED_WINDOW_EXITS: usize = 32;

#[doc(hidden)]
pub type WindowExitSurfaceMerge<'a> = (
    std::borrow::Cow<'a, [RenderableSurface]>,
    std::borrow::Cow<'a, [SceneNodeId]>,
    std::borrow::Cow<'a, [u32]>,
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct WindowExitPainterOrder {
    pub scene_band: u8,
    pub layer_rank: u8,
    pub stack_position: u64,
}

#[doc(hidden)]
pub const fn window_exit_stack_order_key(
    scene_band: u8,
    layer_rank: u8,
    stack_position: u64,
    is_exit: bool,
    stable_order: usize,
) -> (u8, u8, u64, u8, usize) {
    (
        scene_band,
        layer_rank,
        stack_position,
        if is_exit { 0 } else { 1 },
        stable_order,
    )
}

#[derive(Debug, Clone)]
pub struct WindowExitFrozenContent {
    pub window_id: WindowId,
    pub root_surface_id: u32,
    pub scene_node_id: SceneNodeId,
    pub surfaces: Vec<RenderableSurface>,
    pub surface_scene_node_ids: Vec<SceneNodeId>,
    pub visual_root_surface_ids: Vec<u32>,
    pub presentation_owner_root_surface_ids: Vec<u32>,
    pub canonical_rect: PresentationRect,
    pub close_geometry_target: PresentationRect,
    pub close_curve: AnimationCurve,
    pub source_presented_rect: PresentationRect,
    pub source_presented_opacity: PresentationOpacity,
    pub source_presented_clip: PresentationClip,
    pub frozen_decoration: Option<DecorationRenderInstance>,
    pub effect_scene: Arc<ResolvedEffectScene>,
    pub painter_order: WindowExitPainterOrder,
    pub render_generation: u64,
    pub effect_identity_signature: u64,
}

#[derive(Debug)]
pub(crate) struct PreparedWindowExit {
    pub(crate) content: Arc<WindowExitFrozenContent>,
    pub(crate) held_release_obligations: Vec<WindowExitReleaseObligation>,
}

impl PreparedWindowExit {
    pub(crate) fn take_release_obligations(self) -> Vec<WindowExitReleaseObligation> {
        self.held_release_obligations
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WindowExitReleaseObligation {
    pub(crate) surface_id: u32,
    pub(crate) obligation: DmabufReleaseObligation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowExitPropertyRevisions {
    pub(crate) transaction_id: PresentationTransactionId,
    pub(crate) geometry_revision_id: PresentationRevisionId,
    pub(crate) opacity_revision_id: PresentationRevisionId,
}

impl WindowExitPropertyRevisions {
    pub(crate) fn is_exact_pair_for(
        self,
        identity: PresentationRetainedVisualIdentity,
        scene_node_id: SceneNodeId,
    ) -> bool {
        identity.kind() == PresentationRetainedVisualKind::WindowExit
            && identity.scene_node_id() == scene_node_id
            && self.geometry_revision_id != self.opacity_revision_id
    }
}

#[derive(Debug)]
pub(crate) struct WindowExitPayload {
    pub(crate) payload_id: u64,
    pub(crate) motion_started_at: AnimationTime,
    pub(crate) content: Arc<WindowExitFrozenContent>,
    pub(crate) property_revisions: Option<WindowExitPropertyRevisions>,
    held_release_obligations: Vec<WindowExitReleaseObligation>,
}

#[derive(Debug, Clone)]
pub struct WindowExitRenderGroup {
    pub identity: PresentationRetainedVisualIdentity,
    pub payload_id: u64,
    pub content: Arc<WindowExitFrozenContent>,
}

impl WindowExitPayload {
    pub(crate) fn new(
        identity: PresentationRetainedVisualIdentity,
        prepared: PreparedWindowExit,
        motion_started_at: AnimationTime,
    ) -> Result<Self, PreparedWindowExit> {
        if identity.kind() != PresentationRetainedVisualKind::WindowExit
            || identity.scene_node_id() != prepared.content.scene_node_id
            || !aligned_content(&prepared.content)
        {
            return Err(prepared);
        }
        Ok(Self {
            payload_id: identity.revision_id().get(),
            motion_started_at,
            content: prepared.content,
            property_revisions: None,
            held_release_obligations: prepared.held_release_obligations,
        })
    }

    pub(crate) fn held_release_obligations(&self) -> &[WindowExitReleaseObligation] {
        &self.held_release_obligations
    }

    pub(crate) fn take_held_release_obligations(&mut self) -> Vec<WindowExitReleaseObligation> {
        std::mem::take(&mut self.held_release_obligations)
    }
}

fn aligned_content(content: &WindowExitFrozenContent) -> bool {
    let len = content.surfaces.len();
    len > 0
        && content.surface_scene_node_ids.len() == len
        && content.visual_root_surface_ids.len() == len
        && content.presentation_owner_root_surface_ids.len() == len
        && content
            .surfaces
            .iter()
            .zip(content.surface_scene_node_ids.iter())
            .zip(content.visual_root_surface_ids.iter())
            .zip(content.presentation_owner_root_surface_ids.iter())
            .all(
                |(((surface, scene_node_id), visual_root_surface_id), owner_root_surface_id)| {
                    scene_node_id.get() != 0
                        && *visual_root_surface_id != 0
                        && *owner_root_surface_id == content.root_surface_id
                        && surface.surface_id != 0
                },
            )
}

#[derive(Debug, Default)]
pub(crate) struct WindowExitPayloadStore {
    entries: BTreeMap<PresentationRetainedVisualIdentity, WindowExitPayload>,
    prepared: BTreeMap<u32, PreparedWindowExit>,
}

impl WindowExitPayloadStore {
    pub(crate) fn len(&self) -> usize {
        self.entries.len() + self.prepared.len()
    }

    pub(crate) fn can_prepare_root(&self, root_surface_id: u32) -> bool {
        self.len() < MAX_RETAINED_WINDOW_EXITS
            && !self.prepared.contains_key(&root_surface_id)
            && !self
                .entries
                .values()
                .any(|payload| payload.content.root_surface_id == root_surface_id)
    }

    pub(crate) fn prepare_candidate_exact(
        &mut self,
        root_surface_id: u32,
        prepared: PreparedWindowExit,
    ) -> Result<(), PreparedWindowExit> {
        if prepared.content.root_surface_id != root_surface_id
            || !aligned_content(&prepared.content)
            || !self.can_prepare_root(root_surface_id)
        {
            return Err(prepared);
        }
        self.prepared.insert(root_surface_id, prepared);
        Ok(())
    }

    pub(crate) fn take_prepared_root(
        &mut self,
        root_surface_id: u32,
    ) -> Option<PreparedWindowExit> {
        self.prepared.remove(&root_surface_id)
    }

    pub(crate) fn publish_candidate_exact(
        &mut self,
        identity: PresentationRetainedVisualIdentity,
        payload: WindowExitPayload,
    ) -> Result<(), WindowExitPayload> {
        if identity.kind() != PresentationRetainedVisualKind::WindowExit
            || self.entries.contains_key(&identity)
            || self.len() >= MAX_RETAINED_WINDOW_EXITS
            || self.prepared.contains_key(&payload.content.root_surface_id)
            || self
                .entries
                .values()
                .any(|existing| existing.content.root_surface_id == payload.content.root_surface_id)
        {
            return Err(payload);
        }
        self.entries.insert(identity, payload);
        Ok(())
    }

    /// This is only valid before PresentationEngine activates the retained owner.
    pub(crate) fn set_property_revisions_before_activation(
        &mut self,
        identity: PresentationRetainedVisualIdentity,
        revisions: WindowExitPropertyRevisions,
    ) -> bool {
        let Some(payload) = self.entries.get_mut(&identity) else {
            return false;
        };
        if payload.property_revisions.is_some()
            || !revisions.is_exact_pair_for(identity, payload.content.scene_node_id)
        {
            return false;
        }
        payload.property_revisions = Some(revisions);
        true
    }

    pub(crate) fn get_exact(
        &self,
        identity: PresentationRetainedVisualIdentity,
    ) -> Option<&WindowExitPayload> {
        self.entries.get(&identity)
    }

    pub(crate) fn identities(
        &self,
    ) -> impl Iterator<Item = PresentationRetainedVisualIdentity> + '_ {
        self.entries.keys().copied()
    }

    pub(crate) fn active_payloads(
        &self,
    ) -> impl Iterator<Item = (PresentationRetainedVisualIdentity, &WindowExitPayload)> + '_ {
        self.entries.iter().filter_map(|(identity, payload)| {
            payload
                .property_revisions
                .is_some()
                .then_some((*identity, payload))
        })
    }

    pub(crate) fn active_render_groups(&self) -> Vec<WindowExitRenderGroup> {
        self.active_payloads()
            .map(|(identity, payload)| WindowExitRenderGroup {
                identity,
                payload_id: payload.payload_id,
                content: Arc::clone(&payload.content),
            })
            .collect()
    }

    pub(crate) fn retire_exact(
        &mut self,
        identity: PresentationRetainedVisualIdentity,
    ) -> Option<WindowExitPayload> {
        self.entries.remove(&identity)
    }

    pub(crate) fn held_release_obligations(
        &self,
    ) -> impl Iterator<Item = &DmabufReleaseObligation> {
        self.prepared
            .values()
            .flat_map(|prepared| {
                prepared
                    .held_release_obligations
                    .iter()
                    .map(|held| &held.obligation)
            })
            .chain(self.entries.values().flat_map(|payload| {
                payload
                    .held_release_obligations()
                    .iter()
                    .map(|held| &held.obligation)
            }))
    }

    pub(crate) fn drain_all(
        &mut self,
    ) -> (
        Vec<(PresentationRetainedVisualIdentity, WindowExitPayload)>,
        Vec<PreparedWindowExit>,
    ) {
        (
            std::mem::take(&mut self.entries).into_iter().collect(),
            std::mem::take(&mut self.prepared).into_values().collect(),
        )
    }
}

impl CompositorState {
    pub(in crate::compositor) fn active_window_exit_render_groups(
        &self,
    ) -> Vec<WindowExitRenderGroup> {
        self.window_exit_payloads.active_render_groups()
    }

    pub(in crate::compositor) fn window_exit_effects_for_presentation(
        &self,
        presentation: &PresentationSceneSample,
        exits: &[WindowExitRenderGroup],
    ) -> Vec<ResolvedEffectInstance> {
        let mut effects = Vec::new();
        for exit in exits {
            let content = &exit.content;
            let Some(transform) = presentation.transform_for_root(content.root_surface_id) else {
                continue;
            };
            for mut instance in content.effect_scene.instances.iter().cloned() {
                let surface_id = match instance.anchor {
                    crate::compositor::EffectAnchor::BeforeSurface(surface_id)
                    | crate::compositor::EffectAnchor::ReplaceSurface(surface_id)
                    | crate::compositor::EffectAnchor::AfterSurface(surface_id) => surface_id,
                    crate::compositor::EffectAnchor::OutputPostProcess => continue,
                };
                if !content
                    .surfaces
                    .iter()
                    .zip(content.presentation_owner_root_surface_ids.iter())
                    .any(|(surface, owner_root)| {
                        surface.surface_id == surface_id && *owner_root == content.root_surface_id
                    })
                {
                    continue;
                }
                instance.region = crate::compositor::effects::map_effect_region(
                    transform,
                    &instance.region,
                    instance.target_bounds,
                );
                if let Some(target_bounds) =
                    crate::compositor::effects::map_effect_rect(transform, instance.target_bounds)
                {
                    instance.target_bounds = target_bounds;
                }
                instance.signature =
                    instance.signature.wrapping_mul(0x0000_0100_0000_01b3) ^ transform.signature();
                effects.push(instance);
            }
        }
        effects
    }

    pub(in crate::compositor) fn merge_window_exit_surfaces<'a>(
        &self,
        canonical_surfaces: std::borrow::Cow<'a, [RenderableSurface]>,
        canonical_scene_node_ids: std::borrow::Cow<'a, [SceneNodeId]>,
        canonical_owner_roots: std::borrow::Cow<'a, [u32]>,
        exits: &[WindowExitRenderGroup],
    ) -> WindowExitSurfaceMerge<'a> {
        assert_eq!(canonical_surfaces.len(), canonical_scene_node_ids.len());
        assert_eq!(canonical_surfaces.len(), canonical_owner_roots.len());
        if exits.is_empty() {
            return (
                canonical_surfaces,
                canonical_scene_node_ids,
                canonical_owner_roots,
            );
        }

        #[derive(Debug)]
        struct OrderedGroup {
            scene_band: u8,
            layer_rank: u8,
            stack_position: u64,
            tie_rank: u8,
            stable_order: usize,
            root_surface_id: u32,
            surfaces: Vec<RenderableSurface>,
            scene_nodes: Vec<SceneNodeId>,
        }

        let mut groups = Vec::<OrderedGroup>::new();
        let mut canonical_roots = Vec::<u32>::new();
        for root_surface_id in canonical_owner_roots.iter().copied() {
            if !canonical_roots.contains(&root_surface_id) {
                canonical_roots.push(root_surface_id);
            }
        }
        for (stable_order, root_surface_id) in canonical_roots.iter().copied().enumerate() {
            let (scene_band, layer_rank, stack_position, _) =
                self.renderable_root_stack_key(root_surface_id, stable_order);
            let mut surfaces = Vec::new();
            let mut scene_nodes = Vec::new();
            for ((surface, scene_node_id), owner_root) in canonical_surfaces
                .iter()
                .zip(canonical_scene_node_ids.iter().copied())
                .zip(canonical_owner_roots.iter().copied())
            {
                if owner_root == root_surface_id {
                    surfaces.push(surface.clone());
                    scene_nodes.push(scene_node_id);
                }
            }
            groups.push(OrderedGroup {
                scene_band,
                layer_rank,
                stack_position,
                tie_rank: 1,
                stable_order,
                root_surface_id,
                surfaces,
                scene_nodes,
            });
        }
        for (stable_order, exit) in exits.iter().enumerate() {
            let content = &exit.content;
            assert_eq!(content.surfaces.len(), content.surface_scene_node_ids.len());
            assert_eq!(
                content.surfaces.len(),
                content.presentation_owner_root_surface_ids.len()
            );
            assert!(
                content
                    .presentation_owner_root_surface_ids
                    .iter()
                    .all(|owner| *owner == content.root_surface_id)
            );
            let (scene_band, layer_rank, stack_position) = (
                content.painter_order.scene_band,
                content.painter_order.layer_rank,
                content.painter_order.stack_position,
            );
            groups.push(OrderedGroup {
                scene_band,
                layer_rank,
                stack_position,
                // The removed slot precedes a canonical group that shifted
                // into the same stack position, yielding [A, B_exit, C].
                tie_rank: 0,
                stable_order,
                root_surface_id: content.root_surface_id,
                surfaces: content.surfaces.clone(),
                scene_nodes: content.surface_scene_node_ids.clone(),
            });
        }
        groups.sort_by_key(|group| {
            window_exit_stack_order_key(
                group.scene_band,
                group.layer_rank,
                group.stack_position,
                group.tie_rank == 0,
                group.stable_order,
            )
        });

        let capacity = groups.iter().map(|group| group.surfaces.len()).sum();
        let mut surfaces = Vec::with_capacity(capacity);
        let mut scene_nodes = Vec::with_capacity(capacity);
        let mut owner_roots = Vec::with_capacity(capacity);
        for group in groups {
            let count = group.surfaces.len();
            surfaces.extend(group.surfaces);
            scene_nodes.extend(group.scene_nodes);
            owner_roots.extend(std::iter::repeat_n(group.root_surface_id, count));
        }
        assert_eq!(surfaces.len(), scene_nodes.len());
        assert_eq!(surfaces.len(), owner_roots.len());
        (
            std::borrow::Cow::Owned(surfaces),
            std::borrow::Cow::Owned(scene_nodes),
            std::borrow::Cow::Owned(owner_roots),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::window_exit_stack_order_key;
    use super::{
        MAX_RETAINED_WINDOW_EXITS, PreparedWindowExit, WindowExitFrozenContent,
        WindowExitPainterOrder, WindowExitPayloadStore,
    };
    use crate::compositor::SurfacePlacement;
    use crate::compositor::{RenderableSurface, RenderableSurfaceDamage, SurfaceCommitSequence};
    use crate::core::{SceneNodeId, WindowId};
    use crate::presentation_animation::{
        AnimationCurve, EasingCurve, PresentationClip, PresentationOpacity, PresentationRect,
    };
    use crate::render_backend::buffer::{BufferIdAllocator, BufferSize, CommittedSurfaceBuffer};
    use std::sync::Arc;

    fn shm_surface(surface_id: u32) -> RenderableSurface {
        let buffer_id = BufferIdAllocator::default()
            .allocate()
            .expect("test buffer id");
        RenderableSurface {
            surface_id,
            x: 0,
            y: 0,
            width: 4,
            height: 4,
            placement: SurfacePlacement::root_at(0, 0),
            render_backend: crate::compositor::SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: CommittedSurfaceBuffer::shm_snapshot(
                buffer_id,
                BufferSize::new(4, 4).expect("test buffer size"),
                vec![0; 16],
            ),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
            damage: RenderableSurfaceDamage::Full,
        }
    }

    fn prepared_exit(root_surface_id: u32) -> PreparedWindowExit {
        let scene_node_id = SceneNodeId::from_raw(u64::from(root_surface_id) + 1000)
            .expect("test WindowGroup scene node");
        let rect = PresentationRect::new(10.0, 20.0, 300.0, 200.0).expect("test rect");
        PreparedWindowExit {
            content: Arc::new(WindowExitFrozenContent {
                window_id: WindowId::from_raw(u64::from(root_surface_id)).expect("test window id"),
                root_surface_id,
                scene_node_id,
                surfaces: vec![shm_surface(root_surface_id)],
                surface_scene_node_ids: vec![scene_node_id],
                visual_root_surface_ids: vec![root_surface_id],
                presentation_owner_root_surface_ids: vec![root_surface_id],
                canonical_rect: rect,
                close_geometry_target: rect,
                close_curve: AnimationCurve::easing(
                    std::time::Duration::from_millis(180),
                    EasingCurve::EaseInCubic,
                ),
                source_presented_rect: rect,
                source_presented_opacity: PresentationOpacity::OPAQUE,
                source_presented_clip: PresentationClip::Unbounded,
                frozen_decoration: None,
                effect_scene: Arc::new(crate::compositor::ResolvedEffectScene::default()),
                painter_order: WindowExitPainterOrder {
                    scene_band: 2,
                    layer_rank: 2,
                    stack_position: u64::from(root_surface_id),
                },
                render_generation: 1,
                effect_identity_signature: 1,
            }),
            held_release_obligations: Vec::new(),
        }
    }

    #[test]
    fn removed_window_keeps_its_old_stack_slot_between_neighbors() {
        let mut groups = [
            ("A", window_exit_stack_order_key(2, 2, 0, false, 0)),
            ("B_exit", window_exit_stack_order_key(2, 2, 1, true, 0)),
            ("C", window_exit_stack_order_key(2, 2, 1, false, 1)),
        ];
        groups.sort_by_key(|(_, key)| *key);
        assert_eq!(groups.map(|(name, _)| name), ["A", "B_exit", "C"]);
    }

    #[test]
    fn shm_content_is_retained_without_a_client_release_obligation() {
        let mut store = WindowExitPayloadStore::default();
        store
            .prepare_candidate_exact(20, prepared_exit(20))
            .expect("SHM exit candidate");

        assert_eq!(store.len(), 1);
        assert_eq!(store.held_release_obligations().count(), 0);
        assert_eq!(
            store.take_prepared_root(20).unwrap().content.surfaces.len(),
            1
        );
    }

    #[test]
    fn unactivated_exit_reservation_does_not_create_frame_work() {
        let mut state = crate::compositor::CompositorState::new(None);
        state
            .window_exit_payloads
            .prepare_candidate_exact(20, prepared_exit(20))
            .expect("reserve an unactivated exit");

        assert!(!state.presentation_animation_has_pending_visible());
        assert!(!state.has_unowned_frame_work());
    }

    #[test]
    fn retained_exit_store_enforces_unique_root_and_global_bound() {
        let mut store = WindowExitPayloadStore::default();
        for root_surface_id in 1..=MAX_RETAINED_WINDOW_EXITS as u32 {
            store
                .prepare_candidate_exact(root_surface_id, prepared_exit(root_surface_id))
                .expect("within retained Window Exit bound");
        }
        assert_eq!(store.len(), MAX_RETAINED_WINDOW_EXITS);
        assert!(store.prepare_candidate_exact(1, prepared_exit(1)).is_err());
        assert!(
            store
                .prepare_candidate_exact(100, prepared_exit(100))
                .is_err()
        );
        assert_eq!(store.len(), MAX_RETAINED_WINDOW_EXITS);
    }
}
