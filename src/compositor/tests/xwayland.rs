use std::{fs::File, num::NonZeroU64, os::fd::AsFd, path::PathBuf};

use crate::compositor::{
    DesktopWindowKind, LiveRoleInstance, PermanentSurfaceRole, RenderGenerationCause,
    SurfacePlacement, SurfacePublicationSource, SurfaceRoleLifecycle, WindowConstraints,
    WindowMetadata, X11MoveResizeBeginResult, XwaylandSurfaceState,
};
use crate::xwayland::xwm::{
    X11ConfigureFlags, X11ConfigureRequest, X11Geometry, X11MoveResizeDirection,
    X11MoveResizeRequest, X11PublishedState, X11WindowLifecycle, X11WindowSnapshot, X11WindowType,
    X11WindowTypes, XwmAssociationEvent, XwmCommand, XwmEvent,
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

struct StationaryPointerXwaylandFixture {
    server: super::OwnCompositorServer,
    connection: Connection,
    queue: EventQueue<super::RegistryTestState>,
    _parent_surface: client_wl_surface::WlSurface,
    _popup_surface: client_wl_surface::WlSurface,
    _pool: client_wl_shm_pool::WlShmPool,
    _file: File,
    _parent_buffer: client_wl_buffer::WlBuffer,
    _popup_buffer: client_wl_buffer::WlBuffer,
    parent_surface_id: u32,
    popup_surface_id: u32,
}

fn stationary_pointer_xwayland_fixture() -> StationaryPointerXwaylandFixture {
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
    let parent_surface = compositor.create_surface(&qh, ());
    let parent_shell_surface = shell.get_xwayland_surface(&parent_surface, &qh, ());
    parent_shell_surface.set_serial(0x1111_2222, 0x3333_4444);
    parent_surface.commit();
    let popup_surface = compositor.create_surface(&qh, ());
    let popup_shell_surface = shell.get_xwayland_surface(&popup_surface, &qh, ());
    popup_shell_surface.set_serial(0x5555_6666, 0x7777_8888);
    popup_surface.commit();

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
    let parent_buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    parent_surface.attach(Some(&parent_buffer), 0, 0);
    parent_surface.damage_buffer(0, 0, 2, 2);
    parent_surface.commit();
    let popup_buffer = pool.create_buffer(16, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    popup_surface.attach(Some(&popup_buffer), 0, 0);
    popup_surface.damage_buffer(0, 0, 2, 2);
    popup_surface.commit();
    connection.flush().expect("flush XWayland surfaces");
    queue
        .roundtrip(&mut super::RegistryTestState::default())
        .expect("complete XWayland surface commits");

    let mut server = super::stop_test_server(running, server_thread);
    let parent_serial =
        crate::xwayland::serial_from_parts(0x1111_2222, 0x3333_4444).expect("parent serial");
    let popup_serial =
        crate::xwayland::serial_from_parts(0x5555_6666, 0x7777_8888).expect("popup serial");
    let associations = server.take_xwayland_association_events();
    let surface_for_serial = |serial| {
        associations.iter().find_map(|event| match event {
            XwaylandAssociationEvent::Committed {
                serial: event_serial,
                surface_id,
                ..
            } if *event_serial == serial => Some(*surface_id),
            _ => None,
        })
    };
    let parent_surface_id = surface_for_serial(parent_serial).expect("parent association");
    let popup_surface_id = surface_for_serial(popup_serial).expect("popup association");

    StationaryPointerXwaylandFixture {
        server,
        connection,
        queue,
        _parent_surface: parent_surface,
        _popup_surface: popup_surface,
        _pool: pool,
        _file: file,
        _parent_buffer: parent_buffer,
        _popup_buffer: popup_buffer,
        parent_surface_id,
        popup_surface_id,
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
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, XwmCommand::SyncClientLists { .. }))
    );
}

