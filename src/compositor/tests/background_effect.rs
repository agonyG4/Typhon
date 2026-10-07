use super::*;
use crate::astrea_background_effect_coverage::client::astrea_background_effect_coverage_manager_v1 as client_coverage_manager;
use crate::effects::{
    EffectAlphaMode, EffectFailurePolicy, EffectFrameDemand, EffectNode, EffectNodeId,
    EffectOutsets, EffectParameterId, EffectParameterImpact, EffectParameterSpec,
    EffectParameterType, EffectProgram, EffectProgramId, EffectRect, EffectSource,
    EffectUniformValue, EffectWorkingSpace,
    config::{EffectDefinition, EffectManifest, EffectParameterDefinition},
};
use std::os::unix::fs::PermissionsExt;
use std::{collections::BTreeMap, sync::Arc};

fn material_default_manifest() -> EffectManifest {
    let backdrop = EffectNodeId::new(1).unwrap();
    EffectManifest {
        version: 1,
        effects: BTreeMap::from([(
            "glass.liquid".to_owned(),
            EffectDefinition {
                name: "glass.liquid".to_owned(),
                program: EffectProgram {
                    id: EffectProgramId::new(900).unwrap(),
                    nodes: vec![EffectNode::source(backdrop, EffectSource::Backdrop)],
                    output: backdrop,
                    working_space: EffectWorkingSpace::LinearSrgb,
                    alpha_mode: EffectAlphaMode::Opaque,
                    outsets: EffectOutsets::ZERO,
                    frame_demand: EffectFrameDemand::OnDamage,
                    failure_policy: EffectFailurePolicy::Passthrough,
                },
                parameters: BTreeMap::from([(
                    "intensity".to_owned(),
                    EffectParameterDefinition {
                        spec: EffectParameterSpec {
                            id: EffectParameterId::new(1).unwrap(),
                            name: "intensity".to_owned(),
                            ty: EffectParameterType::Float,
                            range: None,
                            impact: EffectParameterImpact::UniformOnly,
                        },
                        default: EffectUniformValue::Float(0.65),
                    },
                )]),
                shader_assets: Vec::new(),
            },
        )]),
    }
}

fn capture_effect_scene(commands: &Sender<ServerCommand>) -> ResolvedEffectScene {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureResolvedEffectScene(reply))
        .expect("effect scene capture command should be accepted");
    wait_for_server_commands(commands);
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("effect scene capture should complete")
}

fn sole_background_blur(
    scene: &ResolvedEffectScene,
) -> &crate::compositor::effects::ResolvedEffectInstance {
    let mut blurs = scene.instances.iter().filter(|instance| {
        instance.program == crate::effects::builtin_background_blur_program_id()
    });
    let blur = blurs.next().expect("background blur should remain active");
    assert!(
        blurs.next().is_none(),
        "the window should have exactly one effective background blur"
    );
    blur
}

fn set_material_program(
    commands: &Sender<ServerCommand>,
    requested_program: &str,
) -> Result<(), String> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::SetMaterialProgramConfiguration {
            configuration: crate::material_program::MaterialProgramConfiguration {
                version: 1,
                requested_program: requested_program.to_owned(),
            },
            reply,
        })
        .expect("material program selection command should be accepted");
    wait_for_server_commands(commands);
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("material program selection should complete")
}

fn set_material_program_parameters(
    commands: &Sender<ServerCommand>,
    configuration: crate::material_program::MaterialProgramParameterConfiguration,
) -> Result<(), String> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::SetMaterialProgramParameterConfiguration {
            configuration,
            reply,
        })
        .expect("material program parameter command should be accepted");
    wait_for_server_commands(commands);
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("material program parameter configuration should complete")
}

type BackgroundEffectConnection = (
    Connection,
    EventQueue<RegistryTestState>,
    client_wl_compositor::WlCompositor,
    client_ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1,
);

fn connect_background_effect_client(
    socket_path: &PathBuf,
) -> Result<BackgroundEffectConnection, Box<dyn std::error::Error>> {
    let connection = Connection::from_socket(UnixStream::connect(socket_path)?)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor = globals.bind(&qh, 1..=6, ())?;
    let manager = globals.bind(&qh, 1..=1, ())?;
    Ok((connection, queue, compositor, manager))
}

#[test]
fn background_effect_global_sends_blur_capability() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let (connection, mut queue, _compositor, _manager) =
            connect_background_effect_client(&socket_path)?;
        let mut state = RegistryTestState::default();
        connection.flush()?;
        queue.roundtrip(&mut state)?;
        assert_eq!(state.background_effect_capabilities, vec![1]);
        Ok(())
    })();

    stop_test_server(running, server_thread);
    result.unwrap();
}

#[test]
fn duplicate_background_effect_object_is_the_exact_manager_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let (connection, queue, compositor, manager) =
            connect_background_effect_client(&socket_path)?;
        let qh = queue.handle();
        let surface = compositor.create_surface(&qh, ());
        let _effect = manager.get_background_effect(&surface, &qh, ());
        connection.roundtrip()?;
        let _duplicate = manager.get_background_effect(&surface, &qh, ());
        connection.flush()?;
        assert!(connection.roundtrip().is_err());
        let error = connection
            .protocol_error()
            .expect("duplicate effect object must be a wire error");
        assert_eq!(error.object_interface, "ext_background_effect_manager_v1");
        assert_eq!(
            error.code,
            client_ext_background_effect_manager_v1::Error::BackgroundEffectExists as u32
        );
        Ok(())
    })();

    stop_test_server(running, server_thread);
    result.unwrap();
}

