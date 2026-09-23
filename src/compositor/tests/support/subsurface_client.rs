#![allow(unused_imports)]
use super::super::*;
use super::{
    client_setup::*, clipboard_dmabuf::*, frame_buffer_client::*, input_client::*,
    locked_relative::*, output_bindings::*, registry_state::*, server_runtime::*, window_ops::*,
};
use crate::compositor::popup::XdgWindowGeometry;
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
    let _buffer = attach_test_buffered_surface(&surface, &shm, &qh, 600, 400)?;
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
    let _buffer = attach_test_buffered_surface(&surface, &shm, &qh, 600, 400)?;
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
    let _buffer = attach_test_buffered_surface(&child, &shm, &qh, 1, 1)?;
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
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    let after_relationship = capture_renderable_surface_snapshot(commands);
    subsurface.set_position(10, 10);
    subsurface.set_desync();
    commit_test_buffered_surface(&child, &shm, &qh, 1972, 1132)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    let before_parent_commit = capture_renderable_surface_snapshot(commands);

    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    let after_adoption = capture_renderable_surface_snapshot(commands);
    Ok(GeckoPreRoleAdoptionSnapshots {
        after_roleless_commit,
        after_relationship,
        before_parent_commit,
        after_adoption,
    })
}

pub(in crate::compositor::tests) fn capture_gecko_window_geometry_evolution(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<Vec<GeckoGeometryPublicationSnapshot>, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ())?;
    let mut state = RegistryTestState::default();

    let child = compositor.create_surface(&qh, ());
    let _initial_child_buffer = attach_test_buffered_surface(&child, &shm, &qh, 1, 1)?;
    child.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 1920, 955)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    wait_for_server_commands(commands);

    let viewport = viewporter.get_viewport(&child, &qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_desync();

    let stages = [
        (
            XdgWindowGeometry::new(0, 0, 1920, 955),
            (1920, 955),
            (1920, 955),
        ),
        (
            XdgWindowGeometry::new(10, 10, 1040, 1105),
            (1060, 1125),
            (1040, 1105),
        ),
        (
            XdgWindowGeometry::new(35, 23, 1040, 1105),
            (1110, 1176),
            (1040, 1105),
        ),
        (
            XdgWindowGeometry::new(43, 28, 1040, 1105),
            (1126, 1192),
            (1040, 1105),
        ),
        (
            XdgWindowGeometry::new(45, 29, 1040, 1105),
            (1130, 1195),
            (1040, 1105),
        ),
    ];
    let mut snapshots = Vec::with_capacity(stages.len());
    let mut _confined_pointer = None;
    let mut _constraint_region = None;

    for (stage_index, (geometry, root_buffer, child_size)) in stages.into_iter().enumerate() {
        if stage_index == 1 {
            // Supersede both pending parent-owned values before either parent
            // publication, as allowed by their double-buffered protocol state.
            xdg_surface.set_window_geometry(7, 8, geometry.width, geometry.height);
            subsurface.set_position(7, 8);
        }
        xdg_surface.set_window_geometry(geometry.x, geometry.y, geometry.width, geometry.height);
        subsurface.set_position(geometry.x, geometry.y);
        viewport.set_destination(child_size.0, child_size.1);
        commit_test_buffered_surface(
            &child,
            &shm,
            &qh,
            child_size.0 as usize,
            child_size.1 as usize,
        )?;
        connection.flush()?;
        queue.roundtrip(&mut state)?;
        wait_for_server_commands(commands);
        let pointer_motion_count_before_parent_commit = state.pointer_motion_count;
        let pointer_enter_count_before_parent_commit = state.pointer_enter_count;
        let pointer_leave_count_before_parent_commit = state.pointer_leave_count;
        let pointer_focus_before_parent_commit = capture_pointer_focus_surface_id(commands);
        let pointer_requests_before_parent_commit =
            capture_pointer_constraint_backend_requests(commands);
        let confined_region_update_count_before_parent_commit =
            pointer_requests_before_parent_commit
                .iter()
                .filter(|request| {
                    matches!(
                        request,
                        PointerConstraintBackendRequest::UpdateConfinedRegion { .. }
                    )
                })
                .count();

        let committed_geometry_before = capture_committed_window_geometry(commands);
        let tree_before_parent_commit = capture_renderable_surface_snapshot(commands);

        commit_test_buffered_surface(
            &parent,
            &shm,
            &qh,
            root_buffer.0 as usize,
            root_buffer.1 as usize,
        )?;
        connection.flush()?;
        queue.roundtrip(&mut state)?;
        wait_for_server_commands(commands);
        let pointer_motion_count_after_parent_commit = state.pointer_motion_count;
        let pointer_enter_count_after_parent_commit = state.pointer_enter_count;
        let pointer_leave_count_after_parent_commit = state.pointer_leave_count;
        let pointer_focus_after_parent_commit = capture_pointer_focus_surface_id(commands);
        let pointer_requests_after_parent_commit =
            capture_pointer_constraint_backend_requests(commands);
        let confined_region_update_count_after_parent_commit = pointer_requests_after_parent_commit
            .iter()
            .filter(|request| {
                matches!(
                    request,
                    PointerConstraintBackendRequest::UpdateConfinedRegion { .. }
                )
            })
            .count();

        let committed_geometry_after = capture_committed_window_geometry(commands);
        let tree_after_parent_commit = capture_renderable_surface_snapshot(commands);
        let root_surface_id = tree_after_parent_commit
            .iter()
            .find(|surface| surface.parent_surface_id.is_none())
            .map(|surface| surface.surface_id);
        let logical_frame_origin = root_surface_id
            .and_then(|surface_id| capture_root_window_geometry(commands, surface_id))
            .map(|geometry| (geometry.placement.local_x, geometry.placement.local_y));

        snapshots.push(GeckoGeometryPublicationSnapshot {
            expected_geometry: geometry,
            requested_position: (geometry.x, geometry.y),
            committed_geometry_before,
            tree_before_parent_commit,
            pointer_motion_count_before_parent_commit,
            pointer_enter_count_before_parent_commit,
            pointer_leave_count_before_parent_commit,
            pointer_focus_before_parent_commit,
            confined_region_update_count_before_parent_commit,
            committed_geometry_after,
            tree_after_parent_commit,
            pointer_motion_count_after_parent_commit,
            pointer_enter_count_after_parent_commit,
            pointer_leave_count_after_parent_commit,
            pointer_focus_after_parent_commit,
            confined_region_update_count_after_parent_commit,
            logical_frame_origin,
        });

        if stage_index == 0 {
            let child_snapshot = snapshots[0]
                .tree_after_parent_commit
                .iter()
                .find(|surface| surface.parent_surface_id.is_some())
                .expect("stage zero should activate the content child");
            commands.send(ServerCommand::PointerMotion {
                x: f64::from(child_snapshot.origin_x + 1037),
                y: f64::from(child_snapshot.origin_y + 519),
            })?;
            wait_for_server_commands(commands);
            queue.roundtrip(&mut state)?;
            assert_eq!(
                state.pointer_enter_surface_id,
                Some(child.id().protocol_id())
            );

            let region = compositor.create_region(&qh, ());
            region.add(1000, 500, 120, 40);
            let confined = constraints.confine_pointer(
                &child,
                &pointer,
                Some(&region),
                client_zwp_pointer_constraints_v1::Lifetime::Persistent,
                &qh,
                (),
            );
            child.commit();
            connection.flush()?;
            wait_for_server_commands(commands);
            queue.roundtrip(&mut state)?;
            let requests = capture_pointer_constraint_backend_requests(commands);
            let backend_id = requests
                .iter()
                .find_map(|request| match request {
                    PointerConstraintBackendRequest::ActivateConfined { id, .. } => Some(*id),
                    _ => None,
                })
                .expect("content child should activate its confined pointer constraint");
            commands.send(ServerCommand::PointerConstraintBackendActivated(backend_id))?;
            wait_for_server_commands(commands);
            queue.roundtrip(&mut state)?;
            let _ = capture_pointer_constraint_backend_requests(commands);
            state.pointer_motion_count = 0;
            state.pointer_enter_count = 0;
            state.pointer_leave_count = 0;
            _confined_pointer = Some(confined);
            _constraint_region = Some(region);
        }
    }

    Ok(snapshots)
}