fn fake_snapshot() -> X11WindowSnapshot {
    X11WindowSnapshot {
        handle: X11WindowHandle::new(
            XwaylandGeneration::new(NonZeroU64::new(1).expect("nonzero")),
            100,
        ),
        surface_id: 7,
        kind: DesktopWindowKind::Managed,
        window_types: X11WindowTypes::default(),
        override_redirect: false,
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
fn xwayland_popup_map_refreshes_pointer_focus_without_pointer_motion() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let mut parent = fake_snapshot();
    parent.surface_id = fixture.parent_surface_id;
    parent.geometry.x = 40;
    parent.geometry.y = 40;
    parent.geometry.width = 2;
    parent.geometry.height = 2;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(parent));

    let parent_placement = fixture.server.renderable_surfaces()[0].placement;
    let pointer_x = f64::from(parent_placement.local_x + 1);
    let pointer_y = f64::from(parent_placement.local_y + 1);
    fixture.server.send_pointer_motion(pointer_x, pointer_y);
    assert_eq!(
        fixture
            .server
            .state
            .pointer_surface
            .as_ref()
            .map(|surface| { crate::compositor::compositor_surface_id(surface) }),
        Some(fixture.parent_surface_id)
    );

    let mut popup = fake_snapshot();
    popup.handle = X11WindowHandle::new(
        XwaylandGeneration::new(NonZeroU64::new(1).expect("generation")),
        101,
    );
    popup.surface_id = fixture.popup_surface_id;
    popup.kind = DesktopWindowKind::OverrideRedirect;
    popup.override_redirect = true;
    popup.geometry = X11Geometry {
        x: parent_placement.local_x,
        y: parent_placement.local_y,
        width: 2,
        height: 2,
    };
    let popup_handle = popup.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(popup));

    assert_eq!(
        fixture
            .server
            .state
            .pointer_surface
            .as_ref()
            .map(|surface| { crate::compositor::compositor_surface_id(surface) }),
        Some(fixture.popup_surface_id)
    );

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowWithdrawn(popup_handle));
    assert_eq!(
        fixture
            .server
            .state
            .pointer_surface
            .as_ref()
            .map(|surface| { crate::compositor::compositor_surface_id(surface) }),
        Some(fixture.parent_surface_id)
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
fn admitted_x11_window_configures_x_to_its_persisted_frame_geometry() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry.x = 0;
    snapshot.geometry.y = 0;

    let commands = fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot.clone()));
    let frame = fixture
        .server
        .state
        .x11_authoritative_geometry(snapshot.handle)
        .expect("admitted X11 geometry");

    assert!(commands.iter().any(|command| matches!(
        command,
        XwmCommand::ConfigureFrame {
            window,
            geometry,
            ..
        } if *window == snapshot.handle
            && geometry == &frame
    )));
}

#[test]
fn x11_stack_request_publishes_final_client_list_order() {
    let socket = super::unique_socket_name();
    let mut server = super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let first = fake_snapshot();
    let mut second = fake_snapshot();
    second.handle = X11WindowHandle::new(generation, 101);
    second.surface_id = 8;
    let first_id = server.state.allocate_window_id().expect("first window id");
    let second_id = server.state.allocate_window_id().expect("second window id");
    server
        .state
        .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(
            first_id,
            first.clone(),
        ))
        .expect("insert first X11 window");
    server
        .state
        .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(
            second_id,
            second.clone(),
        ))
        .expect("insert second X11 window");

    let commands = server.apply_xwayland_window_event(XwmEvent::ConfigureRequested {
        window: first.handle,
        request: X11ConfigureRequest {
            requested: first.geometry,
            fields: X11ConfigureFlags::default(),
            border_width: 0,
            sibling: Some(second.handle),
            stack_mode: Some(crate::xwayland::xwm::X11StackMode::Above),
        },
    });

    assert!(commands.iter().any(|command| matches!(
        command,
        XwmCommand::SyncClientLists { stacking, .. }
            if stacking == &vec![second.handle, first.handle]
    )));
}

#[test]
fn compositor_x11_raise_emits_restacks_and_client_list_sync() {
    let socket = super::unique_socket_name();
    let mut server = super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let first = fake_snapshot();
    let mut second = fake_snapshot();
    second.handle = X11WindowHandle::new(generation, 102);
    second.surface_id = 9;
    let first_id = server.state.allocate_window_id().expect("first window id");
    let second_id = server.state.allocate_window_id().expect("second window id");
    server
        .state
        .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(
            first_id,
            first.clone(),
        ))
        .expect("insert first X11 window");
    server
        .state
        .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(
            second_id,
            second.clone(),
        ))
        .expect("insert second X11 window");
    let _ = server.state.take_backend_commands();

    assert!(server.state.raise_window_id(first_id));
    assert!(server
        .take_xwayland_backend_commands(0)
        .iter()
        .any(|command| matches!(command, XwmCommand::RaiseAndSync { window, .. } if *window == first.handle)));
}