#[test]
fn background_effect_state_is_copied_and_double_buffered() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let persistence_directory = std::env::temp_dir().join(format!(
        "typhon-material-program-{}-{}",
        std::process::id(),
        socket_name
    ));
    std::fs::create_dir(&persistence_directory).unwrap();
    std::fs::set_permissions(
        &persistence_directory,
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    server.state.material_program_control =
        crate::material_program::MaterialProgramControlState::from_store(
            crate::material_program::MaterialProgramConfigurationStore::new(
                persistence_directory.clone(),
            )
            .unwrap(),
        );
    server.state.material_program_parameter_control =
        crate::material_program::MaterialProgramParameterControlState::from_store(
            crate::material_program::MaterialProgramParameterConfigurationStore::new(
                persistence_directory.clone(),
            )
            .unwrap(),
        );
    server.state.material_control = crate::material::MaterialControlState::from_store(
        crate::material::MaterialConfigurationStore::new(persistence_directory.clone()).unwrap(),
    );
    server.set_material_runtime_capabilities(crate::material::MaterialCapabilities::full());
    server
        .state
        .trusted_effect_registry
        .reload(material_default_manifest(), |_| Ok(()))
        .unwrap();
    let builtin_radius_before = server
        .state
        .trusted_effect_registry
        .current()
        .program(crate::effects::BUILTIN_BACKGROUND_BLUR_NAME)
        .unwrap()
        .aggregate_footprint
        .sample_radius_x;
    server
        .set_material_program_configuration(crate::material_program::MaterialProgramConfiguration {
            version: 1,
            requested_program: "glass.liquid".to_owned(),
        })
        .unwrap();
    server
        .set_material_configuration(crate::material::MaterialConfiguration {
            position: 1.0,
            ..crate::material::MaterialConfiguration::default()
        })
        .unwrap();
    let builtin_radius_after = server
        .state
        .trusted_effect_registry
        .current()
        .program(crate::effects::BUILTIN_BACKGROUND_BLUR_NAME)
        .unwrap()
        .aggregate_footprint
        .sample_radius_x;
    assert!(builtin_radius_after > builtin_radius_before);
    let selection_after_material_update = server.material_program_selection_snapshot();
    assert_eq!(
        selection_after_material_update
            .configuration
            .requested_program,
        "glass.liquid"
    );
    assert_eq!(
        selection_after_material_update.effective_program,
        "glass.liquid"
    );
    server
        .set_material_program_configuration(crate::material_program::MaterialProgramConfiguration {
            version: 1,
            requested_program: "glass.liquid".to_owned(),
        })
        .unwrap();
    let parameter_generation = server.state.trusted_effect_registry.current();
    let parameter_configuration = crate::material_program::MaterialProgramParameterConfiguration {
        version: 1,
        program: "glass.liquid".to_owned(),
        schema_signature: parameter_generation.effects["glass.liquid"].parameter_schema_signature(),
        overrides: BTreeMap::from([(
            "intensity".to_owned(),
            crate::material_program::MaterialProgramParameterValue::Float(0.82),
        )]),
    };
    let active_generation = server.state.trusted_effect_registry.current();
    let selection_before_invalid_reload = server.material_program_selection_snapshot();
    let mut invalid_manifest = material_default_manifest();
    invalid_manifest
        .effects
        .get_mut("glass.liquid")
        .unwrap()
        .parameters
        .insert(
            "highlight".to_owned(),
            EffectParameterDefinition {
                spec: EffectParameterSpec {
                    id: EffectParameterId::new(1).unwrap(),
                    name: "highlight".to_owned(),
                    ty: EffectParameterType::Float,
                    range: None,
                    impact: EffectParameterImpact::UniformOnly,
                },
                default: EffectUniformValue::Float(0.2),
            },
        );
    invalid_manifest
        .effects
        .get_mut("glass.liquid")
        .unwrap()
        .shader_assets
        .push(crate::effects::config::EffectShaderAsset {
            module: crate::effects::ShaderModuleId::new(901).unwrap(),
            relative_path: "invalid.frag".into(),
            source: "void main() {}".to_owned(),
            uniforms: Vec::new(),
        });
    assert!(
        server
            .state
            .trusted_effect_registry
            .reload(invalid_manifest, |_| Ok(()))
            .is_err()
    );
    let current_generation = server.state.trusted_effect_registry.current();
    assert!(Arc::ptr_eq(&active_generation, &current_generation));
    assert_eq!(
        server.material_program_selection_snapshot(),
        selection_before_invalid_reload
    );
    let mut blur_policy = server.state.blur_assignment.config();
    blur_policy.applications.wayland = crate::blur_policy::BlurApplicationMode::RulesOnly;
    server
        .state
        .blur_assignment
        .replace_config(blur_policy)
        .unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let connection = Connection::from_socket(UnixStream::connect(&socket_path)?)?;
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let manager: client_ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
        let (surface, xdg_surface, toplevel) =
            create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 8, 8)?;
        let effect = manager.get_background_effect(&surface, &qh, ());
        let region = compositor.create_region(&qh, ());
        region.add(3, 4, 10, 10);
        effect.set_blur_region(Some(&region));
        region.destroy();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;

        let before_commit = capture_effect_scene(&commands);
        assert!(
            before_commit.is_empty(),
            "pending state must not be visible"
        );
        assert_eq!(before_commit.summary.visible_instance_count, 0);
        assert_eq!(
            crate::compositor::direct_scanout_scene_rejection_for_effects(before_commit.summary),
            None,
            "selecting a program without a visible blur assignment must not block Direct Scanout"
        );

        set_material_program_parameters(&commands, parameter_configuration.clone())?;
        let parameters_without_assignment = capture_effect_scene(&commands);
        assert!(parameters_without_assignment.is_empty());
        assert_eq!(
            crate::compositor::direct_scanout_scene_rejection_for_effects(
                parameters_without_assignment.summary
            ),
            None,
            "parameter configuration alone must not block Direct Scanout"
        );

        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;

        let after_commit = capture_effect_scene(&commands);
        assert_eq!(after_commit.instances.len(), 1);
        assert_eq!(
            after_commit.instances[0].program,
            EffectProgramId::new(900).unwrap()
        );
        assert_eq!(
            after_commit.instances[0].frame_demand,
            EffectFrameDemand::OnDamage
        );
        assert_eq!(after_commit.instances[0].parameter_block.values().len(), 1);
        assert_eq!(
            after_commit.instances[0].parameter_block.values()[0].value,
            EffectUniformValue::Float(0.82)
        );
        assert_eq!(
            crate::compositor::direct_scanout_scene_rejection_for_effects(after_commit.summary),
            Some(crate::compositor::DirectScanoutSceneRejection::EffectRequiresComposition)
        );
        set_material_program_parameters(
            &commands,
            crate::material_program::MaterialProgramParameterConfiguration {
                overrides: BTreeMap::new(),
                ..parameter_configuration.clone()
            },
        )?;
        let restored_default = capture_effect_scene(&commands);
        assert_eq!(
            restored_default.instances[0].parameter_block.values()[0].value,
            EffectUniformValue::Float(0.65)
        );
        assert_eq!(
            crate::compositor::direct_scanout_scene_rejection_for_effects(restored_default.summary),
            Some(crate::compositor::DirectScanoutSceneRejection::EffectRequiresComposition)
        );
        set_material_program(&commands, crate::effects::BUILTIN_BACKGROUND_BLUR_NAME)?;
        let selected_builtin = capture_effect_scene(&commands);
        assert_eq!(selected_builtin.instances.len(), 1);
        assert_eq!(
            selected_builtin.instances[0].program,
            crate::effects::builtin_background_blur_program_id()
        );
        set_material_program(&commands, "glass.liquid")?;
        let restored_custom = capture_effect_scene(&commands);
        assert_eq!(
            restored_custom.instances[0].program,
            EffectProgramId::new(900).unwrap()
        );
        assert_eq!(
            restored_custom.instances[0].parameter_block.values()[0].value,
            EffectUniformValue::Float(0.65)
        );
        assert_eq!(after_commit.instances[0].region.rects().len(), 1);
        assert_eq!(
            after_commit.instances[0].region.rects()[0],
            EffectRect::new(75, 76, 5, 4).unwrap()
        );

        effect.set_blur_region(None);
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        assert_eq!(capture_effect_scene(&commands).instances.len(), 1);
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let cleared = capture_effect_scene(&commands);
        assert!(cleared.is_empty());
        assert_eq!(
            crate::compositor::direct_scanout_scene_rejection_for_effects(cleared.summary),
            None,
            "Direct Scanout eligibility must recover when the semantic blur assignment disappears"
        );

        let region = compositor.create_region(&qh, ());
        region.add(0, 0, 2, 2);
        effect.set_blur_region(Some(&region));
        region.destroy();
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        assert_eq!(capture_effect_scene(&commands).instances.len(), 1);

        effect.destroy();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        assert_eq!(capture_effect_scene(&commands).instances.len(), 1);
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        assert!(capture_effect_scene(&commands).is_empty());

        let effect = manager.get_background_effect(&surface, &qh, ());
        let region = compositor.create_region(&qh, ());
        region.add(0, 0, 2, 2);
        effect.set_blur_region(Some(&region));
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        assert_eq!(capture_effect_scene(&commands).instances.len(), 1);
        toplevel.destroy();
        xdg_surface.destroy();
        surface.destroy();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        assert!(capture_effect_scene(&commands).is_empty());
        Ok(())
    })();

    stop_controllable_test_server(commands, server_thread);
    let _ = std::fs::remove_dir_all(persistence_directory);
    result.unwrap();
}