pub(in crate::compositor::tests) fn capture_roleless_parent_subsurface_mapping(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RolelessParentSubsurfaceSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let parent_id = parent.id().protocol_id();
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_parent_roleless_commit = capture_xdg_role_snapshot(commands, parent_id);

    let child = compositor.create_surface(&qh, ());
    let child_id = child.id().protocol_id();
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_desync();
    commit_test_buffered_surface(&child, &shm, &qh, 9, 7)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_child_commit = capture_xdg_role_snapshot(commands, child_id);

    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    Ok(RolelessParentSubsurfaceSnapshots {
        after_parent_roleless_commit,
        after_child_commit,
        after_parent_relationship_commit: capture_xdg_role_snapshot(commands, parent_id),
        child_after_parent_relationship_commit: capture_xdg_role_snapshot(commands, child_id),
    })
}

pub(in crate::compositor::tests) fn capture_roleless_parent_later_mapping(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RolelessParentMappingSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (root, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 30, 20)?;
    root.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    commit_test_buffered_surface(&root, &shm, &qh, 30, 20)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let parent = compositor.create_surface(&qh, ());
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let child = compositor.create_surface(&qh, ());
    let child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    child_subsurface.set_desync();
    commit_test_buffered_surface(&child, &shm, &qh, 9, 7)?;
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_child_relationship_commit = capture_renderable_surface_snapshot(commands);

    commit_test_buffered_surface(&child, &shm, &qh, 13, 11)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_child_replacement = capture_renderable_surface_snapshot(commands);

    let _parent_subsurface = subcompositor.get_subsurface(&parent, &root, &qh, ());
    root.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_parent_becomes_mapped = capture_renderable_surface_snapshot(commands);

    Ok(RolelessParentMappingSnapshots {
        after_child_relationship_commit,
        after_child_replacement,
        after_parent_becomes_mapped,
    })
}