#[test]
fn override_redirect_configure_notify_reconciles_x_stack_order() {
    let socket = super::unique_socket_name();
    let mut server = super::OwnCompositorServer::bind_cpu_composition(&socket)
        .expect("bind fake compositor server");
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
    let mut first = fake_snapshot();
    first.kind = DesktopWindowKind::OverrideRedirect;
    first.override_redirect = true;
    let mut second = first.clone();
    second.handle = X11WindowHandle::new(generation, 103);
    second.surface_id = 10;
    let first_id = server.state.allocate_window_id().expect("first window id");
    let second_id = server.state.allocate_window_id().expect("second window id");
    server
        .state
        .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(
            first_id,
            first.clone(),
        ))
        .expect("insert first OR window");
    server
        .state
        .insert_desktop_window(crate::compositor::DesktopWindow::new_x11(
            second_id,
            second.clone(),
        ))
        .expect("insert second OR window");

    let commands = server.apply_xwayland_window_event(XwmEvent::ConfigureNotify {
        window: first.handle,
        geometry: first.geometry,
        above_sibling: Some(second.handle),
    });

    assert_eq!(server.state.window_stacking, vec![second_id, first_id]);
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, XwmCommand::SyncClientLists { .. }))
    );
}

#[test]
fn multiple_xwayland_commits_preserve_ordered_edges_without_deduplication() {
    let mut fixture = first_buffer_fixture();
    let _ = fixture.server.take_xwayland_buffer_level_events();
    let _ = fixture.server.take_xwayland_buffer_ready_events();

    let server = fixture.server;
    let (running, server_thread) = super::spawn_test_server(server);
    let qh = fixture.queue.handle();
    let second_buffer =
        fixture
            ._pool
            .create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let third_buffer =
        fixture
            ._pool
            .create_buffer(16, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    fixture.surface.attach(Some(&second_buffer), 0, 0);
    fixture.surface.damage_buffer(0, 0, 2, 2);
    fixture.surface.commit();
    fixture.surface.attach(Some(&third_buffer), 0, 0);
    fixture.surface.damage_buffer(0, 0, 2, 2);
    fixture.surface.commit();
    fixture
        .connection
        .flush()
        .expect("flush same-cycle XWayland commits");
    fixture
        .queue
        .roundtrip(&mut super::RegistryTestState::default())
        .expect("complete same-cycle XWayland commits");
    fixture.server = super::stop_test_server(running, server_thread);

    let commits = fixture.server.take_xwayland_buffer_ready_events();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].association_serial, commits[1].association_serial);
    assert!(commits[0].commit_sequence < commits[1].commit_sequence);
    assert_ne!(commits[0].buffer_id, commits[1].buffer_id);
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

    assert_eq!(server.take_xwayland_buffer_level_events().len(), 1);
    assert!(server.take_xwayland_buffer_ready_events().is_empty());
    assert!(server.renderable_surfaces().is_empty());

    let mut snapshot = fake_snapshot();
    snapshot.surface_id = surface_id;
    snapshot.geometry.x = 37;
    snapshot.geometry.y = 42;
    let commands = server.apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, XwmCommand::ConfigureFrame { .. }))
    );
    assert_eq!(server.renderable_surfaces().len(), 1);
    assert_eq!(
        server.renderable_surfaces()[0].buffer_id().get(),
        initial_buffer_id
    );
    assert_eq!(
        server.renderable_surfaces()[0].placement,
        SurfacePlacement::absolute_root_at(
            crate::compositor::render::FIRST_SURFACE_OFFSET.0,
            crate::compositor::render::FIRST_SURFACE_OFFSET.1,
        )
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
        SurfacePlacement::absolute_root_at(
            crate::compositor::render::FIRST_SURFACE_OFFSET.0,
            crate::compositor::render::FIRST_SURFACE_OFFSET.1,
        )
    );
    assert_eq!(
        fixture.server.render_generation_cause(),
        RenderGenerationCause::SurfaceCommit
    );
    assert_eq!(fixture.server.take_xwayland_buffer_ready_events().len(), 0);
}

