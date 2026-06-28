fn create_test_shm_file(pixels: &[u32]) -> Result<File, Box<dyn std::error::Error>> {
    let path = runtime_socket_path(&format!("oblivion-one-shm-{}", unique_socket_name()));
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    fs::remove_file(path)?;
    for pixel in pixels {
        file.write_all(&pixel.to_ne_bytes())?;
    }
    file.flush()?;
    Ok(file)
}

fn spawn_test_server(
    mut server: OwnCompositorServer,
) -> (Arc<AtomicBool>, JoinHandle<OwnCompositorServer>) {
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let server_thread = thread::spawn(move || {
        while server_running.load(Ordering::Relaxed) {
            let _ = server.tick();
            thread::sleep(Duration::from_millis(2));
        }
        server
    });

    (running, server_thread)
}

fn stop_test_server(
    running: Arc<AtomicBool>,
    server_thread: JoinHandle<OwnCompositorServer>,
) -> OwnCompositorServer {
    running.store(false, Ordering::Relaxed);
    server_thread.join().unwrap()
}

#[derive(Clone)]
enum ServerCommand {
    KeyboardKey { key: u32, pressed: bool },
    PointerMotion { x: f64, y: f64 },
    PointerMotionSample(PointerMotionSample),
    ActivatePointerConstraint(PointerConstraintMode),
    PointerButton { button: u32, pressed: bool },
    PointerAxis { horizontal: f64, vertical: f64 },
    BeginFrameAction { x: f64, y: f64 },
    BeginMove { x: f64, y: f64 },
    BeginResize { x: f64, y: f64 },
    UpdateInteraction { x: f64, y: f64 },
    UpdateInteractionResult { x: f64, y: f64, reply: Sender<bool> },
    EndInteraction,
    ResizeFocusedTo { width: u32, height: u32 },
    SetOutputSize { width: u32, height: u32 },
    SetOutputRefresh { refresh_hz: u32 },
    SetOutputScale { scale_factor: f64 },
    MinimizeFocused,
    RestoreNextMinimized,
    ToggleMaximizeFocused,
    ToggleFullscreenFocused,
    CaptureRenderGeneration(Sender<u64>),
    CaptureSceneRenderGeneration(Sender<u64>),
    CaptureRenderGenerationCause(Sender<RenderGenerationCause>),
    CaptureRenderableSurfaceCount(Sender<usize>),
    CaptureRenderableSurfaceSnapshot(Sender<Vec<RenderableSurfaceSnapshot>>),
    CaptureClientCursorSnapshot(Sender<Option<ClientCursorSnapshot>>),
    CaptureXdgRoleSnapshot {
        surface_id: u32,
        reply: Sender<XdgRoleSnapshot>,
    },
    CapturePendingFrameCallbacks(Sender<bool>),
    CaptureOnlyPendingSurfaceFrameCallbacks(Sender<bool>),
    CapturePendingFrameWork(Sender<bool>),
    CaptureIdleInhibited(Sender<bool>),
    CapturePointerConstraintBackendRequests(Sender<Vec<PointerConstraintBackendRequest>>),
    CapturePointerConstraintIds(Sender<Vec<u64>>),
    CaptureLastPointerPosition(Sender<(f64, f64)>),
    CapturePointerFocusSurfaceId(Sender<Option<u32>>),
    UpdatePointerPositionWithoutClientDispatch {
        x: f64,
        y: f64,
        reply: Sender<bool>,
    },
    PointerConstraintBackendActivated(PointerConstraintBackendId),
    PointerConstraintBackendFailed(PointerConstraintBackendId),
    #[allow(dead_code)]
    PointerConstraintBackendDeactivated(PointerConstraintBackendId),
    ClearPointerEnterTracking,
    Barrier(Sender<()>),
    PrepareFrame,
    FinishFrame,
    FinishFrameWithPresentation(FramePresentation),
    PresentFrame,
    Stop,
}

