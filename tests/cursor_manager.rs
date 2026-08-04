use oblivion_one::control_snapshots::{CursorConfigSource, CursorPersistenceSnapshot};
use oblivion_one::cursor_manager::{
    CursorManagerError, CursorThemeLoader, CursorThemeManager, LoadedCursorTheme,
};
use oblivion_one::cursor_persistence::CursorConfigurationStore;
use oblivion_one::cursor_theme::{
    CompositorCursorImage, CursorConfiguration, default_cursor_configuration,
};
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

struct FakeLoader {
    results: VecDeque<Result<(), CursorManagerError>>,
    always_error: Option<CursorManagerError>,
    loads: usize,
}

impl FakeLoader {
    fn succeeds() -> Self {
        Self {
            results: VecDeque::new(),
            always_error: None,
            loads: 0,
        }
    }

    fn fails_once(error: CursorManagerError) -> Self {
        Self {
            results: VecDeque::from([Err(error)]),
            always_error: None,
            loads: 0,
        }
    }

    fn fails_always(error: CursorManagerError) -> Self {
        Self {
            results: VecDeque::new(),
            always_error: Some(error),
            loads: 0,
        }
    }
}

impl CursorThemeLoader for FakeLoader {
    fn load(
        &mut self,
        configuration: &CursorConfiguration,
    ) -> Result<LoadedCursorTheme, CursorManagerError> {
        self.loads += 1;
        if let Some(result) = self.results.pop_front() {
            result?;
        }
        if let Some(error) = self.always_error {
            return Err(error);
        }
        let pixel = (self.loads as u32) << 16 | 0xff;
        let image = CompositorCursorImage::from_argb8888(vec![pixel], 1, 1, 0, 0)
            .map_err(|_| CursorManagerError::ThemeLoadFailed)?;
        Ok(LoadedCursorTheme::new(
            configuration.clone(),
            Arc::new(image),
        ))
    }
}

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "typhon-cursor-manager-test-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl TestRoot {
    fn store(&self) -> CursorConfigurationStore {
        CursorConfigurationStore::new(self.0.clone()).unwrap()
    }
}

fn manager(loader: Box<dyn CursorThemeLoader>) -> (CursorThemeManager, TestRoot) {
    let root = TestRoot::new();
    let store = CursorConfigurationStore::new(root.0.clone()).unwrap();
    let configuration = CursorConfiguration::new("default", 24).unwrap();
    let image = Arc::new(CompositorCursorImage::builtin_fallback());
    let manager = CursorThemeManager::new(
        configuration.clone(),
        LoadedCursorTheme::new(configuration, image),
        CursorConfigSource::Default,
        CursorPersistenceSnapshot::Missing,
        store,
        loader,
    );
    (manager, root)
}

#[test]
fn failed_candidate_load_preserves_active_generation_and_persisted_configuration() {
    let (mut manager, _root) = manager(Box::new(FakeLoader::fails_once(
        CursorManagerError::ThemeNotFound,
    )));
    let configuration = CursorConfiguration::new("Bibata", 24).unwrap();
    let before = manager.snapshot(oblivion_one::control_snapshots::CursorBackendSnapshot::Software);

    assert!(matches!(
        manager.apply(configuration),
        Err(CursorManagerError::ThemeNotFound)
    ));
    assert_eq!(manager.generation(), before.generation);
    assert_eq!(manager.snapshot(before.backend), before);
}

#[test]
fn identical_configuration_is_a_noop_but_reload_reloads_and_increments_generation() {
    let (mut manager, _root) = manager(Box::new(FakeLoader::succeeds()));
    let configuration = CursorConfiguration::new("default", 24).unwrap();
    let first = manager.generation();

    let change = manager.apply(configuration.clone()).unwrap();
    assert!(!change.published);
    assert_eq!(manager.generation(), first);

    let change = manager.reload_with(configuration).unwrap();
    assert!(change.published);
    assert_eq!(manager.generation(), first + 1);
}

