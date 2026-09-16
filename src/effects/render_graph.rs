use std::{fmt::Write as _, num::NonZeroU16};

use crate::compositor::{
    EffectAnchor, EffectAnchorScope, ResolvedEffectInstance, ResolvedEffectScene, VisualGroupId,
};

use super::registry::EffectRegistry;
use super::{
    BUILTIN_EFFECT_PROGRAM_ID, DualKawaseBlurSpec, EffectAlphaMode, EffectFailurePolicy,
    EffectFrameDemand, EffectInstanceId, EffectNode, EffectNodeId, EffectNodeKind, EffectOutsets,
    EffectProgram, EffectProgramId, EffectRect, EffectRegion, EffectRegionClipFallback,
    EffectSource, EffectValidationError, EffectWorkingSpace, MAX_EFFECT_PROGRAM_NODES,
    ValidatedEffectProgram, plan_effect_damage, validate_effect_program,
};

pub const MAX_GRAPH_TEXTURES: usize = 4096;
pub const MAX_GRAPH_PASSES: usize = 4096;
pub const BUILTIN_BACKGROUND_BLUR_NAME: &str = "system.background_blur";

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PeakLiveWorkCounters {
    pass_position_map_entries: usize,
    interval_insertions: usize,
    sweep_steps: usize,
    graph_compiles: usize,
    instance_compiles: usize,
    node_visits: usize,
    program_lookup_map_builds: usize,
    output_map_builds: usize,
}

#[cfg(test)]
thread_local! {
    static PEAK_LIVE_WORK_COUNTERS: std::cell::Cell<PeakLiveWorkCounters> =
        const {
            std::cell::Cell::new(PeakLiveWorkCounters {
                pass_position_map_entries: 0,
                interval_insertions: 0,
                sweep_steps: 0,
                graph_compiles: 0,
                instance_compiles: 0,
                node_visits: 0,
                program_lookup_map_builds: 0,
                output_map_builds: 0,
            })
        };
}

