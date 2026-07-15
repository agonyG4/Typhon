#![allow(unused_imports)]
use super::super::*;
use super::{
    client_setup::*, clipboard_dmabuf::*, frame_buffer_client::*, input_client::*,
    locked_relative::*, output_bindings::*, registry_state::*, server_runtime::*, window_ops::*,
};
pub(in crate::compositor::tests) fn create_min_size_toplevel_then_shrink_resize_before_client_commit(
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
    let (surface, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 320, 220)?;
    toplevel.set_min_size(280, 180);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 324.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 224.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 214.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 114.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_scaled_buffer_toplevel_then_right_edge_shrink_and_commit(
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
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.set_buffer_scale(2);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    attach_test_buffered_surface(&surface, &shm, &qh, 600, 400)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 100.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 264.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 100.0,
    })?;
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    surface.set_buffer_scale(2);
    commit_test_buffered_surface(&surface, &shm, &qh, 520, 400)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_scaled_buffer_toplevel_then_left_edge_shrink_and_commit(
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
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.set_buffer_scale(2);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    attach_test_buffered_surface(&surface, &shm, &qh, 600, 400)?;
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
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    surface.set_buffer_scale(2);
    commit_test_buffered_surface(&surface, &shm, &qh, 520, 400)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_toplevel_then_map_subsurface_before_button_release(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
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
    toplevel.set_app_id("oblivion.implicit-grab-parent".to_string());
    parent.commit();
    connection.flush()?;

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
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let child = compositor.create_surface(&qh, ());
    let child_surface_id = child.id().protocol_id();
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_position(0, 0);
    commit_test_buffered_surface(&child, &shm, &qh, 160, 120)?;
    connection.flush()?;
    state.child_surface_id = Some(child_surface_id);
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: false,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn capture_gecko_pre_role_subsurface_adoption(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<GeckoPreRoleAdoptionSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let child = compositor.create_surface(&qh, ());
    attach_test_buffered_surface(&child, &shm, &qh, 1, 1)?;
    child.commit();
    connection.flush()?;
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    let after_roleless_commit = capture_renderable_surface_snapshot(commands);

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    commit_test_buffered_surface(&parent, &shm, &qh, 1992, 1189)?;

    let viewport = viewporter.get_viewport(&child, &qh, ());
    viewport.set_destination(1920, 1080);
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_position(10, 10);
    subsurface.set_desync();
    commit_test_buffered_surface(&child, &shm, &qh, 1972, 1132)?;
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    let after_adoption = capture_renderable_surface_snapshot(commands);
    Ok(GeckoPreRoleAdoptionSnapshots {
        after_roleless_commit,
        after_adoption,
    })
}

pub(in crate::compositor::tests) fn create_overlapping_subsurfaces_then_place_above_after_parent_commit(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
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
    toplevel.set_app_id("oblivion.subsurface-place-above".to_string());
    parent.commit();

    let lower = compositor.create_surface(&qh, ());
    let lower_id = lower.id().protocol_id();
    let lower_subsurface = subcompositor.get_subsurface(&lower, &parent, &qh, ());
    lower_subsurface.set_position(0, 0);
    commit_test_buffered_surface(&lower, &shm, &qh, 80, 80)?;

    let upper = compositor.create_surface(&qh, ());
    let upper_id = upper.id().protocol_id();
    let upper_subsurface = subcompositor.get_subsurface(&upper, &parent, &qh, ());
    upper_subsurface.set_position(0, 0);
    commit_test_buffered_surface(&upper, &shm, &qh, 81, 81)?;
    parent.commit();
    connection.flush()?;

    let mut state = RegistryTestState {
        child_surface_id: Some(lower_id),
        second_child_surface_id: Some(upper_id),
        ..RegistryTestState::default()
    };
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    assert_eq!(state.pointer_button_surface_id, Some(upper_id));

    lower_subsurface.place_above(&upper);
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    state.pointer_button_surface_id = None;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: false,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    assert_eq!(state.pointer_button_surface_id, Some(upper_id));

    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    state.pointer_button_surface_id = None;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_subsurface_below_parent_and_click_overlap(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
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
    let parent_id = parent.id().protocol_id();
    toplevel.set_app_id("oblivion.subsurface-place-below-parent".to_string());
    parent.commit();

    let child = compositor.create_surface(&qh, ());
    let child_id = child.id().protocol_id();
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_position(0, 0);
    commit_test_buffered_surface(&child, &shm, &qh, 160, 120)?;
    subsurface.place_below(&parent);
    parent.commit();
    connection.flush()?;

    let mut state = RegistryTestState {
        parent_surface_id: Some(parent_id),
        child_surface_id: Some(child_id),
        ..RegistryTestState::default()
    };
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
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

pub(in crate::compositor::tests) fn create_subsurface_with_invalid_restack_reference(
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
    let unrelated = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.place_above(&unrelated);
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

pub(in crate::compositor::tests) fn create_repeated_restack_then_destroy_subsurface(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<
    (
        Vec<RenderableSurfaceSnapshot>,
        Vec<RenderableSurfaceSnapshot>,
    ),
    Box<dyn std::error::Error>,
> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    toplevel.set_app_id("oblivion.subsurface-repeated-reorder".to_string());
    parent.commit();

    let subtree = compositor.create_surface(&qh, ());
    let subtree_subsurface = subcompositor.get_subsurface(&subtree, &parent, &qh, ());
    subtree_subsurface.set_position(0, 0);
    commit_test_buffered_surface(&subtree, &shm, &qh, 80, 80)?;

    let grandchild = compositor.create_surface(&qh, ());
    let grandchild_subsurface = subcompositor.get_subsurface(&grandchild, &subtree, &qh, ());
    grandchild_subsurface.set_position(1, 1);
    commit_test_buffered_surface(&grandchild, &shm, &qh, 40, 40)?;

    let sibling = compositor.create_surface(&qh, ());
    let sibling_subsurface = subcompositor.get_subsurface(&sibling, &parent, &qh, ());
    sibling_subsurface.set_position(0, 0);
    commit_test_buffered_surface(&sibling, &shm, &qh, 81, 81)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    for _ in 0..3 {
        subtree_subsurface.place_above(&sibling);
    }
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    wait_for_server_commands(commands);
    let reordered = capture_renderable_surface_snapshot(commands);

    subtree_subsurface.destroy();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    wait_for_server_commands(commands);
    let after_destroy = capture_renderable_surface_snapshot(commands);

    Ok((reordered, after_destroy))
}

pub(in crate::compositor::tests) fn create_pointer_enter_with_v5_pointer(
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
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_decoy_keyboard_then_focused_toplevel_and_receive_key(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let decoy_stream = UnixStream::connect(socket_path)?;
    let decoy_connection = Connection::from_socket(decoy_stream)?;
    let (decoy_globals, mut decoy_queue) =
        registry_queue_init::<RegistryTestState>(&decoy_connection)?;
    let decoy_qh = decoy_queue.handle();
    let decoy_seat: client_wl_seat::WlSeat = decoy_globals.bind(&decoy_qh, 1..=7, ())?;
    let _decoy_keyboard = decoy_seat.get_keyboard(&decoy_qh, ());
    decoy_connection.flush()?;
    let mut decoy_state = RegistryTestState::default();
    decoy_queue.roundtrip(&mut decoy_state)?;

    let focused_stream = UnixStream::connect(socket_path)?;
    let focused_connection = Connection::from_socket(focused_stream)?;
    let (focused_globals, mut focused_queue) =
        registry_queue_init::<RegistryTestState>(&focused_connection)?;
    let focused_qh = focused_queue.handle();
    let compositor: client_wl_compositor::WlCompositor =
        focused_globals.bind(&focused_qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = focused_globals.bind(&focused_qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = focused_globals.bind(&focused_qh, 1..=7, ())?;
    let _keyboard = seat.get_keyboard(&focused_qh, ());
    let surface = compositor.create_surface(&focused_qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &focused_qh, ());
    let _toplevel = xdg_surface.get_toplevel(&focused_qh, ());
    surface.commit();
    focused_connection.flush()?;

    let mut focused_state = RegistryTestState::default();
    focused_queue.roundtrip(&mut focused_state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 30,
        pressed: true,
    })?;
    focused_queue.roundtrip(&mut focused_state)?;
    Ok(focused_state)
}