pub(in crate::compositor::tests) fn capture_roleless_applied_subsurface_feedback(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let presentation: client_wp_presentation::WpPresentation = globals.bind(&qh, 1..=2, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_desync();
    commit_test_buffered_surface(&child, &shm, &qh, 9, 7)?;
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let _feedback = presentation.feedback(&child, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 13, 11)?;
    connection.flush()?;
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn capture_mapped_subsurface_unmap_remap(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<SubsurfaceUnmapRemapSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 20, 15)?;
    let child = compositor.create_surface(&qh, ());
    let child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    child_subsurface.set_position(2, 3);
    let grandchild = compositor.create_surface(&qh, ());
    let grandchild_subsurface = subcompositor.get_subsurface(&grandchild, &child, &qh, ());
    grandchild_subsurface.set_position(4, 5);

    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    commit_test_buffered_surface(&grandchild, &shm, &qh, 3, 3)?;
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let parent_id = parent.id().protocol_id();
    let child_id = child.id().protocol_id();
    let grandchild_id = grandchild.id().protocol_id();
    let capture_tree = |commands: &Sender<ServerCommand>| SubsurfaceTreeContentSnapshots {
        parent: capture_xdg_role_snapshot(commands, parent_id),
        child: capture_xdg_role_snapshot(commands, child_id),
        grandchild: capture_xdg_role_snapshot(commands, grandchild_id),
    };

    let before_parent_null = capture_tree(commands);
    let before_parent_null_renderables = capture_renderable_surface_snapshot(commands);

    parent.attach(None, 0, 0);
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_parent_null = capture_tree(commands);
    let after_parent_null_renderables = capture_renderable_surface_snapshot(commands);

    child_subsurface.set_desync();
    commit_test_buffered_surface(&child, &shm, &qh, 6, 6)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_hidden_child_replacement = capture_tree(commands);
    let after_hidden_child_replacement_renderables = capture_renderable_surface_snapshot(commands);

    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_parent_remap = capture_tree(commands);
    let after_parent_remap_renderables = capture_renderable_surface_snapshot(commands);

    child.attach(None, 0, 0);
    child.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_child_null = capture_tree(commands);
    let after_child_null_renderables = capture_renderable_surface_snapshot(commands);

    commit_test_buffered_surface(&child, &shm, &qh, 7, 7)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_child_remap = capture_tree(commands);
    let after_child_remap_renderables = capture_renderable_surface_snapshot(commands);

    Ok(SubsurfaceUnmapRemapSnapshots {
        before_parent_null,
        after_parent_null,
        after_hidden_child_replacement,
        after_parent_remap,
        after_child_null,
        after_child_remap,
        before_parent_null_renderables,
        after_parent_null_renderables,
        after_hidden_child_replacement_renderables,
        after_parent_remap_renderables,
        after_child_null_renderables,
        after_child_remap_renderables,
    })
}

#[derive(Debug)]
pub(in crate::compositor::tests) struct DestroyedSubsurfaceRecreateSnapshots {
    pub(in crate::compositor::tests) parent_id: u32,
    pub(in crate::compositor::tests) before_destroy: SubsurfaceTreeContentSnapshots,
    pub(in crate::compositor::tests) after_destroy: SubsurfaceTreeContentSnapshots,
    pub(in crate::compositor::tests) after_recreate: SubsurfaceTreeContentSnapshots,
    pub(in crate::compositor::tests) parent_stack_after_destroy: SubsurfaceStackStateSnapshot,
}

pub(in crate::compositor::tests) fn capture_destroyed_subsurface_recreate(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<DestroyedSubsurfaceRecreateSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 20, 15)?;
    let child = compositor.create_surface(&qh, ());
    let child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    child_subsurface.set_position(2, 3);
    child_subsurface.set_desync();
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let parent_id = parent.id().protocol_id();
    let child_id = child.id().protocol_id();
    let capture_tree = |commands: &Sender<ServerCommand>| SubsurfaceTreeContentSnapshots {
        parent: capture_xdg_role_snapshot(commands, parent_id),
        child: capture_xdg_role_snapshot(commands, child_id),
        grandchild: capture_xdg_role_snapshot(commands, child_id),
    };

    let before_destroy = capture_tree(commands);
    child_subsurface.destroy();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_destroy = capture_tree(commands);
    let parent_stack_after_destroy =
        capture_subsurface_stack_state(commands, before_destroy.parent.surface_id);

    let recreated = subcompositor.get_subsurface(&child, &parent, &qh, ());
    recreated.set_desync();
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_recreate = capture_tree(commands);

    Ok(DestroyedSubsurfaceRecreateSnapshots {
        parent_id,
        before_destroy,
        after_destroy,
        after_recreate,
        parent_stack_after_destroy,
    })
}

