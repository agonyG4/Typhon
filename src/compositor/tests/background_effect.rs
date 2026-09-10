use super::*;
use crate::effects::EffectRect;

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

        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;

        let after_commit = capture_effect_scene(&commands);
        assert_eq!(after_commit.instances.len(), 1);
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
        assert!(capture_effect_scene(&commands).is_empty());

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
                backend: Some("wayland".to_string()),
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
