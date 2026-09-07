use crate::effects::{
    EffectFrameDemand, EffectInstanceId, EffectParameterBlock, EffectParameterDefinition,
    EffectParameterId, EffectProgramId, EffectRect, EffectRegion, EffectUniformValue,
};

use super::{BackgroundEffectRegion, InputRegionOp, SurfaceData};
use wayland_server::Resource;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectAnchor {
    BeforeSurface(u32),
    ReplaceSurface(u32),
    AfterSurface(u32),
    OutputPostProcess,
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
    pub fn new(generation: u64, instances: Vec<ResolvedEffectInstance>) -> Self {
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
        if self.background_effect_enabled && !self.background_effect_surface_ids.is_empty() {
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
                let region = background_effect_output_region(
                    &surface_data.committed_background_effect(),
                    *origin,
                    surface.width,
                    surface.height,
                );
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
                    region,
                    target_bounds,
                    parameter_block: EffectParameterBlock::default(),
                });
            }
        }
        instances.sort_by_key(|instance| instance.id);
        ResolvedEffectScene::new(self.scene_render_generation, instances)
    }

    pub(in crate::compositor) fn refresh_effect_scene_summary(&mut self) {
        self.set_effect_scene_summary(self.resolved_effect_scene().summary);
    }

    pub(in crate::compositor) fn set_background_effect_enabled(&mut self, enabled: bool) {
        if self.background_effect_enabled == enabled {
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

    pub(in crate::compositor) fn apply_protocol_surface_effect(
        &mut self,
        surface_id: u32,
        program: EffectProgramId,
        enabled: bool,
        parameter_block: EffectParameterBlock,
    ) -> bool {
        if !enabled {
            self.clear_internal_surface_effect(surface_id);
            return true;
        }
        let Some(region) = self.surface_effect_region(surface_id) else {
            return false;
        };
        self.set_internal_surface_effect_with_parameters(
            surface_id,
            EffectAnchor::BeforeSurface(surface_id),
            program,
            region,
            parameter_block,
            EffectFrameDemand::OnDamage,
        )
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
        }
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
}
