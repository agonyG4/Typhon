use std::{
    fs,
    num::NonZeroU64,
    os::unix::fs::{PermissionsExt, symlink},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use wayland_client::{
    Connection, Dispatch, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::wl_registry,
};

use crate::native::event_loop::{NativeEventLoop, NativeEventSource};
use crate::process::ChildSupervisor;

use super::{
    XwaylandConfig, XwaylandMode, XwaylandReactorPurpose, XwaylandService, XwaylandStateKind,
    auth::read_cookie_for_tests,
    diagnostics::XwaylandFailureStage,
    display::{DisplayLease, connect_abstract_socket_for_tests},
};

#[path = "displayfd_tests.rs"]
mod displayfd_tests;

#[test]
fn xwayland_mode_parses_only_opt_in_values() {
    assert_eq!(XwaylandMode::parse(None), XwaylandMode::Off);
    assert_eq!(XwaylandMode::parse(Some("off")), XwaylandMode::Off);
    assert_eq!(XwaylandMode::parse(Some("base")), XwaylandMode::BaseLazy);
    assert_eq!(XwaylandMode::parse(Some("lazy")), XwaylandMode::ManagedLazy);
    assert_eq!(
        XwaylandMode::parse(Some("eager")),
        XwaylandMode::ManagedEager
    );
    assert_eq!(XwaylandMode::parse(Some("host")), XwaylandMode::Off);
}

#[test]
fn managed_mode_is_the_only_mode_with_a_normal_app_profile() {
    let root = test_root("managed-profile");
    let base = XwaylandService::bootstrap_with_config(XwaylandConfig::for_tests_at_root(
        XwaylandMode::BaseLazy,
        PathBuf::from("Xwayland"),
        root.clone(),
    ))
    .expect("bootstrap base mode");
    assert!(base.normal_app_environment().is_none());

    let mut managed_config = XwaylandConfig::for_tests_at_root(
        XwaylandMode::ManagedLazy,
        PathBuf::from("Xwayland"),
        root.clone(),
    );
    managed_config.profile = super::config::XwaylandProfile::Managed;
    let managed =
        XwaylandService::bootstrap_with_config(managed_config).expect("bootstrap managed mode");
    assert!(managed.normal_app_environment().is_some());

    drop(base);
    drop(managed);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn off_bootstrap_is_disabled_without_lease_or_process() {
    let service = XwaylandService::bootstrap_with_config(XwaylandConfig::for_tests(
        XwaylandMode::Off,
        PathBuf::from("Xwayland"),
    ))
    .expect("bootstrap off mode");

    assert_eq!(service.state_kind(), XwaylandStateKind::Disabled);
    assert!(service.app_environment().is_none());
    assert_eq!(service.reactor_registrations().count(), 0);
}

#[test]
fn generation_allocator_returns_distinct_nonzero_values() {
    let root = test_root("generation");
    let mut service = XwaylandService::bootstrap_with_config(XwaylandConfig::for_tests_at_root(
        XwaylandMode::BaseLazy,
        PathBuf::from("Xwayland"),
        root.clone(),
    ))
    .expect("bootstrap base mode");

    let first = service.allocate_generation().expect("first generation");
    let second = service.allocate_generation().expect("second generation");
    assert_ne!(first, second);
    assert_ne!(first.get(), 0);
    assert_ne!(second.get(), 0);
    drop(service);
    fs::remove_dir_all(root).expect("remove test root");
}

fn test_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "typhon-xwayland-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("create test root");
    root
}

fn display_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("display test lock")
}

