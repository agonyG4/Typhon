use std::{collections::HashMap, num::NonZeroU16};

use crate::compositor::{EffectAnchor, ResolvedEffectInstance, ResolvedEffectScene};

use super::registry::EffectRegistry;
use super::{
    DualKawaseBlurSpec, EffectAlphaMode, EffectFailurePolicy, EffectFrameDemand, EffectInstanceId,
    EffectNode, EffectNodeId, EffectNodeKind, EffectOutsets, EffectProgram, EffectProgramId,
    EffectRect, EffectRegion, EffectSource, EffectValidationError, EffectWorkingSpace,
    ValidatedEffectProgram, plan_effect_damage, validate_effect_program,
};

pub const MAX_GRAPH_TEXTURES: usize = 4096;
pub const MAX_GRAPH_PASSES: usize = 4096;
pub const BUILTIN_BACKGROUND_BLUR_NAME: &str = "system.background_blur";

pub fn builtin_background_blur_program_id() -> EffectProgramId {
    EffectProgramId::new(1).expect("builtin effect program id is non-zero")
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
    DualKawaseDownsample,
    DualKawaseUpsample,
    Fragment,
    Blend,
    Mask,
    Composite,
    OutputPostProcess,
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
    pub final_damage: EffectRegion,
    pub stats: RenderGraphCompileStats,
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
    ) -> Result<GraphPassId, RenderGraphCompileError> {
        let pass = self.add_pass(kind, inputs, output, damage, instance, anchor, None)?;
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
        compile_instance(
            &mut builder,
            output_texture,
            instance,
            program,
            &effect_damage.capture_region,
            &effect_damage.output_damage,
            output_bounds,
        )?;
    }

    fuse_compatible_local_stages(&mut builder);
    let intermediate_textures = builder
        .textures
        .iter()
        .filter(|texture| texture.source == GraphTextureSource::Intermediate)
        .count();
    let peak_live_intermediates = (0..builder.passes.len())
        .map(|pass_index| {
            builder
                .textures
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
        .unwrap_or(0);
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
        final_damage,
        stats,
    }))
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

fn compile_instance(
    builder: &mut GraphBuilder,
    output_texture: GraphTextureId,
    instance: &ResolvedEffectInstance,
    program: &ValidatedEffectProgram,
    capture_damage: &EffectRegion,
    output_damage: &EffectRegion,
    output_bounds: EffectRect,
) -> Result<(), RenderGraphCompileError> {
    let nodes = program
        .program
        .nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();
    let mut outputs = HashMap::<EffectNodeId, GraphTextureId>::new();

    for node_id in &program.topological_order {
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
                    )?;
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
                let input_plan = builder.texture(inputs[0]);
                let output = builder.add_texture_with_layout(
                    GraphTextureSource::Intermediate,
                    input_plan.domain,
                    input_plan.width,
                    input_plan.height,
                    EffectWorkingSpace::LinearSrgb,
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
                )?;
                builder
                    .passes
                    .last_mut()
                    .expect("stage pass was appended")
                    .parameter_block = instance.parameter_block.clone();
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
    builder.add_pass(
        kind,
        vec![final_texture],
        Some(output_texture),
        output_damage.clone(),
        instance.id,
        instance.anchor,
        None,
    )?;
    Ok(())
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
    use crate::compositor::{EffectAnchor, ResolvedEffectInstance, ResolvedEffectScene};
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
        };
        (ResolvedEffectScene::new(1, vec![instance]), registry)
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
        assert!(graph.passes.iter().any(|pass| {
            pass.kind == RenderPassKind::Fragment
                && matches!(pass.stage, Some(EffectNodeKind::Tint(_)))
        }));
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
    }
}
