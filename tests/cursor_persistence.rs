use oblivion_one::cursor_persistence::{
    CursorConfigurationStore, CursorPersistenceError, CursorPersistenceFault,
};
use oblivion_one::cursor_theme::CursorConfiguration;
use std::fs::{self, OpenOptions};
use std::os::{
    fd::AsRawFd,
    unix::fs::{MetadataExt, PermissionsExt},
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Barrier;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "typhon-cursor-config-test-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        Self(root)
    }

    fn store(&self) -> CursorConfigurationStore {
        CursorConfigurationStore::new(self.0.clone()).unwrap()
    }

    fn file(&self) -> PathBuf {
        self.0.join("AstreaOS/input/cursor.json")
    }

    fn input_dir(&self) -> PathBuf {
        self.0.join("AstreaOS/input")
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn missing_configuration_is_distinguished_from_invalid_configuration() {
    let root = TestRoot::new();
    assert!(matches!(
        root.store().read(),
        Err(CursorPersistenceError::Missing)
    ));
}

#[test]
fn valid_configuration_is_written_atomically_and_read_back() {
    let root = TestRoot::new();
    let store = root.store();
    let configuration = CursorConfiguration::new("Bibata-Modern-Ice", 24).unwrap();
    fs::set_permissions(&root.0, fs::Permissions::from_mode(0o755)).unwrap();

    let outcome = store.write(&configuration).unwrap();

    assert!(outcome.committed);
    assert!(!outcome.cleanup_degraded);

    assert_eq!(store.read().unwrap(), configuration);
    assert_eq!(fs::metadata(&root.0).unwrap().mode() & 0o777, 0o755);
    assert_eq!(
        fs::metadata(root.0.join("AstreaOS")).unwrap().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(root.input_dir()).unwrap().mode() & 0o777,
        0o700
    );
    assert_eq!(fs::metadata(root.file()).unwrap().mode() & 0o777, 0o600);
    // SAFETY: `geteuid` has no pointer arguments and is safe to call for the
    // current process identity check used by this filesystem test.
    let effective_uid = unsafe { libc::geteuid() };
    assert_eq!(fs::metadata(root.file()).unwrap().uid(), effective_uid);
    assert!(transaction_files(&root.0).is_empty());
    assert_eq!(
        fs::read_to_string(root.file()).unwrap(),
        r#"{"version":1,"theme":"Bibata-Modern-Ice","sizePx":24}"#
    );
}

#[test]
fn malformed_and_unsupported_configuration_is_rejected() {
    let root = TestRoot::new();
    let store = root.store();
    store
        .write(&CursorConfiguration::new("default", 24).unwrap())
        .unwrap();

    fs::write(root.file(), b"not-json").unwrap();
    assert!(matches!(store.read(), Err(CursorPersistenceError::Invalid)));

    fs::write(
        root.file(),
        br#"{"version":2,"theme":"default","sizePx":24}"#,
    )
    .unwrap();
    assert!(matches!(store.read(), Err(CursorPersistenceError::Invalid)));
}

#[test]
fn symlinked_owned_directories_and_file_are_rejected() {
    let root = TestRoot::new();
    let store = root.store();
    store
        .write(&CursorConfiguration::new("default", 24).unwrap())
        .unwrap();

    let astrea = root.0.join("AstreaOS");
    let astrea_target = root.0.join("real-AstreaOS");
    fs::rename(&astrea, &astrea_target).unwrap();
    std::os::unix::fs::symlink(&astrea_target, &astrea).unwrap();
    assert!(matches!(
        store.read(),
        Err(CursorPersistenceError::Insecure)
    ));
    fs::remove_file(&astrea).unwrap();
    fs::rename(&astrea_target, &astrea).unwrap();

    let input = root.input_dir();
    let target = root.0.join("real-input");
    fs::rename(&input, &target).unwrap();
    std::os::unix::fs::symlink(&target, &input).unwrap();
    assert!(matches!(
        store.read(),
        Err(CursorPersistenceError::Insecure)
    ));

    fs::remove_file(&input).unwrap();
    fs::create_dir(&input).unwrap();
    fs::set_permissions(&input, fs::Permissions::from_mode(0o700)).unwrap();
    let replacement = root.0.join("replacement.json");
    fs::write(&replacement, b"{}").unwrap();
    std::os::unix::fs::symlink(&replacement, root.file()).unwrap();
    assert!(matches!(
        store.read(),
        Err(CursorPersistenceError::Insecure)
    ));
}

#[test]
fn foreign_or_insecure_metadata_is_rejected_by_read() {
    let root = TestRoot::new();
    let store = root.store();
    store
        .write(&CursorConfiguration::new("default", 24).unwrap())
        .unwrap();
    fs::set_permissions(root.input_dir(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        store.read(),
        Err(CursorPersistenceError::Insecure)
    ));
}

#[test]
fn failed_write_before_rename_keeps_the_replacement_and_leaves_no_temporary_file() {
    let root = TestRoot::new();
    let store = root.store();
    let original = CursorConfiguration::new("default", 24).unwrap();
    store.write(&original).unwrap();

    fs::set_permissions(root.input_dir(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        store.write(&CursorConfiguration::new("other", 32).unwrap()),
        Err(CursorPersistenceError::Insecure)
    ));
    assert_eq!(
        fs::read_to_string(root.file()).unwrap(),
        r#"{"version":1,"theme":"default","sizePx":24}"#
    );
    assert_eq!(store.read().unwrap_err(), CursorPersistenceError::Insecure);
    assert!(transaction_files(&root.0).is_empty());
}

#[test]
fn one_hundred_destination_symlink_replacements_are_rejected_without_overwrite() {
    let root = TestRoot::new();
    let store = root.store();
    let original = CursorConfiguration::new("default", 24).unwrap();
    let replacement = root.0.join("replacement.json");
    store.write(&original).unwrap();

    for index in 0..100 {
        let parked = root.0.join(format!("parked-{index}.json"));
        fs::rename(root.file(), &parked).unwrap();
        fs::write(&replacement, b"replacement must survive").unwrap();
        std::os::unix::fs::symlink(&replacement, root.file()).unwrap();

        assert!(matches!(
            store.write(&CursorConfiguration::new("other", 32).unwrap()),
            Err(CursorPersistenceError::Insecure)
        ));
        assert_eq!(fs::read(&replacement).unwrap(), b"replacement must survive");
        assert!(transaction_files(&root.0).is_empty());

        fs::remove_file(root.file()).unwrap();
        fs::rename(parked, root.file()).unwrap();
    }
}

#[test]
fn precommit_faults_preserve_the_previous_canonical_configuration() {
    let phases = [
        CursorPersistenceFault::TemporaryCreation,
        CursorPersistenceFault::TemporaryWrite,
        CursorPersistenceFault::TemporaryFsync,
        CursorPersistenceFault::Exchange,
        CursorPersistenceFault::PostExchangeIdentityVerification,
        CursorPersistenceFault::NewFileFsync,
        CursorPersistenceFault::FirstDirectoryFsync,
    ];
    for phase in phases {
        let root = TestRoot::new();
        let original = CursorConfiguration::new("default", 24).unwrap();
        root.store().write(&original).unwrap();
        let failing = root.store().with_fault_injection(phase);

        assert_eq!(
            failing.write(&CursorConfiguration::new("other", 32).unwrap()),
            Err(CursorPersistenceError::WriteFailed)
        );
        assert_eq!(root.store().read().unwrap(), original);
        assert!(transaction_files(&root.0).is_empty());
    }
}

#[test]
fn first_publication_faults_leave_no_canonical_configuration() {
    let phases = [
        CursorPersistenceFault::TemporaryCreation,
        CursorPersistenceFault::TemporaryWrite,
        CursorPersistenceFault::TemporaryFsync,
        CursorPersistenceFault::NoReplacePublication,
        CursorPersistenceFault::NewFileFsync,
        CursorPersistenceFault::FirstDirectoryFsync,
    ];
    for phase in phases {
        let root = TestRoot::new();
        let store = root.store().with_fault_injection(phase);
        let configuration = CursorConfiguration::new("default", 24).unwrap();

        assert_eq!(
            store.write(&configuration),
            Err(CursorPersistenceError::WriteFailed)
        );
        assert!(matches!(
            root.store().read(),
            Err(CursorPersistenceError::Missing)
        ));
        assert!(transaction_files(&root.0).is_empty());
    }
}

#[test]
fn postcommit_cleanup_faults_report_commit_without_runtime_disk_divergence() {
    for phase in [
        CursorPersistenceFault::OldInodeCleanup,
        CursorPersistenceFault::CleanupDirectoryFsync,
    ] {
        let root = TestRoot::new();
        let store = root.store();
        store
            .write(&CursorConfiguration::new("default", 24).unwrap())
            .unwrap();
        let next = CursorConfiguration::new("other", 32).unwrap();

        let outcome = store.with_fault_injection(phase).write(&next).unwrap();

        assert!(outcome.committed);
        assert!(outcome.cleanup_degraded);
        assert_eq!(root.store().read().unwrap(), next);
    }
}

#[test]
fn successful_retry_cleans_verified_stale_transaction_debris() {
    let root = TestRoot::new();
    let store = root.store();
    store
        .write(&CursorConfiguration::new("default", 24).unwrap())
        .unwrap();
    let next = CursorConfiguration::new("other", 32).unwrap();

    let outcome = store
        .clone()
        .with_fault_injection(CursorPersistenceFault::OldInodeCleanup)
        .write(&next)
        .unwrap();
    assert!(outcome.cleanup_degraded);
    assert!(!transaction_files(&root.0).is_empty());

    store
        .write(&CursorConfiguration::new("retry", 48).unwrap())
        .unwrap();

    assert!(transaction_files(&root.0).is_empty());
}

#[test]
fn a_second_store_fails_busy_without_touching_the_canonical_file() {
    let root = TestRoot::new();
    let first = root.store();
    let second = root.store();
    let original = CursorConfiguration::new("default", 24).unwrap();
    first.write(&original).unwrap();

    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.input_dir().join("cursor.lock"))
        .unwrap();
    let result = unsafe {
        // SAFETY: `lock` is a valid descriptor for the test lock file and
        // `flock` only changes its advisory lock state.
        libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)
    };
    assert_eq!(result, 0);

    assert_eq!(
        second.write(&CursorConfiguration::new("other", 32).unwrap()),
        Err(CursorPersistenceError::Busy)
    );
    assert_eq!(root.store().read().unwrap(), original);
    assert!(transaction_files(&root.0).is_empty());
}

