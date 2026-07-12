#![allow(unused_imports)]
use super::super::*;
use super::{
    client_setup::*, clipboard_dmabuf::*, frame_buffer_client::*, locked_relative::*,
    output_bindings::*, registry_state::*, server_runtime::*, subsurface_client::*, window_ops::*,
};
pub(in crate::compositor::tests) fn create_focused_toplevel_and_receive_key(
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
    let _keyboard = seat.get_keyboard(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 30,
        pressed: true,
    })?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_focused_toplevel_without_keypress(
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
    let _keyboard = seat.get_keyboard(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_focused_toplevel_then_press_tab(
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
    let _keyboard = seat.get_keyboard(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 15,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_focused_toplevel_and_receive_two_keys(
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
    let _keyboard = seat.get_keyboard(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 30,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 30,
        pressed: false,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_focused_toplevel_and_receive_ctrl_modified_key(
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
    let _keyboard = seat.get_keyboard(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 29,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 30,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_focused_toplevel_and_receive_pointer_motion(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_focused_toplevel_and_receive_pointer_motion_at_seat_version(socket_path, commands, 7)
}

pub(in crate::compositor::tests) fn create_focused_toplevel_and_receive_pointer_motion_at_seat_version(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    seat_version: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, seat_version..=seat_version, ())?;
    let _pointer = seat.get_pointer(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_focused_toplevel_and_receive_relative_pointer_motion(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;

    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_locked_focused_toplevel_and_receive_pointer_motion_sample(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::ActivatePointerConstraint(
        PointerConstraintMode::Locked,
    ))?;
    wait_for_server_commands(commands);
    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;

    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn capture_pointer_constraint_backend_requests(
    commands: &Sender<ServerCommand>,
) -> Vec<PointerConstraintBackendRequest> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePointerConstraintBackendRequests(
            reply,
        ))
        .unwrap();
    receiver.recv().unwrap()
}

pub(in crate::compositor::tests) fn request_lock_activate_and_receive_pointer_motion_sample(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<(RegistryTestState, Vec<PointerConstraintBackendRequest>), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let _lock = constraints.lock_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let requests = capture_pointer_constraint_backend_requests(commands);
    assert_eq!(state.locked_count, 0);
    let backend_id = requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id, .. } => Some(*id),
            _ => None,
        })
        .ok_or("expected locked backend activation request")?;
    commands.send(ServerCommand::PointerConstraintBackendActivated(backend_id))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    assert_eq!(state.locked_count, 1);

    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;
    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok((state, requests))
}

pub(in crate::compositor::tests) fn create_late_pointer_after_focus(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ())?;
    let _pointer_a = seat.get_pointer(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    assert_eq!(state.pointer_enter_count, 1);

    let _pointer_b = seat.get_pointer(&qh, ());
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok(state)
}

pub(in crate::compositor::tests) fn late_pointer_lock_activate_and_receive_relative_motion(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<(RegistryTestState, Vec<PointerConstraintBackendRequest>), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ())?;
    let _pointer_a = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    assert_eq!(state.pointer_enter_count, 1);

    let pointer_b = seat.get_pointer(&qh, ());
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer_b, &qh, ());
    let _lock = constraints.lock_pointer(
        &surface,
        &pointer_b,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let requests = capture_pointer_constraint_backend_requests(commands);
    assert_eq!(state.locked_count, 0);
    let backend_id = requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id, .. } => Some(*id),
            _ => None,
        })
        .ok_or("expected locked backend activation request")?;
    commands.send(ServerCommand::PointerConstraintBackendActivated(backend_id))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;
    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok((state, requests))
}

pub(in crate::compositor::tests) fn lock_activation_repairs_missing_source_pointer_enter_state(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ())?;
    let _pointer_a = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let pointer_b = seat.get_pointer(&qh, ());
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer_b, &qh, ());
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    assert_eq!(state.pointer_enter_count, 2);

    commands.send(ServerCommand::ClearPointerEnterTracking)?;
    wait_for_server_commands(commands);

    let _lock = constraints.lock_pointer(
        &surface,
        &pointer_b,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let requests = capture_pointer_constraint_backend_requests(commands);
    let backend_id = requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id, .. } => Some(*id),
            _ => None,
        })
        .ok_or("expected locked backend activation request")?;
    commands.send(ServerCommand::PointerConstraintBackendActivated(backend_id))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok(state)
}

