use std::{
    env, fs,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::{
        fs::MetadataExt,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
};

use serde_json::json;

use crate::control::{ControlRequest, ControlResponse, encode_request};

use super::{
    control::{ControlRuntimePaths, NativeControlServer, peer_uid_matches},
    event_loop::*,
};

#[test]
fn control_runtime_path_uses_the_instance_name_without_traversal() {
    let runtime_dir = PathBuf::from(env::var_os("XDG_RUNTIME_DIR").expect("runtime directory"));
    let paths = ControlRuntimePaths::for_runtime_dir(&runtime_dir, "oblivion-one-0").unwrap();

    assert_eq!(
        paths.socket_path(),
        runtime_dir
            .join("astrea")
            .join("typhon")
            .join("oblivion-one-0")
            .join("control.sock")
    );
}

#[test]
fn control_runtime_path_rejects_unsafe_instance_names() {
    let runtime_dir = PathBuf::from(env::var_os("XDG_RUNTIME_DIR").expect("runtime directory"));
    for instance in ["", "/", "\\", "..", "a/b", "a\\b", "a\0b", "a\n"] {
        assert!(
            ControlRuntimePaths::for_runtime_dir(&runtime_dir, instance).is_err(),
            "instance {instance:?} should be rejected"
        );
    }
}

#[test]
fn control_peer_policy_requires_the_effective_uid() {
    assert!(peer_uid_matches(1000, 1000));
    assert!(!peer_uid_matches(1001, 1000));
}

fn test_instance(suffix: &str) -> String {
    format!("m2-{suffix}-{}", std::process::id())
}

#[test]
fn control_readiness_preserves_a_generation_safe_token_and_flags() {
    let fd = unsafe {
        // SAFETY: `eventfd` is called with valid flags and its returned file
        // descriptor is immediately wrapped as the sole owner.
        libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK)
    };
    assert!(fd >= 0);
    let source = unsafe {
        // SAFETY: the preceding `eventfd` call returned a valid descriptor.
        OwnedFd::from_raw_fd(fd)
    };
    let mut event_loop = NativeEventLoop::new().unwrap();
    let token = event_loop
        .register(source.as_raw_fd(), NativeEventSource::ControlClient)
        .unwrap();
    let value = 1u64;
    let written = unsafe {
        // SAFETY: `source` is a live eventfd and `value` points to one
        // initialized eventfd-sized value.
        libc::write(
            source.as_raw_fd(),
            (&value as *const u64).cast(),
            std::mem::size_of::<u64>(),
        )
    };
    assert_eq!(written as usize, std::mem::size_of::<u64>());

    let wakeup = event_loop.wait().unwrap();

    assert!(wakeup.reasons.control());
    assert_eq!(wakeup.control_events.len(), 1);
    assert_eq!(wakeup.control_events[0].token, token);
    assert_ne!(wakeup.control_events[0].flags & libc::EPOLLIN as u32, 0);
}

#[test]
fn control_server_decodes_a_request_split_across_readiness_cycles() {
    let runtime_dir = PathBuf::from(env::var_os("XDG_RUNTIME_DIR").expect("runtime directory"));
    let instance = test_instance("split");
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut server = NativeControlServer::bind(&mut event_loop, &runtime_dir, &instance).unwrap();
    let socket_path = server.socket_path().to_path_buf();
    assert_eq!(
        fs::symlink_metadata(&socket_path).unwrap().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::symlink_metadata(socket_path.parent().unwrap())
            .unwrap()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::symlink_metadata(socket_path.parent().unwrap().parent().unwrap())
            .unwrap()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::symlink_metadata(
            socket_path
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
        )
        .unwrap()
        .mode()
            & 0o777,
        0o700
    );
    let mut client = UnixStream::connect(server.socket_path()).unwrap();
    client.set_nonblocking(false).unwrap();
    let request = encode_request(&ControlRequest::new(7, "status", json!({})).unwrap()).unwrap();
    let split = request.len() / 2;
    client.write_all(&request[..split]).unwrap();

    let first_wakeup = event_loop.wait().unwrap();
    let first_pending = server
        .service_events(&mut event_loop, &first_wakeup.control_events, 16)
        .unwrap();
    assert!(first_pending.is_empty());

    client.write_all(&request[split..]).unwrap();
    let second_wakeup = event_loop.wait().unwrap();
    let pending = server
        .service_events(&mut event_loop, &second_wakeup.control_events, 16)
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].1,
        ControlRequest::new(7, "status", json!({})).unwrap()
    );

    server
        .queue_response(
            &mut event_loop,
            pending[0].0,
            ControlResponse::success(7, json!({})),
        )
        .unwrap();
    let write_wakeup = event_loop.wait().unwrap();
    server
        .service_events(&mut event_loop, &write_wakeup.control_events, 16)
        .unwrap();

    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    assert_eq!(
        response,
        "{\"protocol\":\"astrea.control\",\"version\":1,\"id\":7,\"ok\":true,\"result\":{}}\n"
    );
    server.shutdown(&mut event_loop).unwrap();
}

