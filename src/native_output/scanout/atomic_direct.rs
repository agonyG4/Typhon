use std::os::fd::OwnedFd;

use oblivion_one::compositor::{
    CompositorFrameBatchId, DirectScanoutSceneCandidate, DirectScanoutSceneRejection,
};

use super::*;

#[derive(Debug)]
pub(crate) struct SubmittedDirectPrimary {
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) token: PageFlipToken,
    pub(crate) lease: DirectPrimaryLease,
    pub(crate) submit_started_at: MonotonicTimestampNs,
    pub(crate) submit_returned_at: MonotonicTimestampNs,
    pub(crate) out_fence: Option<OwnedFd>,
    pub(crate) frame_id: u64,
    pub(crate) protocol_batch_id: CompositorFrameBatchId,
    pub(crate) target: PresentationTarget,
}

#[derive(Debug)]
pub(crate) struct PresentedDirectPrimary {
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) token: PageFlipToken,
    pub(crate) lease: DirectPrimaryLease,
    pub(crate) presented_at: MonotonicTimestampNs,
    pub(crate) frame_id: u64,
    pub(crate) protocol_batch_id: CompositorFrameBatchId,
    pub(crate) target: PresentationTarget,
    pub(crate) submit_started_at: MonotonicTimestampNs,
    pub(crate) submit_returned_at: MonotonicTimestampNs,
}

// The ledger owns logical obligations, the worker job owns queued resources,
// the arbiter owns kernel identity, and this type owns only submitted,
// presented, or suspended direct leases.
#[derive(Debug, Default)]
pub(crate) struct DirectPrimaryOwnership {
    pub(crate) submitted: Option<SubmittedDirectPrimary>,
    pub(crate) presented: Option<PresentedDirectPrimary>,
    pub(crate) suspended: Vec<DirectPrimaryLease>,
}

#[derive(Debug)]
pub(crate) struct SubmittedDirectPrimaryError {
    pub(crate) error: io::Error,
    pub(crate) submitted: SubmittedDirectPrimary,
}

