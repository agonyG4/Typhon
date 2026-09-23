use super::*;
use crate::core::SceneNodeId;
use crate::presentation_animation::{
    PresentationRetainedVisualIdentity, PresentationRetainedVisualKind, PresentationRevisionId,
    PresentationTransactionId,
};
use crate::window_lifecycle_animation::{
    LampWindowSample, LifecycleDirection, LifecycleSceneSample, LifecycleVisualGroup,
};

fn identity_for_test(window_id: WindowId) -> PresentationRetainedVisualIdentity {
    PresentationRetainedVisualIdentity::new(
        SceneNodeId::from_raw(window_id.get()).expect("test scene node"),
        PresentationRetainedVisualKind::WindowLifecycle,
        PresentationTransactionId::from_raw(1).expect("test transaction id"),
        PresentationRevisionId::from_raw(1).expect("test revision id"),
    )
}

fn active_lamp(anchor_rect: PresentationRect) -> LifecycleSceneSample {
    let visual_group = LifecycleVisualGroup::from_bounds(
        PresentationRect::new(100.0, 100.0, 400.0, 300.0).unwrap(),
        PresentationRect::new(100.0, 100.0, 400.0, 300.0).unwrap(),
        PresentationRect::new(100.0, 100.0, 400.0, 300.0).unwrap(),
        anchor_rect,
        1280,
        800,
    )
    .unwrap();
    LifecycleSceneSample {
        sampled_at: AnimationTime::from_nanos(1),
        lamps: vec![LampWindowSample {
            window_id: WindowId::from_raw(1).unwrap(),
            root_surface_id: 901,
            presentation_identity: identity_for_test(WindowId::from_raw(1).unwrap()),
            payload_id:
                crate::compositor::PresentationRetainedVisualPayloadId::from_origin_identity(
                    identity_for_test(WindowId::from_raw(1).unwrap()),
                ),
            visual_group,
            progress: 0.4,
            opacity: 1.0,
            direction: LifecycleDirection::Minimize,
            mathematically_settled: false,
        }],
        visual_sources: Vec::new(),
    }
}

#[test]
fn active_lamp_promotes_matching_astrea_dock_top_surface() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let mut state = RegistryTestState::default();

    let (dock_surface, _dock_layer) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "astrea-dock",
        64,
        64,
    );
    let dock_child = compositor.create_surface(&qh, ());
    let dock_subsurface = subcompositor.get_subsurface(&dock_child, &dock_surface, &qh, ());
    dock_subsurface.set_position(12, 12);
    commit_test_buffered_surface(&dock_child, &shm, &qh, 16, 16).unwrap();
    dock_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let active_lamp = active_lamp(PresentationRect::new(608.0, 368.0, 64.0, 64.0).unwrap());

    let server = stop_controllable_test_server(commands, server_thread);
    let dock_surface_id = server
        .renderable_surfaces()
        .first()
        .expect("mapped Dock surface")
        .surface_id;
    let external_overlay_surface_ids = server.external_overlay_surface_ids(&active_lamp);

    assert!(external_overlay_surface_ids.contains(&dock_surface_id));
    let dock_tree_ids = server
        .renderable_surfaces()
        .iter()
        .filter(|surface| {
            server.state.root_surface_id_for_surface(surface.surface_id) == dock_surface_id
        })
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    assert_eq!(
        dock_tree_ids.len(),
        2,
        "Dock root and its subsurface must remain renderable"
    );
    assert!(
        dock_tree_ids
            .iter()
            .all(|surface_id| external_overlay_surface_ids.contains(surface_id))
    );
}

#[test]
fn dock_promotion_is_lifecycle_scoped_and_keeps_true_overlays_ordered_after_it() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let mut state = RegistryTestState::default();

    let (_ordinary_surface, _ordinary) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "ordinary-top",
        64,
        64,
    );
    let (_dock_surface, _dock) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "astrea-dock",
        64,
        64,
    );
    let (other_dock_surface, other_dock) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "astrea-dock",
    );
    other_dock.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top | client_zwlr_layer_surface_v1::Anchor::Left,
    );
    other_dock.set_size(64, 64);
    other_dock_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&other_dock_surface, &shm, &qh, 64, 64).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let (_overlay_surface, _overlay) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Overlay,
        "true-overlay",
        32,
        32,
    );

    let server = stop_controllable_test_server(commands, server_thread);
    let all_surface_ids = server
        .renderable_surfaces()
        .iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    let origins = crate::compositor::surface_origins(server.renderable_surfaces());
    let matching_dock_root = server
        .state
        .layer_surfaces
        .iter()
        .filter(|(_, role)| role.namespace == "astrea-dock")
        .find_map(|(root_id, _)| {
            server
                .renderable_surfaces()
                .iter()
                .zip(origins.iter().copied())
                .find(|(surface, origin)| surface.surface_id == *root_id && *origin == (608, 368))
                .map(|_| *root_id)
        })
        .expect("centered Dock root");
    let dock_bounds = server
        .renderable_surfaces()
        .iter()
        .find(|surface| surface.surface_id == matching_dock_root)
        .expect("matching Dock root")
        .clone();
    let dock_origin = origins[server
        .renderable_surfaces()
        .iter()
        .position(|surface| surface.surface_id == matching_dock_root)
        .expect("matching Dock origin")];
    let dock_anchor = PresentationRect::new(
        f64::from(dock_origin.0),
        f64::from(dock_origin.1),
        f64::from(dock_bounds.width),
        f64::from(dock_bounds.height),
    )
    .unwrap();
    let active_lamp = active_lamp(dock_anchor);
    let ordinary = server.external_overlay_surface_ids(&LifecycleSceneSample {
        sampled_at: AnimationTime::from_nanos(1),
        lamps: Vec::new(),
        visual_sources: Vec::new(),
    });
    let promoted = server.external_overlay_surface_ids(&active_lamp);
    let ordinary_root = server
        .state
        .layer_surfaces
        .iter()
        .find(|(_, role)| role.namespace == "ordinary-top")
        .map(|(root_id, _)| *root_id)
        .expect("ordinary Top root");
    let other_dock_root = server
        .state
        .layer_surfaces
        .iter()
        .filter(|(root_id, role)| {
            role.namespace == "astrea-dock" && **root_id != matching_dock_root
        })
        .map(|(root_id, _)| *root_id)
        .next()
        .expect("unmatched Dock root");
    let overlay_root = server
        .state
        .layer_surfaces
        .iter()
        .find(|(_, role)| role.committed.layer == Layer::Overlay)
        .map(|(root_id, _)| *root_id)
        .expect("true Overlay root");

    let overlay_ids = ordinary
        .iter()
        .copied()
        .filter(|id| server.state.root_surface_id_for_surface(*id) == overlay_root)
        .collect::<Vec<_>>();
    assert!(!overlay_ids.is_empty(), "true Overlay must remain external");
    assert!(!ordinary.contains(&ordinary_root));
    assert!(!ordinary.contains(&matching_dock_root));
    assert!(!ordinary.contains(&other_dock_root));
    assert!(
        !ordinary
            .iter()
            .any(|id| server.state.root_surface_id_for_surface(*id) == matching_dock_root)
    );
    assert!(promoted.contains(&matching_dock_root));
    assert!(!promoted.contains(&ordinary_root));
    assert!(!promoted.contains(&other_dock_root));
    assert!(promoted.iter().any(|id| overlay_ids.contains(id)));
    let promoted_dock_index = promoted
        .iter()
        .position(|id| server.state.root_surface_id_for_surface(*id) == matching_dock_root)
        .expect("promoted Dock index");
    let promoted_overlay_index = promoted
        .iter()
        .position(|id| overlay_ids.contains(id))
        .expect("promoted Overlay index");
    assert!(promoted_dock_index < promoted_overlay_index);
    assert!(promoted.iter().all(|id| all_surface_ids.contains(id)));
    let base_ids = all_surface_ids
        .iter()
        .copied()
        .filter(|id| !promoted.contains(id))
        .collect::<Vec<_>>();
    assert!(promoted.iter().all(|id| !base_ids.contains(id)));

    let settled = server.external_overlay_surface_ids(&LifecycleSceneSample {
        sampled_at: AnimationTime::from_nanos(2),
        lamps: Vec::new(),
        visual_sources: Vec::new(),
    });
    assert_eq!(settled, ordinary);
    assert!(!settled.contains(&matching_dock_root));
    assert!(overlay_ids.iter().all(|id| settled.contains(id)));
}