fn spawn_controllable_test_server(
    mut server: OwnCompositorServer,
) -> (Sender<ServerCommand>, JoinHandle<OwnCompositorServer>) {
    let (commands, receiver) = mpsc::channel();
    let server_thread = thread::spawn(move || {
        let mut running = true;
        while running {
            while let Ok(command) = receiver.try_recv() {
                match command {
                    ServerCommand::KeyboardKey { key, pressed } => {
                        server.send_keyboard_key(key, pressed);
                    }
                    ServerCommand::PointerMotion { x, y } => {
                        server.send_pointer_motion(x, y);
                    }
                    ServerCommand::PointerMotionSample(sample) => {
                        server.send_pointer_motion_sample(sample);
                    }
                    ServerCommand::ActivatePointerConstraint(mode) => {
                        server.state.activate_pointer_constraint_for_focused_surface(mode);
                    }
                    ServerCommand::PointerButton { button, pressed } => {
                        server.send_pointer_button(button, pressed);
                    }
                    ServerCommand::PointerAxis {
                        horizontal,
                        vertical,
                    } => {
                        server.send_pointer_axis(horizontal, vertical);
                    }
                    ServerCommand::BeginFrameAction { x, y } => {
                        server.begin_window_frame_action_at(x, y);
                    }
                    ServerCommand::BeginMove { x, y } => {
                        server.begin_window_move_at(x, y);
                    }
                    ServerCommand::BeginResize { x, y } => {
                        server.begin_window_resize_at(x, y);
                    }
                    ServerCommand::UpdateInteraction { x, y } => {
                        server.update_window_interaction(x, y);
                    }
                    ServerCommand::UpdateInteractionResult { x, y, reply } => {
                        let _ = reply.send(server.update_window_interaction(x, y));
                    }
                    ServerCommand::EndInteraction => {
                        server.end_window_interaction();
                    }
                    ServerCommand::ResizeFocusedTo { width, height } => {
                        server.resize_focused_window_to(width, height);
                    }
                    ServerCommand::SetOutputSize { width, height } => {
                        server.set_output_size(width, height);
                    }
                    ServerCommand::SetOutputRefresh { refresh_hz } => {
                        server.set_output_refresh_hz(refresh_hz);
                    }
                    ServerCommand::SetOutputScale { scale_factor } => {
                        server.set_output_scale_factor(scale_factor);
                    }
                    ServerCommand::MinimizeFocused => {
                        server.minimize_focused_window();
                    }
                    ServerCommand::RestoreNextMinimized => {
                        server.restore_next_minimized_window();
                    }
                    ServerCommand::ToggleMaximizeFocused => {
                        server.toggle_maximize_focused_window();
                    }
                    ServerCommand::ToggleFullscreenFocused => {
                        server.toggle_fullscreen_focused_window();
                    }
                    ServerCommand::CaptureRenderGeneration(reply) => {
                        let _ = reply.send(server.render_generation());
                    }
                    ServerCommand::CaptureSceneRenderGeneration(reply) => {
                        let _ = reply.send(server.scene_render_generation());
                    }
                    ServerCommand::CaptureRenderGenerationCause(reply) => {
                        let _ = reply.send(server.render_generation_cause());
                    }
                    ServerCommand::CaptureRenderableSurfaceCount(reply) => {
                        let _ = reply.send(server.renderable_surfaces().len());
                    }
                    ServerCommand::CaptureRenderableSurfaceSnapshot(reply) => {
                        let _ = reply.send(
                            server
                                .renderable_surfaces()
                                .iter()
                                .map(|surface| RenderableSurfaceSnapshot {
                                    surface_id: surface.surface_id,
                                    width: surface.width,
                                    height: surface.height,
                                    parent_surface_id: surface.placement.parent_surface_id,
                                    local_x: surface.placement.local_x,
                                    local_y: surface.placement.local_y,
                                    buffer_id: surface.buffer_id().get(),
                                    generation: surface.generation,
                                    resize_preview_active: surface.visual_clip.is_some(),
                                })
                                .collect(),
                        );
                    }
                    ServerCommand::CaptureClientCursorSnapshot(reply) => {
                        let snapshot = server.client_cursor_render_state().map(|cursor| {
                            ClientCursorSnapshot {
                                surface_id: cursor.surface.surface_id,
                                logical_x: cursor.logical_x,
                                logical_y: cursor.logical_y,
                                width: cursor.surface.width,
                                height: cursor.surface.height,
                            }
                        });
                        let _ = reply.send(snapshot);
                    }
                    ServerCommand::CaptureXdgRoleSnapshot { surface_id, reply } => {
                        let tracked_surface_id =
                            if server.state.toplevel_surfaces.len() == 1 {
                                *server.state.toplevel_surfaces.keys().next().unwrap()
                            } else if server.state.surface_resources.contains_key(&surface_id) {
                                surface_id
                            } else if server.state.surface_resources.len() == 1 {
                                *server.state.surface_resources.keys().next().unwrap()
                            } else {
                                surface_id
                            };
                        let _ = reply.send(XdgRoleSnapshot {
                            surface_registered: server
                                .state
                                .surface_resources
                                .contains_key(&tracked_surface_id),
                            configured: server
                                .state
                                .configured_xdg_surfaces
                                .contains(&tracked_surface_id),
                            toplevel_count: server.state.toplevel_surfaces.len(),
                            toplevel_registered: server
                                .state
                                .toplevel_surfaces
                                .contains_key(&tracked_surface_id),
                            popup_count: server.state.popup_surfaces.len(),
                            window_geometry_present: server
                                .state
                                .surface_window_geometries
                                .contains_key(&tracked_surface_id),
                            placement: server
                                .state
                                .surface_placements
                                .get(&tracked_surface_id)
                                .copied(),
                        });
                    }
                    ServerCommand::CapturePendingFrameCallbacks(reply) => {
                        let _ = reply.send(server.has_pending_frame_callbacks());
                    }
                    ServerCommand::CaptureOnlyPendingSurfaceFrameCallbacks(reply) => {
                        let _ = reply.send(server.has_only_pending_surface_frame_callbacks());
                    }
                    ServerCommand::CapturePendingFrameWork(reply) => {
                        let _ = reply.send(server.has_pending_frame_work());
                    }
                    ServerCommand::CaptureIdleInhibited(reply) => {
                        let _ = reply.send(server.state.idle_inhibited());
                    }
                    ServerCommand::CapturePointerConstraintBackendRequests(reply) => {
                        let _ = reply.send(server.take_pointer_constraint_backend_requests());
                    }
                    ServerCommand::CapturePointerConstraintIds(reply) => {
                        let ids = server
                            .state
                            .pointer_constraints
                            .keys()
                            .copied()
                            .collect();
                        let _ = reply.send(ids);
                    }
                    ServerCommand::CaptureLastPointerPosition(reply) => {
                        let _ =
                            reply.send((server.state.last_pointer_x, server.state.last_pointer_y));
                    }
                    ServerCommand::CapturePointerFocusSurfaceId(reply) => {
                        let _ = reply.send(
                            server
                                .state
                                .pointer_surface
                                .as_ref()
                                .map(compositor_surface_id),
                        );
                    }
                    ServerCommand::UpdatePointerPositionWithoutClientDispatch { x, y, reply } => {
                        let _ = reply.send(
                            server.update_pointer_position_without_client_dispatch(x, y),
                        );
                    }
                    ServerCommand::PointerConstraintBackendActivated(id) => {
                        server.pointer_constraint_backend_activated(id);
                    }
                    ServerCommand::PointerConstraintBackendFailed(id) => {
                        server.pointer_constraint_backend_failed(id, "test failure");
                    }
                    ServerCommand::PointerConstraintBackendDeactivated(id) => {
                        server.pointer_constraint_backend_deactivated(id);
                    }
                    ServerCommand::ClearPointerEnterTracking => {
                        server.state.pointer_entered_surfaces.clear();
                    }
                    ServerCommand::Barrier(reply) => {
                        let _ = reply.send(());
                    }
                    ServerCommand::PrepareFrame => {
                        server.prepare_frame();
                    }
                    ServerCommand::FinishFrame => {
                        server.finish_frame();
                    }
                    ServerCommand::FinishFrameWithPresentation(presentation) => {
                        server.finish_frame_with_presentation(presentation);
                    }
                    ServerCommand::PresentFrame => {
                        server.present_frame();
                    }
                    ServerCommand::Stop => running = false,
                }
            }
            let _ = server.tick();
            thread::sleep(Duration::from_millis(2));
        }
        server
    });

    (commands, server_thread)
}

