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

#[derive(Debug)]
pub(crate) struct DirectPageflipCompletion {
    pub(crate) frame_id: u64,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) token: PageFlipToken,
    pub(crate) surface_id: u32,
    pub(crate) candidate_key: DirectScanoutCandidateKey,
    pub(crate) protocol_batch_id: CompositorFrameBatchId,
    pub(crate) target: PresentationTarget,
    pub(crate) presented_at: MonotonicTimestampNs,
    pub(crate) submit_started_at: MonotonicTimestampNs,
    pub(crate) submit_returned_at: MonotonicTimestampNs,
}

#[derive(Debug)]
pub(crate) enum DirectScanoutAttempt {
    Rejected(DirectScanoutSceneRejection),
    Fallback(&'static str),
    Unchanged,
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
    pub(crate) duplicate_feedback: u64,
    pub(crate) duplicate_settlement: u64,
    pub(crate) early_release_prevented: u64,
    pub(crate) worker_queue_overflow: u64,
    pub(crate) callback_owner_leaks: u64,
    pub(crate) first_blocker: Option<&'static str>,
    pub(crate) blocker_set: u64,
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
        direct_cursor_plan_key(cursor, true),
        0,
    )
}

pub(super) fn direct_scanout_debug(message: impl std::fmt::Display) {
    if std::env::var("TYPHON_DIRECT_SCANOUT_DEBUG").ok().as_deref() == Some("1") {
        eprintln!("direct scanout: {message}");
    }
}

impl DirectPrimaryOwnership {
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

    pub(crate) fn take_submitted_surface_damage(
        &mut self,
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
    ) -> Result<oblivion_one::compositor::SurfaceDamagePresentation, DirectOwnershipError> {
        self.validate_submitted_pageflip(transaction_id, token)?;
        self.submitted
            .as_mut()
            .expect("submitted ownership was validated above")
            .lease
            .take_surface_damage()
            .map_err(|error| DirectOwnershipError { error })
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
        let submitted = self
            .submitted
            .take()
            .expect("submitted ownership was validated above");
        drop(submitted.out_fence);
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

    pub(crate) fn release_presented_for_composed_pageflip(
        &mut self,
    ) -> Option<PresentedDirectPrimary> {
        self.presented.take()
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

    pub(crate) fn clear_after_restore(&mut self) {
        self.submitted.take();
        self.presented.take();
        self.suspended.clear();
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

    pub(super) fn complete_suspended(&mut self) {
        self.ownership.clear_after_restore();
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
            cursor_plan_key: None,
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
    fn pageflip_promotes_submitted_direct_resource_to_presented() {
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
    fn replacement_pageflip_releases_previous_presented_resource() {
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
        ownership.clear_after_restore();
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
        ownership.clear_after_restore();
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
        let released = ownership
            .release_presented_for_composed_pageflip()
            .expect("release old direct resource after composed pageflip");
        assert_eq!(released.token.get(), 91);
        assert!(ownership.presented.is_none());
        drop(released);
        assert_eq!(cleanup_count.load(Ordering::Acquire), 1);
    }
}