#[test]
fn lock_release_allows_the_next_store_and_lock_file_remains_private() {
    let root = TestRoot::new();
    let first = root.store();
    let second = root.store();
    first
        .write(&CursorConfiguration::new("default", 24).unwrap())
        .unwrap();
    let lock_path = root.input_dir().join("cursor.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    let result = unsafe {
        // SAFETY: `lock` is a valid descriptor for the test lock file and
        // `flock` only changes its advisory lock state.
        libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)
    };
    assert_eq!(result, 0);
    drop(lock);

    second
        .write(&CursorConfiguration::new("other", 32).unwrap())
        .unwrap();
    assert_eq!(fs::metadata(lock_path).unwrap().mode() & 0o777, 0o600);
    assert!(
        fs::metadata(root.input_dir().join("cursor.lock"))
            .unwrap()
            .is_file()
    );
}

#[test]
fn active_transaction_debris_is_not_cleaned_while_another_store_holds_the_lock() {
    let root = TestRoot::new();
    let first = root.store();
    let second = root.store();
    first
        .write(&CursorConfiguration::new("default", 24).unwrap())
        .unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.input_dir().join("cursor.lock"))
        .unwrap();
    let result = unsafe {
        // SAFETY: `lock` is a valid descriptor for the test lock file and
        // `flock` only changes its advisory lock state.
        libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)
    };
    assert_eq!(result, 0);
    let debris = root.input_dir().join(".cursor.json.tmp-live");
    fs::write(&debris, b"active transaction").unwrap();

    assert_eq!(
        second.write(&CursorConfiguration::new("other", 32).unwrap()),
        Err(CursorPersistenceError::Busy)
    );
    assert!(debris.exists());
}

