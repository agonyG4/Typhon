use super::tests::{
    fd_identity, fd_is_closed, fd_is_closed_or_reused, reserve_for_test, test_input_fence,
    test_job, wait_for_fence_event,
};
use super::thread::{KmsCommitExecutor, KmsWorkerSubmission, KmsWorkerSubmitFailure};
use super::{
    CursorSidecar, CursorSidecarCoupling, CursorSidecarMailbox, EstablishedKmsBase,
    KmsBundleOwners, KmsCommitBundleIdentity, KmsCommitJob, KmsCommitWorkerHandle, KmsCursorOwner,
    KmsCursorUpdate, KmsPrimaryOwner, KmsPrimaryUpdate, KmsSubmittedOwnership, KmsTestOnlyPolicy,
    KmsValidationBase, KmsWorkerEvent, KmsWorkerFatalJob, ValidationBaseDisposition,
    validation_base_ready,
};
use oblivion_one::native::kms::AtomicCursorVisualState;
use std::{
    collections::VecDeque,
    os::fd::AsRawFd,
    sync::{Arc, Mutex},
    time::Duration,
};

fn test_sidecar(job: &KmsCommitJob, id: u64, coupling: CursorSidecarCoupling) -> CursorSidecar {
    let transaction_id = crate::native_output::OutputTransactionId::new(
        std::num::NonZeroU64::new(id.saturating_mul(100)).unwrap(),
    );
    let transaction = Arc::new(
        crate::native_output::OutputTransaction::cursor_plane_delta(
            transaction_id,
            job.output_generation,
            oblivion_one::native::presentation_deadline::MonotonicTimestampNs::new(id),
            job.target,
            oblivion_one::native::scheduler::NativeOutputPacingMode::ReactiveDouble,
            id,
            None,
            crate::native_output::OutputReleasePlan::Pageflip,
        )
        .unwrap(),
    );
    CursorSidecar {
        id: crate::native_output::presentation::plane::CursorSidecarId::new(
            std::num::NonZeroU64::new(id).unwrap(),
        ),
        transaction,
        revision: crate::native_output::presentation::plane::CursorRevision::initial(),
        assignment: crate::native_output::CursorPlaneAssignment::Atomic {
            desired_epoch: id,
            state: None,
        },
        lease: None,
        coupling,
        created_at: oblivion_one::native::presentation_deadline::MonotonicTimestampNs::new(id),
        deadline: job.target,
        crtc_id: job.crtc_id,
        test_policy: KmsTestOnlyPolicy::Skip,
        capability_key: None,
        validation_base: job.validation_base,
    }
}

fn test_cursor_job(token: u64) -> KmsCommitJob {
    let mut job = test_job(token);
    job.kind = crate::native_output::runtime::AtomicCommitKind::PlaneDelta {
        transaction_id: job.transaction_id,
        cursor_epoch: token,
        framebuffer_id: Some(42),
    };
    job.primary = KmsPrimaryUpdate::Unchanged;
    job.cursor = KmsCursorUpdate::Disable;
    job.test_only = KmsTestOnlyPolicy::Required;
    job
}

fn offer_sidecar(
    handle: &KmsCommitWorkerHandle,
    mut sidecar: CursorSidecar,
) -> Option<CursorSidecar> {
    for _ in 0..1_000 {
        match handle.offer_cursor_sidecar(sidecar) {
            Ok(replaced) => return replaced,
            Err(error) if error.reason == super::KmsWorkerAdmissionError::AdmissionContention => {
                sidecar = *error.sidecar;
                std::thread::yield_now();
            }
            Err(error) => panic!("sidecar offer failed: {:?}", error.reason),
        }
    }
    panic!("sidecar offer remained contended")
}

fn two_owner_job(
    token: u64,
) -> (
    KmsCommitJob,
    crate::native_output::OutputTransactionId,
    crate::native_output::OutputTransactionId,
) {
    let mut job = test_job(token);
    let primary_id = job.transaction_id;
    let cursor_id = crate::native_output::OutputTransactionId::new(
        std::num::NonZeroU64::new(token.saturating_mul(10)).unwrap(),
    );
    let transaction = |id, epoch| {
        Arc::new(
            crate::native_output::OutputTransaction::cursor_plane_delta(
                id,
                1,
                oblivion_one::native::presentation_deadline::MonotonicTimestampNs::new(0),
                job.target,
                oblivion_one::native::scheduler::NativeOutputPacingMode::ReactiveDouble,
                epoch,
                None,
                crate::native_output::OutputReleasePlan::Pageflip,
            )
            .unwrap(),
        )
    };
    job.owners = KmsBundleOwners::new(
        Some(KmsPrimaryOwner {
            transaction: transaction(primary_id, token),
        }),
        Some(KmsCursorOwner {
            transaction: transaction(cursor_id, token.saturating_mul(10)),
            sidecar_id: None,
            revision: crate::native_output::presentation::plane::CursorRevision::initial(),
            capability_key: None,
        }),
    )
    .unwrap();
    (job, primary_id, cursor_id)
}

fn assert_two_owners(
    job: &KmsCommitJob,
    primary_id: crate::native_output::OutputTransactionId,
    cursor_id: crate::native_output::OutputTransactionId,
) {
    assert_eq!(job.owners.primary_transaction_id(), Some(primary_id));
    assert_eq!(job.owners.cursor_transaction_id(), Some(cursor_id));
}

#[test]
fn validation_base_disposition_requires_the_exact_established_bundle() {
    let predecessor = test_job(333).identity();
    let other = test_job(334).identity();

    assert_eq!(
        validation_base_ready(
            EstablishedKmsBase::Bundle(predecessor),
            KmsValidationBase::Predecessor(predecessor),
        ),
        ValidationBaseDisposition::Ready
    );
    assert_eq!(
        validation_base_ready(
            EstablishedKmsBase::Bundle(other),
            KmsValidationBase::Predecessor(predecessor),
        ),
        ValidationBaseDisposition::Invalidated
    );
    assert_eq!(
        validation_base_ready(
            EstablishedKmsBase::Pending(predecessor),
            KmsValidationBase::Predecessor(predecessor),
        ),
        ValidationBaseDisposition::Wait
    );
}