#[test]
fn two_display_allocators_claim_different_leases() {
    let _lock = display_test_lock();
    let root = test_root("unique");
    let first = DisplayLease::allocate_for_tests(&root, 0, 1).expect("first lease");
    let second = DisplayLease::allocate_for_tests(&root, 0, 1).expect("second lease");

    assert_ne!(first.display_number(), second.display_number());
    drop(second);
    drop(first);
    assert!(!root.join(".X0-lock").exists());
    assert!(!root.join(".X1-lock").exists());
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn live_lock_is_skipped() {
    let _lock = display_test_lock();
    let root = test_root("live-lock");
    fs::write(root.join(".X0-lock"), format!("{}\n", std::process::id())).expect("write live lock");

    let lease = DisplayLease::allocate_for_tests(&root, 0, 1).expect("allocate after live lock");
    assert_eq!(lease.display_number(), 1);
    drop(lease);
    fs::remove_file(root.join(".X0-lock")).expect("remove simulated lock");
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn nul_padded_live_lock_is_skipped() {
    let _lock = display_test_lock();
    let root = test_root("live-lock-nul");
    fs::write(
        root.join(".X0-lock"),
        format!("{:010}\0", std::process::id()),
    )
    .expect("write NUL-padded live lock");

    let lease = DisplayLease::allocate_for_tests(&root, 0, 1).expect("allocate after live lock");
    assert_eq!(lease.display_number(), 1);
    drop(lease);
    fs::remove_file(root.join(".X0-lock")).expect("remove simulated lock");
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn stale_lock_is_recovered() {
    let _lock = display_test_lock();
    let root = test_root("stale-lock");
    fs::write(root.join(".X0-lock"), "2147483647\n").expect("write stale lock");

    let lease = DisplayLease::allocate_for_tests(&root, 0, 0).expect("recover stale lock");
    assert_eq!(lease.display_number(), 0);
    drop(lease);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn symlink_lock_is_rejected_without_following_target() {
    let _lock = display_test_lock();
    let root = test_root("symlink-lock");
    let target = root.join("target");
    fs::write(&target, "do not remove").expect("write target");
    symlink(&target, root.join(".X0-lock")).expect("create lock symlink");

    assert!(DisplayLease::allocate_for_tests(&root, 0, 0).is_err());
    assert_eq!(
        fs::read_to_string(target).expect("read target"),
        "do not remove"
    );
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn unsafe_first_display_slot_does_not_hide_safe_later_slot() {
    let _lock = display_test_lock();
    let root = test_root("unsafe-first-slot");
    let target = root.join("target");
    fs::write(&target, "keep").expect("write target");
    symlink(&target, root.join(".X0-lock")).expect("create lock symlink");

    let lease = DisplayLease::allocate_for_tests(&root, 0, 1).expect("use safe later slot");
    assert_eq!(lease.display_number(), 1);
    drop(lease);
    assert_eq!(fs::read_to_string(target).expect("read target"), "keep");
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn stale_authority_file_does_not_block_new_display_lease() {
    let _lock = display_test_lock();
    let root = test_root("stale-authority");
    let auth_dir = root.join("typhon/xwayland");
    fs::create_dir_all(&auth_dir).expect("create auth directory");
    fs::write(auth_dir.join(".Xauthority-0"), b"stale").expect("write stale authority");

    let lease = DisplayLease::allocate_for_tests(&root, 0, 0).expect("ignore stale authority");
    assert_eq!(lease.display_number(), 0);
    assert!(auth_dir.join(".Xauthority-0").exists());
    drop(lease);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn partial_lease_failure_rolls_back_lock_and_auth_artifacts() {
    let _lock = display_test_lock();
    let root = test_root("rollback");
    let socket_path = root.join(".X11-unix/X0");
    fs::create_dir_all(socket_path.parent().expect("socket parent")).expect("create socket dir");
    symlink(&root, &socket_path).expect("create socket symlink");

    assert!(DisplayLease::allocate_for_tests(&root, 0, 0).is_err());
    assert!(!root.join(".X0-lock").exists());
    let auth_dir = root.join("typhon/xwayland");
    assert!(
        !auth_dir.exists()
            || !auth_dir
                .read_dir()
                .expect("read auth directory")
                .any(|entry| entry
                    .expect("read auth entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".Xauthority-0-"))
    );
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn filesystem_and_abstract_sockets_accept_local_connections() {
    let _lock = display_test_lock();
    let root = test_root("sockets");
    let lease = DisplayLease::allocate_for_tests(&root, 0, 0).expect("allocate lease");
    let mode = fs::metadata(lease.filesystem_socket_path())
        .expect("stat filesystem socket")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o666);
    std::os::unix::net::UnixStream::connect(lease.filesystem_socket_path())
        .expect("connect filesystem socket");
    connect_abstract_socket_for_tests(&root.join(".X11-unix"), lease.display_number())
        .expect("connect abstract socket");
    drop(lease);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn auth_file_is_private_and_contains_a_cookie() {
    let _lock = display_test_lock();
    let root = test_root("auth");
    let lease = DisplayLease::allocate_for_tests(&root, 0, 0).expect("allocate lease");
    let metadata = fs::metadata(lease.xauthority_path()).expect("stat auth file");
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    let cookie = read_cookie_for_tests(lease.xauthority_path()).expect("parse auth record");
    assert!(cookie.len() >= 16);
    drop(lease);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn display_lease_debug_never_contains_cookie_bytes() {
    let _lock = display_test_lock();
    let root = test_root("auth-debug");
    let lease = DisplayLease::allocate_for_tests(&root, 0, 0).expect("allocate lease");
    let cookie = read_cookie_for_tests(lease.xauthority_path()).expect("parse auth record");
    let cookie_hex = cookie
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert!(!format!("{lease:?}").contains(&cookie_hex));
    drop(lease);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn dropping_lease_removes_only_owned_artifacts() {
    let _lock = display_test_lock();
    let root = test_root("drop");
    let unrelated = root.join("unrelated");
    fs::write(&unrelated, "keep").expect("write unrelated file");
    let lease = DisplayLease::allocate_for_tests(&root, 0, 0).expect("allocate lease");
    let lock = lease.lock_path().to_owned();
    let auth = lease.xauthority_path().to_owned();
    drop(lease);

    assert!(!lock.exists());
    assert!(!auth.exists());
    assert_eq!(
        fs::read_to_string(unrelated).expect("read unrelated"),
        "keep"
    );
    fs::remove_dir_all(root).expect("remove test root");
}

fn service_at_root(
    mode: XwaylandMode,
    binary: &str,
) -> (PathBuf, XwaylandService, ChildSupervisor) {
    let root = test_root("service");
    let mut config = XwaylandConfig::for_tests_at_root(mode, PathBuf::from(binary), root.clone());
    config.display_min = 1;
    let service = XwaylandService::bootstrap_with_config(config).expect("bootstrap service");
    (root, service, ChildSupervisor::new())
}

fn service_at_root_with_sleeping_binary(
    mode: XwaylandMode,
    label: &str,
) -> (PathBuf, XwaylandService, ChildSupervisor) {
    let root = test_root(label);
    let binary = root.join("xwayland-test-binary");
    fs::write(&binary, "#!/bin/sh\nexec /bin/sleep 30\n").expect("write sleeping test binary");
    let mut permissions = fs::metadata(&binary)
        .expect("stat sleeping test binary")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&binary, permissions).expect("make sleeping test binary executable");
    let mut config = XwaylandConfig::for_tests_at_root(mode, binary, root.clone());
    config.display_min = 1;
    let service = XwaylandService::bootstrap_with_config(config).expect("bootstrap service");
    (root, service, ChildSupervisor::new())
}

fn service_at_root_with_stderr_binary() -> (PathBuf, XwaylandService, ChildSupervisor) {
    let root = test_root("stderr");
    let binary = root.join("xwayland-test-binary");
    fs::write(
        &binary,
        "#!/bin/sh\nprintf 'xwayland diagnostic\\n' >&2\nexec /bin/sleep 30\n",
    )
    .expect("write stderr test binary");
    let mut permissions = fs::metadata(&binary)
        .expect("stat stderr test binary")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&binary, permissions).expect("make stderr test binary executable");
    let mut config =
        XwaylandConfig::for_tests_at_root(XwaylandMode::BaseLazy, binary, root.clone());
    config.display_min = 1;
    config.log_stderr = true;
    let service = XwaylandService::bootstrap_with_config(config).expect("bootstrap service");
    (root, service, ChildSupervisor::new())
}

fn service_at_root_with_displayfd_writer(
    mode: XwaylandMode,
    label: &str,
    close_after_write: bool,
) -> (PathBuf, XwaylandService, ChildSupervisor) {
    let root = test_root(label);
    let binary = root.join("xwayland-test-binary");
    let tail = if close_after_write {
        "exit 0"
    } else {
        "exec /bin/sleep 30"
    };
    fs::write(
        &binary,
        format!("#!/bin/sh\nprintf '%s\\n' \"${{1#:}}\" >&5\n{tail}\n"),
    )
    .expect("write displayfd test binary");
    let mut permissions = fs::metadata(&binary)
        .expect("stat displayfd test binary")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&binary, permissions).expect("make displayfd test binary executable");
    let mut config = XwaylandConfig::for_tests_at_root(mode, binary, root.clone());
    config.display_min = 1;
    let service = XwaylandService::bootstrap_with_config(config).expect("bootstrap service");
    (root, service, ChildSupervisor::new())
}

fn service_at_root_with_delayed_displayfd_writer(
    label: &str,
    close_after_write: bool,
) -> (PathBuf, XwaylandService, ChildSupervisor) {
    let root = test_root(label);
    let binary = root.join("xwayland-test-binary");
    let tail = if close_after_write {
        "exit 0"
    } else {
        "exec /bin/sleep 30"
    };
    fs::write(
        &binary,
        format!("#!/bin/sh\nsleep 0.05\nprintf '%s\\n' \"${{1#:}}\" >&5\n{tail}\n"),
    )
    .expect("write delayed displayfd test binary");
    let mut permissions = fs::metadata(&binary)
        .expect("stat delayed displayfd test binary")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&binary, permissions)
        .expect("make delayed displayfd test binary executable");
    let mut config =
        XwaylandConfig::for_tests_at_root(XwaylandMode::BaseLazy, binary, root.clone());
    config.display_min = 1;
    let service = XwaylandService::bootstrap_with_config(config).expect("bootstrap service");
    (root, service, ChildSupervisor::new())
}

fn reap_one(supervisor: &mut ChildSupervisor) -> crate::process::ChildExit {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let mut exits = supervisor.reap_exited().expect("reap child");
        if let Some(exit) = exits.pop() {
            return exit;
        }
        assert!(std::time::Instant::now() < deadline, "child did not exit");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn base_mode_arms_both_listeners_without_starting_a_process() {
    let (root, service, supervisor) = service_at_root(XwaylandMode::BaseLazy, "/bin/true");
    assert_eq!(service.state_kind(), XwaylandStateKind::Armed);
    assert_eq!(service.reactor_registrations().count(), 2);
    assert_eq!(supervisor.active_count(), 0);
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn stderr_pipe_is_nonblocking_and_closes_without_failing_generation() {
    let (root, mut service, mut supervisor) = service_at_root_with_stderr_binary();
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("generation");
    assert_eq!(
        service
            .reactor_registrations()
            .filter(|registration| registration.purpose == XwaylandReactorPurpose::Stderr)
            .count(),
        1
    );
    service
        .handle_stderr_ready(generation)
        .expect("drain stderr");
    let deadline = Instant::now() + Duration::from_secs(2);
    while service.metrics.stderr_lines == 0 && Instant::now() < deadline {
        service
            .handle_stderr_ready(generation)
            .expect("poll stderr");
        thread::sleep(Duration::from_millis(10));
    }
    assert!(service.metrics.stderr_lines >= 1);

    let process_id = service
        .readiness_snapshot()
        .expect("readiness snapshot")
        .process_id;
    supervisor
        .kill_managed_now(process_id)
        .expect("kill process");
    let deadline = Instant::now() + Duration::from_secs(2);
    while service
        .reactor_registrations()
        .any(|registration| registration.purpose == XwaylandReactorPurpose::Stderr)
        && Instant::now() < deadline
    {
        service
            .handle_stderr_ready(generation)
            .expect("handle stderr EOF");
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        service
            .reactor_registrations()
            .filter(|registration| registration.purpose == XwaylandReactorPurpose::Stderr)
            .count(),
        0
    );
    assert_eq!(service.state_kind(), XwaylandStateKind::Starting);

    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn managed_lazy_environment_triggers_first_generation() {
    let (root, mut service, mut supervisor) =
        service_at_root(XwaylandMode::ManagedLazy, "/bin/true");
    assert!(service.normal_app_environment().is_some());
    assert_eq!(service.reactor_registrations().count(), 2);

    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start managed generation");
    assert_eq!(service.state_kind(), XwaylandStateKind::Starting);
    assert_eq!(supervisor.active_count(), 1);

    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn listeners_are_registered_only_while_armed() {
    let (root, mut service, mut supervisor) = service_at_root(XwaylandMode::BaseLazy, "/bin/true");
    assert_eq!(
        service
            .reactor_registrations()
            .filter(|registration| {
                matches!(
                    registration.purpose,
                    XwaylandReactorPurpose::ListenFilesystem
                        | XwaylandReactorPurpose::ListenAbstract
                )
            })
            .count(),
        2
    );
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    assert_eq!(
        service
            .reactor_registrations()
            .filter(|registration| {
                matches!(
                    registration.purpose,
                    XwaylandReactorPurpose::ListenFilesystem
                        | XwaylandReactorPurpose::ListenAbstract
                )
            })
            .count(),
        0
    );
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn starting_generation_unregisters_parent_listener_sources() {
    let (root, mut service, mut supervisor) = service_at_root(XwaylandMode::BaseLazy, "/bin/true");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let registrations: Vec<_> = service.reactor_registrations().collect();
    assert_eq!(registrations.len(), 1);
    assert_eq!(
        registrations[0].purpose,
        XwaylandReactorPurpose::DisplayReady
    );
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn backoff_does_not_register_readable_listener_sources() {
    let (root, mut service, mut supervisor) = service_at_root(XwaylandMode::BaseLazy, "/bin/false");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let exit = reap_one(&mut supervisor);
    service.handle_process_exit(&exit).expect("handle crash");
    assert_eq!(service.state_kind(), XwaylandStateKind::Backoff);
    assert_eq!(service.reactor_registrations().count(), 0);
    assert!(
        !service
            .handle_reactor_event(
                XwaylandReactorPurpose::ListenFilesystem,
                None,
                libc::EPOLLIN as u32,
                &mut supervisor,
            )
            .expect("ignore listener readiness during backoff")
    );
    service
        .handle_deadline(u64::MAX, &mut supervisor)
        .expect("rearm after backoff");
    assert_eq!(service.state_kind(), XwaylandStateKind::Armed);
    assert_eq!(service.reactor_registrations().count(), 2);
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn rearmed_service_registers_listeners_once_after_backoff() {
    let (root, mut service, mut supervisor) = service_at_root(XwaylandMode::BaseLazy, "/bin/false");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let exit = reap_one(&mut supervisor);
    service.handle_process_exit(&exit).expect("handle crash");
    service
        .handle_deadline(u64::MAX, &mut supervisor)
        .expect("rearm after backoff");
    assert!(
        service
            .handle_listener_readiness(&mut supervisor)
            .expect("start rearmed generation")
    );
    assert_eq!(
        service
            .reactor_registrations()
            .filter(|registration| {
                matches!(
                    registration.purpose,
                    XwaylandReactorPurpose::ListenFilesystem
                        | XwaylandReactorPurpose::ListenAbstract
                )
            })
            .count(),
        0
    );
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn private_wayland_endpoint_can_be_taken_only_once() {
    let (root, mut service, mut supervisor) =
        service_at_root_with_sleeping_binary(XwaylandMode::BaseLazy, "private-endpoint");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("starting generation");
    assert!(service.take_private_wayland_client(generation).is_some());
    assert!(service.take_private_wayland_client(generation).is_none());
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn private_client_protocol_disconnect_fails_only_its_generation() {
    let (root, mut service, mut supervisor) =
        service_at_root_with_sleeping_binary(XwaylandMode::BaseLazy, "private-disconnect");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("starting generation");
    assert!(service.take_private_wayland_client(generation).is_some());
    service
        .handle_private_client_disconnected(generation, &mut supervisor)
        .expect("handle private client disconnect");
    assert_eq!(service.state_kind(), XwaylandStateKind::Backoff);
    assert_eq!(service.reactor_registrations().count(), 0);
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn generation_exhaustion_fails_instead_of_reusing_identity() {
    let root = test_root("generation-exhaustion");
    let mut service = XwaylandService::bootstrap_with_config(XwaylandConfig::for_tests_at_root(
        XwaylandMode::BaseLazy,
        PathBuf::from("Xwayland"),
        root.clone(),
    ))
    .expect("bootstrap service");
    service.next_generation = NonZeroU64::MAX;
    assert!(service.allocate_generation().is_err());
    assert_eq!(service.state_kind(), XwaylandStateKind::Failed);
    drop(service);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn listener_readiness_starts_one_generation_even_when_delivered_twice() {
    let (root, mut service, mut supervisor) = service_at_root(XwaylandMode::BaseLazy, "/bin/true");
    assert!(
        service
            .handle_listener_readiness(&mut supervisor)
            .expect("start generation")
    );
    assert!(
        !service
            .handle_listener_readiness(&mut supervisor)
            .expect("coalesce second readiness")
    );
    assert_eq!(service.state_kind(), XwaylandStateKind::Starting);
    assert_eq!(supervisor.active_count(), 1);
    let exit = reap_one(&mut supervisor);
    service.handle_process_exit(&exit).expect("handle exit");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn pure_epollout_advances_incremental_setup() {
    let (root, mut service, mut supervisor) =
        service_at_root_with_sleeping_binary(XwaylandMode::ManagedLazy, "epollout-setup");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("generation");
    assert!(
        !service
            .handle_reactor_event(
                XwaylandReactorPurpose::Xwm,
                Some(generation),
                libc::EPOLLOUT as u32,
                &mut supervisor,
            )
            .expect("pure writable event is nonfatal")
    );
    assert_eq!(service.xwm_reactor_events_for_tests(), 1);
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn stale_writable_token_is_ignored() {
    let (root, mut service, mut supervisor) =
        service_at_root_with_sleeping_binary(XwaylandMode::ManagedLazy, "stale-writable");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("generation");
    assert!(
        !service
            .handle_reactor_event_with_token(
                XwaylandReactorPurpose::Xwm,
                Some(generation),
                libc::EPOLLOUT as u32,
                u64::MAX,
                &mut supervisor,
            )
            .expect("stale writable event is nonfatal")
    );
    assert_eq!(service.xwm_reactor_events_for_tests(), 0);
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn command_write_failure_does_not_terminate_native_runtime() {
    let (root, mut service, mut supervisor) =
        service_at_root_with_sleeping_binary(XwaylandMode::ManagedLazy, "command-write-failure");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    service.inject_xwm_failure_for_tests(
        &mut supervisor,
        XwaylandFailureStage::CommandWrite,
        "injected command write failure",
    );
    assert_eq!(service.state_kind(), XwaylandStateKind::Backoff);
    assert_eq!(
        service.latest_failure_stage_for_tests(),
        Some(XwaylandFailureStage::CommandWrite)
    );
    service
        .handle_deadline(u64::MAX, &mut supervisor)
        .expect("kill failed generation");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn command_flush_failure_does_not_terminate_native_runtime() {
    let (root, mut service, mut supervisor) =
        service_at_root_with_sleeping_binary(XwaylandMode::ManagedLazy, "command-flush-failure");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    service.inject_xwm_failure_for_tests(
        &mut supervisor,
        XwaylandFailureStage::CommandFlush,
        "injected command flush failure",
    );
    assert_eq!(service.state_kind(), XwaylandStateKind::Backoff);
    assert_eq!(
        service.latest_failure_stage_for_tests(),
        Some(XwaylandFailureStage::CommandFlush)
    );
    service
        .handle_deadline(u64::MAX, &mut supervisor)
        .expect("kill failed generation");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn startup_flush_failure_fails_generation() {
    let (root, mut service, mut supervisor) =
        service_at_root_with_sleeping_binary(XwaylandMode::ManagedLazy, "startup-flush-failure");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    service.inject_xwm_failure_for_tests(
        &mut supervisor,
        XwaylandFailureStage::StartupFlush,
        "injected startup flush failure",
    );
    assert_eq!(service.state_kind(), XwaylandStateKind::Backoff);
    assert_eq!(
        service.latest_failure_stage_for_tests(),
        Some(XwaylandFailureStage::StartupFlush)
    );
    service
        .handle_deadline(u64::MAX, &mut supervisor)
        .expect("kill failed generation");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn buffer_ready_for_stale_generation_is_nonfatal() {
    let (root, mut service, mut supervisor) =
        service_at_root_with_sleeping_binary(XwaylandMode::ManagedLazy, "stale-buffer-ready");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("generation");
    let stale = super::XwaylandGeneration::new(
        NonZeroU64::new(generation.get().saturating_add(1)).expect("nonzero stale generation"),
    );
    service
        .mark_managed_surface_buffer_ready(&mut supervisor, stale, 17)
        .expect("stale buffer-ready event is nonfatal");
    assert_eq!(service.state_kind(), XwaylandStateKind::Starting);
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn xwm_hup_restarts_only_xwayland_generation() {
    let (root, mut service, mut supervisor) =
        service_at_root_with_sleeping_binary(XwaylandMode::ManagedLazy, "xwm-hup");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("generation");
    service
        .handle_reactor_event(
            XwaylandReactorPurpose::Xwm,
            Some(generation),
            libc::EPOLLHUP as u32,
            &mut supervisor,
        )
        .expect("XWM HUP is contained");
    assert_eq!(service.state_kind(), XwaylandStateKind::Backoff);
    assert_eq!(
        service.latest_failure_stage_for_tests(),
        Some(XwaylandFailureStage::Reactor)
    );
    service
        .handle_deadline(u64::MAX, &mut supervisor)
        .expect("kill failed generation");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn display_and_private_shell_readiness_are_both_required() {
    let (root, mut service, mut supervisor) = service_at_root(XwaylandMode::BaseLazy, "/bin/true");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("starting generation");
    let display = service.display_number().expect("reserved display");

    service
        .handle_displayfd_bytes(
            generation,
            format!("{display}\n").as_bytes(),
            &mut supervisor,
        )
        .expect("display readiness");
    assert_eq!(service.state_kind(), XwaylandStateKind::Starting);
    service
        .handle_shell_bind(generation)
        .expect("shell readiness");
    assert_eq!(service.state_kind(), XwaylandStateKind::RunningBase);

    let exit = reap_one(&mut supervisor);
    service.handle_process_exit(&exit).expect("handle exit");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn displayfd_ready_but_shell_not_bound_reports_shell_as_missing() {
    let (root, mut service, mut supervisor) = service_at_root(XwaylandMode::BaseLazy, "/bin/true");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let display_registration = service
        .reactor_registrations()
        .find(|registration| registration.purpose == XwaylandReactorPurpose::DisplayReady)
        .expect("displayfd registration");
    service.note_reactor_registration(display_registration, true);
    let generation = service.generation().expect("starting generation");
    let display = service.display_number().expect("reserved display");
    service
        .handle_displayfd_bytes(
            generation,
            format!("{display}\n").as_bytes(),
            &mut supervisor,
        )
        .expect("display readiness");

    let readiness = service.readiness_snapshot().expect("readiness snapshot");
    assert!(readiness.displayfd_readable);
    assert!(readiness.display_number_validated);
    assert_eq!(readiness.missing_conditions(), ["xwayland_shell_bound"]);

    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn shell_bound_but_displayfd_incomplete_reports_display_validation_as_missing() {
    let (root, mut service, mut supervisor) = service_at_root(XwaylandMode::BaseLazy, "/bin/true");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let display_registration = service
        .reactor_registrations()
        .find(|registration| registration.purpose == XwaylandReactorPurpose::DisplayReady)
        .expect("displayfd registration");
    service.note_reactor_registration(display_registration, true);
    let generation = service.generation().expect("starting generation");
    service
        .handle_shell_bind(generation)
        .expect("shell readiness");

    let readiness = service.readiness_snapshot().expect("readiness snapshot");
    assert!(!readiness.displayfd_readable);
    assert!(!readiness.display_number_validated);
    assert_eq!(
        readiness.missing_conditions(),
        ["displayfd_readable", "display_number_validated"]
    );

    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn shell_readiness_alone_remains_starting_and_reverse_order_completes() {
    let (root, mut service, mut supervisor) = service_at_root(XwaylandMode::BaseLazy, "/bin/true");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("starting generation");
    service
        .handle_shell_bind(generation)
        .expect("shell readiness");
    assert_eq!(service.state_kind(), XwaylandStateKind::Starting);
    let display = service.display_number().expect("reserved display");
    service
        .handle_displayfd_bytes(
            generation,
            format!("{display}\n").as_bytes(),
            &mut supervisor,
        )
        .expect("display readiness");
    assert_eq!(service.state_kind(), XwaylandStateKind::RunningBase);
    let exit = reap_one(&mut supervisor);
    service.handle_process_exit(&exit).expect("handle exit");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn wrong_display_readiness_fails_the_generation_without_running() {
    let (root, mut service, mut supervisor) =
        service_at_root_with_sleeping_binary(XwaylandMode::BaseLazy, "wrong-display");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("starting generation");
    let display = service.display_number().expect("reserved display");

    assert!(
        service
            .handle_displayfd_bytes(
                generation,
                format!("{}\n", display.saturating_add(1)).as_bytes(),
                &mut supervisor,
            )
            .is_err()
    );
    assert_ne!(service.state_kind(), XwaylandStateKind::RunningBase);
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn stale_generation_readiness_cannot_complete_new_generation() {
    let (root, mut service, mut supervisor) = service_at_root(XwaylandMode::BaseLazy, "/bin/true");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start first generation");
    let first = service.generation().expect("first generation");
    let exit = reap_one(&mut supervisor);
    service
        .handle_process_exit(&exit)
        .expect("rearm after clean exit");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start second generation");
    let second = service.generation().expect("second generation");
    assert_ne!(first, second);
    let display = service.display_number().expect("reserved display");
    service
        .handle_displayfd_bytes(first, format!("{display}\n").as_bytes(), &mut supervisor)
        .expect("stale readiness is ignored");
    assert_eq!(service.state_kind(), XwaylandStateKind::Starting);
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn eager_mode_uses_the_same_generation_start_path() {
    let root = test_root("eager");
    let config = XwaylandConfig::for_tests_at_root(
        XwaylandMode::BaseEager,
        PathBuf::from("/bin/true"),
        root.clone(),
    );
    let mut supervisor = ChildSupervisor::new();
    let mut service = XwaylandService::bootstrap_with_supervisor(config, &mut supervisor)
        .expect("bootstrap eager service");
    assert_eq!(service.state_kind(), XwaylandStateKind::Starting);
    assert_eq!(supervisor.active_count(), 1);
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn malformed_and_oversized_displayfd_payloads_fail_safely() {
    let (root, mut service, mut supervisor) =
        service_at_root_with_sleeping_binary(XwaylandMode::BaseLazy, "malformed-display");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("generation");
    assert!(
        service
            .handle_displayfd_bytes(generation, b"not-a-display\n", &mut supervisor)
            .is_err()
    );
    assert_eq!(service.state_kind(), XwaylandStateKind::Backoff);
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");

    let (root, mut service, mut supervisor) =
        service_at_root_with_sleeping_binary(XwaylandMode::BaseLazy, "oversized-display");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("generation");
    assert!(
        service
            .handle_displayfd_bytes(generation, &[b'1'; 33], &mut supervisor)
            .is_err()
    );
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn abnormal_exit_enters_backoff_and_clean_exit_rearms() {
    let (root, mut service, mut supervisor) = service_at_root(XwaylandMode::BaseLazy, "/bin/false");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start crashing generation");
    let exit = reap_one(&mut supervisor);
    service.handle_process_exit(&exit).expect("handle crash");
    assert_eq!(service.state_kind(), XwaylandStateKind::Backoff);
    assert!(service.next_deadline_ns().is_some());
    service
        .handle_deadline(u64::MAX, &mut supervisor)
        .expect("leave backoff");
    assert_eq!(service.state_kind(), XwaylandStateKind::Armed);
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");

    let (root, mut service, mut supervisor) = service_at_root(XwaylandMode::BaseLazy, "/bin/true");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start clean generation");
    let exit = reap_one(&mut supervisor);
    service
        .handle_process_exit(&exit)
        .expect("handle clean exit");
    assert_eq!(service.state_kind(), XwaylandStateKind::Armed);
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn startup_timeout_kills_generation_and_enters_backoff() {
    let (root, mut service, mut supervisor) =
        service_at_root_with_sleeping_binary(XwaylandMode::BaseLazy, "startup-timeout");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    assert_eq!(supervisor.active_count(), 1);
    service
        .handle_deadline(u64::MAX, &mut supervisor)
        .expect("handle startup timeout");
    assert_eq!(service.state_kind(), XwaylandStateKind::Backoff);
    let readiness = service.readiness_snapshot().expect("timeout diagnostics");
    assert_eq!(readiness.process_id.get(), 1);
    assert!(!readiness.readiness_complete);
    assert!(
        readiness
            .missing_conditions()
            .contains(&"xwayland_shell_bound")
    );
    assert!(service.has_pending_reactor_teardown());
    service
        .finish_reactor_teardown()
        .expect("finish source teardown");
    assert!(!service.has_pending_reactor_teardown());
    let deadline = Instant::now() + Duration::from_secs(2);
    while supervisor.active_count() != 0 && Instant::now() < deadline {
        let _ = supervisor.reap_exited().expect("reap timed out child");
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(supervisor.active_count(), 0);
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn timeout_unregisters_generation_source_before_descriptor_release() {
    let (root, mut service, mut supervisor) =
        service_at_root_with_sleeping_binary(XwaylandMode::BaseLazy, "timeout-order");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let registration = service
        .reactor_registrations()
        .find(|registration| registration.purpose == XwaylandReactorPurpose::DisplayReady)
        .expect("displayfd registration");
    let mut event_loop = NativeEventLoop::new().expect("reactor");
    let token = event_loop
        .register(registration.fd, NativeEventSource::XwaylandDisplayReady)
        .expect("register displayfd");
    service.note_reactor_registration(registration, true);

    service
        .handle_deadline(u64::MAX, &mut supervisor)
        .expect("handle startup timeout");
    assert!(service.has_pending_reactor_teardown());
    assert!(
        event_loop
            .unregister(token)
            .expect("unregister before close")
    );
    service
        .finish_reactor_teardown()
        .expect("release after unregister");
    assert!(!service.has_pending_reactor_teardown());

    drop(event_loop);
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn process_exit_during_starting_retires_sources_before_resources() {
    let (root, mut service, mut supervisor) = service_at_root(XwaylandMode::BaseLazy, "/bin/false");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let registration = service
        .reactor_registrations()
        .find(|registration| registration.purpose == XwaylandReactorPurpose::DisplayReady)
        .expect("displayfd registration");
    let mut event_loop = NativeEventLoop::new().expect("reactor");
    let token = event_loop
        .register(registration.fd, NativeEventSource::XwaylandDisplayReady)
        .expect("register displayfd");
    service.note_reactor_registration(registration, true);

    let exit = reap_one(&mut supervisor);
    assert!(
        service
            .handle_process_exit(&exit)
            .expect("handle startup exit")
    );
    assert_eq!(service.state_kind(), XwaylandStateKind::Backoff);
    assert!(service.has_pending_reactor_teardown());
    assert!(event_loop.unregister(token).expect("unregister displayfd"));
    service
        .finish_reactor_teardown()
        .expect("finish source teardown");
    assert!(!service.has_pending_reactor_teardown());

    drop(event_loop);
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn shutdown_unregisters_listeners_before_lease_release() {
    let (root, mut service, mut supervisor) = service_at_root(XwaylandMode::BaseLazy, "/bin/true");
    let display = service.display_number().expect("display");
    let environment = service.app_environment().expect("app environment");
    let registrations = service.reactor_registrations().collect::<Vec<_>>();
    assert_eq!(registrations.len(), 2);

    let mut event_loop = NativeEventLoop::new().expect("reactor");
    let tokens = registrations
        .iter()
        .map(|registration| {
            let source = match registration.purpose {
                XwaylandReactorPurpose::ListenFilesystem
                | XwaylandReactorPurpose::ListenAbstract => NativeEventSource::XwaylandListen,
                _ => panic!("unexpected armed registration"),
            };
            event_loop
                .register(registration.fd, source)
                .expect("register listener")
        })
        .collect::<Vec<_>>();

    service
        .begin_shutdown(&mut supervisor)
        .expect("shutdown service");
    assert!(service.has_pending_reactor_teardown());
    for token in tokens {
        assert!(event_loop.unregister(token).expect("unregister listener"));
    }
    assert_eq!(event_loop.benign_unregistration_count(), 0);
    service
        .finish_reactor_teardown()
        .expect("finish lease teardown");
    assert!(!service.has_pending_reactor_teardown());
    assert!(!root.join(format!(".X{display}-lock")).exists());
    assert!(!environment.xauthority.exists());

    drop(event_loop);
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn stale_child_exit_cannot_stop_current_generation() {
    let (root, mut service, mut supervisor) = service_at_root(XwaylandMode::BaseLazy, "/bin/true");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start first generation");
    let first_exit = reap_one(&mut supervisor);
    service
        .handle_process_exit(&first_exit)
        .expect("rearm after first exit");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start second generation");
    assert!(
        !service
            .handle_process_exit(&first_exit)
            .expect("ignore stale exit")
    );
    assert_eq!(service.state_kind(), XwaylandStateKind::Starting);
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn third_abnormal_exit_enters_failed_without_rearming() {
    let (root, mut service, mut supervisor) = service_at_root(XwaylandMode::BaseLazy, "/bin/false");
    for attempt in 0..3 {
        service
            .handle_listener_readiness(&mut supervisor)
            .expect("start crashing generation");
        let exit = reap_one(&mut supervisor);
        service.handle_process_exit(&exit).expect("handle crash");
        if attempt < 2 {
            assert_eq!(service.state_kind(), XwaylandStateKind::Backoff);
            service
                .handle_deadline(u64::MAX, &mut supervisor)
                .expect("leave backoff");
        }
    }
    assert_eq!(service.state_kind(), XwaylandStateKind::Failed);
    assert!(service.next_deadline_ns().is_none());
    assert_eq!(supervisor.active_count(), 0);
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[derive(Default)]
struct SmokeRegistryState;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for SmokeRegistryState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

#[test]
#[ignore = "requires an installed Xwayland and an explicit --ignored invocation"]
fn installed_xwayland_private_socket_smoke_test() -> Result<(), Box<dyn std::error::Error>> {
    let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) else {
        eprintln!("skipping XWayland smoke test: XDG_RUNTIME_DIR is unset");
        return Ok(());
    };
    let binary = std::env::var_os("TYPHON_XWAYLAND_BINARY")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            ["/usr/bin/Xwayland", "/usr/local/bin/Xwayland"]
                .into_iter()
                .map(PathBuf::from)
                .find(|path| path.is_file())
        });
    let Some(binary) = binary else {
        eprintln!("skipping XWayland smoke test: Xwayland is not installed");
        return Ok(());
    };

    let socket_name = format!("typhon-xwayland-smoke-{}", std::process::id());
    let socket_path = runtime_dir.join(&socket_name);
    let mut compositor =
        crate::compositor::OwnCompositorServer::bind_cpu_composition(&socket_name)?;
    let mut config = XwaylandConfig::for_tests(XwaylandMode::BaseLazy, binary);
    config.display_min = 1;
    config.display_max = 63;
    let mut supervisor = ChildSupervisor::new();
    let mut service = XwaylandService::bootstrap_with_config(config)?;
    let display = service
        .display_number()
        .expect("enabled service has display");
    let lock_path = PathBuf::from(format!("/tmp/.X{display}-lock"));
    let display_socket_path = PathBuf::from(format!("/tmp/.X11-unix/X{display}"));
    let auth_path = service
        .app_environment()
        .expect("enabled service has app environment")
        .xauthority;

    // A real local connection is the lazy trigger. The connection is closed
    // immediately; the inherited X listener remains owned by the service.
    drop(UnixStream::connect(&display_socket_path)?);
    service.handle_reactor_event(
        XwaylandReactorPurpose::ListenFilesystem,
        None,
        libc::EPOLLIN as u32,
        &mut supervisor,
    )?;

    let generation = service
        .generation()
        .expect("listener trigger starts generation");
    let private_stream = service
        .take_private_wayland_client(generation)
        .expect("generation owns a private Wayland stream");
    let identity = compositor.insert_xwayland_client(private_stream, generation)?;
    service.authorize_private_client(generation, identity.client_id.clone());

    let (bind_sender, bind_receiver) = mpsc::channel();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let server_thread = thread::spawn(move || {
        while server_running.load(Ordering::Relaxed) {
            let _ = compositor.tick();
            let binds = compositor.take_xwayland_shell_bind_events();
            if !binds.is_empty() {
                let _ = bind_sender.send(binds);
            }
            thread::sleep(Duration::from_millis(2));
        }
        compositor
    });

    let normal_connection = Connection::from_socket(UnixStream::connect(&socket_path)?)?;
    let (globals, _queue) = registry_queue_init::<SmokeRegistryState>(&normal_connection)?;
    assert!(
        !globals
            .contents()
            .clone_list()
            .into_iter()
            .any(|global| global.interface == "xwayland_shell_v1")
    );
    drop(normal_connection);

    let deadline = Instant::now() + Duration::from_secs(8);
    let mut private_bind_seen = false;
    while Instant::now() < deadline {
        service.handle_displayfd_ready(generation, &mut supervisor)?;
        while let Ok(binds) = bind_receiver.try_recv() {
            for bind in binds {
                assert_eq!(bind.client_id, identity.client_id);
                assert_eq!(bind.generation, generation);
                service.handle_shell_bind_for_client(bind.generation, &bind.client_id)?;
                private_bind_seen = true;
            }
        }
        if service.state_kind() == XwaylandStateKind::RunningBase {
            break;
        }
        for exit in supervisor.reap_exited()? {
            service.handle_process_exit(&exit)?;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(private_bind_seen, "Xwayland did not bind xwayland-shell-v1");
    assert_eq!(service.state_kind(), XwaylandStateKind::RunningBase);

    service.emergency_cleanup(&mut supervisor)?;
    let reap_deadline = Instant::now() + Duration::from_secs(2);
    while supervisor.active_count() != 0 && Instant::now() < reap_deadline {
        let _ = supervisor.reap_exited()?;
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(supervisor.active_count(), 0);
    running.store(false, Ordering::Relaxed);
    drop(
        server_thread
            .join()
            .map_err(|_| std::io::Error::other("XWayland smoke server panicked"))?,
    );
    drop(service);
    assert!(!lock_path.exists());
    assert!(!display_socket_path.exists());
    assert!(!auth_path.exists());
    Ok(())
}
