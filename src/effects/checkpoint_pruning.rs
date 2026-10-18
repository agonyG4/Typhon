use std::collections::HashSet;

use super::render_graph::{CompiledFrameGraph, CompiledRenderPass, GraphPassId, RenderPassKind};
use super::sample_support::{EffectSampleSupportError, derive_effect_sample_support};
use super::{EffectFrameDemand, EffectInstanceId};

/// Why an existing checkpoint dependency was kept or removed by G3a.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectCheckpointDependencyDecisionKind {
    RetainedSampledSupportIntersection,
    PrunedSampleSupportDisjoint,
    RetainedIneligibleTopology(EffectCheckpointDependencyIneligibility),
    RetainedUnresolved(EffectCheckpointDependencyUnresolvedReason),
}

/// One auditable decision for a logical checkpoint dependency.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectCheckpointDependencyDecision {
    pub consumer_instance_id: EffectInstanceId,
    pub producer_instance_id: Option<EffectInstanceId>,
    pub kind: EffectCheckpointDependencyDecisionKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectCheckpointDependencyIneligibility {
    SceneCaptureMissing,
    MultipleSceneCaptures,
    SurfaceCapturePresent,
    ContinuousDemand,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectCheckpointDependencyUnresolvedReason {
    ConsumerInstanceAmbiguous,
    ConsumerCapturePassAmbiguous,
    DependencyMetadataMismatch,
    DuplicateDependency,
    ProducerInstanceMissing(EffectInstanceId),
    ProducerInstanceAmbiguous(EffectInstanceId),
    ProducerNotEarlier(EffectInstanceId),
    ProducerFinalPassMissing(EffectInstanceId),
    ProducerFinalPassAmbiguous(EffectInstanceId),
    ProducerPassIdAmbiguous(GraphPassId),
    ProducerFinalPassNotEarlier(EffectInstanceId),
    PlanApplyFailed,
    SampleSupport(EffectSampleSupportError),
}

/// Accounting for all dependency slots considered by G3a.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EffectCheckpointDependencyPruningStats {
    pub analysis_performed: bool,
    pub edges_before: usize,
    pub edges_retained_intersecting: usize,
    pub edges_pruned: usize,
    pub edges_ineligible: usize,
    pub edges_unresolved: usize,
    pub edges_after: usize,
    pub consumers_became_dependency_free: usize,
    /// Edges preserved without analysis when the runtime/compile policy is off.
    pub edges_retained_without_analysis: usize,
}

impl EffectCheckpointDependencyPruningStats {
    pub fn for_policy_off(graph: &CompiledFrameGraph) -> Self {
        let edges = graph
            .instances
            .iter()
            .map(|instance| instance.dependencies.len())
            .sum();
        Self {
            edges_before: edges,
            edges_after: edges,
            edges_retained_without_analysis: edges,
            ..Self::default()
        }
    }

    pub fn accounting_is_valid(&self) -> bool {
        if !self.analysis_performed {
            return self.edges_before == self.edges_after
                && self.edges_retained_intersecting == 0
                && self.edges_pruned == 0
                && self.edges_ineligible == 0
                && self.edges_unresolved == 0
                && self.edges_retained_without_analysis == self.edges_before;
        }
        self.edges_before
            == self.edges_retained_intersecting
                + self.edges_pruned
                + self.edges_ineligible
                + self.edges_unresolved
            && self.edges_after == self.edges_before.saturating_sub(self.edges_pruned)
            && self.edges_retained_without_analysis == 0
    }
}

