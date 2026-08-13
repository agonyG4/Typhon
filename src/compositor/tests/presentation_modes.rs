use super::*;

#[test]
fn presentation_mode_globals_follow_the_capability_baseline() {
    let safe_socket_name = unique_socket_name();
    let safe_server = OwnCompositorServer::bind(&safe_socket_name).unwrap();
    let safe_socket_path = runtime_socket_path(&safe_socket_name);
    let (safe_running, safe_thread) = spawn_test_server(safe_server);
    let safe_globals = read_registry_globals(&safe_socket_path).unwrap();
    stop_test_server(safe_running, safe_thread);

    assert!(!safe_globals.contains(&"wp_tearing_control_manager_v1".to_string()));
    assert!(!safe_globals.contains(&"wp_content_type_manager_v1".to_string()));

    let qualified_socket_name = unique_socket_name();
    let qualified_server = OwnCompositorServer::bind_native_base(&qualified_socket_name).unwrap();
    let qualified_socket_path = runtime_socket_path(&qualified_socket_name);
    let (qualified_running, qualified_thread) = spawn_test_server(qualified_server);
    let qualified_globals = read_registry_globals(&qualified_socket_path).unwrap();
    stop_test_server(qualified_running, qualified_thread);

    assert!(qualified_globals.contains(&"wp_tearing_control_manager_v1".to_string()));
    assert!(qualified_globals.contains(&"wp_content_type_manager_v1".to_string()));
}

#[test]
fn wayland_presentation_mode_lifecycle_is_double_buffered_and_recreatable() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let stream = UnixStream::connect(&socket_path)?;
        let connection = Connection::from_socket(stream)?;
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let tearing_manager: client_wp_tearing_control_manager_v1::WpTearingControlManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let content_manager: client_wp_content_type_manager_v1::WpContentTypeManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let surface = compositor.create_surface(&qh, ());
        let surface_id = surface.id().protocol_id();
        let tearing = tearing_manager.get_tearing_control(&surface, &qh, ());
        let content = content_manager.get_surface_content_type(&surface, &qh, ());
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;

        tearing.set_presentation_hint(client_wp_tearing_control_v1::PresentationHint::Async);
        content.set_content_type(client_wp_content_type_v1::Type::Photo);
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;

        let (current, pending) =
            capture_surface_presentation_metadata(&commands, surface_id).unwrap();
        assert_eq!(current, SurfacePresentationMetadata::default());
        assert_eq!(pending.hint, SurfacePresentationHint::Async);
        assert_eq!(pending.content_type, SurfaceContentType::Photo);

        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let (current, pending) =
            capture_surface_presentation_metadata(&commands, surface_id).unwrap();
        assert_eq!(current.hint, SurfacePresentationHint::Async);
        assert_eq!(current.content_type, SurfaceContentType::Photo);
        assert_eq!(pending, current);

        content.set_content_type(client_wp_content_type_v1::Type::Video);
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let (current, _) = capture_surface_presentation_metadata(&commands, surface_id).unwrap();
        assert_eq!(current.content_type, SurfaceContentType::Video);

        content.set_content_type(client_wp_content_type_v1::Type::Game);
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let (current, _) = capture_surface_presentation_metadata(&commands, surface_id).unwrap();
        assert_eq!(current.content_type, SurfaceContentType::Game);

        tearing.destroy();
        content.destroy();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let (current, pending) =
            capture_surface_presentation_metadata(&commands, surface_id).unwrap();
        assert_eq!(current.hint, SurfacePresentationHint::Async);
        assert_eq!(current.content_type, SurfaceContentType::Game);
        assert_eq!(pending, SurfacePresentationMetadata::default());

        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let (current, _) = capture_surface_presentation_metadata(&commands, surface_id).unwrap();
        assert_eq!(current, SurfacePresentationMetadata::default());

        let tearing = tearing_manager.get_tearing_control(&surface, &qh, ());
        let content = content_manager.get_surface_content_type(&surface, &qh, ());
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        tearing.set_presentation_hint(client_wp_tearing_control_v1::PresentationHint::Vsync);
        content.set_content_type(client_wp_content_type_v1::Type::Game);
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;

        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let (current, _) = capture_surface_presentation_metadata(&commands, surface_id).unwrap();
        assert_eq!(current.hint, SurfacePresentationHint::Vsync);
        assert_eq!(current.content_type, SurfaceContentType::Game);

        content.set_content_type(client_wp_content_type_v1::Type::None);
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let (current, _) = capture_surface_presentation_metadata(&commands, surface_id).unwrap();
        assert_eq!(current.content_type, SurfaceContentType::None);

        surface.destroy();
        tearing.set_presentation_hint(client_wp_tearing_control_v1::PresentationHint::Async);
        content.set_content_type(client_wp_content_type_v1::Type::Photo);
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        Ok(())
    })();

    let _server = stop_controllable_test_server(commands, server_thread);
    result.unwrap();
}

