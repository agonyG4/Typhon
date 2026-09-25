use super::{
    EffectiveMaterial, MaterialCapabilities, MaterialConfiguration, MaterialCurveV1,
    MaterialOverrides,
};

#[test]
fn default_material_capabilities_are_unavailable() {
    let unavailable = MaterialCapabilities {
        blur_override: false,
        saturation_override: false,
        noise_override: false,
    };
    assert_eq!(MaterialCapabilities::default(), unavailable);
    assert_eq!(MaterialCapabilities::unavailable(), unavailable);
    assert_eq!(
        MaterialCapabilities::full(),
        MaterialCapabilities {
            blur_override: true,
            saturation_override: true,
            noise_override: true,
        }
    );
}

#[test]
fn default_material_uses_astrea_midpoint() {
    let configuration = MaterialConfiguration::default();

    assert_eq!(configuration.version, 1);
    assert_eq!(configuration.position, 0.5);
    assert_eq!(configuration.overrides, MaterialOverrides::default());
}

#[test]
fn endpoints_are_valid_and_out_of_range_or_non_finite_positions_are_rejected() {
    for position in [0.0, 1.0] {
        let configuration = MaterialConfiguration {
            position,
            ..MaterialConfiguration::default()
        };
        assert!(configuration.validate().is_ok());
    }
    for position in [-0.01, 1.01, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let configuration = MaterialConfiguration {
            position,
            ..MaterialConfiguration::default()
        };
        assert!(configuration.validate().is_err());
    }
}

#[test]
fn every_material_override_must_be_finite_and_normalized() {
    let invalid_values = [-0.01, 1.01, f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
    for value in invalid_values {
        let mut configuration = MaterialConfiguration::default();
        configuration.overrides.blur = Some(value);
        assert!(configuration.validate().is_err(), "blur={value}");

        let mut configuration = MaterialConfiguration::default();
        configuration.overrides.saturation = Some(value);
        assert!(configuration.validate().is_err(), "saturation={value}");

        let mut configuration = MaterialConfiguration::default();
        configuration.overrides.noise = Some(value);
        assert!(configuration.validate().is_err(), "noise={value}");
    }
}

#[test]
fn version_one_is_required() {
    let configuration = MaterialConfiguration {
        version: 2,
        ..MaterialConfiguration::default()
    };
    assert!(configuration.validate().is_err());
}

#[test]
fn material_curve_is_deterministic_finite_bounded_and_blur_increases_toward_frosted() {
    let glass = MaterialCurveV1::resolve(0.0, &MaterialOverrides::default()).unwrap();
    let default = MaterialCurveV1::resolve(0.5, &MaterialOverrides::default()).unwrap();
    let frosted = MaterialCurveV1::resolve(1.0, &MaterialOverrides::default()).unwrap();

    assert_eq!(
        default,
        MaterialCurveV1::resolve(0.5, &MaterialOverrides::default()).unwrap()
    );
    for effective in [glass, default, frosted] {
        for value in [effective.blur, effective.saturation, effective.noise] {
            assert!(value.is_finite());
            assert!((0.0..=1.0).contains(&value));
        }
    }
    assert!(glass.blur < default.blur);
    assert!(default.blur < frosted.blur);
}

#[test]
fn overrides_replace_only_their_corresponding_curve_dimensions() {
    let overrides = MaterialOverrides {
        blur: Some(0.91),
        saturation: None,
        noise: Some(0.73),
    };
    let position_a = MaterialCurveV1::resolve(0.2, &overrides).unwrap();
    let position_b = MaterialCurveV1::resolve(0.8, &overrides).unwrap();
    let base_a = MaterialCurveV1::resolve(0.2, &MaterialOverrides::default()).unwrap();
    let base_b = MaterialCurveV1::resolve(0.8, &MaterialOverrides::default()).unwrap();

    assert_eq!(position_a.blur, 0.91);
    assert_eq!(position_b.blur, 0.91);
    assert_eq!(position_a.noise, 0.73);
    assert_eq!(position_b.noise, 0.73);
    assert_eq!(position_a.saturation, base_a.saturation);
    assert_eq!(position_b.saturation, base_b.saturation);
    assert_ne!(position_a.saturation, position_b.saturation);
}

#[test]
fn clearing_an_override_restores_that_dimension_to_the_curve() {
    let configuration = MaterialConfiguration {
        overrides: MaterialOverrides {
            blur: Some(0.0),
            saturation: Some(1.0),
            noise: Some(0.8),
        },
        ..MaterialConfiguration::default()
    };
    let overridden =
        MaterialCurveV1::resolve(configuration.position, &configuration.overrides).unwrap();
    let mut cleared = configuration.overrides.clone();
    cleared.blur = None;
    let effective = MaterialCurveV1::resolve(configuration.position, &cleared).unwrap();
    let base =
        MaterialCurveV1::resolve(configuration.position, &MaterialOverrides::default()).unwrap();

    assert_eq!(overridden.blur, 0.0);
    assert_eq!(effective.blur, base.blur);
    assert_eq!(effective.saturation, overridden.saturation);
    assert_eq!(effective.noise, overridden.noise);
}

#[test]
fn effective_material_is_a_typed_semantic_projection() {
    let effective: EffectiveMaterial =
        MaterialCurveV1::resolve(0.5, &MaterialOverrides::default()).unwrap();
    assert!((0.0..=1.0).contains(&effective.blur));
}

#[test]
fn canonical_program_uses_supported_stages_and_recomputes_blur_footprint() {
    use crate::effects::{EffectFrameDemand, EffectNodeKind, builtin_background_material_program};

    let glass = MaterialConfiguration {
        position: 0.0,
        ..MaterialConfiguration::default()
    }
    .effective()
    .unwrap();
    let frosted = MaterialConfiguration {
        position: 1.0,
        ..MaterialConfiguration::default()
    }
    .effective()
    .unwrap();
    let glass_program = builtin_background_material_program(glass);
    let frosted_program = builtin_background_material_program(frosted);
    assert!(
        frosted_program.aggregate_footprint.sample_radius_x
            > glass_program.aggregate_footprint.sample_radius_x
    );
    assert_eq!(
        glass_program.program.frame_demand,
        EffectFrameDemand::OnDamage
    );
    assert!(
        glass_program
            .program
            .nodes
            .iter()
            .any(|node| matches!(&node.kind, EffectNodeKind::ColorMatrix(_)))
    );
    assert!(
        glass_program
            .program
            .nodes
            .iter()
            .any(|node| matches!(&node.kind, EffectNodeKind::Noise(spec) if spec.amount > 0.0))
    );
}