#[test]
fn dependent_cursor_waits_for_the_exact_predecessor_pageflip_before_testing() {
    let executor = Arc::new(RecordingExecutor::accepting());
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let predecessor = test_job(335);
    let predecessor_identity = predecessor.identity();
    let predecessor_token = predecessor.token;
    let predecessor_transaction = predecessor.transaction_id;
    let mut cursor = test_cursor_job(336);
    cursor.validation_base = KmsValidationBase::Predecessor(predecessor_identity);

    reserve_for_test(&handle, predecessor.kind)
        .enqueue(predecessor)
        .unwrap();
    let predecessor_events = wait_for_fence_event(
        &handle,
        335,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 335),
    );
    reserve_for_test(&handle, cursor.kind)
        .enqueue(cursor)
        .unwrap();
    std::thread::sleep(Duration::from_millis(10));
    assert!(
        !executor
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.framebuffer_id == 0)
    );

    handle
        .ack_pageflip(predecessor_token, predecessor_transaction, 1)
        .unwrap();
    handle.set_established_presented_base(
        crate::native_output::presentation::plane::PlaneStateRevision::new(
            std::num::NonZeroU64::new(2).unwrap(),
        ),
        1,
        7,
    );
    let cursor_events = wait_for_fence_event(
        &handle,
        336,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 336),
    );
    assert!(
        executor
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.framebuffer_id == 0 && request.test_only)
    );
    handle
        .ack_pageflip(
            test_cursor_job(336).token,
            test_cursor_job(336).transaction_id,
            1,
        )
        .unwrap();
    drop((predecessor_events, cursor_events));
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn stale_presented_validation_base_is_returned_before_test_or_submit() {
    let executor = Arc::new(RecordingExecutor::accepting());
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    handle.set_established_presented_base(
        crate::native_output::presentation::plane::PlaneStateRevision::new(
            std::num::NonZeroU64::new(2).unwrap(),
        ),
        1,
        7,
    );
    let mut job = test_cursor_job(3361);
    job.validation_base = KmsValidationBase::Presented {
        snapshot: crate::native_output::presentation::plane::PresentedPlaneSnapshot::legacy(None),
        output_generation: 1,
        crtc_id: 7,
    };
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();
    wait_for_fence_event(&handle, 3361, |event| {
        matches!(
            event,
            KmsWorkerEvent::ValidationBaseInvalidated { job, reason, .. }
                if job.token.get() == 3361
                    && *reason == super::thread::ValidationBaseInvalidationReason::PresentedRevisionChanged
        )
    });
    let requests = executor.requests.lock().unwrap();
    assert!(!requests.iter().any(|request| request.framebuffer_id == 0));
    drop(requests);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn rejected_predecessor_returns_dependent_cursor_before_test_or_submit() {
    let executor = Arc::new(RecordingExecutor {
        test_outcomes: Mutex::new(VecDeque::from([Err(
            oblivion_one::native::kms::AtomicKmsErrorKind::TestOnlyRejected,
        )])),
        submit_outcomes: Mutex::new(VecDeque::from([Ok(())])),
        requests: Mutex::new(Vec::new()),
        real_input_fence_fds: Mutex::new(Vec::new()),
        real_input_fence_open: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let mut predecessor = required_direct_test_job(337);
    predecessor.validation_base = KmsValidationBase::Presented {
        snapshot: crate::native_output::presentation::plane::PresentedPlaneSnapshot::legacy(None),
        output_generation: 1,
        crtc_id: 7,
    };
    let predecessor_identity = predecessor.identity();
    let mut cursor = test_cursor_job(338);
    cursor.validation_base = KmsValidationBase::Predecessor(predecessor_identity);

    let pause = handle.pause_after_dequeue_for_test();
    reserve_for_test(&handle, predecessor.kind)
        .enqueue(predecessor)
        .unwrap();
    pause.wait_until_selected();
    reserve_for_test(&handle, cursor.kind)
        .enqueue(cursor)
        .unwrap();
    pause.release();
    let events = wait_for_fence_event(&handle, 338, |event| {
        matches!(
            event,
            KmsWorkerEvent::ValidationBaseInvalidated { job, .. }
                if job.token.get() == 338
        )
    });
    let requests = executor.requests.lock().unwrap().clone();
    assert!(requests.iter().any(|request| request.test_only));
    assert!(!requests.iter().any(|request| !request.test_only));
    assert!(!requests.iter().any(|request| request.framebuffer_id == 0));
    drop(events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn permanently_rejected_predecessor_invalidates_dependent_before_test_or_submit() {
    let executor = Arc::new(RecordingExecutor {
        test_outcomes: Mutex::new(VecDeque::from([Ok(())])),
        submit_outcomes: Mutex::new(VecDeque::from([Err(
            oblivion_one::native::kms::AtomicKmsErrorKind::FlipRejected,
        )])),
        requests: Mutex::new(Vec::new()),
        real_input_fence_fds: Mutex::new(Vec::new()),
        real_input_fence_open: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let predecessor = required_direct_test_job(3381);
    let predecessor_identity = predecessor.identity();
    let mut cursor = test_cursor_job(3382);
    cursor.validation_base = KmsValidationBase::Predecessor(predecessor_identity);
    let pause = handle.pause_after_dequeue_for_test();
    reserve_for_test(&handle, predecessor.kind)
        .enqueue(predecessor)
        .unwrap();
    pause.wait_until_selected();
    reserve_for_test(&handle, cursor.kind)
        .enqueue(cursor)
        .unwrap();
    pause.release();
    wait_for_fence_event(&handle, 3382, |event| {
        matches!(
            event,
            KmsWorkerEvent::ValidationBaseInvalidated { job, .. }
                if job.token.get() == 3382
        )
    });
    assert!(
        !executor
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.framebuffer_id == 0)
    );
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn busy_exhausted_predecessor_invalidates_dependent_before_test_or_submit() {
    let executor = Arc::new(RecordingExecutor {
        test_outcomes: Mutex::new(VecDeque::from([Ok(())])),
        submit_outcomes: Mutex::new(VecDeque::from([
            Err(oblivion_one::native::kms::AtomicKmsErrorKind::Busy),
            Err(oblivion_one::native::kms::AtomicKmsErrorKind::Busy),
            Err(oblivion_one::native::kms::AtomicKmsErrorKind::Busy),
        ])),
        requests: Mutex::new(Vec::new()),
        real_input_fence_fds: Mutex::new(Vec::new()),
        real_input_fence_open: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let predecessor = required_direct_test_job(3383);
    let predecessor_identity = predecessor.identity();
    let mut cursor = test_cursor_job(3384);
    cursor.validation_base = KmsValidationBase::Predecessor(predecessor_identity);
    let pause = handle.pause_after_dequeue_for_test();
    reserve_for_test(&handle, predecessor.kind)
        .enqueue(predecessor)
        .unwrap();
    pause.wait_until_selected();
    reserve_for_test(&handle, cursor.kind)
        .enqueue(cursor)
        .unwrap();
    pause.release();
    wait_for_fence_event(&handle, 3384, |event| {
        matches!(
            event,
            KmsWorkerEvent::ValidationBaseInvalidated { job, .. }
                if job.token.get() == 3384
        )
    });
    assert!(
        !executor
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.framebuffer_id == 0)
    );
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn mismatched_pageflip_identity_invalidates_dependents_without_releasing_the_inflight_job() {
    let executor = Arc::new(RecordingExecutor::accepting());
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let predecessor = test_job(3391);
    let predecessor_identity = predecessor.identity();
    let mut cursor = test_cursor_job(3392);
    cursor.validation_base = KmsValidationBase::Predecessor(predecessor_identity);
    let predecessor_transaction = predecessor.transaction_id;

    reserve_for_test(&handle, predecessor.kind)
        .enqueue(predecessor)
        .unwrap();
    wait_for_fence_event(
        &handle,
        3391,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 3391),
    );
    reserve_for_test(&handle, cursor.kind)
        .enqueue(cursor)
        .unwrap();

    let mut wrong = predecessor_identity;
    wrong.crtc_id = wrong.crtc_id.saturating_add(1);
    let error = handle.ack_pageflip_identity(wrong, predecessor_transaction);
    assert_eq!(error, Err(super::thread::KmsWorkerAckError::CrtcMismatch));
    let events = wait_for_fence_event(&handle, 3392, |event| {
        matches!(
            event,
            KmsWorkerEvent::ValidationBaseInvalidated { job, .. }
                if job.token.get() == 3392
        )
    });
    assert!(
        !executor
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.framebuffer_id == 0)
    );
    assert!(handle.inflight());
    drop(events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn explicit_predecessor_replacement_returns_dependent_before_test_or_submit() {
    let executor = Arc::new(RecordingExecutor::accepting());
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let predecessor = test_job(3393);
    let predecessor_identity = predecessor.identity();
    let mut cursor = test_cursor_job(3394);
    cursor.validation_base = KmsValidationBase::Predecessor(predecessor_identity);

    reserve_for_test(&handle, predecessor.kind)
        .enqueue(predecessor)
        .unwrap();
    wait_for_fence_event(
        &handle,
        3393,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 3393),
    );
    reserve_for_test(&handle, cursor.kind)
        .enqueue(cursor)
        .unwrap();
    handle.invalidate_validation_base(
        predecessor_identity,
        super::thread::ValidationBaseInvalidationReason::PredecessorTerminal,
    );
    wait_for_fence_event(&handle, 3394, |event| {
        matches!(
            event,
            KmsWorkerEvent::ValidationBaseInvalidated { job, .. }
                if job.token.get() == 3394
        )
    });
    assert!(
        !executor
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.framebuffer_id == 0)
    );
    assert!(handle.inflight());
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn rejection_event_retains_both_logical_bundle_owners() {
    let (job, primary_id, cursor_id) = two_owner_job(33);
    let event = KmsWorkerEvent::TestRejected {
        job,
        error: oblivion_one::native::kms::AtomicKmsError::new(
            oblivion_one::native::kms::AtomicKmsErrorKind::TestOnlyRejected,
            "test",
        ),
    };

    let KmsWorkerEvent::TestRejected { job, .. } = event else {
        unreachable!();
    };
    assert_two_owners(&job, primary_id, cursor_id);
}

#[test]
fn terminal_worker_transports_preserve_both_bundle_owners() {
    let error = || {
        oblivion_one::native::kms::AtomicKmsError::new(
            oblivion_one::native::kms::AtomicKmsErrorKind::FlipRejected,
            "test",
        )
    };
    let (job, primary_id, cursor_id) = two_owner_job(331);
    let submitted =
        KmsSubmittedOwnership {
            job,
            out_fence: None,
            submit_started_at:
                oblivion_one::native::presentation_deadline::MonotonicTimestampNs::new(1),
            submit_returned_at:
                oblivion_one::native::presentation_deadline::MonotonicTimestampNs::new(2),
            queue_residency_ns: 1,
            submit_wake_lateness_ns: 0,
            submission_budget_ns: 1,
        };
    assert_two_owners(&submitted.job, primary_id, cursor_id);

    let (job, _, _) = two_owner_job(331);
    let KmsWorkerEvent::SubmitRejected { job, .. } = (KmsWorkerEvent::SubmitRejected {
        job,
        error: error(),
    }) else {
        unreachable!();
    };
    assert_two_owners(&job, primary_id, cursor_id);

    let (job, _, _) = two_owner_job(331);
    let KmsWorkerEvent::BusyExhausted { job, .. } = (KmsWorkerEvent::BusyExhausted {
        job,
        error: error(),
    }) else {
        unreachable!();
    };
    assert_two_owners(&job, primary_id, cursor_id);

    let (job, _, _) = two_owner_job(331);
    let KmsWorkerEvent::Quiesced { returned_jobs, .. } = (KmsWorkerEvent::Quiesced {
        returned_jobs: vec![job],
        returned_sidecar: None,
    }) else {
        unreachable!();
    };
    assert_two_owners(&returned_jobs[0], primary_id, cursor_id);

    let (job, _, _) = two_owner_job(331);
    let fatal = KmsWorkerFatalJob {
        job,
        uncertain_submit: true,
    };
    assert_two_owners(&fatal.job, primary_id, cursor_id);
}

#[test]
fn changed_cursor_property_requires_an_exact_cursor_owner() {
    let (mut job, _, _) = two_owner_job(332);
    let primary = job.owners.primary().cloned().unwrap();
    let transaction = Arc::clone(&primary.transaction);
    job.owners = KmsBundleOwners::new(Some(primary), None).unwrap();
    job.cursor = KmsCursorUpdate::Disable;

    assert_eq!(
        job.validate_against(&transaction),
        Err(super::KmsCommitPayloadError::MissingCursorOwner)
    );
}

#[test]
fn cursor_job_keeps_immutable_presented_or_predecessor_validation_base() {
    let job = test_job(3321);
    assert!(matches!(
        job.validation_base,
        KmsValidationBase::Presented { .. }
    ));
    let predecessor = KmsCommitBundleIdentity {
        id: job.bundle_id,
        token: job.token,
        output_generation: job.output_generation,
        crtc_id: job.crtc_id,
        primary_transaction_id: Some(job.transaction_id),
        cursor_transaction_id: None,
    };
    assert_ne!(
        KmsValidationBase::Presented {
            snapshot: crate::native_output::presentation::plane::PresentedPlaneSnapshot::legacy(
                None,
            ),
            output_generation: 1,
            crtc_id: 7,
        },
        KmsValidationBase::Predecessor(predecessor)
    );
}

#[test]
fn predecessor_validation_base_must_match_job_output_identity() {
    let mut job = test_job(3322);
    job.validation_base = KmsValidationBase::Predecessor(KmsCommitBundleIdentity {
        id: job.bundle_id,
        token: job.token,
        output_generation: job.output_generation,
        crtc_id: job.crtc_id + 1,
        primary_transaction_id: Some(job.transaction_id),
        cursor_transaction_id: None,
    });
    let transaction = crate::native_output::OutputTransaction::cursor_plane_delta(
        job.transaction_id,
        job.output_generation,
        oblivion_one::native::presentation_deadline::MonotonicTimestampNs::new(0),
        job.target,
        oblivion_one::native::scheduler::NativeOutputPacingMode::ReactiveDouble,
        1,
        None,
        crate::native_output::OutputReleasePlan::Pageflip,
    )
    .unwrap();
    assert_eq!(
        job.validate_against(&transaction),
        Err(super::KmsCommitPayloadError::ValidationBaseMismatch)
    );
}

#[test]
fn sidecar_offered_before_freeze_is_attached_to_exact_primary_bundle() {
    let executor = Arc::new(RecordingExecutor::accepting());
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let job = test_job(333);
    let token = job.token;
    let transaction_id = job.transaction_id;
    let pause = handle.pause_collecting_sidecar_for_test();
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();
    pause.wait_until_selected();
    let sidecar = test_sidecar(&test_job(333), 900, CursorSidecarCoupling::Independent);
    let sidecar_id = sidecar.id;
    assert!(offer_sidecar(&handle, sidecar).is_none());
    pause.release();

    let events = wait_for_fence_event(&handle, 333, |event| {
        matches!(event, KmsWorkerEvent::Submitted { .. })
    });
    assert!(events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::Submitted { ownership }
            if matches!(ownership.job.cursor, KmsCursorUpdate::Disable)
                && ownership.job.owners.cursor().and_then(|owner| owner.sidecar_id)
                    == Some(sidecar_id)
    )));
    handle.ack_pageflip(token, transaction_id, 1).unwrap();
    drop(events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn sidecar_offered_before_primary_dequeue_is_attached() {
    let handle = KmsCommitWorkerHandle::start(Arc::new(RecordingExecutor::accepting())).unwrap();
    let job = test_job(339);
    let token = job.token;
    let transaction_id = job.transaction_id;
    let sidecar = test_sidecar(&job, 908, CursorSidecarCoupling::Independent);
    let id = sidecar.id;
    let expected_identity = KmsCommitBundleIdentity {
        cursor_transaction_id: Some(sidecar.transaction.id()),
        ..job.identity()
    };
    offer_sidecar(&handle, sidecar);
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();

    let events = wait_for_fence_event(&handle, 339, |event| {
        matches!(event, KmsWorkerEvent::Submitted { .. })
    });
    assert!(events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::Submitted { ownership }
            if ownership.job.owners.cursor().and_then(|owner| owner.sidecar_id) == Some(id)
                && ownership.job.identity() == expected_identity
    )));
    handle.ack_pageflip(token, transaction_id, 1).unwrap();
    drop(events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn attachable_primary_is_one_exact_pre_freeze_snapshot() {
    let handle = KmsCommitWorkerHandle::start(Arc::new(RecordingExecutor::accepting())).unwrap();
    let (job, primary_id, _) = two_owner_job(340);
    let token = job.token;
    let target = job.target;
    let pause = handle.pause_after_dequeue_for_test();
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();
    pause.wait_until_selected();

    let attachable = handle
        .attachable_primary(1, 7, target)
        .expect("pre-freeze primary must be attachable");
    assert_eq!(attachable.transaction_id, primary_id);
    assert_eq!(attachable.output_generation, 1);
    assert_eq!(attachable.crtc_id, 7);
    assert_eq!(attachable.target, target);

    let frozen = handle.pause_after_freeze_for_test();
    pause.release();
    frozen.wait_until_selected();
    assert!(handle.attachable_primary(1, 7, target).is_none());
    frozen.release();
    wait_for_fence_event(&handle, 340, |event| {
        matches!(event, KmsWorkerEvent::Submitted { .. })
    });
    handle.ack_pageflip(token, primary_id, 1).unwrap();
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn embedded_cursor_primary_is_attachable_and_exposes_exact_bundle_identity() {
    let handle = KmsCommitWorkerHandle::start(Arc::new(RecordingExecutor::accepting())).unwrap();
    let first = test_job(343);
    let first_identity = first.identity();
    let first_token = first.token;
    let first_transaction = first.transaction_id;
    reserve_for_test(&handle, first.kind)
        .enqueue(first)
        .unwrap();
    wait_for_fence_event(
        &handle,
        343,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token == first_token),
    );

    let (mut primary, _, _) = two_owner_job(344);
    primary.validation_base = KmsValidationBase::Predecessor(first_identity);
    primary.cursor = KmsCursorUpdate::Set(AtomicCursorVisualState {
        visible: true,
        framebuffer_id: Some(344),
        ..AtomicCursorVisualState::hidden(64, 64)
    });
    let primary_identity = primary.identity();
    let primary_transaction = primary.transaction_id;
    reserve_for_test(&handle, primary.kind)
        .enqueue(primary)
        .unwrap();

    let attachable = handle
        .attachable_primary(1, 7, test_job(344).target)
        .expect("embedded cursor primary must remain attachable");
    assert_eq!(attachable.transaction_id, primary_transaction);
    assert_eq!(attachable.bundle_identity, primary_identity);
    assert_eq!(
        attachable.validation_base,
        KmsValidationBase::Predecessor(first_identity)
    );
    assert_eq!(handle.pending_bundle_identity(1, 7), Some(first_identity));

    handle
        .ack_pageflip(first_token, first_transaction, 1)
        .unwrap();
    wait_for_fence_event(
        &handle,
        344,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.identity() == primary_identity),
    );
    handle
        .ack_pageflip(primary_identity.token, primary_transaction, 1)
        .unwrap();
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn queued_primary_snapshot_carries_its_own_validation_base() {
    let handle = KmsCommitWorkerHandle::start(Arc::new(RecordingExecutor::accepting())).unwrap();
    let predecessor = test_job(345);
    let predecessor_identity = predecessor.identity();
    let predecessor_token = predecessor.token;
    let predecessor_transaction = predecessor.transaction_id;
    reserve_for_test(&handle, predecessor.kind)
        .enqueue(predecessor)
        .unwrap();
    wait_for_fence_event(
        &handle,
        345,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token == predecessor_token),
    );

    let (mut primary, _, _) = two_owner_job(346);
    primary.validation_base = KmsValidationBase::Predecessor(predecessor_identity);
    let primary_identity = primary.identity();
    reserve_for_test(&handle, primary.kind)
        .enqueue(primary)
        .unwrap();
    let attachable = handle
        .attachable_primary(1, 7, test_job(346).target)
        .expect("queued primary must be attachable");
    assert_eq!(attachable.bundle_identity, primary_identity);
    assert_eq!(
        attachable.validation_base,
        KmsValidationBase::Predecessor(predecessor_identity)
    );

    handle
        .ack_pageflip(predecessor_token, predecessor_transaction, 1)
        .unwrap();
    wait_for_fence_event(
        &handle,
        346,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.identity() == primary_identity),
    );
    handle
        .ack_pageflip(
            primary_identity.token,
            primary_identity.primary_transaction_id.unwrap(),
            1,
        )
        .unwrap();
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn newer_sidecar_replaces_embedded_cursor_before_freeze() {
    let executor = Arc::new(RecordingExecutor::accepting());
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let (mut job, primary_id, _) = two_owner_job(341);
    let token = job.token;
    let mut embedded = AtomicCursorVisualState::hidden(64, 64);
    embedded.visible = true;
    embedded.framebuffer_id = Some(71);
    embedded.image_generation = 1;
    job.cursor = KmsCursorUpdate::Set(embedded.clone());

    let pause = handle.pause_collecting_sidecar_for_test();
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();
    pause.wait_until_selected();
    let mut sidecar = test_sidecar(&test_job(341), 912, CursorSidecarCoupling::Independent);
    let sidecar_id = sidecar.id;
    let mut newer = embedded.clone();
    newer.x = 1;
    sidecar.revision =
        crate::native_output::presentation::plane::CursorRevision::initial().advance_motion();
    sidecar.assignment = crate::native_output::CursorPlaneAssignment::Atomic {
        desired_epoch: 2,
        state: Some(newer.clone()),
    };
    assert!(offer_sidecar(&handle, sidecar).is_none());
    pause.release();

    let events = wait_for_fence_event(&handle, 341, |event| {
        matches!(event, KmsWorkerEvent::Submitted { .. })
    });
    assert!(events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::Submitted { ownership }
            if ownership.job.owners.cursor().and_then(|owner| owner.sidecar_id) == Some(sidecar_id)
                && ownership.job.cursor == KmsCursorUpdate::Set(newer.clone())
    )));
    handle.ack_pageflip(token, primary_id, 1).unwrap();
    drop(events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn late_bound_bundle_identity_is_published_before_dependents_observe_it() {
    let handle = KmsCommitWorkerHandle::start(Arc::new(RecordingExecutor::accepting())).unwrap();
    let (mut primary, primary_id, _) = two_owner_job(914);
    let mut embedded = AtomicCursorVisualState::hidden(64, 64);
    embedded.visible = true;
    embedded.framebuffer_id = Some(914);
    embedded.image_generation = 1;
    primary.cursor = KmsCursorUpdate::Set(embedded.clone());
    let old_identity = primary.identity();
    let mut sidecar = test_sidecar(&primary, 9141, CursorSidecarCoupling::Independent);
    sidecar.revision =
        crate::native_output::presentation::plane::CursorRevision::initial().advance_motion();
    let mut replacement = embedded;
    replacement.x = 3;
    replacement.image_generation = 2;
    sidecar.assignment = crate::native_output::CursorPlaneAssignment::Atomic {
        desired_epoch: 2,
        state: Some(replacement),
    };
    let replacement_transaction = sidecar.transaction.id();

    let dequeue_pause = handle.pause_after_dequeue_for_test();
    let frozen_pause = handle.pause_after_freeze_for_test();
    let primary_kind = primary.kind;
    reserve_for_test(&handle, primary_kind)
        .enqueue(primary)
        .unwrap();
    dequeue_pause.wait_until_selected();

    assert!(offer_sidecar(&handle, sidecar).is_none());
    dequeue_pause.release();
    frozen_pause.wait_until_selected();

    let replaced_identity = KmsCommitBundleIdentity {
        cursor_transaction_id: Some(replacement_transaction),
        ..old_identity
    };
    assert_eq!(
        handle.pending_bundle_identity(1, 7),
        Some(replaced_identity)
    );

    let mut dependent = test_cursor_job(9142);
    dependent.validation_base = KmsValidationBase::Predecessor(replaced_identity);
    let dependent_kind = dependent.kind;
    let dependent_token = dependent.token;
    let dependent_transaction = dependent.transaction_id;
    reserve_for_test(&handle, dependent_kind)
        .enqueue(dependent)
        .unwrap();
    frozen_pause.release();

    let primary_events = wait_for_fence_event(
        &handle,
        914,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 914),
    );
    assert!(primary_events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::Submitted { ownership }
            if ownership.job.identity() == replaced_identity
    )));
    handle
        .ack_pageflip_identity(replaced_identity, primary_id)
        .unwrap();

    let dependent_events = wait_for_fence_event(
        &handle,
        9142,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 9142),
    );
    assert!(dependent_events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::Submitted { ownership }
            if ownership.job.validation_base == KmsValidationBase::Predecessor(replaced_identity)
    )));
    handle
        .ack_pageflip(dependent_token, dependent_transaction, 1)
        .unwrap();
    drop((primary_events, dependent_events));
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn predecessor_pageflip_releases_latest_sidecar_with_queued_next_primary() {
    let handle = KmsCommitWorkerHandle::start(Arc::new(RecordingExecutor::accepting())).unwrap();
    let first = test_job(340);
    let first_token = first.token;
    let first_transaction = first.transaction_id;
    reserve_for_test(&handle, first.kind)
        .enqueue(first)
        .unwrap();
    let first_events = wait_for_fence_event(&handle, 340, |event| {
        matches!(event, KmsWorkerEvent::Submitted { .. })
    });

    let mut second = test_job(341);
    second.validation_base = KmsValidationBase::Predecessor(
        first_events
            .iter()
            .find_map(|event| {
                if let KmsWorkerEvent::Submitted { ownership } = event {
                    Some(ownership.job.identity())
                } else {
                    None
                }
            })
            .expect("submitted predecessor identity"),
    );
    let second_token = second.token;
    let second_transaction = second.transaction_id;
    let older = test_sidecar(&second, 909, CursorSidecarCoupling::Independent);
    let latest = test_sidecar(&second, 910, CursorSidecarCoupling::Independent);
    let latest_id = latest.id;
    reserve_for_test(&handle, second.kind)
        .enqueue(second)
        .unwrap();
    offer_sidecar(&handle, older);
    assert!(offer_sidecar(&handle, latest).is_some());
    handle
        .ack_pageflip(first_token, first_transaction, 1)
        .unwrap();

    let second_events = wait_for_fence_event(
        &handle,
        341,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token == second_token),
    );
    assert!(second_events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::Submitted { ownership }
            if ownership.job.token == second_token
                && ownership.job.owners.cursor().and_then(|owner| owner.sidecar_id)
                    == Some(latest_id)
    )));
    handle
        .ack_pageflip(second_token, second_transaction, 1)
        .unwrap();
    drop(first_events);
    drop(second_events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn sidecar_offered_after_freeze_remains_pending() {
    let executor = Arc::new(RecordingExecutor::accepting());
    let handle = KmsCommitWorkerHandle::start(executor).unwrap();
    let job = test_job(334);
    let token = job.token;
    let transaction_id = job.transaction_id;
    let pause = handle.pause_after_freeze_for_test();
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();
    pause.wait_until_selected();
    let sidecar = test_sidecar(&test_job(334), 901, CursorSidecarCoupling::Independent);
    let sidecar_id = sidecar.id;
    assert!(offer_sidecar(&handle, sidecar).is_none());
    pause.release();

    let events = wait_for_fence_event(&handle, 334, |event| {
        matches!(event, KmsWorkerEvent::Submitted { .. })
    });
    assert!(events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::Submitted { ownership }
            if matches!(ownership.job.cursor, KmsCursorUpdate::Unchanged)
    )));
    assert_eq!(handle.pending_cursor_sidecar_id(), Some(sidecar_id));
    handle.ack_pageflip(token, transaction_id, 1).unwrap();
    drop(events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn coupled_sidecar_offered_after_freeze_is_returned_when_primary_completes() {
    let handle = KmsCommitWorkerHandle::start(Arc::new(RecordingExecutor::accepting())).unwrap();
    let (job, primary_id, _) = two_owner_job(342);
    let token = job.token;
    let pause = handle.pause_after_freeze_for_test();
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();
    pause.wait_until_selected();

    let sidecar = test_sidecar(
        &test_job(342),
        911,
        CursorSidecarCoupling::MustBundleWith(primary_id),
    );
    let sidecar_id = sidecar.id;
    assert!(offer_sidecar(&handle, sidecar).is_none());
    pause.release();

    let submitted = wait_for_fence_event(&handle, 342, |event| {
        matches!(event, KmsWorkerEvent::Submitted { .. })
    });
    assert!(submitted
        .iter()
        .any(|event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token == token)));
    handle.ack_pageflip(token, primary_id, 1).unwrap();
    let returned = wait_for_fence_event(&handle, 342, |event| {
        matches!(event, KmsWorkerEvent::CursorSidecarReturned { .. })
    });
    assert!(returned.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::CursorSidecarReturned { sidecar, .. } if sidecar.id == sidecar_id
    )));
    assert!(handle.pending_cursor_sidecar_id().is_none());

    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn sidecar_mailbox_replaces_exactly_once_and_never_exceeds_one() {
    let job = test_job(335);
    let first = test_sidecar(&job, 902, CursorSidecarCoupling::Independent);
    let first_id = first.id;
    let second = test_sidecar(&job, 903, CursorSidecarCoupling::Independent);
    let second_id = second.id;
    let mut mailbox = CursorSidecarMailbox::default();

    assert!(mailbox.offer(first).is_none());
    let replaced = mailbox.offer(second).unwrap();
    assert_eq!(replaced.id, first_id);
    assert_eq!(mailbox.len(), 1);
    assert_eq!(mailbox.pending().map(|sidecar| sidecar.id), Some(second_id));
}