#[derive(Debug)]
pub(in crate::compositor::tests) struct DestroyedDmabufSubsurfaceSnapshots {
    pub(in crate::compositor::tests) before_destroy: SurfaceBufferOwnershipSnapshot,
    pub(in crate::compositor::tests) after_destroy: SurfaceBufferOwnershipSnapshot,
    pub(in crate::compositor::tests) after_dormant_replacement: SurfaceBufferOwnershipSnapshot,
    pub(in crate::compositor::tests) after_recreate: SurfaceBufferOwnershipSnapshot,
}

pub(in crate::compositor::tests) fn capture_destroyed_dmabuf_subsurface(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<DestroyedDmabufSubsurfaceSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 20, 15)?;
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let child = compositor.create_surface(&qh, ());
    let child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    child_subsurface.set_desync();
    let first_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff11_2233)?;
    child.attach(Some(&first_buffer), 0, 0);
    child.damage_buffer(0, 0, 2, 2);
    child.commit();
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let child_id = child.id().protocol_id();
    let before_destroy = capture_surface_buffer_ownership(commands, child_id);

    child_subsurface.destroy();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_destroy = capture_surface_buffer_ownership(commands, child_id);

    let second_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff44_5566)?;
    child.attach(Some(&second_buffer), 0, 0);
    child.damage_buffer(0, 0, 2, 2);
    child.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_dormant_replacement = capture_surface_buffer_ownership(commands, child_id);

    let _recreated = subcompositor.get_subsurface(&child, &parent, &qh, ());
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_recreate = capture_surface_buffer_ownership(commands, child_id);

    Ok(DestroyedDmabufSubsurfaceSnapshots {
        before_destroy,
        after_destroy,
        after_dormant_replacement,
        after_recreate,
    })
}

#[derive(Debug)]
pub(in crate::compositor::tests) struct DestroyedCachedSubsurfaceSnapshots {
    pub(in crate::compositor::tests) before_destroy: XdgRoleSnapshot,
    pub(in crate::compositor::tests) after_destroy: XdgRoleSnapshot,
    pub(in crate::compositor::tests) after_dormant_commit: XdgRoleSnapshot,
    pub(in crate::compositor::tests) after_recreate: XdgRoleSnapshot,
    pub(in crate::compositor::tests) after_recreate_renderables: Vec<RenderableSurfaceSnapshot>,
}

