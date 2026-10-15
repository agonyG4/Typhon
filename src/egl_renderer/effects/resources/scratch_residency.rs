use std::time::{Duration, Instant};

pub(super) const SCRATCH_WARM_DELAY: Duration = Duration::from_secs(2);
pub(super) const SCRATCH_COLD_IDLE_DELAY: Duration = Duration::from_secs(10);
pub(super) const SCRATCH_MAINTENANCE_CONTINUATION_DELAY: Duration = Duration::from_millis(5);
pub(super) const MAX_SCRATCH_RETIREMENTS_PER_BATCH: usize = 16;
pub(super) const MAX_SCRATCH_RETIREMENT_BYTES_PER_BATCH: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScratchTrimKind {
    Warm,
    Cold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ScratchEpochUpdate {
    pub epoch: u64,
    pub wrapped: bool,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ScratchResidencyState {
    pub epoch: u64,
    frame_activity_recorded: bool,
    warm_dirty: bool,
    warm_deadline: Option<Instant>,
    cold_deadline: Option<Instant>,
    cold_waiting_for_active: bool,
    pub warm_reclaim_events: u64,
    pub warm_reclaimed_textures: u64,
    pub warm_reclaimed_bytes: u64,
    pub cold_reclaim_events: u64,
    pub cold_reclaimed_textures: u64,
    pub cold_reclaimed_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RetiredScratchTexture {
    pub id: u64,
    pub identity: crate::gpu_residency::GpuResourceIdentity,
}

#[derive(Debug, Default)]
pub(super) struct ScratchTrimBatch {
    pub retired: Vec<RetiredScratchTexture>,
    pub more_eligible: bool,
}

impl ScratchResidencyState {
    pub fn begin_frame(&mut self) {
        self.frame_activity_recorded = false;
    }

    pub fn needs_activity_timestamp(&self) -> bool {
        !self.frame_activity_recorded
    }

    pub fn cold_waiting_for_active(&self) -> bool {
        self.cold_waiting_for_active
    }

    pub fn protected_epoch_window(&self) -> Option<(u64, u64)> {
        (self.epoch != 0).then(|| (self.epoch.saturating_sub(1).max(1), self.epoch))
    }

    pub fn protects_epoch(&self, texture_epoch: u64) -> bool {
        self.protected_epoch_window()
            .is_some_and(|(start, end)| texture_epoch >= start && texture_epoch <= end)
    }

    pub fn record_scratch_activity(&mut self, now: Instant) -> ScratchEpochUpdate {
        if self.frame_activity_recorded {
            return ScratchEpochUpdate {
                epoch: self.epoch,
                wrapped: false,
            };
        }

        let wrapped = self.epoch == u64::MAX;
        self.epoch = if wrapped || self.epoch == 0 {
            1
        } else {
            self.epoch + 1
        };
        self.frame_activity_recorded = true;
        if !self.warm_dirty {
            self.warm_dirty = true;
            self.warm_deadline = Some(now + SCRATCH_WARM_DELAY);
        }
        self.cold_deadline = Some(now + SCRATCH_COLD_IDLE_DELAY);
        self.cold_waiting_for_active = false;
        ScratchEpochUpdate {
            epoch: self.epoch,
            wrapped,
        }
    }

    pub fn reusable_scratch_returned(&mut self, now: Instant) {
        if !self.warm_dirty {
            self.warm_dirty = true;
            self.warm_deadline = Some(now + SCRATCH_WARM_DELAY);
        }
        if self.cold_waiting_for_active {
            self.cold_deadline = Some(now);
            self.cold_waiting_for_active = false;
        }
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        match (self.warm_deadline, self.cold_deadline) {
            (Some(warm), Some(cold)) => Some(warm.min(cold)),
            (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
            (None, None) => None,
        }
    }

    pub fn due_kind(&self, now: Instant) -> Option<ScratchTrimKind> {
        if self.cold_deadline.is_some_and(|deadline| deadline <= now) {
            Some(ScratchTrimKind::Cold)
        } else if self.warm_deadline.is_some_and(|deadline| deadline <= now) {
            Some(ScratchTrimKind::Warm)
        } else {
            None
        }
    }

    pub fn finish_batch(
        &mut self,
        kind: ScratchTrimKind,
        retired_count: usize,
        retired_bytes: u64,
        more_eligible: bool,
        active_scratch_remains: bool,
        now: Instant,
    ) {
        let textures = u64::try_from(retired_count).unwrap_or(u64::MAX);
        match kind {
            ScratchTrimKind::Warm => {
                self.warm_reclaim_events = self.warm_reclaim_events.saturating_add(1);
                self.warm_reclaimed_textures =
                    self.warm_reclaimed_textures.saturating_add(textures);
                self.warm_reclaimed_bytes = self.warm_reclaimed_bytes.saturating_add(retired_bytes);
                if more_eligible {
                    self.warm_deadline = Some(now + SCRATCH_MAINTENANCE_CONTINUATION_DELAY);
                } else {
                    self.warm_dirty = false;
                    self.warm_deadline = None;
                }
            }
            ScratchTrimKind::Cold => {
                self.cold_reclaim_events = self.cold_reclaim_events.saturating_add(1);
                self.cold_reclaimed_textures =
                    self.cold_reclaimed_textures.saturating_add(textures);
                self.cold_reclaimed_bytes = self.cold_reclaimed_bytes.saturating_add(retired_bytes);
                if more_eligible {
                    self.cold_deadline = Some(now + SCRATCH_MAINTENANCE_CONTINUATION_DELAY);
                } else {
                    self.cold_deadline = None;
                    self.warm_dirty = false;
                    self.warm_deadline = None;
                    self.cold_waiting_for_active = active_scratch_remains;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        EffectResourcePool, EffectTextureFilter, EffectTextureFormat, EffectTextureKey,
        EffectTexturePurpose, EffectWorkingSpace,
    };
    use super::{ScratchResidencyState, ScratchTrimKind};
    use std::time::{Duration, Instant};

    const MIB: u64 = 1024 * 1024;

    fn pool() -> EffectResourcePool {
        EffectResourcePool::with_budget(512 * MIB).expect("test pool budget is valid")
    }

    fn key(width: u32, height: u32) -> EffectTextureKey {
        EffectTextureKey::new(
            width,
            height,
            EffectTextureFormat::Rgba8,
            EffectTextureFilter::Nearest,
            EffectWorkingSpace::OutputEncodedSrgb,
        )
    }

    fn acquire_scratch_at(
        pool: &mut EffectResourcePool,
        width: u32,
        height: u32,
        now: Instant,
    ) -> super::super::PooledEffectTexture {
        pool.begin_scratch_activity_frame();
        pool.checkout_with_purpose_at(key(width, height), EffectTexturePurpose::Scratch, Some(now))
            .expect("scratch checkout succeeds")
    }

    fn return_at(
        pool: &mut EffectResourcePool,
        texture: super::super::PooledEffectTexture,
        now: Instant,
    ) {
        pool.return_texture_at(texture, now)
            .expect("texture return succeeds");
    }

    fn allocate_and_return_scratch(
        pool: &mut EffectResourcePool,
        width: u32,
        height: u32,
        now: Instant,
    ) -> super::super::PooledEffectTexture {
        let texture = acquire_scratch_at(pool, width, height, now);
        let returned = texture.clone();
        return_at(pool, texture, now);
        returned
    }

    #[test]
    fn active_scratch_is_never_warm_or_cold_reclaimed() {
        let base = Instant::now();
        let mut pool = pool();
        let active = acquire_scratch_at(&mut pool, 64, 64, base);
        let warm = pool.reclaim_scratch_batch(ScratchTrimKind::Warm, base + Duration::from_secs(2));
        let cold =
            pool.reclaim_scratch_batch(ScratchTrimKind::Cold, base + Duration::from_secs(10));
        assert!(warm.retired.is_empty());
        assert!(cold.retired.is_empty());
        assert!(pool.is_checked_out(active.id));
        assert_eq!(pool.metrics().current_bytes, active.bytes);
    }

    #[test]
    fn checkpoint_lifecycle_and_capture_purposes_are_excluded_from_scratch_trim() {
        let base = Instant::now();
        for purpose in [
            EffectTexturePurpose::Checkpoint,
            EffectTexturePurpose::LifecycleRetained,
            EffectTexturePurpose::Capture,
        ] {
            let mut pool = pool();
            pool.begin_scratch_activity_frame();
            let texture = pool
                .checkout_with_purpose_at(key(32, 32), purpose, Some(base))
                .expect("purpose checkout succeeds");
            let warm =
                pool.reclaim_scratch_batch(ScratchTrimKind::Warm, base + Duration::from_secs(2));
            let cold =
                pool.reclaim_scratch_batch(ScratchTrimKind::Cold, base + Duration::from_secs(10));
            assert!(warm.retired.is_empty());
            assert!(cold.retired.is_empty());
            assert!(pool.is_checked_out(texture.id));
        }
    }

    #[test]
    fn returning_non_scratch_resource_does_not_protect_it_as_recent_scratch() {
        let base = Instant::now();
        let mut pool = pool();
        let recent = allocate_and_return_scratch(&mut pool, 24, 24, base);
        pool.begin_scratch_activity_frame();
        let lifecycle = pool
            .checkout_with_purpose_at(
                key(25, 25),
                EffectTexturePurpose::LifecycleRetained,
                Some(base + Duration::from_millis(10)),
            )
            .expect("lifecycle checkout succeeds");
        return_at(
            &mut pool,
            lifecycle.clone(),
            base + Duration::from_millis(20),
        );

        let trimmed =
            pool.reclaim_scratch_batch(ScratchTrimKind::Warm, base + Duration::from_secs(2));
        assert_eq!(trimmed.retired.len(), 1);
        assert_eq!(trimmed.retired[0].id, lifecycle.id);
        assert!(
            !trimmed
                .retired
                .iter()
                .any(|retired| retired.id == recent.id)
        );
    }

    #[test]
    fn stale_scratch_returned_after_warm_cleanup_arms_another_warm_pass() {
        let base = Instant::now();
        let mut pool = pool();
        let held_stale = acquire_scratch_at(&mut pool, 48, 48, base);
        let _ = allocate_and_return_scratch(&mut pool, 64, 64, base + Duration::from_secs(1));
        let _ = allocate_and_return_scratch(&mut pool, 80, 80, base + Duration::from_millis(1_500));

        let first_warm =
            pool.reclaim_scratch_batch(ScratchTrimKind::Warm, base + Duration::from_secs(2));
        assert!(first_warm.retired.is_empty());
        assert_eq!(
            pool.next_scratch_maintenance_deadline(),
            Some(base + Duration::from_millis(11_500))
        );

        return_at(&mut pool, held_stale.clone(), base + Duration::from_secs(3));
        assert_eq!(
            pool.next_scratch_maintenance_deadline(),
            Some(base + Duration::from_secs(5))
        );

        let second_warm =
            pool.reclaim_scratch_batch(ScratchTrimKind::Warm, base + Duration::from_secs(5));
        assert!(
            second_warm
                .retired
                .iter()
                .any(|retired| retired.id == held_stale.id)
        );
    }

    #[test]
    fn latest_and_previous_scratch_epochs_survive_warm_trim() {
        let base = Instant::now();
        let mut pool = pool();
        let first = allocate_and_return_scratch(&mut pool, 20, 20, base);
        let second =
            allocate_and_return_scratch(&mut pool, 21, 21, base + Duration::from_millis(1));
        let latest =
            allocate_and_return_scratch(&mut pool, 22, 22, base + Duration::from_millis(2));

        let trimmed =
            pool.reclaim_scratch_batch(ScratchTrimKind::Warm, base + Duration::from_secs(2));
        let retired_ids = trimmed
            .retired
            .iter()
            .map(|retired| retired.id)
            .collect::<Vec<_>>();
        assert!(retired_ids.contains(&first.id));
        assert!(!retired_ids.contains(&second.id));
        assert!(!retired_ids.contains(&latest.id));
    }

    #[test]
    fn cold_trim_removes_every_reusable_scratch_texture() {
        let base = Instant::now();
        let mut pool = pool();
        for width in 8..14 {
            allocate_and_return_scratch(
                &mut pool,
                width,
                width,
                base + Duration::from_millis(width as u64),
            );
        }
        let trimmed =
            pool.reclaim_scratch_batch(ScratchTrimKind::Cold, base + Duration::from_secs(11));
        assert_eq!(trimmed.retired.len(), 6);
        assert!(!trimmed.more_eligible);
        let metrics = pool.metrics();
        assert_eq!(metrics.reusable_scratch_count, 0);
        assert_eq!(metrics.reusable_scratch_bytes, 0);
        assert_eq!(metrics.current_bytes, 0);
    }

    #[test]
    fn bulk_trim_keeps_byte_accounting_and_retirement_events_exact() {
        let base = Instant::now();
        let mut pool = pool();
        for width in 4..10 {
            allocate_and_return_scratch(
                &mut pool,
                width,
                width,
                base + Duration::from_millis(width as u64),
            );
        }
        let before_bytes = pool.metrics().current_bytes;
        let before_retirements = pool.event_counters.retirement_count();
        let expected_bytes = pool
            .textures
            .values()
            .flatten()
            .filter(|texture| {
                !texture.checked_out && texture.purpose == EffectTexturePurpose::Scratch
            })
            .map(|texture| texture.bytes)
            .sum::<u64>();

        let trimmed =
            pool.reclaim_scratch_batch(ScratchTrimKind::Cold, base + Duration::from_secs(11));
        assert_eq!(before_bytes - pool.metrics().current_bytes, expected_bytes);
        assert_eq!(
            pool.event_counters.retirement_count() - before_retirements,
            u64::try_from(trimmed.retired.len()).expect("retired count fits u64")
        );
        let retired_identities = trimmed
            .retired
            .iter()
            .map(|retired| (retired.identity.generation(), retired.identity.local_id()))
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(retired_identities.len(), trimmed.retired.len());
        assert_eq!(pool.retirement_count, trimmed.retired.len());
    }

    #[test]
    fn stale_generation_lease_cannot_retire_a_current_pool_texture() {
        let base = Instant::now();
        let mut old_pool = pool();
        let stale = acquire_scratch_at(&mut old_pool, 30, 30, base);
        let mut current_pool = pool();
        let current = acquire_scratch_at(&mut current_pool, 31, 31, base);
        assert!(current_pool.return_texture(stale).is_err());
        assert!(current_pool.is_checked_out(current.id));
        assert_eq!(current_pool.metrics().current_bytes, current.bytes);
    }

    #[test]
    fn empty_key_buckets_are_removed_after_bulk_trim() {
        let base = Instant::now();
        let mut pool = pool();
        allocate_and_return_scratch(&mut pool, 12, 12, base);
        assert_eq!(pool.cached_key_count(), 1);
        let trimmed =
            pool.reclaim_scratch_batch(ScratchTrimKind::Cold, base + Duration::from_secs(10));
        assert_eq!(trimmed.retired.len(), 1);
        assert_eq!(pool.cached_key_count(), 0);
    }

    #[test]
    fn count_and_byte_limits_are_enforced_with_oversized_progress() {
        let base = Instant::now();
        let mut count_pool = pool();
        for width in 1..=20 {
            allocate_and_return_scratch(
                &mut count_pool,
                width,
                1,
                base + Duration::from_millis(width as u64),
            );
        }
        let count_batch = count_pool
            .reclaim_scratch_batch(ScratchTrimKind::Warm, base + Duration::from_millis(2_100));
        assert_eq!(count_batch.retired.len(), 16);
        assert!(count_batch.more_eligible);

        let mut byte_pool = pool();
        for width in 1024..=1038 {
            allocate_and_return_scratch(
                &mut byte_pool,
                width,
                1024,
                base + Duration::from_millis(u64::from(width - 1024)),
            );
        }
        let bytes_before = byte_pool.metrics().current_bytes;
        let byte_batch = byte_pool
            .reclaim_scratch_batch(ScratchTrimKind::Warm, base + Duration::from_millis(2_100));
        assert!(bytes_before - byte_pool.metrics().current_bytes <= 32 * MIB);
        assert!(byte_batch.more_eligible);

        let mut oversized_pool = pool();
        allocate_and_return_scratch(&mut oversized_pool, 4096, 4096, base);
        let bytes_before = oversized_pool.metrics().current_bytes;
        let oversized = oversized_pool
            .reclaim_scratch_batch(ScratchTrimKind::Cold, base + Duration::from_secs(10));
        assert_eq!(oversized.retired.len(), 1);
        assert_eq!(
            bytes_before - oversized_pool.metrics().current_bytes,
            64 * MIB
        );
    }

    #[test]
    fn partial_batches_report_more_work_and_later_batches_finish() {
        let base = Instant::now();
        let mut pool = pool();
        for width in 1..=20 {
            allocate_and_return_scratch(
                &mut pool,
                width,
                1,
                base + Duration::from_millis(width as u64),
            );
        }
        let first =
            pool.reclaim_scratch_batch(ScratchTrimKind::Warm, base + Duration::from_millis(2_100));
        assert!(first.more_eligible);
        let second =
            pool.reclaim_scratch_batch(ScratchTrimKind::Warm, base + Duration::from_millis(2_105));
        assert!(!second.more_eligible);
        assert_eq!(pool.metrics().reusable_scratch_count, 2);
    }

    #[test]
    fn repeated_equivalent_work_reuses_before_warm_trim() {
        let base = Instant::now();
        let mut pool = pool();
        let first = acquire_scratch_at(&mut pool, 100, 100, base);
        return_at(&mut pool, first, base);
        let second = acquire_scratch_at(&mut pool, 100, 100, base + Duration::from_millis(1));
        assert_eq!(second.id, 1);
        assert_eq!(pool.allocation_count, 1);
        assert_eq!(pool.reuse_count, 1);
    }

    #[test]
    fn warm_trim_preserves_the_recent_workload_for_reuse() {
        let base = Instant::now();
        let mut pool = pool();
        allocate_and_return_scratch(&mut pool, 64, 64, base);
        let previous =
            allocate_and_return_scratch(&mut pool, 128, 128, base + Duration::from_millis(1));
        let latest =
            allocate_and_return_scratch(&mut pool, 256, 256, base + Duration::from_millis(2));
        let before_reuse = pool.reuse_count;
        let _ = pool.reclaim_scratch_batch(ScratchTrimKind::Warm, base + Duration::from_secs(2));

        let previous_again = acquire_scratch_at(&mut pool, 128, 128, base + Duration::from_secs(3));
        let latest_again = acquire_scratch_at(&mut pool, 256, 256, base + Duration::from_secs(3));
        assert_eq!(previous_again.id, previous.id);
        assert_eq!(latest_again.id, latest.id);
        assert_eq!(pool.reuse_count, before_reuse + 2);
    }

    #[test]
    fn cold_trim_allows_first_allocation_then_immediate_reuse() {
        let base = Instant::now();
        let mut pool = pool();
        let initial = allocate_and_return_scratch(&mut pool, 96, 96, base);
        let cold =
            pool.reclaim_scratch_batch(ScratchTrimKind::Cold, base + Duration::from_secs(10));
        assert_eq!(cold.retired.len(), 1);
        let allocations = pool.allocation_count;

        let first = acquire_scratch_at(&mut pool, 96, 96, base + Duration::from_secs(11));
        assert_ne!(first.id, initial.id);
        assert_eq!(pool.allocation_count, allocations + 1);
        return_at(&mut pool, first, base + Duration::from_secs(11));
        let second = acquire_scratch_at(
            &mut pool,
            96,
            96,
            base + Duration::from_secs(11) + Duration::from_millis(1),
        );
        assert_eq!(second.id, allocations as u64 + 1);
        assert_eq!(pool.reuse_count, 1);
    }

    #[test]
    fn epoch_wrap_clears_old_marks_before_restarting() {
        let base = Instant::now();
        let mut pool = pool();
        let ancient = allocate_and_return_scratch(&mut pool, 40, 40, base);
        pool.scratch_residency.epoch = u64::MAX;
        if let Some(texture) = pool
            .textures
            .get_mut(&ancient.key)
            .and_then(|textures| textures.iter_mut().find(|texture| texture.id == ancient.id))
        {
            texture.scratch_epoch = u64::MAX;
        }

        let current =
            allocate_and_return_scratch(&mut pool, 41, 41, base + Duration::from_millis(1));
        assert_eq!(pool.scratch_residency.epoch, 1);
        let trimmed =
            pool.reclaim_scratch_batch(ScratchTrimKind::Warm, base + Duration::from_secs(2));
        assert!(
            trimmed
                .retired
                .iter()
                .any(|retired| retired.id == ancient.id)
        );
        assert!(
            !trimmed
                .retired
                .iter()
                .any(|retired| retired.id == current.id)
        );
    }

    #[test]
    fn continuous_activity_does_not_slide_the_armed_warm_deadline() {
        let base = Instant::now();
        let mut state = ScratchResidencyState::default();
        state.begin_frame();
        state.record_scratch_activity(base);
        let first_warm_deadline = state.next_deadline().expect("warm deadline armed");

        state.begin_frame();
        state.record_scratch_activity(base + Duration::from_millis(1_000));
        assert_eq!(state.next_deadline(), Some(first_warm_deadline));
        assert_eq!(first_warm_deadline, base + Duration::from_secs(2));
    }

    #[test]
    fn continuous_actual_scratch_activity_postpones_cold_zero() {
        let base = Instant::now();
        let mut state = ScratchResidencyState::default();
        state.begin_frame();
        state.record_scratch_activity(base);
        state.finish_batch(
            ScratchTrimKind::Warm,
            0,
            0,
            false,
            false,
            base + Duration::from_secs(2),
        );
        state.begin_frame();
        state.record_scratch_activity(base + Duration::from_secs(9));

        assert_eq!(state.due_kind(base + Duration::from_secs(10)), None);
        assert_eq!(
            state.due_kind(base + Duration::from_secs(18)),
            Some(ScratchTrimKind::Warm)
        );
        assert_eq!(
            state.due_kind(base + Duration::from_secs(19)),
            Some(ScratchTrimKind::Cold)
        );
    }

    #[test]
    fn completed_trim_is_a_noop_until_new_scratch_activity() {
        let base = Instant::now();
        let mut pool = pool();
        allocate_and_return_scratch(&mut pool, 32, 32, base);
        let first =
            pool.reclaim_scratch_batch(ScratchTrimKind::Cold, base + Duration::from_secs(10));
        assert_eq!(first.retired.len(), 1);
        let retirements = pool.event_counters.retirement_count();
        let second =
            pool.reclaim_scratch_batch(ScratchTrimKind::Cold, base + Duration::from_secs(30));
        assert!(second.retired.is_empty());
        assert_eq!(pool.event_counters.retirement_count(), retirements);
        assert_eq!(pool.metrics().current_bytes, 0);
    }
}
