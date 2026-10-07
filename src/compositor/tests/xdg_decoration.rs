use super::*;
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1 as client_zxdg_decoration_manager_v1,
    zxdg_toplevel_decoration_v1 as client_zxdg_toplevel_decoration_v1,
};

struct DecorationClient {
    connection: Connection,
    queue: EventQueue<RegistryTestState>,
    manager: client_zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    surface: client_wl_surface::WlSurface,
    xdg_surface: client_xdg_surface::XdgSurface,
    toplevel: client_xdg_toplevel::XdgToplevel,
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
            manager,
            surface,
            xdg_surface,
            toplevel,
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

    fn commit_surface(
        &mut self,
        commands: &Sender<ServerCommand>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.surface.commit();
        self.connection.flush()?;
        self.pump(commands)
    }

    fn decoration_count(&self, commands: &Sender<ServerCommand>) -> usize {
        capture_native_decoration_count(commands)
    }
}

struct MappedDecorationClient {
    connection: Connection,
    queue: EventQueue<RegistryTestState>,
    manager: client_zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    advertised_manager_version: u32,
    shm: client_wl_shm::WlShm,
    timing_manager: Option<client_wp_commit_timing_manager_v1::WpCommitTimingManagerV1>,
    surface: client_wl_surface::WlSurface,
    xdg_surface: client_xdg_surface::XdgSurface,
    toplevel: client_xdg_toplevel::XdgToplevel,
    dmabuf: Option<client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
    syncobj: Option<client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1>,
    state: RegistryTestState,
}

impl MappedDecorationClient {
    fn connect(
        socket_path: &PathBuf,
        commands: &Sender<ServerCommand>,
        manager_version: u32,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let connection = Connection::from_socket(UnixStream::connect(socket_path)?)?;
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let advertised_manager_version = globals.contents().with_list(|globals| {
            globals
                .iter()
                .find(|global| global.interface == "zxdg_decoration_manager_v1")
                .map(|global| global.version)
                .expect("zxdg_decoration_manager_v1 global")
        });
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ())?;
        let dmabuf: Option<client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1> =
            globals.bind(&qh, 3..=3, ()).ok();
        let syncobj: Option<client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1> =
            globals.bind(&qh, 1..=1, ()).ok();
        let manager: client_zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
            globals.bind(&qh, 1..=manager_version, ())?;
        let timing_manager: Option<client_wp_commit_timing_manager_v1::WpCommitTimingManagerV1> =
            globals.bind(&qh, 1..=1, ()).ok();
        let (surface, xdg_surface, toplevel) =
            create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
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
            manager,
            advertised_manager_version,
            shm,
            timing_manager,
            surface,
            xdg_surface,
            toplevel,
            dmabuf,
            syncobj,
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

    fn commit_surface(
        &mut self,
        commands: &Sender<ServerCommand>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.surface.commit();
        self.connection.flush()?;
        self.pump(commands)
    }
}

fn start_server() -> (
    PathBuf,
    Sender<ServerCommand>,
    JoinHandle<OwnCompositorServer>,
) {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).expect("bind test compositor");
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    (socket_path, commands, server_thread)
}

fn stop_server(commands: Sender<ServerCommand>, server_thread: JoinHandle<OwnCompositorServer>) {
    stop_controllable_test_server(commands, server_thread);
}

fn astreactl_control_request(
    commands: &Sender<ServerCommand>,
    command: &'static str,
    args: serde_json::Value,
) -> Result<crate::control_snapshots::AstreactlResult, crate::astreactl::client::AstreactlError> {
    let path = std::env::temp_dir().join(format!(
        "astreactl-decoration-control-{}-{}.sock",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let commands = commands.clone();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request_bytes = Vec::new();
        stream.read_to_end(&mut request_bytes).unwrap();
        let request = crate::control::decode_request(&request_bytes).unwrap();
        let (reply, response) = std::sync::mpsc::channel();
        commands
            .send(ServerCommand::DispatchControlRequest { request, reply })
            .unwrap();
        let response = response.recv_timeout(Duration::from_secs(2)).unwrap();
        stream
            .write_all(&crate::control::encode_response(&response).unwrap())
            .unwrap();
    });

    let result =
        crate::astreactl::client::request_with_args(&path, command, args, Duration::from_secs(2));
    server.join().unwrap();
    let _ = std::fs::remove_file(path);
    result
}

