use std::{collections::HashMap, fmt::Write as _, num::NonZeroU16};

use crate::compositor::{
    EffectAnchor, EffectAnchorScope, ResolvedEffectInstance, ResolvedEffectScene, VisualGroupId,
};

use super::registry::EffectRegistry;
use super::{
    BUILTIN_EFFECT_PROGRAM_ID, DualKawaseBlurSpec, EffectAlphaMode, EffectFailurePolicy,
    EffectFrameDemand, EffectInstanceId, EffectNode, EffectNodeId, EffectNodeKind, EffectOutsets,
    EffectProgram, EffectProgramId, EffectRect, EffectRegion, EffectSource, EffectValidationError,
    EffectWorkingSpace, ValidatedEffectProgram, plan_effect_damage, validate_effect_program,
};

pub const MAX_GRAPH_TEXTURES: usize = 4096;
pub const MAX_GRAPH_PASSES: usize = 4096;
pub const BUILTIN_BACKGROUND_BLUR_NAME: &str = "system.background_blur";

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PeakLiveWorkCounters {
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
fn reset_peak_live_work_counters() {
    PEAK_LIVE_WORK_COUNTERS.with(|counters| counters.set(PeakLiveWorkCounters::default()));
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
fn note_program_lookup_map_build() {
    PEAK_LIVE_WORK_COUNTERS.with(|counters| {
        let mut value = counters.get();
        value.program_lookup_map_builds += 1;
        counters.set(value);
    });
}

#[cfg(test)]
fn note_output_map_build() {
    PEAK_LIVE_WORK_COUNTERS.with(|counters| {
        let mut value = counters.get();
        value.output_map_builds += 1;
        counters.set(value);
    });
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
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenderGraphCompileStats {
    pub effect_instances: usize,
    pub passes: usize,
    pub textures: usize,
    pub intermediate_textures: usize,
    pub peak_live_intermediates: usize,
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

#[derive(Clone, Debug, PartialEq)]
pub struct EffectInstanceExecutionDemand {
    pub id: EffectInstanceId,
    pub output_region: EffectRegion,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectExecutionDemand {
    pub instances: Vec<EffectInstanceExecutionDemand>,
    pub execution_region: EffectRegion,
    pub(crate) conservative_full: bool,
}

impl EffectExecutionDemand {
    pub fn new(
        instances: Vec<EffectInstanceExecutionDemand>,
        execution_region: EffectRegion,
    ) -> Self {
        Self {
            instances,
            execution_region,
            conservative_full: false,
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
}

fn all_visible_instances_with_output_regions(graph: &CompiledFrameGraph) -> EffectExecutionDemand {
    let mut execution_region = EffectRegion::empty();
    let instances = graph
        .instances
        .iter()
        .map(|instance| {
            execution_region = execution_region.union(&instance.output_influence_region);
            if !instance.dependencies.is_empty() {
                execution_region = execution_region.union(&instance.capture_region);
            }
            EffectInstanceExecutionDemand {
                id: instance.id,
                output_region: instance.output_influence_region.clone(),
            }
        })
        .collect();
    EffectExecutionDemand {
        instances,
        execution_region,
        conservative_full: true,
    }
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

pub fn plan_effect_execution_demand(
    graph: &CompiledFrameGraph,
    repair_region: &EffectRegion,
    conservative_full: bool,
) -> EffectExecutionDemand {
    if conservative_full
        || (!repair_region.is_empty() && repair_region.bounding_rect().is_none())
        || !graph_execution_metadata_is_complete(graph)
    {
        return all_visible_instances_with_output_regions(graph);
    }

    let mut output_regions = vec![None; graph.instances.len()];
    for (index, instance) in graph.instances.iter().enumerate() {
        let direct = repair_region.intersect(&instance.output_influence_region);
        if !direct.is_empty() {
            output_regions[index] = Some(direct);
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for consumer_index in 0..graph.instances.len() {
            if output_regions[consumer_index].is_none() {
                continue;
            }
            let consumer = &graph.instances[consumer_index];
            for dependency_id in &consumer.dependencies {
                let Some(dependency_index) = unique_instance_index(graph, *dependency_id) else {
                    return all_visible_instances_with_output_regions(graph);
                };
                let dependency = &graph.instances[dependency_index];
                let required = dependency
                    .output_influence_region
                    .intersect(&consumer.capture_region);
                if required.is_empty() {
                    continue;
                }
                if output_regions[dependency_index]
                    .as_ref()
                    .is_some_and(|existing| existing.intersect(&required) == required)
                {
                    continue;
                }
                let next = output_regions[dependency_index]
                    .as_ref()
                    .map_or_else(|| required.clone(), |existing| existing.union(&required));
                if output_regions[dependency_index].as_ref() != Some(&next) {
                    output_regions[dependency_index] = Some(next);
                    changed = true;
                }
            }
        }
    }

    let mut execution_region = EffectRegion::empty();
    let instances = output_regions
        .into_iter()
        .enumerate()
        .filter_map(|(index, output_region)| {
            output_region.map(|output_region| {
                let instance = &graph.instances[index];
                execution_region = execution_region.union(&output_region);
                if !instance.dependencies.is_empty() {
                    execution_region = execution_region.union(&instance.capture_region);
                }
                EffectInstanceExecutionDemand {
                    id: graph.instances[index].id,
                    output_region,
                }
            })
        })
        .collect();
    EffectExecutionDemand {
        instances,
        execution_region,
        conservative_full: false,
    }
}

fn graph_execution_metadata_is_complete(graph: &CompiledFrameGraph) -> bool {
    graph
        .instances
        .iter()
        .enumerate()
        .all(|(index, instance)| unique_instance_index(graph, instance.id) == Some(index))
        && graph.passes.iter().all(|pass| {
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
    let visible_instances = scene
        .instances
        .iter()
        .filter(|instance| !instance.region.is_empty())
        .collect::<Vec<_>>();
    if visible_instances.is_empty() {
        return Ok(FrameExecutionPlan::LegacyScene);
    }

    let (mut builder, output_texture) = GraphBuilder::new(output_bounds)?;
    let mut final_damage = source_damage.clone();
    let mut checkpoints = Vec::<(GraphPassId, EffectRegion, EffectInstanceId)>::new();
    let mut compiled_instances = Vec::with_capacity(visible_instances.len());

    for instance in visible_instances {
        let program = registry
            .get(instance.program)
            .ok_or(RenderGraphCompileError::MissingProgram(instance.program))?;
        let effect_damage = plan_effect_damage(
            program.aggregate_footprint,
            &instance.region,
            source_damage,
            output_bounds,
        );
        final_damage = final_damage.union(&effect_damage.output_damage);
        let dependencies = checkpoints
            .iter()
            .filter(|(_, region, _)| region.intersects(&effect_damage.capture_region))
            .map(|(pass, _, _)| *pass)
            .collect::<Vec<_>>();
        let dependency_instances = if program
            .program
            .nodes
            .iter()
            .any(|node| matches!(node.kind, EffectNodeKind::Source(EffectSource::Backdrop)))
        {
            checkpoints
                .iter()
                .filter(|(_, region, _)| region.intersects(&effect_damage.capture_region))
                .map(|(_, _, instance_id)| *instance_id)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
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
    let peak_live_intermediates = peak_live_intermediates(&builder.textures, builder.passes.len());
    let stats = RenderGraphCompileStats {
        effect_instances: scene
            .instances
            .iter()
            .filter(|instance| !instance.region.is_empty())
            .count(),
        passes: builder.passes.len(),
        textures: builder.textures.len(),
        intermediate_textures,
        peak_live_intermediates,
    };
    Ok(FrameExecutionPlan::EffectGraph(CompiledFrameGraph {
        passes: builder.passes,
        textures: builder.textures,
        instances: compiled_instances,
        final_damage,
        stats,
    }))
}

fn peak_live_intermediates(textures: &[GraphTexturePlan], pass_count: usize) -> usize {
    if pass_count == 0 {
        return 0;
    }

    let mut delta = vec![0_i32; pass_count + 1];
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
        let first = usize::from(first.get() - 1);
        let last = usize::from(last.get() - 1);
        if first >= pass_count {
            debug_assert!(first < pass_count);
            continue;
        }
        let last = last.min(pass_count - 1);
        if first > last {
            debug_assert!(first <= last);
            continue;
        }
        #[cfg(test)]
        note_peak_live_interval_insertion();
        delta[first] += 1;
        delta[last + 1] -= 1;
    }

    let mut live = 0_i32;
    let mut peak = 0_i32;
    for change in delta.into_iter().take(pass_count) {
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
        let Some(second_stage) = builder.passes[index + 1].stage.as_ref() else {
            index += 1;
            continue;
        };
        let can_fuse = first.kind == RenderPassKind::Fragment
            && second.kind == RenderPassKind::Fragment
            && first.instance == second.instance
            && first.output.is_some()
            && second.inputs == first.output.into_iter().collect::<Vec<_>>()
            && first.fused_stages.is_empty()
            && second.fused_stages.is_empty()
            && compatible_local_stage_order(first_stage, second_stage)
            && first
                .output
                .and_then(|id| builder.textures.iter().find(|texture| texture.id == id))
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
    } = plan;
    let nodes = program
        .program
        .nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();
    #[cfg(test)]
    note_program_lookup_map_build();
    let visual_group = instance.visual_group;
    let mut outputs = HashMap::<EffectNodeId, GraphTextureId>::new();
    #[cfg(test)]
    note_output_map_build();

    for node_id in &program.topological_order {
        #[cfg(test)]
        note_node_visit();
        let node = nodes
            .get(node_id)
            .copied()
            .ok_or(RenderGraphCompileError::InvalidGraph(
                EffectValidationError::MissingOutputNode,
            ))?;
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
                outputs.insert(node.id, texture);
            }
            EffectNodeKind::DualKawaseBlur(spec) => {
                let input = outputs[&node.inputs[0]];
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
                outputs.insert(node.id, current);
            }
            EffectNodeKind::ColorMatrix(_)
            | EffectNodeKind::Tint(_)
            | EffectNodeKind::Noise(_)
            | EffectNodeKind::CustomFragment(_)
            | EffectNodeKind::Blend(_)
            | EffectNodeKind::Mask(_) => {
                let inputs = node
                    .inputs
                    .iter()
                    .map(|input| outputs.get(input).copied())
                    .collect::<Option<Vec<_>>>()
                    .ok_or(RenderGraphCompileError::InvalidGraph(
                        EffectValidationError::MissingInputNode(node.id),
                    ))?;
                let primary_plan = builder.texture(inputs[0]);
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
                    let stage_footprint = super::footprint::node_footprint(&node.kind)
                        .map_err(RenderGraphCompileError::InvalidGraph)?;
                    let normalized_damage = output_damage.expand_clamped_xy(
                        stage_footprint.sample_radius_x,
                        stage_footprint.sample_radius_y,
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
                let input_plan = builder.texture(inputs[0]);
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
                outputs.insert(node.id, output);
            }
        }
    }

    let final_texture = outputs.get(&program.program.output).copied().ok_or(
        RenderGraphCompileError::InvalidGraph(EffectValidationError::MissingOutputNode),
    )?;
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
    use super::*;
    use crate::compositor::{
        EffectAnchor, EffectAnchorScope, EffectSceneOrder, ResolvedEffectInstance,
        ResolvedEffectScene,
    };
    use crate::effects::*;

    fn blur_scene() -> (ResolvedEffectScene, EffectRegistry) {
        let source = EffectNodeId::new(1).unwrap();
        let blur = EffectNodeId::new(2).unwrap();
        let program = EffectProgram {
            id: EffectProgramId::new(1).unwrap(),
            nodes: vec![
                EffectNode::source(source, EffectSource::Backdrop),
                EffectNode::dual_kawase(
                    blur,
                    source,
                    DualKawaseBlurSpec::new(4.0, 2, 1.0).unwrap(),
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
        let region = EffectRegion::from_rect(EffectRect::new(100, 80, 320, 180).unwrap());
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

    fn brute_force_peak(textures: &[GraphTexturePlan], pass_count: usize) -> usize {
        (0..pass_count)
            .map(|pass_index| {
                textures
                    .iter()
                    .filter(|texture| {
                        texture.source == GraphTextureSource::Intermediate
                            && texture
                                .first_use
                                .is_some_and(|first| usize::from(first.get() - 1) <= pass_index)
                            && texture
                                .last_use
                                .is_some_and(|last| usize::from(last.get() - 1) >= pass_index)
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
            assert_eq!(
                peak_live_intermediates(&textures, pass_count),
                brute_force_peak(&textures, pass_count),
                "fixture with {pass_count} passes"
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
            "effects graph: instances=1 passes=6 textures=6 peak_live=2 capture_px=83904 output_px=115200\n"
        ));
        assert!(explanation.contains("capture_scene"));
        assert!(explanation.contains("kawase_down"));
        assert!(!explanation.contains("#version"));
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
        assert!(blend_pass.inputs.iter().all(|input| {
            graph
                .textures
                .iter()
                .find(|texture| texture.id == *input)
                .is_some_and(|texture| texture.working_space == EffectWorkingSpace::LinearSrgb)
        }));
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
            brute_force_peak(&graph.textures, graph.passes.len())
        );
    }

    #[test]
    fn peak_live_work_scales_with_intervals_and_passes() {
        let textures = (1..=512)
            .map(|id| lifetime_texture(id, Some(1), Some(256), GraphTextureSource::Intermediate))
            .collect::<Vec<_>>();
        reset_peak_live_work_counters();

        assert_eq!(peak_live_intermediates(&textures, 256), 512);
        assert_eq!(
            peak_live_work_counters(),
            PeakLiveWorkCounters {
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
    fn c2_effect_workloads_report_repeated_instance_metadata() {
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
            assert_eq!(many_counters.program_lookup_map_builds, effect_count);
            assert_eq!(many_counters.output_map_builds, effect_count);
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
            compile_frame_execution_plan(&scene, &source_damage, output_bounds, &registry)
                .unwrap()
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
        assert_eq!(original_counters.program_lookup_map_builds, 1);
        assert_eq!(changed_counters.program_lookup_map_builds, 1);
        assert_eq!(original_counters.output_map_builds, 1);
        assert_eq!(changed_counters.output_map_builds, 1);
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
