use super::cycle::direct_fallback::DirectFallbackTracker;
use super::kms_worker::{WorkerQueueOutcome, queue_explicit_composited_frame, queue_plane_delta};
pub(super) use super::plane_cycle::cursor_worker_opportunities;
use super::presentation_transactions::{
    DirectTerminalCallbackDisposition, direct_terminal_callback_owner_leaks,
    settle_failed_output_transaction, submit_plane_delta,
};
use super::*;
use crate::native_output::kms_worker::{
    KmsBundleOwners, KmsCommitAdmissionPermit, KmsCommitJob, KmsCommitWorkerHandle,
    KmsCursorUpdate, KmsPrimaryUpdate, KmsTestOnlyPolicy, KmsWorkerAdmissionError,
};
use oblivion_one::native::kms::FramebufferId;

pub(super) fn can_queue_worker_primary(
    worker_mode: bool,
    decision: SchedulerDecision,
    pipeline: Option<&OutputPipelineSnapshot>,
    worker: Option<&KmsCommitWorkerHandle>,
) -> bool {
    worker_mode
        && matches!(
            decision,
            SchedulerDecision::Render
                | SchedulerDecision::SubmitReady
                | SchedulerDecision::SubmitReadyLate
        )
        && pipeline.is_some_and(OutputPipelineSnapshot::can_pre_admit_primary)
        && worker.is_some_and(KmsCommitWorkerHandle::admission_available)
}

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

fn planned_cursor_update(
    output_transactions: &OutputTransactionLedger,
    transaction_id: OutputTransactionId,
) -> NativeResult<KmsCursorUpdate> {
    let transaction = output_transactions
        .transaction(transaction_id)
        .ok_or_else(|| io::Error::other("ready transaction disappeared before cursor planning"))?;
    match transaction.descriptor().planes().cursor() {
        CursorPlaneAssignment::Atomic {
            state: Some(state), ..
        } => Ok(KmsCursorUpdate::Set(state.clone())),
        CursorPlaneAssignment::Atomic { state: None, .. } | CursorPlaneAssignment::Disabled => {
            Ok(KmsCursorUpdate::Disable)
        }
        CursorPlaneAssignment::Unchanged => Ok(KmsCursorUpdate::Unchanged),
    }
}

pub(super) struct WorkerPrimarySubmissionContext<'a> {
    atomic_cursor: Option<&'a NativeAtomicCursor>,
    frame_pacing: &'a mut NativeFramePacing,
}

