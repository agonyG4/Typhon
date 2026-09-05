#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_program_model_is_renderer_independent() {
        use super::super::parameters::EffectParameterBlock;

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

        assert_eq!(program.nodes.len(), 2);
    }

    #[test]
    fn parameter_block_rejects_duplicate_and_non_finite_values() {
        use super::super::parameters::{EffectParameterBlock, EffectUniformValue};
        use super::EffectParameterId;

        let parameter = EffectParameterId::new(1).unwrap();
        let mut block = EffectParameterBlock::default();
        block
            .insert(parameter, EffectUniformValue::Float(1.0))
            .unwrap();
        assert!(
            block
                .insert(parameter, EffectUniformValue::Float(2.0))
                .is_err()
        );
        assert!(
            EffectParameterBlock::from_values([(
                EffectParameterId::new(2).unwrap(),
                EffectUniformValue::Vec2([f32::NAN, 0.0]),
            )])
            .is_err()
        );
    }
}
use std::num::{NonZeroU16, NonZeroU64};

pub const MAX_EFFECT_PROGRAM_NODES: usize = 32;
pub const MAX_EFFECT_INSTANCES_PER_OUTPUT: usize = 128;
pub const MAX_EFFECT_UNIFORMS_PER_SHADER: usize = 64;
pub const MAX_EFFECT_AUX_TEXTURES_PER_SHADER: usize = 8;
pub const MAX_EFFECT_BLUR_PASSES: u8 = 8;
pub const MAX_EFFECT_REGION_RECTS: usize = 128;
pub const MAX_EFFECT_PROGRAMS: usize = 256;
pub const MAX_EFFECT_SHADER_SOURCE_BYTES: usize = 256 * 1024;

macro_rules! typed_id {
    ($name:ident, $raw:ty, $inner:ty) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name($inner);

        impl $name {
            pub const fn new(value: $raw) -> Option<Self> {
                match <$inner>::new(value) {
                    Some(value) => Some(Self(value)),
                    None => None,
                }
            }

            pub const fn get(self) -> $raw {
                self.0.get()
            }
        }
    };
}

