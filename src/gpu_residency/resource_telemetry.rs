//! Fixed, observe-only allocation and driver-object counters.
//!
//! This ledger stores scalar counters only. Resource owners remain responsible
//! for handles, synchronization, and destruction on the correct thread.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use super::ResourceCoverage;

pub const GPU_RESOURCE_CLASS_COUNT: usize = 24;
const GPU_RESOURCE_SNAPSHOT_MAX_ATTEMPTS: u64 = 8;

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
            Self::OutputBacking | Self::DrmFramebuffer | Self::GemHandle => {
                ResourceCoverage::PartiallyInstrumented
            }
            Self::RendererDmabufExternalBacking
            | Self::DirectScanoutDmabufExternalBacking
            | Self::ShaderProgram
            | Self::EglContext
            | Self::EglSurface => ResourceCoverage::Instrumented,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GpuResourceLiveStateSource {
    EventLedger,
    OwnerSnapshot,
    Reconciled,
    #[default]
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuResourceClassSnapshot {
    pub class: GpuResourceClass,
    pub byte_semantics: GpuResourceByteSemantics,
    pub coverage: ResourceCoverage,
    #[serde(default)]
    pub live_state_coverage: ResourceCoverage,
    #[serde(default)]
    pub lifecycle_event_coverage: ResourceCoverage,
    #[serde(default)]
    pub event_live_count: u64,
    #[serde(default)]
    pub event_known_live_bytes: u64,
    #[serde(default)]
    pub owner_observed_live_count: Option<u64>,
    #[serde(default)]
    pub owner_observed_known_live_bytes: Option<u64>,
    #[serde(default)]
    pub owner_observed_coverage: Option<ResourceCoverage>,
    #[serde(default)]
    pub effective_live_count: u64,
    #[serde(default)]
    pub effective_known_live_bytes: u64,
    #[serde(default)]
    pub live_state_source: GpuResourceLiveStateSource,
    /// Positive values mean event-ledger state exceeds owner-observed state.
    #[serde(default)]
    pub live_count_delta: Option<i64>,
    /// Positive values mean event-ledger bytes exceed owner-observed bytes.
    #[serde(default)]
    pub known_live_bytes_delta: Option<i64>,
    #[serde(default)]
    pub live_state_matches: Option<bool>,
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
            live_state_coverage: class.initial_coverage(),
            lifecycle_event_coverage: class.initial_coverage(),
            event_live_count: 0,
            event_known_live_bytes: 0,
            owner_observed_live_count: None,
            owner_observed_known_live_bytes: None,
            owner_observed_coverage: None,
            effective_live_count: 0,
            effective_known_live_bytes: 0,
            live_state_source: if matches!(
                class.initial_coverage(),
                ResourceCoverage::Instrumented | ResourceCoverage::PartiallyInstrumented
            ) {
                GpuResourceLiveStateSource::EventLedger
            } else {
                GpuResourceLiveStateSource::Unavailable
            },
            live_count_delta: None,
            known_live_bytes_delta: None,
            live_state_matches: None,
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
    #[serde(default)]
    pub coherent: bool,
    #[serde(default)]
    pub event_sequence: u64,
    #[serde(default)]
    pub snapshot_retries: u64,
}

impl Default for GpuResourceTelemetrySnapshot {
    fn default() -> Self {
        Self {
            classes: GpuResourceClass::all().map(GpuResourceClassSnapshot::empty),
            coherent: false,
            event_sequence: 0,
            snapshot_retries: 0,
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
        row.owner_observed_live_count = Some(resource_count);
        row.owner_observed_known_live_bytes = Some(known_live_bytes);
        row.owner_observed_coverage = Some(coverage);
        row.recompute_effective_live_state();
        row.peak_live_count = row.peak_live_count.max(row.effective_live_count);
        row.peak_known_live_bytes = row
            .peak_known_live_bytes
            .max(row.effective_known_live_bytes);
    }
}

impl GpuResourceClassSnapshot {
    pub(crate) fn recompute_effective_live_state(&mut self) {
        match (
            self.owner_observed_live_count,
            self.owner_observed_known_live_bytes,
        ) {
            (Some(owner_count), Some(owner_bytes)) => {
                let matches = self.event_live_count == owner_count
                    && self.event_known_live_bytes == owner_bytes;
                self.live_count_delta = Some(signed_delta(self.event_live_count, owner_count));
                self.known_live_bytes_delta =
                    Some(signed_delta(self.event_known_live_bytes, owner_bytes));
                self.live_state_matches = Some(matches);
                if self.lifecycle_event_coverage == ResourceCoverage::Instrumented {
                    self.effective_live_count = self.event_live_count;
                    self.effective_known_live_bytes = self.event_known_live_bytes;
                    self.live_state_coverage = ResourceCoverage::Instrumented;
                    self.live_state_source = if matches {
                        GpuResourceLiveStateSource::Reconciled
                    } else {
                        GpuResourceLiveStateSource::EventLedger
                    };
                } else {
                    self.effective_live_count = owner_count;
                    self.effective_known_live_bytes = owner_bytes;
                    self.live_state_coverage = self
                        .owner_observed_coverage
                        .unwrap_or(ResourceCoverage::Uninstrumented);
                    self.live_state_source = GpuResourceLiveStateSource::OwnerSnapshot;
                }
            }
            _ if self.lifecycle_event_coverage != ResourceCoverage::Uninstrumented
                || self.event_live_count > 0 =>
            {
                self.effective_live_count = self.event_live_count;
                self.effective_known_live_bytes = self.event_known_live_bytes;
                self.live_state_coverage = self.lifecycle_event_coverage;
                self.live_state_source = GpuResourceLiveStateSource::EventLedger;
                self.live_count_delta = None;
                self.known_live_bytes_delta = None;
                self.live_state_matches = None;
            }
            _ => {
                self.effective_live_count = 0;
                self.effective_known_live_bytes = 0;
                self.live_state_coverage = ResourceCoverage::Uninstrumented;
                self.live_state_source = GpuResourceLiveStateSource::Unavailable;
                self.live_count_delta = None;
                self.known_live_bytes_delta = None;
                self.live_state_matches = None;
            }
        }
        // The legacy wire fields have always represented the effective live
        // state after optional owner observation.
        self.live_count = self.effective_live_count;
        self.known_live_bytes = self.effective_known_live_bytes;
    }
}

fn signed_delta(event: u64, owner: u64) -> i64 {
    if event >= owner {
        i64::try_from(event - owner).unwrap_or(i64::MAX)
    } else {
        -i64::try_from(owner - event).unwrap_or(i64::MAX)
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

struct ResourceLedger {
    counters: [ResourceClassCounters; GPU_RESOURCE_CLASS_COUNT],
    active_writers: AtomicU64,
    event_sequence: AtomicU64,
    poisoned: AtomicBool,
}

impl ResourceLedger {
    const fn new() -> Self {
        Self {
            counters: [const { ResourceClassCounters::new() }; GPU_RESOURCE_CLASS_COUNT],
            active_writers: AtomicU64::new(0),
            event_sequence: AtomicU64::new(0),
            poisoned: AtomicBool::new(false),
        }
    }

    fn begin_event(&self) -> ActiveWriterGuard<'_> {
        // The acquire half prevents event mutations from moving before the
        // writer announcement; the release half participates in the atomic
        // writer-count modification order used by snapshot readers.
        self.active_writers.fetch_add(1, Ordering::AcqRel);
        ActiveWriterGuard {
            ledger: self,
            completed: false,
        }
    }

    fn record_allocation(&self, class: GpuResourceClass, bytes: Option<u64>) {
        let writer = self.begin_event();
        let counters = &self.counters[class.index()];
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
        writer.complete();
    }

    fn record_retirement(&self, class: GpuResourceClass, bytes: Option<u64>) {
        let writer = self.begin_event();
        let counters = &self.counters[class.index()];
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
        writer.complete();
    }

    fn record_resize(
        &self,
        class: GpuResourceClass,
        old_bytes: Option<u64>,
        new_bytes: Option<u64>,
    ) {
        let writer = self.begin_event();
        let counters = &self.counters[class.index()];
        if !matches!(
            class.byte_semantics(),
            GpuResourceByteSemantics::OwnedStorageEstimate
                | GpuResourceByteSemantics::ExternalBackingLogical
        ) {
            saturating_add(&counters.accounting_error_count, 1);
            writer.complete();
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
        writer.complete();
    }

    fn snapshot(&self) -> GpuResourceTelemetrySnapshot {
        let mut last = GpuResourceTelemetrySnapshot::default();
        for attempt in 0..GPU_RESOURCE_SNAPSHOT_MAX_ATTEMPTS {
            let writers_before = self.active_writers.load(Ordering::Acquire);
            let sequence_before = self.event_sequence.load(Ordering::Acquire);
            last = self.copy_counters();
            let sequence_after = self.event_sequence.load(Ordering::Acquire);
            let writers_after = self.active_writers.load(Ordering::Acquire);
            let poisoned = self.poisoned.load(Ordering::Acquire);
            last.event_sequence = sequence_after;
            if writers_before == 0
                && writers_after == 0
                && sequence_before == sequence_after
                && !poisoned
            {
                last.coherent = true;
                last.snapshot_retries = attempt;
                return last;
            }
            last.coherent = false;
            last.snapshot_retries = attempt.saturating_add(1);
        }
        last
    }

    fn copy_counters(&self) -> GpuResourceTelemetrySnapshot {
        let mut snapshot = GpuResourceTelemetrySnapshot::default();
        snapshot.classes = GpuResourceClass::all().map(|class| {
            let item = &self.counters[class.index()];
            let mut row = GpuResourceClassSnapshot::empty(class);
            row.event_live_count = item.live_count.load(Ordering::Relaxed);
            row.event_known_live_bytes = item.known_live_bytes.load(Ordering::Relaxed);
            row.effective_live_count = row.event_live_count;
            row.effective_known_live_bytes = row.event_known_live_bytes;
            row.live_count = row.event_live_count;
            row.known_live_bytes = row.event_known_live_bytes;
            row.peak_live_count = item.peak_live_count.load(Ordering::Relaxed);
            row.peak_known_live_bytes = item.peak_known_live_bytes.load(Ordering::Relaxed);
            row.allocation_count_total = item.allocation_count_total.load(Ordering::Relaxed);
            row.allocated_known_bytes_total =
                item.allocated_known_bytes_total.load(Ordering::Relaxed);
            row.retirement_count_total = item.retirement_count_total.load(Ordering::Relaxed);
            row.retired_known_bytes_total = item.retired_known_bytes_total.load(Ordering::Relaxed);
            row.resize_count = item.resize_count.load(Ordering::Relaxed);
            row.unknown_size_live_count = item.unknown_size_live_count.load(Ordering::Relaxed);
            row.accounting_error_count = item.accounting_error_count.load(Ordering::Relaxed);
            if row.coverage == ResourceCoverage::Uninstrumented
                && (row.allocation_count_total > 0 || row.retirement_count_total > 0)
            {
                row.live_state_source = GpuResourceLiveStateSource::EventLedger;
            }
            row
        });
        snapshot
    }
}

struct ActiveWriterGuard<'a> {
    ledger: &'a ResourceLedger,
    completed: bool,
}

impl ActiveWriterGuard<'_> {
    fn complete(mut self) {
        // Acquire the prior completion so concurrent writers form a
        // transitive happens-before chain through this scalar sequence. The
        // release half publishes this event's counter mutations to a reader
        // that accepts the resulting stable sequence.
        let previous = self.ledger.event_sequence.fetch_add(1, Ordering::AcqRel);
        if previous >= u64::MAX - 1 {
            // Saturate the diagnostic sequence and poison coherence before the
            // writer is retired. At this limit no future stable boundary can
            // be proven, so avoid a contended CAS loop on this hot path.
            self.ledger.poisoned.store(true, Ordering::Release);
            self.ledger
                .event_sequence
                .store(u64::MAX, Ordering::Release);
        }
        self.completed = true;
    }
}

impl Drop for ActiveWriterGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.ledger.poisoned.store(true, Ordering::Release);
        }
        // Counter mutations and the completed event sequence happen before
        // this release. A snapshot acquire that observes the quiescent writer
        // count therefore observes the complete event.
        self.ledger.active_writers.fetch_sub(1, Ordering::Release);
    }
}

