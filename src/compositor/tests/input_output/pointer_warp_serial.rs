use super::*;

struct TestSurface {
    surface: client_wl_surface::WlSurface,
    _xdg_surface: client_xdg_surface::XdgSurface,
    _toplevel: client_xdg_toplevel::XdgToplevel,
}

struct PointerWarpFixture {
    socket_path: PathBuf,
    connection: Connection,
    queue: EventQueue<RegistryTestState>,
    state: RegistryTestState,
    globals: wayland_client::globals::GlobalList,
    commands: Sender<ServerCommand>,
    server_thread: Option<std::thread::JoinHandle<OwnCompositorServer>>,
    _compositor: client_wl_compositor::WlCompositor,
    _wm_base: client_xdg_wm_base::XdgWmBase,
    _shm: client_wl_shm::WlShm,
    _seat: client_wl_seat::WlSeat,
    _relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
    _relative_pointer: client_zwp_relative_pointer_v1::ZwpRelativePointerV1,
    constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
    pointer_warp: client_wp_pointer_warp_v1::WpPointerWarpV1,
    pointer: client_wl_pointer::WlPointer,
    surface: TestSurface,
}

impl PointerWarpFixture {
    fn new() -> Self {
        Self::new_at_seat_version(7)
    }

    fn new_at_seat_version(seat_version: u32) -> Self {
        let socket_name = unique_socket_name();
        let capabilities = InputProtocolCapabilities {
            pointer_constraints: true,
            pointer_warp: true,
            relative_pointer: true,
            ..InputProtocolCapabilities::desktop_baseline()
        };
        let server =
            OwnCompositorServer::bind_with_input_capabilities(&socket_name, capabilities).unwrap();
        let socket_path = runtime_socket_path(&socket_name);
        let (commands, server_thread) = spawn_controllable_test_server(server);

        let stream = UnixStream::connect(&socket_path).unwrap();
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=2, ()).unwrap();
        let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=seat_version, ()).unwrap();
        let pointer = seat.get_pointer(&qh, ());
        let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
            globals.bind(&qh, 1..=1, ()).unwrap();
        let relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());
        let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
            globals.bind(&qh, 1..=1, ()).unwrap();
        let pointer_warp: client_wp_pointer_warp_v1::WpPointerWarpV1 =
            globals.bind(&qh, 1..=1, ()).unwrap();
        let (surface, xdg_surface, toplevel) =
            create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120).unwrap();
        surface.commit();
        connection.flush().unwrap();

        let mut state = RegistryTestState::default();
        queue.roundtrip(&mut state).unwrap();

        Self {
            socket_path,
            connection,
            queue,
            state,
            globals,
            commands,
            server_thread: Some(server_thread),
            _compositor: compositor,
            _wm_base: wm_base,
            _shm: shm,
            _seat: seat,
            _relative_manager: relative_manager,
            _relative_pointer: relative_pointer,
            constraints,
            pointer_warp,
            pointer,
            surface: TestSurface {
                surface,
                _xdg_surface: xdg_surface,
                _toplevel: toplevel,
            },
        }
    }

    fn process(&mut self) {
        wait_for_server_commands(&self.commands);
        self.queue.roundtrip(&mut self.state).unwrap();
    }

    fn focus_at(&mut self, x: f64, y: f64) -> u32 {
        self.commands
            .send(ServerCommand::PointerMotion { x, y })
            .unwrap();
        self.process();
        self.state.pointer_enter_serial.unwrap()
    }

    fn create_surface(&self) -> TestSurface {
        let qh = self.queue.handle();
        let (surface, xdg_surface, toplevel) = create_test_buffered_toplevel(
            &self._compositor,
            &self._wm_base,
            &self._shm,
            &qh,
            160,
            120,
        )
        .unwrap();
        surface.commit();
        self.connection.flush().unwrap();
        TestSurface {
            surface,
            _xdg_surface: xdg_surface,
            _toplevel: toplevel,
        }
    }

    fn churn_generic_pointer_serials(&mut self, count: usize) {
        for _ in 0..count {
            self.commands
                .send(ServerCommand::PointerButton {
                    button: 1,
                    pressed: true,
                })
                .unwrap();
            self.commands
                .send(ServerCommand::PointerButton {
                    button: 1,
                    pressed: false,
                })
                .unwrap();
        }
        self.process();
    }

    fn warp(&mut self, surface: &client_wl_surface::WlSurface, x: f64, y: f64, serial: u32) {
        self.pointer_warp
            .warp_pointer(surface, &self.pointer, x, y, serial);
        self.connection.flush().unwrap();
        self.process();
    }

    fn last_pointer_position(&self) -> (f64, f64) {
        let (reply, receiver) = mpsc::channel();
        self.commands
            .send(ServerCommand::CaptureLastPointerPosition(reply))
            .unwrap();
        receiver.recv().unwrap()
    }
}