#[test]
fn destroying_xwayland_surface_preserves_x11_window_identity() {
    let mut fixture = first_buffer_fixture();
    admit_first_buffer(&mut fixture, 37, 42);

    let handle = fake_snapshot().handle;
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("admitted X11 window");

    fixture
        .server
        .state
        .scrub_surface_lifecycle(fixture.surface_id);

    assert_eq!(
        fixture.server.state.window_id_for_x11_handle(handle),
        Some(window_id),
        "destroying a map-local Xwayland surface must not withdraw the X11 window"
    );
}

#[test]
fn xwayland_association_replacement_keeps_window_id_and_updates_root_surface() {
    let mut fixture = first_buffer_fixture();
    admit_first_buffer(&mut fixture, 37, 42);

    let snapshot = fake_snapshot();
    let handle = snapshot.handle;
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("admitted X11 window");

    fixture
        .server
        .apply_xwayland_association_event(XwmAssociationEvent::Associated {
            generation: handle.generation(),
            window: handle,
            surface_id: fixture.surface_id.saturating_add(1),
        });

    assert_eq!(
        fixture.server.state.window_id_for_x11_handle(handle),
        Some(window_id)
    );
    assert_eq!(
        fixture
            .server
            .state
            .window_id_for_surface(fixture.surface_id.saturating_add(1)),
        Some(window_id)
    );
    assert_eq!(
        fixture
            .server
            .state
            .window_id_for_surface(fixture.surface_id),
        None,
        "the old root surface must no longer own the desktop window"
    );
}

#[test]
fn xwayland_attachment_replacement_preserves_frame_and_keyboard_focus() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.parent_surface_id;
    snapshot.geometry.x = 37;
    snapshot.geometry.y = 42;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("admitted X11 window");
    let frame_placement = fixture
        .server
        .state
        .window(window_id)
        .and_then(|window| window.x11_geometry)
        .expect("persistent X11 frame geometry")
        .frame
        .placement;
    assert_eq!(
        fixture
            .server
            .state
            .focused_surface
            .as_ref()
            .map(|surface| crate::compositor::compositor_surface_id(surface)),
        Some(fixture.parent_surface_id)
    );

    fixture
        .server
        .apply_xwayland_association_event(XwmAssociationEvent::Associated {
            generation: handle.generation(),
            window: handle,
            surface_id: fixture.popup_surface_id,
        });

    assert_eq!(
        fixture.server.state.window_id_for_x11_handle(handle),
        Some(window_id)
    );
    assert_eq!(
        fixture
            .server
            .state
            .surface_placement(fixture.popup_surface_id),
        frame_placement,
        "replacement must inherit the persistent frame placement"
    );
    assert_eq!(
        fixture
            .server
            .state
            .focused_surface
            .as_ref()
            .map(|surface| crate::compositor::compositor_surface_id(surface)),
        Some(fixture.popup_surface_id),
        "keyboard focus must transfer to the replacement surface"
    );
    assert_eq!(fixture.server.state.focused_window_id, Some(window_id));

    fixture
        .server
        .apply_xwayland_association_event(XwmAssociationEvent::Removed {
            generation: handle.generation(),
            window: handle,
            surface_id: fixture.parent_surface_id,
        });
    assert_eq!(
        fixture.server.state.window_id_for_x11_handle(handle),
        Some(window_id)
    );
    assert_eq!(
        fixture
            .server
            .state
            .focused_surface
            .as_ref()
            .map(|surface| crate::compositor::compositor_surface_id(surface)),
        Some(fixture.popup_surface_id),
        "late removal of the old attachment must not clear replacement focus"
    );
}

#[test]
fn invalid_xwayland_attachment_does_not_withdraw_the_current_surface() {
    let mut fixture = first_buffer_fixture();
    admit_first_buffer(&mut fixture, 37, 42);
    let handle = fake_snapshot().handle;
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("admitted X11 window");

    fixture
        .server
        .apply_xwayland_association_event(XwmAssociationEvent::Associated {
            generation: handle.generation(),
            window: handle,
            surface_id: 9999,
        });

    assert_eq!(
        fixture.server.state.window_id_for_x11_handle(handle),
        Some(window_id)
    );
    assert_eq!(
        fixture
            .server
            .state
            .window(window_id)
            .map(|window| window.root_surface_id),
        Some(fixture.surface_id),
        "invalid replacement must leave the existing attachment active"
    );
    assert!(
        fixture
            .server
            .renderable_surfaces()
            .iter()
            .any(|surface| surface.surface_id == fixture.surface_id)
    );
}

