use crate::effects::{
    EffectAlphaMode, EffectFailurePolicy, EffectFrameDemand, EffectNode, EffectNodeId,
    EffectOutsets, EffectParameterId, EffectParameterImpact, EffectParameterSpec,
    EffectParameterType, EffectProgram, EffectProgramId, EffectRegistry, EffectRegistryGeneration,
    EffectSource, EffectUniformValue, EffectWorkingSpace, RegisteredEffect,
    validate_effect_program,
};
use std::{collections::BTreeMap, sync::Arc};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, time::SystemTime};

use super::{MaterialProgramCatalogSnapshot, MaterialProgramOrigin, qualifies_registered_effect};

fn registered_effect(
    name: &str,
    frame_demand: EffectFrameDemand,
    sources: &[EffectSource],
    program_id: u64,
) -> RegisteredEffect {
    let nodes = sources
        .iter()
        .enumerate()
        .map(|(index, source)| {
            EffectNode::source(
                EffectNodeId::new(u16::try_from(index + 1).unwrap()).unwrap(),
                *source,
            )
        })
        .collect::<Vec<_>>();
    let program = validate_effect_program(EffectProgram {
        id: EffectProgramId::new(program_id).unwrap(),
        output: nodes[0].id,
        nodes,
        working_space: EffectWorkingSpace::LinearSrgb,
        alpha_mode: EffectAlphaMode::Opaque,
        outsets: EffectOutsets::ZERO,
        frame_demand,
        failure_policy: EffectFailurePolicy::Passthrough,
    })
    .unwrap();
    RegisteredEffect {
        name: name.to_owned(),
        program,
        parameters: BTreeMap::new(),
    }
}

