use super::*;
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1 as client_zxdg_decoration_manager_v1,
    zxdg_toplevel_decoration_v1 as client_zxdg_toplevel_decoration_v1,
};

struct DecorationClient {
    connection: Connection,
    queue: EventQueue<RegistryTestState>,
    surface: client_wl_surface::WlSurface,
    xdg_surface: client_xdg_surface::XdgSurface,
    manager: client_zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    decoration: client_zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
    state: RegistryTestState,
}

impl DecorationClient {
    fn connect(
        socket_path: &PathBuf,
        commands: &Sender<ServerCommand>,
        initial_mode: client_zxdg_toplevel_decoration_v1::Mode,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let connection = Connection::from_socket(UnixStream::connect(socket_path)?)?;
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ())?;
        let manager: client_zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let (surface, xdg_surface, toplevel) =
            create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
        let decoration = manager.get_toplevel_decoration(&toplevel, &qh, ());
        decoration.set_mode(initial_mode);
        surface.commit();
        connection.flush()?;

        let mut state = RegistryTestState {
            suppress_xdg_surface_ack: true,
            suppress_xdg_surface_commit: true,
            ..RegistryTestState::default()
        };
        wait_for_server_commands(commands);
        queue.roundtrip(&mut state)?;
        let initial_serial = *state
            .surface_configure_serials
            .last()
            .ok_or("initial xdg configure was not observed")?;
        xdg_surface.ack_configure(initial_serial);
        commit_registered_initial_xdg_test_buffer(&xdg_surface);
        connection.flush()?;
        wait_for_server_commands(commands);
        queue.roundtrip(&mut state)?;

        Ok(Self {
            connection,
            queue,
            surface,
            xdg_surface,
            manager,
            decoration,
            state,
        })
    }

    fn pump(&mut self, commands: &Sender<ServerCommand>) -> Result<(), Box<dyn std::error::Error>> {
        wait_for_server_commands(commands);
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }

    fn commit_configure(
        &mut self,
        commands: &Sender<ServerCommand>,
        serial: u32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.xdg_surface.ack_configure(serial);
        self.surface.commit();
        self.connection.flush()?;
        self.pump(commands)
    }

    fn titlebar_hit(&self, commands: &Sender<ServerCommand>) -> Option<u32> {
        let surface = capture_renderable_surface_snapshot(commands)
            .into_iter()
            .next()
            .expect("decorated test surface")
            .clone();
        capture_pointer_scene_hit(
            commands,
            f64::from(surface.origin_x + 80),
            f64::from(surface.origin_y - 13),
        )
        .0
    }
}

fn start_server() -> (PathBuf, Sender<ServerCommand>, JoinHandle<OwnCompositorServer>) {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).expect("bind test compositor");
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    (socket_path, commands, server_thread)
}