#[test]
fn live_control_socket_is_not_replaced_and_stale_socket_is_reclaimed() {
    let runtime_dir = PathBuf::from(env::var_os("XDG_RUNTIME_DIR").expect("runtime directory"));
    let instance = test_instance("stale");
    let mut first_loop = NativeEventLoop::new().unwrap();
    let mut first = NativeControlServer::bind(&mut first_loop, &runtime_dir, &instance).unwrap();

    let mut second_loop = NativeEventLoop::new().unwrap();
    let error = NativeControlServer::bind(&mut second_loop, &runtime_dir, &instance).unwrap_err();
    assert!(matches!(
        error,
        super::control::ControlServerError::SocketInUse(_)
    ));
    first.shutdown(&mut first_loop).unwrap();

    let socket_path = runtime_dir
        .join("astrea")
        .join("typhon")
        .join(&instance)
        .join("control.sock");
    let stale = UnixListener::bind(&socket_path).unwrap();
    drop(stale);
    let mut replacement_loop = NativeEventLoop::new().unwrap();
    let mut replacement =
        NativeControlServer::bind(&mut replacement_loop, &runtime_dir, &instance).unwrap();
    assert_eq!(replacement.socket_path(), socket_path);
    replacement.shutdown(&mut replacement_loop).unwrap();
}

#[test]
fn control_listener_never_services_more_than_sixteen_accept_operations() {
    let runtime_dir = PathBuf::from(env::var_os("XDG_RUNTIME_DIR").expect("runtime directory"));
    let instance = test_instance("budget");
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut server = NativeControlServer::bind(&mut event_loop, &runtime_dir, &instance).unwrap();
    let clients = (0..32)
        .map(|_| UnixStream::connect(server.socket_path()).unwrap())
        .collect::<Vec<_>>();

    let mut wakeup = event_loop.wait().unwrap();
    let listener_event = wakeup
        .control_events
        .iter()
        .copied()
        .find(|event| event.token == server.listener_token())
        .unwrap();
    let repeated = vec![listener_event; 32];
    server
        .service_events(&mut event_loop, &repeated, 16)
        .unwrap();
    assert_eq!(server.client_count(), 16);

    for _ in 0..16 {
        wakeup = event_loop.wait().unwrap();
        server
            .service_events(&mut event_loop, &wakeup.control_events, 16)
            .unwrap();
    }
    assert_eq!(server.client_count(), 32);
    drop(clients);
    server.shutdown(&mut event_loop).unwrap();
}

#[test]
fn socket_symlink_is_rejected_without_unlinking_the_target() {
    let runtime_dir = PathBuf::from(env::var_os("XDG_RUNTIME_DIR").expect("runtime directory"));
    let instance = test_instance("symlink");
    let mut setup_loop = NativeEventLoop::new().unwrap();
    let mut setup = NativeControlServer::bind(&mut setup_loop, &runtime_dir, &instance).unwrap();
    let socket_path = setup.socket_path().to_path_buf();
    setup.shutdown(&mut setup_loop).unwrap();
    std::os::unix::fs::symlink("/dev/null", &socket_path).unwrap();

    let mut event_loop = NativeEventLoop::new().unwrap();
    let error = NativeControlServer::bind(&mut event_loop, &runtime_dir, &instance).unwrap_err();

    assert!(matches!(
        error,
        super::control::ControlServerError::UnsafePath(_)
    ));
    assert!(
        fs::symlink_metadata(&socket_path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    fs::remove_file(socket_path).unwrap();
}

#[test]
fn malformed_request_gets_one_bounded_error_response() {
    let runtime_dir = PathBuf::from(env::var_os("XDG_RUNTIME_DIR").expect("runtime directory"));
    let instance = test_instance("malformed");
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut server = NativeControlServer::bind(&mut event_loop, &runtime_dir, &instance).unwrap();
    let mut client = UnixStream::connect(server.socket_path()).unwrap();
    client.write_all(b"{\n").unwrap();

    let wakeup = event_loop.wait().unwrap();
    assert!(
        server
            .service_events(&mut event_loop, &wakeup.control_events, 16)
            .unwrap()
            .is_empty()
    );
    let write_wakeup = event_loop.wait().unwrap();
    server
        .service_events(&mut event_loop, &write_wakeup.control_events, 16)
        .unwrap();
    let response_wakeup = event_loop.wait().unwrap();
    server
        .service_events(&mut event_loop, &response_wakeup.control_events, 16)
        .unwrap();

    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    assert!(response.contains("\"code\":\"malformed_json\""));
    server.shutdown(&mut event_loop).unwrap();
}

#[test]
fn control_client_hangup_is_delivered_without_failing_the_reactor() {
    let mut pipe = [0; 2];
    assert_eq!(
        unsafe {
            // SAFETY: `pipe` points to two writable descriptor slots and the
            // flags request nonblocking close-on-exec descriptors.
            libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK)
        },
        0
    );
    let read = unsafe {
        // SAFETY: pipe2 returned a valid owned read descriptor.
        OwnedFd::from_raw_fd(pipe[0])
    };
    let write = unsafe {
        // SAFETY: pipe2 returned a valid owned write descriptor.
        OwnedFd::from_raw_fd(pipe[1])
    };
    let mut event_loop = NativeEventLoop::new().unwrap();
    let token = event_loop
        .register(read.as_raw_fd(), NativeEventSource::ControlClient)
        .unwrap();
    drop(write);

    let wakeup = event_loop.wait().unwrap();

    assert!(wakeup.reasons.control());
    assert_eq!(wakeup.control_events[0].token, token);
    assert_ne!(wakeup.control_events[0].flags & libc::EPOLLHUP as u32, 0);
}
