//! Runtime-owned cursor configuration, loading, publication, and retirement.

use crate::control_snapshots::{
    CursorAssetSource, CursorBackendSnapshot, CursorConfigSource, CursorPersistenceSnapshot,
    CursorSnapshot,
};
use crate::cursor_persistence::{CursorConfigurationStore, CursorPersistenceError};
use crate::cursor_theme::{
    CompositorCursorImage, CompositorCursorShape, CursorConfiguration, CursorShapeImages,
    CursorThemeLoadError, default_cursor_configuration,
};
use std::collections::VecDeque;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};

const INITIAL_CURSOR_GENERATION: u64 = 1;
const MAX_RETIRED_GENERATIONS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorManagerError {
    InvalidTheme,
    InvalidSize,
    ThemeNotFound,
    ThemeLoadFailed,
    RequiredPointerMissing,
    CursorFileReadFailed,
    CursorFileInvalid,
    ConfigMissing,
    ConfigInvalid,
    ConfigInsecure,
    ConfigWriteFailed,
    ResourceBusy,
    PersistenceBusy,
    WorkerUnavailable,
}

impl CursorManagerError {
    pub const fn detail(self) -> &'static str {
        match self {
            Self::InvalidTheme => "invalid_cursor_theme",
            Self::InvalidSize => "invalid_cursor_size",
            Self::ThemeNotFound => "cursor_theme_not_found",
            Self::ThemeLoadFailed => "cursor_theme_load_failed",
            Self::RequiredPointerMissing => "required_pointer_missing",
            Self::CursorFileReadFailed => "cursor_file_read_failed",
            Self::CursorFileInvalid => "cursor_file_invalid",
            Self::ConfigMissing => "cursor_config_missing",
            Self::ConfigInvalid => "cursor_config_invalid",
            Self::ConfigInsecure => "cursor_config_insecure",
            Self::ConfigWriteFailed => "cursor_config_write_failed",
            Self::ResourceBusy => "cursor_generation_busy",
            Self::PersistenceBusy => "cursor_persistence_busy",
            Self::WorkerUnavailable => "cursor_io_unavailable",
        }
    }
}

impl std::fmt::Display for CursorManagerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.detail())
    }
}

impl std::error::Error for CursorManagerError {}

#[derive(Debug)]
pub struct LoadedCursorTheme {
    pub configuration: CursorConfiguration,
    pub images: CursorShapeImages,
    pub asset_source: CursorAssetSource,
}

impl LoadedCursorTheme {
    pub fn new(configuration: CursorConfiguration, image: Arc<CompositorCursorImage>) -> Self {
        Self {
            configuration,
            images: CursorShapeImages::from_pointer(image),
            asset_source: CursorAssetSource::SystemTheme,
        }
    }

    pub fn from_images(
        configuration: CursorConfiguration,
        images: CursorShapeImages,
        asset_source: CursorAssetSource,
    ) -> Self {
        Self {
            configuration,
            images,
            asset_source,
        }
    }

    pub fn image(&self, shape: CompositorCursorShape) -> Arc<CompositorCursorImage> {
        self.images.image(shape)
    }
}