#[test]
fn motion_replacement_does_not_copy_existing_required_primary_coupling() {
    let job = test_job(338);
    let required = job.transaction_id;
    let coupled = test_sidecar(&job, 906, CursorSidecarCoupling::MustBundleWith(required));
    let motion = test_sidecar(&job, 907, CursorSidecarCoupling::Independent);
    let mut mailbox = CursorSidecarMailbox::default();
    mailbox.offer(coupled);

    let replaced = mailbox.offer(motion).unwrap();
    assert_eq!(
        replaced.coupling,
        CursorSidecarCoupling::MustBundleWith(required)
    );
    assert_eq!(
        mailbox.pending().map(|sidecar| sidecar.coupling),
        Some(CursorSidecarCoupling::Independent)
    );
}

#[test]
fn mismatched_generation_crtc_and_primary_do_not_consume_sidecar() {
    let job = test_job(336);
    let required = job.transaction_id;
    let sidecar = test_sidecar(&job, 904, CursorSidecarCoupling::MustBundleWith(required));
    let id = sidecar.id;
    let mut mailbox = CursorSidecarMailbox::default();
    mailbox.offer(sidecar);

    assert!(
        mailbox
            .claim_for(
                job.output_generation + 1,
                job.crtc_id,
                job.target,
                Some(required),
            )
            .is_none()
    );
    assert!(
        mailbox
            .claim_for(
                job.output_generation,
                job.crtc_id + 1,
                job.target,
                Some(required),
            )
            .is_none()
    );
    assert!(
        mailbox
            .claim_for(
                job.output_generation,
                job.crtc_id,
                job.target,
                Some(crate::native_output::OutputTransactionId::new(
                    std::num::NonZeroU64::new(999).unwrap()
                ))
            )
            .is_none()
    );
    assert_eq!(mailbox.pending().map(|sidecar| sidecar.id), Some(id));
    assert_eq!(
        mailbox
            .claim_for(
                job.output_generation,
                job.crtc_id,
                job.target,
                Some(required),
            )
            .map(|sidecar| sidecar.id),
        Some(id)
    );
    assert_eq!(mailbox.len(), 0);
}