#[test]
fn production_resolution_assigns_public_surface_and_trusted_visual_group_scopes() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    commands
        .send(ServerCommand::AuthorizeAstreaShellPid(std::process::id()))
        .unwrap();
    wait_for_server_commands(&commands);

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let connection = Connection::from_socket(UnixStream::connect(&socket_path)?)?;
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let background_manager: client_ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let effects_manager: crate::astrea_effects::client::astrea_effects_manager_v1::AstreaEffectsManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
        let (surface, _xdg_surface, _toplevel) =
            create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 80, 60)?;

        let public_effect = background_manager.get_background_effect(&surface, &qh, ());
        let region = compositor.create_region(&qh, ());
        region.add(0, 0, 80, 60);
        public_effect.set_blur_region(Some(&region));
        region.destroy();

        let trusted_background =
            effects_manager.get_surface_effect(&surface, "background".to_string(), &qh, ());
        let trusted_content =
            effects_manager.get_surface_effect(&surface, "content".to_string(), &qh, ());
        let trusted_foreground =
            effects_manager.get_surface_effect(&surface, "foreground".to_string(), &qh, ());
        for effect in [&trusted_background, &trusted_content, &trusted_foreground] {
            effect.set_program("system.background_blur".to_string());
        }

        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        for effect in [&trusted_background, &trusted_content, &trusted_foreground] {
            effect.set_enabled(1);
        }
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let scene = capture_effect_scene(&commands);

        assert_eq!(
            scene
                .instances
                .iter()
                .filter(|instance| instance.anchor_scope == EffectAnchorScope::Surface)
                .count(),
            1
        );
        assert_eq!(
            scene
                .instances
                .iter()
                .filter(|instance| instance.anchor_scope == EffectAnchorScope::VisualGroup)
                .count(),
            3
        );
        let public_anchor = scene
            .instances
            .iter()
            .find(|instance| instance.anchor_scope == EffectAnchorScope::Surface)
            .map(|instance| instance.anchor)
            .expect("public background effect must resolve");
        let public_surface_id = match public_anchor {
            EffectAnchor::BeforeSurface(surface_id) => surface_id,
            other => panic!("public background effect must resolve before its surface: {other:?}"),
        };
        for anchor in [
            EffectAnchor::BeforeSurface(public_surface_id),
            EffectAnchor::ReplaceSurface(public_surface_id),
            EffectAnchor::AfterSurface(public_surface_id),
        ] {
            assert_eq!(
                scene
                    .instances
                    .iter()
                    .filter(|instance| {
                        instance.anchor == anchor
                            && instance.anchor_scope == EffectAnchorScope::VisualGroup
                    })
                    .count(),
                1,
                "trusted slot anchor must use visual-group scope: {anchor:?}"
            );
        }
        Ok(())
    })();

    stop_controllable_test_server(commands, server_thread);
    result.unwrap();
}