impl Drop for PointerWarpFixture {
    fn drop(&mut self) {
        if let Some(server_thread) = self.server_thread.take() {
            let _ = self.commands.send(ServerCommand::Stop);
            server_thread.join().unwrap();
        }
    }
}

#[test]
fn current_pointer_enter_serial_survives_generic_serial_churn() {
    let mut fixture = PointerWarpFixture::new();
    let anchor = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    );
    let serial = fixture.focus_at(anchor.0, anchor.1);
    assert_eq!(fixture.state.pointer_enter_count, 1);
    assert_eq!(fixture.state.pointer_leave_count, 0);
    assert_eq!(fixture.state.pointer_enter_serial, Some(serial));

    fixture.churn_generic_pointer_serials(17);
    assert_eq!(fixture.state.pointer_enter_count, 1);
    assert_eq!(fixture.state.pointer_leave_count, 0);

    fixture.state.pointer_motion = false;
    fixture.state.pointer_event_log.clear();
    fixture.warp(&fixture.surface.surface.clone(), 80.0, 60.0, serial);

    let expected = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 80.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 60.0,
    );
    assert_eq!(fixture.last_pointer_position(), expected);
    assert!(fixture.state.pointer_motion);
    assert_eq!(fixture.state.pointer_leave_count, 0);
    assert_eq!(fixture.state.pointer_event_log, vec!["motion", "frame"]);
}

#[test]
fn current_pointer_enter_serial_survives_repeated_lock_unlock_cycles() {
    let mut fixture = PointerWarpFixture::new();
    let anchor = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    );
    let serial = fixture.focus_at(anchor.0, anchor.1);

    for _ in 0..20 {
        fixture.churn_generic_pointer_serials(1);
        fixture.state.locked_count = 0;
        let lock = fixture.constraints.lock_pointer(
            &fixture.surface.surface,
            &fixture.pointer,
            None,
            client_zwp_pointer_constraints_v1::Lifetime::Persistent,
            &fixture.queue.handle(),
            (),
        );
        fixture.surface.surface.commit();
        fixture.connection.flush().unwrap();
        fixture.process();
        activate_backend_locked_pointer(&fixture.commands, &mut fixture.state, &mut fixture.queue)
            .unwrap();

        lock.destroy();
        fixture.surface.surface.commit();
        fixture.connection.flush().unwrap();
        fixture.process();
    }

    fixture.state.pointer_motion = false;
    fixture.warp(&fixture.surface.surface.clone(), 80.0, 60.0, serial);

    let expected = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 80.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 60.0,
    );
    assert_eq!(fixture.last_pointer_position(), expected);
    assert!(fixture.state.pointer_motion);
    assert_eq!(fixture.state.pointer_leave_count, 0);
}

#[test]
fn current_pointer_enter_serial_allows_same_client_target_surface() {
    let mut fixture = PointerWarpFixture::new();
    let target = fixture.create_surface();
    let anchor = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 8.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 8.0,
    );
    let serial = fixture.focus_at(anchor.0, anchor.1);

    fixture.state.pointer_motion = false;
    fixture.state.pointer_event_log.clear();
    fixture.warp(&target.surface, 30.0, 40.0, serial);

    assert_ne!(fixture.last_pointer_position(), anchor);
    assert!(!fixture.state.pointer_motion);
    assert_eq!(
        fixture.state.pointer_event_log,
        vec!["leave", "frame", "enter", "frame"]
    );
}

