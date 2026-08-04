use oblivion_one::cursor_persistence::{CursorConfigurationStore, CursorPersistenceError};
use oblivion_one::cursor_theme::CursorConfiguration;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
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

    store.write(&configuration).unwrap();

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
    assert!(temporary_files(&root.0).is_empty());
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
    assert!(temporary_files(&root.0).is_empty());
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
        assert!(temporary_files(&root.0).is_empty());

        fs::remove_file(root.file()).unwrap();
        fs::rename(parked, root.file()).unwrap();
    }
}

fn temporary_files(root: &Path) -> Vec<PathBuf> {
    let input = root.join("AstreaOS/input");
    fs::read_dir(input)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".cursor.json.tmp-"))
        })
        .collect()
}
