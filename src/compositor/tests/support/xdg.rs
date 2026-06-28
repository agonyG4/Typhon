fn read_registry_globals(socket_path: &PathBuf) -> Result<Vec<String>, Box<dyn std::error::Error>> {
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

fn request_presentation_feedback(
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

fn create_surface_with_presentation_feedback_and_present(
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

fn create_surface_with_unpresented_presentation_feedback(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
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

    Ok(())
}

fn create_client_toplevel(socket_path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
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

fn create_configured_client_toplevel(
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
    toplevel.set_app_id("oblivion.configure-test".to_string());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_toplevel_and_check_initial_commit_configure_order(
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

fn recreate_toplevel_role_on_same_surface(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(RegistryTestState, u32, XdgRoleSnapshot), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let surface = compositor.create_surface(&qh, ());
    let surface_id = surface.id().protocol_id();

    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    xdg_surface.set_window_geometry(5, 6, 111, 77);
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_app_id("oblivion.recreate-a".to_string());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    assert_eq!(state.surface_configure_count, 1);
    assert_eq!(state.toplevel_configure_count, 1);

    toplevel.destroy();
    xdg_surface.destroy();
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    state.surface_configured = false;
    state.toplevel_configured = false;
    state.toplevel_configure_count = 0;

    let recreated_xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let recreated_toplevel = recreated_xdg_surface.get_toplevel(&qh, ());
    recreated_toplevel.set_app_id("oblivion.recreate-b".to_string());
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    Ok((
        state,
        surface_id,
        capture_xdg_role_snapshot(commands, surface_id),
    ))
}

fn create_popup_and_check_initial_commit_configure_order(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-initial-configure-parent".to_string());
    parent.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 120, 90)?;
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

fn create_client_toplevel_with_configured_popup(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-parent".to_string());
    commit_test_buffered_surface(&parent, &shm, &qh, 120, 90)?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
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
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 60, 40)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_client_popup_with_constrained_positioner(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-constraint-parent".to_string());
    commit_test_buffered_surface(&parent, &shm, &qh, 120, 90)?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
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
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 80, 50)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_client_popup_then_reposition(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-reposition-parent".to_string());
    commit_test_buffered_surface(&parent, &shm, &qh, 120, 90)?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
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
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 60, 40)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    let repositioner = wm_base.create_positioner(&qh, ());
    repositioner.set_size(50, 30);
    repositioner.set_anchor_rect(5, 7, 1, 1);
    repositioner.set_anchor(client_xdg_positioner::Anchor::TopLeft);
    repositioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    repositioner.set_offset(1, 1);
    popup.reposition(&repositioner, 77);
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 50, 30)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_client_popup_with_window_geometry(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-window-geometry-parent".to_string());
    parent_xdg_surface.set_window_geometry(8, 9, 100, 80);
    commit_test_buffered_surface(&parent, &shm, &qh, 120, 90)?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    popup_xdg_surface.set_window_geometry(2, 3, 40, 30);
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(40, 30);
    positioner.set_anchor_rect(10, 20, 1, 1);
    positioner.set_anchor(client_xdg_positioner::Anchor::TopLeft);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    let _popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 40, 30)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_non_reactive_popup_then_set_window_geometry(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-gecko-parent".to_string());
    commit_test_buffered_surface(&parent, &shm, &qh, 120, 90)?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
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
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 177, 493)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_grabbed_popup_then_release_under_cursor(
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
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 60, 40)?;
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

fn create_grabbed_popup_under_cursor(
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
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 60, 40)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    Ok((state, popup_surface_id))
}

fn create_grabbed_popup_then_click_outside(
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
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 60, 40)?;
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

fn create_grabbed_popup_then_axis_outside(
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
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 60, 40)?;
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

fn create_client_surface_and_wait_for_enter(
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
    commit_test_buffered_surface(&surface, &shm, &qh, 40, 30)?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_idle_inhibitor_for_surface_and_capture_state(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
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

fn create_client_surface_with_viewport_destination(
    socket_path: &PathBuf,
    buffer_width: u32,
    buffer_height: u32,
    destination_width: u32,
    destination_height: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
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

fn create_client_surface_with_buffer_offset(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    attach_test_buffered_surface(&surface, &shm, &qh, 40, 30)?;
    surface.offset(5, 7);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(())
}

fn create_configured_client_toplevel_then_resize_focused(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    width: u32,
    height: u32,
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
    toplevel.set_app_id("oblivion.resize-test".to_string());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::ResizeFocusedTo { width, height })?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_buffered_toplevel_then_resize_drag(
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

fn create_buffered_toplevel_then_toggle_maximize(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_buffered_toplevel_then_window_commands(
        socket_path,
        commands,
        &[ServerCommand::ToggleMaximizeFocused],
    )
}

fn create_buffered_toplevel_then_toggle_maximize_twice(
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

fn create_buffered_toplevel_then_toggle_fullscreen(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_buffered_toplevel_then_window_commands(
        socket_path,
        commands,
        &[ServerCommand::ToggleFullscreenFocused],
    )
}

fn create_buffered_toplevel_then_window_commands(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    window_commands: &[ServerCommand],
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
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

fn create_buffered_toplevel_then_coalesced_resize_drag(
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

fn create_buffered_toplevel_then_active_resize_configure(
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

fn create_buffered_toplevel_then_resize_drag_without_client_commit_between_frames(
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

fn create_buffered_toplevel_then_queue_resize_configure_and_capture_pending_frame_work(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
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

fn create_buffered_toplevel_then_queue_resize_configure_and_unmap(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
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

fn create_buffered_toplevel_then_prepare_queued_resize_configure_and_capture_pending_frame_work(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(bool, bool), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
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

fn create_buffered_toplevel_then_resize_drag_and_release(
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

fn create_buffered_toplevel_then_alt_top_left_resize_drag_and_release(
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

fn create_csd_toplevel_then_resize_drag_commit_buffer_margin_and_release(
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

fn create_buffered_toplevel_then_measure_configure_only_resize_generation(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(u64, u64), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
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

fn create_buffered_toplevel_request_move_and_drag(
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

fn create_toplevel_request_move_from_client_chrome_surface(
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
    chrome.attach(Some(&chrome_buffer), 0, 0);
    chrome.damage_buffer(0, 0, chrome_width as i32, chrome_height as i32);
    chrome.commit();
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

fn create_buffered_toplevel_request_top_left_resize_and_drag(
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

fn create_buffered_toplevel_then_frame_corner_resize_drag(
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

fn create_buffered_toplevel_then_frame_corner_resize_click_with_tiny_motion(
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

fn create_buffered_toplevel_then_left_edge_shrink_before_client_commit(
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

