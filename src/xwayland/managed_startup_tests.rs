use std::{
    io,
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::{net::UnixListener, net::UnixStream, process::CommandExt},
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use x11rb::{
    connection::Connection,
    protocol::xproto::{AtomEnum, ConnectionExt, CreateWindowAux, PropMode, WindowClass},
    wrapper::ConnectionExt as WrapperConnectionExt,
};

use super::super::{
    XwaylandAssociationEvent, XwaylandGeneration,
    xwm::{XwmCommand, XwmEvent, startup::XwmStartup},
};

struct XwaylandChild {
    process: Child,
    _listener: UnixListener,
    socket_path: PathBuf,
}

impl Drop for XwaylandChild {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn probe_xdpyinfo(display: &str, timeout: Duration) -> io::Result<bool> {
    let mut child = Command::new("xdpyinfo")
        .env("DISPLAY", display)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status.success());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn poll_xwm(fd: i32, timeout_ms: i32) -> io::Result<libc::c_short> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN | libc::POLLOUT,
        revents: 0,
    };
    // SAFETY: `pollfd` points to one live descriptor owned by XwmStartup.
    let result = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(pollfd.revents)
    }
}

fn spawn_xwayland(display_number: u32, wm_child: UnixStream) -> io::Result<XwaylandChild> {
    let socket_path = PathBuf::from(format!("/tmp/.X11-unix/X{display_number}"));
    let listener = UnixListener::bind(&socket_path)?;
    let listen_fd = listener.as_raw_fd();
    let mut command = Command::new("Xwayland");
    command
        .args([
            &format!(":{display_number}"),
            "-rootless",
            "-terminate",
            "-nolisten",
            "tcp",
            "-wm",
            "0",
            "-listenfd",
            "3",
        ])
        .env_remove("DISPLAY")
        .env_remove("XAUTHORITY")
        .env_remove("XWAYLAND_NO_GLAMOR")
        .stdin(Stdio::from(OwnedFd::from(wm_child)))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Xwayland inherits the isolated client listener. The other end of the
    // WM socket is owned by the production XwmStartup below.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(listen_fd, 3) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let process = command.spawn()?;
    Ok(XwaylandChild {
        process,
        _listener: listener,
        socket_path,
    })
}

