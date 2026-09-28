use oblivion_one::control::ControlCommand;
use oblivion_one::effects::{
    CustomFragmentSpec, EffectAlphaMode, EffectFailurePolicy, EffectFootprint, EffectFrameDemand,
    EffectNode, EffectNodeId, EffectOutsets, EffectParameterDefinition, EffectParameterId,
    EffectParameterImpact, EffectParameterRange, EffectParameterSpec, EffectParameterType,
    EffectProgram, EffectProgramId, EffectRegistry, EffectRegistryGeneration, EffectSource,
    EffectUniformBinding, EffectUniformValue, EffectValidationError, EffectWorkingSpace,
    MAX_EFFECT_PARAMETERS, MAX_EFFECT_PROGRAMS, RegisteredEffect, ShaderModuleId,
    TrustedShaderAsset, validate_effect_program, validate_parameter_value,
};
use oblivion_one::material_program::{
    MAX_MATERIAL_PROGRAM_PARAMETER_DOCUMENT_BYTES, MaterialProgramCatalogSnapshot,
    MaterialProgramNameArguments, MaterialProgramParameterConfiguration,
    MaterialProgramParameterConfigurationStore, MaterialProgramParameterControlState,
    MaterialProgramParameterPersistenceDocument, MaterialProgramParameterProgramIntent,
    MaterialProgramParameterRange, MaterialProgramParameterType, MaterialProgramParameterValue,
    MaterialProgramStateSnapshot,
};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

fn parameter_spec(
    ty: EffectParameterType,
    range: Option<EffectParameterRange>,
) -> EffectParameterSpec {
    EffectParameterSpec {
        id: EffectParameterId::new(1).unwrap(),
        name: "intensity".to_owned(),
        ty,
        range,
        impact: EffectParameterImpact::UniformOnly,
    }
}

fn effect_with_parameters(
    parameters: Vec<(
        &'static str,
        EffectParameterType,
        Option<EffectParameterRange>,
        EffectUniformValue,
    )>,
) -> RegisteredEffect {
    effect_with_named_parameters(
        parameters
            .into_iter()
            .map(|(name, ty, range, default)| (name.to_owned(), ty, range, default))
            .collect(),
    )
}

fn effect_with_named_parameters(
    parameters: Vec<(
        String,
        EffectParameterType,
        Option<EffectParameterRange>,
        EffectUniformValue,
    )>,
) -> RegisteredEffect {
    let node = EffectNode::source(EffectNodeId::new(1).unwrap(), EffectSource::Backdrop);
    let program = validate_effect_program(EffectProgram {
        id: EffectProgramId::new(500).unwrap(),
        output: node.id,
        nodes: vec![node],
        working_space: EffectWorkingSpace::LinearSrgb,
        alpha_mode: EffectAlphaMode::Opaque,
        outsets: EffectOutsets::ZERO,
        frame_demand: EffectFrameDemand::OnDamage,
        failure_policy: EffectFailurePolicy::Passthrough,
    })
    .unwrap();
    let parameters = parameters
        .into_iter()
        .enumerate()
        .map(|(index, (name, ty, range, default))| {
            let id = EffectParameterId::new(u16::try_from(index + 1).unwrap()).unwrap();
            (
                name.to_owned(),
                EffectParameterDefinition {
                    spec: EffectParameterSpec {
                        id,
                        name: name.clone(),
                        ty,
                        range,
                        impact: EffectParameterImpact::UniformOnly,
                    },
                    default,
                },
            )
        })
        .collect();
    RegisteredEffect {
        name: "glass.liquid".to_owned(),
        program,
        parameters,
    }
}

fn renumber_effect_ids(
    effect: &RegisteredEffect,
    program_id: u64,
    parameter_id: u16,
) -> RegisteredEffect {
    let mut effect = effect.clone();
    effect.program.program.id = EffectProgramId::new(program_id).unwrap();
    for (offset, parameter) in effect.parameters.values_mut().enumerate() {
        let parameter_id = parameter_id + u16::try_from(offset).unwrap();
        parameter.spec.id = EffectParameterId::new(parameter_id).unwrap();
    }
    effect
}

fn effect_with_shader(
    program_id: u64,
    parameter_id: u16,
    module_id: u64,
    shader_node_id: u16,
    relative_path: &str,
    source: &str,
) -> (RegisteredEffect, TrustedShaderAsset) {
    let parameter_id = EffectParameterId::new(parameter_id).unwrap();
    let module = ShaderModuleId::new(module_id).unwrap();
    let uniform = EffectUniformBinding {
        parameter: parameter_id,
        shader_name: "u_intensity".to_owned(),
    };
    let backdrop = EffectNode::source(EffectNodeId::new(1).unwrap(), EffectSource::Backdrop);
    let shader_node = EffectNode::custom_fragment(
        EffectNodeId::new(shader_node_id).unwrap(),
        backdrop.id,
        CustomFragmentSpec {
            shader: module,
            declared_footprint: EffectFootprint {
                sample_radius_x: 0,
                sample_radius_y: 0,
                output_outsets: EffectOutsets::ZERO,
            },
            uniforms: vec![uniform.clone()],
            auxiliary_inputs: Vec::new(),
        },
    )
    .unwrap();
    let program = validate_effect_program(EffectProgram {
        id: EffectProgramId::new(program_id).unwrap(),
        output: shader_node.id,
        nodes: vec![backdrop, shader_node],
        working_space: EffectWorkingSpace::LinearSrgb,
        alpha_mode: EffectAlphaMode::Opaque,
        outsets: EffectOutsets::ZERO,
        frame_demand: EffectFrameDemand::OnDamage,
        failure_policy: EffectFailurePolicy::Passthrough,
    })
    .unwrap();
    let effect = RegisteredEffect {
        name: "glass.liquid".to_owned(),
        program,
        parameters: BTreeMap::from([(
            "intensity".to_owned(),
            EffectParameterDefinition {
                spec: EffectParameterSpec {
                    id: parameter_id,
                    name: "intensity".to_owned(),
                    ty: EffectParameterType::Float,
                    range: Some(EffectParameterRange::Float { min: 0.0, max: 1.0 }),
                    impact: EffectParameterImpact::UniformOnly,
                },
                default: EffectUniformValue::Float(0.65),
            },
        )]),
    };
    let shader = TrustedShaderAsset {
        module,
        relative_path: PathBuf::from(relative_path),
        source: source.to_owned(),
        uniforms: vec![uniform],
    };
    (effect, shader)
}

fn registry_generation(
    generation: u64,
    effects: Vec<RegisteredEffect>,
) -> Arc<EffectRegistryGeneration> {
    let builtin = oblivion_one::effects::render_graph::builtin_background_blur_program();
    let builtin_name = oblivion_one::effects::BUILTIN_BACKGROUND_BLUR_NAME.to_owned();
    let mut registry = EffectRegistry::empty();
    registry.insert(builtin.clone()).unwrap();
    let mut registered = BTreeMap::from([(
        builtin_name.clone(),
        RegisteredEffect {
            name: builtin_name,
            program: builtin,
            parameters: BTreeMap::new(),
        },
    )]);
    for effect in effects {
        registry.insert(effect.program.clone()).unwrap();
        registered.insert(effect.name.clone(), effect);
    }
    Arc::new(EffectRegistryGeneration {
        generation,
        registry,
        effects: registered,
        shaders: BTreeMap::new(),
    })
}

