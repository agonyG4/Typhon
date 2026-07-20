use std::{fs::File, num::NonZeroU64, os::fd::AsFd, path::PathBuf};

use crate::compositor::{
    DesktopWindowKind, RenderGenerationCause, SurfacePlacement, WindowConstraints, WindowMetadata,
};
use crate::xwayland::xwm::{
    X11Geometry, X11PublishedState, X11WindowLifecycle, X11WindowSnapshot, XwmCommand, XwmEvent,
};
use crate::xwayland::{X11WindowHandle, XwaylandAssociationEvent, XwaylandGeneration};
use wayland_client::protocol::{
    wl_buffer as client_wl_buffer, wl_compositor as client_wl_compositor, wl_shm as client_wl_shm,
    wl_shm_pool as client_wl_shm_pool, wl_surface as client_wl_surface,
};
use wayland_client::{Connection, EventQueue, globals::registry_queue_init};
use wayland_protocols::xwayland::shell::v1::client::xwayland_shell_v1 as client_xwayland_shell_v1;

struct FirstBufferFixture {
    server: super::OwnCompositorServer,
    connection: Connection,
    queue: EventQueue<super::RegistryTestState>,
    surface: client_wl_surface::WlSurface,
    _pool: client_wl_shm_pool::WlShmPool,
    _file: File,
    _buffer: client_wl_buffer::WlBuffer,
    surface_id: u32,
    initial_buffer_id: u64,
}

fn first_buffer_fixture() -> FirstBufferFixture {
    let socket_name = super::unique_socket_name();
    let mut server = super::OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).expect("nonzero generation"));
    let (server_stream, client_stream) = std::os::unix::net::UnixStream::pair().unwrap();
    server
        .insert_xwayland_client(server_stream, generation)
        .expect("insert private XWayland client");
    let (running, server_thread) = super::spawn_test_server(server);

    let connection = Connection::from_socket(client_stream).expect("create Wayland client");
    let (globals, mut queue) = registry_queue_init::<super::RegistryTestState>(&connection)
        .expect("read compositor globals");
    let qh = queue.handle();
    let shell: client_xwayland_shell_v1::XwaylandShellV1 =
        globals.bind(&qh, 1..=1, ()).expect("bind XWayland shell");
    let compositor: client_wl_compositor::WlCompositor =
        globals.bind(&qh, 1..=6, ()).expect("bind compositor");
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).expect("bind shm");
    let surface = compositor.create_surface(&qh, ());
    let xwayland_surface = shell.get_xwayland_surface(&surface, &qh, ());
    xwayland_surface.set_serial(0x0123_4567, 0x89ab_cdef);
    surface.commit();
    connection.flush().expect("flush association");
    queue
        .roundtrip(&mut super::RegistryTestState::default())
        .expect("complete association");

    let file = super::create_test_shm_file(&[
        0xffff_0000,
        0xff00_ff00,
        0xff00_00ff,
        0xffff_ffff,
        0xff10_1010,
        0xff20_2020,
        0xff30_3030,
        0xff40_4040,
    ])
    .expect("create shm buffer");
    let pool = shm.create_pool(file.as_fd(), 32, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().expect("flush first buffer");
    queue
        .roundtrip(&mut super::RegistryTestState::default())
        .expect("complete first buffer commit");

    let mut server = super::stop_test_server(running, server_thread);
    let associations = server.take_xwayland_association_events();
    let surface_id = associations
        .iter()
        .find_map(|event| match event {
            XwaylandAssociationEvent::Committed { surface_id, .. } => Some(*surface_id),
            XwaylandAssociationEvent::Removed { .. } => None,
        })
        .expect("association has compositor surface id");
    let initial_buffer_id = server
        .current_surface_buffer_id(surface_id)
        .expect("first buffer retained");
    assert!(server.take_xwayland_client_disconnect_events().is_empty());
    FirstBufferFixture {
        server,
        connection,
        queue,
        surface,
        _pool: pool,
        _file: file,
        _buffer: buffer,
        surface_id,
        initial_buffer_id: initial_buffer_id.get(),
    }
}

fn admit_first_buffer(fixture: &mut FirstBufferFixture, x: i32, y: i32) {
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry.x = x;
    snapshot.geometry.y = y;
    let commands = fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    assert_eq!(commands.len(), 1);
}

fn fake_snapshot() -> X11WindowSnapshot {
    X11WindowSnapshot {
        handle: X11WindowHandle::new(
            XwaylandGeneration::new(NonZeroU64::new(1).expect("nonzero")),
            100,
        ),
        surface_id: 7,
        kind: DesktopWindowKind::Managed,
        geometry: X11Geometry {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        },
        metadata: WindowMetadata {
            app_id: Some("fake-x11-client".into()),
            title: Some("Fake X11 client".into()),
            pid: None,
        },
        constraints: WindowConstraints::default(),
        state: X11PublishedState::default(),
        transient_for: None,
        supports_delete: true,
        supports_take_focus: true,
        accepts_input: Some(true),
        window_role: None,
        startup_id: None,
        user_time: None,
        urgency: false,
        sync_counter: None,
    }
}

