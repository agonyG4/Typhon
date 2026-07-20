use std::{
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use x11rb::{
    connection::Connection,
    protocol::sync::{ConnectionExt as SyncConnectionExt, Int64},
    protocol::xproto::{AtomEnum, ConnectionExt, CreateWindowAux, PropMode, WindowClass},
    wrapper::ConnectionExt as WrapperConnectionExt,
};

use super::*;
use oblivion_one::{
    compositor::{OwnCompositorServer, XwaylandClientIdentity},
    native::event_loop::monotonic_now_ns,
    process::ChildSupervisor,
    xwayland::{
        XwaylandAssociationEvent, XwaylandConfig, XwaylandGeneration, XwaylandMode,
        XwaylandProfile, XwaylandStateKind,
        xwm::{XwmCommand, XwmEvent},
    },
};

#[derive(Debug)]
struct ServerEvents {
    binds: Vec<XwaylandClientIdentity>,
    associations: Vec<XwaylandAssociationEvent>,
    buffers: Vec<(XwaylandGeneration, u32)>,
}

#[derive(Debug)]
enum CompositorEvent {
    Xwayland(XwmEvent),
}

fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("test lock")
}

fn installed_xwayland() -> Option<PathBuf> {
    std::env::var_os("TYPHON_XWAYLAND_BINARY")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            ["/usr/bin/Xwayland", "/usr/local/bin/Xwayland"]
                .into_iter()
                .map(PathBuf::from)
                .find(|path| path.is_file())
        })
}

fn sync_sources(
    event_loop: &mut NativeEventLoop,
    service: &mut XwaylandService,
    tokens: &mut Vec<(ReactorToken, XwaylandReactorRegistration)>,
) -> io::Result<()> {
    super::sync_xwayland_reactor_sources(event_loop, service, tokens)
        .map_err(|error| io::Error::other(error.to_string()))
}

fn process_server_events(
    receiver: &mpsc::Receiver<ServerEvents>,
    service: &mut XwaylandService,
    supervisor: &mut ChildSupervisor,
    binds_seen: &mut bool,
) {
    while let Ok(events) = receiver.try_recv() {
        for bind in events.binds {
            *binds_seen = true;
            service
                .handle_shell_bind_for_client(bind.generation, &bind.client_id)
                .expect("handle private xwayland-shell bind");
        }
        if !events.associations.is_empty() {
            service.record_association_events(&events.associations);
            for event in events.associations.iter().copied() {
                if let XwaylandAssociationEvent::Committed {
                    generation,
                    surface_id,
                    ..
                } = event
                {
                    service
                        .mark_managed_surface_buffer_ready(supervisor, generation, surface_id)
                        .expect("mark associated native reactor buffer ready");
                }
            }
        }
        for (generation, surface_id) in events.buffers {
            service
                .mark_managed_surface_buffer_ready(supervisor, generation, surface_id)
                .expect("mark native reactor buffer ready");
        }
    }
}

fn dispatch_wakeup(
    wakeup: &NativeWakeup,
    event_loop: &mut NativeEventLoop,
    tokens: &[(ReactorToken, XwaylandReactorRegistration)],
    service: &mut XwaylandService,
    supervisor: &mut ChildSupervisor,
) {
    for event in wakeup.xwayland_events.iter().copied() {
        let registration = tokens
            .iter()
            .find(|(token, _)| *token == event.token)
            .map(|(_, registration)| *registration)
            .expect("native reactor token remains registered");
        service
            .handle_reactor_event_with_token(
                registration.purpose,
                registration.generation,
                event.flags,
                event.token.raw(),
                supervisor,
            )
            .expect("native reactor XWayland event is contained");
    }
    event_loop
        .arm_deadline(Some(
            monotonic_now_ns()
                .expect("native reactor monotonic clock")
                .saturating_add(50_000_000),
        ))
        .expect("arm native reactor test deadline");
}

fn apply_compositor_commands(
    receiver: &mpsc::Receiver<Vec<XwmCommand>>,
    service: &mut XwaylandService,
    supervisor: &mut ChildSupervisor,
) {
    while let Ok(commands) = receiver.try_recv() {
        for command in commands {
            service
                .execute_managed_command(supervisor, command)
                .expect("execute compositor command in native reactor test");
        }
        service
            .flush_managed_commands(supervisor)
            .expect("flush compositor command in native reactor test");
    }
}

