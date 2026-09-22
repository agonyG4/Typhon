use super::*;
use oblivion_one::compositor::ResolvedEffectScene;
use oblivion_one::effects::{
    EffectFootprint, EffectInstanceId, EffectRect, EffectRegion, EffectRegistryGeneration,
    plan_effect_damage,
};

/// Immutable renderer-independent effect damage evidence for one resolved frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NativeEffectDamageFrameSnapshot {
    pub(crate) registry_generation: u64,
    pub(crate) instances: Vec<NativeEffectDamageInstanceSnapshot>,
    pub(crate) frame_local_dirty: EffectRegion,
    pub(crate) conservative_full: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeEffectDamageInstanceSnapshot {
    pub(crate) instance_id: EffectInstanceId,
    pub(crate) region: EffectRegion,
    pub(crate) aggregate_footprint: EffectFootprint,
}

impl NativeEffectDamageFrameSnapshot {
    pub(crate) fn conservative_full(
        registry_generation: u64,
        frame_local_dirty: EffectRegion,
    ) -> Self {
        Self {
            registry_generation,
            instances: Vec::new(),
            frame_local_dirty,
            conservative_full: true,
        }
    }
}

pub(crate) fn freeze_native_effect_damage(
    effects: &ResolvedEffectScene,
    registry_generation: &EffectRegistryGeneration,
    _output_bounds: EffectRect,
) -> NativeEffectDamageFrameSnapshot {
    let mut snapshot = NativeEffectDamageFrameSnapshot {
        registry_generation: registry_generation.generation,
        instances: Vec::new(),
        frame_local_dirty: effects.frame_demand_snapshot().dirty_region,
        conservative_full: false,
    };

    for instance in effects
        .instances
        .iter()
        .filter(|instance| !instance.region.is_empty())
    {
        let footprint = registry_generation
            .effect_for_program(instance.program)
            .map(|effect| effect.program.aggregate_footprint);
        let Some(aggregate_footprint) = footprint else {
            snapshot.conservative_full = true;
            snapshot.instances.push(NativeEffectDamageInstanceSnapshot {
                instance_id: instance.id,
                region: instance.region.clone(),
                aggregate_footprint: EffectFootprint::ZERO,
            });
            continue;
        };
        snapshot.instances.push(NativeEffectDamageInstanceSnapshot {
            instance_id: instance.id,
            region: instance.region.clone(),
            aggregate_footprint,
        });
    }
    snapshot
}

pub(crate) fn expand_physical_effect_damage(
    base_damage: NativeOutputDamage,
    previous: &NativeEffectDamageFrameSnapshot,
    current: &NativeEffectDamageFrameSnapshot,
    output_bounds: EffectRect,
) -> NativeOutputDamage {
    let output_width = output_bounds.width;
    let output_height = output_bounds.height;
    if base_damage.kind == NativeDamageKind::FullOutput {
        return base_damage;
    }
    if previous.registry_generation != current.registry_generation
        && (previous.conservative_full
            || current.conservative_full
            || !previous.instances.is_empty()
            || !current.instances.is_empty())
    {
        return NativeOutputDamage::full_output(output_width, output_height);
    }

    let mut source_damage = effect_region_from_native_damage(&base_damage);
    if !current.frame_local_dirty.is_empty() {
        source_damage = source_damage.union(&current.frame_local_dirty);
    }
    let mut output_damage =
        base_damage.union_effect_region(&current.frame_local_dirty, output_width, output_height);
    if source_damage.is_empty() {
        return output_damage;
    }
    if previous.conservative_full || current.conservative_full {
        return NativeOutputDamage::full_output(output_width, output_height);
    }

    for instance in previous.instances.iter().chain(&current.instances) {
        let plan = plan_effect_damage(
            instance.aggregate_footprint,
            &instance.region,
            &source_damage,
            output_bounds,
        );
        output_damage =
            output_damage.union_effect_region(&plan.output_damage, output_width, output_height);
        if output_damage.kind == NativeDamageKind::FullOutput {
            return output_damage;
        }
    }
    output_damage
}

