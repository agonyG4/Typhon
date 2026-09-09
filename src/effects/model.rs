#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_program_model_is_renderer_independent() {
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

    #[test]
    fn premultiplied_mask_scales_rgb_and_alpha_without_nan() {
        let input = PremultipliedRgba::new(0.4, 0.2, 0.1, 0.5);
        assert_eq!(
            input.apply_mask(0.25, MaskMode::Alpha),
            PremultipliedRgba::new(0.1, 0.05, 0.025, 0.125)
        );
        assert_eq!(
            PremultipliedRgba::new(0.0, 0.0, 0.0, 0.0).apply_mask(1.0, MaskMode::InvertedAlpha),
            PremultipliedRgba::TRANSPARENT
        );
    }

    #[test]
    fn mask_inverted_is_the_same_coverage_operation_as_normal_mask() {
        let input = PremultipliedRgba::new(0.8, 0.4, 0.2, 0.8);
        for alpha in [0.0, 0.25, 0.5, 1.0] {
            let normal = input.apply_mask(alpha, MaskMode::Alpha);
            let inverted = input.apply_mask(1.0 - alpha, MaskMode::InvertedAlpha);
            assert_eq!(normal, inverted);
            assert!(
                [inverted.r, inverted.g, inverted.b, inverted.a]
                    .into_iter()
                    .all(f32::is_finite)
            );
        }
        assert_eq!(
            PremultipliedRgba::new(0.8, 0.8, 0.8, 1.0).apply_mask(0.25, MaskMode::Alpha),
            PremultipliedRgba::new(0.8, 0.8, 0.8, 1.0).apply_mask(0.75, MaskMode::InvertedAlpha)
        );
    }

    #[test]
    fn premultiplied_blend_modes_are_finite_and_source_over_is_exact() {
        let destination = PremultipliedRgba::new(0.2, 0.1, 0.05, 0.5);
        let source = PremultipliedRgba::new(0.4, 0.2, 0.1, 0.5);
        let result = destination.blend(source, BlendMode::SourceOver, 1.0);
        assert_eq!(result, PremultipliedRgba::new(0.5, 0.25, 0.125, 0.75));
        for mode in [BlendMode::Add, BlendMode::Multiply, BlendMode::Screen] {
            let result = destination.blend(source, mode, 0.75);
            assert!(
                [result.r, result.g, result.b, result.a]
                    .into_iter()
                    .all(f32::is_finite)
            );
        }
    }

    #[test]
    fn premultiplied_reference_sanitizes_builtin_extremes() {
        for alpha in [0.0, 0.25, 0.5, 1.0] {
            for mode in [BlendMode::Add, BlendMode::Multiply, BlendMode::Screen] {
                let result = PremultipliedRgba::new(2.0, -1.0, 4.0, alpha).blend(
                    PremultipliedRgba::new(3.0, -2.0, 5.0, alpha),
                    mode,
                    1.0,
                );
                assert!(
                    [result.r, result.g, result.b, result.a]
                        .into_iter()
                        .all(f32::is_finite)
                );
                assert!((0.0..=1.0).contains(&result.a));
                assert!(result.r >= 0.0 && result.r <= result.a);
                assert!(result.g >= 0.0 && result.g <= result.a);
                assert!(result.b >= 0.0 && result.b <= result.a);
            }
            let negative_bias = PremultipliedRgba::new(-4.0, 2.0, -1.0, alpha);
            assert_eq!(negative_bias.r, 0.0);
            assert!(negative_bias.g <= negative_bias.a);
            let noisy = PremultipliedRgba::new(4.0, -3.0, 2.0, alpha);
            assert!(noisy.r <= noisy.a && noisy.g >= 0.0 && noisy.b <= noisy.a);
        }
    }

    #[test]
    fn opaque_alpha_is_forced_only_at_the_explicit_boundary() {
        let input = PremultipliedRgba::new(0.1, 0.05, 0.02, 0.25);
        assert_eq!(input.with_alpha_mode(EffectAlphaMode::Preserve), input);
        assert_eq!(input.with_alpha_mode(EffectAlphaMode::Opaque).a, 1.0);
    }

    #[test]
    fn premultiplied_srgb_round_trip_preserves_saturated_straight_color() {
        for alpha in [0.0, 0.25, 0.5, 1.0] {
            let input = PremultipliedRgba::new(0.8 * alpha, 0.2 * alpha, 0.05 * alpha, alpha);
            let round_trip = input.decode_srgb().encode_srgb();
            if alpha == 0.0 {
                assert_eq!(round_trip, PremultipliedRgba::TRANSPARENT);
            } else {
                assert!((round_trip.r - input.r).abs() < 0.0001);
                assert!((round_trip.g - input.g).abs() < 0.0001);
                assert!((round_trip.b - input.b).abs() < 0.0001);
                assert_eq!(round_trip.a, alpha);
            }
        }
    }

    #[test]
    fn linear_kawase_reference_decodes_each_sample_before_averaging() {
        let samples = [
            PremultipliedRgba::new(0.0, 0.0, 0.0, 1.0),
            PremultipliedRgba::new(1.0, 1.0, 1.0, 1.0),
            PremultipliedRgba::new(0.0, 0.0, 0.0, 1.0),
            PremultipliedRgba::new(1.0, 1.0, 1.0, 1.0),
        ];
        let decoded = samples.map(PremultipliedRgba::decode_srgb);
        let average = PremultipliedRgba::new(
            decoded.iter().map(|sample| sample.r).sum::<f32>() * 0.25,
            decoded.iter().map(|sample| sample.g).sum::<f32>() * 0.25,
            decoded.iter().map(|sample| sample.b).sum::<f32>() * 0.25,
            decoded.iter().map(|sample| sample.a).sum::<f32>() * 0.25,
        );
        let encoded_average = PremultipliedRgba::new(0.5, 0.5, 0.5, 1.0).decode_srgb();
        assert!(average.r > encoded_average.r);
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

pub const BUILTIN_EFFECT_PROGRAM_ID: u64 = 1;
pub const INTERNAL_EFFECT_SHADER_MODULE_DOWNSAMPLE: u64 = 1001;
pub const INTERNAL_EFFECT_SHADER_MODULE_UPSAMPLE: u64 = 1002;
pub const INTERNAL_EFFECT_SHADER_MODULE_COPY: u64 = 1003;
pub const INTERNAL_EFFECT_SHADER_MODULE_COMPOSITE: u64 = 1004;
pub const INTERNAL_EFFECT_SHADER_MODULE_FRAGMENT: u64 = 1005;
pub const INTERNAL_EFFECT_SHADER_MODULE_MASK: u64 = 1006;
pub const INTERNAL_EFFECT_SHADER_MODULE_BLEND: u64 = 1007;
pub const INTERNAL_EFFECT_SHADER_MODULE_IDS: &[u64] = &[
    INTERNAL_EFFECT_SHADER_MODULE_DOWNSAMPLE,
    INTERNAL_EFFECT_SHADER_MODULE_UPSAMPLE,
    INTERNAL_EFFECT_SHADER_MODULE_COPY,
    INTERNAL_EFFECT_SHADER_MODULE_COMPOSITE,
    INTERNAL_EFFECT_SHADER_MODULE_FRAGMENT,
    INTERNAL_EFFECT_SHADER_MODULE_MASK,
    INTERNAL_EFFECT_SHADER_MODULE_BLEND,
];

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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
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
        let mut inputs = vec![input];
        inputs.extend(spec.auxiliary_inputs.iter().copied());
        Ok(Self {
            id,
            inputs,
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

impl ColorMatrixSpec {
    pub const IDENTITY: Self = Self {
        matrix: [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
        bias: [0.0; 4],
    };
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
    pub(crate) fn validate(&self) -> Result<(), EffectValidationError> {
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

/// A finite premultiplied RGBA value used by the renderer-independent effect
/// math contract.  All channels are clamped to the normalized color range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PremultipliedRgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl PremultipliedRgba {
    pub const TRANSPARENT: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        let a = finite_clamp(a);
        Self {
            r: finite_clamp(r).min(a),
            g: finite_clamp(g).min(a),
            b: finite_clamp(b).min(a),
            a,
        }
    }

    pub fn apply_mask(self, mask_alpha: f32, mode: MaskMode) -> Self {
        let coverage = match mode {
            MaskMode::Alpha => finite_clamp(mask_alpha),
            MaskMode::InvertedAlpha => finite_clamp(1.0 - mask_alpha),
        };
        Self::new(
            self.r * coverage,
            self.g * coverage,
            self.b * coverage,
            self.a * coverage,
        )
    }

    pub fn with_alpha_mode(self, mode: EffectAlphaMode) -> Self {
        match mode {
            EffectAlphaMode::Preserve => self,
            EffectAlphaMode::Opaque => Self::new(self.r, self.g, self.b, 1.0),
        }
    }

    /// Blend `source` over `destination` in premultiplied form.
    pub fn blend(self, source: Self, mode: BlendMode, opacity: f32) -> Self {
        let source_alpha = source.a * finite_clamp(opacity);
        let source = Self::new(
            source.r * finite_clamp(opacity),
            source.g * finite_clamp(opacity),
            source.b * finite_clamp(opacity),
            source_alpha,
        );
        let inverse_source_alpha = 1.0 - source.a;
        let alpha = source.a + self.a * inverse_source_alpha;
        let rgb = match mode {
            BlendMode::SourceOver => [
                source.r + self.r * inverse_source_alpha,
                source.g + self.g * inverse_source_alpha,
                source.b + self.b * inverse_source_alpha,
            ],
            BlendMode::Add => [self.r + source.r, self.g + source.g, self.b + source.b],
            BlendMode::Multiply | BlendMode::Screen => {
                let destination_straight = self.straight_rgb();
                let source_straight = source.straight_rgb();
                let blended = match mode {
                    BlendMode::Multiply => [
                        destination_straight[0] * source_straight[0],
                        destination_straight[1] * source_straight[1],
                        destination_straight[2] * source_straight[2],
                    ],
                    BlendMode::Screen => [
                        1.0 - (1.0 - destination_straight[0]) * (1.0 - source_straight[0]),
                        1.0 - (1.0 - destination_straight[1]) * (1.0 - source_straight[1]),
                        1.0 - (1.0 - destination_straight[2]) * (1.0 - source_straight[2]),
                    ],
                    _ => unreachable!(),
                };
                [
                    self.r * (1.0 - source.a)
                        + source.r * (1.0 - self.a)
                        + blended[0] * self.a * source.a,
                    self.g * (1.0 - source.a)
                        + source.g * (1.0 - self.a)
                        + blended[1] * self.a * source.a,
                    self.b * (1.0 - source.a)
                        + source.b * (1.0 - self.a)
                        + blended[2] * self.a * source.a,
                ]
            }
        };
        Self::new(rgb[0], rgb[1], rgb[2], alpha)
    }

    fn straight_rgb(self) -> [f32; 3] {
        if self.a <= f32::EPSILON {
            [0.0; 3]
        } else {
            [self.r / self.a, self.g / self.a, self.b / self.a]
        }
    }

    pub fn decode_srgb(self) -> Self {
        if self.a <= f32::EPSILON {
            return Self::TRANSPARENT;
        }
        let straight = self.straight_rgb();
        let linear = straight.map(srgb_decode);
        Self::new(
            linear[0] * self.a,
            linear[1] * self.a,
            linear[2] * self.a,
            self.a,
        )
    }

    pub fn encode_srgb(self) -> Self {
        if self.a <= f32::EPSILON {
            return Self::TRANSPARENT;
        }
        let straight = self.straight_rgb();
        let encoded = straight.map(srgb_encode);
        Self::new(
            encoded[0] * self.a,
            encoded[1] * self.a,
            encoded[2] * self.a,
            self.a,
        )
    }
}

pub fn srgb_decode(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

pub fn srgb_encode(value: f32) -> f32 {
    if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

fn finite_clamp(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CustomFragmentSpec {
    pub shader: ShaderModuleId,
    pub declared_footprint: EffectFootprint,
    pub uniforms: Vec<EffectUniformBinding>,
    pub auxiliary_inputs: Vec<EffectNodeId>,
}

impl CustomFragmentSpec {
    pub(crate) fn validate(&self) -> Result<(), EffectValidationError> {
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
    UnsupportedStaticTexture(StaticTextureId),
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