#[test]
fn duplicate_tearing_control_is_the_exact_manager_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let connection = Connection::from_socket(UnixStream::connect(&socket_path)?)?;
        let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let manager: client_wp_tearing_control_manager_v1::WpTearingControlManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let surface = compositor.create_surface(&qh, ());
        let _control = manager.get_tearing_control(&surface, &qh, ());
        connection.roundtrip()?;
        let _duplicate = manager.get_tearing_control(&surface, &qh, ());
        connection.flush()?;
        let result = connection.roundtrip();
        assert!(result.is_err());
        let error = connection
            .protocol_error()
            .expect("tearing-control duplicate must be a wire error");
        assert_eq!(error.object_interface, "wp_tearing_control_manager_v1");
        assert_eq!(
            error.code,
            client_wp_tearing_control_manager_v1::Error::TearingControlExists as u32
        );
        Ok(())
    })();
    stop_test_server(running, server_thread);
    result.unwrap();
}

#[test]
fn duplicate_content_type_is_the_exact_manager_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let connection = Connection::from_socket(UnixStream::connect(&socket_path)?)?;
        let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let manager: client_wp_content_type_manager_v1::WpContentTypeManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let surface = compositor.create_surface(&qh, ());
        let _content_type = manager.get_surface_content_type(&surface, &qh, ());
        connection.roundtrip()?;
        let _duplicate = manager.get_surface_content_type(&surface, &qh, ());
        connection.flush()?;
        let result = connection.roundtrip();
        assert!(result.is_err());
        let error = connection
            .protocol_error()
            .expect("content-type duplicate must be a wire error");
        assert_eq!(error.object_interface, "wp_content_type_manager_v1");
        assert_eq!(
            error.code,
            client_wp_content_type_manager_v1::Error::AlreadyConstructed as u32
        );
        Ok(())
    })();
    stop_test_server(running, server_thread);
    result.unwrap();
}

#[test]
fn presentation_metadata_latches_through_synchronized_and_desynchronized_subsurfaces() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let connection = Connection::from_socket(UnixStream::connect(&socket_path)?)?;
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
        let subcompositor: client_wl_subcompositor::WlSubcompositor =
            globals.bind(&qh, 1..=1, ())?;
        let tearing_manager: client_wp_tearing_control_manager_v1::WpTearingControlManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let content_manager: client_wp_content_type_manager_v1::WpContentTypeManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

        let parent = compositor.create_surface(&qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
        let _toplevel = xdg_surface.get_toplevel(&qh, ());
        let child = compositor.create_surface(&qh, ());
        let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
        let child_id = child.id().protocol_id();
        parent.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;

        let tearing = tearing_manager.get_tearing_control(&child, &qh, ());
        let content = content_manager.get_surface_content_type(&child, &qh, ());
        tearing.set_presentation_hint(client_wp_tearing_control_v1::PresentationHint::Async);
        content.set_content_type(client_wp_content_type_v1::Type::None);
        commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let (current, pending_a) = capture_surface_presentation_metadata(&commands, child_id)
            .expect("synchronized child must remain registered");
        assert_eq!(current, SurfacePresentationMetadata::default());
        assert_eq!(pending_a.hint, SurfacePresentationHint::Async);
        assert_eq!(pending_a.content_type, SurfaceContentType::None);

        content.set_content_type(client_wp_content_type_v1::Type::Video);
        commit_test_buffered_surface(&child, &shm, &qh, 6, 6)?;
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let (current, pending_b) = capture_surface_presentation_metadata(&commands, child_id)
            .expect("synchronized child must remain registered");
        assert_eq!(current, SurfacePresentationMetadata::default());
        assert_eq!(pending_b.hint, SurfacePresentationHint::Async);
        assert_eq!(pending_b.content_type, SurfaceContentType::Video);

        parent.commit();
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let (current, pending) = capture_surface_presentation_metadata(&commands, child_id)
            .expect("synchronized child must remain registered");
        assert_eq!(current.hint, SurfacePresentationHint::Async);
        assert_eq!(current.content_type, SurfaceContentType::Video);
        assert_eq!(pending, current);

        subsurface.set_desync();
        tearing.set_presentation_hint(client_wp_tearing_control_v1::PresentationHint::Vsync);
        content.set_content_type(client_wp_content_type_v1::Type::Game);
        commit_test_buffered_surface(&child, &shm, &qh, 7, 7)?;
        connection.flush()?;
        queue.roundtrip(&mut RegistryTestState::default())?;
        let (current, pending) = capture_surface_presentation_metadata(&commands, child_id)
            .expect("desynchronized child must remain registered");
        assert_eq!(current.hint, SurfacePresentationHint::Vsync);
        assert_eq!(current.content_type, SurfaceContentType::Game);
        assert_eq!(pending, current);
        Ok(())
    })();
    stop_controllable_test_server(commands, server_thread);
    result.unwrap();
}
