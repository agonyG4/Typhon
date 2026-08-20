use super::cycle::direct_fallback::DirectFallbackTracker;
use super::kms_worker::{WorkerQueueOutcome, queue_explicit_composited_frame, queue_plane_delta};
pub(super) use super::plane_cycle::cursor_worker_opportunities;
use super::presentation_cursor::{RuntimePlanePlan, presented_delivery_for_plan};
use super::presentation_transactions::{
    DirectTerminalCallbackDisposition, direct_terminal_callback_owner_leaks,
    present_compatibility_frame, settle_failed_output_transaction, submit_plane_delta,
};
use super::*;
use crate::native_output::kms_worker::{
    KmsBundleOwners, KmsCommitAdmissionPermit, KmsCommitJob, KmsCommitTestPolicy,
    KmsCommitWorkerHandle, KmsCursorUpdate, KmsPrimaryCursorPresentation, KmsPrimaryUpdate,
    KmsTestOnlyPolicy, KmsValidationBase, KmsWorkerAdmissionError, PendingBundleSnapshot,
};
use crate::native_output::presentation::plane::{
    CursorRevision, FrozenCursorTestPolicy, FrozenPrimaryCursorPresentation,
};
use oblivion_one::native::kms::FramebufferId;

pub(super) fn record_composited_scene_identity(
    presentation_trace: &mut PresentationTransactionTraceRing,
    transaction_id: OutputTransactionId,
    frame_id: u64,
    render_generation: u64,
    resolved_scene_signature: u64,
    render_damage_signature: u64,
    repair_damage_signature: u64,
    presented_at_render_frame_id: Option<u64>,
    framebuffer_slot: u8,
    buffer_age: Option<u32>,
) -> NativeResult<()> {
    presentation_trace.push(PresentationTransactionEvent::SceneIdentity {
        transaction_id,
        timestamp_ns: monotonic_now_ns()?,
        frame_id,
        render_generation,
        resolved_scene_signature,
        snapshot_scene_signature: resolved_scene_signature,
        render_damage_signature,
        repair_damage_signature,
        presented_at_render_frame_id,
        framebuffer_slot,
        buffer_age,
    });
    Ok(())
}

pub(super) fn replace_atomic_ready_scene(
    scene_history: &mut NativeSceneHistory,
    resolved_snapshot: NativeSceneSnapshot,
    frame_id: u64,
    render_generation: u64,
    cursor: (
        Option<NativeClientCursorDamageState>,
        Option<NativeDamageRect>,
    ),
) {
    let cursor_damage = scene_history.cursor_damage(cursor);
    scene_history.replace_ready(NativeFrameSceneSnapshot {
        frame_id,
        render_generation,
        scene: resolved_snapshot,
        cursor_damage,
    });
}

pub(super) fn record_atomic_rendered_scene(
    scene_history: &mut NativeSceneHistory,
    presentation_trace: &mut PresentationTransactionTraceRing,
    frame_id: u64,
    transaction_id: OutputTransactionId,
    resolved_render_generation: u64,
    resolved_snapshot: NativeSceneSnapshot,
    resolved_scene_signature: u64,
    render_damage_signature: u64,
    repair_damage_signature: u64,
    presented_at_render_frame_id: Option<u64>,
    framebuffer_slot: u8,
    cursor: (
        Option<NativeClientCursorDamageState>,
        Option<NativeDamageRect>,
    ),
    buffer_age: Option<u32>,
) -> NativeResult<()> {
    replace_atomic_ready_scene(
        scene_history,
        resolved_snapshot,
        frame_id,
        resolved_render_generation,
        cursor,
    );
    record_composited_scene_identity(
        presentation_trace,
        transaction_id,
        frame_id,
        resolved_render_generation,
        resolved_scene_signature,
        render_damage_signature,
        repair_damage_signature,
        presented_at_render_frame_id,
        framebuffer_slot,
        buffer_age,
    )
}

pub(super) fn resolve_scene_and_damage<'a>(
    direct: bool,
    width: u32,
    height: u32,
    scene_history: &NativeSceneHistory,
    server: &'a OwnCompositorServer,
    cursor: (
        Option<NativeClientCursorDamageState>,
        Option<NativeDamageRect>,
    ),
) -> (ResolvedNativeFrameScene<'a>, NativeOutputDamage) {
    let resolved_scene = ResolvedNativeFrameScene::from_server(server);
    let output_damage = native_output_damage_for_presented_scene(
        direct,
        width,
        height,
        scene_history,
        &resolved_scene,
        cursor,
    );
    (resolved_scene, output_damage)
}

