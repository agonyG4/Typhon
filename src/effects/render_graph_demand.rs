use std::sync::OnceLock;

use super::{
    CompiledFrameGraph, EffectDemandPlanStats, EffectPassExecutionDemand, EffectRegion,
    EffectVisibleClipFallback, GraphPassId, GraphTextureId, GraphTextureSource, RenderPassKind,
    graph_execution_metadata_issue, required_input_region, texture_producers,
    unique_instance_index,
};

pub const EFFECT_DEMAND_PLANNER_ENV: &str = "TYPHON_EFFECT_DEMAND_PLANNER";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EffectDemandPlannerPolicy {
    #[default]
    Legacy,
    Regional,
}

impl EffectDemandPlannerPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Regional => "regional",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParsedEffectDemandPlannerPolicy {
    pub policy: EffectDemandPlannerPolicy,
    pub unknown_value: bool,
}

pub fn parse_effect_demand_planner_policy(value: Option<&str>) -> ParsedEffectDemandPlannerPolicy {
    let Some(value) = value.map(str::trim) else {
        return ParsedEffectDemandPlannerPolicy {
            policy: EffectDemandPlannerPolicy::Legacy,
            unknown_value: false,
        };
    };

    let policy = if value.eq_ignore_ascii_case("legacy") {
        Some(EffectDemandPlannerPolicy::Legacy)
    } else if value.eq_ignore_ascii_case("regional") {
        Some(EffectDemandPlannerPolicy::Regional)
    } else {
        None
    };
    ParsedEffectDemandPlannerPolicy {
        policy: policy.unwrap_or_default(),
        unknown_value: policy.is_none(),
    }
}

pub fn configured_effect_demand_planner_policy() -> EffectDemandPlannerPolicy {
    static POLICY: OnceLock<EffectDemandPlannerPolicy> = OnceLock::new();
    static WARNED_UNKNOWN: OnceLock<()> = OnceLock::new();

    *POLICY.get_or_init(|| {
        let value = std::env::var(EFFECT_DEMAND_PLANNER_ENV).ok();
        let parsed = parse_effect_demand_planner_policy(value.as_deref());
        if parsed.unknown_value {
            WARNED_UNKNOWN.get_or_init(|| {
                eprintln!(
                    "warning: unknown {EFFECT_DEMAND_PLANNER_ENV} value; using legacy planner"
                );
            });
        }
        parsed.policy
    })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EffectDemandRegionalFallbackReason {
    #[default]
    None,
    CallerConservativeFull,
    RepairRegionUnrepresentable,
    GraphMetadataIncomplete,
    FinalPassMissing,
    PassLookupFailed,
    TextureLookupFailed,
    TextureProducerAmbiguous,
    DependencyOrderingInvalid,
    RequiredInputRegionUnavailable,
    RegionOutsideDomain,
}

impl EffectDemandRegionalFallbackReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::CallerConservativeFull => "caller_conservative_full",
            Self::RepairRegionUnrepresentable => "repair_region_unrepresentable",
            Self::GraphMetadataIncomplete => "graph_metadata_incomplete",
            Self::FinalPassMissing => "final_pass_missing",
            Self::PassLookupFailed => "pass_lookup_failed",
            Self::TextureLookupFailed => "texture_lookup_failed",
            Self::TextureProducerAmbiguous => "texture_producer_ambiguous",
            Self::DependencyOrderingInvalid => "dependency_ordering_invalid",
            Self::RequiredInputRegionUnavailable => "required_input_region_unavailable",
            Self::RegionOutsideDomain => "region_outside_domain",
        }
    }
}

pub(crate) struct RegionalEffectDemandPlan {
    pub(crate) presentation_output_regions: Vec<EffectRegion>,
    pub(crate) pass_regions: Vec<EffectRegion>,
    pub(crate) final_pass_indices: Vec<usize>,
    pub(crate) passes: Vec<EffectPassExecutionDemand>,
    pub(crate) execution_region: EffectRegion,
    pub(crate) visible_clip_fallbacks: Vec<EffectVisibleClipFallback>,
    pub(crate) plan_stats: EffectDemandPlanStats,
}