#[cfg(test)]
thread_local! {
    static CHECKPOINT_INTERSECTION_CHECKS: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn reset_peak_live_work_counters() {
    PEAK_LIVE_WORK_COUNTERS.with(|counters| counters.set(PeakLiveWorkCounters::default()));
    CHECKPOINT_INTERSECTION_CHECKS.with(|checks| checks.set(0));
}

#[cfg(test)]
fn peak_live_work_counters() -> PeakLiveWorkCounters {
    PEAK_LIVE_WORK_COUNTERS.with(std::cell::Cell::get)
}

#[cfg(test)]
fn note_peak_live_interval_insertion() {
    PEAK_LIVE_WORK_COUNTERS.with(|counters| {
        let mut value = counters.get();
        value.interval_insertions += 1;
        counters.set(value);
    });
}

#[cfg(test)]
fn note_peak_live_pass_position_map_entry() {
    PEAK_LIVE_WORK_COUNTERS.with(|counters| {
        let mut value = counters.get();
        value.pass_position_map_entries += 1;
        counters.set(value);
    });
}

#[cfg(test)]
fn note_peak_live_sweep_step() {
    PEAK_LIVE_WORK_COUNTERS.with(|counters| {
        let mut value = counters.get();
        value.sweep_steps += 1;
        counters.set(value);
    });
}

#[cfg(test)]
fn note_graph_compile() {
    PEAK_LIVE_WORK_COUNTERS.with(|counters| {
        let mut value = counters.get();
        value.graph_compiles += 1;
        counters.set(value);
    });
}

#[cfg(test)]
fn note_instance_compile() {
    PEAK_LIVE_WORK_COUNTERS.with(|counters| {
        let mut value = counters.get();
        value.instance_compiles += 1;
        counters.set(value);
    });
}

#[cfg(test)]
fn note_node_visit() {
    PEAK_LIVE_WORK_COUNTERS.with(|counters| {
        let mut value = counters.get();
        value.node_visits += 1;
        counters.set(value);
    });
}

#[cfg(test)]
fn note_checkpoint_intersection_check() {
    CHECKPOINT_INTERSECTION_CHECKS.with(|checks| checks.set(checks.get() + 1));
}

#[cfg(test)]
fn checkpoint_intersection_checks() -> usize {
    CHECKPOINT_INTERSECTION_CHECKS.with(std::cell::Cell::get)
}

pub fn builtin_background_blur_program_id() -> EffectProgramId {
    EffectProgramId::new(BUILTIN_EFFECT_PROGRAM_ID).expect("builtin effect program id is non-zero")
}

pub fn builtin_background_blur_program() -> ValidatedEffectProgram {
    let source = EffectNodeId::new(1).expect("builtin source node id is non-zero");
    let blur = EffectNodeId::new(2).expect("builtin blur node id is non-zero");
    validate_effect_program(EffectProgram {
        id: builtin_background_blur_program_id(),
        nodes: vec![
            EffectNode::source(source, EffectSource::Backdrop),
            EffectNode::dual_kawase(
                blur,
                source,
                DualKawaseBlurSpec::new(4.0, 2, 1.0)
                    .expect("builtin blur specification must validate"),
            ),
        ],
        output: blur,
        working_space: EffectWorkingSpace::LinearSrgb,
        alpha_mode: EffectAlphaMode::Opaque,
        outsets: EffectOutsets::ZERO,
        frame_demand: EffectFrameDemand::OnDamage,
        failure_policy: EffectFailurePolicy::Passthrough,
    })
    .expect("builtin background blur program must validate")
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GraphTextureId(NonZeroU16);

impl GraphTextureId {
    pub fn new(value: u16) -> Option<Self> {
        NonZeroU16::new(value).map(Self)
    }

    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GraphPassId(NonZeroU16);

impl GraphPassId {
    pub fn new(value: u16) -> Option<Self> {
        NonZeroU16::new(value).map(Self)
    }

    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphTextureSource {
    Output,
    CapturedScene,
    CapturedTarget,
    Intermediate,
    Static(super::StaticTextureId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphTextureOrigin {
    BottomLeft,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphTexturePlan {
    pub id: GraphTextureId,
    pub source: GraphTextureSource,
    pub width: u32,
    pub height: u32,
    pub domain: EffectRect,
    pub working_space: EffectWorkingSpace,
    pub origin: GraphTextureOrigin,
    pub first_use: Option<GraphPassId>,
    pub last_use: Option<GraphPassId>,
}

/// Physical texel coverage in logical-top texture coordinates.
///
/// The rectangle is half-open: `[left, right) x [top, bottom)`. It deliberately
/// does not encode framebuffer origin or OpenGL's Y direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GraphTexturePhysicalRect {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl GraphTexturePhysicalRect {
    pub const fn width(self) -> u32 {
        self.right.saturating_sub(self.left)
    }

    pub const fn height(self) -> u32 {
        self.bottom.saturating_sub(self.top)
    }

    pub const fn contains(self, x: u32, y: u32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

fn div_round_up(numerator: u128, denominator: u128) -> u128 {
    numerator.saturating_add(denominator.saturating_sub(1)) / denominator
}

fn logical_edge_to_physical(
    local: u32,
    physical_size: u32,
    logical_size: u32,
    round_up: bool,
) -> u32 {
    let numerator = u128::from(local) * u128::from(physical_size);
    let denominator = u128::from(logical_size);
    let value = if round_up {
        div_round_up(numerator, denominator)
    } else {
        numerator / denominator
    };
    u32::try_from(value).expect("graph texture coverage fits its physical dimension")
}

/// Converts a logical damage rectangle to the physical texels the fullscreen
/// executor rasterizes before any framebuffer-origin Y conversion.
pub fn logical_rect_to_physical_coverage(
    rect: EffectRect,
    texture: &GraphTexturePlan,
) -> Option<GraphTexturePhysicalRect> {
    if texture.width == 0
        || texture.height == 0
        || texture.domain.width == 0
        || texture.domain.height == 0
    {
        return None;
    }
    let clipped = rect.intersect(texture.domain)?;
    let local_left = u32::try_from(i64::from(clipped.x) - i64::from(texture.domain.x)).ok()?;
    let local_top = u32::try_from(i64::from(clipped.y) - i64::from(texture.domain.y)).ok()?;
    let local_right =
        u32::try_from(i64::from(clipped.right()) - i64::from(texture.domain.x)).ok()?;
    let local_bottom =
        u32::try_from(i64::from(clipped.bottom()) - i64::from(texture.domain.y)).ok()?;
    let coverage = GraphTexturePhysicalRect {
        left: logical_edge_to_physical(local_left, texture.width, texture.domain.width, false),
        top: logical_edge_to_physical(local_top, texture.height, texture.domain.height, false),
        right: logical_edge_to_physical(local_right, texture.width, texture.domain.width, true),
        bottom: logical_edge_to_physical(local_bottom, texture.height, texture.domain.height, true),
    };
    (coverage.width() > 0 && coverage.height() > 0).then_some(coverage)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderPassKind {
    SceneCapture,
    SurfaceCapture,
    NormalizeInput,
    DualKawaseDownsample,
    DualKawaseUpsample,
    Fragment,
    Blend,
    Mask,
    Composite,
    OutputPostProcess,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EffectColorConversion {
    #[default]
    None,
    DecodeSrgbToLinear,
    EncodeLinearToSrgb,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompiledRenderPass {
    pub id: GraphPassId,
    pub kind: RenderPassKind,
    pub inputs: Vec<GraphTextureId>,
    pub output: Option<GraphTextureId>,
    pub damage: EffectRegion,
    pub instance: EffectInstanceId,
    pub anchor: EffectAnchor,
    pub blur_radius: Option<f32>,
    pub stage: Option<EffectNodeKind>,
    pub fused_stages: Vec<EffectNodeKind>,
    pub parameter_block: super::EffectParameterBlock,
    pub alpha_mode: EffectAlphaMode,
    pub encode_output: bool,
    pub color_conversion: EffectColorConversion,
    pub checkpoint_dependencies: Vec<GraphPassId>,
    pub visual_group: Option<VisualGroupId>,
    pub anchor_scope: EffectAnchorScope,
    pub visible_clip_fallback: Option<(usize, usize)>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenderGraphCompileStats {
    pub effect_instances: usize,
    pub passes: usize,
    pub textures: usize,
    pub intermediate_textures: usize,
    pub peak_live_intermediates: usize,
    pub region_representation_overflows: usize,
    pub visible_clip_fallbacks: usize,
    pub work_region_bbox_coalesces: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompiledFrameGraph {
    pub passes: Vec<CompiledRenderPass>,
    pub textures: Vec<GraphTexturePlan>,
    pub instances: Vec<CompiledEffectInstance>,
    pub final_damage: EffectRegion,
    pub stats: RenderGraphCompileStats,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompiledEffectInstance {
    pub id: EffectInstanceId,
    pub output_influence_region: EffectRegion,
    pub capture_region: EffectRegion,
    pub dependencies: Vec<EffectInstanceId>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EffectDemandPlanStats {
    pub repair_rect_count: usize,
    pub dependency_edge_count: usize,
    pub dependency_propagations: usize,
    pub max_instance_region_rect_count: usize,
    pub region_representation_overflows: usize,
    pub visible_clip_fallbacks: usize,
    pub work_region_bbox_coalesces: usize,
    pub conservative_full: bool,
    pub pass_count_selected: usize,
    pub partial_pass_count: usize,
    pub full_domain_pass_count: usize,
    pub pass_dependency_propagations: usize,
    pub max_pass_region_rect_count: usize,
    pub pass_conservative_fallbacks: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectInstanceExecutionDemand {
    pub id: EffectInstanceId,
    pub output_region: EffectRegion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectVisibleClipFallback {
    pub instance: EffectInstanceId,
    pub pass: Option<GraphPassId>,
    pub input_rect_count: usize,
    pub clip_rect_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectPassExecutionDemand {
    pub id: GraphPassId,
    pub output_region: EffectRegion,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectExecutionDemand {
    pub instances: Vec<EffectInstanceExecutionDemand>,
    pub execution_region: EffectRegion,
    pub passes: Vec<EffectPassExecutionDemand>,
    pub visible_clip_fallbacks: Vec<EffectVisibleClipFallback>,
    pub(crate) conservative_full: bool,
    conservative_instances: Vec<EffectInstanceId>,
    plan_stats: EffectDemandPlanStats,
}

impl EffectExecutionDemand {
    pub fn new(
        instances: Vec<EffectInstanceExecutionDemand>,
        execution_region: EffectRegion,
    ) -> Self {
        Self {
            instances,
            execution_region,
            passes: Vec::new(),
            visible_clip_fallbacks: Vec::new(),
            conservative_full: false,
            conservative_instances: Vec::new(),
            plan_stats: EffectDemandPlanStats::default(),
        }
    }

    pub fn contains(&self, id: EffectInstanceId) -> bool {
        self.instances.iter().any(|instance| instance.id == id)
    }

    pub fn is_conservative_full(&self) -> bool {
        self.conservative_full
    }

    pub fn output_region(&self, id: EffectInstanceId) -> Option<&EffectRegion> {
        self.instances
            .iter()
            .find(|instance| instance.id == id)
            .map(|instance| &instance.output_region)
    }

    pub fn pass_output_region(&self, id: GraphPassId) -> Option<&EffectRegion> {
        self.passes
            .iter()
            .find(|pass| pass.id == id)
            .map(|pass| &pass.output_region)
    }

    pub fn has_pass_plan(&self) -> bool {
        !self.passes.is_empty()
    }

    pub fn instance_is_conservative_full(&self, id: EffectInstanceId) -> bool {
        self.conservative_instances.contains(&id)
    }

    pub fn plan_stats(&self) -> EffectDemandPlanStats {
        self.plan_stats
    }

    pub fn visible_clip_fallbacks(&self) -> &[EffectVisibleClipFallback] {
        &self.visible_clip_fallbacks
    }
}

fn all_visible_instances_with_output_regions(
    graph: &CompiledFrameGraph,
    repair_rect_count: usize,
) -> EffectExecutionDemand {
    let mut execution_region = EffectRegion::empty();
    let mut max_instance_region_rect_count = 0;
    let mut work_region_bbox_coalesces = graph.stats.work_region_bbox_coalesces;
    let instances = graph
        .instances
        .iter()
        .map(|instance| {
            max_instance_region_rect_count =
                max_instance_region_rect_count.max(instance.output_influence_region.rects().len());
            let (next_execution_region, coalesced) =
                execution_region.union_with_diagnostics(&instance.output_influence_region);
            execution_region = next_execution_region;
            if coalesced {
                work_region_bbox_coalesces = work_region_bbox_coalesces.saturating_add(1);
            }
            if !instance.dependencies.is_empty() {
                let (next_execution_region, coalesced) =
                    execution_region.union_with_diagnostics(&instance.capture_region);
                execution_region = next_execution_region;
                if coalesced {
                    work_region_bbox_coalesces = work_region_bbox_coalesces.saturating_add(1);
                }
            }
            EffectInstanceExecutionDemand {
                id: instance.id,
                output_region: instance.output_influence_region.clone(),
            }
        })
        .collect::<Vec<_>>();
    let passes = conservative_pass_demands(graph, &instances);
    let mut demand = EffectExecutionDemand {
        instances,
        execution_region,
        passes,
        visible_clip_fallbacks: Vec::new(),
        conservative_full: true,
        conservative_instances: Vec::new(),
        plan_stats: EffectDemandPlanStats {
            repair_rect_count,
            dependency_edge_count: dependency_edge_count(graph),
            dependency_propagations: 0,
            max_instance_region_rect_count,
            region_representation_overflows: graph.stats.region_representation_overflows,
            visible_clip_fallbacks: graph.stats.visible_clip_fallbacks,
            work_region_bbox_coalesces,
            conservative_full: true,
            ..EffectDemandPlanStats::default()
        },
    };
    update_pass_plan_stats(
        graph,
        &demand.passes,
        &demand.conservative_instances,
        &mut demand.plan_stats,
    );
    demand
}

fn unique_instance_index(graph: &CompiledFrameGraph, id: EffectInstanceId) -> Option<usize> {
    let mut found = None;
    for (index, instance) in graph.instances.iter().enumerate() {
        if instance.id != id {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some(index);
    }
    found
}

fn dependency_edge_count(graph: &CompiledFrameGraph) -> usize {
    graph.instances.iter().fold(0, |count, instance| {
        count.saturating_add(instance.dependencies.len())
    })
}

pub fn plan_effect_execution_demand(
    graph: &CompiledFrameGraph,
    repair_region: &EffectRegion,
    conservative_full: bool,
) -> EffectExecutionDemand {
    plan_effect_execution_demand_with_kawase_mode(graph, repair_region, conservative_full, false)
}

pub fn plan_effect_execution_demand_with_kawase_mode(
    graph: &CompiledFrameGraph,
    repair_region: &EffectRegion,
    conservative_full: bool,
    full_kawase: bool,
) -> EffectExecutionDemand {
    let repair_rect_count = repair_region.rects().len();
    if conservative_full
        || (!repair_region.is_empty() && repair_region.bounding_rect().is_none())
        || !graph_execution_metadata_is_complete(graph)
    {
        return all_visible_instances_with_output_regions(graph, repair_rect_count);
    }

    let mut output_regions = vec![None; graph.instances.len()];
    let mut max_instance_region_rect_count = 0;
    let mut region_representation_overflows = graph.stats.region_representation_overflows;
    let mut visible_clip_fallback_count = graph.stats.visible_clip_fallbacks;
    let mut work_region_bbox_coalesces = graph.stats.work_region_bbox_coalesces;
    let mut visible_clip_fallback_details = Vec::new();
    for (index, instance) in graph.instances.iter().enumerate() {
        let direct_result =
            repair_region.intersect_bounded_within_result(&instance.output_influence_region);
        if direct_result.overflowed {
            region_representation_overflows = region_representation_overflows.saturating_add(1);
            visible_clip_fallback_count = visible_clip_fallback_count.saturating_add(1);
            visible_clip_fallback_details.push(EffectVisibleClipFallback {
                instance: instance.id,
                pass: None,
                input_rect_count: direct_result.input_rect_count,
                clip_rect_count: direct_result.clip_rect_count,
            });
        }
        let direct = direct_result.region;
        if !direct.is_empty() {
            max_instance_region_rect_count =
                max_instance_region_rect_count.max(direct.rects().len());
            output_regions[index] = Some(direct);
        }
    }

    // Backdrop dependencies are compiled from checkpoints already seen for
    // earlier instances. Reverse traversal therefore sees each consumer's
    // final demand before propagating it to an earlier dependency. Each edge
    // is visited once; structural region equality is not a convergence test.
    let mut dependency_propagations: usize = 0;
    for consumer_index in (0..graph.instances.len()).rev() {
        let consumer = &graph.instances[consumer_index];
        if output_regions[consumer_index].is_none() {
            continue;
        }
        for dependency_id in &consumer.dependencies {
            let Some(dependency_index) = unique_instance_index(graph, *dependency_id) else {
                return all_visible_instances_with_output_regions(graph, repair_rect_count);
            };
            let dependency = &graph.instances[dependency_index];
            let required_result = consumer
                .capture_region
                .intersect_bounded_within_result(&dependency.output_influence_region);
            if required_result.overflowed {
                region_representation_overflows = region_representation_overflows.saturating_add(1);
                visible_clip_fallback_count = visible_clip_fallback_count.saturating_add(1);
                visible_clip_fallback_details.push(EffectVisibleClipFallback {
                    instance: dependency.id,
                    pass: None,
                    input_rect_count: required_result.input_rect_count,
                    clip_rect_count: required_result.clip_rect_count,
                });
            }
            let required = required_result.region;
            if required.is_empty() {
                continue;
            }
            dependency_propagations = dependency_propagations.saturating_add(1);
            let next_candidate = output_regions[dependency_index]
                .as_ref()
                .map_or_else(|| required.clone(), |existing| existing.union(&required));
            let next_result =
                next_candidate.intersect_bounded_within_result(&dependency.output_influence_region);
            if next_result.overflowed {
                region_representation_overflows = region_representation_overflows.saturating_add(1);
                visible_clip_fallback_count = visible_clip_fallback_count.saturating_add(1);
                visible_clip_fallback_details.push(EffectVisibleClipFallback {
                    instance: dependency.id,
                    pass: None,
                    input_rect_count: next_result.input_rect_count,
                    clip_rect_count: next_result.clip_rect_count,
                });
            }
            let next = next_result.region;
            max_instance_region_rect_count = max_instance_region_rect_count.max(next.rects().len());
            output_regions[dependency_index] = Some(next);
        }
    }

    let mut execution_region = EffectRegion::empty();
    let instances = output_regions
        .into_iter()
        .enumerate()
        .filter_map(|(index, output_region)| {
            output_region.map(|output_region| {
                let instance = &graph.instances[index];
                let (next_execution_region, coalesced) =
                    execution_region.union_with_diagnostics(&output_region);
                execution_region = next_execution_region;
                if coalesced {
                    work_region_bbox_coalesces = work_region_bbox_coalesces.saturating_add(1);
                }
                if !instance.dependencies.is_empty() {
                    let (next_execution_region, coalesced) =
                        execution_region.union_with_diagnostics(&instance.capture_region);
                    execution_region = next_execution_region;
                    if coalesced {
                        work_region_bbox_coalesces = work_region_bbox_coalesces.saturating_add(1);
                    }
                }
                EffectInstanceExecutionDemand {
                    id: graph.instances[index].id,
                    output_region,
                }
            })
        })
        .collect();
    let mut demand = EffectExecutionDemand {
        instances,
        execution_region,
        passes: Vec::new(),
        visible_clip_fallbacks: visible_clip_fallback_details,
        conservative_full: false,
        conservative_instances: Vec::new(),
        plan_stats: EffectDemandPlanStats {
            repair_rect_count,
            dependency_edge_count: dependency_edge_count(graph),
            dependency_propagations,
            max_instance_region_rect_count,
            region_representation_overflows,
            visible_clip_fallbacks: visible_clip_fallback_count,
            work_region_bbox_coalesces,
            conservative_full: false,
            ..EffectDemandPlanStats::default()
        },
    };
    plan_effect_pass_execution_demand(graph, &mut demand, full_kawase);
    demand
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct TextureProducer {
    pass_index: Option<usize>,
    valid: bool,
}

fn full_pass_region(graph: &CompiledFrameGraph, pass: &CompiledRenderPass) -> EffectRegion {
    pass.output
        .and_then(|output| graph.textures.iter().find(|texture| texture.id == output))
        .map_or_else(EffectRegion::empty, |texture| {
            EffectRegion::from_rect(texture.domain)
        })
}

fn conservative_pass_region(
    graph: &CompiledFrameGraph,
    pass: &CompiledRenderPass,
    output_region: Option<&EffectRegion>,
) -> EffectRegion {
    if matches!(
        pass.kind,
        RenderPassKind::Composite | RenderPassKind::OutputPostProcess
    ) {
        let Some(output_region) = output_region else {
            return full_pass_region(graph, pass);
        };
        let authoritative = graph
            .instances
            .iter()
            .find(|instance| instance.id == pass.instance)
            .map_or(output_region, |instance| &instance.output_influence_region);
        let constrained = pass.damage.union(output_region);
        return pass
            .output
            .and_then(|output| graph_texture_index(graph, output))
            .map_or_else(
                || constrained.intersect_bounded_within(authoritative),
                |output_index| {
                    constrained
                        .intersect_bounded_within(authoritative)
                        .intersect_rect(graph.textures[output_index].domain)
                },
            );
    }
    full_pass_region(graph, pass)
}

fn conservative_pass_demands(
    graph: &CompiledFrameGraph,
    instances: &[EffectInstanceExecutionDemand],
) -> Vec<EffectPassExecutionDemand> {
    graph
        .passes
        .iter()
        .map(|pass| EffectPassExecutionDemand {
            id: pass.id,
            output_region: conservative_pass_region(
                graph,
                pass,
                instances
                    .iter()
                    .find(|instance| instance.id == pass.instance)
                    .map(|instance| &instance.output_region),
            ),
        })
        .collect()
}

fn graph_texture_index(graph: &CompiledFrameGraph, id: GraphTextureId) -> Option<usize> {
    let mut found = None;
    for (index, texture) in graph.textures.iter().enumerate() {
        if texture.id != id {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some(index);
    }
    found
}

fn texture_producers(graph: &CompiledFrameGraph) -> Vec<TextureProducer> {
    graph
        .textures
        .iter()
        .map(|texture| {
            if matches!(
                texture.source,
                GraphTextureSource::Output | GraphTextureSource::Static(_)
            ) {
                return TextureProducer {
                    pass_index: None,
                    valid: true,
                };
            }
            let mut producer = None;
            let mut valid = true;
            for (pass_index, pass) in graph.passes.iter().enumerate() {
                if pass.output != Some(texture.id) {
                    continue;
                }
                if producer.replace(pass_index).is_some() {
                    valid = false;
                }
            }
            if producer.is_none() {
                valid = false;
            }
            TextureProducer {
                pass_index: producer,
                valid,
            }
        })
        .collect()
}

fn instance_is_selected(demand: &EffectExecutionDemand, instance: EffectInstanceId) -> bool {
    demand.contains(instance)
}

fn mark_instance_conservative(
    demand: &EffectExecutionDemand,
    fallbacks: &mut Vec<EffectInstanceId>,
    instance: EffectInstanceId,
) {
    if instance_is_selected(demand, instance) && !fallbacks.contains(&instance) {
        fallbacks.push(instance);
    }
}

fn pass_input_sampling_radius(pass: &CompiledRenderPass) -> Option<(f64, f64, bool, bool)> {
    match pass.kind {
        RenderPassKind::DualKawaseDownsample | RenderPassKind::DualKawaseUpsample => {
            let radius = pass.blur_radius?;
            if !radius.is_finite() || radius.is_sign_negative() {
                return None;
            }
            Some((f64::from(radius), f64::from(radius), false, true))
        }
        RenderPassKind::NormalizeInput => Some((0.0, 0.0, true, true)),
        RenderPassKind::Composite | RenderPassKind::OutputPostProcess => {
            Some((0.0, 0.0, true, true))
        }
        RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture => {
            Some((0.0, 0.0, false, false))
        }
        RenderPassKind::Fragment | RenderPassKind::Blend | RenderPassKind::Mask => {
            let (radius_x, radius_y) = match pass.stage.as_ref() {
                Some(EffectNodeKind::CustomFragment(spec)) => (
                    spec.declared_footprint.sample_radius_x,
                    spec.declared_footprint.sample_radius_y,
                ),
                _ => (0, 0),
            };
            Some((f64::from(radius_x), f64::from(radius_y), false, false))
        }
    }
}

fn physical_edge_to_logical(
    local: u32,
    logical_size: u32,
    physical_size: u32,
    round_up: bool,
) -> u32 {
    let numerator = u128::from(local) * u128::from(logical_size);
    let denominator = u128::from(physical_size);
    let value = if round_up {
        div_round_up(numerator, denominator)
    } else {
        numerator / denominator
    };
    u32::try_from(value).expect("logical texture coverage fits its domain dimension")
}

fn physical_coverage_to_logical_rect(
    coverage: GraphTexturePhysicalRect,
    texture: &GraphTexturePlan,
) -> Option<EffectRect> {
    if coverage.width() == 0
        || coverage.height() == 0
        || texture.width == 0
        || texture.height == 0
        || texture.domain.width == 0
        || texture.domain.height == 0
    {
        return None;
    }
    let left = physical_edge_to_logical(
        coverage.left.min(texture.width),
        texture.domain.width,
        texture.width,
        false,
    );
    let top = physical_edge_to_logical(
        coverage.top.min(texture.height),
        texture.domain.height,
        texture.height,
        false,
    );
    let right = physical_edge_to_logical(
        coverage.right.min(texture.width),
        texture.domain.width,
        texture.width,
        true,
    );
    let bottom = physical_edge_to_logical(
        coverage.bottom.min(texture.height),
        texture.domain.height,
        texture.height,
        true,
    );
    let x = i64::from(texture.domain.x) + i64::from(left);
    let y = i64::from(texture.domain.y) + i64::from(top);
    let width = right.saturating_sub(left);
    let height = bottom.saturating_sub(top);
    EffectRect::new(
        i32::try_from(x).ok()?,
        i32::try_from(y).ok()?,
        width,
        height,
    )
}

#[derive(Clone, Copy)]
struct TextureAxisMapping {
    output_size: u32,
    output_domain_origin: i32,
    output_domain_size: u32,
    input_size: u32,
    input_domain_origin: i32,
    input_domain_size: u32,
}

fn output_center_to_input_texel(
    output_center: f64,
    mapping: TextureAxisMapping,
    logical_mapping: bool,
) -> f64 {
    if logical_mapping {
        let logical_position = f64::from(mapping.output_domain_origin)
            + output_center * f64::from(mapping.output_domain_size)
                / f64::from(mapping.output_size);
        (logical_position - f64::from(mapping.input_domain_origin)) * f64::from(mapping.input_size)
            / f64::from(mapping.input_domain_size)
            - 0.5
    } else {
        output_center * f64::from(mapping.input_size) / f64::from(mapping.output_size) - 0.5
    }
}

fn sampled_input_axis_bounds(
    output_start: u32,
    output_end: u32,
    mapping: TextureAxisMapping,
    radius: f64,
    logical_mapping: bool,
    linear_filter_support: bool,
) -> Option<(u32, u32)> {
    if output_start >= output_end || mapping.output_size == 0 || mapping.input_size == 0 {
        return None;
    }
    let first_sample =
        output_center_to_input_texel(f64::from(output_start) + 0.5, mapping, logical_mapping)
            - radius;
    let last_sample = output_center_to_input_texel(
        f64::from(output_end.saturating_sub(1)) + 0.5,
        mapping,
        logical_mapping,
    ) + radius;
    if !first_sample.is_finite() || !last_sample.is_finite() {
        return None;
    }
    if last_sample < 0.0 {
        return Some((0, 1));
    }
    if first_sample >= f64::from(mapping.input_size) {
        return Some((mapping.input_size - 1, mapping.input_size));
    }
    let left = first_sample
        .floor()
        .clamp(0.0, f64::from(mapping.input_size)) as u32;
    let right_unclipped = if linear_filter_support {
        last_sample.floor() + 2.0
    } else {
        last_sample.ceil() + 1.0
    };
    let right = right_unclipped.clamp(0.0, f64::from(mapping.input_size)) as u32;
    (right > left).then_some((left, right))
}

fn map_region_to_input_texture(
    demanded_output: &EffectRegion,
    output: &GraphTexturePlan,
    input: &GraphTexturePlan,
    radius_x: f64,
    radius_y: f64,
    logical_mapping: bool,
    linear_filter_support: bool,
) -> Option<EffectRegion> {
    if output.width == 0
        || output.height == 0
        || input.width == 0
        || input.height == 0
        || output.domain.width == 0
        || output.domain.height == 0
        || input.domain.width == 0
        || input.domain.height == 0
        || demanded_output.bounding_rect().is_none()
    {
        return None;
    }

    let mut result = EffectRegion::empty();
    for requested in demanded_output.rects() {
        let Some(output_coverage) = logical_rect_to_physical_coverage(*requested, output) else {
            continue;
        };
        let (left, right) = sampled_input_axis_bounds(
            output_coverage.left,
            output_coverage.right,
            TextureAxisMapping {
                output_size: output.width,
                output_domain_origin: output.domain.x,
                output_domain_size: output.domain.width,
                input_size: input.width,
                input_domain_origin: input.domain.x,
                input_domain_size: input.domain.width,
            },
            radius_x,
            logical_mapping,
            linear_filter_support,
        )?;
        let (top, bottom) = sampled_input_axis_bounds(
            output_coverage.top,
            output_coverage.bottom,
            TextureAxisMapping {
                output_size: output.height,
                output_domain_origin: output.domain.y,
                output_domain_size: output.domain.height,
                input_size: input.height,
                input_domain_origin: input.domain.y,
                input_domain_size: input.domain.height,
            },
            radius_y,
            logical_mapping,
            linear_filter_support,
        )?;
        let required = GraphTexturePhysicalRect {
            left,
            top,
            right,
            bottom,
        };
        result.push(physical_coverage_to_logical_rect(required, input)?);
        result.bounding_rect()?;
    }
    Some(result)
}

fn required_input_region(
    pass: &CompiledRenderPass,
    demanded_output: &EffectRegion,
    output: &GraphTexturePlan,
    input: &GraphTexturePlan,
) -> Option<EffectRegion> {
    let (mut radius_x, mut radius_y, logical_mapping, linear_filter_support) =
        pass_input_sampling_radius(pass)?;
    if let Some(EffectNodeKind::CustomFragment(_)) = pass.stage.as_ref() {
        radius_x *= f64::from(input.width) / f64::from(input.domain.width.max(1));
        radius_y *= f64::from(input.height) / f64::from(input.domain.height.max(1));
    }
    map_region_to_input_texture(
        demanded_output,
        output,
        input,
        radius_x,
        radius_y,
        logical_mapping,
        linear_filter_support,
    )
}

fn full_region_is_demanded(region: &EffectRegion, domain: EffectRect) -> bool {
    region.bounding_rect().is_none() || (region.rects().len() == 1 && region.rects()[0] == domain)
}

fn update_pass_plan_stats(
    graph: &CompiledFrameGraph,
    pass_regions: &[EffectPassExecutionDemand],
    conservative_instances: &[EffectInstanceId],
    stats: &mut EffectDemandPlanStats,
) {
    stats.pass_count_selected = 0;
    stats.partial_pass_count = 0;
    stats.full_domain_pass_count = 0;
    stats.max_pass_region_rect_count = 0;
    stats.pass_conservative_fallbacks = 0;
    for pass_demand in pass_regions {
        if pass_demand.output_region.is_empty() {
            continue;
        }
        stats.pass_count_selected = stats.pass_count_selected.saturating_add(1);
        stats.max_pass_region_rect_count = stats
            .max_pass_region_rect_count
            .max(pass_demand.output_region.rects().len());
        let full_domain = graph
            .passes
            .iter()
            .find(|pass| pass.id == pass_demand.id)
            .and_then(|pass| pass.output)
            .and_then(|output| graph.textures.iter().find(|texture| texture.id == output))
            .is_some_and(|texture| {
                full_region_is_demanded(&pass_demand.output_region, texture.domain)
            });
        if full_domain {
            stats.full_domain_pass_count = stats.full_domain_pass_count.saturating_add(1);
        } else {
            stats.partial_pass_count = stats.partial_pass_count.saturating_add(1);
        }
        if graph
            .passes
            .iter()
            .find(|pass| pass.id == pass_demand.id)
            .is_some_and(|pass| conservative_instances.contains(&pass.instance))
        {
            stats.pass_conservative_fallbacks = stats.pass_conservative_fallbacks.saturating_add(1);
        }
    }
}

fn plan_effect_pass_execution_demand(
    graph: &CompiledFrameGraph,
    demand: &mut EffectExecutionDemand,
    full_kawase: bool,
) {
    if demand.is_conservative_full() {
        demand.passes = conservative_pass_demands(graph, &demand.instances);
        update_pass_plan_stats(
            graph,
            &demand.passes,
            &demand.conservative_instances,
            &mut demand.plan_stats,
        );
        return;
    }

    let producers = texture_producers(graph);
    let mut pass_regions = vec![EffectRegion::empty(); graph.passes.len()];
    let mut conservative_instances = Vec::new();
    for instance_demand in &demand.instances {
        let final_pass = graph.passes.iter().enumerate().rev().find(|(_, pass)| {
            pass.instance == instance_demand.id
                && matches!(
                    pass.kind,
                    RenderPassKind::Composite | RenderPassKind::OutputPostProcess
                )
        });
        let Some((pass_index, pass)) = final_pass else {
            mark_instance_conservative(demand, &mut conservative_instances, instance_demand.id);
            continue;
        };
        let Some(output) = pass.output.and_then(|id| graph_texture_index(graph, id)) else {
            mark_instance_conservative(demand, &mut conservative_instances, instance_demand.id);
            continue;
        };
        let output_plan = &graph.textures[output];
        let Some(instance) = graph
            .instances
            .iter()
            .find(|instance| instance.id == instance_demand.id)
        else {
            mark_instance_conservative(demand, &mut conservative_instances, instance_demand.id);
            continue;
        };
        let seed_candidate = pass.damage.union(&instance_demand.output_region);
        let seed_result =
            seed_candidate.intersect_bounded_within_result(&instance.output_influence_region);
        if seed_result.overflowed {
            demand.plan_stats.region_representation_overflows = demand
                .plan_stats
                .region_representation_overflows
                .saturating_add(1);
            demand.plan_stats.visible_clip_fallbacks =
                demand.plan_stats.visible_clip_fallbacks.saturating_add(1);
        }
        let seed = seed_result.region.intersect_rect(output_plan.domain);
        if seed.bounding_rect().is_none() {
            mark_instance_conservative(demand, &mut conservative_instances, instance_demand.id);
        } else {
            pass_regions[pass_index] = seed;
        }
    }

    let selected_passes = graph
        .passes
        .iter()
        .map(|pass| instance_is_selected(demand, pass.instance))
        .collect::<Vec<_>>();
    if full_kawase {
        for (pass_index, pass) in graph.passes.iter().enumerate() {
            if selected_passes[pass_index]
                && matches!(
                    pass.kind,
                    RenderPassKind::DualKawaseDownsample | RenderPassKind::DualKawaseUpsample
                )
            {
                pass_regions[pass_index] = full_pass_region(graph, pass);
            }
        }
    }
    let mut pass_dependency_propagations = 0usize;
    let mut visited_edges = Vec::new();
    for pass_index in (0..graph.passes.len()).rev() {
        if !selected_passes[pass_index] || pass_regions[pass_index].is_empty() {
            continue;
        }
        let pass = &graph.passes[pass_index];
        let Some(output_id) = pass.output else {
            mark_instance_conservative(demand, &mut conservative_instances, pass.instance);
            continue;
        };
        let Some(output_index) = graph_texture_index(graph, output_id) else {
            mark_instance_conservative(demand, &mut conservative_instances, pass.instance);
            continue;
        };
        let output_plan = &graph.textures[output_index];
        if matches!(
            pass.kind,
            RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
        ) {
            continue;
        }
        for input_id in &pass.inputs {
            let Some(input_index) = graph_texture_index(graph, *input_id) else {
                mark_instance_conservative(demand, &mut conservative_instances, pass.instance);
                continue;
            };
            let input_plan = &graph.textures[input_index];
            if matches!(input_plan.source, GraphTextureSource::Static(_)) {
                continue;
            }
            let Some(producer) = producers.get(input_index).copied() else {
                mark_instance_conservative(demand, &mut conservative_instances, pass.instance);
                continue;
            };
            if !producer.valid {
                mark_instance_conservative(demand, &mut conservative_instances, pass.instance);
                continue;
            }
            let Some(producer_index) = producer.pass_index else {
                mark_instance_conservative(demand, &mut conservative_instances, pass.instance);
                continue;
            };
            if graph.passes[producer_index].instance != pass.instance {
                mark_instance_conservative(demand, &mut conservative_instances, pass.instance);
                mark_instance_conservative(
                    demand,
                    &mut conservative_instances,
                    graph.passes[producer_index].instance,
                );
                continue;
            }
            if producer_index >= pass_index {
                mark_instance_conservative(demand, &mut conservative_instances, pass.instance);
                mark_instance_conservative(
                    demand,
                    &mut conservative_instances,
                    graph.passes[producer_index].instance,
                );
                continue;
            }
            if visited_edges.contains(&(pass_index, producer_index)) {
                continue;
            }
            visited_edges.push((pass_index, producer_index));
            let Some(required) =
                required_input_region(pass, &pass_regions[pass_index], output_plan, input_plan)
            else {
                mark_instance_conservative(demand, &mut conservative_instances, pass.instance);
                continue;
            };
            if required.is_empty() {
                continue;
            }
            pass_dependency_propagations = pass_dependency_propagations.saturating_add(1);
            let (next, coalesced) = pass_regions[producer_index].union_with_diagnostics(&required);
            if coalesced {
                demand.plan_stats.work_region_bbox_coalesces = demand
                    .plan_stats
                    .work_region_bbox_coalesces
                    .saturating_add(1);
            }
            if next.bounding_rect().is_none() {
                mark_instance_conservative(demand, &mut conservative_instances, pass.instance);
                mark_instance_conservative(
                    demand,
                    &mut conservative_instances,
                    graph.passes[producer_index].instance,
                );
            } else {
                pass_regions[producer_index] = next;
            }
        }
    }

    for (pass_index, pass) in graph.passes.iter().enumerate() {
        if conservative_instances.contains(&pass.instance) {
            pass_regions[pass_index] =
                conservative_pass_region(graph, pass, demand.output_region(pass.instance));
        }
        if full_kawase
            && selected_passes[pass_index]
            && matches!(
                pass.kind,
                RenderPassKind::DualKawaseDownsample | RenderPassKind::DualKawaseUpsample
            )
        {
            pass_regions[pass_index] = full_pass_region(graph, pass);
        }
    }
    demand.conservative_instances = conservative_instances;
    demand.passes = graph
        .passes
        .iter()
        .enumerate()
        .filter(|(_, pass)| instance_is_selected(demand, pass.instance))
        .map(|(pass_index, pass)| EffectPassExecutionDemand {
            id: pass.id,
            output_region: pass_regions[pass_index].clone(),
        })
        .collect();
    demand.plan_stats.pass_dependency_propagations = pass_dependency_propagations;
    update_pass_plan_stats(
        graph,
        &demand.passes,
        &demand.conservative_instances,
        &mut demand.plan_stats,
    );
}

fn graph_execution_metadata_is_complete(graph: &CompiledFrameGraph) -> bool {
    // The compiler records dependencies from checkpoints that precede the
    // current instance. Demand planning relies on that topological ordering,
    // so malformed metadata takes the conservative fallback below.
    graph.instances.iter().enumerate().all(|(index, instance)| {
        unique_instance_index(graph, instance.id) == Some(index)
            && instance.dependencies.iter().all(|dependency_id| {
                unique_instance_index(graph, *dependency_id)
                    .is_some_and(|dependency_index| dependency_index < index)
            })
    }) && graph.passes.iter().all(|pass| {
        unique_instance_index(graph, pass.instance).is_some()
            && pass
                .inputs
                .iter()
                .chain(pass.output.iter())
                .all(|texture_id| {
                    graph
                        .textures
                        .iter()
                        .any(|texture| texture.id == *texture_id)
                })
    })
}

impl CompiledFrameGraph {
    /// Returns a bounded, deterministic description suitable for diagnostics.
    /// It names logical stages and allocation geometry but never includes shader
    /// source or user-provided uniform values.
    pub fn explain(&self) -> String {
        let capture_pixels = self
            .textures
            .iter()
            .filter(|texture| {
                matches!(
                    texture.source,
                    GraphTextureSource::CapturedScene | GraphTextureSource::CapturedTarget
                )
            })
            .map(|texture| u64::from(texture.width).saturating_mul(u64::from(texture.height)))
            .sum::<u64>();
        let output_pixels = region_pixels(&self.final_damage);
        let mut explanation = format!(
            "effects graph: instances={} passes={} textures={} peak_live={} capture_px={} output_px={}\n",
            self.stats.effect_instances,
            self.stats.passes,
            self.stats.textures,
            self.stats.peak_live_intermediates,
            capture_pixels,
            output_pixels,
        );
        for (index, pass) in self.passes.iter().enumerate() {
            let _ = write!(
                explanation,
                "  #{index} {} instance={} anchor={} ",
                pass_label(pass),
                pass.instance.get(),
                anchor_label(pass.anchor),
            );
            if let Some(input) = pass.inputs.first() {
                let _ = write!(explanation, "input=t{} ", input.get());
            }
            if pass.inputs.len() > 1 {
                let _ = write!(explanation, "inputs={} ", pass.inputs.len());
            }
            if let Some(output) = pass.output {
                let _ = write!(explanation, "output=t{} ", output.get());
                if let Some(texture) = self.textures.iter().find(|texture| texture.id == output) {
                    let _ = write!(explanation, "size={}x{} ", texture.width, texture.height);
                }
            }
            if let Some(radius) = pass.blur_radius {
                let _ = write!(explanation, "radius={radius} ");
            }
            explanation.push('\n');
        }
        explanation
    }
}

fn pass_label(pass: &CompiledRenderPass) -> &'static str {
    match pass.kind {
        RenderPassKind::SceneCapture => "capture_scene",
        RenderPassKind::SurfaceCapture => "capture_target",
        RenderPassKind::NormalizeInput => "normalize_input",
        RenderPassKind::DualKawaseDownsample => "kawase_down",
        RenderPassKind::DualKawaseUpsample => "kawase_up",
        RenderPassKind::Fragment => pass.stage.as_ref().map_or("fragment", node_label),
        RenderPassKind::Blend => "blend",
        RenderPassKind::Mask => "mask",
        RenderPassKind::Composite => "composite",
        RenderPassKind::OutputPostProcess => "output_post_process",
    }
}

fn node_label(node: &EffectNodeKind) -> &'static str {
    match node {
        EffectNodeKind::Source(_) => "source",
        EffectNodeKind::DualKawaseBlur(_) => "kawase",
        EffectNodeKind::ColorMatrix(_) => "color_matrix",
        EffectNodeKind::Tint(_) => "tint",
        EffectNodeKind::Noise(_) => "noise",
        EffectNodeKind::CustomFragment(_) => "custom",
        EffectNodeKind::Blend(_) => "blend",
        EffectNodeKind::Mask(_) => "mask",
    }
}

fn anchor_label(anchor: EffectAnchor) -> String {
    match anchor {
        EffectAnchor::BeforeSurface(id) => format!("before_surface={id}"),
        EffectAnchor::ReplaceSurface(id) => format!("replace_surface={id}"),
        EffectAnchor::AfterSurface(id) => format!("after_surface={id}"),
        EffectAnchor::OutputPostProcess => "output_post_process".to_owned(),
    }
}

fn region_pixels(region: &EffectRegion) -> u64 {
    region.rects().iter().fold(0u64, |total, rect| {
        total.saturating_add(u64::from(rect.width).saturating_mul(u64::from(rect.height)))
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrameExecutionPlan {
    LegacyScene,
    EffectGraph(CompiledFrameGraph),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderGraphCompileError {
    MissingProgram(EffectProgramId),
    InvalidGraph(EffectValidationError),
    TooManyTextures,
    TooManyPasses,
    TextureIdOverflow,
    UnsupportedNode(EffectNodeId),
    UnsupportedStaticTexture(super::StaticTextureId),
}

impl std::fmt::Display for RenderGraphCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for RenderGraphCompileError {}

struct GraphBuilder {
    passes: Vec<CompiledRenderPass>,
    textures: Vec<GraphTexturePlan>,
}

impl GraphBuilder {
    fn new(output_bounds: EffectRect) -> Result<(Self, GraphTextureId), RenderGraphCompileError> {
        let mut builder = Self {
            passes: Vec::new(),
            textures: Vec::new(),
        };
        let output = builder.add_texture_with_layout(
            GraphTextureSource::Output,
            output_bounds,
            output_bounds.width,
            output_bounds.height,
            EffectWorkingSpace::OutputEncodedSrgb,
        )?;
        Ok((builder, output))
    }

    fn add_texture_with_layout(
        &mut self,
        source: GraphTextureSource,
        domain: EffectRect,
        width: u32,
        height: u32,
        working_space: EffectWorkingSpace,
    ) -> Result<GraphTextureId, RenderGraphCompileError> {
        if self.textures.len() >= MAX_GRAPH_TEXTURES {
            return Err(RenderGraphCompileError::TooManyTextures);
        }
        let index = u16::try_from(self.textures.len() + 1)
            .map_err(|_| RenderGraphCompileError::TextureIdOverflow)?;
        let id = GraphTextureId::new(index).ok_or(RenderGraphCompileError::TextureIdOverflow)?;
        self.textures.push(GraphTexturePlan {
            id,
            source,
            width: width.max(1),
            height: height.max(1),
            domain,
            working_space,
            origin: GraphTextureOrigin::BottomLeft,
            first_use: None,
            last_use: None,
        });
        Ok(id)
    }

    fn touch(&mut self, texture: GraphTextureId, pass: GraphPassId) {
        let entry = self
            .textures
            .get_mut(usize::from(texture.get() - 1))
            .expect("graph texture id must refer to a builder texture");
        if entry.first_use.is_none() {
            entry.first_use = Some(pass);
        }
        entry.last_use = Some(pass);
    }

    fn texture(&self, texture: GraphTextureId) -> GraphTexturePlan {
        self.textures
            .get(usize::from(texture.get() - 1))
            .expect("graph texture id must refer to a builder texture")
            .clone()
    }

    #[allow(clippy::too_many_arguments)]
    #[expect(
        clippy::too_many_arguments,
        reason = "graph pass fields stay explicit at the lowering boundary"
    )]
    fn add_pass(
        &mut self,
        kind: RenderPassKind,
        inputs: Vec<GraphTextureId>,
        output: Option<GraphTextureId>,
        damage: EffectRegion,
        instance: EffectInstanceId,
        anchor: EffectAnchor,
        blur_radius: Option<f32>,
        anchor_scope: EffectAnchorScope,
    ) -> Result<GraphPassId, RenderGraphCompileError> {
        if self.passes.len() >= MAX_GRAPH_PASSES {
            return Err(RenderGraphCompileError::TooManyPasses);
        }
        let index = u16::try_from(self.passes.len() + 1)
            .map_err(|_| RenderGraphCompileError::TextureIdOverflow)?;
        let id = GraphPassId::new(index).ok_or(RenderGraphCompileError::TextureIdOverflow)?;
        for texture in inputs.iter().copied().chain(output) {
            self.touch(texture, id);
        }
        self.passes.push(CompiledRenderPass {
            id,
            kind,
            inputs,
            output,
            damage,
            instance,
            anchor,
            blur_radius,
            stage: None,
            fused_stages: Vec::new(),
            parameter_block: super::EffectParameterBlock::default(),
            alpha_mode: EffectAlphaMode::Preserve,
            encode_output: false,
            color_conversion: EffectColorConversion::None,
            checkpoint_dependencies: Vec::new(),
            visual_group: None,
            anchor_scope,
            visible_clip_fallback: None,
        });
        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    fn add_stage_pass(
        &mut self,
        kind: RenderPassKind,
        inputs: Vec<GraphTextureId>,
        output: Option<GraphTextureId>,
        damage: EffectRegion,
        instance: EffectInstanceId,
        anchor: EffectAnchor,
        stage: EffectNodeKind,
        anchor_scope: EffectAnchorScope,
    ) -> Result<GraphPassId, RenderGraphCompileError> {
        let pass = self.add_pass(
            kind,
            inputs,
            output,
            damage,
            instance,
            anchor,
            None,
            anchor_scope,
        )?;
        self.passes
            .last_mut()
            .expect("stage pass was appended")
            .stage = Some(stage);
        self.passes
            .last_mut()
            .expect("stage pass was appended")
            .parameter_block = super::EffectParameterBlock::default();
        Ok(pass)
    }
}

pub fn compile_frame_execution_plan(
    scene: &ResolvedEffectScene,
    source_damage: &EffectRegion,
    output_bounds: EffectRect,
    registry: &EffectRegistry,
) -> Result<FrameExecutionPlan, RenderGraphCompileError> {
    #[cfg(test)]
    note_graph_compile();
    let visible_instance_count = scene
        .instances
        .iter()
        .filter(|instance| !instance.region.is_empty())
        .count();
    if visible_instance_count == 0 {
        return Ok(FrameExecutionPlan::LegacyScene);
    }

    let (mut builder, output_texture) = GraphBuilder::new(output_bounds)?;
    let mut final_damage = source_damage.clone();
    let mut checkpoints = Vec::<(GraphPassId, EffectRegion, EffectInstanceId)>::new();
    let mut compiled_instances = Vec::with_capacity(visible_instance_count);
    let mut region_representation_overflows = 0usize;
    let mut visible_clip_fallbacks = 0usize;

    for instance in scene
        .instances
        .iter()
        .filter(|instance| !instance.region.is_empty())
    {
        let program = registry
            .get(instance.program)
            .ok_or(RenderGraphCompileError::MissingProgram(instance.program))?;
        let effect_damage = plan_effect_damage(
            program.aggregate_footprint,
            &instance.region,
            source_damage,
            output_bounds,
        );
        if effect_damage.output_clip_fallback.is_some() {
            region_representation_overflows = region_representation_overflows.saturating_add(1);
            visible_clip_fallbacks = visible_clip_fallbacks.saturating_add(1);
        }
        final_damage = final_damage.union(&effect_damage.output_damage);
        let (dependencies, dependency_instances) = if program.lowering.uses_backdrop {
            let mut dependencies = Vec::new();
            let mut dependency_instances = Vec::new();
            for (pass, region, instance_id) in &checkpoints {
                #[cfg(test)]
                note_checkpoint_intersection_check();
                if region.intersects(&effect_damage.capture_region) {
                    dependencies.push(*pass);
                    dependency_instances.push(*instance_id);
                }
            }
            (dependencies, dependency_instances)
        } else {
            (Vec::new(), Vec::new())
        };
        let checkpoint = compile_instance(
            &mut builder,
            output_texture,
            instance,
            program,
            InstanceCompilePlan {
                capture_damage: &effect_damage.capture_region,
                output_damage: &effect_damage.output_damage,
                output_bounds,
                checkpoint_dependencies: &dependencies,
                output_clip_fallback: effect_damage.output_clip_fallback,
            },
        )?;
        compiled_instances.push(CompiledEffectInstance {
            id: instance.id,
            output_influence_region: effect_damage.dependency_region.clone(),
            capture_region: effect_damage.capture_region,
            dependencies: dependency_instances,
        });
        checkpoints.push((checkpoint, effect_damage.dependency_region, instance.id));
    }

    fuse_compatible_local_stages(&mut builder);
    let intermediate_textures = builder
        .textures
        .iter()
        .filter(|texture| texture.source == GraphTextureSource::Intermediate)
        .count();
    let peak_live_intermediates = peak_live_intermediates(&builder.passes, &builder.textures);
    let stats = RenderGraphCompileStats {
        effect_instances: visible_instance_count,
        passes: builder.passes.len(),
        textures: builder.textures.len(),
        intermediate_textures,
        peak_live_intermediates,
        region_representation_overflows,
        visible_clip_fallbacks,
        work_region_bbox_coalesces: 0,
    };
    Ok(FrameExecutionPlan::EffectGraph(CompiledFrameGraph {
        passes: builder.passes,
        textures: builder.textures,
        instances: compiled_instances,
        final_damage,
        stats,
    }))
}

fn build_pass_position_map(passes: &[CompiledRenderPass]) -> Vec<Option<usize>> {
    let max_pass_id = passes
        .iter()
        .map(|pass| usize::from(pass.id.get()))
        .max()
        .unwrap_or(0);
    let mut position_by_id = vec![None; max_pass_id.saturating_add(1)];
    for (position, pass) in passes.iter().enumerate() {
        let id = usize::from(pass.id.get());
        debug_assert!(
            id <= MAX_GRAPH_PASSES,
            "compiled pass ID must fit the graph pass bound"
        );
        #[cfg(test)]
        note_peak_live_pass_position_map_entry();
        if let Some(mapped_position) = position_by_id.get_mut(id) {
            debug_assert!(
                mapped_position.is_none(),
                "compiled pass IDs must be unique for peak-live mapping"
            );
            *mapped_position = Some(position);
        }
    }
    position_by_id
}

fn peak_live_intermediates(passes: &[CompiledRenderPass], textures: &[GraphTexturePlan]) -> usize {
    if passes.is_empty() {
        return 0;
    }

    let position_by_id = build_pass_position_map(passes);

    let mut delta = vec![0_i32; passes.len() + 1];
    for texture in textures
        .iter()
        .filter(|texture| texture.source == GraphTextureSource::Intermediate)
    {
        let (first, last) = match (texture.first_use, texture.last_use) {
            (None, None) => continue,
            (None, Some(_)) | (Some(_), None) => {
                debug_assert!(false, "intermediate texture must have a complete lifetime");
                continue;
            }
            (Some(first), Some(last)) => (first, last),
        };
        let interval = match (
            position_by_id
                .get(usize::from(first.get()))
                .copied()
                .flatten(),
            position_by_id
                .get(usize::from(last.get()))
                .copied()
                .flatten(),
        ) {
            (Some(first), Some(last)) if first <= last => (first, last),
            (Some(first), Some(last)) => {
                debug_assert!(first <= last);
                continue;
            }
            _ => {
                debug_assert!(
                    false,
                    "intermediate texture lifetime references a removed pass"
                );
                (0, passes.len() - 1)
            }
        };
        #[cfg(test)]
        note_peak_live_interval_insertion();
        delta[interval.0] += 1;
        delta[interval.1 + 1] -= 1;
    }

    let mut live = 0_i32;
    let mut peak = 0_i32;
    for change in delta.into_iter().take(passes.len()) {
        #[cfg(test)]
        note_peak_live_sweep_step();
        live += change;
        peak = peak.max(live);
    }
    usize::try_from(peak).unwrap_or(0)
}

fn fuse_compatible_local_stages(builder: &mut GraphBuilder) {
    let mut index = 0;
    while index + 1 < builder.passes.len() {
        let (first, second) = (&builder.passes[index], &builder.passes[index + 1]);
        let Some(first_stage) = first.stage.as_ref() else {
            index += 1;
            continue;
        };
        let Some(first_output) = first.output else {
            index += 1;
            continue;
        };
        let Some(second_stage) = builder.passes[index + 1].stage.as_ref() else {
            index += 1;
            continue;
        };
        let can_fuse = first.kind == RenderPassKind::Fragment
            && second.kind == RenderPassKind::Fragment
            && first.instance == second.instance
            && second.inputs.len() == 1
            && second.inputs[0] == first_output
            && first.fused_stages.is_empty()
            && second.fused_stages.is_empty()
            && compatible_local_stage_order(first_stage, second_stage)
            && builder
                .textures
                .iter()
                .find(|texture| texture.id == first_output)
                .is_some_and(|texture| texture.last_use == Some(second.id));
        if !can_fuse {
            index += 1;
            continue;
        }
        let second_stage = second_stage.clone();
        let old_output = first.output.expect("fusion input output exists");
        let new_output = second.output.expect("fusion output exists");
        let second_id = second.id;
        let first_id = first.id;
        builder.passes[index].output = Some(new_output);
        builder.passes[index].damage = builder.passes[index + 1].damage.clone();
        builder.passes[index].fused_stages.push(second_stage);
        builder.passes.remove(index + 1);
        if let Some(texture) = builder
            .textures
            .iter_mut()
            .find(|texture| texture.id == old_output)
        {
            texture.first_use = None;
            texture.last_use = None;
        }
        if let Some(texture) = builder
            .textures
            .iter_mut()
            .find(|texture| texture.id == new_output)
        {
            if texture.first_use == Some(second_id) {
                texture.first_use = Some(first_id);
            }
            if texture.last_use == Some(second_id) {
                texture.last_use = Some(first_id);
            }
        }
    }
}

fn compatible_local_stage_order(first: &EffectNodeKind, second: &EffectNodeKind) -> bool {
    fn rank(stage: &EffectNodeKind) -> Option<u8> {
        Some(match stage {
            EffectNodeKind::ColorMatrix(_) => 0,
            EffectNodeKind::Tint(_) => 1,
            EffectNodeKind::Noise(_) => 2,
            _ => return None,
        })
    }
    rank(first)
        .zip(rank(second))
        .is_some_and(|(first, second)| second > first)
}

struct InstanceCompilePlan<'a> {
    capture_damage: &'a EffectRegion,
    output_damage: &'a EffectRegion,
    output_bounds: EffectRect,
    checkpoint_dependencies: &'a [GraphPassId],
    output_clip_fallback: Option<EffectRegionClipFallback>,
}

fn resolve_output(
    outputs: &[Option<GraphTextureId>; MAX_EFFECT_PROGRAM_NODES],
    slot: usize,
    node: EffectNodeId,
) -> Result<GraphTextureId, RenderGraphCompileError> {
    outputs
        .get(slot)
        .copied()
        .flatten()
        .ok_or(RenderGraphCompileError::InvalidGraph(
            EffectValidationError::MissingInputNode(node),
        ))
}

fn store_output(
    outputs: &mut [Option<GraphTextureId>; MAX_EFFECT_PROGRAM_NODES],
    slot: usize,
    texture: GraphTextureId,
) -> Result<(), RenderGraphCompileError> {
    let output = outputs
        .get_mut(slot)
        .ok_or(RenderGraphCompileError::InvalidGraph(
            EffectValidationError::MissingOutputNode,
        ))?;
    *output = Some(texture);
    Ok(())
}

fn compile_instance(
    builder: &mut GraphBuilder,
    output_texture: GraphTextureId,
    instance: &ResolvedEffectInstance,
    program: &ValidatedEffectProgram,
    plan: InstanceCompilePlan<'_>,
) -> Result<GraphPassId, RenderGraphCompileError> {
    #[cfg(test)]
    note_instance_compile();
    let InstanceCompilePlan {
        capture_damage,
        output_damage,
        output_bounds,
        checkpoint_dependencies,
        output_clip_fallback,
    } = plan;
    let visual_group = instance.visual_group;
    let mut outputs = [None; MAX_EFFECT_PROGRAM_NODES];

    for (slot, step) in program.lowering.steps.iter().enumerate() {
        #[cfg(test)]
        note_node_visit();
        let node = program.program.nodes.get(step.node_index).ok_or(
            RenderGraphCompileError::InvalidGraph(EffectValidationError::MissingOutputNode),
        )?;
        match &node.kind {
            EffectNodeKind::Source(source) => {
                if let EffectSource::StaticTexture(id) = source {
                    return Err(RenderGraphCompileError::UnsupportedStaticTexture(*id));
                }
                let texture_source = match source {
                    EffectSource::Backdrop => GraphTextureSource::CapturedScene,
                    EffectSource::TargetContent => GraphTextureSource::CapturedTarget,
                    EffectSource::StaticTexture(id) => GraphTextureSource::Static(*id),
                };
                let (domain, width, height, working_space) = match source {
                    EffectSource::Backdrop => {
                        let domain = capture_damage.bounding_rect().unwrap_or(output_bounds);
                        (
                            domain,
                            domain.width,
                            domain.height,
                            EffectWorkingSpace::OutputEncodedSrgb,
                        )
                    }
                    EffectSource::TargetContent => (
                        instance.target_bounds,
                        instance.target_bounds.width,
                        instance.target_bounds.height,
                        EffectWorkingSpace::OutputEncodedSrgb,
                    ),
                    EffectSource::StaticTexture(_) => unreachable!("static textures are rejected"),
                };
                let texture = builder.add_texture_with_layout(
                    texture_source,
                    domain,
                    width,
                    height,
                    working_space,
                )?;
                if !matches!(source, EffectSource::StaticTexture(_)) {
                    let kind = match source {
                        EffectSource::Backdrop => RenderPassKind::SceneCapture,
                        EffectSource::TargetContent => RenderPassKind::SurfaceCapture,
                        EffectSource::StaticTexture(_) => unreachable!(),
                    };
                    builder.add_pass(
                        kind,
                        Vec::new(),
                        Some(texture),
                        capture_damage.clone(),
                        instance.id,
                        instance.anchor,
                        None,
                        instance.anchor_scope,
                    )?;
                    let capture_pass = builder
                        .passes
                        .last_mut()
                        .expect("capture pass was appended");
                    if matches!(source, EffectSource::Backdrop) {
                        capture_pass.checkpoint_dependencies = checkpoint_dependencies.to_vec();
                    }
                    capture_pass.visual_group = visual_group;
                }
                store_output(&mut outputs, slot, texture)?;
            }
            EffectNodeKind::DualKawaseBlur(spec) => {
                let input_slot = step.input_slots.first().copied().ok_or(
                    RenderGraphCompileError::InvalidGraph(EffectValidationError::MissingInputNode(
                        node.id,
                    )),
                )?;
                let input = resolve_output(&outputs, input_slot, node.id)?;
                let input_plan = builder.texture(input);
                let processing_width = scaled_dimension(input_plan.width, spec.scale);
                let processing_height = scaled_dimension(input_plan.height, spec.scale);
                let mut current = input;
                for level in 1..=spec.passes {
                    let divisor = 1_u32 << level.min(31);
                    let width = ceil_div(processing_width, divisor);
                    let height = ceil_div(processing_height, divisor);
                    let texture = builder.add_texture_with_layout(
                        GraphTextureSource::Intermediate,
                        input_plan.domain,
                        width,
                        height,
                        EffectWorkingSpace::LinearSrgb,
                    )?;
                    builder.add_pass(
                        RenderPassKind::DualKawaseDownsample,
                        vec![current],
                        Some(texture),
                        capture_damage.clone(),
                        instance.id,
                        instance.anchor,
                        Some(spec.radius),
                        instance.anchor_scope,
                    )?;
                    current = texture;
                }
                for level in (0..spec.passes).rev() {
                    let divisor = 1_u32 << level.min(31);
                    let width = ceil_div(processing_width, divisor);
                    let height = ceil_div(processing_height, divisor);
                    let texture = builder.add_texture_with_layout(
                        GraphTextureSource::Intermediate,
                        input_plan.domain,
                        width,
                        height,
                        EffectWorkingSpace::LinearSrgb,
                    )?;
                    builder.add_pass(
                        RenderPassKind::DualKawaseUpsample,
                        vec![current],
                        Some(texture),
                        output_damage.clone(),
                        instance.id,
                        instance.anchor,
                        Some(spec.radius),
                        instance.anchor_scope,
                    )?;
                    current = texture;
                }
                store_output(&mut outputs, slot, current)?;
            }
            EffectNodeKind::ColorMatrix(_)
            | EffectNodeKind::Tint(_)
            | EffectNodeKind::Noise(_)
            | EffectNodeKind::CustomFragment(_)
            | EffectNodeKind::Blend(_)
            | EffectNodeKind::Mask(_) => {
                let inputs = step
                    .input_slots
                    .iter()
                    .copied()
                    .map(|input_slot| resolve_output(&outputs, input_slot, node.id))
                    .collect::<Result<Vec<_>, _>>()?;
                let primary_input =
                    inputs
                        .first()
                        .copied()
                        .ok_or(RenderGraphCompileError::InvalidGraph(
                            EffectValidationError::MissingInputNode(node.id),
                        ))?;
                let primary_plan = builder.texture(primary_input);
                let multi_input = inputs.len() > 1;
                let mut normalized_inputs = Vec::with_capacity(inputs.len());
                for input in inputs {
                    let input_plan = builder.texture(input);
                    let needs_normalization = multi_input
                        && (input_plan.domain != primary_plan.domain
                            || input_plan.width != primary_plan.width
                            || input_plan.height != primary_plan.height
                            || input_plan.working_space != program.program.working_space
                            || input_plan.origin != primary_plan.origin);
                    if !needs_normalization {
                        normalized_inputs.push(input);
                        continue;
                    }
                    let normalized = builder.add_texture_with_layout(
                        GraphTextureSource::Intermediate,
                        primary_plan.domain,
                        primary_plan.width,
                        primary_plan.height,
                        program.program.working_space,
                    )?;
                    let normalized_damage = output_damage.expand_clamped_xy(
                        step.stage_footprint.sample_radius_x,
                        step.stage_footprint.sample_radius_y,
                        primary_plan.domain,
                    );
                    builder.add_pass(
                        RenderPassKind::NormalizeInput,
                        vec![input],
                        Some(normalized),
                        normalized_damage,
                        instance.id,
                        instance.anchor,
                        None,
                        instance.anchor_scope,
                    )?;
                    let normalize_pass = builder
                        .passes
                        .last_mut()
                        .expect("input normalization pass was appended");
                    normalize_pass.color_conversion =
                        color_conversion(input_plan.working_space, program.program.working_space);
                    normalize_pass.visual_group = visual_group;
                    normalized_inputs.push(normalized);
                }
                let inputs = normalized_inputs;
                let primary_input =
                    inputs
                        .first()
                        .copied()
                        .ok_or(RenderGraphCompileError::InvalidGraph(
                            EffectValidationError::MissingInputNode(node.id),
                        ))?;
                let input_plan = builder.texture(primary_input);
                let output = builder.add_texture_with_layout(
                    GraphTextureSource::Intermediate,
                    input_plan.domain,
                    input_plan.width,
                    input_plan.height,
                    program.program.working_space,
                )?;
                let kind = match node.kind {
                    EffectNodeKind::Blend(_) => RenderPassKind::Blend,
                    EffectNodeKind::Mask(_) => RenderPassKind::Mask,
                    _ => RenderPassKind::Fragment,
                };
                builder.add_stage_pass(
                    kind,
                    inputs,
                    Some(output),
                    output_damage.clone(),
                    instance.id,
                    instance.anchor,
                    node.kind.clone(),
                    instance.anchor_scope,
                )?;
                builder
                    .passes
                    .last_mut()
                    .expect("stage pass was appended")
                    .parameter_block = instance.parameter_block.clone();
                builder
                    .passes
                    .last_mut()
                    .expect("stage pass was appended")
                    .color_conversion = if multi_input {
                    EffectColorConversion::None
                } else {
                    color_conversion(input_plan.working_space, program.program.working_space)
                };
                builder
                    .passes
                    .last_mut()
                    .expect("stage pass was appended")
                    .visual_group = visual_group;
                store_output(&mut outputs, slot, output)?;
            }
        }
    }

    let final_texture = outputs
        .get(program.lowering.output_slot)
        .copied()
        .flatten()
        .ok_or(RenderGraphCompileError::InvalidGraph(
            EffectValidationError::MissingOutputNode,
        ))?;
    let kind = match instance.anchor {
        EffectAnchor::OutputPostProcess => RenderPassKind::OutputPostProcess,
        EffectAnchor::BeforeSurface(_)
        | EffectAnchor::ReplaceSurface(_)
        | EffectAnchor::AfterSurface(_) => RenderPassKind::Composite,
    };
    let composite = builder.add_pass(
        kind,
        vec![final_texture],
        Some(output_texture),
        output_damage.clone(),
        instance.id,
        instance.anchor,
        None,
        instance.anchor_scope,
    )?;
    let final_working_space = builder.texture(final_texture).working_space;
    let encode_output = final_working_space == EffectWorkingSpace::LinearSrgb;
    let final_pass = builder
        .passes
        .last_mut()
        .expect("final composite pass was appended");
    final_pass.alpha_mode = program.program.alpha_mode;
    final_pass.color_conversion =
        color_conversion(final_working_space, EffectWorkingSpace::OutputEncodedSrgb);
    final_pass.encode_output =
        encode_output || final_pass.color_conversion == EffectColorConversion::EncodeLinearToSrgb;
    final_pass.visual_group = visual_group;
    final_pass.visible_clip_fallback =
        output_clip_fallback.map(|fallback| (fallback.input_rect_count, fallback.clip_rect_count));
    Ok(composite)
}

fn color_conversion(
    input: EffectWorkingSpace,
    output: EffectWorkingSpace,
) -> EffectColorConversion {
    match (input, output) {
        (EffectWorkingSpace::OutputEncodedSrgb, EffectWorkingSpace::LinearSrgb) => {
            EffectColorConversion::DecodeSrgbToLinear
        }
        (EffectWorkingSpace::LinearSrgb, EffectWorkingSpace::OutputEncodedSrgb) => {
            EffectColorConversion::EncodeLinearToSrgb
        }
        _ => EffectColorConversion::None,
    }
}

fn scaled_dimension(value: u32, scale: f32) -> u32 {
    (f64::from(value) * f64::from(scale)).ceil().max(1.0) as u32
}

fn ceil_div(value: u32, divisor: u32) -> u32 {
    value.saturating_add(divisor.saturating_sub(1)) / divisor.max(1)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::compositor::{
        EffectAnchor, EffectAnchorScope, EffectSceneOrder, ResolvedEffectInstance,
        ResolvedEffectScene,
    };
    use crate::effects::*;

    fn blur_scene() -> (ResolvedEffectScene, EffectRegistry) {
        blur_scene_with_region(EffectRegion::from_rect(
            EffectRect::new(100, 80, 320, 180).unwrap(),
        ))
    }

    fn blur_scene_with_region(region: EffectRegion) -> (ResolvedEffectScene, EffectRegistry) {
        blur_scene_with_region_and_scale(region, 1.0)
    }

    fn blur_scene_with_region_and_scale(
        region: EffectRegion,
        scale: f32,
    ) -> (ResolvedEffectScene, EffectRegistry) {
        let source = EffectNodeId::new(1).unwrap();
        let blur = EffectNodeId::new(2).unwrap();
        let program = EffectProgram {
            id: EffectProgramId::new(1).unwrap(),
            nodes: vec![
                EffectNode::source(source, EffectSource::Backdrop),
                EffectNode::dual_kawase(
                    blur,
                    source,
                    DualKawaseBlurSpec::new(4.0, 2, scale).unwrap(),
                ),
            ],
            output: blur,
            working_space: EffectWorkingSpace::LinearSrgb,
            alpha_mode: EffectAlphaMode::Opaque,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        };
        let validated = validate_effect_program(program).unwrap();
        let mut registry = EffectRegistry::empty();
        registry.insert(validated).unwrap();
        let instance = ResolvedEffectInstance {
            id: EffectInstanceId::new(1).unwrap(),
            program: EffectProgramId::new(1).unwrap(),
            anchor: EffectAnchor::BeforeSurface(1),
            target_bounds: region.bounding_rect().unwrap(),
            region,
            parameter_block: EffectParameterBlock::default(),
            signature: 7,
            frame_demand: EffectFrameDemand::OnDamage,
            visual_group: None,
            anchor_scope: EffectAnchorScope::VisualGroup,
            scene_order: EffectSceneOrder::for_anchor(EffectAnchor::BeforeSurface(1)),
        };
        (ResolvedEffectScene::new(1, vec![instance]), registry)
    }

    fn separated_blur_scene() -> (ResolvedEffectScene, EffectRegistry) {
        let (scene, registry) = blur_scene();
        let first = scene.instances[0].clone();
        let mut second = first.clone();
        second.id = EffectInstanceId::new(2).unwrap();
        second.region = EffectRegion::from_rect(EffectRect::new(1200, 80, 320, 180).unwrap());
        second.target_bounds = second.region.bounding_rect().unwrap();
        (ResolvedEffectScene::new(1, vec![first, second]), registry)
    }

    fn test_instance(
        program: EffectProgramId,
        id: u64,
        region: EffectRegion,
    ) -> ResolvedEffectInstance {
        ResolvedEffectInstance {
            id: EffectInstanceId::new(id).expect("test instance id is non-zero"),
            program,
            anchor: EffectAnchor::OutputPostProcess,
            target_bounds: region.bounding_rect().expect("test effect bounds"),
            region,
            parameter_block: EffectParameterBlock::default(),
            signature: id,
            frame_demand: EffectFrameDemand::OnDamage,
            visual_group: None,
            anchor_scope: EffectAnchorScope::VisualGroup,
            scene_order: EffectSceneOrder::for_anchor(EffectAnchor::OutputPostProcess),
        }
    }

    fn target_content_registry() -> EffectRegistry {
        let source = EffectNodeId::new(1).unwrap();
        let program = validate_effect_program(EffectProgram {
            id: EffectProgramId::new(14).unwrap(),
            nodes: vec![EffectNode::source(source, EffectSource::TargetContent)],
            output: source,
            working_space: EffectWorkingSpace::OutputEncodedSrgb,
            alpha_mode: EffectAlphaMode::Preserve,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        })
        .unwrap();
        let mut registry = EffectRegistry::empty();
        registry.insert(program).unwrap();
        registry
    }

    fn fused_local_stage_scene(instance_count: usize) -> (ResolvedEffectScene, EffectRegistry) {
        let source = EffectNodeId::new(1).unwrap();
        let matrix = EffectNodeId::new(2).unwrap();
        let tint = EffectNodeId::new(3).unwrap();
        let mut nodes = vec![
            EffectNode::source(source, EffectSource::Backdrop),
            EffectNode::color_matrix(
                matrix,
                source,
                ColorMatrixSpec {
                    matrix: [
                        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0,
                        1.0,
                    ],
                    bias: [0.0; 4],
                },
            )
            .unwrap(),
            EffectNode::tint(tint, matrix, TintSpec::WHITE),
        ];
        let custom = EffectNodeId::new(4).unwrap();
        nodes.push(
            EffectNode::custom_fragment(
                custom,
                tint,
                CustomFragmentSpec {
                    shader: ShaderModuleId::new(12).unwrap(),
                    declared_footprint: EffectFootprint::symmetric(0),
                    uniforms: Vec::new(),
                    auxiliary_inputs: Vec::new(),
                },
            )
            .unwrap(),
        );
        let output = custom;
        let program = validate_effect_program(EffectProgram {
            id: EffectProgramId::new(13).unwrap(),
            nodes,
            output,
            working_space: EffectWorkingSpace::LinearSrgb,
            alpha_mode: EffectAlphaMode::Opaque,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        })
        .unwrap();
        let mut registry = EffectRegistry::empty();
        registry.insert(program).unwrap();
        let instances = (0..instance_count)
            .map(|index| {
                let region = EffectRegion::from_rect(
                    EffectRect::new(10 + (index as i32) * 40, 10, 20, 20)
                        .expect("test effect region"),
                );
                ResolvedEffectInstance {
                    id: EffectInstanceId::new(
                        u64::try_from(index + 1).expect("test instance id fits"),
                    )
                    .expect("test instance id is non-zero"),
                    program: EffectProgramId::new(13).unwrap(),
                    anchor: EffectAnchor::OutputPostProcess,
                    target_bounds: region.bounding_rect().unwrap(),
                    region,
                    parameter_block: EffectParameterBlock::default(),
                    signature: u64::try_from(index + 1).expect("test signature fits"),
                    frame_demand: EffectFrameDemand::OnDamage,
                    visual_group: None,
                    anchor_scope: EffectAnchorScope::VisualGroup,
                    scene_order: EffectSceneOrder::for_anchor(EffectAnchor::OutputPostProcess),
                }
            })
            .collect();
        (ResolvedEffectScene::new(1, instances), registry)
    }

    fn lifetime_texture(
        id: u16,
        first_use: Option<u16>,
        last_use: Option<u16>,
        source: GraphTextureSource,
    ) -> GraphTexturePlan {
        GraphTexturePlan {
            id: GraphTextureId::new(id).expect("test texture id must be non-zero"),
            source,
            width: 1,
            height: 1,
            domain: EffectRect::new(0, 0, 1, 1).expect("test texture domain"),
            working_space: EffectWorkingSpace::LinearSrgb,
            origin: GraphTextureOrigin::BottomLeft,
            first_use: first_use
                .map(|value| GraphPassId::new(value).expect("test pass id must be non-zero")),
            last_use: last_use
                .map(|value| GraphPassId::new(value).expect("test pass id must be non-zero")),
        }
    }

    fn test_pass(id: u16) -> CompiledRenderPass {
        CompiledRenderPass {
            id: GraphPassId::new(id).expect("test pass id must be non-zero"),
            kind: RenderPassKind::Fragment,
            inputs: Vec::new(),
            output: None,
            damage: EffectRegion::empty(),
            instance: EffectInstanceId::new(1).expect("test instance id must be non-zero"),
            anchor: EffectAnchor::OutputPostProcess,
            blur_radius: None,
            stage: None,
            fused_stages: Vec::new(),
            parameter_block: EffectParameterBlock::default(),
            alpha_mode: EffectAlphaMode::Preserve,
            encode_output: false,
            color_conversion: EffectColorConversion::None,
            checkpoint_dependencies: Vec::new(),
            visual_group: None,
            anchor_scope: EffectAnchorScope::VisualGroup,
            visible_clip_fallback: None,
        }
    }

    fn sampling_texture(
        id: u16,
        source: GraphTextureSource,
        domain: EffectRect,
        width: u32,
        height: u32,
    ) -> GraphTexturePlan {
        GraphTexturePlan {
            id: GraphTextureId::new(id).expect("test texture id must be non-zero"),
            source,
            width,
            height,
            domain,
            working_space: EffectWorkingSpace::LinearSrgb,
            origin: GraphTextureOrigin::BottomLeft,
            first_use: None,
            last_use: None,
        }
    }

    fn sampling_pass(
        id: u16,
        kind: RenderPassKind,
        input: GraphTextureId,
        output: GraphTextureId,
        radius: f32,
    ) -> CompiledRenderPass {
        let mut pass = test_pass(id);
        pass.kind = kind;
        pass.inputs = vec![input];
        pass.output = Some(output);
        pass.blur_radius = Some(radius);
        pass
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the independent oracle spells out each coordinate-space input"
    )]
    fn reference_output_center_to_input_texel(
        output_center: f64,
        output_origin: i32,
        output_domain_size: u32,
        output_size: u32,
        input_origin: i32,
        input_domain_size: u32,
        input_size: u32,
        logical_mapping: bool,
    ) -> f64 {
        if logical_mapping {
            let logical_position = f64::from(output_origin)
                + output_center * f64::from(output_domain_size) / f64::from(output_size);
            (logical_position - f64::from(input_origin)) * f64::from(input_size)
                / f64::from(input_domain_size)
                - 0.5
        } else {
            output_center * f64::from(input_size) / f64::from(output_size) - 0.5
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the independent oracle spells out each sampling-space input"
    )]
    fn reference_sampled_texels(
        kind: RenderPassKind,
        output_coverage: GraphTexturePhysicalRect,
        output: &GraphTexturePlan,
        input: &GraphTexturePlan,
        radius_x: f64,
        radius_y: f64,
        logical_mapping: bool,
        linear_filter_support: bool,
    ) -> BTreeSet<(u32, u32)> {
        let offsets = match kind {
            RenderPassKind::DualKawaseDownsample => vec![
                (radius_x, radius_y),
                (-radius_x, -radius_y),
                (radius_x, -radius_y),
                (-radius_x, radius_y),
            ],
            RenderPassKind::DualKawaseUpsample => vec![
                (0.0, 0.0),
                (radius_x, 0.0),
                (-radius_x, 0.0),
                (0.0, radius_y),
                (0.0, -radius_y),
            ],
            _ => vec![(0.0, 0.0)],
        };
        let mut sampled = BTreeSet::new();
        for output_y in output_coverage.top..output_coverage.bottom {
            for output_x in output_coverage.left..output_coverage.right {
                let center_x = reference_output_center_to_input_texel(
                    f64::from(output_x) + 0.5,
                    output.domain.x,
                    output.domain.width,
                    output.width,
                    input.domain.x,
                    input.domain.width,
                    input.width,
                    logical_mapping,
                );
                let center_y = reference_output_center_to_input_texel(
                    f64::from(output_y) + 0.5,
                    output.domain.y,
                    output.domain.height,
                    output.height,
                    input.domain.y,
                    input.domain.height,
                    input.height,
                    logical_mapping,
                );
                for (offset_x, offset_y) in &offsets {
                    let sample_x = center_x + offset_x;
                    let sample_y = center_y + offset_y;
                    let base_x = sample_x.floor() as i64;
                    let base_y = sample_y.floor() as i64;
                    let extra = if linear_filter_support { 1 } else { 0 };
                    for x_offset in 0..=extra {
                        for y_offset in 0..=extra {
                            let texel_x = base_x
                                .saturating_add(i64::from(x_offset))
                                .clamp(0, i64::from(input.width - 1))
                                as u32;
                            let texel_y = base_y
                                .saturating_add(i64::from(y_offset))
                                .clamp(0, i64::from(input.height - 1))
                                as u32;
                            sampled.insert((texel_x, texel_y));
                        }
                    }
                }
            }
        }
        sampled
    }

    fn physical_region_contains(
        region: &EffectRegion,
        texture: &GraphTexturePlan,
        x: u32,
        y: u32,
    ) -> bool {
        region.rects().iter().any(|rect| {
            logical_rect_to_physical_coverage(*rect, texture)
                .is_some_and(|coverage| coverage.contains(x, y))
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "coverage sweeps keep dimensions, origins, and mapping explicit"
    )]
    fn assert_sample_coverage(
        kind: RenderPassKind,
        input_width: u32,
        input_height: u32,
        output_width: u32,
        output_height: u32,
        output_origin: (i32, i32),
        input_origin: (i32, i32),
        logical_mapping: bool,
    ) {
        let input = sampling_texture(
            1,
            GraphTextureSource::CapturedScene,
            EffectRect::new(input_origin.0, input_origin.1, input_width, input_height)
                .expect("input domain"),
            input_width,
            input_height,
        );
        let output = sampling_texture(
            2,
            GraphTextureSource::Intermediate,
            EffectRect::new(output_origin.0, output_origin.1, input_width, input_height)
                .expect("output domain"),
            output_width,
            output_height,
        );
        let radius: f64 = match kind {
            RenderPassKind::DualKawaseDownsample | RenderPassKind::DualKawaseUpsample => 4.0,
            _ => 0.0,
        };
        let pass = sampling_pass(1, kind, input.id, output.id, radius as f32);
        let mut xs = if output_width <= 16 {
            (0..output_width).collect()
        } else {
            vec![0, output_width - 1, 1, output_width - 2]
        };
        let mut ys = if output_height <= 16 {
            (0..output_height).collect()
        } else {
            vec![0, output_height - 1, 1, output_height - 2]
        };
        xs.sort_unstable();
        xs.dedup();
        ys.sort_unstable();
        ys.dedup();

        for output_x in xs {
            for output_y in &ys {
                let demanded = physical_coverage_to_logical_rect(
                    GraphTexturePhysicalRect {
                        left: output_x,
                        top: *output_y,
                        right: output_x + 1,
                        bottom: *output_y + 1,
                    },
                    &output,
                )
                .map(EffectRegion::from_rect)
                .expect("edge output pixel has a logical cover");
                let output_coverage =
                    logical_rect_to_physical_coverage(demanded.rects()[0], &output)
                        .expect("logical output demand rasterizes");
                let expected = reference_sampled_texels(
                    kind,
                    output_coverage,
                    &output,
                    &input,
                    radius,
                    radius,
                    logical_mapping,
                    true,
                );
                let required = required_input_region(&pass, &demanded, &output, &input)
                    .expect("sampled edge maps to input demand");
                for (sampled_x, sampled_y) in expected {
                    assert!(
                        physical_region_contains(&required, &input, sampled_x, sampled_y),
                        "{kind:?} {input_width}x{input_height}->{output_width}x{output_height} at output ({output_x},{output_y}) misses input ({sampled_x},{sampled_y})"
                    );
                }
            }
        }
    }

    #[test]
    fn pass_position_map_uses_actual_maximum_id() {
        let passes = [1, 2, 4, 5].into_iter().map(test_pass).collect::<Vec<_>>();

        let position_by_id = build_pass_position_map(&passes);

        assert_eq!(position_by_id.len(), 6);
    }

    #[test]
    fn pass_position_map_preserves_high_gapped_ids() {
        let passes = [1, 3, 8].into_iter().map(test_pass).collect::<Vec<_>>();

        let position_by_id = build_pass_position_map(&passes);

        assert_eq!(position_by_id[1], Some(0));
        assert_eq!(position_by_id[3], Some(1));
        assert_eq!(position_by_id[8], Some(2));
        assert_eq!(position_by_id[2], None);
        assert_eq!(position_by_id[7], None);
    }

    fn fusion_candidate_builder(additional_input: bool, wrong_input: bool) -> GraphBuilder {
        let bounds = EffectRect::new(0, 0, 16, 16).expect("test bounds");
        let (mut builder, source) = GraphBuilder::new(bounds).expect("test graph builder");
        let first_output = builder
            .add_texture_with_layout(
                GraphTextureSource::Intermediate,
                bounds,
                bounds.width,
                bounds.height,
                EffectWorkingSpace::LinearSrgb,
            )
            .expect("first output texture");
        let second_output = builder
            .add_texture_with_layout(
                GraphTextureSource::Intermediate,
                bounds,
                bounds.width,
                bounds.height,
                EffectWorkingSpace::LinearSrgb,
            )
            .expect("second output texture");
        let unrelated = builder
            .add_texture_with_layout(
                GraphTextureSource::Intermediate,
                bounds,
                bounds.width,
                bounds.height,
                EffectWorkingSpace::LinearSrgb,
            )
            .expect("unrelated input texture");
        let instance = EffectInstanceId::new(1).expect("test instance id");
        let damage = EffectRegion::from_rect(bounds);
        let anchor = EffectAnchor::OutputPostProcess;
        let anchor_scope = EffectAnchorScope::VisualGroup;
        builder
            .add_stage_pass(
                RenderPassKind::Fragment,
                vec![source],
                Some(first_output),
                damage.clone(),
                instance,
                anchor,
                EffectNodeKind::ColorMatrix(ColorMatrixSpec::IDENTITY),
                anchor_scope,
            )
            .expect("first stage pass");
        let second_id = GraphPassId::new(2).expect("test second pass id");
        builder.touch(first_output, second_id);
        let second_inputs = if wrong_input {
            vec![unrelated]
        } else if additional_input {
            vec![first_output, unrelated]
        } else {
            vec![first_output]
        };
        builder
            .add_stage_pass(
                RenderPassKind::Fragment,
                second_inputs,
                Some(second_output),
                damage,
                instance,
                anchor,
                EffectNodeKind::Tint(TintSpec::WHITE),
                anchor_scope,
            )
            .expect("second stage pass");
        builder
    }

    #[test]
    fn fusion_accepts_exact_single_matching_input() {
        let mut builder = fusion_candidate_builder(false, false);

        fuse_compatible_local_stages(&mut builder);

        assert_eq!(builder.passes.len(), 1);
        assert_eq!(builder.passes[0].fused_stages.len(), 1);
    }

    #[test]
    fn fusion_rejects_additional_second_input() {
        let mut builder = fusion_candidate_builder(true, false);

        fuse_compatible_local_stages(&mut builder);

        assert_eq!(builder.passes.len(), 2);
    }

    #[test]
    fn fusion_rejects_wrong_single_input() {
        let mut builder = fusion_candidate_builder(false, true);

        fuse_compatible_local_stages(&mut builder);

        assert_eq!(builder.passes.len(), 2);
    }

    fn brute_force_peak(passes: &[CompiledRenderPass], textures: &[GraphTexturePlan]) -> usize {
        passes
            .iter()
            .enumerate()
            .map(|(current_position, _)| {
                textures
                    .iter()
                    .filter(|texture| {
                        texture.source == GraphTextureSource::Intermediate
                            && texture.first_use.zip(texture.last_use).is_some_and(
                                |(first, last)| {
                                    let first_position = passes
                                        .iter()
                                        .position(|pass| pass.id == first)
                                        .expect("reference lifetime first pass must survive");
                                    let last_position = passes
                                        .iter()
                                        .position(|pass| pass.id == last)
                                        .expect("reference lifetime last pass must survive");
                                    first_position <= current_position
                                        && current_position <= last_position
                                },
                            )
                    })
                    .count()
            })
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn interval_sweep_matches_brute_force_reference_fixtures() {
        let fixtures = [
            (
                1,
                vec![lifetime_texture(
                    1,
                    Some(1),
                    Some(1),
                    GraphTextureSource::Intermediate,
                )],
            ),
            (
                4,
                vec![
                    lifetime_texture(1, Some(1), Some(3), GraphTextureSource::Intermediate),
                    lifetime_texture(2, Some(2), Some(4), GraphTextureSource::Intermediate),
                ],
            ),
            (
                6,
                vec![
                    lifetime_texture(1, Some(1), Some(2), GraphTextureSource::Intermediate),
                    lifetime_texture(2, Some(4), Some(6), GraphTextureSource::Intermediate),
                ],
            ),
            (
                6,
                vec![
                    lifetime_texture(1, Some(1), Some(6), GraphTextureSource::Intermediate),
                    lifetime_texture(2, Some(2), Some(5), GraphTextureSource::Intermediate),
                    lifetime_texture(3, Some(3), Some(4), GraphTextureSource::Intermediate),
                ],
            ),
            (
                3,
                vec![
                    lifetime_texture(1, Some(2), Some(2), GraphTextureSource::Intermediate),
                    lifetime_texture(2, None, None, GraphTextureSource::Intermediate),
                    lifetime_texture(3, Some(1), Some(3), GraphTextureSource::Output),
                ],
            ),
        ];

        for (pass_count, textures) in fixtures {
            let passes = (1..=pass_count)
                .map(|id| test_pass(u16::try_from(id).expect("test pass count fits")))
                .collect::<Vec<_>>();
            assert_eq!(
                peak_live_intermediates(&passes, &textures),
                brute_force_peak(&passes, &textures),
                "fixture with {pass_count} passes"
            );
        }
    }

    #[test]
    fn post_fusion_peak_live_handles_gapped_ids_and_inclusive_lifetimes() {
        let fixtures = [
            (
                vec![1, 2, 4, 5],
                vec![lifetime_texture(
                    1,
                    Some(4),
                    Some(5),
                    GraphTextureSource::Intermediate,
                )],
                1,
            ),
            (
                vec![1, 3, 5],
                vec![lifetime_texture(
                    1,
                    Some(5),
                    Some(5),
                    GraphTextureSource::Intermediate,
                )],
                1,
            ),
            (
                vec![1, 3, 6, 8],
                vec![
                    lifetime_texture(1, Some(3), Some(6), GraphTextureSource::Intermediate),
                    lifetime_texture(2, Some(6), Some(6), GraphTextureSource::Intermediate),
                ],
                2,
            ),
        ];

        for (ids, textures, expected_peak) in fixtures {
            let passes = ids.into_iter().map(test_pass).collect::<Vec<_>>();
            assert_eq!(
                brute_force_peak(&passes, &textures),
                expected_peak,
                "semantic reference fixture should define the expected peak"
            );
            assert_eq!(
                peak_live_intermediates(&passes, &textures),
                brute_force_peak(&passes, &textures),
                "stable IDs must resolve through surviving pass order"
            );
        }
    }

    #[test]
    fn no_visible_effects_compile_to_legacy_scene() {
        let plan = compile_frame_execution_plan(
            &ResolvedEffectScene::default(),
            &EffectRegion::empty(),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &EffectRegistry::empty(),
        )
        .unwrap();
        assert!(matches!(plan, FrameExecutionPlan::LegacyScene));
    }

    #[test]
    fn compile_counts_only_visible_effect_instances() {
        let (mut scene, registry) = fused_local_stage_scene(3);
        scene.instances[1].region = EffectRegion::empty();
        let source_damage = EffectRegion::from_rect(EffectRect::new(0, 0, 200, 100).unwrap());
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &source_damage,
            EffectRect::new(0, 0, 200, 100).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("visible effects must compile to an effect graph");
        };

        assert_eq!(graph.stats.effect_instances, 2);
        assert_eq!(graph.instances.len(), 2);
    }

    #[test]
    fn builtin_background_blur_has_stable_identity_and_two_pass_shape() {
        let registry = EffectRegistry::with_builtin_background_blur();
        let id = builtin_background_blur_program_id();
        let program = &registry
            .get(id)
            .expect("builtin blur must be registered")
            .program;

        assert_eq!(BUILTIN_BACKGROUND_BLUR_NAME, "system.background_blur");
        assert_eq!(program.id, id);
        assert_eq!(program.nodes.len(), 2);
        assert!(matches!(
            program.nodes[1].kind,
            EffectNodeKind::DualKawaseBlur(_)
        ));
    }

    #[test]
    fn encoded_source_only_result_does_not_double_encode() {
        let source = EffectNodeId::new(1).unwrap();
        let program = validate_effect_program(EffectProgram {
            id: EffectProgramId::new(19).unwrap(),
            nodes: vec![EffectNode::source(source, EffectSource::Backdrop)],
            output: source,
            working_space: EffectWorkingSpace::OutputEncodedSrgb,
            alpha_mode: EffectAlphaMode::Preserve,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        })
        .unwrap();
        let mut registry = EffectRegistry::empty();
        registry.insert(program).unwrap();
        let region = EffectRegion::from_rect(EffectRect::new(10, 10, 20, 20).unwrap());
        let scene = ResolvedEffectScene::new(
            1,
            vec![ResolvedEffectInstance {
                id: EffectInstanceId::new(1).unwrap(),
                program: EffectProgramId::new(19).unwrap(),
                anchor: EffectAnchor::OutputPostProcess,
                target_bounds: region.bounding_rect().unwrap(),
                region: region.clone(),
                parameter_block: EffectParameterBlock::default(),
                signature: 1,
                frame_demand: EffectFrameDemand::OnDamage,
                visual_group: None,
                anchor_scope: EffectAnchorScope::VisualGroup,
                scene_order: EffectSceneOrder::for_anchor(EffectAnchor::OutputPostProcess),
            }],
        );
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &region,
            EffectRect::new(0, 0, 100, 100).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("visible source effect must compile");
        };
        let final_pass = graph.passes.last().unwrap();
        assert!(!final_pass.encode_output);
        assert_eq!(final_pass.alpha_mode, EffectAlphaMode::Preserve);
        assert_eq!(final_pass.color_conversion, EffectColorConversion::None);
    }

    #[test]
    fn two_pass_blur_has_semantic_topology() {
        let (scene, registry) = blur_scene();
        let plan = compile_frame_execution_plan(
            &scene,
            &EffectRegion::from_rect(EffectRect::new(100, 80, 320, 180).unwrap()),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap();
        let FrameExecutionPlan::EffectGraph(graph) = plan else {
            panic!("visible effects must compile to an effect graph");
        };
        assert_eq!(
            graph
                .passes
                .iter()
                .map(|pass| pass.kind)
                .collect::<Vec<_>>(),
            vec![
                RenderPassKind::SceneCapture,
                RenderPassKind::DualKawaseDownsample,
                RenderPassKind::DualKawaseDownsample,
                RenderPassKind::DualKawaseUpsample,
                RenderPassKind::DualKawaseUpsample,
                RenderPassKind::Composite,
            ]
        );
    }

    #[test]
    fn intermediate_textures_expose_first_and_last_use() {
        let (scene, registry) = blur_scene();
        let plan = compile_frame_execution_plan(
            &scene,
            &EffectRegion::from_rect(EffectRect::new(100, 80, 320, 180).unwrap()),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap();
        let FrameExecutionPlan::EffectGraph(graph) = plan else {
            panic!("visible effects must compile to an effect graph");
        };
        assert!(
            graph
                .textures
                .iter()
                .filter(|texture| texture.source == GraphTextureSource::Intermediate)
                .all(|texture| texture.first_use.is_some() && texture.last_use.is_some())
        );
        assert!(graph.stats.peak_live_intermediates > 0);
    }

    #[test]
    fn dual_kawase_uses_decreasing_then_increasing_physical_levels() {
        let (scene, registry) = blur_scene();
        let plan = compile_frame_execution_plan(
            &scene,
            &EffectRegion::from_rect(EffectRect::new(100, 80, 320, 180).unwrap()),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap();
        let FrameExecutionPlan::EffectGraph(graph) = plan else {
            panic!("visible effects must compile to an effect graph");
        };
        let dimensions = graph
            .textures
            .iter()
            .filter(|texture| texture.source == GraphTextureSource::Intermediate)
            .map(|texture| (texture.width, texture.height))
            .collect::<Vec<_>>();
        assert_eq!(
            dimensions,
            vec![(184, 114), (92, 57), (184, 114), (368, 228)]
        );
        assert!(
            graph
                .passes
                .iter()
                .filter(|pass| matches!(
                    pass.kind,
                    RenderPassKind::DualKawaseDownsample | RenderPassKind::DualKawaseUpsample
                ))
                .all(|pass| pass.blur_radius == Some(4.0))
        );
    }

    #[test]
    fn backdrop_capture_is_local_to_the_effect_dependency_domain() {
        let (scene, registry) = blur_scene();
        let plan = compile_frame_execution_plan(
            &scene,
            &EffectRegion::from_rect(EffectRect::new(100, 80, 320, 180).unwrap()),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap();
        let FrameExecutionPlan::EffectGraph(graph) = plan else {
            panic!("visible effects must compile to an effect graph");
        };
        let capture = graph
            .textures
            .iter()
            .find(|texture| texture.source == GraphTextureSource::CapturedScene)
            .unwrap();
        assert_eq!(capture.domain, EffectRect::new(76, 56, 368, 228).unwrap());
    }

    #[test]
    fn overlapping_backdrop_stacks_expose_ordered_checkpoint_dependencies() {
        let (scene, registry) = blur_scene();
        let mut second = scene.instances[0].clone();
        second.id = EffectInstanceId::new(2).unwrap();
        second.signature = second.signature.saturating_add(1);
        let scene = ResolvedEffectScene::new(1, vec![scene.instances[0].clone(), second]);
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &EffectRegion::from_rect(EffectRect::new(100, 80, 320, 180).unwrap()),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("overlapping effects must compile to an effect graph");
        };
        let captures = graph
            .passes
            .iter()
            .filter(|pass| pass.kind == RenderPassKind::SceneCapture)
            .collect::<Vec<_>>();
        assert_eq!(captures.len(), 2);
        assert!(captures[0].checkpoint_dependencies.is_empty());
        assert_eq!(captures[1].checkpoint_dependencies.len(), 1);
        assert!(graph.passes.iter().any(|pass| {
            pass.kind == RenderPassKind::Composite
                && captures[1].checkpoint_dependencies.contains(&pass.id)
        }));
    }

    fn repeated_region(rect: EffectRect, count: usize) -> EffectRegion {
        let mut region = EffectRegion::empty();
        for _ in 0..count {
            region.push(rect);
        }
        region
    }

    fn region_area(region: &EffectRegion) -> u64 {
        region.rects().iter().fold(0_u64, |total, rect| {
            total.saturating_add(u64::from(rect.width) * u64::from(rect.height))
        })
    }

    fn translated_region(region: &EffectRegion, dx: i32, dy: i32) -> EffectRegion {
        let mut translated = EffectRegion::empty();
        for rect in region.rects() {
            translated.push(
                EffectRect::new(rect.x + dx, rect.y + dy, rect.width, rect.height)
                    .expect("translated test rectangle is valid"),
            );
        }
        translated
    }

    fn relative_rects(region: &EffectRegion, dx: i32, dy: i32) -> Vec<EffectRect> {
        region
            .rects()
            .iter()
            .map(|rect| {
                EffectRect::new(rect.x - dx, rect.y - dy, rect.width, rect.height)
                    .expect("relative test rectangle is valid")
            })
            .collect()
    }

    fn demand_test_instance(
        id: u64,
        output_influence_region: EffectRegion,
        capture_region: EffectRegion,
        dependencies: Vec<EffectInstanceId>,
    ) -> CompiledEffectInstance {
        CompiledEffectInstance {
            id: EffectInstanceId::new(id).unwrap(),
            output_influence_region,
            capture_region,
            dependencies,
        }
    }

    fn demand_test_graph(instances: Vec<CompiledEffectInstance>) -> CompiledFrameGraph {
        CompiledFrameGraph {
            passes: Vec::new(),
            textures: Vec::new(),
            instances,
            final_damage: EffectRegion::empty(),
            stats: RenderGraphCompileStats::default(),
        }
    }

    fn region_covers_region(container: &EffectRegion, required: &EffectRegion) -> bool {
        if required.is_empty() {
            return true;
        }
        if required.bounding_rect().is_none() {
            return false;
        }
        required.rects().iter().all(|rect| {
            let right = rect.right().saturating_sub(1);
            let bottom = rect.bottom().saturating_sub(1);
            [
                (rect.x, rect.y),
                (right, rect.y),
                (rect.x, bottom),
                (right, bottom),
            ]
            .into_iter()
            .all(|(x, y)| container.contains_point(x, y))
        })
    }

    fn region_is_subset_of(region: &EffectRegion, clip: &EffectRegion) -> bool {
        region.rects().iter().all(|rect| {
            (rect.y..rect.bottom())
                .all(|y| (rect.x..rect.right()).all(|x| clip.contains_point(x, y)))
        })
    }

    fn semantically_equal_regions(left: &EffectRegion, right: &EffectRegion) -> bool {
        left.subtract(right).is_empty() && right.subtract(left).is_empty()
    }

    #[test]
    fn fragmented_dependency_coverage_is_propagated_once() {
        let rect = EffectRect::new(20, 30, 100, 80).unwrap();
        let first_id = EffectInstanceId::new(1).unwrap();
        let second_id = EffectInstanceId::new(2).unwrap();
        let graph = demand_test_graph(vec![
            demand_test_instance(
                1,
                EffectRegion::from_rect(rect),
                EffectRegion::from_rect(rect),
                Vec::new(),
            ),
            demand_test_instance(
                2,
                EffectRegion::from_rect(rect),
                EffectRegion::from_rect(rect),
                vec![first_id],
            ),
        ]);
        let repair = repeated_region(rect, 71);

        let demand = plan_effect_execution_demand(&graph, &repair, false);

        assert!(demand.contains(first_id));
        assert!(demand.contains(second_id));
        assert_eq!(demand.plan_stats().dependency_edge_count, 1);
        assert_eq!(demand.plan_stats().dependency_propagations, 1);
        assert!(demand.plan_stats().max_instance_region_rect_count <= 128);
    }

    #[test]
    fn fragmented_visible_repair_falls_back_to_output_influence_without_bbox() {
        let instance = EffectInstanceId::new(1).unwrap();
        let mut output = EffectRegion::from_rect(EffectRect::new(0, 0, 200, 10).unwrap());
        output.push(EffectRect::new(300, 0, 200, 10).unwrap());
        let graph = demand_test_graph(vec![demand_test_instance(
            1,
            output.clone(),
            output.clone(),
            Vec::new(),
        )]);
        let mut repair = EffectRegion::empty();
        for index in 0..128 {
            repair.push(EffectRect::new(index, 0, 500 - index as u32, 10).unwrap());
        }

        let demand = plan_effect_execution_demand(&graph, &repair, false);

        assert_eq!(demand.output_region(instance), Some(&output));
        assert!(region_is_subset_of(
            demand.output_region(instance).unwrap(),
            &output
        ));
        assert!(
            !demand
                .output_region(instance)
                .unwrap()
                .contains_point(200, 0)
        );
        assert_eq!(demand.plan_stats().region_representation_overflows, 1);
        assert_eq!(demand.plan_stats().visible_clip_fallbacks, 1);
        assert_eq!(
            demand.visible_clip_fallbacks(),
            &[EffectVisibleClipFallback {
                instance,
                pass: None,
                input_rect_count: 128,
                clip_rect_count: 2,
            }]
        );
    }

    #[test]
    fn fragmented_backdrop_dependency_falls_back_within_dependency_output() {
        let lower = EffectInstanceId::new(1).unwrap();
        let mut lower_output = EffectRegion::from_rect(EffectRect::new(1000, 0, 200, 1).unwrap());
        lower_output.push(EffectRect::new(1400, 0, 100, 1).unwrap());
        let consumer_output = EffectRegion::from_rect(EffectRect::new(0, 0, 10, 1).unwrap());
        let mut consumer_capture = EffectRegion::empty();
        for index in 0..128 {
            consumer_capture.push(EffectRect::new(1000 + index, 0, 500 - index as u32, 1).unwrap());
        }
        let graph = demand_test_graph(vec![
            demand_test_instance(1, lower_output.clone(), lower_output.clone(), Vec::new()),
            demand_test_instance(2, consumer_output.clone(), consumer_capture, vec![lower]),
        ]);

        let demand = plan_effect_execution_demand(&graph, &consumer_output, false);

        assert_eq!(demand.output_region(lower), Some(&lower_output));
        assert!(region_is_subset_of(
            demand.output_region(lower).unwrap(),
            &lower_output
        ));
        assert!(!demand.output_region(lower).unwrap().contains_point(1200, 0));
        assert_eq!(demand.plan_stats().region_representation_overflows, 1);
        assert_eq!(demand.plan_stats().visible_clip_fallbacks, 1);
        assert_eq!(
            demand.visible_clip_fallbacks(),
            &[EffectVisibleClipFallback {
                instance: lower,
                pass: None,
                input_rect_count: 128,
                clip_rect_count: 2,
            }]
        );
    }

    #[test]
    fn fragmented_transitive_dependencies_propagate_in_reverse_order() {
        let first_id = EffectInstanceId::new(1).unwrap();
        let second_id = EffectInstanceId::new(2).unwrap();
        let third_id = EffectInstanceId::new(3).unwrap();
        let first_region = EffectRegion::from_rect(EffectRect::new(0, 0, 100, 100).unwrap());
        let second_region = repeated_region(EffectRect::new(200, 0, 50, 100).unwrap(), 9);
        let third_region = EffectRegion::from_rect(EffectRect::new(400, 0, 50, 100).unwrap());
        let graph = demand_test_graph(vec![
            demand_test_instance(1, first_region.clone(), first_region.clone(), Vec::new()),
            demand_test_instance(2, second_region, first_region.clone(), vec![first_id]),
            demand_test_instance(
                3,
                third_region,
                EffectRegion::from_rect(EffectRect::new(200, 0, 50, 100).unwrap()),
                vec![second_id],
            ),
        ]);
        let repair = EffectRegion::from_rect(EffectRect::new(400, 0, 50, 100).unwrap());

        let demand = plan_effect_execution_demand(&graph, &repair, false);

        assert!(demand.contains(first_id));
        assert!(demand.contains(second_id));
        assert!(demand.contains(third_id));
        assert_eq!(demand.plan_stats().dependency_edge_count, 2);
        assert_eq!(demand.plan_stats().dependency_propagations, 2);
    }

    #[test]
    fn dependency_metadata_must_reference_earlier_instances() {
        let first_id = EffectInstanceId::new(1).unwrap();
        let second_id = EffectInstanceId::new(2).unwrap();
        let region = EffectRegion::from_rect(EffectRect::new(0, 0, 20, 20).unwrap());
        let mut graph = demand_test_graph(vec![
            demand_test_instance(1, region.clone(), region.clone(), Vec::new()),
            demand_test_instance(2, region.clone(), region, vec![first_id]),
        ]);
        assert!(graph_execution_metadata_is_complete(&graph));

        graph.instances[0].dependencies.push(second_id);

        assert!(!graph_execution_metadata_is_complete(&graph));
        let demand = plan_effect_execution_demand(
            &graph,
            &EffectRegion::from_rect(EffectRect::new(0, 0, 20, 20).unwrap()),
            false,
        );
        assert!(demand.is_conservative_full());
    }

    #[test]
    fn moving_visual_group_backdrop_demand_is_bounded_across_fragmented_repairs() {
        let output_bounds = EffectRect::new(0, 0, 256, 192).unwrap();
        let source_damage = EffectRegion::from_rect(output_bounds);
        let (base_scene, registry) = blur_scene();
        let base_instance = base_scene.instances[0].clone();
        let translations = [-16, -8, 0, 1, 32, 80, 128, 80, 1, -8, -16];
        let mut graph_shape = None;

        for translation in translations.into_iter().cycle().take(256) {
            let instances = [0_u32, 1, 2, 3]
                .into_iter()
                .map(|index| {
                    let mut instance = base_instance.clone();
                    let x = 16 + translation + i32::try_from(index).unwrap() * 24;
                    let region = EffectRegion::from_rect(EffectRect::new(x, 48, 64, 80).unwrap());
                    instance.id = EffectInstanceId::new(u64::from(index + 1)).unwrap();
                    instance.anchor = EffectAnchor::BeforeSurface(100 + index);
                    instance.anchor_scope = EffectAnchorScope::VisualGroup;
                    instance.visual_group = Some(VisualGroupId::new(index + 1).unwrap());
                    instance.scene_order = EffectSceneOrder::for_anchor(instance.anchor);
                    instance.target_bounds = region.bounding_rect().unwrap();
                    instance.region = region;
                    instance.signature = u64::from(index + 1);
                    instance
                })
                .collect();
            let scene = ResolvedEffectScene::new(1, instances);
            let FrameExecutionPlan::EffectGraph(graph) =
                compile_frame_execution_plan(&scene, &source_damage, output_bounds, &registry)
                    .unwrap()
            else {
                panic!("moving visual-group backdrop topology must compile");
            };

            assert_eq!(graph.instances.len(), 4);
            assert!(
                graph.passes.iter().all(|pass| {
                    pass.anchor_scope == EffectAnchorScope::VisualGroup
                        && (!matches!(
                            pass.kind,
                            RenderPassKind::SceneCapture | RenderPassKind::Composite
                        ) || pass.visual_group.is_some())
                }),
                "unexpected pass topology"
            );
            assert!(graph.instances.iter().enumerate().all(|(index, instance)| {
                instance.dependencies.iter().all(|dependency_id| {
                    unique_instance_index(&graph, *dependency_id)
                        .is_some_and(|dependency_index| dependency_index < index)
                })
            }));

            let moving_region = graph
                .instances
                .last()
                .expect("fourth visual-group instance")
                .output_influence_region
                .clone();
            let fragmented_repair = moving_region
                .rects()
                .first()
                .copied()
                .map_or_else(EffectRegion::empty, |rect| repeated_region(rect, 71));
            let equivalent_repair = moving_region.clone();
            let fragmented_demand = plan_effect_execution_demand(&graph, &fragmented_repair, false);
            let equivalent_demand = plan_effect_execution_demand(&graph, &equivalent_repair, false);
            let edge_count = graph
                .instances
                .iter()
                .map(|instance| instance.dependencies.len())
                .sum::<usize>();
            assert!(
                edge_count > 0,
                "overlapping backdrop topology lost dependencies"
            );
            assert!(
                graph
                    .passes
                    .iter()
                    .filter(|pass| pass.kind == RenderPassKind::SceneCapture)
                    .any(|pass| !pass.checkpoint_dependencies.is_empty())
            );
            let shape = (graph.passes.len(), graph.textures.len());
            if let Some(first_shape) = graph_shape {
                assert_eq!(first_shape, shape, "translation changed graph topology");
            } else {
                graph_shape = Some(shape);
            }

            assert!(!fragmented_demand.is_conservative_full());
            assert_eq!(
                fragmented_demand
                    .instances
                    .iter()
                    .map(|instance| instance.id)
                    .collect::<Vec<_>>(),
                equivalent_demand
                    .instances
                    .iter()
                    .map(|instance| instance.id)
                    .collect::<Vec<_>>(),
                "repair representation changed selected identities at translation {translation}"
            );
            assert!(semantically_equal_regions(
                &fragmented_demand.execution_region,
                &equivalent_demand.execution_region
            ));
            assert!(
                fragmented_demand.plan_stats().dependency_propagations <= edge_count,
                "dependency edge propagated more than once at translation {translation}"
            );
            assert!(
                fragmented_demand
                    .plan_stats()
                    .max_instance_region_rect_count
                    <= MAX_EFFECT_REGION_RECTS
            );
            assert!(
                fragmented_demand
                    .instances
                    .iter()
                    .all(|instance| !instance.output_region.is_empty())
            );
            for (consumer_index, consumer) in graph.instances.iter().enumerate() {
                let Some(consumer_demand) = fragmented_demand.output_region(consumer.id) else {
                    continue;
                };
                assert!(region_covers_region(
                    &fragmented_demand.execution_region,
                    consumer_demand
                ));
                if !consumer.dependencies.is_empty() {
                    assert!(region_covers_region(
                        &fragmented_demand.execution_region,
                        &consumer.capture_region
                    ));
                }
                for dependency_id in &consumer.dependencies {
                    let dependency_index = unique_instance_index(&graph, *dependency_id)
                        .expect("validated dependency index");
                    let required = consumer.capture_region.intersect_bounded_within(
                        &graph.instances[dependency_index].output_influence_region,
                    );
                    if !required.is_empty() {
                        let dependency_demand = fragmented_demand
                            .output_region(*dependency_id)
                            .expect("required dependency must be selected");
                        assert!(region_covers_region(dependency_demand, &required));
                    }
                }
                assert!(
                    consumer_demand.rects().len() <= MAX_EFFECT_REGION_RECTS,
                    "selected output region exceeded representation bound at instance {consumer_index}"
                );
            }
        }
    }

    #[test]
    fn target_content_only_effects_skip_backdrop_checkpoint_scans() {
        let registry = target_content_registry();
        let program = EffectProgramId::new(14).unwrap();
        let region = EffectRegion::from_rect(EffectRect::new(100, 80, 320, 180).unwrap());
        let scene = ResolvedEffectScene::new(
            1,
            vec![
                test_instance(program, 1, region.clone()),
                test_instance(program, 2, region),
            ],
        );
        reset_peak_live_work_counters();

        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &EffectRegion::from_rect(EffectRect::new(0, 0, 1920, 1080).unwrap()),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("target-content effects must compile to an effect graph");
        };

        assert_eq!(checkpoint_intersection_checks(), 0);
        assert!(
            graph
                .instances
                .iter()
                .all(|instance| instance.dependencies.is_empty())
        );
        assert!(
            graph
                .passes
                .iter()
                .filter(|pass| pass.kind == RenderPassKind::SurfaceCapture)
                .all(|pass| pass.checkpoint_dependencies.is_empty())
        );
    }

    #[test]
    fn backdrop_dependency_collection_scans_each_checkpoint_once() {
        let (scene, registry) = blur_scene();
        let base = scene.instances[0].clone();
        let instances = (1..=3)
            .map(|id| {
                let mut instance = base.clone();
                instance.id = EffectInstanceId::new(id).unwrap();
                instance.signature = id;
                instance
            })
            .collect();
        let scene = ResolvedEffectScene::new(1, instances);
        reset_peak_live_work_counters();

        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &EffectRegion::from_rect(EffectRect::new(0, 0, 1920, 1080).unwrap()),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("backdrop effects must compile to an effect graph");
        };

        assert_eq!(checkpoint_intersection_checks(), 3);
        assert!(graph.instances[0].dependencies.is_empty());
        assert_eq!(
            graph.instances[1].dependencies,
            vec![EffectInstanceId::new(1).unwrap()]
        );
        assert_eq!(
            graph.instances[2].dependencies,
            vec![
                EffectInstanceId::new(1).unwrap(),
                EffectInstanceId::new(2).unwrap()
            ]
        );
    }

    #[test]
    fn non_intersecting_backdrop_checkpoints_remain_excluded() {
        let (scene, registry) = separated_blur_scene();
        reset_peak_live_work_counters();

        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &EffectRegion::from_rect(EffectRect::new(0, 0, 1920, 1080).unwrap()),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("separated backdrop effects must compile to an effect graph");
        };

        assert_eq!(checkpoint_intersection_checks(), 1);
        assert!(
            graph
                .instances
                .iter()
                .all(|instance| instance.dependencies.is_empty())
        );
        assert!(
            graph
                .passes
                .iter()
                .filter(|pass| pass.kind == RenderPassKind::SceneCapture)
                .all(|pass| pass.checkpoint_dependencies.is_empty())
        );
    }

    #[test]
    fn higher_public_child_effect_depends_on_lower_effect_and_its_content() {
        let (base_scene, registry) = blur_scene();
        let program = base_scene.instances[0].program;
        let group = VisualGroupId::new(9).unwrap();
        let child = |id, surface_id, surface_order| {
            let region = EffectRegion::from_rect(EffectRect::new(100, 80, 320, 180).unwrap());
            ResolvedEffectInstance {
                id: EffectInstanceId::new(id).unwrap(),
                program,
                anchor: EffectAnchor::BeforeSurface(surface_id),
                target_bounds: region.bounding_rect().unwrap(),
                region,
                parameter_block: EffectParameterBlock::default(),
                signature: id,
                frame_demand: EffectFrameDemand::OnDamage,
                visual_group: Some(group),
                anchor_scope: EffectAnchorScope::Surface,
                scene_order: EffectSceneOrder {
                    group_order: group.get(),
                    surface_order,
                    phase: 0,
                },
            }
        };
        let lower = child(20, 20, 0);
        let higher = child(1, 10, 1);
        let scene = ResolvedEffectScene::new(1, vec![higher.clone(), lower.clone()]);
        assert_eq!(
            scene
                .instances
                .iter()
                .map(|instance| instance.id)
                .collect::<Vec<_>>(),
            vec![lower.id, higher.id]
        );

        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &EffectRegion::from_rect(EffectRect::new(100, 80, 320, 180).unwrap()),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("overlapping public child effects must compile to an effect graph");
        };
        let captures = graph
            .passes
            .iter()
            .filter(|pass| pass.kind == RenderPassKind::SceneCapture)
            .collect::<Vec<_>>();
        assert_eq!(captures.len(), 2);
        assert_eq!(captures[0].instance, lower.id);
        assert_eq!(captures[1].instance, higher.id);
        assert_eq!(captures[0].checkpoint_dependencies, Vec::new());
        assert_eq!(captures[1].checkpoint_dependencies.len(), 1);
        assert!(graph.passes.iter().any(|pass| {
            pass.kind == RenderPassKind::Composite
                && captures[1].checkpoint_dependencies.contains(&pass.id)
                && pass.instance == lower.id
        }));
        let higher_capture_texture = graph
            .textures
            .iter()
            .find(|texture| {
                texture.source == GraphTextureSource::CapturedScene
                    && graph.passes.iter().any(|pass| {
                        pass.instance == higher.id
                            && pass.kind == RenderPassKind::SceneCapture
                            && pass.output == Some(texture.id)
                    })
            })
            .expect("higher child must capture its resolved backdrop");
        assert_eq!(
            higher_capture_texture.domain,
            EffectRect::new(76, 56, 368, 228).unwrap()
        );
    }

    #[test]
    fn graph_explanation_is_deterministic_and_source_free() {
        let (scene, registry) = blur_scene();
        let plan = compile_frame_execution_plan(
            &scene,
            &EffectRegion::from_rect(EffectRect::new(100, 80, 320, 180).unwrap()),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap();
        let FrameExecutionPlan::EffectGraph(graph) = plan else {
            panic!("visible effects must compile to an effect graph");
        };
        let explanation = graph.explain();
        assert_eq!(explanation, graph.explain());
        assert!(explanation.starts_with(
            "effects graph: instances=1 passes=6 textures=6 peak_live=2 capture_px=83904 output_px=57600\n"
        ));
        assert!(explanation.contains("capture_scene"));
        assert!(explanation.contains("kawase_down"));
        assert!(!explanation.contains("#version"));
    }

    #[test]
    fn sparse_node_ids_compile_with_dense_lowering_storage() {
        let source = EffectNodeId::new(1).unwrap();
        let tint = EffectNodeId::new(30_000).unwrap();
        let noise = EffectNodeId::new(u16::MAX).unwrap();
        let program = validate_effect_program(EffectProgram {
            id: EffectProgramId::new(15).unwrap(),
            nodes: vec![
                EffectNode::source(source, EffectSource::TargetContent),
                EffectNode::tint(tint, source, TintSpec::WHITE),
                EffectNode::noise(noise, tint, NoiseSpec::new(NoiseKind::Hash, 0.1).unwrap()),
            ],
            output: noise,
            working_space: EffectWorkingSpace::OutputEncodedSrgb,
            alpha_mode: EffectAlphaMode::Preserve,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        })
        .unwrap();
        assert_eq!(program.lowering.steps.len(), 3);
        let mut registry = EffectRegistry::empty();
        registry.insert(program).unwrap();
        let region = EffectRegion::from_rect(EffectRect::new(10, 10, 20, 20).unwrap());
        let scene = ResolvedEffectScene::new(
            1,
            vec![test_instance(
                EffectProgramId::new(15).unwrap(),
                1,
                region.clone(),
            )],
        );

        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &region,
            EffectRect::new(0, 0, 100, 100).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("sparse node ids must compile to an effect graph");
        };

        assert_eq!(graph.stats.effect_instances, 1);
        assert_eq!(graph.stats.passes, 3);
        assert_eq!(graph.stats.textures, 4);
    }

    #[test]
    fn declaration_order_does_not_change_compiled_graph_semantics() {
        let backdrop = EffectNodeId::new(1).unwrap();
        let target = EffectNodeId::new(2).unwrap();
        let blend = EffectNodeId::new(3).unwrap();
        let make_program = |nodes| {
            validate_effect_program(EffectProgram {
                id: EffectProgramId::new(16).unwrap(),
                nodes,
                output: blend,
                working_space: EffectWorkingSpace::LinearSrgb,
                alpha_mode: EffectAlphaMode::Preserve,
                outsets: EffectOutsets::ZERO,
                frame_demand: EffectFrameDemand::OnDamage,
                failure_policy: EffectFailurePolicy::Passthrough,
            })
            .unwrap()
        };
        let first = make_program(vec![
            EffectNode::source(backdrop, EffectSource::Backdrop),
            EffectNode::source(target, EffectSource::TargetContent),
            EffectNode::blend(
                blend,
                vec![backdrop, target],
                BlendSpec::new(BlendMode::SourceOver, 1.0).unwrap(),
            ),
        ]);
        let second = make_program(vec![
            EffectNode::blend(
                blend,
                vec![backdrop, target],
                BlendSpec::new(BlendMode::SourceOver, 1.0).unwrap(),
            ),
            EffectNode::source(target, EffectSource::TargetContent),
            EffectNode::source(backdrop, EffectSource::Backdrop),
        ]);
        assert_eq!(first.topological_order, second.topological_order);
        let region = EffectRegion::from_rect(EffectRect::new(10, 10, 20, 20).unwrap());
        let scene = ResolvedEffectScene::new(
            1,
            vec![test_instance(
                EffectProgramId::new(16).unwrap(),
                1,
                region.clone(),
            )],
        );
        let compile = |program| {
            let mut registry = EffectRegistry::empty();
            registry.insert(program).unwrap();
            let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
                &scene,
                &region,
                EffectRect::new(0, 0, 100, 100).unwrap(),
                &registry,
            )
            .unwrap() else {
                panic!("equivalent programs must compile to an effect graph");
            };
            graph
        };

        assert_eq!(compile(first), compile(second));
    }

    #[test]
    fn selected_output_can_precede_an_unrelated_compiled_node() {
        let source = EffectNodeId::new(1).unwrap();
        let output = EffectNodeId::new(2).unwrap();
        let unrelated = EffectNodeId::new(3).unwrap();
        let program = validate_effect_program(EffectProgram {
            id: EffectProgramId::new(17).unwrap(),
            nodes: vec![
                EffectNode::source(source, EffectSource::TargetContent),
                EffectNode::tint(output, source, TintSpec::WHITE),
                EffectNode::source(unrelated, EffectSource::TargetContent),
            ],
            output,
            working_space: EffectWorkingSpace::OutputEncodedSrgb,
            alpha_mode: EffectAlphaMode::Preserve,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        })
        .unwrap();
        assert_eq!(program.lowering.output_slot, 1);
        assert_eq!(program.lowering.steps.len(), 3);
        let mut registry = EffectRegistry::empty();
        registry.insert(program).unwrap();
        let region = EffectRegion::from_rect(EffectRect::new(10, 10, 20, 20).unwrap());
        let scene = ResolvedEffectScene::new(
            1,
            vec![test_instance(
                EffectProgramId::new(17).unwrap(),
                1,
                region.clone(),
            )],
        );

        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &region,
            EffectRect::new(0, 0, 100, 100).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("unrelated valid nodes must remain compiled");
        };
        let tint_output = graph
            .passes
            .iter()
            .find(|pass| {
                pass.kind == RenderPassKind::Fragment
                    && matches!(pass.stage, Some(EffectNodeKind::Tint(_)))
            })
            .and_then(|pass| pass.output)
            .expect("selected output stage must be compiled");
        let final_pass = graph.passes.last().expect("final composite pass");
        assert_eq!(final_pass.inputs, vec![tint_output]);
        assert_eq!(
            graph
                .passes
                .iter()
                .filter(|pass| pass.kind == RenderPassKind::SurfaceCapture)
                .count(),
            2
        );
        assert_eq!(graph.stats.passes, 4);
    }

    #[test]
    fn built_in_local_nodes_compile_to_real_stage_passes() {
        let source = EffectNodeId::new(1).unwrap();
        let tint = EffectNodeId::new(2).unwrap();
        let program = validate_effect_program(EffectProgram {
            id: EffectProgramId::new(9).unwrap(),
            nodes: vec![
                EffectNode::source(source, EffectSource::Backdrop),
                EffectNode::tint(tint, source, TintSpec::WHITE),
            ],
            output: tint,
            working_space: EffectWorkingSpace::LinearSrgb,
            alpha_mode: EffectAlphaMode::Opaque,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        })
        .unwrap();
        let mut registry = EffectRegistry::empty();
        registry.insert(program).unwrap();
        let region = EffectRegion::from_rect(EffectRect::new(10, 10, 20, 20).unwrap());
        let scene = ResolvedEffectScene::new(
            1,
            vec![ResolvedEffectInstance {
                id: EffectInstanceId::new(1).unwrap(),
                program: EffectProgramId::new(9).unwrap(),
                anchor: EffectAnchor::OutputPostProcess,
                target_bounds: region.bounding_rect().unwrap(),
                region: region.clone(),
                parameter_block: EffectParameterBlock::default(),
                signature: 1,
                frame_demand: EffectFrameDemand::OnDamage,
                visual_group: None,
                anchor_scope: EffectAnchorScope::VisualGroup,
                scene_order: EffectSceneOrder::for_anchor(EffectAnchor::OutputPostProcess),
            }],
        );
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &region,
            EffectRect::new(0, 0, 100, 100).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("visible local stage must compile to an effect graph");
        };
        assert_eq!(
            graph.passes.last().unwrap().color_conversion,
            EffectColorConversion::EncodeLinearToSrgb
        );
        assert!(graph.passes.iter().any(|pass| {
            pass.kind == RenderPassKind::Fragment
                && matches!(pass.stage, Some(EffectNodeKind::Tint(_)))
        }));
    }

    #[test]
    fn multi_input_stages_normalize_each_input_to_the_validated_working_space() {
        let backdrop = EffectNodeId::new(1).unwrap();
        let target = EffectNodeId::new(2).unwrap();
        let blend = EffectNodeId::new(3).unwrap();
        let program = validate_effect_program(EffectProgram {
            id: EffectProgramId::new(11).unwrap(),
            nodes: vec![
                EffectNode::source(backdrop, EffectSource::Backdrop),
                EffectNode::source(target, EffectSource::TargetContent),
                EffectNode::blend(
                    blend,
                    vec![backdrop, target],
                    BlendSpec::new(BlendMode::SourceOver, 1.0).unwrap(),
                ),
            ],
            output: blend,
            working_space: EffectWorkingSpace::LinearSrgb,
            alpha_mode: EffectAlphaMode::Preserve,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        })
        .unwrap();
        let mut registry = EffectRegistry::empty();
        registry.insert(program).unwrap();
        let region = EffectRegion::from_rect(EffectRect::new(10, 10, 20, 20).unwrap());
        let scene = ResolvedEffectScene::new(
            1,
            vec![ResolvedEffectInstance {
                id: EffectInstanceId::new(1).unwrap(),
                program: EffectProgramId::new(11).unwrap(),
                anchor: EffectAnchor::OutputPostProcess,
                target_bounds: region.bounding_rect().unwrap(),
                region: region.clone(),
                parameter_block: EffectParameterBlock::default(),
                signature: 1,
                frame_demand: EffectFrameDemand::OnDamage,
                visual_group: None,
                anchor_scope: EffectAnchorScope::VisualGroup,
                scene_order: EffectSceneOrder::for_anchor(EffectAnchor::OutputPostProcess),
            }],
        );
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &region,
            EffectRect::new(0, 0, 100, 100).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("multi-input effect must compile");
        };
        let normalize = graph
            .passes
            .iter()
            .filter(|pass| pass.kind == RenderPassKind::NormalizeInput)
            .collect::<Vec<_>>();
        assert_eq!(normalize.len(), 2);
        let source_outputs = graph
            .passes
            .iter()
            .filter(|pass| {
                matches!(
                    pass.kind,
                    RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
                )
            })
            .map(|pass| pass.output.unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            normalize
                .iter()
                .map(|pass| pass.inputs[0])
                .collect::<Vec<_>>(),
            source_outputs
        );
        assert!(
            normalize
                .iter()
                .all(|pass| { pass.color_conversion == EffectColorConversion::DecodeSrgbToLinear })
        );
        let blend_pass = graph
            .passes
            .iter()
            .find(|pass| pass.kind == RenderPassKind::Blend)
            .unwrap();
        assert_eq!(blend_pass.color_conversion, EffectColorConversion::None);
        assert_eq!(blend_pass.inputs.len(), 2);
        assert_eq!(
            blend_pass.inputs,
            normalize
                .iter()
                .map(|pass| pass.output.unwrap())
                .collect::<Vec<_>>()
        );
        assert!(blend_pass.inputs.iter().all(|input| {
            graph
                .textures
                .iter()
                .find(|texture| texture.id == *input)
                .is_some_and(|texture| texture.working_space == EffectWorkingSpace::LinearSrgb)
        }));
        let demand = plan_effect_execution_demand(&graph, &region, false);
        assert!(blend_pass.inputs.iter().all(|input| {
            graph
                .passes
                .iter()
                .filter(|pass| pass.output == Some(*input))
                .all(|pass| {
                    demand
                        .pass_output_region(pass.id)
                        .is_some_and(|required| !required.is_empty())
                })
        }));
        assert!(demand.plan_stats().pass_dependency_propagations >= 3);
    }

    #[test]
    fn mask_demand_propagates_to_its_content_input() {
        let source = EffectNodeId::new(1).unwrap();
        let mask = EffectNodeId::new(2).unwrap();
        let program = validate_effect_program(EffectProgram {
            id: EffectProgramId::new(20).unwrap(),
            nodes: vec![
                EffectNode::source(source, EffectSource::Backdrop),
                EffectNode::mask(
                    mask,
                    source,
                    MaskSpec {
                        mode: MaskMode::Alpha,
                    },
                ),
            ],
            output: mask,
            working_space: EffectWorkingSpace::LinearSrgb,
            alpha_mode: EffectAlphaMode::Preserve,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        })
        .unwrap();
        let mut registry = EffectRegistry::empty();
        registry.insert(program).unwrap();
        let region = EffectRegion::from_rect(EffectRect::new(20, 30, 40, 25).unwrap());
        let scene = ResolvedEffectScene::new(
            1,
            vec![test_instance(
                EffectProgramId::new(20).unwrap(),
                1,
                region.clone(),
            )],
        );
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &region,
            EffectRect::new(0, 0, 100, 100).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("mask effect must compile to an effect graph");
        };
        let demand = plan_effect_execution_demand(&graph, &region, false);
        let mask_pass = graph
            .passes
            .iter()
            .find(|pass| pass.kind == RenderPassKind::Mask)
            .expect("mask pass");
        let input = mask_pass.inputs[0];
        let producer = graph
            .passes
            .iter()
            .find(|pass| pass.output == Some(input))
            .expect("mask content producer");
        assert!(
            !demand
                .pass_output_region(producer.id)
                .expect("content demand")
                .is_empty()
        );
    }

    #[test]
    fn normalize_input_uses_non_zero_domains_and_consuming_footprint_damage() {
        let backdrop = EffectNodeId::new(1).unwrap();
        let target = EffectNodeId::new(2).unwrap();
        let custom = EffectNodeId::new(3).unwrap();
        let program = validate_effect_program(EffectProgram {
            id: EffectProgramId::new(12).unwrap(),
            nodes: vec![
                EffectNode::source(backdrop, EffectSource::Backdrop),
                EffectNode::source(target, EffectSource::TargetContent),
                EffectNode::custom_fragment(
                    custom,
                    backdrop,
                    CustomFragmentSpec {
                        shader: ShaderModuleId::new(12).unwrap(),
                        declared_footprint: EffectFootprint::symmetric(4),
                        uniforms: Vec::new(),
                        auxiliary_inputs: vec![target],
                    },
                )
                .unwrap(),
            ],
            output: custom,
            working_space: EffectWorkingSpace::LinearSrgb,
            alpha_mode: EffectAlphaMode::Preserve,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        })
        .unwrap();
        let mut registry = EffectRegistry::empty();
        registry.insert(program).unwrap();
        let region = EffectRegion::from_rect(EffectRect::new(500, 200, 30, 20).unwrap());
        let scene = ResolvedEffectScene::new(
            1,
            vec![ResolvedEffectInstance {
                id: EffectInstanceId::new(1).unwrap(),
                program: EffectProgramId::new(12).unwrap(),
                anchor: EffectAnchor::OutputPostProcess,
                target_bounds: region.bounding_rect().unwrap(),
                region: region.clone(),
                parameter_block: EffectParameterBlock::default(),
                signature: 1,
                frame_demand: EffectFrameDemand::OnDamage,
                visual_group: None,
                anchor_scope: EffectAnchorScope::VisualGroup,
                scene_order: EffectSceneOrder::for_anchor(EffectAnchor::OutputPostProcess),
            }],
        );
        let source_damage = EffectRegion::from_rect(EffectRect::new(512, 212, 2, 2).unwrap());
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &source_damage,
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("multi-input custom effect must compile");
        };
        let normalize = graph
            .passes
            .iter()
            .find(|pass| pass.kind == RenderPassKind::NormalizeInput)
            .expect("auxiliary input requires normalization");
        let normalized_domain = graph
            .textures
            .iter()
            .find(|texture| texture.id == normalize.output.unwrap())
            .unwrap()
            .domain;
        assert_eq!(normalized_domain.x, 496);
        assert_eq!(normalized_domain.y, 196);
        assert!(normalize.damage.contains_point(504, 204));
        assert!(normalize.damage.contains_point(521, 221));
        assert!(!normalize.damage.contains_point(496, 196));
        assert!(
            normalize
                .damage
                .rects()
                .iter()
                .all(|rect| rect.intersect(normalized_domain).is_some())
        );
    }

    #[test]
    fn custom_fragment_preserves_primary_and_auxiliary_input_order() {
        let primary = EffectNodeId::new(1).unwrap();
        let auxiliary_a = EffectNodeId::new(2).unwrap();
        let auxiliary_b = EffectNodeId::new(3).unwrap();
        let custom = EffectNodeId::new(4).unwrap();
        let program = validate_effect_program(EffectProgram {
            id: EffectProgramId::new(18).unwrap(),
            nodes: vec![
                EffectNode::source(primary, EffectSource::Backdrop),
                EffectNode::source(auxiliary_a, EffectSource::TargetContent),
                EffectNode::source(auxiliary_b, EffectSource::TargetContent),
                EffectNode::custom_fragment(
                    custom,
                    primary,
                    CustomFragmentSpec {
                        shader: ShaderModuleId::new(18).unwrap(),
                        declared_footprint: EffectFootprint::ZERO,
                        uniforms: Vec::new(),
                        auxiliary_inputs: vec![auxiliary_a, auxiliary_b],
                    },
                )
                .unwrap(),
            ],
            output: custom,
            working_space: EffectWorkingSpace::LinearSrgb,
            alpha_mode: EffectAlphaMode::Preserve,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        })
        .unwrap();
        assert_eq!(
            program.lowering.steps.last().unwrap().input_slots,
            vec![0, 1, 2]
        );
        let mut registry = EffectRegistry::empty();
        registry.insert(program).unwrap();
        let region = EffectRegion::from_rect(EffectRect::new(10, 10, 20, 20).unwrap());
        let scene = ResolvedEffectScene::new(
            1,
            vec![test_instance(
                EffectProgramId::new(18).unwrap(),
                1,
                region.clone(),
            )],
        );

        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &region,
            EffectRect::new(0, 0, 100, 100).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("custom fragment must compile to an effect graph");
        };
        let source_outputs = graph
            .passes
            .iter()
            .filter(|pass| {
                matches!(
                    pass.kind,
                    RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
                )
            })
            .map(|pass| pass.output.unwrap())
            .collect::<Vec<_>>();
        let normalize = graph
            .passes
            .iter()
            .filter(|pass| pass.kind == RenderPassKind::NormalizeInput)
            .collect::<Vec<_>>();
        let custom_pass = graph
            .passes
            .iter()
            .find(|pass| {
                pass.kind == RenderPassKind::Fragment
                    && matches!(pass.stage, Some(EffectNodeKind::CustomFragment(_)))
            })
            .expect("custom fragment stage pass");
        assert_eq!(
            normalize
                .iter()
                .map(|pass| pass.inputs[0])
                .collect::<Vec<_>>(),
            source_outputs
        );
        assert_eq!(
            custom_pass.inputs,
            normalize
                .iter()
                .map(|pass| pass.output.unwrap())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn compatible_zero_footprint_local_stages_fuse_in_canonical_order() {
        let source = EffectNodeId::new(1).unwrap();
        let matrix = EffectNodeId::new(2).unwrap();
        let tint = EffectNodeId::new(3).unwrap();
        let program = validate_effect_program(EffectProgram {
            id: EffectProgramId::new(10).unwrap(),
            nodes: vec![
                EffectNode::source(source, EffectSource::Backdrop),
                EffectNode::color_matrix(
                    matrix,
                    source,
                    ColorMatrixSpec {
                        matrix: [
                            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0,
                            0.0, 1.0,
                        ],
                        bias: [0.0; 4],
                    },
                )
                .unwrap(),
                EffectNode::tint(
                    tint,
                    matrix,
                    TintSpec::new([1.0, 0.9, 0.8, 1.0], 0.5).unwrap(),
                ),
            ],
            output: tint,
            working_space: EffectWorkingSpace::LinearSrgb,
            alpha_mode: EffectAlphaMode::Opaque,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        })
        .unwrap();
        let mut registry = EffectRegistry::empty();
        registry.insert(program).unwrap();
        let region = EffectRegion::from_rect(EffectRect::new(10, 10, 20, 20).unwrap());
        let scene = ResolvedEffectScene::new(
            1,
            vec![ResolvedEffectInstance {
                id: EffectInstanceId::new(1).unwrap(),
                program: EffectProgramId::new(10).unwrap(),
                anchor: EffectAnchor::OutputPostProcess,
                target_bounds: region.bounding_rect().unwrap(),
                region: region.clone(),
                parameter_block: EffectParameterBlock::default(),
                signature: 1,
                frame_demand: EffectFrameDemand::OnDamage,
                visual_group: None,
                anchor_scope: EffectAnchorScope::VisualGroup,
                scene_order: EffectSceneOrder::for_anchor(EffectAnchor::OutputPostProcess),
            }],
        );
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &region,
            EffectRect::new(0, 0, 100, 100).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("visible local stages must compile to an effect graph");
        };
        let stages = graph
            .passes
            .iter()
            .filter(|pass| pass.kind == RenderPassKind::Fragment)
            .collect::<Vec<_>>();
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0].fused_stages.len(), 1);
        assert!(matches!(
            stages[0].stage,
            Some(EffectNodeKind::ColorMatrix(_))
        ));
        assert_eq!(
            graph.stats.peak_live_intermediates,
            brute_force_peak(&graph.passes, &graph.textures)
        );
    }

    #[test]
    fn compiled_mid_graph_fusion_preserves_surviving_pass_order_for_peak_live() {
        let (scene, registry) = fused_local_stage_scene(1);
        let region = EffectRegion::from_rect(EffectRect::new(0, 0, 100, 100).unwrap());
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &region,
            EffectRect::new(0, 0, 100, 100).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("fused local stage scene must compile to an effect graph");
        };
        let pass_ids = graph
            .passes
            .iter()
            .map(|pass| pass.id.get())
            .collect::<Vec<_>>();
        assert!(pass_ids.windows(2).any(|pair| pair[1] > pair[0] + 1));
        assert!(
            graph
                .passes
                .iter()
                .any(|pass| !pass.fused_stages.is_empty())
        );
        assert_eq!(
            graph.stats.peak_live_intermediates,
            brute_force_peak(&graph.passes, &graph.textures)
        );
    }

    #[test]
    fn compiled_multiple_fusions_keep_peak_live_exact_for_late_ids() {
        let (scene, registry) = fused_local_stage_scene(3);
        let region = EffectRegion::from_rect(EffectRect::new(0, 0, 200, 100).unwrap());
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &region,
            EffectRect::new(0, 0, 200, 100).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("multiple fused local stage scene must compile to an effect graph");
        };
        let pass_ids = graph
            .passes
            .iter()
            .map(|pass| pass.id.get())
            .collect::<Vec<_>>();
        let gap_count = pass_ids
            .windows(2)
            .filter(|pair| pair[1] > pair[0] + 1)
            .count();
        assert!(gap_count >= 2, "expected multiple fusion-created ID gaps");
        assert!(graph.textures.iter().any(|texture| {
            texture.source == GraphTextureSource::Intermediate
                && texture
                    .first_use
                    .is_some_and(|first| usize::from(first.get()) > graph.passes.len())
        }));
        assert_eq!(
            graph.stats.peak_live_intermediates,
            brute_force_peak(&graph.passes, &graph.textures)
        );
    }

    #[test]
    fn peak_live_work_scales_with_intervals_and_passes() {
        let passes = (1..=256).map(test_pass).collect::<Vec<_>>();
        let textures = (1..=512)
            .map(|id| lifetime_texture(id, Some(1), Some(256), GraphTextureSource::Intermediate))
            .collect::<Vec<_>>();
        reset_peak_live_work_counters();

        assert_eq!(peak_live_intermediates(&passes, &textures), 512);
        assert_eq!(
            peak_live_work_counters(),
            PeakLiveWorkCounters {
                pass_position_map_entries: 256,
                interval_insertions: 512,
                sweep_steps: 256,
                graph_compiles: 0,
                instance_compiles: 0,
                node_visits: 0,
                program_lookup_map_builds: 0,
                output_map_builds: 0,
            }
        );
    }

    #[test]
    fn c2_effect_workloads_remove_repeated_instance_metadata() {
        let (single_scene, registry) = blur_scene();
        reset_peak_live_work_counters();
        let single_plan = compile_frame_execution_plan(
            &single_scene,
            &EffectRegion::from_rect(EffectRect::new(0, 0, 1920, 1080).unwrap()),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap();
        let single_counters = peak_live_work_counters();

        let base_instance = single_scene.instances[0].clone();
        let single_stats = match &single_plan {
            FrameExecutionPlan::EffectGraph(graph) => graph.stats,
            FrameExecutionPlan::LegacyScene => panic!("single effect must compile to a graph"),
        };
        println!("C2 effect one={single_counters:?} stats={single_stats:?}");
        assert_eq!(single_counters.graph_compiles, 1);
        assert_eq!(single_counters.instance_compiles, 1);
        assert_eq!(single_counters.program_lookup_map_builds, 0);
        assert_eq!(single_counters.output_map_builds, 0);

        for effect_count in [8_usize, 32] {
            let many_instances = (0..effect_count)
                .map(|index| {
                    let mut instance = base_instance.clone();
                    let id = u64::try_from(index + 1).expect("test effect id fits");
                    let region = EffectRegion::from_rect(
                        EffectRect::new(100 + (index as i32) * 40, 80, 160, 120)
                            .expect("test effect region"),
                    );
                    instance.id = EffectInstanceId::new(id).expect("test effect id is non-zero");
                    instance.region = region.clone();
                    instance.target_bounds = region.bounding_rect().expect("test effect bounds");
                    instance.signature = id;
                    instance
                })
                .collect::<Vec<_>>();
            let many_scene = ResolvedEffectScene::new(single_scene.generation, many_instances);
            reset_peak_live_work_counters();
            let many_plan = compile_frame_execution_plan(
                &many_scene,
                &EffectRegion::from_rect(EffectRect::new(0, 0, 1920, 1080).unwrap()),
                EffectRect::new(0, 0, 1920, 1080).unwrap(),
                &registry,
            )
            .unwrap();
            let many_counters = peak_live_work_counters();
            let many_stats = match &many_plan {
                FrameExecutionPlan::EffectGraph(graph) => graph.stats,
                FrameExecutionPlan::LegacyScene => panic!("many effects must compile to a graph"),
            };
            println!(
                "C2 effect count={effect_count} counters={many_counters:?} stats={many_stats:?}"
            );
            assert_eq!(many_counters.graph_compiles, 1);
            assert_eq!(many_counters.instance_compiles, effect_count);
            assert_eq!(many_counters.program_lookup_map_builds, 0);
            assert_eq!(many_counters.output_map_builds, 0);
            assert_eq!(
                many_counters.node_visits,
                effect_count * single_counters.node_visits
            );
            assert!(many_counters.node_visits > single_counters.node_visits);
        }
    }

    #[test]
    fn c2_uniform_only_parameter_changes_repeat_graph_work_without_topology_change() {
        let (scene, registry) = blur_scene();
        let mut changed_instance = scene.instances[0].clone();
        changed_instance
            .parameter_block
            .insert(
                EffectParameterId::new(1).expect("test parameter id"),
                EffectUniformValue::Float(0.75),
            )
            .expect("test uniform value");
        changed_instance.signature = changed_instance.signature.wrapping_add(1);
        let changed_scene = ResolvedEffectScene::new(scene.generation, vec![changed_instance]);
        let source_damage = EffectRegion::from_rect(EffectRect::new(0, 0, 1920, 1080).unwrap());
        let output_bounds = EffectRect::new(0, 0, 1920, 1080).unwrap();

        reset_peak_live_work_counters();
        let FrameExecutionPlan::EffectGraph(original) =
            compile_frame_execution_plan(&scene, &source_damage, output_bounds, &registry).unwrap()
        else {
            panic!("uniform-only workload must compile to an effect graph");
        };
        let original_counters = peak_live_work_counters();

        reset_peak_live_work_counters();
        let FrameExecutionPlan::EffectGraph(changed) =
            compile_frame_execution_plan(&changed_scene, &source_damage, output_bounds, &registry)
                .unwrap()
        else {
            panic!("uniform-only workload must compile to an effect graph");
        };
        let changed_counters = peak_live_work_counters();

        println!("C2 uniform-only original={original_counters:?} changed={changed_counters:?}");
        assert_eq!(original_counters.program_lookup_map_builds, 0);
        assert_eq!(original_counters.output_map_builds, 0);
        assert_eq!(changed_counters.program_lookup_map_builds, 0);
        assert_eq!(changed_counters.output_map_builds, 0);
        assert_eq!(
            (
                original.stats.passes,
                original.stats.textures,
                original.stats.peak_live_intermediates
            ),
            (
                changed.stats.passes,
                changed.stats.textures,
                changed.stats.peak_live_intermediates
            )
        );
        assert_eq!(original_counters, changed_counters);
    }

    #[test]
    fn c2_geometry_changes_repeat_graph_work_with_new_texture_geometry() {
        let (scene, registry) = blur_scene();
        let mut changed_instance = scene.instances[0].clone();
        changed_instance.region =
            EffectRegion::from_rect(EffectRect::new(100, 80, 640, 240).unwrap());
        changed_instance.target_bounds = changed_instance.region.bounding_rect().unwrap();
        let changed_scene = ResolvedEffectScene::new(scene.generation, vec![changed_instance]);
        let source_damage = EffectRegion::from_rect(EffectRect::new(0, 0, 1920, 1080).unwrap());
        let output_bounds = EffectRect::new(0, 0, 1920, 1080).unwrap();

        reset_peak_live_work_counters();
        let FrameExecutionPlan::EffectGraph(original) =
            compile_frame_execution_plan(&scene, &source_damage, output_bounds, &registry).unwrap()
        else {
            panic!("original geometry workload must compile to an effect graph");
        };
        let original_counters = peak_live_work_counters();

        reset_peak_live_work_counters();
        let FrameExecutionPlan::EffectGraph(changed) =
            compile_frame_execution_plan(&changed_scene, &source_damage, output_bounds, &registry)
                .unwrap()
        else {
            panic!("changed geometry workload must compile to an effect graph");
        };
        let changed_counters = peak_live_work_counters();

        println!(
            "C2 geometry-animation original={original_counters:?} changed={changed_counters:?}"
        );
        assert_ne!(original.textures, changed.textures);
        assert_eq!(original_counters.graph_compiles, 1);
        assert_eq!(changed_counters.graph_compiles, 1);
        assert_eq!(original_counters.instance_compiles, 1);
        assert_eq!(changed_counters.instance_compiles, 1);
        assert_eq!(original_counters.program_lookup_map_builds, 0);
        assert_eq!(changed_counters.program_lookup_map_builds, 0);
        assert_eq!(original_counters.output_map_builds, 0);
        assert_eq!(changed_counters.output_map_builds, 0);
    }

    #[test]
    fn unrelated_repair_prunes_separated_effect() {
        let (scene, registry) = separated_blur_scene();
        let first = scene.instances[0].clone();
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &EffectRegion::from_rect(EffectRect::new(1400, 500, 20, 20).unwrap()),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("visible effects must compile to an effect graph");
        };

        let demand = plan_effect_execution_demand(
            &graph,
            &EffectRegion::from_rect(EffectRect::new(100, 80, 320, 180).unwrap()),
            false,
        );

        assert_eq!(demand.instances.len(), 1);
        assert_eq!(demand.instances[0].id, first.id);
        assert!(!demand.execution_region.is_empty());
    }

    #[test]
    fn full_repair_keeps_all_visible_effects_live() {
        let (scene, registry) = separated_blur_scene();
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &EffectRegion::empty(),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("visible effects must compile to an effect graph");
        };

        let demand = plan_effect_execution_demand(&graph, &EffectRegion::empty(), true);

        assert_eq!(demand.instances.len(), 2);
        assert!(demand.contains(EffectInstanceId::new(1).unwrap()));
        assert!(demand.contains(EffectInstanceId::new(2).unwrap()));
    }

    #[test]
    fn unknown_dependency_falls_back_to_all_visible_effects() {
        let (scene, registry) = separated_blur_scene();
        let FrameExecutionPlan::EffectGraph(mut graph) = compile_frame_execution_plan(
            &scene,
            &EffectRegion::empty(),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("visible effects must compile to an effect graph");
        };
        graph.instances[0].dependencies = vec![EffectInstanceId::new(99).unwrap()];

        let demand = plan_effect_execution_demand(
            &graph,
            &graph.instances[0].output_influence_region,
            false,
        );

        assert_eq!(demand.instances.len(), 2);
        assert!(demand.is_conservative_full());
    }

    #[test]
    fn blur_source_halo_keeps_effect_output_demanded() {
        let (scene, registry) = blur_scene();
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &EffectRegion::from_rect(EffectRect::new(88, 80, 12, 20).unwrap()),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("visible effects must compile to an effect graph");
        };

        assert!(graph.final_damage.contains_point(100, 80));
        let demand = plan_effect_execution_demand(&graph, &graph.final_damage, false);
        assert!(demand.contains(EffectInstanceId::new(1).unwrap()));
    }

    #[test]
    fn small_blur_repair_plans_partial_pass_demand() {
        let (scene, registry) = blur_scene();
        let output_bounds = EffectRect::new(0, 0, 1920, 1080).unwrap();
        let FrameExecutionPlan::EffectGraph(graph) =
            compile_frame_execution_plan(&scene, &EffectRegion::empty(), output_bounds, &registry)
                .unwrap()
        else {
            panic!("visible blur must compile to an effect graph");
        };
        let repair = EffectRegion::from_rect(EffectRect::new(220, 140, 12, 10).unwrap());
        let demand = plan_effect_execution_demand(&graph, &repair, false);
        let instance = EffectInstanceId::new(1).unwrap();
        let area = |region: &EffectRegion| {
            region.rects().iter().fold(0_u64, |total, rect| {
                total.saturating_add(u64::from(rect.width) * u64::from(rect.height))
            })
        };
        let capture = graph
            .passes
            .iter()
            .find(|pass| pass.kind == RenderPassKind::SceneCapture)
            .expect("blur has a capture pass");
        let composite = graph
            .passes
            .iter()
            .find(|pass| pass.instance == instance && pass.kind == RenderPassKind::Composite)
            .expect("blur has a composite pass");
        let capture_demand = demand
            .pass_output_region(capture.id)
            .expect("capture demand is planned");
        let composite_demand = demand
            .pass_output_region(composite.id)
            .expect("composite demand is planned");
        let capture_texture = graph
            .textures
            .iter()
            .find(|texture| texture.source == GraphTextureSource::CapturedScene)
            .expect("blur has a captured scene texture");

        assert!(!composite_demand.is_empty());
        assert!(capture_demand.is_empty() || area(capture_demand) > area(composite_demand));
        assert!(
            area(capture_demand)
                < u64::from(capture_texture.width) * u64::from(capture_texture.height)
        );
        assert!(
            graph
                .passes
                .iter()
                .filter(|pass| pass.instance == instance)
                .all(|pass| demand.pass_output_region(pass.id).is_some())
        );
    }

    #[test]
    fn full_kawase_debug_mode_expands_only_internal_passes() {
        let (scene, registry) = blur_scene();
        let output_bounds = EffectRect::new(0, 0, 1920, 1080).unwrap();
        let FrameExecutionPlan::EffectGraph(graph) =
            compile_frame_execution_plan(&scene, &EffectRegion::empty(), output_bounds, &registry)
                .unwrap()
        else {
            panic!("visible blur must compile to an effect graph");
        };
        let repair = EffectRegion::from_rect(EffectRect::new(220, 140, 12, 10).unwrap());
        let demand = plan_effect_execution_demand_with_kawase_mode(&graph, &repair, false, true);

        for pass in graph.passes.iter().filter(|pass| {
            matches!(
                pass.kind,
                RenderPassKind::DualKawaseDownsample | RenderPassKind::DualKawaseUpsample
            )
        }) {
            let output = graph
                .textures
                .iter()
                .find(|texture| texture.id == pass.output.unwrap())
                .expect("Kawase output texture");
            assert_eq!(
                demand.pass_output_region(pass.id),
                Some(&EffectRegion::from_rect(output.domain)),
                "debug full-Kawase mode must fully execute {:?}",
                pass.kind,
            );
        }

        let capture = graph
            .passes
            .iter()
            .find(|pass| pass.kind == RenderPassKind::SceneCapture)
            .expect("blur capture");
        let capture_output = graph
            .textures
            .iter()
            .find(|texture| texture.id == capture.output.expect("capture output"))
            .expect("blur capture texture");
        let capture_demand = demand
            .pass_output_region(capture.id)
            .expect("full-Kawase capture demand");
        for y in capture_output.domain.y..capture_output.domain.bottom() {
            for x in capture_output.domain.x..capture_output.domain.right() {
                assert!(
                    capture_demand.contains_point(x, y),
                    "full internal Kawase demand must propagate to the SceneCapture producer at ({x}, {y})"
                );
            }
        }

        let composite = graph
            .passes
            .iter()
            .find(|pass| pass.kind == RenderPassKind::Composite)
            .expect("blur composite");
        let composite_demand = demand
            .pass_output_region(composite.id)
            .expect("composite demand");
        let composite_output = graph
            .textures
            .iter()
            .find(|texture| texture.id == composite.output.unwrap())
            .expect("composite output texture");
        assert_ne!(
            composite_demand,
            &EffectRegion::from_rect(composite_output.domain),
            "full internal Kawase must not broaden final visible demand"
        );
    }

    #[test]
    fn precise_final_seed_stays_within_disconnected_output_influence() {
        let output_texture = GraphTextureId::new(1).unwrap();
        let output_domain = EffectRect::new(0, 0, 500, 10).unwrap();
        let mut output = EffectRegion::from_rect(EffectRect::new(0, 0, 1, 1).unwrap());
        output.push(EffectRect::new(400, 0, 1, 1).unwrap());
        let mut pass = test_pass(1);
        pass.kind = RenderPassKind::Composite;
        pass.output = Some(output_texture);
        pass.damage = repeated_region(EffectRect::new(0, 0, 500, 1).unwrap(), 128);
        let graph = CompiledFrameGraph {
            passes: vec![pass.clone()],
            textures: vec![GraphTexturePlan {
                id: output_texture,
                source: GraphTextureSource::Output,
                width: output_domain.width,
                height: output_domain.height,
                domain: output_domain,
                working_space: EffectWorkingSpace::OutputEncodedSrgb,
                origin: GraphTextureOrigin::BottomLeft,
                first_use: None,
                last_use: None,
            }],
            instances: vec![demand_test_instance(
                1,
                output.clone(),
                output.clone(),
                Vec::new(),
            )],
            final_damage: EffectRegion::empty(),
            stats: RenderGraphCompileStats::default(),
        };

        let demand = plan_effect_execution_demand(
            &graph,
            &EffectRegion::from_rect(EffectRect::new(0, 0, 500, 1).unwrap()),
            false,
        );
        let final_demand = demand
            .pass_output_region(pass.id)
            .expect("final pass demand");

        assert!(region_is_subset_of(final_demand, &output));
        assert!(final_demand.contains_point(0, 0));
        assert!(final_demand.contains_point(400, 0));
        assert!(!final_demand.contains_point(200, 0));
    }

    #[test]
    fn full_repair_keeps_complete_internal_domains_and_constrains_final() {
        let (scene, registry) = blur_scene();
        let output_bounds = EffectRect::new(0, 0, 1920, 1080).unwrap();
        let FrameExecutionPlan::EffectGraph(graph) =
            compile_frame_execution_plan(&scene, &EffectRegion::empty(), output_bounds, &registry)
                .unwrap()
        else {
            panic!("visible blur must compile to an effect graph");
        };

        let demand = plan_effect_execution_demand(&graph, &EffectRegion::empty(), true);

        assert_eq!(demand.plan_stats().pass_count_selected, graph.passes.len());
        assert_eq!(demand.plan_stats().partial_pass_count, 1);
        assert_eq!(
            demand.plan_stats().full_domain_pass_count,
            graph.passes.len() - 1
        );
        for pass in &graph.passes {
            let output = pass.output.expect("compiled pass output");
            let texture = graph
                .textures
                .iter()
                .find(|texture| texture.id == output)
                .expect("compiled pass output texture");
            let expected = if matches!(
                pass.kind,
                RenderPassKind::Composite | RenderPassKind::OutputPostProcess
            ) {
                pass.damage
                    .union(&graph.instances[0].output_influence_region)
                    .intersect_rect(texture.domain)
            } else {
                EffectRegion::from_rect(texture.domain)
            };
            assert_eq!(demand.pass_output_region(pass.id), Some(&expected));
        }
    }

    #[test]
    fn conservative_final_composite_stays_within_output_influence() {
        let (scene, registry) = blur_scene();
        let output_bounds = EffectRect::new(0, 0, 1920, 1080).unwrap();
        let FrameExecutionPlan::EffectGraph(graph) =
            compile_frame_execution_plan(&scene, &EffectRegion::empty(), output_bounds, &registry)
                .unwrap()
        else {
            panic!("visible blur must compile to an effect graph");
        };
        let demand = plan_effect_execution_demand(&graph, &EffectRegion::empty(), true);
        let instance = &graph.instances[0];
        let composite = graph
            .passes
            .iter()
            .find(|pass| pass.instance == instance.id && pass.kind == RenderPassKind::Composite)
            .expect("blur has a composite pass");
        let capture = graph
            .textures
            .iter()
            .find(|texture| texture.source == GraphTextureSource::CapturedScene)
            .expect("blur has an expanded capture domain");
        let composite_demand = demand
            .pass_output_region(composite.id)
            .expect("composite demand is planned");
        let expected = composite
            .damage
            .union(&instance.output_influence_region)
            .intersect_rect(output_bounds);

        assert_eq!(capture.domain, EffectRect::new(76, 56, 368, 228).unwrap());
        assert_eq!(
            instance.output_influence_region,
            EffectRegion::from_rect(EffectRect::new(100, 80, 320, 180).unwrap())
        );
        assert_eq!(composite_demand, &expected);
        assert_ne!(composite_demand, &EffectRegion::from_rect(output_bounds));
        assert!(!composite_demand.contains_point(capture.domain.x, capture.domain.y));
        assert!(composite_demand.contains_point(100, 80));
    }

    #[test]
    fn precise_and_conservative_repaints_keep_composite_extent_equal() {
        let (scene, registry) = blur_scene();
        let output_bounds = EffectRect::new(0, 0, 1920, 1080).unwrap();
        let FrameExecutionPlan::EffectGraph(graph) =
            compile_frame_execution_plan(&scene, &EffectRegion::empty(), output_bounds, &registry)
                .unwrap()
        else {
            panic!("visible blur must compile to an effect graph");
        };
        let visible = graph.instances[0].output_influence_region.clone();
        let composite = graph
            .passes
            .iter()
            .find(|pass| pass.kind == RenderPassKind::Composite)
            .expect("blur has a composite pass");

        let precise = plan_effect_execution_demand(&graph, &visible, false);
        let conservative = plan_effect_execution_demand(&graph, &EffectRegion::empty(), true);
        let full_then_precise = (
            plan_effect_execution_demand(&graph, &EffectRegion::empty(), true),
            plan_effect_execution_demand(&graph, &visible, false),
        );
        let precise_then_full = (
            plan_effect_execution_demand(&graph, &visible, false),
            plan_effect_execution_demand(&graph, &EffectRegion::empty(), true),
        );
        let precise_composite = precise
            .pass_output_region(composite.id)
            .expect("precise composite demand");
        let conservative_composite = conservative
            .pass_output_region(composite.id)
            .expect("conservative composite demand");

        assert_eq!(precise_composite, conservative_composite);
        assert_eq!(precise_composite, &visible);
        assert_eq!(
            full_then_precise
                .0
                .pass_output_region(composite.id)
                .expect("full-first composite demand"),
            precise_then_full
                .1
                .pass_output_region(composite.id)
                .expect("full-second composite demand")
        );
        assert_eq!(
            full_then_precise
                .1
                .pass_output_region(composite.id)
                .expect("precise-second composite demand"),
            precise_then_full
                .0
                .pass_output_region(composite.id)
                .expect("precise-first composite demand")
        );
        assert!(!precise_composite.contains_point(76, 56));
        assert!(!conservative_composite.contains_point(444, 300));
    }

    #[test]
    fn zero_footprint_local_stage_propagates_unchanged() {
        let (scene, registry) = fused_local_stage_scene(1);
        let region = EffectRegion::from_rect(EffectRect::new(10, 10, 20, 20).unwrap());
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &region,
            EffectRect::new(0, 0, 100, 100).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("local stage must compile to an effect graph");
        };
        let demand = plan_effect_execution_demand(&graph, &region, false);
        let local_passes = graph
            .passes
            .iter()
            .filter(|pass| pass.kind == RenderPassKind::Fragment)
            .collect::<Vec<_>>();
        assert!(!local_passes.is_empty());
        let first = demand
            .pass_output_region(local_passes[0].id)
            .expect("local stage demand");
        for pass in local_passes {
            assert_eq!(demand.pass_output_region(pass.id), Some(first));
        }
    }

    #[test]
    fn custom_fragment_declared_footprint_expands_upstream() {
        let backdrop = EffectNodeId::new(1).unwrap();
        let custom = EffectNodeId::new(2).unwrap();
        let program = validate_effect_program(EffectProgram {
            id: EffectProgramId::new(19).unwrap(),
            nodes: vec![
                EffectNode::source(backdrop, EffectSource::Backdrop),
                EffectNode::custom_fragment(
                    custom,
                    backdrop,
                    CustomFragmentSpec {
                        shader: ShaderModuleId::new(19).unwrap(),
                        declared_footprint: EffectFootprint::symmetric(6),
                        uniforms: Vec::new(),
                        auxiliary_inputs: Vec::new(),
                    },
                )
                .unwrap(),
            ],
            output: custom,
            working_space: EffectWorkingSpace::LinearSrgb,
            alpha_mode: EffectAlphaMode::Preserve,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        })
        .unwrap();
        let mut registry = EffectRegistry::empty();
        registry.insert(program).unwrap();
        let visible = EffectRegion::from_rect(EffectRect::new(300, 200, 80, 60).unwrap());
        let scene = ResolvedEffectScene::new(
            1,
            vec![test_instance(
                EffectProgramId::new(19).unwrap(),
                1,
                visible.clone(),
            )],
        );
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &visible,
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("custom fragment must compile to an effect graph");
        };
        let demand = plan_effect_execution_demand(&graph, &visible, false);
        let custom_pass = graph
            .passes
            .iter()
            .find(|pass| {
                pass.kind == RenderPassKind::Fragment
                    && matches!(pass.stage, Some(EffectNodeKind::CustomFragment(_)))
            })
            .expect("custom fragment pass");
        let capture_pass = graph
            .passes
            .iter()
            .find(|pass| pass.kind == RenderPassKind::SceneCapture)
            .expect("custom fragment capture pass");
        assert!(
            region_area(
                demand
                    .pass_output_region(capture_pass.id)
                    .expect("capture demand")
            ) > region_area(
                demand
                    .pass_output_region(custom_pass.id)
                    .expect("custom demand")
            )
        );
    }

    #[test]
    fn scaled_composite_includes_linear_filter_neighbors() {
        let instance = EffectInstanceId::new(1).unwrap();
        let capture = GraphTextureId::new(1).unwrap();
        let downsample = GraphTextureId::new(2).unwrap();
        let downsampled = GraphTextureId::new(3).unwrap();
        let upsample = GraphTextureId::new(4).unwrap();
        let effect = GraphTextureId::new(5).unwrap();
        let output = GraphTextureId::new(6).unwrap();
        let output_domain = EffectRect::new(0, 0, 1921, 1081).unwrap();
        let effect_domain = output_domain;
        let visible = EffectRegion::from_rect(EffectRect::new(500, 250, 801, 501).unwrap());
        let mut downsample_pass = test_pass(2);
        downsample_pass.kind = RenderPassKind::DualKawaseDownsample;
        downsample_pass.instance = instance;
        downsample_pass.inputs = vec![capture];
        downsample_pass.output = Some(downsample);
        downsample_pass.blur_radius = Some(4.0);
        let mut second_downsample_pass = test_pass(3);
        second_downsample_pass.kind = RenderPassKind::DualKawaseDownsample;
        second_downsample_pass.instance = instance;
        second_downsample_pass.inputs = vec![downsample];
        second_downsample_pass.output = Some(downsampled);
        second_downsample_pass.blur_radius = Some(4.0);
        let mut first_upsample_pass = test_pass(4);
        first_upsample_pass.kind = RenderPassKind::DualKawaseUpsample;
        first_upsample_pass.instance = instance;
        first_upsample_pass.inputs = vec![downsampled];
        first_upsample_pass.output = Some(upsample);
        first_upsample_pass.blur_radius = Some(4.0);
        let mut second_upsample_pass = test_pass(5);
        second_upsample_pass.kind = RenderPassKind::DualKawaseUpsample;
        second_upsample_pass.instance = instance;
        second_upsample_pass.inputs = vec![upsample];
        second_upsample_pass.output = Some(effect);
        second_upsample_pass.blur_radius = Some(4.0);
        let mut composite = test_pass(6);
        composite.kind = RenderPassKind::Composite;
        composite.instance = instance;
        composite.inputs = vec![effect];
        composite.output = Some(output);
        let final_effect_pass_id = second_upsample_pass.id;
        let mut scene_capture = test_pass(1);
        scene_capture.kind = RenderPassKind::SceneCapture;
        scene_capture.instance = instance;
        scene_capture.output = Some(capture);

        let texture = |id, source, domain, width, height| GraphTexturePlan {
            id,
            source,
            width,
            height,
            domain,
            working_space: EffectWorkingSpace::LinearSrgb,
            origin: GraphTextureOrigin::BottomLeft,
            first_use: None,
            last_use: None,
        };
        let graph = CompiledFrameGraph {
            passes: vec![
                scene_capture,
                downsample_pass,
                second_downsample_pass,
                first_upsample_pass,
                second_upsample_pass,
                composite.clone(),
            ],
            textures: vec![
                texture(
                    capture,
                    GraphTextureSource::CapturedScene,
                    EffectRect::new(476, 226, 849, 549).unwrap(),
                    849,
                    549,
                ),
                texture(
                    downsample,
                    GraphTextureSource::Intermediate,
                    EffectRect::new(476, 226, 849, 549).unwrap(),
                    425,
                    275,
                ),
                texture(
                    downsampled,
                    GraphTextureSource::Intermediate,
                    EffectRect::new(476, 226, 849, 549).unwrap(),
                    213,
                    138,
                ),
                texture(
                    upsample,
                    GraphTextureSource::Intermediate,
                    EffectRect::new(476, 226, 849, 549).unwrap(),
                    425,
                    275,
                ),
                texture(
                    effect,
                    GraphTextureSource::Intermediate,
                    effect_domain,
                    961,
                    541,
                ),
                texture(
                    output,
                    GraphTextureSource::Output,
                    output_domain,
                    1921,
                    1081,
                ),
            ],
            instances: vec![CompiledEffectInstance {
                id: instance,
                output_influence_region: visible.clone(),
                capture_region: EffectRegion::from_rect(
                    EffectRect::new(476, 226, 849, 549).unwrap(),
                ),
                dependencies: Vec::new(),
            }],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let demand = plan_effect_execution_demand(&graph, &visible, false);
        let required = demand
            .pass_output_region(final_effect_pass_id)
            .expect("scaled effect texture demand");

        assert!(required.contains_point(498, 248));
        assert!(required.contains_point(1303, 752));
        assert!(required.rects().iter().any(|rect| rect.x < 500));
        assert!(required.rects().iter().any(|rect| rect.right() > 1301));
    }

    #[test]
    fn frame_1721_rasterized_downsample_output_requires_produced_row_747() {
        let input = sampling_texture(
            1,
            GraphTextureSource::CapturedScene,
            EffectRect::new(145, 148, 1112, 873).unwrap(),
            1112,
            873,
        );
        let output = sampling_texture(
            2,
            GraphTextureSource::Intermediate,
            EffectRect::new(145, 148, 1112, 873).unwrap(),
            556,
            437,
        );
        let pass = sampling_pass(
            2,
            RenderPassKind::DualKawaseDownsample,
            input.id,
            output.id,
            4.0,
        );
        let demanded_output = EffectRegion::from_rect(EffectRect::new(647, 901, 610, 120).unwrap());
        let output_coverage =
            logical_rect_to_physical_coverage(demanded_output.rects()[0], &output)
                .expect("frame-1721 demand rasterizes");

        assert_eq!(
            output_coverage,
            GraphTexturePhysicalRect {
                left: 251,
                top: 376,
                right: 556,
                bottom: 437,
            }
        );
        let sampled_row = (((f64::from(output_coverage.top) + 0.5) * f64::from(input.height)
            / f64::from(output.height))
            - 0.5
            - 4.0)
            .floor() as u32;
        assert_eq!(
            sampled_row, 747,
            "row 376's negative Kawase tap reaches row 747"
        );

        let planned = required_input_region(&pass, &demanded_output, &output, &input)
            .expect("frame-1721 demand maps to a producer region");
        let planned_physical_top = planned
            .rects()
            .iter()
            .filter_map(|rect| logical_rect_to_physical_coverage(*rect, &input))
            .map(|coverage| coverage.top)
            .min()
            .expect("planned producer region is non-empty");
        let planned_physical_left = planned
            .rects()
            .iter()
            .filter_map(|rect| logical_rect_to_physical_coverage(*rect, &input))
            .map(|coverage| coverage.left)
            .min()
            .expect("planned producer region is non-empty");
        assert_eq!(planned_physical_top, 747);
        assert!(
            physical_region_contains(&planned, &input, planned_physical_left, sampled_row),
            "every texel sampled by rasterized row {} must be produced; planned producer coverage starts at physical row {planned_physical_top}, leaving row {sampled_row} undefined",
            output_coverage.top
        );
    }

    #[test]
    fn raster_aware_kawase_demand_covers_exhaustive_edge_oracle() {
        for input_width in 1..=16 {
            for input_height in 1..=16 {
                assert_sample_coverage(
                    RenderPassKind::DualKawaseDownsample,
                    input_width,
                    input_height,
                    input_width.div_ceil(2),
                    input_height.div_ceil(2),
                    (145, 148),
                    (145, 148),
                    false,
                );
            }
        }

        for (input_width, input_height, output_width, output_height) in [
            (1112, 873, 556, 437),
            (1112, 873, 278, 219),
            (873, 437, 437, 219),
            (437, 321, 219, 161),
            (321, 181, 161, 91),
            (321, 181, 81, 46),
            (181, 91, 91, 46),
        ] {
            assert_sample_coverage(
                RenderPassKind::DualKawaseDownsample,
                input_width,
                input_height,
                output_width,
                output_height,
                (145, 148),
                (145, 148),
                false,
            );
        }

        for (input_width, input_height, output_width, output_height) in [
            (1, 1, 2, 2),
            (2, 3, 4, 6),
            (81, 46, 161, 91),
            (219, 161, 437, 321),
            (437, 219, 873, 437),
        ] {
            assert_sample_coverage(
                RenderPassKind::DualKawaseUpsample,
                input_width,
                input_height,
                output_width,
                output_height,
                (145, 148),
                (145, 148),
                false,
            );
        }
    }

    #[test]
    fn scaled_blur_graph_uses_raster_aware_dimensions_for_odd_domains() {
        let domain = EffectRect::new(145, 148, 321, 181).unwrap();
        let (scene, registry) =
            blur_scene_with_region_and_scale(EffectRegion::from_rect(domain), 0.5);
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &EffectRegion::from_rect(domain),
            domain,
            &registry,
        )
        .unwrap() else {
            panic!("scaled blur must compile to an effect graph");
        };
        let blur_dimensions = graph
            .passes
            .iter()
            .filter(|pass| {
                matches!(
                    pass.kind,
                    RenderPassKind::DualKawaseDownsample | RenderPassKind::DualKawaseUpsample
                )
            })
            .map(|pass| {
                let output = graph
                    .textures
                    .iter()
                    .find(|texture| texture.id == pass.output.unwrap())
                    .expect("scaled blur pass output texture");
                (pass.kind, output.width, output.height)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            blur_dimensions,
            vec![
                (RenderPassKind::DualKawaseDownsample, 81, 46),
                (RenderPassKind::DualKawaseDownsample, 41, 23),
                (RenderPassKind::DualKawaseUpsample, 81, 46),
                (RenderPassKind::DualKawaseUpsample, 161, 91),
            ]
        );
    }

    #[test]
    fn raster_aware_logical_mapping_covers_translated_non_unit_scale_oracle() {
        for kind in [RenderPassKind::NormalizeInput, RenderPassKind::Composite] {
            assert_sample_coverage(kind, 5, 4, 8, 6, (0, 0), (13, 17), true);
            assert_sample_coverage(kind, 7, 5, 11, 9, (0, 0), (145, 148), true);
        }
    }

    #[test]
    fn odd_blur_dimensions_round_outward_and_clip_to_capture_domain() {
        let visible = EffectRegion::from_rect(EffectRect::new(101, 79, 321, 181).unwrap());
        let (scene, registry) = blur_scene_with_region(visible.clone());
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &visible,
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("odd-sized blur must compile to an effect graph");
        };
        let repair = EffectRegion::from_rect(EffectRect::new(180, 120, 9, 7).unwrap());
        let demand = plan_effect_execution_demand(&graph, &repair, false);
        let capture_pass = graph
            .passes
            .iter()
            .find(|pass| pass.kind == RenderPassKind::SceneCapture)
            .expect("blur capture pass");
        let capture_texture = graph
            .textures
            .iter()
            .find(|texture| texture.id == capture_pass.output.unwrap())
            .expect("blur capture texture");
        let capture_demand = demand
            .pass_output_region(capture_pass.id)
            .expect("capture demand");
        assert!(capture_demand.rects().iter().all(|rect| {
            rect.x >= capture_texture.domain.x
                && rect.y >= capture_texture.domain.y
                && rect.right() <= capture_texture.domain.right()
                && rect.bottom() <= capture_texture.domain.bottom()
        }));
        assert!(demand.plan_stats().max_pass_region_rect_count <= MAX_EFFECT_REGION_RECTS);
    }

    #[test]
    fn translated_domains_preserve_relative_pass_demand() {
        let first_visible = EffectRegion::from_rect(EffectRect::new(100, 80, 320, 180).unwrap());
        let second_visible = translated_region(&first_visible, 400, 220);
        let (first_scene, registry) = blur_scene_with_region(first_visible.clone());
        let (second_scene, _) = blur_scene_with_region(second_visible.clone());
        let output_bounds = EffectRect::new(0, 0, 1920, 1080).unwrap();
        let FrameExecutionPlan::EffectGraph(first_graph) =
            compile_frame_execution_plan(&first_scene, &first_visible, output_bounds, &registry)
                .unwrap()
        else {
            panic!("first blur must compile to an effect graph");
        };
        let FrameExecutionPlan::EffectGraph(second_graph) =
            compile_frame_execution_plan(&second_scene, &second_visible, output_bounds, &registry)
                .unwrap()
        else {
            panic!("translated blur must compile to an effect graph");
        };
        let first_repair = EffectRegion::from_rect(EffectRect::new(120, 100, 12, 10).unwrap());
        let second_repair = translated_region(&first_repair, 400, 220);
        let first_demand = plan_effect_execution_demand(&first_graph, &first_repair, false);
        let second_demand = plan_effect_execution_demand(&second_graph, &second_repair, false);
        assert_eq!(first_graph.passes.len(), second_graph.passes.len());
        for (first_pass, second_pass) in first_graph.passes.iter().zip(&second_graph.passes) {
            let first_region = first_demand.pass_output_region(first_pass.id).unwrap();
            let second_region = second_demand.pass_output_region(second_pass.id).unwrap();
            assert_eq!(
                relative_rects(first_region, 0, 0),
                relative_rects(second_region, 400, 220)
            );
        }
    }

    #[test]
    fn fragmented_pass_demand_stays_within_effect_region_bound() {
        let (scene, registry) = blur_scene();
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &EffectRegion::empty(),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("visible blur must compile to an effect graph");
        };
        let mut repair = EffectRegion::empty();
        for index in 0..MAX_EFFECT_REGION_RECTS {
            repair.push(
                EffectRect::new(
                    110 + (index as i32 % 16) * 12,
                    90 + (index as i32 / 16) * 12,
                    4,
                    4,
                )
                .unwrap(),
            );
        }
        let demand = plan_effect_execution_demand(&graph, &repair, false);

        assert!(
            demand
                .passes
                .iter()
                .all(|pass| pass.output_region.rects().len() <= MAX_EFFECT_REGION_RECTS)
        );
        assert!(demand.plan_stats().max_pass_region_rect_count <= MAX_EFFECT_REGION_RECTS);
        assert!(demand.plan_stats().pass_dependency_propagations <= graph.passes.len() * 2);
    }

    #[test]
    fn transitive_backdrop_dependencies_keep_the_earliest_effect_live() {
        let (scene, registry) = blur_scene();
        let mut first = scene.instances[0].clone();
        first.region = EffectRegion::from_rect(EffectRect::new(100, 80, 20, 180).unwrap());
        first.target_bounds = first.region.bounding_rect().unwrap();
        let mut second = first.clone();
        second.id = EffectInstanceId::new(2).unwrap();
        second.region = EffectRegion::from_rect(EffectRect::new(130, 80, 20, 180).unwrap());
        second.target_bounds = second.region.bounding_rect().unwrap();
        let mut third = second.clone();
        third.id = EffectInstanceId::new(3).unwrap();
        third.region = EffectRegion::from_rect(EffectRect::new(160, 80, 20, 180).unwrap());
        third.target_bounds = third.region.bounding_rect().unwrap();
        let scene = ResolvedEffectScene::new(1, vec![first, second, third]);
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &EffectRegion::empty(),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("visible effects must compile to an effect graph");
        };

        assert!(
            graph.instances[1]
                .dependencies
                .contains(&EffectInstanceId::new(1).unwrap())
        );
        assert!(
            graph.instances[2]
                .dependencies
                .contains(&EffectInstanceId::new(2).unwrap())
        );
        assert!(
            !graph.instances[2]
                .dependencies
                .contains(&EffectInstanceId::new(1).unwrap())
        );
        let demand = plan_effect_execution_demand(
            &graph,
            &EffectRegion::from_rect(EffectRect::new(160, 80, 20, 180).unwrap()),
            false,
        );
        assert_eq!(demand.instances.len(), 3);
    }

    #[test]
    fn empty_repair_does_not_make_effects_live() {
        let (scene, registry) = separated_blur_scene();
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &EffectRegion::empty(),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("visible effects must compile to an effect graph");
        };

        let demand = plan_effect_execution_demand(&graph, &EffectRegion::empty(), false);
        assert!(demand.instances.is_empty());
        assert!(demand.execution_region.is_empty());
    }

    #[test]
    fn target_content_does_not_invent_backdrop_dependency() {
        let (backdrop_scene, mut registry) = blur_scene();
        let source = EffectNodeId::new(1).unwrap();
        let program = validate_effect_program(EffectProgram {
            id: EffectProgramId::new(2).unwrap(),
            nodes: vec![EffectNode::source(source, EffectSource::TargetContent)],
            output: source,
            working_space: EffectWorkingSpace::OutputEncodedSrgb,
            alpha_mode: EffectAlphaMode::Preserve,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        })
        .unwrap();
        registry.insert(program).unwrap();
        let mut backdrop = backdrop_scene.instances[0].clone();
        backdrop.region = EffectRegion::from_rect(EffectRect::new(100, 80, 20, 180).unwrap());
        backdrop.target_bounds = backdrop.region.bounding_rect().unwrap();
        let mut target = backdrop.clone();
        target.id = EffectInstanceId::new(2).unwrap();
        target.program = EffectProgramId::new(2).unwrap();
        target.anchor = EffectAnchor::OutputPostProcess;
        target.scene_order = EffectSceneOrder::for_anchor(EffectAnchor::OutputPostProcess);
        target.region = EffectRegion::from_rect(EffectRect::new(130, 80, 20, 180).unwrap());
        target.target_bounds = target.region.bounding_rect().unwrap();
        let scene = ResolvedEffectScene::new(1, vec![backdrop, target]);
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &EffectRegion::empty(),
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("visible effects must compile to an effect graph");
        };

        assert!(graph.instances[1].dependencies.is_empty());
        let demand = plan_effect_execution_demand(
            &graph,
            &EffectRegion::from_rect(EffectRect::new(130, 80, 20, 180).unwrap()),
            false,
        );
        assert!(demand.contains(EffectInstanceId::new(2).unwrap()));
        assert!(!demand.contains(EffectInstanceId::new(1).unwrap()));
    }
}
