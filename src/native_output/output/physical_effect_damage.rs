use super::*;
use oblivion_one::compositor::{EffectAnchor, EffectAnchorScope, ResolvedEffectScene};
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
    /// Trusted built-in backdrop source boundary, frozen with this frame.
    pub(crate) backdrop_anchor: Option<(u32, EffectAnchorScope)>,
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
        let trusted_backdrop = instance.program == oblivion_one::effects::builtin_background_blur_program_id()
            && registry_generation.effect_for_program(instance.program).is_some_and(|effect| {
                effect.program.program.nodes.iter().any(|node| matches!(node.kind,
                    oblivion_one::effects::EffectNodeKind::Source(oblivion_one::effects::EffectSource::Backdrop)))
                    && effect.program.program.nodes.iter().all(|node| !matches!(node.kind,
                        oblivion_one::effects::EffectNodeKind::Source(source) if source != oblivion_one::effects::EffectSource::Backdrop))
            });
        let backdrop_anchor = match instance.anchor {
            EffectAnchor::BeforeSurface(surface_id) if trusted_backdrop => {
                Some((surface_id, instance.anchor_scope))
            }
            _ => None,
        };
        let footprint = registry_generation
            .effect_for_program(instance.program)
            .map(|effect| effect.program.aggregate_footprint);
        let Some(aggregate_footprint) = footprint else {
            snapshot.conservative_full = true;
            snapshot.instances.push(NativeEffectDamageInstanceSnapshot {
                instance_id: instance.id,
                backdrop_anchor,
                region: instance.region.clone(),
                aggregate_footprint: EffectFootprint::ZERO,
            });
            continue;
        };
        snapshot.instances.push(NativeEffectDamageInstanceSnapshot {
            instance_id: instance.id,
            backdrop_anchor,
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

/// A stable content-only transition has exact per-surface source evidence.
/// Unknown recipes and structural transitions keep the conservative path.
pub(crate) fn expand_ordered_backdrop_content_damage(
    base_damage: &NativeOutputDamage,
    previous: &NativeSceneSnapshot,
    current: &NativeSceneSnapshot,
    output_bounds: EffectRect,
) -> Option<NativeOutputDamage> {
    let effects = &current.physical_effect_damage;
    if effects.conservative_full
        || !effects.frame_local_dirty.is_empty()
        || previous.physical_effect_damage != *effects
        || previous.effect_identity_signature != current.effect_identity_signature
        || previous.effect_damage != current.effect_damage
        || previous.decorations != current.decorations
        || previous.popup_surface_ids != current.popup_surface_ids
        || previous.external_overlay_surface_ids != current.external_overlay_surface_ids
        || previous.visibility_signature != current.visibility_signature
        || previous.surfaces.len() != current.surfaces.len()
        || previous
            .surfaces
            .iter()
            .zip(&current.surfaces)
            .any(|(old, new)| {
                old.surface_id != new.surface_id
                    || old.scene_node_id != new.scene_node_id
                    || old.visual_root_surface_id != new.visual_root_surface_id
                    || old.presentation_owner_root_surface_id
                        != new.presentation_owner_root_surface_id
                    || old.bounds != new.bounds
            })
    {
        return None;
    }
    let mut ordered = Vec::with_capacity(effects.instances.len());
    for instance in &effects.instances {
        let (anchor_id, scope) = instance.backdrop_anchor?;
        let anchor_index = current
            .surfaces
            .iter()
            .position(|surface| surface.surface_id == anchor_id)?;
        let source_end = match scope {
            EffectAnchorScope::Surface => anchor_index,
            EffectAnchorScope::VisualGroup => {
                let root = current.surfaces[anchor_index].visual_root_surface_id;
                current
                    .surfaces
                    .iter()
                    .position(|surface| surface.visual_root_surface_id == root)?
            }
        };
        ordered.push((source_end, instance));
    }
    // Preserve original effect order for effects at the same source boundary.
    ordered.sort_by_key(|(source_end, _)| *source_end);
    let mut output_damage = base_damage.clone();
    let mut source_damage = EffectRegion::empty();
    let mut next_surface = 0;
    for (source_end, instance) in ordered {
        for surface in &current.surfaces[next_surface..source_end] {
            let rects = match &surface.damage {
                NativeSurfaceDamageEvidence::AuthoritativeEmpty => &[][..],
                NativeSurfaceDamageEvidence::Known(rects) => rects.as_slice(),
                NativeSurfaceDamageEvidence::HistoryLost => {
                    std::slice::from_ref(surface.bounds.as_ref()?)
                }
            };
            for rect in rects {
                if let Some(rect) = rect
                    .clipped_to_output(output_bounds.width, output_bounds.height)
                    .and_then(|rect| EffectRect::new(rect.x, rect.y, rect.width, rect.height))
                {
                    source_damage = source_damage.union(&EffectRegion::from_rect(rect));
                }
            }
        }
        next_surface = source_end;
        let plan = plan_effect_damage(
            instance.aggregate_footprint,
            &instance.region,
            &source_damage,
            output_bounds,
        );
        output_damage = output_damage.union_effect_region(
            &plan.output_damage,
            output_bounds.width,
            output_bounds.height,
        );
        // A later backdrop can observe this effect's output as well as lower
        // surface content. Keep this causal chain even when regions overlap.
        source_damage = source_damage.union(&plan.output_damage);
    }
    Some(output_damage)
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
            composite_opacity: None,
            id: EffectInstanceId::new(id).expect("effect instance id"),
            program,
            anchor,
            region: EffectRegion::from_rect(region),
            target_bounds: region,
            parameter_block: EffectParameterBlock::default(),
            coverage: None,
            signature: id,
            frame_demand,
            visual_group: None,
            anchor_scope: EffectAnchorScope::Surface,
            backdrop_reuse: oblivion_one::compositor::EffectBackdropReuse::Exact,
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
