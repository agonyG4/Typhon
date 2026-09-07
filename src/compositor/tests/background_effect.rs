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
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
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
