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
}