fn stop_server(commands: Sender<ServerCommand>, server_thread: JoinHandle<OwnCompositorServer>) {
    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn dynamic_client_to_server_waits_for_ack_and_commit() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = DecorationClient::connect(
        &socket_path,
        &commands,
        client_zxdg_toplevel_decoration_v1::Mode::ClientSide,
    )
    .expect("connect decoration client");
    assert!(client.titlebar_hit(&commands).is_none());
    let generation_before = capture_scene_render_generation(&commands);
    let serial_count_before = client.state.surface_configure_count;
    let decoration_count_before = client.state.decoration_configure_count;

    client
        .decoration
        .set_mode(client_zxdg_toplevel_decoration_v1::Mode::ServerSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();

    assert_eq!(
        client.state.surface_configure_count,
        serial_count_before + 1
    );
    assert_eq!(
        client.state.decoration_configure_count,
        decoration_count_before + 1
    );
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&2));
    assert!(client.titlebar_hit(&commands).is_none());
    assert_eq!(capture_scene_render_generation(&commands), generation_before);

    let serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, serial).unwrap();
    assert!(client.titlebar_hit(&commands).is_some());
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before + 1
    );
    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn dynamic_server_to_client_waits_for_ack_and_commit() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = DecorationClient::connect(
        &socket_path,
        &commands,
        client_zxdg_toplevel_decoration_v1::Mode::ServerSide,
    )
    .expect("connect decoration client");
    assert!(client.titlebar_hit(&commands).is_some());
    let generation_before = capture_scene_render_generation(&commands);

    client
        .decoration
        .set_mode(client_zxdg_toplevel_decoration_v1::Mode::ClientSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert!(client.titlebar_hit(&commands).is_some());
    assert_eq!(capture_scene_render_generation(&commands), generation_before);

    let serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, serial).unwrap();
    assert!(client.titlebar_hit(&commands).is_none());
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before + 1
    );
    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn unset_mode_uses_the_same_configure_transaction() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = DecorationClient::connect(
        &socket_path,
        &commands,
        client_zxdg_toplevel_decoration_v1::Mode::ClientSide,
    )
    .expect("connect decoration client");
    let generation_before = capture_scene_render_generation(&commands);
    client.decoration.unset_mode();
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert!(client.titlebar_hit(&commands).is_none());
    assert_eq!(capture_scene_render_generation(&commands), generation_before);
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&2));

    let serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, serial).unwrap();
    assert!(client.titlebar_hit(&commands).is_some());
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before + 1
    );
    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn repeated_identical_preference_does_not_create_a_configure_loop() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = DecorationClient::connect(
        &socket_path,
        &commands,
        client_zxdg_toplevel_decoration_v1::Mode::ClientSide,
    )
    .expect("connect decoration client");
    let serial_count_before = client.state.surface_configure_count;
    let decoration_count_before = client.state.decoration_configure_count;
    let generation_before = capture_scene_render_generation(&commands);

    client
        .decoration
        .set_mode(client_zxdg_toplevel_decoration_v1::Mode::ServerSide);
    client
        .decoration
        .set_mode(client_zxdg_toplevel_decoration_v1::Mode::ServerSide);
    client
        .decoration
        .set_mode(client_zxdg_toplevel_decoration_v1::Mode::ServerSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();

    assert_eq!(
        client.state.surface_configure_count,
        serial_count_before + 1
    );
    assert_eq!(
        client.state.decoration_configure_count,
        decoration_count_before + 1
    );
    assert_eq!(capture_scene_render_generation(&commands), generation_before);
    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn newest_acknowledged_decoration_configure_wins() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = DecorationClient::connect(
        &socket_path,
        &commands,
        client_zxdg_toplevel_decoration_v1::Mode::ClientSide,
    )
    .expect("connect decoration client");
    let serial_count_before = client.state.surface_configure_count;

    client
        .decoration
        .set_mode(client_zxdg_toplevel_decoration_v1::Mode::ServerSide);
    client
        .decoration
        .set_mode(client_zxdg_toplevel_decoration_v1::Mode::ClientSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(
        client.state.surface_configure_count,
        serial_count_before + 2
    );
    assert_eq!(
        &client.state.decoration_configure_modes
            [client.state.decoration_configure_modes.len() - 2..],
        &[2, 1]
    );
    assert!(client.titlebar_hit(&commands).is_none());

    let newest_serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, newest_serial).unwrap();
    assert!(client.titlebar_hit(&commands).is_none());
    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn initial_map_has_one_coherent_decoration_transaction() {
    let (socket_path, commands, server_thread) = start_server();
    let client = DecorationClient::connect(
        &socket_path,
        &commands,
        client_zxdg_toplevel_decoration_v1::Mode::ServerSide,
    )
    .expect("connect decoration client");
    assert_eq!(capture_renderable_surface_snapshot(&commands).len(), 1);
    assert_eq!(client.state.surface_configure_count, 1);
    assert_eq!(client.state.decoration_configure_count, 1);
    assert_eq!(client.state.decoration_configure_modes, vec![2]);
    let relevant = client
        .state
        .toplevel_event_log
        .iter()
        .copied()
        .filter(|event| {
            matches!(
                *event,
                "decoration_configure" | "toplevel_configure" | "xdg_surface_configure"
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        relevant,
        vec![
            "decoration_configure",
            "toplevel_configure",
            "xdg_surface_configure"
        ]
    );
    drop(client);
    stop_server(commands, server_thread);
}