fn registry_generation(
    generation: u64,
    effects: Vec<RegisteredEffect>,
) -> Arc<EffectRegistryGeneration> {
    let builtin_program = crate::effects::render_graph::builtin_background_blur_program();
    let builtin_name = crate::effects::BUILTIN_BACKGROUND_BLUR_NAME.to_owned();
    let mut registry = EffectRegistry::empty();
    registry.insert(builtin_program.clone()).unwrap();
    let mut registered = BTreeMap::from([(
        builtin_name.clone(),
        RegisteredEffect {
            name: builtin_name,
            program: builtin_program,
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

fn manifest_for_effect(effect: &RegisteredEffect) -> crate::effects::EffectManifest {
    crate::effects::EffectManifest {
        version: 1,
        effects: BTreeMap::from([(
            effect.name.clone(),
            crate::effects::EffectDefinition {
                name: effect.name.clone(),
                program: effect.program.program.clone(),
                parameters: effect.parameters.clone(),
                shader_assets: vec![crate::effects::config::EffectShaderAsset {
                    module: crate::effects::ShaderModuleId::new(777).unwrap(),
                    relative_path: "material.frag".into(),
                    source: "void main() {}".to_owned(),
                    uniforms: Vec::new(),
                }],
            },
        )]),
    }
}

#[test]
fn on_damage_backdrop_effects_qualify_as_global_material_programs() {
    let mut effect = registered_effect(
        "glass.liquid",
        EffectFrameDemand::OnDamage,
        &[EffectSource::Backdrop],
        100,
    );
    effect.parameters.insert(
        "intensity".to_owned(),
        crate::effects::EffectParameterDefinition {
            spec: EffectParameterSpec {
                id: EffectParameterId::new(1).unwrap(),
                name: "intensity".to_owned(),
                ty: EffectParameterType::Float,
                range: None,
                impact: EffectParameterImpact::UniformOnly,
            },
            default: EffectUniformValue::Float(0.65),
        },
    );

    assert!(qualifies_registered_effect(&effect));
}

#[test]
fn material_program_qualification_rejects_invalid_default_parameter_blocks() {
    let mut effect = registered_effect(
        "glass.invalid",
        EffectFrameDemand::OnDamage,
        &[EffectSource::Backdrop],
        134,
    );
    for (name, default) in [("intensity", 0.65), ("highlight", 0.2)] {
        effect.parameters.insert(
            name.to_owned(),
            crate::effects::EffectParameterDefinition {
                spec: EffectParameterSpec {
                    id: EffectParameterId::new(1).unwrap(),
                    name: name.to_owned(),
                    ty: EffectParameterType::Float,
                    range: None,
                    impact: EffectParameterImpact::UniformOnly,
                },
                default: EffectUniformValue::Float(default),
            },
        );
    }

    assert!(effect.default_parameter_block().is_err());
    assert!(!qualifies_registered_effect(&effect));
}

#[test]
fn continuous_backdrop_effects_do_not_qualify_as_global_material_programs() {
    let effect = registered_effect(
        "glass.continuous",
        EffectFrameDemand::Continuous,
        &[EffectSource::Backdrop],
        101,
    );

    assert!(!qualifies_registered_effect(&effect));
}

#[test]
fn target_content_sources_disqualify_global_material_programs() {
    let effect = registered_effect(
        "glass.target",
        EffectFrameDemand::OnDamage,
        &[EffectSource::Backdrop, EffectSource::TargetContent],
        102,
    );

    assert!(!qualifies_registered_effect(&effect));
}

#[test]
fn non_uniform_only_parameters_disqualify_global_material_programs() {
    let mut effect = registered_effect(
        "glass.footprint",
        EffectFrameDemand::OnDamage,
        &[EffectSource::Backdrop],
        128,
    );
    effect.parameters.insert(
        "radius".to_owned(),
        crate::effects::EffectParameterDefinition {
            spec: EffectParameterSpec {
                id: EffectParameterId::new(1).unwrap(),
                name: "radius".to_owned(),
                ty: EffectParameterType::Float,
                range: None,
                impact: EffectParameterImpact::Footprint,
            },
            default: EffectUniformValue::Float(1.0),
        },
    );

    assert!(!qualifies_registered_effect(&effect));
}

#[test]
fn catalog_presents_builtin_first_and_qualified_local_programs_in_name_order() {
    let generation = registry_generation(
        17,
        vec![
            registered_effect(
                "glass.zeta",
                EffectFrameDemand::OnDamage,
                &[EffectSource::Backdrop],
                103,
            ),
            registered_effect(
                "glass.alpha",
                EffectFrameDemand::OnDamage,
                &[EffectSource::Backdrop],
                104,
            ),
            registered_effect(
                "glass.continuous",
                EffectFrameDemand::Continuous,
                &[EffectSource::Backdrop],
                105,
            ),
        ],
    );
    let catalog = MaterialProgramCatalogSnapshot::from_registry_generation(&generation, true);

    assert_eq!(catalog.registry_generation, generation.generation);
    assert_eq!(
        catalog
            .programs
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["system.background_blur", "glass.alpha", "glass.zeta"]
    );
    assert_eq!(catalog.programs[0].origin, MaterialProgramOrigin::System);
    assert!(generation.effects.contains_key("glass.continuous"));
    assert!(
        generation
            .registry
            .get(generation.effects["glass.continuous"].program.program.id)
            .is_some()
    );
}

#[test]
fn catalog_wire_entries_expose_only_stable_identity_and_bounded_schema_summary() {
    let generation = registry_generation(
        4,
        vec![registered_effect(
            "glass.liquid",
            EffectFrameDemand::OnDamage,
            &[EffectSource::Backdrop],
            106,
        )],
    );
    let catalog = MaterialProgramCatalogSnapshot::from_registry_generation(&generation, false);
    let json = serde_json::to_value(catalog).unwrap();
    let entry = json["programs"][1].as_object().unwrap();

    assert_eq!(
        entry
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        ["name", "origin", "parameterCount", "schemaSignature"]
            .into_iter()
            .collect()
    );
    assert_eq!(entry["name"], "glass.liquid");
    assert_eq!(entry["origin"], "trusted_local");
    let json = json.to_string();
    assert!(!json.contains("relativePath"));
    assert!(!json.contains("shaderSource"));
    assert!(!json.contains("ShaderModuleId"));
    assert!(!json.contains("EffectProgramId"));

    let mut strict_catalog = serde_json::to_value(
        MaterialProgramCatalogSnapshot::from_registry_generation(&generation, false),
    )
    .unwrap();
    strict_catalog["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<MaterialProgramCatalogSnapshot>(strict_catalog).is_err());
}

#[test]
fn maximum_catalog_response_stays_below_control_response_budget() {
    let suffix_length = 8;
    let prefix = "p".repeat(crate::effects::MAX_EFFECT_NAME_BYTES - suffix_length);
    let effects = (0..crate::effects::MAX_EFFECT_PROGRAMS - 1)
        .map(|index| {
            let name = format!("{prefix}{index:08}");
            registered_effect(
                &name,
                EffectFrameDemand::OnDamage,
                &[EffectSource::Backdrop],
                1_000 + u64::try_from(index).unwrap(),
            )
        })
        .collect();
    let generation = registry_generation(99, effects);
    let catalog = MaterialProgramCatalogSnapshot::from_registry_generation(&generation, true);
    let response =
        crate::control::ControlResponse::success(12, serde_json::to_value(catalog).unwrap());
    let encoded = crate::control::encode_response(&response).unwrap();

    assert_eq!(
        generation.effects.len(),
        crate::effects::MAX_EFFECT_PROGRAMS
    );
    assert!(encoded.len() < crate::control::MAX_RESPONSE_BYTES);
}

#[test]
fn material_program_configuration_is_strict_and_defaults_to_the_builtin() {
    let configuration = super::MaterialProgramConfiguration::default();
    assert_eq!(
        configuration.requested_program,
        crate::effects::BUILTIN_BACKGROUND_BLUR_NAME
    );
    configuration.validate().unwrap();

    let json = serde_json::to_value(&configuration).unwrap();
    assert_eq!(
        json.as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        ["requestedProgram", "version"].into_iter().collect()
    );
    assert!(
        serde_json::from_value::<super::MaterialProgramConfiguration>(serde_json::json!({
            "version": 1,
            "requestedProgram": "glass.liquid",
            "shaderPath": "effect.frag"
        }))
        .is_err()
    );
}

#[test]
fn material_program_names_use_the_trusted_effect_bounded_grammar() {
    for invalid in ["", "a/b", "has space", "glass\n", "é"] {
        let configuration = super::MaterialProgramConfiguration {
            version: 1,
            requested_program: invalid.to_owned(),
        };
        assert!(configuration.validate().is_err(), "accepted {invalid:?}");
    }
    let too_long = "a".repeat(crate::effects::MAX_EFFECT_NAME_BYTES + 1);
    assert!(
        super::MaterialProgramConfiguration {
            version: 1,
            requested_program: too_long,
        }
        .validate()
        .is_err()
    );
    assert!(
        super::MaterialProgramConfiguration {
            version: 2,
            requested_program: crate::effects::BUILTIN_BACKGROUND_BLUR_NAME.to_owned(),
        }
        .validate()
        .is_err()
    );
}

fn temporary_config_home() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "typhon-material-program-tests-{}-{}",
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

#[test]
fn material_program_persistence_uses_the_separate_private_file() {
    let home = temporary_config_home();
    let store = super::MaterialProgramConfigurationStore::new(home.clone()).unwrap();
    let configuration = super::MaterialProgramConfiguration {
        version: 1,
        requested_program: "glass.liquid".to_owned(),
    };

    store.write(&configuration).unwrap();

    assert_eq!(store.read().unwrap(), configuration);
    assert_eq!(
        store.configuration_file().file_name().unwrap(),
        "material-program.json"
    );
    assert_eq!(
        fs::metadata(store.configuration_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    for directory in [home.join("AstreaOS"), home.join("AstreaOS/typhon")] {
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    let _ = fs::remove_dir_all(home);
}

#[test]
fn material_program_persistence_rejects_invalid_documents_and_symlinks() {
    let home = temporary_config_home();
    let store = super::MaterialProgramConfigurationStore::new(home.clone()).unwrap();
    let path = store.configuration_file().to_owned();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&path, br#"{"version":2,"requestedProgram":"glass.liquid"}"#).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        store.read(),
        Err(super::MaterialProgramPersistenceError::Invalid)
    );

    fs::remove_file(&path).unwrap();
    fs::write(
        &path,
        vec![b'x'; super::MAX_MATERIAL_PROGRAM_CONFIGURATION_BYTES + 1],
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        store.read(),
        Err(super::MaterialProgramPersistenceError::Invalid)
    );

    fs::remove_file(&path).unwrap();
    let target = home.join("target.json");
    fs::write(&target, b"{} ").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert_eq!(
        store.read(),
        Err(super::MaterialProgramPersistenceError::Insecure)
    );
    assert_eq!(
        store.write(&super::MaterialProgramConfiguration::default()),
        Err(super::MaterialProgramPersistenceError::Insecure)
    );
    let _ = fs::remove_dir_all(home);
}

fn program_configuration(name: &str) -> super::MaterialProgramConfiguration {
    super::MaterialProgramConfiguration {
        version: 1,
        requested_program: name.to_owned(),
    }
}

#[test]
fn selection_defaults_to_the_builtin_and_serializes_null_fallback_explicitly() {
    let generation = registry_generation(3, Vec::new());
    let state = super::MaterialProgramControlState::from_store(
        super::MaterialProgramConfigurationStore::unavailable(),
    );
    let snapshot = state.snapshot(&generation, false);

    assert_eq!(
        snapshot.configuration.requested_program,
        crate::effects::BUILTIN_BACKGROUND_BLUR_NAME
    );
    assert_eq!(
        snapshot.effective_program,
        crate::effects::BUILTIN_BACKGROUND_BLUR_NAME
    );
    assert_eq!(snapshot.fallback_reason, None);
    assert_eq!(snapshot.registry_generation, generation.generation);
    assert!(!snapshot.rendering_available);
    let serialized = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(serialized["fallbackReason"], serde_json::Value::Null);
    let mut missing_fallback_field = serialized;
    missing_fallback_field
        .as_object_mut()
        .unwrap()
        .remove("fallbackReason");
    assert!(
        serde_json::from_value::<super::MaterialProgramSelectionSnapshot>(missing_fallback_field)
            .is_err()
    );
}

#[test]
fn qualified_selection_persists_and_advances_only_its_own_generation() {
    let home = temporary_config_home();
    let store = super::MaterialProgramConfigurationStore::new(home.clone()).unwrap();
    let mut state = super::MaterialProgramControlState::from_store(store.clone());
    let generation = registry_generation(
        8,
        vec![registered_effect(
            "glass.liquid",
            EffectFrameDemand::OnDamage,
            &[EffectSource::Backdrop],
            120,
        )],
    );

    let update = state
        .set_configuration(program_configuration("glass.liquid"), &generation, false)
        .unwrap();

    assert!(update.changed);
    assert_eq!(update.snapshot.generation, 1);
    assert_eq!(
        update.snapshot.source,
        super::MaterialProgramConfigSource::Runtime
    );
    assert_eq!(update.snapshot.effective_program, "glass.liquid");
    assert_eq!(store.read().unwrap(), program_configuration("glass.liquid"));
    assert!(!update.snapshot.rendering_available);
    let reopened = super::MaterialProgramControlState::from_store(store.clone());
    let persisted = reopened.snapshot(&generation, true);
    assert_eq!(
        persisted.source,
        super::MaterialProgramConfigSource::Persisted
    );
    assert_eq!(persisted.configuration.requested_program, "glass.liquid");
    let _ = fs::remove_dir_all(home);
}

#[test]
fn identical_selection_is_a_persistence_and_generation_noop() {
    let mut state = super::MaterialProgramControlState::from_store(
        super::MaterialProgramConfigurationStore::unavailable(),
    );
    let generation = registry_generation(1, Vec::new());
    let before = state.snapshot(&generation, false);

    let update = state
        .set_configuration(
            program_configuration(crate::effects::BUILTIN_BACKGROUND_BLUR_NAME),
            &generation,
            false,
        )
        .unwrap();

    assert!(!update.changed);
    assert_eq!(update.snapshot, before);
}

#[test]
fn unknown_and_unqualified_programs_are_rejected_without_state_change() {
    let home = temporary_config_home();
    let store = super::MaterialProgramConfigurationStore::new(home.clone()).unwrap();
    let mut state = super::MaterialProgramControlState::from_store(store.clone());
    let generation = registry_generation(
        5,
        vec![registered_effect(
            "glass.continuous",
            EffectFrameDemand::Continuous,
            &[EffectSource::Backdrop],
            121,
        )],
    );
    let before = state.snapshot(&generation, true);

    assert!(matches!(
        state.set_configuration(program_configuration("glass.unknown"), &generation, true),
        Err(super::MaterialProgramMutationError::UnknownProgram)
    ));
    assert!(matches!(
        state.set_configuration(program_configuration("glass.continuous"), &generation, true),
        Err(super::MaterialProgramMutationError::Unqualified)
    ));
    assert_eq!(state.snapshot(&generation, true), before);
    assert!(store.read().is_err());
    let _ = fs::remove_dir_all(home);
}

#[test]
fn same_requested_program_is_idempotent_after_missing_fallback() {
    let home = temporary_config_home();
    let store = super::MaterialProgramConfigurationStore::new(home.clone()).unwrap();
    let mut state = super::MaterialProgramControlState::from_store(store.clone());
    let selected = registry_generation(
        20,
        vec![registered_effect(
            "glass.liquid",
            EffectFrameDemand::OnDamage,
            &[EffectSource::Backdrop],
            130,
        )],
    );
    state
        .set_configuration(program_configuration("glass.liquid"), &selected, true)
        .unwrap();

    let fallback = registry_generation(
        21,
        vec![registered_effect(
            "glass.other",
            EffectFrameDemand::Continuous,
            &[EffectSource::Backdrop],
            131,
        )],
    );
    let before = state.snapshot(&fallback, true);
    assert_eq!(
        before.fallback_reason,
        Some(super::MaterialProgramFallbackReason::Missing)
    );
    let configuration_path = store.configuration_file().to_owned();
    fs::remove_file(&configuration_path).unwrap();
    fs::create_dir(&configuration_path).unwrap();

    let update = state
        .set_configuration(program_configuration("glass.liquid"), &fallback, true)
        .unwrap();

    assert!(!update.changed);
    assert_eq!(update.snapshot, before);
    assert_eq!(
        update.snapshot.configuration.requested_program,
        "glass.liquid"
    );
    assert_eq!(
        update.snapshot.effective_program,
        crate::effects::BUILTIN_BACKGROUND_BLUR_NAME
    );
    assert_eq!(
        update.snapshot.fallback_reason,
        Some(super::MaterialProgramFallbackReason::Missing)
    );
    assert!(configuration_path.is_dir());

    assert!(matches!(
        state.set_configuration(
            program_configuration("glass.does_not_exist"),
            &fallback,
            true
        ),
        Err(super::MaterialProgramMutationError::UnknownProgram)
    ));
    assert!(matches!(
        state.set_configuration(program_configuration("glass.other"), &fallback, true),
        Err(super::MaterialProgramMutationError::Unqualified)
    ));
    assert_eq!(state.snapshot(&fallback, true), before);
    assert!(configuration_path.is_dir());
    let _ = fs::remove_dir_all(home);
}

#[test]
fn same_requested_program_is_idempotent_after_unqualified_fallback() {
    let home = temporary_config_home();
    let store = super::MaterialProgramConfigurationStore::new(home.clone()).unwrap();
    let mut state = super::MaterialProgramControlState::from_store(store.clone());
    let selected = registry_generation(
        22,
        vec![registered_effect(
            "glass.liquid",
            EffectFrameDemand::OnDamage,
            &[EffectSource::Backdrop],
            132,
        )],
    );
    state
        .set_configuration(program_configuration("glass.liquid"), &selected, true)
        .unwrap();

    let fallback = registry_generation(
        23,
        vec![registered_effect(
            "glass.liquid",
            EffectFrameDemand::Continuous,
            &[EffectSource::Backdrop],
            133,
        )],
    );
    let before = state.snapshot(&fallback, true);
    assert_eq!(
        before.fallback_reason,
        Some(super::MaterialProgramFallbackReason::Unqualified)
    );
    let configuration_path = store.configuration_file().to_owned();
    fs::remove_file(&configuration_path).unwrap();
    fs::create_dir(&configuration_path).unwrap();

    let update = state
        .set_configuration(program_configuration("glass.liquid"), &fallback, true)
        .unwrap();

    assert!(!update.changed);
    assert_eq!(update.snapshot, before);
    assert_eq!(
        update.snapshot.configuration.requested_program,
        "glass.liquid"
    );
    assert_eq!(
        update.snapshot.effective_program,
        crate::effects::BUILTIN_BACKGROUND_BLUR_NAME
    );
    assert_eq!(
        update.snapshot.fallback_reason,
        Some(super::MaterialProgramFallbackReason::Unqualified)
    );
    assert!(configuration_path.is_dir());
    let _ = fs::remove_dir_all(home);
}

#[test]
fn persistence_failure_publishes_no_selection_candidate() {
    let mut state = super::MaterialProgramControlState::from_store(
        super::MaterialProgramConfigurationStore::unavailable(),
    );
    let generation = registry_generation(
        6,
        vec![registered_effect(
            "glass.liquid",
            EffectFrameDemand::OnDamage,
            &[EffectSource::Backdrop],
            122,
        )],
    );
    let before = state.snapshot(&generation, true);

    assert!(matches!(
        state.set_configuration(program_configuration("glass.liquid"), &generation, true),
        Err(super::MaterialProgramMutationError::Persistence(_))
    ));
    assert_eq!(state.snapshot(&generation, true), before);
}

#[test]
fn requested_selection_falls_back_and_restores_across_registry_generations() {
    let home = temporary_config_home();
    let store = super::MaterialProgramConfigurationStore::new(home.clone()).unwrap();
    let mut state = super::MaterialProgramControlState::from_store(store.clone());
    let qualified = registry_generation(
        10,
        vec![registered_effect(
            "glass.liquid",
            EffectFrameDemand::OnDamage,
            &[EffectSource::Backdrop],
            123,
        )],
    );
    state
        .set_configuration(program_configuration("glass.liquid"), &qualified, true)
        .unwrap();

    let missing = registry_generation(11, Vec::new());
    let missing_snapshot = state.snapshot(&missing, true);
    assert_eq!(
        missing_snapshot.configuration.requested_program,
        "glass.liquid"
    );
    assert_eq!(
        missing_snapshot.effective_program,
        crate::effects::BUILTIN_BACKGROUND_BLUR_NAME
    );
    assert_eq!(
        missing_snapshot.fallback_reason,
        Some(super::MaterialProgramFallbackReason::Missing)
    );
    assert_eq!(store.read().unwrap(), program_configuration("glass.liquid"));

    let unqualified = registry_generation(
        12,
        vec![registered_effect(
            "glass.liquid",
            EffectFrameDemand::Continuous,
            &[EffectSource::Backdrop],
            124,
        )],
    );
    let unqualified_snapshot = state.snapshot(&unqualified, true);
    assert_eq!(
        unqualified_snapshot.fallback_reason,
        Some(super::MaterialProgramFallbackReason::Unqualified)
    );

    let restored = registry_generation(
        13,
        vec![registered_effect(
            "glass.liquid",
            EffectFrameDemand::OnDamage,
            &[EffectSource::Backdrop],
            125,
        )],
    );
    let restored_snapshot = state.snapshot(&restored, true);
    assert_eq!(restored_snapshot.effective_program, "glass.liquid");
    assert_eq!(restored_snapshot.fallback_reason, None);
    assert_eq!(restored_snapshot.generation, 1);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn failed_registry_reload_keeps_the_selected_program_effective() {
    let home = temporary_config_home();
    let store = super::MaterialProgramConfigurationStore::new(home.clone()).unwrap();
    let mut state = super::MaterialProgramControlState::from_store(store);
    let registry = crate::effects::TrustedEffectRegistry::with_builtin_background_blur();
    let effect = registered_effect(
        "glass.liquid",
        EffectFrameDemand::OnDamage,
        &[EffectSource::Backdrop],
        126,
    );
    let manifest = manifest_for_effect(&effect);
    let active = registry.reload(manifest.clone(), |_| Ok(())).unwrap();
    state
        .set_configuration(program_configuration("glass.liquid"), &active, true)
        .unwrap();

    assert!(
        registry
            .reload(manifest, |_| Err("test compile failure".to_owned()))
            .is_err()
    );
    let after_failed_reload = registry.current();
    let snapshot = state.snapshot(&after_failed_reload, true);

    assert!(Arc::ptr_eq(&active, &after_failed_reload));
    assert_eq!(snapshot.effective_program, "glass.liquid");
    assert_eq!(snapshot.fallback_reason, None);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn rejected_schema_reload_keeps_selected_registry_generation_and_program() {
    let home = temporary_config_home();
    let store = super::MaterialProgramConfigurationStore::new(home.clone()).unwrap();
    let mut state = super::MaterialProgramControlState::from_store(store);
    let registry = crate::effects::TrustedEffectRegistry::with_builtin_background_blur();
    let effect = registered_effect(
        "glass.liquid",
        EffectFrameDemand::OnDamage,
        &[EffectSource::Backdrop],
        135,
    );
    let valid = manifest_for_effect(&effect);
    let active = registry.reload(valid.clone(), |_| Ok(())).unwrap();
    state
        .set_configuration(program_configuration("glass.liquid"), &active, true)
        .unwrap();
    let selection_before = state.snapshot(&active, true);

    let mut invalid = valid;
    invalid
        .effects
        .get_mut("glass.liquid")
        .unwrap()
        .parameters
        .extend([
            (
                "intensity".to_owned(),
                crate::effects::EffectParameterDefinition {
                    spec: EffectParameterSpec {
                        id: EffectParameterId::new(1).unwrap(),
                        name: "intensity".to_owned(),
                        ty: EffectParameterType::Float,
                        range: None,
                        impact: EffectParameterImpact::UniformOnly,
                    },
                    default: EffectUniformValue::Float(0.65),
                },
            ),
            (
                "highlight".to_owned(),
                crate::effects::EffectParameterDefinition {
                    spec: EffectParameterSpec {
                        id: EffectParameterId::new(1).unwrap(),
                        name: "highlight".to_owned(),
                        ty: EffectParameterType::Float,
                        range: None,
                        impact: EffectParameterImpact::UniformOnly,
                    },
                    default: EffectUniformValue::Float(0.2),
                },
            ),
        ]);

    assert!(
        registry
            .reload(invalid, |_| panic!(
                "invalid parameter schema reached shader precompile"
            ))
            .is_err()
    );

    let current = registry.current();
    assert!(Arc::ptr_eq(&active, &current));
    let selection_after = state.snapshot(&current, true);
    assert_eq!(selection_after, selection_before);
    assert_eq!(
        selection_after.configuration.requested_program,
        "glass.liquid"
    );
    assert_eq!(selection_after.effective_program, "glass.liquid");
    let _ = fs::remove_dir_all(home);
}