#[test]
fn quiesce_returns_pending_sidecar_owner() {
    let handle = KmsCommitWorkerHandle::start(Arc::new(RecordingExecutor::accepting())).unwrap();
    let sidecar = test_sidecar(&test_job(337), 905, CursorSidecarCoupling::Independent);
    let id = sidecar.id;
    offer_sidecar(&handle, sidecar);
    handle.request_quiesce();
    let events = wait_for_fence_event(&handle, 337, |event| {
        matches!(event, KmsWorkerEvent::Quiesced { .. })
    });
    assert!(events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::Quiesced {
            returned_sidecar: Some(sidecar),
            ..
        } if sidecar.id == id
    )));
    handle.join().unwrap();
}

#[test]
fn shutdown_snapshot_returns_pending_sidecar_owner() {
    let handle = KmsCommitWorkerHandle::start(Arc::new(RecordingExecutor::accepting())).unwrap();
    let sidecar = test_sidecar(&test_job(342), 911, CursorSidecarCoupling::Independent);
    let id = sidecar.id;
    offer_sidecar(&handle, sidecar);

    let snapshot = handle.begin_shutdown_quiesce().unwrap();
    assert_eq!(snapshot.pending_sidecar.map(|sidecar| sidecar.id), Some(id));
    handle.join().unwrap();
}