#[test]
fn layer_popup_renders_above_top_parent_and_gets_input_first() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
    let _pointer = seat.get_pointer(&qh, ());
    let mut state = RegistryTestState::default();

    let (_parent_surface, parent_layer) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "popup-parent-top",
        200,
        120,
    );
    let (_popup_surface, _popup) = create_layer_popup(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &wm_base,
        &qh,
        &parent_layer,
        60,
        40,
    );
    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(surfaces.len(), 2);
    let popup_surface_id = surfaces[1].surface_id;
    assert_eq!(surfaces[1].surface_id, popup_surface_id);
    assert_eq!(surfaces[1].parent_surface_id, Some(surfaces[0].surface_id));
    let popup_output_x = surfaces[1].origin_x + 5;
    let popup_output_y = surfaces[1].origin_y + 5;

    commands
        .send(ServerCommand::PointerMotion {
            x: f64::from(popup_output_x),
            y: f64::from(popup_output_y),
        })
        .unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(
        capture_pointer_focus_surface_id(&commands),
        Some(popup_surface_id)
    );

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn duplicate_layer_popup_association_is_rejected() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (running, server_thread) = spawn_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let mut state = RegistryTestState::default();
    let (_parent_surface, parent_layer) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "popup-parent-duplicate",
        200,
        120,
    );
    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(60, 40);
    positioner.set_anchor_rect(0, 0, 1, 1);
    let popup = popup_xdg_surface.get_popup(None, &positioner, &qh, ());
    parent_layer.get_popup(&popup);
    parent_layer.get_popup(&popup);
    connection.flush().unwrap();

    assert!(queue.roundtrip(&mut state).is_err());
    stop_test_server(running, server_thread);
}