#[test]
fn v11_same_surface_client_warp_uses_warp_event_without_motion() {
    let mut fixture = PointerWarpFixture::new_at_seat_version(11);
    let anchor = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    );
    let serial = fixture.focus_at(anchor.0, anchor.1);

    fixture.state.pointer_motion = false;
    fixture.state.pointer_surface_x = None;
    fixture.state.pointer_surface_y = None;
    fixture.state.pointer_event_log.clear();
    fixture.warp(&fixture.surface.surface.clone(), 80.0, 60.0, serial);

    let expected = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 80.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 60.0,
    );
    assert_eq!(fixture.last_pointer_position(), expected);
    assert!(!fixture.state.pointer_motion);
    assert_eq!(fixture.state.pointer_surface_x, Some(80.0));
    assert_eq!(fixture.state.pointer_surface_y, Some(60.0));
    assert_eq!(fixture.state.relative_motion_count, 0);
    assert_eq!(fixture.state.pointer_event_log, vec!["warp", "frame"]);
}

#[test]
fn mixed_pointer_resource_versions_receive_their_own_reposition_event() {
    let mut fixture = PointerWarpFixture::new_at_seat_version(11);
    let qh = fixture.queue.handle();
    let legacy_seat: client_wl_seat::WlSeat = fixture.globals.bind(&qh, 1..=7, ()).unwrap();
    let legacy_pointer = legacy_seat.get_pointer(&qh, ());
    fixture.connection.flush().unwrap();

    let anchor = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    );
    fixture.focus_at(anchor.0, anchor.1);
    let modern_id = fixture.pointer.id().protocol_id();
    let legacy_id = legacy_pointer.id().protocol_id();
    let modern_serial = fixture
        .state
        .pointer_enter_serials
        .iter()
        .find_map(|(pointer_id, serial)| (*pointer_id == modern_id).then_some(*serial))
        .expect("expected v11 pointer enter serial");

    fixture.state.pointer_motion = false;
    fixture.state.pointer_event_log.clear();
    fixture.state.pointer_warp_resource_ids.clear();
    fixture.state.pointer_motion_resource_ids.clear();
    fixture.warp(&fixture.surface.surface.clone(), 80.0, 60.0, modern_serial);

    assert_eq!(fixture.state.pointer_warp_resource_ids, vec![modern_id]);
    assert_eq!(fixture.state.pointer_motion_resource_ids, vec![legacy_id]);
    assert_eq!(
        fixture.state.pointer_event_log,
        vec!["warp", "frame", "motion", "frame"]
    );
}

#[test]
fn v11_cross_surface_reposition_is_enter_frame_isolated() {
    let mut fixture = PointerWarpFixture::new_at_seat_version(11);
    let target = fixture.create_surface();
    let anchor = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 8.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 8.0,
    );
    let serial = fixture.focus_at(anchor.0, anchor.1);

    fixture.state.pointer_event_log.clear();
    fixture.warp(&target.surface, 30.0, 40.0, serial);

    assert_eq!(
        fixture.state.pointer_event_log,
        vec!["leave", "frame", "enter", "frame"]
    );
}

#[test]
fn v11_reposition_respects_implicit_grab_owner() {
    let mut fixture = PointerWarpFixture::new_at_seat_version(11);
    let target = fixture.create_surface();
    let anchor = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 8.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 8.0,
    );
    let serial = fixture.focus_at(anchor.0, anchor.1);
    fixture
        .commands
        .send(ServerCommand::PointerButton {
            button: 272,
            pressed: true,
        })
        .unwrap();
    fixture.process();

    fixture.state.pointer_event_log.clear();
    fixture.warp(&target.surface, 70.0, 50.0, serial);

    assert_eq!(
        fixture.state.pointer_enter_surface_id,
        Some(fixture.surface.surface.id().protocol_id())
    );
    assert_eq!(fixture.state.pointer_event_log, vec!["warp", "frame"]);
    assert!(fixture.state.pointer_surface_x.unwrap() > 160.0);
    assert!(fixture.state.pointer_surface_y.unwrap() > 120.0);
}