#[test]
fn production_wayland_auto_blur_uses_committed_xdg_geometry_and_client_blur_stays_surface_local() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    server.state.set_background_effect_enabled(true);
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    commands
        .send(ServerCommand::AuthorizeAstreaShellPid(std::process::id()))
        .unwrap();
    wait_for_server_commands(&commands);

    let mut blur_policy = crate::blur_policy::BlurPolicyConfig::default();
    blur_policy.enabled = true;
    blur_policy.applications.wayland = crate::blur_policy::BlurApplicationMode::Auto;
    replace_blur_policy_config(&commands, blur_policy.clone());

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let connection = Connection::from_socket(UnixStream::connect(&socket_path)?)?;
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let background_manager: client_ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let coverage_manager: client_coverage_manager::AstreaBackgroundEffectCoverageManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
        let (surface, xdg_surface, _toplevel) =
            create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 120, 100)?;
        let coverage = coverage_manager.get_coverage(&surface, &qh, ());
        coverage.set_rounded_rect(0.5, 0.25, 72.0, 34.0, 17.0);
        coverage.set_triangle(35.0, 40.5, 47.0, 40.5, 41.0, 48.0);
        xdg_surface.set_window_geometry(10, 12, 100, 70);
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;

        let raw_surface_origin = crate::compositor::render::FIRST_SURFACE_OFFSET;
        let expected_window_bounds =
            EffectRect::new(raw_surface_origin.0, raw_surface_origin.1, 100, 70).unwrap();
        let auto_scene = capture_effect_scene(&commands);
        assert_eq!(auto_scene.instances.len(), 1);
        assert_eq!(
            auto_scene.instances[0].anchor_scope,
            EffectAnchorScope::VisualGroup
        );
        assert_eq!(
            auto_scene.instances[0].region.rects(),
            &[expected_window_bounds],
            "automatic blur must use the committed XDG window geometry translated from the raw root origin"
        );
        assert_eq!(
            auto_scene.instances[0].target_bounds,
            expected_window_bounds
        );
        assert_eq!(
            auto_scene.instances[0].coverage, None,
            "automatic blur does not consume private client coverage"
        );

        let public_effect = background_manager.get_background_effect(&surface, &qh, ());
        let client_region = compositor.create_region(&qh, ());
        client_region.add(0, 0, 5, 5);
        public_effect.set_blur_region(Some(&client_region));
        client_region.destroy();
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;

        let expected_client_bounds =
            EffectRect::new(raw_surface_origin.0 - 10, raw_surface_origin.1 - 12, 5, 5).unwrap();
        let client_scene = capture_effect_scene(&commands);
        assert_eq!(client_scene.instances.len(), 1);
        assert_eq!(
            client_scene.instances[0].anchor_scope,
            EffectAnchorScope::Surface
        );
        assert_eq!(
            client_scene.instances[0].region.rects(),
            &[expected_client_bounds],
            "a client region remains rooted at the raw wl_surface origin"
        );
        assert_eq!(
            client_scene.instances[0].target_bounds, expected_client_bounds,
            "analytic coverage must not broaden the public coarse work bounds"
        );
        let client_coverage = client_scene.instances[0]
            .coverage
            .as_ref()
            .expect("committed client coverage refines the public client blur");
        let rounded_rect = client_coverage.rounded_rect.unwrap();
        assert_eq!(rounded_rect.x, f64::from(expected_client_bounds.x) + 0.5);
        assert_eq!(rounded_rect.y, f64::from(expected_client_bounds.y) + 0.25);
        assert_eq!(rounded_rect.width, 72.0);
        assert_eq!(rounded_rect.height, 34.0);
        assert!(client_coverage.triangle.is_some());

        coverage.set_rounded_rect(0.5, 0.25, 72.0, 34.0, 17.0);
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let identical_coverage_scene = capture_effect_scene(&commands);
        assert_eq!(identical_coverage_scene.signature, client_scene.signature);
        assert_eq!(identical_coverage_scene.generation, client_scene.generation);

        let client_signature = identical_coverage_scene.instances[0].signature;
        coverage.set_rounded_rect(0.5, 0.25, 72.0, 34.0, 16.0);
        let pending_coverage_scene = capture_effect_scene(&commands);
        assert_eq!(
            pending_coverage_scene.instances[0].coverage,
            identical_coverage_scene.instances[0].coverage,
            "a private coverage request remains pending until wl_surface.commit"
        );
        assert_eq!(
            pending_coverage_scene.instances[0].signature,
            client_signature
        );

        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let committed_radius_scene = capture_effect_scene(&commands);
        assert_eq!(
            committed_radius_scene.instances[0]
                .coverage
                .as_ref()
                .unwrap()
                .rounded_rect
                .unwrap()
                .radius,
            16.0
        );
        assert_ne!(
            committed_radius_scene.instances[0].signature,
            client_signature
        );
        assert_ne!(committed_radius_scene.generation, client_scene.generation);

        coverage.clear();
        let pending_clear_scene = capture_effect_scene(&commands);
        assert!(pending_clear_scene.instances[0].coverage.is_some());
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let cleared_scene = capture_effect_scene(&commands);
        assert_eq!(cleared_scene.instances[0].coverage, None);

        coverage.set_rounded_rect(-2.5, -1.25, 72.0, 34.0, 17.0);
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let negative_local_scene = capture_effect_scene(&commands);
        assert_eq!(
            negative_local_scene.instances[0]
                .coverage
                .as_ref()
                .unwrap()
                .rounded_rect
                .unwrap()
                .x,
            f64::from(expected_client_bounds.x) - 2.5
        );
        assert_eq!(
            negative_local_scene.instances[0].target_bounds,
            expected_client_bounds
        );
        coverage.destroy();
        assert!(negative_local_scene.instances[0].coverage.is_some());
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        assert_eq!(capture_effect_scene(&commands).instances[0].coverage, None);

        public_effect.set_blur_region(None);
        xdg_surface.set_window_geometry(0, -24, 944, 526);
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;

        let expected_negative_offset_bounds =
            EffectRect::new(raw_surface_origin.0, raw_surface_origin.1, 120, 100).unwrap();
        let negative_offset_scene = capture_effect_scene(&commands);
        assert_eq!(negative_offset_scene.instances.len(), 1);
        assert_eq!(
            negative_offset_scene.instances[0].region.rects(),
            &[expected_negative_offset_bounds],
            "explicit geometry is clamped to the committed tree before becoming the logical candidate"
        );
        assert_ne!(
            negative_offset_scene.instances[0].signature, auto_scene.instances[0].signature,
            "a committed XDG geometry change must alter the resolved effect signature"
        );
        assert_eq!(
            negative_offset_scene.instances[0].coverage, None,
            "the automatic/rule assignment remains unrefined even while client coverage is stored"
        );
        blur_policy.enabled = false;
        replace_blur_policy_config(&commands, blur_policy);
        assert!(
            capture_effect_scene(&commands).is_empty(),
            "private coverage without a committed public blur must not create an effect"
        );
        Ok(())
    })();

    stop_controllable_test_server(commands, server_thread);
    result.unwrap();
}