#[test]
fn layer_parent_unmap_destroy_and_popup_destroy_cleanup_popup_state() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let mut state = RegistryTestState::default();
    let (parent_surface, parent_layer) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "popup-parent-unmap",
        200,
        120,
    );
    let (_popup_surface, popup) = create_layer_popup(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &wm_base,
        &qh,
        &parent_layer,
        60,
        40,
    );
    assert_eq!(capture_renderable_surface_count(&commands), 2);

    popup.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    let (_popup_surface, _popup) = create_layer_popup(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &wm_base,
        &qh,
        &parent_layer,
        60,
        40,
    );
    assert_eq!(capture_renderable_surface_count(&commands), 2);
    parent_surface.attach(None, 0, 0);
    parent_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn layer_parent_unmap_dismisses_popup_but_late_popup_destroy_is_idempotent() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let mut state = RegistryTestState::default();

    let (parent_surface, parent_layer) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "popup-parent-unmap",
        200,
        80,
    );
    let (popup_surface, popup) = create_layer_popup(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &wm_base,
        &qh,
        &parent_layer,
        64,
        40,
    );
    let popup_surface_id = popup_surface.id().protocol_id();
    assert_eq!(capture_renderable_surface_count(&commands), 2);

    parent_surface.attach(None, 0, 0);
    parent_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let snapshot = capture_xdg_role_snapshot(&commands, popup_surface_id);
    assert_eq!(snapshot.popup_count, 1);
    assert_eq!(snapshot.popup_node_count, 1);
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    popup.destroy();
    popup_surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let snapshot = capture_xdg_role_snapshot(&commands, popup_surface_id);
    assert_eq!(snapshot.popup_count, 0);
    assert_eq!(snapshot.popup_node_count, 0);
    assert!(!snapshot.popup_grab_active);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn parent_layer_change_restacks_popup_with_parent() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    create_client_toplevel_with_sized_shm_buffer(&socket_path, 300, 200).unwrap();
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let mut state = RegistryTestState::default();
    let (parent_surface, parent_layer) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "popup-parent-layer-change",
        200,
        120,
    );
    let (_popup_surface, _popup) = create_layer_popup(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &wm_base,
        &qh,
        &parent_layer,
        60,
        40,
    );
    let popup_surface_id = capture_renderable_surface_snapshot(&commands)
        .last()
        .map(|surface| surface.surface_id)
        .unwrap();
    assert_eq!(
        capture_renderable_surface_snapshot(&commands)
            .last()
            .map(|surface| surface.surface_id),
        Some(popup_surface_id)
    );

    parent_layer.set_layer(client_zwlr_layer_shell_v1::Layer::Bottom);
    parent_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(surfaces[1].surface_id, popup_surface_id);
    assert_eq!(surfaces[2].width, 300);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn layer_surface_shm_render_unmap_and_remap_use_normal_scene_lifecycle() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();
    let (surface, layer_surface) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-shm-render",
        64,
        32,
    );
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    surface.attach(None, 0, 0);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 64, 32).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);
    layer_surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn layer_surface_frame_callback_and_presentation_feedback_publish_normally() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let presentation: client_wp_presentation::WpPresentation =
        globals.bind(&qh, 1..=2, ()).unwrap();
    let mut state = RegistryTestState::default();
    let (surface, _layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Overlay,
        "layer-frame-presentation",
    );
    _layer_surface.set_size(32, 32);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    surface.frame(&qh, ());
    presentation.feedback(&surface, &qh, ());
    commit_test_buffered_surface(&surface, &shm, &qh, 32, 32).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    commands.send(ServerCommand::PresentFrame).unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert!(state.frame_done);
    assert_eq!(state.presentation_presented_count, 1);
    assert_eq!(state.presentation_discarded_count, 0);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn layer_surface_dmabuf_import_and_reuse_publish_normally() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let first_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff11_1111).unwrap();
    let second_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff22_2222).unwrap();
    let (surface, _layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-dmabuf",
    );
    _layer_surface.set_size(2, 2);
    surface.set_buffer_scale(1);
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    surface.attach(Some(&first_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let first_snapshot = capture_renderable_surface_snapshot(&commands);
    assert_eq!(first_snapshot.len(), 1);

    surface.attach(Some(&second_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let second_snapshot = capture_renderable_surface_snapshot(&commands);
    assert_eq!(second_snapshot.len(), 1);
    assert_ne!(first_snapshot[0].buffer_id, second_snapshot[0].buffer_id);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn layer_surface_explicit_sync_waits_then_publishes_and_destroy_pending_is_safe() {
    let Some(acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-explicit-sync",
    );
    layer_surface.set_size(2, 2);
    let sync_surface = syncobj.get_surface(&surface, &qh, ());
    let acquire_timeline_fd = acquire_timeline.export_timeline_fd().unwrap();
    let release_timeline_fd = release_timeline.export_timeline_fd().unwrap();
    let sync_acquire_timeline = syncobj.import_timeline(acquire_timeline_fd.as_fd(), &qh, ());
    let sync_release_timeline = syncobj.import_timeline(release_timeline_fd.as_fd(), &qh, ());
    let first_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff44_4444).unwrap();
    let second_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff55_5555).unwrap();
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    acquire_timeline.signal_point(1).unwrap();
    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 1);
    sync_surface.set_release_point(&sync_release_timeline, 0, 2);
    surface.attach(Some(&first_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 3);
    sync_surface.set_release_point(&sync_release_timeline, 0, 4);
    surface.attach(Some(&second_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let blocked = capture_renderable_surface_snapshot(&commands);
    assert_eq!(blocked.len(), 1);
    acquire_timeline.signal_point(3).unwrap();
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let published = capture_renderable_surface_snapshot(&commands);
    assert_ne!(blocked[0].buffer_id, published[0].buffer_id);

    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 5);
    sync_surface.set_release_point(&sync_release_timeline, 0, 6);
    surface.attach(Some(&first_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    layer_surface.destroy();
    surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 7);
    assert!(queue.roundtrip(&mut state).is_err());

    commands.send(ServerCommand::Stop).unwrap();
    let server = server_thread.join().unwrap();
    let record = server
        .state
        .protocol_error_trace
        .records()
        .next()
        .expect("syncobj surface-destroyed error should be attributed");
    assert_eq!(record.interface, ProtocolErrorInterface::Syncobj);
    assert_eq!(record.category, ProtocolErrorCategory::SurfaceDestroyed);
    assert_eq!(
        record.error_code,
        Some(crate::compositor::SYNCOBJ_SURFACE_ERROR_NO_SURFACE)
    );
}

#[test]
fn layer_surface_explicit_sync_survives_timeline_proxy_destruction() {
    let Some(acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-timeline-proxy-destroy",
    );
    layer_surface.set_size(2, 2);
    let sync_surface = syncobj.get_surface(&surface, &qh, ());
    let acquire_timeline_fd = acquire_timeline.export_timeline_fd().unwrap();
    let release_timeline_fd = release_timeline.export_timeline_fd().unwrap();
    let sync_acquire_timeline = syncobj.import_timeline(acquire_timeline_fd.as_fd(), &qh, ());
    let sync_release_timeline = syncobj.import_timeline(release_timeline_fd.as_fd(), &qh, ());
    let buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff44_4444).unwrap();
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 1);
    sync_surface.set_release_point(&sync_release_timeline, 0, 2);
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    sync_acquire_timeline.destroy();
    sync_release_timeline.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    acquire_timeline.signal_point(1).unwrap();
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    layer_surface.destroy();
    surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn layer_surface_explicit_sync_survives_sync_surface_proxy_destruction() {
    let Some(acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-sync-surface-proxy-destroy",
    );
    layer_surface.set_size(2, 2);
    let sync_surface = syncobj.get_surface(&surface, &qh, ());
    let acquire_timeline_fd = acquire_timeline.export_timeline_fd().unwrap();
    let release_timeline_fd = release_timeline.export_timeline_fd().unwrap();
    let sync_acquire_timeline = syncobj.import_timeline(acquire_timeline_fd.as_fd(), &qh, ());
    let sync_release_timeline = syncobj.import_timeline(release_timeline_fd.as_fd(), &qh, ());
    let buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff44_4444).unwrap();
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 1);
    sync_surface.set_release_point(&sync_release_timeline, 0, 2);
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    sync_surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    acquire_timeline.signal_point(1).unwrap();
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    layer_surface.destroy();
    surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn layer_surface_explicit_sync_keeps_points_set_before_timeline_proxy_destruction() {
    let Some(acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-points-before-timeline-destroy",
    );
    layer_surface.set_size(2, 2);
    let sync_surface = syncobj.get_surface(&surface, &qh, ());
    let acquire_timeline_fd = acquire_timeline.export_timeline_fd().unwrap();
    let release_timeline_fd = release_timeline.export_timeline_fd().unwrap();
    let sync_acquire_timeline = syncobj.import_timeline(acquire_timeline_fd.as_fd(), &qh, ());
    let sync_release_timeline = syncobj.import_timeline(release_timeline_fd.as_fd(), &qh, ());
    let buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff44_4444).unwrap();
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 1);
    sync_surface.set_release_point(&sync_release_timeline, 0, 2);
    sync_acquire_timeline.destroy();
    sync_release_timeline.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    acquire_timeline.signal_point(1).unwrap();
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    layer_surface.destroy();
    surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn destroying_sync_surface_discards_uncommitted_points_for_a_fresh_replacement() {
    let Some(acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-sync-surface-replacement",
    );
    layer_surface.set_size(2, 2);
    let sync_surface = syncobj.get_surface(&surface, &qh, ());
    let acquire_timeline_fd = acquire_timeline.export_timeline_fd().unwrap();
    let release_timeline_fd = release_timeline.export_timeline_fd().unwrap();
    let sync_acquire_timeline = syncobj.import_timeline(acquire_timeline_fd.as_fd(), &qh, ());
    let sync_release_timeline = syncobj.import_timeline(release_timeline_fd.as_fd(), &qh, ());
    let first_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff44_4444).unwrap();
    let second_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff55_5555).unwrap();
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 1);
    sync_surface.set_release_point(&sync_release_timeline, 0, 2);
    sync_surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    surface.attach(Some(&first_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);
    let first_snapshot = capture_renderable_surface_snapshot(&commands);
    assert_ne!(first_snapshot[0].buffer_id, 0);

    let replacement = syncobj.get_surface(&surface, &qh, ());
    replacement.set_acquire_point(&sync_acquire_timeline, 0, 3);
    replacement.set_release_point(&sync_release_timeline, 0, 4);
    surface.attach(Some(&second_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let blocked = capture_renderable_surface_snapshot(&commands);
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0].buffer_id, first_snapshot[0].buffer_id);

    acquire_timeline.signal_point(3).unwrap();
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let published = capture_renderable_surface_snapshot(&commands);
    assert_eq!(published.len(), 1);
    assert_ne!(published[0].buffer_id, first_snapshot[0].buffer_id);

    replacement.destroy();
    layer_surface.destroy();
    surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn superseded_unready_explicit_sync_commit_discards_presentation_feedback() {
    let Some(first_acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(second_acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let presentation: client_wp_presentation::WpPresentation =
        globals.bind(&qh, 1..=2, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-explicit-sync-feedback",
    );
    layer_surface.set_size(2, 2);
    let sync_surface = syncobj.get_surface(&surface, &qh, ());
    let first_acquire_fd = first_acquire_timeline.export_timeline_fd().unwrap();
    let second_acquire_fd = second_acquire_timeline.export_timeline_fd().unwrap();
    let release_fd = release_timeline.export_timeline_fd().unwrap();
    let first_acquire = syncobj.import_timeline(first_acquire_fd.as_fd(), &qh, ());
    let second_acquire = syncobj.import_timeline(second_acquire_fd.as_fd(), &qh, ());
    let release = syncobj.import_timeline(release_fd.as_fd(), &qh, ());
    let first_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff44_4444).unwrap();
    let second_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff55_5555).unwrap();
    surface.commit();
    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    let feedback = presentation.feedback(&surface, &qh, ());
    sync_surface.set_acquire_point(&first_acquire, 0, 1);
    sync_surface.set_release_point(&release, 0, 2);
    surface.attach(Some(&first_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    wait_for_server_commands(&commands);
    assert_eq!(capture_renderable_surface_count(&commands), 0);

    second_acquire_timeline.signal_point(1).unwrap();
    sync_surface.set_acquire_point(&second_acquire, 0, 1);
    sync_surface.set_release_point(&release, 0, 3);
    surface.attach(Some(&second_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commands.send(ServerCommand::PresentFrame).unwrap();
    queue.roundtrip(&mut state).unwrap();

    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.presentation_presented_count, 0);
    assert_eq!(state.presentation_discarded_count, 1);
    assert_eq!(
        state.presentation_feedback_event_log,
        vec![(feedback.id().protocol_id(), "discarded")]
    );
}

#[test]
fn synchronized_child_replacement_preserves_the_latched_surface_tree() {
    let Some(acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let mut state = RegistryTestState::default();
    let (parent, _layer_surface) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "latched-tree-replacement",
        64,
        32,
    );
    let child_a = compositor.create_surface(&qh, ());
    let child_b = compositor.create_surface(&qh, ());
    let _subsurface_a = subcompositor.get_subsurface(&child_a, &parent, &qh, ());
    let _subsurface_b = subcompositor.get_subsurface(&child_b, &parent, &qh, ());
    let sync_surface_a = syncobj.get_surface(&child_a, &qh, ());
    let acquire_fd = acquire_timeline.export_timeline_fd().unwrap();
    let acquire = syncobj.import_timeline(acquire_fd.as_fd(), &qh, ());
    let release_fd = release_timeline.export_timeline_fd().unwrap();
    let release = syncobj.import_timeline(release_fd.as_fd(), &qh, ());
    let buffer_a1 = create_test_dmabuf_buffer_with_size(&dmabuf, &qh, 0xff11_1111, 2, 2).unwrap();
    let buffer_a2 = create_test_dmabuf_buffer_with_size(&dmabuf, &qh, 0xff22_2222, 3, 3).unwrap();
    let _buffer_b1 = attach_test_buffered_surface(&child_b, &shm, &qh, 4, 4).unwrap();

    sync_surface_a.set_acquire_point(&acquire, 0, 1);
    sync_surface_a.set_release_point(&release, 0, 2);
    child_a.attach(Some(&buffer_a1), 0, 0);
    child_a.damage_buffer(0, 0, 2, 2);
    child_a.commit();
    child_b.commit();
    let callback = parent.frame(&qh, ());
    state.tracked_frame_callback_id = Some(callback.id().protocol_id());
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let latched_tree = capture_pending_surface_tree_transactions(&commands);
    assert_eq!(latched_tree.len(), 1);
    assert_eq!(latched_tree[0].1.len(), 3);
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    sync_surface_a.set_acquire_point(&acquire, 0, 3);
    sync_surface_a.set_release_point(&release, 0, 4);
    child_a.attach(Some(&buffer_a2), 0, 0);
    child_a.damage_buffer(0, 0, 3, 3);
    child_a.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(
        capture_pending_surface_tree_transactions(&commands),
        latched_tree
    );
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    acquire_timeline.signal_point(1).unwrap();
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let first_publication = capture_renderable_surface_snapshot(&commands);
    assert_eq!(
        state
            .frame_completion_event_log
            .iter()
            .filter(|event| **event == "frame_callback")
            .count(),
        1
    );
    assert!(
        first_publication
            .iter()
            .any(|surface| surface.width == 2 && surface.height == 2)
    );
    assert!(
        first_publication
            .iter()
            .any(|surface| surface.width == 4 && surface.height == 4)
    );
    assert!(
        !first_publication
            .iter()
            .any(|surface| surface.width == 3 && surface.height == 3)
    );

    acquire_timeline.signal_point(3).unwrap();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let second_publication = capture_renderable_surface_snapshot(&commands);
    assert!(
        second_publication
            .iter()
            .any(|surface| surface.width == 3 && surface.height == 3)
    );
    assert!(
        second_publication
            .iter()
            .any(|surface| surface.width == 4 && surface.height == 4)
    );

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn synchronized_subsurface_explicit_sync_survives_bufferless_commit() {
    let Some(acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let mut state = RegistryTestState::default();
    let (parent, _layer_surface) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-subsurface-explicit-sync",
        64,
        32,
    );
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_sync();
    let sync_surface = syncobj.get_surface(&child, &qh, ());
    let acquire_fd = acquire_timeline.export_timeline_fd().unwrap();
    let release_fd = release_timeline.export_timeline_fd().unwrap();
    let acquire = syncobj.import_timeline(acquire_fd.as_fd(), &qh, ());
    let release = syncobj.import_timeline(release_fd.as_fd(), &qh, ());
    let buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff44_4444).unwrap();

    sync_surface.set_acquire_point(&acquire, 0, 1);
    sync_surface.set_release_point(&release, 0, 2);
    child.attach(Some(&buffer), 0, 0);
    child.damage_buffer(0, 0, 2, 2);
    child.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    child.commit();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    acquire_timeline.signal_point(1).unwrap();
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(surfaces.len(), 2);
    assert_ne!(surfaces[1].buffer_id, 0);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn synchronized_subsurface_explicit_sync_survives_sync_surface_proxy_destruction() {
    let Some(acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let mut state = RegistryTestState::default();
    let (parent, _layer_surface) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-subsurface-sync-surface-destroy",
        64,
        32,
    );
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_sync();
    let sync_surface = syncobj.get_surface(&child, &qh, ());
    let acquire_fd = acquire_timeline.export_timeline_fd().unwrap();
    let release_fd = release_timeline.export_timeline_fd().unwrap();
    let acquire = syncobj.import_timeline(acquire_fd.as_fd(), &qh, ());
    let release = syncobj.import_timeline(release_fd.as_fd(), &qh, ());
    let buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff44_4444).unwrap();

    sync_surface.set_acquire_point(&acquire, 0, 1);
    sync_surface.set_release_point(&release, 0, 2);
    child.attach(Some(&buffer), 0, 0);
    child.damage_buffer(0, 0, 2, 2);
    child.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    child.commit();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    sync_surface.destroy();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    acquire_timeline.signal_point(1).unwrap();
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(surfaces.len(), 2);
    assert_ne!(surfaces[1].buffer_id, 0);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn synchronized_subsurface_bufferless_explicit_sync_points_are_rejected_at_commit() {
    let Some(acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (running, server_thread) = spawn_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let mut state = RegistryTestState::default();
    let (parent, _layer_surface) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-subsurface-invalid-explicit-sync",
        64,
        32,
    );
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    let sync_surface = syncobj.get_surface(&child, &qh, ());
    let acquire_fd = acquire_timeline.export_timeline_fd().unwrap();
    let release_fd = release_timeline.export_timeline_fd().unwrap();
    let acquire = syncobj.import_timeline(acquire_fd.as_fd(), &qh, ());
    let release = syncobj.import_timeline(release_fd.as_fd(), &qh, ());
    let buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff55_5555).unwrap();
    acquire_timeline.signal_point(1).unwrap();
    sync_surface.set_acquire_point(&acquire, 0, 1);
    sync_surface.set_release_point(&release, 0, 2);
    child.attach(Some(&buffer), 0, 0);
    child.damage_buffer(0, 0, 2, 2);
    child.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    sync_surface.set_acquire_point(&acquire, 0, 3);
    sync_surface.set_release_point(&release, 0, 4);
    child.attach(None, 0, 0);
    child.commit();
    connection.flush().unwrap();

    assert!(queue.roundtrip(&mut state).is_err());
    stop_test_server(running, server_thread);
}

#[test]
fn delayed_bufferless_publication_does_not_consume_future_explicit_sync_points() {
    let Some(acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let mut state = RegistryTestState::default();
    let (parent, _layer_surface) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-subsurface-future-explicit-sync",
        64,
        32,
    );
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    let sync_surface = syncobj.get_surface(&child, &qh, ());
    let acquire_fd = acquire_timeline.export_timeline_fd().unwrap();
    let release_fd = release_timeline.export_timeline_fd().unwrap();
    let acquire = syncobj.import_timeline(acquire_fd.as_fd(), &qh, ());
    let release = syncobj.import_timeline(release_fd.as_fd(), &qh, ());
    let buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff66_6666).unwrap();

    child.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    acquire_timeline.signal_point(1).unwrap();
    sync_surface.set_acquire_point(&acquire, 0, 1);
    sync_surface.set_release_point(&release, 0, 2);
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    child.attach(Some(&buffer), 0, 0);
    child.damage_buffer(0, 0, 2, 2);
    child.commit();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(surfaces.len(), 2);
    assert_ne!(surfaces[1].buffer_id, 0);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn synchronized_subsurface_bufferless_commits_preserve_then_null_clears_attachment_sync() {
    let Some(acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let mut state = RegistryTestState::default();
    let (parent, _layer_surface) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-subsurface-bufferless-chain",
        64,
        32,
    );
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    let sync_surface = syncobj.get_surface(&child, &qh, ());
    let acquire_fd = acquire_timeline.export_timeline_fd().unwrap();
    let release_fd = release_timeline.export_timeline_fd().unwrap();
    let acquire = syncobj.import_timeline(acquire_fd.as_fd(), &qh, ());
    let release = syncobj.import_timeline(release_fd.as_fd(), &qh, ());
    let buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff77_7777).unwrap();

    acquire_timeline.signal_point(1).unwrap();
    sync_surface.set_acquire_point(&acquire, 0, 1);
    sync_surface.set_release_point(&release, 0, 2);
    child.attach(Some(&buffer), 0, 0);
    child.damage_buffer(0, 0, 2, 2);
    child.commit();
    child.commit();
    child.commit();
    child.commit();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let retained = capture_renderable_surface_snapshot(&commands);
    assert_eq!(retained.len(), 2);
    assert_ne!(retained[1].buffer_id, 0);

    child.attach(None, 0, 0);
    child.commit();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn synchronized_subsurface_attachment_replacement_uses_new_explicit_sync_state() {
    let Some(acquire_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let Some(release_timeline) =
        test_syncobj_device().and_then(|device| device.create_timeline_for_tests().ok())
    else {
        return;
    };
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
        globals.bind(&qh, 3..=3, ()).unwrap();
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let mut state = RegistryTestState::default();
    let (parent, _layer_surface) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-subsurface-replacement-sync",
        64,
        32,
    );
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    let sync_surface = syncobj.get_surface(&child, &qh, ());
    let acquire_fd = acquire_timeline.export_timeline_fd().unwrap();
    let release_fd = release_timeline.export_timeline_fd().unwrap();
    let acquire = syncobj.import_timeline(acquire_fd.as_fd(), &qh, ());
    let release = syncobj.import_timeline(release_fd.as_fd(), &qh, ());
    let first_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff88_8888).unwrap();
    let second_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff99_9999).unwrap();

    acquire_timeline.signal_point(1).unwrap();
    sync_surface.set_acquire_point(&acquire, 0, 1);
    sync_surface.set_release_point(&release, 0, 2);
    child.attach(Some(&first_buffer), 0, 0);
    child.damage_buffer(0, 0, 2, 2);
    child.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    sync_surface.set_acquire_point(&acquire, 0, 3);
    sync_surface.set_release_point(&release, 0, 4);
    child.attach(Some(&second_buffer), 0, 0);
    child.damage_buffer(0, 0, 2, 2);
    child.commit();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_renderable_surface_count(&commands), 1);

    acquire_timeline.signal_point(3).unwrap();
    commands.send(ServerCommand::PresentFrame).unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(surfaces.len(), 2);
    assert_ne!(surfaces[1].buffer_id, 0);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn layer_root_publishes_synchronized_subsurface_transaction() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let mut state = RegistryTestState::default();
    let (parent, _layer_surface) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-subsurface",
        64,
        32,
    );
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_position(10, 12);
    commit_test_buffered_surface(&child, &shm, &qh, 16, 8).unwrap();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let surfaces = capture_renderable_surface_snapshot(&commands);
    assert_eq!(surfaces.len(), 2);
    assert_eq!(surfaces[1].parent_surface_id, Some(surfaces[0].surface_id));
    assert_eq!(surfaces[1].local_x, 10);
    assert_eq!(surfaces[1].local_y, 12);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

#[test]
fn synchronized_subsurface_bufferless_commit_discards_older_feedback() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let stream = UnixStream::connect(&socket_path).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let presentation: client_wp_presentation::WpPresentation =
        globals.bind(&qh, 1..=2, ()).unwrap();
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 =
        globals.bind(&qh, 4..=4, ()).unwrap();
    let mut state = RegistryTestState::default();
    let (parent, _layer_surface) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-subsurface-feedback",
        64,
        32,
    );
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    let feedback = presentation.feedback(&child, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 16, 8).unwrap();
    child.commit();
    parent.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    commands.send(ServerCommand::PresentFrame).unwrap();
    queue.roundtrip(&mut state).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(state.presentation_presented_count, 0);
    assert_eq!(state.presentation_discarded_count, 1);
    assert_eq!(
        state.presentation_feedback_event_log,
        vec![(feedback.id().protocol_id(), "discarded")]
    );
}

