use std::sync::atomic::{AtomicBool, AtomicU64, Ordering, fence};

use serde::{Deserialize, Serialize};

/// Copy-only effects scratch policy diagnostics. Estimated byte fields are
/// compositor estimates, not a report of physical VRAM residency.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct ScratchResidencyTelemetry {
    pub latest_activity_epoch: u64,
    pub protected_epoch_start: Option<u64>,
    pub protected_epoch_end: Option<u64>,
    pub protected_reusable_count: u64,
    pub protected_reusable_estimated_bytes: u64,
    pub warm_reclaim_events: u64,
    pub warm_reclaimed_texture_count: u64,
    pub warm_reclaimed_estimated_bytes: u64,
    pub cold_reclaim_events: u64,
    pub cold_reclaimed_texture_count: u64,
    pub cold_reclaimed_estimated_bytes: u64,
    pub pressure_eviction_count: u64,
}

/// Lock-free snapshot shared with the control plane. The renderer remains the
/// sole owner of the underlying resources; this type only copies diagnostics.
#[derive(Debug, Default)]
pub struct AtomicEffectScratchResidencySnapshot {
    sequence: AtomicU64,
    initialized: AtomicBool,
    fields: [AtomicU64; FIELD_COUNT],
}

const FIELD_COUNT: usize = 17;
const OWNER_GENERATION: usize = 0;
const ACTIVE_COUNT: usize = 1;
const ACTIVE_BYTES: usize = 2;
const REUSABLE_COUNT: usize = 3;
const REUSABLE_BYTES: usize = 4;
const LATEST_EPOCH: usize = 5;
const PROTECTED_EPOCH_START: usize = 6;
const PROTECTED_EPOCH_END: usize = 7;
const PROTECTED_COUNT: usize = 8;
const PROTECTED_BYTES: usize = 9;
const WARM_EVENTS: usize = 10;
const WARM_TEXTURES: usize = 11;
const WARM_BYTES: usize = 12;
const COLD_EVENTS: usize = 13;
const COLD_TEXTURES: usize = 14;
const COLD_BYTES: usize = 15;
const PRESSURE_EVICTIONS: usize = 16;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EffectScratchResidencyLiveSnapshot {
    pub owner_generation: u64,
    pub active_count: u64,
    pub active_bytes: u64,
    pub reusable_count: u64,
    pub reusable_bytes: u64,
    pub policy: ScratchResidencyTelemetry,
}

impl AtomicEffectScratchResidencySnapshot {
    pub fn publish(
        &self,
        owner_generation: u64,
        active_count: usize,
        active_bytes: u64,
        reusable_count: usize,
        reusable_bytes: u64,
        policy: ScratchResidencyTelemetry,
    ) {
        self.sequence.fetch_add(1, Ordering::AcqRel);
        let values = [
            owner_generation,
            u64::try_from(active_count).unwrap_or(u64::MAX),
            active_bytes,
            u64::try_from(reusable_count).unwrap_or(u64::MAX),
            reusable_bytes,
            policy.latest_activity_epoch,
            policy.protected_epoch_start.unwrap_or_default(),
            policy.protected_epoch_end.unwrap_or_default(),
            policy.protected_reusable_count,
            policy.protected_reusable_estimated_bytes,
            policy.warm_reclaim_events,
            policy.warm_reclaimed_texture_count,
            policy.warm_reclaimed_estimated_bytes,
            policy.cold_reclaim_events,
            policy.cold_reclaimed_texture_count,
            policy.cold_reclaimed_estimated_bytes,
            policy.pressure_eviction_count,
        ];
        for (field, value) in self.fields.iter().zip(values) {
            field.store(value, Ordering::Relaxed);
        }
        self.sequence.fetch_add(1, Ordering::Release);
        self.initialized.store(true, Ordering::Release);
    }

    pub fn snapshot(&self) -> Option<EffectScratchResidencyLiveSnapshot> {
        if !self.initialized.load(Ordering::Acquire) {
            return None;
        }
        loop {
            let before = self.sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let value = |index: usize| self.fields[index].load(Ordering::Relaxed);
            let owner_generation = value(OWNER_GENERATION);
            let active_count = value(ACTIVE_COUNT);
            let active_bytes = value(ACTIVE_BYTES);
            let reusable_count = value(REUSABLE_COUNT);
            let reusable_bytes = value(REUSABLE_BYTES);
            let protected_epoch_start = value(PROTECTED_EPOCH_START);
            let protected_epoch_end = value(PROTECTED_EPOCH_END);
            let policy = ScratchResidencyTelemetry {
                latest_activity_epoch: value(LATEST_EPOCH),
                protected_epoch_start: (protected_epoch_start != 0)
                    .then_some(protected_epoch_start),
                protected_epoch_end: (protected_epoch_end != 0).then_some(protected_epoch_end),
                protected_reusable_count: value(PROTECTED_COUNT),
                protected_reusable_estimated_bytes: value(PROTECTED_BYTES),
                warm_reclaim_events: value(WARM_EVENTS),
                warm_reclaimed_texture_count: value(WARM_TEXTURES),
                warm_reclaimed_estimated_bytes: value(WARM_BYTES),
                cold_reclaim_events: value(COLD_EVENTS),
                cold_reclaimed_texture_count: value(COLD_TEXTURES),
                cold_reclaimed_estimated_bytes: value(COLD_BYTES),
                pressure_eviction_count: value(PRESSURE_EVICTIONS),
            };
            fence(Ordering::Acquire);
            let after = self.sequence.load(Ordering::Acquire);
            if before == after && after & 1 == 0 {
                return Some(EffectScratchResidencyLiveSnapshot {
                    owner_generation,
                    active_count,
                    active_bytes,
                    reusable_count,
                    reusable_bytes,
                    policy,
                });
            }
        }
    }
}