#[test]
fn synchronized_child_coverage_is_cached_merged_and_published_with_parent() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    server.state.set_background_effect_enabled(true);
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    commands
        .send(ServerCommand::AuthorizeAstreaShellPid(std::process::id()))
        .unwrap();
    wait_for_server_commands(&commands);

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let connection = Connection::from_socket(UnixStream::connect(&socket_path)?)?;
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let subcompositor: client_wl_subcompositor::WlSubcompositor =
            globals.bind(&qh, 1..=1, ())?;
        let background_manager: client_ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let coverage_manager: client_coverage_manager::AstreaBackgroundEffectCoverageManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
        let (parent, _xdg_surface, _toplevel) =
            create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 100, 80)?;
        let child = compositor.create_surface(&qh, ());
        let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
        subsurface.set_position(4, 6);
        let public_effect = background_manager.get_background_effect(&child, &qh, ());
        let region = compositor.create_region(&qh, ());
        region.add(0, 0, 64, 48);
        public_effect.set_blur_region(Some(&region));
        region.destroy();
        let coverage = coverage_manager.get_coverage(&child, &qh, ());
        coverage.set_rounded_rect(0.25, 0.5, 52.0, 32.0, 12.0);

        commit_test_buffered_surface(&child, &shm, &qh, 64, 48)?;
        parent.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let initial_scene = capture_effect_scene(&commands);
        let initial = initial_scene
            .instances
            .iter()
            .find(|instance| instance.coverage.is_some())
            .expect("parent publication makes synchronized child coverage visible");
        assert_eq!(
            initial
                .coverage
                .as_ref()
                .unwrap()
                .rounded_rect
                .unwrap()
                .radius,
            12.0
        );

        coverage.set_rounded_rect(0.25, 0.5, 52.0, 32.0, 16.0);
        child.commit();
        coverage.set_rounded_rect(0.25, 0.5, 52.0, 32.0, 14.0);
        child.commit();
        child.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let cached_scene = capture_effect_scene(&commands);
        assert_eq!(cached_scene.signature, initial_scene.signature);
        let cached = cached_scene
            .instances
            .iter()
            .find(|instance| instance.coverage.is_some())
            .expect("cached child coverage remains in the scene");
        assert_eq!(
            cached.coverage.as_ref(),
            initial.coverage.as_ref(),
            "synchronized child changes remain cached until parent publication"
        );

        parent.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let published_scene = capture_effect_scene(&commands);
        let published = published_scene
            .instances
            .iter()
            .find(|instance| instance.coverage.is_some())
            .expect("published child blur retains its coverage");
        assert_eq!(
            published
                .coverage
                .as_ref()
                .unwrap()
                .rounded_rect
                .unwrap()
                .radius,
            14.0
        );
        assert_ne!(published_scene.signature, initial_scene.signature);

        coverage.destroy();
        child.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        assert_eq!(
            capture_effect_scene(&commands).signature,
            published_scene.signature
        );
        parent.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let destroyed_scene = capture_effect_scene(&commands);
        assert!(
            destroyed_scene
                .instances
                .iter()
                .all(|instance| instance.coverage.is_none())
        );
        Ok(())
    })();

    stop_controllable_test_server(commands, server_thread);
    result.unwrap();
}

