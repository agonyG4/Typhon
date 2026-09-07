use std::{collections::HashMap, num::NonZeroU16};

use crate::compositor::{EffectAnchor, ResolvedEffectInstance, ResolvedEffectScene};

use super::{
    DualKawaseBlurSpec, EffectAlphaMode, EffectFailurePolicy, EffectFrameDemand, EffectInstanceId,
    EffectNode, EffectNodeId, EffectNodeKind, EffectOutsets, EffectProgram, EffectProgramId,
    EffectRect, EffectRegion, EffectSource, EffectValidationError, EffectWorkingSpace,
    MAX_EFFECT_PROGRAMS, ValidatedEffectProgram, plan_effect_damage, validate_effect_program,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphTexturePlan {
    pub id: GraphTextureId,
    pub source: GraphTextureSource,
    pub width: u32,
    pub height: u32,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledRenderPass {
    pub id: GraphPassId,
    pub kind: RenderPassKind,
    pub inputs: Vec<GraphTextureId>,
    pub output: Option<GraphTextureId>,
    pub damage: EffectRegion,
    pub instance: EffectInstanceId,
    pub anchor: EffectAnchor,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenderGraphCompileStats {
    pub effect_instances: usize,
    pub passes: usize,
    pub textures: usize,
    pub intermediate_textures: usize,
    pub peak_live_intermediates: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledFrameGraph {
    pub passes: Vec<CompiledRenderPass>,
    pub textures: Vec<GraphTexturePlan>,
    pub final_damage: EffectRegion,
    pub stats: RenderGraphCompileStats,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameExecutionPlan {
    LegacyScene,
    EffectGraph(CompiledFrameGraph),
}

#[derive(Clone, Debug, Default)]
pub struct EffectRegistry {
    programs: HashMap<EffectProgramId, ValidatedEffectProgram>,
}

impl EffectRegistry {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn with_builtin_background_blur() -> Self {
        let mut registry = Self::empty();
        registry
            .insert(builtin_background_blur_program())
            .expect("builtin effect registry has capacity");
        registry
    }

    pub fn insert(&mut self, program: ValidatedEffectProgram) -> Result<(), EffectRegistryError> {
        if self.programs.len() >= MAX_EFFECT_PROGRAMS
            && !self.programs.contains_key(&program.program.id)
        {
            return Err(EffectRegistryError::Full);
        }
        self.programs.insert(program.program.id, program);
        Ok(())
    }

    pub fn get(&self, id: EffectProgramId) -> Option<&ValidatedEffectProgram> {
        self.programs.get(&id)
    }

    pub fn len(&self) -> usize {
        self.programs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.programs.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectRegistryError {
    Full,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderGraphCompileError {
    MissingProgram(EffectProgramId),
    InvalidGraph(EffectValidationError),
    TooManyTextures,
    TooManyPasses,
    TextureIdOverflow,
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
        let output = builder.add_texture(GraphTextureSource::Output, output_bounds)?;
        Ok((builder, output))
    }

    fn add_texture(
        &mut self,
        source: GraphTextureSource,
        bounds: EffectRect,
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
            width: bounds.width,
            height: bounds.height,
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

    fn add_pass(
        &mut self,
        kind: RenderPassKind,
        inputs: Vec<GraphTextureId>,
        output: Option<GraphTextureId>,
        damage: EffectRegion,
        instance: EffectInstanceId,
        anchor: EffectAnchor,
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
        });
        Ok(id)
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
                let texture_source = match source {
                    EffectSource::Backdrop => GraphTextureSource::CapturedScene,
                    EffectSource::TargetContent => GraphTextureSource::CapturedTarget,
                    EffectSource::StaticTexture(id) => GraphTextureSource::Static(*id),
                };
                let texture = builder.add_texture(texture_source, output_bounds)?;
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
                    )?;
                }
                outputs.insert(node.id, texture);
            }
            EffectNodeKind::DualKawaseBlur(spec) => {
                let input = outputs[&node.inputs[0]];
                let mut current = input;
                for _ in 0..spec.passes {
                    let texture = builder.add_texture(
                        GraphTextureSource::Intermediate,
                        scaled_bounds(output_bounds, spec.scale),
                    )?;
                    builder.add_pass(
                        RenderPassKind::DualKawaseDownsample,
                        vec![current],
                        Some(texture),
                        capture_damage.clone(),
                        instance.id,
                        instance.anchor,
                    )?;
                    current = texture;
                }
                for _ in 0..spec.passes {
                    let texture = builder.add_texture(
                        GraphTextureSource::Intermediate,
                        scaled_bounds(output_bounds, spec.scale),
                    )?;
                    builder.add_pass(
                        RenderPassKind::DualKawaseUpsample,
                        vec![current],
                        Some(texture),
                        output_damage.clone(),
                        instance.id,
                        instance.anchor,
                    )?;
                    current = texture;
                }
                outputs.insert(node.id, current);
            }
            EffectNodeKind::ColorMatrix(_)
            | EffectNodeKind::Tint(_)
            | EffectNodeKind::Noise(_)
            | EffectNodeKind::CustomFragment(_) => {
                let input = node
                    .inputs
                    .iter()
                    .map(|input| outputs[input])
                    .collect::<Vec<_>>();
                let texture =
                    builder.add_texture(GraphTextureSource::Intermediate, output_bounds)?;
                builder.add_pass(
                    RenderPassKind::Fragment,
                    input,
                    Some(texture),
                    output_damage.clone(),
                    instance.id,
                    instance.anchor,
                )?;
                outputs.insert(node.id, texture);
            }
            EffectNodeKind::Blend(_) => {
                let input = node
                    .inputs
                    .iter()
                    .map(|input| outputs[input])
                    .collect::<Vec<_>>();
                let texture =
                    builder.add_texture(GraphTextureSource::Intermediate, output_bounds)?;
                builder.add_pass(
                    RenderPassKind::Blend,
                    input,
                    Some(texture),
                    output_damage.clone(),
                    instance.id,
                    instance.anchor,
                )?;
                outputs.insert(node.id, texture);
            }
            EffectNodeKind::Mask(_) => {
                let input = node
                    .inputs
                    .iter()
                    .map(|input| outputs[input])
                    .collect::<Vec<_>>();
                let texture =
                    builder.add_texture(GraphTextureSource::Intermediate, output_bounds)?;
                builder.add_pass(
                    RenderPassKind::Mask,
                    input,
                    Some(texture),
                    output_damage.clone(),
                    instance.id,
                    instance.anchor,
                )?;
                outputs.insert(node.id, texture);
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
    )?;
    Ok(())
}

fn scaled_bounds(bounds: EffectRect, scale: f32) -> EffectRect {
    let width = (f64::from(bounds.width) * f64::from(scale)).ceil().max(1.0) as u32;
    let height = (f64::from(bounds.height) * f64::from(scale))
        .ceil()
        .max(1.0) as u32;
    EffectRect::new(bounds.x, bounds.y, width, height).unwrap_or(bounds)
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
}