fn expect_decoration_orphaned_error(connection: &Connection, decoration_id: u32) {
    let error = match connection.roundtrip() {
        Err(wayland_client::backend::WaylandError::Protocol(error)) => error,
        Err(error) => panic!("expected a Wayland protocol error, got {error:?}"),
        Ok(_) => panic!("destroying a toplevel with a live decoration must be fatal"),
    };
    assert_eq!(error.object_interface, "zxdg_toplevel_decoration_v1");
    assert_eq!(
        error.code,
        client_zxdg_toplevel_decoration_v1::Error::Orphaned as u32
    );
    assert_eq!(error.object_id, decoration_id);
}

#[test]
fn astreactl_control_round_trip_sets_server_policy_for_xdg_without_decoration_object() {
    let (socket_path, commands, server_thread) = start_server();
    let client = MappedDecorationClient::connect(&socket_path, &commands, 1)
        .expect("connect XDG client without a decoration object");
    assert_eq!(capture_native_decoration_count(&commands), 0);

    let windows = astreactl_control_request(&commands, "windows", serde_json::json!({}))
        .expect("query window list through astreactl");
    let window = match windows {
        crate::control_snapshots::AstreactlResult::Windows(snapshot) => {
            assert_eq!(snapshot.windows.len(), 1);
            snapshot.windows.into_iter().next().unwrap()
        }
        _ => panic!("windows command returned the wrong typed result"),
    };
    assert_eq!(
        window.kind,
        crate::control_snapshots::WindowKindSnapshot::XdgToplevel
    );
    let window_id = window.id.0;
    let (reply, state) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::CaptureWindowDecorationState {
            id: window_id,
            reply,
        })
        .unwrap();
    assert_eq!(
        state.recv_timeout(Duration::from_secs(2)).unwrap(),
        Some(("client_preference".to_string(), "ClientSide".to_string()))
    );

    let configure_count_before = client.state.surface_configure_count;
    let server_result = astreactl_control_request(
        &commands,
        "window.decoration-policy.set",
        serde_json::json!({"id": window_id, "policy": "server"}),
    )
    .expect("server policy command succeeds");
    match server_result {
        crate::control_snapshots::AstreactlResult::WindowDecorationPolicy(snapshot) => {
            assert_eq!(snapshot.id.0, window_id);
            assert_eq!(snapshot.policy, "server");
            assert_eq!(snapshot.effective_mode, "ServerSide");
        }
        _ => panic!("decoration policy command returned the wrong typed result"),
    }
    assert_eq!(client.state.surface_configure_count, configure_count_before);
    assert_eq!(client.state.decoration_configure_count, 0);
    assert_eq!(capture_native_decoration_count(&commands), 1);

    let invalid_policy = astreactl_control_request(
        &commands,
        "window.decoration-policy.set",
        serde_json::json!({"id": window_id, "policy": "invalid"}),
    );
    assert!(matches!(
        invalid_policy,
        Err(crate::astreactl::client::AstreactlError::Server(error))
            if error.code.as_str() == "invalid_argument"
    ));

    let missing_window = astreactl_control_request(
        &commands,
        "window.decoration-policy.set",
        serde_json::json!({"id": u64::MAX, "policy": "server"}),
    );
    assert!(matches!(
        missing_window,
        Err(crate::astreactl::client::AstreactlError::Server(error))
            if error.code.as_str() == "invalid_argument"
                && error.message == "unknown window id"
    ));

    let client_result = astreactl_control_request(
        &commands,
        "window.decoration-policy.set",
        serde_json::json!({"id": window_id, "policy": "client_preference"}),
    )
    .expect("client preference command succeeds");
    match client_result {
        crate::control_snapshots::AstreactlResult::WindowDecorationPolicy(snapshot) => {
            assert_eq!(snapshot.id.0, window_id);
            assert_eq!(snapshot.policy, "client_preference");
            assert_eq!(snapshot.effective_mode, "ClientSide");
        }
        _ => panic!("decoration policy command returned the wrong typed result"),
    }
    assert_eq!(capture_native_decoration_count(&commands), 0);
    drop(client);
    stop_server(commands, server_thread);
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
    assert_eq!(client.decoration_count(&commands), 0);
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
    assert_eq!(client.decoration_count(&commands), 0);
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before
    );

    let serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, serial).unwrap();
    assert_eq!(client.decoration_count(&commands), 1);
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before + 1
    );
    client.commit_surface(&commands).unwrap();
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before + 1
    );
    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn compositor_server_policy_keeps_live_csd_until_commit_and_restores_client_preference() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = DecorationClient::connect(
        &socket_path,
        &commands,
        client_zxdg_toplevel_decoration_v1::Mode::ClientSide,
    )
    .expect("connect decoration client");
    let generation_before = capture_scene_render_generation(&commands);
    let serial_count_before = client.state.surface_configure_count;
    let decoration_count_before = client.state.decoration_configure_count;
    assert_eq!(client.decoration_count(&commands), 0);

    assert!(set_most_recent_window_decoration_policy(
        &commands,
        crate::wm::WindowDecorationPolicy::Server,
    ));
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
    assert_eq!(client.decoration_count(&commands), 0);
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before
    );

    let server_serial = *client.state.surface_configure_serials.last().unwrap();
    client.xdg_surface.ack_configure(server_serial);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(client.decoration_count(&commands), 0);
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before
    );

    client.surface.commit();
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(client.decoration_count(&commands), 1);
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before + 1
    );

    let stable_serial_count = client.state.surface_configure_count;
    let stable_decoration_count = client.state.decoration_configure_count;
    client
        .decoration
        .set_mode(client_zxdg_toplevel_decoration_v1::Mode::ClientSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(client.state.surface_configure_count, stable_serial_count);
    assert_eq!(
        client.state.decoration_configure_count,
        stable_decoration_count
    );
    assert_eq!(client.decoration_count(&commands), 1);

    assert!(set_most_recent_window_decoration_policy(
        &commands,
        crate::wm::WindowDecorationPolicy::ClientPreference,
    ));
    client.pump(&commands).unwrap();
    assert_eq!(
        client.state.surface_configure_count,
        stable_serial_count + 1
    );
    assert_eq!(
        client.state.decoration_configure_count,
        stable_decoration_count + 1
    );
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&1));
    assert_eq!(client.decoration_count(&commands), 1);

    let client_serial = *client.state.surface_configure_serials.last().unwrap();
    client.xdg_surface.ack_configure(client_serial);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(client.decoration_count(&commands), 1);
    client.surface.commit();
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(client.decoration_count(&commands), 0);
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before + 2
    );
    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn late_decoration_object_inherits_forced_server_visual_baseline() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = MappedDecorationClient::connect(&socket_path, &commands, 1)
        .expect("connect mapped XDG client without decorations");
    assert_eq!(capture_native_decoration_count(&commands), 0);

    assert!(set_most_recent_window_decoration_policy(
        &commands,
        crate::wm::WindowDecorationPolicy::Server,
    ));
    assert_eq!(capture_native_decoration_count(&commands), 1);
    let generation_before_creation = capture_scene_render_generation(&commands);

    let qh = client.queue.handle();
    let decoration = client
        .manager
        .get_toplevel_decoration(&client.toplevel, &qh, ());
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&2));
    assert_eq!(capture_native_decoration_count(&commands), 1);
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before_creation
    );

    let serial = *client.state.surface_configure_serials.last().unwrap();
    client.xdg_surface.ack_configure(serial);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);
    client.surface.commit();
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before_creation
    );

    decoration.destroy();
    client.connection.flush().unwrap();
    client.surface.commit();
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before_creation
    );

    let recreated =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&2));
    assert_eq!(capture_native_decoration_count(&commands), 1);
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before_creation
    );
    let recreated_serial = *client.state.surface_configure_serials.last().unwrap();
    client.xdg_surface.ack_configure(recreated_serial);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);
    client.surface.commit();
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before_creation
    );

    recreated.destroy();
    client.connection.flush().unwrap();
    client.surface.commit();
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before_creation
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
    assert_eq!(client.decoration_count(&commands), 1);
    let generation_before = capture_scene_render_generation(&commands);

    client
        .decoration
        .set_mode(client_zxdg_toplevel_decoration_v1::Mode::ClientSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(client.decoration_count(&commands), 1);
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before
    );

    let serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, serial).unwrap();
    assert_eq!(client.decoration_count(&commands), 0);
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
    assert_eq!(client.decoration_count(&commands), 0);
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before
    );
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&2));

    let serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, serial).unwrap();
    assert_eq!(client.decoration_count(&commands), 1);
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
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before
    );
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
    assert_eq!(client.decoration_count(&commands), 0);

    let newest_serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, newest_serial).unwrap();
    assert_eq!(client.decoration_count(&commands), 0);
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

