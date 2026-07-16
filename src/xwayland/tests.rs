use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::{Mutex, MutexGuard, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    XwaylandConfig, XwaylandMode, XwaylandService, XwaylandStateKind,
    auth::read_cookie_for_tests,
    display::{DisplayLease, connect_abstract_socket_for_tests},
};

#[test]
fn xwayland_mode_parses_only_opt_in_values() {
    assert_eq!(XwaylandMode::parse(None), XwaylandMode::Off);
    assert_eq!(XwaylandMode::parse(Some("off")), XwaylandMode::Off);
    assert_eq!(XwaylandMode::parse(Some("base")), XwaylandMode::BaseLazy);
    assert_eq!(XwaylandMode::parse(Some("eager")), XwaylandMode::BaseEager);
    assert_eq!(XwaylandMode::parse(Some("host")), XwaylandMode::Off);
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
    let mut service = XwaylandService::bootstrap_with_config(XwaylandConfig::for_tests(
        XwaylandMode::BaseLazy,
        PathBuf::from("Xwayland"),
    ))
    .expect("bootstrap base mode");

    let first = service.allocate_generation();
    let second = service.allocate_generation();
    assert_ne!(first, second);
    assert_ne!(first.get(), 0);
    assert_ne!(second.get(), 0);
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
fn partial_lease_failure_rolls_back_lock_and_auth_artifacts() {
    let _lock = display_test_lock();
    let root = test_root("rollback");
    let socket_path = root.join(".X11-unix/X0");
    fs::create_dir_all(socket_path.parent().expect("socket parent")).expect("create socket dir");
    symlink(&root, &socket_path).expect("create socket symlink");

    assert!(DisplayLease::allocate_for_tests(&root, 0, 0).is_err());
    assert!(!root.join(".X0-lock").exists());
    assert!(!root.join("typhon/xwayland/.Xauthority-0").exists());
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn filesystem_and_abstract_sockets_accept_local_connections() {
    let _lock = display_test_lock();
    let root = test_root("sockets");
    let lease = DisplayLease::allocate_for_tests(&root, 0, 0).expect("allocate lease");
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