pub(crate) fn plan_regional_effect_demand(
    graph: &CompiledFrameGraph,
    repair_region: &EffectRegion,
    conservative_full: bool,
    full_kawase: bool,
) -> Result<RegionalEffectDemandPlan, EffectDemandRegionalFallbackReason> {
    if conservative_full {
        return Err(EffectDemandRegionalFallbackReason::CallerConservativeFull);
    }
    if !repair_region.is_empty() && repair_region.bounding_rect().is_none() {
        return Err(EffectDemandRegionalFallbackReason::RepairRegionUnrepresentable);
    }
    if graph_execution_metadata_issue(graph).is_some() {
        return Err(EffectDemandRegionalFallbackReason::GraphMetadataIncomplete);
    }

    let producers = texture_producers(graph);
    let final_pass_indices = validate_regional_graph(graph, &producers)?;
    let mut presentation_output_regions = vec![EffectRegion::empty(); graph.instances.len()];
    let mut pass_regions = vec![EffectRegion::empty(); graph.passes.len()];
    let mut visible_clip_fallbacks = Vec::new();
    let mut max_instance_region_rect_count = 0;
    let mut region_representation_overflows = graph.stats.region_representation_overflows;
    let mut visible_clip_fallback_count = graph.stats.visible_clip_fallbacks;
    let mut work_region_bbox_coalesces = graph.stats.work_region_bbox_coalesces;

    for (instance_index, instance) in graph.instances.iter().enumerate() {
        let direct_result =
            repair_region.intersect_bounded_within_result(&instance.output_influence_region);
        if direct_result.overflowed {
            region_representation_overflows = region_representation_overflows.saturating_add(1);
            visible_clip_fallback_count = visible_clip_fallback_count.saturating_add(1);
            visible_clip_fallbacks.push(EffectVisibleClipFallback {
                instance: instance.id,
                pass: None,
                input_rect_count: direct_result.input_rect_count,
                clip_rect_count: direct_result.clip_rect_count,
            });
        }
        let direct = direct_result.region;
        if direct.is_empty() {
            continue;
        }
        max_instance_region_rect_count = max_instance_region_rect_count.max(direct.rects().len());
        presentation_output_regions[instance_index] = direct.clone();

        let final_pass_index = final_pass_indices[instance_index];
        let final_pass = &graph.passes[final_pass_index];
        let final_output = unique_texture_index(graph, final_pass.output)
            .ok_or(EffectDemandRegionalFallbackReason::TextureLookupFailed)?;
        let final_output = &graph.textures[final_output];
        if !region_is_within_domain(&direct, final_output.domain) {
            return Err(EffectDemandRegionalFallbackReason::RegionOutsideDomain);
        }
        let seed_candidate = final_pass.damage.union(&direct);
        let seed_result =
            seed_candidate.intersect_bounded_within_result(&instance.output_influence_region);
        if seed_result.overflowed {
            region_representation_overflows = region_representation_overflows.saturating_add(1);
            visible_clip_fallback_count = visible_clip_fallback_count.saturating_add(1);
            visible_clip_fallbacks.push(EffectVisibleClipFallback {
                instance: instance.id,
                pass: Some(final_pass.id),
                input_rect_count: seed_result.input_rect_count,
                clip_rect_count: seed_result.clip_rect_count,
            });
        }
        if !region_is_within_domain(&seed_result.region, final_output.domain) {
            return Err(EffectDemandRegionalFallbackReason::RegionOutsideDomain);
        }
        let seed = seed_result.region;
        union_pass_region(
            &mut pass_regions,
            final_pass_index,
            &seed,
            final_output.domain,
            &mut work_region_bbox_coalesces,
        )?;
    }

    let mut causal_dependency_edges_considered = 0usize;
    let mut causal_dependency_propagations = 0usize;
    let mut causal_dependency_pruned_empty = 0usize;
    let mut pass_dependency_propagations = 0usize;
    let mut visited_texture_edges = Vec::new();

    // Checkpoint edges point from a later SceneCapture pass to an earlier
    // effect's final pass. Reverse graph order therefore processes each
    // dependency only after all of its later consumers have contributed.
    for pass_index in (0..graph.passes.len()).rev() {
        if pass_regions[pass_index].is_empty() {
            continue;
        }
        let pass = &graph.passes[pass_index];
        let output_index = unique_texture_index(graph, pass.output)
            .ok_or(EffectDemandRegionalFallbackReason::TextureLookupFailed)?;
        let output = &graph.textures[output_index];

        if full_kawase
            && matches!(
                pass.kind,
                RenderPassKind::DualKawaseDownsample | RenderPassKind::DualKawaseUpsample
            )
        {
            let full = super::render_graph::full_pass_region(graph, pass);
            if !region_is_within_domain(&full, output.domain) {
                return Err(EffectDemandRegionalFallbackReason::RegionOutsideDomain);
            }
            pass_regions[pass_index] = full;
        }

        if pass.kind == RenderPassKind::SceneCapture {
            for dependency_pass_id in &pass.checkpoint_dependencies {
                causal_dependency_edges_considered =
                    causal_dependency_edges_considered.saturating_add(1);
                let dependency_pass_index = unique_pass_index(graph, *dependency_pass_id)
                    .ok_or(EffectDemandRegionalFallbackReason::PassLookupFailed)?;
                if dependency_pass_index >= pass_index {
                    return Err(EffectDemandRegionalFallbackReason::DependencyOrderingInvalid);
                }
                let dependency_pass = &graph.passes[dependency_pass_index];
                let dependency_instance_index =
                    unique_instance_index(graph, dependency_pass.instance)
                        .ok_or(EffectDemandRegionalFallbackReason::GraphMetadataIncomplete)?;
                let dependency_instance = &graph.instances[dependency_instance_index];
                let required_result = pass_regions[pass_index]
                    .intersect_bounded_within_result(&dependency_instance.output_influence_region);
                if required_result.overflowed {
                    region_representation_overflows =
                        region_representation_overflows.saturating_add(1);
                    visible_clip_fallback_count = visible_clip_fallback_count.saturating_add(1);
                    visible_clip_fallbacks.push(EffectVisibleClipFallback {
                        instance: dependency_instance.id,
                        pass: Some(dependency_pass.id),
                        input_rect_count: required_result.input_rect_count,
                        clip_rect_count: required_result.clip_rect_count,
                    });
                }
                if required_result.region.is_empty() {
                    causal_dependency_pruned_empty =
                        causal_dependency_pruned_empty.saturating_add(1);
                    continue;
                }
                let final_pass_index = final_pass_indices[dependency_instance_index];
                if final_pass_index != dependency_pass_index {
                    return Err(EffectDemandRegionalFallbackReason::DependencyOrderingInvalid);
                }
                let dependency_output_index =
                    unique_texture_index(graph, dependency_pass.output)
                        .ok_or(EffectDemandRegionalFallbackReason::TextureLookupFailed)?;
                union_pass_region(
                    &mut pass_regions,
                    dependency_pass_index,
                    &required_result.region,
                    graph.textures[dependency_output_index].domain,
                    &mut work_region_bbox_coalesces,
                )?;
                causal_dependency_propagations = causal_dependency_propagations.saturating_add(1);
            }
            continue;
        }

        if pass.kind == RenderPassKind::SurfaceCapture {
            continue;
        }

        for input_id in &pass.inputs {
            let input_index = unique_texture_index(graph, Some(*input_id))
                .ok_or(EffectDemandRegionalFallbackReason::TextureLookupFailed)?;
            let input = &graph.textures[input_index];
            if matches!(
                input.source,
                GraphTextureSource::PrecomputedBackdrop | GraphTextureSource::Static(_)
            ) {
                continue;
            }
            let producer = producers
                .get(input_index)
                .copied()
                .ok_or(EffectDemandRegionalFallbackReason::TextureProducerAmbiguous)?;
            if !producer.valid {
                return Err(EffectDemandRegionalFallbackReason::TextureProducerAmbiguous);
            }
            let producer_index = producer
                .pass_index
                .ok_or(EffectDemandRegionalFallbackReason::TextureProducerAmbiguous)?;
            if producer_index >= pass_index
                || graph.passes[producer_index].instance != pass.instance
            {
                return Err(EffectDemandRegionalFallbackReason::DependencyOrderingInvalid);
            }
            if visited_texture_edges.contains(&(pass_index, producer_index)) {
                continue;
            }
            visited_texture_edges.push((pass_index, producer_index));
            let Some(required) =
                required_input_region(pass, &pass_regions[pass_index], output, input)
            else {
                return Err(EffectDemandRegionalFallbackReason::RequiredInputRegionUnavailable);
            };
            if required.is_empty() {
                continue;
            }
            pass_dependency_propagations = pass_dependency_propagations.saturating_add(1);
            union_pass_region(
                &mut pass_regions,
                producer_index,
                &required,
                input.domain,
                &mut work_region_bbox_coalesces,
            )?;
        }
    }

    let mut execution_region = EffectRegion::empty();
    let mut max_pass_region_rect_count = 0;
    let mut passes = Vec::new();
    for (pass_index, pass) in graph.passes.iter().enumerate() {
        let region = &pass_regions[pass_index];
        if region.is_empty() {
            continue;
        }
        max_pass_region_rect_count = max_pass_region_rect_count.max(region.rects().len());
        let (next, coalesced) = execution_region.union_with_diagnostics(region);
        execution_region = next;
        if coalesced {
            work_region_bbox_coalesces = work_region_bbox_coalesces.saturating_add(1);
        }
        passes.push(EffectPassExecutionDemand {
            id: pass.id,
            output_region: region.clone(),
        });
    }
    for final_pass_index in &final_pass_indices {
        max_instance_region_rect_count =
            max_instance_region_rect_count.max(pass_regions[*final_pass_index].rects().len());
    }

    let plan_stats = EffectDemandPlanStats {
        repair_rect_count: repair_region.rects().len(),
        dependency_edge_count: graph.instances.iter().fold(0, |count, instance| {
            count.saturating_add(instance.dependencies.len())
        }),
        dependency_propagations: causal_dependency_propagations,
        causal_dependency_edges_considered,
        causal_dependency_propagations,
        causal_dependency_pruned_empty,
        max_instance_region_rect_count,
        region_representation_overflows,
        visible_clip_fallbacks: visible_clip_fallback_count,
        work_region_bbox_coalesces,
        pass_dependency_propagations,
        max_pass_region_rect_count,
        ..EffectDemandPlanStats::default()
    };

    Ok(RegionalEffectDemandPlan {
        presentation_output_regions,
        pass_regions,
        final_pass_indices,
        passes,
        execution_region,
        visible_clip_fallbacks,
        plan_stats,
    })
}