#[test]
fn destroy_plain_commit_latches_client_side_decoration() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = DecorationClient::connect(
        &socket_path,
        &commands,
        client_zxdg_toplevel_decoration_v1::Mode::ServerSide,
    )
    .expect("connect decoration client");
    let generation_before = capture_scene_render_generation(&commands);
    let decoration_events_before = client.state.decoration_configure_count;
    let surface_configures_before = client.state.surface_configure_count;

    client.decoration.destroy();
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(client.decoration_count(&commands), 1);
    assert_eq!(
        client.state.decoration_configure_count,
        decoration_events_before
    );
    assert_eq!(
        client.state.surface_configure_count,
        surface_configures_before
    );
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before
    );

    client.commit_surface(&commands).unwrap();
    assert_eq!(client.decoration_count(&commands), 0);
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before + 1
    );
    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn stale_server_side_decoration_ack_cannot_override_destroy_commit() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = DecorationClient::connect(
        &socket_path,
        &commands,
        client_zxdg_toplevel_decoration_v1::Mode::ClientSide,
    )
    .expect("connect decoration client");
    client
        .decoration
        .set_mode(client_zxdg_toplevel_decoration_v1::Mode::ServerSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    let pending_server_side_serial = *client.state.surface_configure_serials.last().unwrap();

    client.decoration.destroy();
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    client
        .commit_configure(&commands, pending_server_side_serial)
        .unwrap();

    assert_eq!(client.decoration_count(&commands), 0);
    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn fullscreen_decoration_visibility_follows_the_applied_mode() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = DecorationClient::connect(
        &socket_path,
        &commands,
        client_zxdg_toplevel_decoration_v1::Mode::ServerSide,
    )
    .expect("connect decoration client");
    assert_eq!(client.decoration_count(&commands), 1);

    commands
        .send(ServerCommand::ToggleFullscreenFocused)
        .unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(client.decoration_count(&commands), 1);
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&2));
    let serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, serial).unwrap();
    assert_eq!(client.decoration_count(&commands), 0);

    commands
        .send(ServerCommand::ToggleFullscreenFocused)
        .unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(client.decoration_count(&commands), 0);
    let serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, serial).unwrap();
    assert_eq!(client.decoration_count(&commands), 1);
    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn v1_decoration_creation_after_mapped_content_uses_configure_transaction() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = MappedDecorationClient::connect(&socket_path, &commands, 1)
        .expect("connect v1 decoration client");
    assert_eq!(client.advertised_manager_version, 2);
    assert_eq!(client.manager.version(), 1);
    let decoration_configures_before = client.state.decoration_configure_count;
    let surface_configures_before = client.state.surface_configure_count;
    let decoration =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert!(decoration.is_alive());
    assert_eq!(
        client.state.decoration_configure_count,
        decoration_configures_before + 1
    );
    assert_eq!(
        client.state.surface_configure_count,
        surface_configures_before + 1
    );
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&2));
    assert_eq!(capture_native_decoration_count(&commands), 0);

    let serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, serial).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);

    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn v1_late_decoration_respects_explicit_client_side_preference() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = MappedDecorationClient::connect(&socket_path, &commands, 1)
        .expect("connect v1 decoration client");
    let decoration =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    decoration.set_mode(client_zxdg_toplevel_decoration_v1::Mode::ClientSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert!(decoration.is_alive());
    assert_eq!(
        client.state.decoration_configure_modes.last(),
        Some(&(client_zxdg_toplevel_decoration_v1::Mode::ClientSide as u32))
    );
    assert_eq!(capture_native_decoration_count(&commands), 0);

    let serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, serial).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 0);

    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn second_live_decoration_object_reports_already_constructed() {
    let (socket_path, commands, server_thread) = start_server();
    let client = DecorationClient::connect(
        &socket_path,
        &commands,
        client_zxdg_toplevel_decoration_v1::Mode::ServerSide,
    )
    .expect("connect v1 decoration client");
    let _second =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    client.connection.flush().unwrap();
    wait_for_server_commands(&commands);
    let observed = expect_protocol_error(
        &client.connection,
        "zxdg_decoration_manager_v1",
        client_zxdg_toplevel_decoration_v1::Error::AlreadyConstructed as u32,
    );
    assert!(observed.message.contains("already has a decoration"));
    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn v2_decoration_creation_after_mapped_content_keeps_client_side_until_commit() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = MappedDecorationClient::connect(&socket_path, &commands, 2)
        .expect("connect v2 decoration client");
    assert_eq!(client.manager.version(), 2);
    assert_eq!(client.advertised_manager_version, 2);
    let decoration_configures_before = client.state.decoration_configure_count;
    let surface_configures_before = client.state.surface_configure_count;
    let decoration =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(
        client.state.decoration_configure_count,
        decoration_configures_before + 1
    );
    assert_eq!(
        client.state.surface_configure_count,
        surface_configures_before + 1
    );
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&2));
    assert_eq!(capture_native_decoration_count(&commands), 0);
    assert!(decoration.is_alive());

    let serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, serial).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);
    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn destroying_toplevel_with_live_decoration_posts_orphaned() {
    let (socket_path, commands, server_thread) = start_server();
    let client = DecorationClient::connect(
        &socket_path,
        &commands,
        client_zxdg_toplevel_decoration_v1::Mode::ServerSide,
    )
    .expect("connect decoration client");

    let decoration_id = client.decoration.id().protocol_id();
    client.toplevel.destroy();
    client.connection.flush().unwrap();
    expect_decoration_orphaned_error(&client.connection, decoration_id);

    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn destroying_decoration_before_toplevel_allows_normal_teardown() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = DecorationClient::connect(
        &socket_path,
        &commands,
        client_zxdg_toplevel_decoration_v1::Mode::ServerSide,
    )
    .expect("connect decoration client");

    client.decoration.destroy();
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    client.toplevel.destroy();
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert!(client.connection.protocol_error().is_none());
    client.xdg_surface.destroy();
    client.surface.destroy();
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();

    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn destroying_toplevel_with_recreated_live_decoration_posts_orphaned() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = MappedDecorationClient::connect(&socket_path, &commands, 2)
        .expect("connect v2 decoration client");
    let first =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    first.destroy();
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    let second =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();

    let decoration_id = second.id().protocol_id();
    client.toplevel.destroy();
    client.connection.flush().unwrap();
    expect_decoration_orphaned_error(&client.connection, decoration_id);

    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn delayed_surface_commit_uses_captured_decoration_state_without_syncobj() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    server.set_presentation_clock(PresentationClock::Monotonic);
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let mut client = MappedDecorationClient::connect(&socket_path, &commands, 2)
        .expect("connect v2 decoration client");
    let decoration =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    if client.state.decoration_configure_modes.last() != Some(&2) {
        decoration.set_mode(client_zxdg_toplevel_decoration_v1::Mode::ServerSide);
        client.connection.flush().unwrap();
        client.pump(&commands).unwrap();
    }
    let serial_a = *client.state.surface_configure_serials.last().unwrap();
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&2));
    assert_eq!(capture_native_decoration_count(&commands), 0);

    // ACK A, then hold C1 in the real surface-tree transaction queue using
    // the compositor's commit-timing readiness path.
    client.xdg_surface.ack_configure(serial_a);
    let timer = client
        .timing_manager
        .as_ref()
        .expect("commit timing manager global")
        .get_timer(&client.surface, &client.queue.handle(), ());
    let now = PresentationTimestamp::from_clock(PresentationClock::Monotonic).unwrap();
    let (seconds_hi, seconds_lo) = now.protocol_seconds();
    let target_seconds = seconds_lo.saturating_add(1);
    timer.set_timestamp(seconds_hi, target_seconds, now.nanoseconds());
    client.surface.commit();
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();

    let blocked = capture_xdg_role_snapshot(&commands, client.surface.id().protocol_id());
    assert_eq!(blocked.pending_explicit_sync_commits, 0);
    assert_eq!(blocked.pending_surface_tree_transactions, 1);
    assert_eq!(capture_native_decoration_count(&commands), 0);

    // ACK B without a surface commit. Publishing C1 must still apply A.
    decoration.set_mode(client_zxdg_toplevel_decoration_v1::Mode::ClientSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    let serial_b = *client.state.surface_configure_serials.last().unwrap();
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&1));
    client.xdg_surface.ack_configure(serial_b);
    client.connection.flush().unwrap();
    client.queue.roundtrip(&mut client.state).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(capture_native_decoration_count(&commands), 0);

    std::thread::sleep(std::time::Duration::from_millis(1_100));
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    client.queue.roundtrip(&mut client.state).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);

    // C2 is the first surface commit after ACK B and therefore captures B.
    client.commit_surface(&commands).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 0);

    drop(client);
    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn v2_destroy_and_recreate_before_commit_retains_previous_mode() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = MappedDecorationClient::connect(&socket_path, &commands, 2)
        .expect("connect v2 decoration client");
    let decoration =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    decoration.set_mode(client_zxdg_toplevel_decoration_v1::Mode::ServerSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    let serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, serial).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);
    let generation_before = capture_scene_render_generation(&commands);
    decoration.destroy();
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    let recreated =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&2));
    assert_eq!(capture_native_decoration_count(&commands), 1);
    assert_eq!(
        capture_scene_render_generation(&commands),
        generation_before
    );
    assert!(recreated.is_alive());

    // This commit crosses the abandoned D1 destruction boundary, but D2 has
    // not acknowledged a configure yet, so the previous applied SSD remains.
    client.commit_surface(&commands).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);
    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn v2_destroy_commit_and_recreate_starts_client_side() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = MappedDecorationClient::connect(&socket_path, &commands, 2)
        .expect("connect v2 decoration client");
    let decoration =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    decoration.set_mode(client_zxdg_toplevel_decoration_v1::Mode::ServerSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    let serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, serial).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);
    decoration.destroy();
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    client.commit_surface(&commands).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 0);
    let recreated =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&2));
    assert!(recreated.is_alive());
    assert_eq!(capture_native_decoration_count(&commands), 0);
    let recreated_serial = *client.state.surface_configure_serials.last().unwrap();
    client
        .commit_configure(&commands, recreated_serial)
        .unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);
    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn old_decoration_generation_ack_cannot_apply_to_recreated_object() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = MappedDecorationClient::connect(&socket_path, &commands, 2)
        .expect("connect v2 decoration client");
    let first =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    first.set_mode(client_zxdg_toplevel_decoration_v1::Mode::ServerSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    let server_serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, server_serial).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);

    first.set_mode(client_zxdg_toplevel_decoration_v1::Mode::ClientSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    let stale_serial = *client.state.surface_configure_serials.last().unwrap();
    client.xdg_surface.ack_configure(stale_serial);
    first.destroy();
    let second =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();

    // The next commit has no ACK for the second object's configure. The
    // acknowledged ClientSide state still belongs to the destroyed object.
    client.commit_surface(&commands).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);

    second.set_mode(client_zxdg_toplevel_decoration_v1::Mode::ClientSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    let second_serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, second_serial).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 0);

    drop(client);
    stop_server(commands, server_thread);
}