#[test]
fn destroyed_minimized_xwayland_surface_is_not_restored_as_stale_content() {
    let mut fixture = first_buffer_fixture();
    admit_first_buffer(&mut fixture, 37, 42);

    let handle = fake_snapshot().handle;
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("admitted X11 window");
    assert!(fixture.server.state.minimize_desktop_window(window_id));
    assert!(fixture.server.renderable_surfaces().is_empty());

    fixture
        .server
        .state
        .unregister_surface_resource(fixture.surface_id);
    assert!(
        fixture
            .server
            .state
            .restore_minimized_desktop_window(window_id)
    );
    assert!(
        fixture
            .server
            .renderable_surfaces()
            .iter()
            .all(|surface| surface.surface_id != fixture.surface_id)
    );
}

#[test]
fn replacement_surface_commit_stays_hidden_while_x11_window_is_minimized() {
    let mut fixture = first_buffer_fixture();
    admit_first_buffer(&mut fixture, 37, 42);

    let handle = fake_snapshot().handle;
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("admitted X11 window");
    assert!(fixture.server.state.minimize_desktop_window(window_id));

    let replacement_surface_id = fixture.surface_id.saturating_add(1);
    fixture
        .server
        .state
        .attach_x11_surface(handle, replacement_surface_id)
        .expect("attach replacement surface");
    let pending = fixture
        .server
        .state
        .current_surface_buffers
        .get(&fixture.surface_id)
        .cloned()
        .expect("replacement commit buffer");
    fixture.server.state.commit_xwayland_surface_buffer(
        replacement_surface_id,
        pending,
        Vec::new(),
        SurfacePublicationSource::Immediate,
    );

    assert!(fixture.server.renderable_surfaces().is_empty());
}

#[test]
fn restore_adopts_active_xwayland_surface_after_old_surface_is_retired() {
    let mut fixture = first_buffer_fixture();
    admit_first_buffer(&mut fixture, 37, 42);

    let handle = fake_snapshot().handle;
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("admitted X11 window");
    assert!(fixture.server.state.minimize_desktop_window(window_id));

    let replacement_surface_id = fixture.surface_id.saturating_add(1);
    fixture.server.state.surface_role_lifecycles.insert(
        replacement_surface_id,
        SurfaceRoleLifecycle {
            permanent: Some(PermanentSurfaceRole::Xwayland),
            live_instance: Some(LiveRoleInstance::Xwayland),
            xdg_association: false,
        },
    );
    fixture.server.state.xwayland.surface_states.insert(
        replacement_surface_id,
        XwaylandSurfaceState {
            generation: handle.generation(),
            pending_serial: None,
            committed_serial: None,
            association_object_alive: true,
        },
    );
    fixture
        .server
        .state
        .withdraw_xwayland_surface_content(fixture.surface_id);
    fixture
        .server
        .state
        .attach_x11_surface(handle, replacement_surface_id)
        .expect("attach replacement surface");
    let pending = fixture
        .server
        .state
        .current_surface_buffers
        .get(&fixture.surface_id)
        .cloned()
        .expect("replacement commit buffer");
    fixture.server.state.commit_xwayland_surface_buffer(
        replacement_surface_id,
        pending,
        Vec::new(),
        SurfacePublicationSource::Immediate,
    );

    assert!(fixture.server.renderable_surfaces().is_empty());
    assert!(
        fixture
            .server
            .state
            .restore_minimized_desktop_window(window_id)
    );
    assert_eq!(
        fixture.server.state.window_id_for_x11_handle(handle),
        Some(window_id)
    );
    assert!(
        fixture
            .server
            .renderable_surfaces()
            .iter()
            .any(|surface| surface.surface_id == replacement_surface_id)
    );
    assert!(
        fixture
            .server
            .renderable_surfaces()
            .iter()
            .all(|surface| surface.surface_id != fixture.surface_id)
    );
}

#[test]
fn x11_window_ready_initial_focus_activates_normal_toplevel() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let handle = snapshot.handle;

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));

    assert!(
        fixture
            .server
            .take_xwayland_backend_commands(0)
            .iter()
            .any(|command| matches!(
                command,
                XwmCommand::Focus {
                    window: Some(window),
                    ..
                } if *window == handle
            ))
    );
}