#[test]
fn installed_xwayland_transport_reaches_running_through_direct_xwm_poll() {
    let Some(_wayland_display) = std::env::var_os("WAYLAND_DISPLAY") else {
        return;
    };
    let Some(_runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") else {
        return;
    };
    if Command::new("Xwayland").arg("-version").output().is_err()
        || Command::new("xdpyinfo").arg("-version").output().is_err()
    {
        return;
    }

    let (wm_parent, wm_child) = UnixStream::pair().expect("create XWM socket pair");
    let Some(display_number) = (210..260).find(|display_number| {
        !PathBuf::from(format!("/tmp/.X11-unix/X{display_number}")).exists()
    }) else {
        return;
    };
    let xwayland = match spawn_xwayland(display_number, wm_child) {
        Ok(child) => child,
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => return,
        Err(error) => panic!("installed Xwayland should spawn: {error}"),
    };
    let display = format!(":{display_number}");
    assert!(
        !probe_xdpyinfo(&display, Duration::from_millis(300)).expect("probe before startup"),
        "X11 listener became usable before Typhon's XwmStartup completed"
    );

    let generation = XwaylandGeneration::new(std::num::NonZeroU64::new(1).unwrap());
    let mut startup = XwmStartup::new(generation, wm_parent).expect("create XwmStartup");
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut seen_states = Vec::new();
    let mut xwm = loop {
        assert!(
            Instant::now() < deadline,
            "XwmStartup did not reach Running"
        );
        let fd = startup.raw_fd().expect("startup descriptor");
        let revents = poll_xwm(fd, 100).expect("poll XWM descriptor");
        let readable = revents & libc::POLLIN != 0;
        let writable = revents & libc::POLLOUT != 0;
        if !readable && !writable {
            continue;
        }
        seen_states.push(startup.state());
        if startup.state() == super::super::xwm::startup::XwmStartupState::ManagerMessagePending {
            assert!(
                seen_states
                    .contains(&super::super::xwm::startup::XwmStartupState::SelectionVerified)
            );
        }
        if writable {
            let _ = startup
                .flush_output()
                .expect("XwmStartup writable flush should be nonblocking");
        }
        if let Some(xwm) = startup.progress().expect("XwmStartup should fail locally") {
            break xwm;
        }
    };

    assert!(seen_states.contains(&super::super::xwm::startup::XwmStartupState::SelectionPending));
    assert!(seen_states.contains(&super::super::xwm::startup::XwmStartupState::SelectionVerified));
    assert!(
        seen_states.contains(&super::super::xwm::startup::XwmStartupState::ManagerMessagePending)
    );

    let (client, screen) = x11rb::connect(Some(&display)).expect("connect X11 client listener");
    let wm_s0 = client
        .intern_atom(false, b"WM_S0")
        .expect("intern WM_S0")
        .reply()
        .expect("WM_S0 reply")
        .atom;
    let owner = client
        .get_selection_owner(wm_s0)
        .expect("query WM_S0 owner")
        .reply()
        .expect("WM_S0 owner reply")
        .owner;
    assert_eq!(owner, xwm.supporting_wm_check());
    assert_eq!(screen, 0);
    assert!(probe_xdpyinfo(&display, Duration::from_secs(2)).expect("probe after startup"));

    let root = client.setup().roots[screen].root;
    let window = client.generate_id().expect("allocate test X11 window");
    client
        .create_window(
            client.setup().roots[screen].root_depth,
            window,
            root,
            100,
            100,
            420,
            180,
            0,
            WindowClass::INPUT_OUTPUT,
            client.setup().roots[screen].root_visual,
            &CreateWindowAux::new(),
        )
        .expect("create managed X11 window");
    let wm_name = client
        .intern_atom(false, b"WM_NAME")
        .expect("intern WM_NAME")
        .reply()
        .expect("WM_NAME reply")
        .atom;
    client
        .change_property8(
            PropMode::REPLACE,
            window,
            wm_name,
            AtomEnum::STRING,
            b"Typhon production-driver MapNotify regression",
        )
        .expect("set test window title");
    client
        .map_window(window)
        .expect("request managed X11 window map");
    client.flush().expect("flush managed X11 window");
    client
        .get_geometry(window)
        .expect("query managed X11 window geometry")
        .reply()
        .expect("managed X11 window geometry reply");
    client
        .get_window_attributes(root)
        .expect("query X11 root attributes")
        .reply()
        .expect("X11 root attributes reply");

    let event_deadline = Instant::now() + Duration::from_secs(10);
    let mut map_handle = None;
    let mut association_injected = false;
    let mut ready_snapshot = None;
    let mut drained_events = 0usize;
    while Instant::now() < event_deadline {
        let revents = poll_xwm(xwm.raw_fd(), 100).expect("poll running XWM");
        if revents & libc::POLLOUT != 0 {
            let _ = xwm
                .flush_output()
                .expect("running XWM writable flush should be nonblocking");
        }
        if revents & libc::POLLIN != 0 {
            drained_events = drained_events.saturating_add(
                xwm.drain_events(256)
                    .expect("running XWM event drain should succeed")
                    .processed as usize,
            );
        }
        let events = xwm.take_events().collect::<Vec<_>>();
        for event in events {
            match event {
                XwmEvent::WindowMapRequested(handle) if handle.xid() == window => {
                    xwm.execute(XwmCommand::Map(handle))
                        .expect("production XWM map command");
                    xwm.flush().expect("flush production XWM map command");
                    map_handle = Some(handle);
                }
                XwmEvent::WindowReady(snapshot) if snapshot.handle.xid() == window => {
                    ready_snapshot = Some(snapshot);
                }
                _ => {}
            }
        }
        if let Some(handle) = map_handle
            && !association_injected
        {
            let serial = std::num::NonZeroU64::new(0xfeed_beef).expect("association serial");
            xwm.note_x11_surface_serial(handle, serial.get() as u32, (serial.get() >> 32) as u32)
                .expect("record X11 surface serial");
            xwm.ingest_wayland_association(XwaylandAssociationEvent::Committed {
                generation,
                serial,
                surface_id: 77,
            })
            .expect("record Wayland surface association");
            xwm.mark_surface_buffer_ready(generation, 77)
                .expect("record first surface buffer");
            association_injected = true;
        }
        if ready_snapshot.is_some() {
            break;
        }
    }
    let snapshot = ready_snapshot.unwrap_or_else(|| {
        let attributes = client
            .get_window_attributes(window)
            .expect("query failed managed X11 window attributes")
            .reply()
            .expect("failed managed X11 window attributes reply");
        panic!(
            "real XWM event path should emit WindowReady; drained_events={drained_events} window_count={} map_state={:?}",
            xwm.window_count(),
            attributes.map_state,
        );
    });
    assert_eq!(snapshot.handle.xid(), window);
    assert_eq!(snapshot.surface_id, 77);
    xwm.execute(XwmCommand::SyncClientLists {
        client_list: vec![snapshot.handle],
        stacking: vec![snapshot.handle],
    })
    .expect("publish real X11 client list");
    xwm.flush().expect("flush real X11 client list");
    let client_list_atom = client
        .intern_atom(false, b"_NET_CLIENT_LIST")
        .expect("intern client list atom")
        .reply()
        .expect("client list atom reply")
        .atom;
    let client_list = client
        .get_property(false, root, client_list_atom, AtomEnum::WINDOW, 0, 1024)
        .expect("query real client list")
        .reply()
        .expect("real client list reply");
    assert!(
        client_list
            .value32()
            .is_some_and(|mut values| values.any(|xid| xid == window))
    );

    drop(client);
    drop(xwm);
    drop(startup);
    drop(xwayland);
}