pub(super) fn replace_ready_scene_and_signature(
    scene_history: &mut NativeSceneHistory,
    resolved_scene: &ResolvedNativeFrameScene<'_>,
    frame_index: u64,
    cursor: (
        Option<NativeClientCursorDamageState>,
        Option<NativeDamageRect>,
    ),
) -> u64 {
    replace_ready_scene(
        scene_history,
        resolved_scene,
        frame_index,
        cursor.0,
        cursor.1,
    );
    resolved_scene.scene_identity_signature()
}

pub(super) fn record_compatibility_scene_identity(
    presentation_trace: &mut PresentationTransactionTraceRing,
    scanout: &NativeScanoutBackend,
    transaction_id: Option<OutputTransactionId>,
    frame_id: u64,
    render_generation: u64,
    resolved_scene_signature: u64,
) -> NativeResult<()> {
    let Some(transaction_id) = transaction_id else {
        return Ok(());
    };
    let buffer = scanout.buffer_snapshot();
    let framebuffer_slot = buffer
        .pending
        .or(buffer.current)
        .and_then(|slot| u8::try_from(slot).ok())
        .unwrap_or(0);
    record_composited_scene_identity(
        presentation_trace,
        transaction_id,
        frame_id,
        render_generation,
        resolved_scene_signature,
        0,
        0,
        None,
        framebuffer_slot,
        None,
    )
}

