use std::{
    collections::HashMap,
    io,
    os::fd::{AsFd, AsRawFd, OwnedFd},
};

use oblivion_one::compositor::{DmabufGpuReleaseLeaseId, OwnCompositorServer};
use oblivion_one::native::{
    event_loop::{NativeEventLoop, NativeEventSource, ReactorToken},
    sync_file::query_sync_file_info,
};

use crate::native_output::NativePerfField;
use crate::native_output::OutputTransactionId;
use crate::native_output::kms_worker::KmsCommitWorkerHandle;
use crate::native_output::pacing::BoundedSamples;
use crate::native_output::scanout::AtomicEglGbmScanout;

const DMABUF_RELEASE_RETRY_BASE_DELAY_NS: u64 = 1_000_000;
const DMABUF_RELEASE_RETRY_MAX_DELAY_NS: u64 = 250_000_000;
pub(crate) const DMABUF_GPU_RELEASE_CORRELATION_CAPACITY: usize = 256;
const DMABUF_GPU_RELEASE_SAMPLE_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DmabufGpuReleaseSafety {
    pub(crate) direct_kms_ownership_live: bool,
}

impl DmabufGpuReleaseSafety {
    pub(crate) const fn from_ownership(
        atomic_submitted_or_presented: bool,
        worker_queued: bool,
        worker_executing: bool,
        worker_inflight: bool,
    ) -> Self {
        Self {
            direct_kms_ownership_live: atomic_submitted_or_presented
                || worker_queued
                || worker_executing
                || worker_inflight,
        }
    }

    pub(crate) const fn permits_compositor_gpu_release(self) -> bool {
        !self.direct_kms_ownership_live
    }
}