#[test]
fn successful_change_persists_before_publishing_and_retains_old_generation_until_release() {
    let (mut manager, _root) = manager(Box::new(FakeLoader::succeeds()));
    let old_image = manager.active_image();
    let configuration = CursorConfiguration::new("Bibata", 32).unwrap();

    let change = manager.apply(configuration.clone()).unwrap();

    assert!(change.published);
    assert_eq!(manager.generation(), 2);
    assert_eq!(
        manager
            .snapshot(oblivion_one::control_snapshots::CursorBackendSnapshot::Software)
            .source,
        CursorConfigSource::Control
    );
    assert_eq!(manager.retired_generation_count(), 1);
    drop(old_image);
    manager.collect_retired_generations();
    assert_eq!(manager.retired_generation_count(), 0);
}

#[test]
fn persistence_failure_does_not_publish_candidate() {
    let root = TestRoot::new();
    let store = CursorConfigurationStore::new(root.0.join("missing")).unwrap();
    let configuration = CursorConfiguration::new("default", 24).unwrap();
    let image = Arc::new(CompositorCursorImage::builtin_fallback());
    let mut manager = CursorThemeManager::new(
        configuration.clone(),
        LoadedCursorTheme::new(configuration, image),
        CursorConfigSource::Default,
        CursorPersistenceSnapshot::Missing,
        store,
        Box::new(FakeLoader::succeeds()),
    );
    let result = manager.apply(CursorConfiguration::new("Bibata", 24).unwrap());
    assert!(matches!(result, Err(CursorManagerError::ConfigWriteFailed)));
    assert_eq!(manager.generation(), 1);
}

#[test]
fn missing_configuration_uses_the_default_without_a_warning_state() {
    let root = TestRoot::new();
    let store = CursorConfigurationStore::new(root.0.clone()).unwrap();
    let manager = CursorThemeManager::startup(store, Box::new(FakeLoader::succeeds()));

    assert_eq!(manager.source(), CursorConfigSource::Default);
    assert_eq!(manager.persistence(), CursorPersistenceSnapshot::Missing);
    assert_eq!(
        manager.active_configuration(),
        &default_cursor_configuration()
    );
}

#[test]
fn valid_persisted_configuration_is_loaded_at_startup() {
    let root = TestRoot::new();
    let store = CursorConfigurationStore::new(root.0.clone()).unwrap();
    let configuration = CursorConfiguration::new("Bibata-Modern-Ice", 32).unwrap();
    store.write(&configuration).unwrap();

    let manager = CursorThemeManager::startup(store, Box::new(FakeLoader::succeeds()));

    assert_eq!(manager.source(), CursorConfigSource::Config);
    assert_eq!(manager.persistence(), CursorPersistenceSnapshot::Saved);
    assert_eq!(manager.active_configuration(), &configuration);
}

#[test]
fn malformed_persisted_configuration_falls_back_to_the_default() {
    let root = TestRoot::new();
    let store = CursorConfigurationStore::new(root.0.clone()).unwrap();
    store
        .write(&CursorConfiguration::new("default", 24).unwrap())
        .unwrap();
    fs::write(store.configuration_file(), b"{}\n").unwrap();

    let manager = CursorThemeManager::startup(store, Box::new(FakeLoader::succeeds()));

    assert_eq!(manager.source(), CursorConfigSource::Default);
    assert_eq!(manager.persistence(), CursorPersistenceSnapshot::Invalid);
    assert_eq!(
        manager.active_configuration(),
        &default_cursor_configuration()
    );
}

#[test]
fn unavailable_persisted_theme_falls_back_without_changing_the_active_default() {
    let root = TestRoot::new();
    let store = CursorConfigurationStore::new(root.0.clone()).unwrap();
    store
        .write(&CursorConfiguration::new("missing-theme", 24).unwrap())
        .unwrap();

    let manager = CursorThemeManager::startup(
        store,
        Box::new(FakeLoader::fails_once(CursorManagerError::ThemeNotFound)),
    );

    assert_eq!(manager.source(), CursorConfigSource::Default);
    assert_eq!(manager.persistence(), CursorPersistenceSnapshot::Invalid);
    assert_eq!(
        manager.active_configuration(),
        &default_cursor_configuration()
    );
}

