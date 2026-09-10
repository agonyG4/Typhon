use std::collections::HashMap;

use crate::presentation_animation::{
    PresentationGroupTransform, PresentationRect, PresentationSceneSample,
};

use crate::effects::{
    EffectFrameDemand, EffectInstanceId, EffectParameterBlock, EffectParameterDefinition,
    EffectParameterId, EffectProgramId, EffectRect, EffectRegion, EffectUniformValue,
};

use super::{
    BackgroundEffectRegion, InputRegionOp, SurfaceData, VisualGroupId, VisualStackGroup,
    visual_stack_groups,
};
use wayland_server::Resource;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectAnchor {
    BeforeSurface(u32),
    ReplaceSurface(u32),
    AfterSurface(u32),
    OutputPostProcess,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum EffectAnchorScope {
    #[default]
    Surface,
    VisualGroup,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EffectSceneOrder {
    pub group_order: u32,
    pub surface_order: u32,
    pub phase: u8,
}

impl EffectSceneOrder {
    pub const fn for_anchor(anchor: EffectAnchor) -> Self {
        match anchor {
            EffectAnchor::BeforeSurface(_) => Self {
                group_order: u32::MAX.saturating_sub(1),
                surface_order: 0,
                phase: 0,
            },
            EffectAnchor::ReplaceSurface(_) => Self {
                group_order: u32::MAX.saturating_sub(1),
                surface_order: 0,
                phase: 1,
            },
            EffectAnchor::AfterSurface(_) => Self {
                group_order: u32::MAX.saturating_sub(1),
                surface_order: 0,
                phase: 2,
            },
            EffectAnchor::OutputPostProcess => Self {
                group_order: u32::MAX,
                surface_order: u32::MAX,
                phase: 3,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SurfaceEffectSlot {
    Background,
    Content,
    Foreground,
}

impl SurfaceEffectSlot {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "background" => Some(Self::Background),
            "content" => Some(Self::Content),
            "foreground" => Some(Self::Foreground),
            _ => None,
        }
    }

    pub(crate) const fn anchor(self, surface_id: u32) -> EffectAnchor {
        match self {
            Self::Background => EffectAnchor::BeforeSurface(surface_id),
            Self::Content => EffectAnchor::ReplaceSurface(surface_id),
            Self::Foreground => EffectAnchor::AfterSurface(surface_id),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SurfaceEffectBindingKey {
    pub(crate) surface_id: u32,
    pub(crate) slot: SurfaceEffectSlot,
}

#[derive(Debug, Default)]
pub(crate) struct SurfaceEffectBindingOwners {
    owners: HashMap<SurfaceEffectBindingKey, u64>,
}

impl SurfaceEffectBindingOwners {
    pub(crate) fn claim(&mut self, key: SurfaceEffectBindingKey, owner: u64) -> bool {
        if self.owners.contains_key(&key) {
            return false;
        }
        self.owners.insert(key, owner);
        true
    }

    pub(crate) fn owns(&self, key: SurfaceEffectBindingKey, owner: u64) -> bool {
        self.owners.get(&key) == Some(&owner)
    }

    pub(crate) fn release(&mut self, key: SurfaceEffectBindingKey, owner: u64) -> bool {
        if !self.owns(key, owner) {
            return false;
        }
        self.owners.remove(&key).is_some()
    }
}

#[derive(Debug)]
pub(crate) struct ProtocolSurfaceEffectBinding {
    pub(crate) owner_id: u64,
    pub(crate) instance: Option<ResolvedEffectInstance>,
    pub(crate) program_name: Option<String>,
    pub(crate) program_generation: Option<u64>,
    pub(crate) schema_signature: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedEffectInstance {
    pub id: EffectInstanceId,
    pub program: EffectProgramId,
    pub anchor: EffectAnchor,
    pub region: EffectRegion,
    pub target_bounds: EffectRect,
    pub parameter_block: EffectParameterBlock,
    pub signature: u64,
    pub frame_demand: EffectFrameDemand,
    pub visual_group: Option<VisualGroupId>,
    pub anchor_scope: EffectAnchorScope,
    pub scene_order: EffectSceneOrder,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EffectSceneSummary {
    pub visible_instance_count: u32,
    pub requires_composition: bool,
    pub continuous_instance_count: u32,
    pub maximum_capture_pixels: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedEffectScene {
    pub instances: Vec<ResolvedEffectInstance>,
    pub summary: EffectSceneSummary,
    pub generation: u64,
    pub signature: u64,
}

impl Default for ResolvedEffectScene {
    fn default() -> Self {
        Self::new(0, Vec::new())
    }
}

impl ResolvedEffectScene {
    pub fn new(generation: u64, mut instances: Vec<ResolvedEffectInstance>) -> Self {
        instances.sort_by_key(effect_semantic_sort_key);
        let visible_instance_count = u32::try_from(instances.len()).unwrap_or(u32::MAX);
        let continuous_instance_count = u32::try_from(
            instances
                .iter()
                .filter(|instance| instance.frame_demand == EffectFrameDemand::Continuous)
                .count(),
        )
        .unwrap_or(u32::MAX);
        let maximum_capture_pixels = instances.iter().fold(0u64, |total, instance| {
            total.saturating_add(
                u64::from(instance.target_bounds.width)
                    .saturating_mul(u64::from(instance.target_bounds.height)),
            )
        });
        let summary = EffectSceneSummary {
            visible_instance_count,
            requires_composition: visible_instance_count != 0,
            continuous_instance_count,
            maximum_capture_pixels,
        };
        let signature = scene_signature(generation, &instances);
        Self {
            instances,
            summary,
            generation,
            signature,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    pub fn frame_demand_snapshot(&self) -> EffectFrameDemandSnapshot {
        EffectFrameDemandSnapshot::from_visible_instances(self.instances.iter().cloned())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EffectFrameDemandSnapshot {
    pub continuous_visible: bool,
    pub next_deadline_ns: Option<u64>,
    pub dirty_region: EffectRegion,
}

impl EffectFrameDemandSnapshot {
    pub fn from_visible_instances<I>(instances: I) -> Self
    where
        I: IntoIterator<Item = ResolvedEffectInstance>,
    {
        let mut demand = Self::default();
        for instance in instances {
            if instance.frame_demand == EffectFrameDemand::Continuous {
                demand.continuous_visible = true;
                demand.dirty_region = demand.dirty_region.union(&instance.region);
            }
        }
        demand
    }
}

impl super::CompositorState {
    pub(in crate::compositor) fn trusted_effect_registry(
        &self,
    ) -> &crate::effects::TrustedEffectRegistry {
        &self.trusted_effect_registry
    }

    pub(in crate::compositor) fn reconcile_trusted_effect_bindings(&mut self) {
        let generation = self.trusted_effect_registry.current();
        let mut removed = false;
        for binding in self.protocol_surface_effects.values_mut() {
            let had_state = binding.instance.is_some()
                || binding.program_name.is_some()
                || binding.program_generation.is_some()
                || binding.schema_signature.is_some();
            let valid = binding
                .instance
                .as_ref()
                .and_then(|instance| {
                    let name = binding.program_name.as_deref()?;
                    let program = generation.program_id(name)?;
                    let schema = generation.schema_signature(program)?;
                    (program == instance.program && Some(schema) == binding.schema_signature)
                        .then_some(())
                })
                .is_some();
            if !valid {
                binding.instance = None;
                binding.program_name = None;
                binding.program_generation = None;
                binding.schema_signature = None;
                removed |= had_state;
            } else if valid {
                binding.program_generation = Some(generation.generation);
            }
        }
        if removed {
            self.advance_render_generation_with_scene_effect(
                super::RenderGenerationCause::EffectBinding,
                true,
            );
            self.set_effect_scene_summary(self.resolved_effect_scene().summary);
        }
    }

    pub(in crate::compositor) fn effect_scene_summary(&self) -> EffectSceneSummary {
        self.effect_scene_summary
    }

    #[allow(dead_code)] // Used by internal effect assignment and later protocol adapters.
    pub(in crate::compositor) fn set_effect_scene_summary(&mut self, summary: EffectSceneSummary) {
        self.effect_scene_summary = summary;
    }

    pub(in crate::compositor) fn resolved_effect_scene(&self) -> ResolvedEffectScene {
        let mut instances = self
            .internal_surface_effects
            .values()
            .cloned()
            .collect::<Vec<_>>();
        instances.extend(
            self.protocol_surface_effects
                .values()
                .filter_map(|binding| binding.instance.clone()),
        );
        if self.background_effect_enabled {
            for (surface, origin) in self
                .active_scene_surfaces()
                .iter()
                .zip(self.active_scene_surface_origins())
            {
                let Some(surface_resource) = self.surface_resource_by_id(surface.surface_id) else {
                    continue;
                };
                let Some(surface_data) = surface_resource.data::<SurfaceData>() else {
                    continue;
                };
                let client_request_committed =
                    !surface_data.committed_background_effect().ops().is_empty();
                let root_surface_id = self.root_surface_id_for_surface(surface.surface_id);
                if root_surface_id != surface.surface_id && !client_request_committed {
                    continue;
                }
                let assignment = self.blur_assignment_for_surface(surface, surface_data);
                let region = match assignment {
                    crate::blur_policy::BlurAssignment::None => continue,
                    crate::blur_policy::BlurAssignment::ClientExact => {
                        background_effect_output_region(
                            &surface_data.committed_background_effect(),
                            *origin,
                            surface.width,
                            surface.height,
                        )
                    }
                    crate::blur_policy::BlurAssignment::CompositorSynthesized => {
                        let Some(rect) =
                            EffectRect::new(origin.0, origin.1, surface.width, surface.height)
                        else {
                            continue;
                        };
                        EffectRegion::from_rect(rect)
                    }
                };
                if assignment == crate::blur_policy::BlurAssignment::CompositorSynthesized
                    && self.has_existing_background_blur_instance(surface.surface_id)
                {
                    continue;
                }
                let Some(target_bounds) = region.bounding_rect() else {
                    continue;
                };
                instances.push(ResolvedEffectInstance {
                    id: background_effect_instance_id(surface.surface_id),
                    program: crate::effects::builtin_background_blur_program_id(),
                    anchor: EffectAnchor::BeforeSurface(surface.surface_id),
                    signature: internal_effect_signature(
                        surface.surface_id,
                        EffectAnchor::BeforeSurface(surface.surface_id),
                        crate::effects::builtin_background_blur_program_id(),
                        &region,
                        &EffectParameterBlock::default(),
                    ),
                    frame_demand: EffectFrameDemand::OnDamage,
                    visual_group: self.visual_group_for_surface(surface.surface_id),
                    region,
                    target_bounds,
                    parameter_block: EffectParameterBlock::default(),
                    anchor_scope: EffectAnchorScope::Surface,
                    scene_order: EffectSceneOrder::for_anchor(EffectAnchor::BeforeSurface(
                        surface.surface_id,
                    )),
                });
            }
        }
        for instance in &mut instances {
            instance.scene_order = self.scene_order_for_instance(instance);
        }
        instances.sort_by_key(effect_semantic_sort_key);
        ResolvedEffectScene::new(self.scene_render_generation, instances)
    }

    fn scene_order_for_instance(&self, instance: &ResolvedEffectInstance) -> EffectSceneOrder {
        let phase = match instance.anchor {
            EffectAnchor::BeforeSurface(_) => 0,
            EffectAnchor::ReplaceSurface(_) => 1,
            EffectAnchor::AfterSurface(_) => 2,
            EffectAnchor::OutputPostProcess => 3,
        };
        let Some(surface_id) = (match instance.anchor {
            EffectAnchor::BeforeSurface(surface_id)
            | EffectAnchor::ReplaceSurface(surface_id)
            | EffectAnchor::AfterSurface(surface_id) => Some(surface_id),
            EffectAnchor::OutputPostProcess => None,
        }) else {
            return EffectSceneOrder {
                group_order: u32::MAX,
                surface_order: u32::MAX,
                phase,
            };
        };
        let group_order = instance
            .visual_group
            .map_or(u32::MAX.saturating_sub(1), VisualGroupId::get);
        let surface_order = match instance.anchor_scope {
            EffectAnchorScope::Surface => self
                .active_scene_surfaces()
                .iter()
                .position(|surface| surface.surface_id == surface_id)
                .and_then(|index| u32::try_from(index).ok())
                .unwrap_or(u32::MAX.saturating_sub(1)),
            EffectAnchorScope::VisualGroup => 0,
        };
        EffectSceneOrder {
            group_order,
            surface_order,
            phase,
        }
    }

    pub(in crate::compositor) fn resolved_effect_scene_with_presentation(
        &self,
        presentation: &PresentationSceneSample,
    ) -> ResolvedEffectScene {
        let scene = self.resolved_effect_scene();
        if presentation.transforms.is_empty() {
            return scene;
        }

        let instances = scene
            .instances
            .into_iter()
            .map(|mut instance| {
                let surface_id = match instance.anchor {
                    EffectAnchor::BeforeSurface(surface_id)
                    | EffectAnchor::ReplaceSurface(surface_id)
                    | EffectAnchor::AfterSurface(surface_id) => Some(surface_id),
                    EffectAnchor::OutputPostProcess => None,
                };
                let Some(transform) = surface_id
                    .map(|surface_id| self.root_surface_id_for_surface(surface_id))
                    .and_then(|root| presentation.transform_for_root(root))
                else {
                    return instance;
                };

                instance.region =
                    map_effect_region(transform, &instance.region, instance.target_bounds);
                if let Some(target_bounds) = map_effect_rect(transform, instance.target_bounds) {
                    instance.target_bounds = target_bounds;
                }
                instance.signature =
                    instance.signature.wrapping_mul(0x0000_0100_0000_01b3) ^ transform.signature();
                instance
            })
            .collect();
        ResolvedEffectScene::new(scene.generation, instances)
    }

    pub(in crate::compositor) fn refresh_effect_scene_summary(&mut self) {
        self.set_effect_scene_summary(self.resolved_effect_scene().summary);
    }

    pub(in crate::compositor) fn set_background_effect_enabled(&mut self, enabled: bool) {
        let resolver_changed = self.blur_assignment.set_renderer_supported(enabled);
        if self.background_effect_enabled == enabled && !resolver_changed {
            return;
        }
        self.background_effect_enabled = enabled;
        self.advance_render_generation_with_scene_effect(
            super::RenderGenerationCause::EffectBinding,
            true,
        );
        self.refresh_effect_scene_summary();
    }

    #[allow(dead_code)] // Invoked by internal effect qualification and protocol adapters.
    pub(in crate::compositor) fn set_internal_surface_effect(
        &mut self,
        surface_id: u32,
        anchor: EffectAnchor,
        program: EffectProgramId,
        region: EffectRegion,
    ) -> bool {
        self.set_internal_surface_effect_with_parameters(
            surface_id,
            anchor,
            program,
            region,
            EffectParameterBlock::default(),
            EffectFrameDemand::OnDamage,
        )
    }

    pub(in crate::compositor) fn set_internal_surface_effect_with_parameters(
        &mut self,
        surface_id: u32,
        anchor: EffectAnchor,
        program: EffectProgramId,
        region: EffectRegion,
        parameter_block: EffectParameterBlock,
        frame_demand: EffectFrameDemand,
    ) -> bool {
        let Some(target_bounds) = region.bounding_rect() else {
            return false;
        };
        if region.is_empty() {
            return false;
        }

        let id = if let Some(instance) = self.internal_surface_effects.get(&surface_id) {
            instance.id
        } else {
            let candidate = self.next_internal_effect_instance_id.max(1);
            let Some(next) = candidate.checked_add(1) else {
                return false;
            };
            let Some(id) = EffectInstanceId::new(candidate) else {
                return false;
            };
            self.next_internal_effect_instance_id = next;
            id
        };
        let instance = ResolvedEffectInstance {
            id,
            program,
            anchor,
            region: region.clone(),
            target_bounds,
            parameter_block: parameter_block.clone(),
            signature: internal_effect_signature(
                surface_id,
                anchor,
                program,
                &region,
                &parameter_block,
            ),
            frame_demand,
            visual_group: self.visual_group_for_surface(surface_id),
            anchor_scope: EffectAnchorScope::VisualGroup,
            scene_order: EffectSceneOrder::for_anchor(anchor),
        };
        if self.internal_surface_effects.get(&surface_id) == Some(&instance) {
            return false;
        }

        self.internal_surface_effects.insert(surface_id, instance);
        self.advance_render_generation_with_scene_effect(
            super::RenderGenerationCause::EffectBinding,
            true,
        );
        self.set_effect_scene_summary(self.resolved_effect_scene().summary);
        true
    }

    pub(in crate::compositor) fn effect_program_id_for_name(
        &self,
        name: &str,
    ) -> Option<EffectProgramId> {
        self.trusted_effect_registry.current().program_id(name)
    }

    pub(in crate::compositor) fn prepare_protocol_surface_effect_program(
        &mut self,
        binding_id: u64,
        surface_id: u32,
        slot: SurfaceEffectSlot,
        name: &str,
        program: EffectProgramId,
    ) -> bool {
        let key = SurfaceEffectBindingKey { surface_id, slot };
        let generation = self.trusted_effect_registry.current();
        let Some(schema_signature) = generation.schema_signature(program) else {
            return false;
        };
        if generation.program_id(name) != Some(program) {
            return false;
        }
        let Some(binding) = self.protocol_surface_effects.get_mut(&key) else {
            return false;
        };
        if binding.owner_id != binding_id {
            return false;
        }
        binding.program_name = Some(name.to_owned());
        binding.program_generation = Some(generation.generation);
        binding.schema_signature = Some(schema_signature);
        true
    }

    pub(in crate::compositor) fn effect_parameter_definition(
        &self,
        program: EffectProgramId,
        name: &str,
    ) -> Option<EffectParameterDefinition> {
        self.trusted_effect_registry
            .current()
            .effects
            .values()
            .find(|effect| effect.program.program.id == program)
            .and_then(|effect| effect.parameters.get(name))
            .cloned()
    }

    pub(in crate::compositor) fn effect_parameter_defaults(
        &self,
        program: EffectProgramId,
    ) -> Vec<(EffectParameterId, EffectUniformValue)> {
        self.trusted_effect_registry
            .current()
            .effects
            .values()
            .find(|effect| effect.program.program.id == program)
            .map(|effect| {
                effect
                    .parameters
                    .values()
                    .map(|parameter| (parameter.spec.id, parameter.default))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(in crate::compositor) fn surface_effect_region(
        &self,
        surface_id: u32,
    ) -> Option<EffectRegion> {
        let (surface, origin) = self
            .active_scene_surfaces()
            .iter()
            .zip(self.active_scene_surface_origins())
            .find(|(surface, _)| surface.surface_id == surface_id)?;
        Some(EffectRegion::from_rect(EffectRect::new(
            origin.0,
            origin.1,
            surface.width,
            surface.height,
        )?))
    }

    pub(in crate::compositor) fn visual_group_for_surface(
        &self,
        surface_id: u32,
    ) -> Option<VisualGroupId> {
        let surfaces = self.active_scene_surfaces();
        let groups = visual_stack_groups(surfaces, self.active_scene_popup_surface_ids());
        groups
            .iter()
            .position(|group| {
                group
                    .surface_indices()
                    .iter()
                    .any(|index| surfaces[*index].surface_id == surface_id)
            })
            .and_then(VisualStackGroup::id_for_order)
    }

    pub(in crate::compositor) fn apply_protocol_surface_effect(
        &mut self,
        binding_id: u64,
        surface_id: u32,
        slot: SurfaceEffectSlot,
        program: EffectProgramId,
        enabled: bool,
        parameter_block: EffectParameterBlock,
    ) -> bool {
        let key = SurfaceEffectBindingKey { surface_id, slot };
        let owns_binding = self
            .protocol_surface_effects
            .get(&key)
            .is_some_and(|binding| binding.owner_id == binding_id);
        if !owns_binding {
            return false;
        }
        let generation = self.trusted_effect_registry.current();
        let Some(registered) = generation.effect_for_program(program) else {
            return false;
        };
        let binding_is_current = self
            .protocol_surface_effects
            .get(&key)
            .is_some_and(|binding| {
                binding.program_generation == Some(generation.generation)
                    && binding.schema_signature == Some(registered.schema_signature())
                    && binding.program_name.as_deref() == Some(registered.name.as_str())
            });
        if !binding_is_current {
            return false;
        }
        if !enabled {
            let removed = self
                .protocol_surface_effects
                .get_mut(&key)
                .and_then(|binding| binding.instance.take())
                .is_some();
            if removed {
                self.advance_render_generation_with_scene_effect(
                    super::RenderGenerationCause::EffectBinding,
                    true,
                );
                self.set_effect_scene_summary(self.resolved_effect_scene().summary);
            }
            return true;
        }
        let Some(region) = self.surface_effect_region(surface_id) else {
            return false;
        };
        let Some(target_bounds) = region.bounding_rect() else {
            return false;
        };
        let id = if let Some(instance) = self
            .protocol_surface_effects
            .get(&key)
            .and_then(|binding| binding.instance.as_ref())
        {
            instance.id
        } else {
            let candidate = self.next_protocol_effect_instance_id.max(1_u64 << 62);
            let Some(next) = candidate.checked_add(1) else {
                return false;
            };
            let Some(id) = EffectInstanceId::new(candidate) else {
                return false;
            };
            self.next_protocol_effect_instance_id = next;
            id
        };
        let instance = ResolvedEffectInstance {
            id,
            program,
            anchor: slot.anchor(surface_id),
            region: region.clone(),
            target_bounds,
            parameter_block: parameter_block.clone(),
            signature: internal_effect_signature(
                surface_id,
                slot.anchor(surface_id),
                program,
                &region,
                &parameter_block,
            ),
            frame_demand: registered.program.program.frame_demand,
            visual_group: self.visual_group_for_surface(surface_id),
            anchor_scope: EffectAnchorScope::VisualGroup,
            scene_order: EffectSceneOrder::for_anchor(slot.anchor(surface_id)),
        };
        let changed = self
            .protocol_surface_effects
            .get(&key)
            .and_then(|binding| binding.instance.as_ref())
            != Some(&instance);
        if changed {
            self.protocol_surface_effects
                .get_mut(&key)
                .expect("validated protocol binding must exist")
                .instance = Some(instance);
            self.advance_render_generation_with_scene_effect(
                super::RenderGenerationCause::EffectBinding,
                true,
            );
            self.set_effect_scene_summary(self.resolved_effect_scene().summary);
        }
        true
    }

    pub(in crate::compositor) fn claim_protocol_surface_effect(
        &mut self,
        surface_id: u32,
        slot: SurfaceEffectSlot,
    ) -> Option<u64> {
        let key = SurfaceEffectBindingKey { surface_id, slot };
        let owner_id = self.next_protocol_surface_binding_id.max(1);
        let next = owner_id.checked_add(1)?;
        if !self.surface_effect_binding_owners.claim(key, owner_id) {
            return None;
        }
        self.next_protocol_surface_binding_id = next;
        self.protocol_surface_effects.insert(
            key,
            ProtocolSurfaceEffectBinding {
                owner_id,
                instance: None,
                program_name: None,
                program_generation: None,
                schema_signature: None,
            },
        );
        Some(owner_id)
    }

    pub(in crate::compositor) fn release_protocol_surface_effect(
        &mut self,
        key: SurfaceEffectBindingKey,
        owner_id: u64,
    ) -> bool {
        if !self.surface_effect_binding_owners.release(key, owner_id) {
            return false;
        }
        let removed = self
            .protocol_surface_effects
            .remove(&key)
            .and_then(|binding| binding.instance)
            .is_some();
        if removed {
            self.advance_render_generation_with_scene_effect(
                super::RenderGenerationCause::EffectBinding,
                true,
            );
            self.set_effect_scene_summary(self.resolved_effect_scene().summary);
        }
        true
    }

    pub(in crate::compositor) fn release_protocol_surface_effects_for_surface(
        &mut self,
        surface_id: u32,
    ) {
        let keys = self
            .protocol_surface_effects
            .keys()
            .filter(|key| key.surface_id == surface_id)
            .copied()
            .collect::<Vec<_>>();
        let mut removed_instance = false;
        for key in keys {
            let Some(binding) = self.protocol_surface_effects.remove(&key) else {
                continue;
            };
            let _ = self
                .surface_effect_binding_owners
                .release(key, binding.owner_id);
            removed_instance |= binding.instance.is_some();
        }
        if removed_instance {
            self.advance_render_generation_with_scene_effect(
                super::RenderGenerationCause::EffectBinding,
                true,
            );
            self.set_effect_scene_summary(self.resolved_effect_scene().summary);
        }
    }

    #[allow(dead_code)] // Invoked by internal effect qualification and protocol adapters.
    pub(in crate::compositor) fn clear_internal_surface_effect(&mut self, surface_id: u32) -> bool {
        if self.internal_surface_effects.remove(&surface_id).is_none() {
            return false;
        }
        self.advance_render_generation_with_scene_effect(
            super::RenderGenerationCause::EffectBinding,
            true,
        );
        self.set_effect_scene_summary(self.resolved_effect_scene().summary);
        true
    }

    fn has_existing_background_blur_instance(&self, surface_id: u32) -> bool {
        let program = crate::effects::builtin_background_blur_program_id();
        let anchor = EffectAnchor::BeforeSurface(surface_id);
        self.internal_surface_effects
            .values()
            .chain(
                self.protocol_surface_effects
                    .values()
                    .filter_map(|binding| binding.instance.as_ref()),
            )
            .any(|instance| instance.program == program && instance.anchor == anchor)
    }
}

fn effect_semantic_sort_key(instance: &ResolvedEffectInstance) -> (EffectSceneOrder, u64) {
    (instance.scene_order, instance.id.get())
}

fn map_effect_rect(transform: PresentationGroupTransform, rect: EffectRect) -> Option<EffectRect> {
    let canonical = PresentationRect::new(
        f64::from(rect.x),
        f64::from(rect.y),
        f64::from(rect.width),
        f64::from(rect.height),
    )?;
    let mapped = transform.map_rect(canonical)?;
    let left = mapped.x().floor();
    let top = mapped.y().floor();
    let right = (mapped.x() + mapped.width()).ceil();
    let bottom = (mapped.y() + mapped.height()).ceil();
    if !left.is_finite() || !top.is_finite() || !right.is_finite() || !bottom.is_finite() {
        return None;
    }
    let left = left.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    let top = top.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    let right = right.clamp(f64::from(i32::MIN), f64::from(i32::MAX));
    let bottom = bottom.clamp(f64::from(i32::MIN), f64::from(i32::MAX));
    let width = (right - f64::from(left)).max(1.0).min(f64::from(u32::MAX)) as u32;
    let height = (bottom - f64::from(top)).max(1.0).min(f64::from(u32::MAX)) as u32;
    EffectRect::new(left, top, width, height)
}

fn map_effect_region(
    transform: PresentationGroupTransform,
    region: &EffectRegion,
    fallback_bounds: EffectRect,
) -> EffectRegion {
    if region.rects().is_empty() && !region.is_empty() {
        return map_effect_rect(transform, fallback_bounds)
            .map(EffectRegion::from_rect)
            .unwrap_or_default();
    }
    let mut mapped = EffectRegion::empty();
    for rect in region.rects() {
        if let Some(rect) = map_effect_rect(transform, *rect) {
            mapped.push(rect);
        }
    }
    mapped
}

fn background_effect_instance_id(surface_id: u32) -> EffectInstanceId {
    EffectInstanceId::new((1_u64 << 63) | (u64::from(surface_id) + 1))
        .expect("background effect instance id is non-zero")
}

pub(in crate::compositor) fn background_effect_output_region(
    region: &BackgroundEffectRegion,
    origin: (i32, i32),
    surface_width: u32,
    surface_height: u32,
) -> EffectRegion {
    let mut rects = Vec::new();
    for operation in region.ops() {
        let Some(local) = clipped_background_effect_rect(
            operation.rect().coordinates(),
            surface_width,
            surface_height,
        ) else {
            continue;
        };
        let Some(output) = translate_effect_rect(local, origin) else {
            continue;
        };
        match operation {
            InputRegionOp::Add(_) => rects.push(output),
            InputRegionOp::Subtract(_) => {
                rects = rects
                    .into_iter()
                    .flat_map(|existing| subtract_effect_rect(existing, output))
                    .collect();
            }
        }
    }
    let mut output = EffectRegion::empty();
    for rect in rects {
        output.push(rect);
    }
    output
}

fn clipped_background_effect_rect(
    (x, y, width, height): (i32, i32, i32, i32),
    surface_width: u32,
    surface_height: u32,
) -> Option<EffectRect> {
    let left = i64::from(x).max(0);
    let top = i64::from(y).max(0);
    let right = i64::from(x)
        .saturating_add(i64::from(width))
        .min(i64::from(surface_width));
    let bottom = i64::from(y)
        .saturating_add(i64::from(height))
        .min(i64::from(surface_height));
    effect_rect_from_edges(left, top, right, bottom)
}

fn translate_effect_rect(rect: EffectRect, origin: (i32, i32)) -> Option<EffectRect> {
    let x = i64::from(origin.0).saturating_add(i64::from(rect.x));
    let y = i64::from(origin.1).saturating_add(i64::from(rect.y));
    let right = x.saturating_add(i64::from(rect.width));
    let bottom = y.saturating_add(i64::from(rect.height));
    effect_rect_from_edges(x, y, right, bottom)
}

fn subtract_effect_rect(source: EffectRect, excluded: EffectRect) -> Vec<EffectRect> {
    let left = i64::from(source.x).max(i64::from(excluded.x));
    let top = i64::from(source.y).max(i64::from(excluded.y));
    let right = i64::from(source.right()).min(i64::from(excluded.right()));
    let bottom = i64::from(source.bottom()).min(i64::from(excluded.bottom()));
    if right <= left || bottom <= top {
        return vec![source];
    }

    [
        effect_rect_from_edges(
            i64::from(source.x),
            i64::from(source.y),
            i64::from(source.right()),
            top,
        ),
        effect_rect_from_edges(
            i64::from(source.x),
            bottom,
            i64::from(source.right()),
            i64::from(source.bottom()),
        ),
        effect_rect_from_edges(i64::from(source.x), top, left, bottom),
        effect_rect_from_edges(right, top, i64::from(source.right()), bottom),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn effect_rect_from_edges(left: i64, top: i64, right: i64, bottom: i64) -> Option<EffectRect> {
    if right <= left || bottom <= top {
        return None;
    }
    let x = i32::try_from(left).ok()?;
    let y = i32::try_from(top).ok()?;
    let width = u32::try_from(right - left).ok()?;
    let height = u32::try_from(bottom - top).ok()?;
    EffectRect::new(x, y, width, height)
}

#[allow(dead_code)] // Called by the staged internal assignment API.
fn internal_effect_signature(
    surface_id: u32,
    anchor: EffectAnchor,
    program: EffectProgramId,
    region: &EffectRegion,
    parameter_block: &EffectParameterBlock,
) -> u64 {
    let mut signature = 0xcbf2_9ce4_8422_2325_u64;
    for value in [
        u64::from(surface_id),
        program.get(),
        match anchor {
            EffectAnchor::BeforeSurface(id) => u64::from(id),
            EffectAnchor::ReplaceSurface(id) => u64::from(id) ^ (1 << 32),
            EffectAnchor::AfterSurface(id) => u64::from(id) ^ (2 << 32),
            EffectAnchor::OutputPostProcess => u64::MAX,
        },
    ] {
        signature ^= value;
        signature = signature.wrapping_mul(0x1000_0000_01b3);
    }
    for rect in region.rects() {
        for value in [
            rect.x as u64,
            rect.y as u64,
            u64::from(rect.width),
            u64::from(rect.height),
        ] {
            signature ^= value;
            signature = signature.wrapping_mul(0x1000_0000_01b3);
        }
    }
    for value in parameter_block.values() {
        signature ^= u64::from(value.id.get());
        signature = signature.wrapping_mul(0x1000_0000_01b3);
        match value.value {
            EffectUniformValue::Float(value) => signature ^= u64::from(value.to_bits()),
            EffectUniformValue::Vec2(value) => {
                for value in value {
                    signature ^= u64::from(value.to_bits());
                }
            }
            EffectUniformValue::Vec3(value) => {
                for value in value {
                    signature ^= u64::from(value.to_bits());
                }
            }
            EffectUniformValue::Vec4(value) => {
                for value in value {
                    signature ^= u64::from(value.to_bits());
                }
            }
            EffectUniformValue::Int(value) => signature ^= value as u64,
        }
        signature = signature.wrapping_mul(0x1000_0000_01b3);
    }
    signature
}

fn scene_signature(generation: u64, instances: &[ResolvedEffectInstance]) -> u64 {
    let mut signature = 0xcbf2_9ce4_8422_2325_u64 ^ generation;
    for instance in instances {
        for value in [
            instance.id.get(),
            instance.program.get(),
            instance.signature,
            match instance.anchor {
                EffectAnchor::BeforeSurface(id)
                | EffectAnchor::ReplaceSurface(id)
                | EffectAnchor::AfterSurface(id) => u64::from(id),
                EffectAnchor::OutputPostProcess => u64::MAX,
            },
            match instance.anchor_scope {
                EffectAnchorScope::Surface => 0,
                EffectAnchorScope::VisualGroup => 1,
            },
            u64::from(instance.scene_order.group_order),
            u64::from(instance.scene_order.surface_order),
            u64::from(instance.scene_order.phase),
        ] {
            signature ^= value;
            signature = signature.wrapping_mul(0x1000_0000_01b3);
        }
        for rect in instance.region.rects() {
            for value in [
                rect.x as u64,
                rect.y as u64,
                u64::from(rect.width),
                u64::from(rect.height),
            ] {
                signature ^= value;
                signature = signature.wrapping_mul(0x1000_0000_01b3);
            }
        }
    }
    signature
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::{InputRegionRect, SurfaceInputRegion};
    use crate::effects::{
        EffectInstanceId, EffectParameterBlock, EffectProgramId, EffectRect, EffectRegion,
    };

    fn test_effect_instance(
        id: EffectInstanceId,
        frame_demand: crate::effects::EffectFrameDemand,
        region: EffectRegion,
    ) -> ResolvedEffectInstance {
        let target_bounds = region
            .bounding_rect()
            .expect("test effect region must not be empty");
        ResolvedEffectInstance {
            id,
            program: EffectProgramId::new(1).unwrap(),
            anchor: EffectAnchor::OutputPostProcess,
            region,
            target_bounds,
            parameter_block: EffectParameterBlock::default(),
            signature: 1,
            frame_demand,
            visual_group: None,
            anchor_scope: EffectAnchorScope::VisualGroup,
            scene_order: EffectSceneOrder::for_anchor(EffectAnchor::OutputPostProcess),
        }
    }

    #[test]
    fn presentation_effect_regions_follow_group_scale() {
        let transform = PresentationGroupTransform::new(
            7,
            crate::presentation_animation::TransitionId::new(
                std::num::NonZeroU64::new(5).expect("non-zero transition id"),
            ),
            PresentationRect::new(10.0, 20.0, 100.0, 80.0).expect("canonical group"),
            PresentationRect::new(15.0, 30.0, 200.0, 160.0).expect("presented group"),
            false,
        );
        let region = EffectRegion::from_rect(EffectRect::new(20, 30, 10, 8).unwrap());
        let mapped = map_effect_region(transform, &region, region.bounding_rect().unwrap());
        assert_eq!(mapped.rects(), &[EffectRect::new(35, 50, 20, 16).unwrap()]);
    }

    #[test]
    fn empty_effect_scene_does_not_require_composition_or_frames() {
        let scene = ResolvedEffectScene::default();
        assert_eq!(scene.summary.visible_instance_count, 0);
        assert!(!scene.summary.requires_composition);
        assert_eq!(scene.summary.continuous_instance_count, 0);
    }

    #[test]
    fn resolved_effect_scene_is_immutable_after_source_state_changes() {
        let region = EffectRegion::from_rect(EffectRect::new(10, 20, 80, 40).unwrap());
        let instance = test_effect_instance(
            EffectInstanceId::new(1).unwrap(),
            crate::effects::EffectFrameDemand::OnDamage,
            region,
        );
        let scene = ResolvedEffectScene::new(7, vec![instance]);
        let mut source_state = scene.instances.clone();
        source_state[0].signature = 99;
        assert_eq!(scene.instances[0].signature, 1);
        assert_eq!(scene.generation, 7);
    }

    #[test]
    fn continuous_demand_is_localized_to_visible_instances() {
        let region = EffectRegion::from_rect(EffectRect::new(10, 20, 80, 40).unwrap());
        let instance = test_effect_instance(
            EffectInstanceId::new(1).unwrap(),
            crate::effects::EffectFrameDemand::Continuous,
            region,
        );
        let demand = EffectFrameDemandSnapshot::from_visible_instances([instance]);
        assert!(demand.continuous_visible);
        assert!(demand.dirty_region.contains_point(10, 20));
    }

    #[test]
    fn changing_effect_scene_changes_its_render_identity() {
        let first_region = EffectRegion::from_rect(EffectRect::new(10, 20, 80, 40).unwrap());
        let second_region = EffectRegion::from_rect(EffectRect::new(20, 20, 80, 40).unwrap());
        let first = ResolvedEffectScene::new(
            1,
            vec![test_effect_instance(
                EffectInstanceId::new(1).unwrap(),
                crate::effects::EffectFrameDemand::OnDamage,
                first_region,
            )],
        );
        let second = ResolvedEffectScene::new(
            1,
            vec![test_effect_instance(
                EffectInstanceId::new(1).unwrap(),
                crate::effects::EffectFrameDemand::OnDamage,
                second_region,
            )],
        );
        assert_ne!(first.signature, second.signature);
    }

    #[test]
    fn internal_surface_effect_binding_updates_scene_summary_generation_and_damage() {
        let mut state = crate::compositor::CompositorState::new(None);
        let program = crate::effects::EffectProgramId::new(1).unwrap();
        let first_region = EffectRegion::from_rect(EffectRect::new(100, 80, 320, 180).unwrap());
        let second_region = EffectRegion::from_rect(EffectRect::new(120, 80, 320, 180).unwrap());
        let initial_generation = state.scene_render_generation;

        assert!(state.set_internal_surface_effect(
            42,
            EffectAnchor::BeforeSurface(42),
            program,
            first_region.clone(),
        ));
        assert_eq!(state.effect_scene_summary().visible_instance_count, 1);
        assert!(state.effect_scene_summary().requires_composition);
        assert!(state.scene_render_generation > initial_generation);
        assert_eq!(
            crate::compositor::direct_scanout_scene_rejection_for_effects(
                state.effect_scene_summary()
            ),
            Some(crate::compositor::DirectScanoutSceneRejection::EffectRequiresComposition)
        );

        let first_scene = state.resolved_effect_scene();
        assert_eq!(first_scene.instances.len(), 1);
        assert_eq!(first_scene.instances[0].program, program);
        assert_eq!(
            first_scene.instances[0].anchor,
            EffectAnchor::BeforeSurface(42)
        );

        assert!(state.set_internal_surface_effect(
            42,
            EffectAnchor::BeforeSurface(42),
            program,
            second_region.clone(),
        ));
        let second_scene = state.resolved_effect_scene();
        assert_ne!(first_scene.signature, second_scene.signature);
        let transition = crate::effects::effect_transition_damage(
            &first_scene.instances[0].region,
            &second_scene.instances[0].region,
        );
        assert!(transition.contains_point(100, 80));
        assert!(transition.contains_point(120, 80));

        assert!(state.clear_internal_surface_effect(42));
        assert!(state.resolved_effect_scene().is_empty());
        assert_eq!(state.effect_scene_summary(), EffectSceneSummary::default());
        assert_eq!(
            crate::compositor::direct_scanout_scene_rejection_for_effects(
                state.effect_scene_summary()
            ),
            None
        );
    }

    #[test]
    fn background_effect_regions_are_clipped_translated_and_subtracted() {
        let region =
            BackgroundEffectRegion::from_surface_input_region(SurfaceInputRegion::Custom(vec![
                InputRegionOp::Add(InputRegionRect::new(0, 0, 10, 10).unwrap()),
                InputRegionOp::Subtract(InputRegionRect::new(4, 3, 2, 2).unwrap()),
            ]));
        let output = background_effect_output_region(&region, (100, 50), 8, 8);

        assert!(output.contains_point(100, 50));
        assert!(output.contains_point(107, 57));
        assert!(
            !output.contains_point(104, 53),
            "rects={:?}",
            output.rects()
        );
        assert!(!output.contains_point(105, 54));
        assert!(!output.contains_point(108, 58));
    }

    #[test]
    fn surface_slots_have_typed_anchor_semantics() {
        assert_eq!(
            SurfaceEffectSlot::parse("background"),
            Some(SurfaceEffectSlot::Background)
        );
        assert_eq!(
            SurfaceEffectSlot::parse("content"),
            Some(SurfaceEffectSlot::Content)
        );
        assert_eq!(
            SurfaceEffectSlot::parse("foreground"),
            Some(SurfaceEffectSlot::Foreground)
        );
        assert_eq!(SurfaceEffectSlot::parse("output"), None);
        assert_eq!(SurfaceEffectSlot::parse("unknown"), None);
        assert_eq!(
            SurfaceEffectSlot::Background.anchor(42),
            EffectAnchor::BeforeSurface(42)
        );
        assert_eq!(
            SurfaceEffectSlot::Content.anchor(42),
            EffectAnchor::ReplaceSurface(42)
        );
        assert_eq!(
            SurfaceEffectSlot::Foreground.anchor(42),
            EffectAnchor::AfterSurface(42)
        );
    }

    #[test]
    fn disabled_bindings_claim_slots_and_stale_destroy_cannot_remove_replacement() {
        let key = SurfaceEffectBindingKey {
            surface_id: 42,
            slot: SurfaceEffectSlot::Background,
        };
        let mut owners = SurfaceEffectBindingOwners::default();

        assert!(owners.claim(key, 1));
        assert!(!owners.claim(key, 2));
        assert!(owners.owns(key, 1));
        assert!(!owners.release(key, 2));
        assert!(owners.owns(key, 1));
        assert!(owners.release(key, 1));
        assert!(!owners.owns(key, 1));
        assert!(owners.claim(key, 2));
        assert!(!owners.release(key, 1));
        assert!(owners.owns(key, 2));
    }

    #[test]
    fn surface_destruction_releases_every_owned_slot_once() {
        let mut state = crate::compositor::CompositorState::new(None);
        let background = SurfaceEffectBindingKey {
            surface_id: 42,
            slot: SurfaceEffectSlot::Background,
        };
        let foreground = SurfaceEffectBindingKey {
            surface_id: 42,
            slot: SurfaceEffectSlot::Foreground,
        };
        let background_owner = state
            .claim_protocol_surface_effect(42, SurfaceEffectSlot::Background)
            .unwrap();
        let foreground_owner = state
            .claim_protocol_surface_effect(42, SurfaceEffectSlot::Foreground)
            .unwrap();

        state.release_protocol_surface_effects_for_surface(42);

        assert!(
            !state
                .surface_effect_binding_owners
                .owns(background, background_owner)
        );
        assert!(
            !state
                .surface_effect_binding_owners
                .owns(foreground, foreground_owner)
        );
        assert!(
            state
                .claim_protocol_surface_effect(42, SurfaceEffectSlot::Background)
                .is_some()
        );
        state.release_protocol_surface_effects_for_surface(42);
        assert!(
            state
                .claim_protocol_surface_effect(42, SurfaceEffectSlot::Background)
                .is_some()
        );
    }

    #[test]
    fn semantic_effect_order_ignores_creation_order_and_preserves_slot_phase() {
        let region = EffectRegion::from_rect(EffectRect::new(0, 0, 10, 10).unwrap());
        let mut foreground = test_effect_instance(
            EffectInstanceId::new(30).unwrap(),
            crate::effects::EffectFrameDemand::OnDamage,
            region.clone(),
        );
        foreground.anchor = EffectAnchor::AfterSurface(20);
        foreground.visual_group = VisualGroupId::new(2);
        foreground.scene_order = EffectSceneOrder {
            group_order: 2,
            surface_order: 0,
            phase: 2,
        };
        let mut background = test_effect_instance(
            EffectInstanceId::new(20).unwrap(),
            crate::effects::EffectFrameDemand::OnDamage,
            region.clone(),
        );
        background.anchor = EffectAnchor::BeforeSurface(10);
        background.visual_group = VisualGroupId::new(1);
        background.scene_order = EffectSceneOrder {
            group_order: 1,
            surface_order: 0,
            phase: 0,
        };
        let mut content = test_effect_instance(
            EffectInstanceId::new(10).unwrap(),
            crate::effects::EffectFrameDemand::OnDamage,
            region,
        );
        content.anchor = EffectAnchor::ReplaceSurface(10);
        content.visual_group = VisualGroupId::new(1);
        content.scene_order = EffectSceneOrder {
            group_order: 1,
            surface_order: 0,
            phase: 1,
        };

        let scene = ResolvedEffectScene::new(1, vec![foreground, content, background]);
        let order = scene
            .instances
            .iter()
            .map(|instance| instance.id.get())
            .collect::<Vec<_>>();
        assert_eq!(order, vec![20, 10, 30]);
    }
}