#[test]
fn v11_lock_restore_uses_warp_without_relative_motion() {
    let mut fixture = PointerWarpFixture::new_at_seat_version(11);
    let anchor = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    );
    fixture.focus_at(anchor.0, anchor.1);
    let lock = fixture.constraints.lock_pointer(
        &fixture.surface.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &fixture.queue.handle(),
        (),
    );
    fixture.surface.surface.commit();
    fixture.connection.flush().unwrap();
    fixture.process();
    activate_backend_locked_pointer(&fixture.commands, &mut fixture.state, &mut fixture.queue)
        .unwrap();

    let restore_local = (70.0, 50.0);
    lock.set_cursor_position_hint(restore_local.0, restore_local.1);
    fixture.surface.surface.commit();
    lock.destroy();
    fixture.surface.surface.commit();
    fixture.connection.flush().unwrap();
    fixture.process();
    let requests = capture_pointer_constraint_backend_requests(&fixture.commands);
    let backend_id = requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::Deactivate { id, .. } => Some(*id),
            _ => None,
        })
        .expect("expected lock deactivation request");

    fixture.state.pointer_event_log.clear();
    fixture.state.relative_motion_count = 0;
    fixture
        .commands
        .send(ServerCommand::PointerConstraintBackendDeactivated(
            backend_id,
        ))
        .unwrap();
    fixture.process();
    assert!(
        fixture.state.pointer_event_log.is_empty(),
        "backend restore ACK must not publish fallback before dispatch grace"
    );
    let (pending_after_ack, ack_requests) =
        capture_pending_locked_pointer_reveal_and_backend_requests(&fixture.commands);
    assert!(pending_after_ack);
    assert!(!ack_requests.iter().any(|request| matches!(
        request,
        PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true }
    )));

    for _ in 0..4 {
        fixture.process();
    }
    let reveal_requests = capture_pointer_constraint_backend_requests(&fixture.commands);

    assert_eq!(fixture.state.pointer_event_log, vec!["warp", "frame"]);
    assert_eq!(fixture.state.pointer_surface_x, Some(restore_local.0));
    assert_eq!(fixture.state.pointer_surface_y, Some(restore_local.1));
    assert_eq!(fixture.state.relative_motion_count, 0);
    assert_eq!(
        fixture.last_pointer_position(),
        (
            f64::from(render::FIRST_SURFACE_OFFSET.0) + restore_local.0,
            f64::from(render::FIRST_SURFACE_OFFSET.1) + restore_local.1,
        )
    );
    assert!(reveal_requests.iter().any(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true }
        )
    }));
    assert!(!capture_pending_locked_pointer_reveal(&fixture.commands));
}

#[test]
fn v11_client_warp_after_backend_ack_settles_unlock() {
    let mut fixture = PointerWarpFixture::new_at_seat_version(11);
    let anchor = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    );
    let serial = fixture.focus_at(anchor.0, anchor.1);
    let lock = fixture.constraints.lock_pointer(
        &fixture.surface.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &fixture.queue.handle(),
        (),
    );
    fixture.surface.surface.commit();
    fixture.connection.flush().unwrap();
    fixture.process();
    activate_backend_locked_pointer(&fixture.commands, &mut fixture.state, &mut fixture.queue)
        .unwrap();

    lock.set_cursor_position_hint(120.0, 0.0);
    fixture.surface.surface.commit();
    lock.destroy();
    fixture.surface.surface.commit();
    fixture.connection.flush().unwrap();
    fixture.process();
    let unlock_requests = capture_pointer_constraint_backend_requests(&fixture.commands);
    let backend_id = unlock_requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::Deactivate { id, .. } => Some(*id),
            _ => None,
        })
        .expect("expected lock deactivation request");

    fixture.state.pointer_event_log.clear();
    fixture
        .commands
        .send(ServerCommand::PointerConstraintBackendDeactivated(
            backend_id,
        ))
        .unwrap();
    fixture.process();
    assert!(
        fixture.state.pointer_event_log.is_empty(),
        "backend ACK must leave a post-unlock warp window"
    );
    fixture.state.pointer_motion = false;
    fixture.state.pointer_surface_x = None;
    fixture.state.pointer_surface_y = None;
    fixture.state.pointer_event_log.clear();
    fixture.warp(&fixture.surface.surface.clone(), 30.0, 0.0, serial);
    let (pending_after_warp, final_requests) =
        capture_pending_locked_pointer_reveal_and_backend_requests(&fixture.commands);

    assert_eq!(
        fixture.last_pointer_position(),
        (
            f64::from(render::FIRST_SURFACE_OFFSET.0) + 30.0,
            f64::from(render::FIRST_SURFACE_OFFSET.1),
        )
    );
    assert_eq!(fixture.state.pointer_event_log, vec!["warp", "frame"]);
    assert!(!fixture.state.pointer_motion);
    assert!(!pending_after_warp);
    let warp_index = final_requests.iter().position(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::WarpPointer {
                position: OutputPosition { x, y }
            } if (*x, *y) == (
                f64::from(render::FIRST_SURFACE_OFFSET.0) + 30.0,
                f64::from(render::FIRST_SURFACE_OFFSET.1),
            )
        )
    });
    let visible_index = final_requests.iter().position(|request| {
        matches!(
            request,
            PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true }
        )
    });
    assert!(warp_index.is_some());
    assert!(visible_index.is_some());
    assert!(warp_index.unwrap() < visible_index.unwrap());
}

