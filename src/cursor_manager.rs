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
use std::sync::Arc;

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

pub trait CursorThemeLoader {
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
    store: CursorConfigurationStore,
    loader: Box<dyn CursorThemeLoader>,
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
            store,
            loader,
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
        self.apply_with_kind(configuration, ChangeKind::Combined)
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
        self.apply_with_kind(configuration, ChangeKind::Theme)
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
        self.apply_with_kind(configuration, ChangeKind::Size)
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
        let configuration = self.store.read().map_err(|error| {
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
            ChangeKind::Reload,
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
        kind: ChangeKind,
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
        self.store.write(&configuration).map_err(|error| {
            self.diagnostics.persistence_failures =
                self.diagnostics.persistence_failures.saturating_add(1);
            map_persistence_error(error)
        })?;
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
        match self.loader.load(configuration) {
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
        kind: ChangeKind,
    ) -> CursorChange {
        let previous = std::mem::replace(&mut self.active, candidate);
        self.retired
            .push_back(RetiredCursorGeneration { theme: previous });
        self.desired = configuration;
        self.source = source;
        self.persistence = persistence;
        self.generation = self.generation.saturating_add(1);
        match kind {
            ChangeKind::Theme => {
                self.diagnostics.successful_theme_changes =
                    self.diagnostics.successful_theme_changes.saturating_add(1);
            }
            ChangeKind::Size => {
                self.diagnostics.successful_size_changes =
                    self.diagnostics.successful_size_changes.saturating_add(1);
            }
            ChangeKind::Combined => {
                self.diagnostics.successful_combined_changes = self
                    .diagnostics
                    .successful_combined_changes
                    .saturating_add(1);
            }
            ChangeKind::Reload => {
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
enum ChangeKind {
    Theme,
    Size,
    Combined,
    Reload,
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
    }
}

fn persistence_snapshot(error: CursorPersistenceError) -> CursorPersistenceSnapshot {
    match error {
        CursorPersistenceError::Missing => CursorPersistenceSnapshot::Missing,
        CursorPersistenceError::Invalid => CursorPersistenceSnapshot::Invalid,
        CursorPersistenceError::Insecure => CursorPersistenceSnapshot::Insecure,
        CursorPersistenceError::WriteFailed => CursorPersistenceSnapshot::WriteFailed,
    }
}

fn map_persistence_error(error: CursorPersistenceError) -> CursorManagerError {
    match error {
        CursorPersistenceError::Missing => CursorManagerError::ConfigMissing,
        CursorPersistenceError::Invalid => CursorManagerError::ConfigInvalid,
        CursorPersistenceError::Insecure => CursorManagerError::ConfigInsecure,
        CursorPersistenceError::WriteFailed => CursorManagerError::ConfigWriteFailed,
    }
}
