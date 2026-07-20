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
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let server_thread = thread::spawn(move || {
        while server_running.load(Ordering::Relaxed) {
            let _ = compositor.tick();
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
                    ready_snapshot = Some(snapshot);
                }
                _ => {}
            }
        }
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