/// Immutable decisions and validated dependency replacements for one graph.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EffectCheckpointDependencyPruningPlan {
    pub decisions: Vec<EffectCheckpointDependencyDecision>,
    pub stats: EffectCheckpointDependencyPruningStats,
    mutations: Vec<ConsumerDependencyMutation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConsumerDependencyMutation {
    consumer_instance_id: EffectInstanceId,
    consumer_capture_pass_id: GraphPassId,
    original_dependencies: Vec<EffectInstanceId>,
    original_checkpoint_dependencies: Vec<GraphPassId>,
    retained_dependencies: Vec<EffectInstanceId>,
    retained_checkpoint_dependencies: Vec<GraphPassId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectCheckpointDependencyPruningApplyError {
    InvalidPlanAccounting,
    GraphChangedSincePlanning,
}

/// Derive a complete G3a plan without changing `graph`.
pub fn plan_effect_checkpoint_dependency_pruning(
    graph: &CompiledFrameGraph,
) -> EffectCheckpointDependencyPruningPlan {
    let mut decisions = Vec::new();
    let mut mutations = Vec::new();
    let mut seen_consumers = HashSet::new();
    let mut consumers_became_dependency_free = 0usize;

    for instance in &graph.instances {
        if !seen_consumers.insert(instance.id) {
            continue;
        }

        let instance_entries = graph
            .instances
            .iter()
            .filter(|candidate| candidate.id == instance.id)
            .collect::<Vec<_>>();
        let capture_passes = graph
            .passes
            .iter()
            .filter(|pass| {
                pass.instance == instance.id && pass.kind == RenderPassKind::SceneCapture
            })
            .collect::<Vec<_>>();
        let surface_capture_exists = graph.passes.iter().any(|pass| {
            pass.instance == instance.id && pass.kind == RenderPassKind::SurfaceCapture
        });
        let instance_dependencies = instance_entries
            .iter()
            .flat_map(|entry| entry.dependencies.iter().copied())
            .collect::<Vec<_>>();
        let max_checkpoint_dependencies = capture_passes
            .iter()
            .map(|pass| pass.checkpoint_dependencies.len())
            .max()
            .unwrap_or(0);
        let edge_slots = instance_dependencies.len().max(max_checkpoint_dependencies);
        if edge_slots == 0 {
            continue;
        }

        if instance_entries.len() != 1 {
            push_unresolved_decisions(
                &mut decisions,
                instance.id,
                edge_slots,
                &instance_dependencies,
                &capture_passes,
                graph,
                EffectCheckpointDependencyUnresolvedReason::ConsumerInstanceAmbiguous,
            );
            continue;
        }

        let ineligibility = if instance.frame_demand == EffectFrameDemand::Continuous {
            Some(EffectCheckpointDependencyIneligibility::ContinuousDemand)
        } else if surface_capture_exists {
            Some(EffectCheckpointDependencyIneligibility::SurfaceCapturePresent)
        } else if capture_passes.is_empty() {
            Some(EffectCheckpointDependencyIneligibility::SceneCaptureMissing)
        } else if capture_passes.len() != 1 {
            Some(EffectCheckpointDependencyIneligibility::MultipleSceneCaptures)
        } else {
            None
        };
        if let Some(reason) = ineligibility {
            push_ineligible_decisions(
                &mut decisions,
                instance.id,
                edge_slots,
                &instance_dependencies,
                &capture_passes,
                graph,
                reason,
            );
            continue;
        }

        let capture_pass = capture_passes[0];
        if graph
            .passes
            .iter()
            .filter(|pass| pass.id == capture_pass.id)
            .count()
            != 1
        {
            push_unresolved_decisions(
                &mut decisions,
                instance.id,
                edge_slots,
                &instance_dependencies,
                &capture_passes,
                graph,
                EffectCheckpointDependencyUnresolvedReason::ConsumerCapturePassAmbiguous,
            );
            continue;
        }

        let dependency_passes = match resolve_consumer_dependencies(graph, instance, capture_pass) {
            Ok(dependencies) => dependencies,
            Err(reason) => {
                push_unresolved_decisions(
                    &mut decisions,
                    instance.id,
                    edge_slots,
                    &instance_dependencies,
                    &capture_passes,
                    graph,
                    reason,
                );
                continue;
            }
        };

        // G2 support is derived exactly once for this eligible consumer.
        let support = match derive_effect_sample_support(graph, instance.id) {
            Ok(support) => support,
            Err(error) => {
                push_unresolved_decisions(
                    &mut decisions,
                    instance.id,
                    edge_slots,
                    &instance_dependencies,
                    &capture_passes,
                    graph,
                    EffectCheckpointDependencyUnresolvedReason::SampleSupport(error),
                );
                continue;
            }
        };

        let Some(resolved_producers) = instance
            .dependencies
            .iter()
            .map(|producer_id| {
                graph
                    .instances
                    .iter()
                    .find(|candidate| candidate.id == *producer_id)
            })
            .collect::<Option<Vec<_>>>()
        else {
            push_unresolved_decisions(
                &mut decisions,
                instance.id,
                edge_slots,
                &instance_dependencies,
                &capture_passes,
                graph,
                EffectCheckpointDependencyUnresolvedReason::DependencyMetadataMismatch,
            );
            continue;
        };

        let mut retained_dependencies = Vec::with_capacity(instance.dependencies.len());
        let mut retained_checkpoint_dependencies = Vec::with_capacity(dependency_passes.len());
        let mut pruned_for_consumer = 0usize;
        for ((&producer_id, &producer_pass_id), producer) in instance
            .dependencies
            .iter()
            .zip(dependency_passes.iter())
            .zip(resolved_producers)
        {
            if producer
                .output_influence_region
                .intersects(&support.backdrop_region)
            {
                retained_dependencies.push(producer_id);
                retained_checkpoint_dependencies.push(producer_pass_id);
                decisions.push(EffectCheckpointDependencyDecision {
                    consumer_instance_id: instance.id,
                    producer_instance_id: Some(producer_id),
                    kind:
                        EffectCheckpointDependencyDecisionKind::RetainedSampledSupportIntersection,
                });
            } else {
                pruned_for_consumer += 1;
                decisions.push(EffectCheckpointDependencyDecision {
                    consumer_instance_id: instance.id,
                    producer_instance_id: Some(producer_id),
                    kind: EffectCheckpointDependencyDecisionKind::PrunedSampleSupportDisjoint,
                });
            }
        }

        if pruned_for_consumer != 0 {
            if retained_dependencies.is_empty() {
                consumers_became_dependency_free += 1;
            }
            mutations.push(ConsumerDependencyMutation {
                consumer_instance_id: instance.id,
                consumer_capture_pass_id: capture_pass.id,
                original_dependencies: instance.dependencies.clone(),
                original_checkpoint_dependencies: capture_pass.checkpoint_dependencies.clone(),
                retained_dependencies,
                retained_checkpoint_dependencies,
            });
        }
    }

    // Count SceneCapture-only orphan metadata as unresolved edge slots too.
    for pass in graph.passes.iter().filter(|pass| {
        pass.kind == RenderPassKind::SceneCapture && !pass.checkpoint_dependencies.is_empty()
    }) {
        if seen_consumers.contains(&pass.instance) {
            continue;
        }
        for checkpoint_dependency in &pass.checkpoint_dependencies {
            decisions.push(EffectCheckpointDependencyDecision {
                consumer_instance_id: pass.instance,
                producer_instance_id: resolve_producer_for_checkpoint(
                    graph,
                    *checkpoint_dependency,
                ),
                kind: EffectCheckpointDependencyDecisionKind::RetainedUnresolved(
                    EffectCheckpointDependencyUnresolvedReason::DependencyMetadataMismatch,
                ),
            });
        }
    }

    let mut stats = EffectCheckpointDependencyPruningStats {
        analysis_performed: true,
        consumers_became_dependency_free,
        ..EffectCheckpointDependencyPruningStats::default()
    };
    stats.edges_before = decisions.len();
    for decision in &decisions {
        match &decision.kind {
            EffectCheckpointDependencyDecisionKind::RetainedSampledSupportIntersection => {
                stats.edges_retained_intersecting += 1;
            }
            EffectCheckpointDependencyDecisionKind::PrunedSampleSupportDisjoint => {
                stats.edges_pruned += 1;
            }
            EffectCheckpointDependencyDecisionKind::RetainedIneligibleTopology(_) => {
                stats.edges_ineligible += 1;
            }
            EffectCheckpointDependencyDecisionKind::RetainedUnresolved(_) => {
                stats.edges_unresolved += 1;
            }
        }
    }
    stats.edges_after = stats.edges_before.saturating_sub(stats.edges_pruned);

    EffectCheckpointDependencyPruningPlan {
        decisions,
        stats,
        mutations,
    }
}

/// Apply a previously validated plan only if every planned graph snapshot still matches.
/// All mutations are preflighted before either dependency representation changes.
pub fn apply_effect_checkpoint_dependency_pruning(
    graph: &mut CompiledFrameGraph,
    plan: &EffectCheckpointDependencyPruningPlan,
) -> Result<(), EffectCheckpointDependencyPruningApplyError> {
    if !plan.stats.accounting_is_valid()
        || plan.stats.analysis_performed
            != (!plan.decisions.is_empty() || plan.stats.edges_before == 0)
        || plan.decisions.len() != plan.stats.edges_before
        || plan
            .mutations
            .iter()
            .map(|mutation| {
                mutation
                    .original_dependencies
                    .len()
                    .saturating_sub(mutation.retained_dependencies.len())
            })
            .sum::<usize>()
            != plan.stats.edges_pruned
    {
        return Err(EffectCheckpointDependencyPruningApplyError::InvalidPlanAccounting);
    }

    let mut mutation_indices = Vec::with_capacity(plan.mutations.len());
    for mutation in &plan.mutations {
        let instance_indices = graph
            .instances
            .iter()
            .enumerate()
            .filter(|(_, instance)| instance.id == mutation.consumer_instance_id)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let capture_indices = graph
            .passes
            .iter()
            .enumerate()
            .filter(|(_, pass)| {
                pass.id == mutation.consumer_capture_pass_id
                    && pass.instance == mutation.consumer_instance_id
                    && pass.kind == RenderPassKind::SceneCapture
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if instance_indices.len() != 1 || capture_indices.len() != 1 {
            return Err(EffectCheckpointDependencyPruningApplyError::GraphChangedSincePlanning);
        }
        let instance_index = instance_indices[0];
        let capture_index = capture_indices[0];
        if graph.instances[instance_index].dependencies != mutation.original_dependencies
            || graph.passes[capture_index].checkpoint_dependencies
                != mutation.original_checkpoint_dependencies
            || mutation.retained_dependencies.len()
                != mutation.retained_checkpoint_dependencies.len()
        {
            return Err(EffectCheckpointDependencyPruningApplyError::GraphChangedSincePlanning);
        }
        mutation_indices.push((instance_index, capture_index));
    }

    for (mutation, (instance_index, capture_index)) in plan.mutations.iter().zip(mutation_indices) {
        graph.instances[instance_index].dependencies = mutation.retained_dependencies.clone();
        graph.passes[capture_index].checkpoint_dependencies =
            mutation.retained_checkpoint_dependencies.clone();
    }
    Ok(())
}

/// Return a no-analysis plan for the policy-off path, preserving its edge count.
pub fn disabled_pruning_plan(
    graph: Option<&CompiledFrameGraph>,
) -> EffectCheckpointDependencyPruningPlan {
    let mut plan = EffectCheckpointDependencyPruningPlan::default();
    if let Some(graph) = graph {
        plan.stats = EffectCheckpointDependencyPruningStats::for_policy_off(graph);
    }
    plan
}

/// Apply a pruning plan and convert an unexpected stale-plan rejection into a
/// fully retained, explicitly unresolved plan for telemetry.
pub fn apply_checkpoint_dependency_pruning_plan(
    graph: &mut CompiledFrameGraph,
    mut plan: EffectCheckpointDependencyPruningPlan,
) -> EffectCheckpointDependencyPruningPlan {
    if apply_effect_checkpoint_dependency_pruning(graph, &plan).is_ok() {
        return plan;
    }

    let edges_before = plan.stats.edges_before;
    for decision in &mut plan.decisions {
        decision.kind = EffectCheckpointDependencyDecisionKind::RetainedUnresolved(
            EffectCheckpointDependencyUnresolvedReason::PlanApplyFailed,
        );
    }
    plan.mutations.clear();
    plan.stats = EffectCheckpointDependencyPruningStats {
        analysis_performed: true,
        edges_before,
        edges_unresolved: edges_before,
        edges_after: edges_before,
        ..EffectCheckpointDependencyPruningStats::default()
    };
    plan
}

fn resolve_consumer_dependencies(
    graph: &CompiledFrameGraph,
    consumer: &super::render_graph::CompiledEffectInstance,
    consumer_capture: &CompiledRenderPass,
) -> Result<Vec<GraphPassId>, EffectCheckpointDependencyUnresolvedReason> {
    if consumer.dependencies.len() != consumer_capture.checkpoint_dependencies.len() {
        return Err(EffectCheckpointDependencyUnresolvedReason::DependencyMetadataMismatch);
    }
    let mut seen_instances = HashSet::new();
    if consumer
        .dependencies
        .iter()
        .any(|dependency| !seen_instances.insert(*dependency))
    {
        return Err(EffectCheckpointDependencyUnresolvedReason::DuplicateDependency);
    }
    let mut seen_passes = HashSet::new();
    if consumer_capture
        .checkpoint_dependencies
        .iter()
        .any(|dependency| !seen_passes.insert(*dependency))
    {
        return Err(EffectCheckpointDependencyUnresolvedReason::DuplicateDependency);
    }

    let consumer_index = graph
        .instances
        .iter()
        .position(|instance| instance.id == consumer.id)
        .ok_or(EffectCheckpointDependencyUnresolvedReason::ConsumerInstanceAmbiguous)?;
    let capture_index = graph
        .passes
        .iter()
        .position(|pass| pass.id == consumer_capture.id)
        .ok_or(EffectCheckpointDependencyUnresolvedReason::ConsumerCapturePassAmbiguous)?;

    let mut result = Vec::with_capacity(consumer.dependencies.len());
    for (&producer_id, &capture_dependency) in consumer
        .dependencies
        .iter()
        .zip(consumer_capture.checkpoint_dependencies.iter())
    {
        let producers = graph
            .instances
            .iter()
            .enumerate()
            .filter(|(_, instance)| instance.id == producer_id)
            .collect::<Vec<_>>();
        let [(producer_index, _)] = producers.as_slice() else {
            return Err(if producers.is_empty() {
                EffectCheckpointDependencyUnresolvedReason::ProducerInstanceMissing(producer_id)
            } else {
                EffectCheckpointDependencyUnresolvedReason::ProducerInstanceAmbiguous(producer_id)
            });
        };
        if *producer_index >= consumer_index {
            return Err(
                EffectCheckpointDependencyUnresolvedReason::ProducerNotEarlier(producer_id),
            );
        }

        let final_passes = graph
            .passes
            .iter()
            .enumerate()
            .filter(|(_, pass)| {
                pass.instance == producer_id
                    && matches!(
                        pass.kind,
                        RenderPassKind::Composite | RenderPassKind::OutputPostProcess
                    )
            })
            .collect::<Vec<_>>();
        let [(producer_pass_index, producer_pass)] = final_passes.as_slice() else {
            return Err(if final_passes.is_empty() {
                EffectCheckpointDependencyUnresolvedReason::ProducerFinalPassMissing(producer_id)
            } else {
                EffectCheckpointDependencyUnresolvedReason::ProducerFinalPassAmbiguous(producer_id)
            });
        };
        if graph
            .passes
            .iter()
            .filter(|pass| pass.id == producer_pass.id)
            .count()
            != 1
        {
            return Err(
                EffectCheckpointDependencyUnresolvedReason::ProducerPassIdAmbiguous(
                    producer_pass.id,
                ),
            );
        }
        if *producer_pass_index >= capture_index {
            return Err(
                EffectCheckpointDependencyUnresolvedReason::ProducerFinalPassNotEarlier(
                    producer_id,
                ),
            );
        }
        if producer_pass.id != capture_dependency {
            return Err(EffectCheckpointDependencyUnresolvedReason::DependencyMetadataMismatch);
        }
        result.push(producer_pass.id);
    }
    Ok(result)
}

fn push_ineligible_decisions(
    decisions: &mut Vec<EffectCheckpointDependencyDecision>,
    consumer_id: EffectInstanceId,
    edge_slots: usize,
    instance_dependencies: &[EffectInstanceId],
    capture_passes: &[&CompiledRenderPass],
    graph: &CompiledFrameGraph,
    reason: EffectCheckpointDependencyIneligibility,
) {
    for index in 0..edge_slots {
        let producer_id = instance_dependencies
            .get(index)
            .copied()
            .or_else(|| checkpoint_producer_at(capture_passes, index, graph));
        decisions.push(EffectCheckpointDependencyDecision {
            consumer_instance_id: consumer_id,
            producer_instance_id: producer_id,
            kind: EffectCheckpointDependencyDecisionKind::RetainedIneligibleTopology(
                reason.clone(),
            ),
        });
    }
}

fn push_unresolved_decisions(
    decisions: &mut Vec<EffectCheckpointDependencyDecision>,
    consumer_id: EffectInstanceId,
    edge_slots: usize,
    instance_dependencies: &[EffectInstanceId],
    capture_passes: &[&CompiledRenderPass],
    graph: &CompiledFrameGraph,
    reason: EffectCheckpointDependencyUnresolvedReason,
) {
    for index in 0..edge_slots {
        let producer_id = instance_dependencies
            .get(index)
            .copied()
            .or_else(|| checkpoint_producer_at(capture_passes, index, graph));
        decisions.push(EffectCheckpointDependencyDecision {
            consumer_instance_id: consumer_id,
            producer_instance_id: producer_id,
            kind: EffectCheckpointDependencyDecisionKind::RetainedUnresolved(reason.clone()),
        });
    }
}

fn checkpoint_producer_at(
    capture_passes: &[&CompiledRenderPass],
    dependency_index: usize,
    graph: &CompiledFrameGraph,
) -> Option<EffectInstanceId> {
    let checkpoint_id = capture_passes
        .iter()
        .find_map(|pass| pass.checkpoint_dependencies.get(dependency_index).copied())?;
    resolve_producer_for_checkpoint(graph, checkpoint_id)
}

fn resolve_producer_for_checkpoint(
    graph: &CompiledFrameGraph,
    checkpoint_id: GraphPassId,
) -> Option<EffectInstanceId> {
    let mut producer = None;
    for pass in graph.passes.iter().filter(|pass| {
        pass.id == checkpoint_id
            && matches!(
                pass.kind,
                RenderPassKind::Composite | RenderPassKind::OutputPostProcess
            )
    }) {
        if producer.replace(pass.instance).is_some() {
            return None;
        }
    }
    producer
}