fn validate_regional_graph(
    graph: &CompiledFrameGraph,
    producers: &[super::render_graph::TextureProducer],
) -> Result<Vec<usize>, EffectDemandRegionalFallbackReason> {
    for (index, pass) in graph.passes.iter().enumerate() {
        if graph.passes[..index]
            .iter()
            .any(|other| other.id == pass.id)
        {
            return Err(EffectDemandRegionalFallbackReason::PassLookupFailed);
        }
    }
    for (index, texture) in graph.textures.iter().enumerate() {
        if graph.textures[..index]
            .iter()
            .any(|other| other.id == texture.id)
        {
            return Err(EffectDemandRegionalFallbackReason::TextureLookupFailed);
        }
    }

    let mut final_pass_indices = vec![None; graph.instances.len()];
    for (pass_index, pass) in graph.passes.iter().enumerate() {
        let instance_index = unique_instance_index(graph, pass.instance)
            .ok_or(EffectDemandRegionalFallbackReason::GraphMetadataIncomplete)?;
        if matches!(
            pass.kind,
            RenderPassKind::Composite | RenderPassKind::OutputPostProcess
        ) {
            if final_pass_indices[instance_index]
                .replace(pass_index)
                .is_some()
            {
                return Err(EffectDemandRegionalFallbackReason::FinalPassMissing);
            }
        }
        if unique_texture_index(graph, pass.output).is_none() {
            return Err(EffectDemandRegionalFallbackReason::TextureLookupFailed);
        }
        if matches!(
            pass.kind,
            RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
        ) && !pass.inputs.is_empty()
        {
            return Err(EffectDemandRegionalFallbackReason::GraphMetadataIncomplete);
        }
        for input_id in &pass.inputs {
            let input_index = unique_texture_index(graph, Some(*input_id))
                .ok_or(EffectDemandRegionalFallbackReason::TextureLookupFailed)?;
            let input = &graph.textures[input_index];
            if matches!(
                input.source,
                GraphTextureSource::PrecomputedBackdrop | GraphTextureSource::Static(_)
            ) {
                continue;
            }
            let producer = producers
                .get(input_index)
                .copied()
                .ok_or(EffectDemandRegionalFallbackReason::TextureProducerAmbiguous)?;
            if !producer.valid {
                return Err(EffectDemandRegionalFallbackReason::TextureProducerAmbiguous);
            }
            let producer_index = producer
                .pass_index
                .ok_or(EffectDemandRegionalFallbackReason::TextureProducerAmbiguous)?;
            if producer_index >= pass_index
                || graph.passes[producer_index].instance != pass.instance
            {
                return Err(EffectDemandRegionalFallbackReason::DependencyOrderingInvalid);
            }
        }
        if pass.kind != RenderPassKind::SceneCapture && !pass.checkpoint_dependencies.is_empty() {
            return Err(EffectDemandRegionalFallbackReason::GraphMetadataIncomplete);
        }
        for (dependency_index, dependency_pass_id) in
            pass.checkpoint_dependencies.iter().enumerate()
        {
            if pass.checkpoint_dependencies[..dependency_index].contains(dependency_pass_id) {
                return Err(EffectDemandRegionalFallbackReason::GraphMetadataIncomplete);
            }
            let dependency_pass_index = unique_pass_index(graph, *dependency_pass_id)
                .ok_or(EffectDemandRegionalFallbackReason::PassLookupFailed)?;
            if dependency_pass_index >= pass_index {
                return Err(EffectDemandRegionalFallbackReason::DependencyOrderingInvalid);
            }
            let dependency_instance_index =
                unique_instance_index(graph, graph.passes[dependency_pass_index].instance)
                    .ok_or(EffectDemandRegionalFallbackReason::GraphMetadataIncomplete)?;
            if !matches!(
                graph.passes[dependency_pass_index].kind,
                RenderPassKind::Composite | RenderPassKind::OutputPostProcess
            ) || !graph.instances[instance_index]
                .dependencies
                .contains(&graph.instances[dependency_instance_index].id)
            {
                return Err(EffectDemandRegionalFallbackReason::DependencyOrderingInvalid);
            }
        }
    }

    final_pass_indices
        .into_iter()
        .map(|pass_index| pass_index.ok_or(EffectDemandRegionalFallbackReason::FinalPassMissing))
        .collect()
}

