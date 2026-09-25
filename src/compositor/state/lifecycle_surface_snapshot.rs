use crate::compositor::{
    RenderableSurface, SurfacePlacement, SurfacePresentationKey, SurfaceVisualAperture,
};
use crate::render_backend::buffer::BufferSize;
use std::collections::{HashMap, HashSet};

/// Presentation-only topology captured for one immutable lifecycle payload.
/// Surface buffers and commit state remain owned by the live compositor.
#[derive(Debug, Clone)]
pub(crate) struct RetainedSurfacePresentationSnapshot {
    pub(crate) root_key: SurfacePresentationKey,
    pub(crate) nodes: Vec<RetainedSurfacePresentationNode>,
}

#[derive(Debug, Clone)]
pub(crate) struct RetainedSurfacePresentationNode {
    pub(crate) key: SurfacePresentationKey,
    pub(crate) parent_key: Option<SurfacePresentationKey>,
    pub(crate) render_parent_key: Option<SurfacePresentationKey>,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) placement: SurfacePlacement,
    pub(crate) render_placement: Option<SurfacePlacement>,
    pub(crate) visual_clip: Option<SurfaceVisualAperture>,
    pub(crate) render_target_size: Option<BufferSize>,
}

impl RetainedSurfacePresentationSnapshot {
    /// Captures the root's usable retained tree in painter order. Missing
    /// presentation generations are never substituted with render generations.
    pub(crate) fn capture(
        root_surface_id: u32,
        surfaces: &[RenderableSurface],
        presentation_generations: &HashMap<u32, u64>,
    ) -> Option<Self> {
        let mut by_surface_id = HashMap::with_capacity(surfaces.len());
        for (index, surface) in surfaces.iter().enumerate() {
            if by_surface_id.insert(surface.surface_id, index).is_some() {
                return None;
            }
        }

        let root_index = *by_surface_id.get(&root_surface_id)?;
        let root_surface = &surfaces[root_index];
        if root_surface.placement.parent_surface_id.is_some()
            || root_surface
                .render_placement
                .is_some_and(|placement| placement.parent_surface_id.is_some())
        {
            return None;
        }
        let root_generation = *presentation_generations.get(&root_surface_id)?;
        let root_key = SurfacePresentationKey {
            surface_id: root_surface_id,
            generation: root_generation,
        };

        fn reaches_root(
            surface_id: u32,
            root_surface_id: u32,
            surfaces: &[RenderableSurface],
            by_surface_id: &HashMap<u32, usize>,
            visiting: &mut HashSet<u32>,
            memo: &mut HashMap<u32, bool>,
        ) -> bool {
            if surface_id == root_surface_id {
                return true;
            }
            if let Some(result) = memo.get(&surface_id) {
                return *result;
            }
            if !visiting.insert(surface_id) {
                return false;
            }
            let result = by_surface_id
                .get(&surface_id)
                .and_then(|index| surfaces.get(*index))
                .and_then(|surface| surface.placement.parent_surface_id)
                .is_some_and(|parent_id| {
                    by_surface_id.contains_key(&parent_id)
                        && reaches_root(
                            parent_id,
                            root_surface_id,
                            surfaces,
                            by_surface_id,
                            visiting,
                            memo,
                        )
                });
            visiting.remove(&surface_id);
            memo.insert(surface_id, result);
            result
        }

        let mut ancestry = HashMap::from([(root_surface_id, true)]);
        for surface in surfaces {
            if !reaches_root(
                surface.surface_id,
                root_surface_id,
                surfaces,
                &by_surface_id,
                &mut HashSet::new(),
                &mut ancestry,
            ) {
                continue;
            }
            if !presentation_generations.contains_key(&surface.surface_id) {
                return None;
            }
        }

        let keys_by_id = by_surface_id
            .keys()
            .filter_map(|surface_id| {
                presentation_generations
                    .get(surface_id)
                    .copied()
                    .map(|generation| {
                        (
                            *surface_id,
                            SurfacePresentationKey {
                                surface_id: *surface_id,
                                generation,
                            },
                        )
                    })
            })
            .collect::<HashMap<_, _>>();

        let mut nodes_by_key = HashMap::new();
        let mut invalid_render_parent = HashSet::new();
        for surface in surfaces {
            if !ancestry.get(&surface.surface_id).copied().unwrap_or(false) {
                continue;
            }
            let key = *keys_by_id.get(&surface.surface_id)?;
            let parent_key = surface
                .placement
                .parent_surface_id
                .and_then(|parent_id| keys_by_id.get(&parent_id).copied());
            if surface.placement.parent_surface_id.is_some() && parent_key.is_none() {
                return None;
            }
            let render_parent_id = surface
                .render_placement
                .and_then(|placement| placement.parent_surface_id);
            let render_parent_key =
                render_parent_id.and_then(|parent_id| keys_by_id.get(&parent_id).copied());
            if render_parent_id.is_some() && render_parent_key.is_none() {
                invalid_render_parent.insert(key);
            }
            nodes_by_key.insert(
                key,
                RetainedSurfacePresentationNode {
                    key,
                    parent_key,
                    render_parent_key,
                    x: surface.x,
                    y: surface.y,
                    width: surface.width,
                    height: surface.height,
                    placement: surface.placement,
                    render_placement: surface.render_placement,
                    visual_clip: surface.visual_clip.clone(),
                    render_target_size: surface.render_target_size,
                },
            );
        }

        fn has_valid_captured_ancestry(
            key: SurfacePresentationKey,
            root_key: SurfacePresentationKey,
            nodes: &HashMap<SurfacePresentationKey, RetainedSurfacePresentationNode>,
            invalid_render_parent: &HashSet<SurfacePresentationKey>,
            visiting: &mut HashSet<SurfacePresentationKey>,
            memo: &mut HashMap<SurfacePresentationKey, bool>,
        ) -> bool {
            if key == root_key {
                return nodes.contains_key(&root_key) && !invalid_render_parent.contains(&key);
            }
            if let Some(result) = memo.get(&key) {
                return *result;
            }
            if invalid_render_parent.contains(&key) || !visiting.insert(key) {
                return false;
            }
            let result = nodes.get(&key).is_some_and(|node| {
                node.parent_key.is_some_and(|parent_key| {
                    has_valid_captured_ancestry(
                        parent_key,
                        root_key,
                        nodes,
                        invalid_render_parent,
                        visiting,
                        memo,
                    )
                }) && node.render_parent_key.is_none_or(|parent_key| {
                    has_valid_captured_ancestry(
                        parent_key,
                        root_key,
                        nodes,
                        invalid_render_parent,
                        visiting,
                        memo,
                    )
                })
            });
            visiting.remove(&key);
            memo.insert(key, result);
            result
        }

        if !nodes_by_key.contains_key(&root_key) {
            return None;
        }
        let mut valid_ancestry = HashMap::new();
        let nodes = surfaces
            .iter()
            .filter_map(|surface| {
                let key = *keys_by_id.get(&surface.surface_id)?;
                if has_valid_captured_ancestry(
                    key,
                    root_key,
                    &nodes_by_key,
                    &invalid_render_parent,
                    &mut HashSet::new(),
                    &mut valid_ancestry,
                ) {
                    nodes_by_key.get(&key).cloned()
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        Some(Self { root_key, nodes })
    }

    /// Projects current content through captured topology and captured order.
    /// A destroyed or reincarnated ancestor makes its captured descendants
    /// ineligible; no old buffer is retained to fill the gap.
    pub(crate) fn project(
        &self,
        live_surfaces: &[RenderableSurface],
        presentation_generations: &HashMap<u32, u64>,
    ) -> Vec<RenderableSurface> {
        let live_by_key = live_surfaces
            .iter()
            .filter_map(|surface| {
                presentation_generations
                    .get(&surface.surface_id)
                    .copied()
                    .map(|generation| {
                        (
                            SurfacePresentationKey {
                                surface_id: surface.surface_id,
                                generation,
                            },
                            surface,
                        )
                    })
            })
            .collect::<HashMap<_, _>>();
        let nodes_by_key = self
            .nodes
            .iter()
            .map(|node| (node.key, node))
            .collect::<HashMap<_, _>>();

        fn live_ancestry_available(
            key: SurfacePresentationKey,
            root_key: SurfacePresentationKey,
            live_by_key: &HashMap<SurfacePresentationKey, &RenderableSurface>,
            nodes: &HashMap<SurfacePresentationKey, &RetainedSurfacePresentationNode>,
            visiting: &mut HashSet<SurfacePresentationKey>,
            memo: &mut HashMap<SurfacePresentationKey, bool>,
        ) -> bool {
            if let Some(result) = memo.get(&key) {
                return *result;
            }
            if !live_by_key.contains_key(&key) || !visiting.insert(key) {
                return false;
            }
            let result = if key == root_key {
                true
            } else {
                nodes.get(&key).is_some_and(|node| {
                    node.parent_key.is_some_and(|parent_key| {
                        live_ancestry_available(
                            parent_key,
                            root_key,
                            live_by_key,
                            nodes,
                            visiting,
                            memo,
                        )
                    }) && node.render_parent_key.is_none_or(|parent_key| {
                        live_ancestry_available(
                            parent_key,
                            root_key,
                            live_by_key,
                            nodes,
                            visiting,
                            memo,
                        )
                    })
                })
            };
            visiting.remove(&key);
            memo.insert(key, result);
            result
        }

        let mut available = HashMap::new();
        self.nodes
            .iter()
            .filter_map(|node| {
                live_ancestry_available(
                    node.key,
                    self.root_key,
                    &live_by_key,
                    &nodes_by_key,
                    &mut HashSet::new(),
                    &mut available,
                )
                .then(|| {
                    let mut projected = (*live_by_key.get(&node.key)?).clone();
                    projected.x = node.x;
                    projected.y = node.y;
                    projected.width = node.width;
                    projected.height = node.height;
                    projected.placement = node.placement;
                    projected.render_placement = node.render_placement;
                    projected.visual_clip = node.visual_clip.clone();
                    projected.render_target_size = node.render_target_size;
                    Some(projected)
                })
                .flatten()
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn test_root(surface_id: u32) -> Self {
        let root_key = SurfacePresentationKey {
            surface_id,
            generation: 1,
        };
        Self {
            root_key,
            nodes: vec![RetainedSurfacePresentationNode {
                key: root_key,
                parent_key: None,
                render_parent_key: None,
                x: 0,
                y: 0,
                width: 1,
                height: 1,
                placement: SurfacePlacement::root(),
                render_placement: None,
                visual_clip: None,
                render_target_size: None,
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::{
        RenderableSurfaceDamage, SurfaceCommitSequence, SurfaceRenderBackend, SurfaceTargetRect,
        ViewportSourceRect,
    };
    use crate::render_backend::buffer::{BufferIdAllocator, CommittedSurfaceBuffer};
    use wayland_server::protocol::wl_output;

    fn test_surface(surface_id: u32, placement: SurfacePlacement) -> RenderableSurface {
        let size = BufferSize::new(100, 100).expect("test buffer size");
        let identity = BufferIdAllocator::default()
            .allocate()
            .expect("test buffer identity");
        RenderableSurface {
            surface_id,
            x: 0,
            y: 0,
            width: size.width,
            height: size.height,
            placement,
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: CommittedSurfaceBuffer::shm_snapshot(
                identity,
                size,
                vec![0xff00_0000; 100 * 100],
            ),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: RenderableSurfaceDamage::Full,
        }
    }

    #[test]
    fn projection_freezes_topology_and_keeps_live_content_mapping() {
        let root = test_surface(42, SurfacePlacement::root());
        let mut captured_child = test_surface(43, SurfacePlacement::subsurface(42, 16, 24));
        captured_child.x = 16;
        captured_child.y = 24;
        captured_child.width = 64;
        captured_child.height = 64;
        captured_child.render_placement = Some(SurfacePlacement::subsurface(42, 12, 20));
        captured_child.visual_clip = Some(SurfaceVisualAperture::logical_only(
            SurfaceTargetRect::new(1, 2, 60, 61),
        ));
        captured_child.render_target_size = Some(BufferSize::new(60, 61).expect("target size"));

        let generations = HashMap::from([(42, 7), (43, 9)]);
        let snapshot = RetainedSurfacePresentationSnapshot::capture(
            42,
            &[root.clone(), captured_child.clone()],
            &generations,
        )
        .expect("usable root and child topology");

        let original_buffer_id = captured_child.buffer_id();
        let original_clip = captured_child.visual_clip.clone();
        let mut live_child = captured_child.clone();
        live_child.x = -48;
        live_child.y = 72;
        live_child.width = 120;
        live_child.height = 96;
        live_child.placement = SurfacePlacement::subsurface(42, -48, 72);
        live_child.render_placement = Some(SurfacePlacement::subsurface(42, -50, 70));
        live_child.visual_clip = None;
        live_child.render_target_size = Some(BufferSize::new(120, 96).expect("live target size"));
        live_child.generation = 100;
        live_child.commit_sequence = SurfaceCommitSequence(10);
        live_child.buffer_scale = 2;
        live_child.buffer_transform = wl_output::Transform::_90;
        live_child.viewport_source = Some(ViewportSourceRect {
            x: 1.0,
            y: 2.0,
            width: 20.0,
            height: 30.0,
        });
        live_child.viewport_destination = Some(BufferSize::new(40, 50).expect("viewport size"));
        live_child.damage =
            RenderableSurfaceDamage::Partial(vec![crate::compositor::SurfaceDamageRect {
                x: 1,
                y: 2,
                width: 3,
                height: 4,
            }]);
        let mut buffer_ids = BufferIdAllocator::default();
        let _ = buffer_ids.allocate().expect("first test buffer identity");
        let live_buffer_id = buffer_ids.allocate().expect("new live buffer identity");
        let live_buffer_key = live_buffer_id.id();
        live_child.buffer = CommittedSurfaceBuffer::shm_snapshot(
            live_buffer_id,
            BufferSize::new(120, 96).expect("new live buffer size"),
            vec![0xff12_3456; 120 * 96],
        );

        let projected = snapshot.project(&[root, live_child.clone()], &generations);
        let projected_child = projected
            .iter()
            .find(|surface| surface.surface_id == 43)
            .expect("captured child projected");
        assert_eq!((projected_child.x, projected_child.y), (16, 24));
        assert_eq!((projected_child.width, projected_child.height), (64, 64));
        assert_eq!(
            projected_child.placement,
            SurfacePlacement::subsurface(42, 16, 24)
        );
        assert_eq!(
            projected_child.render_placement,
            Some(SurfacePlacement::subsurface(42, 12, 20))
        );
        assert_eq!(projected_child.visual_clip, original_clip);
        assert_eq!(
            projected_child.render_target_size,
            Some(BufferSize::new(60, 61).expect("captured target size"))
        );
        assert_ne!(original_buffer_id, projected_child.buffer_id());
        assert_eq!(projected_child.buffer_id(), live_buffer_key);
        assert_eq!(projected_child.generation, 100);
        assert_eq!(projected_child.commit_sequence, SurfaceCommitSequence(10));
        assert_eq!(projected_child.buffer_scale, 2);
        assert_eq!(projected_child.buffer_transform, wl_output::Transform::_90);
        assert_eq!(projected_child.viewport_source, live_child.viewport_source);
        assert_eq!(
            projected_child.viewport_destination,
            live_child.viewport_destination
        );
        assert_eq!(projected_child.damage, live_child.damage);
        assert_eq!(
            live_child.placement,
            SurfacePlacement::subsurface(42, -48, 72)
        );
        assert_eq!((live_child.width, live_child.height), (120, 96));
    }

    #[test]
    fn projection_preserves_order_and_rejects_new_destroyed_and_reincarnated_surfaces() {
        let root = test_surface(42, SurfacePlacement::root());
        let child = test_surface(43, SurfacePlacement::subsurface(42, 1, 2));
        let grandchild = test_surface(44, SurfacePlacement::subsurface(43, 3, 4));
        let generations = HashMap::from([(42, 1), (43, 1), (44, 1), (45, 1)]);
        let snapshot = RetainedSurfacePresentationSnapshot::capture(
            42,
            &[root.clone(), child.clone(), grandchild.clone()],
            &generations,
        )
        .expect("complete captured tree");
        let new_surface = test_surface(45, SurfacePlacement::subsurface(42, 500, 500));

        let reordered = snapshot.project(
            &[grandchild.clone(), new_surface, root.clone(), child.clone()],
            &generations,
        );
        assert_eq!(
            reordered
                .iter()
                .map(|surface| surface.surface_id)
                .collect::<Vec<_>>(),
            vec![42, 43, 44]
        );

        let child_destroyed = snapshot.project(std::slice::from_ref(&root), &generations);
        assert_eq!(
            child_destroyed
                .iter()
                .map(|surface| surface.surface_id)
                .collect::<Vec<_>>(),
            vec![42]
        );

        let parent_destroyed = snapshot.project(&[root.clone(), grandchild.clone()], &generations);
        assert_eq!(
            parent_destroyed
                .iter()
                .map(|surface| surface.surface_id)
                .collect::<Vec<_>>(),
            vec![42]
        );

        let reincarnated_parent = test_surface(43, SurfacePlacement::subsurface(42, -48, 72));
        let reincarnated = snapshot.project(
            &[root, reincarnated_parent.clone(), grandchild],
            &HashMap::from([(42, 1), (43, 2), (44, 1)]),
        );
        assert_eq!(
            reincarnated
                .iter()
                .map(|surface| surface.surface_id)
                .collect::<Vec<_>>(),
            vec![42]
        );
        assert_eq!(reincarnated_parent.generation, child.generation);
    }

    #[test]
    fn capture_requires_exact_presentation_generations_for_the_retained_tree() {
        let root = test_surface(42, SurfacePlacement::root());
        let child = test_surface(43, SurfacePlacement::subsurface(42, 1, 2));

        assert!(
            RetainedSurfacePresentationSnapshot::capture(
                42,
                &[root.clone(), child.clone()],
                &HashMap::from([(43, 1)]),
            )
            .is_none()
        );
        assert!(
            RetainedSurfacePresentationSnapshot::capture(
                42,
                &[root, child],
                &HashMap::from([(42, 1)]),
            )
            .is_none()
        );
    }
}
