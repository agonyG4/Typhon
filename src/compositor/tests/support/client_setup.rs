#![allow(unused_imports)]
use super::super::*;
use super::{
    clipboard_dmabuf::*, frame_buffer_client::*, input_client::*, locked_relative::*,
    output_bindings::*, registry_state::*, server_runtime::*, subsurface_client::*, window_ops::*,
};
pub(in crate::compositor::tests) fn read_registry_globals(
    socket_path: &PathBuf,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, _queue) = registry_queue_init::<RegistryTestState>(&connection)?;

    Ok(globals
        .contents()
        .clone_list()
        .into_iter()
        .map(|global| global.interface)
        .collect())
}

pub(in crate::compositor::tests) fn request_presentation_feedback(
    socket_path: &PathBuf,
) -> Result<Connection, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let presentation: client_wp_presentation::WpPresentation = globals.bind(&qh, 1..=2, ())?;
    let surface = compositor.create_surface(&qh, ());
    let _feedback = presentation.feedback(&surface, &qh, ());
    connection.flush()?;
    Ok(connection)
}

pub(in crate::compositor::tests) fn retain_live_test_connection(connection: Connection) {
    static RETAINED_CONNECTIONS: std::sync::OnceLock<Mutex<Vec<Connection>>> =
        std::sync::OnceLock::new();
    RETAINED_CONNECTIONS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap()
        .push(connection);
}