#[test]
fn delayed_surface_commit_uses_decoration_state_captured_at_commit() {
    let Some(device) = test_syncobj_device() else {
        return;
    };
    let Some(acquire_timeline) = device.create_timeline_for_tests().ok() else {
        return;
    };
    let Some(release_timeline) = device.create_timeline_for_tests().ok() else {
        return;
    };

    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let mut client = MappedDecorationClient::connect(&socket_path, &commands, 2)
        .expect("connect v2 decoration client");
    let dmabuf = client.dmabuf.as_ref().expect("dmabuf global").clone();
    let syncobj = client.syncobj.as_ref().expect("syncobj global");
    let sync_surface = syncobj.get_surface(&client.surface, &client.queue.handle(), ());
    let acquire_fd = acquire_timeline.export_timeline_fd().unwrap();
    let release_fd = release_timeline.export_timeline_fd().unwrap();
    let acquire = syncobj.import_timeline(acquire_fd.as_fd(), &client.queue.handle(), ());
    let release = syncobj.import_timeline(release_fd.as_fd(), &client.queue.handle(), ());
    let buffer = create_test_dmabuf_buffer(&dmabuf, &client.queue.handle(), 0xff44_5566).unwrap();
    let decoration =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    let serial_a = *client.state.surface_configure_serials.last().unwrap();
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&2));
    assert_eq!(capture_native_decoration_count(&commands), 0);

    // ACK A and commit C1. The unsignaled acquire point holds C1 after its
    // decoration context has been captured by wl_surface.commit.
    client.xdg_surface.ack_configure(serial_a);
    acquire_timeline.signal_point(1).unwrap();
    assert!(!acquire_timeline.point_signaled(2).unwrap());
    sync_surface.set_acquire_point(&acquire, 0, 2);
    sync_surface.set_release_point(&release, 0, 2);
    client.surface.attach(Some(&buffer), 0, 0);
    client.surface.damage_buffer(0, 0, 2, 2);
    client.surface.commit();
    client.connection.flush().unwrap();
    client.queue.roundtrip(&mut client.state).unwrap();
    wait_for_server_commands(&commands);
    let blocked = capture_xdg_role_snapshot(&commands, client.surface.id().protocol_id());
    assert_eq!(
        blocked.pending_explicit_sync_commits + blocked.pending_surface_tree_transactions,
        1,
        "C1 must remain unpublished while its acquire point is unsignaled: {blocked:?}"
    );
    assert_eq!(capture_native_decoration_count(&commands), 0);

    // ACK B without a surface commit. Publishing C1 must still apply A.
    decoration.set_mode(client_zxdg_toplevel_decoration_v1::Mode::ClientSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    let serial_b = *client.state.surface_configure_serials.last().unwrap();
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&1));
    client.xdg_surface.ack_configure(serial_b);
    client.connection.flush().unwrap();
    client.queue.roundtrip(&mut client.state).unwrap();

    acquire_timeline.signal_point(2).unwrap();
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    client.queue.roundtrip(&mut client.state).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);

    // C2 is the first surface commit after ACK B and therefore captures B.
    client.commit_surface(&commands).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 0);

    // Restore SSD on D1, then hold the commit that follows D1 destruction.
    decoration.set_mode(client_zxdg_toplevel_decoration_v1::Mode::ServerSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    let serial_c = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, serial_c).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);

    decoration.destroy();
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    let second_buffer =
        create_test_dmabuf_buffer(&dmabuf, &client.queue.handle(), 0xff66_7788).unwrap();
    sync_surface.set_acquire_point(&acquire, 0, 3);
    sync_surface.set_release_point(&release, 0, 4);
    client.surface.attach(Some(&second_buffer), 0, 0);
    client.surface.damage_buffer(0, 0, 2, 2);
    client.surface.commit();
    client.connection.flush().unwrap();
    client.queue.roundtrip(&mut client.state).unwrap();
    wait_for_server_commands(&commands);
    let blocked_destroy = capture_xdg_role_snapshot(&commands, client.surface.id().protocol_id());
    assert_eq!(
        blocked_destroy.pending_explicit_sync_commits
            + blocked_destroy.pending_surface_tree_transactions,
        1,
        "C4 must remain unpublished while its acquire point is unsignaled: {blocked_destroy:?}"
    );
    assert_eq!(capture_native_decoration_count(&commands), 1);

    // D2 is created after C4's surface commit boundary, while C4 is held.
    let second_decoration =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    let second_decoration_serial = *client.state.surface_configure_serials.last().unwrap();
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&2));

    acquire_timeline.signal_point(3).unwrap();
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    client.queue.roundtrip(&mut client.state).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 0);
    client.xdg_surface.ack_configure(second_decoration_serial);
    client.connection.flush().unwrap();
    client.commit_surface(&commands).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);
    assert!(second_decoration.is_alive());

    drop(client);
    stop_controllable_test_server(commands, server_thread);
}

