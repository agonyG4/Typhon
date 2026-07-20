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

use x11rb::protocol::xproto::ConnectionExt;

use super::super::{XwaylandGeneration, xwm::startup::XwmStartup};

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
fn installed_xwayland_listener_reaches_running_through_real_xwm_startup() {
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
    let xwm = loop {
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

    drop(client);
    drop(xwm);
    drop(startup);
    drop(xwayland);
}