#[test]
fn arrangement_change_advances_render_generation_without_buffer_special_case() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();
    let (surface, layer_surface) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "layer-arrangement-damage",
        200,
        32,
    );
    let before = capture_render_generation(&commands);
    let before_y = capture_renderable_surface_snapshot(&commands)[0].local_y;

    layer_surface.set_anchor(client_zwlr_layer_surface_v1::Anchor::Top);
    layer_surface.set_margin(12, 0, 0, 0);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let after = capture_render_generation(&commands);
    let after_y = capture_renderable_surface_snapshot(&commands)[0].local_y;
    assert!(after > before);
    assert_ne!(before_y, after_y);

    commands.send(ServerCommand::Stop).unwrap();
    let _server = server_thread.join().unwrap();
}

fn map_exclusive_overlay_pair(
    connection: &Connection,
    queue: &mut EventQueue<RegistryTestState>,
    state: &mut RegistryTestState,
    compositor: &client_wl_compositor::WlCompositor,
    shm: &client_wl_shm::WlShm,
    layer_shell: &client_zwlr_layer_shell_v1::ZwlrLayerShellV1,
    qh: &QueueHandle<RegistryTestState>,
) -> (client_wl_surface::WlSurface, client_wl_surface::WlSurface) {
    let mut map = |namespace: &str| {
        let (surface, layer_surface) = create_layer_surface(
            compositor,
            layer_shell,
            qh,
            client_zwlr_layer_shell_v1::Layer::Overlay,
            namespace,
        );
        layer_surface.set_size(64, 64);
        layer_surface.set_keyboard_interactivity(
            client_zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
        );
        surface.commit();
        connection.flush().unwrap();
        queue.roundtrip(state).unwrap();
        commit_test_buffered_surface(&surface, shm, qh, 64, 64).unwrap();
        connection.flush().unwrap();
        queue.roundtrip(state).unwrap();
        surface
    };
    (map("f10-exclusive-a"), map("f10-exclusive-b"))
}

