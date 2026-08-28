use std::{
    fs,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::{
        fs::MetadataExt,
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::json;

use crate::control::{ControlRequest, ControlResponse, encode_request};

use super::{
    control::{
        ControlRuntimePaths, MAX_CONTROL_CLIENTS, MAX_CONTROL_OPERATIONS_PER_CYCLE,
        NativeControlServer, force_temporary_collisions_for_test, peer_uid_matches,
        remove_instance_directory_after_lock_for_test, temporary_socket_path_for_test,
    },
    event_loop::*,
};

static TEST_RUNTIME_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestRuntime(PathBuf);

impl TestRuntime {
    fn new() -> Self {
        let sequence = TEST_RUNTIME_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(format!(
            "/tmp/typhon-control-test-{}-{sequence}",
            std::process::id(),
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestRuntime {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn control_runtime_path_uses_the_instance_name_without_traversal() {
    let runtime = TestRuntime::new();
    let runtime_dir = runtime.path();
    let paths = ControlRuntimePaths::for_runtime_dir(runtime_dir, "oblivion-one-0").unwrap();

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
    let runtime = TestRuntime::new();
    let runtime_dir = runtime.path();
    for instance in ["", "/", "\\", ".", "./", "..", "a/b", "a\\b", "a\0b", "a\n"] {
        assert!(
            ControlRuntimePaths::for_runtime_dir(runtime_dir, instance).is_err(),
            "instance {instance:?} should be rejected"
        );
    }
}

#[test]
fn control_peer_policy_requires_the_effective_uid() {
    assert!(peer_uid_matches(1000, 1000));
    assert!(!peer_uid_matches(1001, 1000));
}

#[test]
fn temporary_listener_path_is_unique_and_private() {
    let runtime = TestRuntime::new();
    let paths = ControlRuntimePaths::for_runtime_dir(runtime.path(), "temporary").unwrap();
    fs::create_dir_all(paths.socket_path().parent().unwrap()).unwrap();
    let first = temporary_socket_path_for_test(paths.socket_path().parent().unwrap()).unwrap();
    let second = temporary_socket_path_for_test(paths.socket_path().parent().unwrap()).unwrap();
    assert_ne!(first, second);
    assert_eq!(first.parent(), paths.socket_path().parent());
    let name = first.file_name().unwrap().to_string_lossy();
    assert!(name.starts_with("control.sock.tmp-") || name.starts_with("t-"));
}

#[test]
fn publication_preserves_the_listener_device_and_inode() {
    let runtime = TestRuntime::new();
    let instance = test_instance("identity");
    let mut event_loop = NativeEventLoop::new().unwrap();
    let server = NativeControlServer::bind(&mut event_loop, runtime.path(), &instance).unwrap();
    let metadata = fs::symlink_metadata(server.socket_path()).unwrap();
    assert_eq!(
        (metadata.dev(), metadata.ino()),
        server.captured_socket_identity()
    );
    assert_eq!(metadata.mode() & 0o777, 0o600);
}

#[test]
fn existing_live_control_socket_is_never_overwritten() {
    let runtime = TestRuntime::new();
    let runtime_dir = runtime.path();
    let instance = test_instance("conflict");
    let paths = ControlRuntimePaths::for_runtime_dir(runtime_dir, &instance).unwrap();
    fs::create_dir_all(paths.socket_path().parent().unwrap()).unwrap();
    let socket_dir = paths.socket_path().parent().unwrap();
    for directory in [
        socket_dir,
        socket_dir.parent().unwrap(),
        socket_dir.parent().unwrap().parent().unwrap(),
    ] {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let existing = UnixListener::bind(paths.socket_path()).unwrap();
    let before = fs::symlink_metadata(paths.socket_path()).unwrap();
    let mut event_loop = NativeEventLoop::new().unwrap();
    let error = NativeControlServer::bind(&mut event_loop, runtime_dir, &instance).unwrap_err();
    assert!(
        matches!(&error, super::control::ControlServerError::SocketInUse(_)),
        "{error:?}"
    );
    let after = fs::symlink_metadata(paths.socket_path()).unwrap();
    assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
    drop(existing);
}

#[test]
fn shutdown_is_idempotent_and_never_removes_a_replacement_socket() {
    let runtime = TestRuntime::new();
    let instance = test_instance("shutdown");
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut server = NativeControlServer::bind(&mut event_loop, runtime.path(), &instance).unwrap();
    let socket_path = server.socket_path().to_path_buf();
    fs::remove_file(&socket_path).unwrap();
    let replacement = UnixListener::bind(&socket_path).unwrap();

    server.shutdown(&mut event_loop).unwrap();
    server.shutdown(&mut event_loop).unwrap();
    assert!(socket_path.exists());
    drop(server);
    assert!(socket_path.exists());
    drop(replacement);
}

#[test]
fn instance_directory_disappearance_fails_closed_and_fresh_bind_reacquires_lock() {
    let runtime = TestRuntime::new();
    let instance = test_instance("recreated");
    let paths = ControlRuntimePaths::for_runtime_dir(runtime.path(), &instance).unwrap();
    let mut failed_loop = NativeEventLoop::new().unwrap();
    remove_instance_directory_after_lock_for_test(paths.socket_dir_for_test());
    let error = NativeControlServer::bind(&mut failed_loop, runtime.path(), &instance).unwrap_err();
    assert!(
        matches!(&error, super::control::ControlServerError::ListenerFailure(message) if message.contains("directory disappeared")),
        "{error:?}"
    );

    let mut fresh_loop = NativeEventLoop::new().unwrap();
    let mut fresh = NativeControlServer::bind(&mut fresh_loop, runtime.path(), &instance).unwrap();
    assert!(fresh.socket_path().exists());
    let mut competing_loop = NativeEventLoop::new().unwrap();
    assert!(matches!(
        NativeControlServer::bind(&mut competing_loop, runtime.path(), &instance),
        Err(super::control::ControlServerError::InstanceLocked(_))
    ));
    fresh.shutdown(&mut fresh_loop).unwrap();
}

#[test]
fn temporary_collision_retries_are_bounded_and_leave_no_endpoint() {
    let runtime = TestRuntime::new();
    let instance = test_instance("collision");
    let paths = ControlRuntimePaths::for_runtime_dir(runtime.path(), &instance).unwrap();
    force_temporary_collisions_for_test(paths.socket_dir_for_test(), 16);
    let mut event_loop = NativeEventLoop::new().unwrap();
    let error = NativeControlServer::bind(&mut event_loop, runtime.path(), &instance).unwrap_err();
    assert!(matches!(
        error,
        super::control::ControlServerError::ListenerFailure(message)
            if message.contains("collision limit")
    ));
    assert!(!paths.socket_path().exists());
    let socket_dir = paths.socket_path().parent().unwrap();
    if socket_dir.exists() {
        assert!(
            fs::read_dir(socket_dir)
                .unwrap()
                .flatten()
                .all(|entry| entry.file_name() != "control.sock")
        );
    }

    let mut fresh_loop = NativeEventLoop::new().unwrap();
    let mut fresh = NativeControlServer::bind(&mut fresh_loop, runtime.path(), &instance).unwrap();
    fresh.shutdown(&mut fresh_loop).unwrap();
}

#[test]
fn different_instance_names_have_independent_locks_and_listeners() {
    let runtime = TestRuntime::new();
    let mut first_loop = NativeEventLoop::new().unwrap();
    let mut first =
        NativeControlServer::bind(&mut first_loop, runtime.path(), "m2-independent-a").unwrap();
    let mut second_loop = NativeEventLoop::new().unwrap();
    let mut second =
        NativeControlServer::bind(&mut second_loop, runtime.path(), "m2-independent-b").unwrap();
    assert_ne!(first.socket_path(), second.socket_path());
    first.shutdown(&mut first_loop).unwrap();
    second.shutdown(&mut second_loop).unwrap();
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
    let runtime = TestRuntime::new();
    let runtime_dir = runtime.path();
    let instance = test_instance("split");
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut server = NativeControlServer::bind(&mut event_loop, runtime_dir, &instance).unwrap();
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
fn complete_request_survives_peer_write_half_close() {
    let runtime = TestRuntime::new();
    let instance = test_instance("half-close");
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut server = NativeControlServer::bind(&mut event_loop, runtime.path(), &instance).unwrap();
    let mut client = UnixStream::connect(server.socket_path()).unwrap();
    let request = encode_request(&ControlRequest::new(11, "status", json!({})).unwrap()).unwrap();
    client.write_all(&request).unwrap();
    client.shutdown(std::net::Shutdown::Write).unwrap();

    let mut pending = Vec::new();
    for _ in 0..4 {
        let wakeup = event_loop.wait().unwrap();
        pending = server
            .service_events(&mut event_loop, &wakeup.control_events, 16)
            .unwrap();
        if !pending.is_empty() {
            break;
        }
    }
    assert_eq!(pending.len(), 1);
    server
        .queue_response(
            &mut event_loop,
            pending[0].0,
            ControlResponse::success(11, json!({})),
        )
        .unwrap();
    let write_wakeup = event_loop.wait().unwrap();
    server
        .service_events(&mut event_loop, &write_wakeup.control_events, 16)
        .unwrap();

    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    assert!(response.contains("\"id\":11"));
    server.shutdown(&mut event_loop).unwrap();
}

#[test]
fn instance_lock_is_retained_until_server_drop() {
    let runtime = TestRuntime::new();
    let instance = test_instance("lock");
    let mut first_loop = NativeEventLoop::new().unwrap();
    let mut first = NativeControlServer::bind(&mut first_loop, runtime.path(), &instance)
        .unwrap_or_else(|error| panic!("runtime={} error={error:?}", runtime.path().display()));
    first.shutdown(&mut first_loop).unwrap();

    let mut second_loop = NativeEventLoop::new().unwrap();
    let second = NativeControlServer::bind(&mut second_loop, runtime.path(), &instance);
    assert!(
        matches!(
            &second,
            Err(super::control::ControlServerError::InstanceLocked(_))
        ),
        "{second:?}"
    );
    drop(first);

    let mut third_loop = NativeEventLoop::new().unwrap();
    let mut third = NativeControlServer::bind(&mut third_loop, runtime.path(), &instance).unwrap();
    assert!(third.socket_path().exists());
    third.shutdown(&mut third_loop).unwrap();
}

#[test]
fn stalled_clients_expire_with_a_bounded_budget() {
    let runtime = TestRuntime::new();
    let instance = test_instance("timeout");
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut server = NativeControlServer::bind(&mut event_loop, runtime.path(), &instance).unwrap();
    let clients = (0..MAX_CONTROL_CLIENTS)
        .map(|_| UnixStream::connect(server.socket_path()).unwrap())
        .collect::<Vec<_>>();
    let wakeup = event_loop.wait().unwrap();
    server
        .service_events(
            &mut event_loop,
            &wakeup.control_events,
            MAX_CONTROL_OPERATIONS_PER_CYCLE,
        )
        .unwrap();
    for _ in 0..64 {
        let wakeup = event_loop.wait().unwrap();
        server
            .service_events(
                &mut event_loop,
                &wakeup.control_events,
                MAX_CONTROL_OPERATIONS_PER_CYCLE,
            )
            .unwrap();
        if server.client_count() == MAX_CONTROL_CLIENTS {
            break;
        }
    }
    assert_eq!(server.client_count(), MAX_CONTROL_CLIENTS);
    let deadline = u64::MAX;
    server.expire_idle_clients(&mut event_loop, deadline, 16);
    assert_eq!(server.client_count(), 16);
    server.expire_idle_clients(&mut event_loop, deadline, MAX_CONTROL_CLIENTS);
    assert_eq!(server.client_count(), 0);
    assert_eq!(
        server.counters().request_timeouts,
        MAX_CONTROL_CLIENTS as u64
    );
    drop(clients);
    server.shutdown(&mut event_loop).unwrap();
}

#[test]
fn live_control_socket_is_not_replaced_and_stale_socket_is_reclaimed() {
    let runtime = TestRuntime::new();
    let runtime_dir = runtime.path();
    let instance = test_instance("stale");
    let mut first_loop = NativeEventLoop::new().unwrap();
    let mut first = NativeControlServer::bind(&mut first_loop, runtime_dir, &instance).unwrap();

    let mut second_loop = NativeEventLoop::new().unwrap();
    let error = NativeControlServer::bind(&mut second_loop, runtime_dir, &instance).unwrap_err();
    assert!(matches!(
        error,
        super::control::ControlServerError::InstanceLocked(_)
    ));
    first.shutdown(&mut first_loop).unwrap();
    drop(first);

    let socket_path = runtime_dir
        .join("astrea")
        .join("typhon")
        .join(&instance)
        .join("control.sock");
    let stale = UnixListener::bind(&socket_path).unwrap();
    drop(stale);
    let mut replacement_loop = NativeEventLoop::new().unwrap();
    let mut replacement =
        NativeControlServer::bind(&mut replacement_loop, runtime_dir, &instance).unwrap();
    assert_eq!(replacement.socket_path(), socket_path);
    replacement.shutdown(&mut replacement_loop).unwrap();
}

#[test]
fn control_listener_never_services_more_than_sixteen_accept_operations() {
    let runtime = TestRuntime::new();
    let runtime_dir = runtime.path();
    let instance = test_instance("budget");
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut server = NativeControlServer::bind(&mut event_loop, runtime_dir, &instance).unwrap();
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
    let runtime = TestRuntime::new();
    let runtime_dir = runtime.path();
    let instance = test_instance("symlink");
    let mut setup_loop = NativeEventLoop::new().unwrap();
    let mut setup = NativeControlServer::bind(&mut setup_loop, runtime_dir, &instance).unwrap();
    let socket_path = setup.socket_path().to_path_buf();
    setup.shutdown(&mut setup_loop).unwrap();
    drop(setup);
    std::os::unix::fs::symlink("/dev/null", &socket_path).unwrap();

    let mut event_loop = NativeEventLoop::new().unwrap();
    let error = NativeControlServer::bind(&mut event_loop, runtime_dir, &instance).unwrap_err();

    assert!(
        matches!(&error, super::control::ControlServerError::UnsafePath(_)),
        "{error:?}"
    );
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
    let runtime = TestRuntime::new();
    let runtime_dir = runtime.path();
    let instance = test_instance("malformed");
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut server = NativeControlServer::bind(&mut event_loop, runtime_dir, &instance).unwrap();
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