#[test]
fn failed_reload_preserves_the_active_generation() {
    let (mut manager, _root) = manager(Box::new(FakeLoader::succeeds()));
    let before = manager.snapshot(oblivion_one::control_snapshots::CursorBackendSnapshot::Software);

    assert!(matches!(
        manager.reload(),
        Err(CursorManagerError::ConfigMissing)
    ));
    assert_eq!(manager.snapshot(before.backend), before);
}

#[test]
fn one_hundred_alternating_size_changes_leave_only_the_active_generation() {
    let (mut manager, _root) = manager(Box::new(FakeLoader::succeeds()));
    for index in 0..100 {
        let size_px = if index % 2 == 0 { 8 } else { 256 };
        manager.set_size(size_px).unwrap();
        manager.collect_retired_generations();
    }
    assert_eq!(manager.generation(), 101);
    assert_eq!(manager.retired_generation_count(), 0);
    assert_eq!(manager.active_configuration().size_px, 256);
}

#[test]
fn one_hundred_alternating_theme_changes_leave_the_final_theme_active() {
    let (mut manager, _root) = manager(Box::new(FakeLoader::succeeds()));
    for index in 0..100 {
        let theme = if index % 2 == 0 { "theme-a" } else { "theme-b" };
        manager.set_theme(theme).unwrap();
        manager.collect_retired_generations();
    }
    assert_eq!(manager.generation(), 101);
    assert_eq!(manager.retired_generation_count(), 0);
    assert_eq!(manager.active_configuration().theme, "theme-b");
}

#[test]
fn one_hundred_combined_changes_publish_the_final_pair() {
    let (mut manager, _root) = manager(Box::new(FakeLoader::succeeds()));
    for index in 0..100 {
        let configuration = if index % 2 == 0 {
            CursorConfiguration::new("theme-a", 8).unwrap()
        } else {
            CursorConfiguration::new("theme-b", 256).unwrap()
        };
        manager.apply(configuration).unwrap();
        manager.collect_retired_generations();
    }
    assert_eq!(manager.generation(), 101);
    assert_eq!(manager.retired_generation_count(), 0);
    assert_eq!(manager.active_configuration().theme, "theme-b");
    assert_eq!(manager.active_configuration().size_px, 256);
}

#[test]
fn one_hundred_reloads_publish_without_leaking_generations() {
    let (mut manager, root) = manager(Box::new(FakeLoader::succeeds()));
    root.store()
        .write(&CursorConfiguration::new("reload-theme", 24).unwrap())
        .unwrap();
    for _ in 0..100 {
        manager.reload().unwrap();
        manager.collect_retired_generations();
    }
    assert_eq!(manager.generation(), 101);
    assert_eq!(manager.retired_generation_count(), 0);
    assert_eq!(manager.active_configuration().theme, "reload-theme");
}

#[test]
fn one_hundred_failed_theme_loads_leave_runtime_and_persistence_unchanged() {
    let (mut manager, root) = manager(Box::new(FakeLoader::fails_always(
        CursorManagerError::ThemeNotFound,
    )));
    let before = manager.snapshot(oblivion_one::control_snapshots::CursorBackendSnapshot::Software);
    for _ in 0..100 {
        assert!(matches!(
            manager.set_theme("missing-theme"),
            Err(CursorManagerError::ThemeNotFound)
        ));
    }
    assert_eq!(manager.snapshot(before.backend), before);
    assert_eq!(manager.diagnostics().theme_load_failures, 100);
    assert!(matches!(
        root.store().read(),
        Err(oblivion_one::cursor_persistence::CursorPersistenceError::Missing)
    ));
}

#[test]
fn one_hundred_generation_retirement_cycles_release_all_unowned_generations() {
    let (mut manager, _root) = manager(Box::new(FakeLoader::succeeds()));
    for index in 0..100 {
        manager
            .set_size(if index % 2 == 0 { 8 } else { 256 })
            .unwrap();
        manager.collect_retired_generations();
    }
    assert_eq!(manager.retired_generation_count(), 0);
    assert_eq!(manager.diagnostics().retired_generations, 100);
}