#[test]
fn xwayland_map_lifecycle_keeps_rendering_after_x11_map() {
    assert_ne!(
        X11WindowLifecycle::MapCommanded,
        X11WindowLifecycle::Renderable
    );
    assert_ne!(
        X11WindowLifecycle::MappedAwaitingAssociation,
        X11WindowLifecycle::Renderable
    );
}

#[test]
fn managed_window_receives_exactly_one_map_command_from_fake_x11_events() {
    let socket =
        std::env::temp_dir().join(format!("typhon-xwayland-map-test-{}", std::process::id()));
    let mut server = super::OwnCompositorServer::bind_cpu_composition(
        PathBuf::from(&socket).to_string_lossy().into_owned(),
    )
    .expect("bind fake compositor server");
    let snapshot = fake_snapshot();
    let first = server.apply_xwayland_window_event(XwmEvent::WindowMapRequested(snapshot.handle));
    assert_eq!(
        first
            .iter()
            .filter(|command| matches!(command, XwmCommand::Map(_)))
            .count(),
        1
    );
    let ready = server.apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    assert_eq!(
        ready
            .iter()
            .filter(|command| matches!(command, XwmCommand::Map(_)))
            .count(),
        0
    );
    drop(server);
    let _ = std::fs::remove_file(socket);
}

#[test]
fn override_redirect_never_receives_managed_map_command() {
    let socket =
        std::env::temp_dir().join(format!("typhon-xwayland-override-{}", std::process::id()));
    let mut server = super::OwnCompositorServer::bind_cpu_composition(
        PathBuf::from(&socket).to_string_lossy().into_owned(),
    )
    .expect("bind fake compositor server");
    let mut snapshot = fake_snapshot();
    snapshot.kind = DesktopWindowKind::OverrideRedirect;
    let commands = server.apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, XwmCommand::Map(_)))
    );
    drop(server);
    let _ = std::fs::remove_file(socket);
}

#[test]
fn xwayland_first_buffer_before_window_is_retained() {
    let mut fixture = first_buffer_fixture();

    assert!(fixture.server.renderable_surfaces().is_empty());
    assert_eq!(
        fixture
            .server
            .current_surface_buffer_id(fixture.surface_id)
            .expect("first buffer retained")
            .get(),
        fixture.initial_buffer_id
    );
    assert_eq!(fixture.server.take_xwayland_buffer_ready_events().len(), 1);
}

#[test]
fn xwayland_buffer_committed_before_serial_becomes_ready() {
    let socket_name = super::unique_socket_name();
    let mut server = super::OwnCompositorServer::bind_cpu_composition(&socket_name)
        .expect("bind compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).expect("nonzero generation"));
    let (server_stream, client_stream) = std::os::unix::net::UnixStream::pair().unwrap();
    server
        .insert_xwayland_client(server_stream, generation)
        .expect("insert private XWayland client");
    let (running, server_thread) = super::spawn_test_server(server);

    let connection = Connection::from_socket(client_stream).expect("create Wayland client");
    let (globals, mut queue) = registry_queue_init::<super::RegistryTestState>(&connection)
        .expect("read compositor globals");
    let qh = queue.handle();
    let shell: client_xwayland_shell_v1::XwaylandShellV1 =
        globals.bind(&qh, 1..=1, ()).expect("bind XWayland shell");
    let compositor: client_wl_compositor::WlCompositor =
        globals.bind(&qh, 1..=6, ()).expect("bind compositor");
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).expect("bind shm");
    let surface = compositor.create_surface(&qh, ());

    let file = super::create_test_shm_file(&[
        0xffff_0000,
        0xff00_ff00,
        0xff00_00ff,
        0xffff_ffff,
        0xff10_1010,
        0xff20_2020,
        0xff30_3030,
        0xff40_4040,
    ])
    .expect("create shm buffer");
    let pool = shm.create_pool(file.as_fd(), 32, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().expect("flush unassigned buffer");
    queue
        .roundtrip(&mut super::RegistryTestState::default())
        .expect("complete unassigned buffer commit");

    let xwayland_surface = shell.get_xwayland_surface(&surface, &qh, ());
    xwayland_surface.set_serial(0x0123_4567, 0x89ab_cdef);
    surface.commit();
    connection.flush().expect("flush serial-only association");
    queue
        .roundtrip(&mut super::RegistryTestState::default())
        .expect("complete serial-only association");

    let mut server = super::stop_test_server(running, server_thread);
    let associations = server.take_xwayland_association_events();
    assert_eq!(associations.len(), 1);
    let surface_id = associations
        .iter()
        .find_map(|event| match event {
            XwaylandAssociationEvent::Committed { surface_id, .. } => Some(*surface_id),
            XwaylandAssociationEvent::Removed { .. } => None,
        })
        .expect("association has compositor surface id");
    let initial_buffer_id = server
        .current_surface_buffer_id(surface_id)
        .expect("pre-association buffer retained")
        .get();

    assert_eq!(server.take_xwayland_buffer_ready_events().len(), 1);
    assert!(server.renderable_surfaces().is_empty());

    let mut snapshot = fake_snapshot();
    snapshot.surface_id = surface_id;
    snapshot.geometry.x = 37;
    snapshot.geometry.y = 42;
    assert_eq!(
        server
            .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot))
            .len(),
        1
    );
    assert_eq!(server.renderable_surfaces().len(), 1);
    assert_eq!(
        server.renderable_surfaces()[0].buffer_id().get(),
        initial_buffer_id
    );
    assert_eq!(
        server.renderable_surfaces()[0].placement,
        SurfacePlacement::absolute_root_at(37, 42)
    );
}

