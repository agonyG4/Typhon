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
        released: Option<Box<PresentedDirectPrimary>>,
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
    pub(crate) callback_owner_leaks: u64,
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
        worker_owns_current: bool,
    ) -> CompositedTransitionResult {
        match self.request_direct_release(DirectReleaseProof::ComposedPageflip, worker_owns_current)
        {
            DirectReleaseOutcome::Released { presented, .. } => {
                let released = presented;
                if released.is_some() {
                    self.counters.exits = self.counters.exits.saturating_add(1);
                    self.counters.fallback_cycles = self.counters.fallback_cycles.saturating_add(1);
                }
                self.inhibit_until_composited_present = false;
                CompositedTransitionResult::Completed { released }
            }
            DirectReleaseOutcome::Deferred { .. } => CompositedTransitionResult::Fatal {
                reason: DirectReleaseViolation::Submitted,
            },
            DirectReleaseOutcome::Violation { reason } => {
                CompositedTransitionResult::Fatal { reason }
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
mod tests {
    use super::*;
    use crate::native_output::kms_worker::{
        KmsCommitJob, KmsCursorUpdate, KmsPrimaryUpdate, KmsTestOnlyPolicy,
    };
    use crate::native_output::runtime::AtomicCommitKind;
    use crate::native_output::scanout::DirectPrimaryLease;
    use oblivion_one::native::kms::FramebufferId;
    use std::os::fd::AsFd;
    use std::sync::atomic::Ordering;

    fn test_key() -> DirectScanoutCandidateKey {
        DirectScanoutCandidateKey {
            content: OutputContentKey::new(
                7,
                std::num::NonZeroU64::new(42).expect("test buffer ID"),
                ContentEpochId::new(std::num::NonZeroU64::new(3).expect("test content epoch")),
                1920,
                1080,
                0x3432_5241,
                0,
                0,
                1_000,
                0,
            ),
            output_generation: 1,
            cursor_content_key: None,
            color_epoch: 0,
        }
    }

    #[test]
    fn direct_content_classifier_prefers_confirmed_presented_content() {
        let candidate = test_key();

        assert_eq!(
            classify_direct_content(candidate, Some(candidate), Some(candidate)),
            DirectContentDisposition::MatchesPresented
        );
    }

    #[test]
    fn direct_content_classifier_treats_output_generation_change_as_new_content() {
        let presented = test_key();
        let candidate = DirectScanoutCandidateKey {
            output_generation: 2,
            ..presented
        };

        assert_eq!(
            classify_direct_content(candidate, Some(presented), None),
            DirectContentDisposition::NewContent
        );
    }

    fn test_target() -> PresentationTarget {
        let now = MonotonicTimestampNs::new(10);
        PresentationTarget {
            sequence: 2,
            presentation_time: now,
            submit_not_before: now,
            render_start_deadline: now,
            refresh_interval: std::time::Duration::from_millis(10),
            reason: PresentationTargetReason::ReactiveDouble,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
        }
    }

    fn test_submitted(token: u64, lease: DirectPrimaryLease) -> SubmittedDirectPrimary {
        SubmittedDirectPrimary {
            transaction_id: OutputTransactionId::new(
                std::num::NonZeroU64::new(token).expect("test transaction ID"),
            ),
            token: PageFlipToken::new(token).expect("test token"),
            lease,
            submit_started_at: MonotonicTimestampNs::new(11),
            submit_returned_at: MonotonicTimestampNs::new(12),
            out_fence: None,
            frame_id: token,
            protocol_batch_id: CompositorFrameBatchId::new(
                std::num::NonZeroU64::new(token).expect("test batch ID"),
            ),
            target: test_target(),
        }
    }

    fn presented_ownership_for_release_test() -> DirectPrimaryOwnership {
        let key = test_key();
        let (lease, _cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 43);
        let mut ownership = DirectPrimaryOwnership::default();
        ownership
            .accept_submitted(test_submitted(143, lease))
            .expect("accept direct resource");
        ownership
            .complete_pageflip(
                OutputTransactionId::new(std::num::NonZeroU64::new(143).unwrap()),
                PageFlipToken::new(143).unwrap(),
                MonotonicTimestampNs::new(14),
            )
            .expect("present direct resource");
        ownership
    }

    fn direct_control_for_transition_test() -> DirectScanoutControl {
        let drm = std::fs::File::open("/dev/null").expect("test DRM file");
        DirectScanoutControl::new(drm.as_fd(), 1)
    }

    #[test]
    fn queued_direct_release_is_deferred() {
        let mut ownership = DirectPrimaryOwnership::default();
        assert!(matches!(
            ownership.request_direct_release(DirectReleaseProof::Unproven, true),
            DirectReleaseOutcome::Deferred {
                reason: DirectReleaseDeferral::WorkerOwnership
            }
        ));
    }

    #[test]
    fn executing_direct_release_is_deferred() {
        let mut ownership = DirectPrimaryOwnership::default();
        assert!(matches!(
            ownership.request_direct_release(DirectReleaseProof::Unproven, true),
            DirectReleaseOutcome::Deferred {
                reason: DirectReleaseDeferral::WorkerOwnership
            }
        ));
    }

    #[test]
    fn submitted_event_release_is_deferred() {
        let mut ownership = DirectPrimaryOwnership::default();
        assert!(matches!(
            ownership.request_direct_release(DirectReleaseProof::Unproven, true),
            DirectReleaseOutcome::Deferred {
                reason: DirectReleaseDeferral::WorkerOwnership
            }
        ));
    }

    #[test]
    fn physical_submitted_release_is_deferred() {
        let key = test_key();
        let (lease, _cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 144);
        let mut ownership = DirectPrimaryOwnership::default();
        ownership
            .accept_submitted(test_submitted(144, lease))
            .expect("accept direct resource");

        assert!(matches!(
            ownership.request_direct_release(DirectReleaseProof::Unproven, false),
            DirectReleaseOutcome::Deferred {
                reason: DirectReleaseDeferral::SubmittedOwnership
            }
        ));
    }

    #[test]
    fn presented_direct_release_is_deferred_until_replacement() {
        let mut ownership = presented_ownership_for_release_test();

        assert!(matches!(
            ownership.request_direct_release(DirectReleaseProof::Unproven, false),
            DirectReleaseOutcome::Deferred {
                reason: DirectReleaseDeferral::UnprovenTeardown
            }
        ));
        assert!(matches!(
            ownership.request_direct_release(DirectReleaseProof::ComposedPageflip, false),
            DirectReleaseOutcome::Released { .. }
        ));
    }

    #[test]
    fn composed_assignment_is_published_only_after_direct_release() {
        let mut control = direct_control_for_transition_test();
        control.ownership = presented_ownership_for_release_test();
        let result = control.complete_composited_transition(false);

        assert!(matches!(
            result,
            CompositedTransitionResult::Completed { .. }
        ));
        assert!(control.ownership.presented.is_none());
    }

    #[test]
    fn deferred_direct_release_does_not_publish_composed_assignment() {
        let mut ownership = presented_ownership_for_release_test();
        let result = ownership.request_direct_release(DirectReleaseProof::Unproven, false);

        assert!(matches!(result, DirectReleaseOutcome::Deferred { .. }));
        assert!(ownership.presented.is_some());
    }

    #[test]
    fn direct_release_violation_does_not_publish_composed_assignment() {
        let mut control = direct_control_for_transition_test();
        control.ownership = presented_ownership_for_release_test();
        let result = control.complete_composited_transition(true);

        assert!(matches!(result, CompositedTransitionResult::Fatal { .. }));
        assert!(control.ownership.presented.is_some());
    }

    #[test]
    fn failed_composited_transition_retains_presented_direct_lease() {
        let mut control = direct_control_for_transition_test();
        control.ownership = presented_ownership_for_release_test();
        let result = control.complete_composited_transition(true);

        assert!(matches!(result, CompositedTransitionResult::Fatal { .. }));
        assert!(control.ownership.presented.is_some());
    }

    #[test]
    fn successful_composited_transition_releases_lease_once() {
        let mut control = direct_control_for_transition_test();
        control.ownership = presented_ownership_for_release_test();
        let result = control.complete_composited_transition(false);

        let CompositedTransitionResult::Completed { released } = result else {
            panic!("composited transition should release presented direct ownership")
        };
        drop(released);
        assert!(control.ownership.presented.is_none());
    }

    #[test]
    fn successful_composited_transition_clears_inhibition() {
        let mut control = direct_control_for_transition_test();
        control.ownership = presented_ownership_for_release_test();
        control.inhibit_until_composited_present = true;

        assert!(matches!(
            control.complete_composited_transition(false),
            CompositedTransitionResult::Completed { .. }
        ));
        assert!(!control.inhibit_until_composited_present);
    }

    #[test]
    fn successful_composited_transition_updates_exit_metrics_once() {
        let mut control = direct_control_for_transition_test();
        control.ownership = presented_ownership_for_release_test();

        let _ = control.complete_composited_transition(false);
        let _ = control.complete_composited_transition(false);

        assert_eq!(control.counters.exits, 1);
        assert_eq!(control.counters.fallback_cycles, 1);
    }

    #[test]
    fn failed_composited_transition_enters_quarantine() {
        let mut control = direct_control_for_transition_test();
        control.ownership = presented_ownership_for_release_test();

        assert!(matches!(
            control.complete_composited_transition(true),
            CompositedTransitionResult::Fatal { .. }
        ));
        assert!(control.ownership.presented.is_some());
        assert_eq!(control.counters.early_release_violations, 1);
        assert!(control.inhibit_until_composited_present);
    }

    #[test]
    fn composed_transition_owner_mismatch_increments_release_violation() {
        let mut control = direct_control_for_transition_test();
        control.ownership = presented_ownership_for_release_test();
        assert!(matches!(
            control.request_direct_release(DirectReleaseProof::ComposedPageflip, true),
            DirectReleaseOutcome::Violation { .. }
        ));
        assert!(control.ownership.presented.is_some());
        assert_eq!(control.counters.early_release_violations, 1);
    }

    #[test]
    fn submitted_owner_release_attempt_increments_release_violation() {
        let key = test_key();
        let (lease, _cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 145);
        let mut control = direct_control_for_transition_test();
        control
            .ownership
            .accept_submitted(test_submitted(145, lease))
            .expect("accept direct resource");

        assert!(matches!(
            control.request_direct_release(DirectReleaseProof::ComposedPageflip, false),
            DirectReleaseOutcome::Violation { .. }
        ));
        assert!(control.ownership.submitted.is_some());
        assert_eq!(control.counters.early_release_violations, 1);
    }

    #[test]
    fn unsafe_release_never_drops_lease() {
        let mut ownership = presented_ownership_for_release_test();
        let _ = ownership.request_direct_release(DirectReleaseProof::ComposedPageflip, true);

        assert!(ownership.presented.is_some());
    }

    #[test]
    fn unproven_teardown_release_is_deferred() {
        let mut control = direct_control_for_transition_test();
        control.ownership = presented_ownership_for_release_test();
        assert!(matches!(
            control.request_direct_release(DirectReleaseProof::Unproven, false),
            DirectReleaseOutcome::Deferred {
                reason: DirectReleaseDeferral::UnprovenTeardown
            }
        ));
        assert!(matches!(
            control.request_direct_release(DirectReleaseProof::Unproven, false),
            DirectReleaseOutcome::Deferred { .. }
        ));
        assert_eq!(control.counters.early_release_prevented, 2);
    }

    #[test]
    fn restored_release_is_safe() {
        let mut control = direct_control_for_transition_test();
        control.ownership = presented_ownership_for_release_test();
        assert!(matches!(
            control.request_direct_release(DirectReleaseProof::Restored, false),
            DirectReleaseOutcome::Released { .. }
        ));
        assert_eq!(control.counters.early_release_prevented, 0);
        assert_eq!(control.counters.early_release_violations, 0);
    }

    #[test]
    fn target_destroyed_release_is_safe() {
        let mut control = direct_control_for_transition_test();
        control.ownership = presented_ownership_for_release_test();
        assert!(matches!(
            control.request_direct_release(DirectReleaseProof::TargetDestroyed, false),
            DirectReleaseOutcome::Released { .. }
        ));
        assert_eq!(control.counters.early_release_prevented, 0);
        assert_eq!(control.counters.early_release_violations, 0);
    }

    #[test]
    fn worker_queue_owns_direct_resource_before_submit() {
        let key = test_key();
        let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
        let job = KmsCommitJob {
            transaction_id: OutputTransactionId::new(std::num::NonZeroU64::new(80).unwrap()),
            token: PageFlipToken::new(80).unwrap(),
            output_generation: 1,
            crtc_id: 7,
            kind: AtomicCommitKind::DirectPrimary {
                transaction_id: OutputTransactionId::new(std::num::NonZeroU64::new(80).unwrap()),
                direct_token: PageFlipToken::new(80).unwrap(),
                framebuffer_id: 42,
            },
            target: test_target(),
            queued_at: MonotonicTimestampNs::new(10),
            primary: KmsPrimaryUpdate::Framebuffer {
                framebuffer: FramebufferId::new(42).unwrap(),
                in_fence: None,
                request_out_fence: true,
            },
            cursor: KmsCursorUpdate::Unchanged,
            cursor_pin: None,
            direct_primary_lease: Some(lease),
            test_only_duration_ns: None,
            pacing_frame_id: None,
            test_only: KmsTestOnlyPolicy::Required,
            ready_submit: false,
        };
        let ownership = DirectPrimaryOwnership::default();

        assert!(job.direct_primary_lease.is_some());
        assert!(ownership.submitted.is_none());
        assert!(ownership.presented.is_none());
        drop(job);
        assert_eq!(cleanup_count.load(Ordering::Acquire), 1);
    }

    #[test]
    fn submitted_event_transfers_direct_resource_to_physical_ownership() {
        let key = test_key();
        let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
        let mut ownership = DirectPrimaryOwnership::default();
        let submitted = test_submitted(81, lease);

        ownership
            .accept_submitted(submitted)
            .expect("accept submitted direct resource");

        let stored = ownership.submitted.as_ref().expect("submitted ownership");
        assert_eq!(stored.transaction_id.get(), 81);
        assert_eq!(stored.token.get(), 81);
        assert_eq!(stored.lease.key(), key);
        assert_eq!(stored.lease.validation_key(), test_validation_key(1));
        assert_eq!(stored.lease.surface_id(), key.content.surface_id);
        assert_eq!(stored.lease.framebuffer_id(), 42);
        assert_eq!(stored.submit_started_at.get(), 11);
        assert_eq!(stored.submit_returned_at.get(), 12);
        assert!(stored.out_fence.is_none());
        assert!(ownership.presented.is_none());
        assert_eq!(cleanup_count.load(Ordering::Acquire), 0);
    }

    #[test]
    fn presented_direct_ownership_matches_confirmed_assignment() {
        let key = test_key();
        let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
        let mut ownership = DirectPrimaryOwnership::default();
        ownership
            .accept_submitted(test_submitted(82, lease))
            .expect("accept submitted direct resource");

        let (presented, replaced) = ownership
            .complete_pageflip(
                OutputTransactionId::new(std::num::NonZeroU64::new(82).unwrap()),
                PageFlipToken::new(82).unwrap(),
                MonotonicTimestampNs::new(13),
            )
            .expect("complete direct pageflip");

        assert_eq!(presented.transaction_id.get(), 82);
        assert_eq!(presented.token.get(), 82);
        assert_eq!(presented.lease.key(), key);
        assert_eq!(presented.lease.surface_id(), key.content.surface_id);
        assert_eq!(presented.lease.framebuffer_id(), 42);
        assert_eq!(presented.lease.key().content.content_epoch.get(), 3);
        assert_eq!(presented.presented_at.get(), 13);
        assert!(replaced.is_none());
        assert!(ownership.submitted.is_none());
        assert_eq!(
            ownership
                .presented
                .as_ref()
                .expect("presented ownership")
                .lease
                .framebuffer_id(),
            42
        );
        assert_eq!(cleanup_count.load(Ordering::Acquire), 0);
    }

    #[test]
    fn replacement_pageflip_releases_replaced_direct_lease() {
        let key = test_key();
        let (lease_a, cleanup_a) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
        let (lease_b, cleanup_b) = DirectPrimaryLease::test_fixture_with_probe(key, 43);
        let mut ownership = DirectPrimaryOwnership::default();
        ownership
            .accept_submitted(test_submitted(83, lease_a))
            .expect("accept first submitted resource");
        ownership
            .complete_pageflip(
                OutputTransactionId::new(std::num::NonZeroU64::new(83).unwrap()),
                PageFlipToken::new(83).unwrap(),
                MonotonicTimestampNs::new(13),
            )
            .expect("present first direct resource");
        ownership
            .accept_submitted(test_submitted(84, lease_b))
            .expect("accept replacement resource");

        assert_eq!(cleanup_a.load(Ordering::Acquire), 0);
        assert_eq!(cleanup_b.load(Ordering::Acquire), 0);
        let (_presented, replaced) = ownership
            .complete_pageflip(
                OutputTransactionId::new(std::num::NonZeroU64::new(84).unwrap()),
                PageFlipToken::new(84).unwrap(),
                MonotonicTimestampNs::new(14),
            )
            .expect("present replacement direct resource");
        assert_eq!(cleanup_a.load(Ordering::Acquire), 0);
        drop(replaced);
        assert_eq!(cleanup_a.load(Ordering::Acquire), 1);
        assert_eq!(cleanup_b.load(Ordering::Acquire), 0);
    }

    #[test]
    fn rejected_queued_direct_job_never_enters_submitted_ownership() {
        let key = test_key();
        let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
        let ownership = DirectPrimaryOwnership::default();
        assert!(ownership.submitted.is_none());
        assert!(ownership.presented.is_none());
        drop(lease);
        assert_eq!(cleanup_count.load(Ordering::Acquire), 1);
    }

    #[test]
    fn exact_token_rejection_preserves_submitted_ownership() {
        let key = test_key();
        let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
        let mut ownership = DirectPrimaryOwnership::default();
        ownership
            .accept_submitted(test_submitted(85, lease))
            .expect("accept submitted direct resource");

        let error = ownership
            .complete_pageflip(
                OutputTransactionId::new(std::num::NonZeroU64::new(85).unwrap()),
                PageFlipToken::new(86).unwrap(),
                MonotonicTimestampNs::new(13),
            )
            .expect_err("wrong token must reject");

        assert!(error.error.to_string().contains("token"));
        assert_eq!(ownership.submitted.as_ref().unwrap().token.get(), 85);
        assert!(ownership.presented.is_none());
        assert_eq!(cleanup_count.load(Ordering::Acquire), 0);
    }

    #[test]
    fn exact_transaction_rejection_preserves_submitted_ownership() {
        let key = test_key();
        let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 42);
        let mut ownership = DirectPrimaryOwnership::default();
        ownership
            .accept_submitted(test_submitted(86, lease))
            .expect("accept submitted direct resource");

        let error = ownership
            .complete_pageflip(
                OutputTransactionId::new(std::num::NonZeroU64::new(87).unwrap()),
                PageFlipToken::new(86).unwrap(),
                MonotonicTimestampNs::new(13),
            )
            .expect_err("wrong transaction must reject");

        assert!(error.error.to_string().contains("transaction"));
        assert_eq!(
            ownership.submitted.as_ref().unwrap().transaction_id.get(),
            86
        );
        assert!(ownership.presented.is_none());
        assert_eq!(cleanup_count.load(Ordering::Acquire), 0);
    }

    #[test]
    fn direct_pageflip_physical_prepare_failure_preserves_submitted_ownership() {
        let key = test_key();
        let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 87);
        let mut ownership = DirectPrimaryOwnership::default();
        ownership
            .accept_submitted(test_submitted(87, lease))
            .expect("accept submitted direct resource");

        let error = ownership
            .prepare_pageflip(
                OutputTransactionId::new(std::num::NonZeroU64::new(88).unwrap()),
                PageFlipToken::new(87).unwrap(),
                MonotonicTimestampNs::new(13),
            )
            .expect_err("physical preparation must reject a wrong transaction");

        assert!(error.error.to_string().contains("transaction"));
        assert!(ownership.submitted.is_some());
        assert!(ownership.presented.is_none());
        assert_eq!(cleanup_count.load(Ordering::Acquire), 0);
    }

    #[test]
    fn direct_pageflip_prepare_rejects_missing_surface_damage() {
        let key = test_key();
        let (lease, cleanup_count) =
            DirectPrimaryLease::test_fixture_with_probe_and_damage(key, 87, None);
        let mut ownership = DirectPrimaryOwnership::default();
        ownership
            .accept_submitted(test_submitted(87, lease))
            .expect("accept submitted direct resource");

        assert!(
            ownership
                .prepare_pageflip(
                    OutputTransactionId::new(std::num::NonZeroU64::new(87).unwrap()),
                    PageFlipToken::new(87).unwrap(),
                    MonotonicTimestampNs::new(13),
                )
                .is_err()
        );
        assert!(ownership.submitted.is_some());
        assert!(ownership.presented.is_none());
        assert_eq!(cleanup_count.load(Ordering::Acquire), 0);
    }

    #[test]
    fn restore_moves_submitted_direct_resource_to_suspended_ownership() {
        let key = test_key();
        let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 88);
        let mut ownership = DirectPrimaryOwnership::default();
        ownership
            .accept_submitted(test_submitted(88, lease))
            .expect("accept submitted direct resource");

        ownership
            .abandon_submitted_for_restore(PageFlipToken::new(88).unwrap())
            .expect("move submitted resource to restore ownership");

        assert!(ownership.submitted.is_none());
        assert!(ownership.presented.is_none());
        assert_eq!(ownership.suspended.len(), 1);
        assert_eq!(cleanup_count.load(Ordering::Acquire), 0);
        let DirectReleaseOutcome::Released {
            presented,
            suspended,
        } = ownership.request_direct_release(DirectReleaseProof::Restored, false)
        else {
            panic!("restored ownership should be releasable")
        };
        drop(presented);
        drop(suspended);
        assert_eq!(cleanup_count.load(Ordering::Acquire), 1);
    }

    #[test]
    fn second_submitted_direct_resource_is_returned_without_mutating_first() {
        let key = test_key();
        let (lease_a, cleanup_a) = DirectPrimaryLease::test_fixture_with_probe(key, 89);
        let (lease_b, cleanup_b) = DirectPrimaryLease::test_fixture_with_probe(key, 90);
        let mut ownership = DirectPrimaryOwnership::default();
        ownership
            .accept_submitted(test_submitted(89, lease_a))
            .expect("accept first submitted resource");

        let error = ownership
            .accept_submitted(test_submitted(90, lease_b))
            .expect_err("second submitted resource must be rejected");
        assert!(error.error.to_string().contains("already exists"));
        assert_eq!(ownership.submitted.as_ref().unwrap().token.get(), 89);
        assert_eq!(cleanup_a.load(Ordering::Acquire), 0);
        drop(error);
        assert_eq!(cleanup_b.load(Ordering::Acquire), 1);
        ownership
            .abandon_submitted_for_restore(PageFlipToken::new(89).unwrap())
            .expect("move first submitted resource to restore ownership");
        let DirectReleaseOutcome::Released {
            presented,
            suspended,
        } = ownership.request_direct_release(DirectReleaseProof::Restored, false)
        else {
            panic!("restored ownership should be releasable")
        };
        drop(presented);
        drop(suspended);
        assert_eq!(cleanup_a.load(Ordering::Acquire), 1);
    }

    #[test]
    fn direct_to_composed_releases_direct_resource_after_composed_pageflip() {
        let key = test_key();
        let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 91);
        let mut ownership = DirectPrimaryOwnership::default();
        ownership
            .accept_submitted(test_submitted(91, lease))
            .expect("accept direct resource");
        ownership
            .complete_pageflip(
                OutputTransactionId::new(std::num::NonZeroU64::new(91).unwrap()),
                PageFlipToken::new(91).unwrap(),
                MonotonicTimestampNs::new(15),
            )
            .expect("present direct resource");

        assert_eq!(cleanup_count.load(Ordering::Acquire), 0);
        let DirectReleaseOutcome::Released {
            presented: Some(released),
            suspended,
        } = ownership.request_direct_release(DirectReleaseProof::ComposedPageflip, false)
        else {
            panic!("release old direct resource after composed pageflip");
        };
        assert!(suspended.is_empty());
        assert_eq!(released.token.get(), 91);
        assert!(ownership.presented.is_none());
        drop(released);
        assert_eq!(cleanup_count.load(Ordering::Acquire), 1);
    }

    #[test]
    fn legacy_direct_state_is_not_required_for_session_recovery() {
        let key = test_key();
        let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 92);
        let mut ownership = DirectPrimaryOwnership::default();
        ownership
            .accept_submitted(test_submitted(92, lease))
            .expect("accept direct resource");

        ownership
            .suspend_for_restore()
            .expect("suspend direct ownership");
        assert!(ownership.submitted.is_none());
        assert!(ownership.presented.is_none());
        assert_eq!(ownership.suspended.len(), 1);
        let DirectReleaseOutcome::Released {
            presented,
            suspended,
        } = ownership.request_direct_release(DirectReleaseProof::Restored, false)
        else {
            panic!("restored ownership should be releasable")
        };
        drop(presented);
        drop(suspended);
        assert_eq!(cleanup_count.load(Ordering::Acquire), 1);
    }

    #[test]
    fn direct_physical_ownership_rejects_second_submission() {
        let key = test_key();
        let (lease, cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 92);
        let mut ownership = DirectPrimaryOwnership::default();
        ownership
            .accept_submitted(test_submitted(92, lease))
            .expect("accept submitted direct resource");

        let (second_lease, second_cleanup) = DirectPrimaryLease::test_fixture_with_probe(key, 93);
        let error = ownership
            .accept_submitted(test_submitted(93, second_lease))
            .expect_err("a second direct physical owner must be rejected");

        assert!(error.error.to_string().contains("already exists"));
        assert!(ownership.submitted.is_some());
        drop(error);
        assert_eq!(cleanup_count.load(Ordering::Acquire), 0);
        assert_eq!(second_cleanup.load(Ordering::Acquire), 1);
    }
}