pub(super) fn worker_ctx<'a>(
    atomic_cursor: Option<&'a NativeAtomicCursor>,
    frame_pacing: &'a mut NativeFramePacing,
) -> WorkerPrimarySubmissionContext<'a> {
    WorkerPrimarySubmissionContext {
        atomic_cursor,
        frame_pacing,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DirectWorkerQueueResult {
    Queued,
    AdmissionRejected,
}

#[derive(Debug)]
struct DirectWorkerAdmissionGuard {
    transaction_id: OutputTransactionId,
    token: PageFlipToken,
    pacing_frame_id: Option<u64>,
    ready_submit: bool,
    transaction_queued: bool,
    arbiter_reserved: bool,
    worker_permit: Option<KmsCommitAdmissionPermit>,
}

impl DirectWorkerAdmissionGuard {
    fn new(
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
        pacing_frame_id: Option<u64>,
        ready_submit: bool,
        worker_permit: KmsCommitAdmissionPermit,
    ) -> Self {
        Self {
            transaction_id,
            token,
            pacing_frame_id,
            ready_submit,
            transaction_queued: false,
            arbiter_reserved: false,
            worker_permit: Some(worker_permit),
        }
    }

    fn rollback(
        &mut self,
        frame_pacing: &mut NativeFramePacing,
        atomic_commit_arbiter: &mut AtomicCommitArbiter,
        output_transactions: &mut OutputTransactionLedger,
    ) -> Result<(), String> {
        let mut failures = Vec::new();
        if let Some(frame_id) = self.pacing_frame_id
            && !frame_pacing.cancel_worker_submission(Some(frame_id), self.ready_submit)
        {
            failures.push("pacing reservation identity mismatch".to_string());
        }
        if self.arbiter_reserved
            && atomic_commit_arbiter
                .reject_worker_queued(self.token)
                .is_none()
        {
            failures.push("arbiter rollback identity mismatch".to_string());
        }
        if self.transaction_queued
            && let Err(error) = output_transactions.rollback_queued(self.transaction_id)
        {
            failures.push(format!("transaction rollback failed: {error}"));
        }
        self.worker_permit.take();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }

    fn commit(&mut self) {
        debug_assert!(self.transaction_queued);
        debug_assert!(self.arbiter_reserved);
        debug_assert!(self.worker_permit.is_none());
    }
}

#[allow(clippy::too_many_arguments)]
fn quarantine_direct_admission_failure(
    emergency_quarantined_worker_jobs: &mut Vec<KmsCommitJob>,
    direct_fallback_tracker: &mut Option<DirectFallbackTracker>,
    worker: &KmsCommitWorkerHandle,
    guard: DirectWorkerAdmissionGuard,
    job: KmsCommitJob,
    frame_scheduler: &mut NativeFrameScheduler,
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    scanout: &mut NativeScanoutBackend,
    reason: String,
) -> NativeResult<DirectWorkerQueueResult> {
    drop(guard);
    emergency_quarantined_worker_jobs.push(job);
    worker.mark_admission_fatal();
    scanout.note_direct_fallback_cycles(0);
    *direct_fallback_tracker = None;
    frame_scheduler.abandon_for_session_suspend();
    atomic_commit_arbiter.abandon_for_recovery();
    scanout.suspend_page_flip()?;
    Err(io::Error::other(format!(
        "direct worker admission rollback could not prove cancellation: {reason}"
    ))
    .into())
}

fn settle_failed_direct_worker_transaction(
    scanout: &mut NativeScanoutBackend,
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    transaction_id: OutputTransactionId,
    protocol_batch_id: oblivion_one::compositor::CompositorFrameBatchId,
    stage: OutputTransactionFailureStage,
    at: MonotonicTimestampNs,
) -> NativeResult<()> {
    let obligations = output_transactions
        .transaction(transaction_id)
        .ok_or_else(|| io::Error::other("direct worker transaction disappeared"))?
        .descriptor()
        .obligations();
    let callback_owner_leaks = direct_terminal_callback_owner_leaks(
        server,
        transaction_id,
        obligations,
        DirectTerminalCallbackDisposition::Retryable,
    );
    settle_failed_output_transaction(
        output_transactions,
        transaction_id,
        stage,
        at,
        |obligations| {
            if obligations.frame_batch_id() == Some(protocol_batch_id) {
                server.restore_frame_batch_after_render_failure(protocol_batch_id);
            }
            Ok(())
        },
    )?;
    scanout.note_direct_callback_owner_leaks(callback_owner_leaks);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn queue_plane_delta_for_presentation(
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
    match queue_plane_delta(
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
        WorkerQueueOutcome::CursorQueued { .. } | WorkerQueueOutcome::SidecarQueued { .. } => {
            Ok(SchedulerDecision::WaitForPageFlip)
        }
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
        let decision = queue_plane_delta_for_presentation(
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
    let decision = submit_plane_delta(
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
    cursor_update: KmsCursorUpdate,
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
        cursor_update,
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
        WorkerQueueOutcome::CursorQueued { .. } | WorkerQueueOutcome::SidecarQueued { .. } => Err(
            io::Error::other("composited worker admission returned a cursor queue result").into(),
        ),
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
    context: WorkerPrimarySubmissionContext<'_>,
    ready_submit: bool,
) -> NativeResult<Option<(u64, u32, OutputTransactionId, bool)>> {
    if worker_mode {
        let worker = worker.ok_or_else(|| io::Error::other("worker transport has no worker"))?;
        let cursor_update = planned_cursor_update(output_transactions, transaction_id)?;
        let cursor_pin = match &cursor_update {
            KmsCursorUpdate::Set(state) => worker_cursor_pin(context.atomic_cursor, Some(state))?,
            KmsCursorUpdate::Unchanged | KmsCursorUpdate::Disable => None,
        };
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
            cursor_update,
            cursor_pin,
            pacing_frame_id,
            ready_submit,
        )?
        .map(|(token, framebuffer_id, transaction_id)| {
            (token, framebuffer_id, transaction_id, true)
        }));
    }
    let (token, framebuffer_id, transaction_id) =
        explicit.submit_ready_frame(kms_backend, server, output_transactions)?;
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
        WorkerQueueOutcome::CursorQueued { .. } | WorkerQueueOutcome::SidecarQueued { .. } => Err(
            io::Error::other("compatibility worker admission returned a cursor queue result")
                .into(),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn finish_direct_worker_queued(
    worker: &KmsCommitWorkerHandle,
    scanout: &mut NativeScanoutBackend,
    emergency_quarantined_worker_jobs: &mut Vec<KmsCommitJob>,
    direct_fallback_tracker: &mut Option<DirectFallbackTracker>,
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
    admission: KmsCommitAdmissionPermit,
    test_only: KmsTestOnlyPolicy,
) -> NativeResult<DirectWorkerQueueResult> {
    let commit_token = PageFlipToken::new(token)
        .ok_or_else(|| io::Error::other("Direct Atomic worker token is zero"))?;
    let kind = AtomicCommitKind::DirectPrimary {
        transaction_id,
        direct_token: commit_token,
        framebuffer_id,
    };
    let queued_at_ns = monotonic_now_ns()?;
    let pacing_frame_id = context.frame_pacing.worker_submission_frame_id(false);
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
    let job = KmsCommitJob {
        bundle_id:
            crate::native_output::presentation::plane::KmsCommitBundleId::from_pageflip_token(
                commit_token,
            ),
        owners: KmsBundleOwners::for_legacy_transaction(
            kind,
            std::sync::Arc::new(
                output_transactions
                    .transaction(transaction_id)
                    .ok_or_else(|| io::Error::other("direct worker transaction disappeared"))?
                    .descriptor()
                    .clone(),
            ),
        ),
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
        test_only_duration_ns: None,
        pacing_frame_id,
        test_only,
        ready_submit: false,
    };
    debug_assert!(job.direct_primary_lease.is_some());
    debug_assert!(matches!(job.kind, AtomicCommitKind::DirectPrimary { .. }));
    let descriptor = output_transactions
        .transaction(transaction_id)
        .ok_or_else(|| io::Error::other("direct worker transaction disappeared"))?;
    if let Err(error) = job.validate_against(descriptor.descriptor()) {
        settle_failed_direct_worker_transaction(
            scanout,
            server,
            output_transactions,
            transaction_id,
            protocol_batch_id,
            OutputTransactionFailureStage::BackendOwnershipTransfer,
            MonotonicTimestampNs::new(queued_at_ns),
        )?;
        drop(job);
        return Err(io::Error::other(format!("invalid direct worker payload: {error:?}")).into());
    }
    let mut guard = DirectWorkerAdmissionGuard::new(
        transaction_id,
        commit_token,
        pacing_frame_id,
        false,
        admission,
    );
    if let Err(error) = output_transactions.mark_queued(
        transaction_id,
        output_generation,
        MonotonicTimestampNs::new(queued_at_ns),
    ) {
        let rollback = guard.rollback(
            context.frame_pacing,
            atomic_commit_arbiter,
            output_transactions,
        );
        if let Err(reason) = rollback {
            return quarantine_direct_admission_failure(
                emergency_quarantined_worker_jobs,
                direct_fallback_tracker,
                worker,
                guard,
                job,
                frame_scheduler,
                atomic_commit_arbiter,
                scanout,
                reason,
            );
        }
        settle_failed_direct_worker_transaction(
            scanout,
            server,
            output_transactions,
            transaction_id,
            protocol_batch_id,
            OutputTransactionFailureStage::KmsSubmit,
            MonotonicTimestampNs::new(queued_at_ns),
        )?;
        drop(job);
        return Err(io::Error::other(error).into());
    }
    guard.transaction_queued = true;
    if let Err(error) = atomic_commit_arbiter.reserve_worker_queued(
        commit_token,
        output_generation,
        crtc_id,
        kind,
        queued_at_ns,
    ) {
        let rollback = guard.rollback(
            context.frame_pacing,
            atomic_commit_arbiter,
            output_transactions,
        );
        if let Err(reason) = rollback {
            return quarantine_direct_admission_failure(
                emergency_quarantined_worker_jobs,
                direct_fallback_tracker,
                worker,
                guard,
                job,
                frame_scheduler,
                atomic_commit_arbiter,
                scanout,
                reason,
            );
        }
        settle_failed_direct_worker_transaction(
            scanout,
            server,
            output_transactions,
            transaction_id,
            protocol_batch_id,
            OutputTransactionFailureStage::BackendOwnershipTransfer,
            MonotonicTimestampNs::new(monotonic_now_ns()?),
        )?;
        drop(job);
        return Err(io::Error::other(error).into());
    }
    guard.arbiter_reserved = true;
    let admission = guard
        .worker_permit
        .take()
        .expect("direct worker admission permit was just retained");
    if let Err(error) = admission.enqueue(job) {
        let returned_job = error.job;
        scanout.note_direct_worker_admission_rejected(matches!(
            error.reason,
            KmsWorkerAdmissionError::QueueFull
        ));
        let rollback = guard.rollback(
            context.frame_pacing,
            atomic_commit_arbiter,
            output_transactions,
        );
        if let Err(reason) = rollback {
            return quarantine_direct_admission_failure(
                emergency_quarantined_worker_jobs,
                direct_fallback_tracker,
                worker,
                guard,
                returned_job,
                frame_scheduler,
                atomic_commit_arbiter,
                scanout,
                reason,
            );
        }
        settle_failed_direct_worker_transaction(
            scanout,
            server,
            output_transactions,
            transaction_id,
            protocol_batch_id,
            OutputTransactionFailureStage::BackendOwnershipTransfer,
            MonotonicTimestampNs::new(monotonic_now_ns()?),
        )?;
        drop(returned_job);
        return Ok(DirectWorkerQueueResult::AdmissionRejected);
    }
    guard.commit();
    presentation_trace.push(PresentationTransactionEvent::WorkerQueued {
        transaction_id,
        timestamp_ns: queued_at_ns,
    });
    *last_rendered_scene_generation = scene_generation;
    *last_renderable_surfaces = server.renderable_surfaces().to_vec();
    *last_software_cursor_damage = current_software_cursor_damage;
    *frame_index = frame_index.saturating_add(1);
    *frame_submitted = true;
    Ok(DirectWorkerQueueResult::Queued)
}

pub(super) fn worker_cursor_queue_available(
    worker_mode: bool,
    worker: Option<&KmsCommitWorkerHandle>,
    arbiter: &AtomicCommitArbiter,
) -> bool {
    worker_mode
        && worker.is_some_and(|worker| {
            worker.has_attachable_primary_opportunity()
                || (arbiter.kernel_commit_submitted()
                    && arbiter.worker_slot_available()
                    && arbiter
                        .kernel_submitted_kind()
                        .is_some_and(|kind| !matches!(kind, AtomicCommitKind::PlaneDelta { .. }))
                    && worker.admission_available())
        })
}
