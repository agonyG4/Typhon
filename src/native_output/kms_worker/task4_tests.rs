use super::tests::{
    fd_identity, fd_is_closed, fd_is_closed_or_reused, reserve_for_test, test_input_fence,
    test_job, wait_for_fence_event,
};
use super::thread::{KmsCommitExecutor, KmsWorkerSubmission, KmsWorkerSubmitFailure};
use super::{
    CursorSidecar, CursorSidecarCoupling, CursorSidecarMailbox, KmsBundleOwners,
    KmsCommitBundleIdentity, KmsCommitJob, KmsCommitWorkerHandle, KmsCursorOwner, KmsCursorUpdate,
    KmsPrimaryOwner, KmsPrimaryUpdate, KmsSubmittedOwnership, KmsTestOnlyPolicy, KmsValidationBase,
    KmsWorkerEvent, KmsWorkerFatalJob,
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
        KmsValidationBase::Presented(_)
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
        KmsValidationBase::Presented(
            crate::native_output::presentation::plane::PresentedPlaneSnapshot::legacy(None)
        ),
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
    offer_sidecar(&handle, sidecar);
    reserve_for_test(&handle, job.kind).enqueue(job).unwrap();

    let events = wait_for_fence_event(&handle, 339, |event| {
        matches!(event, KmsWorkerEvent::Submitted { .. })
    });
    assert!(events.iter().any(|event| matches!(
        event,
        KmsWorkerEvent::Submitted { ownership }
            if ownership.job.owners.cursor().and_then(|owner| owner.sidecar_id) == Some(id)
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

    let second = test_job(341);
    let second_token = second.token;
    let second_transaction = second.transaction_id;
    reserve_for_test(&handle, second.kind)
        .enqueue(second)
        .unwrap();
    let older = test_sidecar(&test_job(341), 909, CursorSidecarCoupling::Independent);
    let latest = test_sidecar(&test_job(341), 910, CursorSidecarCoupling::Independent);
    let latest_id = latest.id;
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