pub trait CursorThemeLoader: Send {
    fn load(
        &mut self,
        configuration: &CursorConfiguration,
    ) -> Result<LoadedCursorTheme, CursorThemeLoadError>;
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CursorDiagnostics {
    pub get_commands: u64,
    pub successful_theme_changes: u64,
    pub successful_size_changes: u64,
    pub successful_combined_changes: u64,
    pub successful_reloads: u64,
    pub validation_failures: u64,
    pub theme_load_failures: u64,
    pub persistence_failures: u64,
    pub no_op_requests: u64,
    pub hardware_fallbacks: u64,
    pub retired_generations: u64,
    pub cursor_jobs_submitted: u64,
    pub cursor_jobs_rejected_busy: u64,
    pub worker_load_failures: u64,
    pub cursor_file_overflow: u64,
    pub frame_bound_violations: u64,
    pub persistence_precommit_failures: u64,
    pub persistence_commits: u64,
    pub persistence_cleanup_degradations: u64,
    pub stale_client_completions: u64,
    pub asynchronous_publications: u64,
    pub worker_notification_failures: u64,
    pub worker_terminal_failures: u64,
    pub worker_unavailable_requests: u64,
    pub persistence_lock_contentions: u64,
    pub cross_instance_transaction_admissions: u64,
}

#[derive(Debug)]
pub struct CursorChange {
    pub published: bool,
    pub generation: u64,
    pub image: Arc<CompositorCursorImage>,
}

struct RetiredCursorGeneration {
    theme: LoadedCursorTheme,
}

pub struct CursorThemeManager {
    desired: CursorConfiguration,
    active: LoadedCursorTheme,
    generation: u64,
    source: CursorConfigSource,
    persistence: CursorPersistenceSnapshot,
    store: Option<CursorConfigurationStore>,
    loader: Option<Box<dyn CursorThemeLoader>>,
    retired: VecDeque<RetiredCursorGeneration>,
    diagnostics: CursorDiagnostics,
}

impl CursorThemeManager {
    pub fn new(
        desired: CursorConfiguration,
        active: LoadedCursorTheme,
        source: CursorConfigSource,
        persistence: CursorPersistenceSnapshot,
        store: CursorConfigurationStore,
        loader: Box<dyn CursorThemeLoader>,
    ) -> Self {
        Self {
            desired,
            active,
            generation: INITIAL_CURSOR_GENERATION,
            source,
            persistence,
            store: Some(store),
            loader: Some(loader),
            retired: VecDeque::new(),
            diagnostics: CursorDiagnostics::default(),
        }
    }

    pub fn startup(store: CursorConfigurationStore, loader: Box<dyn CursorThemeLoader>) -> Self {
        let default = default_cursor_configuration();
        let mut loader = loader;
        let (desired, active, source, persistence) = match store.read() {
            Ok(configuration) => match loader.load(&configuration) {
                Ok(theme) => (
                    configuration,
                    theme,
                    CursorConfigSource::Config,
                    CursorPersistenceSnapshot::Saved,
                ),
                Err(error) => {
                    eprintln!("cursor configuration: using default ({error})");
                    (
                        configuration,
                        startup_default_theme(&mut *loader, &default),
                        CursorConfigSource::Config,
                        CursorPersistenceSnapshot::Saved,
                    )
                }
            },
            Err(error) => {
                if !matches!(error, CursorPersistenceError::Missing) {
                    eprintln!("cursor configuration: using default ({error})");
                }
                (
                    default.clone(),
                    startup_default_theme(&mut *loader, &default),
                    CursorConfigSource::Default,
                    persistence_snapshot(error),
                )
            }
        };
        Self::new(desired, active, source, persistence, store, loader)
    }

    pub fn apply(
        &mut self,
        configuration: CursorConfiguration,
    ) -> Result<CursorChange, CursorManagerError> {
        self.apply_with_kind(configuration, CursorMutationKind::Combined)
    }

    pub fn set_theme(&mut self, theme: &str) -> Result<CursorChange, CursorManagerError> {
        let configuration =
            CursorConfiguration::new(theme, self.desired.size_px).map_err(|error| match error {
                crate::cursor_theme::CursorConfigurationError::InvalidTheme => {
                    self.diagnostics.validation_failures =
                        self.diagnostics.validation_failures.saturating_add(1);
                    CursorManagerError::InvalidTheme
                }
                crate::cursor_theme::CursorConfigurationError::InvalidSize => {
                    CursorManagerError::InvalidSize
                }
            })?;
        self.apply_with_kind(configuration, CursorMutationKind::Theme)
    }

    pub fn set_size(&mut self, size_px: u32) -> Result<CursorChange, CursorManagerError> {
        let configuration = CursorConfiguration::new(&self.desired.theme, size_px).map_err(
            |error| match error {
                crate::cursor_theme::CursorConfigurationError::InvalidTheme => {
                    CursorManagerError::InvalidTheme
                }
                crate::cursor_theme::CursorConfigurationError::InvalidSize => {
                    self.diagnostics.validation_failures =
                        self.diagnostics.validation_failures.saturating_add(1);
                    CursorManagerError::InvalidSize
                }
            },
        )?;
        self.apply_with_kind(configuration, CursorMutationKind::Size)
    }

