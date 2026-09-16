use super::*;
use crate::native_output::kms_worker::{
    KmsBundleOwners, KmsCommitJob, KmsCommitTestPolicy, KmsPrimaryCursorPresentation,
    KmsPrimaryUpdate, KmsTestOnlyPolicy, KmsValidationBase, KmsWorkerDispatchTailObservation,
};
use crate::native_output::presentation::kms_timing::KmsPresentationTimingObservation;
use crate::native_output::presentation::plane::{
    CursorCoupling, CursorPlanePoint, CursorRevision, KmsCommitBundleId, PresentedCursorDelivery,
    PresentedCursorState, PresentedPlaneSnapshot,
};
use oblivion_one::native::adaptive_buffering::{EstimatorRecoveryDisposition, ProvenDeadlineMiss};
use oblivion_one::native::kms::FramebufferId;
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
        deadline_overrun_ns,
        guard_before_ns,
        guard_ns,
        increased,
        decayed: false,
        cap_hit,
    }
}

fn apply_recovery_evidence(
    accepted: bool,
    apply_guard_before_ns: u64,
    apply_guard_after_ns: u64,
    apply_guard_increased: bool,
) -> KmsPresentationTimingObservation {
    KmsPresentationTimingObservation {
        accepted,
        apply_guard_before_ns,
        apply_guard_after_ns,
        apply_guard_increased,
    }
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
            Some(apply_recovery_evidence(true, 100_000, 150_000, true)),
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
            Some(apply_recovery_evidence(true, 100_000, 150_000, true)),
        ),
        EstimatorRecoveryDisposition::PreserveEstimatorState
    );
    for evidence in [
        apply_recovery_evidence(false, 100_000, 100_000, false),
        apply_recovery_evidence(true, 100_000, 100_000, false),
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
