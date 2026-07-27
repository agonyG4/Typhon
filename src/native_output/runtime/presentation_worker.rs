use super::kms_worker::{WorkerQueueOutcome, queue_cursor_only, queue_explicit_composited_frame};
use super::presentation_transactions::submit_cursor_only;
use super::*;
use crate::native_output::kms_worker::{
    KmsCommitJob, KmsCommitWorkerHandle, KmsCursorUpdate, KmsPrimaryUpdate, KmsTestOnlyPolicy,
};
use oblivion_one::native::kms::FramebufferId;

pub(super) fn worker_cursor_pin(
    atomic_cursor: Option<&NativeAtomicCursor>,
    cursor: Option<&AtomicCursorVisualState>,
) -> NativeResult<Option<CursorFramebufferPin>> {
    match (atomic_cursor, cursor) {
        (Some(native_cursor), Some(state)) if state.framebuffer_id.is_some() => {
            Ok(Some(native_cursor.pin_framebuffer_for(state)?))
        }
        _ => Ok(None),
    }
}

pub(super) struct WorkerPrimarySubmissionContext<'a> {
    atomic_cursor: Option<&'a NativeAtomicCursor>,
    frame_pacing: &'a NativeFramePacing,
}

pub(super) fn worker_ctx<'a>(
    atomic_cursor: Option<&'a NativeAtomicCursor>,
    frame_pacing: &'a NativeFramePacing,
) -> WorkerPrimarySubmissionContext<'a> {
    WorkerPrimarySubmissionContext {
        atomic_cursor,
        frame_pacing,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn queue_cursor_only_for_presentation(
    worker: &KmsCommitWorkerHandle,
    cursor: &mut NativeAtomicCursor,
    desired: Option<AtomicCursorVisualState>,
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    output_transactions: &mut OutputTransactionLedger,
    presentation_trace: &mut PresentationTransactionTraceRing,
    cursor_target: PresentationTarget,
    crtc_id: u32,
    output_generation: u64,
    pacing_mode: NativeOutputPacingMode,
    cursor_epoch: u64,
) -> NativeResult<SchedulerDecision> {
    match queue_cursor_only(
        worker,
        cursor,
        desired,
        atomic_commit_arbiter,
        output_transactions,
        presentation_trace,
        cursor_target,
        crtc_id,
        output_generation,
        pacing_mode,
        cursor_epoch,
    )? {
        WorkerQueueOutcome::CursorQueued { .. } => Ok(SchedulerDecision::WaitForPageFlip),
        WorkerQueueOutcome::Unavailable(_) => Ok(SchedulerDecision::Idle),
        WorkerQueueOutcome::Queued { .. } => {
            Err(io::Error::other("cursor worker admission returned a primary queue result").into())
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn present_cursor_for_presentation(
    worker_mode: bool,
    worker: Option<&KmsCommitWorkerHandle>,
    kms_backend: &KmsBackendSelection,
    cursor: &mut NativeAtomicCursor,
    desired: Option<AtomicCursorVisualState>,
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    output_transactions: &mut OutputTransactionLedger,
    presentation_trace: &mut PresentationTransactionTraceRing,
    cursor_target: PresentationTarget,
    crtc_id: u32,
    output_generation: u64,
    pacing_mode: NativeOutputPacingMode,
    cursor_epoch: u64,
    last_submitted_cursor_epoch: &mut u64,
    cursor_output_arbitration: &mut NativeCursorOutputArbitration,
    frame_scheduler: &mut NativeFrameScheduler,
    pacing_now_ns: u64,
    perf: NativePerfLogger,
    client_cursor_active: bool,
    cursor_render_mode: &mut NativeCursorRenderMode,
    effective_cursor: &mut Option<AtomicCursorVisualState>,
    queued_redraw_requested: &mut bool,
    last_client_cursor_damage: &mut Option<NativeClientCursorDamageState>,
    last_software_cursor_damage: &mut Option<NativeDamageRect>,
    current_client_cursor_damage: Option<NativeClientCursorDamageState>,
    current_software_cursor_damage: Option<NativeDamageRect>,
) -> NativeResult<Option<SchedulerDecision>> {
    if worker_mode {
        let worker = worker.ok_or_else(|| io::Error::other("worker transport has no worker"))?;
        let decision = queue_cursor_only_for_presentation(
            worker,
            cursor,
            desired,
            atomic_commit_arbiter,
            output_transactions,
            presentation_trace,
            cursor_target,
            crtc_id,
            output_generation,
            pacing_mode,
            cursor_epoch,
        )?;
        return Ok((decision != SchedulerDecision::Idle).then_some(decision));
    }
    let decision = submit_cursor_only(
        kms_backend,
        cursor,
        desired,
        atomic_commit_arbiter,
        output_transactions,
        presentation_trace,
        cursor_target,
        crtc_id,
        output_generation,
        pacing_mode,
        cursor_epoch,
        cursor_output_arbitration,
        frame_scheduler,
        pacing_now_ns,
        perf,
        client_cursor_active,
        cursor_render_mode,
        effective_cursor,
        queued_redraw_requested,
        last_client_cursor_damage,
        last_software_cursor_damage,
        current_client_cursor_damage,
        current_software_cursor_damage,
    )?;
    if decision == SchedulerDecision::WaitForPageFlip {
        *last_submitted_cursor_epoch = cursor_epoch;
    }
    Ok(Some(decision))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn queue_explicit_ready_for_presentation(
    worker: &KmsCommitWorkerHandle,
    explicit: &mut AtomicEglGbmScanout,
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    presentation_trace: &mut PresentationTransactionTraceRing,
    transaction_id: OutputTransactionId,
    output_generation: u64,
    crtc_id: u32,
    cursor: Option<&AtomicCursorVisualState>,
    cursor_pin: Option<CursorFramebufferPin>,
    pacing_frame_id: Option<u64>,
    ready_submit: bool,
) -> NativeResult<Option<(u64, u32, OutputTransactionId)>> {
    match queue_explicit_composited_frame(
        worker,
        explicit,
        server,
        output_transactions,
        atomic_commit_arbiter,
        presentation_trace,
        transaction_id,
        output_generation,
        crtc_id,
        cursor,
        cursor_pin,
        pacing_frame_id,
        ready_submit,
    )? {
        WorkerQueueOutcome::Queued {
            transaction_id,
            token,
            framebuffer_id,
        } => Ok(Some((token.get(), framebuffer_id.get(), transaction_id))),
        WorkerQueueOutcome::Unavailable(_) => Ok(None),
        WorkerQueueOutcome::CursorQueued { .. } => Err(io::Error::other(
            "composited worker admission returned a cursor queue result",
        )
        .into()),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn submit_explicit_ready_for_presentation(
    worker_mode: bool,
    worker: Option<&KmsCommitWorkerHandle>,
    explicit: &mut AtomicEglGbmScanout,
    kms_backend: &KmsBackendSelection,
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    presentation_trace: &mut PresentationTransactionTraceRing,
    transaction_id: OutputTransactionId,
    output_generation: u64,
    crtc_id: u32,
    cursor: Option<&AtomicCursorVisualState>,
    context: WorkerPrimarySubmissionContext<'_>,
    ready_submit: bool,
) -> NativeResult<Option<(u64, u32, OutputTransactionId, bool)>> {
    if worker_mode {
        let worker = worker.ok_or_else(|| io::Error::other("worker transport has no worker"))?;
        let cursor_pin = worker_cursor_pin(context.atomic_cursor, cursor)?;
        let pacing_frame_id = context
            .frame_pacing
            .worker_submission_frame_id(ready_submit);
        return Ok(queue_explicit_ready_for_presentation(
            worker,
            explicit,
            server,
            output_transactions,
            atomic_commit_arbiter,
            presentation_trace,
            transaction_id,
            output_generation,
            crtc_id,
            cursor,
            cursor_pin,
            pacing_frame_id,
            ready_submit,
        )?
        .map(|(token, framebuffer_id, transaction_id)| {
            (token, framebuffer_id, transaction_id, true)
        }));
    }
    let (token, framebuffer_id, transaction_id) =
        explicit.submit_ready_frame(kms_backend, server, output_transactions, cursor)?;
    Ok(Some((token, framebuffer_id, transaction_id, false)))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn queue_compatibility_for_presentation(
    worker: &KmsCommitWorkerHandle,
    scanout: &mut NativeScanoutBackend,
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    presentation_trace: &mut PresentationTransactionTraceRing,
    output_generation: u64,
    crtc_id: u32,
    target: PresentationTarget,
    pacing_mode: NativeOutputPacingMode,
    render_generation: u64,
    cursor: Option<&AtomicCursorVisualState>,
    cursor_pin: Option<CursorFramebufferPin>,
    pacing_frame_id: Option<u64>,
    cursor_epoch: u64,
) -> NativeResult<Option<(NativePresentResult, Option<OutputTransactionId>)>> {
    match super::kms_worker::queue_atomic_compatibility_frame(
        worker,
        scanout,
        server,
        output_transactions,
        atomic_commit_arbiter,
        presentation_trace,
        output_generation,
        crtc_id,
        target,
        pacing_mode,
        render_generation,
        cursor,
        cursor_pin,
        pacing_frame_id,
        cursor_epoch,
    )? {
        WorkerQueueOutcome::Queued {
            transaction_id,
            token,
            framebuffer_id,
        } => Ok(Some((
            NativePresentResult::AsyncSubmitted {
                token: token.get(),
                framebuffer_id: framebuffer_id.get(),
                transaction_id: Some(transaction_id),
            },
            None,
        ))),
        WorkerQueueOutcome::Unavailable(_) => Ok(None),
        WorkerQueueOutcome::CursorQueued { .. } => Err(io::Error::other(
            "compatibility worker admission returned a cursor queue result",
        )
        .into()),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn finish_direct_worker_queued(
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    presentation_trace: &mut PresentationTransactionTraceRing,
    frame_scheduler: &mut NativeFrameScheduler,
    _cursor_output_arbitration: &mut NativeCursorOutputArbitration,
    effective_cursor: Option<&AtomicCursorVisualState>,
    context: WorkerPrimarySubmissionContext<'_>,
    output_generation: u64,
    crtc_id: u32,
    scene_generation: u64,
    _cursor_epoch: u64,
    current_software_cursor_damage: Option<NativeDamageRect>,
    last_rendered_scene_generation: &mut u64,
    _last_submitted_cursor_epoch: &mut u64,
    last_renderable_surfaces: &mut Vec<RenderableSurface>,
    last_software_cursor_damage: &mut Option<NativeDamageRect>,
    frame_index: &mut u64,
    frame_submitted: &mut bool,
    transaction_id: OutputTransactionId,
    token: u64,
    framebuffer_id: u32,
    direct_target: PresentationTarget,
    direct_lease: DirectPrimaryLease,
    admission: crate::native_output::kms_worker::KmsCommitAdmissionPermit,
    test_only: KmsTestOnlyPolicy,
) -> NativeResult<()> {
    let commit_token = PageFlipToken::new(token)
        .ok_or_else(|| io::Error::other("Direct Atomic worker token is zero"))?;
    let kind = AtomicCommitKind::DirectPrimary {
        transaction_id,
        direct_token: commit_token,
        framebuffer_id,
    };
    let queued_at_ns = monotonic_now_ns()?;
    let protocol_batch_id = {
        let transaction = output_transactions
            .transaction(transaction_id)
            .ok_or_else(|| io::Error::other("direct worker transaction disappeared"))?;
        match transaction.descriptor().content() {
            OutputTransactionContent::Direct { .. } => {}
            _ => return Err(io::Error::other("direct worker transaction is not direct").into()),
        }
        transaction
            .descriptor()
            .obligations()
            .frame_batch_id()
            .ok_or_else(|| io::Error::other("direct worker transaction has no frame batch"))?
    };
    if let Err(error) = output_transactions.mark_queued(
        transaction_id,
        output_generation,
        MonotonicTimestampNs::new(queued_at_ns),
    ) {
        server.restore_frame_batch_after_render_failure(protocol_batch_id);
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::KmsSubmit,
            MonotonicTimestampNs::new(queued_at_ns),
            |_| Ok(()),
        )?;
        return Err(io::Error::other(error).into());
    }
    if let Err(error) = atomic_commit_arbiter.reserve_worker_queued(
        commit_token,
        output_generation,
        crtc_id,
        kind,
        queued_at_ns,
    ) {
        server.restore_frame_batch_after_render_failure(protocol_batch_id);
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::BackendOwnershipTransfer,
            MonotonicTimestampNs::new(monotonic_now_ns()?),
            |_| Ok(()),
        )?;
        return Err(io::Error::other(error).into());
    }
    let job = KmsCommitJob {
        transaction_id,
        token: commit_token,
        output_generation,
        crtc_id,
        kind,
        target: direct_target,
        queued_at: MonotonicTimestampNs::new(queued_at_ns),
        primary: KmsPrimaryUpdate::Framebuffer {
            framebuffer: FramebufferId::new(framebuffer_id)
                .ok_or_else(|| io::Error::other("Direct worker framebuffer ID is zero"))?,
            in_fence: None,
            request_out_fence: true,
        },
        cursor: effective_cursor.map_or(KmsCursorUpdate::Unchanged, |state| {
            KmsCursorUpdate::Set(state.clone())
        }),
        cursor_pin: worker_cursor_pin(context.atomic_cursor, effective_cursor)?,
        direct_primary_lease: Some(direct_lease),
        pacing_frame_id: context.frame_pacing.worker_submission_frame_id(false),
        test_only,
        ready_submit: false,
    };
    let descriptor = output_transactions
        .transaction(transaction_id)
        .ok_or_else(|| io::Error::other("direct worker transaction disappeared"))?;
    if let Err(error) = job.validate_against(descriptor.descriptor()) {
        let _ = atomic_commit_arbiter.reject_worker_queued(commit_token);
        server.restore_frame_batch_after_render_failure(protocol_batch_id);
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::BackendOwnershipTransfer,
            MonotonicTimestampNs::new(queued_at_ns),
            |_| Ok(()),
        )?;
        drop(job);
        return Err(io::Error::other(format!("invalid direct worker payload: {error:?}")).into());
    }
    if let Err(error) = admission.enqueue(job) {
        let _ = atomic_commit_arbiter.reject_worker_queued(commit_token);
        let returned_job = error.job;
        server.restore_frame_batch_after_render_failure(protocol_batch_id);
        settle_failed_output_transaction(
            output_transactions,
            transaction_id,
            OutputTransactionFailureStage::BackendOwnershipTransfer,
            MonotonicTimestampNs::new(monotonic_now_ns()?),
            |_| Ok(()),
        )?;
        drop(returned_job);
        return Err(
            io::Error::other(format!("direct worker enqueue failed: {:?}", error.reason)).into(),
        );
    }
    presentation_trace.push(PresentationTransactionEvent::WorkerQueued {
        transaction_id,
        timestamp_ns: queued_at_ns,
    });
    frame_scheduler
        .reserve_worker_submission(token, transaction_id.get())
        .map_err(io::Error::other)?;
    *last_rendered_scene_generation = scene_generation;
    *last_renderable_surfaces = server.renderable_surfaces().to_vec();
    *last_software_cursor_damage = current_software_cursor_damage;
    *frame_index = frame_index.saturating_add(1);
    *frame_submitted = true;
    Ok(())
}

pub(super) fn direct_worker_admission(
    worker_mode: bool,
    worker: Option<&KmsCommitWorkerHandle>,
) -> NativeResult<Option<crate::native_output::kms_worker::KmsCommitAdmissionPermit>> {
    if !worker_mode {
        return Ok(None);
    }
    let worker = worker
        .ok_or_else(|| io::Error::other("worker transport is active without a worker handle"))?;
    Ok(worker.try_reserve_admission_slot().ok())
}

pub(super) fn worker_cursor_queue_available(
    worker_mode: bool,
    worker: Option<&KmsCommitWorkerHandle>,
    arbiter: &AtomicCommitArbiter,
) -> bool {
    worker_mode
        && arbiter.kernel_commit_submitted()
        && arbiter.worker_slot_available()
        && arbiter
            .kernel_submitted_kind()
            .is_some_and(|kind| !matches!(kind, AtomicCommitKind::CursorOnly { .. }))
        && worker.is_some_and(|worker| worker.admission_available())
}
