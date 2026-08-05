use oblivion_one::control_snapshots::{
    CursorAssetSource, CursorConfigSource, CursorPersistenceSnapshot,
};
use oblivion_one::cursor_manager::{
    CursorIoError, CursorIoOperation, CursorIoSubmitError, CursorJobId, CursorManagerError,
    CursorMutationKind, CursorThemeLoader, CursorThemeManager, LoadedCursorTheme,
};
use oblivion_one::cursor_persistence::CursorConfigurationStore;
use oblivion_one::cursor_theme::{
    CompositorCursorImage, CursorConfiguration, CursorShapeImages, CursorThemeLoadError,
    default_cursor_configuration,
};
use std::collections::VecDeque;
use std::fs;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Barrier;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

struct FakeLoader {
    results: VecDeque<Result<(), CursorThemeLoadError>>,
    always_error: Option<CursorThemeLoadError>,
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

    fn fails_once(error: CursorThemeLoadError) -> Self {
        Self {
            results: VecDeque::from([Err(error)]),
            always_error: None,
            loads: 0,
        }
    }

    fn fails_always(error: CursorThemeLoadError) -> Self {
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
    ) -> Result<LoadedCursorTheme, CursorThemeLoadError> {
        self.loads += 1;
        if let Some(result) = self.results.pop_front() {
            result?;
        }
        if let Some(error) = self.always_error {
            return Err(error);
        }
        let pixel = (self.loads as u32) << 16 | 0xff;
        let image = |offset| {
            CompositorCursorImage::from_argb8888(vec![pixel + offset], 1, 1, 0, 0)
                .map(Arc::new)
                .map_err(|_| CursorThemeLoadError::CursorFileInvalid)
        };
        let images = CursorShapeImages::from_images(
            image(0)?,
            image(1)?,
            image(2)?,
            image(3)?,
            image(4)?,
            image(5)?,
        );
        Ok(LoadedCursorTheme::from_images(
            configuration.clone(),
            images,
            CursorAssetSource::SystemTheme,
        ))
    }
}

struct BlockingLoader {
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
}

struct PanicLoader {
    entered: Arc<Barrier>,
}

impl CursorThemeLoader for PanicLoader {
    fn load(
        &mut self,
        _configuration: &CursorConfiguration,
    ) -> Result<LoadedCursorTheme, CursorThemeLoadError> {
        self.entered.wait();
        panic!("controlled cursor loader panic");
    }
}

impl CursorThemeLoader for BlockingLoader {
    fn load(
        &mut self,
        configuration: &CursorConfiguration,
    ) -> Result<LoadedCursorTheme, CursorThemeLoadError> {
        self.entered.wait();
        self.release.wait();
        Ok(LoadedCursorTheme::new(
            configuration.clone(),
            Arc::new(CompositorCursorImage::builtin_fallback()),
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
        CursorThemeLoadError::RequiredPointerMissing,
    )));
    let configuration = CursorConfiguration::new("Bibata", 24).unwrap();
    let before = manager.snapshot(oblivion_one::control_snapshots::CursorBackendSnapshot::Software);

