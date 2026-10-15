use super::{DualKawaseBlurSpec, EffectFootprint, EffectNodeKind, EffectValidationError};

/// Returns an aggregate source-space bound for the compiled Dual Kawase chain.
///
/// For `P` levels, the input texel scales of the downsample passes are bounded
/// by `1, 2/s, ..., 2^(P-1)/s`; the upsample passes use
/// `2/s, 4/s, ..., 2^P/s`. Both sums are at most
/// `3 * (2^P - 1) / s`. `scaled_dimension()` followed by `ceil_div()` can only
/// make a level texture larger than its ideal fractional size, so this also
/// bounds every odd and tiny domain.
///
/// Each Kawase sample moves by `radius` input texels. Reverse demand also
/// covers the sampler's linear-filtered texel cells: `sampled_input_axis_bounds()`
/// can reach two input texels beyond that offset, and converting the covered
/// physical input cells back to logical bounds adds at most one more input
/// texel. Converting a demanded logical output interval to physical coverage
/// adds one output texel. For every downsample after the first, that output
/// texel is at most two input texels; for an upsample it is at most one. The
/// first downsample is exceptional: its output texel can span `2/s` source
/// pixels, so account for that separately. Thus offset, linear-filter, input
/// conversion, and output-coverage support are bounded by
/// `(radius + 5) * 3 * (2^P - 1) / s + 2/s`.
///
/// The final logical Composite can add one output pixel and three final-input
/// texels (`3/s`); another output pixel covers conversion between sampled
/// pixel cells and the planner's integer damage rectangle. Captures start at
/// 1:1 source texel per logical pixel. The bound deliberately allows slack so
/// it remains valid for every domain size, including ceil-div and odd sizes.
pub fn dual_kawase_sampling_radius(
    radius: f32,
    passes: u8,
    scale: f32,
) -> Result<u32, EffectValidationError> {
    let spec = DualKawaseBlurSpec::new(radius, passes, scale)?;
    let level_reach = (1u64 << spec.passes) - 1;
    let level_sum = level_reach as f64;
    let inverse_scale = 1.0 / f64::from(spec.scale);
    let reach = 3.0 * level_sum * (f64::from(spec.radius) + 5.0) * inverse_scale
        + 2.0 * inverse_scale
        + 2.0
        + 3.0 * inverse_scale;
    if !reach.is_finite() || reach > f64::from(u32::MAX) {
        return Err(EffectValidationError::FootprintOverflow);
    }
    Ok(reach.ceil() as u32)
}

pub(crate) fn node_footprint(
    kind: &EffectNodeKind,
) -> Result<EffectFootprint, EffectValidationError> {
    match kind {
        EffectNodeKind::Source(_) => Ok(EffectFootprint::ZERO),
        EffectNodeKind::DualKawaseBlur(spec) => Ok(EffectFootprint::symmetric(
            dual_kawase_sampling_radius(spec.radius, spec.passes, spec.scale)?,
        )),
        EffectNodeKind::ColorMatrix(spec) => {
            spec.validate()?;
            Ok(EffectFootprint::ZERO)
        }
        EffectNodeKind::Tint(spec) => {
            if spec.color.iter().all(|value| value.is_finite()) && spec.amount.is_finite() {
                Ok(EffectFootprint::ZERO)
            } else {
                Err(EffectValidationError::NonFiniteValue)
            }
        }
        EffectNodeKind::Noise(spec) => {
            if spec.amount.is_finite() {
                Ok(EffectFootprint::ZERO)
            } else {
                Err(EffectValidationError::NonFiniteValue)
            }
        }
        EffectNodeKind::CustomFragment(spec) => {
            spec.validate()?;
            Ok(spec.declared_footprint)
        }
        EffectNodeKind::Blend(spec) => {
            if spec.opacity.is_finite() && (0.0..=1.0).contains(&spec.opacity) {
                Ok(EffectFootprint::ZERO)
            } else {
                Err(EffectValidationError::NonFiniteValue)
            }
        }
        EffectNodeKind::Mask(_) => Ok(EffectFootprint::ZERO),
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn dual_kawase_footprint_is_monotonic_for_radius() {
        let smaller = dual_kawase_sampling_radius(2.0, 2, 1.0).unwrap();
        let larger = dual_kawase_sampling_radius(4.0, 2, 1.0).unwrap();
        assert!(larger >= smaller);
    }

    #[test]
    fn dual_kawase_footprint_is_monotonic_for_passes() {
        let fewer = dual_kawase_sampling_radius(4.0, 1, 1.0).unwrap();
        let more = dual_kawase_sampling_radius(4.0, 2, 1.0).unwrap();
        assert!(more >= fewer);
    }

    #[test]
    fn custom_fragment_requires_a_declared_footprint() {
        let shader = ShaderModuleId::new(1).unwrap();
        let input = EffectNodeId::new(1).unwrap();
        let spec = CustomFragmentSpec {
            shader,
            declared_footprint: EffectFootprint::ZERO,
            uniforms: Vec::new(),
            auxiliary_inputs: Vec::new(),
        };
        let node = EffectNode::custom_fragment(EffectNodeId::new(2).unwrap(), input, spec);
        assert!(node.is_ok());
    }

    #[test]
    fn local_builtins_have_zero_stage_sampling_radius() {
        let color = EffectNodeKind::ColorMatrix(ColorMatrixSpec {
            matrix: [1.0; 16],
            bias: [0.0; 4],
        });
        let tint = EffectNodeKind::Tint(TintSpec::WHITE);
        let noise = EffectNodeKind::Noise(NoiseSpec::new(NoiseKind::Hash, 0.2).unwrap());
        assert_eq!(node_footprint(&color).unwrap(), EffectFootprint::ZERO);
        assert_eq!(node_footprint(&tint).unwrap(), EffectFootprint::ZERO);
        assert_eq!(node_footprint(&noise).unwrap(), EffectFootprint::ZERO);
    }

    #[test]
    fn mask_and_blend_preserve_input_dependency_footprints() {
        let source = EffectNodeId::new(1).unwrap();
        let blur = EffectNodeId::new(2).unwrap();
        let mask = EffectNodeId::new(3).unwrap();
        let blend = EffectNodeId::new(4).unwrap();
        let program = EffectProgram {
            id: EffectProgramId::new(1).unwrap(),
            nodes: vec![
                EffectNode::source(source, EffectSource::Backdrop),
                EffectNode::dual_kawase(
                    blur,
                    source,
                    DualKawaseBlurSpec::new(4.0, 1, 1.0).unwrap(),
                ),
                EffectNode::mask(
                    mask,
                    blur,
                    MaskSpec {
                        mode: MaskMode::Alpha,
                    },
                ),
                EffectNode::blend(
                    blend,
                    vec![mask, source],
                    BlendSpec::new(BlendMode::SourceOver, 1.0).unwrap(),
                ),
            ],
            output: blend,
            working_space: EffectWorkingSpace::LinearSrgb,
            alpha_mode: EffectAlphaMode::Preserve,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        };
        let validated = validate_effect_program(program).unwrap();
        assert_eq!(
            validated.aggregate_footprint,
            EffectFootprint::symmetric(dual_kawase_sampling_radius(4.0, 1, 1.0).unwrap())
        );
    }
}