#[test]
fn x11_window_ready_initial_focus_skips_auxiliary_popup() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.window_types = X11WindowTypes::new(vec![X11WindowType::PopupMenu]);
    let handle = snapshot.handle;

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));

    assert!(
        fixture
            .server
            .take_xwayland_backend_commands(0)
            .iter()
            .all(|command| !matches!(
                command,
                XwmCommand::Focus {
                    window: Some(window),
                    ..
                } if *window == handle
            ))
    );
}

#[test]
fn late_kind_change_removes_normal_membership_without_focus_flash() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let handle = snapshot.handle;

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let _ = fixture.server.take_xwayland_backend_commands(0);

    let metadata_commands = fixture
        .server
        .apply_xwayland_window_event(XwmEvent::MetadataChanged {
            window: handle,
            delta: crate::xwayland::xwm::X11MetadataDelta::Kind(
                DesktopWindowKind::OverrideRedirect,
            ),
        });

    assert!(
        metadata_commands
            .iter()
            .all(|command| !matches!(command, XwmCommand::Raise(_)))
    );

    assert!(fixture.server.state.x11_client_lists().0.is_empty());
    assert!(
        fixture
            .server
            .take_xwayland_backend_commands(0)
            .iter()
            .all(|command| !matches!(
                command,
                XwmCommand::Focus {
                    window: Some(window),
                    ..
                } if *window == handle
            ))
    );
}

#[test]
fn x11_client_moveresize_requires_held_button_and_starts_move() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: 100,
        y: 100,
        width: 2,
        height: 2,
    };
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));

    let request = XwmEvent::MoveResizeRequested {
        window: handle,
        request: X11MoveResizeRequest {
            root_x: 101,
            root_y: 101,
            direction: X11MoveResizeDirection::Move,
            button: 1,
            source: 1,
        },
    };
    fixture.server.apply_xwayland_window_event(request.clone());
    assert!(!fixture.server.window_interaction_active());

    fixture.server.send_pointer_motion(101.0, 101.0);
    fixture.server.send_pointer_button(0x110, true);
    fixture.server.apply_xwayland_window_event(request);

    let interaction = fixture
        .server
        .window_interaction_debug_snapshot()
        .expect("X11 client move interaction");
    assert_eq!(
        interaction.kind,
        crate::compositor::WindowInteractionKind::Move
    );
    assert_eq!(
        interaction.source,
        crate::compositor::WindowInteractionSource::X11NetWmMoveResize
    );
    assert_eq!(interaction.trigger_button, Some(0x110));
}

#[test]
fn x11_moveresize_uses_seat_button_state_across_surface_boundary() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));

    fixture.server.send_pointer_motion(101.0, 101.0);
    fixture.server.send_pointer_button(0x110, true);
    fixture
        .server
        .state
        .held_pointer_buttons
        .last_mut()
        .expect("held seat button")
        .root_surface_id = fixture.surface_id.saturating_add(1);

    let result = fixture.server.state.begin_x11_client_window_interaction(
        handle,
        101.0,
        101.0,
        crate::compositor::WindowInteractionKind::Move,
        1,
    );
    assert_eq!(result, X11MoveResizeBeginResult::Began);
}

#[test]
fn release_after_client_message_does_not_retroactively_reject_request() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    fixture.server.send_pointer_motion(101.0, 101.0);
    fixture.server.send_pointer_button(0x110, true);

    let result = fixture.server.state.begin_x11_client_window_interaction(
        handle,
        101.0,
        101.0,
        crate::compositor::WindowInteractionKind::Move,
        1,
    );
    assert_eq!(result, X11MoveResizeBeginResult::Began);
    assert!(fixture.server.end_window_interaction_for_button(0x110));
    fixture.server.send_pointer_button(0x110, false);
    assert!(!fixture.server.window_interaction_active());
}

#[test]
fn release_before_client_message_rejects_stale_request() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    fixture.server.send_pointer_motion(101.0, 101.0);
    fixture.server.send_pointer_button(0x110, true);
    fixture.server.send_pointer_button(0x110, false);

    let result = fixture.server.state.begin_x11_client_window_interaction(
        handle,
        101.0,
        101.0,
        crate::compositor::WindowInteractionKind::Move,
        1,
    );
    assert_eq!(result, X11MoveResizeBeginResult::NoPressedButton);
}