#[test]
fn production_wayland_auto_blur_follows_fullscreen_and_restore_presentation() {
    const MIDPOINT_NANOS: u64 = 125_000_000;
    const SETTLED_NANOS: u64 = 500_000_000;

    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    server.state.set_background_effect_enabled(true);
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    replace_blur_policy_config(&commands, crate::blur_policy::BlurPolicyConfig::default());

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let connection = Connection::from_socket(UnixStream::connect(&socket_path)?)?;
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
        let (surface, xdg_surface, _toplevel) =
            create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 120, 100)?;
        xdg_surface.set_window_geometry(10, 12, 100, 70);
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;

        let setup_scene = capture_effect_scene(&commands);
        let setup_blur = sole_background_blur(&setup_scene);
        let root_surface_id = match setup_blur.anchor {
            EffectAnchor::BeforeSurface(surface_id) => surface_id,
            other => panic!("automatic background blur should anchor before its root: {other:?}"),
        };
        let settle_root_presentation = |frame_id| {
            commands
                .send(ServerCommand::PublishFocusedPresentationAfter {
                    frame_id,
                    elapsed_nanos: SETTLED_NANOS,
                })
                .unwrap();
            wait_for_server_commands(&commands);
            commands
                .send(ServerCommand::CancelRootPresentationProperties { root_surface_id })
                .unwrap();
            commands
                .send(ServerCommand::PublishTestPresentationAt {
                    frame_id,
                    at: AnimationTime::from_nanos(u64::MAX),
                })
                .unwrap();
            wait_for_server_commands(&commands);
        };
        settle_root_presentation(90);

        let normal_scene = capture_effect_scene(&commands);
        let normal_blur = sole_background_blur(&normal_scene);
        assert_eq!(normal_blur.id, setup_blur.id);
        assert_eq!(normal_blur.anchor_scope, EffectAnchorScope::VisualGroup);
        let stable_id = normal_blur.id;
        let stable_program = normal_blur.program;
        let stable_anchor = normal_blur.anchor;
        let normal_region = normal_blur.region.clone();

        commands
            .send(ServerCommand::ToggleFullscreenFocused)
            .unwrap();
        wait_for_server_commands(&commands);
        assert_eq!(
            capture_fullscreen_render_plan_metrics(&commands).owner_root_surface_id,
            Some(root_surface_id),
            "fullscreen ownership should update before geometry animation sampling"
        );

        let assert_stable_blur = |scene: &ResolvedEffectScene| {
            let blur = sole_background_blur(scene);
            assert_eq!(
                blur.id, stable_id,
                "blur instance identity should be stable"
            );
            assert_eq!(
                blur.program, stable_program,
                "blur program should be stable"
            );
            assert_eq!(blur.anchor, stable_anchor, "blur anchor should be stable");
            assert_eq!(blur.anchor_scope, EffectAnchorScope::VisualGroup);
        };

        let fullscreen_immediate_scene = capture_effect_scene(&commands);
        assert_stable_blur(&fullscreen_immediate_scene);
        let fullscreen_region = sole_background_blur(&fullscreen_immediate_scene)
            .region
            .clone();
        assert_ne!(fullscreen_region, normal_region);

        let fullscreen_start = capture_focused_presentation_effect_scene_after(&commands, 0);
        assert_stable_blur(&fullscreen_start);
        let fullscreen_start_region = sole_background_blur(&fullscreen_start).region.clone();

        let fullscreen_midpoint =
            capture_focused_presentation_effect_scene_after(&commands, MIDPOINT_NANOS);
        assert_stable_blur(&fullscreen_midpoint);
        let fullscreen_midpoint_region = &sole_background_blur(&fullscreen_midpoint).region;
        assert_ne!(
            fullscreen_midpoint_region, &fullscreen_start_region,
            "fullscreen midpoint blur should move from the transition start geometry"
        );
        assert_ne!(
            fullscreen_midpoint_region, &normal_region,
            "fullscreen midpoint blur should move away from the old window rectangle"
        );
        assert_ne!(
            fullscreen_midpoint_region, &fullscreen_region,
            "fullscreen midpoint blur should not jump to the final fullscreen rectangle"
        );

        let fullscreen_settled_presentation =
            capture_focused_presentation_effect_scene_after(&commands, SETTLED_NANOS);
        assert_stable_blur(&fullscreen_settled_presentation);
        assert_eq!(
            sole_background_blur(&fullscreen_settled_presentation).region,
            fullscreen_region,
            "presentation-mapped blur should reach the final fullscreen geometry"
        );
        settle_root_presentation(100);
        let fullscreen_settled = capture_effect_scene(&commands);
        assert_stable_blur(&fullscreen_settled);
        assert_eq!(
            sole_background_blur(&fullscreen_settled).region,
            fullscreen_region,
            "settled fullscreen blur should cover the final fullscreen geometry"
        );
        assert!(
            capture_direct_scanout_candidate(&commands).is_err(),
            "an alpha-capable fullscreen window with active blur requires composition"
        );

        commands
            .send(ServerCommand::ToggleFullscreenFocused)
            .unwrap();
        wait_for_server_commands(&commands);
        assert_eq!(
            capture_fullscreen_render_plan_metrics(&commands).owner_root_surface_id,
            None,
            "restoring should clear fullscreen ownership before its geometry animation"
        );

        let restore_immediate_scene = capture_effect_scene(&commands);
        assert_stable_blur(&restore_immediate_scene);
        let restore_region = sole_background_blur(&restore_immediate_scene)
            .region
            .clone();
        assert_eq!(restore_region, normal_region);

        let restore_start = capture_focused_presentation_effect_scene_after(&commands, 0);
        assert_stable_blur(&restore_start);
        let restore_start_region = sole_background_blur(&restore_start).region.clone();

        let restore_midpoint =
            capture_focused_presentation_effect_scene_after(&commands, MIDPOINT_NANOS);
        assert_stable_blur(&restore_midpoint);
        let restore_midpoint_region = &sole_background_blur(&restore_midpoint).region;
        assert_ne!(
            restore_midpoint_region, &restore_start_region,
            "restore midpoint blur should move from the transition start geometry"
        );
        assert_ne!(
            restore_midpoint_region, &fullscreen_region,
            "restore midpoint blur should move away from the fullscreen rectangle"
        );
        assert_ne!(
            restore_midpoint_region, &normal_region,
            "restore midpoint blur should not jump to the final normal rectangle"
        );

        let restore_settled_presentation =
            capture_focused_presentation_effect_scene_after(&commands, SETTLED_NANOS);
        assert_stable_blur(&restore_settled_presentation);
        assert_eq!(
            sole_background_blur(&restore_settled_presentation).region,
            normal_region,
            "presentation-mapped blur should return to the normal window geometry"
        );
        settle_root_presentation(101);
        let normal_settled = capture_effect_scene(&commands);
        assert_stable_blur(&normal_settled);
        assert_eq!(
            sole_background_blur(&normal_settled).region,
            normal_region,
            "settled restore should return the automatic blur to its normal geometry"
        );
        Ok(())
    })();

    stop_controllable_test_server(commands, server_thread);
    result.unwrap();
}