fn stop_controllable_test_server(
    commands: Sender<ServerCommand>,
    server_thread: JoinHandle<OwnCompositorServer>,
) -> OwnCompositorServer {
    let _ = commands.send(ServerCommand::Stop);
    server_thread.join().unwrap()
}

fn wait_for_server_commands(commands: &Sender<ServerCommand>) {
    let (reply, receiver) = mpsc::channel();
    commands.send(ServerCommand::Barrier(reply)).unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should process command barrier");
}

fn capture_render_generation(commands: &Sender<ServerCommand>) -> u64 {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureRenderGeneration(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report render generation")
}

fn capture_scene_render_generation(commands: &Sender<ServerCommand>) -> u64 {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureSceneRenderGeneration(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report scene render generation")
}

fn capture_pointer_focus_surface_id(commands: &Sender<ServerCommand>) -> Option<u32> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePointerFocusSurfaceId(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report pointer focus surface")
}

fn capture_render_generation_cause(commands: &Sender<ServerCommand>) -> RenderGenerationCause {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureRenderGenerationCause(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report render generation cause")
}

fn capture_renderable_surface_count(commands: &Sender<ServerCommand>) -> usize {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureRenderableSurfaceCount(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report renderable surface count")
}

fn capture_renderable_surface_snapshot(
    commands: &Sender<ServerCommand>,
) -> Vec<RenderableSurfaceSnapshot> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureRenderableSurfaceSnapshot(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report renderable surface snapshot")
}

fn capture_client_cursor_snapshot(
    commands: &Sender<ServerCommand>,
) -> Option<ClientCursorSnapshot> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureClientCursorSnapshot(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report client cursor snapshot")
}

fn capture_xdg_role_snapshot(commands: &Sender<ServerCommand>, surface_id: u32) -> XdgRoleSnapshot {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureXdgRoleSnapshot { surface_id, reply })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report XDG role snapshot")
}

fn capture_pending_frame_callbacks(commands: &Sender<ServerCommand>) -> bool {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePendingFrameCallbacks(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report pending frame callbacks")
}

fn capture_only_pending_surface_frame_callbacks(commands: &Sender<ServerCommand>) -> bool {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureOnlyPendingSurfaceFrameCallbacks(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report pending surface frame callback state")
}

fn capture_pending_frame_work(commands: &Sender<ServerCommand>) -> bool {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePendingFrameWork(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report pending frame work")
}

fn update_interaction_and_report(commands: &Sender<ServerCommand>, x: f64, y: f64) -> bool {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::UpdateInteractionResult { x, y, reply })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report interaction update")
}

fn runtime_socket_path(socket_name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join(socket_name)
}

fn activate_backend_locked_pointer(
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

fn locked_relative_motion_survives_stale_hit_test(
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

fn run_locked_relative_motion_targets_exact_source_pointer(
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

fn run_locked_relative_motion_falls_back_to_same_client_pointer_resource(
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

fn run_locked_relative_motion_fallback_does_not_cross_clients(
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

    commands
        .send(ServerCommand::PointerMotionSample(PointerMotionSample {
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

fn run_locked_relative_motion_dispatches_to_all_same_client_resources(
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

fn clear_locked_relative_motion_observations(state: &mut RegistryTestState) {
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

struct LockedRelativeFrameResult {
    state: RegistryTestState,
    relative_ids: Vec<u32>,
    pointer_ids: Vec<u32>,
}

fn run_locked_relative_motion_shared_source_pointer_frames(
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

fn run_locked_relative_motion_different_source_pointer_frames(
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

fn capture_pointer_constraint_ids(commands: &Sender<ServerCommand>) -> Vec<u64> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePointerConstraintIds(reply))
        .unwrap();
    receiver.recv().unwrap()
}

fn run_multi_client_pointer_constraints_remain_independent(
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

    commands
        .send(ServerCommand::PointerMotionSample(PointerMotionSample {
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

    Ok((id_a, PointerConstraintBackendId {
        constraint_id: ids.iter().copied().find(|id| *id != id_a.constraint_id).unwrap(),
        generation: id_a.generation,
    }))
}

fn run_locked_relative_motion_survives_surface_tree_churn(
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

fn unique_socket_name() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("oblivion-one-test-{}-{now}", std::process::id())
}