pub(super) fn record_immediate_scene_identity(
    presentation_trace: &mut PresentationTransactionTraceRing,
    transaction_id: Option<OutputTransactionId>,
    frame_id: u64,
    render_generation: u64,
    resolved_scene_signature: u64,
) -> NativeResult<OutputTransactionId> {
    let transaction_id = transaction_id.ok_or_else(|| {
        io::Error::other("immediate compatibility presentation has no transaction")
    })?;
    record_composited_scene_identity(
        presentation_trace,
        transaction_id,
        frame_id,
        render_generation,
        resolved_scene_signature,
        0,
        0,
        None,
        0,
        None,
    )?;
    Ok(transaction_id)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn present_composited_compatibility_frame(
    scanout: &mut NativeScanoutBackend,
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    output_generation: u64,
    crtc_id: u32,
    presentation_deadline: &PresentationDeadlinePlanner,
    scheduled_presentation_target: Option<PresentationTarget>,
    scheduler_now: MonotonicTimestampNs,
    pacing_mode: NativeOutputPacingMode,
    render_generation: u64,
    effective_cursor: Option<&AtomicCursorVisualState>,
    cursor_epoch: u64,
    frame_id: u64,
    kms_backend: &KmsBackendSelection,
    scene_history: &mut NativeSceneHistory,
) -> NativeResult<(NativePresentResult, Option<OutputTransactionId>)> {
    let compatibility_target = scheduled_presentation_target
        .or_else(|| presentation_deadline.reactive_target(scheduler_now))
        .ok_or_else(|| {
            io::Error::other("compatibility pageflip started without a presentation target")
        })?;
    let result = present_compatibility_frame(
        scanout,
        server,
        output_transactions,
        output_generation,
        crtc_id,
        compatibility_target,
        pacing_mode,
        render_generation,
        effective_cursor,
        cursor_epoch,
        frame_id,
        Some(kms_backend),
        |scanout, presentation_mode| {
            scanout.present(kms_backend, effective_cursor, presentation_mode)
        },
    );
    if result.is_err() {
        scene_history.discard_ready();
    }
    result
}

pub(super) fn native_scene_damage_for_resolved_scene(
    width: u32,
    height: u32,
    previous_scene: &NativeSceneSnapshot,
    resolved_scene: &ResolvedNativeFrameScene<'_>,
    cursor_damage: NativeCursorDamageBounds,
) -> NativeOutputDamage {
    native_output_damage_for_scene_snapshots(
        width,
        height,
        previous_scene,
        &resolved_scene.snapshot(),
        cursor_damage,
    )
}

pub(super) fn native_output_damage_for_resolved_scene(
    direct: bool,
    width: u32,
    height: u32,
    previous_scene: &NativeSceneSnapshot,
    resolved_scene: &ResolvedNativeFrameScene<'_>,
    cursor_damage: NativeCursorDamageBounds,
) -> NativeOutputDamage {
    if direct {
        NativeOutputDamage::full_output(width, height)
    } else {
        native_scene_damage_for_resolved_scene(
            width,
            height,
            previous_scene,
            resolved_scene,
            cursor_damage,
        )
    }
}

pub(super) fn native_output_damage_for_presented_scene(
    direct: bool,
    width: u32,
    height: u32,
    scene_history: &NativeSceneHistory,
    resolved_scene: &ResolvedNativeFrameScene<'_>,
    cursor: (
        Option<NativeClientCursorDamageState>,
        Option<NativeDamageRect>,
    ),
) -> NativeOutputDamage {
    let Some(previous_scene) = scene_history.presented_scene_if_any() else {
        return NativeOutputDamage::full_output(width, height);
    };
    native_output_damage_for_resolved_scene(
        direct,
        width,
        height,
        previous_scene,
        resolved_scene,
        scene_history.cursor_damage(cursor),
    )
}

pub(super) fn replace_ready_scene(
    scene_history: &mut NativeSceneHistory,
    resolved_scene: &ResolvedNativeFrameScene<'_>,
    frame_index: u64,
    current_client_cursor_damage: Option<NativeClientCursorDamageState>,
    current_software_cursor_damage: Option<NativeDamageRect>,
) {
    scene_history.replace_ready(NativeFrameSceneSnapshot::from_resolved_frame_scene(
        frame_index,
        resolved_scene,
        scene_history.cursor_damage((current_client_cursor_damage, current_software_cursor_damage)),
    ));
}

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

pub(super) fn validation_base_for_submission(
    worker: Option<&KmsCommitWorkerHandle>,
    presented_planes: crate::native_output::presentation::plane::PresentedPlaneSnapshot,
    output_generation: u64,
    crtc_id: u32,
) -> Option<KmsValidationBase> {
    worker
        .and_then(|worker| worker.pending_bundle_snapshot(output_generation, crtc_id))
        .map_or_else(
            || {
                Some(KmsValidationBase::Presented {
                    snapshot: presented_planes,
                    output_generation,
                    crtc_id,
                })
            },
            |snapshot| match snapshot {
                PendingBundleSnapshot::MutablePreFreeze { .. } => None,
                PendingBundleSnapshot::Frozen(identity)
                | PendingBundleSnapshot::InFlight(identity) => {
                    Some(KmsValidationBase::Predecessor(identity))
                }
            },
        )
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
    validation_base: KmsValidationBase,
    cursor_delivery: crate::native_output::presentation::plane::PresentedCursorDelivery,
    primary_cursor_presentation: KmsPrimaryCursorPresentation,
}

pub(super) fn worker_ctx<'a>(
    atomic_cursor: Option<&'a NativeAtomicCursor>,
    frame_pacing: &'a mut NativeFramePacing,
    validation_base: KmsValidationBase,
    cursor_delivery: crate::native_output::presentation::plane::PresentedCursorDelivery,
    primary_cursor_presentation: KmsPrimaryCursorPresentation,
) -> WorkerPrimarySubmissionContext<'a> {
    WorkerPrimarySubmissionContext {
        atomic_cursor,
        frame_pacing,
        validation_base,
        cursor_delivery,
        primary_cursor_presentation,
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
    validation_base: KmsValidationBase,
    attachable_primary: Option<crate::native_output::kms_worker::AttachablePrimary>,
    cursor_action: crate::native_output::presentation::plane_policy::CursorPlaneAction,
    cursor_delivery: crate::native_output::presentation::plane::PresentedCursorDelivery,
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
        validation_base,
        attachable_primary,
        cursor_action,
        cursor_delivery,
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
    validation_base: KmsValidationBase,
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
    plane_plan: Option<&RuntimePlanePlan>,
) -> NativeResult<Option<SchedulerDecision>> {
    if worker_mode {
        let worker = worker.ok_or_else(|| io::Error::other("worker transport has no worker"))?;
        let cursor_delivery = presented_delivery_for_plan(plane_plan, &desired);
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
            validation_base,
            plane_plan.and_then(|plan| plan.attachable_primary),
            plane_plan.map_or(
                crate::native_output::presentation::plane_policy::CursorPlaneAction::Independent,
                |plan| plan.decision.cursor_action,
            ),
            cursor_delivery,
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
    cursor_delivery: crate::native_output::presentation::plane::PresentedCursorDelivery,
    primary_cursor_presentation: KmsPrimaryCursorPresentation,
    pacing_frame_id: Option<u64>,
    test_policy: KmsCommitTestPolicy,
    ready_submit: bool,
    validation_base: KmsValidationBase,
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
        cursor_delivery,
        primary_cursor_presentation,
        pacing_frame_id,
        test_policy,
        ready_submit,
        validation_base,
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
        let frozen_cursor_plan = explicit
            .swapchain()?
            .ready_cursor_plan()
            .ok_or_else(|| io::Error::other("ready explicit frame has no frozen cursor plan"))?;
        let frozen_cursor_delivery = frozen_cursor_plan.delivery;
        let frozen_primary_cursor_presentation =
            kms_primary_cursor_presentation(frozen_cursor_plan.primary_presentation);
        let cursor_update = planned_cursor_update(output_transactions, transaction_id)?;
        let pacing_frame_id = context
            .frame_pacing
            .reserve_worker_submission(ready_submit)
            .map_err(io::Error::other)?;
        let test_only = match frozen_cursor_plan.cursor_test_policy {
            FrozenCursorTestPolicy::Required => KmsTestOnlyPolicy::Required,
            FrozenCursorTestPolicy::Skip => KmsTestOnlyPolicy::Skip,
        };
        let primary_test_only =
            output_transactions
                .transaction(transaction_id)
                .is_some_and(|transaction| {
                    let descriptor = transaction.descriptor();
                    descriptor.presentation_mode().is_async()
                        && descriptor
                            .async_validation_key()
                            .map(|key| !explicit.async_validation_is_accepted(key))
                            .unwrap_or(true)
                });
        let result = match queue_explicit_ready_for_presentation(
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
            frozen_cursor_delivery,
            frozen_primary_cursor_presentation,
            pacing_frame_id,
            KmsCommitTestPolicy {
                primary: if primary_test_only {
                    KmsTestOnlyPolicy::Required
                } else {
                    KmsTestOnlyPolicy::Skip
                },
                cursor: test_only,
            },
            ready_submit,
            context.validation_base,
        ) {
            Ok(result) => result.map(|(token, framebuffer_id, transaction_id)| {
                (token, framebuffer_id, transaction_id, true)
            }),
            Err(error) => {
                if pacing_frame_id.is_some()
                    && !context
                        .frame_pacing
                        .cancel_worker_submission(pacing_frame_id, ready_submit)
                {
                    return Err(io::Error::other(
                        "failed explicit worker submission pacing identity mismatch",
                    )
                    .into());
                }
                return Err(error);
            }
        };
        if result.is_none()
            && pacing_frame_id.is_some()
            && !context
                .frame_pacing
                .cancel_worker_submission(pacing_frame_id, ready_submit)
        {
            return Err(io::Error::other(
                "unavailable explicit worker submission pacing identity mismatch",
            )
            .into());
        }
        return Ok(result);
    }
    let (token, framebuffer_id, transaction_id) =
        explicit.submit_ready_frame(kms_backend, server, output_transactions)?;
    Ok(Some((token, framebuffer_id, transaction_id, false)))
}