#[test]
fn repainting_older_exclusive_overlay_does_not_steal_focus() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();
    let (older, _newer) = map_exclusive_overlay_pair(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
    );
    let snapshots = capture_renderable_surface_snapshot(&commands);
    let older_id = snapshots[0].surface_id;
    let newer_id = snapshots[1].surface_id;
    assert_eq!(capture_focused_surface_id(&commands), Some(newer_id));

    commit_test_buffered_surface(&older, &shm, &qh, 64, 64).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(capture_focused_surface_id(&commands), Some(newer_id));
    assert_eq!(
        capture_renderable_surface_snapshot(&commands)
            .into_iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>(),
        vec![older_id, newer_id]
    );

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn repainting_older_overlay_does_not_change_same_layer_render_order() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();
    let (older, _newer) = map_exclusive_overlay_pair(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
    );
    let before = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();

    commit_test_buffered_surface(&older, &shm, &qh, 64, 64).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let after = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    assert_eq!(after, before);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn geometry_only_layer_commit_preserves_lifecycle_order() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();
    let (surface, layer_surface) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "f10-geometry-order",
    );
    layer_surface.set_anchor(client_zwlr_layer_surface_v1::Anchor::Top);
    layer_surface.set_size(1280, 32);
    layer_surface.set_exclusive_zone(32);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&surface, &shm, &qh, 1280, 32).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let surface_id = surface.id().protocol_id();
    let before = capture_layer_surface_lifecycle_state(&commands, surface_id).unwrap();
    layer_surface.set_exclusive_zone(48);
    surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let after = capture_layer_surface_lifecycle_state(&commands, surface_id).unwrap();

    assert_eq!(after.order, before.order);
    assert_eq!(capture_usable_output_geometry(&commands).y, 48.0);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn content_only_layer_repaints_do_not_run_layer_maintenance() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();
    let (surface, _layer_surface) = create_mapped_layer_surface(
        &connection,
        &mut queue,
        &mut state,
        &compositor,
        &shm,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "f10-maintenance-counter",
        64,
        64,
    );
    let before = capture_core_compliance_metrics(&commands);

    for _ in 0..1_000 {
        commit_test_buffered_surface(&surface, &shm, &qh, 64, 64).unwrap();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
    }

    let after = capture_core_compliance_metrics(&commands);
    assert_eq!(
        after.layer_surface_arrangement_passes - before.layer_surface_arrangement_passes,
        0
    );
    assert_eq!(
        after.layer_surface_stack_reorder_passes - before.layer_surface_stack_reorder_passes,
        0
    );
    assert_eq!(
        after.layer_surface_keyboard_focus_recomputations
            - before.layer_surface_keyboard_focus_recomputations,
        0
    );

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn first_map_and_remap_receive_fresh_lifecycle_orders() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();

    let (first_surface, first_layer) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "f10-first-map",
    );
    first_layer.set_size(64, 64);
    first_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&first_surface, &shm, &qh, 64, 64).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let first_id = first_surface.id().protocol_id();
    let first_order = capture_layer_surface_lifecycle_state(&commands, first_id)
        .unwrap()
        .order;

    let (second_surface, second_layer) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "f10-second-map",
    );
    second_layer.set_size(64, 64);
    second_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&second_surface, &shm, &qh, 64, 64).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let second_id = second_surface.id().protocol_id();
    let second_order = capture_layer_surface_lifecycle_state(&commands, second_id)
        .unwrap()
        .order;
    assert!(second_order > first_order);

    first_surface.attach(None, 0, 0);
    first_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert!(
        !capture_layer_surface_lifecycle_state(&commands, first_id)
            .unwrap()
            .mapped
    );

    first_surface.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&first_surface, &shm, &qh, 64, 64).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let remapped_order = capture_layer_surface_lifecycle_state(&commands, first_id)
        .unwrap()
        .order;
    assert!(remapped_order > second_order);

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn repaint_preserves_same_layer_exclusive_reservation_order() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();

    let mut map_reserved = |namespace: &str, zone: i32| {
        let (surface, layer_surface) = create_layer_surface(
            &compositor,
            &layer_shell,
            &qh,
            client_zwlr_layer_shell_v1::Layer::Top,
            namespace,
        );
        layer_surface.set_anchor(
            client_zwlr_layer_surface_v1::Anchor::Top
                | client_zwlr_layer_surface_v1::Anchor::Left
                | client_zwlr_layer_surface_v1::Anchor::Right,
        );
        layer_surface.set_size(0, 24);
        layer_surface.set_exclusive_zone(zone);
        surface.commit();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        commit_test_buffered_surface(&surface, &shm, &qh, 1280, 24).unwrap();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        surface
    };
    let older = map_reserved("f10-reservation-older", 32);
    let newer = map_reserved("f10-reservation-newer", 48);
    let older_id = older.id().protocol_id();
    let newer_id = newer.id().protocol_id();
    let before_orders = (
        capture_layer_surface_lifecycle_state(&commands, older_id)
            .unwrap()
            .order,
        capture_layer_surface_lifecycle_state(&commands, newer_id)
            .unwrap()
            .order,
    );
    let before_geometry = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .map(|surface| {
            (
                surface.surface_id,
                surface.local_y,
                surface.width,
                surface.height,
            )
        })
        .collect::<Vec<_>>();
    let before_usable = capture_usable_output_geometry(&commands);

    for _ in 0..3 {
        commit_test_buffered_surface(&older, &shm, &qh, 1280, 24).unwrap();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
    }

    let after_orders = (
        capture_layer_surface_lifecycle_state(&commands, older_id)
            .unwrap()
            .order,
        capture_layer_surface_lifecycle_state(&commands, newer_id)
            .unwrap()
            .order,
    );
    let after_geometry = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .map(|surface| {
            (
                surface.surface_id,
                surface.local_y,
                surface.width,
                surface.height,
            )
        })
        .collect::<Vec<_>>();
    let after_usable = capture_usable_output_geometry(&commands);
    assert_eq!(after_orders, before_orders);
    assert_eq!(after_geometry, before_geometry);
    assert_eq!(
        (
            after_usable.x,
            after_usable.y,
            after_usable.width,
            after_usable.height
        ),
        (
            before_usable.x,
            before_usable.y,
            before_usable.width,
            before_usable.height
        )
    );

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn set_layer_changes_band_without_refreshing_lifecycle_order() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();
    let (older, older_layer) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "f10-set-layer-older",
    );
    older_layer.set_size(64, 64);
    older.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&older, &shm, &qh, 64, 64).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let (newer, newer_layer) = create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        client_zwlr_layer_shell_v1::Layer::Top,
        "f10-set-layer-newer",
    );
    newer_layer.set_size(64, 64);
    newer.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    commit_test_buffered_surface(&newer, &shm, &qh, 64, 64).unwrap();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();

    let older_id = older.id().protocol_id();
    let before_stack = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    let before_order = capture_layer_surface_lifecycle_state(&commands, older_id)
        .unwrap()
        .order;
    let before_metrics = capture_core_compliance_metrics(&commands);
    older_layer.set_layer(client_zwlr_layer_shell_v1::Layer::Overlay);
    older.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let after = capture_layer_surface_lifecycle_state(&commands, older_id).unwrap();
    let after_metrics = capture_core_compliance_metrics(&commands);
    assert_eq!(after.order, before_order);
    assert_eq!(after.layer_rank, 4);
    assert_eq!(
        capture_renderable_surface_snapshot(&commands)
            .into_iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>(),
        vec![before_stack[1], before_stack[0]]
    );
    assert_eq!(
        after_metrics.layer_surface_arrangement_passes
            - before_metrics.layer_surface_arrangement_passes,
        1
    );
    assert_eq!(
        after_metrics.layer_surface_stack_reorder_passes
            - before_metrics.layer_surface_stack_reorder_passes,
        1
    );

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn keyboard_policy_rearbitrates_without_restaking() {
    let socket_name = unique_socket_name();
    let socket_path = runtime_socket_path(&socket_name);
    let server = OwnCompositorServer::bind_cpu_composition(socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let (connection, mut queue, qh, compositor, shm, layer_shell) =
        connect_layer_client(&socket_path);
    let mut state = RegistryTestState::default();
    let mut map = |namespace: &str, interactivity| {
        let (surface, layer_surface) = create_layer_surface(
            &compositor,
            &layer_shell,
            &qh,
            client_zwlr_layer_shell_v1::Layer::Overlay,
            namespace,
        );
        layer_surface.set_size(64, 64);
        layer_surface.set_keyboard_interactivity(interactivity);
        surface.commit();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        commit_test_buffered_surface(&surface, &shm, &qh, 64, 64).unwrap();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        (surface, layer_surface)
    };
    let (older, older_layer) = map(
        "f10-keyboard-older",
        client_zwlr_layer_surface_v1::KeyboardInteractivity::None,
    );
    let (newer, newer_layer) = map(
        "f10-keyboard-newer",
        client_zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
    );
    let older_id = older.id().protocol_id();
    let newer_id = newer.id().protocol_id();
    let before_stack = capture_renderable_surface_snapshot(&commands)
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    let older_scene_id = before_stack[0];
    let newer_scene_id = before_stack[1];
    let before_orders = (
        capture_layer_surface_lifecycle_state(&commands, older_id)
            .unwrap()
            .order,
        capture_layer_surface_lifecycle_state(&commands, newer_id)
            .unwrap()
            .order,
    );
    assert_eq!(capture_focused_surface_id(&commands), Some(newer_scene_id));

    let before_metrics = capture_core_compliance_metrics(&commands);
    older_layer
        .set_keyboard_interactivity(client_zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);
    older.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    let after_metrics = capture_core_compliance_metrics(&commands);
    assert_eq!(capture_focused_surface_id(&commands), Some(newer_scene_id));
    assert_eq!(
        capture_renderable_surface_snapshot(&commands)
            .into_iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>(),
        before_stack
    );
    assert_eq!(
        (
            capture_layer_surface_lifecycle_state(&commands, older_id)
                .unwrap()
                .order,
            capture_layer_surface_lifecycle_state(&commands, newer_id)
                .unwrap()
                .order,
        ),
        before_orders
    );
    assert_eq!(
        after_metrics.layer_surface_arrangement_passes
            - before_metrics.layer_surface_arrangement_passes,
        0
    );
    assert_eq!(
        after_metrics.layer_surface_stack_reorder_passes
            - before_metrics.layer_surface_stack_reorder_passes,
        0
    );
    assert_eq!(
        after_metrics.layer_surface_keyboard_focus_recomputations
            - before_metrics.layer_surface_keyboard_focus_recomputations,
        1
    );

    newer_layer
        .set_keyboard_interactivity(client_zwlr_layer_surface_v1::KeyboardInteractivity::None);
    newer.commit();
    connection.flush().unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(capture_focused_surface_id(&commands), Some(older_scene_id));
    assert_eq!(
        capture_renderable_surface_snapshot(&commands)
            .into_iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>(),
        before_stack
    );

    let _server = stop_controllable_test_server(commands, server_thread);
}