#[test]
fn v1_destroy_recreate_keeps_baseline_until_new_configure_commit() {
    let (socket_path, commands, server_thread) = start_server();
    let mut client = MappedDecorationClient::connect(&socket_path, &commands, 1)
        .expect("connect v1 decoration client");
    let first =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert!(first.is_alive());
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&2));
    assert_eq!(capture_native_decoration_count(&commands), 0);

    let first_serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, first_serial).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);

    first.set_mode(client_zxdg_toplevel_decoration_v1::Mode::ClientSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    let stale_serial = *client.state.surface_configure_serials.last().unwrap();
    client.xdg_surface.ack_configure(stale_serial);
    first.destroy();
    let second =
        client
            .manager
            .get_toplevel_decoration(&client.toplevel, &client.queue.handle(), ());
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert!(second.is_alive());
    assert_eq!(client.state.decoration_configure_modes.last(), Some(&2));

    // The acknowledged ClientSide configure belongs to the old generation.
    // A commit without acknowledging the recreated object's configure must
    // retain the previous ServerSide publication.
    client.commit_surface(&commands).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 1);

    second.set_mode(client_zxdg_toplevel_decoration_v1::Mode::ClientSide);
    client.connection.flush().unwrap();
    client.pump(&commands).unwrap();
    assert_eq!(
        client.state.decoration_configure_modes.last(),
        Some(&(client_zxdg_toplevel_decoration_v1::Mode::ClientSide as u32))
    );
    let second_serial = *client.state.surface_configure_serials.last().unwrap();
    client.commit_configure(&commands, second_serial).unwrap();
    assert_eq!(capture_native_decoration_count(&commands), 0);

    drop(client);
    stop_server(commands, server_thread);
}