#[test]
fn independent_sidecar_promotes_at_deadline_but_coupled_sidecar_never_does() {
    let job = test_job(343);
    let independent = test_sidecar(&job, 912, CursorSidecarCoupling::Independent);
    let independent_id = independent.id;
    let mut mailbox = CursorSidecarMailbox::default();
    mailbox.offer(independent);
    assert_eq!(
        mailbox
            .take_independent_due(job.output_generation, job.crtc_id, job.target)
            .map(|sidecar| sidecar.id),
        Some(independent_id)
    );

    let coupled = test_sidecar(
        &job,
        913,
        CursorSidecarCoupling::MustBundleWith(job.transaction_id),
    );
    let coupled_id = coupled.id;
    mailbox.offer(coupled);
    assert!(
        mailbox
            .take_independent_due(job.output_generation, job.crtc_id, job.target)
            .is_none()
    );
    assert_eq!(
        mailbox.pending().map(|sidecar| sidecar.id),
        Some(coupled_id)
    );
}

fn required_direct_test_job(token: u64) -> KmsCommitJob {
    let mut job = test_job(token);
    job.test_only = KmsTestOnlyPolicy::Required;
    job.cursor = KmsCursorUpdate::Set(AtomicCursorVisualState {
        framebuffer_id: Some(91),
        visible: true,
        ..AtomicCursorVisualState::hidden(64, 64)
    });
    job.primary = KmsPrimaryUpdate::Framebuffer {
        framebuffer: oblivion_one::native::kms::FramebufferId::new(42).unwrap(),
        in_fence: None,
        request_out_fence: true,
    };
    job
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedAtomicRequest {
    framebuffer_id: u32,
    cursor: KmsCursorUpdate,
    test_only: bool,
    request_out_fence: bool,
}

#[derive(Debug)]
struct RecordingExecutor {
    test_outcomes: Mutex<VecDeque<Result<(), oblivion_one::native::kms::AtomicKmsErrorKind>>>,
    submit_outcomes: Mutex<VecDeque<Result<(), oblivion_one::native::kms::AtomicKmsErrorKind>>>,
    requests: Mutex<Vec<RecordedAtomicRequest>>,
    real_input_fence_fds: Mutex<Vec<i32>>,
    real_input_fence_open: Mutex<Vec<bool>>,
}

impl RecordingExecutor {
    fn accepting() -> Self {
        Self {
            test_outcomes: Mutex::new(VecDeque::from([Ok(())])),
            submit_outcomes: Mutex::new(VecDeque::from([Ok(())])),
            requests: Mutex::new(Vec::new()),
            real_input_fence_fds: Mutex::new(Vec::new()),
            real_input_fence_open: Mutex::new(Vec::new()),
        }
    }

    fn record(&self, job: &KmsCommitJob, test_only: bool) {
        let (framebuffer_id, request_out_fence) = match &job.primary {
            KmsPrimaryUpdate::Framebuffer {
                framebuffer,
                request_out_fence,
                ..
            } => (framebuffer.get(), *request_out_fence),
            KmsPrimaryUpdate::Unchanged => (0, false),
        };
        self.requests.lock().unwrap().push(RecordedAtomicRequest {
            framebuffer_id,
            cursor: job.cursor.clone(),
            test_only,
            request_out_fence: !test_only && request_out_fence,
        });
    }

    fn next_outcome(
        outcomes: &Mutex<VecDeque<Result<(), oblivion_one::native::kms::AtomicKmsErrorKind>>>,
    ) -> Result<(), KmsWorkerSubmitFailure> {
        match outcomes.lock().unwrap().pop_front().unwrap_or(Ok(())) {
            Ok(()) => Ok(()),
            Err(kind) => Err(KmsWorkerSubmitFailure::new(kind, "fake Atomic ioctl")),
        }
    }
}

impl KmsCommitExecutor for RecordingExecutor {
    fn test_only(&self, job: &KmsCommitJob) -> Result<(), KmsWorkerSubmitFailure> {
        self.record(job, true);
        Self::next_outcome(&self.test_outcomes)
    }

    fn submit(&self, job: &KmsCommitJob) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
        self.record(job, false);
        let input_fence_fd = match &job.primary {
            KmsPrimaryUpdate::Framebuffer { in_fence, .. } => {
                in_fence.as_ref().map(AsRawFd::as_raw_fd)
            }
            KmsPrimaryUpdate::Unchanged => None,
        };
        self.real_input_fence_fds
            .lock()
            .unwrap()
            .push(input_fence_fd.unwrap_or(-1));
        self.real_input_fence_open
            .lock()
            .unwrap()
            .push(input_fence_fd.is_some_and(|fd| !fd_is_closed(fd)));
        Self::next_outcome(&self.submit_outcomes)?;
        Ok(KmsWorkerSubmission { out_fence: None })
    }
}

