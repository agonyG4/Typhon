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

    pub(in crate::compositor) fn set_effect_scene_summary(&mut self, summary: EffectSceneSummary) {
        self.effect_scene_summary = summary;
    }
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
}