#[test]
fn pending_unlock_accepts_same_client_cross_surface_warp() {
    let mut fixture = PointerWarpFixture::new_at_seat_version(11);
    let target = fixture.create_surface();
    fixture.process();
    let anchor = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 8.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 8.0,
    );
    let serial = fixture.focus_at(anchor.0, anchor.1);
    let lock = fixture.constraints.lock_pointer(
        &fixture.surface.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &fixture.queue.handle(),
        (),
    );
    fixture.surface.surface.commit();
    fixture.connection.flush().unwrap();
    fixture.process();
    activate_backend_locked_pointer(&fixture.commands, &mut fixture.state, &mut fixture.queue)
        .unwrap();

    lock.set_cursor_position_hint(120.0, 0.0);
    fixture.surface.surface.commit();
    lock.destroy();
    fixture.surface.surface.commit();
    fixture.connection.flush().unwrap();
    fixture.process();
    let unlock_requests = capture_pointer_constraint_backend_requests(&fixture.commands);
    let backend_id = unlock_requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::Deactivate { id, .. } => Some(*id),
            _ => None,
        })
        .expect("expected lock deactivation request");

    fixture.state.pointer_event_log.clear();
    fixture.warp(&target.surface, 30.0, 40.0, serial);
    let (pending_after_warp, warp_requests) =
        capture_pending_locked_pointer_reveal_and_backend_requests(&fixture.commands);
    assert!(pending_after_warp);
    assert_eq!(
        fixture.state.pointer_event_log,
        vec!["leave", "frame", "enter", "frame"]
    );

    fixture
        .commands
        .send(ServerCommand::PointerConstraintBackendDeactivated(
            backend_id,
        ))
        .unwrap();
    fixture.process();
    let (pending_after_ack, settlement_requests) =
        capture_pending_locked_pointer_reveal_and_backend_requests(&fixture.commands);
    assert!(!pending_after_ack);
    let client_warp_position = warp_requests.iter().find_map(|request| match request {
        PointerConstraintBackendRequest::WarpPointer { position } => Some(*position),
        _ => None,
    });
    let final_pointer_position = fixture.last_pointer_position();
    assert_eq!(
        client_warp_position,
        Some(OutputPosition {
            x: final_pointer_position.0,
            y: final_pointer_position.1,
        })
    );
    assert!(settlement_requests.iter().any(|request| matches!(
        request,
        PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true }
    )));
    assert_eq!(
        fixture.state.pointer_event_log,
        vec!["leave", "frame", "enter", "frame"]
    );
    assert_eq!(
        fixture.state.pointer_enter_surface_id,
        Some(target.surface.id().protocol_id())
    );
}

