use super::*;
use crate::native_output::kms_worker::{
    KmsBundleOwners, KmsCommitJob, KmsCursorUpdate, KmsPrimaryCursorPresentation, KmsPrimaryUpdate,
    KmsTestOnlyPolicy, KmsValidationBase,
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
        physical_claim: oblivion_one::native::presentation_deadline::PrimaryRefreshClaim {
            sequence: 2,
            presentation_time: now,
            clock_generation: 1,
        },
        selection_evidence: Default::default(),
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

fn expected_presented_identity_for_release_test() -> ExpectedPresentedDirectPrimary {
    let key = test_key();
    ExpectedPresentedDirectPrimary {
        transaction_id: OutputTransactionId::new(std::num::NonZeroU64::new(143).unwrap()),
        token: PageFlipToken::new(143).unwrap(),
        surface_id: key.content.surface_id,
        candidate_key: key,
        framebuffer_id: 43,
    }
}

#[test]
fn exact_presented_direct_identity_retires_successfully() {
    let mut ownership = presented_ownership_for_release_test();

    let PresentedDirectRetirement::Retired { lease } =
        ownership.retire_presented_direct(expected_presented_identity_for_release_test(), false)
    else {
        panic!("exact presented direct identity should retire");
    };
    assert_eq!(lease.framebuffer_id(), 43);
    assert!(ownership.presented.is_none());
}

#[test]
fn presented_direct_identity_validation_uses_physical_ownership() {
    let ownership = presented_ownership_for_release_test();
    assert_eq!(
        ownership.validate_presented_identity(expected_presented_identity_for_release_test()),
        Ok(())
    );
    assert_eq!(
        ownership.validate_presented_identity(ExpectedPresentedDirectPrimary {
            framebuffer_id: 44,
            ..expected_presented_identity_for_release_test()
        }),
        Err(DirectRetirementMismatch::FramebufferId)
    );
}

#[test]
fn presented_direct_identity_validation_rejects_submitted_or_suspended_only_ownership() {
    let key = test_key();
    let (submitted_lease, _cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 43);
    let mut submitted_only = DirectPrimaryOwnership::default();
    submitted_only
        .accept_submitted(test_submitted(143, submitted_lease))
        .expect("submitted direct resource");
    assert_eq!(
        submitted_only.validate_presented_identity(expected_presented_identity_for_release_test()),
        Err(DirectRetirementMismatch::MissingOwnership)
    );

    let (suspended_lease, _cleanup_count) = DirectPrimaryLease::test_fixture_with_probe(key, 43);
    let mut suspended_only = DirectPrimaryOwnership::default();
    suspended_only.suspended.push(suspended_lease);
    assert_eq!(
        suspended_only.validate_presented_identity(expected_presented_identity_for_release_test()),
        Err(DirectRetirementMismatch::MissingOwnership)
    );
}

fn assert_retirement_mismatch(
    expected: ExpectedPresentedDirectPrimary,
    reason: DirectRetirementMismatch,
) {
    let mut ownership = presented_ownership_for_release_test();
    let PresentedDirectRetirement::Mismatch {
        retained,
        reason: actual,
        ..
    } = ownership.retire_presented_direct(expected, false)
    else {
        panic!("presented identity should be rejected");
    };
    assert_eq!(actual, reason);
    assert_eq!(retained.lease.framebuffer_id(), 43);
    ownership.presented = Some(*retained);
    assert!(ownership.presented.is_some());
}

#[test]
fn missing_presented_ownership_does_not_publish_composed_assignment() {
    let mut ownership = DirectPrimaryOwnership::default();
    assert!(matches!(
        ownership.retire_presented_direct(expected_presented_identity_for_release_test(), false,),
        PresentedDirectRetirement::Missing { .. }
    ));
}

#[test]
fn transaction_mismatch_does_not_retire_presented_lease() {
    let expected = ExpectedPresentedDirectPrimary {
        transaction_id: OutputTransactionId::new(std::num::NonZeroU64::new(144).unwrap()),
        ..expected_presented_identity_for_release_test()
    };
    assert_retirement_mismatch(expected, DirectRetirementMismatch::TransactionId);
}

#[test]
fn pageflip_token_mismatch_does_not_retire_presented_lease() {
    let expected = ExpectedPresentedDirectPrimary {
        token: PageFlipToken::new(144).unwrap(),
        ..expected_presented_identity_for_release_test()
    };
    assert_retirement_mismatch(expected, DirectRetirementMismatch::PageflipToken);
}

#[test]
fn candidate_key_mismatch_does_not_retire_presented_lease() {
    let key = expected_presented_identity_for_release_test().candidate_key;
    let expected = ExpectedPresentedDirectPrimary {
        candidate_key: DirectScanoutCandidateKey {
            content: OutputContentKey {
                content_epoch: ContentEpochId::new(std::num::NonZeroU64::new(4).unwrap()),
                ..key.content
            },
            ..key
        },
        ..expected_presented_identity_for_release_test()
    };
    assert_retirement_mismatch(expected, DirectRetirementMismatch::CandidateKey);
}

#[test]
fn surface_identity_mismatch_does_not_retire_presented_lease() {
    let expected = ExpectedPresentedDirectPrimary {
        surface_id: 8,
        ..expected_presented_identity_for_release_test()
    };
    assert_retirement_mismatch(expected, DirectRetirementMismatch::SurfaceId);
}

#[test]
fn framebuffer_identity_mismatch_does_not_retire_presented_lease() {
    let expected = ExpectedPresentedDirectPrimary {
        framebuffer_id: 44,
        ..expected_presented_identity_for_release_test()
    };
    assert_retirement_mismatch(expected, DirectRetirementMismatch::FramebufferId);
}

#[test]
fn content_epoch_mismatch_does_not_retire_presented_lease() {
    let key = expected_presented_identity_for_release_test().candidate_key;
    let expected = ExpectedPresentedDirectPrimary {
        candidate_key: DirectScanoutCandidateKey {
            content: OutputContentKey {
                content_epoch: ContentEpochId::new(std::num::NonZeroU64::new(4).unwrap()),
                ..key.content
            },
            ..key
        },
        ..expected_presented_identity_for_release_test()
    };
    assert_retirement_mismatch(expected, DirectRetirementMismatch::CandidateKey);
}

fn direct_control_for_transition_test() -> DirectScanoutControl {
    let drm = std::fs::File::open("/dev/null").expect("test DRM file");
    DirectScanoutControl::new(drm.as_fd(), 1)
}

#[test]
fn retirement_mismatch_enters_fatal_quarantine() {
    let mut control = direct_control_for_transition_test();
    control.ownership = presented_ownership_for_release_test();
    let expected = ExpectedPresentedDirectPrimary {
        framebuffer_id: 44,
        ..expected_presented_identity_for_release_test()
    };

    assert!(matches!(
        control.complete_composited_transition(expected, false),
        CompositedTransitionResult::Fatal {
            reason: DirectReleaseViolation::Retirement(DirectRetirementMismatch::FramebufferId)
        }
    ));
    assert!(control.ownership.presented.is_some());
    assert_eq!(control.counters.early_release_violations, 1);
}

#[test]
fn successful_retirement_releases_exact_lease_once() {
    let mut control = direct_control_for_transition_test();
    control.ownership = presented_ownership_for_release_test();

    let CompositedTransitionResult::Completed { released } = control
        .complete_composited_transition(expected_presented_identity_for_release_test(), false)
    else {
        panic!("exact presented direct identity should retire");
    };
    assert!(released.is_some());
    assert!(control.ownership.presented.is_none());
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
    let result = control
        .complete_composited_transition(expected_presented_identity_for_release_test(), false);

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
    let result = control
        .complete_composited_transition(expected_presented_identity_for_release_test(), true);

    assert!(matches!(result, CompositedTransitionResult::Fatal { .. }));
    assert!(control.ownership.presented.is_some());
}

#[test]
fn failed_composited_transition_retains_presented_direct_lease() {
    let mut control = direct_control_for_transition_test();
    control.ownership = presented_ownership_for_release_test();
    let result = control
        .complete_composited_transition(expected_presented_identity_for_release_test(), true);

    assert!(matches!(result, CompositedTransitionResult::Fatal { .. }));
    assert!(control.ownership.presented.is_some());
}

#[test]
fn successful_composited_transition_releases_lease_once() {
    let mut control = direct_control_for_transition_test();
    control.ownership = presented_ownership_for_release_test();
    let result = control
        .complete_composited_transition(expected_presented_identity_for_release_test(), false);

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
        control
            .complete_composited_transition(expected_presented_identity_for_release_test(), false,),
        CompositedTransitionResult::Completed { .. }
    ));
    assert!(!control.inhibit_until_composited_present);
}

