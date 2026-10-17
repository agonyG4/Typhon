//! Observe-only descriptions of compositor GPU resource owners.
//!
//! The owner subsystems remain authoritative for resource lifetime. This module
//! contains only copyable summaries and generation-tagged accounting events.

use serde::{Deserialize, Serialize};

mod resource_telemetry;
mod scratch_telemetry;
pub use resource_telemetry::{
    GPU_RESOURCE_CLASS_COUNT, GpuResourceByteSemantics, GpuResourceClass, GpuResourceClassSnapshot,
    GpuResourceLiveStateSource, GpuResourceTelemetrySnapshot, gpu_resource_telemetry_snapshot,
    record_gpu_resource_allocation, record_gpu_resource_resize, record_gpu_resource_retirement,
};
pub use scratch_telemetry::{
    AtomicEffectScratchResidencySnapshot, EffectScratchResidencyLiveSnapshot,
    ScratchResidencyTelemetry,
};

pub const RESOURCE_DOMAIN_COUNT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceDomain {
    OutputScanout,
    EffectsScratch,
    EffectsCheckpoint,
    ClientImport,
    LifecycleRetainedVisual,
    Capture,
    CursorDecoration,
    OtherUnknown,
}

impl ResourceDomain {
    const fn index(self) -> usize {
        match self {
            Self::OutputScanout => 0,
            Self::EffectsScratch => 1,
            Self::EffectsCheckpoint => 2,
            Self::ClientImport => 3,
            Self::LifecycleRetainedVisual => 4,
            Self::Capture => 5,
            Self::CursorDecoration => 6,
            Self::OtherUnknown => 7,
        }
    }