static RESOURCE_LEDGER: ResourceLedger = ResourceLedger::new();

/// Record an owner-confirmed resource creation. No handle or owner is retained.
pub fn record_gpu_resource_allocation(class: GpuResourceClass, bytes: Option<u64>) {
    RESOURCE_LEDGER.record_allocation(class, bytes);
}

/// Record an owner-confirmed resource retirement after its lifetime ended.
pub fn record_gpu_resource_retirement(class: GpuResourceClass, bytes: Option<u64>) {
    RESOURCE_LEDGER.record_retirement(class, bytes);
}

/// Record a size change without changing the resource count.
pub fn record_gpu_resource_resize(
    class: GpuResourceClass,
    old_bytes: Option<u64>,
    new_bytes: Option<u64>,
) {
    RESOURCE_LEDGER.record_resize(class, old_bytes, new_bytes);
}

/// Return a copy-only snapshot of the fixed counter table.
pub fn gpu_resource_telemetry_snapshot() -> GpuResourceTelemetrySnapshot {
    RESOURCE_LEDGER.snapshot()
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

#[cfg(test)]
mod tests {
    use super::{
        GPU_RESOURCE_SNAPSHOT_MAX_ATTEMPTS, GpuResourceClass, ResourceLedger,
        gpu_resource_telemetry_snapshot,
    };
    use std::sync::{Arc, Barrier, atomic::Ordering};

    #[test]
    fn stable_snapshot_reports_a_coherent_event_boundary() {
        let snapshot = serde_json::to_value(gpu_resource_telemetry_snapshot())
            .expect("resource telemetry snapshot serializes");

        assert!(snapshot["coherent"].is_boolean());
        assert!(snapshot["event_sequence"].as_u64().is_some());
        assert!(snapshot["snapshot_retries"].as_u64().is_some());
    }

    #[test]
    fn allocation_updates_all_related_counters_as_one_event() {
        let ledger = ResourceLedger::new();
        ledger.record_allocation(GpuResourceClass::OutputBacking, Some(128));

        let snapshot = ledger.snapshot();
        let row = snapshot.class(GpuResourceClass::OutputBacking);
        assert!(snapshot.coherent);
        assert_eq!(snapshot.event_sequence, 1);
        assert_eq!(row.event_live_count, 1);
        assert_eq!(row.event_known_live_bytes, 128);
        assert_eq!(row.allocation_count_total, 1);
        assert_eq!(row.allocated_known_bytes_total, 128);
        assert_eq!(row.peak_live_count, 1);
        assert_eq!(row.peak_known_live_bytes, 128);
        assert_eq!(row.accounting_error_count, 0);
    }

    #[test]
    fn renderer_external_mapping_and_alias_views_have_independent_lifetimes() {
        let ledger = ResourceLedger::new();
        let logical_bytes = 2_048;
        ledger.record_allocation(
            GpuResourceClass::RendererDmabufExternalBacking,
            Some(logical_bytes),
        );
        ledger.record_allocation(GpuResourceClass::ImportedEglImageView, None);
        ledger.record_allocation(GpuResourceClass::ImportedGlTextureView, None);

        let live = ledger.snapshot();
        assert_eq!(
            live.class(GpuResourceClass::RendererDmabufExternalBacking)
                .event_known_live_bytes,
            logical_bytes
        );
        assert_eq!(
            live.class(GpuResourceClass::ImportedEglImageView)
                .event_known_live_bytes,
            0
        );
        assert_eq!(
            live.class(GpuResourceClass::ImportedGlTextureView)
                .event_known_live_bytes,
            0
        );

        ledger.record_retirement(GpuResourceClass::ImportedEglImageView, None);
        assert_eq!(
            ledger
                .snapshot()
                .class(GpuResourceClass::RendererDmabufExternalBacking)
                .event_live_count,
            1
        );
        ledger.record_retirement(
            GpuResourceClass::RendererDmabufExternalBacking,
            Some(logical_bytes),
        );
        assert_eq!(
            ledger
                .snapshot()
                .class(GpuResourceClass::RendererDmabufExternalBacking)
                .event_live_count,
            0
        );
    }

    #[test]
    fn unknown_renderer_mapping_size_remains_live_without_invented_bytes() {
        let ledger = ResourceLedger::new();
        ledger.record_allocation(GpuResourceClass::RendererDmabufExternalBacking, None);
        let row = ledger
            .snapshot()
            .class(GpuResourceClass::RendererDmabufExternalBacking);
        assert_eq!(row.event_live_count, 1);
        assert_eq!(row.event_known_live_bytes, 0);
        assert_eq!(row.unknown_size_live_count, 1);
    }

    #[test]
    fn retirement_returns_event_live_state_to_zero() {
        let ledger = ResourceLedger::new();
        ledger.record_allocation(GpuResourceClass::OutputBacking, Some(128));
        ledger.record_retirement(GpuResourceClass::OutputBacking, Some(128));

        let row = ledger.snapshot().class(GpuResourceClass::OutputBacking);
        assert_eq!(row.event_live_count, 0);
        assert_eq!(row.event_known_live_bytes, 0);
        assert_eq!(row.retirement_count_total, 1);
        assert_eq!(row.retired_known_bytes_total, 128);
        assert_eq!(row.accounting_error_count, 0);
    }

    #[test]
    fn known_to_known_resize_retires_old_and_allocates_new_size() {
        let ledger = ResourceLedger::new();
        ledger.record_allocation(GpuResourceClass::OutputBacking, Some(32));
        ledger.record_resize(GpuResourceClass::OutputBacking, Some(32), Some(48));

        let row = ledger.snapshot().class(GpuResourceClass::OutputBacking);
        assert_eq!(row.event_live_count, 1);
        assert_eq!(row.event_known_live_bytes, 48);
        assert_eq!(row.resize_count, 1);
        assert_eq!(row.allocated_known_bytes_total, 80);
        assert_eq!(row.retired_known_bytes_total, 32);
        assert_eq!(row.accounting_error_count, 0);
    }

    #[test]
    fn known_to_unknown_resize_tracks_unknown_live_size() {
        let ledger = ResourceLedger::new();
        ledger.record_allocation(GpuResourceClass::OutputBacking, Some(32));
        ledger.record_resize(GpuResourceClass::OutputBacking, Some(32), None);

        let row = ledger.snapshot().class(GpuResourceClass::OutputBacking);
        assert_eq!(row.event_live_count, 1);
        assert_eq!(row.event_known_live_bytes, 0);
        assert_eq!(row.unknown_size_live_count, 1);
        assert_eq!(row.accounting_error_count, 0);
    }

    #[test]
    fn unknown_to_known_resize_tracks_known_live_size() {
        let ledger = ResourceLedger::new();
        ledger.record_allocation(GpuResourceClass::OutputBacking, None);
        ledger.record_resize(GpuResourceClass::OutputBacking, None, Some(32));

        let row = ledger.snapshot().class(GpuResourceClass::OutputBacking);
        assert_eq!(row.event_live_count, 1);
        assert_eq!(row.event_known_live_bytes, 32);
        assert_eq!(row.unknown_size_live_count, 0);
        assert_eq!(row.accounting_error_count, 0);
    }

    #[test]
    fn alias_classes_reject_byte_payloads() {
        let ledger = ResourceLedger::new();
        ledger.record_allocation(GpuResourceClass::ImportedEglImageView, Some(32));

        let row = ledger
            .snapshot()
            .class(GpuResourceClass::ImportedEglImageView);
        assert_eq!(row.event_live_count, 1);
        assert_eq!(row.event_known_live_bytes, 0);
        assert_eq!(row.accounting_error_count, 1);
    }

    #[test]
    fn driver_managed_classes_keep_byte_cost_unknown() {
        let ledger = ResourceLedger::new();
        ledger.record_allocation(GpuResourceClass::EglContext, Some(32));

        let row = ledger.snapshot().class(GpuResourceClass::EglContext);
        assert_eq!(row.event_live_count, 1);
        assert_eq!(row.event_known_live_bytes, 0);
        assert_eq!(row.unknown_size_live_count, 1);
        assert_eq!(row.accounting_error_count, 1);
    }

    #[test]
    fn retirement_underflow_records_an_error_without_wrapping() {
        let ledger = ResourceLedger::new();
        ledger.record_retirement(GpuResourceClass::OutputBacking, Some(32));

        let row = ledger.snapshot().class(GpuResourceClass::OutputBacking);
        assert_eq!(row.event_live_count, 0);
        assert_eq!(row.event_known_live_bytes, 0);
        assert_eq!(row.accounting_error_count, 2);
    }

    #[test]
    fn peak_values_remain_monotonic_after_retirement() {
        let ledger = ResourceLedger::new();
        ledger.record_allocation(GpuResourceClass::OutputBacking, Some(64));
        ledger.record_retirement(GpuResourceClass::OutputBacking, Some(64));
        ledger.record_allocation(GpuResourceClass::OutputBacking, Some(16));

        let row = ledger.snapshot().class(GpuResourceClass::OutputBacking);
        assert_eq!(row.peak_live_count, 1);
        assert_eq!(row.peak_known_live_bytes, 64);
        assert_eq!(row.event_live_count, 1);
        assert_eq!(row.event_known_live_bytes, 16);
    }

    #[test]
    fn saturating_counters_never_wrap() {
        let ledger = ResourceLedger::new();
        let counters = &ledger.counters[GpuResourceClass::OutputBacking.index()];
        counters
            .allocation_count_total
            .store(u64::MAX, std::sync::atomic::Ordering::Relaxed);
        counters
            .live_count
            .store(u64::MAX, std::sync::atomic::Ordering::Relaxed);
        counters
            .peak_live_count
            .store(u64::MAX, std::sync::atomic::Ordering::Relaxed);
        counters
            .allocated_known_bytes_total
            .store(u64::MAX, std::sync::atomic::Ordering::Relaxed);
        counters
            .known_live_bytes
            .store(u64::MAX, std::sync::atomic::Ordering::Relaxed);
        counters
            .peak_known_live_bytes
            .store(u64::MAX, std::sync::atomic::Ordering::Relaxed);

        ledger.record_allocation(GpuResourceClass::OutputBacking, Some(1));

        let row = ledger.snapshot().class(GpuResourceClass::OutputBacking);
        assert_eq!(row.allocation_count_total, u64::MAX);
        assert_eq!(row.event_live_count, u64::MAX);
        assert_eq!(row.allocated_known_bytes_total, u64::MAX);
        assert_eq!(row.event_known_live_bytes, u64::MAX);
        assert_eq!(row.peak_live_count, u64::MAX);
        assert_eq!(row.peak_known_live_bytes, u64::MAX);
    }

    #[test]
    fn snapshot_rejects_a_paused_half_written_event() {
        let ledger = ResourceLedger::new();
        let writer = ledger.begin_event();
        ledger.counters[GpuResourceClass::OutputBacking.index()]
            .live_count
            .store(1, std::sync::atomic::Ordering::Relaxed);

        let snapshot = ledger.snapshot();
        assert!(!snapshot.coherent);
        assert_eq!(snapshot.event_sequence, 0);
        assert_eq!(
            snapshot.snapshot_retries,
            GPU_RESOURCE_SNAPSHOT_MAX_ATTEMPTS
        );

        drop(writer);
        assert!(!ledger.snapshot().coherent);
    }

    #[test]
    fn concurrent_writers_cannot_publish_a_coherent_mixed_snapshot() {
        let ledger = Arc::new(ResourceLedger::new());
        let barrier = Arc::new(Barrier::new(3));
        let handles = [
            GpuResourceClass::OutputBacking,
            GpuResourceClass::CursorBacking,
        ]
        .into_iter()
        .map(|class| {
            let ledger = Arc::clone(&ledger);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let writer = ledger.begin_event();
                let counters = &ledger.counters[class.index()];
                counters
                    .live_count
                    .store(1, std::sync::atomic::Ordering::Relaxed);
                barrier.wait();
                barrier.wait();
                counters
                    .allocation_count_total
                    .store(1, std::sync::atomic::Ordering::Relaxed);
                writer.complete();
            })
        })
        .collect::<Vec<_>>();

        barrier.wait();
        let during_events = ledger.snapshot();
        assert!(!during_events.coherent);
        assert_eq!(during_events.event_sequence, 0);
        barrier.wait();
        for handle in handles {
            handle.join().expect("writer thread completes");
        }

        let stable = ledger.snapshot();
        assert!(stable.coherent);
        assert_eq!(stable.event_sequence, 2);
        for class in [
            GpuResourceClass::OutputBacking,
            GpuResourceClass::CursorBacking,
        ] {
            let row = stable.class(class);
            assert_eq!(row.event_live_count, 1);
            assert_eq!(row.allocation_count_total, 1);
        }
    }

    #[test]
    fn bounded_retries_return_incoherent_without_spinning_forever() {
        let ledger = ResourceLedger::new();
        let writer = ledger.begin_event();

        let snapshot = ledger.snapshot();
        assert!(!snapshot.coherent);
        assert_eq!(
            snapshot.snapshot_retries,
            GPU_RESOURCE_SNAPSHOT_MAX_ATTEMPTS
        );

        writer.complete();
        assert!(ledger.snapshot().coherent);
    }

    #[test]
    fn stable_local_snapshot_is_coherent_without_retries() {
        let snapshot = ResourceLedger::new().snapshot();

        assert!(snapshot.coherent);
        assert_eq!(snapshot.event_sequence, 0);
        assert_eq!(snapshot.snapshot_retries, 0);
    }

    #[test]
    fn event_sequence_changes_only_when_an_event_completes() {
        let ledger = ResourceLedger::new();
        let writer = ledger.begin_event();
        let in_flight = ledger.snapshot();

        assert_eq!(in_flight.event_sequence, 0);
        assert!(!in_flight.coherent);
        writer.complete();

        let completed = ledger.snapshot();
        assert_eq!(completed.event_sequence, 1);
        assert!(completed.coherent);
    }

    #[test]
    fn event_sequence_saturation_never_wraps_or_reports_coherence() {
        let ledger = ResourceLedger::new();
        ledger.event_sequence.store(u64::MAX - 1, Ordering::Relaxed);

        ledger.record_allocation(GpuResourceClass::OutputBacking, Some(1));

        let snapshot = ledger.snapshot();
        assert_eq!(snapshot.event_sequence, u64::MAX);
        assert!(!snapshot.coherent);
        ledger.record_allocation(GpuResourceClass::OutputBacking, Some(1));
        assert_eq!(ledger.event_sequence.load(Ordering::Acquire), u64::MAX);
    }
}
