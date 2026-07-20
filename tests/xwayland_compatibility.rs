use std::os::unix::process::CommandExt;
use std::{
    fs, io,
    os::{fd::AsRawFd, unix::net::UnixListener, unix::net::UnixStream},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use x11rb::{
    CURRENT_TIME,
    connection::Connection,
    protocol::xproto::{
        ClientMessageData, ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask,
        WindowClass,
    },
    rust_connection::{DefaultStream, RustConnection},
};

#[test]
fn session_inspector_uses_explicit_values_or_unconnected_status() {
    let source =
        std::fs::read_to_string("bin/check-xwayland-session").expect("session inspector source");
    assert!(!source.contains("reported by Typhon"));
    assert!(source.contains("unsupported/not connected"));
}

#[test]
fn diagnostic_script_handles_unset_display() {
    let output = Command::new("./bin/check-xwayland-session")
        .env_remove("DISPLAY")
        .env_remove("XAUTHORITY")
        .output()
        .expect("session inspector should start without X variables");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("DISPLAY=unsupported/not connected"));
    assert!(stdout.contains("XAUTHORITY=unsupported/not connected"));
    assert!(stdout.contains("filesystem_socket=unsupported/not connected"));
    assert!(stdout.contains("abstract_socket=unsupported/not connected"));
}

struct XdpyinfoProbe {
    completed: bool,
    success: bool,
}

fn probe_xdpyinfo(display: &str, timeout: Duration) -> io::Result<XdpyinfoProbe> {
    let mut child = Command::new("xdpyinfo")
        .env("DISPLAY", display)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(XdpyinfoProbe {
                completed: true,
                success: status.success(),
            });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(XdpyinfoProbe {
                completed: false,
                success: false,
            });
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn installed_xwayland_listener_contract_after_external_wm_s0_claim() {
    let Some(wayland_display) = std::env::var_os("WAYLAND_DISPLAY") else {
        return;
    };
    let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") else {
        return;
    };
    if Command::new("Xwayland").arg("-version").output().is_err()
        || Command::new("xdpyinfo").arg("-version").output().is_err()
    {
        return;
    }

    let (wm_parent, wm_child) = UnixStream::pair().expect("create XWM socket pair");
    let Some((display_number, socket_path, listener)) = (90..200).find_map(|display_number| {
        let socket_path = PathBuf::from(format!("/tmp/.X11-unix/X{display_number}"));
        match UnixListener::bind(&socket_path) {
            Ok(listener) => Some((display_number, socket_path, listener)),
            Err(_) => None,
        }
    }) else {
        return;
    };
    let listen_fd = listener.as_raw_fd();
    let mut xwayland = Command::new("Xwayland");
    xwayland
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
        .env("WAYLAND_DISPLAY", &wayland_display)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env_remove("DISPLAY")
        .env_remove("XAUTHORITY")
        .env_remove("XWAYLAND_NO_GLAMOR")
        .stdin(Stdio::from(std::os::fd::OwnedFd::from(wm_child)))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Xwayland inherits the compositor-created filesystem listener. Keeping
    // the listener outside Xwayland makes this a real client-listener test,
    // rather than accidentally probing an unrelated DISPLAY.
    unsafe {
        xwayland.pre_exec(move || {
            if libc::dup2(listen_fd, 3) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut xwayland = xwayland.spawn().expect("installed Xwayland should spawn");
    let display = format!(":{display_number}");

    let before = probe_xdpyinfo(&display, Duration::from_millis(500))
        .expect("xdpyinfo before WM_S0 claim should spawn");
    if before.success {
        let _ = xwayland.kill();
        let _ = xwayland.wait();
        drop(listener);
        let _ = fs::remove_file(&socket_path);
        panic!(
            "X11 listener became usable before WM_S0 verification on {display} (completed={})",
            before.completed,
        );
    }

    let (wm_stream, _) =
        DefaultStream::from_unix_stream(wm_parent).expect("wrap XWM connection stream");
    let wm = RustConnection::connect_to_stream(wm_stream, 0)
        .expect("XWM connection setup should complete");
    let root = wm.setup().roots[0].root;
    let wm_s0 = wm
        .intern_atom(false, b"WM_S0")
        .expect("intern WM_S0")
        .reply()
        .expect("WM_S0 reply")
        .atom;
    let manager = wm
        .intern_atom(false, b"MANAGER")
        .expect("intern MANAGER")
        .reply()
        .expect("MANAGER reply")
        .atom;
    let supporting = wm.generate_id().expect("supporting window id");
    wm.create_window(
        0,
        supporting,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new(),
    )
    .expect("create supporting window")
    .check()
    .expect("supporting window creation should succeed");
    wm.set_selection_owner(supporting, wm_s0, CURRENT_TIME)
        .expect("claim WM_S0")
        .check()
        .expect("WM_S0 claim should succeed");
    let owner = wm
        .get_selection_owner(wm_s0)
        .expect("query WM_S0 owner")
        .reply()
        .expect("WM_S0 owner reply")
        .owner;
    assert_eq!(
        owner, supporting,
        "WM_S0 owner must be the supporting window"
    );
    wm.send_event(
        false,
        root,
        EventMask::STRUCTURE_NOTIFY,
        ClientMessageEvent::new(
            32,
            root,
            manager,
            ClientMessageData::from([CURRENT_TIME, wm_s0, supporting, 0, 0]),
        ),
    )
    .expect("queue MANAGER ClientMessage")
    .check()
    .expect("MANAGER ClientMessage should be sent");
    wm.flush().expect("flush WM_S0 claim");

    let after = probe_xdpyinfo(&display, Duration::from_secs(2))
        .expect("xdpyinfo after WM_S0 claim should spawn");
    let _ = xwayland.kill();
    let _ = xwayland.wait();
    drop(listener);
    let _ = fs::remove_file(&socket_path);
    assert!(
        after.success,
        "X11 listener did not open after WM_S0 verification on {display}"
    );
}

#[test]
#[ignore = "requires a native managed Typhon session and installed X11 clients"]
fn installed_x11_compatibility_smoke() {
    let display = std::env::var("DISPLAY").expect("native test requires DISPLAY");
    let xdpyinfo = Command::new("xdpyinfo")
        .env("DISPLAY", &display)
        .status()
        .expect("xdpyinfo must be installed");
    assert!(xdpyinfo.success(), "xdpyinfo failed on {display}");
    let xmessage = Command::new("xmessage")
        .env("DISPLAY", &display)
        .args(["-timeout", "1", "Typhon XWayland smoke"])
        .status()
        .expect("xmessage must be installed");
    assert!(xmessage.success(), "xmessage failed on {display}");
}

#[test]
#[ignore = "requires a native managed Typhon session"]
fn session_inspector_is_standalone() {
    let status = Command::new("./bin/check-xwayland-session")
        .status()
        .expect("session inspector");
    assert!(status.success());
}
