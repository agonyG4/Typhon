//! Fixed, observe-only allocation and driver-object counters.
//!
//! This ledger stores scalar counters only. Resource owners remain responsible
//! for handles, synchronization, and destruction on the correct thread.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use super::ResourceCoverage;

pub const GPU_RESOURCE_CLASS_COUNT: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuResourceClass {
    OutputBacking,
    FallbackScanoutBacking,
    CursorBacking,
    ShmUploadTextureBacking,
    UiTextureBacking,
    EffectTextureBacking,
    CaptureBacking,
    UploadBufferBacking,
    RendererDmabufExternalBacking,
    DirectScanoutDmabufExternalBacking,
    ImportedEglImageView,
    ImportedGlTextureView,
    GlTextureObject,
    FramebufferObject,
    GlBufferObject,
    DrmFramebuffer,
    GemHandle,
    VertexArrayObject,
    ShaderProgram,
    ShaderObject,
    QueryObject,
    EglContext,
    EglSurface,
    OtherUnknown,
}

impl GpuResourceClass {
    const fn index(self) -> usize {
        match self {
            Self::OutputBacking => 0,
            Self::FallbackScanoutBacking => 1,
            Self::CursorBacking => 2,
            Self::ShmUploadTextureBacking => 3,
            Self::UiTextureBacking => 4,
            Self::EffectTextureBacking => 5,
            Self::CaptureBacking => 6,
            Self::UploadBufferBacking => 7,
            Self::RendererDmabufExternalBacking => 8,
            Self::DirectScanoutDmabufExternalBacking => 9,
            Self::ImportedEglImageView => 10,
            Self::ImportedGlTextureView => 11,
            Self::GlTextureObject => 12,
            Self::FramebufferObject => 13,
            Self::GlBufferObject => 14,
            Self::DrmFramebuffer => 15,
            Self::GemHandle => 16,
            Self::VertexArrayObject => 17,
            Self::ShaderProgram => 18,
            Self::ShaderObject => 19,
            Self::QueryObject => 20,
            Self::EglContext => 21,
            Self::EglSurface => 22,
            Self::OtherUnknown => 23,
        }
    }

    const fn all() -> [Self; GPU_RESOURCE_CLASS_COUNT] {
        [
            Self::OutputBacking,
            Self::FallbackScanoutBacking,
            Self::CursorBacking,
            Self::ShmUploadTextureBacking,
            Self::UiTextureBacking,
            Self::EffectTextureBacking,
            Self::CaptureBacking,
            Self::UploadBufferBacking,
            Self::RendererDmabufExternalBacking,
            Self::DirectScanoutDmabufExternalBacking,
            Self::ImportedEglImageView,
            Self::ImportedGlTextureView,
            Self::GlTextureObject,
            Self::FramebufferObject,
            Self::GlBufferObject,
            Self::DrmFramebuffer,
            Self::GemHandle,
            Self::VertexArrayObject,
            Self::ShaderProgram,
            Self::ShaderObject,
            Self::QueryObject,
            Self::EglContext,
            Self::EglSurface,
            Self::OtherUnknown,
        ]
    }

    const fn byte_semantics(self) -> GpuResourceByteSemantics {
        match self {
            Self::OutputBacking
            | Self::FallbackScanoutBacking
            | Self::CursorBacking
            | Self::ShmUploadTextureBacking
            | Self::UiTextureBacking
            | Self::EffectTextureBacking
            | Self::CaptureBacking
            | Self::UploadBufferBacking => GpuResourceByteSemantics::OwnedStorageEstimate,
            Self::RendererDmabufExternalBacking | Self::DirectScanoutDmabufExternalBacking => {
                GpuResourceByteSemantics::ExternalBackingLogical
            }
            Self::ImportedEglImageView | Self::ImportedGlTextureView => {
                GpuResourceByteSemantics::AliasViewNoStorage
            }
            Self::GlTextureObject
            | Self::FramebufferObject
            | Self::GlBufferObject
            | Self::DrmFramebuffer
            | Self::GemHandle
            | Self::VertexArrayObject
            | Self::ShaderProgram
            | Self::ShaderObject
            | Self::QueryObject
            | Self::EglContext
            | Self::EglSurface
            | Self::OtherUnknown => GpuResourceByteSemantics::DriverManagedUnknown,
        }
    }

