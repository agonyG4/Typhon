#![allow(unused_imports)]
use super::super::*;
use super::{
    client_setup::*, clipboard_dmabuf::*, frame_buffer_client::*, input_client::*,
    locked_relative::*, registry_state::*, server_runtime::*, subsurface_client::*, window_ops::*,
};
pub(in crate::compositor::tests) fn bind_output_and_seat(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let _output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ())?;
    let _seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn bind_output_at_version(
    socket_path: &PathBuf,
    version: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let _output: client_wl_output::WlOutput = globals.bind(&qh, version..=version, ())?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn bind_seat_at_version(
    socket_path: &PathBuf,
    version: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let _seat: client_wl_seat::WlSeat = globals.bind(&qh, version..=version, ())?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn bind_output_then_set_output_size(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    width: u32,
    height: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let _output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ())?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::SetOutputSize { width, height })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn bind_output_then_set_output_refresh(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    refresh_hz: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let _output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ())?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::SetOutputRefresh { refresh_hz })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn bind_output_then_set_output_refresh_and_size(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    refresh_hz: u32,
    width: u32,
    height: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let _output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ())?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::SetOutputRefresh { refresh_hz })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::SetOutputSize { width, height })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_fractional_scale_surface_then_set_output_scale(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    scale_factor: f64,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let fractional_scale_manager: client_wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    let _fractional_scale = fractional_scale_manager.get_fractional_scale(&surface, &qh, ());
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::SetOutputScale { scale_factor })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_mapped_surface_then_set_output_preferences(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    compositor_version: u32,
    scale_factor: f64,
    transform: Option<u32>,
    restore_defaults: bool,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor =
        globals.bind(&qh, compositor_version..=compositor_version, ())?;
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
    commands.send(ServerCommand::SetOutputScale { scale_factor })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    if let Some(transform) = transform {
        commands.send(ServerCommand::SetOutputPreferredTransform(transform))?;
        wait_for_server_commands(commands);
        queue.roundtrip(&mut state)?;
        if restore_defaults {
            commands.send(ServerCommand::SetOutputScale { scale_factor })?;
            wait_for_server_commands(commands);
            queue.roundtrip(&mut state)?;
            commands.send(ServerCommand::SetOutputPreferredTransform(0))?;
            wait_for_server_commands(commands);
            queue.roundtrip(&mut state)?;
            commands.send(ServerCommand::SetOutputScale { scale_factor: 1.0 })?;
            wait_for_server_commands(commands);
            queue.roundtrip(&mut state)?;
        }
    }
    Ok(state)
}

pub(in crate::compositor::tests) fn create_mapped_surface_then_move_outside_output(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
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
    commands.send(ServerCommand::SetFocusedRootVisualGeometry {
        placement: SurfacePlacement::absolute_root_at(-500, -500),
        width: 40,
        height: 30,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_mapped_surface_then_unmap_and_remap(
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
    surface.attach(None, 0, 0);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    commit_test_buffered_surface_after_configure(
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

pub(in crate::compositor::tests) fn create_duplicate_fractional_scale_surface(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let fractional_scale_manager: client_wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    let _first_fractional_scale = fractional_scale_manager.get_fractional_scale(&surface, &qh, ());
    let _second_fractional_scale = fractional_scale_manager.get_fractional_scale(&surface, &qh, ());
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_client_surface_with_buffer_scale(
    socket_path: &PathBuf,
    buffer_width: u32,
    buffer_height: u32,
    buffer_scale: i32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    assign_test_toplevel(&globals, &qh, &surface)?;
    surface.set_buffer_scale(buffer_scale);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
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

pub(in crate::compositor::tests) fn request_keyboard_from_seat(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    request_keyboard_from_seat_at_version(socket_path, 7)
}

pub(in crate::compositor::tests) fn request_keyboard_from_seat_at_version(
    socket_path: &PathBuf,
    version: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let seat: client_wl_seat::WlSeat = globals.bind(&qh, version..=version, ())?;
    let _keyboard = seat.get_keyboard(&qh, ());
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_client_subsurface(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let parent = compositor.create_surface(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());

    subsurface.set_position(10, 12);
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

pub(in crate::compositor::tests) fn create_client_data_device(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ())?;
    let _source = manager.create_data_source(&qh, ());
    let _device = manager.get_data_device(&seat, &qh, ());
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}
