fn create_min_size_toplevel_then_shrink_resize_before_client_commit(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
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

fn create_scaled_buffer_toplevel_then_right_edge_shrink_and_commit(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.set_buffer_scale(2);
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

fn create_scaled_buffer_toplevel_then_left_edge_shrink_and_commit(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.set_buffer_scale(2);
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

fn create_toplevel_then_map_subsurface_before_button_release(
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

fn create_overlapping_subsurfaces_then_place_above_after_parent_commit(
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

fn create_subsurface_below_parent_and_click_overlap(
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

fn create_subsurface_with_invalid_restack_reference(
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

fn create_repeated_restack_then_destroy_subsurface(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<
    (Vec<RenderableSurfaceSnapshot>, Vec<RenderableSurfaceSnapshot>),
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

fn create_pointer_enter_with_v5_pointer(
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

fn create_decoy_keyboard_then_focused_toplevel_and_receive_key(
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

fn create_surface_with_frame_callback(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let surface = compositor.create_surface(&qh, ());
    let _callback = surface.frame(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_surface_with_buffer_frame_callback(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_surface_with_delayed_buffer_frame_callback(socket_path, commands, Duration::ZERO)
}

fn create_surface_with_unpresented_buffer_frame_callback(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let _callback = surface.frame(&qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    Ok(())
}

fn create_visible_surface_frame_callback_without_commit_and_present(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;

    let _callback = surface.frame(&qh, ());
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    assert!(!state.frame_done);

    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_visible_surface_frame_callback_without_commit_and_capture_protocol_only(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let _callback = surface.frame(&qh, ());
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    assert!(!state.frame_done);

    Ok(capture_only_pending_surface_frame_callbacks(commands))
}

fn create_surface_with_delayed_buffer_frame_callback(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    before_present_delay: Duration,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let _callback = surface.frame(&qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    assert!(!state.frame_done);

    thread::sleep(before_present_delay);
    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_surface_with_buffer_release(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    assert_eq!(state.buffer_release_count, 0);

    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_dmabuf_surface_then_replace_buffer(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_dmabuf_surface_then_replace_buffer_inner(socket_path, commands, false)
}

fn create_dmabuf_surface_then_replace_buffer_and_present_twice(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_dmabuf_surface_then_replace_buffer_inner(socket_path, commands, true)
}

fn create_dmabuf_surface_then_replace_buffer_inner(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    extra_present: bool,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;
    let first_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff11_1111)?;
    let second_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff22_2222)?;

    let surface = compositor.create_surface(&qh, ());
    surface.attach(Some(&first_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    assert_eq!(state.buffer_release_count, 0);

    surface.attach(Some(&second_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    assert_eq!(state.buffer_release_count, 0);
    if extra_present {
        commands.send(ServerCommand::PresentFrame)?;
        queue.roundtrip(&mut state)?;
    }
    Ok(state)
}

fn create_syncobj_dmabuf_surface_and_present(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    acquire_timeline: &DrmSyncobjTimeline,
    release_timeline: &DrmSyncobjTimeline,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let sync_surface = syncobj.get_surface(&surface, &qh, ());
    let acquire_timeline_fd = acquire_timeline.export_timeline_fd()?;
    let release_timeline_fd = release_timeline.export_timeline_fd()?;
    let sync_acquire_timeline = syncobj.import_timeline(acquire_timeline_fd.as_fd(), &qh, ());
    let sync_release_timeline = syncobj.import_timeline(release_timeline_fd.as_fd(), &qh, ());
    let first_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff44_4444)?;
    let second_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff55_5555)?;

    acquire_timeline.signal_point(1)?;
    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 1);
    sync_surface.set_release_point(&sync_release_timeline, 0, 2);
    surface.attach(Some(&first_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    assert!(!release_timeline.point_signaled(2)?);

    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;

    acquire_timeline.signal_point(3)?;
    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 3);
    sync_surface.set_release_point(&sync_release_timeline, 0, 4);
    surface.attach(Some(&second_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    assert!(!release_timeline.point_signaled(2)?);

    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_test_dmabuf_buffer(
    dmabuf: &client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
    qh: &QueueHandle<RegistryTestState>,
    pixel: u32,
) -> Result<client_wl_buffer::WlBuffer, Box<dyn std::error::Error>> {
    let file = create_test_shm_file(&[pixel, pixel, pixel, pixel])?;
    let params = dmabuf.create_params(qh, ());
    params.add(file.as_fd(), 0, 0, 8, 0, 0);
    Ok(params.create_immed(
        2,
        2,
        DRM_FORMAT_ARGB8888,
        client_zwp_linux_buffer_params_v1::Flags::empty(),
        qh,
        (),
    ))
}

fn test_syncobj_device() -> Option<DrmSyncobjDevice> {
    DrmSyncobjDevice::open_available()
}

fn create_client_toplevel_with_shm_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());

    toplevel.set_app_id("oblivion.buffer-test".to_string());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_client_toplevel_with_shm_damage_only_update(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let mut file = create_test_shm_file(&[0xff11_1111, 0xff22_2222, 0xff33_3333, 0xff44_4444])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());

    toplevel.set_app_id("oblivion.damage-only-test".to_string());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;

    file.seek(SeekFrom::Start(0))?;
    for pixel in [0xffaa_0000_u32, 0xff00_aa00, 0xff00_00aa, 0xffaa_aa00] {
        file.write_all(&pixel.to_ne_bytes())?;
    }
    file.flush()?;
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_client_toplevel_with_dmabuf_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let params = dmabuf.create_params(&qh, ());
    params.add(file.as_fd(), 0, 0, 8, 0, 0);
    let buffer = params.create_immed(
        2,
        2,
        DRM_FORMAT_ARGB8888,
        client_zwp_linux_buffer_params_v1::Flags::empty(),
        &qh,
        (),
    );
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());

    toplevel.set_app_id("oblivion.dmabuf-test".to_string());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_toplevel_with_resized_shm_pool_buffer(
    socket_path: &PathBuf,
    resized_pool_size: i32,
    buffer_offset: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[
        0xffff_0000,
        0xff00_ff00,
        0xff00_00ff,
        0xffff_ffff,
        0xff55_0000,
        0xff00_5500,
        0xff00_0055,
        0xff55_5555,
    ])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    pool.resize(resized_pool_size);
    let buffer = pool.create_buffer(
        buffer_offset,
        2,
        2,
        8,
        client_wl_shm::Format::Argb8888,
        &qh,
        (),
    );
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());

    toplevel.set_app_id("oblivion.shm-resize-test".to_string());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn resize_shm_pool_to_invalid_size(
    socket_path: &PathBuf,
    resized_pool_size: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    pool.resize(resized_pool_size);
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_shm_pool_with_invalid_size(
    socket_path: &PathBuf,
    pool_size: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000])?;
    let _pool = shm.create_pool(file.as_fd(), pool_size, &qh, ());
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_client_toplevel_with_shm_then_dmabuf_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;

    let shm_file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(shm_file.as_fd(), 16, &qh, ());
    let shm_buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());

    let dmabuf_file = create_test_shm_file(&[0xff11_1111, 0xff22_2222, 0xff33_3333, 0xff44_4444])?;
    let params = dmabuf.create_params(&qh, ());
    params.add(dmabuf_file.as_fd(), 0, 0, 8, 0, 0);
    let dmabuf_buffer = params.create_immed(
        2,
        2,
        DRM_FORMAT_ARGB8888,
        client_zwp_linux_buffer_params_v1::Flags::empty(),
        &qh,
        (),
    );

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_app_id("oblivion.dmabuf-switch-test".to_string());

    surface.attach(Some(&shm_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;

    surface.attach(Some(&dmabuf_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_client_toplevel_with_sized_shm_buffer(
    socket_path: &PathBuf,
    width: usize,
    height: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    create_client_toplevel_with_app_id_and_sized_shm_buffer(
        socket_path,
        "oblivion.buffer-test",
        width,
        height,
    )
}

fn create_client_toplevel_with_app_id_and_sized_shm_buffer(
    socket_path: &PathBuf,
    app_id: &str,
    width: usize,
    height: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, width, height)?;

    toplevel.set_app_id(app_id.to_string());
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_two_live_client_toplevels_and_capture_surface_count(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<usize, Box<dyn std::error::Error>> {
    let first = LiveTestClient::connect(socket_path)?;
    first.create_toplevel("oblivion.client-one", 2, 2)?;
    let second = LiveTestClient::connect(socket_path)?;
    second.create_toplevel("oblivion.client-two", 3, 2)?;
    wait_for_server_commands(commands);
    let count = capture_renderable_surface_count(commands);
    drop((first, second));
    Ok(count)
}

struct LiveTestClient {
    connection: Connection,
    queue: wayland_client::EventQueue<RegistryTestState>,
    compositor: client_wl_compositor::WlCompositor,
    wm_base: client_xdg_wm_base::XdgWmBase,
    shm: client_wl_shm::WlShm,
}

impl LiveTestClient {
    fn connect(socket_path: &PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let stream = UnixStream::connect(socket_path)?;
        let connection = Connection::from_socket(stream)?;
        let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
        Ok(Self {
            connection,
            queue,
            compositor,
            wm_base,
            shm,
        })
    }

    fn create_toplevel(
        &self,
        app_id: &str,
        width: usize,
        height: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let _surface = self.create_toplevel_surface(app_id, width, height)?;
        Ok(())
    }

    fn create_toplevel_surface(
        &self,
        app_id: &str,
        width: usize,
        height: usize,
    ) -> Result<client_wl_surface::WlSurface, Box<dyn std::error::Error>> {
        let qh = self.queue.handle();
        let (surface, _xdg_surface, toplevel) = create_test_buffered_toplevel(
            &self.compositor,
            &self.wm_base,
            &self.shm,
            &qh,
            width,
            height,
        )?;
        toplevel.set_app_id(app_id.to_string());
        surface.commit();
        self.connection.flush()?;
        self.connection.roundtrip()?;
        Ok(surface)
    }

    fn commit_surface(
        &self,
        surface: &client_wl_surface::WlSurface,
        width: usize,
        height: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let qh = self.queue.handle();
        commit_test_buffered_surface(surface, &self.shm, &qh, width, height)?;
        self.connection.flush()?;
        self.connection.roundtrip()?;
        Ok(())
    }
}

fn create_test_buffered_toplevel(
    compositor: &client_wl_compositor::WlCompositor,
    wm_base: &client_xdg_wm_base::XdgWmBase,
    shm: &client_wl_shm::WlShm,
    qh: &QueueHandle<RegistryTestState>,
    width: usize,
    height: usize,
) -> Result<
    (
        client_wl_surface::WlSurface,
        client_xdg_surface::XdgSurface,
        client_xdg_toplevel::XdgToplevel,
    ),
    Box<dyn std::error::Error>,
> {
    let surface = compositor.create_surface(qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, qh, ());
    let toplevel = xdg_surface.get_toplevel(qh, ());
    attach_test_buffered_surface(&surface, shm, qh, width, height)?;
    Ok((surface, xdg_surface, toplevel))
}

fn commit_test_buffered_surface(
    surface: &client_wl_surface::WlSurface,
    shm: &client_wl_shm::WlShm,
    qh: &QueueHandle<RegistryTestState>,
    width: usize,
    height: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    attach_test_buffered_surface(surface, shm, qh, width, height)?;
    surface.commit();
    Ok(())
}

fn attach_test_buffered_surface(
    surface: &client_wl_surface::WlSurface,
    shm: &client_wl_shm::WlShm,
    qh: &QueueHandle<RegistryTestState>,
    width: usize,
    height: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let pixels = vec![0xff20_3040; width * height];
    let file = create_test_shm_file(&pixels)?;
    let pool = shm.create_pool(file.as_fd(), (pixels.len() * 4) as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        (width * 4) as i32,
        client_wl_shm::Format::Argb8888,
        qh,
        (),
    );
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, width as i32, height as i32);
    Ok(())
}

fn create_client_toplevel_with_positioned_subsurface_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent_file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let parent_pool = shm.create_pool(parent_file.as_fd(), 16, &qh, ());
    let parent_buffer =
        parent_pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());

    let child_file = create_test_shm_file(&[0xffff_ffff])?;
    let child_pool = shm.create_pool(child_file.as_fd(), 4, &qh, ());
    let child_buffer =
        child_pool.create_buffer(0, 1, 1, 4, client_wl_shm::Format::Argb8888, &qh, ());

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());

    subsurface.set_position(10, 12);
    parent.attach(Some(&parent_buffer), 0, 0);
    parent.damage_buffer(0, 0, 2, 2);
    child.attach(Some(&child_buffer), 0, 0);
    child.damage_buffer(0, 0, 1, 1);
    child.commit();
    parent.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_subsurface_buffer_before_parent_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());

    subsurface.set_position(0, 0);
    commit_test_buffered_surface(&child, &shm, &qh, 1, 1)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 2, 2)?;
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn capture_default_synchronized_child_before_and_after_parent_commit(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<SynchronizedCommitSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());

    let before_child_generation = capture_render_generation(commands);
    commit_test_buffered_surface(&child, &shm, &qh, 11, 7)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let before_parent = capture_renderable_surface_snapshot(commands);
    let after_child_generation = capture_render_generation(commands);

    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_parent = capture_renderable_surface_snapshot(commands);
    let after_parent_generation = capture_render_generation(commands);

    Ok(SynchronizedCommitSnapshots {
        before_parent,
        after_parent,
        before_child_generation,
        after_child_generation,
        after_parent_generation,
    })
}

fn capture_subsurface_position_before_and_after_parent_commit(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<
    (Vec<RenderableSurfaceSnapshot>, Vec<RenderableSurfaceSnapshot>),
    Box<dyn std::error::Error>,
> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 11, 7)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    subsurface.set_position(30, 40);
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let before_parent = capture_renderable_surface_snapshot(commands);

    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_parent = capture_renderable_surface_snapshot(commands);
    Ok((before_parent, after_parent))
}

fn capture_multiple_synchronized_child_commits(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<MultipleSynchronizedCommitSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    commit_test_buffered_surface(&child, &shm, &qh, 11, 7)?;
    commit_test_buffered_surface(&child, &shm, &qh, 13, 9)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    let before_parent = capture_renderable_surface_snapshot(commands);
    let superseded_buffer_releases = state.buffer_release_count;
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    let after_parent = capture_renderable_surface_snapshot(commands);
    Ok(MultipleSynchronizedCommitSnapshots {
        before_parent,
        after_parent,
        superseded_buffer_releases,
    })
}

fn capture_cached_child_before_and_after_set_desync(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<
    (Vec<RenderableSurfaceSnapshot>, Vec<RenderableSurfaceSnapshot>),
    Box<dyn std::error::Error>,
> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    commit_test_buffered_surface(&child, &shm, &qh, 9, 7)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let before_desync = capture_renderable_surface_snapshot(commands);
    subsurface.set_desync();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_desync = capture_renderable_surface_snapshot(commands);
    Ok((before_desync, after_desync))
}

fn capture_effectively_synchronized_grandchild_update(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<
    (Vec<RenderableSurfaceSnapshot>, Vec<RenderableSurfaceSnapshot>),
    Box<dyn std::error::Error>,
> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let root = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&root, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let _child_role = subcompositor.get_subsurface(&child, &root, &qh, ());
    let grandchild = compositor.create_surface(&qh, ());
    let grandchild_role = subcompositor.get_subsurface(&grandchild, &child, &qh, ());
    grandchild_role.set_desync();

    commit_test_buffered_surface(&grandchild, &shm, &qh, 3, 3)?;
    commit_test_buffered_surface(&child, &shm, &qh, 7, 7)?;
    commit_test_buffered_surface(&root, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    commit_test_buffered_surface(&grandchild, &shm, &qh, 9, 5)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let before_root = capture_renderable_surface_snapshot(commands);
    root.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_root = capture_renderable_surface_snapshot(commands);
    Ok((before_root, after_root))
}

fn capture_decorated_tree_during_root_resize_commit(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<
    (Vec<RenderableSurfaceSnapshot>, Vec<RenderableSurfaceSnapshot>),
    Box<dyn std::error::Error>,
> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let root = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&root, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let titlebar = compositor.create_surface(&qh, ());
    let titlebar_role = subcompositor.get_subsurface(&titlebar, &root, &qh, ());
    titlebar_role.set_position(0, 0);
    let border = compositor.create_surface(&qh, ());
    let border_role = subcompositor.get_subsurface(&border, &root, &qh, ());
    border_role.set_position(0, 20);

    commit_test_buffered_surface(&titlebar, &shm, &qh, 300, 20)?;
    commit_test_buffered_surface(&border, &shm, &qh, 10, 180)?;
    commit_test_buffered_surface(&root, &shm, &qh, 300, 200)?;
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

    let resized_width = usize::try_from(state.toplevel_width)?;
    let resized_height = usize::try_from(state.toplevel_height)?;
    commit_test_buffered_surface(&titlebar, &shm, &qh, resized_width, 20)?;
    commit_test_buffered_surface(&border, &shm, &qh, 10, resized_height - 20)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    let before_root = capture_renderable_surface_snapshot(commands);

    commit_test_buffered_surface(&root, &shm, &qh, resized_width, resized_height)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    let after_root = capture_renderable_surface_snapshot(commands);
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok((before_root, after_root))
}

fn capture_synchronized_child_frame_callback_lifecycle(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(bool, bool), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    let _callback = child.frame(&qh, ());
    child.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let before_parent = state.frame_done;

    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok((before_parent, state.frame_done))
}

fn capture_root_commit_before_synchronized_child_update(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RootBeforeChildSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor =
        globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let root = compositor.create_surface(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &root, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;
    commit_test_buffered_surface(&root, &shm, &qh, 20, 15)?;
    connection.flush()?;
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    commit_test_buffered_surface(&root, &shm, &qh, 30, 25)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    let after_root = capture_renderable_surface_snapshot(commands);
    commit_test_buffered_surface(&child, &shm, &qh, 9, 7)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    let after_child_without_parent = capture_renderable_surface_snapshot(commands);
    root.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    let after_next_parent = capture_renderable_surface_snapshot(commands);
    Ok(RootBeforeChildSnapshots {
        after_root,
        after_child_without_parent,
        after_next_parent,
    })
}

fn create_toplevel_then_attach_null_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    commit_test_buffered_surface(&surface, &shm, &qh, 2, 2)?;
    surface.attach(None, 0, 0);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_toplevel_with_nested_subsurfaces_then_attach_null_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let grandchild = compositor.create_surface(&qh, ());
    let child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    let grandchild_subsurface = subcompositor.get_subsurface(&grandchild, &child, &qh, ());

    child_subsurface.set_position(10, 12);
    grandchild_subsurface.set_position(3, 4);
    commit_test_buffered_surface(&grandchild, &shm, &qh, 1, 1)?;
    commit_test_buffered_surface(&child, &shm, &qh, 1, 1)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 2, 2)?;
    parent.attach(None, 0, 0);
    parent.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

