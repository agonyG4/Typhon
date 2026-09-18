use super::*;
use crate::native_output::kms_worker::{
    KmsBundleOwners, KmsCommitJob, KmsCommitTestPolicy, KmsPrimaryCursorPresentation,
    KmsPrimaryUpdate, KmsTestOnlyPolicy, KmsValidationBase, KmsWorkerDispatchModel,
    KmsWorkerDispatchTailObservation,
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
use oblivion_one::native::presentation_deadline::TargetSelectionEvidence;
use std::num::NonZeroU64;
use std::sync::Arc;

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

fn journal_with_warm_paired_samples() -> AdaptiveRenderJournal {
    let mut journal = AdaptiveRenderJournal::default();
    let target = reactive_test_target();
    for sample in 0..20 {
        let base = sample * 2_000_000;
        journal.record_frame_service_observation(FrameTimingObservation {
            frame_id: sample,
            target,
            composite_started_at: MonotonicTimestampNs::new(base),
            fence_exported_at: MonotonicTimestampNs::new(base + 1),
            fence_signaled_at: Some((
                MonotonicTimestampNs::new(base + 1_000_000),
                FenceTimestampQuality::ExactSyncFile,
            )),
            submit_started_at: Some(MonotonicTimestampNs::new(base + 1_000_000)),
            submit_returned_at: Some(MonotonicTimestampNs::new(base + 1_001_000)),
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
            None,
        ),
        PresentationDeadlineAssessment::Advisory(AdvisoryOpportunitySlip::KmsDispatch)
    );
}

#[test]
fn advisory_dispatch_does_not_change_warm_paired_or_active_recovery() {
    let mut warm = journal_with_warm_paired_samples();
    let refresh = std::time::Duration::from_millis(10);
    assert_eq!(
        warm.prediction(refresh).estimator_mode,
        oblivion_one::native::adaptive_buffering::PredictionEstimatorMode::WarmPaired
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
        oblivion_one::native::adaptive_buffering::PredictionEstimatorMode::WarmPaired
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
            fence_exported_at: MonotonicTimestampNs::new(base + 1),
            fence_signaled_at: Some((
                MonotonicTimestampNs::new(base + 1_000_000),
                FenceTimestampQuality::ExactSyncFile,
            )),
            submit_started_at: Some(MonotonicTimestampNs::new(base + 1_000_000)),
            submit_returned_at: Some(MonotonicTimestampNs::new(base + 1_001_000)),
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
            Some((MonotonicTimestampNs::new(81), quality)),
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
        assess_presentation_deadline(binding, KmsPresentationOutcome::KmsDispatchMiss, None),
        PresentationDeadlineAssessment::Proven(ProvenDeadlineMiss::KmsDispatch)
    );
    assert_eq!(
        assess_presentation_deadline(
            reactive_test_target(),
            KmsPresentationOutcome::KmsApplyGuardMiss,
            None,
        ),
        PresentationDeadlineAssessment::Proven(ProvenDeadlineMiss::KmsApplyGuard)
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
        output_generation: 1,
        crtc_id: 7,
        kind,
        target,
        submit_window: crate::native_output::presentation::kms_timing::KmsSubmitWindow::try_new(
            target.presentation_time.get(),
            target.submit_not_before().get(),
            0,
            0,
        )
        .unwrap(),
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
