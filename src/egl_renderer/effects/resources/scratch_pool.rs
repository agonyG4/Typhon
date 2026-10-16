use std::time::Instant;

use super::super::metrics::EffectResourceMetrics;
use super::scratch_residency;
use super::{EffectResourceError, EffectResourcePool, EffectTexturePurpose, PooledEffectTexture};

impl EffectResourcePool {
    pub(super) fn checkout_with_purpose(
        &mut self,
        key: super::EffectTextureKey,
        purpose: EffectTexturePurpose,
    ) -> Result<PooledEffectTexture, EffectResourceError> {
        let activity_time = (purpose == EffectTexturePurpose::Scratch
            && self.scratch_residency.needs_activity_timestamp())
        .then(Instant::now);
        self.checkout_with_purpose_at(key, purpose, activity_time)
    }

    pub(super) fn checkout_with_purpose_at(
        &mut self,
        key: super::EffectTextureKey,
        purpose: EffectTexturePurpose,
        activity_time: Option<Instant>,
    ) -> Result<PooledEffectTexture, EffectResourceError> {
        let bytes = key.estimated_bytes()?;
        self.clock = self.clock.saturating_add(1);
        if let Some(texture) = self
            .textures
            .get_mut(&key)
            .and_then(|textures| textures.iter_mut().find(|texture| !texture.checked_out))
        {
            texture.checked_out = true;
            texture.last_used = self.clock;
            texture.purpose = purpose;
            texture.scratch_epoch = 0;
            self.reuse_count = self.reuse_count.saturating_add(1);
            let id = texture.id;
            let mut checked_out = texture.clone();
            if purpose == EffectTexturePurpose::Scratch {
                let epoch = self.record_scratch_checkout(activity_time);
                if let Some(cached) = self
                    .textures
                    .get_mut(&key)
                    .and_then(|textures| textures.iter_mut().find(|texture| texture.id == id))
                {
                    cached.scratch_epoch = epoch;
                    checked_out.scratch_epoch = epoch;
                }
            }
            return Ok(checked_out);
        }

        self.evict_until_fits(bytes);
        let new_current = self
            .current_bytes
            .checked_add(bytes)
            .ok_or(EffectResourceError::SizeOverflow)?;
        if new_current > self.budget_bytes {
            return Err(EffectResourceError::BudgetExceeded {
                requested_bytes: bytes,
                current_bytes: self.current_bytes,
                budget_bytes: self.budget_bytes,
            });
        }
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(EffectResourceError::TextureIdOverflow)?;
        let texture = PooledEffectTexture {
            id,
            key,
            bytes,
            last_used: self.clock,
            checked_out: true,
            purpose,
            scratch_epoch: 0,
            identity: crate::gpu_residency::GpuResourceIdentity::new(
                self.event_counters.generation(),
                id,
            ),
        };
        self.textures.entry(key).or_default().push(texture.clone());
        self.current_bytes = new_current;
        self.peak_bytes = self.peak_bytes.max(new_current);
        self.allocation_count = self.allocation_count.saturating_add(1);
        self.event_counters.record_allocation(texture.identity);
        if purpose == EffectTexturePurpose::Scratch {
            let epoch = self.record_scratch_checkout(activity_time);
            if let Some(cached) = self
                .textures
                .get_mut(&key)
                .and_then(|textures| textures.iter_mut().find(|cached| cached.id == id))
            {
                cached.scratch_epoch = epoch;
            }
            let mut checked_out = texture;
            checked_out.scratch_epoch = epoch;
            Ok(checked_out)
        } else {
            Ok(texture)
        }
    }

    pub fn return_texture(
        &mut self,
        texture: PooledEffectTexture,
    ) -> Result<(), EffectResourceError> {
        self.return_texture_with_time(texture, None)
    }

    #[cfg(test)]
    pub(super) fn return_texture_at(
        &mut self,
        texture: PooledEffectTexture,
        now: Instant,
    ) -> Result<(), EffectResourceError> {
        self.return_texture_with_time(texture, Some(now))
    }

    fn return_texture_with_time(
        &mut self,
        texture: PooledEffectTexture,
        now: Option<Instant>,
    ) -> Result<(), EffectResourceError> {
        if texture.identity.generation() != self.event_counters.generation()
            || texture.identity.local_id() != texture.id
        {
            self.event_counters.record_retirement(texture.identity);
            return Err(EffectResourceError::UnknownTexture(texture.id));
        }
        let Some(cached) = self
            .textures
            .get_mut(&texture.key)
            .and_then(|textures| textures.iter_mut().find(|cached| cached.id == texture.id))
        else {
            return Err(EffectResourceError::UnknownTexture(texture.id));
        };
        if !cached.checked_out {
            return Err(EffectResourceError::TextureAlreadyReturned(texture.id));
        }
        let (was_scratch, returned_epoch) = {
            let was_scratch = cached.purpose == EffectTexturePurpose::Scratch;
            cached.checked_out = false;
            cached.last_used = self.clock;
            cached.purpose = EffectTexturePurpose::Scratch;
            if !was_scratch {
                cached.scratch_epoch = 0;
            }
            (was_scratch, cached.scratch_epoch)
        };
        let cold_waiting_for_active = self.scratch_residency.cold_waiting_for_active();
        let returned_scratch_is_stale =
            !was_scratch || !self.scratch_residency.protects_epoch(returned_epoch);
        if returned_scratch_is_stale || cold_waiting_for_active {
            self.scratch_residency
                .reusable_scratch_returned(now.unwrap_or_else(Instant::now));
        }
        Ok(())
    }

