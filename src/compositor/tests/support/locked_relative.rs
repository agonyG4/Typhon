#![allow(unused_imports)]
use super::super::*;
use super::{
    client_setup::*, clipboard_dmabuf::*, frame_buffer_client::*, input_client::*,
    output_bindings::*, registry_state::*, server_runtime::*, subsurface_client::*, window_ops::*,
};
pub(in crate::compositor::tests) fn runtime_socket_path(socket_name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join(socket_name)
}

pub(in crate::compositor::tests) fn activate_backend_locked_pointer(
    commands: &Sender<ServerCommand>,
    state: &mut RegistryTestState,
    queue: &mut EventQueue<RegistryTestState>,
) -> Result<PointerConstraintBackendId, Box<dyn std::error::Error>> {
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
    queue.roundtrip(state)?;
    assert_eq!(state.locked_count, 1);
    Ok(backend_id)
}

pub(in crate::compositor::tests) fn locked_relative_motion_survives_stale_hit_test(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
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
    let pointer = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    let parent_surface_id = parent.id().protocol_id();
    parent.commit();

    let mut state = RegistryTestState {
        parent_surface_id: Some(parent_surface_id),
        ..RegistryTestState::default()
    };
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let _lock = constraints.lock_pointer(
        &parent,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    activate_backend_locked_pointer(commands, &mut state, &mut queue)?;

    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_position(0, 0);
    let region = compositor.create_region(&qh, ());
    region.add(0, 0, 160, 120);
    child.set_input_region(Some(&region));
    commit_test_buffered_surface(&child, &shm, &qh, 160, 120)?;
    parent.commit();
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let enter_before_motion = state.pointer_enter_count;
    let leave_before_motion = state.pointer_leave_count;
    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;
    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    assert_eq!(state.pointer_enter_count, enter_before_motion);
    assert_eq!(state.pointer_leave_count, leave_before_motion);
    Ok(state)
}

pub(in crate::compositor::tests) fn run_locked_relative_motion_targets_exact_source_pointer(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<(RegistryTestState, u32, u32), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ())?;
    let pointer_a = seat.get_pointer(&qh, ());
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
    let relative_a = relative_manager.get_relative_pointer(&pointer_a, &qh, ());
    let relative_b = relative_manager.get_relative_pointer(&pointer_b, &qh, ());
    let relative_a_id = relative_a.id().protocol_id();
    let relative_b_id = relative_b.id().protocol_id();
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
    activate_backend_locked_pointer(commands, &mut state, &mut queue)?;

    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok((state, relative_a_id, relative_b_id))
}

pub(in crate::compositor::tests) fn run_locked_relative_motion_falls_back_to_same_client_pointer_resource(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<(RegistryTestState, u32), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ())?;
    let pointer_a = seat.get_pointer(&qh, ());
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
    let relative_a = relative_manager.get_relative_pointer(&pointer_a, &qh, ());
    let relative_a_id = relative_a.id().protocol_id();
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
    activate_backend_locked_pointer(commands, &mut state, &mut queue)?;

    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;
    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok((state, relative_a_id))
}

pub(in crate::compositor::tests) fn run_locked_relative_motion_fallback_does_not_cross_clients(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(RegistryTestState, RegistryTestState), Box<dyn std::error::Error>> {
    let locked_stream = UnixStream::connect(socket_path)?;
    let locked_connection = Connection::from_socket(locked_stream)?;
    let (locked_globals, mut locked_queue) =
        registry_queue_init::<RegistryTestState>(&locked_connection)?;
    let locked_qh = locked_queue.handle();
    let locked_compositor: client_wl_compositor::WlCompositor =
        locked_globals.bind(&locked_qh, 1..=6, ())?;
    let locked_wm_base: client_xdg_wm_base::XdgWmBase =
        locked_globals.bind(&locked_qh, 1..=6, ())?;
    let locked_seat: client_wl_seat::WlSeat = locked_globals.bind(&locked_qh, 5..=5, ())?;
    let locked_pointer = locked_seat.get_pointer(&locked_qh, ());
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        locked_globals.bind(&locked_qh, 1..=1, ())?;

    let locked_surface = locked_compositor.create_surface(&locked_qh, ());
    let locked_xdg_surface = locked_wm_base.get_xdg_surface(&locked_surface, &locked_qh, ());
    let _locked_toplevel = locked_xdg_surface.get_toplevel(&locked_qh, ());
    locked_surface.commit();
    locked_connection.flush()?;

    let mut locked_state = RegistryTestState::default();
    locked_queue.roundtrip(&mut locked_state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    locked_queue.roundtrip(&mut locked_state)?;

    let _lock = constraints.lock_pointer(
        &locked_surface,
        &locked_pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &locked_qh,
        (),
    );
    locked_connection.flush()?;
    wait_for_server_commands(commands);
    locked_queue.roundtrip(&mut locked_state)?;
    activate_backend_locked_pointer(commands, &mut locked_state, &mut locked_queue)?;

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
        timestamp_usec: 505,
        absolute: None,
        relative: Some(RelativePointerMotion {
            dx: 3.0,
            dy: -2.0,
            dx_unaccelerated: 3.0,
            dy_unaccelerated: -2.0,
        }),
    }))?;
    wait_for_server_commands(commands);
    let mut other_state = RegistryTestState::default();
    other_queue.roundtrip(&mut other_state)?;

    Ok((locked_state, other_state))
}