#[derive(Debug)]
pub(crate) struct DirectOwnershipError {
    pub(crate) error: io::Error,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DirectPageflipInfo {
    pub(crate) frame_id: u64,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) token: PageFlipToken,
    pub(crate) surface_id: u32,
    pub(crate) candidate_key: DirectScanoutCandidateKey,
    pub(crate) protocol_batch_id: CompositorFrameBatchId,
    pub(crate) target: PresentationTarget,
    pub(crate) submit_started_at: MonotonicTimestampNs,
    pub(crate) submit_returned_at: MonotonicTimestampNs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExpectedPresentedDirectPrimary {
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) token: PageFlipToken,
    pub(crate) surface_id: u32,
    pub(crate) candidate_key: DirectScanoutCandidateKey,
    pub(crate) framebuffer_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectRetirementMismatch {
    MissingOwnership,
    SubmittedOwnership,
    WorkerOwnership,
    SuspendedOwnership,
    TransactionId,
    PageflipToken,
    CandidateKey,
    SurfaceId,
    FramebufferId,
}

#[derive(Debug)]
pub(crate) enum PresentedDirectRetirement {
    Retired {
        lease: DirectPrimaryLease,
    },
    Mismatch {
        expected: ExpectedPresentedDirectPrimary,
        retained: Box<PresentedDirectPrimary>,
        reason: DirectRetirementMismatch,
    },
    Missing {
        expected: ExpectedPresentedDirectPrimary,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedDirectPageflip {
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) token: PageFlipToken,
    pub(crate) presented_at: MonotonicTimestampNs,
    pub(crate) surface_damage: oblivion_one::compositor::SurfaceDamagePresentation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectReleaseProof {
    ComposedPageflip,
    Restored,
    TargetDestroyed,
    Unproven,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectReleaseDeferral {
    SubmittedOwnership,
    WorkerOwnership,
    UnprovenTeardown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectReleaseViolation {
    Submitted,
    Worker,
    Suspended,
    Retirement(DirectRetirementMismatch),
}

#[derive(Debug)]
pub(crate) enum DirectReleaseOutcome {
    Released {
        presented: Option<Box<PresentedDirectPrimary>>,
        suspended: Vec<DirectPrimaryLease>,
    },
    Deferred {
        reason: DirectReleaseDeferral,
    },
    Violation {
        reason: DirectReleaseViolation,
    },
}

#[derive(Debug)]
pub(crate) enum CompositedTransitionResult {
    Completed {
        released: Option<Box<DirectPrimaryLease>>,
    },
    Fatal {
        reason: DirectReleaseViolation,
    },
}

#[derive(Debug)]
pub(crate) struct DirectPageflipCompletion {
    pub(crate) frame_id: u64,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) token: PageFlipToken,
    pub(crate) surface_id: u32,
    pub(crate) framebuffer_id: u32,
    pub(crate) candidate_key: DirectScanoutCandidateKey,
    pub(crate) protocol_batch_id: CompositorFrameBatchId,
    pub(crate) target: PresentationTarget,
    pub(crate) presented_at: MonotonicTimestampNs,
    pub(crate) submit_started_at: MonotonicTimestampNs,
    pub(crate) submit_returned_at: MonotonicTimestampNs,
    pub(crate) surface_damage: oblivion_one::compositor::SurfaceDamagePresentation,
    pub(crate) replaced: Option<PresentedDirectPrimary>,
}

#[derive(Debug)]
pub(crate) enum DirectScanoutAttempt {
    Rejected(DirectScanoutSceneRejection),
    Fallback(&'static str),
    Unchanged,
    AdmissionRejected {
        transaction_id: OutputTransactionId,
        reason: crate::native_output::kms_worker::KmsWorkerAdmissionError,
    },
    WorkerQueued {
        transaction_id: OutputTransactionId,
        token: u64,
        framebuffer_id: u32,
        cursor_revision: Option<crate::native_output::presentation::plane::CursorRevision>,
        lease: Box<DirectPrimaryLease>,
        admission: crate::native_output::kms_worker::KmsCommitAdmissionPermit,
        test_only: crate::native_output::kms_worker::KmsTestOnlyPolicy,
    },
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct DirectScanoutCounters {
    pub(crate) candidate_checks: u64,
    pub(crate) candidates_accepted: u64,
    pub(crate) import_attempts: u64,
    pub(crate) import_cache_hits: u64,
    pub(crate) import_failures: u64,
    pub(crate) test_only_attempts: u64,
    pub(crate) test_only_rejections: u64,
    pub(crate) submit_rejections: u64,
    pub(crate) submissions: u64,
    pub(crate) presentations: u64,
    pub(crate) entries: u64,
    pub(crate) direct_replacements: u64,
    pub(crate) exits: u64,
    pub(crate) combined_cursor_rejections: u64,
    pub(crate) fallback_redraws: u64,
    pub(crate) same_buffer_resubmissions: u64,
    pub(crate) same_buffer_suppressed: u64,
    pub(crate) out_fences_received: u64,
    pub(crate) out_fence_missing: u64,
    pub(crate) test_only_timing: TimingSummary,
    pub(crate) real_submit_timing: TimingSummary,
    pub(crate) composited_fallbacks: u64,
    pub(crate) stale_candidate_rejections: u64,
    pub(crate) cleanup_failures: u64,
    pub(crate) composited_render_ahead_suppressed: u64,
    pub(crate) worker_admission_rejected: u64,
    pub(crate) live_leases: u64,
    pub(crate) validation_cache_hits: u64,
    pub(crate) validation_cache_misses: u64,
    pub(crate) real_submit_attempts: u64,
    pub(crate) fallback_cycles: u64,
    pub(crate) fallback_cycles_current: u64,
    pub(crate) fallback_cycles_last: u64,
    pub(crate) fallback_cycles_max: u64,
    pub(crate) duplicate_feedback: u64,
    pub(crate) early_release_prevented: u64,
    pub(crate) early_release_violations: u64,
    pub(crate) dmabuf_feedback_unchanged_rebuilds: u64,
    pub(crate) worker_queue_overflow: u64,
    pub(crate) callback_owner_leak_events: u64,
    pub(crate) callback_owner_leaked_callbacks: u64,
    pub(crate) first_blocker: Option<&'static str>,
    pub(crate) blocker_set: u64,
}

impl DirectScanoutCounters {
    pub(crate) fn record_test_only(&mut self, duration_ns: u64, rejected: bool) {
        self.test_only_attempts = self.test_only_attempts.saturating_add(1);
        self.test_only_timing.record(duration_ns);
        if rejected {
            self.test_only_rejections = self.test_only_rejections.saturating_add(1);
        }
    }

    pub(crate) fn record_real_submit_attempt(&mut self, rejected: bool) {
        self.real_submit_attempts = self.real_submit_attempts.saturating_add(1);
        if rejected {
            self.submit_rejections = self.submit_rejections.saturating_add(1);
        }
    }
}

pub(crate) struct DirectScanoutControl {
    pub(crate) ownership: DirectPrimaryOwnership,
    pub(crate) framebuffer_cache: DirectFramebufferCache,
    pub(crate) inhibit_until_composited_present: bool,
    pub(crate) counters: DirectScanoutCounters,
    pub(crate) drm_generation: u64,
    pub(crate) validation_cache: DirectPlaneValidationCache,
    pub(crate) live_lease_count: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub(super) identity_viewport_metadata_logged: bool,
    pub(super) last_debug_candidate: Option<(u32, u64, u64, u64)>,
}

pub(super) fn direct_candidate_key(
    candidate: &DirectScanoutSceneCandidate,
    drm_generation: u64,
    cursor: Option<&AtomicCursorVisualState>,
) -> Option<DirectScanoutCandidateKey> {
    DirectScanoutCandidateKey::from_candidate(
        candidate,
        drm_generation,
        direct_cursor_content_key(cursor, true),
        0,
    )
}

pub(super) fn direct_scanout_debug(message: impl std::fmt::Display) {
    if std::env::var("TYPHON_DIRECT_SCANOUT_DEBUG").ok().as_deref() == Some("1") {
        eprintln!("direct scanout: {message}");
    }
}

impl DirectPrimaryOwnership {
    fn retirement_mismatch(
        &self,
        expected: ExpectedPresentedDirectPrimary,
        worker_owns_current: bool,
    ) -> Option<DirectRetirementMismatch> {
        if self.presented.is_none() {
            return Some(DirectRetirementMismatch::MissingOwnership);
        }
        if self.submitted.is_some() {
            return Some(DirectRetirementMismatch::SubmittedOwnership);
        }
        if worker_owns_current {
            return Some(DirectRetirementMismatch::WorkerOwnership);
        }
        if !self.suspended.is_empty() {
            return Some(DirectRetirementMismatch::SuspendedOwnership);
        }
        let presented = self
            .presented
            .as_ref()
            .expect("presented ownership checked");
        (presented.transaction_id != expected.transaction_id)
            .then_some(DirectRetirementMismatch::TransactionId)
            .or_else(|| {
                (presented.token != expected.token)
                    .then_some(DirectRetirementMismatch::PageflipToken)
            })
            .or_else(|| {
                (presented.lease.key() != expected.candidate_key)
                    .then_some(DirectRetirementMismatch::CandidateKey)
            })
            .or_else(|| {
                (presented.lease.surface_id() != expected.surface_id)
                    .then_some(DirectRetirementMismatch::SurfaceId)
            })
            .or_else(|| {
                (presented.lease.framebuffer_id() != expected.framebuffer_id)
                    .then_some(DirectRetirementMismatch::FramebufferId)
            })
    }

    pub(crate) fn validate_presented_direct(
        &self,
        expected: ExpectedPresentedDirectPrimary,
        worker_owns_current: bool,
    ) -> Result<(), DirectRetirementMismatch> {
        self.retirement_mismatch(expected, worker_owns_current)
            .map_or(Ok(()), Err)
    }

    pub(crate) fn retire_presented_direct(
        &mut self,
        expected: ExpectedPresentedDirectPrimary,
        worker_owns_current: bool,
    ) -> PresentedDirectRetirement {
        let reason = self.retirement_mismatch(expected, worker_owns_current);
        if matches!(reason, Some(DirectRetirementMismatch::MissingOwnership)) {
            return PresentedDirectRetirement::Missing { expected };
        }
        if let Some(reason) = reason {
            return PresentedDirectRetirement::Mismatch {
                expected,
                retained: Box::new(self.presented.take().expect("presented ownership checked")),
                reason,
            };
        }
        PresentedDirectRetirement::Retired {
            lease: self
                .presented
                .take()
                .expect("presented ownership checked")
                .lease,
        }
    }

    pub(crate) fn request_direct_release(
        &mut self,
        proof: DirectReleaseProof,
        worker_owns_current: bool,
    ) -> DirectReleaseOutcome {
        match proof {
            DirectReleaseProof::ComposedPageflip => {
                if self.submitted.is_some() {
                    return DirectReleaseOutcome::Violation {
                        reason: DirectReleaseViolation::Submitted,
                    };
                }
                if worker_owns_current {
                    return DirectReleaseOutcome::Violation {
                        reason: DirectReleaseViolation::Worker,
                    };
                }
                if !self.suspended.is_empty() {
                    return DirectReleaseOutcome::Violation {
                        reason: DirectReleaseViolation::Suspended,
                    };
                }
                DirectReleaseOutcome::Released {
                    presented: self.presented.take().map(Box::new),
                    suspended: Vec::new(),
                }
            }
            DirectReleaseProof::Unproven => {
                if self.submitted.is_some() {
                    DirectReleaseOutcome::Deferred {
                        reason: DirectReleaseDeferral::SubmittedOwnership,
                    }
                } else if worker_owns_current {
                    DirectReleaseOutcome::Deferred {
                        reason: DirectReleaseDeferral::WorkerOwnership,
                    }
                } else if self.presented.is_some() || !self.suspended.is_empty() {
                    DirectReleaseOutcome::Deferred {
                        reason: DirectReleaseDeferral::UnprovenTeardown,
                    }
                } else {
                    DirectReleaseOutcome::Released {
                        presented: None,
                        suspended: Vec::new(),
                    }
                }
            }
            DirectReleaseProof::Restored | DirectReleaseProof::TargetDestroyed => {
                if self.submitted.is_some() {
                    return DirectReleaseOutcome::Deferred {
                        reason: DirectReleaseDeferral::SubmittedOwnership,
                    };
                }
                if worker_owns_current {
                    return DirectReleaseOutcome::Deferred {
                        reason: DirectReleaseDeferral::WorkerOwnership,
                    };
                }
                DirectReleaseOutcome::Released {
                    presented: self.presented.take().map(Box::new),
                    suspended: std::mem::take(&mut self.suspended),
                }
            }
        }
    }

    fn validate_submitted_pageflip(
        &self,
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
    ) -> Result<&SubmittedDirectPrimary, DirectOwnershipError> {
        let Some(submitted) = self.submitted.as_ref() else {
            return Err(DirectOwnershipError {
                error: io::Error::other("direct pageflip has no submitted ownership"),
            });
        };
        if submitted.transaction_id != transaction_id {
            return Err(DirectOwnershipError {
                error: io::Error::other(
                    "direct pageflip transaction does not match submitted ownership",
                ),
            });
        }
        if submitted.token != token {
            return Err(DirectOwnershipError {
                error: io::Error::other("direct pageflip token does not match submitted ownership"),
            });
        }
        Ok(submitted)
    }

    pub(crate) fn submitted_pageflip_info(
        &self,
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
    ) -> Result<DirectPageflipInfo, DirectOwnershipError> {
        Ok(self
            .validate_submitted_pageflip(transaction_id, token)?
            .pageflip_info())
    }

    pub(crate) fn accept_submitted(
        &mut self,
        submitted: SubmittedDirectPrimary,
    ) -> Result<(), Box<SubmittedDirectPrimaryError>> {
        if self.submitted.is_some() {
            return Err(Box::new(SubmittedDirectPrimaryError {
                error: io::Error::other("direct submitted ownership already exists"),
                submitted,
            }));
        }
        self.submitted = Some(submitted);
        Ok(())
    }

    pub(crate) fn complete_pageflip(
        &mut self,
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
        presented_at: MonotonicTimestampNs,
    ) -> Result<(&PresentedDirectPrimary, Option<PresentedDirectPrimary>), DirectOwnershipError>
    {
        self.validate_submitted_pageflip(transaction_id, token)?;
        let mut submitted = self
            .submitted
            .take()
            .expect("submitted ownership was validated above");
        submitted.out_fence.take();
        let presented = PresentedDirectPrimary {
            transaction_id: submitted.transaction_id,
            token: submitted.token,
            lease: submitted.lease,
            presented_at,
            frame_id: submitted.frame_id,
            protocol_batch_id: submitted.protocol_batch_id,
            target: submitted.target,
            submit_started_at: submitted.submit_started_at,
            submit_returned_at: submitted.submit_returned_at,
        };
        let replaced = self.presented.replace(presented);
        Ok((
            self.presented
                .as_ref()
                .expect("presented ownership was just installed"),
            replaced,
        ))
    }

    pub(crate) fn prepare_pageflip(
        &self,
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
        presented_at: MonotonicTimestampNs,
    ) -> Result<PreparedDirectPageflip, DirectOwnershipError> {
        let submitted = self.validate_submitted_pageflip(transaction_id, token)?;
        let surface_damage = submitted
            .lease
            .clone_surface_damage()
            .map_err(|error| DirectOwnershipError { error })?;
        Ok(PreparedDirectPageflip {
            transaction_id,
            token,
            presented_at,
            surface_damage,
        })
    }

    pub(crate) fn commit_prepared_pageflip(
        &mut self,
        prepared: PreparedDirectPageflip,
    ) -> (
        Option<PresentedDirectPrimary>,
        oblivion_one::compositor::SurfaceDamagePresentation,
    ) {
        let mut submitted = self
            .submitted
            .take()
            .expect("submitted ownership was validated during pageflip preparation");
        submitted.out_fence.take();
        let damage = submitted
            .lease
            .take_surface_damage()
            .expect("surface damage was validated during pageflip preparation");
        let presented = PresentedDirectPrimary {
            transaction_id: submitted.transaction_id,
            token: submitted.token,
            lease: submitted.lease,
            presented_at: prepared.presented_at,
            frame_id: submitted.frame_id,
            protocol_batch_id: submitted.protocol_batch_id,
            target: submitted.target,
            submit_started_at: submitted.submit_started_at,
            submit_returned_at: submitted.submit_returned_at,
        };
        let replaced = self.presented.replace(presented);
        debug_assert!(replaced.as_ref().is_none_or(|old| {
            old.transaction_id != prepared.transaction_id || old.token != prepared.token
        }));
        debug_assert_eq!(damage, prepared.surface_damage);
        (replaced, damage)
    }

    pub(crate) fn abandon_submitted_for_restore(
        &mut self,
        token: PageFlipToken,
    ) -> Result<(), DirectOwnershipError> {
        let Some(submitted) = self.submitted.as_ref() else {
            return Err(DirectOwnershipError {
                error: io::Error::other("direct restore has no submitted ownership"),
            });
        };
        if submitted.token != token {
            return Err(DirectOwnershipError {
                error: io::Error::other("direct restore token does not match submitted ownership"),
            });
        }
        let submitted = self
            .submitted
            .take()
            .expect("submitted ownership was validated above");
        drop(submitted.out_fence);
        self.suspended.push(submitted.lease);
        Ok(())
    }

    pub(crate) fn suspend_for_restore(&mut self) -> io::Result<()> {
        if let Some(token) = self.submitted.as_ref().map(|submitted| submitted.token) {
            self.abandon_submitted_for_restore(token)
                .map_err(|error| error.error)?;
        }
        if let Some(presented) = self.presented.take() {
            self.suspended.push(presented.lease);
        }
        Ok(())
    }

    pub(crate) fn disarm_drm_cleanup(&mut self) {
        if let Some(submitted) = self.submitted.as_ref() {
            submitted.lease.disarm_drm_cleanup();
        }
        if let Some(presented) = self.presented.as_ref() {
            presented.lease.disarm_drm_cleanup();
        }
        for lease in &self.suspended {
            lease.disarm_drm_cleanup();
        }
    }
}

impl SubmittedDirectPrimary {
    fn pageflip_info(&self) -> DirectPageflipInfo {
        DirectPageflipInfo {
            frame_id: self.frame_id,
            transaction_id: self.transaction_id,
            token: self.token,
            surface_id: self.lease.surface_id(),
            candidate_key: self.lease.key(),
            protocol_batch_id: self.protocol_batch_id,
            target: self.target,
            submit_started_at: self.submit_started_at,
            submit_returned_at: self.submit_returned_at,
        }
    }
}

impl PresentedDirectPrimary {
    fn pageflip_info(&self) -> DirectPageflipInfo {
        DirectPageflipInfo {
            frame_id: self.frame_id,
            transaction_id: self.transaction_id,
            token: self.token,
            surface_id: self.lease.surface_id(),
            candidate_key: self.lease.key(),
            protocol_batch_id: self.protocol_batch_id,
            target: self.target,
            submit_started_at: self.submit_started_at,
            submit_returned_at: self.submit_returned_at,
        }
    }
}

impl DirectScanoutControl {
    pub(crate) fn validate_composited_transition(
        &mut self,
        expected: ExpectedPresentedDirectPrimary,
        worker_owns_current: bool,
    ) -> Result<(), DirectReleaseViolation> {
        match self
            .ownership
            .validate_presented_direct(expected, worker_owns_current)
        {
            Ok(()) => Ok(()),
            Err(reason) => {
                self.counters.early_release_violations =
                    self.counters.early_release_violations.saturating_add(1);
                Err(DirectReleaseViolation::Retirement(reason))
            }
        }
    }

    pub(super) fn new(drm: std::os::fd::BorrowedFd<'_>, generation: u64) -> Self {
        Self {
            ownership: DirectPrimaryOwnership::default(),
            framebuffer_cache: DirectFramebufferCache::new(drm, generation),
            inhibit_until_composited_present: true,
            counters: DirectScanoutCounters::default(),
            drm_generation: generation,
            validation_cache: DirectPlaneValidationCache::default(),
            live_lease_count: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            identity_viewport_metadata_logged: false,
            last_debug_candidate: None,
        }
    }

    pub(crate) fn pending_token(&self) -> Option<PageFlipToken> {
        self.ownership.submitted.as_ref().map(|frame| frame.token)
    }

    pub(crate) fn page_flip_pending(&self) -> bool {
        self.ownership.submitted.is_some()
    }

    pub(crate) fn record_direct_validation_success(&mut self, key: DirectPlaneValidationKey) {
        self.validation_cache.record_success(key);
    }

    pub(crate) fn invalidate_direct_validation(&mut self, key: DirectPlaneValidationKey) {
        self.validation_cache.invalidate(key);
    }

    pub(crate) fn invalidate_direct_validation_cache(&mut self) {
        self.validation_cache.invalidate_all();
    }

    pub(crate) fn active_surface(&self) -> Option<u32> {
        self.ownership
            .submitted
            .as_ref()
            .map(|frame| frame.lease.surface_id())
            .or_else(|| {
                self.ownership
                    .presented
                    .as_ref()
                    .map(|frame| frame.lease.surface_id())
            })
    }

    pub(crate) fn disarm_drm_cleanup(&mut self) {
        self.framebuffer_cache.clear_disarmed();
        self.ownership.disarm_drm_cleanup();
    }

    pub(crate) fn complete_composited_transition(
        &mut self,
        expected: ExpectedPresentedDirectPrimary,
        worker_owns_current: bool,
    ) -> CompositedTransitionResult {
        match self
            .ownership
            .retire_presented_direct(expected, worker_owns_current)
        {
            PresentedDirectRetirement::Retired { lease } => {
                self.counters.exits = self.counters.exits.saturating_add(1);
                self.inhibit_until_composited_present = false;
                CompositedTransitionResult::Completed {
                    released: Some(Box::new(lease)),
                }
            }
            PresentedDirectRetirement::Mismatch {
                retained, reason, ..
            } => {
                self.ownership.presented = Some(*retained);
                self.counters.early_release_violations =
                    self.counters.early_release_violations.saturating_add(1);
                CompositedTransitionResult::Fatal {
                    reason: DirectReleaseViolation::Retirement(reason),
                }
            }
            PresentedDirectRetirement::Missing { .. } => {
                self.counters.early_release_violations =
                    self.counters.early_release_violations.saturating_add(1);
                CompositedTransitionResult::Fatal {
                    reason: DirectReleaseViolation::Retirement(
                        DirectRetirementMismatch::MissingOwnership,
                    ),
                }
            }
        }
    }

    pub(crate) fn request_direct_release(
        &mut self,
        proof: DirectReleaseProof,
        worker_owns_current: bool,
    ) -> DirectReleaseOutcome {
        let outcome = self
            .ownership
            .request_direct_release(proof, worker_owns_current);
        match outcome {
            DirectReleaseOutcome::Deferred { reason } => {
                self.counters.early_release_prevented =
                    self.counters.early_release_prevented.saturating_add(1);
                DirectReleaseOutcome::Deferred { reason }
            }
            DirectReleaseOutcome::Violation { reason } => {
                self.counters.early_release_violations =
                    self.counters.early_release_violations.saturating_add(1);
                DirectReleaseOutcome::Violation { reason }
            }
            DirectReleaseOutcome::Released {
                presented,
                suspended,
            } => DirectReleaseOutcome::Released {
                presented,
                suspended,
            },
        }
    }

    pub(super) fn complete_suspended(&mut self) -> DirectReleaseOutcome {
        self.request_direct_release(DirectReleaseProof::Restored, false)
    }

    pub(super) fn suspend(&mut self) -> io::Result<()> {
        self.ownership.suspend_for_restore()?;
        self.inhibit_until_composited_present = true;
        Ok(())
    }
}

#[cfg(test)]
#[path = "atomic_direct_tests.rs"]
mod tests;