#[test]
fn production_wayland_auto_blur_is_stable_across_transient_opaque_region_changes() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    server.state.set_background_effect_enabled(true);
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let blur_policy = crate::blur_policy::BlurPolicyConfig {
        enabled: true,
        applications: crate::blur_policy::BlurApplicationPolicy {
            wayland: crate::blur_policy::BlurApplicationMode::Auto,
            ..crate::blur_policy::BlurApplicationPolicy::default()
        },
        ..crate::blur_policy::BlurPolicyConfig::default()
    };
    replace_blur_policy_config(&commands, blur_policy);

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let connection = Connection::from_socket(UnixStream::connect(&socket_path)?)?;
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
        let (surface, xdg_surface, _toplevel) =
            create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 120, 100)?;
        xdg_surface.set_window_geometry(0, 0, 120, 100);
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;

        let initial_scene = capture_effect_scene(&commands);
        assert_eq!(initial_scene.instances.len(), 1);
        assert_eq!(
            initial_scene.instances[0].anchor_scope,
            EffectAnchorScope::VisualGroup
        );
        let initial_region = initial_scene.instances[0].region.clone();
        let initial_signature = initial_scene.instances[0].signature;

        let spotlight_opaque = compositor.create_region(&qh, ());
        spotlight_opaque.add(20, 20, 60, 40);
        surface.set_opaque_region(Some(&spotlight_opaque));
        spotlight_opaque.destroy();
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;

        let spotlight_scene = capture_effect_scene(&commands);
        assert_eq!(spotlight_scene.instances[0].region, initial_region);
        assert_eq!(spotlight_scene.instances[0].signature, initial_signature);

        let moved_spotlight_opaque = compositor.create_region(&qh, ());
        moved_spotlight_opaque.add(30, 10, 25, 25);
        surface.set_opaque_region(Some(&moved_spotlight_opaque));
        moved_spotlight_opaque.destroy();
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;

        let reshaped_spotlight_scene = capture_effect_scene(&commands);
        assert_eq!(reshaped_spotlight_scene.instances[0].region, initial_region);
        assert_eq!(
            reshaped_spotlight_scene.instances[0].signature,
            initial_signature
        );

        let empty_opaque = compositor.create_region(&qh, ());
        surface.set_opaque_region(Some(&empty_opaque));
        empty_opaque.destroy();
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;

        let after_empty_region_scene = capture_effect_scene(&commands);
        assert_eq!(after_empty_region_scene.instances[0].region, initial_region);
        assert_eq!(
            after_empty_region_scene.instances[0].signature,
            initial_signature
        );

        surface.set_opaque_region(None);
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;

        let after_null_region_scene = capture_effect_scene(&commands);
        assert_eq!(after_null_region_scene.instances[0].region, initial_region);
        assert_eq!(
            after_null_region_scene.instances[0].signature,
            initial_signature
        );
        Ok(())
    })();

    stop_controllable_test_server(commands, server_thread);
    result.unwrap();
}