fn registry_generation_with_shader(
    generation: u64,
    effect: RegisteredEffect,
    shader: TrustedShaderAsset,
) -> Arc<EffectRegistryGeneration> {
    let mut generation = registry_generation(generation, vec![effect]);
    Arc::get_mut(&mut generation)
        .unwrap()
        .shaders
        .insert(shader.module, shader);
    generation
}

fn effect_manifest(effect: &RegisteredEffect) -> oblivion_one::effects::EffectManifest {
    oblivion_one::effects::EffectManifest {
        version: 1,
        effects: BTreeMap::from([(
            effect.name.clone(),
            oblivion_one::effects::EffectDefinition {
                name: effect.name.clone(),
                program: effect.program.program.clone(),
                parameters: effect.parameters.clone(),
                shader_assets: Vec::new(),
            },
        )]),
    }
}

fn temporary_config_home() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "typhon-material-parameters-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn parameter_configuration(
    effect: &RegisteredEffect,
    overrides: BTreeMap<String, MaterialProgramParameterValue>,
) -> MaterialProgramParameterConfiguration {
    MaterialProgramParameterConfiguration {
        version: 1,
        program: effect.name.clone(),
        schema_signature: effect.parameter_schema_signature(),
        overrides,
    }
}

#[test]
fn public_parameter_schema_signature_ignores_internal_program_ids() {
    let effect = effect_with_parameters(vec![(
        "intensity",
        EffectParameterType::Float,
        Some(EffectParameterRange::Float { min: 0.0, max: 1.0 }),
        EffectUniformValue::Float(0.65),
    )]);
    let mut renumbered = effect.clone();
    renumbered.program.program.id = EffectProgramId::new(501).unwrap();

    assert_eq!(
        effect.parameter_schema_signature(),
        renumbered.parameter_schema_signature()
    );
    assert_ne!(effect.schema_signature(), renumbered.schema_signature());
}

#[test]
fn public_parameter_schema_signature_ignores_internal_parameter_ids() {
    let effect = effect_with_parameters(vec![(
        "intensity",
        EffectParameterType::Float,
        Some(EffectParameterRange::Float { min: 0.0, max: 1.0 }),
        EffectUniformValue::Float(0.65),
    )]);
    let mut renumbered = effect.clone();
    renumbered.parameters.get_mut("intensity").unwrap().spec.id =
        EffectParameterId::new(7).unwrap();

    assert_eq!(
        effect.parameter_schema_signature(),
        renumbered.parameter_schema_signature()
    );
    assert_ne!(effect.schema_signature(), renumbered.schema_signature());
}

#[test]
fn public_parameter_schema_signature_is_independent_of_parameter_insertion_order() {
    let first = effect_with_parameters(vec![
        (
            "intensity",
            EffectParameterType::Float,
            Some(EffectParameterRange::Float { min: 0.0, max: 1.0 }),
            EffectUniformValue::Float(0.65),
        ),
        (
            "count",
            EffectParameterType::Int,
            Some(EffectParameterRange::Int { min: 1, max: 9 }),
            EffectUniformValue::Int(3),
        ),
    ]);
    let reordered = effect_with_parameters(vec![
        (
            "count",
            EffectParameterType::Int,
            Some(EffectParameterRange::Int { min: 1, max: 9 }),
            EffectUniformValue::Int(3),
        ),
        (
            "intensity",
            EffectParameterType::Float,
            Some(EffectParameterRange::Float { min: 0.0, max: 1.0 }),
            EffectUniformValue::Float(0.65),
        ),
    ]);

    assert_eq!(
        first.parameter_schema_signature(),
        reordered.parameter_schema_signature()
    );
    assert_ne!(first.schema_signature(), reordered.schema_signature());
}

#[test]
fn public_parameter_schema_signature_ignores_shader_source_and_path_changes() {
    let (effect_a, shader_a) = effect_with_shader(
        500,
        1,
        900,
        2,
        "shaders/glass.wgsl",
        "fn shade() { return 0.25; }",
    );
    let (effect_b, shader_b) = effect_with_shader(
        500,
        1,
        900,
        2,
        "shaders/glass-rewritten.wgsl",
        "fn shade() { return 0.75; }",
    );
    let generation_a = registry_generation_with_shader(71, effect_a, shader_a);
    let generation_b = registry_generation_with_shader(72, effect_b, shader_b);
    let registered_a = &generation_a.effects["glass.liquid"];
    let registered_b = &generation_b.effects["glass.liquid"];

    assert_ne!(
        generation_a.shaders.values().next().unwrap().source,
        generation_b.shaders.values().next().unwrap().source
    );
    assert_ne!(
        generation_a.shaders.values().next().unwrap().relative_path,
        generation_b.shaders.values().next().unwrap().relative_path
    );
    assert_eq!(
        registered_a.parameter_schema_signature(),
        registered_b.parameter_schema_signature()
    );
}

#[test]
fn public_parameter_schema_signature_ignores_graph_and_runtime_identity_churn() {
    let (effect_a, shader_a) =
        effect_with_shader(500, 1, 900, 2, "shaders/glass.wgsl", "fn shade() {} ");
    let (effect_b, shader_b) =
        effect_with_shader(501, 7, 901, 3, "shaders/rebuilt.wgsl", "fn shade() { } ");
    let generation_a = registry_generation_with_shader(73, effect_a.clone(), shader_a);
    let generation_b = registry_generation_with_shader(74, effect_b.clone(), shader_b);

    assert_ne!(effect_a.program.program.id, effect_b.program.program.id);
    assert_ne!(
        effect_a.parameters["intensity"].spec.id,
        effect_b.parameters["intensity"].spec.id
    );
    assert_ne!(
        effect_a.program.program.output,
        effect_b.program.program.output
    );
    assert_ne!(
        generation_a.shaders.keys().next().unwrap(),
        generation_b.shaders.keys().next().unwrap()
    );
    assert_ne!(generation_a.generation, generation_b.generation);
    assert_ne!(effect_a.schema_signature(), effect_b.schema_signature());
    assert_eq!(
        effect_a.parameter_schema_signature(),
        effect_b.parameter_schema_signature()
    );

    let mut different_frame_demand = effect_a.clone();
    different_frame_demand.program.program.frame_demand = EffectFrameDemand::Continuous;
    assert_ne!(
        effect_a.schema_signature(),
        different_frame_demand.schema_signature()
    );
    assert_eq!(
        effect_a.parameter_schema_signature(),
        different_frame_demand.parameter_schema_signature()
    );
}