impl EffectScratchResidencyLiveSnapshot {
    pub fn domain_snapshot(self) -> crate::gpu_residency::GpuResidencyDomainSnapshot {
        use crate::gpu_residency::{
            GpuResidencyDomainSnapshot, GpuResidencyOwner, ResourceCoverage, ResourceDomain,
            ResourceOwnership,
        };

        let mut summary = GpuResidencyDomainSnapshot::new(
            GpuResidencyOwner::EglRenderer,
            ResourceDomain::EffectsScratch,
            ResourceOwnership::CompositorOwned,
            ResourceCoverage::Instrumented,
        );
        summary.owner_generation = self.owner_generation;
        summary.resource_count = self.active_count.saturating_add(self.reusable_count);
        summary.compositor_owned_estimated_bytes =
            self.active_bytes.saturating_add(self.reusable_bytes);
        summary.active_count = self.active_count;
        summary.active_bytes = self.active_bytes;
        summary.reusable_count = self.reusable_count;
        summary.reusable_bytes = self.reusable_bytes;
        summary.scratch_residency = Some(self.policy);
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::{AtomicEffectScratchResidencySnapshot, ScratchResidencyTelemetry};
    use std::sync::Arc;

    #[test]
    fn atomic_scratch_snapshot_publishes_consistent_values_without_a_mutex() {
        let snapshot = Arc::new(AtomicEffectScratchResidencySnapshot::default());
        let writer = Arc::clone(&snapshot);
        let thread = std::thread::spawn(move || {
            for active in 0..20_000_u64 {
                writer.publish(
                    7,
                    active as usize,
                    active * 4,
                    (20_000 - active) as usize,
                    (20_000 - active) * 4,
                    ScratchResidencyTelemetry {
                        latest_activity_epoch: active,
                        ..Default::default()
                    },
                );
            }
        });

        while !thread.is_finished() {
            if let Some(value) = snapshot.snapshot() {
                assert_eq!(value.active_bytes, value.active_count * 4);
                assert_eq!(value.reusable_bytes, value.reusable_count * 4);
                assert_eq!(value.active_count + value.reusable_count, 20_000);
                assert_eq!(value.owner_generation, 7);
            }
        }
        thread.join().expect("snapshot writer exits");
    }

    #[test]
    fn scratch_policy_telemetry_round_trips_as_copy_only_diagnostics() {
        let snapshot = AtomicEffectScratchResidencySnapshot::default();
        let policy = ScratchResidencyTelemetry {
            latest_activity_epoch: 9,
            protected_epoch_start: Some(8),
            protected_epoch_end: Some(9),
            protected_reusable_count: 4,
            protected_reusable_estimated_bytes: 1024,
            warm_reclaim_events: 3,
            warm_reclaimed_texture_count: 5,
            warm_reclaimed_estimated_bytes: 2048,
            cold_reclaim_events: 1,
            cold_reclaimed_texture_count: 8,
            cold_reclaimed_estimated_bytes: 4096,
            pressure_eviction_count: 2,
        };
        snapshot.publish(11, 0, 0, 4, 1024, policy);
        assert_eq!(
            snapshot.snapshot().expect("snapshot published").policy,
            policy
        );
        let encoded = serde_json::to_value(policy).expect("policy serializes");
        assert_eq!(encoded["protectedEpochStart"], 8);
        assert_eq!(encoded["warmReclaimedEstimatedBytes"], 2048);
        assert!(serde_json::from_value::<ScratchResidencyTelemetry>(encoded).is_ok());
    }

    #[test]
    fn effects_scratch_domain_round_trips_and_accepts_older_snapshots() {
        let publisher = AtomicEffectScratchResidencySnapshot::default();
        publisher.publish(
            11,
            2,
            4096,
            3,
            8192,
            ScratchResidencyTelemetry {
                latest_activity_epoch: 9,
                protected_epoch_start: Some(8),
                protected_epoch_end: Some(9),
                ..Default::default()
            },
        );
        let domain = publisher
            .snapshot()
            .expect("scratch snapshot was published")
            .domain_snapshot();
        assert_eq!(domain.resource_count, 5);
        assert_eq!(domain.compositor_owned_estimated_bytes, 12_288);
        assert_eq!(domain.active_count, 2);
        assert_eq!(domain.reusable_count, 3);
        assert_eq!(domain.scratch_residency.unwrap().latest_activity_epoch, 9);

        let encoded = serde_json::to_value(domain).expect("domain serializes");
        let mut older = encoded.clone();
        older
            .as_object_mut()
            .expect("domain is an object")
            .remove("scratch_residency");
        assert!(
            serde_json::from_value::<crate::gpu_residency::GpuResidencyDomainSnapshot>(older)
                .expect("older snapshot deserializes")
                .scratch_residency
                .is_none()
        );
        assert_eq!(
            serde_json::from_value::<crate::gpu_residency::GpuResidencyDomainSnapshot>(encoded)
                .expect("new snapshot deserializes")
                .scratch_residency
                .unwrap()
                .protected_epoch_start,
            Some(8)
        );
    }
}