#[test]
fn stale_backend_deactivation_cannot_settle_newer_constraint() {
    let mut fixture = PointerWarpFixture::new_at_seat_version(11);
    let origin_x = f64::from(render::FIRST_SURFACE_OFFSET.0);
    let origin_y = f64::from(render::FIRST_SURFACE_OFFSET.1);
    let anchor = (origin_x + 20.0, origin_y + 14.0);
    fixture.focus_at(anchor.0, anchor.1);
    let lock_one = fixture.constraints.lock_pointer(
        &fixture.surface.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &fixture.queue.handle(),
        (),
    );
    fixture.surface.surface.commit();
    fixture.connection.flush().unwrap();
    fixture.process();
    activate_backend_locked_pointer(&fixture.commands, &mut fixture.state, &mut fixture.queue)
        .unwrap();

    lock_one.set_cursor_position_hint(120.0, 0.0);
    lock_one.destroy();
    fixture.surface.surface.commit();
    fixture.connection.flush().unwrap();
    fixture.process();
    let unlock_requests = capture_pointer_constraint_backend_requests(&fixture.commands);
    let backend_one = unlock_requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::Deactivate { id, .. } => Some(*id),
            _ => None,
        })
        .expect("expected first lock deactivation request");

    let _lock_two = fixture.constraints.lock_pointer(
        &fixture.surface.surface,
        &fixture.pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &fixture.queue.handle(),
        (),
    );
    fixture.surface.surface.commit();
    fixture.connection.flush().unwrap();
    fixture.process();
    let newer_requests = capture_pointer_constraint_backend_requests(&fixture.commands);
    let backend_two = newer_requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id, .. } => Some(*id),
            _ => None,
        })
        .expect("expected newer lock activation request");
    assert_ne!(backend_one, backend_two);

    fixture.state.pointer_event_log.clear();
    fixture
        .commands
        .send(ServerCommand::PointerConstraintBackendDeactivated(
            backend_one,
        ))
        .unwrap();
    fixture.process();
    for _ in 0..4 {
        fixture.process();
    }
    let (pending_after_stale_ack, stale_requests) =
        capture_pending_locked_pointer_reveal_and_backend_requests(&fixture.commands);
    assert!(pending_after_stale_ack);
    assert!(fixture.state.pointer_event_log.is_empty());
    assert!(!stale_requests.iter().any(|request| matches!(
        request,
        PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true }
    )));
    assert_eq!(
        fixture.last_pointer_position(),
        (origin_x + 120.0, origin_y)
    );

    fixture
        .commands
        .send(ServerCommand::PointerConstraintBackendActivated(
            backend_two,
        ))
        .unwrap();
    fixture.process();
    assert!(!capture_pending_locked_pointer_reveal(&fixture.commands));
}

#[test]
fn wrong_client_enter_serial_is_rejected() {
    let mut fixture = PointerWarpFixture::new();
    let stream_b = UnixStream::connect(&fixture.socket_path).unwrap();
    let connection_b = Connection::from_socket(stream_b).unwrap();
    let (globals_b, _queue_b) = registry_queue_init::<RegistryTestState>(&connection_b).unwrap();
    let qh_b = _queue_b.handle();
    let compositor_b: client_wl_compositor::WlCompositor =
        globals_b.bind(&qh_b, 1..=6, ()).unwrap();
    let wm_base_b: client_xdg_wm_base::XdgWmBase = globals_b.bind(&qh_b, 1..=6, ()).unwrap();
    let shm_b: client_wl_shm::WlShm = globals_b.bind(&qh_b, 1..=2, ()).unwrap();
    let seat_b: client_wl_seat::WlSeat = globals_b.bind(&qh_b, 1..=7, ()).unwrap();
    let pointer_b = seat_b.get_pointer(&qh_b, ());
    let pointer_warp_b: client_wp_pointer_warp_v1::WpPointerWarpV1 =
        globals_b.bind(&qh_b, 1..=1, ()).unwrap();
    let (surface_b, _xdg_surface_b, _toplevel_b) =
        create_test_buffered_toplevel(&compositor_b, &wm_base_b, &shm_b, &qh_b, 160, 120).unwrap();
    surface_b.commit();
    connection_b.flush().unwrap();
    let anchor_a = (
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    );
    let serial_a = fixture.focus_at(anchor_a.0, anchor_a.1);
    pointer_warp_b.warp_pointer(&surface_b, &pointer_b, 30.0, 40.0, serial_a);
    connection_b.flush().unwrap();
    wait_for_server_commands(&fixture.commands);
    fixture.queue.roundtrip(&mut fixture.state).unwrap();

    assert_eq!(fixture.last_pointer_position(), anchor_a);
    assert!(
        capture_pointer_constraint_backend_requests(&fixture.commands)
            .iter()
            .all(|request| !matches!(request, PointerConstraintBackendRequest::WarpPointer { .. }))
    );
}
