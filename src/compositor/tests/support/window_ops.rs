#![allow(unused_imports)]
use super::super::*;
use super::{
    client_setup::*, clipboard_dmabuf::*, frame_buffer_client::*, input_client::*,
    locked_relative::*, output_bindings::*, registry_state::*, server_runtime::*,
    subsurface_client::*,
};
pub(in crate::compositor::tests) fn create_idle_inhibitor_for_surface_and_capture_state(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let idle_manager: client_zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    let _inhibitor = idle_manager.create_inhibitor(&surface, &qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    wait_for_server_commands(commands);
    let (reply, rx) = mpsc::channel();
    commands.send(ServerCommand::CaptureIdleInhibited(reply))?;
    Ok(rx.recv_timeout(Duration::from_secs(1))?)
}

pub(in crate::compositor::tests) fn create_client_surface_with_viewport_destination(
    socket_path: &PathBuf,
    buffer_width: u32,
    buffer_height: u32,
    destination_width: u32,
    destination_height: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    assign_test_toplevel(&globals, &qh, &surface)?;
    let viewport = viewporter.get_viewport(&surface, &qh, ());
    viewport.set_destination(destination_width as i32, destination_height as i32);
    commit_test_buffered_surface(
        &surface,
        &shm,
        &qh,
        buffer_width as usize,
        buffer_height as usize,
    )?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_client_surface_with_buffer_offset(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    assign_test_toplevel(&globals, &qh, &surface)?;
    attach_test_buffered_surface(&surface, &shm, &qh, 40, 30)?;
    surface.offset(5, 7);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(())
}

pub(in crate::compositor::tests) fn create_configured_client_toplevel_then_resize_focused(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    width: u32,
    height: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_app_id("oblivion.resize-test".to_string());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::ResizeFocusedTo { width, height })?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_then_resize_drag(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let width = 100;
    let height = 80;
    let pixels = vec![0xff20_3040; width * height];
    let file = create_test_shm_file(&pixels)?;
    let pool = shm.create_pool(file.as_fd(), (pixels.len() * 4) as i32, &qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        (width * 4) as i32,
        client_wl_shm::Format::Argb8888,
        &qh,
        (),
    );

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, width as i32, height as i32);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginResize {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 90.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 70.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 290.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 190.0,
    })?;
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_then_toggle_maximize(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_buffered_toplevel_then_window_commands(
        socket_path,
        commands,
        &[ServerCommand::ToggleMaximizeFocused],
    )
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_then_toggle_maximize_twice(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_buffered_toplevel_then_window_commands(
        socket_path,
        commands,
        &[
            ServerCommand::ToggleMaximizeFocused,
            ServerCommand::ToggleMaximizeFocused,
        ],
    )
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_then_toggle_fullscreen(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_buffered_toplevel_then_window_commands(
        socket_path,
        commands,
        &[ServerCommand::ToggleFullscreenFocused],
    )
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_then_window_commands(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    window_commands: &[ServerCommand],
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    for command in window_commands {
        commands.send(command.clone())?;
        wait_for_server_commands(commands);
        queue.roundtrip(&mut state)?;
    }
    Ok(state)
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_then_coalesced_resize_drag(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 314.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 214.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 330.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 224.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_then_active_resize_configure(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    commands.send(ServerCommand::PrepareFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_then_resize_drag_without_client_commit_between_frames(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    commands.send(ServerCommand::PrepareFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 384.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 264.0,
    })?;
    commands.send(ServerCommand::PrepareFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_then_queue_resize_configure_and_capture_pending_frame_work(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    wait_for_server_commands(commands);
    let pending = capture_pending_frame_work(commands);
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok(pending)
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_then_queue_resize_configure_and_unmap(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    wait_for_server_commands(commands);
    surface.attach(None, 0, 0);
    surface.commit();
    connection.flush()?;
    wait_for_server_commands(commands);
    let pending = capture_pending_frame_work(commands);
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok(pending)
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_then_prepare_queued_resize_configure_and_capture_pending_frame_work(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(bool, bool), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    wait_for_server_commands(commands);
    let before_prepare = capture_pending_frame_work(commands);
    commands.send(ServerCommand::PrepareFrame)?;
    wait_for_server_commands(commands);
    let after_prepare = capture_pending_frame_work(commands);
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok((before_prepare, after_prepare))
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_then_resize_drag_and_release(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let resized_width = usize::try_from(state.toplevel_width)?;
    let resized_height = usize::try_from(state.toplevel_height)?;
    commit_test_buffered_surface(&surface, &shm, &qh, resized_width, resized_height)?;
    connection.flush()?;
    wait_for_server_commands(commands);

    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_then_alt_top_left_resize_drag_and_release(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginResize {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 40.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 40.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0),
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 10.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    commit_test_buffered_surface(
        &surface,
        &shm,
        &qh,
        usize::try_from(state.toplevel_width)?,
        usize::try_from(state.toplevel_height)?,
    )?;
    connection.flush()?;
    wait_for_server_commands(commands);

    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_csd_toplevel_then_resize_drag_commit_buffer_margin_and_release(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 332, 242)?;
    xdg_surface.set_window_geometry(16, 10, 300, 200);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginResize {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 200.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 120.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 240.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 150.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    xdg_surface.set_window_geometry(16, 10, state.toplevel_width, state.toplevel_height);
    commit_test_buffered_surface(
        &surface,
        &shm,
        &qh,
        usize::try_from(state.toplevel_width + 32)?,
        usize::try_from(state.toplevel_height + 42)?,
    )?;
    connection.flush()?;
    wait_for_server_commands(commands);

    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn capture_csd_resize_regression_snapshot(
    commands: &Sender<ServerCommand>,
    state: &RegistryTestState,
) -> CsdResizeRegressionSnapshot {
    CsdResizeRegressionSnapshot {
        toplevel_width: state.toplevel_width,
        toplevel_height: state.toplevel_height,
        toplevel_configure_count: state.toplevel_configure_count,
        surfaces: capture_renderable_surface_snapshot(commands),
        visual: capture_toplevel_visual_geometry(commands),
        window_geometry: capture_committed_window_geometry(commands),
    }
}

pub(in crate::compositor::tests) fn capture_csd_consecutive_resize_regression_snapshots(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<CsdConsecutiveResizeSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 332, 242)?;
    xdg_surface.set_window_geometry(16, 10, 300, 200);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::BeginResize {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 200.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 120.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 240.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 150.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    xdg_surface.set_window_geometry(16, 10, state.toplevel_width, state.toplevel_height);
    commit_test_buffered_surface(
        &surface,
        &shm,
        &qh,
        usize::try_from(state.toplevel_width + 32)?,
        usize::try_from(state.toplevel_height + 42)?,
    )?;
    connection.flush()?;
    wait_for_server_commands(commands);
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    xdg_surface.set_window_geometry(16, 10, state.toplevel_width, state.toplevel_height);
    surface.commit();
    connection.flush()?;
    wait_for_server_commands(commands);
    let first_final = capture_csd_resize_regression_snapshot(commands, &state);

    commands.send(ServerCommand::BeginResize {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 300.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 180.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 296.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 180.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let second_preview = capture_csd_resize_regression_snapshot(commands, &state);

    xdg_surface.set_window_geometry(16, 10, state.toplevel_width, state.toplevel_height);
    commit_test_buffered_surface(
        &surface,
        &shm,
        &qh,
        usize::try_from(state.toplevel_width + 32)?,
        usize::try_from(state.toplevel_height + 42)?,
    )?;
    connection.flush()?;
    wait_for_server_commands(commands);
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    xdg_surface.set_window_geometry(16, 10, state.toplevel_width, state.toplevel_height);
    surface.commit();
    connection.flush()?;
    wait_for_server_commands(commands);
    let second_final = capture_csd_resize_regression_snapshot(commands, &state);

    commands.send(ServerCommand::BeginResize {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 300.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 180.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 297.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 180.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let third_preview = capture_csd_resize_regression_snapshot(commands, &state);

    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);

    Ok(CsdConsecutiveResizeSnapshots {
        first_final,
        second_preview,
        second_final,
        third_preview,
    })
}

pub(in crate::compositor::tests) fn capture_csd_top_left_resize_regression_snapshot(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<CsdTopLeftResizeSnapshot, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 332, 242)?;
    xdg_surface.set_window_geometry(16, 10, 300, 200);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::BeginResize {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 200.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 120.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 240.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 150.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    xdg_surface.set_window_geometry(16, 10, state.toplevel_width, state.toplevel_height);
    commit_test_buffered_surface(
        &surface,
        &shm,
        &qh,
        usize::try_from(state.toplevel_width + 32)?,
        usize::try_from(state.toplevel_height + 42)?,
    )?;
    connection.flush()?;
    wait_for_server_commands(commands);
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    xdg_surface.set_window_geometry(16, 10, state.toplevel_width, state.toplevel_height);
    surface.commit();
    connection.flush()?;
    wait_for_server_commands(commands);
    let first_final = capture_csd_resize_regression_snapshot(commands, &state);

    commands.send(ServerCommand::BeginResize {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 8.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 8.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 12.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 13.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let top_left_preview = capture_csd_resize_regression_snapshot(commands, &state);

    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);

    Ok(CsdTopLeftResizeSnapshot {
        first_final,
        top_left_preview,
    })
}

pub(in crate::compositor::tests) fn capture_csd_window_geometry_pending_and_committed_resize_snapshots(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<WindowGeometryCommitSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 332, 242)?;
    xdg_surface.set_window_geometry(16, 10, 300, 200);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginResize {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 200.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 120.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 240.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 150.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let before_pending = capture_renderable_surface_snapshot(commands);
    let before_pending_geometry = capture_committed_window_geometry(commands);
    xdg_surface.set_window_geometry(16, 30, state.toplevel_width, state.toplevel_height);
    connection.flush()?;
    wait_for_server_commands(commands);
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 260.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 165.0,
    })?;
    wait_for_server_commands(commands);
    let after_pending_without_commit = capture_renderable_surface_snapshot(commands);
    let after_pending_without_commit_geometry = capture_committed_window_geometry(commands);

    surface.commit();
    connection.flush()?;
    wait_for_server_commands(commands);
    let after_geometry_commit = capture_renderable_surface_snapshot(commands);
    let after_geometry_commit_geometry = capture_committed_window_geometry(commands);

    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok(WindowGeometryCommitSnapshots {
        before_pending,
        before_pending_geometry,
        after_pending_without_commit,
        after_pending_without_commit_geometry,
        after_geometry_commit,
        after_geometry_commit_geometry,
    })
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_then_measure_configure_only_resize_generation(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(u64, u64), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    let before_resize = capture_render_generation(commands);

    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    let after_resize = capture_render_generation(commands);

    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok((before_resize, after_resize))
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_request_move_and_drag(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let _pointer = seat.get_pointer(&qh, ());
    let (surface, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 100, 80)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 12.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    queue.roundtrip(&mut state)?;

    let serial = state
        .pointer_button_serial
        .ok_or_else(|| io::Error::other("pointer button serial was not delivered"))?;
    toplevel._move(&seat, serial);
    connection.flush()?;
    wait_for_server_commands(commands);
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 52.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 42.0,
    })?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: false,
    })?;
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok(state)
}

pub(in crate::compositor::tests) fn create_toplevel_request_move_from_client_chrome_surface(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let _pointer = seat.get_pointer(&qh, ());

    let (surface, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 100, 80)?;
    surface.commit();

    let chrome_width = 120;
    let chrome_height = 20;
    let chrome_pixels = vec![0xff70_7070; chrome_width * chrome_height];
    let chrome_file = create_test_shm_file(&chrome_pixels)?;
    let chrome_pool = shm.create_pool(
        chrome_file.as_fd(),
        (chrome_pixels.len() * 4) as i32,
        &qh,
        (),
    );
    let chrome_buffer = chrome_pool.create_buffer(
        0,
        chrome_width as i32,
        chrome_height as i32,
        (chrome_width * 4) as i32,
        client_wl_shm::Format::Argb8888,
        &qh,
        (),
    );
    let chrome = compositor.create_surface(&qh, ());
    let chrome_subsurface = subcompositor.get_subsurface(&chrome, &surface, &qh, ());
    chrome_subsurface.set_position(render::SURFACE_CASCADE_STEP, render::SURFACE_CASCADE_STEP);
    chrome.attach(Some(&chrome_buffer), 0, 0);
    chrome.damage_buffer(0, 0, chrome_width as i32, chrome_height as i32);
    chrome.commit();
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0 + render::SURFACE_CASCADE_STEP) + 12.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1 + render::SURFACE_CASCADE_STEP) + 14.0,
    })?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    queue.roundtrip(&mut state)?;

    let serial = state
        .pointer_button_serial
        .ok_or_else(|| io::Error::other("pointer button serial was not delivered"))?;
    toplevel._move(&seat, serial);
    connection.flush()?;
    wait_for_server_commands(commands);
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0 + render::SURFACE_CASCADE_STEP) + 92.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1 + render::SURFACE_CASCADE_STEP) + 74.0,
    })?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: false,
    })?;
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok(state)
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_request_top_left_resize_and_drag(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let _pointer = seat.get_pointer(&qh, ());
    let (surface, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 2.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 2.0,
    })?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    queue.roundtrip(&mut state)?;

    let serial = state
        .pointer_button_serial
        .ok_or_else(|| io::Error::other("pointer button serial was not delivered"))?;
    toplevel.resize(&seat, serial, client_xdg_toplevel::ResizeEdge::TopLeft);
    connection.flush()?;
    wait_for_server_commands(commands);
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) - 38.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) - 28.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let resized_width = usize::try_from(state.toplevel_width)?;
    let resized_height = usize::try_from(state.toplevel_height)?;
    commit_test_buffered_surface(&surface, &shm, &qh, resized_width, resized_height)?;
    connection.flush()?;
    wait_for_server_commands(commands);
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: false,
    })?;
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok(state)
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_then_frame_corner_resize_drag(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok(state)
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_then_frame_corner_resize_click_with_tiny_motion(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 305.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 205.0,
    })?;
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_then_left_edge_shrink_before_client_commit(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    retain_live_test_connection(connection.clone());
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) - 3.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 100.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 37.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 100.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}