#[test]
fn direct_worker_tests_then_submits_the_same_primary_and_cursor_state() {
    let executor = Arc::new(RecordingExecutor::accepting());
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let job = required_direct_test_job(34);
    let token = job.token;
    let transaction_id = job.transaction_id;
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();

    let events = wait_for_fence_event(
        &handle,
        34,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 34),
    );
    let requests = executor.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].test_only);
    assert!(!requests[1].test_only);
    assert_eq!(requests[0].framebuffer_id, requests[1].framebuffer_id);
    assert_eq!(requests[0].cursor, requests[1].cursor);
    assert!(!requests[0].request_out_fence);
    assert!(requests[1].request_out_fence);
    assert!(events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::Submitted { ownership }
            if ownership.job.test_only_duration_ns.is_some()
    )));

    handle.ack_pageflip(token, transaction_id, 1).unwrap();
    drop(events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn direct_test_rejection_prevents_real_submit() {
    let executor = Arc::new(RecordingExecutor {
        test_outcomes: Mutex::new(VecDeque::from([Err(
            oblivion_one::native::kms::AtomicKmsErrorKind::TestOnlyRejected,
        )])),
        submit_outcomes: Mutex::new(VecDeque::from([Ok(())])),
        requests: Mutex::new(Vec::new()),
        real_input_fence_fds: Mutex::new(Vec::new()),
        real_input_fence_open: Mutex::new(Vec::new()),
    });
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let job = required_direct_test_job(35);
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();

    let events = wait_for_fence_event(
        &handle,
        35,
        |event| matches!(event, KmsWorkerEvent::TestRejected { job, .. } if job.token.get() == 35),
    );
    let requests = executor.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].test_only);
    assert!(events.iter().any(|event| {
        matches!(event, KmsWorkerEvent::TestRejected { job, .. } if job.token.get() == 35)
    }));
    assert!(events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::TestRejected { job, .. }
            if job.test_only_duration_ns.is_some()
    )));

    drop(events);
    handle.request_quiesce();
    handle.join().unwrap();
}