#[test]
fn public_child_effects_follow_production_scene_order_not_identifiers() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let mut blur_policy = server.state.blur_assignment.config();
    blur_policy.applications.wayland = crate::blur_policy::BlurApplicationMode::RulesOnly;
    blur_policy
        .window_rules
        .push(crate::blur_policy::BlurWindowRule {
            name: "visual-group-root".to_string(),
            matcher: crate::blur_policy::BlurWindowMatch {
                app_id: Some("^org\\.example\\.visual-group$".to_string()),
                title: None,
                backend: Some(crate::blur_policy::BlurBackend::Wayland),
            },
            action: crate::blur_policy::BlurRuleAction::Enable,
        });
    server
        .state
        .blur_assignment
        .replace_config(blur_policy)
        .unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let connection = Connection::from_socket(UnixStream::connect(&socket_path)?)?;
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let subcompositor: client_wl_subcompositor::WlSubcompositor =
            globals.bind(&qh, 1..=1, ())?;
        let background_manager: client_ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
        let (parent, _xdg_surface, toplevel) =
            create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 100, 80)?;
        toplevel.set_app_id("org.example.visual-group".to_string());
        let child_a = compositor.create_surface(&qh, ());
        let child_a_subsurface = subcompositor.get_subsurface(&child_a, &parent, &qh, ());
        child_a_subsurface.set_position(0, 0);
        let child_b = compositor.create_surface(&qh, ());
        let child_b_subsurface = subcompositor.get_subsurface(&child_b, &parent, &qh, ());
        child_b_subsurface.set_position(0, 0);

        let effect_a = background_manager.get_background_effect(&child_a, &qh, ());
        let region_a = compositor.create_region(&qh, ());
        region_a.add(0, 0, 80, 60);
        effect_a.set_blur_region(Some(&region_a));
        region_a.destroy();
        let effect_b = background_manager.get_background_effect(&child_b, &qh, ());
        let region_b = compositor.create_region(&qh, ());
        region_b.add(0, 0, 80, 60);
        effect_b.set_blur_region(Some(&region_b));
        region_b.destroy();

        commit_test_buffered_surface(&child_a, &shm, &qh, 80, 60)?;
        commit_test_buffered_surface(&child_b, &shm, &qh, 80, 60)?;
        parent.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        child_a_subsurface.place_above(&child_b);
        parent.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        wait_for_server_commands(&commands);

        let scene = capture_effect_scene(&commands);
        let effect_order = scene
            .instances
            .iter()
            .map(|instance| match instance.anchor {
                EffectAnchor::BeforeSurface(surface_id)
                | EffectAnchor::ReplaceSurface(surface_id)
                | EffectAnchor::AfterSurface(surface_id) => surface_id,
                EffectAnchor::OutputPostProcess => 0,
            })
            .collect::<Vec<_>>();
        assert_eq!(effect_order.len(), 3);
        let root_effect = scene
            .instances
            .iter()
            .find(|instance| instance.anchor_scope == EffectAnchorScope::VisualGroup)
            .expect("desktop rule must synthesize a visual-group background effect");
        assert_eq!(root_effect.anchor_scope, EffectAnchorScope::VisualGroup);
        assert_eq!(root_effect.scene_order.surface_order, 0);
        assert!(root_effect.visual_group.is_some());
        assert_eq!(
            scene
                .instances
                .iter()
                .filter(|instance| instance.anchor != root_effect.anchor)
                .map(|instance| instance.scene_order.surface_order)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        let mut identifier_order = effect_order.clone();
        identifier_order.sort_unstable();
        assert_ne!(effect_order, identifier_order);
        assert!(
            scene
                .instances
                .iter()
                .skip(1)
                .all(|instance| { instance.anchor_scope == EffectAnchorScope::Surface })
        );
        Ok(())
    })();

    stop_controllable_test_server(commands, server_thread);
    result.unwrap();
}