#[test]
fn symlinked_cursor_lock_fails_closed_without_touching_the_configuration() {
    let root = TestRoot::new();
    let store = root.store();
    let original = CursorConfiguration::new("default", 24).unwrap();
    store.write(&original).unwrap();
    let lock_path = root.input_dir().join("cursor.lock");
    let target = root.0.join("lock-target");
    fs::rename(&lock_path, &target).unwrap();
    std::os::unix::fs::symlink(&target, &lock_path).unwrap();

    assert_eq!(
        store.write(&CursorConfiguration::new("other", 32).unwrap()),
        Err(CursorPersistenceError::Insecure)
    );
    assert_eq!(root.store().read().unwrap(), original);
    assert!(target.exists());
}

#[test]
fn incorrectly_permissioned_cursor_lock_fails_closed_without_rewriting_configuration() {
    let root = TestRoot::new();
    let store = root.store();
    let original = CursorConfiguration::new("default", 24).unwrap();
    store.write(&original).unwrap();
    let lock_path = root.input_dir().join("cursor.lock");
    fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o644)).unwrap();

    assert_eq!(
        store.write(&CursorConfiguration::new("other", 32).unwrap()),
        Err(CursorPersistenceError::Insecure)
    );
    assert_eq!(root.store().read().unwrap(), original);
}