#[test]
fn window_ready_publishes_retained_xwayland_buffer() {
    let mut fixture = first_buffer_fixture();
    assert!(fixture.server.renderable_surfaces().is_empty());
    assert_eq!(fixture.server.take_xwayland_buffer_ready_events().len(), 1);

    admit_first_buffer(&mut fixture, 37, 42);
    assert_eq!(fixture.server.renderable_surfaces().len(), 1);
    let surface = &fixture.server.renderable_surfaces()[0];
    assert_eq!(surface.surface_id, fixture.surface_id);
    assert_eq!(surface.buffer_id().get(), fixture.initial_buffer_id);
    assert_eq!(
        surface.placement,
        SurfacePlacement::absolute_root_at(37, 42)
    );
    assert_eq!(
        fixture.server.render_generation_cause(),
        RenderGenerationCause::SurfaceCommit
    );
    assert_eq!(fixture.server.take_xwayland_buffer_ready_events().len(), 0);
}

#[test]
fn single_frame_x11_client_becomes_renderable_without_second_commit() {
    let mut fixture = first_buffer_fixture();
    admit_first_buffer(&mut fixture, 0, 0);

    assert_eq!(fixture.server.renderable_surfaces().len(), 1);
    assert_eq!(
        fixture.server.renderable_surfaces()[0].buffer_id().get(),
        fixture.initial_buffer_id
    );
}

#[test]
fn retained_buffer_uses_admitted_window_placement() {
    let mut fixture = first_buffer_fixture();
    admit_first_buffer(&mut fixture, 123, 456);

    assert_eq!(
        fixture.server.renderable_surfaces()[0].placement,
        SurfacePlacement::absolute_root_at(123, 456)
    );
}

#[test]
fn future_buffer_replaces_adopted_initial_buffer() {
    let mut fixture = first_buffer_fixture();
    admit_first_buffer(&mut fixture, 0, 0);
    let initial_buffer_id = fixture.initial_buffer_id;
    assert_eq!(fixture.server.take_xwayland_buffer_ready_events().len(), 1);

    let server = fixture.server;
    let (running, server_thread) = super::spawn_test_server(server);
    let qh = fixture.queue.handle();
    let second_buffer =
        fixture
            ._pool
            .create_buffer(16, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    fixture.surface.attach(Some(&second_buffer), 0, 0);
    fixture.surface.damage_buffer(0, 0, 2, 2);
    fixture.surface.commit();
    fixture
        .connection
        .flush()
        .expect("flush replacement buffer");
    fixture
        .queue
        .roundtrip(&mut super::RegistryTestState::default())
        .expect("complete replacement buffer commit");
    fixture.server = super::stop_test_server(running, server_thread);

    let replacement_id = fixture
        .server
        .current_surface_buffer_id(fixture.surface_id)
        .expect("replacement buffer retained")
        .get();
    assert_ne!(replacement_id, initial_buffer_id);
    assert_eq!(fixture.server.renderable_surfaces().len(), 1);
    assert_eq!(
        fixture.server.renderable_surfaces()[0].buffer_id().get(),
        replacement_id
    );
    assert_eq!(fixture.server.take_xwayland_buffer_ready_events().len(), 1);
}

#[test]
fn window_admission_failure_does_not_publish_retained_buffer() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.handle = X11WindowHandle::new(
        XwaylandGeneration::new(NonZeroU64::new(2).expect("nonzero")),
        snapshot.handle.xid(),
    );

    assert!(
        fixture
            .server
            .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot))
            .is_empty()
    );
    assert!(fixture.server.renderable_surfaces().is_empty());
    assert_eq!(
        fixture
            .server
            .current_surface_buffer_id(fixture.surface_id)
            .expect("retained buffer remains")
            .get(),
        fixture.initial_buffer_id
    );
}

#[test]
fn stale_generation_buffer_is_not_adopted() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.handle = X11WindowHandle::new(
        XwaylandGeneration::new(NonZeroU64::new(2).expect("nonzero")),
        snapshot.handle.xid(),
    );

    let _ = fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    assert!(fixture.server.renderable_surfaces().is_empty());
}

#[test]
fn adoption_does_not_emit_duplicate_buffer_ready() {
    let mut fixture = first_buffer_fixture();
    assert_eq!(fixture.server.take_xwayland_buffer_ready_events().len(), 1);

    admit_first_buffer(&mut fixture, 0, 0);

    assert!(
        fixture
            .server
            .take_xwayland_buffer_ready_events()
            .is_empty()
    );
}