pub(in crate::compositor::tests) fn run_locked_relative_motion_dispatches_to_all_same_client_resources(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<(RegistryTestState, Vec<u32>), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ())?;
    let pointer_a = seat.get_pointer(&qh, ());
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
    let relative_a = relative_manager.get_relative_pointer(&pointer_a, &qh, ());
    let relative_b = relative_manager.get_relative_pointer(&pointer_b, &qh, ());
    let expected_ids = vec![relative_a.id().protocol_id(), relative_b.id().protocol_id()];
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
    activate_backend_locked_pointer(commands, &mut state, &mut queue)?;

    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok((state, expected_ids))
}

pub(in crate::compositor::tests) fn clear_locked_relative_motion_observations(
    state: &mut RegistryTestState,
) {
    state.pointer_frame_count = 0;
    state.pointer_frame_resource_ids.clear();
    state.pointer_event_log.clear();
    state.relative_motion_count = 0;
    state.relative_motion_resource_ids.clear();
    state.relative_motion_utime = None;
    state.relative_motion_dx = None;
    state.relative_motion_dy = None;
    state.relative_motion_dx_unaccel = None;
    state.relative_motion_dy_unaccel = None;
    state.sdl_pending_relative_motion_count = 0;
    state.sdl_camera_motion_count = 0;
    state.pointer_button = false;
}

pub(in crate::compositor::tests) struct LockedRelativeFrameResult {
    pub(in crate::compositor::tests) state: RegistryTestState,
    pub(in crate::compositor::tests) relative_ids: Vec<u32>,
    pub(in crate::compositor::tests) pointer_ids: Vec<u32>,
}

pub(in crate::compositor::tests) fn run_locked_relative_motion_shared_source_pointer_frames(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<LockedRelativeFrameResult, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let pointer_id = pointer.id().protocol_id();
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

    let relative_a = relative_manager.get_relative_pointer(&pointer, &qh, ());
    let relative_b = relative_manager.get_relative_pointer(&pointer, &qh, ());
    let relative_ids = vec![relative_a.id().protocol_id(), relative_b.id().protocol_id()];
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
    activate_backend_locked_pointer(commands, &mut state, &mut queue)?;

    clear_locked_relative_motion_observations(&mut state);
    commands.send(ServerCommand::PointerMotionSample(PointerMotionSample {
        timestamp_usec: 808,
        absolute: None,
        relative: Some(RelativePointerMotion {
            dx: 4.0,
            dy: -1.0,
            dx_unaccelerated: 4.0,
            dy_unaccelerated: -1.0,
        }),
    }))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok(LockedRelativeFrameResult {
        state,
        relative_ids,
        pointer_ids: vec![pointer_id],
    })
}

pub(in crate::compositor::tests) fn run_locked_relative_motion_different_source_pointer_frames(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<LockedRelativeFrameResult, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ())?;
    let pointer_a = seat.get_pointer(&qh, ());
    let pointer_b = seat.get_pointer(&qh, ());
    let pointer_ids = vec![pointer_a.id().protocol_id(), pointer_b.id().protocol_id()];
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

    let relative_a = relative_manager.get_relative_pointer(&pointer_a, &qh, ());
    let relative_b = relative_manager.get_relative_pointer(&pointer_b, &qh, ());
    let relative_ids = vec![relative_a.id().protocol_id(), relative_b.id().protocol_id()];
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
    activate_backend_locked_pointer(commands, &mut state, &mut queue)?;

    clear_locked_relative_motion_observations(&mut state);
    commands.send(ServerCommand::PointerMotionSample(PointerMotionSample {
        timestamp_usec: 809,
        absolute: None,
        relative: Some(RelativePointerMotion {
            dx: -2.0,
            dy: 5.0,
            dx_unaccelerated: -2.0,
            dy_unaccelerated: 5.0,
        }),
    }))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok(LockedRelativeFrameResult {
        state,
        relative_ids,
        pointer_ids,
    })
}

pub(in crate::compositor::tests) fn capture_pointer_constraint_ids(
    commands: &Sender<ServerCommand>,
) -> Vec<u64> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePointerConstraintIds(reply))
        .unwrap();
    receiver.recv().unwrap()
}

