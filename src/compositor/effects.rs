use crate::effects::{
    EffectFrameDemand, EffectInstanceId, EffectParameterBlock, EffectProgramId, EffectRect,
    EffectRegion,
};

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
        instances.sort_by_key(|instance| instance.id);
        ResolvedEffectScene::new(self.scene_render_generation, instances)
    }

    #[allow(dead_code)] // Invoked by internal effect qualification and protocol adapters.
    pub(in crate::compositor) fn set_internal_surface_effect(
        &mut self,
        surface_id: u32,
        anchor: EffectAnchor,
        program: EffectProgramId,
        region: EffectRegion,
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
            parameter_block: EffectParameterBlock::default(),
            signature: internal_effect_signature(surface_id, anchor, program, &region),
            frame_demand: EffectFrameDemand::OnDamage,
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

#[allow(dead_code)] // Called by the staged internal assignment API.
fn internal_effect_signature(
    surface_id: u32,
    anchor: EffectAnchor,
    program: EffectProgramId,
    region: &EffectRegion,
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
}