fn effect_region_from_native_damage(damage: &NativeOutputDamage) -> EffectRegion {
    match damage.kind {
        NativeDamageKind::Empty => EffectRegion::empty(),
        NativeDamageKind::FullOutput => EffectRegion::empty(),
        NativeDamageKind::SurfaceDamage => damage
            .rects
            .iter()
            .filter_map(|rect| EffectRect::new(rect.x, rect.y, rect.width, rect.height))
            .fold(EffectRegion::empty(), |region, rect| {
                region.union(&EffectRegion::from_rect(rect))
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oblivion_one::compositor::{
        EffectAnchor, EffectAnchorScope, EffectSceneOrder, ResolvedEffectInstance,
    };
    use oblivion_one::effects::{EffectFrameDemand, EffectParameterBlock, EffectProgramId};

    fn instance(
        id: u64,
        program: EffectProgramId,
        anchor: EffectAnchor,
        region: EffectRect,
        frame_demand: EffectFrameDemand,
    ) -> ResolvedEffectInstance {
        ResolvedEffectInstance {
            id: EffectInstanceId::new(id).expect("effect instance id"),
            program,
            anchor,
            region: EffectRegion::from_rect(region),
            target_bounds: region,
            parameter_block: EffectParameterBlock::default(),
            signature: id,
            frame_demand,
            visual_group: None,
            anchor_scope: EffectAnchorScope::Surface,
            scene_order: EffectSceneOrder::for_anchor(anchor),
        }
    }

    #[test]
    fn freeze_matches_visible_renderer_instances_and_keeps_postprocess() {
        let generation = EffectRegistryGeneration::with_builtin_background_blur();
        let program = oblivion_one::effects::builtin_background_blur_program_id();
        let output_bounds = EffectRect::new(0, 0, 100, 80).expect("output bounds");
        let mut empty_instance = instance(
            5,
            program,
            EffectAnchor::BeforeSurface(13),
            EffectRect::new(0, 0, 1, 1).unwrap(),
            EffectFrameDemand::OnDamage,
        );
        empty_instance.region = EffectRegion::empty();
        let effects = ResolvedEffectScene::new(
            1,
            vec![
                instance(
                    1,
                    program,
                    EffectAnchor::BeforeSurface(10),
                    EffectRect::new(10, 10, 10, 10).unwrap(),
                    EffectFrameDemand::OnDamage,
                ),
                instance(
                    2,
                    program,
                    EffectAnchor::ReplaceSurface(11),
                    EffectRect::new(30, 10, 10, 10).unwrap(),
                    EffectFrameDemand::Continuous,
                ),
                instance(
                    3,
                    program,
                    EffectAnchor::AfterSurface(12),
                    EffectRect::new(50, 10, 10, 10).unwrap(),
                    EffectFrameDemand::OnDamage,
                ),
                instance(
                    4,
                    program,
                    EffectAnchor::OutputPostProcess,
                    EffectRect::new(70, 10, 10, 10).unwrap(),
                    EffectFrameDemand::Continuous,
                ),
                empty_instance,
            ],
        );

        let snapshot = freeze_native_effect_damage(&effects, &generation, output_bounds);

        assert_eq!(snapshot.registry_generation, generation.generation);
        assert_eq!(snapshot.instances.len(), 4);
        assert!(!snapshot.conservative_full);
        assert!(
            snapshot
                .instances
                .iter()
                .any(|instance| instance.instance_id.get() == 4)
        );
        assert!(snapshot.frame_local_dirty.contains_point(30, 10));
        assert!(snapshot.frame_local_dirty.contains_point(70, 10));
    }

    #[test]
    fn freeze_marks_missing_programs_conservative_without_omitting_instance() {
        let generation = EffectRegistryGeneration::empty();
        let program = EffectProgramId::new(999).expect("missing program id");
        let effects = ResolvedEffectScene::new(
            1,
            vec![instance(
                8,
                program,
                EffectAnchor::OutputPostProcess,
                EffectRect::new(10, 10, 20, 20).unwrap(),
                EffectFrameDemand::OnDamage,
            )],
        );
        let snapshot = freeze_native_effect_damage(
            &effects,
            &generation,
            EffectRect::new(0, 0, 100, 80).unwrap(),
        );

        assert_eq!(snapshot.registry_generation, generation.generation);
        assert!(snapshot.conservative_full);
        assert_eq!(snapshot.instances.len(), 1);
        assert_eq!(snapshot.instances[0].instance_id.get(), 8);
    }
}