pub(in crate::compositor::tests) fn create_surface_with_presentation_feedback_and_present(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    completion: ServerCommand,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let presentation: client_wp_presentation::WpPresentation = globals.bind(&qh, 1..=2, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let _feedback = presentation.feedback(&surface, &qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(completion)?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_surface_with_unpresented_presentation_feedback(
    socket_path: &PathBuf,
) -> Result<Connection, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let presentation: client_wp_presentation::WpPresentation = globals.bind(&qh, 1..=2, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let _feedback = presentation.feedback(&surface, &qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    Ok(connection)
}

pub(in crate::compositor::tests) fn create_client_toplevel(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());

    toplevel.set_app_id("oblivion.test".to_string());
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

pub(in crate::compositor::tests) fn create_configured_client_toplevel(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_configured_client_toplevel_at_version(socket_path, 6)
}

pub(in crate::compositor::tests) fn create_configured_client_toplevel_at_version(
    socket_path: &PathBuf,
    version: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=version, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=version, ())?;
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_app_id("oblivion.configure-test".to_string());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_toplevel_and_check_initial_commit_configure_order(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_app_id("oblivion.initial-configure-order".to_string());
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    state.configured_before_initial_commit = state.surface_configured || state.toplevel_configured;

    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    state.configured_after_initial_commit = state.surface_configured && state.toplevel_configured;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_popup_and_check_initial_commit_configure_order(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let mut state = RegistryTestState::default();
    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-initial-configure-parent".to_string());
    parent.commit();
    connection.flush()?;

    queue.roundtrip(&mut state)?;
    commit_test_buffered_surface_after_initial_configure(
        &parent,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        120,
        90,
    )?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    state.surface_configured = false;
    state.popup_configured = false;
    state.toplevel_configured = false;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(60, 40);
    positioner.set_anchor_rect(10, 20, 30, 10);
    positioner.set_anchor(client_xdg_positioner::Anchor::BottomRight);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    let _popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    state.configured_before_initial_commit = state.surface_configured || state.popup_configured;

    popup_surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    state.configured_after_initial_commit = state.surface_configured && state.popup_configured;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_client_toplevel_with_configured_popup(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let mut state = RegistryTestState::default();
    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-parent".to_string());
    commit_test_buffered_surface_after_initial_configure(
        &parent,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        120,
        90,
    )?;
    connection.flush()?;

    queue.roundtrip(&mut state)?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(60, 40);
    positioner.set_anchor_rect(10, 20, 30, 10);
    positioner.set_anchor(client_xdg_positioner::Anchor::BottomRight);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    positioner.set_offset(3, 4);
    let _popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    commit_test_buffered_surface_after_initial_configure(
        &popup_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        60,
        40,
    )?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_client_popup_with_constrained_positioner(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let mut state = RegistryTestState::default();
    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-constraint-parent".to_string());
    commit_test_buffered_surface_after_initial_configure(
        &parent,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        120,
        90,
    )?;
    connection.flush()?;

    queue.roundtrip(&mut state)?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(80, 50);
    positioner.set_parent_size(120, 90);
    positioner.set_anchor_rect(110, 80, 10, 10);
    positioner.set_anchor(client_xdg_positioner::Anchor::BottomRight);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    positioner.set_constraint_adjustment(
        client_xdg_positioner::ConstraintAdjustment::SlideX
            | client_xdg_positioner::ConstraintAdjustment::SlideY,
    );
    let _popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    commit_test_buffered_surface_after_initial_configure(
        &popup_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        80,
        50,
    )?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_client_popup_then_reposition(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let mut state = RegistryTestState::default();
    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-reposition-parent".to_string());
    commit_test_buffered_surface_after_initial_configure(
        &parent,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        120,
        90,
    )?;
    connection.flush()?;

    queue.roundtrip(&mut state)?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let initial_positioner = wm_base.create_positioner(&qh, ());
    initial_positioner.set_size(60, 40);
    initial_positioner.set_anchor_rect(10, 20, 30, 10);
    initial_positioner.set_anchor(client_xdg_positioner::Anchor::BottomRight);
    initial_positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    let popup =
        popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &initial_positioner, &qh, ());
    commit_test_buffered_surface_after_initial_configure(
        &popup_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        60,
        40,
    )?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    let repositioner = wm_base.create_positioner(&qh, ());
    repositioner.set_size(50, 30);
    repositioner.set_anchor_rect(5, 7, 1, 1);
    repositioner.set_anchor(client_xdg_positioner::Anchor::TopLeft);
    repositioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    repositioner.set_offset(1, 1);
    popup.reposition(&repositioner, 77);
    commit_test_buffered_surface_after_configure(
        &popup_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        50,
        30,
    )?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_client_popup_with_window_geometry(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let mut state = RegistryTestState::default();
    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-window-geometry-parent".to_string());
    parent_xdg_surface.set_window_geometry(8, 9, 100, 80);
    commit_test_buffered_surface_after_initial_configure(
        &parent,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        120,
        90,
    )?;
    connection.flush()?;

    queue.roundtrip(&mut state)?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(40, 30);
    positioner.set_anchor_rect(10, 20, 1, 1);
    positioner.set_anchor(client_xdg_positioner::Anchor::TopLeft);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    let _popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    popup_xdg_surface.set_window_geometry(2, 3, 40, 30);
    commit_test_buffered_surface_after_initial_configure(
        &popup_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        40,
        30,
    )?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_non_reactive_popup_then_set_window_geometry(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let mut state = RegistryTestState::default();
    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-gecko-parent".to_string());
    commit_test_buffered_surface_after_initial_configure(
        &parent,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        120,
        90,
    )?;
    connection.flush()?;

    queue.roundtrip(&mut state)?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(1, 1);
    positioner.set_anchor_rect(0, 0, 1, 1);
    let _popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    popup_surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    assert_eq!(state.popup_configure_count, 1);

    state.popup_configure_count = 0;
    popup_xdg_surface.set_window_geometry(0, 0, 177, 493);
    commit_test_buffered_surface_after_initial_configure(
        &popup_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        177,
        493,
    )?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_grabbed_popup_then_release_under_cursor(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(RegistryTestState, u32), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let _pointer = seat.get_pointer(&qh, ());
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (_parent_surface, parent_xdg_surface, parent_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 120, 90)?;
    parent_toplevel.set_app_id("oblivion.popup-grab-parent".to_string());
    _parent_surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0 + 47),
        y: f64::from(render::FIRST_SURFACE_OFFSET.1 + 37),
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x111,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let serial = state
        .pointer_button_serial
        .ok_or_else(|| io::Error::other("pointer button serial was not delivered"))?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_surface_id = popup_surface.id().protocol_id();
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(60, 40);
    positioner.set_anchor_rect(10, 20, 30, 10);
    positioner.set_anchor(client_xdg_positioner::Anchor::BottomRight);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    positioner.set_offset(3, 4);
    let popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    popup.grab(&seat, serial);
    commit_test_buffered_surface_after_initial_configure(
        &popup_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        60,
        40,
    )?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    state.pointer_button_surface_id = None;
    commands.send(ServerCommand::PointerButton {
        button: 0x111,
        pressed: false,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok((state, popup_surface_id))
}

pub(in crate::compositor::tests) fn create_grabbed_popup_under_cursor(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(RegistryTestState, u32), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let _pointer = seat.get_pointer(&qh, ());
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (_parent_surface, parent_xdg_surface, parent_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 120, 90)?;
    parent_toplevel.set_app_id("oblivion.popup-grab-parent".to_string());
    _parent_surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0 + 47),
        y: f64::from(render::FIRST_SURFACE_OFFSET.1 + 37),
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x111,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let serial = state
        .pointer_button_serial
        .ok_or_else(|| io::Error::other("pointer button serial was not delivered"))?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_surface_id = popup_surface.id().protocol_id();
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(60, 40);
    positioner.set_anchor_rect(10, 20, 30, 10);
    positioner.set_anchor(client_xdg_positioner::Anchor::BottomRight);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    positioner.set_offset(3, 4);
    let popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    popup.grab(&seat, serial);
    commit_test_buffered_surface_after_initial_configure(
        &popup_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        60,
        40,
    )?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    Ok((state, popup_surface_id))
}

pub(in crate::compositor::tests) fn create_grabbed_popup_then_click_outside(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let _pointer = seat.get_pointer(&qh, ());
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (parent_surface, parent_xdg_surface, parent_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 120, 90)?;
    parent_toplevel.set_app_id("oblivion.popup-grab-dismiss-parent".to_string());
    parent_surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0 + 47),
        y: f64::from(render::FIRST_SURFACE_OFFSET.1 + 37),
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x111,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let serial = state
        .pointer_button_serial
        .ok_or_else(|| io::Error::other("pointer button serial was not delivered"))?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(60, 40);
    positioner.set_anchor_rect(10, 20, 30, 10);
    positioner.set_anchor(client_xdg_positioner::Anchor::BottomRight);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    positioner.set_offset(3, 4);
    let popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    popup.grab(&seat, serial);
    commit_test_buffered_surface_after_initial_configure(
        &popup_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        60,
        40,
    )?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::PointerButton {
        button: 0x111,
        pressed: false,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    state.popup_done = false;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0 + 5),
        y: f64::from(render::FIRST_SURFACE_OFFSET.1 + 5),
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok(state)
}

pub(in crate::compositor::tests) fn create_grabbed_popup_then_axis_outside(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let _pointer = seat.get_pointer(&qh, ());
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (parent_surface, parent_xdg_surface, parent_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 120, 90)?;
    parent_toplevel.set_app_id("oblivion.popup-grab-axis-parent".to_string());
    parent_surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0 + 47),
        y: f64::from(render::FIRST_SURFACE_OFFSET.1 + 37),
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x111,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let serial = state
        .pointer_button_serial
        .ok_or_else(|| io::Error::other("pointer button serial was not delivered"))?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(60, 40);
    positioner.set_anchor_rect(10, 20, 30, 10);
    positioner.set_anchor(client_xdg_positioner::Anchor::BottomRight);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    positioner.set_offset(3, 4);
    let popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    popup.grab(&seat, serial);
    commit_test_buffered_surface_after_initial_configure(
        &popup_surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        60,
        40,
    )?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    state.pointer_axis = false;
    state.pointer_vertical_axis = None;
    commands.send(ServerCommand::PointerButton {
        button: 0x111,
        pressed: false,
    })?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0 + 5),
        y: f64::from(render::FIRST_SURFACE_OFFSET.1 + 5),
    })?;
    commands.send(ServerCommand::PointerAxis {
        horizontal: 0.0,
        vertical: 15.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok(state)
}

pub(in crate::compositor::tests) fn create_client_surface_and_wait_for_enter(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let _output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    assign_test_toplevel(&globals, &qh, &surface)?;
    let mut state = RegistryTestState::default();
    commit_test_buffered_surface_after_initial_configure(
        &surface,
        &shm,
        &qh,
        &connection,
        &mut queue,
        &mut state,
        40,
        30,
    )?;
    connection.flush()?;

    queue.roundtrip(&mut state)?;
    Ok(state)
}
