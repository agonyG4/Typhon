use super::{DualKawaseBlurSpec, EffectFootprint, EffectNodeKind, EffectValidationError};

/// Returns the conservative physical source radius of the implemented
/// downsample/upsample chain.  Each level samples a two-pixel-wide Kawase
/// offset and doubles its source-space reach; the upsample chain mirrors the
/// same levels.  `scale` converts the processing-space reach back to source
/// pixels.
pub fn dual_kawase_sampling_radius(
    radius: f32,
    passes: u8,
    scale: f32,
) -> Result<u32, EffectValidationError> {
    let spec = DualKawaseBlurSpec::new(radius, passes, scale)?;
    let level_reach = (1u64 << spec.passes) - 1;
    let reach = f64::from(spec.radius) * (2.0 * level_reach as f64) / f64::from(spec.scale);
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
        assert_eq!(validated.aggregate_footprint, EffectFootprint::symmetric(8));
    }
}