    assert!(matches!(
        manager.apply(configuration),
        Err(CursorManagerError::RequiredPointerMissing)
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
        Box::new(FakeLoader::fails_always(
            CursorThemeLoadError::ThemeNotFound,
        )),
    );

    assert_eq!(manager.source(), CursorConfigSource::Config);
    assert_eq!(manager.persistence(), CursorPersistenceSnapshot::Saved);
    assert_eq!(
        manager.desired_configuration(),
        &CursorConfiguration::new("missing-theme", 24).unwrap()
    );
    assert_eq!(
        manager.active_configuration(),
        &default_cursor_configuration()
    );
    assert_eq!(manager.asset_source(), CursorAssetSource::BuiltinFallback);
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
        CursorThemeLoadError::RequiredPointerMissing,
    )));
    let before = manager.snapshot(oblivion_one::control_snapshots::CursorBackendSnapshot::Software);
    for _ in 0..100 {
        assert!(matches!(
            manager.set_theme("missing-theme"),
            Err(CursorManagerError::RequiredPointerMissing)
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

#[test]
fn one_hundred_builtin_fallback_snapshots_report_the_asset_source_truthfully() {
    for _ in 0..100 {
        let root = TestRoot::new();
        let manager = CursorThemeManager::startup(
            root.store(),
            Box::new(FakeLoader::fails_always(
                CursorThemeLoadError::ThemeNotFound,
            )),
        );
        assert_eq!(manager.asset_source(), CursorAssetSource::BuiltinFallback);
        assert_eq!(
            manager.desired_configuration(),
            &default_cursor_configuration()
        );
    }
}

#[test]
fn one_hundred_multi_shape_retirement_cycles_release_after_the_final_shape_owner() {
    let (mut manager, _root) = manager(Box::new(FakeLoader::succeeds()));
    for index in 0..100 {
        let held_shape = manager.active_image_for_shape(
            oblivion_one::cursor_theme::CompositorCursorShape::ResizeDiagonalNeSw,
        );
        manager
            .set_size(if index % 2 == 0 { 8 } else { 256 })
            .unwrap();
        manager.collect_retired_generations();
        assert_eq!(manager.retired_generation_count(), 1);
        drop(held_shape);
        manager.collect_retired_generations();
        assert_eq!(manager.retired_generation_count(), 0);
    }
}

#[test]
fn cursor_io_worker_rejects_a_second_mutation_while_the_first_is_running() {
    let root = TestRoot::new();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let worker = oblivion_one::cursor_manager::CursorIoWorker::new(
        root.store(),
        Box::new(BlockingLoader {
            entered: entered.clone(),
            release: release.clone(),
        }),
    )
    .unwrap();
    let configuration = CursorConfiguration::new("theme-a", 24).unwrap();
    let operation = CursorIoOperation::Apply {
        job_id: CursorJobId(1),
        configuration: configuration.clone(),
        persist: true,
        kind: CursorMutationKind::Theme,
    };

    worker.submit(operation).unwrap();
    entered.wait();

    assert_eq!(
        worker.submit(CursorIoOperation::Apply {
            job_id: CursorJobId(2),
            configuration,
            persist: true,
            kind: CursorMutationKind::Theme,
        }),
        Err(CursorIoSubmitError::Busy)
    );
    release.wait();
    assert!(worker.receive_completion().is_some());
}

#[test]
fn panicking_cursor_io_worker_publishes_terminal_completion_and_becomes_unavailable() {
    let root = TestRoot::new();
    let entered = Arc::new(Barrier::new(2));
    let worker = oblivion_one::cursor_manager::CursorIoWorker::new(
        root.store(),
        Box::new(PanicLoader {
            entered: entered.clone(),
        }),
    )
    .unwrap();
    let descriptor_flags = unsafe {
        // SAFETY: the worker owns a live eventfd and `F_GETFD` only reads its
        // descriptor flags.
        libc::fcntl(worker.event_fd(), libc::F_GETFD)
    };
    assert_ne!(descriptor_flags & libc::FD_CLOEXEC, 0);
    worker
        .submit(CursorIoOperation::Apply {
            job_id: CursorJobId(1),
            configuration: CursorConfiguration::new("theme-a", 24).unwrap(),
            persist: true,
            kind: CursorMutationKind::Theme,
        })
        .unwrap();
    entered.wait();

    let mut completion = None;
    for _ in 0..10_000 {
        if let Some(value) = worker.try_completion() {
            completion = Some(value);
            break;
        }
        std::thread::yield_now();
    }
    let completion = completion.expect("panic completion should be delivered");
    assert_eq!(completion.job_id, CursorJobId(1));
    assert!(matches!(
        completion.result,
        Err(CursorIoError::WorkerPanicked)
    ));

    for _ in 0..10_000 {
        if !worker.is_available() {
            break;
        }
        std::thread::yield_now();
    }
    assert!(!worker.is_available());
    assert!(!worker.is_busy());
    assert_eq!(
        worker.submit(CursorIoOperation::Reload {
            job_id: CursorJobId(2),
        }),
        Err(CursorIoSubmitError::Unavailable)
    );
}

#[test]
fn one_hundred_sequential_cursor_io_jobs_leave_one_published_configuration() {
    let root = TestRoot::new();
    let worker = oblivion_one::cursor_manager::CursorIoWorker::new(
        root.store(),
        Box::new(FakeLoader::succeeds()),
    )
    .unwrap();

    for index in 0..100 {
        let configuration = CursorConfiguration::new(
            format!("theme-{index}"),
            if index % 2 == 0 { 8 } else { 256 },
        )
        .unwrap();
        worker
            .submit(CursorIoOperation::Apply {
                job_id: CursorJobId(index + 1),
                configuration: configuration.clone(),
                persist: true,
                kind: CursorMutationKind::Combined,
            })
            .unwrap();
        let completion = worker.receive_completion().unwrap();
        assert_eq!(completion.job_id, CursorJobId(index + 1));
        assert_eq!(completion.result.unwrap().configuration, configuration);
        worker.drain_notification().unwrap();
    }

    assert_eq!(
        root.store().read().unwrap(),
        CursorConfiguration::new("theme-99", 256).unwrap()
    );
    let entries = fs::read_dir(root.0.join("AstreaOS/input"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(".cursor.json."))
        })
        .count();
    assert_eq!(entries, 0);
}

#[test]
fn real_native_event_loop_routes_cursor_completion_without_blocking_other_sources() {
    let root = TestRoot::new();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let worker = oblivion_one::cursor_manager::CursorIoWorker::new(
        root.store(),
        Box::new(BlockingLoader {
            entered: entered.clone(),
            release: release.clone(),
        }),
    )
    .unwrap();
    let mut event_loop = oblivion_one::native::event_loop::NativeEventLoop::new().unwrap();
    let cursor_token = event_loop
        .register(
            worker.event_fd(),
            oblivion_one::native::event_loop::NativeEventSource::CursorIoWorker,
        )
        .unwrap();
    let timer = event_loop.register(
        event_fd().as_raw_fd(),
        oblivion_one::native::event_loop::NativeEventSource::Timer,
    );
    assert!(timer.is_err());
    let input = event_fd();
    let control = event_fd();
    let wayland = event_fd();
    event_loop
        .register(
            input.as_raw_fd(),
            oblivion_one::native::event_loop::NativeEventSource::Input(0),
        )
        .unwrap();
    event_loop
        .register(
            control.as_raw_fd(),
            oblivion_one::native::event_loop::NativeEventSource::ControlClient,
        )
        .unwrap();
    event_loop
        .register(
            wayland.as_raw_fd(),
            oblivion_one::native::event_loop::NativeEventSource::WaylandClients,
        )
        .unwrap();
    event_loop
        .arm_deadline(Some(
            oblivion_one::native::event_loop::monotonic_now_ns().unwrap(),
        ))
        .unwrap();

    worker
        .submit(CursorIoOperation::Apply {
            job_id: CursorJobId(1),
            configuration: CursorConfiguration::new("event-loop", 24).unwrap(),
            persist: false,
            kind: CursorMutationKind::Theme,
        })
        .unwrap();
    entered.wait();
    signal_fd(input.as_raw_fd());
    signal_fd(control.as_raw_fd());
    signal_fd(wayland.as_raw_fd());

    let wakeup = event_loop.wait().unwrap();
    assert!(wakeup.reasons.timer());
    assert!(wakeup.reasons.input());
    assert!(wakeup.reasons.control());
    assert!(wakeup.reasons.wayland_clients());
    assert!(!wakeup.reasons.cursor_io_worker());
    assert!(
        wakeup
            .control_events
            .iter()
            .all(|event| { event.token != cursor_token })
    );
    drain_fd(input.as_raw_fd());
    drain_fd(control.as_raw_fd());
    drain_fd(wayland.as_raw_fd());

    release.wait();
    let completion_wakeup = event_loop.wait().unwrap();
    assert!(completion_wakeup.reasons.cursor_io_worker());
    assert!(completion_wakeup.control_events.is_empty());
    assert_eq!(completion_wakeup.cursor_io_events.len(), 1);
    assert_eq!(completion_wakeup.cursor_io_events[0].token, cursor_token);
    worker.drain_notification().unwrap();
    assert!(worker.try_completion().is_some());
    assert!(worker.try_completion().is_none());
}

fn event_fd() -> OwnedFd {
    let fd = unsafe {
        // SAFETY: `eventfd` has no pointer arguments and returns an owned
        // nonblocking close-on-exec descriptor.
        libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK)
    };
    assert!(fd >= 0);
    unsafe {
        // SAFETY: `eventfd` returned a new owned descriptor.
        OwnedFd::from_raw_fd(fd)
    }
}

fn signal_fd(fd: std::os::fd::RawFd) {
    let value = 1_u64;
    let count = unsafe {
        // SAFETY: `value` is valid readable storage for one eventfd word and
        // `fd` is a live eventfd descriptor owned by this test.
        libc::write(
            fd,
            (&value as *const u64).cast(),
            std::mem::size_of::<u64>(),
        )
    };
    assert_eq!(count, std::mem::size_of::<u64>() as isize);
}

fn drain_fd(fd: std::os::fd::RawFd) {
    let mut value = 0_u64;
    let count = unsafe {
        // SAFETY: `value` is writable storage for one eventfd word and `fd`
        // is a live eventfd descriptor owned by this test.
        libc::read(
            fd,
            (&mut value as *mut u64).cast(),
            std::mem::size_of::<u64>(),
        )
    };
    assert_eq!(count, std::mem::size_of::<u64>() as isize);
}