fn unique_pass_index(graph: &CompiledFrameGraph, id: GraphPassId) -> Option<usize> {
    let mut found = None;
    for (index, pass) in graph.passes.iter().enumerate() {
        if pass.id != id {
            continue;
        }
        if found.replace(index).is_some() {
            return None;
        }
    }
    found
}

fn unique_texture_index(graph: &CompiledFrameGraph, id: Option<GraphTextureId>) -> Option<usize> {
    let id = id?;
    let mut found = None;
    for (index, texture) in graph.textures.iter().enumerate() {
        if texture.id != id {
            continue;
        }
        if found.replace(index).is_some() {
            return None;
        }
    }
    found
}

fn region_is_within_domain(region: &EffectRegion, domain: super::EffectRect) -> bool {
    region.rects().iter().all(|rect| {
        rect.x >= domain.x
            && rect.y >= domain.y
            && rect.right() <= domain.right()
            && rect.bottom() <= domain.bottom()
    })
}

fn union_pass_region(
    pass_regions: &mut [EffectRegion],
    pass_index: usize,
    required: &EffectRegion,
    domain: super::EffectRect,
    work_region_bbox_coalesces: &mut usize,
) -> Result<(), EffectDemandRegionalFallbackReason> {
    if required.is_empty() {
        return Ok(());
    }
    if !region_is_within_domain(required, domain) {
        return Err(EffectDemandRegionalFallbackReason::RegionOutsideDomain);
    }
    let (next, coalesced) = pass_regions[pass_index].union_with_diagnostics(required);
    if coalesced {
        *work_region_bbox_coalesces = work_region_bbox_coalesces.saturating_add(1);
    }
    if !region_is_within_domain(&next, domain) {
        return Err(EffectDemandRegionalFallbackReason::RegionOutsideDomain);
    }
    pass_regions[pass_index] = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planner_policy_parser_defaults_and_normalizes_values() {
        for (input, expected) in [
            (None, EffectDemandPlannerPolicy::Legacy),
            (Some("legacy"), EffectDemandPlannerPolicy::Legacy),
            (Some("LEGACY"), EffectDemandPlannerPolicy::Legacy),
            (Some("regional"), EffectDemandPlannerPolicy::Regional),
            (Some(" regional "), EffectDemandPlannerPolicy::Regional),
        ] {
            let parsed = parse_effect_demand_planner_policy(input);
            assert_eq!(parsed.policy, expected);
            assert!(!parsed.unknown_value);
        }
    }

    #[test]
    fn planner_policy_parser_marks_unknown_values_and_defaults_to_legacy() {
        let parsed = parse_effect_demand_planner_policy(Some(" future "));
        assert_eq!(parsed.policy, EffectDemandPlannerPolicy::Legacy);
        assert!(parsed.unknown_value);
    }
}