#[test]
fn public_parameter_schema_signature_changes_for_semantic_parameter_fields() {
    let effect = effect_with_parameters(vec![(
        "intensity",
        EffectParameterType::Float,
        Some(EffectParameterRange::Float { min: 0.0, max: 1.0 }),
        EffectUniformValue::Float(0.65),
    )]);
    let baseline = effect.parameter_schema_signature();

    let mut renamed = effect.clone();
    let mut parameter = renamed.parameters.remove("intensity").unwrap();
    parameter.spec.name = "strength".to_owned();
    renamed.parameters.insert("strength".to_owned(), parameter);
    assert_ne!(renamed.parameter_schema_signature(), baseline);

    let mut changed_type = effect.clone();
    changed_type
        .parameters
        .get_mut("intensity")
        .unwrap()
        .spec
        .ty = EffectParameterType::Int;
    assert_ne!(changed_type.parameter_schema_signature(), baseline);

    let mut changed_range = effect.clone();
    changed_range
        .parameters
        .get_mut("intensity")
        .unwrap()
        .spec
        .range = Some(EffectParameterRange::Float { min: 0.0, max: 0.9 });
    assert_ne!(changed_range.parameter_schema_signature(), baseline);

    let mut changed_default = effect.clone();
    changed_default
        .parameters
        .get_mut("intensity")
        .unwrap()
        .default = EffectUniformValue::Float(0.7);
    assert_ne!(changed_default.parameter_schema_signature(), baseline);

    let mut changed_impact = effect.clone();
    changed_impact
        .parameters
        .get_mut("intensity")
        .unwrap()
        .spec
        .impact = EffectParameterImpact::Footprint;
    assert_ne!(changed_impact.parameter_schema_signature(), baseline);

    let mut renamed_program = effect;
    renamed_program.name = "glass.renamed".to_owned();
    assert_ne!(renamed_program.parameter_schema_signature(), baseline);
}

#[test]
fn material_program_catalog_exposes_the_stable_parameter_schema_signature() {
    let effect = effect_with_parameters(vec![(
        "intensity",
        EffectParameterType::Float,
        Some(EffectParameterRange::Float { min: 0.0, max: 1.0 }),
        EffectUniformValue::Float(0.65),
    )]);
    let renumbered = renumber_effect_ids(&effect, 501, 7);
    let generation_a = registry_generation(75, vec![effect.clone()]);
    let generation_b = registry_generation(76, vec![renumbered.clone()]);
    let catalog_a = MaterialProgramCatalogSnapshot::from_registry_generation(&generation_a, true);
    let catalog_b = MaterialProgramCatalogSnapshot::from_registry_generation(&generation_b, true);
    let entry_a = catalog_a
        .programs
        .iter()
        .find(|entry| entry.name == "glass.liquid")
        .unwrap();
    let entry_b = catalog_b
        .programs
        .iter()
        .find(|entry| entry.name == "glass.liquid")
        .unwrap();

    assert_eq!(
        entry_a.schema_signature,
        effect.parameter_schema_signature()
    );
    assert_eq!(
        entry_b.schema_signature,
        renumbered.parameter_schema_signature()
    );
    assert_eq!(entry_a.schema_signature, entry_b.schema_signature);
}

#[test]
fn shared_parameter_validation_checks_type_finiteness_and_declared_range() {
    let spec = parameter_spec(
        EffectParameterType::Float,
        Some(EffectParameterRange::Float { min: 0.0, max: 1.0 }),
    );

    assert!(validate_parameter_value(&spec, EffectUniformValue::Float(0.72)).is_ok());
    assert!(validate_parameter_value(&spec, EffectUniformValue::Float(1.01)).is_err());
    assert!(validate_parameter_value(&spec, EffectUniformValue::Float(f32::NAN)).is_err());
    assert!(validate_parameter_value(&spec, EffectUniformValue::Int(1)).is_err());

    let unbounded = parameter_spec(EffectParameterType::Float, None);
    assert!(validate_parameter_value(&unbounded, EffectUniformValue::Float(-900.0)).is_ok());
    assert_eq!(
        validate_parameter_value(&spec, EffectUniformValue::Float(1.01)),
        Err(EffectValidationError::InvalidParameterValue)
    );
}

#[test]
fn shared_parameter_validation_checks_integer_and_component_ranges() {
    let integer = parameter_spec(
        EffectParameterType::Int,
        Some(EffectParameterRange::Int { min: -2, max: 7 }),
    );
    assert!(validate_parameter_value(&integer, EffectUniformValue::Int(7)).is_ok());
    assert!(validate_parameter_value(&integer, EffectUniformValue::Int(8)).is_err());

    let vectors = [
        (
            EffectParameterType::Vec2,
            EffectParameterRange::FloatComponents {
                min: [0.1, 0.2, 0.0, 0.0],
                max: [0.3, 0.4, 0.0, 0.0],
                components: 2,
            },
            EffectUniformValue::Vec2([0.2, 0.3]),
            EffectUniformValue::Vec2([0.2, 0.5]),
        ),
        (
            EffectParameterType::Vec3,
            EffectParameterRange::FloatComponents {
                min: [0.1, 0.2, 0.3, 0.0],
                max: [0.3, 0.4, 0.5, 0.0],
                components: 3,
            },
            EffectUniformValue::Vec3([0.2, 0.3, 0.4]),
            EffectUniformValue::Vec3([0.2, 0.3, 0.6]),
        ),
        (
            EffectParameterType::Vec4,
            EffectParameterRange::FloatComponents {
                min: [0.1, 0.2, 0.3, 0.4],
                max: [0.3, 0.4, 0.5, 0.6],
                components: 4,
            },
            EffectUniformValue::Vec4([0.2, 0.3, 0.4, 0.5]),
            EffectUniformValue::Vec4([0.2, 0.3, 0.4, 0.7]),
        ),
    ];

    for (ty, range, accepted, rejected) in vectors {
        let spec = parameter_spec(ty, Some(range));
        assert!(validate_parameter_value(&spec, accepted).is_ok());
        assert!(validate_parameter_value(&spec, rejected).is_err());
    }
}