    pub fn apply_values(
        &mut self,
        theme: &str,
        size_px: u32,
    ) -> Result<CursorChange, CursorManagerError> {
        let configuration = CursorConfiguration::new(theme, size_px).map_err(|error| {
            self.diagnostics.validation_failures =
                self.diagnostics.validation_failures.saturating_add(1);
            match error {
                crate::cursor_theme::CursorConfigurationError::InvalidTheme => {
                    CursorManagerError::InvalidTheme
                }
                crate::cursor_theme::CursorConfigurationError::InvalidSize => {
                    CursorManagerError::InvalidSize
                }
            }
        })?;
        self.apply(configuration)
    }

    pub fn reload(&mut self) -> Result<CursorChange, CursorManagerError> {
        let configuration = self
            .store
            .as_ref()
            .ok_or(CursorManagerError::ResourceBusy)?
            .read()
            .map_err(|error| {
                self.diagnostics.persistence_failures =
                    self.diagnostics.persistence_failures.saturating_add(1);
                map_persistence_error(error)
            })?;
        self.reload_with(configuration)
    }

    pub fn reload_with(
        &mut self,
        configuration: CursorConfiguration,
    ) -> Result<CursorChange, CursorManagerError> {
        let candidate = self.load_candidate(&configuration)?;
        self.ensure_retirement_capacity()?;
        Ok(self.publish(
            configuration,
            candidate,
            CursorConfigSource::Config,
            CursorPersistenceSnapshot::Saved,
            CursorMutationKind::Reload,
        ))
    }

    pub fn snapshot(&self, backend: CursorBackendSnapshot) -> CursorSnapshot {
        CursorSnapshot {
            desired_theme: self.desired.theme.clone(),
            desired_size_px: self.desired.size_px,
            active_theme: self.active.configuration.theme.clone(),
            active_size_px: self.active.configuration.size_px,
            generation: self.generation,
            backend,
            source: self.source,
            persistence: self.persistence,
            asset_source: self.active.asset_source,
        }
    }

    pub fn active_image(&self) -> Arc<CompositorCursorImage> {
        self.active.image(CompositorCursorShape::Pointer)
    }

    pub fn active_image_for_shape(
        &self,
        shape: CompositorCursorShape,
    ) -> Arc<CompositorCursorImage> {
        self.active.image(shape)
    }

    pub const fn asset_source(&self) -> CursorAssetSource {
        self.active.asset_source
    }

    pub fn desired_configuration(&self) -> &CursorConfiguration {
        &self.desired
    }