#[test]
fn direct_test_only_does_not_consume_input_fence() {
    let executor = Arc::new(RecordingExecutor::accepting());
    let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
    let fence = test_input_fence();
    let raw_fd = fence.as_raw_fd();
    let original_identity = fd_identity(raw_fd);
    let mut job = required_direct_test_job(36);
    job.primary = KmsPrimaryUpdate::Framebuffer {
        framebuffer: oblivion_one::native::kms::FramebufferId::new(42).unwrap(),
        in_fence: Some(fence),
        request_out_fence: true,
    };
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();

    let events = wait_for_fence_event(
        &handle,
        36,
        |event| matches!(event, KmsWorkerEvent::Submitted { ownership } if ownership.job.token.get() == 36),
    );
    assert_eq!(*executor.real_input_fence_fds.lock().unwrap(), vec![raw_fd]);
    assert_eq!(*executor.real_input_fence_open.lock().unwrap(), vec![true]);
    assert_eq!(executor.requests.lock().unwrap().len(), 2);

    handle
        .ack_pageflip(test_job(36).token, test_job(36).transaction_id, 1)
        .unwrap();
    drop(events);
    for _ in 0..100 {
        if fd_is_closed_or_reused(raw_fd, original_identity.as_deref()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(fd_is_closed_or_reused(raw_fd, original_identity.as_deref()));
    handle.request_quiesce();
    handle.join().unwrap();
}