    const fn all() -> [Self; RESOURCE_DOMAIN_COUNT] {
        [
            Self::OutputScanout,
            Self::EffectsScratch,
            Self::EffectsCheckpoint,
            Self::ClientImport,
            Self::LifecycleRetainedVisual,
            Self::Capture,
            Self::CursorDecoration,
            Self::OtherUnknown,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuResidencyOwner {
    NativeOutput,
    EglRenderer,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceOwnership {
    CompositorOwned,
    ClientImportedExternallyBacked,
    DriverOrContextUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceResidency {
    ScanoutKmsPinned,
    SynchronizationPinned,
    Active,
    WarmReusable,
    ColdReusable,
    FixedPoolReusable,
    ExternalMapping,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceCoverage {
    Instrumented,
    PartiallyInstrumented,
    Uninstrumented,
}

impl Default for ResourceCoverage {
    fn default() -> Self {
        Self::Uninstrumented
    }
}

/// One fixed owner/domain row. Byte totals are estimates or logical payloads,
/// and state counters are descriptive rather than reclamation authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuResidencyDomainSnapshot {
    pub owner: GpuResidencyOwner,
    pub domain: ResourceDomain,
    pub ownership: ResourceOwnership,
    pub coverage: ResourceCoverage,
    pub owner_generation: u64,
    pub resource_count: u64,
    pub compositor_owned_estimated_bytes: u64,
    pub externally_backed_logical_bytes: u64,
    pub unique_observed_dmabuf_bytes: u64,
    pub pinned_count: u64,
    pub pinned_bytes: u64,
    pub active_count: u64,
    pub active_bytes: u64,
    pub reusable_count: u64,
    pub reusable_bytes: u64,
    pub fixed_pool_reusable_count: u64,
    pub fixed_pool_reusable_bytes: u64,
    pub external_mapping_count: u64,
    pub external_mapping_bytes: u64,
    pub unknown_count: u64,
    pub unknown_bytes: u64,
    pub backing_identity_ambiguous_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scratch_residency: Option<ScratchResidencyTelemetry>,
}

impl Default for GpuResidencyDomainSnapshot {
    fn default() -> Self {
        Self::new(
            GpuResidencyOwner::Unknown,
            ResourceDomain::OtherUnknown,
            ResourceOwnership::DriverOrContextUnknown,
            ResourceCoverage::Uninstrumented,
        )
    }
}

impl GpuResidencyDomainSnapshot {
    pub const fn new(
        owner: GpuResidencyOwner,
        domain: ResourceDomain,
        ownership: ResourceOwnership,
        coverage: ResourceCoverage,
    ) -> Self {
        Self {
            owner,
            domain,
            ownership,
            coverage,
            owner_generation: 0,
            resource_count: 0,
            compositor_owned_estimated_bytes: 0,
            externally_backed_logical_bytes: 0,
            unique_observed_dmabuf_bytes: 0,
            pinned_count: 0,
            pinned_bytes: 0,
            active_count: 0,
            active_bytes: 0,
            reusable_count: 0,
            reusable_bytes: 0,
            fixed_pool_reusable_count: 0,
            fixed_pool_reusable_bytes: 0,
            external_mapping_count: 0,
            external_mapping_bytes: 0,
            unknown_count: 0,
            unknown_bytes: 0,
            backing_identity_ambiguous_count: 0,
            scratch_residency: None,
        }
    }

    /// Add an owner-confirmed observation. `bytes` means a compositor estimate
    /// for compositor-owned objects and a logical payload for client imports.
    pub fn record_resource(
        &mut self,
        residency: ResourceResidency,
        bytes: Option<u64>,
        is_external_mapping: bool,
        backing_identity_ambiguous: bool,
    ) {
        self.record_resources(
            residency,
            1,
            bytes,
            is_external_mapping,
            backing_identity_ambiguous,
        );
    }

    pub fn record_resources(
        &mut self,
        residency: ResourceResidency,
        count: u64,
        bytes: Option<u64>,
        is_external_mapping: bool,
        backing_identity_ambiguous: bool,
    ) {
        if count == 0 {
            return;
        }
        self.resource_count = self.resource_count.saturating_add(count);

        if let Some(bytes) = bytes {
            match self.ownership {
                ResourceOwnership::CompositorOwned => {
                    self.compositor_owned_estimated_bytes =
                        self.compositor_owned_estimated_bytes.saturating_add(bytes);
                }
                ResourceOwnership::ClientImportedExternallyBacked => {
                    self.externally_backed_logical_bytes =
                        self.externally_backed_logical_bytes.saturating_add(bytes);
                }
                ResourceOwnership::DriverOrContextUnknown => {
                    self.unknown_bytes = self.unknown_bytes.saturating_add(bytes);
                }
            }
        }

        match residency {
            ResourceResidency::ScanoutKmsPinned => {
                self.pinned_count = self.pinned_count.saturating_add(count);
                self.active_count = self.active_count.saturating_add(count);
                add_optional_bytes(&mut self.pinned_bytes, bytes);
                add_optional_bytes(&mut self.active_bytes, bytes);
            }
            ResourceResidency::SynchronizationPinned => {
                self.pinned_count = self.pinned_count.saturating_add(count);
                add_optional_bytes(&mut self.pinned_bytes, bytes);
            }
            ResourceResidency::Active => {
                self.active_count = self.active_count.saturating_add(count);
                add_optional_bytes(&mut self.active_bytes, bytes);
            }
            ResourceResidency::WarmReusable | ResourceResidency::ColdReusable => {
                self.reusable_count = self.reusable_count.saturating_add(count);
                add_optional_bytes(&mut self.reusable_bytes, bytes);
            }
            ResourceResidency::FixedPoolReusable => {
                self.fixed_pool_reusable_count =
                    self.fixed_pool_reusable_count.saturating_add(count);
                add_optional_bytes(&mut self.fixed_pool_reusable_bytes, bytes);
            }
            ResourceResidency::ExternalMapping => {}
            ResourceResidency::Unknown => {}
        }
        if is_external_mapping || residency == ResourceResidency::ExternalMapping {
            self.external_mapping_count = self.external_mapping_count.saturating_add(count);
            add_optional_bytes(&mut self.external_mapping_bytes, bytes);
        }
        if bytes.is_none()
            || residency == ResourceResidency::Unknown
            || self.ownership == ResourceOwnership::DriverOrContextUnknown
        {
            self.unknown_count = self.unknown_count.saturating_add(count);
        }
        if backing_identity_ambiguous {
            self.backing_identity_ambiguous_count =
                self.backing_identity_ambiguous_count.saturating_add(count);
        }
    }
}

fn add_optional_bytes(total: &mut u64, bytes: Option<u64>) {
    if let Some(bytes) = bytes {
        *total = total.saturating_add(bytes);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub enum OutputSlotProtection {
    ScanoutPinned,
    SynchronizationPinned,
    FixedPoolReusable,
    Unknown,
}

/// Describe the fixed explicit output pool without exposing GBM/DRM handles.
pub fn output_pool_domain_snapshot(
    generation: u64,
    width: u32,
    height: u32,
    slots: [OutputSlotProtection; 3],
) -> GpuResidencyDomainSnapshot {
    let bytes_per_slot = u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(4);
    let mut summary = GpuResidencyDomainSnapshot::new(
        GpuResidencyOwner::NativeOutput,
        ResourceDomain::OutputScanout,
        ResourceOwnership::CompositorOwned,
        ResourceCoverage::Instrumented,
    );
    summary.owner_generation = generation;
    for state in slots {
        let residency = match state {
            OutputSlotProtection::ScanoutPinned => ResourceResidency::ScanoutKmsPinned,
            OutputSlotProtection::SynchronizationPinned => ResourceResidency::SynchronizationPinned,
            OutputSlotProtection::FixedPoolReusable => ResourceResidency::FixedPoolReusable,
            OutputSlotProtection::Unknown => ResourceResidency::Unknown,
        };
        summary.record_resource(residency, Some(bytes_per_slot), false, false);
    }
    summary
}

/// Compact renderer owner snapshot carried in the existing rendered-frame
/// statistics handoff. No handles or mutable owner state cross this boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub struct GpuResidencyRendererSummary {
    pub effects_scratch: GpuResidencyDomainSnapshot,
    pub effects_checkpoint: GpuResidencyDomainSnapshot,
    pub lifecycle_retained_visual: GpuResidencyDomainSnapshot,
    pub capture: GpuResidencyDomainSnapshot,
    pub client_import: GpuResidencyDomainSnapshot,
    pub renderer_import_cache: RendererImportCacheDiagnostics,
    pub client_import_observation_available: bool,
}

/// Durable renderer DMA-BUF cache counters copied from the cache owner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RendererImportCacheDiagnostics {
    pub active_mapping_count: u64,
    pub active_known_logical_bytes: u64,
    pub active_unknown_size_count: u64,
    pub cached_mapping_count: u64,
    pub cached_known_logical_bytes: u64,
    pub cached_unknown_size_count: u64,
    pub cache_peak_entries: u64,
    pub cache_peak_known_logical_bytes: u64,
    pub cache_hits_total: u64,
    pub cache_misses_total: u64,
    pub cache_insertions_total: u64,
    pub cache_replacements_total: u64,
    pub cache_evictions_dead_total: u64,
    pub cache_evictions_surface_bound_total: u64,
    pub cache_evictions_surface_destroyed_total: u64,
    pub imports_total: u64,
    pub current_resource_reuses_total: u64,
    pub cache_reuses_total: u64,
    pub import_failures_total: u64,
    pub max_entries_for_one_surface: u64,
    pub per_surface_entry_limit: u64,
}

/// Direct-import cache diagnostics copied from the output owner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectImportCacheDiagnostics {
    pub current_entries: u64,
    pub capacity: u64,
    pub reclaimable_entries: u64,
    pub externally_pinned_entries: u64,
    pub known_logical_mapping_bytes: u64,
    pub unknown_logical_mapping_count: u64,
    pub hits_total: u64,
    pub imports_total: u64,
    pub dead_identity_evictions_total: u64,
    pub capacity_evictions_total: u64,
    pub session_rebind_retirements_total: u64,
    pub cleanup_failures: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UniqueExternalBackingUnavailableReason {
    PhysicalBackingIdentityUnavailable,
}

/// Separate accounting domains that must not be added into physical VRAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuMemoryAttributionSummary {
    pub owned_storage_estimated_bytes: u64,
    pub external_mapping_logical_bytes: u64,
    pub alias_view_live_count: u64,
    pub driver_managed_unknown_live_object_count: u64,
    pub live_unresolved_class_count: u64,
    pub live_unresolved_domain_count: u64,
    pub identity_ambiguous_external_mapping_count: u64,
    pub unique_external_backing_bytes: Option<u64>,
    pub unique_external_backing_bytes_unavailable_reason:
        Option<UniqueExternalBackingUnavailableReason>,
    pub physical_vram_claim: bool,
}

impl Default for GpuMemoryAttributionSummary {
    fn default() -> Self {
        Self {
            owned_storage_estimated_bytes: 0,
            external_mapping_logical_bytes: 0,
            alias_view_live_count: 0,
            driver_managed_unknown_live_object_count: 0,
            live_unresolved_class_count: 0,
            live_unresolved_domain_count: 0,
            identity_ambiguous_external_mapping_count: 0,
            unique_external_backing_bytes: None,
            unique_external_backing_bytes_unavailable_reason: Some(
                UniqueExternalBackingUnavailableReason::PhysicalBackingIdentityUnavailable,
            ),
            physical_vram_claim: false,
        }
    }
}

impl Default for GpuResidencyRendererSummary {
    fn default() -> Self {
        Self {
            effects_scratch: GpuResidencyDomainSnapshot::new(
                GpuResidencyOwner::EglRenderer,
                ResourceDomain::EffectsScratch,
                ResourceOwnership::CompositorOwned,
                ResourceCoverage::Instrumented,
            ),
            effects_checkpoint: GpuResidencyDomainSnapshot::new(
                GpuResidencyOwner::EglRenderer,
                ResourceDomain::EffectsCheckpoint,
                ResourceOwnership::CompositorOwned,
                ResourceCoverage::Instrumented,
            ),
            lifecycle_retained_visual: GpuResidencyDomainSnapshot::new(
                GpuResidencyOwner::EglRenderer,
                ResourceDomain::LifecycleRetainedVisual,
                ResourceOwnership::CompositorOwned,
                ResourceCoverage::Instrumented,
            ),
            capture: GpuResidencyDomainSnapshot::new(
                GpuResidencyOwner::EglRenderer,
                ResourceDomain::Capture,
                ResourceOwnership::CompositorOwned,
                ResourceCoverage::PartiallyInstrumented,
            ),
            client_import: GpuResidencyDomainSnapshot::new(
                GpuResidencyOwner::EglRenderer,
                ResourceDomain::ClientImport,
                ResourceOwnership::ClientImportedExternallyBacked,
                ResourceCoverage::Instrumented,
            ),
            renderer_import_cache: RendererImportCacheDiagnostics::default(),
            client_import_observation_available: false,
        }
    }
}

/// Bounded on-demand diagnostics object. Imported logical payload bytes are
/// reported separately and are never labeled or combined as Typhon VRAM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuResidencySnapshot {
    pub byte_semantics: String,
    pub domains: [GpuResidencyDomainSnapshot; RESOURCE_DOMAIN_COUNT],
    #[serde(default)]
    pub resource_telemetry: GpuResourceTelemetrySnapshot,
    #[serde(default)]
    pub attribution_summary: GpuMemoryAttributionSummary,
    #[serde(default)]
    pub renderer_import_cache: RendererImportCacheDiagnostics,
    #[serde(default)]
    pub renderer_import_cache_available: bool,
    #[serde(default)]
    pub direct_import_cache: DirectImportCacheDiagnostics,
    #[serde(default)]
    pub live_unresolved_class_count: u64,
    #[serde(default)]
    pub live_unresolved_domain_count: u64,
    pub compositor_owned_estimated_bytes: u64,
    pub externally_backed_imported_logical_bytes: u64,
    pub unique_observed_dmabuf_bytes: u64,
    pub resource_count: u64,
    pub pinned_count: u64,
    pub pinned_bytes: u64,
    pub active_count: u64,
    pub active_bytes: u64,
    pub reusable_count: u64,
    pub reusable_bytes: u64,
    pub fixed_pool_reusable_count: u64,
    pub fixed_pool_reusable_bytes: u64,
    pub external_mapping_count: u64,
    pub external_mapping_bytes: u64,
    pub unknown_count: u64,
    pub unknown_bytes: u64,
    pub unaccounted_domain_count: u64,
}

impl Default for GpuResidencySnapshot {
    fn default() -> Self {
        let mut snapshot = Self {
            byte_semantics: String::from(
                "estimated compositor bytes and logical imported bytes; not VRAM",
            ),
            domains: [GpuResidencyDomainSnapshot::default(); RESOURCE_DOMAIN_COUNT],
            resource_telemetry: GpuResourceTelemetrySnapshot::default(),
            attribution_summary: GpuMemoryAttributionSummary::default(),
            renderer_import_cache: RendererImportCacheDiagnostics::default(),
            renderer_import_cache_available: false,
            direct_import_cache: DirectImportCacheDiagnostics::default(),
            live_unresolved_class_count: 0,
            live_unresolved_domain_count: 0,
            compositor_owned_estimated_bytes: 0,
            externally_backed_imported_logical_bytes: 0,
            unique_observed_dmabuf_bytes: 0,
            resource_count: 0,
            pinned_count: 0,
            pinned_bytes: 0,
            active_count: 0,
            active_bytes: 0,
            reusable_count: 0,
            reusable_bytes: 0,
            fixed_pool_reusable_count: 0,
            fixed_pool_reusable_bytes: 0,
            external_mapping_count: 0,
            external_mapping_bytes: 0,
            unknown_count: 0,
            unknown_bytes: 0,
            unaccounted_domain_count: 0,
        };
        for domain in ResourceDomain::all() {
            snapshot.domains[domain.index()] = GpuResidencyDomainSnapshot {
                domain,
                ..GpuResidencyDomainSnapshot::default()
            };
        }
        snapshot.domains[ResourceDomain::OutputScanout.index()] = GpuResidencyDomainSnapshot::new(
            GpuResidencyOwner::NativeOutput,
            ResourceDomain::OutputScanout,
            ResourceOwnership::CompositorOwned,
            ResourceCoverage::Uninstrumented,
        );
        snapshot.domains[ResourceDomain::EffectsScratch.index()] = GpuResidencyDomainSnapshot::new(
            GpuResidencyOwner::EglRenderer,
            ResourceDomain::EffectsScratch,
            ResourceOwnership::CompositorOwned,
            ResourceCoverage::Uninstrumented,
        );
        snapshot.domains[ResourceDomain::EffectsCheckpoint.index()] =
            GpuResidencyDomainSnapshot::new(
                GpuResidencyOwner::EglRenderer,
                ResourceDomain::EffectsCheckpoint,
                ResourceOwnership::CompositorOwned,
                ResourceCoverage::Uninstrumented,
            );
        snapshot.domains[ResourceDomain::ClientImport.index()] = GpuResidencyDomainSnapshot::new(
            GpuResidencyOwner::EglRenderer,
            ResourceDomain::ClientImport,
            ResourceOwnership::ClientImportedExternallyBacked,
            ResourceCoverage::PartiallyInstrumented,
        );
        snapshot.domains[ResourceDomain::LifecycleRetainedVisual.index()] =
            GpuResidencyDomainSnapshot::new(
                GpuResidencyOwner::EglRenderer,
                ResourceDomain::LifecycleRetainedVisual,
                ResourceOwnership::CompositorOwned,
                ResourceCoverage::Uninstrumented,
            );
        snapshot.domains[ResourceDomain::Capture.index()] = GpuResidencyDomainSnapshot::new(
            GpuResidencyOwner::EglRenderer,
            ResourceDomain::Capture,
            ResourceOwnership::CompositorOwned,
            ResourceCoverage::PartiallyInstrumented,
        );
        snapshot.domains[ResourceDomain::CursorDecoration.index()] =
            GpuResidencyDomainSnapshot::new(
                GpuResidencyOwner::EglRenderer,
                ResourceDomain::CursorDecoration,
                ResourceOwnership::CompositorOwned,
                ResourceCoverage::Uninstrumented,
            );
        snapshot.domains[ResourceDomain::OtherUnknown.index()] = GpuResidencyDomainSnapshot::new(
            GpuResidencyOwner::Unknown,
            ResourceDomain::OtherUnknown,
            ResourceOwnership::DriverOrContextUnknown,
            ResourceCoverage::Uninstrumented,
        );
        snapshot.recompute_totals();
        snapshot
    }
}

impl GpuResidencySnapshot {
    pub fn from_renderer(summary: GpuResidencyRendererSummary) -> Self {
        let mut snapshot = Self::default();
        snapshot.renderer_import_cache = summary.renderer_import_cache;
        snapshot.renderer_import_cache_available = summary.client_import_observation_available;
        snapshot.set_domain(summary.effects_scratch);
        snapshot.set_domain(summary.effects_checkpoint);
        snapshot.set_domain(summary.lifecycle_retained_visual);
        snapshot.set_domain(summary.capture);
        snapshot.set_domain(summary.client_import);
        if !summary.client_import_observation_available {
            snapshot.domains[ResourceDomain::ClientImport.index()].coverage =
                ResourceCoverage::Uninstrumented;
        }
        snapshot.recompute_totals();
        snapshot
    }

    pub fn set_domain(&mut self, summary: GpuResidencyDomainSnapshot) {
        self.domains[summary.domain.index()] = summary;
    }

    pub fn recompute_totals(&mut self) {
        self.compositor_owned_estimated_bytes = 0;
        self.externally_backed_imported_logical_bytes = 0;
        self.unique_observed_dmabuf_bytes = 0;
        self.resource_count = 0;
        self.pinned_count = 0;
        self.pinned_bytes = 0;
        self.active_count = 0;
        self.active_bytes = 0;
        self.reusable_count = 0;
        self.reusable_bytes = 0;
        self.fixed_pool_reusable_count = 0;
        self.fixed_pool_reusable_bytes = 0;
        self.external_mapping_count = 0;
        self.external_mapping_bytes = 0;
        self.unknown_count = 0;
        self.unknown_bytes = 0;
        self.unaccounted_domain_count = 0;
        for row in self.domains {
            self.compositor_owned_estimated_bytes = self
                .compositor_owned_estimated_bytes
                .saturating_add(row.compositor_owned_estimated_bytes);
            self.externally_backed_imported_logical_bytes = self
                .externally_backed_imported_logical_bytes
                .saturating_add(row.externally_backed_logical_bytes);
            self.unique_observed_dmabuf_bytes = self
                .unique_observed_dmabuf_bytes
                .saturating_add(row.unique_observed_dmabuf_bytes);
            self.resource_count = self.resource_count.saturating_add(row.resource_count);
            self.pinned_count = self.pinned_count.saturating_add(row.pinned_count);
            self.pinned_bytes = self.pinned_bytes.saturating_add(row.pinned_bytes);
            self.active_count = self.active_count.saturating_add(row.active_count);
            self.active_bytes = self.active_bytes.saturating_add(row.active_bytes);
            self.reusable_count = self.reusable_count.saturating_add(row.reusable_count);
            self.reusable_bytes = self.reusable_bytes.saturating_add(row.reusable_bytes);
            self.fixed_pool_reusable_count = self
                .fixed_pool_reusable_count
                .saturating_add(row.fixed_pool_reusable_count);
            self.fixed_pool_reusable_bytes = self
                .fixed_pool_reusable_bytes
                .saturating_add(row.fixed_pool_reusable_bytes);
            self.external_mapping_count = self
                .external_mapping_count
                .saturating_add(row.external_mapping_count);
            self.external_mapping_bytes = self
                .external_mapping_bytes
                .saturating_add(row.external_mapping_bytes);
            self.unknown_count = self.unknown_count.saturating_add(row.unknown_count);
            self.unknown_bytes = self.unknown_bytes.saturating_add(row.unknown_bytes);
            if row.coverage != ResourceCoverage::Instrumented {
                self.unaccounted_domain_count = self.unaccounted_domain_count.saturating_add(1);
            }
        }

        let mut resource_telemetry = gpu_resource_telemetry_snapshot();
        for row in self.domains {
            let class = match row.domain {
                ResourceDomain::OutputScanout => Some(GpuResourceClass::OutputBacking),
                ResourceDomain::EffectsScratch
                | ResourceDomain::EffectsCheckpoint
                | ResourceDomain::LifecycleRetainedVisual
                | ResourceDomain::Capture => None,
                ResourceDomain::ClientImport => {
                    Some(GpuResourceClass::RendererDmabufExternalBacking)
                }
                ResourceDomain::CursorDecoration => Some(GpuResourceClass::CursorBacking),
                ResourceDomain::OtherUnknown => Some(GpuResourceClass::OtherUnknown),
            };
            let Some(class) = class else {
                continue;
            };
            if row.coverage == ResourceCoverage::Uninstrumented && row.resource_count == 0 {
                continue;
            }
            let known_live_bytes = match row.ownership {
                ResourceOwnership::CompositorOwned => row.compositor_owned_estimated_bytes,
                ResourceOwnership::ClientImportedExternallyBacked => row.external_mapping_bytes,
                ResourceOwnership::DriverOrContextUnknown => row.unknown_bytes,
            };
            let live_count = match row.ownership {
                ResourceOwnership::ClientImportedExternallyBacked => row.external_mapping_count,
                _ => row.resource_count,
            };
            resource_telemetry.observe_owner_domain(
                class,
                live_count,
                known_live_bytes,
                row.coverage,
            );
        }
        for class in [
            GpuResourceClass::ShmUploadTextureBacking,
            GpuResourceClass::UiTextureBacking,
            GpuResourceClass::CursorBacking,
            GpuResourceClass::CaptureBacking,
            GpuResourceClass::UploadBufferBacking,
            GpuResourceClass::FallbackScanoutBacking,
        ] {
            let class_snapshot = resource_telemetry.class(class);
            self.resource_count = self
                .resource_count
                .saturating_add(class_snapshot.live_count);
            self.compositor_owned_estimated_bytes = self
                .compositor_owned_estimated_bytes
                .saturating_add(class_snapshot.known_live_bytes);
        }
        self.live_unresolved_domain_count = count_live_unresolved_domains(&self.domains);
        self.attribution_summary =
            build_attribution_summary(&resource_telemetry, self.live_unresolved_domain_count);
        self.live_unresolved_class_count = self.attribution_summary.live_unresolved_class_count;
        self.resource_telemetry = resource_telemetry;
    }
}

fn build_attribution_summary(
    resource_telemetry: &GpuResourceTelemetrySnapshot,
    live_unresolved_domain_count: u64,
) -> GpuMemoryAttributionSummary {
    let mut summary = GpuMemoryAttributionSummary::default();
    for class in &resource_telemetry.classes {
        match class.byte_semantics {
            GpuResourceByteSemantics::OwnedStorageEstimate => {
                summary.owned_storage_estimated_bytes = summary
                    .owned_storage_estimated_bytes
                    .saturating_add(class.effective_known_live_bytes);
            }
            GpuResourceByteSemantics::ExternalBackingLogical => {
                summary.external_mapping_logical_bytes = summary
                    .external_mapping_logical_bytes
                    .saturating_add(class.effective_known_live_bytes);
                summary.identity_ambiguous_external_mapping_count = summary
                    .identity_ambiguous_external_mapping_count
                    .saturating_add(class.effective_live_count);
            }
            GpuResourceByteSemantics::AliasViewNoStorage => {
                summary.alias_view_live_count = summary
                    .alias_view_live_count
                    .saturating_add(class.effective_live_count);
            }
            GpuResourceByteSemantics::DriverManagedUnknown => {
                summary.driver_managed_unknown_live_object_count = summary
                    .driver_managed_unknown_live_object_count
                    .saturating_add(class.effective_live_count);
            }
        }
        if class.effective_live_count > 0
            && class.live_state_coverage != ResourceCoverage::Instrumented
        {
            summary.live_unresolved_class_count =
                summary.live_unresolved_class_count.saturating_add(1);
        }
    }
    summary.live_unresolved_domain_count = live_unresolved_domain_count;
    summary
}

fn count_live_unresolved_domains(
    domains: &[GpuResidencyDomainSnapshot; RESOURCE_DOMAIN_COUNT],
) -> u64 {
    domains
        .iter()
        .filter(|domain| {
            domain.resource_count > 0 && domain.coverage != ResourceCoverage::Instrumented
        })
        .count() as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub struct GpuResourceIdentity {
    generation: u64,
    local_id: u64,
}

impl GpuResourceIdentity {
    pub const fn new(generation: u64, local_id: u64) -> Self {
        Self {
            generation,
            local_id,
        }
    }

    pub const fn generation(self) -> u64 {
        self.generation
    }

    pub const fn local_id(self) -> u64 {
        self.local_id
    }
}

/// Counts owner-confirmed lifecycle notifications only. It stores no resource
/// inventory and never grants or withdraws a resource's lifetime obligation.
#[derive(Debug, Clone, Copy)]
#[doc(hidden)]
pub struct GenerationEventCounters {
    generation: u64,
    enabled: bool,
    allocations: u64,
    retirements: u64,
    stale_events: u64,
}

impl GenerationEventCounters {
    pub const fn new(generation: u64) -> Self {
        Self {
            generation,
            enabled: true,
            allocations: 0,
            retirements: 0,
            stale_events: 0,
        }
    }

    #[doc(hidden)]
    pub const fn disabled(generation: u64) -> Self {
        Self {
            generation,
            enabled: false,
            allocations: 0,
            retirements: 0,
            stale_events: 0,
        }
    }

    pub fn record_allocation(&mut self, identity: GpuResourceIdentity) -> bool {
        if !self.enabled {
            return false;
        }
        if !self.matches(identity) {
            self.stale_events = self.stale_events.saturating_add(1);
            return false;
        }
        self.allocations = self.allocations.saturating_add(1);
        true
    }

    pub fn record_retirement(&mut self, identity: GpuResourceIdentity) -> bool {
        if !self.enabled {
            return false;
        }
        if !self.matches(identity) {
            self.stale_events = self.stale_events.saturating_add(1);
            return false;
        }
        self.retirements = self.retirements.saturating_add(1);
        true
    }

    const fn matches(&self, identity: GpuResourceIdentity) -> bool {
        identity.generation == self.generation
    }

    pub const fn allocation_count(self) -> u64 {
        self.allocations
    }

    pub const fn retirement_count(self) -> u64 {
        self.retirements
    }

    pub const fn generation(self) -> u64 {
        self.generation
    }

    #[doc(hidden)]
    pub const fn stale_event_count(self) -> u64 {
        self.stale_events
    }

    pub const fn live_count(self) -> u64 {
        self.allocations.saturating_sub(self.retirements)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn telemetry_row_mut(
        telemetry: &mut GpuResourceTelemetrySnapshot,
        class: GpuResourceClass,
    ) -> &mut GpuResourceClassSnapshot {
        telemetry
            .classes
            .iter_mut()
            .find(|row| row.class == class)
            .expect("resource class exists")
    }

    #[test]
    fn explicit_output_slots_follow_owner_protection_state() {
        let summary = output_pool_domain_snapshot(
            12,
            1920,
            1080,
            [
                OutputSlotProtection::ScanoutPinned,
                OutputSlotProtection::SynchronizationPinned,
                OutputSlotProtection::FixedPoolReusable,
            ],
        );

        assert_eq!(summary.owner_generation, 12);
        assert_eq!(summary.resource_count, 3);
        assert_eq!(
            summary.compositor_owned_estimated_bytes,
            1920 * 1080 * 4 * 3
        );
        assert_eq!(summary.pinned_count, 2);
        assert_eq!(summary.active_count, 1);
        assert_eq!(summary.reusable_count, 0);
        assert_eq!(summary.fixed_pool_reusable_count, 1);
    }

    #[test]
    fn imported_mappings_are_external_logical_bytes_not_compositor_bytes() {
        let mut summary = GpuResidencyDomainSnapshot::new(
            GpuResidencyOwner::EglRenderer,
            ResourceDomain::ClientImport,
            ResourceOwnership::ClientImportedExternallyBacked,
            ResourceCoverage::Instrumented,
        );
        summary.record_resource(ResourceResidency::Active, Some(4096), true, true);

        assert_eq!(summary.compositor_owned_estimated_bytes, 0);
        assert_eq!(summary.externally_backed_logical_bytes, 4096);
        assert_eq!(summary.unique_observed_dmabuf_bytes, 0);
        assert_eq!(summary.external_mapping_count, 1);
        assert_eq!(summary.active_count, 1);
        assert_eq!(summary.backing_identity_ambiguous_count, 1);
    }

    #[test]
    fn snapshot_serialization_labels_estimates_and_logical_bytes_without_vram_claims() {
        let mut renderer = GpuResidencyRendererSummary::default();
        renderer
            .client_import
            .record_resource(ResourceResidency::Active, Some(4096), true, true);
        let mut snapshot = GpuResidencySnapshot::from_renderer(renderer);
        snapshot.set_domain(output_pool_domain_snapshot(
            12,
            10,
            10,
            [
                OutputSlotProtection::ScanoutPinned,
                OutputSlotProtection::SynchronizationPinned,
                OutputSlotProtection::FixedPoolReusable,
            ],
        ));
        snapshot.recompute_totals();
        let json = serde_json::to_value(snapshot).expect("serialize residency snapshot");

        assert_eq!(
            json["byte_semantics"],
            "estimated compositor bytes and logical imported bytes; not VRAM"
        );
        assert_eq!(json["compositor_owned_estimated_bytes"], 1200);
        assert_eq!(json["fixed_pool_reusable_count"], 1);
        assert_eq!(json["reusable_count"], 0);
        assert_eq!(json["externally_backed_imported_logical_bytes"], 4096);
        assert_eq!(json["unique_observed_dmabuf_bytes"], 0);
        assert_eq!(
            json["domains"][3]["ownership"],
            "client_imported_externally_backed"
        );
        assert_eq!(json["domains"][3]["backing_identity_ambiguous_count"], 1);
    }

    #[test]
    fn old_generation_retirement_cannot_touch_reused_local_id() {
        let mut events = GenerationEventCounters::new(8);
        let new_resource = GpuResourceIdentity::new(8, 3);
        let stale_resource = GpuResourceIdentity::new(7, 3);

        assert!(events.record_allocation(new_resource));
        assert!(!events.record_retirement(stale_resource));
        assert_eq!(events.live_count(), 1);
        assert_eq!(events.stale_event_count(), 1);
        assert!(events.record_retirement(new_resource));
        assert_eq!(events.allocation_count(), 1);
        assert_eq!(events.retirement_count(), 1);
        assert_eq!(events.live_count(), 0);
    }

    #[test]
    fn disabled_event_accounting_is_a_noop() {
        let mut events = GenerationEventCounters::disabled(4);
        let resource = GpuResourceIdentity::new(4, 1);

        assert!(!events.record_allocation(resource));
        assert!(!events.record_retirement(resource));
        assert_eq!(events.allocation_count(), 0);
        assert_eq!(events.retirement_count(), 0);
        assert_eq!(events.live_count(), 0);
    }

    #[test]
    fn owner_observation_preserves_event_state_and_reports_reconciliation() {
        let mut telemetry = GpuResourceTelemetrySnapshot::default();
        let class = GpuResourceClass::RendererDmabufExternalBacking;
        let row = telemetry_row_mut(&mut telemetry, class);
        row.lifecycle_event_coverage = ResourceCoverage::Instrumented;
        row.event_live_count = 5;
        row.event_known_live_bytes = 4096;
        row.effective_live_count = 5;
        row.effective_known_live_bytes = 4096;
        row.live_count = 5;
        row.known_live_bytes = 4096;

        telemetry.observe_owner_domain(class, 5, 4096, ResourceCoverage::PartiallyInstrumented);

        let row = telemetry.class(class);
        assert_eq!(row.event_live_count, 5);
        assert_eq!(row.event_known_live_bytes, 4096);
        assert_eq!(row.owner_observed_live_count, Some(5));
        assert_eq!(row.owner_observed_known_live_bytes, Some(4096));
        assert_eq!(row.effective_live_count, 5);
        assert_eq!(row.effective_known_live_bytes, 4096);
        assert_eq!(row.live_count, 5);
        assert_eq!(row.known_live_bytes, 4096);
        assert_eq!(
            row.live_state_source,
            GpuResourceLiveStateSource::Reconciled
        );
        assert_eq!(row.live_state_matches, Some(true));
        assert_eq!(row.live_count_delta, Some(0));
        assert_eq!(row.known_live_bytes_delta, Some(0));
    }

    #[test]
    fn event_owner_count_mismatch_remains_visible_without_being_called_a_leak() {
        let mut telemetry = GpuResourceTelemetrySnapshot::default();
        let class = GpuResourceClass::ShaderProgram;
        let row = telemetry_row_mut(&mut telemetry, class);
        row.event_live_count = 4;
        row.event_known_live_bytes = 0;
        row.effective_live_count = 4;
        row.live_count = 4;

        telemetry.observe_owner_domain(class, 3, 0, ResourceCoverage::PartiallyInstrumented);

        let row = telemetry.class(class);
        assert_eq!(row.event_live_count, 4);
        assert_eq!(row.owner_observed_live_count, Some(3));
        assert_eq!(row.effective_live_count, 4);
        assert_eq!(row.live_count_delta, Some(1));
        assert_eq!(row.live_state_matches, Some(false));
        assert_eq!(
            row.live_state_source,
            GpuResourceLiveStateSource::EventLedger
        );
    }

    #[test]
    fn event_owner_byte_mismatch_is_visible() {
        let mut telemetry = GpuResourceTelemetrySnapshot::default();
        let class = GpuResourceClass::ShaderProgram;
        let row = telemetry_row_mut(&mut telemetry, class);
        row.event_live_count = 1;
        row.event_known_live_bytes = 4096;
        row.effective_live_count = 1;
        row.effective_known_live_bytes = 4096;

        telemetry.observe_owner_domain(class, 1, 3072, ResourceCoverage::Instrumented);

        let row = telemetry.class(class);
        assert_eq!(row.known_live_bytes_delta, Some(1024));
        assert_eq!(row.live_state_matches, Some(false));
        assert_eq!(row.effective_known_live_bytes, 4096);
        assert_eq!(
            row.live_state_source,
            GpuResourceLiveStateSource::EventLedger
        );
    }

    #[test]
    fn no_owner_observation_leaves_event_state_authoritative() {
        let mut telemetry = GpuResourceTelemetrySnapshot::default();
        let class = GpuResourceClass::ShaderProgram;
        let row = telemetry_row_mut(&mut telemetry, class);
        row.event_live_count = 2;
        row.effective_live_count = 2;
        row.live_count = 2;
        row.recompute_effective_live_state();

        let row = telemetry.class(class);
        assert_eq!(row.owner_observed_live_count, None);
        assert_eq!(row.effective_live_count, 2);
        assert_eq!(
            row.live_state_source,
            GpuResourceLiveStateSource::EventLedger
        );
    }

    #[test]
    fn default_renderer_handoff_does_not_fabricate_a_zero_owner_observation() {
        let snapshot = GpuResidencySnapshot::from_renderer(GpuResidencyRendererSummary::default());
        let row = snapshot
            .resource_telemetry
            .class(GpuResourceClass::RendererDmabufExternalBacking);
        assert_eq!(row.owner_observed_live_count, None);
        assert!(!snapshot.renderer_import_cache_available);
    }

    #[test]
    fn serialization_keeps_event_and_owner_observations_distinct() {
        let mut telemetry = GpuResourceTelemetrySnapshot::default();
        let class = GpuResourceClass::ShaderProgram;
        let row = telemetry_row_mut(&mut telemetry, class);
        row.event_live_count = 5;
        row.event_known_live_bytes = 0;
        row.effective_live_count = 5;
        row.live_count = 5;
        telemetry.observe_owner_domain(class, 3, 0, ResourceCoverage::PartiallyInstrumented);
        let row_index = telemetry
            .classes
            .iter()
            .position(|row| row.class == class)
            .expect("resource class exists");

        let json = serde_json::to_value(telemetry).expect("telemetry serializes");
        assert_eq!(json["classes"][row_index]["event_live_count"], 5);
        assert_eq!(json["classes"][row_index]["owner_observed_live_count"], 3);
        assert_eq!(json["classes"][row_index]["effective_live_count"], 5);
        assert_eq!(json["classes"][row_index]["live_count"], 5);
        assert_eq!(
            json["classes"][row_index]["live_state_source"],
            "event_ledger"
        );
    }

    #[test]
    fn older_resource_snapshots_deserialize_with_safe_coherence_defaults() {
        let mut json = serde_json::to_value(GpuResourceTelemetrySnapshot::default())
            .expect("default telemetry serializes");
        json.as_object_mut().expect("object").remove("coherent");
        json.as_object_mut()
            .expect("object")
            .remove("event_sequence");
        json.as_object_mut()
            .expect("object")
            .remove("snapshot_retries");
        for class in json["classes"].as_array_mut().expect("classes") {
            let row = class.as_object_mut().expect("class row");
            for key in [
                "live_state_coverage",
                "lifecycle_event_coverage",
                "event_live_count",
                "event_known_live_bytes",
                "owner_observed_live_count",
                "owner_observed_known_live_bytes",
                "owner_observed_coverage",
                "effective_live_count",
                "effective_known_live_bytes",
                "live_state_source",
                "live_count_delta",
                "known_live_bytes_delta",
                "live_state_matches",
            ] {
                row.remove(key);
            }
        }

        let restored = serde_json::from_value::<GpuResourceTelemetrySnapshot>(json)
            .expect("older telemetry snapshot deserializes");
        assert!(!restored.coherent);
        assert_eq!(restored.event_sequence, 0);
        assert_eq!(restored.snapshot_retries, 0);
    }

    #[test]
    fn attribution_summary_never_claims_physical_vram_or_unique_backing_bytes() {
        let mut snapshot = GpuResidencySnapshot::default();
        snapshot.recompute_totals();
        let json = serde_json::to_value(snapshot).expect("residency snapshot serializes");

        assert_eq!(json["attribution_summary"]["physical_vram_claim"], false);
        assert!(json["attribution_summary"]["unique_external_backing_bytes"].is_null());
        assert_eq!(
            json["attribution_summary"]["unique_external_backing_bytes_unavailable_reason"],
            "physical_backing_identity_unavailable"
        );
    }

    #[test]
    fn older_residency_snapshots_receive_safe_stage_two_point_three_defaults() {
        let mut json = serde_json::to_value(GpuResidencySnapshot::default())
            .expect("default residency snapshot serializes");
        let object = json.as_object_mut().expect("snapshot object");
        for key in [
            "attribution_summary",
            "renderer_import_cache",
            "renderer_import_cache_available",
            "direct_import_cache",
            "live_unresolved_class_count",
            "live_unresolved_domain_count",
        ] {
            object.remove(key);
        }

        let restored = serde_json::from_value::<GpuResidencySnapshot>(json)
            .expect("older residency snapshot deserializes");
        assert!(!restored.attribution_summary.physical_vram_claim);
        assert_eq!(
            restored.attribution_summary.unique_external_backing_bytes,
            None
        );
        assert_eq!(restored.renderer_import_cache.imports_total, 0);
        assert!(!restored.renderer_import_cache_available);
        assert_eq!(restored.direct_import_cache.current_entries, 0);
        assert_eq!(restored.live_unresolved_class_count, 0);
        assert_eq!(restored.live_unresolved_domain_count, 0);
    }

    #[test]
    fn attribution_summary_keeps_owned_external_alias_and_unknown_objects_separate() {
        let mut telemetry = GpuResourceTelemetrySnapshot::default();
        for (class, live_count, bytes, coverage) in [
            (
                GpuResourceClass::OutputBacking,
                1,
                11,
                ResourceCoverage::Instrumented,
            ),
            (
                GpuResourceClass::CursorBacking,
                1,
                3,
                ResourceCoverage::Instrumented,
            ),
            (
                GpuResourceClass::RendererDmabufExternalBacking,
                2,
                20,
                ResourceCoverage::Instrumented,
            ),
            (
                GpuResourceClass::DirectScanoutDmabufExternalBacking,
                1,
                0,
                ResourceCoverage::Instrumented,
            ),
            (
                GpuResourceClass::ImportedEglImageView,
                4,
                0,
                ResourceCoverage::Instrumented,
            ),
            (
                GpuResourceClass::ImportedGlTextureView,
                5,
                0,
                ResourceCoverage::Instrumented,
            ),
            (
                GpuResourceClass::GlTextureObject,
                9,
                0,
                ResourceCoverage::Instrumented,
            ),
            (
                GpuResourceClass::DrmFramebuffer,
                1,
                0,
                ResourceCoverage::PartiallyInstrumented,
            ),
        ] {
            let row = telemetry
                .classes
                .iter_mut()
                .find(|row| row.class == class)
                .expect("resource class exists");
            row.effective_live_count = live_count;
            row.effective_known_live_bytes = bytes;
            row.live_state_coverage = coverage;
        }

        let summary = build_attribution_summary(&telemetry, 0);
        assert_eq!(summary.owned_storage_estimated_bytes, 14);
        assert_eq!(summary.external_mapping_logical_bytes, 20);
        assert_eq!(summary.alias_view_live_count, 9);
        assert_eq!(summary.driver_managed_unknown_live_object_count, 10);
        assert_eq!(summary.live_unresolved_class_count, 1);
        assert_eq!(summary.live_unresolved_domain_count, 0);
        assert_eq!(summary.identity_ambiguous_external_mapping_count, 3);
        assert_eq!(summary.unique_external_backing_bytes, None);
        assert!(!summary.physical_vram_claim);
    }

    #[test]
    fn live_unresolved_domain_count_ignores_empty_unknown_rows_and_finds_live_partial_rows() {
        let mut snapshot = GpuResidencySnapshot::default();
        assert_eq!(count_live_unresolved_domains(&snapshot.domains), 0);

        snapshot.domains[ResourceDomain::ClientImport.index()].resource_count = 2;
        snapshot.domains[ResourceDomain::ClientImport.index()].coverage =
            ResourceCoverage::PartiallyInstrumented;
        assert_eq!(count_live_unresolved_domains(&snapshot.domains), 1);
    }
}