pub(in crate::compositor::tests) fn capture_destroyed_cached_subsurface(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<DestroyedCachedSubsurfaceSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 20, 15)?;
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let child = compositor.create_surface(&qh, ());
    let child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    commit_test_buffered_surface(&child, &shm, &qh, 13, 9)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let child_id = child.id().protocol_id();
    let before_destroy = capture_xdg_role_snapshot(commands, child_id);
    child_subsurface.destroy();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_destroy = capture_xdg_role_snapshot(commands, child_id);

    commit_test_buffered_surface(&child, &shm, &qh, 17, 11)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_dormant_commit = capture_xdg_role_snapshot(commands, child_id);

    let _recreated = subcompositor.get_subsurface(&child, &parent, &qh, ());
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_recreate = capture_xdg_role_snapshot(commands, child_id);
    let after_recreate_renderables = capture_renderable_surface_snapshot(commands);

    Ok(DestroyedCachedSubsurfaceSnapshots {
        before_destroy,
        after_destroy,
        after_dormant_commit,
        after_recreate,
        after_recreate_renderables,
    })
}

#[derive(Debug)]
pub(in crate::compositor::tests) struct DetachedLatchedSubsurfaceSnapshots {
    pub(in crate::compositor::tests) parent: XdgRoleSnapshot,
    pub(in crate::compositor::tests) child_after_destroy: XdgRoleSnapshot,
    pub(in crate::compositor::tests) child_after_parent_release: XdgRoleSnapshot,
}

pub(in crate::compositor::tests) fn capture_detached_latched_subsurface(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<DetachedLatchedSubsurfaceSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let timing: client_wp_commit_timing_manager_v1::WpCommitTimingManagerV1 =
        globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 20, 15)?;
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let child = compositor.create_surface(&qh, ());
    let child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;

    let timer = timing.get_timer(&parent, &qh, ());
    let now = PresentationTimestamp::from_clock(PresentationClock::Monotonic)?;
    let (seconds_hi, seconds_lo) = now.protocol_seconds();
    timer.set_timestamp(seconds_hi, seconds_lo.saturating_add(1), now.nanoseconds());
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let parent_id = parent.id().protocol_id();
    let child_id = child.id().protocol_id();
    child_subsurface.destroy();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let child_after_destroy = capture_xdg_role_snapshot(commands, child_id);
    let parent_after_destroy = capture_xdg_role_snapshot(commands, parent_id);

    std::thread::sleep(std::time::Duration::from_millis(1_100));
    queue.roundtrip(&mut RegistryTestState::default())?;
    let child_after_parent_release = capture_xdg_role_snapshot(commands, child_id);

    Ok(DetachedLatchedSubsurfaceSnapshots {
        parent: parent_after_destroy,
        child_after_destroy,
        child_after_parent_release,
    })
}

#[derive(Debug)]
pub(in crate::compositor::tests) struct DetachedInheritedDesyncSnapshots {
    pub(in crate::compositor::tests) child_after_destroy: XdgRoleSnapshot,
    pub(in crate::compositor::tests) grandchild_after_destroy: XdgRoleSnapshot,
    pub(in crate::compositor::tests) child_after_recreate: XdgRoleSnapshot,
    pub(in crate::compositor::tests) grandchild_after_recreate: XdgRoleSnapshot,
}

pub(in crate::compositor::tests) fn capture_detached_inherited_desync_subsurface(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<DetachedInheritedDesyncSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 20, 15)?;
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let child = compositor.create_surface(&qh, ());
    let child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let grandchild = compositor.create_surface(&qh, ());
    let grandchild_subsurface = subcompositor.get_subsurface(&grandchild, &child, &qh, ());
    grandchild_subsurface.set_desync();
    commit_test_buffered_surface(&grandchild, &shm, &qh, 3, 3)?;

    let child_id = child.id().protocol_id();
    let grandchild_id = grandchild.id().protocol_id();
    child_subsurface.destroy();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let child_after_destroy = capture_xdg_role_snapshot(commands, child_id);
    let grandchild_after_destroy = capture_xdg_role_snapshot(commands, grandchild_id);

    let _recreated = subcompositor.get_subsurface(&child, &parent, &qh, ());
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    child.commit();
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let child_after_recreate = capture_xdg_role_snapshot(commands, child_id);
    let grandchild_after_recreate = capture_xdg_role_snapshot(commands, grandchild_id);

    Ok(DetachedInheritedDesyncSnapshots {
        child_after_destroy,
        grandchild_after_destroy,
        child_after_recreate,
        grandchild_after_recreate,
    })
}