#[test]
fn moveresize_button_mismatch_is_observable() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    fixture.server.send_pointer_motion(101.0, 101.0);
    fixture.server.send_pointer_button(0x110, true);

    let result = fixture.server.state.begin_x11_client_window_interaction(
        handle,
        101.0,
        101.0,
        crate::compositor::WindowInteractionKind::Move,
        3,
    );
    assert_eq!(result, X11MoveResizeBeginResult::ButtonMismatch);
}

#[test]
fn x11_client_moveresize_maps_edges_and_accepts_cancel() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: 100,
        y: 100,
        width: 2,
        height: 2,
    };
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    fixture.server.send_pointer_motion(101.0, 101.0);
    fixture.server.send_pointer_button(0x110, true);

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::MoveResizeRequested {
            window: handle,
            request: X11MoveResizeRequest {
                root_x: 101,
                root_y: 101,
                direction: X11MoveResizeDirection::TopLeft,
                button: 1,
                source: 1,
            },
        });
    assert_eq!(
        fixture
            .server
            .window_interaction_debug_snapshot()
            .expect("X11 client resize interaction")
            .kind,
        crate::compositor::WindowInteractionKind::Resize(crate::compositor::ResizeEdges::new(
            true, false, true, false
        ))
    );

    let other_handle = X11WindowHandle::new(
        XwaylandGeneration::new(NonZeroU64::new(1).expect("generation")),
        handle.xid().saturating_add(1),
    );
    assert!(
        !fixture
            .server
            .state
            .cancel_x11_client_window_interaction(other_handle)
    );
    assert!(fixture.server.window_interaction_active());

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::MoveResizeRequested {
            window: handle,
            request: X11MoveResizeRequest {
                root_x: 101,
                root_y: 101,
                direction: X11MoveResizeDirection::Cancel,
                button: 1,
                source: 1,
            },
        });
    assert!(!fixture.server.window_interaction_active());
}

#[test]
fn x11_partial_moveresize_preserves_unrequested_geometry() {
    let mut fixture = first_buffer_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.surface_id;
    snapshot.geometry = X11Geometry {
        x: 100,
        y: 120,
        width: 640,
        height: 480,
    };
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));

    let commands = fixture
        .server
        .apply_xwayland_window_event(XwmEvent::ConfigureRequested {
            window: handle,
            request: X11ConfigureRequest {
                requested: X11Geometry {
                    x: 200,
                    y: 0,
                    width: 0,
                    height: 0,
                },
                fields: X11ConfigureFlags {
                    x: true,
                    ..X11ConfigureFlags::default()
                },
                border_width: 0,
                sibling: None,
                stack_mode: None,
            },
        });

    assert!(matches!(
        commands.as_slice(),
        [XwmCommand::Configure { geometry, fields, .. }]
            if *geometry == X11Geometry {
                x: 200,
                y: crate::compositor::render::FIRST_SURFACE_OFFSET.1,
                width: 640,
                height: 480,
            } && fields.x
    ));
}

#[test]
fn x11_configure_notify_does_not_mutate_committed_buffer_extent() {
    let mut fixture = first_buffer_fixture();
    admit_first_buffer(&mut fixture, 100, 120);
    let handle = fake_snapshot().handle;

    assert_eq!(
        fixture.server.renderable_surfaces()[0].width,
        2,
        "fixture starts with a 2-pixel committed buffer"
    );
    assert_eq!(fixture.server.renderable_surfaces()[0].height, 2);

    fixture.server.state.reconcile_x11_configure_notify(
        handle,
        X11Geometry {
            x: 200,
            y: 220,
            width: 1200,
            height: 900,
        },
    );

    let committed = &fixture.server.renderable_surfaces()[0];
    assert_eq!(committed.width, 2);
    assert_eq!(committed.height, 2);
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
fn retained_buffer_uses_compositor_placement_for_managed_window() {
    let mut fixture = first_buffer_fixture();
    admit_first_buffer(&mut fixture, 123, 456);

    assert_eq!(
        fixture.server.renderable_surfaces()[0].placement,
        SurfacePlacement::absolute_root_at(
            crate::compositor::render::FIRST_SURFACE_OFFSET.0,
            crate::compositor::render::FIRST_SURFACE_OFFSET.1,
        )
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