    pub(super) fn set_purpose(
        &mut self,
        texture: &PooledEffectTexture,
        purpose: EffectTexturePurpose,
    ) {
        if texture.identity.generation() != self.event_counters.generation()
            || texture.identity.local_id() != texture.id
        {
            return;
        }
        if let Some(cached) = self
            .textures
            .get_mut(&texture.key)
            .and_then(|textures| textures.iter_mut().find(|cached| cached.id == texture.id))
        {
            cached.purpose = purpose;
            if purpose != EffectTexturePurpose::Scratch {
                cached.scratch_epoch = 0;
            }
        }
    }

    pub(super) fn begin_scratch_activity_frame(&mut self) {
        self.scratch_residency.begin_frame();
    }

    fn record_scratch_checkout(&mut self, activity_time: Option<Instant>) -> u64 {
        if !self.scratch_residency.needs_activity_timestamp() {
            return self.scratch_residency.epoch;
        }
        let update = self
            .scratch_residency
            .record_scratch_activity(activity_time.unwrap_or_else(Instant::now));
        if update.wrapped {
            for texture in self.textures.values_mut().flatten() {
                texture.scratch_epoch = 0;
            }
        }
        update.epoch
    }

    pub(super) fn next_scratch_maintenance_deadline(&self) -> Option<Instant> {
        self.scratch_residency.next_deadline()
    }

    pub(super) fn reclaim_scratch_batch(
        &mut self,
        kind: scratch_residency::ScratchTrimKind,
        now: Instant,
    ) -> scratch_residency::ScratchTrimBatch {
        use scratch_residency::{
            MAX_SCRATCH_RETIREMENT_BYTES_PER_BATCH, MAX_SCRATCH_RETIREMENTS_PER_BATCH,
        };

        if self.scratch_residency.due_kind(now) != Some(kind) {
            return scratch_residency::ScratchTrimBatch::default();
        }

        let protected_start = self.scratch_residency.epoch.saturating_sub(1).max(1);
        let protected_end = self.scratch_residency.epoch;
        let mut retired = Vec::new();
        let mut estimated_bytes = 0_u64;
        let mut more_eligible = false;
        let mut active_scratch_remains = false;

        self.textures.retain(|_, textures| {
            textures.retain(|texture| {
                if texture.purpose != EffectTexturePurpose::Scratch {
                    return true;
                }
                if texture.checked_out {
                    active_scratch_remains = true;
                    return true;
                }

                let eligible = kind == scratch_residency::ScratchTrimKind::Cold
                    || texture.scratch_epoch == 0
                    || texture.scratch_epoch < protected_start
                    || texture.scratch_epoch > protected_end;
                if !eligible {
                    return true;
                }

                let within_count_limit = retired.len() < MAX_SCRATCH_RETIREMENTS_PER_BATCH;
                let within_byte_limit = retired.is_empty()
                    || texture.bytes
                        <= MAX_SCRATCH_RETIREMENT_BYTES_PER_BATCH.saturating_sub(estimated_bytes);
                if within_count_limit && within_byte_limit {
                    estimated_bytes = estimated_bytes.saturating_add(texture.bytes);
                    retired.push(scratch_residency::RetiredScratchTexture {
                        id: texture.id,
                        identity: texture.identity,
                    });
                    false
                } else {
                    more_eligible = true;
                    true
                }
            });
            !textures.is_empty()
        });

        self.current_bytes = self
            .current_bytes
            .checked_sub(estimated_bytes)
            .expect("retired scratch bytes are included in pool accounting");
        self.retirement_count = self.retirement_count.saturating_add(retired.len());
        for texture in &retired {
            self.event_counters.record_retirement(texture.identity);
        }
        self.scratch_residency.finish_batch(
            kind,
            retired.len(),
            estimated_bytes,
            more_eligible,
            active_scratch_remains,
            now,
        );
        scratch_residency::ScratchTrimBatch {
            retired,
            more_eligible,
        }
    }