#[derive(Debug)]
pub(in crate::compositor::tests) struct DetachedSiblingTransactionSnapshots {
    pub(in crate::compositor::tests) child_after_destroy: XdgRoleSnapshot,
    pub(in crate::compositor::tests) renderables_after_destroy: Vec<RenderableSurfaceSnapshot>,
    pub(in crate::compositor::tests) renderables_after_parent_release:
        Vec<RenderableSurfaceSnapshot>,
}

pub(in crate::compositor::tests) fn capture_detached_sibling_transaction(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<DetachedSiblingTransactionSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let timing: client_wp_commit_timing_manager_v1::WpCommitTimingManagerV1 =
        globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 20, 15)?;
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let child = compositor.create_surface(&qh, ());
    let child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    let sibling = compositor.create_surface(&qh, ());
    let _sibling_subsurface = subcompositor.get_subsurface(&sibling, &parent, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;
    commit_test_buffered_surface(&sibling, &shm, &qh, 6, 6)?;
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    commit_test_buffered_surface(&child, &shm, &qh, 13, 9)?;
    commit_test_buffered_surface(&sibling, &shm, &qh, 9, 9)?;
    let timer = timing.get_timer(&parent, &qh, ());
    let now = PresentationTimestamp::from_clock(PresentationClock::Monotonic)?;
    let (seconds_hi, seconds_lo) = now.protocol_seconds();
    timer.set_timestamp(seconds_hi, seconds_lo.saturating_add(1), now.nanoseconds());
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let child_id = child.id().protocol_id();
    child_subsurface.destroy();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let child_after_destroy = capture_xdg_role_snapshot(commands, child_id);
    let renderables_after_destroy = capture_renderable_surface_snapshot(commands);

    std::thread::sleep(std::time::Duration::from_millis(1_100));
    queue.roundtrip(&mut RegistryTestState::default())?;
    let renderables_after_parent_release = capture_renderable_surface_snapshot(commands);

    Ok(DetachedSiblingTransactionSnapshots {
        child_after_destroy,
        renderables_after_destroy,
        renderables_after_parent_release,
    })
}

#[derive(Debug)]
pub(in crate::compositor::tests) struct DestroyedNestedSubsurfaceSnapshots {
    pub(in crate::compositor::tests) parent_id: u32,
    pub(in crate::compositor::tests) child_id: u32,
    pub(in crate::compositor::tests) grandchild_id: u32,
    pub(in crate::compositor::tests) before_destroy: SubsurfaceTreeContentSnapshots,
    pub(in crate::compositor::tests) after_destroy: SubsurfaceTreeContentSnapshots,
    pub(in crate::compositor::tests) after_recreate: SubsurfaceTreeContentSnapshots,
    pub(in crate::compositor::tests) parent_stack_after_destroy: SubsurfaceStackStateSnapshot,
    pub(in crate::compositor::tests) child_stack_before_destroy: SubsurfaceStackStateSnapshot,
    pub(in crate::compositor::tests) child_stack_after_destroy: SubsurfaceStackStateSnapshot,
}

pub(in crate::compositor::tests) fn capture_destroyed_nested_subsurface(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<DestroyedNestedSubsurfaceSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 20, 15)?;
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let child = compositor.create_surface(&qh, ());
    let child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    child_subsurface.set_position(2, 3);
    child_subsurface.set_desync();
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let grandchild = compositor.create_surface(&qh, ());
    let grandchild_subsurface = subcompositor.get_subsurface(&grandchild, &child, &qh, ());
    grandchild_subsurface.set_position(4, 5);
    grandchild_subsurface.set_desync();
    commit_test_buffered_surface(&grandchild, &shm, &qh, 3, 3)?;
    child.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let parent_id = parent.id().protocol_id();
    let child_id = child.id().protocol_id();
    let grandchild_id = grandchild.id().protocol_id();
    let capture_tree = |commands: &Sender<ServerCommand>| SubsurfaceTreeContentSnapshots {
        parent: capture_xdg_role_snapshot(commands, parent_id),
        child: capture_xdg_role_snapshot(commands, child_id),
        grandchild: capture_xdg_role_snapshot(commands, grandchild_id),
    };

    let before_destroy = capture_tree(commands);
    let child_stack_before_destroy =
        capture_subsurface_stack_state(commands, before_destroy.child.surface_id);
    child_subsurface.destroy();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_destroy = capture_tree(commands);
    let parent_stack_after_destroy =
        capture_subsurface_stack_state(commands, before_destroy.parent.surface_id);
    let child_stack_after_destroy =
        capture_subsurface_stack_state(commands, before_destroy.child.surface_id);

    let _recreated = subcompositor.get_subsurface(&child, &parent, &qh, ());
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_recreate = capture_tree(commands);

    Ok(DestroyedNestedSubsurfaceSnapshots {
        parent_id,
        child_id,
        grandchild_id,
        before_destroy,
        after_destroy,
        after_recreate,
        parent_stack_after_destroy,
        child_stack_before_destroy,
        child_stack_after_destroy,
    })
}