typed_id!(EffectProgramId, u64, NonZeroU64);
typed_id!(EffectInstanceId, u64, NonZeroU64);
typed_id!(EffectNodeId, u16, NonZeroU16);
typed_id!(EffectParameterId, u16, NonZeroU16);
typed_id!(ShaderModuleId, u64, NonZeroU64);
typed_id!(StaticTextureId, u64, NonZeroU64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectWorkingSpace {
    OutputEncodedSrgb,
    LinearSrgb,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectAlphaMode {
    Opaque,
    Preserve,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EffectOutsets {
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub left: u32,
}

impl EffectOutsets {
    pub const ZERO: Self = Self {
        top: 0,
        right: 0,
        bottom: 0,
        left: 0,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectFootprint {
    pub sample_radius_x: u32,
    pub sample_radius_y: u32,
    pub output_outsets: EffectOutsets,
}

impl EffectFootprint {
    pub const ZERO: Self = Self {
        sample_radius_x: 0,
        sample_radius_y: 0,
        output_outsets: EffectOutsets::ZERO,
    };

    pub const fn symmetric(radius: u32) -> Self {
        Self {
            sample_radius_x: radius,
            sample_radius_y: radius,
            output_outsets: EffectOutsets::ZERO,
        }
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            sample_radius_x: self.sample_radius_x.max(other.sample_radius_x),
            sample_radius_y: self.sample_radius_y.max(other.sample_radius_y),
            output_outsets: EffectOutsets {
                top: self.output_outsets.top.max(other.output_outsets.top),
                right: self.output_outsets.right.max(other.output_outsets.right),
                bottom: self.output_outsets.bottom.max(other.output_outsets.bottom),
                left: self.output_outsets.left.max(other.output_outsets.left),
            },
        }
    }

    pub fn compose(self, stage: Self) -> Result<Self, EffectValidationError> {
        Ok(Self {
            sample_radius_x: self
                .sample_radius_x
                .checked_add(stage.sample_radius_x)
                .ok_or(EffectValidationError::FootprintOverflow)?,
            sample_radius_y: self
                .sample_radius_y
                .checked_add(stage.sample_radius_y)
                .ok_or(EffectValidationError::FootprintOverflow)?,
            output_outsets: EffectOutsets {
                top: self
                    .output_outsets
                    .top
                    .checked_add(stage.output_outsets.top)
                    .ok_or(EffectValidationError::FootprintOverflow)?,
                right: self
                    .output_outsets
                    .right
                    .checked_add(stage.output_outsets.right)
                    .ok_or(EffectValidationError::FootprintOverflow)?,
                bottom: self
                    .output_outsets
                    .bottom
                    .checked_add(stage.output_outsets.bottom)
                    .ok_or(EffectValidationError::FootprintOverflow)?,
                left: self
                    .output_outsets
                    .left
                    .checked_add(stage.output_outsets.left)
                    .ok_or(EffectValidationError::FootprintOverflow)?,
            },
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectFrameDemand {
    OnDamage,
    Continuous,
    Manual,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectFailurePolicy {
    Passthrough,
    DisableInstance,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectProgram {
    pub id: EffectProgramId,
    pub nodes: Vec<EffectNode>,
    pub output: EffectNodeId,
    pub working_space: EffectWorkingSpace,
    pub alpha_mode: EffectAlphaMode,
    pub outsets: EffectOutsets,
    pub frame_demand: EffectFrameDemand,
    pub failure_policy: EffectFailurePolicy,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectNode {
    pub id: EffectNodeId,
    pub inputs: Vec<EffectNodeId>,
    pub kind: EffectNodeKind,
}

impl EffectNode {
    pub fn source(id: EffectNodeId, source: EffectSource) -> Self {
        Self {
            id,
            inputs: Vec::new(),
            kind: EffectNodeKind::Source(source),
        }
    }

    pub fn dual_kawase(id: EffectNodeId, input: EffectNodeId, spec: DualKawaseBlurSpec) -> Self {
        Self {
            id,
            inputs: vec![input],
            kind: EffectNodeKind::DualKawaseBlur(spec),
        }
    }

    pub fn tint(id: EffectNodeId, input: EffectNodeId, spec: TintSpec) -> Self {
        Self {
            id,
            inputs: vec![input],
            kind: EffectNodeKind::Tint(spec),
        }
    }

    pub fn color_matrix(
        id: EffectNodeId,
        input: EffectNodeId,
        spec: ColorMatrixSpec,
    ) -> Result<Self, EffectValidationError> {
        spec.validate()?;
        Ok(Self {
            id,
            inputs: vec![input],
            kind: EffectNodeKind::ColorMatrix(spec),
        })
    }

    pub fn noise(id: EffectNodeId, input: EffectNodeId, spec: NoiseSpec) -> Self {
        Self {
            id,
            inputs: vec![input],
            kind: EffectNodeKind::Noise(spec),
        }
    }

    pub fn custom_fragment(
        id: EffectNodeId,
        input: EffectNodeId,
        spec: CustomFragmentSpec,
    ) -> Result<Self, EffectValidationError> {
        spec.validate()?;
        Ok(Self {
            id,
            inputs: vec![input],
            kind: EffectNodeKind::CustomFragment(spec),
        })
    }

    pub fn blend(id: EffectNodeId, inputs: Vec<EffectNodeId>, spec: BlendSpec) -> Self {
        Self {
            id,
            inputs,
            kind: EffectNodeKind::Blend(spec),
        }
    }

    pub fn mask(id: EffectNodeId, input: EffectNodeId, spec: MaskSpec) -> Self {
        Self {
            id,
            inputs: vec![input],
            kind: EffectNodeKind::Mask(spec),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EffectNodeKind {
    Source(EffectSource),
    DualKawaseBlur(DualKawaseBlurSpec),
    ColorMatrix(ColorMatrixSpec),
    Tint(TintSpec),
    Noise(NoiseSpec),
    CustomFragment(CustomFragmentSpec),
    Blend(BlendSpec),
    Mask(MaskSpec),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectSource {
    Backdrop,
    TargetContent,
    StaticTexture(StaticTextureId),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DualKawaseBlurSpec {
    pub radius: f32,
    pub passes: u8,
    pub scale: f32,
}

impl DualKawaseBlurSpec {
    pub fn new(radius: f32, passes: u8, scale: f32) -> Result<Self, EffectValidationError> {
        if !radius.is_finite() || !scale.is_finite() {
            return Err(EffectValidationError::NonFiniteValue);
        }
        if !radius.is_sign_positive() || radius == 0.0 {
            return Err(EffectValidationError::InvalidBlurRadius);
        }
        if !(1..=MAX_EFFECT_BLUR_PASSES).contains(&passes) {
            return Err(EffectValidationError::InvalidBlurPassCount);
        }
        if !(0.0625..=1.0).contains(&scale) {
            return Err(EffectValidationError::InvalidScale);
        }
        Ok(Self {
            radius,
            passes,
            scale,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorMatrixSpec {
    pub matrix: [f32; 16],
    pub bias: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TintSpec {
    pub color: [f32; 4],
    pub amount: f32,
}

impl TintSpec {
    pub const WHITE: Self = Self {
        color: [1.0, 1.0, 1.0, 1.0],
        amount: 1.0,
    };

    pub fn new(color: [f32; 4], amount: f32) -> Result<Self, EffectValidationError> {
        if color.iter().all(|value| value.is_finite()) && amount.is_finite() {
            Ok(Self { color, amount })
        } else {
            Err(EffectValidationError::NonFiniteValue)
        }
    }
}

impl ColorMatrixSpec {
    fn validate(&self) -> Result<(), EffectValidationError> {
        if self
            .matrix
            .iter()
            .chain(self.bias.iter())
            .all(|v| v.is_finite())
        {
            Ok(())
        } else {
            Err(EffectValidationError::NonFiniteValue)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoiseKind {
    Hash,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoiseSpec {
    pub kind: NoiseKind,
    pub amount: f32,
}

impl NoiseSpec {
    pub fn new(kind: NoiseKind, amount: f32) -> Result<Self, EffectValidationError> {
        if amount.is_finite() {
            Ok(Self { kind, amount })
        } else {
            Err(EffectValidationError::NonFiniteValue)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlendMode {
    SourceOver,
    Add,
    Multiply,
    Screen,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendSpec {
    pub mode: BlendMode,
    pub opacity: f32,
}

impl BlendSpec {
    pub fn new(mode: BlendMode, opacity: f32) -> Result<Self, EffectValidationError> {
        if opacity.is_finite() && (0.0..=1.0).contains(&opacity) {
            Ok(Self { mode, opacity })
        } else {
            Err(EffectValidationError::NonFiniteValue)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaskMode {
    Alpha,
    InvertedAlpha,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaskSpec {
    pub mode: MaskMode,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CustomFragmentSpec {
    pub shader: ShaderModuleId,
    pub declared_footprint: EffectFootprint,
    pub uniforms: Vec<EffectUniformBinding>,
    pub auxiliary_inputs: Vec<EffectNodeId>,
}

impl CustomFragmentSpec {
    fn validate(&self) -> Result<(), EffectValidationError> {
        if self.uniforms.len() > MAX_EFFECT_UNIFORMS_PER_SHADER {
            return Err(EffectValidationError::TooManyUniforms);
        }
        if self.auxiliary_inputs.len() > MAX_EFFECT_AUX_TEXTURES_PER_SHADER {
            return Err(EffectValidationError::TooManyAuxTextures);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectUniformBinding {
    pub parameter: EffectParameterId,
    pub shader_name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectValidationError {
    InvalidId,
    MissingOutputNode,
    MissingInputNode(EffectNodeId),
    DuplicateNodeId(EffectNodeId),
    Cycle,
    InvalidArity { node: EffectNodeId },
    TooManyNodes,
    TooManyUniforms,
    TooManyAuxTextures,
    NonFiniteValue,
    InvalidBlurRadius,
    InvalidBlurPassCount,
    InvalidScale,
    FootprintOverflow,
}

impl std::fmt::Display for EffectValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for EffectValidationError {}