    pub(crate) fn metrics(&self) -> EffectResourceMetrics {
        let mut cached_texture_count = 0;
        let mut checked_out_texture_count = 0;
        let mut active_scratch_count = 0;
        let mut active_scratch_bytes = 0_u64;
        let mut reusable_scratch_count = 0;
        let mut reusable_scratch_bytes = 0_u64;
        let mut protected_reusable_scratch_count = 0_u64;
        let mut protected_reusable_scratch_bytes = 0_u64;
        let mut checkpoint_count = 0;
        let mut checkpoint_bytes = 0_u64;
        let mut lifecycle_retained_count = 0;
        let mut lifecycle_retained_bytes = 0_u64;
        let mut capture_count = 0;
        let mut capture_bytes = 0_u64;
        for texture in self.textures.values().flatten() {
            cached_texture_count += 1;
            checked_out_texture_count += usize::from(texture.checked_out);
            match texture.purpose {
                EffectTexturePurpose::Scratch if texture.checked_out => {
                    active_scratch_count += 1;
                    active_scratch_bytes = active_scratch_bytes.saturating_add(texture.bytes);
                }
                EffectTexturePurpose::Scratch => {
                    reusable_scratch_count += 1;
                    reusable_scratch_bytes = reusable_scratch_bytes.saturating_add(texture.bytes);
                    if self.scratch_residency.protects_epoch(texture.scratch_epoch) {
                        protected_reusable_scratch_count =
                            protected_reusable_scratch_count.saturating_add(1);
                        protected_reusable_scratch_bytes =
                            protected_reusable_scratch_bytes.saturating_add(texture.bytes);
                    }
                }
                EffectTexturePurpose::Checkpoint => {
                    checkpoint_count += 1;
                    checkpoint_bytes = checkpoint_bytes.saturating_add(texture.bytes);
                }
                EffectTexturePurpose::LifecycleRetained => {
                    lifecycle_retained_count += 1;
                    lifecycle_retained_bytes =
                        lifecycle_retained_bytes.saturating_add(texture.bytes);
                }
                EffectTexturePurpose::Capture => {
                    capture_count += 1;
                    capture_bytes = capture_bytes.saturating_add(texture.bytes);
                }
            }
        }
        let (protected_epoch_start, protected_epoch_end) = self
            .scratch_residency
            .protected_epoch_window()
            .map_or((None, None), |(start, end)| (Some(start), Some(end)));
        EffectResourceMetrics {
            generation: self.event_counters.generation(),
            current_bytes: self.current_bytes,
            peak_bytes: self.peak_bytes,
            budget_bytes: self.budget_bytes,
            cached_key_count: self.cached_key_count(),
            cached_texture_count,
            checked_out_texture_count,
            eviction_count: self.eviction_count,
            allocation_count: self.allocation_count,
            reuse_count: self.reuse_count,
            retirement_count: self.retirement_count,
            active_scratch_count,
            active_scratch_bytes,
            reusable_scratch_count,
            reusable_scratch_bytes,
            scratch_residency: crate::gpu_residency::ScratchResidencyTelemetry {
                latest_activity_epoch: self.scratch_residency.epoch,
                protected_epoch_start,
                protected_epoch_end,
                protected_reusable_count: protected_reusable_scratch_count,
                protected_reusable_estimated_bytes: protected_reusable_scratch_bytes,
                warm_reclaim_events: self.scratch_residency.warm_reclaim_events,
                warm_reclaimed_texture_count: self.scratch_residency.warm_reclaimed_textures,
                warm_reclaimed_estimated_bytes: self.scratch_residency.warm_reclaimed_bytes,
                cold_reclaim_events: self.scratch_residency.cold_reclaim_events,
                cold_reclaimed_texture_count: self.scratch_residency.cold_reclaimed_textures,
                cold_reclaimed_estimated_bytes: self.scratch_residency.cold_reclaimed_bytes,
                pressure_eviction_count: u64::try_from(self.eviction_count).unwrap_or(u64::MAX),
                pressure_evicted_estimated_bytes: self.pressure_evicted_estimated_bytes,
                protected_pressure_eviction_count: self.protected_pressure_eviction_count,
                protected_pressure_evicted_estimated_bytes: self
                    .protected_pressure_evicted_estimated_bytes,
                pool_current_estimated_bytes: self.current_bytes,
                pool_peak_estimated_bytes: self.peak_bytes,
                pool_hard_budget_estimated_bytes: self.budget_bytes,
                cached_key_count: u64::try_from(self.cached_key_count()).unwrap_or(u64::MAX),
                allocation_count: u64::try_from(self.allocation_count).unwrap_or(u64::MAX),
                reuse_count: u64::try_from(self.reuse_count).unwrap_or(u64::MAX),
                retirement_count: u64::try_from(self.retirement_count).unwrap_or(u64::MAX),
            },
            checkpoint_count,
            checkpoint_bytes,
            lifecycle_retained_count,
            lifecycle_retained_bytes,
            capture_count,
            capture_bytes,
        }
    }
}
