use std::collections::HashMap;

use oblivion_one::compositor::VisualGroupId;

use super::{
    geometry::{EglDrawCommand, EglDrawLayer, EglRect, EglTexturedVertex, SurfaceSampling},
    scene::EglSceneSurfaceSignature,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::egl_renderer) struct EglCheckpointRectBits([u32; 4]);

impl EglCheckpointRectBits {
    pub(in crate::egl_renderer) fn from_rect(rect: EglRect) -> Self {
        Self([
            rect.x().to_bits(),
            rect.y().to_bits(),
            rect.width().to_bits(),
            rect.height().to_bits(),
        ])
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::egl_renderer) enum EglCheckpointPixelSourceIdentity {
    Layer(EglDrawLayer),
    Surface(EglSceneSurfaceSignature),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::egl_renderer) struct EglCheckpointCommandCausalSnapshot {
    pub(in crate::egl_renderer) layer: EglDrawLayer,
    pub(in crate::egl_renderer) visual_group: Option<VisualGroupId>,
    pub(in crate::egl_renderer) bounds: EglCheckpointRectBits,
    pub(in crate::egl_renderer) opaque_regions: Vec<EglCheckpointRectBits>,
    pub(in crate::egl_renderer) presentation_clip: Option<EglCheckpointRectBits>,
    pub(in crate::egl_renderer) vertex_start: u32,
    pub(in crate::egl_renderer) vertex_count: u32,
    pub(in crate::egl_renderer) sampling: SurfaceSampling,
    pub(in crate::egl_renderer) presentation_opacity_bits: u32,
    pub(in crate::egl_renderer) presentation_owner_root: Option<u32>,
    pub(in crate::egl_renderer) vertices: Vec<[u32; 4]>,
    pub(in crate::egl_renderer) pixel_source: Option<EglCheckpointPixelSourceIdentity>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::egl_renderer) struct EglCheckpointSceneCausalSnapshot {
    pub(in crate::egl_renderer) output_size: (u32, u32),
    pub(in crate::egl_renderer) commands: Vec<EglCheckpointCommandCausalSnapshot>,
    pub(in crate::egl_renderer) presentation_visual_group_owners: HashMap<VisualGroupId, u32>,
}

impl EglCheckpointSceneCausalSnapshot {
    pub(in crate::egl_renderer) fn new(
        output_size: (u32, u32),
        commands: &[EglDrawCommand],
        vertices: &[EglTexturedVertex],
        surface_signatures: &[EglSceneSurfaceSignature],
        presentation_opacities: &[f32],
        presentation_visual_group_owners: &HashMap<VisualGroupId, u32>,
    ) -> Self {
        let surface_signatures = surface_signatures
            .iter()
            .map(|signature| (signature.surface_id, *signature))
            .collect::<HashMap<_, _>>();
        let commands = commands
            .iter()
            .enumerate()
            .map(|(command_index, command)| {
                let start = usize::try_from(command.vertex_start).ok();
                let end = start.and_then(|start| {
                    usize::try_from(command.vertex_count)
                        .ok()
                        .and_then(|count| start.checked_add(count))
                });
                let command_vertices = start
                    .zip(end)
                    .and_then(|(start, end)| vertices.get(start..end))
                    .map(|vertices| {
                        vertices
                            .iter()
                            .map(|vertex| {
                                [
                                    vertex.position[0].to_bits(),
                                    vertex.position[1].to_bits(),
                                    vertex.uv[0].to_bits(),
                                    vertex.uv[1].to_bits(),
                                ]
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let vertex_range_is_valid = start
                    .zip(end)
                    .is_some_and(|(start, end)| vertices.get(start..end).is_some());
                let pixel_source = match command.layer {
                    EglDrawLayer::Surface(surface_id) => surface_signatures
                        .get(&surface_id)
                        .copied()
                        .map(EglCheckpointPixelSourceIdentity::Surface),
                    EglDrawLayer::Solid(_)
                    | EglDrawLayer::SolidRgba(_)
                    | EglDrawLayer::DecorationAsset(_) => {
                        Some(EglCheckpointPixelSourceIdentity::Layer(command.layer))
                    }
                    EglDrawLayer::LifecycleResolvedVisual(_) | EglDrawLayer::Cursor => None,
                };
                EglCheckpointCommandCausalSnapshot {
                    layer: command.layer,
                    visual_group: command.visual_group,
                    bounds: EglCheckpointRectBits::from_rect(command.bounds),
                    opaque_regions: command
                        .opaque_regions
                        .iter()
                        .copied()
                        .map(EglCheckpointRectBits::from_rect)
                        .collect(),
                    presentation_clip: command
                        .presentation_clip
                        .map(EglCheckpointRectBits::from_rect),
                    vertex_start: command.vertex_start,
                    vertex_count: command.vertex_count,
                    sampling: command.sampling,
                    presentation_opacity_bits: presentation_opacities
                        .get(command_index)
                        .copied()
                        .unwrap_or(1.0)
                        .to_bits(),
                    presentation_owner_root: command
                        .visual_group
                        .and_then(|group| presentation_visual_group_owners.get(&group).copied()),
                    vertices: if vertex_range_is_valid {
                        command_vertices
                    } else {
                        Vec::new()
                    },
                    pixel_source: pixel_source.filter(|_| vertex_range_is_valid),
                }
            })
            .collect();
        Self {
            output_size,
            commands,
            presentation_visual_group_owners: presentation_visual_group_owners.clone(),
        }
    }

    pub(in crate::egl_renderer) fn presentation_owner_for_visual_group(
        &self,
        visual_group: Option<VisualGroupId>,
    ) -> Option<u32> {
        visual_group.and_then(|group| self.presentation_visual_group_owners.get(&group).copied())
    }

    pub(in crate::egl_renderer) fn unchanged_command_prefix_len(&self, current: &Self) -> usize {
        if self.output_size != current.output_size {
            return 0;
        }
        self.commands
            .iter()
            .zip(&current.commands)
            .take_while(|(previous, current)| {
                previous.pixel_source.is_some()
                    && current.pixel_source.is_some()
                    && previous == current
            })
            .count()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::egl_renderer) struct CheckpointEffectCausalState {
    pub(in crate::egl_renderer) semantic_signature: u64,
    pub(in crate::egl_renderer) frame_demand: oblivion_one::effects::EffectFrameDemand,
    pub(in crate::egl_renderer) causal_backdrop_only: bool,
    pub(in crate::egl_renderer) capture_owner_root: Option<u32>,
    pub(in crate::egl_renderer) composition_boundary: Option<(
        usize,
        oblivion_one::compositor::EffectAnchor,
        Option<VisualGroupId>,
        oblivion_one::compositor::EffectAnchorScope,
    )>,
    pub(in crate::egl_renderer) dependencies: Vec<oblivion_one::effects::EffectInstanceId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::egl_renderer) struct CheckpointCausalState {
    pub(in crate::egl_renderer) scene: EglCheckpointSceneCausalSnapshot,
    pub(in crate::egl_renderer) effects:
        HashMap<oblivion_one::effects::EffectInstanceId, CheckpointEffectCausalState>,
}