pub(in crate::compositor::tests) fn capture_inactive_descendant_across_parent_null(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<InactiveDescendantSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 20, 15)?;
    let child = compositor.create_surface(&qh, ());
    let _child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    let grandchild = compositor.create_surface(&qh, ());
    let grandchild_subsurface = subcompositor.get_subsurface(&grandchild, &child, &qh, ());
    grandchild_subsurface.set_position(4, 5);

    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    commit_test_buffered_surface(&grandchild, &shm, &qh, 3, 3)?;
    child.commit();
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let parent_id = parent.id().protocol_id();
    let child_id = child.id().protocol_id();
    let grandchild_id = grandchild.id().protocol_id();
    let capture_tree = |commands: &Sender<ServerCommand>| SubsurfaceTreeContentSnapshots {
        parent: capture_xdg_role_snapshot(commands, parent_id),
        child: capture_xdg_role_snapshot(commands, child_id),
        grandchild: capture_xdg_role_snapshot(commands, grandchild_id),
    };

    let before_parent_null = capture_tree(commands);
    parent.attach(None, 0, 0);
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_parent_null = capture_tree(commands);

    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_parent_remap = capture_tree(commands);

    Ok(InactiveDescendantSnapshots {
        before_parent_null,
        after_parent_null,
        after_parent_remap,
    })
}

pub(in crate::compositor::tests) fn capture_dmabuf_subsurface_ownership_across_parent_null(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<DmabufSubsurfaceOwnershipSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 20, 15)?;
    let parent_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff44_5566)?;
    let child = compositor.create_surface(&qh, ());
    let child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    child_subsurface.set_desync();

    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let child_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff11_2233)?;
    child.attach(Some(&child_buffer), 0, 0);
    child.damage_buffer(0, 0, 2, 2);
    child.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    parent.attach(Some(&parent_buffer), 0, 0);
    parent.damage_buffer(0, 0, 2, 2);
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let parent_id = parent.id().protocol_id();
    let child_id = child.id().protocol_id();
    let before_parent_null = capture_surface_buffer_ownership(commands, parent_id);
    let before_parent_null_child = capture_surface_buffer_ownership(commands, child_id);

    parent.attach(None, 0, 0);
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_parent_null = capture_surface_buffer_ownership(commands, parent_id);
    let after_parent_null_child = capture_surface_buffer_ownership(commands, child_id);

    child.attach(None, 0, 0);
    child.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_child_null = capture_surface_buffer_ownership(commands, child_id);

    Ok(DmabufSubsurfaceOwnershipSnapshots {
        before_parent_null,
        before_parent_null_child,
        after_parent_null,
        after_parent_null_child,
        after_child_null,
    })
}

pub(in crate::compositor::tests) fn capture_desynchronized_subsurface_before_parent_commit(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<DesynchronizedSubsurfaceSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 20, 15)?;
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;

    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_desync();
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let before_parent = capture_renderable_surface_snapshot(commands);

    commit_test_buffered_surface(&child, &shm, &qh, 9, 7)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_latest_child = capture_renderable_surface_snapshot(commands);

    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_parent = capture_renderable_surface_snapshot(commands);

    Ok(DesynchronizedSubsurfaceSnapshots {
        before_parent,
        after_latest_child,
        after_parent,
    })
}