#[test]
fn one_hundred_alternating_stores_leave_one_valid_configuration_and_lock() {
    let root = TestRoot::new();
    let first = root.store();
    let second = root.store();
    for index in 0..100 {
        let configuration = CursorConfiguration::new(
            if index % 2 == 0 { "store-a" } else { "store-b" },
            if index % 2 == 0 { 8 } else { 256 },
        )
        .unwrap();
        if index % 2 == 0 {
            first.write(&configuration).unwrap();
        } else {
            second.write(&configuration).unwrap();
        }
    }
    assert_eq!(
        root.store().read().unwrap(),
        CursorConfiguration::new("store-b", 256).unwrap()
    );
    assert!(transaction_files(&root.0).is_empty());
    assert_eq!(
        fs::metadata(root.input_dir().join("cursor.lock"))
            .unwrap()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn concurrent_first_publication_is_serialized_to_one_valid_document() {
    let root = TestRoot::new();
    let first = root.store();
    let second = root.store();
    let barrier = Arc::new(Barrier::new(3));
    std::thread::scope(|scope| {
        let first_barrier = barrier.clone();
        let first_handle = scope.spawn(move || {
            first_barrier.wait();
            first.write(&CursorConfiguration::new("first", 24).unwrap())
        });
        let second_barrier = barrier.clone();
        let second_handle = scope.spawn(move || {
            second_barrier.wait();
            second.write(&CursorConfiguration::new("second", 32).unwrap())
        });
        barrier.wait();
        let first_result = first_handle.join().unwrap();
        let second_result = second_handle.join().unwrap();
        assert!(matches!(
            first_result,
            Ok(_) | Err(CursorPersistenceError::Busy)
        ));
        assert!(matches!(
            second_result,
            Ok(_) | Err(CursorPersistenceError::Busy)
        ));
        assert!(first_result.is_ok() || second_result.is_ok());
    });

    let final_configuration = root.store().read().unwrap();
    assert!(
        final_configuration == CursorConfiguration::new("first", 24).unwrap()
            || final_configuration == CursorConfiguration::new("second", 32).unwrap()
    );
    assert!(transaction_files(&root.0).is_empty());
}

fn transaction_files(root: &Path) -> Vec<PathBuf> {
    let input = root.join("AstreaOS/input");
    fs::read_dir(input)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(".cursor.json.tmp-")
                        || name.starts_with(".cursor.json.quarantine-")
                })
        })
        .collect()
}