pub(in crate::compositor::tests) fn relative_motion_for_focused_client_is_not_broadcast_to_other_client(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let focused_stream = UnixStream::connect(socket_path)?;
    let focused_connection = Connection::from_socket(focused_stream)?;
    let (focused_globals, mut focused_queue) =
        registry_queue_init::<RegistryTestState>(&focused_connection)?;
    let focused_qh = focused_queue.handle();
    let focused_compositor: client_wl_compositor::WlCompositor =
        focused_globals.bind(&focused_qh, 1..=6, ())?;
    let focused_wm_base: client_xdg_wm_base::XdgWmBase =
        focused_globals.bind(&focused_qh, 1..=6, ())?;
    let focused_seat: client_wl_seat::WlSeat = focused_globals.bind(&focused_qh, 5..=5, ())?;
    let _focused_pointer = focused_seat.get_pointer(&focused_qh, ());
    let focused_surface = focused_compositor.create_surface(&focused_qh, ());
    let focused_xdg_surface = focused_wm_base.get_xdg_surface(&focused_surface, &focused_qh, ());
    let _focused_toplevel = focused_xdg_surface.get_toplevel(&focused_qh, ());
    focused_surface.commit();
    focused_connection.flush()?;

    let mut focused_state = RegistryTestState::default();
    focused_queue.roundtrip(&mut focused_state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    focused_queue.roundtrip(&mut focused_state)?;
    assert_eq!(focused_state.pointer_enter_count, 1);

    let other_stream = UnixStream::connect(socket_path)?;
    let other_connection = Connection::from_socket(other_stream)?;
    let (other_globals, mut other_queue) =
        registry_queue_init::<RegistryTestState>(&other_connection)?;
    let other_qh = other_queue.handle();
    let other_seat: client_wl_seat::WlSeat = other_globals.bind(&other_qh, 5..=5, ())?;
    let other_pointer = other_seat.get_pointer(&other_qh, ());
    let other_relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        other_globals.bind(&other_qh, 1..=1, ())?;
    let _other_relative_pointer =
        other_relative_manager.get_relative_pointer(&other_pointer, &other_qh, ());
    other_connection.flush()?;
    wait_for_server_commands(commands);

    commands.send(ServerCommand::PointerMotionSample(PointerMotionSample {
        timestamp_usec: 44,
        absolute: None,
        relative: Some(RelativePointerMotion {
            dx: 1.0,
            dy: 1.0,
            dx_unaccelerated: 1.0,
            dy_unaccelerated: 1.0,
        }),
    }))?;
    wait_for_server_commands(commands);
    let mut other_state = RegistryTestState::default();
    other_queue.roundtrip(&mut other_state)?;

    Ok(other_state)
}

pub(in crate::compositor::tests) fn create_focused_toplevel_and_receive_pointer_axis(
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
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerAxis {
        horizontal: 0.0,
        vertical: 15.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_toplevel_then_click_and_move_pointer_on_same_surface(
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
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let _pointer = seat.get_pointer(&qh, ());
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 24.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 18.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_toplevel_then_set_and_commit_cursor_surface(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    remove_content: bool,
    request_frame_callback: bool,
    motion_after_commit: Option<(f64, f64)>,
) -> Result<CursorSurfaceCommitSnapshot, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let Some(serial) = state.pointer_enter_serial else {
        return Err("expected pointer enter serial before set_cursor".into());
    };
    let cursor_surface = compositor.create_surface(&qh, ());
    pointer.set_cursor(serial, Some(&cursor_surface), 1, 1);
    if request_frame_callback {
        cursor_surface.frame(&qh, ());
    }
    commit_test_buffered_surface(&cursor_surface, &shm, &qh, 24, 24)?;
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let callback_state = if request_frame_callback {
        let before = capture_pending_frame_callbacks(commands);
        commands.send(ServerCommand::FinishFrame)?;
        wait_for_server_commands(commands);
        let after = capture_pending_frame_callbacks(commands);
        Some((before, after))
    } else {
        None
    };

    if let Some((x, y)) = motion_after_commit {
        commands.send(ServerCommand::PointerMotion { x, y })?;
        wait_for_server_commands(commands);
        queue.roundtrip(&mut state)?;
    }

    if remove_content {
        cursor_surface.attach(None, 0, 0);
        cursor_surface.commit();
        connection.flush()?;
        wait_for_server_commands(commands);
        queue.roundtrip(&mut state)?;
    }

    Ok(CursorSurfaceCommitSnapshot {
        renderable_count: capture_renderable_surface_count(commands),
        cursor: capture_client_cursor_snapshot(commands),
        callback_state,
        cause: capture_render_generation_cause(commands),
    })
}

pub(in crate::compositor::tests) fn exercise_client_cursor_state_transitions(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<CursorTransitionSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let serial = state
        .pointer_enter_serial
        .ok_or("missing pointer enter serial")?;

    let cursor_surface = compositor.create_surface(&qh, ());
    pointer.set_cursor(serial, Some(&cursor_surface), 1, 1);
    commit_test_buffered_surface(&cursor_surface, &shm, &qh, 24, 24)?;
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let initial = capture_client_cursor_snapshot(commands);

    pointer.set_cursor(serial, Some(&cursor_surface), 5, 7);
    connection.flush()?;
    wait_for_server_commands(commands);
    let hotspot_changed = capture_client_cursor_snapshot(commands);

    pointer.set_cursor(serial, None, 0, 0);
    connection.flush()?;
    wait_for_server_commands(commands);
    let hidden = capture_client_cursor_snapshot(commands);

    pointer.set_cursor(serial, Some(&cursor_surface), 2, 3);
    connection.flush()?;
    wait_for_server_commands(commands);
    let reselected = capture_client_cursor_snapshot(commands);

    cursor_surface.destroy();
    connection.flush()?;
    wait_for_server_commands(commands);
    let destroyed = capture_client_cursor_snapshot(commands);

    Ok(CursorTransitionSnapshots {
        initial,
        hotspot_changed,
        hidden,
        reselected,
        destroyed,
    })
}

pub(in crate::compositor::tests) fn create_client_cursor_then_update_position_without_dispatch(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    x: f64,
    y: f64,
) -> Result<CompositorOnlyCursorMotionSnapshot, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let serial = state
        .pointer_enter_serial
        .ok_or("missing pointer enter serial")?;

    let cursor_surface = compositor.create_surface(&qh, ());
    pointer.set_cursor(serial, Some(&cursor_surface), 3, 4);
    commit_test_buffered_surface(&cursor_surface, &shm, &qh, 24, 24)?;
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let render_generation_before = capture_render_generation(commands);
    let scene_generation_before = capture_scene_render_generation(commands);
    let pointer_event_log_before = state.pointer_event_log.clone();
    let relative_motion_count_before = state.relative_motion_count;
    let pointer_focus_surface_before = capture_pointer_focus_surface_id(commands);
    let (reply, receiver) = mpsc::channel();
    commands.send(ServerCommand::UpdatePointerPositionWithoutClientDispatch { x, y, reply })?;
    let visual_changed = receiver.recv_timeout(Duration::from_secs(1))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok(CompositorOnlyCursorMotionSnapshot {
        cursor: capture_client_cursor_snapshot(commands),
        visual_changed,
        render_generation_before,
        render_generation_after: capture_render_generation(commands),
        scene_generation_before,
        scene_generation_after: capture_scene_render_generation(commands),
        cause: capture_render_generation_cause(commands),
        pointer_event_log_before,
        pointer_event_log_after: state.pointer_event_log,
        relative_motion_count_before,
        relative_motion_count_after: state.relative_motion_count,
        pointer_focus_surface_before,
        pointer_focus_surface_after: capture_pointer_focus_surface_id(commands),
    })
}

pub(in crate::compositor::tests) fn create_client_cursor_then_synchronize_compositor_only_motion_and_send_normal_sample(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    x: f64,
    y: f64,
) -> Result<CompositorOnlyCursorSynchronizationSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let serial = state
        .pointer_enter_serial
        .ok_or("missing pointer enter serial")?;

    let cursor_surface = compositor.create_surface(&qh, ());
    pointer.set_cursor(serial, Some(&cursor_surface), 3, 4);
    commit_test_buffered_surface(&cursor_surface, &shm, &qh, 24, 24)?;
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let button_serial = state
        .pointer_button_serial
        .ok_or_else(|| io::Error::other("pointer button serial was not delivered"))?;

    _toplevel.resize(
        &seat,
        button_serial,
        client_xdg_toplevel::ResizeEdge::BottomRight,
    );
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let initial = capture_cursor_motion_state(&state, commands, false);
    let (reply, receiver) = mpsc::channel();
    commands.send(ServerCommand::UpdatePointerPositionWithoutClientDispatch { x, y, reply })?;
    let visual_changed = receiver.recv_timeout(Duration::from_secs(1))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let compositor_only = capture_cursor_motion_state(&state, commands, visual_changed);

    let (reply, receiver) = mpsc::channel();
    commands.send(ServerCommand::UpdateInteractionResult { x, y, reply })?;
    let interaction_update_applied = receiver.recv_timeout(Duration::from_secs(1))?;
    wait_for_server_commands(commands);
    commands.send(ServerCommand::PrepareFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let interaction = capture_cursor_motion_state(&state, commands, false);
    let resize_visual_active =
        capture_toplevel_visual_geometry(commands).is_some_and(|visual| visual.active_resize);

    commands.send(ServerCommand::PointerMotion { x, y })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let normal_motion = capture_cursor_motion_state(&state, commands, false);

    Ok(CompositorOnlyCursorSynchronizationSnapshots {
        initial,
        compositor_only,
        interaction,
        normal_motion,
        interaction_update_applied,
        resize_visual_active,
    })
}

pub(in crate::compositor::tests) fn create_client_cursor_then_window_interaction(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    hidden: bool,
    resize: bool,
) -> Result<WindowInteractionCursorSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    let start_x = f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0;
    let start_y = f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0;
    commands.send(ServerCommand::PointerMotion {
        x: start_x,
        y: start_y,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let serial = state
        .pointer_enter_serial
        .ok_or("missing pointer enter serial")?;

    let _cursor_surface = if hidden {
        pointer.set_cursor(serial, None, 0, 0);
        None
    } else {
        let cursor_surface = compositor.create_surface(&qh, ());
        pointer.set_cursor(serial, Some(&cursor_surface), 3, 4);
        commit_test_buffered_surface(&cursor_surface, &shm, &qh, 24, 24)?;
        Some(cursor_surface)
    };
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let before_cursor = capture_client_cursor_snapshot(commands);
    let before_state = capture_interaction_cursor_state(commands);
    let pointer_motion_count_before = state
        .pointer_event_log
        .iter()
        .filter(|event| **event == "motion")
        .count();
    let relative_motion_count_before = state.relative_motion_count;

    let begin_x = if resize {
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 158.0
    } else {
        start_x
    };
    let begin_y = if resize {
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 118.0
    } else {
        start_y
    };
    commands.send(if resize {
        ServerCommand::BeginResize {
            x: begin_x,
            y: begin_y,
        }
    } else {
        ServerCommand::BeginMove {
            x: begin_x,
            y: begin_y,
        }
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let update_x = f64::from(render::FIRST_SURFACE_OFFSET.0) + if resize { 300.0 } else { 80.0 };
    let update_y = f64::from(render::FIRST_SURFACE_OFFSET.1) + if resize { 250.0 } else { 60.0 };
    let (reply, receiver) = std::sync::mpsc::channel();
    commands.send(ServerCommand::UpdatePointerPositionWithoutClientDispatch {
        x: update_x,
        y: update_y,
        reply,
    })?;
    receiver.recv_timeout(Duration::from_secs(1))?;
    wait_for_server_commands(commands);
    commands.send(ServerCommand::UpdateInteraction {
        x: update_x,
        y: update_y,
    })?;
    commands.send(ServerCommand::PrepareFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let during_cursor = capture_client_cursor_snapshot(commands);
    let during_state = capture_interaction_cursor_state(commands);
    let pointer_motion_count_during = state
        .pointer_event_log
        .iter()
        .filter(|event| **event == "motion")
        .count();
    let relative_motion_count_during = state.relative_motion_count;
    let resize_geometry_during = capture_toplevel_visual_geometry(commands);
    let moved_root_origin_during = capture_renderable_surface_snapshot(commands)
        .into_iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .map(|surface| (surface.origin_x, surface.origin_y));

    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let after_cursor = capture_client_cursor_snapshot(commands);
    let after_state = capture_interaction_cursor_state(commands);
    let pointer_motion_count_after = state
        .pointer_event_log
        .iter()
        .filter(|event| **event == "motion")
        .count();
    let relative_motion_count_after = state.relative_motion_count;

    Ok(WindowInteractionCursorSnapshots {
        before_cursor,
        during_cursor,
        after_cursor,
        before_state,
        during_state,
        after_state,
        pointer_motion_count_before,
        pointer_motion_count_during,
        pointer_motion_count_after,
        relative_motion_count_before,
        relative_motion_count_during,
        relative_motion_count_after,
        resize_geometry_during,
        moved_root_origin_during,
    })
}

fn capture_cursor_motion_state(
    state: &RegistryTestState,
    commands: &Sender<ServerCommand>,
    visual_changed: bool,
) -> CursorMotionStateSnapshot {
    CursorMotionStateSnapshot {
        cursor: capture_client_cursor_snapshot(commands),
        visual_changed,
        render_generation: capture_render_generation(commands),
        scene_generation: capture_scene_render_generation(commands),
        cause: capture_render_generation_cause(commands),
        pointer_event_log: state.pointer_event_log.clone(),
        pointer_motion_count: state
            .pointer_event_log
            .iter()
            .filter(|event| **event == "motion")
            .count(),
        relative_motion_count: state.relative_motion_count,
        pointer_focus_surface: capture_pointer_focus_surface_id(commands),
    }
}

pub(in crate::compositor::tests) fn create_buffered_toplevel_and_receive_surface_local_pointer_motion(
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
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let _pointer = seat.get_pointer(&qh, ());

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
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_toplevel_with_empty_input_subsurface_and_click_overlap(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_toplevel_with_custom_input_subsurface_and_click_overlap(socket_path, commands, None)
}

pub(in crate::compositor::tests) fn create_toplevel_with_custom_input_subsurface_and_click_overlap(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    child_region: Option<(i32, i32, i32, i32)>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let _pointer = seat.get_pointer(&qh, ());

    let (parent, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    let parent_surface_id = parent.id().protocol_id();
    toplevel.set_app_id("oblivion.input-region-parent".to_string());
    parent.commit();

    let child = compositor.create_surface(&qh, ());
    let child_surface_id = child.id().protocol_id();
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_position(0, 0);
    let region = compositor.create_region(&qh, ());
    if let Some((x, y, width, height)) = child_region {
        region.add(x, y, width, height);
    }
    child.set_input_region(Some(&region));
    commit_test_buffered_surface(&child, &shm, &qh, 160, 120)?;
    parent.commit();
    connection.flush()?;

    let mut state = RegistryTestState {
        parent_surface_id: Some(parent_surface_id),
        child_surface_id: Some(child_surface_id),
        ..RegistryTestState::default()
    };
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
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