pub(in crate::compositor::tests) fn capture_destroyed_latched_subsurface_snapshot(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<Vec<RenderableSurfaceSnapshot>, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let timing: client_wp_commit_timing_manager_v1::WpCommitTimingManagerV1 =
        globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 20, 15)?;
    let child = compositor.create_surface(&qh, ());
    let _child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let doomed = compositor.create_surface(&qh, ());
    let doomed_subsurface = subcompositor.get_subsurface(&doomed, &parent, &qh, ());
    doomed_subsurface.set_position(30, 40);
    commit_test_buffered_surface(&doomed, &shm, &qh, 9, 7)?;
    let timer = timing.get_timer(&parent, &qh, ());
    let now = PresentationTimestamp::from_clock(PresentationClock::Monotonic)?;
    let (seconds_hi, seconds_lo) = now.protocol_seconds();
    timer.set_timestamp(seconds_hi, seconds_lo.saturating_add(1), now.nanoseconds());
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    doomed_subsurface.destroy();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    queue.roundtrip(&mut RegistryTestState::default())?;
    Ok(capture_renderable_surface_snapshot(commands))
}

#[derive(Debug)]
pub(in crate::compositor::tests) struct DestroyRecreateSubsurfaceSnapshots {
    pub(in crate::compositor::tests) before_release: Vec<RenderableSurfaceSnapshot>,
    pub(in crate::compositor::tests) old_stack: SubsurfaceStackStateSnapshot,
    pub(in crate::compositor::tests) after_release: Vec<RenderableSurfaceSnapshot>,
    pub(in crate::compositor::tests) new_stack: SubsurfaceStackStateSnapshot,
}

pub(in crate::compositor::tests) fn capture_destroy_recreate_subsurface_aba(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<DestroyRecreateSubsurfaceSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let timing: client_wp_commit_timing_manager_v1::WpCommitTimingManagerV1 =
        globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 20, 15)?;
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let parent_id = capture_renderable_surface_snapshot(commands)
        .into_iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .map(|surface| surface.surface_id)
        .expect("parent should be renderable before ABA capture");

    let child = compositor.create_surface(&qh, ());
    let first_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    first_subsurface.set_position(10, 10);
    first_subsurface.set_desync();
    commit_test_buffered_surface(&child, &shm, &qh, 11, 7)?;

    let first_timer = timing.get_timer(&parent, &qh, ());
    let first_now = PresentationTimestamp::from_clock(PresentationClock::Monotonic)?;
    let (first_seconds_hi, first_seconds_lo) = first_now.protocol_seconds();
    first_timer.set_timestamp(
        first_seconds_hi,
        first_seconds_lo.saturating_add(1),
        first_now.nanoseconds(),
    );
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    first_subsurface.destroy();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    let second_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    second_subsurface.set_position(20, 20);
    second_subsurface.set_desync();
    commit_test_buffered_surface(&child, &shm, &qh, 13, 9)?;

    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    std::thread::sleep(std::time::Duration::from_millis(100));
    queue.roundtrip(&mut RegistryTestState::default())?;
    wait_for_server_commands(commands);
    let before_release = capture_renderable_surface_snapshot(commands);
    let old_stack = capture_subsurface_stack_state(commands, parent_id);

    std::thread::sleep(std::time::Duration::from_millis(1_100));
    queue.roundtrip(&mut RegistryTestState::default())?;
    wait_for_server_commands(commands);
    let after_release = capture_renderable_surface_snapshot(commands);
    let new_stack = capture_subsurface_stack_state(commands, parent_id);
    Ok(DestroyRecreateSubsurfaceSnapshots {
        before_release,
        old_stack,
        after_release,
        new_stack,
    })
}

pub(in crate::compositor::tests) fn capture_preactivation_subsurface_presentation_feedback(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let presentation: client_wp_presentation::WpPresentation = globals.bind(&qh, 1..=2, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_desync();
    let _feedback = presentation.feedback(&child, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 9, 7)?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
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
    subtree.commit();

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
    let shm: client_wl_shm::WlShm = focused_globals.bind(&focused_qh, 1..=1, ())?;
    let seat: client_wl_seat::WlSeat = focused_globals.bind(&focused_qh, 1..=7, ())?;
    let _keyboard = seat.get_keyboard(&focused_qh, ());
    let surface = compositor.create_surface(&focused_qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &focused_qh, ());
    let _toplevel = xdg_surface.get_toplevel(&focused_qh, ());
    surface.commit();
    focused_connection.flush()?;

    let mut focused_state = RegistryTestState::default();
    focused_queue.roundtrip(&mut focused_state)?;
    commit_test_buffered_surface(&surface, &shm, &focused_qh, 32, 32)?;
    focused_connection.flush()?;
    wait_for_server_commands(commands);
    focused_queue.roundtrip(&mut focused_state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 30,
        pressed: true,
    })?;
    focused_queue.roundtrip(&mut focused_state)?;
    Ok(focused_state)
}