pub(crate) fn dmabuf_gpu_release_safety(
    explicit: &AtomicEglGbmScanout,
    worker: Option<&KmsCommitWorkerHandle>,
) -> DmabufGpuReleaseSafety {
    // The KMS worker owns DirectPrimaryLease independently of the explicit
    // scanout state.  A GL completion fence cannot prove that worker-owned
    // framebuffer access has ended, so every worker phase is conservative.
    let (worker_queued, worker_executing, worker_inflight) = worker
        .map(|worker| {
            let (queued, executing, inflight) = worker.direct_content_keys();
            (queued.is_some(), executing.is_some(), inflight.is_some())
        })
        .unwrap_or_default();
    DmabufGpuReleaseSafety::from_ownership(
        explicit.has_live_direct_kms_ownership(),
        worker_queued,
        worker_executing,
        worker_inflight,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DmabufReleaseRetryReason {
    NoGpuProofAvailable,
    CompletionFdDuplicationFailed,
    ReactorRegistrationFailed,
    DirectKmsOwnershipBlocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DmabufGpuReleaseOrigin {
    Composited {
        transaction_id: OutputTransactionId,
    },
    NoVisual,
    DeferredRetry,
    #[cfg(test)]
    Uncorrelated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DmabufGpuReleaseCorrelation {
    obligation_count: usize,
    registered_at_ns: u64,
    gpu_signal_ns: Option<u64>,
    pageflip_ns: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DmabufGpuReleaseQualificationSummary {
    pub(crate) composited_correlations_armed: u64,
    pub(crate) composited_correlations_paired: u64,
    pub(crate) release_before_pageflip_leases: u64,
    pub(crate) release_before_pageflip_obligations: u64,
    pub(crate) release_after_pageflip_leases: u64,
    pub(crate) release_after_pageflip_obligations: u64,
    pub(crate) release_same_timestamp_leases: u64,
    pub(crate) exact_signal_timestamps: u64,
    pub(crate) signal_timestamp_unavailable: u64,
    pub(crate) correlations_unpairable_signal_timestamp: u64,
    pub(crate) already_signaled_before_registration: u64,
    pub(crate) timestamp_order_anomalies: u64,
    pub(crate) correlation_pending: usize,
    pub(crate) correlation_overflows: u64,
    pub(crate) correlation_duplicates: u64,
    pub(crate) gpu_release_registry_wait_p50_us: u64,
    pub(crate) gpu_release_registry_wait_p95_us: u64,
    pub(crate) gpu_release_registry_wait_p99_us: u64,
    pub(crate) release_to_pageflip_lead_p50_us: u64,
    pub(crate) release_to_pageflip_lead_p95_us: u64,
    pub(crate) release_to_pageflip_lead_p99_us: u64,
    pub(crate) pageflip_to_release_lag_p50_us: u64,
    pub(crate) pageflip_to_release_lag_p95_us: u64,
    pub(crate) pageflip_to_release_lag_p99_us: u64,
}

#[derive(Debug, Default)]
pub(crate) struct DmabufGpuReleaseObservability {
    correlations: HashMap<OutputTransactionId, DmabufGpuReleaseCorrelation>,
    summary: DmabufGpuReleaseQualificationSummary,
    fence_wait_ns: BoundedSamples<DMABUF_GPU_RELEASE_SAMPLE_CAPACITY>,
    release_to_pageflip_lead_ns: BoundedSamples<DMABUF_GPU_RELEASE_SAMPLE_CAPACITY>,
    pageflip_to_release_lag_ns: BoundedSamples<DMABUF_GPU_RELEASE_SAMPLE_CAPACITY>,
}

impl DmabufGpuReleaseObservability {
    fn arm_composited(
        &mut self,
        transaction_id: OutputTransactionId,
        obligation_count: usize,
        registered_at_ns: u64,
    ) -> bool {
        if self.correlations.contains_key(&transaction_id) {
            self.summary.correlation_duplicates =
                self.summary.correlation_duplicates.saturating_add(1);
            return false;
        }
        if self.correlations.len() >= DMABUF_GPU_RELEASE_CORRELATION_CAPACITY {
            self.summary.correlation_overflows =
                self.summary.correlation_overflows.saturating_add(1);
            return false;
        }
        self.correlations.insert(
            transaction_id,
            DmabufGpuReleaseCorrelation {
                obligation_count,
                registered_at_ns,
                gpu_signal_ns: None,
                pageflip_ns: None,
            },
        );
        self.summary.composited_correlations_armed =
            self.summary.composited_correlations_armed.saturating_add(1);
        true
    }

    fn note_gpu_signal(&mut self, transaction_id: OutputTransactionId, signal_ns: u64) {
        let Some(correlation) = self.correlations.get_mut(&transaction_id) else {
            return;
        };
        correlation.gpu_signal_ns = Some(signal_ns);
        self.finish_if_paired(transaction_id);
    }

    pub(crate) fn note_composited_pageflip(
        &mut self,
        transaction_id: OutputTransactionId,
        pageflip_ns: u64,
    ) {
        let Some(correlation) = self.correlations.get_mut(&transaction_id) else {
            return;
        };
        correlation.pageflip_ns = Some(pageflip_ns);
        self.finish_if_paired(transaction_id);
    }

    fn finish_if_paired(&mut self, transaction_id: OutputTransactionId) {
        let Some(correlation) = self.correlations.get(&transaction_id).copied() else {
            return;
        };
        let (Some(gpu_signal_ns), Some(pageflip_ns)) =
            (correlation.gpu_signal_ns, correlation.pageflip_ns)
        else {
            return;
        };
        self.correlations.remove(&transaction_id);
        self.summary.composited_correlations_paired = self
            .summary
            .composited_correlations_paired
            .saturating_add(1);
        match gpu_signal_ns.cmp(&pageflip_ns) {
            std::cmp::Ordering::Less => {
                self.summary.release_before_pageflip_leases = self
                    .summary
                    .release_before_pageflip_leases
                    .saturating_add(1);
                self.summary.release_before_pageflip_obligations = self
                    .summary
                    .release_before_pageflip_obligations
                    .saturating_add(correlation.obligation_count as u64);
                self.release_to_pageflip_lead_ns
                    .record(pageflip_ns.saturating_sub(gpu_signal_ns));
            }
            std::cmp::Ordering::Equal => {
                self.summary.release_same_timestamp_leases =
                    self.summary.release_same_timestamp_leases.saturating_add(1);
            }
            std::cmp::Ordering::Greater => {
                self.summary.release_after_pageflip_leases =
                    self.summary.release_after_pageflip_leases.saturating_add(1);
                self.summary.release_after_pageflip_obligations = self
                    .summary
                    .release_after_pageflip_obligations
                    .saturating_add(correlation.obligation_count as u64);
                self.pageflip_to_release_lag_ns
                    .record(gpu_signal_ns.saturating_sub(pageflip_ns));
            }
        }
    }

    fn record_exact_signal(
        &mut self,
        origin: DmabufGpuReleaseOrigin,
        registered_at_ns: u64,
        signal_ns: u64,
    ) {
        self.summary.exact_signal_timestamps =
            self.summary.exact_signal_timestamps.saturating_add(1);
        if registered_at_ns == 0 {
            // Test-only or legacy uncorrelated registrations have no
            // trustworthy registration timestamp for a wait sample.
        } else if signal_ns < registered_at_ns {
            self.summary.already_signaled_before_registration = self
                .summary
                .already_signaled_before_registration
                .saturating_add(1);
            self.fence_wait_ns.record(0);
        } else {
            self.fence_wait_ns
                .record(signal_ns.saturating_sub(registered_at_ns));
        }
        if let DmabufGpuReleaseOrigin::Composited { transaction_id } = origin {
            self.note_gpu_signal(transaction_id, signal_ns);
        }
    }

    #[cfg(test)]
    fn note_non_composited_signal(&mut self, origin: DmabufGpuReleaseOrigin) {
        debug_assert!(!matches!(origin, DmabufGpuReleaseOrigin::Composited { .. }));
        self.summary.exact_signal_timestamps =
            self.summary.exact_signal_timestamps.saturating_add(1);
    }

    fn note_timestamp_unavailable(&mut self, origin: DmabufGpuReleaseOrigin) {
        self.summary.signal_timestamp_unavailable =
            self.summary.signal_timestamp_unavailable.saturating_add(1);
        if let DmabufGpuReleaseOrigin::Composited { transaction_id } = origin
            && self.correlations.remove(&transaction_id).is_some()
        {
            self.summary.correlations_unpairable_signal_timestamp = self
                .summary
                .correlations_unpairable_signal_timestamp
                .saturating_add(1);
        }
    }

    fn summary(&self) -> DmabufGpuReleaseQualificationSummary {
        let (wait_p50, wait_p95, wait_p99) = self.fence_wait_ns.percentiles();
        let (lead_p50, lead_p95, lead_p99) = self.release_to_pageflip_lead_ns.percentiles();
        let (lag_p50, lag_p95, lag_p99) = self.pageflip_to_release_lag_ns.percentiles();
        let mut summary = self.summary;
        summary.correlation_pending = self.correlations.len();
        summary.gpu_release_registry_wait_p50_us = wait_p50 / 1_000;
        summary.gpu_release_registry_wait_p95_us = wait_p95 / 1_000;
        summary.gpu_release_registry_wait_p99_us = wait_p99 / 1_000;
        summary.release_to_pageflip_lead_p50_us = lead_p50 / 1_000;
        summary.release_to_pageflip_lead_p95_us = lead_p95 / 1_000;
        summary.release_to_pageflip_lead_p99_us = lead_p99 / 1_000;
        summary.pageflip_to_release_lag_p50_us = lag_p50 / 1_000;
        summary.pageflip_to_release_lag_p95_us = lag_p95 / 1_000;
        summary.pageflip_to_release_lag_p99_us = lag_p99 / 1_000;
        summary
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum DmabufReleaseRetryState {
    #[default]
    Idle,
    Pending {
        reason: DmabufReleaseRetryReason,
        next_retry_deadline_ns: u64,
        attempts: u32,
    },
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DmabufGpuReleaseMetrics {
    pub(crate) leases_registered: u64,
    pub(crate) leases_completed: u64,
    pub(crate) leases_requeued: u64,
    pub(crate) obligations_armed: u64,
    pub(crate) obligations_completed: u64,
    pub(crate) fences_created: u64,
    pub(crate) fences_signaled: u64,
    pub(crate) no_visual_fence_only: u64,
    pub(crate) fence_creation_failures: u64,
    pub(crate) completion_fd_failures: u64,
    pub(crate) registration_failures: u64,
    pub(crate) retry_skipped_current_token: u64,
    pub(crate) active_leases: u64,
    pub(crate) peak_active_leases: u64,
}

#[derive(Debug)]
struct DmabufGpuReleaseWatch {
    lease_id: DmabufGpuReleaseLeaseId,
    completion_fd: OwnedFd,
    origin: DmabufGpuReleaseOrigin,
    registered_at_ns: u64,
}

#[derive(Debug, Default)]
pub(crate) struct DmabufGpuReleaseRegistry {
    next_lease_id: u64,
    watches: HashMap<ReactorToken, DmabufGpuReleaseWatch>,
    metrics: DmabufGpuReleaseMetrics,
    observability: DmabufGpuReleaseObservability,
    retry: DmabufReleaseRetryState,
}

impl DmabufGpuReleaseRegistry {
    pub(crate) fn allocate_lease_id(&mut self) -> io::Result<DmabufGpuReleaseLeaseId> {
        let id = std::num::NonZeroU64::new(self.next_lease_id.max(1))
            .ok_or_else(|| io::Error::other("DMA-BUF GPU release lease IDs exhausted"))?;
        self.next_lease_id = self
            .next_lease_id
            .max(1)
            .checked_add(1)
            .ok_or_else(|| io::Error::other("DMA-BUF GPU release lease IDs exhausted"))?;
        Ok(DmabufGpuReleaseLeaseId::new(id))
    }

    #[cfg(test)]
    pub(crate) fn register(
        &mut self,
        lease_id: DmabufGpuReleaseLeaseId,
        completion_fd: OwnedFd,
        event_loop: &mut NativeEventLoop,
    ) -> io::Result<ReactorToken> {
        self.register_with_origin(
            lease_id,
            completion_fd,
            event_loop,
            DmabufGpuReleaseOrigin::Uncorrelated,
            0,
            0,
        )
    }

    pub(crate) fn register_with_origin(
        &mut self,
        lease_id: DmabufGpuReleaseLeaseId,
        completion_fd: OwnedFd,
        event_loop: &mut NativeEventLoop,
        origin: DmabufGpuReleaseOrigin,
        obligation_count: usize,
        registered_at_ns: u64,
    ) -> io::Result<ReactorToken> {
        if self
            .watches
            .values()
            .any(|watch| watch.lease_id == lease_id)
        {
            return Err(io::Error::other(
                "DMA-BUF GPU release lease ID is already active",
            ));
        }
        let token = event_loop.register(
            completion_fd.as_raw_fd(),
            NativeEventSource::DmabufGpuRelease,
        )?;
        if self
            .watches
            .insert(
                token,
                DmabufGpuReleaseWatch {
                    lease_id,
                    completion_fd,
                    origin,
                    registered_at_ns,
                },
            )
            .is_some()
        {
            let _ = event_loop.unregister(token);
            return Err(io::Error::other(
                "native reactor returned a duplicate DMA-BUF release token",
            ));
        }
        if let DmabufGpuReleaseOrigin::Composited { transaction_id } = origin {
            self.observability
                .arm_composited(transaction_id, obligation_count, registered_at_ns);
        }
        self.metrics.leases_registered = self.metrics.leases_registered.saturating_add(1);
        self.metrics.fences_created = self.metrics.fences_created.saturating_add(1);
        self.metrics.active_leases = self.metrics.active_leases.saturating_add(1);
        self.metrics.peak_active_leases = self
            .metrics
            .peak_active_leases
            .max(self.metrics.active_leases);
        Ok(token)
    }

    pub(crate) fn note_registration_failure(&mut self) {
        self.metrics.registration_failures = self.metrics.registration_failures.saturating_add(1);
    }

    pub(crate) fn note_completion_fd_failure(&mut self) {
        self.metrics.completion_fd_failures = self.metrics.completion_fd_failures.saturating_add(1);
    }

    pub(crate) fn note_fence_creation_failure(&mut self) {
        self.metrics.fence_creation_failures =
            self.metrics.fence_creation_failures.saturating_add(1);
    }

    pub(crate) fn note_obligations_armed(&mut self, count: usize) {
        self.metrics.obligations_armed =
            self.metrics.obligations_armed.saturating_add(count as u64);
    }

    pub(crate) fn note_no_visual_fence_only(&mut self) {
        self.metrics.no_visual_fence_only = self.metrics.no_visual_fence_only.saturating_add(1);
    }

    pub(crate) fn schedule_retry_if_needed(
        &mut self,
        reason: DmabufReleaseRetryReason,
        now_ns: u64,
    ) {
        if matches!(self.retry, DmabufReleaseRetryState::Idle) {
            self.retry = DmabufReleaseRetryState::Pending {
                reason,
                next_retry_deadline_ns: now_ns.saturating_add(DMABUF_RELEASE_RETRY_BASE_DELAY_NS),
                attempts: 0,
            };
        }
    }

    pub(crate) fn update_retry_for_deferred_work(
        &mut self,
        deferred_count: usize,
        retryable_count: usize,
        reason: DmabufReleaseRetryReason,
        now_ns: u64,
    ) {
        if retryable_count == 0 {
            if deferred_count > 0 {
                self.metrics.retry_skipped_current_token =
                    self.metrics.retry_skipped_current_token.saturating_add(1);
            }
            self.complete_retry();
        } else {
            self.schedule_retry_if_needed(reason, now_ns);
        }
    }

    pub(crate) fn retry_after_failure(&mut self, reason: DmabufReleaseRetryReason, now_ns: u64) {
        let attempts = match self.retry {
            DmabufReleaseRetryState::Idle => 1,
            DmabufReleaseRetryState::Pending { attempts, .. } => attempts.saturating_add(1),
        };
        let shift = attempts.min(8);
        let delay = DMABUF_RELEASE_RETRY_BASE_DELAY_NS
            .saturating_mul(1_u64 << shift)
            .min(DMABUF_RELEASE_RETRY_MAX_DELAY_NS);
        self.retry = DmabufReleaseRetryState::Pending {
            reason,
            next_retry_deadline_ns: now_ns.saturating_add(delay),
            attempts,
        };
    }

    pub(crate) fn retry_due(&self, now_ns: u64) -> bool {
        matches!(
            self.retry,
            DmabufReleaseRetryState::Pending {
                next_retry_deadline_ns,
                ..
            } if next_retry_deadline_ns <= now_ns
        )
    }

    pub(crate) fn retry_deadline_ns(&self) -> Option<u64> {
        match self.retry {
            DmabufReleaseRetryState::Idle => None,
            DmabufReleaseRetryState::Pending {
                next_retry_deadline_ns,
                ..
            } => Some(next_retry_deadline_ns),
        }
    }

    #[cfg(test)]
    pub(crate) fn retry_attempts(&self) -> u32 {
        match self.retry {
            DmabufReleaseRetryState::Idle => 0,
            DmabufReleaseRetryState::Pending { attempts, .. } => attempts,
        }
    }

    #[cfg(test)]
    pub(crate) fn is_visual_work(&self) -> bool {
        false
    }

    pub(crate) fn complete_retry(&mut self) {
        self.retry = DmabufReleaseRetryState::Idle;
    }

    pub(crate) fn metrics(&self) -> DmabufGpuReleaseMetrics {
        self.metrics
    }

    pub(crate) fn qualification_summary(&self) -> DmabufGpuReleaseQualificationSummary {
        self.observability.summary()
    }

    pub(crate) fn note_composited_pageflip(
        &mut self,
        transaction_id: OutputTransactionId,
        pageflip_ns: u64,
    ) {
        self.observability
            .note_composited_pageflip(transaction_id, pageflip_ns);
    }

    #[cfg(test)]
    pub(crate) fn active_count(&self) -> usize {
        self.watches.len()
    }

    pub(crate) fn service_ready(
        &mut self,
        tokens: &[ReactorToken],
        event_loop: &mut NativeEventLoop,
        server: &mut OwnCompositorServer,
    ) -> io::Result<usize> {
        self.service_ready_with_timestamp(
            tokens,
            event_loop,
            |watch| {
                query_sync_file_info(watch.completion_fd.as_fd())
                    .map(|info| Some(info.signal_timestamp_ns))
            },
            |lease_id| server.complete_dmabuf_gpu_release_lease(lease_id),
        )
    }

    #[cfg(test)]
    fn service_ready_with(
        &mut self,
        tokens: &[ReactorToken],
        event_loop: &mut NativeEventLoop,
        complete: impl FnMut(DmabufGpuReleaseLeaseId) -> usize,
    ) -> io::Result<usize> {
        self.service_ready_with_timestamp(tokens, event_loop, |_| Ok(None), complete)
    }

    fn service_ready_with_timestamp(
        &mut self,
        tokens: &[ReactorToken],
        event_loop: &mut NativeEventLoop,
        mut timestamp: impl FnMut(&DmabufGpuReleaseWatch) -> io::Result<Option<u64>>,
        mut complete: impl FnMut(DmabufGpuReleaseLeaseId) -> usize,
    ) -> io::Result<usize> {
        let mut completed = 0;
        for token in tokens.iter().copied() {
            if !self.watches.contains_key(&token) {
                continue;
            }
            // Unregister before removing the owned watch.  If the reactor
            // reports an error, the watch remains the registry's obligation.
            let _ = event_loop.unregister(token)?;
            let Some(watch) = self.watches.remove(&token) else {
                continue;
            };
            match timestamp(&watch) {
                Ok(Some(signal_ns)) => self.observability.record_exact_signal(
                    watch.origin,
                    watch.registered_at_ns,
                    signal_ns,
                ),
                Ok(None) | Err(_) => self.observability.note_timestamp_unavailable(watch.origin),
            }
            let watch_completed = complete(watch.lease_id);
            let _completion_fd = watch.completion_fd;
            completed += watch_completed;
            self.metrics.leases_completed = self.metrics.leases_completed.saturating_add(1);
            self.metrics.obligations_completed = self
                .metrics
                .obligations_completed
                .saturating_add(watch_completed as u64);
            self.metrics.fences_signaled = self.metrics.fences_signaled.saturating_add(1);
            self.metrics.active_leases = self.metrics.active_leases.saturating_sub(1);
        }
        Ok(completed)
    }

    pub(crate) fn cancel_all(
        &mut self,
        event_loop: &mut NativeEventLoop,
        server: &mut OwnCompositorServer,
    ) -> io::Result<usize> {
        self.cancel_all_with(event_loop, |lease_id| {
            server.requeue_dmabuf_gpu_release_lease(lease_id)
        })
    }

    fn cancel_all_with(
        &mut self,
        event_loop: &mut NativeEventLoop,
        mut requeue: impl FnMut(DmabufGpuReleaseLeaseId) -> usize,
    ) -> io::Result<usize> {
        let tokens = self.watches.keys().copied().collect::<Vec<_>>();
        let mut requeued = 0;
        for token in tokens {
            let _ = event_loop.unregister(token)?;
            let Some(watch) = self.watches.remove(&token) else {
                continue;
            };
            let _completion_fd = watch.completion_fd;
            requeued += requeue(watch.lease_id);
            self.metrics.leases_requeued = self.metrics.leases_requeued.saturating_add(1);
            self.metrics.active_leases = self.metrics.active_leases.saturating_sub(1);
        }
        Ok(requeued)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn arm_composited_dmabuf_release(
    registry: &mut DmabufGpuReleaseRegistry,
    server: &mut OwnCompositorServer,
    explicit: &AtomicEglGbmScanout,
    safety: DmabufGpuReleaseSafety,
    event_loop: &mut NativeEventLoop,
    batch_id: oblivion_one::compositor::CompositorFrameBatchId,
    lease_id: Option<DmabufGpuReleaseLeaseId>,
    transaction_id: OutputTransactionId,
) -> io::Result<usize> {
    let Some(lease_id) = lease_id else {
        return Ok(0);
    };
    let count = server.frame_batch_dmabuf_release_count(batch_id);
    if count == 0 || !safety.permits_compositor_gpu_release() {
        return Ok(0);
    }
    let completion_fd = match explicit.duplicate_ready_render_completion_fd() {
        Ok(fd) => fd,
        Err(_) => {
            registry.note_completion_fd_failure();
            return Ok(0);
        }
    };
    let transferred = server.transfer_frame_batch_dmabuf_releases_to_gpu_lease(batch_id, lease_id);
    if transferred == 0 {
        return Ok(0);
    }
    registry.note_obligations_armed(transferred);
    match registry.register_with_origin(
        lease_id,
        completion_fd,
        event_loop,
        DmabufGpuReleaseOrigin::Composited { transaction_id },
        transferred,
        oblivion_one::native::event_loop::monotonic_now_ns().unwrap_or(0),
    ) {
        Ok(_) => Ok(transferred),
        Err(_) => {
            registry.note_registration_failure();
            server.requeue_dmabuf_gpu_release_lease(lease_id);
            Ok(0)
        }
    }
}

impl super::NativeRuntime {
    pub(super) fn service_due_dmabuf_release_retry(
        &mut self,
        now_ns: u64,
    ) -> super::NativeResult<()> {
        if !self.dmabuf_gpu_release_registry.retry_due(now_ns) {
            return Ok(());
        }
        let deferred_count = self.server.deferred_dmabuf_release_count();
        let retryable_count = self.server.retryable_deferred_dmabuf_release_count();
        if deferred_count == 0 || retryable_count == 0 {
            self.dmabuf_gpu_release_registry
                .update_retry_for_deferred_work(
                    deferred_count,
                    retryable_count,
                    DmabufReleaseRetryReason::NoGpuProofAvailable,
                    now_ns,
                );
            return Ok(());
        }

        let safety = match &*self.scanout {
            crate::native_output::scanout::NativeScanoutBackend::AtomicEglGbm(explicit) => {
                dmabuf_gpu_release_safety(explicit, self.kms_commit_worker.as_ref())
            }
            _ => {
                // Compatibility backends retain their existing conservative
                // presentation-bound authority; they never enter this retry
                // loop.
                self.dmabuf_gpu_release_registry.complete_retry();
                return Ok(());
            }
        };
        if !safety.permits_compositor_gpu_release() {
            self.dmabuf_gpu_release_registry
                .retry_after_failure(DmabufReleaseRetryReason::DirectKmsOwnershipBlocked, now_ns);
            return Ok(());
        }

        let lease_id = self.dmabuf_gpu_release_registry.allocate_lease_id()?;
        let release_fence = match &*self.scanout {
            crate::native_output::scanout::NativeScanoutBackend::AtomicEglGbm(explicit) => {
                match explicit.create_render_fence() {
                    Ok(fence) => fence,
                    Err(_) => {
                        self.dmabuf_gpu_release_registry
                            .note_fence_creation_failure();
                        self.dmabuf_gpu_release_registry.retry_after_failure(
                            DmabufReleaseRetryReason::NoGpuProofAvailable,
                            now_ns,
                        );
                        return Ok(());
                    }
                }
            }
            _ => unreachable!("scanout backend changed during DMA-BUF retry"),
        };
        let completion_fd = match release_fence.duplicate_completion_fd() {
            Ok(fd) => fd,
            Err(_) => {
                self.dmabuf_gpu_release_registry
                    .note_completion_fd_failure();
                self.dmabuf_gpu_release_registry.retry_after_failure(
                    DmabufReleaseRetryReason::CompletionFdDuplicationFailed,
                    now_ns,
                );
                return Ok(());
            }
        };
        let transferred = self
            .server
            .transfer_deferred_dmabuf_releases_to_gpu_lease(lease_id);
        if transferred == 0 {
            self.dmabuf_gpu_release_registry.complete_retry();
            return Ok(());
        }
        self.dmabuf_gpu_release_registry
            .note_obligations_armed(transferred);
        match self.dmabuf_gpu_release_registry.register_with_origin(
            lease_id,
            completion_fd,
            &mut self.event_loop,
            DmabufGpuReleaseOrigin::DeferredRetry,
            transferred,
            oblivion_one::native::event_loop::monotonic_now_ns().unwrap_or(now_ns),
        ) {
            Ok(_) => {
                self.dmabuf_gpu_release_registry.complete_retry();
            }
            Err(_) => {
                self.dmabuf_gpu_release_registry.note_registration_failure();
                self.server.requeue_dmabuf_gpu_release_lease(lease_id);
                self.dmabuf_gpu_release_registry.retry_after_failure(
                    DmabufReleaseRetryReason::ReactorRegistrationFailed,
                    now_ns,
                );
            }
        }
        Ok(())
    }
}

impl super::NativeRuntime {
    pub(super) fn service_dmabuf_gpu_releases(
        &mut self,
        tokens: &[ReactorToken],
    ) -> super::NativeResult<()> {
        if tokens.is_empty() {
            return Ok(());
        }
        let completed = self.dmabuf_gpu_release_registry.service_ready(
            tokens,
            &mut self.event_loop,
            &mut self.server,
        )?;
        self.perf.log("native.dmabuf_gpu_release", || {
            vec![
                NativePerfField::usize("watches", tokens.len()),
                NativePerfField::usize("obligations_completed", completed),
            ]
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::{AsRawFd, FromRawFd};

    use super::*;

    fn pipe() -> (OwnedFd, OwnedFd) {
        let mut fds = [-1; 2];
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        (unsafe { OwnedFd::from_raw_fd(fds[0]) }, unsafe {
            OwnedFd::from_raw_fd(fds[1])
        })
    }

    fn lease_id(registry: &mut DmabufGpuReleaseRegistry) -> DmabufGpuReleaseLeaseId {
        registry.allocate_lease_id().unwrap()
    }

    #[test]
    fn gpu_release_readiness_is_a_dedicated_reactor_domain() {
        let mut event_loop = NativeEventLoop::new().unwrap();
        let (read, write) = pipe();
        let mut registry = DmabufGpuReleaseRegistry::default();
        let id = lease_id(&mut registry);
        let token = registry.register(id, read, &mut event_loop).unwrap();

        assert_eq!(registry.active_count(), 1);
        assert_eq!(
            event_loop.source_for_token(token),
            Some(NativeEventSource::DmabufGpuRelease)
        );
        unsafe { libc::write(write.as_raw_fd(), [1_u8].as_ptr().cast(), 1) };
        let wakeup = event_loop.wait().unwrap();
        assert!(wakeup.reasons.dmabuf_gpu_release());
        assert_eq!(wakeup.dmabuf_gpu_release_tokens, vec![token]);
        assert!(wakeup.explicit_sync_acquire_tokens.is_empty());

        let mut completed_ids = Vec::new();
        registry
            .service_ready_with(&wakeup.dmabuf_gpu_release_tokens, &mut event_loop, |id| {
                completed_ids.push(id);
                1
            })
            .unwrap();
        assert_eq!(completed_ids, vec![id]);
        assert_eq!(registry.active_count(), 0);
        assert_eq!(event_loop.source_for_token(token), None);
        assert_eq!(registry.metrics().leases_completed, 1);
    }

    #[test]
    fn multiple_release_watches_are_serviced_independently() {
        let mut event_loop = NativeEventLoop::new().unwrap();
        let (read_a, write_a) = pipe();
        let (read_b, write_b) = pipe();
        let mut registry = DmabufGpuReleaseRegistry::default();
        let id_a = lease_id(&mut registry);
        let id_b = lease_id(&mut registry);
        let token_a = registry.register(id_a, read_a, &mut event_loop).unwrap();
        let token_b = registry.register(id_b, read_b, &mut event_loop).unwrap();
        unsafe {
            libc::write(write_a.as_raw_fd(), [1_u8].as_ptr().cast(), 1);
            libc::write(write_b.as_raw_fd(), [1_u8].as_ptr().cast(), 1);
        }
        let wakeup = event_loop.wait().unwrap();
        let mut completed_ids = Vec::new();
        registry
            .service_ready_with(&wakeup.dmabuf_gpu_release_tokens, &mut event_loop, |id| {
                completed_ids.push(id);
                1
            })
            .unwrap();
        assert_eq!(completed_ids.len(), 2);
        assert!(completed_ids.contains(&id_a));
        assert!(completed_ids.contains(&id_b));
        assert_eq!(registry.active_count(), 0);
        assert_eq!(event_loop.source_for_token(token_a), None);
        assert_eq!(event_loop.source_for_token(token_b), None);
    }

    #[test]
    fn cancellation_requeues_and_unregisters_without_a_wake_loop() {
        let mut event_loop = NativeEventLoop::new().unwrap();
        let (read, _write) = pipe();
        let mut registry = DmabufGpuReleaseRegistry::default();
        let id = lease_id(&mut registry);
        let token = registry.register(id, read, &mut event_loop).unwrap();
        let mut requeued_ids = Vec::new();
        registry
            .cancel_all_with(&mut event_loop, |id| {
                requeued_ids.push(id);
                1
            })
            .unwrap();
        assert_eq!(requeued_ids, vec![id]);
        assert_eq!(registry.active_count(), 0);
        assert_eq!(event_loop.source_for_token(token), None);
        assert_eq!(registry.metrics().leases_requeued, 1);
    }

    #[test]
    fn every_worker_direct_ownership_state_blocks_gpu_release() {
        for (queued, executing, inflight) in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let safety = DmabufGpuReleaseSafety::from_ownership(false, queued, executing, inflight);
            assert!(safety.direct_kms_ownership_live);
            assert!(!safety.permits_compositor_gpu_release());
        }
    }

    #[test]
    fn empty_worker_and_atomic_direct_ownership_allows_gpu_release() {
        let safety = DmabufGpuReleaseSafety::from_ownership(false, false, false, false);

        assert!(!safety.direct_kms_ownership_live);
        assert!(safety.permits_compositor_gpu_release());
    }

    #[test]
    fn deferred_release_retry_debt_uses_capped_backoff_without_visual_work() {
        let mut registry = DmabufGpuReleaseRegistry::default();
        registry.schedule_retry_if_needed(DmabufReleaseRetryReason::NoGpuProofAvailable, 1_000);

        let first_deadline = registry.retry_deadline_ns().unwrap();
        assert!(first_deadline > 1_000);
        assert!(!registry.retry_due(first_deadline - 1));
        assert!(registry.retry_due(first_deadline));
        assert!(!registry.is_visual_work());

        registry.retry_after_failure(
            DmabufReleaseRetryReason::CompletionFdDuplicationFailed,
            first_deadline,
        );
        let second_deadline = registry.retry_deadline_ns().unwrap();
        assert!(second_deadline > first_deadline);
        assert_eq!(registry.retry_attempts(), 1);

        for attempt in 0..32 {
            registry.retry_after_failure(
                DmabufReleaseRetryReason::ReactorRegistrationFailed,
                second_deadline + attempt,
            );
        }
        assert!(registry.retry_deadline_ns().unwrap() - second_deadline < 1_000_000_000);
        assert!(!registry.is_visual_work());
        registry.complete_retry();
        assert_eq!(registry.retry_deadline_ns(), None);
    }

    #[test]
    fn current_token_only_deferred_work_does_not_arm_retry_debt() {
        let mut registry = DmabufGpuReleaseRegistry::default();
        for now_ns in (0..1_000_000_000).step_by(1_000_000) {
            registry.update_retry_for_deferred_work(
                1,
                0,
                DmabufReleaseRetryReason::NoGpuProofAvailable,
                now_ns,
            );
        }

        assert_eq!(registry.retry_deadline_ns(), None);
        assert_eq!(registry.retry_attempts(), 0);
        assert_eq!(registry.metrics().retry_skipped_current_token, 1_000);
    }

    fn transaction_id(value: u64) -> crate::native_output::OutputTransactionId {
        crate::native_output::OutputTransactionId::new(
            std::num::NonZeroU64::new(value).expect("transaction ID is non-zero"),
        )
    }

    #[test]
    fn qualification_classifies_gpu_completion_before_pageflip_by_timestamp() {
        let mut observability = DmabufGpuReleaseObservability::default();
        let transaction_id = transaction_id(1);
        observability.arm_composited(transaction_id, 3, 1_000_000);
        observability.note_gpu_signal(transaction_id, 2_000_000);
        observability.note_composited_pageflip(transaction_id, 5_000_000);

        let summary = observability.summary();
        assert_eq!(summary.composited_correlations_paired, 1);
        assert_eq!(summary.release_before_pageflip_leases, 1);
        assert_eq!(summary.release_before_pageflip_obligations, 3);
        assert_eq!(summary.release_to_pageflip_lead_p50_us, 3_000);
    }

    #[test]
    fn qualification_classifies_gpu_completion_after_pageflip_by_timestamp() {
        let mut observability = DmabufGpuReleaseObservability::default();
        let transaction_id = transaction_id(2);
        observability.arm_composited(transaction_id, 2, 1_000_000);
        observability.note_composited_pageflip(transaction_id, 4_000_000);
        observability.note_gpu_signal(transaction_id, 5_000_000);

        let summary = observability.summary();
        assert_eq!(summary.release_after_pageflip_leases, 1);
        assert_eq!(summary.release_after_pageflip_obligations, 2);
        assert_eq!(summary.pageflip_to_release_lag_p50_us, 1_000);
    }

    #[test]
    fn qualification_equal_timestamps_are_not_before_or_after() {
        let mut observability = DmabufGpuReleaseObservability::default();
        let transaction_id = transaction_id(3);
        observability.arm_composited(transaction_id, 1, 1_000_000);
        observability.note_gpu_signal(transaction_id, 4_000_000);
        observability.note_composited_pageflip(transaction_id, 4_000_000);

        let summary = observability.summary();
        assert_eq!(summary.release_same_timestamp_leases, 1);
        assert_eq!(summary.release_before_pageflip_leases, 0);
        assert_eq!(summary.release_after_pageflip_leases, 0);
    }

    #[test]
    fn qualification_uses_physical_timestamps_not_delivery_order() {
        let mut gpu_first = DmabufGpuReleaseObservability::default();
        let first = transaction_id(4);
        gpu_first.arm_composited(first, 1, 1_000_000);
        gpu_first.note_gpu_signal(first, 1_100_000);
        gpu_first.note_composited_pageflip(first, 1_000_000);

        let mut pageflip_first = DmabufGpuReleaseObservability::default();
        let second = transaction_id(5);
        pageflip_first.arm_composited(second, 1, 1_000_000);
        pageflip_first.note_composited_pageflip(second, 1_100_000);
        pageflip_first.note_gpu_signal(second, 1_000_000);

        assert_eq!(gpu_first.summary().release_after_pageflip_leases, 1);
        assert_eq!(pageflip_first.summary().release_before_pageflip_leases, 1);
    }

    #[test]
    fn timestamp_unavailability_does_not_classify_or_block_release() {
        let mut event_loop = NativeEventLoop::new().unwrap();
        let (read, write) = pipe();
        let mut registry = DmabufGpuReleaseRegistry::default();
        let id = lease_id(&mut registry);
        let token = registry
            .register_with_origin(
                id,
                read,
                &mut event_loop,
                DmabufGpuReleaseOrigin::Composited {
                    transaction_id: transaction_id(6),
                },
                1,
                1_000_000,
            )
            .unwrap();
        unsafe { libc::write(write.as_raw_fd(), [1_u8].as_ptr().cast(), 1) };
        let wakeup = event_loop.wait().unwrap();
        let mut completed = Vec::new();
        registry
            .service_ready_with_timestamp(
                &wakeup.dmabuf_gpu_release_tokens,
                &mut event_loop,
                |_| Err(io::Error::other("timestamp unavailable")),
                |lease_id| {
                    completed.push(lease_id);
                    1
                },
            )
            .unwrap();

        assert_eq!(completed, vec![id]);
        let summary = registry.qualification_summary();
        assert_eq!(summary.signal_timestamp_unavailable, 1);
        assert_eq!(summary.correlations_unpairable_signal_timestamp, 1);
        assert_eq!(summary.correlation_pending, 0);
        assert_eq!(summary.composited_correlations_paired, 0);
        assert_eq!(event_loop.source_for_token(token), None);
    }

    #[test]
    fn no_visual_and_deferred_retry_origins_do_not_create_pageflip_pairs() {
        let mut observability = DmabufGpuReleaseObservability::default();
        observability.note_non_composited_signal(DmabufGpuReleaseOrigin::NoVisual);
        observability.note_non_composited_signal(DmabufGpuReleaseOrigin::DeferredRetry);

        let summary = observability.summary();
        assert_eq!(summary.exact_signal_timestamps, 2);
        assert_eq!(summary.composited_correlations_paired, 0);
        assert_eq!(summary.release_before_pageflip_leases, 0);
        assert_eq!(summary.release_after_pageflip_leases, 0);
    }

    #[test]
    fn qualification_fence_wait_percentiles_use_bounded_samples() {
        let mut observability = DmabufGpuReleaseObservability::default();
        for (index, wait_ns) in [1, 2, 3, 4, 5].into_iter().enumerate() {
            observability.record_exact_signal(
                DmabufGpuReleaseOrigin::NoVisual,
                (index as u64 + 1) * 1_000_000,
                (index as u64 + 1) * 1_000_000 + wait_ns * 1_000_000,
            );
        }

        let summary = observability.summary();
        assert_eq!(summary.gpu_release_registry_wait_p50_us, 3_000);
        assert_eq!(summary.gpu_release_registry_wait_p95_us, 5_000);
        assert_eq!(summary.gpu_release_registry_wait_p99_us, 5_000);
    }

    #[test]
    fn inverted_registration_and_signal_timestamps_are_excluded_from_wait_samples() {
        let mut observability = DmabufGpuReleaseObservability::default();
        observability.record_exact_signal(DmabufGpuReleaseOrigin::NoVisual, 5_000_000, 4_000_000);

        let summary = observability.summary();
        assert_eq!(summary.already_signaled_before_registration, 1);
        assert_eq!(summary.timestamp_order_anomalies, 0);
        assert_eq!(summary.gpu_release_registry_wait_p50_us, 0);
    }

    #[test]
    fn qualification_correlation_capacity_is_bounded() {
        let mut observability = DmabufGpuReleaseObservability::default();
        for value in 1..=(DMABUF_GPU_RELEASE_CORRELATION_CAPACITY as u64 + 7) {
            let _ = observability.arm_composited(transaction_id(value), 1, value);
        }

        let summary = observability.summary();
        assert_eq!(
            summary.correlation_pending,
            DMABUF_GPU_RELEASE_CORRELATION_CAPACITY
        );
        assert_eq!(summary.correlation_overflows, 7);
    }

    #[test]
    fn qualification_pairs_only_matching_transactions() {
        let mut observability = DmabufGpuReleaseObservability::default();
        let first = transaction_id(300);
        let second = transaction_id(301);
        let third = transaction_id(302);
        observability.arm_composited(first, 1, 1);
        observability.arm_composited(second, 2, 1);
        observability.arm_composited(third, 3, 1);
        observability.note_composited_pageflip(second, 20);
        observability.note_gpu_signal(third, 30);
        observability.note_gpu_signal(second, 10);
        observability.note_composited_pageflip(third, 40);

        let summary = observability.summary();
        assert_eq!(summary.composited_correlations_paired, 2);
        assert_eq!(summary.release_before_pageflip_obligations, 5);
        assert_eq!(summary.correlation_pending, 1);
    }

    #[test]
    fn duplicate_transaction_correlation_does_not_replace_original() {
        let mut observability = DmabufGpuReleaseObservability::default();
        let transaction_id = transaction_id(400);
        assert!(observability.arm_composited(transaction_id, 1, 1));
        assert!(!observability.arm_composited(transaction_id, 2, 2));
        observability.note_gpu_signal(transaction_id, 3);
        observability.note_composited_pageflip(transaction_id, 4);

        let summary = observability.summary();
        assert_eq!(summary.correlation_duplicates, 1);
        assert_eq!(summary.release_before_pageflip_obligations, 1);
    }

    #[test]
    fn pre_signaled_completion_is_zero_wait_not_a_timestamp_anomaly() {
        let mut observability = DmabufGpuReleaseObservability::default();
        observability.record_exact_signal(DmabufGpuReleaseOrigin::NoVisual, 5_000_000, 4_000_000);

        let summary = observability.summary();
        assert_eq!(summary.already_signaled_before_registration, 1);
        assert_eq!(summary.timestamp_order_anomalies, 0);
        assert_eq!(summary.gpu_release_registry_wait_p50_us, 0);
    }

    #[test]
    fn post_registration_completion_records_remaining_registry_wait() {
        let mut observability = DmabufGpuReleaseObservability::default();
        observability.record_exact_signal(DmabufGpuReleaseOrigin::NoVisual, 4_000_000, 5_500_000);

        let summary = observability.summary();
        assert_eq!(summary.already_signaled_before_registration, 0);
        assert_eq!(summary.gpu_release_registry_wait_p50_us, 1_500);
    }

    #[test]
    fn registry_wait_percentiles_include_pre_signaled_zero_samples() {
        let mut observability = DmabufGpuReleaseObservability::default();
        observability.record_exact_signal(DmabufGpuReleaseOrigin::NoVisual, 5, 4);
        for (index, wait_ns) in [1, 2, 3, 4].into_iter().enumerate() {
            let registered_at_ns = (index as u64 + 1) * 10_000_000;
            observability.record_exact_signal(
                DmabufGpuReleaseOrigin::NoVisual,
                registered_at_ns,
                registered_at_ns + wait_ns * 1_000_000,
            );
        }

        let summary = observability.summary();
        assert_eq!(summary.gpu_release_registry_wait_p50_us, 2_000);
        assert_eq!(summary.gpu_release_registry_wait_p95_us, 4_000);
        assert_eq!(summary.gpu_release_registry_wait_p99_us, 4_000);
    }

    #[test]
    fn registration_wait_semantics_do_not_change_physical_pageflip_classification() {
        let mut observability = DmabufGpuReleaseObservability::default();
        let transaction_id = transaction_id(600);
        observability.arm_composited(transaction_id, 1, 5_000_000);
        observability.note_gpu_signal(transaction_id, 4_000_000);
        observability.note_composited_pageflip(transaction_id, 8_000_000);

        let summary = observability.summary();
        assert_eq!(summary.release_before_pageflip_leases, 1);
        assert_eq!(summary.release_to_pageflip_lead_p50_us, 4_000);
        assert_eq!(summary.already_signaled_before_registration, 0);
    }

    #[test]
    fn unavailable_composited_timestamp_removes_unpairable_correlation() {
        let mut observability = DmabufGpuReleaseObservability::default();
        let transaction_id = transaction_id(601);
        observability.arm_composited(transaction_id, 1, 1);
        observability.note_composited_pageflip(transaction_id, 2);
        observability
            .note_timestamp_unavailable(DmabufGpuReleaseOrigin::Composited { transaction_id });

        let summary = observability.summary();
        assert_eq!(summary.signal_timestamp_unavailable, 1);
        assert_eq!(summary.correlations_unpairable_signal_timestamp, 1);
        assert_eq!(summary.correlation_pending, 0);
        assert_eq!(summary.composited_correlations_paired, 0);
        assert_eq!(summary.release_before_pageflip_leases, 0);
        assert_eq!(summary.release_after_pageflip_leases, 0);
        assert_eq!(summary.release_same_timestamp_leases, 0);
    }

    #[test]
    fn repeated_unavailable_composited_timestamps_do_not_fill_ledger() {
        let mut observability = DmabufGpuReleaseObservability::default();
        for value in 1..=(DMABUF_GPU_RELEASE_CORRELATION_CAPACITY as u64 + 7) {
            let transaction_id = transaction_id(value);
            let _ = observability.arm_composited(transaction_id, 1, value);
            observability
                .note_timestamp_unavailable(DmabufGpuReleaseOrigin::Composited { transaction_id });
        }

        let summary = observability.summary();
        assert_eq!(summary.correlation_pending, 0);
        assert_eq!(summary.correlation_overflows, 0);
        assert_eq!(
            summary.correlations_unpairable_signal_timestamp,
            DMABUF_GPU_RELEASE_CORRELATION_CAPACITY as u64 + 7,
        );
        assert_eq!(
            summary.composited_correlations_armed,
            DMABUF_GPU_RELEASE_CORRELATION_CAPACITY as u64 + 7,
        );
    }

    #[test]
    fn armed_counts_only_successful_correlation_insertions() {
        let mut observability = DmabufGpuReleaseObservability::default();
        let transaction_id = transaction_id(602);
        assert!(observability.arm_composited(transaction_id, 1, 1));
        assert!(!observability.arm_composited(transaction_id, 1, 2));

        let summary = observability.summary();
        assert_eq!(summary.composited_correlations_armed, 1);
        assert_eq!(summary.correlation_duplicates, 1);
    }

    #[test]
    fn normal_registered_lease_completes_once_while_correlation_is_metrics_only() {
        let mut event_loop = NativeEventLoop::new().unwrap();
        let (read, write) = pipe();
        let mut registry = DmabufGpuReleaseRegistry::default();
        let id = lease_id(&mut registry);
        let transaction_id = transaction_id(500);
        let token = registry
            .register_with_origin(
                id,
                read,
                &mut event_loop,
                DmabufGpuReleaseOrigin::Composited { transaction_id },
                3,
                1_000,
            )
            .unwrap();
        unsafe { libc::write(write.as_raw_fd(), [1_u8].as_ptr().cast(), 1) };
        let wakeup = event_loop.wait().unwrap();
        let mut completions = 0;
        registry
            .service_ready_with_timestamp(
                &wakeup.dmabuf_gpu_release_tokens,
                &mut event_loop,
                |_| Ok(Some(2_000)),
                |_| {
                    completions += 1;
                    3
                },
            )
            .unwrap();
        registry.note_composited_pageflip(transaction_id, 3_000);

        assert_eq!(completions, 1);
        assert_eq!(registry.metrics().leases_completed, 1);
        assert_eq!(
            registry
                .qualification_summary()
                .composited_correlations_paired,
            1
        );
        assert_eq!(event_loop.source_for_token(token), None);
    }
}
