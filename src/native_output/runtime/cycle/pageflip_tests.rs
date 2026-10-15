use super::*;
use crate::native_output::kms_worker::{
    KmsBundleOwners, KmsCommitJob, KmsCommitTestPolicy, KmsCommitWorkerHandle,
    KmsPrimaryCursorPresentation, KmsPrimaryUpdate, KmsTestOnlyPolicy, KmsValidationBase,
    KmsWorkerDispatchModel, KmsWorkerDispatchTailObservation,
};
use crate::native_output::pacing::{ContentCadenceAttribution, classify_content_frame};
use crate::native_output::presentation::kms_timing::KmsPresentationOutcome;
use crate::native_output::presentation::kms_timing::KmsPresentationTimingObservation;
use crate::native_output::presentation::plane::{
    CursorCoupling, CursorPlanePoint, CursorRevision, KmsCommitBundleId, PresentedCursorDelivery,
    PresentedCursorState, PresentedPlaneSnapshot,
};
use oblivion_one::native::adaptive_buffering::{
    AdaptiveRenderJournal, EstimatorRecoveryDisposition, FenceTimestampQuality,
    FrameTimingObservation, ProvenDeadlineMiss,
};
use oblivion_one::native::kms::FramebufferId;
use oblivion_one::native::presentation_deadline::{
    PresentationDeadlinePlanner, TargetSelectionEvidence,
};
use oblivion_one::native::presentation_timing::PresentationTiming;
use std::num::NonZeroU64;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;

struct SwapchainSuspendedRecovery<'a> {
    swapchain: &'a mut AtomicOutputSwapchain,
}

impl SuspendedOutputRecoveryBackend for SwapchainSuspendedRecovery<'_> {
    fn has_suspended_output_ownership(&self) -> NativeResult<bool> {
        Ok(self.swapchain.has_suspended_ownership())
    }

    fn recover_suspended_ready_if_signaled(&mut self) -> NativeResult<bool> {
        Ok(self.swapchain.recover_suspended_frames_if_signaled(drop)?)
    }

    fn suspended_fence_fd(&self) -> NativeResult<Option<std::os::fd::RawFd>> {
        Ok(self.swapchain.suspended_fence_fd()?)
    }
}

impl DirectEntryOwnershipBackend for SwapchainSuspendedRecovery<'_> {
    fn swapchain_mut(&mut self) -> std::io::Result<&mut AtomicOutputSwapchain> {
        Ok(self.swapchain)
    }
}

struct SuspendedWithoutReadinessFd;

impl SuspendedOutputRecoveryBackend for SuspendedWithoutReadinessFd {
    fn has_suspended_output_ownership(&self) -> NativeResult<bool> {
        Ok(true)
    }

    fn recover_suspended_ready_if_signaled(&mut self) -> NativeResult<bool> {
        Ok(false)
    }

    fn suspended_fence_fd(&self) -> NativeResult<Option<std::os::fd::RawFd>> {
        Ok(None)
    }
}