    pub fn active_configuration(&self) -> &CursorConfiguration {
        &self.active.configuration
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn source(&self) -> CursorConfigSource {
        self.source
    }

    pub const fn persistence(&self) -> CursorPersistenceSnapshot {
        self.persistence
    }

    pub const fn diagnostics(&self) -> CursorDiagnostics {
        self.diagnostics
    }

    pub fn start_io_worker(&mut self) -> io::Result<CursorIoWorker> {
        let store = self
            .store
            .take()
            .ok_or_else(|| io::Error::other("cursor I/O worker already started"))?;
        let Some(loader) = self.loader.take() else {
            self.store = Some(store);
            return Err(io::Error::other("cursor I/O loader already moved"));
        };
        CursorIoWorker::new(store, loader)
    }

    pub fn configuration_for_theme(
        &mut self,
        theme: &str,
    ) -> Result<CursorConfiguration, CursorManagerError> {
        CursorConfiguration::new(theme, self.desired.size_px).map_err(|error| {
            self.diagnostics.validation_failures =
                self.diagnostics.validation_failures.saturating_add(1);
            map_configuration_error(error)
        })
    }

    pub fn configuration_for_size(
        &mut self,
        size_px: u32,
    ) -> Result<CursorConfiguration, CursorManagerError> {
        CursorConfiguration::new(&self.desired.theme, size_px).map_err(|error| {
            self.diagnostics.validation_failures =
                self.diagnostics.validation_failures.saturating_add(1);
            map_configuration_error(error)
        })
    }

    pub fn configuration_for_values(
        &mut self,
        theme: &str,
        size_px: u32,
    ) -> Result<CursorConfiguration, CursorManagerError> {
        CursorConfiguration::new(theme, size_px).map_err(|error| {
            self.diagnostics.validation_failures =
                self.diagnostics.validation_failures.saturating_add(1);
            map_configuration_error(error)
        })
    }

    pub fn is_no_op(&self, configuration: &CursorConfiguration) -> bool {
        self.desired == *configuration && self.active.configuration == *configuration
    }

    pub fn note_no_op(&mut self) {
        self.diagnostics.no_op_requests = self.diagnostics.no_op_requests.saturating_add(1);
    }

    pub fn ensure_mutation_capacity(&mut self) -> Result<(), CursorManagerError> {
        self.ensure_retirement_capacity()
    }

    pub fn publish_prepared(&mut self, prepared: PreparedCursorMutation) -> CursorChange {
        if prepared.persistence_cleanup_degraded {
            self.diagnostics.persistence_cleanup_degradations = self
                .diagnostics
                .persistence_cleanup_degradations
                .saturating_add(1);
        }
        if prepared.persisted {
            self.diagnostics.persistence_commits =
                self.diagnostics.persistence_commits.saturating_add(1);
            self.diagnostics.cross_instance_transaction_admissions = self
                .diagnostics
                .cross_instance_transaction_admissions
                .saturating_add(1);
        }
        self.diagnostics.asynchronous_publications =
            self.diagnostics.asynchronous_publications.saturating_add(1);
        self.publish(
            prepared.configuration,
            prepared.candidate,
            prepared.source,
            prepared.persistence,
            prepared.kind,
        )
    }

    pub fn note_cursor_job_submitted(&mut self) {
        self.diagnostics.cursor_jobs_submitted =
            self.diagnostics.cursor_jobs_submitted.saturating_add(1);
    }

    pub fn note_cursor_job_busy(&mut self) {
        self.diagnostics.cursor_jobs_rejected_busy =
            self.diagnostics.cursor_jobs_rejected_busy.saturating_add(1);
    }

    pub fn note_worker_error(&mut self, error: CursorIoError) {
        if matches!(error, CursorIoError::Load(_)) {
            self.diagnostics.worker_load_failures =
                self.diagnostics.worker_load_failures.saturating_add(1);
            self.diagnostics.theme_load_failures =
                self.diagnostics.theme_load_failures.saturating_add(1);
        }
        match error {
            CursorIoError::Load(CursorThemeLoadError::CursorFileTooLarge) => {
                self.diagnostics.cursor_file_overflow =
                    self.diagnostics.cursor_file_overflow.saturating_add(1);
            }
            CursorIoError::Load(CursorThemeLoadError::FrameBoundsExceeded) => {
                self.diagnostics.frame_bound_violations =
                    self.diagnostics.frame_bound_violations.saturating_add(1);
            }
            CursorIoError::Persistence(CursorPersistenceError::Busy) => {
                self.note_persistence_lock_contention();
            }
            CursorIoError::Persistence(_) => {
                self.diagnostics.persistence_failures =
                    self.diagnostics.persistence_failures.saturating_add(1);
                self.diagnostics.persistence_precommit_failures = self
                    .diagnostics
                    .persistence_precommit_failures
                    .saturating_add(1);
            }
            CursorIoError::WorkerPanicked | CursorIoError::WorkerUnavailable => {
                self.diagnostics.worker_terminal_failures =
                    self.diagnostics.worker_terminal_failures.saturating_add(1);
            }
            CursorIoError::Load(_) => {}
        }
    }

    pub fn note_worker_notification_failure(&mut self) {
        self.diagnostics.worker_notification_failures = self
            .diagnostics
            .worker_notification_failures
            .saturating_add(1);
    }

    pub fn note_worker_unavailable(&mut self) {
        self.diagnostics.worker_unavailable_requests = self
            .diagnostics
            .worker_unavailable_requests
            .saturating_add(1);
    }

    pub fn note_persistence_lock_contention(&mut self) {
        self.diagnostics.persistence_lock_contentions = self
            .diagnostics
            .persistence_lock_contentions
            .saturating_add(1);
    }

    pub fn note_stale_client_completion(&mut self) {
        self.diagnostics.stale_client_completions =
            self.diagnostics.stale_client_completions.saturating_add(1);
    }

    pub fn note_get(&mut self) {
        self.diagnostics.get_commands = self.diagnostics.get_commands.saturating_add(1);
    }

    pub fn note_validation_failure(&mut self) {
        self.diagnostics.validation_failures =
            self.diagnostics.validation_failures.saturating_add(1);
    }

    pub fn note_persistence_failure(&mut self) {
        self.diagnostics.persistence_failures =
            self.diagnostics.persistence_failures.saturating_add(1);
    }

    pub fn note_hardware_fallback(&mut self) {
        self.diagnostics.hardware_fallbacks = self.diagnostics.hardware_fallbacks.saturating_add(1);
    }

    pub fn retired_generation_count(&self) -> usize {
        self.retired.len()
    }

    pub fn collect_retired_generations(&mut self) {
        let before = self.retired.len();
        self.retired
            .retain(|retired| retired.theme.images.has_external_owner());
        let retired = before.saturating_sub(self.retired.len());
        self.diagnostics.retired_generations = self
            .diagnostics
            .retired_generations
            .saturating_add(retired as u64);
    }

    fn apply_with_kind(
        &mut self,
        configuration: CursorConfiguration,
        kind: CursorMutationKind,
    ) -> Result<CursorChange, CursorManagerError> {
        if self.desired == configuration && self.active.configuration == configuration {
            self.diagnostics.no_op_requests = self.diagnostics.no_op_requests.saturating_add(1);
            return Ok(CursorChange {
                published: false,
                generation: self.generation,
                image: self.active_image(),
            });
        }
        let candidate = self.load_candidate(&configuration)?;
        self.ensure_retirement_capacity()?;
        let outcome = self
            .store
            .as_ref()
            .ok_or(CursorManagerError::ResourceBusy)?
            .write(&configuration)
            .map_err(|error| {
                self.diagnostics.persistence_failures =
                    self.diagnostics.persistence_failures.saturating_add(1);
                self.diagnostics.persistence_precommit_failures = self
                    .diagnostics
                    .persistence_precommit_failures
                    .saturating_add(1);
                map_persistence_error(error)
            })?;
        self.diagnostics.persistence_commits =
            self.diagnostics.persistence_commits.saturating_add(1);
        if outcome.cleanup_degraded {
            self.diagnostics.persistence_cleanup_degradations = self
                .diagnostics
                .persistence_cleanup_degradations
                .saturating_add(1);
        }
        Ok(self.publish(
            configuration,
            candidate,
            CursorConfigSource::Control,
            CursorPersistenceSnapshot::Saved,
            kind,
        ))
    }

    fn load_candidate(
        &mut self,
        configuration: &CursorConfiguration,
    ) -> Result<LoadedCursorTheme, CursorManagerError> {
        let Some(loader) = self.loader.as_mut() else {
            return Err(CursorManagerError::ResourceBusy);
        };
        match loader.load(configuration) {
            Ok(candidate) => Ok(candidate),
            Err(error) => {
                self.diagnostics.theme_load_failures =
                    self.diagnostics.theme_load_failures.saturating_add(1);
                Err(map_theme_load_error(error))
            }
        }
    }

    fn ensure_retirement_capacity(&mut self) -> Result<(), CursorManagerError> {
        self.collect_retired_generations();
        if self.retired.len() >= MAX_RETIRED_GENERATIONS {
            Err(CursorManagerError::ResourceBusy)
        } else {
            Ok(())
        }
    }

    fn publish(
        &mut self,
        configuration: CursorConfiguration,
        candidate: LoadedCursorTheme,
        source: CursorConfigSource,
        persistence: CursorPersistenceSnapshot,
        kind: CursorMutationKind,
    ) -> CursorChange {
        let previous = std::mem::replace(&mut self.active, candidate);
        self.retired
            .push_back(RetiredCursorGeneration { theme: previous });
        self.desired = configuration;
        self.source = source;
        self.persistence = persistence;
        self.generation = self.generation.saturating_add(1);
        match kind {
            CursorMutationKind::Theme => {
                self.diagnostics.successful_theme_changes =
                    self.diagnostics.successful_theme_changes.saturating_add(1);
            }
            CursorMutationKind::Size => {
                self.diagnostics.successful_size_changes =
                    self.diagnostics.successful_size_changes.saturating_add(1);
            }
            CursorMutationKind::Combined => {
                self.diagnostics.successful_combined_changes = self
                    .diagnostics
                    .successful_combined_changes
                    .saturating_add(1);
            }
            CursorMutationKind::Reload => {
                self.diagnostics.successful_reloads =
                    self.diagnostics.successful_reloads.saturating_add(1);
            }
        }
        CursorChange {
            published: true,
            generation: self.generation,
            image: self.active_image(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CursorMutationKind {
    Theme,
    Size,
    Combined,
    Reload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorJobId(pub u64);

#[derive(Debug)]
pub enum CursorIoOperation {
    Apply {
        job_id: CursorJobId,
        configuration: CursorConfiguration,
        persist: bool,
        kind: CursorMutationKind,
    },
    Reload {
        job_id: CursorJobId,
    },
}

impl CursorIoOperation {
    pub const fn job_id(&self) -> CursorJobId {
        match self {
            Self::Apply { job_id, .. } | Self::Reload { job_id } => *job_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorIoSubmitError {
    Busy,
    Closed,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorIoError {
    Load(CursorThemeLoadError),
    Persistence(CursorPersistenceError),
    WorkerPanicked,
    WorkerUnavailable,
}

#[derive(Debug)]
pub struct PreparedCursorMutation {
    pub job_id: CursorJobId,
    pub configuration: CursorConfiguration,
    pub candidate: LoadedCursorTheme,
    pub source: CursorConfigSource,
    pub persistence: CursorPersistenceSnapshot,
    pub kind: CursorMutationKind,
    pub persisted: bool,
    pub persistence_cleanup_degraded: bool,
}

#[derive(Debug)]
pub struct CursorIoCompletion {
    pub job_id: CursorJobId,
    pub result: Result<PreparedCursorMutation, CursorIoError>,
}

pub struct CursorIoWorker {
    jobs: SyncSender<CursorIoOperation>,
    completions: Receiver<CursorIoCompletion>,
    notification: Arc<OwnedFd>,
    busy: Arc<AtomicBool>,
    available: Arc<AtomicBool>,
    _thread: JoinHandle<()>,
}

impl CursorIoWorker {
    pub fn new(
        store: CursorConfigurationStore,
        mut loader: Box<dyn CursorThemeLoader>,
    ) -> io::Result<Self> {
        let notification = Arc::new(create_event_fd()?);
        let worker_notification = Arc::clone(&notification);
        let (jobs, job_receiver) = mpsc::sync_channel::<CursorIoOperation>(1);
        let (completion_sender, completions) = mpsc::sync_channel::<CursorIoCompletion>(1);
        let busy = Arc::new(AtomicBool::new(false));
        let worker_busy = busy.clone();
        let available = Arc::new(AtomicBool::new(true));
        let worker_available = available.clone();
        let thread = thread::Builder::new()
            .name("typhon-cursor-io".to_string())
            .spawn(move || {
                while let Ok(operation) = job_receiver.recv() {
                    let job_id = operation.job_id();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        execute_cursor_io(operation, &store, &mut *loader)
                    }))
                    .unwrap_or(Err(CursorIoError::WorkerPanicked));
                    let terminal = matches!(result, Err(CursorIoError::WorkerPanicked));
                    let completion = CursorIoCompletion { job_id, result };
                    let delivered = completion_sender.send(completion).is_ok();
                    worker_busy.store(false, Ordering::Release);
                    if !delivered {
                        worker_available.store(false, Ordering::Release);
                        break;
                    }
                    if let Err(error) = notify_eventfd(&worker_notification) {
                        eprintln!("cursor I/O worker notification failed: {error}");
                        worker_available.store(false, Ordering::Release);
                        break;
                    }
                    if terminal {
                        worker_available.store(false, Ordering::Release);
                        break;
                    }
                }
                worker_available.store(false, Ordering::Release);
            })?;
        Ok(Self {
            jobs,
            completions,
            notification,
            busy,
            available,
            _thread: thread,
        })
    }

    pub fn event_fd(&self) -> RawFd {
        self.notification.as_raw_fd()
    }

    pub fn submit(&self, operation: CursorIoOperation) -> Result<(), CursorIoSubmitError> {
        if !self.available.load(Ordering::Acquire) {
            return Err(CursorIoSubmitError::Unavailable);
        }
        if self.busy.swap(true, Ordering::AcqRel) {
            return Err(CursorIoSubmitError::Busy);
        }
        match self.jobs.try_send(operation) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.busy.store(false, Ordering::Release);
                Err(CursorIoSubmitError::Busy)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.busy.store(false, Ordering::Release);
                self.available.store(false, Ordering::Release);
                Err(CursorIoSubmitError::Unavailable)
            }
        }
    }

    pub fn drain_notification(&self) -> io::Result<()> {
        drain_eventfd(&self.notification)
    }

    pub fn try_completion(&self) -> Option<CursorIoCompletion> {
        self.completions.try_recv().ok()
    }

    pub fn receive_completion(&self) -> Option<CursorIoCompletion> {
        self.completions.recv().ok()
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    pub fn is_available(&self) -> bool {
        self.available.load(Ordering::Acquire)
    }
}

fn execute_cursor_io(
    operation: CursorIoOperation,
    store: &CursorConfigurationStore,
    loader: &mut dyn CursorThemeLoader,
) -> Result<PreparedCursorMutation, CursorIoError> {
    match operation {
        CursorIoOperation::Apply {
            job_id,
            configuration,
            persist,
            kind,
        } => {
            let candidate = loader.load(&configuration).map_err(CursorIoError::Load)?;
            let outcome = if persist {
                Some(
                    store
                        .write(&configuration)
                        .map_err(CursorIoError::Persistence)?,
                )
            } else {
                None
            };
            Ok(PreparedCursorMutation {
                job_id,
                configuration,
                candidate,
                source: CursorConfigSource::Control,
                persistence: CursorPersistenceSnapshot::Saved,
                kind,
                persisted: outcome.is_some_and(|outcome| outcome.committed),
                persistence_cleanup_degraded: outcome
                    .is_some_and(|outcome| outcome.cleanup_degraded),
            })
        }
        CursorIoOperation::Reload { job_id } => {
            let configuration = store.read().map_err(CursorIoError::Persistence)?;
            let candidate = loader.load(&configuration).map_err(CursorIoError::Load)?;
            Ok(PreparedCursorMutation {
                job_id,
                configuration,
                candidate,
                source: CursorConfigSource::Config,
                persistence: CursorPersistenceSnapshot::Saved,
                kind: CursorMutationKind::Reload,
                persisted: false,
                persistence_cleanup_degraded: false,
            })
        }
    }
}

fn create_event_fd() -> io::Result<OwnedFd> {
    let fd = unsafe {
        // SAFETY: `eventfd` has no pointer arguments and creates an owned
        // nonblocking close-on-exec descriptor for worker notifications.
        libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK)
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe {
        // SAFETY: `eventfd` returned a new owned descriptor.
        OwnedFd::from_raw_fd(fd)
    })
}

fn notify_eventfd(fd: &OwnedFd) -> io::Result<()> {
    notify_eventfd_with(|value| {
        let count = unsafe {
            // SAFETY: `value` is valid readable storage for one eventfd word;
            // `fd` is a live eventfd borrowed for this write.
            libc::write(
                fd.as_raw_fd(),
                (value as *const u64).cast(),
                std::mem::size_of::<u64>(),
            )
        };
        if count < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(count as usize)
        }
    })
}

fn drain_eventfd(fd: &OwnedFd) -> io::Result<()> {
    drain_eventfd_with(|value| {
        let count = unsafe {
            // SAFETY: `value` is valid writable storage for one eventfd word;
            // `fd` is a live nonblocking eventfd borrowed for this read.
            libc::read(
                fd.as_raw_fd(),
                (value as *mut u64).cast(),
                std::mem::size_of::<u64>(),
            )
        };
        if count < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(count as usize)
        }
    })
}

fn notify_eventfd_with(mut write: impl FnMut(&u64) -> io::Result<usize>) -> io::Result<()> {
    let value = 1_u64;
    loop {
        let count = write(&value);
        match count {
            Ok(size) if size == std::mem::size_of::<u64>() => return Ok(()),
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "short cursor worker notification",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => return Err(error),
        }
    }
}

fn drain_eventfd_with(mut read: impl FnMut(&mut u64) -> io::Result<usize>) -> io::Result<()> {
    loop {
        let mut value = 0_u64;
        match read(&mut value) {
            Ok(size) if size == std::mem::size_of::<u64>() => continue,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "short cursor worker notification",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => return Err(error),
        }
    }
}

pub struct SystemCursorThemeLoader;

impl CursorThemeLoader for SystemCursorThemeLoader {
    fn load(
        &mut self,
        configuration: &CursorConfiguration,
    ) -> Result<LoadedCursorTheme, CursorThemeLoadError> {
        let images =
            crate::cursor_theme::load_cursor_theme(&configuration.theme, configuration.size_px)?;
        Ok(LoadedCursorTheme::from_images(
            configuration.clone(),
            images,
            CursorAssetSource::SystemTheme,
        ))
    }
}

fn startup_default_theme(
    loader: &mut dyn CursorThemeLoader,
    configuration: &CursorConfiguration,
) -> LoadedCursorTheme {
    loader.load(configuration).unwrap_or_else(|_| {
        LoadedCursorTheme::from_images(
            configuration.clone(),
            CursorShapeImages::from_pointer(Arc::new(CompositorCursorImage::builtin_fallback())),
            CursorAssetSource::BuiltinFallback,
        )
    })
}

fn map_theme_load_error(error: CursorThemeLoadError) -> CursorManagerError {
    match error {
        CursorThemeLoadError::ThemeNotFound => CursorManagerError::ThemeNotFound,
        CursorThemeLoadError::RequiredPointerMissing => CursorManagerError::RequiredPointerMissing,
        CursorThemeLoadError::CursorFileReadFailed => CursorManagerError::CursorFileReadFailed,
        CursorThemeLoadError::CursorFileInvalid => CursorManagerError::CursorFileInvalid,
        CursorThemeLoadError::CursorFileTooLarge | CursorThemeLoadError::FrameBoundsExceeded => {
            CursorManagerError::CursorFileInvalid
        }
    }
}

fn map_configuration_error(
    error: crate::cursor_theme::CursorConfigurationError,
) -> CursorManagerError {
    match error {
        crate::cursor_theme::CursorConfigurationError::InvalidTheme => {
            CursorManagerError::InvalidTheme
        }
        crate::cursor_theme::CursorConfigurationError::InvalidSize => {
            CursorManagerError::InvalidSize
        }
    }
}

fn persistence_snapshot(error: CursorPersistenceError) -> CursorPersistenceSnapshot {
    match error {
        CursorPersistenceError::Missing => CursorPersistenceSnapshot::Missing,
        CursorPersistenceError::Invalid => CursorPersistenceSnapshot::Invalid,
        CursorPersistenceError::Insecure => CursorPersistenceSnapshot::Insecure,
        CursorPersistenceError::WriteFailed => CursorPersistenceSnapshot::WriteFailed,
        CursorPersistenceError::Busy => CursorPersistenceSnapshot::WriteFailed,
    }
}

fn map_persistence_error(error: CursorPersistenceError) -> CursorManagerError {
    match error {
        CursorPersistenceError::Missing => CursorManagerError::ConfigMissing,
        CursorPersistenceError::Invalid => CursorManagerError::ConfigInvalid,
        CursorPersistenceError::Insecure => CursorManagerError::ConfigInsecure,
        CursorPersistenceError::WriteFailed => CursorManagerError::ConfigWriteFailed,
        CursorPersistenceError::Busy => CursorManagerError::PersistenceBusy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_retries_interrupted_writes_and_accepts_a_full_eventfd() {
        let mut calls = 0;
        let result = notify_eventfd_with(|_| {
            calls += 1;
            if calls == 1 {
                Err(io::Error::from(io::ErrorKind::Interrupted))
            } else {
                Ok(std::mem::size_of::<u64>())
            }
        });

        assert!(result.is_ok());
        assert_eq!(calls, 2);
    }

    #[test]
    fn notification_treats_a_full_eventfd_as_already_notified() {
        let result = notify_eventfd_with(|_| Err(io::Error::from(io::ErrorKind::WouldBlock)));

        assert!(result.is_ok());
    }

    #[test]
    fn notification_rejects_short_writes() {
        let result = notify_eventfd_with(|_| Ok(std::mem::size_of::<u64>() - 1));

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn notification_drain_retries_interrupted_reads_until_would_block() {
        let mut calls = 0;
        let result = drain_eventfd_with(|_| {
            calls += 1;
            match calls {
                1 => Err(io::Error::from(io::ErrorKind::Interrupted)),
                2 => Ok(std::mem::size_of::<u64>()),
                _ => Err(io::Error::from(io::ErrorKind::WouldBlock)),
            }
        });

        assert!(result.is_ok());
        assert_eq!(calls, 3);
    }

    #[test]
    fn notification_drain_rejects_short_reads() {
        let result = drain_eventfd_with(|_| Ok(std::mem::size_of::<u64>() - 1));

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }
}