#[test]
fn successful_composited_transition_updates_exit_metrics_once() {
    let mut control = direct_control_for_transition_test();
    control.ownership = presented_ownership_for_release_test();

    let expected = expected_presented_identity_for_release_test();
    let _ = control.complete_composited_transition(expected, false);
    let _ = control.complete_composited_transition(expected, false);

    assert_eq!(control.counters.exits, 1);
    assert_eq!(control.counters.fallback_cycles, 0);
    assert_eq!(control.counters.fallback_cycles_last, 0);
    assert_eq!(control.counters.fallback_cycles_max, 0);
}

#[test]
fn failed_composited_transition_enters_quarantine() {
    let mut control = direct_control_for_transition_test();
    control.ownership = presented_ownership_for_release_test();

    assert!(matches!(
        control
            .complete_composited_transition(expected_presented_identity_for_release_test(), true,),
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
        bundle_id:
            crate::native_output::presentation::plane::KmsCommitBundleId::from_pageflip_token(
                PageFlipToken::new(80).unwrap(),
            ),
        owners: KmsBundleOwners::legacy_unchecked(),
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
        submit_window: crate::native_output::presentation::kms_timing::KmsSubmitWindow::try_new(
            0, 0, 0, 0,
        )
        .unwrap(),
        validation_base: KmsValidationBase::Presented {
            snapshot: crate::native_output::presentation::plane::PresentedPlaneSnapshot::legacy(
                None,
            ),
            output_generation: 1,
            crtc_id: 7,
        },
        queued_at: MonotonicTimestampNs::new(10),
        primary: KmsPrimaryUpdate::Framebuffer {
            framebuffer: FramebufferId::new(42).unwrap(),
            in_fence: None,
            request_out_fence: true,
        },
        cursor: KmsCursorUpdate::Unchanged,
        cursor_delivery: crate::native_output::presentation::plane::PresentedCursorDelivery::Hidden,
        primary_cursor_presentation: KmsPrimaryCursorPresentation::Preserve,
        cursor_pin: None,
        direct_primary_lease: Some(lease),
        test_only_duration_ns: None,
        pacing_frame_id: None,
        test_policy: crate::native_output::kms_worker::KmsCommitTestPolicy::from_primary(
            KmsTestOnlyPolicy::Required,
        ),
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