fn pageflip_test_controllable_render_fence() -> (
    crate::egl_renderer::native_fence::NativeRenderFence,
    OwnedFd,
) {
    let event = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    assert!(event >= 0);
    let signal = unsafe { OwnedFd::from_raw_fd(event) };
    let submission = unsafe { libc::fcntl(signal.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    assert!(submission >= 0);
    (
        crate::egl_renderer::native_fence::NativeRenderFence::from_submission_fd(unsafe {
            OwnedFd::from_raw_fd(submission)
        }),
        signal,
    )
}

fn signal_pageflip_test_fence(signal: &OwnedFd) {
    let value = 1u64.to_ne_bytes();
    assert_eq!(
        unsafe { libc::write(signal.as_raw_fd(), value.as_ptr().cast(), value.len()) },
        value.len() as isize
    );
}

#[test]
fn suspended_output_recovery_requires_a_wake_owner_and_preserves_an_existing_token() {
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut output_render_fence_token = None;
    let error = recover_or_arm_suspended_output_ownership(
        &mut SuspendedWithoutReadinessFd,
        &mut event_loop,
        &mut output_render_fence_token,
    )
    .expect_err("suspended ownership with no fence and no token is invalid");
    assert!(
        error
            .to_string()
            .contains("suspended output ownership has no readiness fence to wait for")
    );

    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    assert!(fd >= 0);
    let owned_fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let existing = event_loop
        .register(fd, NativeEventSource::OutputRenderFence)
        .unwrap();
    output_render_fence_token = Some(existing);
    let waiting = recover_or_arm_suspended_output_ownership(
        &mut SuspendedWithoutReadinessFd,
        &mut event_loop,
        &mut output_render_fence_token,
    )
    .unwrap();
    assert_eq!(waiting, SuspendedOutputRecovery::Waiting);
    assert_eq!(output_render_fence_token, Some(existing));
    assert_eq!(
        event_loop.source_for_token(existing),
        Some(NativeEventSource::OutputRenderFence)
    );
    event_loop.unregister(existing).unwrap();
    drop(owned_fd);
}

fn pageflip_timestamp(
    presented_at: MonotonicTimestampNs,
    uncertainty_ns: u64,
) -> PageflipTimestampEstimate {
    PageflipTimestampEstimate {
        presented_at,
        uncertainty: PageflipTimestampUncertainty::BoundedNs(uncertainty_ns),
    }
}

fn variable_reservation(
    opportunity_id: oblivion_one::native::buffering::PresentationOpportunityId,
    anchor: MonotonicTimestampNs,
    earliest_present: MonotonicTimestampNs,
    presentation_not_before: Option<MonotonicTimestampNs>,
) -> crate::native_output::presentation::transaction::VariablePresentationReservation {
    let opportunity = oblivion_one::native::buffering::PresentationOpportunity::new(
        opportunity_id,
        PresentationTiming::VrrWindow {
            anchor,
            earliest_present,
            presentation_not_before,
            preferred_present: presentation_not_before.unwrap_or(earliest_present),
            service_deadline: None,
            shortest_interval: std::time::Duration::from_nanos(6_060_606),
            longest_interval: None,
        },
    );
    crate::native_output::presentation::transaction::VariablePresentationReservation::try_new(
        opportunity,
        PresentationTargetReason::ReactiveDouble,
    )
    .expect("VRR opportunity creates variable reservation")
}

#[test]
fn variable_pageflip_rejects_physical_time_before_its_reserved_lower_bound() {
    let anchor = MonotonicTimestampNs::new(1_000_000_000);
    let mut deadline = PresentationDeadlinePlanner::new(std::time::Duration::from_nanos(6_060_606));
    deadline.note_presented(anchor);
    let opportunity_id = deadline
        .next_variable_opportunity_id()
        .expect("next sequence is available");
    let earliest_present = MonotonicTimestampNs::new(anchor.get() + 6_060_606);
    let presentation_not_before = MonotonicTimestampNs::new(anchor.get() + 8_000_000);
    let reservation = variable_reservation(
        opportunity_id,
        anchor,
        earliest_present,
        Some(presentation_not_before),
    );
    let materially_early = MonotonicTimestampNs::new(anchor.get() + 7_000_000);

    let result = prepare_physical_presentation_for_reservation(
        &deadline,
        crate::native_output::presentation::transaction::FramePresentationReservation::Variable(
            reservation,
        ),
        pageflip_timestamp(materially_early, 2_999),
    );

    assert!(
        result.is_err(),
        "the client not-before bound must be enforced for physical pageflips"
    );
}

#[test]
fn variable_pageflip_accepts_a_timestamp_at_its_lower_bound() {
    let anchor = MonotonicTimestampNs::new(1_000_000_000);
    let lower_bound = MonotonicTimestampNs::new(anchor.get() + 8_000_000);
    let reservation = variable_reservation(
        oblivion_one::native::buffering::PresentationOpportunityId::new(1, 1),
        anchor,
        MonotonicTimestampNs::new(anchor.get() + 6_000_000),
        Some(lower_bound),
    );

    assert_eq!(
        validate_variable_physical_time(
            crate::native_output::presentation::transaction::FramePresentationReservation::Variable(
                reservation,
            ),
            pageflip_timestamp(lower_bound, 0),
        ),
        Ok(())
    );
}

#[test]
fn variable_pageflip_accepts_early_estimate_inside_derived_timestamp_uncertainty() {
    let anchor = MonotonicTimestampNs::new(1_000_000_000);
    let lower_bound = MonotonicTimestampNs::new(anchor.get() + 8_000_000);
    let reservation = variable_reservation(
        oblivion_one::native::buffering::PresentationOpportunityId::new(1, 1),
        anchor,
        lower_bound,
        None,
    );
    let estimate = PageflipTimestampEstimate {
        presented_at: MonotonicTimestampNs::new(lower_bound.get() - 1_000),
        uncertainty: PageflipTimestampUncertainty::BoundedNs(2_999),
    };

    assert_eq!(
        validate_variable_physical_time(
            crate::native_output::presentation::transaction::FramePresentationReservation::Variable(
                reservation,
            ),
            estimate,
        ),
        Ok(())
    );
}

#[test]
fn variable_pageflip_lower_bound_uses_later_presentation_not_before() {
    let anchor = MonotonicTimestampNs::new(1_000_000_000);
    let earliest_present = MonotonicTimestampNs::new(anchor.get() + 6_060_606);
    let presentation_not_before = MonotonicTimestampNs::new(anchor.get() + 8_000_000);
    let reservation = variable_reservation(
        oblivion_one::native::buffering::PresentationOpportunityId::new(1, 1),
        anchor,
        earliest_present,
        Some(presentation_not_before),
    );
    let frame_reservation =
        crate::native_output::presentation::transaction::FramePresentationReservation::Variable(
            reservation,
        );

    assert_eq!(
        variable_presentation_lower_bound(frame_reservation),
        Some(presentation_not_before)
    );
    assert!(matches!(
        validate_variable_physical_time(
            frame_reservation,
            pageflip_timestamp(earliest_present, 0),
        ),
        Err(VariablePhysicalTimeViolation::EarlierThanLowerBound {
            required_lower_bound_ns,
            ..
        }) if required_lower_bound_ns == presentation_not_before.get()
    ));
}

#[test]
fn variable_pageflip_uses_earliest_as_lower_bound_when_not_before_is_absent() {
    let anchor = MonotonicTimestampNs::new(1_000_000_000);
    let earliest_present = MonotonicTimestampNs::new(anchor.get() + 6_060_606);
    let reservation = variable_reservation(
        oblivion_one::native::buffering::PresentationOpportunityId::new(1, 1),
        anchor,
        earliest_present,
        None,
    );
    let frame_reservation =
        crate::native_output::presentation::transaction::FramePresentationReservation::Variable(
            reservation,
        );

    assert_eq!(
        variable_presentation_lower_bound(frame_reservation),
        Some(earliest_present)
    );
    assert_eq!(
        validate_variable_physical_time(frame_reservation, pageflip_timestamp(earliest_present, 0),),
        Ok(())
    );
}

#[test]
fn monotonic_pageflip_timestamp_uncertainty_uses_bracket_and_microsecond_resolution() {
    let estimate = estimate_pageflip_timestamp(
        oblivion_one::native::drm::DrmTimestampClock::Monotonic,
        1,
        100_000,
        1_100_100,
        1_000_000_000,
        1_000_004_000,
        1_000_000_000,
    );

    assert_eq!(
        estimate.presented_at,
        MonotonicTimestampNs::new(999_902_000)
    );
    assert_eq!(
        estimate.uncertainty,
        PageflipTimestampUncertainty::BoundedNs(2_999)
    );
}

#[test]
fn realtime_pageflip_timestamp_has_unbounded_monotonic_mapping_uncertainty() {
    let estimate = estimate_pageflip_timestamp(
        oblivion_one::native::drm::DrmTimestampClock::Realtime,
        1,
        100_000,
        1_100_100,
        1_000_000_000,
        1_000_004_000,
        1_000_000_000,
    );

    assert_eq!(
        estimate.uncertainty,
        PageflipTimestampUncertainty::Unbounded
    );
    let anchor = MonotonicTimestampNs::new(1_000_000_000);
    let lower_bound = MonotonicTimestampNs::new(anchor.get() + 8_000_000);
    let reservation = variable_reservation(
        oblivion_one::native::buffering::PresentationOpportunityId::new(1, 1),
        anchor,
        lower_bound,
        None,
    );
    assert!(matches!(
        validate_variable_physical_time(
            crate::native_output::presentation::transaction::FramePresentationReservation::Variable(
                reservation,
            ),
            estimate,
        ),
        Err(VariablePhysicalTimeViolation::TimestampUncertaintyUnavailable { .. })
    ));
}

#[test]
fn variable_pageflip_consumes_exactly_its_reserved_id_after_short_and_long_intervals() {
    let anchor = MonotonicTimestampNs::new(1_000_000_000);
    let interval = std::time::Duration::from_nanos(6_060_606);
    for elapsed_ns in [9_400_000, 100_000_000] {
        let mut deadline = PresentationDeadlinePlanner::new(interval);
        deadline.note_presented(anchor);
        let expected_id = deadline
            .next_variable_opportunity_id()
            .expect("next sequence is available");
        let presented_at = MonotonicTimestampNs::new(anchor.get() + elapsed_ns);
        let opportunity = oblivion_one::native::buffering::PresentationOpportunity::new(
            expected_id,
            PresentationTiming::VrrWindow {
                anchor,
                earliest_present: MonotonicTimestampNs::new(anchor.get() + 6_060_606),
                presentation_not_before: None,
                preferred_present: presented_at,
                service_deadline: None,
                shortest_interval: interval,
                longest_interval: None,
            },
        );
        let reservation = crate::native_output::presentation::transaction::
            VariablePresentationReservation::try_new(
                opportunity,
                PresentationTargetReason::ReactiveDouble,
            )
            .expect("VRR opportunity creates variable reservation");

        let prepared = prepare_physical_presentation_for_reservation(
            &deadline,
            crate::native_output::presentation::transaction::FramePresentationReservation::Variable(
                reservation,
            ),
            pageflip_timestamp(presented_at, 2_999),
        )
        .expect("physical pageflip consumes the reserved opportunity");
        assert_eq!(
            prepared.claim().opportunity_id(),
            reservation.opportunity().id()
        );
        assert_eq!(prepared.claim().sequence, expected_id.sequence());
        assert_eq!(prepared.claim().presentation_time, presented_at);

        deadline.commit_presented(prepared);
        assert_eq!(
            deadline.next_variable_opportunity_id().unwrap().sequence(),
            expected_id.sequence() + 1
        );
    }
}

#[test]
fn variable_render_fence_timing_never_produces_a_fixed_deadline_miss() {
    let reservation =
        crate::native_output::presentation::transaction::VariablePresentationReservation::try_new(
            oblivion_one::native::buffering::PresentationOpportunity::new(
                oblivion_one::native::buffering::PresentationOpportunityId::new(1, 1),
                PresentationTiming::VrrWindow {
                    anchor: MonotonicTimestampNs::new(1_000_000_000),
                    earliest_present: MonotonicTimestampNs::new(1_006_060_606),
                    presentation_not_before: None,
                    preferred_present: MonotonicTimestampNs::new(1_009_400_000),
                    service_deadline: None,
                    shortest_interval: std::time::Duration::from_nanos(6_060_606),
                    longest_interval: None,
                },
            ),
            oblivion_one::native::presentation_deadline::PresentationTargetReason::ReactiveDouble,
        )
        .expect("VRR window creates a Variable reservation");
    let signaled_at = MonotonicTimestampNs::new(1_100_000_000);
    let journal = AdaptiveRenderJournal::default();

    assert_eq!(
        super::classify_pending_fence_deadline_miss(
            crate::native_output::presentation::transaction::FramePresentationReservation::Variable(
                reservation,
            ),
            crate::native_output::presentation::kms_timing::KmsPresentationTiming::variable(
                1_050_000_000,
            ),
            FenceTimestampQuality::ExactSyncFile,
            signaled_at,
            &journal,
            0,
        )
        .expect("Variable fence timing is domain-valid"),
        None,
        "fence lateness relative to preferred_present is not a deadline miss"
    );

    let fixed_target = reactive_test_target();
    let fixed_window =
        crate::native_output::presentation::kms_timing::KmsSubmitWindow::try_new(100, 0, 0, 0)
            .expect("test fixed KMS submit window");
    assert_eq!(
        super::classify_pending_fence_deadline_miss(
            crate::native_output::presentation::transaction::FramePresentationReservation::Fixed(
                fixed_target,
            ),
            crate::native_output::presentation::kms_timing::KmsPresentationTiming::Fixed(
                fixed_window,
            ),
            FenceTimestampQuality::ExactSyncFile,
            MonotonicTimestampNs::new(101),
            &journal,
            0,
        )
        .expect("Fixed fence timing is domain-valid"),
        Some(ProvenDeadlineMiss::ExactRender),
        "Fixed deadline assessment remains active"
    );
}

fn plane_delta_transaction(
    transaction_id: u64,
    output_generation: u64,
    cursor_epoch: u64,
    mode: OutputPresentationMode,
    content_type: DrmContentType,
) -> OutputTransaction {
    OutputTransaction::cursor_plane_delta(
        OutputId::from_raw(1).unwrap(),
        OutputTransactionId::new(NonZeroU64::new(transaction_id).unwrap()),
        output_generation,
        MonotonicTimestampNs::new(transaction_id),
        worker_test_target(),
        NativeOutputPacingMode::ReactiveDouble,
        cursor_epoch,
        None,
        OutputReleasePlan::Pageflip,
    )
    .unwrap()
    .with_presentation_state(mode, content_type)
    .unwrap()
}

#[test]
fn successful_plane_delta_pageflips_confirm_mode_and_content_type_without_primary_completion() {
    let mut confirmed = ConfirmedKmsPresentationState::default();
    let adaptive = plane_delta_transaction(
        1,
        7,
        1,
        OutputPresentationMode::AdaptiveSync,
        DrmContentType::Photo,
    );
    assert!(confirm_kms_presentation_state(
        &mut confirmed,
        &adaptive,
        7,
        true
    ));
    assert_eq!(confirmed.mode, OutputPresentationMode::AdaptiveSync);
    assert_eq!(confirmed.content_type, DrmContentType::Photo);
    assert_eq!(confirmed.output_generation, 7);

    let vsync =
        plane_delta_transaction(2, 7, 2, OutputPresentationMode::Vsync, DrmContentType::Game);
    assert!(confirm_kms_presentation_state(
        &mut confirmed,
        &vsync,
        7,
        true
    ));
    assert_eq!(confirmed.mode, OutputPresentationMode::Vsync);
    assert_eq!(confirmed.content_type, DrmContentType::Game);

    let delta_completion = AtomicCommitCompletion::Completed {
        kind: AtomicCommitKind::PlaneDelta {
            transaction_id: OutputTransactionId::new(NonZeroU64::new(2).unwrap()),
            cursor_epoch: 2,
            framebuffer_id: None,
        },
        submitted_at_ns: 10,
    };
    assert!(is_non_primary_atomic_completion(delta_completion));
    assert!(
        !AtomicCommitKind::PlaneDelta {
            transaction_id: OutputTransactionId::new(NonZeroU64::new(2).unwrap()),
            cursor_epoch: 2,
            framebuffer_id: None,
        }
        .is_primary()
    );
}

#[test]
fn stale_generation_and_rejected_plane_delta_do_not_confirm_presentation_state() {
    let mut confirmed = ConfirmedKmsPresentationState {
        mode: OutputPresentationMode::AdaptiveSync,
        content_type: DrmContentType::Photo,
        output_generation: 7,
    };
    let before = confirmed;
    let stale =
        plane_delta_transaction(3, 6, 3, OutputPresentationMode::Vsync, DrmContentType::Game);
    assert!(!confirm_kms_presentation_state(
        &mut confirmed,
        &stale,
        7,
        true
    ));
    assert_eq!(confirmed, before);

    let rejected =
        plane_delta_transaction(4, 7, 4, OutputPresentationMode::Vsync, DrmContentType::Game);
    assert!(!confirm_kms_presentation_state(
        &mut confirmed,
        &rejected,
        7,
        false
    ));
    assert_eq!(confirmed, before);
}

#[test]
fn seat_disable_wins_over_a_same_batch_recovery_fence_wake() {
    assert!(!should_continue_resuming_recovery(true, true, true));
    assert!(should_continue_resuming_recovery(true, true, false));
    assert!(!should_continue_resuming_recovery(true, false, false));
}

fn dispatch_recovery_evidence(
    binding_target: bool,
    fair_dispatch_chance: bool,
    deadline_overrun_ns: u64,
    guard_before_ns: u64,
    guard_ns: u64,
    increased: bool,
    cap_hit: bool,
) -> KmsWorkerDispatchTailObservation {
    KmsWorkerDispatchTailObservation {
        binding_target,
        fair_dispatch_chance,
        dequeued_before_planned_wake: fair_dispatch_chance,
        deadline_overrun_ns,
        guard_before_ns,
        guard_ns,
        increased,
        decayed: false,
        cap_hit,
    }
}

fn reactive_test_target() -> PresentationTarget {
    PresentationTarget {
        reason: PresentationTargetReason::ReactiveDouble,
        ..worker_test_target()
    }
}

fn render_readiness_test_window() -> crate::native_output::presentation::kms_timing::KmsSubmitWindow
{
    crate::native_output::presentation::kms_timing::KmsSubmitWindow::try_new(100, 40, 20, 20)
        .unwrap()
}

fn fallback_render_readiness() -> RenderReadinessObservation {
    RenderReadinessObservation::from_frame(None, MonotonicTimestampNs::new(81))
}

fn journal_with_warm_parallel_samples() -> AdaptiveRenderJournal {
    let mut journal = AdaptiveRenderJournal::default();
    let target = reactive_test_target();
    for sample in 0..20 {
        let base = sample * 2_000_000;
        journal.record_fixed_submission_budget(500_000);
        journal.record_frame_service_observation(FrameTimingObservation {
            frame_id: sample,
            target,
            composite_started_at: MonotonicTimestampNs::new(base),
            worker_queued_at: Some(MonotonicTimestampNs::new(base + 1_000_000)),
            fence_signaled_at: Some((
                MonotonicTimestampNs::new(base + 1_000_000),
                FenceTimestampQuality::ExactSyncFile,
            )),
        });
    }
    journal
}

#[test]
fn reactive_dispatch_slip_is_advisory_not_proven_recovery_evidence() {
    assert_eq!(
        assess_presentation_deadline(
            reactive_test_target(),
            KmsPresentationOutcome::KmsDispatchMiss,
            fallback_render_readiness(),
        ),
        PresentationDeadlineAssessment::Advisory(AdvisoryOpportunitySlip::KmsDispatch)
    );
}

#[test]
fn advisory_dispatch_does_not_change_warm_parallel_or_active_recovery() {
    let mut warm = journal_with_warm_parallel_samples();
    let refresh = std::time::Duration::from_millis(10);
    assert_eq!(
        warm.prediction(refresh).estimator_mode,
        oblivion_one::native::adaptive_buffering::PredictionEstimatorMode::WarmParallel
    );
    let before_warm = warm.prediction(refresh).miss_recovery_remaining;
    apply_deadline_assessment(
        PresentationDeadlineAssessment::Advisory(AdvisoryOpportunitySlip::KmsDispatch),
        &mut warm,
        refresh,
        None,
        None,
    );
    assert_eq!(
        warm.prediction(refresh).miss_recovery_remaining,
        before_warm
    );
    assert_eq!(
        warm.prediction(refresh).estimator_mode,
        oblivion_one::native::adaptive_buffering::PredictionEstimatorMode::WarmParallel
    );

    let mut recovering = AdaptiveRenderJournal::default();
    apply_deadline_assessment(
        PresentationDeadlineAssessment::Proven(ProvenDeadlineMiss::ExactRender),
        &mut recovering,
        refresh,
        None,
        None,
    );
    for sample in 0..5 {
        let base = sample * 2_000_000;
        recovering.record_frame_service_observation(FrameTimingObservation {
            frame_id: sample,
            target: reactive_test_target(),
            composite_started_at: MonotonicTimestampNs::new(base),
            worker_queued_at: Some(MonotonicTimestampNs::new(base + 1_000_000)),
            fence_signaled_at: Some((
                MonotonicTimestampNs::new(base + 1_000_000),
                FenceTimestampQuality::ExactSyncFile,
            )),
        });
    }
    let before_recovery = recovering.prediction(refresh).miss_recovery_remaining;
    apply_deadline_assessment(
        PresentationDeadlineAssessment::Advisory(AdvisoryOpportunitySlip::KmsDispatch),
        &mut recovering,
        refresh,
        None,
        None,
    );
    assert_eq!(
        recovering.prediction(refresh).miss_recovery_remaining,
        before_recovery
    );
}

#[test]
fn advisory_target_render_readiness_misses_remain_proven() {
    let target = reactive_test_target();
    let refresh = std::time::Duration::from_millis(10);
    let window =
        crate::native_output::presentation::kms_timing::KmsSubmitWindow::try_new(100, 40, 20, 20)
            .unwrap();
    for (quality, expected) in [
        (
            FenceTimestampQuality::ExactSyncFile,
            ProvenDeadlineMiss::ExactRender,
        ),
        (
            FenceTimestampQuality::ObservedApproximate,
            ProvenDeadlineMiss::GuardedApproximateRender,
        ),
    ] {
        let outcome = KmsPresentationOutcome::classify(
            &window,
            Some(81),
            90,
            target.sequence,
            target.sequence + 1,
        );
        assert_eq!(outcome, KmsPresentationOutcome::RenderReadinessMiss);
        let assessment = assess_presentation_deadline(
            target,
            outcome,
            RenderReadinessObservation::from_frame(
                Some((MonotonicTimestampNs::new(81), quality)),
                MonotonicTimestampNs::new(70),
            ),
        );
        assert_eq!(assessment, PresentationDeadlineAssessment::Proven(expected));

        let mut journal = AdaptiveRenderJournal::default();
        let recovery = apply_deadline_assessment(assessment, &mut journal, refresh, None, None)
            .expect("render readiness remains recovery evidence");
        assert_eq!(recovery.miss, expected);
        assert_eq!(
            recovery.disposition,
            EstimatorRecoveryDisposition::ResetIndependentHorizon
        );
        assert_eq!(recovery.recovery_remaining_after, 20);
    }
}

#[test]
fn binding_dispatch_and_apply_guard_assessments_remain_proven() {
    let binding = worker_test_target();
    assert_eq!(
        assess_presentation_deadline(
            binding,
            KmsPresentationOutcome::KmsDispatchMiss,
            fallback_render_readiness(),
        ),
        PresentationDeadlineAssessment::Proven(ProvenDeadlineMiss::KmsDispatch)
    );
    assert_eq!(
        assess_presentation_deadline(
            reactive_test_target(),
            KmsPresentationOutcome::KmsApplyGuardMiss,
            fallback_render_readiness(),
        ),
        PresentationDeadlineAssessment::Proven(ProvenDeadlineMiss::KmsApplyGuard)
    );
}

#[test]
fn exact_fence_readiness_wins_over_rendered_at_for_classification_and_recovery() {
    let readiness = RenderReadinessObservation::from_frame(
        Some((
            MonotonicTimestampNs::new(81),
            FenceTimestampQuality::ExactSyncFile,
        )),
        MonotonicTimestampNs::new(70),
    );
    let outcome = KmsPresentationOutcome::classify(
        &render_readiness_test_window(),
        Some(readiness.observed_at().get()),
        79,
        4,
        4,
    );

    assert_eq!(readiness.source(), RenderReadinessSource::ExactSyncFile);
    assert_eq!(readiness.observed_at().get(), 81);
    assert_eq!(readiness.fence_ready_at_ns(), Some(81));
    assert_eq!(outcome, KmsPresentationOutcome::RenderReadinessMiss);
    assert_eq!(
        assess_presentation_deadline(worker_test_target(), outcome, readiness),
        PresentationDeadlineAssessment::Proven(ProvenDeadlineMiss::ExactRender)
    );
}

#[test]
fn approximate_fence_readiness_is_guarded_render_recovery_evidence() {
    let readiness = RenderReadinessObservation::from_frame(
        Some((
            MonotonicTimestampNs::new(81),
            FenceTimestampQuality::ObservedApproximate,
        )),
        MonotonicTimestampNs::new(70),
    );
    let outcome = KmsPresentationOutcome::classify(
        &render_readiness_test_window(),
        Some(readiness.observed_at().get()),
        79,
        4,
        4,
    );

    assert_eq!(
        readiness.source(),
        RenderReadinessSource::ObservedApproximate
    );
    assert_eq!(readiness.observed_at().get(), 81);
    assert_eq!(readiness.fence_ready_at_ns(), Some(81));
    assert_eq!(outcome, KmsPresentationOutcome::RenderReadinessMiss);
    assert_eq!(
        assess_presentation_deadline(worker_test_target(), outcome, readiness),
        PresentationDeadlineAssessment::Proven(ProvenDeadlineMiss::GuardedApproximateRender)
    );
}

#[test]
fn rendered_at_fallback_is_an_observation_without_payload_ready_proof() {
    let readiness = fallback_render_readiness();
    let outcome = KmsPresentationOutcome::classify(
        &render_readiness_test_window(),
        Some(readiness.observed_at().get()),
        79,
        4,
        4,
    );

    assert_eq!(
        readiness.source(),
        RenderReadinessSource::RenderedAtFallback
    );
    assert_eq!(readiness.observed_at().get(), 81);
    assert_eq!(readiness.fence_ready_at_ns(), None);
    assert_eq!(
        outcome,
        KmsPresentationOutcome::RenderReadinessMiss,
        "rendered_at still drives the existing physical lower-bound classification"
    );
    assert_eq!(
        assess_presentation_deadline(worker_test_target(), outcome, readiness),
        PresentationDeadlineAssessment::Proven(ProvenDeadlineMiss::ExactRender)
    );
}

#[test]
fn rendered_at_fallback_exposes_observation_lateness_without_fence_lateness() {
    let window = render_readiness_test_window();
    let readiness = RenderReadinessObservation::from_frame(
        None,
        MonotonicTimestampNs::new(window.commit_complete_deadline_ns() + 250_000),
    );
    let fields = render_readiness_diagnostic_fields(
        readiness,
        &window,
        MonotonicTimestampNs::new(10),
        MonotonicTimestampNs::new(250_080),
    );
    let line = pacing_line("proven_deadline_miss", &fields);

    assert_eq!(
        readiness.observation_lateness_ns(window.commit_complete_deadline_ns()),
        250_000
    );
    assert_eq!(
        readiness.fence_lateness_ns(window.commit_complete_deadline_ns()),
        None
    );
    assert!(line.contains("render_readiness_source=rendered_at_fallback"));
    assert!(line.contains("fence_timestamp_quality=none"));
    assert!(line.contains("payload_ready_at_ns=none"));
    assert!(line.contains("render_readiness_lateness_ns=none"));
    assert!(line.contains("render_readiness_observed_at_ns=250080"));
    assert!(line.contains("render_readiness_observation_lateness_ns=250000"));
}

#[test]
fn on_time_rendered_at_fallback_does_not_create_a_render_readiness_miss() {
    let readiness = RenderReadinessObservation::from_frame(
        None,
        MonotonicTimestampNs::new(render_readiness_test_window().commit_complete_deadline_ns()),
    );
    let outcome = KmsPresentationOutcome::classify(
        &render_readiness_test_window(),
        Some(readiness.observed_at().get()),
        79,
        4,
        4,
    );

    assert_eq!(outcome, KmsPresentationOutcome::TargetHit);
    assert_eq!(
        assess_presentation_deadline(worker_test_target(), outcome, readiness),
        PresentationDeadlineAssessment::None
    );
}

#[test]
fn pending_proven_render_evidence_wins_over_pageflip_fallback_reconstruction() {
    let assessment = resolve_presentation_deadline_assessment(
        Some(ProvenDeadlineMiss::GuardedApproximateRender),
        worker_test_target(),
        Some(KmsPresentationOutcome::RenderReadinessMiss),
        RenderReadinessObservation::from_frame(None, MonotonicTimestampNs::new(81)),
    );

    assert_eq!(
        assessment,
        PresentationDeadlineAssessment::Proven(ProvenDeadlineMiss::GuardedApproximateRender)
    );
}

#[test]
fn advisory_dispatch_remains_submit_limited_for_content_attribution() {
    assert_eq!(
        classify_content_frame(
            false,
            None,
            1_000_000,
            1,
            1,
            TargetSelectionEvidence {
                earliest_feasible_sequence: 1,
                binding: false,
            },
            1,
            false,
            true,
            false,
        ),
        ContentCadenceAttribution::SubmitLimited
    );
}

fn apply_recovery_evidence(
    accepted: bool,
    apply_guard_before_ns: u64,
    apply_guard_after_ns: u64,
    apply_guard_increased: bool,
    apply_guard_cap_hit: bool,
) -> KmsPresentationTimingObservation {
    KmsPresentationTimingObservation {
        accepted,
        apply_guard_before_ns,
        apply_guard_after_ns,
        apply_guard_increased,
        apply_guard_cap_hit,
    }
}

#[test]
fn dispatch_budget_diagnostics_distinguish_used_from_post_adaptation_budget() {
    let window = crate::native_output::presentation::kms_timing::KmsSubmitWindow::try_new(
        1_000_000, 0, 100_000, 100_000,
    )
    .unwrap();
    let mut model = KmsWorkerDispatchModel::default();
    let _observation = model.observe_submission_deadline(
        window.commit_complete_deadline_ns(),
        window.commit_complete_deadline_ns() + 10_000,
        true,
        true,
    );
    let budget_after_adaptation_ns = model.budget().dispatch_budget_ns;
    assert_eq!(window.dispatch_budget_ns(), 100_000);
    assert!(budget_after_adaptation_ns > window.dispatch_budget_ns());

    let fields = dispatch_budget_diagnostic_fields(
        Some(window.dispatch_budget_ns()),
        Some(budget_after_adaptation_ns),
    );
    assert_eq!(
        pacing_line("proven_deadline_miss", &fields),
        format!(
            "typhon pacing: event=proven_deadline_miss dispatch_budget_used_ns=100000 dispatch_budget_after_adaptation_ns={budget_after_adaptation_ns}"
        )
    );
}

#[test]
fn render_misses_always_reset_independent_recovery_horizon() {
    for miss in [
        ProvenDeadlineMiss::ExactRender,
        ProvenDeadlineMiss::GuardedApproximateRender,
    ] {
        assert_eq!(
            recovery_disposition(miss, None, None),
            EstimatorRecoveryDisposition::ResetIndependentHorizon
        );
    }
    assert_eq!(
        recovery_disposition(
            ProvenDeadlineMiss::ExactRender,
            Some(dispatch_recovery_evidence(
                true, true, 12_000, 100_000, 162_000, true, false
            )),
            Some(apply_recovery_evidence(true, 100_000, 150_000, true, false)),
        ),
        EstimatorRecoveryDisposition::ResetIndependentHorizon,
        "pending render readiness evidence retains precedence over KMS evidence"
    );
}

#[test]
fn dispatch_recovery_requires_complete_matching_worker_evidence() {
    let recovered = dispatch_recovery_evidence(true, true, 12_000, 100_000, 162_000, true, false);
    assert_eq!(
        recovery_disposition(ProvenDeadlineMiss::KmsDispatch, Some(recovered), None),
        EstimatorRecoveryDisposition::PreserveEstimatorState
    );

    for evidence in [
        dispatch_recovery_evidence(false, true, 12_000, 100_000, 162_000, true, false),
        dispatch_recovery_evidence(true, false, 12_000, 100_000, 100_000, false, false),
        dispatch_recovery_evidence(true, true, 0, 100_000, 100_000, false, false),
        dispatch_recovery_evidence(true, true, 12_000, 100_000, 112_000, true, true),
    ] {
        assert_eq!(
            recovery_disposition(ProvenDeadlineMiss::KmsDispatch, Some(evidence), None),
            EstimatorRecoveryDisposition::ResetIndependentHorizon
        );
    }
    assert_eq!(
        recovery_disposition(ProvenDeadlineMiss::KmsDispatch, None, None),
        EstimatorRecoveryDisposition::ResetIndependentHorizon
    );
}

#[test]
fn apply_recovery_requires_accepted_guard_increase() {
    assert_eq!(
        recovery_disposition(
            ProvenDeadlineMiss::KmsApplyGuard,
            None,
            Some(apply_recovery_evidence(true, 100_000, 150_000, true, false)),
        ),
        EstimatorRecoveryDisposition::PreserveEstimatorState
    );
    for evidence in [
        apply_recovery_evidence(false, 100_000, 100_000, false, false),
        apply_recovery_evidence(true, 100_000, 100_000, false, false),
        apply_recovery_evidence(true, 3_000_000, 3_000_000, false, true),
        apply_recovery_evidence(true, 2_987_500, 3_000_000, true, true),
    ] {
        assert_eq!(
            recovery_disposition(ProvenDeadlineMiss::KmsApplyGuard, None, Some(evidence)),
            EstimatorRecoveryDisposition::ResetIndependentHorizon
        );
    }
    assert_eq!(
        recovery_disposition(ProvenDeadlineMiss::KmsApplyGuard, None, None),
        EstimatorRecoveryDisposition::ResetIndependentHorizon
    );
}

fn worker_test_target() -> PresentationTarget {
    PresentationTarget {
        sequence: 1,
        presentation_time: MonotonicTimestampNs::new(10),
        submit_not_before: MonotonicTimestampNs::new(8),
        render_start_deadline: MonotonicTimestampNs::new(6),
        refresh_interval: std::time::Duration::from_nanos(10),
        reason: PresentationTargetReason::ForcedValidation,
        clock_generation: 1,
        estimated: false,
        predicted_unreachable: false,
        physical_claim: oblivion_one::native::presentation_deadline::PrimaryRefreshClaim {
            sequence: 1,
            presentation_time: MonotonicTimestampNs::new(10),
            clock_generation: 1,
        },
        selection_evidence: Default::default(),
    }
}

fn worker_test_frame_batch(frame_id: u64) -> oblivion_one::compositor::CompositorFrameBatchId {
    let socket = format!(
        "typhon-worker-presented-primary-test-{}-{frame_id}",
        std::process::id(),
    );
    let mut server = OwnCompositorServer::bind(socket).expect("worker test Wayland socket");
    server.take_frame_batch_for_render(frame_id)
}

#[test]
fn nvidia_ready_pacing_divergence_is_terminalized_before_a_new_output_epoch() {
    use crate::egl_renderer::native_fence::NativeRenderFence;
    use crate::native_output::NativeCursorDamageBounds;
    use crate::native_output::pacing::NativeFramePacing;
    use crate::native_output::presentation::plane::{
        FrozenCursorTestPolicy, FrozenPrimaryCursorPlan, FrozenPrimaryCursorPresentation,
        PresentedCursorDelivery,
    };
    use crate::native_output::presentation::transaction::OutputTransaction;
    use crate::native_output::runtime::scene_history::NativeFrameSceneSnapshot;
    use crate::native_output::runtime::{NativeFrameScheduler, NativeSceneHistory};
    use crate::native_output::scanout::{AtomicOutputSwapchain, OutputSlotId, OutputSlotSet};
    use oblivion_one::compositor::PresentationFrameSnapshot;
    use oblivion_one::core::OutputId;
    use oblivion_one::native::presentation_deadline::MonotonicTimestampNs;
    use oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot;
    use std::cell::Cell;
    use std::os::fd::{FromRawFd, OwnedFd};

    let output_id = OutputId::from_raw(1).expect("test output ID is nonzero");
    let output_epoch = oblivion_one::native::OutputEpochId::initial();
    let slots = OutputSlotSet::new([
        OutputSlotId::new(0).unwrap(),
        OutputSlotId::new(1).unwrap(),
        OutputSlotId::new(2).unwrap(),
    ])
    .unwrap();
    let mut swapchain =
        AtomicOutputSwapchain::from_presented_slots(slots, OutputSlotId::new(0).unwrap(), 1)
            .unwrap();
    let slot = swapchain.acquire_render_slot().unwrap();
    let mut pipe = [-1; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
        0
    );
    unsafe { libc::close(pipe[1]) };
    let render_fence =
        NativeRenderFence::from_submission_fd(unsafe { OwnedFd::from_raw_fd(pipe[0]) });
    let cursor_plan = FrozenPrimaryCursorPlan {
        delivery: PresentedCursorDelivery::Hidden,
        primary_presentation: FrozenPrimaryCursorPresentation::Preserve,
        cursor_test_policy: FrozenCursorTestPolicy::Required,
    };
    swapchain
        .prepare_ready_for_test(slot, render_fence, cursor_plan, None)
        .unwrap();
    let owner = swapchain.ready_identity().expect("physical READY exists");
    let batch_id = owner.protocol_batch_id;

    let mut scene_history = NativeSceneHistory::new(NativeFrameSceneSnapshot {
        output_id,
        frame_id: 0,
        render_generation: 0,
        scene: NativeSceneSnapshot::from_surfaces_with_scene_nodes(&[], &[], Vec::new(), &[]),
        cursor_damage: NativeCursorDamageBounds::default(),
        presentation: PresentationFrameSnapshot::empty_for_output(output_id),
        lifecycle: LifecycleFrameSnapshot::default(),
        canonical_scene_evidence: None,
        window_exit_evidence: Vec::new(),
    });
    assert!(scene_history.replace_ready(NativeFrameSceneSnapshot {
        output_id,
        frame_id: owner.frame_id,
        render_generation: owner.render_generation,
        scene: NativeSceneSnapshot::from_surfaces_with_scene_nodes(&[], &[], Vec::new(), &[]),
        cursor_damage: NativeCursorDamageBounds::default(),
        presentation: PresentationFrameSnapshot::empty_for_output(output_id),
        lifecycle: LifecycleFrameSnapshot::default(),
        canonical_scene_evidence: None,
        window_exit_evidence: Vec::new(),
    }));

    let mut frame_pacing = NativeFramePacing::from_env();
    frame_pacing.queue_visual(1, owner.render_generation);
    frame_pacing
        .note_render_started(NativeOutputPacingMode::ReactiveDouble, false)
        .unwrap();
    frame_pacing.bind_physical_output_frame(owner).unwrap();
    assert!(frame_pacing.active.is_some());
    assert!(frame_pacing.ready.is_none());
    assert!(
        !frame_pacing.abandon_ready_frame(),
        "reproduces the observed READY state with no pacing READY lane"
    );

    let mut frame_scheduler = NativeFrameScheduler::new_with_refresh_interval_ns(10, 0);
    frame_scheduler.note_render_ahead_ready();
    assert!(frame_scheduler.ready_frame_queued());

    let mut output_transactions = OutputTransactionLedger::new_for_output(output_id);
    let transaction = OutputTransaction::composited(
        output_id,
        owner.transaction_id,
        output_epoch.get(),
        MonotonicTimestampNs::new(1),
        owner.fixed_target().expect("test frame has a fixed target"),
        NativeOutputPacingMode::ReactiveDouble,
        owner.frame_id,
        owner.render_generation,
        owner.pool_generation,
        owner.slot,
        owner.framebuffer_id.get(),
        None,
        batch_id,
    )
    .unwrap();
    output_transactions.insert(transaction).unwrap();
    output_transactions
        .mark_ready(owner.transaction_id, MonotonicTimestampNs::new(2))
        .unwrap();

    let protocol_settlements = Cell::new(0);
    terminalize_unsubmitted_ready_frame(
        &mut swapchain,
        owner,
        output_epoch,
        &mut scene_history,
        &mut frame_pacing,
        &mut frame_scheduler,
        &mut output_transactions,
        MonotonicTimestampNs::new(3),
        |obligations| {
            assert_eq!(obligations.frame_batch_id(), Some(batch_id));
            protocol_settlements.set(protocol_settlements.get() + 1);
            Ok(())
        },
    )
    .expect("old READY has one authoritative terminalization path");

    assert_eq!(protocol_settlements.get(), 1);
    assert!(output_transactions.active_transaction_ids().is_empty());
    assert!(matches!(
        output_transactions
            .transaction_including_terminal(owner.transaction_id)
            .expect("old transaction is retained as a terminal record")
            .state(),
        OutputTransactionState::Terminal(OutputTransactionTerminal::Dropped {
            reason: OutputTransactionDropReason::SafeAbandonment,
            ..
        })
    ));
    assert!(!scene_history.has_ready_frame());
    assert!(!frame_scheduler.ready_frame_queued());
    assert!(!frame_pacing.has_unsettled_output_frame_ownership());
    assert_eq!(frame_pacing.unsubmitted_physical_identity(), None);
    assert_eq!(swapchain.ready_identity(), None);
    assert!(swapchain.has_suspended_ownership());
    assert!(swapchain.suspended_fences_signaled().unwrap());
    while let Some(frame) = swapchain.take_suspended_frame() {
        drop(frame);
    }
    swapchain.recover_suspended_slot(true).unwrap();
    assert!(!swapchain.has_suspended_ownership());

    let new_output_epoch = output_epoch.next();
    frame_pacing.rebase_after_output_epoch();
    let mut new_frame_scheduler = NativeFrameScheduler::new_with_refresh_interval_ns(10, 4);
    frame_pacing.queue_visual(4, owner.render_generation + 1);
    new_frame_scheduler.queue_visual_work();
    assert!(frame_pacing.active.is_some());
    assert!(new_frame_scheduler.visual_work_queued());

    let stale_predecessor_token =
        PageFlipToken::new(owner.transaction_id.get()).expect("old predecessor token is nonzero");
    let mut current_pageflip_arbiter = AtomicCommitArbiter::new();
    assert_eq!(
        current_pageflip_arbiter.complete(stale_predecessor_token, new_output_epoch.get(), 12,),
        AtomicCommitCompletion::Stale,
        "a predecessor pageflip arriving after publication has no current owner"
    );
    assert!(!current_pageflip_arbiter.atomic_commit_pending());

    assert!(!new_frame_scheduler.ready_frame_queued());
    assert_eq!(new_output_epoch.get(), output_epoch.get() + 1);

    let repeated_retirement = terminalize_unsubmitted_ready_frame(
        &mut swapchain,
        owner,
        new_output_epoch,
        &mut scene_history,
        &mut frame_pacing,
        &mut new_frame_scheduler,
        &mut output_transactions,
        MonotonicTimestampNs::new(5),
        |_| {
            protocol_settlements.set(protocol_settlements.get() + 1);
            Ok(())
        },
    );
    assert!(repeated_retirement.is_err());
    assert_eq!(protocol_settlements.get(), 1);
    assert!(frame_pacing.active.is_some());
    assert!(new_frame_scheduler.visual_work_queued());
}

#[test]
fn atomic_ready_scene_publication_uses_physical_generation_after_cursor_change() {
    use crate::native_output::NativeSceneSnapshot;
    use crate::native_output::presentation::ledger::OutputTransactionStateKind;
    use oblivion_one::compositor::PresentationFrameSnapshot;
    use oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot;

    let mut fixture = variable_retry_fixture(VariableRetryPacingLane::Ready);
    let owner = fixture.owner;
    // A cursor-only compositor generation can move ahead while the frozen
    // scene generation stays at its previous value.
    let frozen_scene_generation = owner.render_generation.saturating_add(1);
    assert_ne!(frozen_scene_generation, owner.render_generation);

    fixture.scene_history.discard_ready();
    super::super::presentation_worker::replace_atomic_ready_scene(
        &mut fixture.scene_history,
        NativeSceneSnapshot::from_surfaces_with_scene_nodes(&[], &[], Vec::new(), &[]),
        owner,
        PresentationFrameSnapshot::empty_for_output(owner.output_id),
        LifecycleFrameSnapshot::default(),
        (None, None),
    );

    assert_eq!(
        fixture.scene_history.ready_frame_identity(),
        Some((owner.frame_id, owner.render_generation)),
        "the READY scene identity must use the same render generation as its physical frame"
    );
    assert_eq!(fixture.swapchain.ready_identity(), Some(owner));
    let transaction = fixture
        .output_transactions
        .transaction(owner.transaction_id)
        .expect("READY transaction owner");
    assert_eq!(
        transaction.state().kind(),
        OutputTransactionStateKind::Ready
    );
    assert_eq!(
        transaction.descriptor().composited_frame_identity(),
        Some((
            owner.frame_id,
            owner.render_generation,
            owner.pool_generation
        ))
    );
    assert_eq!(
        fixture.frame_pacing.unsubmitted_physical_identity(),
        Some(owner)
    );
    assert!(fixture.frame_scheduler.ready_frame_queued());
    validate_reconfiguration_frame_ownership_for_swapchain(
        &fixture.swapchain,
        fixture.output_epoch,
        &fixture.scene_history,
        &fixture.frame_pacing,
        &fixture.frame_scheduler,
        &fixture.output_transactions,
    )
    .expect("strict reconfiguration validation accepts the complete READY ownership bundle");
}

#[test]
fn worker_queued_and_ready_successor_roles_are_individually_coherent() {
    let mut fixture = variable_retry_fixture(VariableRetryPacingLane::Ready);
    let output_epoch = fixture.output_epoch;
    let crtc_id = 7;
    let mut arbiter = AtomicCommitArbiter::new();
    let worker_token = PageFlipToken::new(802).unwrap();
    let (_worker, worker_ticket) =
        queue_fixture_ready_as_worker(&mut fixture, &mut arbiter, worker_token, crtc_id);
    let ready_successor = append_ready_successor(&mut fixture, 2, 2_000_000_000);

    assert_ne!(ready_successor, fixture.owner);
    assert_eq!(
        fixture
            .swapchain
            .worker_queued_identity()
            .map(|owner| owner.frame),
        Some(fixture.owner)
    );
    assert_eq!(fixture.swapchain.ready_identity(), Some(ready_successor));
    assert_eq!(
        fixture.scene_history.submitted_identity(worker_token.get()),
        Some((
            fixture.owner.output_id,
            fixture.owner.frame_id,
            fixture.owner.render_generation
        ))
    );
    assert_eq!(
        fixture.scene_history.ready_frame_identity(),
        Some((ready_successor.frame_id, ready_successor.render_generation))
    );
    assert_eq!(
        fixture.frame_pacing.worker_reservation_ticket(),
        Some(worker_ticket)
    );
    assert_eq!(
        fixture.frame_pacing.unsubmitted_physical_identity(),
        Some(ready_successor)
    );
    assert!(fixture.frame_scheduler.ready_frame_queued());
    validate_reconfiguration_frame_ownership_for_swapchain_with_arbiter(
        &fixture.swapchain,
        output_epoch,
        crtc_id,
        &fixture.scene_history,
        &fixture.frame_pacing,
        &fixture.frame_scheduler,
        &arbiter,
        &fixture.output_transactions,
    )
    .expect("worker A and independently coherent READY successor B may coexist");
}

#[test]
fn worker_queued_role_rejects_missing_or_mismatched_submitted_scene() {
    use crate::native_output::presentation::ledger::OutputTransactionStateKind;

    let crtc_id = 7;
    let mut missing = variable_retry_fixture(VariableRetryPacingLane::Ready);
    let mut missing_arbiter = AtomicCommitArbiter::new();
    let token = PageFlipToken::new(811).unwrap();
    let (_worker, _) =
        queue_fixture_ready_as_worker(&mut missing, &mut missing_arbiter, token, crtc_id);
    missing
        .scene_history
        .discard_submission_exact(
            token.get(),
            missing.owner.output_id,
            missing.owner.frame_id,
            missing.owner.render_generation,
        )
        .unwrap();
    assert!(
        validate_reconfiguration_frame_ownership_for_swapchain_with_arbiter(
            &missing.swapchain,
            missing.output_epoch,
            crtc_id,
            &missing.scene_history,
            &missing.frame_pacing,
            &missing.frame_scheduler,
            &missing_arbiter,
            &missing.output_transactions,
        )
        .is_err()
    );

    let mut wrong_scene = variable_retry_fixture(VariableRetryPacingLane::Ready);
    let mut wrong_scene_arbiter = AtomicCommitArbiter::new();
    let wrong_scene_token = PageFlipToken::new(812).unwrap();
    let (_worker, _) = queue_fixture_ready_as_worker(
        &mut wrong_scene,
        &mut wrong_scene_arbiter,
        wrong_scene_token,
        crtc_id,
    );
    let original = wrong_scene.owner;
    wrong_scene
        .scene_history
        .rollback_submission_to_ready(
            wrong_scene_token.get(),
            original.output_id,
            original.frame_id,
            original.render_generation,
        )
        .unwrap();
    let successor = append_ready_successor(&mut wrong_scene, 2, 2_000_000_000);
    assert_ne!(successor.frame_id, original.frame_id);
    wrong_scene
        .scene_history
        .queue_submission_or_error(wrong_scene_token.get())
        .unwrap();
    assert_eq!(
        wrong_scene
            .scene_history
            .submitted_identity(wrong_scene_token.get()),
        Some((
            successor.output_id,
            successor.frame_id,
            successor.render_generation
        ))
    );
    let wrong_error = validate_reconfiguration_frame_ownership_for_swapchain_with_arbiter(
        &wrong_scene.swapchain,
        wrong_scene.output_epoch,
        crtc_id,
        &wrong_scene.scene_history,
        &wrong_scene.frame_pacing,
        &wrong_scene.frame_scheduler,
        &wrong_scene_arbiter,
        &wrong_scene.output_transactions,
    )
    .expect_err("a submitted scene for successor B cannot validate worker frame A");
    assert!(wrong_error.to_string().contains("submitted scene"));

    // The transaction role must remain Queued until the worker resolves.
    let mut wrong_transaction = variable_retry_fixture(VariableRetryPacingLane::Ready);
    let mut transaction_arbiter = AtomicCommitArbiter::new();
    let transaction_token = PageFlipToken::new(813).unwrap();
    let (_worker, _) = queue_fixture_ready_as_worker(
        &mut wrong_transaction,
        &mut transaction_arbiter,
        transaction_token,
        crtc_id,
    );
    assert_eq!(
        wrong_transaction
            .output_transactions
            .transaction(wrong_transaction.owner.transaction_id)
            .unwrap()
            .state()
            .kind(),
        OutputTransactionStateKind::Queued
    );
    wrong_transaction
        .output_transactions
        .rollback_queued(wrong_transaction.owner.transaction_id)
        .unwrap();
    assert!(
        validate_reconfiguration_frame_ownership_for_swapchain_with_arbiter(
            &wrong_transaction.swapchain,
            wrong_transaction.output_epoch,
            crtc_id,
            &wrong_transaction.scene_history,
            &wrong_transaction.frame_pacing,
            &wrong_transaction.frame_scheduler,
            &transaction_arbiter,
            &wrong_transaction.output_transactions,
        )
        .is_err()
    );
}

#[test]
fn worker_queued_role_rejects_orphaned_arbiter_pacing_and_scheduler_owners() {
    use crate::native_output::presentation::ledger::OutputTransactionLedger;

    let crtc_id = 7;

    let mut wrong_arbiter = variable_retry_fixture(VariableRetryPacingLane::Ready);
    let mut correct_arbiter = AtomicCommitArbiter::new();
    let token = PageFlipToken::new(821).unwrap();
    let (_worker, ticket) =
        queue_fixture_ready_as_worker(&mut wrong_arbiter, &mut correct_arbiter, token, crtc_id);
    let wrong_transaction_id = OutputTransactionId::new(NonZeroU64::new(99_999).unwrap());
    let wrong_kind = AtomicCommitKind::CompositedPrimary {
        transaction_id: wrong_transaction_id,
        frame_id: wrong_arbiter.owner.frame_id,
        framebuffer_id: wrong_arbiter.owner.framebuffer_id.get(),
    };
    let mut missing_transaction_arbiter = AtomicCommitArbiter::new();
    missing_transaction_arbiter
        .reserve_worker_queued(
            token,
            wrong_arbiter.output_epoch.get(),
            crtc_id,
            wrong_kind,
            3,
        )
        .unwrap();
    assert!(
        validate_reconfiguration_frame_ownership_for_swapchain_with_arbiter(
            &wrong_arbiter.swapchain,
            wrong_arbiter.output_epoch,
            crtc_id,
            &wrong_arbiter.scene_history,
            &wrong_arbiter.frame_pacing,
            &wrong_arbiter.frame_scheduler,
            &missing_transaction_arbiter,
            &wrong_arbiter.output_transactions,
        )
        .is_err()
    );

    let mut mismatched_token_arbiter = AtomicCommitArbiter::new();
    mismatched_token_arbiter
        .reserve_worker_queued(
            PageFlipToken::new(token.get() + 1).unwrap(),
            wrong_arbiter.output_epoch.get(),
            crtc_id,
            AtomicCommitKind::CompositedPrimary {
                transaction_id: wrong_arbiter.owner.transaction_id,
                frame_id: wrong_arbiter.owner.frame_id,
                framebuffer_id: wrong_arbiter.owner.framebuffer_id.get(),
            },
            3,
        )
        .unwrap();
    assert!(
        validate_reconfiguration_frame_ownership_for_swapchain_with_arbiter(
            &wrong_arbiter.swapchain,
            wrong_arbiter.output_epoch,
            crtc_id,
            &wrong_arbiter.scene_history,
            &wrong_arbiter.frame_pacing,
            &wrong_arbiter.frame_scheduler,
            &mismatched_token_arbiter,
            &wrong_arbiter.output_transactions,
        )
        .is_err()
    );

    let mut wrong_ticket_identity = wrong_arbiter.owner;
    wrong_ticket_identity.frame_id = wrong_ticket_identity.frame_id.saturating_add(1);
    assert!(
        wrong_arbiter
            .frame_pacing
            .validate_worker_queued_physical_owner(ticket, wrong_ticket_identity)
            .is_err()
    );
    assert!(
        wrong_arbiter
            .frame_pacing
            .cancel_worker_submission(Some(ticket))
    );
    assert!(
        validate_reconfiguration_frame_ownership_for_swapchain_with_arbiter(
            &wrong_arbiter.swapchain,
            wrong_arbiter.output_epoch,
            crtc_id,
            &wrong_arbiter.scene_history,
            &wrong_arbiter.frame_pacing,
            &wrong_arbiter.frame_scheduler,
            &correct_arbiter,
            &wrong_arbiter.output_transactions,
        )
        .is_err()
    );

    let mut wrong_scheduler = variable_retry_fixture(VariableRetryPacingLane::Ready);
    let mut scheduler_arbiter = AtomicCommitArbiter::new();
    let scheduler_token = PageFlipToken::new(822).unwrap();
    let (_worker, _) = queue_fixture_ready_as_worker(
        &mut wrong_scheduler,
        &mut scheduler_arbiter,
        scheduler_token,
        crtc_id,
    );
    wrong_scheduler.frame_scheduler.note_ready_frame(None);
    assert!(
        validate_reconfiguration_frame_ownership_for_swapchain_with_arbiter(
            &wrong_scheduler.swapchain,
            wrong_scheduler.output_epoch,
            crtc_id,
            &wrong_scheduler.scene_history,
            &wrong_scheduler.frame_pacing,
            &wrong_scheduler.frame_scheduler,
            &scheduler_arbiter,
            &wrong_scheduler.output_transactions,
        )
        .is_err()
    );

    let mut orphan = variable_retry_fixture(VariableRetryPacingLane::Ready);
    assert!(orphan.swapchain.suspend_abandon_ready().unwrap());
    orphan
        .scene_history
        .discard_ready_if_identity(orphan.owner.frame_id, orphan.owner.render_generation);
    orphan.frame_scheduler.consume_ready_frame();
    let empty_ledger = OutputTransactionLedger::new_for_output(orphan.owner.output_id);
    let empty_arbiter = AtomicCommitArbiter::new();
    assert!(
        validate_reconfiguration_frame_ownership_for_swapchain_with_arbiter(
            &orphan.swapchain,
            orphan.output_epoch,
            crtc_id,
            &orphan.scene_history,
            &orphan.frame_pacing,
            &orphan.frame_scheduler,
            &empty_arbiter,
            &empty_ledger,
        )
        .is_err()
    );
}

#[test]
fn invalidated_worker_replan_reaches_output_configuration_with_coherent_ready_owners() {
    use crate::native_output::NativeOutputReconfigurationBoundary;
    use crate::native_output::kms_worker::ValidationBaseInvalidationReason;
    use crate::native_output::presentation::ledger::{
        OutputTransactionDropReason, OutputTransactionStateKind, OutputTransactionTerminal,
    };
    use crate::native_output::presentation::transaction::OutputProtocolObligations;
    use crate::native_output::runtime::kms_worker::{
        ValidationBaseInvalidationDisposition, validation_base_invalidation_disposition,
    };
    use oblivion_one::native::presentation_deadline::MonotonicTimestampNs;

    let mut fixture = variable_retry_fixture(VariableRetryPacingLane::Ready);
    let owner = fixture.owner;
    let output_epoch = fixture.output_epoch;
    let crtc_id = 7;
    let worker_token = PageFlipToken::new(701).unwrap();
    let queued_at = MonotonicTimestampNs::new(30);
    let kind = AtomicCommitKind::CompositedPrimary {
        transaction_id: owner.transaction_id,
        frame_id: owner.frame_id,
        framebuffer_id: owner.framebuffer_id.get(),
    };
    let pacing_ticket = fixture
        .frame_pacing
        .reserve_worker_submission(true)
        .expect("READY pacing reservation")
        .expect("worker pacing ticket");
    let (submission_fence, _) = fixture
        .swapchain
        .take_ready_for_worker(worker_token, queued_at)
        .expect("physical READY moves to WORKER_QUEUED");
    fixture
        .output_transactions
        .mark_queued(owner.transaction_id, output_epoch.get(), queued_at)
        .expect("logical READY moves to Queued");
    let mut arbiter = AtomicCommitArbiter::new();
    arbiter
        .reserve_worker_queued(
            worker_token,
            output_epoch.get(),
            crtc_id,
            kind,
            queued_at.get(),
        )
        .expect("worker owns the Atomic commit lane");
    assert!(fixture.scene_history.queue_submission(worker_token.get()));
    fixture.frame_scheduler.consume_ready_frame();

    assert_eq!(
        validation_base_invalidation_disposition(
            true,
            false,
            output_epoch.get(),
            output_epoch.get(),
            ValidationBaseInvalidationReason::PresentedRevisionChanged,
        ),
        ValidationBaseInvalidationDisposition::Replan
    );
    let request = ExplicitWorkerReplanRequest {
        output_id: owner.output_id,
        output_epoch,
        crtc_id,
        transaction_id: owner.transaction_id,
        token: worker_token,
        kind,
        frame_id: owner.frame_id,
        framebuffer_id: owner.framebuffer_id.get(),
        reservation: owner.reservation,
        queued_at,
        ready_submit: true,
        pacing_ticket: Some(pacing_ticket),
        submission_fence_available: true,
    };
    let validated = preflight_explicit_worker_replan(
        &fixture.swapchain,
        request,
        &fixture.scene_history,
        &fixture.frame_pacing,
        &fixture.frame_scheduler,
        &arbiter,
        &fixture.output_transactions,
    )
    .expect("the validation-base replan begins from one exact submitted owner");
    let mut submission_fence = Some(submission_fence);
    let mut cursor_owner = None;
    restore_explicit_worker_replan(
        validated,
        &mut fixture.swapchain,
        &mut submission_fence,
        &mut cursor_owner,
        &mut fixture.scene_history,
        &mut fixture.frame_pacing,
        &mut fixture.frame_scheduler,
        &mut arbiter,
        &mut fixture.output_transactions,
        queued_at.get(),
    )
    .expect("production replan restores the exact frozen READY ownership");
    drop(submission_fence);

    assert_eq!(fixture.swapchain.ready_identity(), Some(owner));
    assert_eq!(
        fixture.scene_history.ready_frame_identity(),
        Some((owner.frame_id, owner.render_generation))
    );
    assert_eq!(
        fixture.frame_pacing.unsubmitted_physical_identity(),
        Some(owner)
    );
    assert!(fixture.frame_scheduler.ready_frame_queued());
    assert_eq!(
        fixture
            .output_transactions
            .transaction(owner.transaction_id)
            .unwrap()
            .state()
            .kind(),
        OutputTransactionStateKind::Ready
    );
    validate_reconfiguration_frame_ownership_for_swapchain(
        &fixture.swapchain,
        output_epoch,
        &fixture.scene_history,
        &fixture.frame_pacing,
        &fixture.frame_scheduler,
        &fixture.output_transactions,
    )
    .expect("the output-configuration validation gate accepts the restored bundle");

    let boundary = NativeOutputReconfigurationBoundary {
        presentation_transaction_owned: true,
        ..NativeOutputReconfigurationBoundary::default()
    };
    assert!(boundary.is_safe_to_retire_unsubmitted_ready_frame());
    terminalize_unsubmitted_ready_frame(
        &mut fixture.swapchain,
        owner,
        output_epoch,
        &mut fixture.scene_history,
        &mut fixture.frame_pacing,
        &mut fixture.frame_scheduler,
        &mut fixture.output_transactions,
        MonotonicTimestampNs::new(40),
        |obligations: OutputProtocolObligations| {
            assert_eq!(obligations.frame_batch_id(), Some(owner.protocol_batch_id));
            Ok(())
        },
    )
    .expect("the output-configuration boundary retires the complete READY bundle");

    assert_eq!(fixture.swapchain.ready_identity(), None);
    assert!(!fixture.scene_history.has_ready_frame());
    assert!(!fixture.frame_pacing.has_unsettled_output_frame_ownership());
    assert!(!fixture.frame_scheduler.ready_frame_queued());
    assert!(fixture.swapchain.has_suspended_ownership());
    assert!(matches!(
        fixture
            .output_transactions
            .transaction_including_terminal(owner.transaction_id)
            .unwrap()
            .state(),
        OutputTransactionState::Terminal(OutputTransactionTerminal::Dropped {
            reason: OutputTransactionDropReason::SafeAbandonment,
            ..
        })
    ));
    validate_reconfiguration_frame_ownership_for_swapchain(
        &fixture.swapchain,
        output_epoch,
        &fixture.scene_history,
        &fixture.frame_pacing,
        &fixture.frame_scheduler,
        &fixture.output_transactions,
    )
    .expect("no READY owner remains fragmented after retirement");

    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut output_render_fence_token = None;
    let recovery = recover_or_arm_suspended_output_ownership(
        &mut SwapchainSuspendedRecovery {
            swapchain: &mut fixture.swapchain,
        },
        &mut event_loop,
        &mut output_render_fence_token,
    )
    .expect("output configuration either recovers or waits on the real render fence");
    match recovery {
        SuspendedOutputRecovery::Recovered => {
            assert!(!fixture.swapchain.has_suspended_ownership());
            assert!(output_render_fence_token.is_none());
        }
        SuspendedOutputRecovery::Waiting => {
            let token = output_render_fence_token.expect("registered render-fence wake owner");
            assert_eq!(
                event_loop.source_for_token(token),
                Some(NativeEventSource::OutputRenderFence)
            );
        }
        SuspendedOutputRecovery::None => panic!("suspended READY retirement has a real owner"),
    }
}

#[derive(Clone, Copy)]
enum VariableRetryPacingLane {
    Active,
    Ready,
}

struct VariableRetryFixture {
    output_epoch: oblivion_one::native::OutputEpochId,
    owner: crate::native_output::scanout::OutputFrameIdentitySnapshot,
    swapchain: crate::native_output::scanout::AtomicOutputSwapchain,
    scene_history: crate::native_output::runtime::NativeSceneHistory,
    frame_pacing: crate::native_output::pacing::NativeFramePacing,
    frame_scheduler: crate::native_output::runtime::NativeFrameScheduler,
    output_transactions: crate::native_output::presentation::ledger::OutputTransactionLedger,
    server: OwnCompositorServer,
}

fn signaled_retry_render_fence() -> crate::egl_renderer::native_fence::NativeRenderFence {
    use std::os::fd::{FromRawFd, OwnedFd};

    let mut pipe = [-1; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
        0
    );
    unsafe { libc::close(pipe[1]) };
    crate::egl_renderer::native_fence::NativeRenderFence::from_submission_fd(unsafe {
        OwnedFd::from_raw_fd(pipe[0])
    })
}

fn variable_retry_fixture(lane: VariableRetryPacingLane) -> VariableRetryFixture {
    variable_retry_fixture_with_render_fence(lane, signaled_retry_render_fence())
}

fn variable_retry_fixture_with_render_fence(
    lane: VariableRetryPacingLane,
    render_fence: crate::egl_renderer::native_fence::NativeRenderFence,
) -> VariableRetryFixture {
    use crate::native_output::NativeCursorDamageBounds;
    use crate::native_output::pacing::NativeFramePacing;
    use crate::native_output::presentation::kms_timing::KmsPresentationTiming;
    use crate::native_output::presentation::plane::{
        FrozenCursorTestPolicy, FrozenPrimaryCursorPlan, FrozenPrimaryCursorPresentation,
        PresentedCursorDelivery,
    };
    use crate::native_output::presentation::transaction::OutputTransaction;
    use crate::native_output::runtime::scene_history::NativeFrameSceneSnapshot;
    use crate::native_output::runtime::{NativeFrameScheduler, NativeSceneHistory};
    use crate::native_output::scanout::{OutputSlotId, OutputSlotSet};
    use oblivion_one::compositor::PresentationFrameSnapshot;
    use oblivion_one::core::OutputId;
    use oblivion_one::native::presentation_deadline::MonotonicTimestampNs;
    use oblivion_one::native::scheduler::NativeOutputPacingMode;
    use oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot;

    static NEXT_SOCKET_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

    let output_id = OutputId::from_raw(1).expect("test output ID is nonzero");
    let output_epoch = oblivion_one::native::OutputEpochId::initial();
    let slots = OutputSlotSet::new([
        OutputSlotId::new(0).unwrap(),
        OutputSlotId::new(1).unwrap(),
        OutputSlotId::new(2).unwrap(),
    ])
    .unwrap();
    let mut swapchain = crate::native_output::scanout::AtomicOutputSwapchain::from_presented_slots(
        slots,
        OutputSlotId::new(0).unwrap(),
        1,
    )
    .unwrap();
    let slot = swapchain.acquire_render_slot().unwrap();
    let cursor_plan = FrozenPrimaryCursorPlan {
        delivery: PresentedCursorDelivery::Hidden,
        primary_presentation: FrozenPrimaryCursorPresentation::Preserve,
        cursor_test_policy: FrozenCursorTestPolicy::Required,
    };
    swapchain
        .prepare_ready_for_test(slot, render_fence, cursor_plan, None)
        .unwrap();

    let anchor = MonotonicTimestampNs::new(1_000_000_000);
    let earliest = MonotonicTimestampNs::new(anchor.get() + 6_060_606);
    let reservation = variable_reservation(
        oblivion_one::native::buffering::PresentationOpportunityId::new(1, 1),
        anchor,
        earliest,
        None,
    );
    swapchain
        .set_ready_variable_presentation_for_test(
            reservation,
            KmsPresentationTiming::variable(earliest.get()),
        )
        .unwrap();
    let owner = swapchain.ready_identity().expect("physical READY exists");

    let socket_id = NEXT_SOCKET_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let socket = format!(
        "typhon-variable-retry-test-{}-{}-{}",
        std::process::id(),
        owner.frame_id,
        socket_id
    );
    let mut server = OwnCompositorServer::bind(socket).expect("test Wayland socket");
    let batch_id = server.take_frame_batch_for_render(owner.frame_id);
    assert_eq!(batch_id, owner.protocol_batch_id);

    let mut scene_history = NativeSceneHistory::new(NativeFrameSceneSnapshot {
        output_id,
        frame_id: 0,
        render_generation: 0,
        scene: NativeSceneSnapshot::from_surfaces_with_scene_nodes(&[], &[], Vec::new(), &[]),
        cursor_damage: NativeCursorDamageBounds::default(),
        presentation: PresentationFrameSnapshot::empty_for_output(output_id),
        lifecycle: LifecycleFrameSnapshot::default(),
        canonical_scene_evidence: None,
        window_exit_evidence: Vec::new(),
    });
    assert!(scene_history.replace_ready(NativeFrameSceneSnapshot {
        output_id,
        frame_id: owner.frame_id,
        render_generation: owner.render_generation,
        scene: NativeSceneSnapshot::from_surfaces_with_scene_nodes(&[], &[], Vec::new(), &[]),
        cursor_damage: NativeCursorDamageBounds::default(),
        presentation: PresentationFrameSnapshot::empty_for_output(output_id),
        lifecycle: LifecycleFrameSnapshot::default(),
        canonical_scene_evidence: None,
        window_exit_evidence: Vec::new(),
    }));

    let mut frame_pacing = NativeFramePacing::from_env();
    frame_pacing.queue_visual(1, owner.render_generation);
    frame_pacing
        .note_render_started(NativeOutputPacingMode::ReactiveDouble, false)
        .unwrap();
    frame_pacing.bind_physical_output_frame(owner).unwrap();
    let mut frame_scheduler = NativeFrameScheduler::new_with_refresh_interval_ns(10, 0);
    if matches!(lane, VariableRetryPacingLane::Ready) {
        server.note_frame_callbacks_deferred_ready(batch_id);
        frame_pacing.note_ready_frame(2, true);
        frame_scheduler.note_ready_frame(None);
    }

    let transaction = OutputTransaction::composited_variable_with_direct_equivalence(
        output_id,
        owner.transaction_id,
        output_epoch.get(),
        MonotonicTimestampNs::new(1),
        reservation,
        crate::native_output::DrmContentType::Graphics,
        NativeOutputPacingMode::ReactiveDouble,
        owner.frame_id,
        owner.render_generation,
        owner.pool_generation,
        owner.slot,
        owner.framebuffer_id.get(),
        None,
        batch_id,
        None,
    )
    .unwrap();
    let mut output_transactions =
        crate::native_output::presentation::ledger::OutputTransactionLedger::new_for_output(
            output_id,
        );
    output_transactions.insert(transaction).unwrap();
    output_transactions
        .mark_ready(owner.transaction_id, MonotonicTimestampNs::new(2))
        .unwrap();

    VariableRetryFixture {
        output_epoch,
        owner,
        swapchain,
        scene_history,
        frame_pacing,
        frame_scheduler,
        output_transactions,
        server,
    }
}

fn assert_variable_retry_terminalizes(
    lane: VariableRetryPacingLane,
    rejection: crate::native_output::scanout::VariableRetryRejection,
) {
    use crate::native_output::presentation::ledger::{
        OutputTransactionDropReason, OutputTransactionState, OutputTransactionTerminal,
    };
    use crate::native_output::presentation::plane::{
        FrozenCursorTestPolicy, FrozenPrimaryCursorPlan, FrozenPrimaryCursorPresentation,
        PresentedCursorDelivery,
    };
    use crate::native_output::scanout::OutputFrameIdentitySnapshot;
    use oblivion_one::native::presentation_deadline::MonotonicTimestampNs;

    let mut fixture = variable_retry_fixture(lane);
    let owner = fixture.owner;
    match rejection {
        crate::native_output::scanout::VariableRetryRejection::TestOnly => {
            assert!(fixture.swapchain.suspend_abandon_ready().unwrap());
        }
        crate::native_output::scanout::VariableRetryRejection::RealSubmit => {
            let frame = fixture.swapchain.take_ready_for_submission().unwrap();
            let mut no_completion_fence = None;
            fixture
                .swapchain
                .suspend_failed_submission(frame, &mut no_completion_fence)
                .unwrap();
        }
    }
    assert_eq!(fixture.swapchain.ready_identity(), None);
    assert!(fixture.swapchain.contains_suspended_frame(owner));
    assert!(
        !fixture
            .swapchain
            .contains_suspended_frame(OutputFrameIdentitySnapshot {
                frame_id: owner.frame_id.saturating_add(100),
                ..owner
            })
    );

    let output_epoch_before = fixture.output_epoch;
    super::terminalize_suspended_frame_for_variable_retry(
        &mut fixture.swapchain,
        owner,
        fixture.output_epoch,
        &mut fixture.scene_history,
        &mut fixture.frame_pacing,
        &mut fixture.frame_scheduler,
        &mut fixture.server,
        &mut fixture.output_transactions,
        MonotonicTimestampNs::new(3),
        rejection,
        &crate::native_output::NativePerfLogger::from_env(),
    )
    .expect("runtime settles the exact suspended Variable frame");

    assert_eq!(fixture.output_epoch, output_epoch_before);
    assert_eq!(fixture.scene_history.ready_frame_identity(), None);
    assert!(!fixture.frame_scheduler.ready_frame_queued());
    assert_eq!(fixture.frame_pacing.active, None);
    assert_eq!(fixture.frame_pacing.ready, None);
    assert_eq!(fixture.frame_pacing.active_physical_identity(), None);
    assert_eq!(fixture.frame_pacing.unsubmitted_physical_identity(), None);
    assert_eq!(fixture.swapchain.ready_identity(), None);
    assert!(fixture.swapchain.has_suspended_ownership());
    assert!(fixture.swapchain.contains_suspended_frame(owner));
    assert!(
        fixture
            .output_transactions
            .active_transaction_ids()
            .is_empty()
    );
    assert!(matches!(
        fixture
            .output_transactions
            .transaction_including_terminal(owner.transaction_id)
            .expect("rejected transaction remains in the terminal ledger")
            .state(),
        OutputTransactionState::Terminal(OutputTransactionTerminal::Dropped {
            reason: OutputTransactionDropReason::SafeAbandonment,
            ..
        })
    ));

    let repeated = super::terminalize_suspended_frame_for_variable_retry(
        &mut fixture.swapchain,
        owner,
        fixture.output_epoch,
        &mut fixture.scene_history,
        &mut fixture.frame_pacing,
        &mut fixture.frame_scheduler,
        &mut fixture.server,
        &mut fixture.output_transactions,
        MonotonicTimestampNs::new(4),
        rejection,
        &crate::native_output::NativePerfLogger::from_env(),
    );
    assert!(
        repeated.is_err(),
        "logical retirement cannot be applied twice"
    );
    assert!(
        fixture
            .output_transactions
            .active_transaction_ids()
            .is_empty()
    );

    assert!(fixture.swapchain.suspended_fences_signaled().unwrap());
    while let Some(frame) = fixture.swapchain.take_suspended_frame() {
        drop(frame);
    }
    fixture.swapchain.recover_suspended_slot(true).unwrap();

    let successor_slot = fixture.swapchain.acquire_render_slot().unwrap();
    fixture
        .swapchain
        .prepare_ready_for_test(
            successor_slot,
            signaled_retry_render_fence(),
            FrozenPrimaryCursorPlan {
                delivery: PresentedCursorDelivery::Hidden,
                primary_presentation: FrozenPrimaryCursorPresentation::Preserve,
                cursor_test_policy: FrozenCursorTestPolicy::Required,
            },
            None,
        )
        .unwrap();
    let successor = fixture.swapchain.ready_identity().unwrap();
    assert_ne!(successor.frame_id, owner.frame_id);
    assert_ne!(successor.transaction_id, owner.transaction_id);
    assert_eq!(successor.output_id, owner.output_id);
    assert_eq!(fixture.output_epoch.get(), output_epoch_before.get());
    fixture
        .frame_pacing
        .queue_visual(5, successor.render_generation);
    fixture
        .frame_pacing
        .note_render_started(
            oblivion_one::native::scheduler::NativeOutputPacingMode::ReactiveDouble,
            false,
        )
        .unwrap();
    fixture
        .frame_pacing
        .bind_physical_output_frame(successor)
        .expect("the terminalized predecessor releases the active pacing owner");
    assert_eq!(
        fixture.frame_pacing.active_physical_identity(),
        Some(successor)
    );
}

#[test]
fn variable_test_only_rejection_terminalizes_immediate_active_owner() {
    assert_variable_retry_terminalizes(
        VariableRetryPacingLane::Active,
        crate::native_output::scanout::VariableRetryRejection::TestOnly,
    );
}

#[test]
fn variable_real_submit_rejection_terminalizes_immediate_active_owner() {
    assert_variable_retry_terminalizes(
        VariableRetryPacingLane::Active,
        crate::native_output::scanout::VariableRetryRejection::RealSubmit,
    );
}

#[test]
fn variable_test_only_rejection_terminalizes_deferred_ready_owner() {
    assert_variable_retry_terminalizes(
        VariableRetryPacingLane::Ready,
        crate::native_output::scanout::VariableRetryRejection::TestOnly,
    );
}

#[test]
fn variable_real_submit_rejection_terminalizes_deferred_ready_owner() {
    assert_variable_retry_terminalizes(
        VariableRetryPacingLane::Ready,
        crate::native_output::scanout::VariableRetryRejection::RealSubmit,
    );
}

#[test]
fn worker_queue_full_preserves_deferred_ready_ownership() {
    use crate::native_output::presentation::ledger::OutputTransactionState;

    let mut fixture = variable_retry_fixture(VariableRetryPacingLane::Ready);
    let owner = fixture.owner;
    let pacing_ticket = fixture
        .frame_pacing
        .reserve_worker_submission(true)
        .expect("READY pacing owner can reserve a worker attempt");
    assert!(pacing_ticket.is_some());
    let outcome =
        crate::native_output::runtime::presentation_worker::defer_explicit_worker_submission(
            &mut fixture.frame_pacing,
            pacing_ticket,
            crate::native_output::kms_worker::KmsWorkerAdmissionError::QueueFull,
        )
        .expect("queue-full worker admission defers the same READY frame");

    assert!(matches!(
        outcome,
        crate::native_output::runtime::presentation_worker::ExplicitReadySubmissionOutcome::AdmissionDeferred {
            reason: crate::native_output::kms_worker::KmsWorkerAdmissionError::QueueFull,
        }
    ));
    assert_eq!(fixture.swapchain.ready_identity(), Some(owner));
    assert_eq!(fixture.frame_pacing.active, None);
    assert!(fixture.frame_pacing.ready.is_some());
    assert_eq!(
        fixture.frame_pacing.unsubmitted_physical_identity(),
        Some(owner)
    );
    assert_eq!(
        fixture.scene_history.ready_frame_identity(),
        Some((owner.frame_id, owner.render_generation))
    );
    assert!(fixture.frame_scheduler.ready_frame_queued());
    assert!(
        fixture
            .output_transactions
            .active_transaction_ids()
            .contains(&owner.transaction_id)
    );
    assert!(matches!(
        fixture
            .output_transactions
            .transaction(owner.transaction_id)
            .unwrap()
            .state(),
        OutputTransactionState::Ready { .. }
    ));
}

#[test]
fn queued_worker_variable_rejection_settles_its_ticket_once() {
    use crate::native_output::presentation::ledger::{
        OutputTransactionDropReason, OutputTransactionState, OutputTransactionTerminal,
    };
    use crate::native_output::runtime::presentation_transactions::settle_dropped_output_transaction;
    use oblivion_one::native::presentation_deadline::MonotonicTimestampNs;

    let mut fixture = variable_retry_fixture(VariableRetryPacingLane::Ready);
    let owner = fixture.owner;
    let token = PageFlipToken::new(900).expect("worker token is nonzero");
    let pacing_ticket = fixture
        .frame_pacing
        .reserve_worker_submission(true)
        .expect("worker reservation uses the READY pacing owner");
    fixture
        .output_transactions
        .mark_queued(
            owner.transaction_id,
            fixture.output_epoch.get(),
            MonotonicTimestampNs::new(3),
        )
        .unwrap();
    let in_fence = fixture
        .swapchain
        .take_ready_for_worker(token, MonotonicTimestampNs::new(3))
        .unwrap()
        .0;
    fixture
        .scene_history
        .queue_submission_or_error(token.get())
        .unwrap();
    fixture.frame_scheduler.consume_ready_frame();
    let mut completion_fence = Some(in_fence);

    // This is the existing asynchronous worker rejection order: submitted
    // scene and pacing ticket are retired by the worker boundary, then the
    // physical job is suspended and its transaction is safely abandoned.
    fixture.scene_history.discard_submission(token.get());
    assert!(fixture.frame_pacing.cancel_worker_submission(pacing_ticket));
    fixture
        .swapchain
        .suspend_abandon_worker_queued_with_completion_fence(token, &mut completion_fence)
        .unwrap();
    settle_dropped_output_transaction(
        &mut fixture.output_transactions,
        owner.transaction_id,
        OutputTransactionDropReason::SafeAbandonment,
        MonotonicTimestampNs::new(4),
        |obligations| {
            crate::native_output::runtime::presentation_transactions::restore_presentation_feedback_obligation(
                &mut fixture.server,
                obligations,
            );
            let batch_id = obligations
                .frame_batch_id()
                .expect("worker Variable transaction retains its frame batch");
            fixture.server.restore_frame_batch_after_render_failure(batch_id);
            Ok(())
        },
    )
    .unwrap();
    fixture
        .frame_pacing
        .queue_visual(6, owner.render_generation + 1);
    fixture.frame_scheduler.queue_visual_work();

    assert_eq!(fixture.frame_pacing.unsubmitted_physical_identity(), None);
    assert_eq!(fixture.scene_history.submitted_frame_id(token.get()), None);
    assert!(!fixture.frame_scheduler.ready_frame_queued());
    assert!(fixture.swapchain.contains_suspended_frame(owner));
    assert!(
        fixture
            .output_transactions
            .active_transaction_ids()
            .is_empty()
    );
    assert!(matches!(
        fixture
            .output_transactions
            .transaction_including_terminal(owner.transaction_id)
            .unwrap()
            .state(),
        OutputTransactionState::Terminal(OutputTransactionTerminal::Dropped {
            reason: OutputTransactionDropReason::SafeAbandonment,
            ..
        })
    ));
    assert!(
        !fixture.frame_pacing.cancel_worker_submission(pacing_ticket),
        "the worker pacing ticket is already settled and cannot be consumed twice"
    );
}

fn worker_composited_job() -> (KmsCommitJob, AtomicOutputSwapchain) {
    let transaction_id = OutputTransactionId::new(NonZeroU64::new(41).unwrap());
    let token = PageFlipToken::new(41).unwrap();
    let target = worker_test_target();
    let slot = OutputSlotId::new(0).unwrap();
    let transaction = Arc::new(
        OutputTransaction::composited(
            oblivion_one::core::OutputId::from_raw(1).expect("test output id"),
            transaction_id,
            1,
            MonotonicTimestampNs::new(0),
            target,
            NativeOutputPacingMode::PredictiveTriple,
            41,
            1,
            1,
            slot,
            42,
            None,
            worker_test_frame_batch(41),
        )
        .unwrap(),
    );
    let kind = AtomicCommitKind::CompositedPrimary {
        transaction_id,
        frame_id: 41,
        framebuffer_id: 42,
    };
    let owners = KmsBundleOwners::for_transaction(kind, transaction, None, None).unwrap();
    let job = KmsCommitJob {
        bundle_id: KmsCommitBundleId::from_pageflip_token(token),
        owners,
        output_id: oblivion_one::core::OutputId::from_raw(1).expect("nonzero output id"),
        transaction_id,
        token,
        output_epoch: crate::native_output::OutputEpochId::initial(),
        crtc_id: 7,
        kind,
        reservation:
            crate::native_output::presentation::transaction::FramePresentationReservation::Fixed(
                target,
            ),
        presentation_timing:
            crate::native_output::presentation::kms_timing::KmsPresentationTiming::Fixed(
                crate::native_output::presentation::kms_timing::KmsSubmitWindow::try_new(
                    target.presentation_time.get(),
                    target.submit_not_before().get(),
                    0,
                    0,
                )
                .unwrap(),
            ),
        validation_base: KmsValidationBase::Presented {
            output_id: oblivion_one::core::OutputId::from_raw(1).expect("nonzero output id"),
            snapshot: PresentedPlaneSnapshot::legacy(None),
            output_generation: 1,
            crtc_id: 7,
        },
        queued_at: MonotonicTimestampNs::new(0),
        primary: KmsPrimaryUpdate::Framebuffer {
            framebuffer: FramebufferId::new(42).unwrap(),
            in_fence: None,
            request_out_fence: false,
        },
        cursor: KmsCursorUpdate::Unchanged,
        cursor_delivery: PresentedCursorDelivery::Hidden,
        primary_cursor_presentation: KmsPrimaryCursorPresentation::Preserve,
        cursor_pin: None,
        direct_primary_lease: None,
        test_only_duration_ns: None,
        pacing_ticket: None,
        test_policy: KmsCommitTestPolicy::from_primary(KmsTestOnlyPolicy::Skip),
        ready_submit: true,
    };
    let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
        OutputSlotSet::new([
            OutputSlotId::new(0).unwrap(),
            OutputSlotId::new(1).unwrap(),
            OutputSlotId::new(2).unwrap(),
        ])
        .unwrap(),
        slot,
        1,
    )
    .unwrap();
    swapchain.set_current_framebuffer_id(FramebufferId::new(42).unwrap());
    (job, swapchain)
}

#[test]
fn worker_presented_primary_uses_current_swapchain_identity() {
    let (job, swapchain) = worker_composited_job();
    let presented = presented_primary_from_worker_job(&job, Some(&swapchain));
    assert!(matches!(
        presented,
        Some(PresentedPrimaryAssignment::Composed {
            transaction_id,
            token,
            slot,
            framebuffer_id,
            pool_generation,
            presentation_serial,
            ..
        }) if transaction_id == job.transaction_id
            && token == job.token
            && slot == swapchain.current()
            && framebuffer_id == 42
            && pool_generation == swapchain.pool_generation()
            && presentation_serial == swapchain.presentation_serial()
    ));

    let mut wrong = swapchain;
    wrong.set_current_framebuffer_id(FramebufferId::new(43).unwrap());
    assert!(presented_primary_from_worker_job(&job, Some(&wrong)).is_none());
}

#[test]
fn primary_software_presentation_wins_over_disabled_cursor_owner() {
    let software = PresentedCursorState {
        revision: CursorRevision::initial().advance_image(),
        coupling: CursorCoupling::EmbeddedInPrimary,
        delivery: PresentedCursorDelivery::Software,
        framebuffer_id: None,
        image_generation: None,
        source: None,
        visible: true,
        output_position: CursorPlanePoint { x: 200, y: 300 },
        hotspot: CursorPlanePoint { x: 4, y: 5 },
    };
    let old_hardware = PresentedCursorState {
        revision: CursorRevision::initial(),
        coupling: CursorCoupling::IndependentPlane,
        delivery: PresentedCursorDelivery::Hardware,
        framebuffer_id: Some(91),
        image_generation: Some(1),
        source: None,
        visible: true,
        output_position: CursorPlanePoint { x: 10, y: 20 },
        hotspot: CursorPlanePoint { x: 1, y: 2 },
    };

    assert_eq!(
        select_cursor_promotion(
            KmsPrimaryCursorPresentation::Promote(software),
            Some(old_hardware),
        ),
        Some(software)
    );
}

#[test]
fn primary_pageflip_uses_frozen_cursor_presentation_metadata() {
    let frozen_state = AtomicCursorVisualState::hidden(64, 64);
    let frozen = PresentedCursorState::from_atomic_with_delivery(
        CursorRevision::initial().advance_image(),
        CursorCoupling::EmbeddedInPrimary,
        crate::native_output::presentation::plane::PresentedCursorDelivery::Software,
        &frozen_state,
    );
    let expected = frozen;

    assert_eq!(
        frozen_primary_cursor_presentation(KmsPrimaryCursorPresentation::Promote(frozen)),
        Some(expected)
    );
}

#[test]
fn preserved_primary_cursor_does_not_fabricate_a_new_presentation() {
    assert_eq!(
        frozen_primary_cursor_presentation(KmsPrimaryCursorPresentation::Preserve),
        None
    );
}

#[test]
fn software_primary_metadata_freezes_revision_before_desired_advances() {
    let mut cursor = crate::native_output::output::test_cursor_for_worker();
    cursor.set_position(11, 22);
    let frozen_state = cursor.desired().clone();
    let frozen_revision = cursor.desired_revision();
    let metadata =
        crate::native_output::runtime::presentation_cursor::freeze_primary_cursor_presentation(
            crate::native_output::presentation::plane::PresentedCursorDelivery::Hidden,
            crate::native_output::presentation::plane::PresentedCursorDelivery::Software,
            Some(&frozen_state),
            Some(&cursor),
            7,
        );

    cursor.set_position(900, 901);
    let KmsPrimaryCursorPresentation::Promote(frozen) = metadata else {
        panic!("software primary must carry frozen cursor metadata");
    };
    assert_eq!(frozen.revision, frozen_revision);
    assert_eq!(frozen.output_position.x, 11);
    assert_eq!(frozen.output_position.y, 22);
    assert_eq!(frozen.delivery, PresentedCursorDelivery::Software);
}

fn queue_fixture_ready_as_worker(
    fixture: &mut VariableRetryFixture,
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    token: PageFlipToken,
    crtc_id: u32,
) -> (
    KmsCommitWorkerHandle,
    crate::native_output::pacing::WorkerPacingTicket,
) {
    use crate::native_output::kms_worker::{KmsCursorUpdate, KmsPrimaryUpdate};
    use crate::native_output::presentation::kms_timing::KmsPresentationTiming;
    use crate::native_output::presentation::plane::{KmsCommitBundleId, PresentedPlaneSnapshot};
    use crate::native_output::presentation::transaction::OutputTransaction;
    use oblivion_one::native::kms::FramebufferId;
    use oblivion_one::native::presentation_deadline::MonotonicTimestampNs;

    let owner = fixture.owner;
    let now = MonotonicTimestampNs::new(3);
    let pacing_ticket = fixture
        .frame_pacing
        .reserve_worker_submission(true)
        .expect("READY pacing owner reserves a worker attempt")
        .expect("pacing is enabled in the fixture");
    fixture
        .output_transactions
        .mark_queued(owner.transaction_id, fixture.output_epoch.get(), now)
        .unwrap();
    let in_fence = fixture
        .swapchain
        .take_ready_for_worker(token, now)
        .unwrap()
        .0;
    fixture
        .scene_history
        .queue_submission_or_error(token.get())
        .unwrap();
    fixture.frame_scheduler.consume_ready_frame();

    let kind = AtomicCommitKind::CompositedPrimary {
        transaction_id: owner.transaction_id,
        frame_id: owner.frame_id,
        framebuffer_id: owner.framebuffer_id.get(),
    };
    atomic_commit_arbiter
        .reserve_worker_queued(token, fixture.output_epoch.get(), crtc_id, kind, now.get())
        .unwrap();
    let worker_transaction = Arc::new(
        OutputTransaction::composited_variable_with_direct_equivalence(
            owner.output_id,
            owner.transaction_id,
            fixture.output_epoch.get(),
            now,
            match owner.reservation {
                crate::native_output::presentation::transaction::FramePresentationReservation::Variable(
                    reservation,
                ) => reservation,
                crate::native_output::presentation::transaction::FramePresentationReservation::Fixed(
                    _,
                ) => panic!("worker fixture must use a variable reservation"),
                crate::native_output::presentation::transaction::FramePresentationReservation::DeferredO1(
                    _,
                ) => panic!("worker fixture must not use a deferred O1 reservation"),
            },
            crate::native_output::DrmContentType::Graphics,
            oblivion_one::native::scheduler::NativeOutputPacingMode::ReactiveDouble,
            owner.frame_id,
            owner.render_generation,
            owner.pool_generation,
            owner.slot,
            owner.framebuffer_id.get(),
            None,
            owner.protocol_batch_id,
            None,
        )
        .unwrap(),
    );
    let owners = KmsBundleOwners::for_transaction(kind, worker_transaction, None, None).unwrap();
    let earliest = MonotonicTimestampNs::new(1_006_060_606);
    let job = KmsCommitJob {
        bundle_id: KmsCommitBundleId::from_pageflip_token(token),
        owners,
        output_id: owner.output_id,
        transaction_id: owner.transaction_id,
        token,
        output_epoch: fixture.output_epoch,
        crtc_id,
        kind,
        reservation: owner.reservation,
        presentation_timing: KmsPresentationTiming::variable(earliest.get()),
        validation_base: KmsValidationBase::Presented {
            output_id: owner.output_id,
            snapshot: PresentedPlaneSnapshot::legacy(None),
            output_generation: fixture.output_epoch.get(),
            crtc_id,
        },
        queued_at: now,
        primary: KmsPrimaryUpdate::Framebuffer {
            framebuffer: FramebufferId::new(owner.framebuffer_id.get()).unwrap(),
            in_fence: Some(in_fence),
            request_out_fence: false,
        },
        cursor: KmsCursorUpdate::Unchanged,
        cursor_delivery: PresentedCursorDelivery::Hidden,
        primary_cursor_presentation: KmsPrimaryCursorPresentation::Preserve,
        cursor_pin: None,
        direct_primary_lease: None,
        test_only_duration_ns: None,
        pacing_ticket: Some(pacing_ticket),
        test_policy: KmsCommitTestPolicy::from_primary(KmsTestOnlyPolicy::Skip),
        ready_submit: true,
    };
    (
        KmsCommitWorkerHandle::with_queued_job_for_test(job),
        pacing_ticket,
    )
}

fn append_ready_successor(
    fixture: &mut VariableRetryFixture,
    opportunity_sequence: u64,
    anchor_ns: u64,
) -> OutputFrameIdentitySnapshot {
    append_ready_successor_with_render_fence(
        fixture,
        opportunity_sequence,
        anchor_ns,
        signaled_retry_render_fence(),
    )
}

fn append_ready_successor_with_render_fence(
    fixture: &mut VariableRetryFixture,
    opportunity_sequence: u64,
    anchor_ns: u64,
    render_fence: crate::egl_renderer::native_fence::NativeRenderFence,
) -> OutputFrameIdentitySnapshot {
    use crate::native_output::NativeCursorDamageBounds;
    use crate::native_output::presentation::kms_timing::KmsPresentationTiming;
    use crate::native_output::presentation::plane::{
        FrozenCursorTestPolicy, FrozenPrimaryCursorPlan, FrozenPrimaryCursorPresentation,
    };
    use crate::native_output::presentation::transaction::OutputTransaction;
    use crate::native_output::runtime::scene_history::NativeFrameSceneSnapshot;
    use oblivion_one::compositor::PresentationFrameSnapshot;
    use oblivion_one::native::buffering::PresentationOpportunityId;
    use oblivion_one::native::presentation_deadline::MonotonicTimestampNs;
    use oblivion_one::native::scheduler::NativeOutputPacingMode;
    use oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot;

    let slot = fixture.swapchain.acquire_render_slot().unwrap();
    fixture
        .swapchain
        .prepare_ready_for_test(
            slot,
            render_fence,
            FrozenPrimaryCursorPlan {
                delivery: PresentedCursorDelivery::Hidden,
                primary_presentation: FrozenPrimaryCursorPresentation::Preserve,
                cursor_test_policy: FrozenCursorTestPolicy::Required,
            },
            None,
        )
        .unwrap();
    let anchor = MonotonicTimestampNs::new(anchor_ns);
    let earliest = MonotonicTimestampNs::new(anchor.get() + 6_060_606);
    let reservation = variable_reservation(
        PresentationOpportunityId::new(1, opportunity_sequence),
        anchor,
        earliest,
        None,
    );
    fixture
        .swapchain
        .set_ready_variable_presentation_for_test(
            reservation,
            KmsPresentationTiming::variable(earliest.get()),
        )
        .unwrap();
    let owner = fixture.swapchain.ready_identity().unwrap();
    let batch_id = fixture.server.take_frame_batch_for_render(owner.frame_id);
    assert_eq!(batch_id, owner.protocol_batch_id);
    fixture.server.note_frame_callbacks_deferred_ready(batch_id);

    fixture
        .frame_pacing
        .queue_visual(5, owner.render_generation);
    fixture
        .frame_pacing
        .note_render_started(NativeOutputPacingMode::ReactiveDouble, false)
        .unwrap();
    fixture
        .frame_pacing
        .bind_physical_output_frame(owner)
        .unwrap();
    fixture.frame_pacing.note_ready_frame(6, true);
    fixture.frame_scheduler.queue_visual_work();
    fixture.frame_scheduler.consume_visual_work();
    fixture.frame_scheduler.note_ready_frame(None);

    assert!(
        fixture
            .scene_history
            .replace_ready(NativeFrameSceneSnapshot {
                output_id: owner.output_id,
                frame_id: owner.frame_id,
                render_generation: owner.render_generation,
                scene: NativeSceneSnapshot::from_surfaces_with_scene_nodes(
                    &[],
                    &[],
                    Vec::new(),
                    &[]
                ),
                cursor_damage: NativeCursorDamageBounds::default(),
                presentation: PresentationFrameSnapshot::empty_for_output(owner.output_id),
                lifecycle: LifecycleFrameSnapshot::default(),
                canonical_scene_evidence: None,
                window_exit_evidence: Vec::new(),
            })
    );
    let transaction = OutputTransaction::composited_variable_with_direct_equivalence(
        owner.output_id,
        owner.transaction_id,
        fixture.output_epoch.get(),
        MonotonicTimestampNs::new(7),
        reservation,
        crate::native_output::DrmContentType::Graphics,
        NativeOutputPacingMode::ReactiveDouble,
        owner.frame_id,
        owner.render_generation,
        owner.pool_generation,
        owner.slot,
        owner.framebuffer_id.get(),
        None,
        batch_id,
        None,
    )
    .unwrap();
    fixture.output_transactions.insert(transaction).unwrap();
    fixture
        .output_transactions
        .mark_ready(owner.transaction_id, MonotonicTimestampNs::new(8))
        .unwrap();
    owner
}

#[test]
fn direct_entry_retires_worker_and_ready_successors_before_invalidating_scene_baseline() {
    use crate::native_output::presentation::ledger::{
        OutputTransactionDropReason, OutputTransactionState, OutputTransactionTerminal,
    };
    use oblivion_one::native::presentation_deadline::MonotonicTimestampNs;

    let mut mismatch = variable_retry_fixture(VariableRetryPacingLane::Ready);
    assert!(
        mismatch
            .scene_history
            .discard_ready_if_identity(mismatch.owner.frame_id, mismatch.owner.render_generation)
    );
    let mismatch_error = validate_reconfiguration_frame_ownership_for_swapchain(
        &mismatch.swapchain,
        mismatch.output_epoch,
        &mismatch.scene_history,
        &mismatch.frame_pacing,
        &mismatch.frame_scheduler,
        &mismatch.output_transactions,
    )
    .expect_err("the strict validator rejects physical READY without scene READY");
    assert!(
        mismatch_error
            .to_string()
            .contains("unsubmitted READY has no matching scene-history READY owner")
    );

    let mut fixture = variable_retry_fixture(VariableRetryPacingLane::Ready);
    let output_epoch = fixture.output_epoch;
    let crtc_id = 7;
    let direct_token = PageFlipToken::new(700).unwrap();
    let direct_transaction_id = OutputTransactionId::new(NonZeroU64::new(700).unwrap());
    let direct_kind = AtomicCommitKind::DirectPrimary {
        transaction_id: direct_transaction_id,
        direct_token,
        framebuffer_id: 90,
    };
    let mut atomic_commit_arbiter = AtomicCommitArbiter::new();
    atomic_commit_arbiter
        .reserve_worker_queued(direct_token, output_epoch.get(), crtc_id, direct_kind, 1)
        .unwrap();
    atomic_commit_arbiter
        .mark_kernel_submitted(direct_token, 2, 3)
        .unwrap();

    let worker_token = PageFlipToken::new(701).unwrap();
    let (worker, worker_pacing_ticket) = queue_fixture_ready_as_worker(
        &mut fixture,
        &mut atomic_commit_arbiter,
        worker_token,
        crtc_id,
    );
    let ready_owner = append_ready_successor(&mut fixture, 2, 2_000_000_000);
    assert_eq!(
        fixture
            .swapchain
            .worker_queued_identity()
            .map(|identity| identity.frame),
        Some(fixture.owner)
    );
    assert_eq!(fixture.swapchain.ready_identity(), Some(ready_owner));
    assert_eq!(
        fixture.scene_history.submitted_frame_id(worker_token.get()),
        Some(fixture.owner.frame_id)
    );
    assert_eq!(
        fixture.scene_history.ready_frame_identity(),
        Some((ready_owner.frame_id, ready_owner.render_generation))
    );
    assert_eq!(
        fixture.frame_pacing.unsubmitted_physical_identity(),
        Some(ready_owner)
    );
    assert!(fixture.frame_scheduler.ready_frame_queued());
    assert!(
        fixture
            .output_transactions
            .active_transaction_ids()
            .contains(&fixture.owner.transaction_id)
    );
    assert!(
        fixture
            .output_transactions
            .active_transaction_ids()
            .contains(&ready_owner.transaction_id)
    );
    let direct_completion =
        atomic_commit_arbiter.complete(direct_token, output_epoch.get(), crtc_id);
    assert!(matches!(
        direct_completion,
        AtomicCommitCompletion::Completed { kind, .. } if kind == direct_kind
    ));

    let mut atomic_cursor = None;
    let mut cursor_output_arbitration = NativeCursorOutputArbitration::default();
    let mut emergency_worker_jobs = Vec::new();
    let mut queued_redraw_requested = false;
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut output_render_fence_token = None;
    let retirement = retire_direct_entry_future_composited_owners(
        &mut SwapchainSuspendedRecovery {
            swapchain: &mut fixture.swapchain,
        },
        &mut event_loop,
        &mut output_render_fence_token,
        Some(&worker),
        crtc_id,
        output_epoch,
        &mut fixture.scene_history,
        &mut fixture.frame_pacing,
        &mut fixture.frame_scheduler,
        &mut atomic_cursor,
        &mut cursor_output_arbitration,
        &mut atomic_commit_arbiter,
        &mut fixture.server,
        &mut fixture.output_transactions,
        &mut emergency_worker_jobs,
        &mut queued_redraw_requested,
        direct_transaction_id,
        direct_token,
        NativePerfLogger::from_env(),
    )
    .unwrap();
    fixture
        .scene_history
        .invalidate_presented_composited_baseline();
    assert_eq!(fixture.scene_history.presented_frame_id(), None);

    assert_eq!(retirement.worker_frame_id, Some(fixture.owner.frame_id));
    assert_eq!(retirement.ready_frame_id, Some(ready_owner.frame_id));
    assert_eq!(fixture.swapchain.worker_queued_identity(), None);
    assert_eq!(fixture.swapchain.ready_identity(), None);
    assert!(!fixture.swapchain.contains_suspended_frame(fixture.owner));
    assert!(!fixture.swapchain.contains_suspended_frame(ready_owner));
    assert!(!fixture.swapchain.has_suspended_ownership());
    assert!(output_render_fence_token.is_none());
    assert_eq!(
        fixture.scene_history.submitted_frame_id(worker_token.get()),
        None
    );
    assert_eq!(fixture.scene_history.ready_frame_identity(), None);
    assert_eq!(fixture.frame_pacing.ready, None);
    assert_eq!(fixture.frame_pacing.unsubmitted_physical_identity(), None);
    assert!(!fixture.frame_pacing.active_worker_owned());
    assert!(!fixture.frame_scheduler.ready_frame_queued());
    assert!(fixture.frame_scheduler.visual_work_queued());
    assert!(queued_redraw_requested);
    assert!(emergency_worker_jobs.is_empty());
    assert!(atomic_commit_arbiter.worker_queued_kind().is_none());
    assert!(
        fixture
            .output_transactions
            .active_transaction_ids()
            .is_empty()
    );
    for owner in [fixture.owner, ready_owner] {
        assert!(matches!(
            fixture
                .output_transactions
                .transaction_including_terminal(owner.transaction_id)
                .unwrap()
                .state(),
            OutputTransactionState::Terminal(OutputTransactionTerminal::Dropped {
                reason: OutputTransactionDropReason::SafeAbandonment,
                ..
            })
        ));
    }
    assert!(
        !fixture
            .frame_pacing
            .cancel_worker_submission(Some(worker_pacing_ticket))
    );
    assert_eq!(fixture.output_epoch, output_epoch);

    assert!(fixture.swapchain.render_target_available_for_limit(2));

    validate_reconfiguration_frame_ownership_for_swapchain(
        &fixture.swapchain,
        output_epoch,
        &fixture.scene_history,
        &fixture.frame_pacing,
        &fixture.frame_scheduler,
        &fixture.output_transactions,
    )
    .unwrap();
    assert!(fixture.frame_scheduler.visual_work_queued());

    let fresh_owner = append_ready_successor(&mut fixture, 3, 3_000_000_000);
    assert_eq!(fixture.scene_history.presented_frame_id(), None);
    assert_eq!(fixture.swapchain.ready_identity(), Some(fresh_owner));
    assert_eq!(
        fixture.scene_history.ready_frame_identity(),
        Some((fresh_owner.frame_id, fresh_owner.render_generation))
    );
    assert_eq!(
        fixture.frame_pacing.unsubmitted_physical_identity(),
        Some(fresh_owner)
    );
    assert!(fixture.frame_scheduler.ready_frame_queued());
    assert!(
        fixture
            .output_transactions
            .active_transaction_ids()
            .contains(&fresh_owner.transaction_id)
    );
    validate_reconfiguration_frame_ownership_for_swapchain(
        &fixture.swapchain,
        output_epoch,
        &fixture.scene_history,
        &fixture.frame_pacing,
        &fixture.frame_scheduler,
        &fixture.output_transactions,
    )
    .unwrap();

    let fresh_token = PageFlipToken::new(702).unwrap();
    let submitted_at = MonotonicTimestampNs::new(9);
    let _fresh_pacing_ticket = fixture
        .frame_pacing
        .reserve_worker_submission(true)
        .expect("fresh READY frame reserves a worker submit")
        .expect("pacing is enabled in the fixture");
    fixture
        .output_transactions
        .mark_queued(fresh_owner.transaction_id, output_epoch.get(), submitted_at)
        .unwrap();
    fixture
        .swapchain
        .take_ready_for_worker(fresh_token, submitted_at)
        .unwrap();
    fixture
        .scene_history
        .queue_submission_or_error(fresh_token.get())
        .unwrap();
    fixture.frame_scheduler.consume_ready_frame();
    let fresh_kind = AtomicCommitKind::CompositedPrimary {
        transaction_id: fresh_owner.transaction_id,
        frame_id: fresh_owner.frame_id,
        framebuffer_id: fresh_owner.framebuffer_id.get(),
    };
    atomic_commit_arbiter
        .reserve_worker_queued(
            fresh_token,
            output_epoch.get(),
            crtc_id,
            fresh_kind,
            submitted_at.get(),
        )
        .unwrap();
    assert_eq!(
        fixture
            .swapchain
            .worker_queued_identity()
            .map(|identity| identity.frame),
        Some(fresh_owner)
    );
    assert_eq!(
        fixture.scene_history.submitted_frame_id(fresh_token.get()),
        Some(fresh_owner.frame_id)
    );
    assert!(matches!(
        fixture
            .output_transactions
            .transaction(fresh_owner.transaction_id)
            .unwrap()
            .state(),
        OutputTransactionState::Queued { .. }
    ));
    assert_eq!(atomic_commit_arbiter.worker_queued_kind(), Some(fresh_kind));
    assert!(!fixture.frame_scheduler.ready_frame_queued());
    assert_eq!(fixture.output_epoch, output_epoch);
}

#[test]
fn direct_entry_waits_for_each_suspended_successor_fence_before_reconfiguration() {
    use crate::native_output::presentation::ledger::{
        OutputTransactionDropReason, OutputTransactionState, OutputTransactionTerminal,
    };

    let (worker_fence, worker_signal) = pageflip_test_controllable_render_fence();
    let mut fixture =
        variable_retry_fixture_with_render_fence(VariableRetryPacingLane::Ready, worker_fence);
    let output_epoch = fixture.output_epoch;
    let crtc_id = 7;
    let direct_token = PageFlipToken::new(750).unwrap();
    let direct_transaction_id = OutputTransactionId::new(NonZeroU64::new(750).unwrap());
    let direct_kind = AtomicCommitKind::DirectPrimary {
        transaction_id: direct_transaction_id,
        direct_token,
        framebuffer_id: 95,
    };
    let mut atomic_commit_arbiter = AtomicCommitArbiter::new();
    atomic_commit_arbiter
        .reserve_worker_queued(direct_token, output_epoch.get(), crtc_id, direct_kind, 1)
        .unwrap();
    atomic_commit_arbiter
        .mark_kernel_submitted(direct_token, 2, 3)
        .unwrap();

    let worker_token = PageFlipToken::new(751).unwrap();
    let (worker, worker_pacing_ticket) = queue_fixture_ready_as_worker(
        &mut fixture,
        &mut atomic_commit_arbiter,
        worker_token,
        crtc_id,
    );
    let (ready_fence, ready_signal) = pageflip_test_controllable_render_fence();
    let ready_owner =
        append_ready_successor_with_render_fence(&mut fixture, 2, 2_000_000_000, ready_fence);
    let direct_completion =
        atomic_commit_arbiter.complete(direct_token, output_epoch.get(), crtc_id);
    assert!(matches!(
        direct_completion,
        AtomicCommitCompletion::Completed { kind, .. } if kind == direct_kind
    ));

    let mut atomic_cursor = None;
    let mut cursor_output_arbitration = NativeCursorOutputArbitration::default();
    let mut emergency_worker_jobs = Vec::new();
    let mut queued_redraw_requested = false;
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut output_render_fence_token = None;
    let retirement = retire_direct_entry_future_composited_owners(
        &mut SwapchainSuspendedRecovery {
            swapchain: &mut fixture.swapchain,
        },
        &mut event_loop,
        &mut output_render_fence_token,
        Some(&worker),
        crtc_id,
        output_epoch,
        &mut fixture.scene_history,
        &mut fixture.frame_pacing,
        &mut fixture.frame_scheduler,
        &mut atomic_cursor,
        &mut cursor_output_arbitration,
        &mut atomic_commit_arbiter,
        &mut fixture.server,
        &mut fixture.output_transactions,
        &mut emergency_worker_jobs,
        &mut queued_redraw_requested,
        direct_transaction_id,
        direct_token,
        NativePerfLogger::from_env(),
    )
    .unwrap();
    fixture
        .scene_history
        .invalidate_presented_composited_baseline();
    assert_eq!(fixture.scene_history.presented_frame_id(), None);

    assert_eq!(retirement.worker_frame_id, Some(fixture.owner.frame_id));
    assert_eq!(retirement.ready_frame_id, Some(ready_owner.frame_id));
    assert!(fixture.swapchain.has_suspended_ownership());
    assert!(output_render_fence_token.is_some());
    assert!(!fixture.swapchain.render_target_available_for_limit(2));
    assert!(fixture.swapchain.acquire_render_slot().is_err());
    assert_eq!(fixture.swapchain.worker_queued_identity(), None);
    assert_eq!(fixture.swapchain.ready_identity(), None);
    assert_eq!(
        fixture.scene_history.submitted_frame_id(worker_token.get()),
        None
    );
    assert_eq!(fixture.scene_history.ready_frame_identity(), None);
    assert_eq!(fixture.frame_pacing.ready, None);
    assert_eq!(fixture.frame_pacing.unsubmitted_physical_identity(), None);
    assert!(!fixture.frame_pacing.active_worker_owned());
    assert!(!fixture.frame_scheduler.ready_frame_queued());
    assert!(fixture.frame_scheduler.visual_work_queued());
    assert!(queued_redraw_requested);
    assert!(emergency_worker_jobs.is_empty());
    assert!(atomic_commit_arbiter.worker_queued_kind().is_none());
    assert!(
        fixture
            .output_transactions
            .active_transaction_ids()
            .is_empty()
    );
    for owner in [fixture.owner, ready_owner] {
        assert!(matches!(
            fixture
                .output_transactions
                .transaction_including_terminal(owner.transaction_id)
                .unwrap()
                .state(),
            OutputTransactionState::Terminal(OutputTransactionTerminal::Dropped {
                reason: OutputTransactionDropReason::SafeAbandonment,
                ..
            })
        ));
    }
    assert!(
        !fixture
            .frame_pacing
            .cancel_worker_submission(Some(worker_pacing_ticket))
    );

    let first_token = output_render_fence_token.unwrap();
    signal_pageflip_test_fence(&worker_signal);
    let first_wakeup = event_loop.wait().unwrap();
    assert!(first_wakeup.reasons.output_render_fence());
    let first_progress = settle_suspended_output_fence_wake(
        &mut SwapchainSuspendedRecovery {
            swapchain: &mut fixture.swapchain,
        },
        &mut event_loop,
        &mut output_render_fence_token,
    )
    .unwrap();
    assert_eq!(first_progress, SuspendedOutputRecovery::Waiting);
    let second_token = output_render_fence_token.expect("READY successor fence is re-armed");
    assert_ne!(first_token, second_token);
    assert!(fixture.swapchain.has_suspended_ownership());
    assert!(!fixture.swapchain.render_target_available_for_limit(2));

    signal_pageflip_test_fence(&ready_signal);
    let second_wakeup = event_loop.wait().unwrap();
    assert!(second_wakeup.reasons.output_render_fence());
    let recovered = settle_suspended_output_fence_wake(
        &mut SwapchainSuspendedRecovery {
            swapchain: &mut fixture.swapchain,
        },
        &mut event_loop,
        &mut output_render_fence_token,
    )
    .unwrap();
    assert_eq!(recovered, SuspendedOutputRecovery::Recovered);
    assert!(output_render_fence_token.is_none());
    assert!(!fixture.swapchain.has_suspended_ownership());
    assert!(fixture.swapchain.render_target_available_for_limit(2));
    assert_eq!(fixture.swapchain.worker_queued_identity(), None);
    assert_eq!(fixture.swapchain.ready_identity(), None);
    assert_eq!(
        fixture.scene_history.submitted_frame_id(worker_token.get()),
        None
    );
    assert_eq!(fixture.scene_history.ready_frame_identity(), None);
    assert_eq!(fixture.frame_pacing.ready, None);
    assert_eq!(fixture.frame_pacing.unsubmitted_physical_identity(), None);
    assert!(!fixture.frame_pacing.active_worker_owned());
    assert!(!fixture.frame_scheduler.ready_frame_queued());
    assert!(
        fixture
            .output_transactions
            .active_transaction_ids()
            .is_empty()
    );
    assert_eq!(fixture.output_epoch, output_epoch);
    validate_reconfiguration_frame_ownership_for_swapchain(
        &fixture.swapchain,
        output_epoch,
        &fixture.scene_history,
        &fixture.frame_pacing,
        &fixture.frame_scheduler,
        &fixture.output_transactions,
    )
    .unwrap();

    let fresh_owner = append_ready_successor(&mut fixture, 3, 3_000_000_000);
    assert_eq!(fixture.scene_history.presented_frame_id(), None);
    assert_eq!(fixture.swapchain.ready_identity(), Some(fresh_owner));
    assert_eq!(
        fixture.scene_history.ready_frame_identity(),
        Some((fresh_owner.frame_id, fresh_owner.render_generation))
    );
    assert!(fixture.frame_scheduler.ready_frame_queued());
    assert!(
        fixture
            .output_transactions
            .active_transaction_ids()
            .contains(&fresh_owner.transaction_id)
    );
    validate_reconfiguration_frame_ownership_for_swapchain(
        &fixture.swapchain,
        output_epoch,
        &fixture.scene_history,
        &fixture.frame_pacing,
        &fixture.frame_scheduler,
        &fixture.output_transactions,
    )
    .unwrap();
}

#[test]
fn suspended_output_recovery_advances_one_fence_at_a_time() {
    let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
        explicit_slot_set(),
        OutputSlotId::new(0).unwrap(),
        74,
    )
    .unwrap();
    let worker_slot = swapchain.acquire_render_slot().unwrap();
    let (worker_fence, worker_signal) = pageflip_test_controllable_render_fence();
    swapchain
        .finish_render(worker_slot, 1, worker_fence)
        .unwrap();
    let worker_token = PageFlipToken::new(740).unwrap();
    let (submission_fence, _) = swapchain
        .take_ready_for_worker(worker_token, MonotonicTimestampNs::new(10))
        .unwrap();
    drop(submission_fence);

    let ready_slot = swapchain.acquire_render_slot().unwrap();
    let (ready_fence, ready_signal) = pageflip_test_controllable_render_fence();
    swapchain.finish_render(ready_slot, 1, ready_fence).unwrap();
    swapchain.suspend_abandon_future_frames().unwrap();
    assert!(swapchain.has_suspended_ownership());

    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut output_render_fence_token = None;
    let first_wait = recover_or_arm_suspended_output_ownership(
        &mut SwapchainSuspendedRecovery {
            swapchain: &mut swapchain,
        },
        &mut event_loop,
        &mut output_render_fence_token,
    )
    .unwrap();
    assert_eq!(first_wait, SuspendedOutputRecovery::Waiting);
    let first_token = output_render_fence_token.expect("first suspended fence is armed");
    assert_eq!(
        event_loop.source_for_token(first_token),
        Some(NativeEventSource::OutputRenderFence)
    );
    assert!(!swapchain.render_target_available_for_limit(2));
    assert!(swapchain.acquire_render_slot().is_err());

    signal_pageflip_test_fence(&worker_signal);
    let first_wakeup = event_loop.wait().unwrap();
    assert!(first_wakeup.reasons.output_render_fence());
    let progressed = settle_suspended_output_fence_wake(
        &mut SwapchainSuspendedRecovery {
            swapchain: &mut swapchain,
        },
        &mut event_loop,
        &mut output_render_fence_token,
    )
    .unwrap();
    assert_eq!(progressed, SuspendedOutputRecovery::Waiting);
    let second_token = output_render_fence_token.expect("remaining fence is re-armed");
    assert_ne!(first_token, second_token);
    assert!(!swapchain.render_target_available_for_limit(2));

    signal_pageflip_test_fence(&ready_signal);
    let second_wakeup = event_loop.wait().unwrap();
    assert!(second_wakeup.reasons.output_render_fence());
    let recovered = settle_suspended_output_fence_wake(
        &mut SwapchainSuspendedRecovery {
            swapchain: &mut swapchain,
        },
        &mut event_loop,
        &mut output_render_fence_token,
    )
    .unwrap();
    assert_eq!(recovered, SuspendedOutputRecovery::Recovered);
    assert!(output_render_fence_token.is_none());
    assert!(!swapchain.has_suspended_ownership());
    assert!(swapchain.render_target_available_for_limit(2));
    assert!(swapchain.acquire_render_slot().is_ok());
    swapchain.validate_invariants().unwrap();
}