#[test]
fn x11_window_reaches_window_ready_without_direct_fd_polling() {
    let _lock = test_lock();
    if std::env::var_os("WAYLAND_DISPLAY").is_none()
        || std::env::var_os("XDG_RUNTIME_DIR").is_none()
    {
        return;
    }
    let Some(binary) = installed_xwayland() else {
        return;
    };

    let socket_name = format!("typhon-native-reactor-{}", std::process::id());
    let mut compositor =
        OwnCompositorServer::bind_cpu_composition(&socket_name).expect("bind test compositor");
    let config = XwaylandConfig {
        mode: XwaylandMode::ManagedEager,
        profile: XwaylandProfile::Managed,
        binary,
        log_stderr: false,
        display_min: 270,
        display_max: 320,
    };
    let mut supervisor = ChildSupervisor::new();
    let mut service = XwaylandService::bootstrap_with_config(config).expect("bootstrap service");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start eager generation");
    let generation = service.generation().expect("starting generation");
    let private_stream = service
        .take_private_wayland_client(generation)
        .expect("take private Wayland client");
    let identity = compositor
        .insert_xwayland_client(private_stream, generation)
        .expect("insert private Wayland client");
    service.authorize_private_client(generation, identity.client_id.clone());

    let (events_sender, events_receiver) = mpsc::channel();
    let (compositor_event_sender, compositor_event_receiver) = mpsc::channel();
    let (compositor_command_sender, compositor_command_receiver) = mpsc::channel();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let server_thread = thread::spawn(move || {
        while server_running.load(Ordering::Relaxed) {
            let _ = compositor.tick();
            while let Ok(CompositorEvent::Xwayland(event)) = compositor_event_receiver.try_recv() {
                let commands = compositor.apply_xwayland_window_event(event);
                let _ = compositor_command_sender.send(commands);
            }
            let events = ServerEvents {
                binds: compositor.take_xwayland_shell_bind_events(),
                associations: compositor.take_xwayland_association_events(),
                buffers: compositor.take_xwayland_buffer_ready_events(),
            };
            if !events.binds.is_empty()
                || !events.associations.is_empty()
                || !events.buffers.is_empty()
            {
                let _ = events_sender.send(events);
            }
            thread::sleep(Duration::from_millis(2));
        }
        compositor
    });

    let mut event_loop = NativeEventLoop::new().expect("create native reactor");
    let mut tokens = Vec::new();
    sync_sources(&mut event_loop, &mut service, &mut tokens).expect("register XWayland sources");
    event_loop
        .arm_deadline(Some(
            monotonic_now_ns()
                .expect("native reactor monotonic clock")
                .saturating_add(50_000_000),
        ))
        .expect("arm startup deadline");

    let startup_deadline = Instant::now() + Duration::from_secs(20);
    let mut binds_seen = false;
    let mut startup_xwm_token = None;
    while service.state_kind() != XwaylandStateKind::Running {
        assert!(
            Instant::now() < startup_deadline,
            "native XWM did not reach Running"
        );
        let wakeup = event_loop.wait().expect("wait for startup reactor event");
        if wakeup.reasons.timer() {
            service
                .handle_deadline(
                    monotonic_now_ns().expect("native reactor monotonic clock"),
                    &mut supervisor,
                )
                .expect("handle startup deadline");
        }
        dispatch_wakeup(
            &wakeup,
            &mut event_loop,
            &tokens,
            &mut service,
            &mut supervisor,
        );
        process_server_events(
            &events_receiver,
            &mut service,
            &mut supervisor,
            &mut binds_seen,
        );
        apply_compositor_commands(&compositor_command_receiver, &mut service, &mut supervisor);
        if service.state_kind() == XwaylandStateKind::Starting && binds_seen {
            let _ = service.initialize_managed_xwm(generation, &mut supervisor);
        }
        sync_sources(&mut event_loop, &mut service, &mut tokens)
            .expect("synchronize startup-to-running sources");
        if startup_xwm_token.is_none() {
            startup_xwm_token = tokens
                .iter()
                .find(|(_, registration)| registration.purpose == XwaylandReactorPurpose::Xwm)
                .map(|(token, _)| *token);
        }
    }
    assert!(binds_seen, "private xwayland-shell bind was not delivered");
    let running_xwm_tokens = tokens
        .iter()
        .filter(|(_, registration)| registration.purpose == XwaylandReactorPurpose::Xwm)
        .collect::<Vec<_>>();
    assert_eq!(
        running_xwm_tokens.len(),
        1,
        "exactly one running XWM source"
    );
    let running_xwm_token = running_xwm_tokens[0].0;
    let startup_xwm_token = startup_xwm_token.expect("startup XWM token");
    assert_ne!(startup_xwm_token, running_xwm_token);
    assert_eq!(
        running_xwm_tokens[0].1.fd,
        service.managed_xwm_fd(generation).expect("running XWM fd")
    );
    assert!(
        !service
            .handle_reactor_event_with_token(
                XwaylandReactorPurpose::Xwm,
                Some(generation),
                libc::EPOLLIN as u32,
                startup_xwm_token.raw(),
                &mut supervisor,
            )
            .expect("stale startup token is contained")
    );

    let display = service.display_number().expect("managed display number");
    let display_name = format!(":{display}");
    let (client, screen) = x11rb::connect(Some(&display_name)).expect("connect X11 client");
    let root_window = client.setup().roots[screen].root;
    let window = client.generate_id().expect("allocate X11 window");
    let sync_counter = client.generate_id().expect("allocate XSync counter");
    client
        .sync_create_counter(sync_counter, Int64 { hi: 0, lo: 0 })
        .expect("create XSync counter");
    let sync_counter_atom = client
        .intern_atom(false, b"_NET_WM_SYNC_REQUEST_COUNTER")
        .expect("intern XSync counter atom")
        .reply()
        .expect("XSync counter atom reply")
        .atom;
    client
        .create_window(
            client.setup().roots[screen].root_depth,
            window,
            root_window,
            120,
            120,
            420,
            180,
            0,
            WindowClass::INPUT_OUTPUT,
            client.setup().roots[screen].root_visual,
            &CreateWindowAux::new(),
        )
        .expect("create X11 managed window");
    client
        .change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            b"native reactor XWayland window",
        )
        .expect("set X11 window title");
    client
        .change_property32(
            PropMode::REPLACE,
            window,
            sync_counter_atom,
            AtomEnum::CARDINAL,
            &[sync_counter],
        )
        .expect("set X11 sync counter");
    client.map_window(window).expect("map X11 managed window");
    client.flush().expect("flush X11 managed window");
    client
        .get_geometry(window)
        .expect("force X11 request processing")
        .reply()
        .expect("X11 geometry reply");

    let window_deadline = Instant::now() + Duration::from_secs(15);
    let mut map_requested = false;
    let mut ready_snapshot = None;
    while ready_snapshot.is_none() {
        assert!(
            Instant::now() < window_deadline,
            "X11 window did not reach WindowReady through NativeEventLoop"
        );
        let wakeup = event_loop
            .wait()
            .expect("wait for running XWM reactor event");
        if wakeup.reasons.timer() {
            service
                .handle_deadline(
                    monotonic_now_ns().expect("native reactor monotonic clock"),
                    &mut supervisor,
                )
                .expect("handle running deadline");
        }
        dispatch_wakeup(
            &wakeup,
            &mut event_loop,
            &tokens,
            &mut service,
            &mut supervisor,
        );
        process_server_events(
            &events_receiver,
            &mut service,
            &mut supervisor,
            &mut binds_seen,
        );
        for event in service.take_managed_xwm_events() {
            match event {
                XwmEvent::WindowMapRequested(handle) => {
                    if handle.xid() == window {
                        map_requested = true;
                    }
                    service
                        .execute_managed_command(&mut supervisor, XwmCommand::Map(handle))
                        .expect("execute compositor map command");
                    service
                        .flush_managed_commands(&mut supervisor)
                        .expect("flush compositor map command");
                }
                XwmEvent::WindowReady(snapshot) if snapshot.handle.xid() == window => {
                    compositor_event_sender
                        .send(CompositorEvent::Xwayland(XwmEvent::WindowReady(
                            snapshot.clone(),
                        )))
                        .expect("send parent WindowReady to compositor");
                    ready_snapshot = Some(snapshot);
                }
                _ => {}
            }
        }
        apply_compositor_commands(&compositor_command_receiver, &mut service, &mut supervisor);
        sync_sources(&mut event_loop, &mut service, &mut tokens)
            .expect("synchronize running XWM source");
    }
    assert!(map_requested, "MapRequest did not reach the managed XWM");
    assert_eq!(
        service.managed_xwm_root_event_mask(generation),
        Some(0x780000),
        "root event mask remains selected on the running XWM connection"
    );
    let snapshot = ready_snapshot.expect("WindowReady snapshot");
    assert_eq!(snapshot.sync_counter, Some(sync_counter as u64));
    for (width, height) in [(460, 220), (520, 260), (580, 300)] {
        service
            .execute_managed_command(
                &mut supervisor,
                XwmCommand::BeginResizeSync {
                    window: snapshot.handle,
                    geometry: oblivion_one::xwayland::xwm::X11Geometry {
                        x: 120,
                        y: 120,
                        width,
                        height,
                    },
                    counter_value: 0,
                    deadline_ns: monotonic_now_ns()
                        .expect("native reactor resize clock")
                        .saturating_add(500_000_000),
                    final_pending: false,
                },
            )
            .expect("issue native reactor synchronized resize");
    }
    service
        .flush_managed_commands(&mut supervisor)
        .expect("flush native reactor synchronized resize");
    assert_eq!(
        service.resize_sync_snapshot(snapshot.handle),
        Some((1, false)),
        "rapid pointer geometries keep one outstanding XSync transaction"
    );
    service
        .execute_managed_command(
            &mut supervisor,
            XwmCommand::SyncClientLists {
                client_list: vec![snapshot.handle],
                stacking: vec![snapshot.handle],
            },
        )
        .expect("publish native reactor client list");
    service
        .flush_managed_commands(&mut supervisor)
        .expect("flush native reactor client list");
    let client_list_atom = client
        .intern_atom(false, b"_NET_CLIENT_LIST")
        .expect("intern client-list atom")
        .reply()
        .expect("client-list atom reply")
        .atom;
    let client_list = client
        .get_property(
            false,
            root_window,
            client_list_atom,
            AtomEnum::WINDOW,
            0,
            1024,
        )
        .expect("query native reactor client list")
        .reply()
        .expect("native reactor client-list reply");
    assert!(
        client_list
            .value32()
            .is_some_and(|mut values| values.any(|xid| xid == window))
    );

    // Map a real X11 popup through the same NativeEventLoop path. The
    // compositor thread receives WindowReady and publishes the semantic
    // client list, so this never drains XWM directly from the test.
    let transient_for_atom = client
        .intern_atom(false, b"WM_TRANSIENT_FOR")
        .expect("intern transient-for atom")
        .reply()
        .expect("transient-for atom reply")
        .atom;
    let window_type_atom = client
        .intern_atom(false, b"_NET_WM_WINDOW_TYPE")
        .expect("intern window-type atom")
        .reply()
        .expect("window-type atom reply")
        .atom;
    let popup_menu_atom = client
        .intern_atom(false, b"_NET_WM_WINDOW_TYPE_POPUP_MENU")
        .expect("intern popup-menu atom")
        .reply()
        .expect("popup-menu atom reply")
        .atom;
    let parent_geometry_before = client
        .get_geometry(window)
        .expect("query parent geometry before popup")
        .reply()
        .expect("parent geometry before popup reply");
    let popup = client.generate_id().expect("allocate X11 popup");
    client
        .create_window(
            client.setup().roots[screen].root_depth,
            popup,
            root_window,
            160,
            160,
            180,
            90,
            0,
            WindowClass::INPUT_OUTPUT,
            client.setup().roots[screen].root_visual,
            &CreateWindowAux::new(),
        )
        .expect("create X11 popup window");
    client
        .change_property32(
            PropMode::REPLACE,
            popup,
            transient_for_atom,
            AtomEnum::WINDOW,
            &[window],
        )
        .expect("set popup transient parent");
    client
        .change_property32(
            PropMode::REPLACE,
            popup,
            window_type_atom,
            AtomEnum::ATOM,
            &[popup_menu_atom],
        )
        .expect("set popup window type");
    client.map_window(popup).expect("map X11 popup window");
    client.flush().expect("flush X11 popup window");

    let popup_deadline = Instant::now() + Duration::from_secs(15);
    let mut popup_snapshot = None;
    while popup_snapshot.is_none() {
        assert!(
            Instant::now() < popup_deadline,
            "X11 popup did not reach WindowReady through NativeEventLoop"
        );
        let wakeup = event_loop.wait().expect("wait for popup reactor event");
        if wakeup.reasons.timer() {
            service
                .handle_deadline(
                    monotonic_now_ns().expect("native reactor monotonic clock"),
                    &mut supervisor,
                )
                .expect("handle popup deadline");
        }
        dispatch_wakeup(
            &wakeup,
            &mut event_loop,
            &tokens,
            &mut service,
            &mut supervisor,
        );
        process_server_events(
            &events_receiver,
            &mut service,
            &mut supervisor,
            &mut binds_seen,
        );
        for event in service.take_managed_xwm_events() {
            match event {
                XwmEvent::WindowMapRequested(handle) => {
                    service
                        .execute_managed_command(&mut supervisor, XwmCommand::Map(handle))
                        .expect("execute popup map command");
                    service
                        .flush_managed_commands(&mut supervisor)
                        .expect("flush popup map command");
                }
                XwmEvent::WindowReady(snapshot) if snapshot.handle.xid() == popup => {
                    compositor_event_sender
                        .send(CompositorEvent::Xwayland(XwmEvent::WindowReady(
                            snapshot.clone(),
                        )))
                        .expect("send popup WindowReady to compositor");
                    popup_snapshot = Some(snapshot);
                }
                _ => {}
            }
        }
        apply_compositor_commands(&compositor_command_receiver, &mut service, &mut supervisor);
        sync_sources(&mut event_loop, &mut service, &mut tokens)
            .expect("synchronize popup reactor sources");
    }
    let popup_snapshot = popup_snapshot.expect("popup WindowReady snapshot");
    assert_eq!(
        popup_snapshot.transient_for.map(|handle| handle.xid()),
        Some(window)
    );
    assert_eq!(
        popup_snapshot.window_type,
        Some(oblivion_one::xwayland::xwm::X11WindowType::PopupMenu)
    );

    // The parent remains in the normal EWMH list while the popup is absent,
    // and mapping it does not change the parent's X11 geometry.
    apply_compositor_commands(&compositor_command_receiver, &mut service, &mut supervisor);
    let client_list = client
        .get_property(
            false,
            root_window,
            client_list_atom,
            AtomEnum::WINDOW,
            0,
            1024,
        )
        .expect("query client list after popup")
        .reply()
        .expect("client list after popup reply");
    assert!(
        client_list
            .value32()
            .is_some_and(|mut values| values.all(|xid| xid != popup))
    );
    let parent_geometry_after = client
        .get_geometry(window)
        .expect("query parent geometry after popup")
        .reply()
        .expect("parent geometry after popup reply");
    assert_eq!(
        (
            parent_geometry_before.x,
            parent_geometry_before.y,
            parent_geometry_before.width,
            parent_geometry_before.height
        ),
        (
            parent_geometry_after.x,
            parent_geometry_after.y,
            parent_geometry_after.width,
            parent_geometry_after.height
        ),
    );
    client.unmap_window(popup).expect("unmap X11 popup window");
    client.flush().expect("flush X11 popup unmap");

    service
        .emergency_cleanup(&mut supervisor)
        .expect("clean up native reactor XWayland");
    sync_sources(&mut event_loop, &mut service, &mut tokens)
        .expect("retire native reactor sources");
    running.store(false, Ordering::Relaxed);
    let _ = server_thread.join().expect("join compositor test thread");
    drop(client);
    drop(service);
    drop(supervisor);
}