#[test]
fn material_program_parameter_values_are_strict_and_checked_for_f32_conversion() {
    let value = MaterialProgramParameterValue::Float(0.72);
    assert_eq!(
        serde_json::to_value(value).unwrap(),
        serde_json::json!({"type": "float", "value": 0.72})
    );
    assert!(
        serde_json::from_value::<MaterialProgramParameterValue>(serde_json::json!({
            "type": "vec2",
            "value": [0.2, 0.8],
            "parameterId": 7
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<MaterialProgramParameterValue>(serde_json::json!({
            "type": "float",
            "value": "0.72"
        }))
        .is_err()
    );
    assert!(value.to_effect_uniform_value().is_ok());
    assert!(
        MaterialProgramParameterValue::Float(f64::NAN)
            .to_effect_uniform_value()
            .is_err()
    );
    assert!(
        MaterialProgramParameterValue::Float(f64::MAX)
            .to_effect_uniform_value()
            .is_err()
    );
    assert!(
        MaterialProgramParameterValue::Float(
            f64::from(f32::MIN_POSITIVE) * f64::from(f32::EPSILON) * 0.1
        )
        .to_effect_uniform_value()
        .is_err()
    );
    assert!(
        serde_json::from_value::<MaterialProgramParameterRange>(serde_json::json!({
            "type": "float_components",
            "components": 2,
            "min": [0.0],
            "max": [1.0, 1.0]
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<MaterialProgramParameterRange>(serde_json::json!({
            "type": "float",
            "min": 1.0,
            "max": 0.0,
            "rendererId": 7
        }))
        .is_err()
    );
    assert!(
        serde_json::to_value(MaterialProgramParameterRange::Float {
            min: f64::NAN,
            max: 1.0,
        })
        .is_err()
    );
}

#[test]
fn description_sorts_typed_parameters_and_applies_matching_persistent_overrides() {
    let effect = effect_with_parameters(vec![
        (
            "unbounded",
            EffectParameterType::Float,
            None,
            EffectUniformValue::Float(-5.0),
        ),
        (
            "tint",
            EffectParameterType::Vec4,
            Some(EffectParameterRange::FloatComponents {
                min: [0.0, 0.0, 0.0, 0.0],
                max: [1.0, 1.0, 1.0, 1.0],
                components: 4,
            }),
            EffectUniformValue::Vec4([0.1, 0.2, 0.3, 0.4]),
        ),
        (
            "offset",
            EffectParameterType::Vec2,
            Some(EffectParameterRange::FloatComponents {
                min: [0.0, 0.0, 0.0, 0.0],
                max: [1.0, 1.0, 0.0, 0.0],
                components: 2,
            }),
            EffectUniformValue::Vec2([0.25, 0.75]),
        ),
        (
            "count",
            EffectParameterType::Int,
            Some(EffectParameterRange::Int { min: 1, max: 5 }),
            EffectUniformValue::Int(3),
        ),
        (
            "color",
            EffectParameterType::Vec3,
            Some(EffectParameterRange::FloatComponents {
                min: [0.0, 0.0, 0.0, 0.0],
                max: [1.0, 1.0, 1.0, 0.0],
                components: 3,
            }),
            EffectUniformValue::Vec3([0.2, 0.4, 0.6]),
        ),
        (
            "alpha",
            EffectParameterType::Float,
            Some(EffectParameterRange::Float { min: 0.0, max: 1.0 }),
            EffectUniformValue::Float(0.65),
        ),
    ]);
    let generation = registry_generation(13, vec![effect.clone()]);
    let home = temporary_config_home();
    let store = MaterialProgramParameterConfigurationStore::new(home.clone()).unwrap();
    let mut state = MaterialProgramParameterControlState::from_store(store);
    let update = state
        .set_configuration(
            parameter_configuration(
                &effect,
                BTreeMap::from([(
                    "alpha".to_owned(),
                    MaterialProgramParameterValue::Float(0.82),
                )]),
            ),
            &generation,
        )
        .unwrap();
    let description = update.snapshot.description.unwrap();

    assert_eq!(
        description
            .parameters
            .iter()
            .map(|parameter| parameter.name.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "color", "count", "offset", "tint", "unbounded"]
    );
    let alpha = &description.parameters[0];
    assert_eq!(alpha.parameter_type, MaterialProgramParameterType::Float);
    assert_eq!(
        alpha.range,
        Some(MaterialProgramParameterRange::Float { min: 0.0, max: 1.0 })
    );
    assert_eq!(alpha.default, MaterialProgramParameterValue::Float(0.65));
    assert_eq!(alpha.effective, MaterialProgramParameterValue::Float(0.82));
    assert!(alpha.overridden);
    assert_eq!(
        description.parameters[1].parameter_type,
        MaterialProgramParameterType::Vec3
    );
    assert_eq!(
        description.parameters[1].range,
        Some(MaterialProgramParameterRange::FloatComponents {
            components: 3,
            min: vec![0.0, 0.0, 0.0],
            max: vec![1.0, 1.0, 1.0],
        })
    );
    assert_eq!(
        description.parameters[1].default,
        MaterialProgramParameterValue::Vec3([0.2, 0.4, 0.6])
    );
    assert_eq!(
        description.parameters[1].effective,
        MaterialProgramParameterValue::Vec3([0.2, 0.4, 0.6])
    );
    assert_eq!(
        description.parameters[2].parameter_type,
        MaterialProgramParameterType::Int
    );
    assert_eq!(
        description.parameters[2].range,
        Some(MaterialProgramParameterRange::Int { min: 1, max: 5 })
    );
    assert_eq!(
        description.parameters[2].default,
        MaterialProgramParameterValue::Int(3)
    );
    assert_eq!(
        description.parameters[2].effective,
        MaterialProgramParameterValue::Int(3)
    );
    assert_eq!(
        description.parameters[3].parameter_type,
        MaterialProgramParameterType::Vec2
    );
    assert_eq!(
        description.parameters[3].range,
        Some(MaterialProgramParameterRange::FloatComponents {
            components: 2,
            min: vec![0.0, 0.0],
            max: vec![1.0, 1.0],
        })
    );
    assert_eq!(
        description.parameters[3].default,
        MaterialProgramParameterValue::Vec2([0.25, 0.75])
    );
    assert_eq!(
        description.parameters[3].effective,
        MaterialProgramParameterValue::Vec2([0.25, 0.75])
    );
    assert_eq!(
        description.parameters[4].parameter_type,
        MaterialProgramParameterType::Vec4
    );
    assert_eq!(
        description.parameters[4].range,
        Some(MaterialProgramParameterRange::FloatComponents {
            components: 4,
            min: vec![0.0, 0.0, 0.0, 0.0],
            max: vec![1.0, 1.0, 1.0, 1.0],
        })
    );
    assert_eq!(
        description.parameters[4].default,
        MaterialProgramParameterValue::Vec4([0.1, 0.2, 0.3, 0.4])
    );
    assert_eq!(
        description.parameters[4].effective,
        MaterialProgramParameterValue::Vec4([0.1, 0.2, 0.3, 0.4])
    );
    assert_eq!(description.parameters[5].range, None);
    assert!(!description.parameters[5].overridden);

    let encoded = serde_json::to_string(&description).unwrap();
    for forbidden in [
        "parameterId",
        "programId",
        "shaderModuleId",
        "shaderPath",
        "glsl",
        "EffectParameterImpact",
    ] {
        assert!(
            !encoded.contains(forbidden),
            "description leaked {forbidden}"
        );
    }
    let _ = fs::remove_dir_all(home);
}

#[test]
fn built_in_background_blur_has_a_zero_parameter_description() {
    let generation = registry_generation(14, Vec::new());
    let home = temporary_config_home();
    let store = MaterialProgramParameterConfigurationStore::new(home.clone()).unwrap();
    let state = MaterialProgramParameterControlState::from_store(store);

    let description = state
        .describe(
            oblivion_one::effects::BUILTIN_BACKGROUND_BLUR_NAME,
            &generation,
        )
        .unwrap();

    assert_eq!(description.parameters, Vec::new());
    assert_eq!(
        description.origin,
        oblivion_one::material_program::MaterialProgramOrigin::System
    );
    let _ = fs::remove_dir_all(home);
}

#[test]
fn atomic_state_snapshot_derives_catalog_and_selection_from_one_generation() {
    let generation = registry_generation(15, vec![effect_with_parameters(Vec::new())]);
    let selection_state = oblivion_one::material_program::MaterialProgramControlState::default();

    let snapshot = MaterialProgramStateSnapshot::from_registry_generation(
        &generation,
        &selection_state,
        false,
    );
    assert_eq!(snapshot.catalog.registry_generation, 15);
    assert_eq!(snapshot.selection.registry_generation, 15);
    assert_eq!(
        snapshot.catalog.rendering_available,
        snapshot.selection.rendering_available
    );
    assert_eq!(snapshot.selection.rendering_available, false);
}

#[test]
fn new_material_program_commands_have_canonical_names_and_strict_description_args() {
    for (name, command) in [
        (
            "material.program.state.get",
            ControlCommand::MaterialProgramStateGet,
        ),
        (
            "material.program.describe",
            ControlCommand::MaterialProgramDescribe,
        ),
        (
            "material.program.parameters.set",
            ControlCommand::MaterialProgramParametersSet,
        ),
    ] {
        assert_eq!(ControlCommand::parse(name), Some(command));
        assert_eq!(command.as_str(), name);
    }
    assert!(
        serde_json::from_value::<MaterialProgramNameArguments>(serde_json::json!({
            "name": "glass.liquid",
            "shaderPath": "/tmp/effect.frag"
        }))
        .is_err()
    );
    assert!(serde_json::from_value::<MaterialProgramNameArguments>(serde_json::json!({})).is_err());
    assert!(
        serde_json::from_value::<MaterialProgramParameterConfiguration>(serde_json::json!({
            "version": 1,
            "program": "glass.liquid",
            "schemaSignature": 4,
            "overrides": {},
            "effectProgramId": 18
        }))
        .is_err()
    );
}

#[test]
fn changed_parameter_configs_validate_against_the_current_schema_without_partial_writes() {
    use oblivion_one::material_program::MaterialProgramParameterMutationError as MutationError;

    let effect = effect_with_parameters(vec![
        (
            "intensity",
            EffectParameterType::Float,
            Some(EffectParameterRange::Float { min: 0.0, max: 1.0 }),
            EffectUniformValue::Float(0.65),
        ),
        (
            "count",
            EffectParameterType::Int,
            Some(EffectParameterRange::Int { min: 1, max: 9 }),
            EffectUniformValue::Int(3),
        ),
    ]);
    let generation = registry_generation(31, vec![effect.clone()]);
    let home = temporary_config_home();
    let store = MaterialProgramParameterConfigurationStore::new(home.clone()).unwrap();
    let mut state = MaterialProgramParameterControlState::from_store(store.clone());
    let accepted = parameter_configuration(
        &effect,
        BTreeMap::from([(
            "intensity".to_owned(),
            MaterialProgramParameterValue::Float(0.82),
        )]),
    );
    state
        .set_configuration(accepted.clone(), &generation)
        .unwrap();
    let persistent_before = store.read().unwrap();
    let bytes_before =
        fs::read(home.join("AstreaOS/typhon/material-program-parameters.json")).unwrap();
    let parameter_generation_before = state.generation();
    let registry_before = generation.clone();

    let mut type_mismatch = accepted.clone();
    type_mismatch.overrides.insert(
        "intensity".to_owned(),
        MaterialProgramParameterValue::Int(1),
    );
    let mut unknown_parameter = accepted.clone();
    unknown_parameter.overrides.clear();
    unknown_parameter.overrides.insert(
        "undeclared".to_owned(),
        MaterialProgramParameterValue::Float(0.2),
    );
    let mut out_of_range = accepted.clone();
    out_of_range.overrides.insert(
        "intensity".to_owned(),
        MaterialProgramParameterValue::Float(1.2),
    );
    let mut stale_schema = accepted.clone();
    stale_schema.schema_signature = stale_schema.schema_signature.wrapping_add(1);
    let mut unknown_program = accepted.clone();
    unknown_program.program = "missing.program".to_owned();
    let non_finite = MaterialProgramParameterConfiguration {
        overrides: BTreeMap::from([(
            "intensity".to_owned(),
            MaterialProgramParameterValue::Float(f64::NAN),
        )]),
        ..accepted.clone()
    };
    let mut unqualified = effect.clone();
    unqualified.name = "glass.continuous".to_owned();
    unqualified.program.program.frame_demand = EffectFrameDemand::Continuous;
    let unqualified_generation = registry_generation(32, vec![unqualified.clone()]);
    let unqualified_config = parameter_configuration(
        &unqualified,
        BTreeMap::from([(
            "intensity".to_owned(),
            MaterialProgramParameterValue::Float(0.82),
        )]),
    );

    for (candidate, registry, expected) in [
        (
            type_mismatch,
            generation.clone(),
            MutationError::InvalidParameterValue,
        ),
        (
            unknown_parameter,
            generation.clone(),
            MutationError::UnknownParameter,
        ),
        (
            out_of_range,
            generation.clone(),
            MutationError::InvalidParameterValue,
        ),
        (stale_schema, generation.clone(), MutationError::StaleSchema),
        (
            unknown_program,
            generation.clone(),
            MutationError::UnknownProgram,
        ),
        (
            unqualified_config,
            unqualified_generation,
            MutationError::Unqualified,
        ),
        (
            non_finite,
            generation.clone(),
            MutationError::InvalidConfiguration,
        ),
    ] {
        let registry_generation_before = registry.generation;
        assert_eq!(
            state.set_configuration(candidate, &registry).unwrap_err(),
            expected
        );
        assert_eq!(state.generation(), parameter_generation_before);
        assert_eq!(store.read().unwrap(), persistent_before);
        assert_eq!(
            fs::read(home.join("AstreaOS/typhon/material-program-parameters.json")).unwrap(),
            bytes_before
        );
        assert_eq!(registry.generation, registry_generation_before);
        assert!(Arc::ptr_eq(&registry, &registry_before) || registry.generation == 32);
    }

    let renumbered = renumber_effect_ids(&effect, 505, 7);
    let renumbered_generation = registry_generation(33, vec![renumbered.clone()]);
    let updated = state
        .set_configuration(
            parameter_configuration(
                &renumbered,
                BTreeMap::from([(
                    "intensity".to_owned(),
                    MaterialProgramParameterValue::Float(0.91),
                )]),
            ),
            &renumbered_generation,
        )
        .unwrap();
    assert!(updated.changed);
    assert_eq!(state.generation(), parameter_generation_before + 1);
    assert_eq!(
        updated.snapshot.description.unwrap().schema_signature,
        renumbered.parameter_schema_signature()
    );
    let block = state.parameter_block_for_effect(&renumbered).unwrap();
    assert_eq!(block.values()[0].id, EffectParameterId::new(7).unwrap());
    assert_eq!(block.values()[0].value, EffectUniformValue::Float(0.91));
    let _ = fs::remove_dir_all(home);
}

#[test]
fn persistence_failure_and_identical_replay_publish_no_parameter_mutation() {
    use oblivion_one::material_program::MaterialProgramParameterMutationError as MutationError;

    let effect = effect_with_parameters(vec![(
        "intensity",
        EffectParameterType::Float,
        Some(EffectParameterRange::Float { min: 0.0, max: 1.0 }),
        EffectUniformValue::Float(0.65),
    )]);
    let generation = registry_generation(33, vec![effect.clone()]);
    let missing_generation = registry_generation(34, Vec::new());
    let home = temporary_config_home();
    let store = MaterialProgramParameterConfigurationStore::new(home.clone()).unwrap();
    let mut state = MaterialProgramParameterControlState::from_store(store.clone());
    let candidate = parameter_configuration(
        &effect,
        BTreeMap::from([(
            "intensity".to_owned(),
            MaterialProgramParameterValue::Float(0.82),
        )]),
    );
    state
        .set_configuration(candidate.clone(), &generation)
        .unwrap();
    let document_before = store.read().unwrap();
    let bytes_before =
        fs::read(home.join("AstreaOS/typhon/material-program-parameters.json")).unwrap();
    let parameter_generation_before = state.generation();

    let replay = state
        .set_configuration(candidate, &missing_generation)
        .unwrap();
    assert!(!replay.changed);
    assert_eq!(
        replay.snapshot.parameter_generation,
        parameter_generation_before
    );
    assert!(replay.snapshot.description.is_none());
    assert_eq!(state.generation(), parameter_generation_before);
    assert_eq!(store.read().unwrap(), document_before);
    assert_eq!(
        fs::read(home.join("AstreaOS/typhon/material-program-parameters.json")).unwrap(),
        bytes_before
    );

    let unavailable = MaterialProgramParameterConfigurationStore::unavailable();
    let mut failing_state = MaterialProgramParameterControlState::from_store(unavailable);
    assert!(matches!(
        failing_state.set_configuration(
            parameter_configuration(
                &effect,
                BTreeMap::from([(
                    "intensity".to_owned(),
                    MaterialProgramParameterValue::Float(0.9),
                )]),
            ),
            &generation,
        ),
        Err(MutationError::Persistence(_))
    ));
    assert_eq!(failing_state.generation(), 0);
    assert_eq!(
        failing_state
            .describe("glass.liquid", &generation)
            .unwrap()
            .parameters[0]
            .effective,
        MaterialProgramParameterValue::Float(0.65)
    );
    let _ = fs::remove_dir_all(home);
}

#[test]
fn matching_persisted_schema_overrides_suspend_and_restore_across_registry_generations() {
    let schema_a = effect_with_parameters(vec![
        (
            "intensity",
            EffectParameterType::Float,
            Some(EffectParameterRange::Float { min: 0.0, max: 1.0 }),
            EffectUniformValue::Float(0.65),
        ),
        (
            "count",
            EffectParameterType::Int,
            Some(EffectParameterRange::Int { min: 1, max: 9 }),
            EffectUniformValue::Int(3),
        ),
    ]);
    let schema_b = effect_with_parameters(vec![
        (
            "intensity",
            EffectParameterType::Float,
            Some(EffectParameterRange::Float { min: 0.0, max: 2.0 }),
            EffectUniformValue::Float(0.4),
        ),
        (
            "count",
            EffectParameterType::Int,
            Some(EffectParameterRange::Int { min: 1, max: 9 }),
            EffectUniformValue::Int(5),
        ),
    ]);
    assert_ne!(
        schema_a.parameter_schema_signature(),
        schema_b.parameter_schema_signature()
    );
    let generation_a = registry_generation(41, vec![schema_a.clone()]);
    let generation_missing = registry_generation(42, Vec::new());
    let generation_b = registry_generation(43, vec![schema_b.clone()]);
    let schema_a_renumbered = renumber_effect_ids(&schema_a, 501, 7);
    let generation_a_renumbered = registry_generation(44, vec![schema_a_renumbered.clone()]);
    let generation_a_restored = registry_generation(45, vec![schema_a.clone()]);
    let home = temporary_config_home();
    let store = MaterialProgramParameterConfigurationStore::new(home.clone()).unwrap();
    let mut state = MaterialProgramParameterControlState::from_store(store.clone());
    let candidate = parameter_configuration(
        &schema_a,
        BTreeMap::from([
            (
                "intensity".to_owned(),
                MaterialProgramParameterValue::Float(0.82),
            ),
            ("count".to_owned(), MaterialProgramParameterValue::Int(6)),
        ]),
    );
    state
        .set_configuration(candidate.clone(), &generation_a)
        .unwrap();
    let persisted = store.read().unwrap();
    let persisted_json = serde_json::to_string(&persisted).unwrap();
    assert!(persisted_json.contains("\"schemaSignature\""));
    assert!(persisted_json.contains("\"intensity\""));
    for renderer_identity_field in [
        "EffectProgramId",
        "EffectParameterId",
        "programId",
        "parameterId",
        "shaderModuleId",
    ] {
        assert!(!persisted_json.contains(renderer_identity_field));
    }
    assert_eq!(state.generation(), 1);

    let replay = state
        .set_configuration(candidate, &generation_missing)
        .unwrap();
    assert!(!replay.changed);
    assert!(replay.snapshot.description.is_none());
    assert_eq!(state.generation(), 1);
    assert_eq!(store.read().unwrap(), persisted);

    let description_a_renumbered = state
        .describe("glass.liquid", &generation_a_renumbered)
        .unwrap();
    assert_eq!(description_a_renumbered.registry_generation, 44);
    assert_eq!(description_a_renumbered.parameter_generation, 1);
    assert_eq!(
        description_a_renumbered.schema_signature,
        schema_a.parameter_schema_signature()
    );
    assert_eq!(
        description_a_renumbered
            .parameters
            .iter()
            .find(|parameter| parameter.name == "intensity")
            .unwrap()
            .effective,
        MaterialProgramParameterValue::Float(0.82)
    );
    assert!(
        description_a_renumbered
            .parameters
            .iter()
            .all(|parameter| parameter.overridden)
    );
    let parameter_block = state
        .parameter_block_for_effect(&schema_a_renumbered)
        .unwrap();
    assert_eq!(parameter_block.values().len(), 2);
    let count_parameter_id = schema_a_renumbered.parameters["count"].spec.id;
    let intensity_parameter_id = schema_a_renumbered.parameters["intensity"].spec.id;
    assert_eq!(count_parameter_id, EffectParameterId::new(7).unwrap());
    assert_eq!(intensity_parameter_id, EffectParameterId::new(8).unwrap());
    assert!(
        parameter_block
            .values()
            .iter()
            .all(|value| value.id != EffectParameterId::new(1).unwrap()
                && value.id != EffectParameterId::new(2).unwrap())
    );
    assert_eq!(
        parameter_block
            .values()
            .iter()
            .find(|value| value.id == intensity_parameter_id)
            .unwrap()
            .value,
        EffectUniformValue::Float(0.82)
    );
    assert_eq!(
        parameter_block
            .values()
            .iter()
            .find(|value| value.id == count_parameter_id)
            .unwrap()
            .value,
        EffectUniformValue::Int(6)
    );
    assert_eq!(state.generation(), 1);
    assert_eq!(store.read().unwrap(), persisted);

    let description_b = state.describe("glass.liquid", &generation_b).unwrap();
    assert_eq!(description_b.registry_generation, 43);
    assert_eq!(description_b.parameter_generation, 1);
    assert!(
        description_b
            .parameters
            .iter()
            .all(|parameter| { parameter.effective == parameter.default && !parameter.overridden })
    );
    assert_eq!(
        description_b
            .parameters
            .iter()
            .find(|parameter| parameter.name == "intensity")
            .unwrap()
            .default,
        MaterialProgramParameterValue::Float(0.4)
    );
    assert_eq!(
        description_b
            .parameters
            .iter()
            .find(|parameter| parameter.name == "count")
            .unwrap()
            .default,
        MaterialProgramParameterValue::Int(5)
    );
    assert_eq!(state.generation(), 1);
    assert_eq!(store.read().unwrap(), persisted);

    let description_a = state
        .describe("glass.liquid", &generation_a_restored)
        .unwrap();
    assert_eq!(description_a.registry_generation, 45);
    assert_eq!(description_a.parameter_generation, 1);
    assert_eq!(
        description_a
            .parameters
            .iter()
            .find(|parameter| parameter.name == "intensity")
            .unwrap()
            .effective,
        MaterialProgramParameterValue::Float(0.82)
    );
    assert!(
        description_a
            .parameters
            .iter()
            .all(|parameter| parameter.overridden)
    );
    assert_eq!(
        description_a
            .parameters
            .iter()
            .find(|parameter| parameter.name == "count")
            .unwrap()
            .effective,
        MaterialProgramParameterValue::Int(6)
    );
    assert_eq!(store.read().unwrap(), persisted);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn trusted_registry_reload_preserves_missing_and_stale_override_intent() {
    let schema_a = effect_with_parameters(vec![(
        "intensity",
        EffectParameterType::Float,
        Some(EffectParameterRange::Float { min: 0.0, max: 1.0 }),
        EffectUniformValue::Float(0.65),
    )]);
    let schema_b = effect_with_parameters(vec![(
        "intensity",
        EffectParameterType::Float,
        Some(EffectParameterRange::Float { min: 0.0, max: 2.0 }),
        EffectUniformValue::Float(0.4),
    )]);
    let trusted_registry = oblivion_one::effects::TrustedEffectRegistry::new();
    let generation_a = trusted_registry
        .reload(effect_manifest(&schema_a), |_| Ok(()))
        .unwrap();
    let home = temporary_config_home();
    let store = MaterialProgramParameterConfigurationStore::new(home.clone()).unwrap();
    let mut state = MaterialProgramParameterControlState::from_store(store.clone());
    let candidate = parameter_configuration(
        &schema_a,
        BTreeMap::from([(
            "intensity".to_owned(),
            MaterialProgramParameterValue::Float(0.82),
        )]),
    );
    state
        .set_configuration(candidate.clone(), &generation_a)
        .unwrap();
    let persisted = store.read().unwrap();
    let parameter_generation = state.generation();
    assert_eq!(parameter_generation, 1);

    let schema_a_renumbered = renumber_effect_ids(&schema_a, 501, 7);
    let generation_a_renumbered = trusted_registry
        .reload(effect_manifest(&schema_a_renumbered), |_| Ok(()))
        .unwrap();
    assert!(generation_a_renumbered.generation > generation_a.generation);
    let description_a_renumbered = state
        .describe("glass.liquid", &generation_a_renumbered)
        .unwrap();
    assert_eq!(
        description_a_renumbered.schema_signature,
        schema_a.parameter_schema_signature()
    );
    assert_eq!(
        description_a_renumbered.parameter_generation,
        parameter_generation
    );
    assert_eq!(
        description_a_renumbered.parameters[0].effective,
        MaterialProgramParameterValue::Float(0.82)
    );
    assert!(description_a_renumbered.parameters[0].overridden);
    let block_a_renumbered = state
        .parameter_block_for_effect(&generation_a_renumbered.effects["glass.liquid"])
        .unwrap();
    assert_eq!(block_a_renumbered.values().len(), 1);
    assert_eq!(
        block_a_renumbered.values()[0].id,
        EffectParameterId::new(7).unwrap()
    );
    assert_eq!(
        block_a_renumbered.values()[0].value,
        EffectUniformValue::Float(0.82)
    );
    assert_eq!(state.generation(), parameter_generation);
    assert_eq!(store.read().unwrap(), persisted);

    let generation_missing = trusted_registry
        .reload(
            oblivion_one::effects::EffectManifest {
                version: 1,
                effects: BTreeMap::new(),
            },
            |_| Ok(()),
        )
        .unwrap();
    assert!(generation_missing.generation > generation_a.generation);
    assert!(state.describe("glass.liquid", &generation_missing).is_err());
    let replay = state
        .set_configuration(candidate.clone(), &generation_missing)
        .unwrap();
    assert!(!replay.changed);
    assert_eq!(state.generation(), parameter_generation);

    let generation_b = trusted_registry
        .reload(effect_manifest(&schema_b), |_| Ok(()))
        .unwrap();
    assert!(generation_b.generation > generation_missing.generation);
    let description_b = state.describe("glass.liquid", &generation_b).unwrap();
    assert_eq!(description_b.parameter_generation, parameter_generation);
    assert_eq!(
        description_b.parameters[0].effective,
        MaterialProgramParameterValue::Float(0.4)
    );
    assert!(!description_b.parameters[0].overridden);
    let block_b = state
        .parameter_block_for_effect(&generation_b.effects["glass.liquid"])
        .unwrap();
    assert_eq!(block_b.values()[0].value, EffectUniformValue::Float(0.4));
    assert_eq!(store.read().unwrap(), persisted);

    let generation_a_restored = trusted_registry
        .reload(effect_manifest(&schema_a), |_| Ok(()))
        .unwrap();
    assert!(generation_a_restored.generation > generation_b.generation);
    let description_a = state
        .describe("glass.liquid", &generation_a_restored)
        .unwrap();
    assert_eq!(description_a.parameter_generation, parameter_generation);
    assert_eq!(
        description_a.parameters[0].effective,
        MaterialProgramParameterValue::Float(0.82)
    );
    assert!(description_a.parameters[0].overridden);
    let block_a = state
        .parameter_block_for_effect(&generation_a_restored.effects["glass.liquid"])
        .unwrap();
    assert_eq!(block_a.values()[0].value, EffectUniformValue::Float(0.82));
    assert_eq!(store.read().unwrap(), persisted);
    assert_eq!(state.generation(), parameter_generation);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn maximum_persisted_document_and_control_payload_fit_their_explicit_bounds() {
    let program_name = |index: usize| {
        let suffix = format!("p{index:03}");
        format!("{}{}", "a".repeat(128 - suffix.len()), suffix)
    };
    let parameter_name = |index: usize| {
        let suffix = format!("v{index:03}");
        format!("{}{}", "b".repeat(128 - suffix.len()), suffix)
    };
    let overrides = || {
        (0..MAX_EFFECT_PARAMETERS)
            .map(|index| {
                (
                    parameter_name(index),
                    MaterialProgramParameterValue::Vec4([
                        f64::from(f32::MAX),
                        f64::from(f32::MAX),
                        f64::from(f32::MAX),
                        f64::from(f32::MAX),
                    ]),
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    let document = MaterialProgramParameterPersistenceDocument {
        version: 1,
        programs: (0..MAX_EFFECT_PROGRAMS)
            .map(|index| {
                (
                    program_name(index),
                    MaterialProgramParameterProgramIntent {
                        schema_signature: index as u64,
                        overrides: overrides(),
                    },
                )
            })
            .collect(),
    };
    document.validate().unwrap();
    let encoded_document = serde_json::to_vec(&document).unwrap();
    assert!(encoded_document.len() <= MAX_MATERIAL_PROGRAM_PARAMETER_DOCUMENT_BYTES);

    let effect_parameters = (0..MAX_EFFECT_PARAMETERS)
        .map(|index| {
            (
                parameter_name(index),
                EffectParameterType::Vec4,
                Some(EffectParameterRange::FloatComponents {
                    min: [f32::MIN; 4],
                    max: [f32::MAX; 4],
                    components: 4,
                }),
                EffectUniformValue::Vec4([0.0; 4]),
            )
        })
        .collect::<Vec<_>>();
    let mut effect = effect_with_named_parameters(effect_parameters);
    effect.name = program_name(0);
    let generation = registry_generation(57, vec![effect.clone()]);
    let home = temporary_config_home();
    let mut state = MaterialProgramParameterControlState::from_store(
        MaterialProgramParameterConfigurationStore::new(home.clone()).unwrap(),
    );
    let configuration = MaterialProgramParameterConfiguration {
        version: 1,
        program: effect.name.clone(),
        schema_signature: effect.parameter_schema_signature(),
        overrides: (0..MAX_EFFECT_PARAMETERS)
            .map(|index| {
                (
                    parameter_name(index),
                    MaterialProgramParameterValue::Vec4([f64::from(f32::MAX); 4]),
                )
            })
            .collect(),
    };
    configuration.validate().unwrap();
    let update = state
        .set_configuration(configuration.clone(), &generation)
        .unwrap();
    let request = serde_json::to_vec(&serde_json::json!({
        "id": u64::MAX,
        "command": "material.program.parameters.set",
        "args": configuration,
    }))
    .unwrap();
    assert!(request.len() < oblivion_one::control::MAX_REQUEST_BYTES);

    let description = update.snapshot.description.unwrap();
    let response = serde_json::to_vec(&serde_json::json!({
        "id": u64::MAX,
        "ok": true,
        "result": description,
    }))
    .unwrap();
    assert_eq!(description.parameters.len(), MAX_EFFECT_PARAMETERS);
    assert!(response.len() < oblivion_one::control::MAX_RESPONSE_BYTES);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn persistence_rejects_unknown_fields_versions_and_invalid_vectors() {
    for invalid in [
        serde_json::json!({"version": 1, "programs": {}, "registryGeneration": 9}),
        serde_json::json!({"version": 2, "programs": {}}),
        serde_json::json!({
            "version": 1,
            "programs": {
                "glass.liquid": {
                    "schemaSignature": 4,
                    "overrides": {
                        "offset": {"type": "vec2", "value": [0.1, 0.2, 0.3]}
                    }
                }
            }
        }),
        serde_json::json!({
            "version": 1,
            "programs": {
                "glass.liquid": {
                    "schemaSignature": 4,
                    "overrides": {},
                    "effective": {}
                }
            }
        }),
    ] {
        let unsupported_version =
            invalid.get("version").and_then(serde_json::Value::as_u64) == Some(2);
        let decoded =
            serde_json::from_value::<MaterialProgramParameterPersistenceDocument>(invalid);
        if unsupported_version {
            assert!(decoded.unwrap().validate().is_err());
        } else {
            assert!(decoded.is_err());
        }
    }

    let too_many_parameters = MaterialProgramParameterPersistenceDocument {
        version: 1,
        programs: BTreeMap::from([(
            "glass.liquid".to_owned(),
            MaterialProgramParameterProgramIntent {
                schema_signature: 5,
                overrides: (0..=MAX_EFFECT_PARAMETERS)
                    .map(|index| {
                        (
                            format!("parameter-{index}"),
                            MaterialProgramParameterValue::Float(0.5),
                        )
                    })
                    .collect(),
            },
        )]),
    };
    assert!(too_many_parameters.validate().is_err());

    let too_many_programs = MaterialProgramParameterPersistenceDocument {
        version: 1,
        programs: (0..=MAX_EFFECT_PROGRAMS)
            .map(|index| {
                (
                    format!("program-{index}"),
                    MaterialProgramParameterProgramIntent {
                        schema_signature: index as u64,
                        overrides: BTreeMap::new(),
                    },
                )
            })
            .collect(),
    };
    assert!(too_many_programs.validate().is_err());
}

#[test]
fn invalid_matching_persisted_overrides_fall_back_to_all_manifest_defaults() {
    let effect = effect_with_parameters(vec![
        (
            "intensity",
            EffectParameterType::Float,
            Some(EffectParameterRange::Float { min: 0.0, max: 1.0 }),
            EffectUniformValue::Float(0.65),
        ),
        (
            "count",
            EffectParameterType::Int,
            Some(EffectParameterRange::Int { min: 1, max: 9 }),
            EffectUniformValue::Int(3),
        ),
    ]);
    let generation = registry_generation(56, vec![effect.clone()]);
    let home = temporary_config_home();
    let store = MaterialProgramParameterConfigurationStore::new(home.clone()).unwrap();
    store
        .write(&MaterialProgramParameterPersistenceDocument {
            version: 1,
            programs: BTreeMap::from([(
                effect.name.clone(),
                MaterialProgramParameterProgramIntent {
                    schema_signature: effect.parameter_schema_signature(),
                    overrides: BTreeMap::from([
                        (
                            "intensity".to_owned(),
                            MaterialProgramParameterValue::Float(0.82),
                        ),
                        (
                            "count".to_owned(),
                            MaterialProgramParameterValue::Float(4.0),
                        ),
                    ]),
                },
            )]),
        })
        .unwrap();
    let state = MaterialProgramParameterControlState::from_store(store);

    let block = state.parameter_block_for_effect(&effect).unwrap();
    assert_eq!(block.values().len(), 2);
    assert_eq!(block.values()[0].value, EffectUniformValue::Float(0.65));
    assert_eq!(block.values()[1].value, EffectUniformValue::Int(3));
    assert_eq!(
        state
            .describe("glass.liquid", &generation)
            .unwrap()
            .parameters
            .iter()
            .map(|descriptor| descriptor.overridden)
            .collect::<Vec<_>>(),
        [false, false]
    );
    let _ = fs::remove_dir_all(home);
}