pub(in crate::compositor::tests) fn run_multi_client_pointer_constraints_remain_independent(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(PointerConstraintBackendId, PointerConstraintBackendId), Box<dyn std::error::Error>> {
    #[allow(clippy::type_complexity)]
    fn setup_client(
        socket_path: &PathBuf,
    ) -> Result<
        (
            Connection,
            EventQueue<RegistryTestState>,
            client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
            client_wl_surface::WlSurface,
            client_wl_pointer::WlPointer,
        ),
        Box<dyn std::error::Error>,
    > {
        let stream = UnixStream::connect(socket_path)?;
        let connection = Connection::from_socket(stream)?;
        let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
        let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
        let pointer = seat.get_pointer(&qh, ());
        let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
            globals.bind(&qh, 1..=1, ())?;
        let (surface, _xdg_surface, _toplevel) =
            create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 120, 90)?;
        surface.commit();
        connection.flush()?;
        Ok((connection, queue, constraints, surface, pointer))
    }

    let (connection_a, mut queue_a, constraints_a, surface_a, pointer_a) =
        setup_client(socket_path)?;
    let mut state_a = RegistryTestState::default();
    queue_a.roundtrip(&mut state_a)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    wait_for_server_commands(commands);
    queue_a.roundtrip(&mut state_a)?;

    let _lock_a = constraints_a.lock_pointer(
        &surface_a,
        &pointer_a,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &queue_a.handle(),
        (),
    );
    connection_a.flush()?;
    wait_for_server_commands(commands);
    queue_a.roundtrip(&mut state_a)?;
    let requests_a = capture_pointer_constraint_backend_requests(commands);
    let id_a = requests_a
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id, .. } => Some(*id),
            _ => None,
        })
        .ok_or("expected client A locked backend activation request")?;

    let (connection_b, mut queue_b, constraints_b, surface_b, pointer_b) =
        setup_client(socket_path)?;
    let mut state_b = RegistryTestState::default();
    queue_b.roundtrip(&mut state_b)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    wait_for_server_commands(commands);
    queue_b.roundtrip(&mut state_b)?;

    let _lock_b = constraints_b.lock_pointer(
        &surface_b,
        &pointer_b,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &queue_b.handle(),
        (),
    );
    connection_b.flush()?;
    wait_for_server_commands(commands);
    queue_b.roundtrip(&mut state_b)?;
    let ids = capture_pointer_constraint_ids(commands);
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    assert!(ids.contains(&id_a.constraint_id));

    commands.send(ServerCommand::PointerConstraintBackendActivated(id_a))?;
    wait_for_server_commands(commands);
    queue_a.roundtrip(&mut state_a)?;
    assert_eq!(state_a.locked_count, 1);
    assert_eq!(state_b.locked_count, 0);

    let wrong_client_activation = PointerConstraintBackendId {
        constraint_id: id_a.constraint_id,
        generation: id_a.generation.wrapping_add(999),
    };
    commands.send(ServerCommand::PointerConstraintBackendActivated(
        wrong_client_activation,
    ))?;
    wait_for_server_commands(commands);
    queue_b.roundtrip(&mut state_b)?;
    assert_eq!(state_b.locked_count, 0);

    commands.send(ServerCommand::PointerMotionSample(PointerMotionSample {
        timestamp_usec: 1,
        absolute: None,
        relative: Some(RelativePointerMotion {
            dx: 3.0,
            dy: 1.0,
            dx_unaccelerated: 3.0,
            dy_unaccelerated: 1.0,
        }),
    }))?;
    wait_for_server_commands(commands);
    queue_b.roundtrip(&mut state_b)?;
    assert_eq!(state_b.relative_motion_count, 0);

    Ok((
        id_a,
        PointerConstraintBackendId {
            constraint_id: ids
                .iter()
                .copied()
                .find(|id| *id != id_a.constraint_id)
                .unwrap(),
            generation: id_a.generation,
        },
    ))
}

pub(in crate::compositor::tests) fn run_locked_relative_motion_survives_surface_tree_churn(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
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
    let pointer = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    parent.commit();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let _lock = constraints.lock_pointer(
        &parent,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    activate_backend_locked_pointer(commands, &mut state, &mut queue)?;

    let lower = compositor.create_surface(&qh, ());
    let lower_subsurface = subcompositor.get_subsurface(&lower, &parent, &qh, ());
    lower_subsurface.set_position(0, 0);
    commit_test_buffered_surface(&lower, &shm, &qh, 80, 80)?;

    let upper = compositor.create_surface(&qh, ());
    let upper_subsurface = subcompositor.get_subsurface(&upper, &parent, &qh, ());
    upper_subsurface.set_position(0, 0);
    let region = compositor.create_region(&qh, ());
    region.add(0, 0, 80, 80);
    upper.set_input_region(Some(&region));
    commit_test_buffered_surface(&upper, &shm, &qh, 80, 80)?;
    lower_subsurface.place_above(&upper);
    parent.commit();
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn unique_socket_name() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("oblivion-one-test-{}-{now}", std::process::id())
}