    const fn initial_coverage(self) -> ResourceCoverage {
        match self {
            Self::ShmUploadTextureBacking
            | Self::UploadBufferBacking
            | Self::CursorBacking
            | Self::UiTextureBacking
            | Self::FallbackScanoutBacking
            | Self::EffectTextureBacking
            | Self::CaptureBacking
            | Self::GlTextureObject
            | Self::FramebufferObject
            | Self::GlBufferObject
            | Self::VertexArrayObject
            | Self::ShaderObject
            | Self::QueryObject
            | Self::ImportedEglImageView
            | Self::ImportedGlTextureView => ResourceCoverage::Instrumented,
            Self::OutputBacking
            | Self::RendererDmabufExternalBacking
            | Self::DirectScanoutDmabufExternalBacking
            | Self::DrmFramebuffer
            | Self::GemHandle
            | Self::ShaderProgram
            | Self::EglContext
            | Self::EglSurface => ResourceCoverage::PartiallyInstrumented,
            _ => ResourceCoverage::Uninstrumented,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuResourceByteSemantics {
    OwnedStorageEstimate,
    ExternalBackingLogical,
    AliasViewNoStorage,
    DriverManagedUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuResourceClassSnapshot {
    pub class: GpuResourceClass,
    pub byte_semantics: GpuResourceByteSemantics,
    pub coverage: ResourceCoverage,
    pub live_count: u64,
    /// Bytes follow `byte_semantics`; this is never a physical-VRAM claim.
    pub known_live_bytes: u64,
    pub peak_live_count: u64,
    pub peak_known_live_bytes: u64,
    pub allocation_count_total: u64,
    pub allocated_known_bytes_total: u64,
    pub retirement_count_total: u64,
    pub retired_known_bytes_total: u64,
    pub resize_count: u64,
    pub unknown_size_live_count: u64,
    pub accounting_error_count: u64,
}

impl GpuResourceClassSnapshot {
    fn empty(class: GpuResourceClass) -> Self {
        Self {
            class,
            byte_semantics: class.byte_semantics(),
            coverage: class.initial_coverage(),
            live_count: 0,
            known_live_bytes: 0,
            peak_live_count: 0,
            peak_known_live_bytes: 0,
            allocation_count_total: 0,
            allocated_known_bytes_total: 0,
            retirement_count_total: 0,
            retired_known_bytes_total: 0,
            resize_count: 0,
            unknown_size_live_count: 0,
            accounting_error_count: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuResourceTelemetrySnapshot {
    pub classes: [GpuResourceClassSnapshot; GPU_RESOURCE_CLASS_COUNT],
}

impl Default for GpuResourceTelemetrySnapshot {
    fn default() -> Self {
        Self {
            classes: GpuResourceClass::all().map(GpuResourceClassSnapshot::empty),
        }
    }
}

impl GpuResourceTelemetrySnapshot {
    pub fn class(&self, class: GpuResourceClass) -> GpuResourceClassSnapshot {
        self.classes[class.index()]
    }

    pub(crate) fn observe_owner_domain(
        &mut self,
        class: GpuResourceClass,
        resource_count: u64,
        known_live_bytes: u64,
        coverage: ResourceCoverage,
    ) {
        let row = &mut self.classes[class.index()];
        row.coverage = coverage;
        row.live_count = resource_count;
        row.known_live_bytes = known_live_bytes;
        row.peak_live_count = row.peak_live_count.max(resource_count);
        row.peak_known_live_bytes = row.peak_known_live_bytes.max(known_live_bytes);
    }
}

struct ResourceClassCounters {
    live_count: AtomicU64,
    known_live_bytes: AtomicU64,
    peak_live_count: AtomicU64,
    peak_known_live_bytes: AtomicU64,
    allocation_count_total: AtomicU64,
    allocated_known_bytes_total: AtomicU64,
    retirement_count_total: AtomicU64,
    retired_known_bytes_total: AtomicU64,
    resize_count: AtomicU64,
    unknown_size_live_count: AtomicU64,
    accounting_error_count: AtomicU64,
}

impl ResourceClassCounters {
    const fn new() -> Self {
        Self {
            live_count: AtomicU64::new(0),
            known_live_bytes: AtomicU64::new(0),
            peak_live_count: AtomicU64::new(0),
            peak_known_live_bytes: AtomicU64::new(0),
            allocation_count_total: AtomicU64::new(0),
            allocated_known_bytes_total: AtomicU64::new(0),
            retirement_count_total: AtomicU64::new(0),
            retired_known_bytes_total: AtomicU64::new(0),
            resize_count: AtomicU64::new(0),
            unknown_size_live_count: AtomicU64::new(0),
            accounting_error_count: AtomicU64::new(0),
        }
    }
}

static RESOURCE_CLASS_COUNTERS: [ResourceClassCounters; GPU_RESOURCE_CLASS_COUNT] =
    [const { ResourceClassCounters::new() }; GPU_RESOURCE_CLASS_COUNT];

fn counters() -> &'static [ResourceClassCounters; GPU_RESOURCE_CLASS_COUNT] {
    &RESOURCE_CLASS_COUNTERS
}

/// Record an owner-confirmed resource creation. No handle or owner is retained.
pub fn record_gpu_resource_allocation(class: GpuResourceClass, bytes: Option<u64>) {
    let counters = &counters()[class.index()];
    saturating_add(&counters.allocation_count_total, 1);
    let live_count = saturating_add(&counters.live_count, 1);
    counters
        .peak_live_count
        .fetch_max(live_count, Ordering::Relaxed);
    match class.byte_semantics() {
        GpuResourceByteSemantics::OwnedStorageEstimate
        | GpuResourceByteSemantics::ExternalBackingLogical => match bytes {
            Some(bytes) => {
                saturating_add(&counters.allocated_known_bytes_total, bytes);
                let live_bytes = saturating_add(&counters.known_live_bytes, bytes);
                counters
                    .peak_known_live_bytes
                    .fetch_max(live_bytes, Ordering::Relaxed);
            }
            None => {
                saturating_add(&counters.unknown_size_live_count, 1);
            }
        },
        GpuResourceByteSemantics::AliasViewNoStorage => {
            if bytes.is_some() {
                saturating_add(&counters.accounting_error_count, 1);
            }
        }
        GpuResourceByteSemantics::DriverManagedUnknown => {
            saturating_add(&counters.unknown_size_live_count, 1);
            if bytes.is_some() {
                saturating_add(&counters.accounting_error_count, 1);
            }
        }
    }
}

/// Record an owner-confirmed resource retirement after its lifetime ended.
pub fn record_gpu_resource_retirement(class: GpuResourceClass, bytes: Option<u64>) {
    let counters = &counters()[class.index()];
    saturating_add(&counters.retirement_count_total, 1);
    subtract_or_record_error(&counters.live_count, 1, &counters.accounting_error_count);
    match class.byte_semantics() {
        GpuResourceByteSemantics::OwnedStorageEstimate
        | GpuResourceByteSemantics::ExternalBackingLogical => match bytes {
            Some(bytes) => {
                saturating_add(&counters.retired_known_bytes_total, bytes);
                subtract_or_record_error(
                    &counters.known_live_bytes,
                    bytes,
                    &counters.accounting_error_count,
                );
            }
            None => subtract_or_record_error(
                &counters.unknown_size_live_count,
                1,
                &counters.accounting_error_count,
            ),
        },
        GpuResourceByteSemantics::AliasViewNoStorage => {
            if bytes.is_some() {
                saturating_add(&counters.accounting_error_count, 1);
            }
        }
        GpuResourceByteSemantics::DriverManagedUnknown => {
            subtract_or_record_error(
                &counters.unknown_size_live_count,
                1,
                &counters.accounting_error_count,
            );
            if bytes.is_some() {
                saturating_add(&counters.accounting_error_count, 1);
            }
        }
    }
}

/// Record a size change without changing the resource count.
pub fn record_gpu_resource_resize(
    class: GpuResourceClass,
    old_bytes: Option<u64>,
    new_bytes: Option<u64>,
) {
    let counters = &counters()[class.index()];
    if !matches!(
        class.byte_semantics(),
        GpuResourceByteSemantics::OwnedStorageEstimate
            | GpuResourceByteSemantics::ExternalBackingLogical
    ) {
        saturating_add(&counters.accounting_error_count, 1);
        return;
    }
    saturating_add(&counters.resize_count, 1);
    match old_bytes {
        Some(bytes) => {
            saturating_add(&counters.retired_known_bytes_total, bytes);
            subtract_or_record_error(
                &counters.known_live_bytes,
                bytes,
                &counters.accounting_error_count,
            );
        }
        None => subtract_or_record_error(
            &counters.unknown_size_live_count,
            1,
            &counters.accounting_error_count,
        ),
    }
    match new_bytes {
        Some(bytes) => {
            saturating_add(&counters.allocated_known_bytes_total, bytes);
            let live_bytes = saturating_add(&counters.known_live_bytes, bytes);
            counters
                .peak_known_live_bytes
                .fetch_max(live_bytes, Ordering::Relaxed);
        }
        None => {
            saturating_add(&counters.unknown_size_live_count, 1);
        }
    }
}

/// Return a copy-only snapshot of the fixed counter table.
pub fn gpu_resource_telemetry_snapshot() -> GpuResourceTelemetrySnapshot {
    let counters = counters();
    GpuResourceTelemetrySnapshot {
        classes: GpuResourceClass::all().map(|class| {
            let item = &counters[class.index()];
            GpuResourceClassSnapshot {
                class,
                byte_semantics: class.byte_semantics(),
                coverage: class.initial_coverage(),
                live_count: item.live_count.load(Ordering::Relaxed),
                known_live_bytes: item.known_live_bytes.load(Ordering::Relaxed),
                peak_live_count: item.peak_live_count.load(Ordering::Relaxed),
                peak_known_live_bytes: item.peak_known_live_bytes.load(Ordering::Relaxed),
                allocation_count_total: item.allocation_count_total.load(Ordering::Relaxed),
                allocated_known_bytes_total: item
                    .allocated_known_bytes_total
                    .load(Ordering::Relaxed),
                retirement_count_total: item.retirement_count_total.load(Ordering::Relaxed),
                retired_known_bytes_total: item.retired_known_bytes_total.load(Ordering::Relaxed),
                resize_count: item.resize_count.load(Ordering::Relaxed),
                unknown_size_live_count: item.unknown_size_live_count.load(Ordering::Relaxed),
                accounting_error_count: item.accounting_error_count.load(Ordering::Relaxed),
            }
        }),
    }
}

fn saturating_add(counter: &AtomicU64, amount: u64) -> u64 {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(current.saturating_add(amount))
        })
        .unwrap_or_else(|previous| previous)
        .saturating_add(amount)
}

fn subtract_or_record_error(counter: &AtomicU64, amount: u64, errors: &AtomicU64) {
    if counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_sub(amount)
        })
        .is_err()
    {
        saturating_add(errors, 1);
    }
}