fn kms_primary_cursor_presentation(
    presentation: FrozenPrimaryCursorPresentation,
) -> KmsPrimaryCursorPresentation {
    match presentation {
        FrozenPrimaryCursorPresentation::Preserve => KmsPrimaryCursorPresentation::Preserve,
        FrozenPrimaryCursorPresentation::Promote(state) => {
            KmsPrimaryCursorPresentation::Promote(state)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn queue_compatibility_for_presentation(
    worker: &KmsCommitWorkerHandle,
    scanout: &mut NativeScanoutBackend,
    kms_backend: &KmsBackendSelection,
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
    cursor_revision: Option<CursorRevision>,
    cursor_delivery: crate::native_output::presentation::plane::PresentedCursorDelivery,
    primary_cursor_presentation: KmsPrimaryCursorPresentation,
    cursor_pin: Option<CursorFramebufferPin>,
    cursor_capability_key: Option<
        crate::native_output::presentation::plane_policy::CursorCapabilityKey,
    >,
    pacing_frame_id: Option<u64>,
    test_policy: KmsCommitTestPolicy,
    cursor_epoch: u64,
    validation_base: KmsValidationBase,
) -> NativeResult<Option<(NativePresentResult, Option<OutputTransactionId>)>> {
    match super::kms_worker::queue_atomic_compatibility_frame(
        worker,
        scanout,
        kms_backend,
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
        cursor_revision,
        cursor_delivery,
        primary_cursor_presentation,
        cursor_pin,
        cursor_capability_key,
        pacing_frame_id,
        test_policy,
        cursor_epoch,
        validation_base,
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
    cursor_revision: Option<CursorRevision>,
    context: WorkerPrimarySubmissionContext<'_>,
    output_generation: u64,
    crtc_id: u32,
    scene_generation: u64,
    _cursor_epoch: u64,
    last_rendered_scene_generation: &mut u64,
    _last_submitted_cursor_epoch: &mut u64,
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
    let direct_surface_id = direct_lease.surface_id();
    let direct_framebuffer_id = direct_lease.framebuffer_id();
    let direct_candidate_key = direct_lease.key();
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
    let mut job = KmsCommitJob {
        bundle_id:
            crate::native_output::presentation::plane::KmsCommitBundleId::from_pageflip_token(
                commit_token,
            ),
        owners: KmsBundleOwners::for_transaction(
            kind,
            std::sync::Arc::new(
                output_transactions
                    .transaction(transaction_id)
                    .ok_or_else(|| io::Error::other("direct worker transaction disappeared"))?
                    .descriptor()
                    .clone(),
            ),
            cursor_revision,
            context.atomic_cursor.and_then(|cursor| {
                effective_cursor.and_then(|state| cursor.capability_key_for(state))
            }),
        )
        .map_err(|error| io::Error::other(format!("invalid direct cursor owner: {error:?}")))?,
        transaction_id,
        token: commit_token,
        output_generation,
        crtc_id,
        kind,
        target: direct_target,
        validation_base: context.validation_base,
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
        cursor_delivery: context.cursor_delivery,
        primary_cursor_presentation: context.primary_cursor_presentation,
        cursor_pin: worker_cursor_pin(context.atomic_cursor, effective_cursor)?,
        direct_primary_lease: Some(direct_lease),
        test_only_duration_ns: None,
        pacing_frame_id,
        test_policy: KmsCommitTestPolicy::from_primary(test_only),
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
    let pacing_frame_id = context
        .frame_pacing
        .reserve_worker_submission(false)
        .map_err(io::Error::other)?;
    job.pacing_frame_id = pacing_frame_id;
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
    presentation_trace.push(PresentationTransactionEvent::DirectIdentity {
        transaction_id,
        timestamp_ns: queued_at_ns,
        surface_id: direct_surface_id,
        framebuffer_id: direct_framebuffer_id,
        content_epoch: direct_candidate_key.content.content_epoch.get(),
        candidate_key: direct_candidate_key,
        submission_token: token,
        pageflip_token: token,
    });
    presentation_trace.push(PresentationTransactionEvent::WorkerQueued {
        transaction_id,
        timestamp_ns: queued_at_ns,
    });
    *last_rendered_scene_generation = scene_generation;
    *frame_index = frame_index.saturating_add(1);
    *frame_submitted = true;
    Ok(DirectWorkerQueueResult::Queued)
}

pub(super) fn worker_cursor_queue_available(
    worker_mode: bool,
    worker: Option<&KmsCommitWorkerHandle>,
    arbiter: &AtomicCommitArbiter,
    attachable_primary: Option<crate::native_output::kms_worker::AttachablePrimary>,
) -> bool {
    worker_mode
        && worker.is_some_and(|worker| {
            attachable_primary.is_some()
                || (arbiter.kernel_commit_submitted()
                    && arbiter.worker_slot_available()
                    && arbiter
                        .kernel_submitted_kind()
                        .is_some_and(|kind| !matches!(kind, AtomicCommitKind::PlaneDelta { .. }))
                    && worker.admission_available())
        })
}
