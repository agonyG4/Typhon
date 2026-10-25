//! Reactor-side completion delivery for the Atomic render worker.
//!
//! The worker event source only transports completed renderer-owned values.
//! Output transactions and protocol ownership remain in the normal reactor
//! settlement path.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_RENDER_WORKER_JOB_ID: AtomicU64 = AtomicU64::new(1);

pub(super) struct PendingWorkerCapture {
    pub(super) job_id: crate::native_output::scanout::RenderWorkerJobId,
    pub(super) control_epoch: u64,
    pub(super) pending: oblivion_one::compositor::AstreaScreenCapturePending,
    pub(super) dimensions: crate::native_output::screen_capture::CaptureDimensions,
    pub(super) cancelled: bool,
}

pub(super) struct PendingWorkerEffectReload {
    pub(super) job_id: crate::native_output::scanout::RenderWorkerJobId,
    pub(super) control_epoch: u64,
    pub(super) token: oblivion_one::native::event_loop::ReactorToken,
    pub(super) request_id: u64,
}

pub(super) fn next_render_worker_job_id()
-> io::Result<crate::native_output::scanout::RenderWorkerJobId> {
    let value = NEXT_RENDER_WORKER_JOB_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map_err(|_| io::Error::other("render-worker capture ID exhausted"))?;
    crate::native_output::scanout::RenderWorkerJobId::new(value)
        .ok_or_else(|| io::Error::other("render-worker capture ID is zero"))
}

pub(super) const fn effect_reload_admission_available(
    worker_active: bool,
    worker_busy: bool,
    reload_pending: bool,
) -> bool {
    worker_active && !worker_busy && !reload_pending
}

/// A completed render worker cannot submit KMS from its completion callback.
/// Even without a target/lane wait, the physical frame must become scheduler
/// READY before another render can reuse the active pacing owner.
fn handoff_render_worker_ready(
    frame_pacing: &mut NativeFramePacing,
    frame_scheduler: &mut NativeFrameScheduler,
    ready_at_ns: u64,
    waits_for_target: bool,
    render_ahead: bool,
) -> Result<(), &'static str> {
    frame_pacing.finish_render_worker_with_ready_frame(ready_at_ns, waits_for_target)?;
    // The KMS worker times this submission, so the scheduler must not wait
    // for or retarget the frame's physical reservation.
    frame_scheduler.note_untimed_ready_frame();
    if render_ahead {
        frame_pacing.note_predictive_unbound_ready();
    }
    Ok(())
}

/// End a worker attempt without READY only after scanout returns a successful
/// terminal no-frame outcome. `settle_render_completion` emits these variants
/// after its pre-GPU cancellation or post-GPU completion and protocol
/// obligation settlement have succeeded; an `Err` carries no such proof.
fn settle_render_worker_after_terminal_no_frame(
    frame_pacing: &mut NativeFramePacing,
    outcome: &crate::native_output::scanout::AtomicFrameRenderOutcome,
) -> Result<(), &'static str> {
    match outcome {
        crate::native_output::scanout::AtomicFrameRenderOutcome::Skipped { .. }
        | crate::native_output::scanout::AtomicFrameRenderOutcome::RetryPresentationState {
            ..
        }
        | crate::native_output::scanout::AtomicFrameRenderOutcome::LifecycleFallback { .. } => {
            frame_pacing.finish_render_worker_without_frame()
        }
        crate::native_output::scanout::AtomicFrameRenderOutcome::Rendered { .. }
        | crate::native_output::scanout::AtomicFrameRenderOutcome::WorkerDispatched { .. } => {
            Err("render worker outcome does not prove terminal no-frame ownership")
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExpectedRenderWorkerFrame {
    output_epoch: crate::native_output::OutputEpochId,
    output_id: oblivion_one::core::OutputId,
    frame_id: u64,
    transaction_id: crate::native_output::OutputTransactionId,
    protocol_batch_id: oblivion_one::compositor::CompositorFrameBatchId,
    slot: crate::native_output::scanout::OutputSlotId,
    framebuffer_id: u32,
    render_generation: u64,
    pool_generation: u64,
    reservation: crate::native_output::presentation::transaction::FramePresentationReservation,
}

impl ExpectedRenderWorkerFrame {
    fn from_context(
        context: &crate::native_output::scanout::AtomicRenderSettlementContext,
        output_id: oblivion_one::core::OutputId,
    ) -> Self {
        Self {
            output_epoch: context.output_epoch,
            output_id,
            frame_id: context.frame_id,
            transaction_id: context.transaction_id,
            protocol_batch_id: context.protocol_batch_id,
            slot: context.slot,
            framebuffer_id: context.framebuffer_id,
            render_generation: context.render_generation,
            pool_generation: context.pool_generation,
            reservation: context.prepare_intent.map_or(
                context.reservation.frame_reservation(),
                crate::native_output::presentation::transaction::FramePresentationReservation::DeferredO1,
            ),
        }
    }

    fn identity_snapshot(
        self,
    ) -> Option<crate::native_output::scanout::OutputFrameIdentitySnapshot> {
        Some(crate::native_output::scanout::OutputFrameIdentitySnapshot {
            output_id: self.output_id,
            frame_id: self.frame_id,
            protocol_batch_id: self.protocol_batch_id,
            transaction_id: self.transaction_id,
            slot: self.slot,
            framebuffer_id: oblivion_one::native::kms::FramebufferId::new(self.framebuffer_id)?,
            render_generation: self.render_generation,
            pool_generation: self.pool_generation,
            reservation: self.reservation,
        })
    }

    fn exactly_matches(
        self,
        identity: crate::native_output::scanout::OutputFrameIdentitySnapshot,
    ) -> bool {
        self.identity_snapshot() == Some(identity)
    }

    fn may_own_same_physical_slot(
        self,
        identity: crate::native_output::scanout::OutputFrameIdentitySnapshot,
    ) -> bool {
        self.slot == identity.slot
            || self.framebuffer_id == identity.framebuffer_id.get()
            || self.output_id != identity.output_id
            || self.frame_id == identity.frame_id
            || self.transaction_id == identity.transaction_id
            || self.protocol_batch_id == identity.protocol_batch_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RenderWorkerPhysicalOwnershipSnapshot {
    output_id: oblivion_one::core::OutputId,
    pool_generation: u64,
    ready_identity: Option<crate::native_output::scanout::OutputFrameIdentitySnapshot>,
    rendering_slot: Option<crate::native_output::scanout::OutputSlotId>,
    quarantined_slot: Option<crate::native_output::scanout::OutputSlotId>,
    suspended_expected_frame: bool,
    has_suspended_ownership: bool,
    worker_queued: Option<crate::native_output::scanout::QueuedOutputFrameIdentitySnapshot>,
    kernel_submitted: Option<crate::native_output::scanout::QueuedOutputFrameIdentitySnapshot>,
}

impl RenderWorkerPhysicalOwnershipSnapshot {
    fn from_swapchain(
        expected: ExpectedRenderWorkerFrame,
        swapchain: &crate::native_output::scanout::AtomicOutputSwapchain,
    ) -> Self {
        let expected_identity = expected.identity_snapshot();
        Self {
            output_id: swapchain.output_id(),
            pool_generation: swapchain.pool_generation(),
            ready_identity: swapchain.ready_identity(),
            rendering_slot: swapchain.rendering_slot(),
            quarantined_slot: swapchain.quarantine_slot_id(),
            suspended_expected_frame: expected_identity
                .is_some_and(|identity| swapchain.contains_suspended_frame(identity)),
            has_suspended_ownership: swapchain.has_suspended_ownership(),
            worker_queued: swapchain.worker_queued_identity(),
            kernel_submitted: swapchain.pending_identity(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderWorkerSettlementFailureDisposition {
    PhysicalReadyRetained,
    PhysicalOwnershipUnsettled,
    PhysicalOwnershipConflict,
    OutputEpochMismatch,
    SwapchainUnavailable,
    OutputIdentityMismatch,
    PoolGenerationMismatch,
    NoTerminalRetirementProof,
}

/// Preserve a worker's real pacing owner if scanout crossed into READY before
/// a later transaction-sidecar step failed. Failure to observe the expected
/// frame in a particular swapchain role is not proof of terminalization: the
/// frame may be suspended, queued, submitted, owned by another swapchain, or
/// have outstanding GPU/protocol obligations. Error settlement may release
/// this pin only through an independently proven terminal outcome.
fn settle_render_worker_failure_ownership(
    pacing: &mut NativeFramePacing,
    expected: ExpectedRenderWorkerFrame,
    current_output_epoch: crate::native_output::OutputEpochId,
    ownership: Option<RenderWorkerPhysicalOwnershipSnapshot>,
    now_ns: u64,
) -> Result<RenderWorkerSettlementFailureDisposition, &'static str> {
    if expected.output_epoch != current_output_epoch {
        return Ok(RenderWorkerSettlementFailureDisposition::OutputEpochMismatch);
    }
    let Some(ownership) = ownership else {
        return Ok(RenderWorkerSettlementFailureDisposition::SwapchainUnavailable);
    };
    if ownership.output_id != expected.output_id {
        return Ok(RenderWorkerSettlementFailureDisposition::OutputIdentityMismatch);
    }
    if ownership.pool_generation != expected.pool_generation {
        return Ok(RenderWorkerSettlementFailureDisposition::PoolGenerationMismatch);
    }

    let expected_is_still_owned = ownership.rendering_slot == Some(expected.slot)
        || ownership.quarantined_slot == Some(expected.slot)
        || ownership.suspended_expected_frame
        || ownership
            .worker_queued
            .is_some_and(|queued| expected.may_own_same_physical_slot(queued.frame))
        || ownership
            .kernel_submitted
            .is_some_and(|submitted| expected.may_own_same_physical_slot(submitted.frame));
    let ready_identity = ownership.ready_identity;
    let ready_matches = ready_identity.is_some_and(|identity| expected.exactly_matches(identity));
    let ready_may_alias =
        ready_identity.is_some_and(|identity| expected.may_own_same_physical_slot(identity));

    if ready_matches && expected_is_still_owned {
        return Ok(RenderWorkerSettlementFailureDisposition::PhysicalOwnershipConflict);
    }
    if expected_is_still_owned {
        return Ok(RenderWorkerSettlementFailureDisposition::PhysicalOwnershipUnsettled);
    }
    if ready_may_alias && !ready_matches {
        return Ok(RenderWorkerSettlementFailureDisposition::PhysicalOwnershipConflict);
    }
    if let Some(identity) = ready_identity.filter(|identity| expected.exactly_matches(*identity)) {
        pacing.bind_physical_output_frame(identity)?;
        pacing.finish_render_worker_with_ready_frame(now_ns, false)?;
        return Ok(RenderWorkerSettlementFailureDisposition::PhysicalReadyRetained);
    }
    if ownership.has_suspended_ownership {
        return Ok(RenderWorkerSettlementFailureDisposition::PhysicalOwnershipUnsettled);
    }
    Ok(RenderWorkerSettlementFailureDisposition::NoTerminalRetirementProof)
}

impl NativeRuntime {
    fn complete_worker_effect_reload(
        &mut self,
        job_id: crate::native_output::scanout::RenderWorkerJobId,
        control_epoch: u64,
        result: Result<Option<crate::native_output::scanout::PreparedWorkerEffectReload>, String>,
    ) -> NativeResult<()> {
        if self.pending_render_worker_identity.is_some()
            || self.pending_render_worker_context.is_some()
            || self.pending_render_worker_outer.is_some()
            || self.pending_render_worker_completion.is_some()
            || self.pending_render_worker_capture.is_some()
        {
            return Err(io::Error::other(
                "effect reload completion arrived while another render continuation owns the worker",
            )
            .into());
        }
        let Some(pending) = self.pending_worker_effect_reload.take() else {
            return Err(io::Error::other(
                "effect reload completion arrived without reactor request ownership",
            )
            .into());
        };
        if pending.job_id != job_id || pending.control_epoch != control_epoch {
            return Err(io::Error::other(
                "effect reload completion job identity or renderer control epoch mismatch",
            )
            .into());
        }

        let mut request_redraw = false;
        let response = match result {
            Ok(Some(reload)) => {
                let generation = self
                    .server
                    .trusted_effect_registry()
                    .publish_prepared_reload(reload.prepared)
                    .map_err(|error| {
                        io::Error::other(format!(
                            "render worker installed an effect generation that could not become authoritative: {error}"
                        ))
                    })?;
                let scene_generation = self.server.scene_render_generation();
                self.server.reconcile_trusted_effect_bindings();
                if self.server.scene_render_generation() == scene_generation {
                    self.server.note_trusted_effect_registry_reload();
                }
                request_redraw = true;
                let snapshot = oblivion_one::control_snapshots::TrustedEffectsReloadSnapshot {
                    manifest_path: reload.manifest_path,
                    generation: generation.generation,
                    effect_count: generation.effects.len(),
                };
                match serde_json::to_value(snapshot) {
                    Ok(snapshot) => oblivion_one::control::ControlResponse::success(
                        pending.request_id,
                        snapshot,
                    ),
                    Err(_) => oblivion_one::control::ControlResponse::failure(
                        pending.request_id,
                        oblivion_one::control::ControlError::new(
                            oblivion_one::control::ControlErrorCode::Internal,
                            "effect reload snapshot failed",
                        ),
                    ),
                }
            }
            Ok(None) => oblivion_one::control::ControlResponse::failure(
                pending.request_id,
                oblivion_one::control::ControlError::new(
                    oblivion_one::control::ControlErrorCode::Internal,
                    "trusted effects manifest is not present",
                ),
            ),
            Err(error) => oblivion_one::control::ControlResponse::failure(
                pending.request_id,
                oblivion_one::control::ControlError::new(
                    oblivion_one::control::ControlErrorCode::Internal,
                    format!("trusted effect reload failed: {error}"),
                ),
            ),
        };
        if self.control_server.has_client(pending.token) {
            self.control_server
                .queue_response(&mut self.event_loop, pending.token, response)?;
        }
        if request_redraw {
            self.queued_redraw_requested = true;
            self.event_loop.request_continuation(
                oblivion_one::native::event_loop::NativeContinuationReason::FrameScheduler,
            )?;
        }
        Ok(())
    }

    fn complete_worker_capture(
        &mut self,
        job_id: crate::native_output::scanout::RenderWorkerJobId,
        control_epoch: u64,
        result: io::Result<Vec<u8>>,
    ) -> NativeResult<()> {
        let Some(pending) = self.pending_render_worker_capture.take() else {
            return Err(io::Error::other(
                "render worker capture completed without reactor ownership",
            )
            .into());
        };
        if pending.job_id != job_id || pending.control_epoch != control_epoch {
            pending.pending.capture.failed("render_failed".to_owned());
            return Err(io::Error::other(
                "render worker capture identity or control epoch mismatch",
            )
            .into());
        }
        if pending.cancelled {
            return Ok(());
        }
        let payload = match result {
            Ok(payload) if payload.len() == pending.dimensions.payload_len => payload,
            Ok(_) => {
                pending.pending.capture.failed("render_failed".to_owned());
                return Ok(());
            }
            Err(error) => {
                eprintln!("native screen capture render failed: {error}");
                pending.pending.capture.failed("render_failed".to_owned());
                return Ok(());
            }
        };
        let memfd = match crate::native_output::screen_capture::create_sealed_memfd(&payload) {
            Ok(memfd) => memfd,
            Err(error) => {
                eprintln!("native screen capture allocation failed: {error}");
                pending
                    .pending
                    .capture
                    .failed("allocation_failed".to_owned());
                return Ok(());
            }
        };
        pending.pending.capture.ready(
            memfd.as_fd(),
            pending.dimensions.width,
            pending.dimensions.height,
            pending.dimensions.stride,
            crate::native_output::screen_capture::RGBA8888_FORMAT,
        );
        Ok(())
    }

    #[allow(dead_code, reason = "Reserved for alternate runtime paths")]
    pub(super) fn expect_render_worker_completion(
        &mut self,
        identity: crate::native_output::scanout::AtomicRenderWorkerIdentity,
        context: crate::native_output::scanout::AtomicRenderSettlementContext,
    ) -> NativeResult<()> {
        if self.pending_render_worker_identity.is_some()
            || self.pending_render_worker_context.is_some()
            || self.pending_render_worker_completion.is_some()
        {
            return Err(io::Error::other(
                "render worker admission attempted while prior completion ownership is active",
            )
            .into());
        }
        self.pending_render_worker_identity = Some(identity);
        self.pending_render_worker_context = Some(context);
        Ok(())
    }

    pub(super) fn settle_pending_render_worker_completion(
        &mut self,
        cycle: &mut super::NativeCycleState,
    ) -> NativeResult<()> {
        let Some(completion) = self.pending_render_worker_completion.take() else {
            return Ok(());
        };
        let context = self
            .pending_render_worker_context
            .take()
            .ok_or_else(|| io::Error::other("worker completion lost render settlement context"))?;
        let mut outer = self
            .pending_render_worker_outer
            .take()
            .ok_or_else(|| io::Error::other("worker completion lost presentation continuation"))?;
        if self.pending_render_worker_identity.is_some() {
            return Err(io::Error::other(
                "worker completion was settled before its dispatch identity was consumed",
            )
            .into());
        }

        let release_safety = context.dmabuf_gpu_release_safety;
        let release_lease_id = context.dmabuf_gpu_release_lease_id;
        let expected_worker_frame =
            ExpectedRenderWorkerFrame::from_context(&context, self.output_id);
        let outcome = self.scanout.settle_render_worker_completion(
            context,
            completion.frozen_frame,
            completion.result,
            completion.gpu_sampling_started,
            completion.no_visual_release_fence,
            &mut self.server,
            &mut self.output_transactions,
        );
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                let ownership = self.scanout.explicit_output_swapchain().map(|swapchain| {
                    RenderWorkerPhysicalOwnershipSnapshot::from_swapchain(
                        expected_worker_frame,
                        swapchain,
                    )
                });
                let disposition = settle_render_worker_failure_ownership(
                    &mut self.frame_pacing,
                    expected_worker_frame,
                    self.output_epoch,
                    ownership,
                    monotonic_now_ns().map_err(|clock_error| {
                        io::Error::other(format!(
                            "render worker settlement failed ({error}); could not timestamp ownership preservation: {clock_error}"
                        ))
                    })?,
                )
                .map_err(|owner_error| {
                    io::Error::other(format!(
                        "render worker settlement failed ({error}); pacing owner settlement failed ({owner_error})"
                    ))
                })?;
                return Err(Box::new(io::Error::other(format!(
                    "render worker settlement failed ({error}); physical ownership disposition={disposition:?} current_output_epoch={} ownership={ownership:?} expected={expected_worker_frame:?}",
                    self.output_epoch.get(),
                ))));
            }
        };

        if matches!(
            &outcome,
            crate::native_output::scanout::AtomicFrameRenderOutcome::Skipped { .. }
                | crate::native_output::scanout::AtomicFrameRenderOutcome::RetryPresentationState { .. }
                | crate::native_output::scanout::AtomicFrameRenderOutcome::LifecycleFallback { .. }
        ) {
            settle_render_worker_after_terminal_no_frame(&mut self.frame_pacing, &outcome)
                .map_err(io::Error::other)?;
        }

        let mut frame_completed = cycle.frame_completed;
        let mut frame_rendered = cycle.frame_rendered;
        let frame_submitted = cycle.frame_submitted;
        match outcome {
            crate::native_output::scanout::AtomicFrameRenderOutcome::WorkerDispatched {
                ..
            } => {
                return Err(io::Error::other(
                    "render worker completion unexpectedly dispatched another render",
                )
                .into());
            }
            nonrendered @ (crate::native_output::scanout::AtomicFrameRenderOutcome::Skipped { .. }
            | crate::native_output::scanout::AtomicFrameRenderOutcome::RetryPresentationState {
                ..
            }
            | crate::native_output::scanout::AtomicFrameRenderOutcome::LifecycleFallback {
                ..
            }) => {
                super::presentation_worker::settle_atomic_nonrendered_outcome(
                    super::presentation_worker::AtomicNonRenderedOutcomeContext {
                        server: &mut self.server,
                        frame_pacing: &mut self.frame_pacing,
                        frame_scheduler: &mut self.frame_scheduler,
                        dmabuf_gpu_release_registry: &mut self.dmabuf_gpu_release_registry,
                        event_loop: &mut self.event_loop,
                        render_telemetry: &mut self.render_telemetry,
                        last_rendered_scene_generation:
                            &mut self.last_rendered_scene_generation,
                        presentation_deadline: &mut self.presentation_deadline,
                        scheduled_presentation_target:
                            &mut self.scheduled_presentation_target,
                        queued_redraw_requested: &mut self.queued_redraw_requested,
                        last_acquire_ready_at_ns: &mut self.last_acquire_ready_at_ns,
                        render_demand_coalesced_count: self.render_demand_coalesced_count,
                        frame_completed: &mut frame_completed,
                        perf: self.perf,
                        release_safety,
                        accepted_clients: None,
                        pending_frame_work: None,
                    },
                    &outer,
                    nonrendered,
                )?;
            }
            rendered @ crate::native_output::scanout::AtomicFrameRenderOutcome::Rendered {
                ..
            } => {
                let explicit = match &mut *self.scanout {
                    crate::native_output::scanout::NativeScanoutBackend::AtomicEglGbm(
                        explicit,
                    ) => explicit,
                    _ => {
                        return Err(io::Error::other(
                            "Atomic render worker completion reached a compatibility scanout",
                        )
                        .into());
                    }
                };
                let settled = super::presentation_worker::settle_atomic_rendered_outcome(
                    super::presentation_worker::AtomicRenderedOutcomeContext {
                        explicit,
                        server: &mut self.server,
                        frame_pacing: &mut self.frame_pacing,
                        frame_scheduler: &mut self.frame_scheduler,
                        dmabuf_gpu_release_registry: &mut self.dmabuf_gpu_release_registry,
                        event_loop: &mut self.event_loop,
                        scene_history: &mut self.scene_history,
                        presentation_trace: &mut self.presentation_trace,
                        render_telemetry: &mut self.render_telemetry,
                        frame_rendered: &mut frame_rendered,
                        release_safety,
                        release_lease_id,
                        finish_render_work_on_failure: false,
                        atomic_commit_pending: self.atomic_commit_arbiter.atomic_commit_pending(),
                    },
                    &mut outer,
                    rendered,
                )?;
                let frame_id = settled.frame_id;
                let protocol_batch_id = settled.protocol_batch_id;
                let waits_for_target = settled.waits_for_target;
                if let Some(failure) = settled.deferred_o1_binding_failure {
                    if !outer.render_ahead || !waits_for_target {
                        return Err(io::Error::other(
                            "deferred O1 worker result became stale outside its guarded lane",
                        )
                        .into());
                    }
                    self.server
                        .note_frame_callbacks_deferred_ready(protocol_batch_id);
                    self.frame_pacing
                        .finish_render_worker_with_ready_frame(
                            settled.ready_at_ns,
                            waits_for_target,
                        )
                        .map_err(io::Error::other)?;
                    self.frame_scheduler.note_ready_frame(None);
                    let owner = self
                        .scanout
                        .explicit_output_swapchain()
                        .and_then(|swapchain| swapchain.ready_identity())
                        .ok_or_else(|| {
                            io::Error::other("stale deferred O1 worker frame has no ready owner")
                        })?;
                    match failure {
                        DeferredO1BindingFailure::IdentityMismatch => self
                            .frame_pacing
                            .note_predictive_unbound_abandoned_identity_physical(Some(owner)),
                        DeferredO1BindingFailure::GenerationMismatch => self
                            .frame_pacing
                            .note_predictive_unbound_abandoned_generation_physical(Some(owner)),
                    }
                    match &mut *self.scanout {
                        crate::native_output::scanout::NativeScanoutBackend::AtomicEglGbm(
                            explicit,
                        ) => super::cycle::abandon_overtaken_ready(
                            explicit,
                            owner,
                            self.output_epoch,
                            &mut self.scene_history,
                            &mut self.frame_pacing,
                            &mut self.frame_scheduler,
                            &mut self.server,
                            &mut self.output_transactions,
                            MonotonicTimestampNs::new(settled.ready_at_ns),
                        )?,
                        _ => {
                            return Err(io::Error::other(
                                "Atomic render worker completion reached a compatibility scanout",
                            )
                            .into());
                        }
                    }
                    self.queued_redraw_requested = true;
                } else {
                    // Worker completion only produces a physical READY; KMS
                    // submission belongs to the next reactor scheduling pass.
                    // Without this handoff, reactive frames remain in ACTIVE
                    // and the next render attempts to bind a second frame.
                    self.server
                        .note_frame_callbacks_deferred_ready(protocol_batch_id);
                    handoff_render_worker_ready(
                        &mut self.frame_pacing,
                        &mut self.frame_scheduler,
                        settled.ready_at_ns,
                        waits_for_target,
                        outer.render_ahead,
                    )
                    .map_err(io::Error::other)?;
                }
                self.frame_pacing.log(
                    "render_complete",
                    vec![
                        PacingField::u64("frame_id", frame_id),
                        PacingField::u64("output_frame_id", frame_id),
                        PacingField::u64("render_generation", outer.render_generation),
                        PacingField::u64("render_observed_at_ns", outer.render_observed_at_ns),
                        PacingField::option_u64(
                            "target_vblank_sequence",
                            outer.frame_target_sequence,
                        ),
                        PacingField::option_u64(
                            "target_presentation_ns",
                            outer.frame_target_presentation_ns,
                        ),
                        PacingField::bool("render_ahead", outer.render_ahead),
                    ],
                );
                self.last_rendered_scene_generation = outer.scene_generation;
                let _ = (
                    outer.output_epoch.get(),
                    outer.cursor_epoch,
                    outer.pacing_mode,
                );
            }
        }
        cycle.record_presentation_result(frame_completed, frame_rendered, frame_submitted);
        self.event_loop.request_continuation(
            oblivion_one::native::event_loop::NativeContinuationReason::FrameScheduler,
        )?;
        Ok(())
    }

    fn retire_stale_render_worker_continuation(
        &mut self,
        identity: crate::native_output::scanout::AtomicRenderWorkerIdentity,
    ) {
        if self.pending_render_worker_identity != Some(identity) {
            return;
        }
        self.pending_render_worker_identity = None;
        self.pending_render_worker_context.take();
        self.pending_render_worker_outer.take();
        self.pending_render_worker_completion.take();
    }

    pub(super) fn process_render_worker_events(&mut self) -> NativeResult<()> {
        loop {
            let event = self.scanout.try_take_render_worker_event()?;
            let Some(event) = event else {
                return Ok(());
            };
            match event {
                crate::native_output::scanout::AtomicRenderWorkerEvent::Rendered(completion) => {
                    let identity = completion.identity();
                    if !identity.belongs_to_output_epoch(self.output_epoch) {
                        self.perf.log("native.render_worker.stale_completion", || {
                            vec![
                                NativePerfField::u64(
                                    "completion_output_epoch",
                                    completion.output_epoch.get(),
                                ),
                                NativePerfField::u64(
                                    "current_output_epoch",
                                    self.output_epoch.get(),
                                ),
                                NativePerfField::u64("job_id", completion.job_id.get()),
                                NativePerfField::u64("frame_id", completion.frame_id),
                            ]
                        });
                        // The worker has completed its transport ownership. Drop
                        // the stale result and its fence/resources without
                        // publishing it into the current output epoch.
                        self.retire_stale_render_worker_continuation(identity);
                        drop(completion);
                        continue;
                    }
                    if self.pending_render_worker_capture.is_some() {
                        return Err(io::Error::other(
                            "render completion arrived while a capture continuation owns the worker",
                        )
                        .into());
                    }
                    if self.pending_render_worker_completion.is_some() {
                        return Err(io::Error::other(
                            "render worker produced a second completion before reactor settlement",
                        )
                        .into());
                    }
                    let expected = self.pending_render_worker_identity.take().ok_or_else(|| {
                        io::Error::other(
                            "render worker completion arrived without an executing continuation",
                        )
                    })?;
                    if self.pending_render_worker_context.is_none() {
                        return Err(io::Error::other(
                            "render worker completion arrived without reactor settlement evidence",
                        )
                        .into());
                    }
                    let outer = self.pending_render_worker_outer.as_ref().ok_or_else(|| {
                        io::Error::other(
                            "render worker completion arrived without outer presentation evidence",
                        )
                    })?;
                    if !crate::native_output::scanout::completion_matches_job(
                        expected,
                        completion.identity(),
                    ) {
                        return Err(io::Error::other(
                            "render worker completion identity does not match the executing continuation",
                        )
                        .into());
                    }
                    let completion_observed_at_ns = monotonic_now_ns()?;
                    if super::presentation_cycle::render_worker_received_visual_demand(
                        outer.coalesced_demand_count_at_dispatch,
                        self.render_demand_coalesced_count,
                    ) {
                        self.queued_redraw_requested = true;
                    }
                    let dispatch_to_start_ns = completion
                        .worker_started_at_ns
                        .map(|started| started.saturating_sub(completion.dispatch_at_ns));
                    self.perf.log("native.render_worker.completion", || {
                        // Worker frames never reach the GLES repaint fields of
                        // `native.frame`, so effect pool churn rides here next
                        // to the scene drawing time it may cost.
                        let effect_stats = match &completion.result {
                            Ok(AtomicSlotRenderOutcome::Rendered(parts)) => Some(&parts.stats),
                            _ => None,
                        };
                        let effect_field = |key, value: fn(&GlesSceneFrameStats) -> usize| {
                            NativePerfField::str(
                                key,
                                effect_stats.map_or_else(
                                    || "unavailable".to_owned(),
                                    |stats| value(stats).to_string(),
                                ),
                            )
                        };
                        vec![
                            effect_field("effect_instances_executed", |stats| {
                                stats.effect_instances_executed
                            }),
                            effect_field("effect_resource_allocations_total", |stats| {
                                stats.effect_resource_allocations_total
                            }),
                            effect_field("effect_resource_reuses_total", |stats| {
                                stats.effect_resource_reuses_total
                            }),
                            NativePerfField::u64("job_id", completion.job_id.get()),
                            NativePerfField::u64("frame_id", completion.frame_id),
                            NativePerfField::u64("transaction_id", completion.transaction_id.get()),
                            NativePerfField::u64("dispatch_at_ns", completion.dispatch_at_ns),
                            NativePerfField::str(
                                "worker_started_at_ns",
                                completion
                                    .worker_started_at_ns
                                    .map_or_else(|| "unavailable".to_owned(), |v| v.to_string()),
                            ),
                            NativePerfField::str(
                                "worker_finished_at_ns",
                                completion
                                    .worker_finished_at_ns
                                    .map_or_else(|| "unavailable".to_owned(), |v| v.to_string()),
                            ),
                            NativePerfField::u64(
                                "reactor_completion_observed_at_ns",
                                completion_observed_at_ns,
                            ),
                            NativePerfField::str(
                                "dispatch_to_start_ns",
                                dispatch_to_start_ns
                                    .map_or_else(|| "unavailable".to_owned(), |v| v.to_string()),
                            ),
                            NativePerfField::u64("worker_wall_ns", completion.worker_wall_ns),
                            NativePerfField::str(
                                "worker_thread_cpu_ns",
                                completion
                                    .worker_thread_cpu_ns
                                    .map_or_else(|| "unavailable".to_owned(), |v| v.to_string()),
                            ),
                            NativePerfField::u64(
                                "egl_context_activation_ns",
                                completion.phase_durations.egl_context_activation_ns,
                            ),
                            NativePerfField::u64(
                                "scene_drawing_ns",
                                completion.phase_durations.scene_drawing_ns,
                            ),
                            NativePerfField::u64(
                                "native_render_fence_export_ns",
                                completion.phase_durations.native_render_fence_export_ns,
                            ),
                            NativePerfField::u64(
                                "render_demand_coalesced_count",
                                self.render_demand_coalesced_count,
                            ),
                        ]
                    });
                    self.pending_render_worker_completion = Some(completion);
                    self.event_loop.request_continuation(
                        oblivion_one::native::event_loop::NativeContinuationReason::FrameScheduler,
                    )?;
                }
                crate::native_output::scanout::AtomicRenderWorkerEvent::CandidatePrepared {
                    identity,
                    result,
                } => {
                    if self.pending_render_worker_identity.is_some()
                        || self.pending_render_worker_context.is_some()
                        || self.pending_render_worker_outer.is_some()
                        || self.pending_render_worker_completion.is_some()
                        || self.pending_render_worker_capture.is_some()
                        || self.pending_worker_effect_reload.is_some()
                    {
                        return Err(io::Error::other(
                            "output candidate completion arrived while another render continuation owns the worker",
                        )
                        .into());
                    }
                    self.render_worker_candidate_prepared(identity, result)?;
                }
                crate::native_output::scanout::AtomicRenderWorkerEvent::CandidatePromoted {
                    identity,
                } => self.render_worker_candidate_promoted(identity)?,
                crate::native_output::scanout::AtomicRenderWorkerEvent::CandidateDiscarded {
                    identity,
                } => self.render_worker_candidate_discarded(identity)?,
                crate::native_output::scanout::AtomicRenderWorkerEvent::ReleaseFenceReady {
                    job_id,
                    control_epoch,
                    result,
                } => self.render_worker_release_fence_ready(job_id, control_epoch, result)?,
                crate::native_output::scanout::AtomicRenderWorkerEvent::Captured {
                    job_id,
                    control_epoch,
                    result,
                } => {
                    if self.pending_render_worker_identity.is_some()
                        || self.pending_render_worker_context.is_some()
                        || self.pending_render_worker_outer.is_some()
                        || self.pending_render_worker_completion.is_some()
                    {
                        return Err(io::Error::other(
                            "capture completion arrived while a frame continuation owns the worker",
                        )
                        .into());
                    }
                    self.complete_worker_capture(job_id, control_epoch, result)?;
                    self.event_loop.request_continuation(
                        oblivion_one::native::event_loop::NativeContinuationReason::ScreenCaptureBacklog,
                    )?;
                }
                crate::native_output::scanout::AtomicRenderWorkerEvent::EffectsReloaded {
                    job_id,
                    control_epoch,
                    result,
                } => {
                    self.complete_worker_effect_reload(job_id, control_epoch, result)?;
                }
                crate::native_output::scanout::AtomicRenderWorkerEvent::Fatal {
                    job_id,
                    gpu_execution_uncertain,
                    reason,
                } => {
                    return Err(io::Error::other(format!(
                        "Atomic render worker failed: reason={reason} job_id={:?} gpu_execution_uncertain={gpu_execution_uncertain}; synchronous fallback is forbidden after worker ownership",
                        job_id.map(|id| id.get()),
                    ))
                    .into());
                }
            }
        }
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use crate::native_output::scanout::OutputFrameIdentitySnapshot;
    use std::os::fd::{FromRawFd, OwnedFd};

    fn physical_identity(
        frame_id: u64,
        slot: u8,
        pool_generation: u64,
    ) -> OutputFrameIdentitySnapshot {
        OutputFrameIdentitySnapshot {
            output_id: oblivion_one::core::OutputId::from_raw(1).unwrap(),
            frame_id,
            protocol_batch_id: oblivion_one::compositor::CompositorFrameBatchId::new(
                std::num::NonZeroU64::new(frame_id).unwrap(),
            ),
            transaction_id: crate::native_output::OutputTransactionId::new(
                std::num::NonZeroU64::new(frame_id + 100).unwrap(),
            ),
            slot: crate::native_output::scanout::OutputSlotId::new(slot).unwrap(),
            framebuffer_id: oblivion_one::native::kms::FramebufferId::new(frame_id as u32).unwrap(),
            render_generation: 17,
            pool_generation,
            reservation:
                crate::native_output::presentation::transaction::test_deferred_o1_reservation(),
        }
    }

    fn expected(identity: OutputFrameIdentitySnapshot) -> ExpectedRenderWorkerFrame {
        ExpectedRenderWorkerFrame {
            output_epoch: crate::native_output::OutputEpochId::initial(),
            output_id: identity.output_id,
            frame_id: identity.frame_id,
            transaction_id: identity.transaction_id,
            protocol_batch_id: identity.protocol_batch_id,
            slot: identity.slot,
            framebuffer_id: identity.framebuffer_id.get(),
            render_generation: identity.render_generation,
            pool_generation: identity.pool_generation,
            reservation: identity.reservation,
        }
    }

    fn empty_ownership(
        expected: ExpectedRenderWorkerFrame,
    ) -> RenderWorkerPhysicalOwnershipSnapshot {
        RenderWorkerPhysicalOwnershipSnapshot {
            output_id: expected.output_id,
            pool_generation: expected.pool_generation,
            ready_identity: None,
            rendering_slot: None,
            quarantined_slot: None,
            suspended_expected_frame: false,
            has_suspended_ownership: false,
            worker_queued: None,
            kernel_submitted: None,
        }
    }

    fn test_swapchain(
        pool_generation: u64,
    ) -> crate::native_output::scanout::AtomicOutputSwapchain {
        let slots = crate::native_output::scanout::OutputSlotSet::new([
            crate::native_output::scanout::OutputSlotId::new(0).unwrap(),
            crate::native_output::scanout::OutputSlotId::new(1).unwrap(),
            crate::native_output::scanout::OutputSlotId::new(2).unwrap(),
        ])
        .unwrap();
        crate::native_output::scanout::AtomicOutputSwapchain::from_presented_slots_for_output(
            oblivion_one::core::OutputId::from_raw(1).unwrap(),
            slots,
            crate::native_output::scanout::OutputSlotId::new(0).unwrap(),
            pool_generation,
        )
        .unwrap()
    }

    fn test_render_fence() -> crate::egl_renderer::native_fence::NativeRenderFence {
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

    fn ready_swapchain(
        pool_generation: u64,
    ) -> (
        crate::native_output::scanout::AtomicOutputSwapchain,
        OutputFrameIdentitySnapshot,
    ) {
        let mut swapchain = test_swapchain(pool_generation);
        let slot = swapchain.acquire_render_slot().unwrap();
        swapchain
            .finish_render(slot, 17, test_render_fence())
            .unwrap();
        let identity = swapchain.ready_identity().unwrap();
        (swapchain, identity)
    }

    fn dispatched_pacing() -> NativeFramePacing {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enable_for_test();
        pacing.queue_visual(1, 17);
        pacing
            .note_render_started(NativeOutputPacingMode::ReactiveDouble, false)
            .expect("render attempt has its real active owner");
        pacing
            .mark_render_worker_dispatched()
            .expect("worker dispatch pins the active owner");
        pacing.cancel_unsubmitted_render();
        pacing
    }

    #[test]
    fn settlement_error_after_physical_ready_preserves_its_pacing_owner() {
        let (swapchain, identity) = ready_swapchain(4);
        let mut pacing = dispatched_pacing();
        let ownership =
            RenderWorkerPhysicalOwnershipSnapshot::from_swapchain(expected(identity), &swapchain);

        let disposition = settle_render_worker_failure_ownership(
            &mut pacing,
            expected(identity),
            crate::native_output::OutputEpochId::initial(),
            Some(ownership),
            22,
        )
        .expect("exact READY identity should be adopted");

        assert_eq!(
            disposition,
            RenderWorkerSettlementFailureDisposition::PhysicalReadyRetained
        );
        assert_eq!(pacing.unsubmitted_physical_identity(), Some(identity));
        assert!(pacing.has_unsettled_output_frame_ownership());
    }

    #[test]
    fn settlement_error_with_unsettled_or_generation_mismatched_slot_keeps_owner_pinned() {
        let expected_identity = physical_identity(9, 1, 4);
        let mismatched_generation = physical_identity(9, 1, 5);
        let mut pacing = dispatched_pacing();
        let mut ownership = empty_ownership(expected(expected_identity));
        ownership.ready_identity = Some(mismatched_generation);

        let disposition = settle_render_worker_failure_ownership(
            &mut pacing,
            expected(expected_identity),
            crate::native_output::OutputEpochId::initial(),
            Some(ownership),
            22,
        )
        .expect("same physical slot with different generation must stay pinned");

        assert_eq!(
            disposition,
            RenderWorkerSettlementFailureDisposition::PhysicalOwnershipConflict
        );
        assert_eq!(pacing.unsubmitted_physical_identity(), None);
        assert!(pacing.has_unsettled_output_frame_ownership());
    }

    #[test]
    fn ready_identity_mismatch_is_not_adopted() {
        let (swapchain, identity) = ready_swapchain(4);
        let mut wrong_output = identity;
        wrong_output.output_id = oblivion_one::core::OutputId::from_raw(2).unwrap();
        let mut wrong_frame = identity;
        wrong_frame.frame_id += 1;
        let mut wrong_transaction = identity;
        wrong_transaction.transaction_id = crate::native_output::OutputTransactionId::new(
            std::num::NonZeroU64::new(identity.transaction_id.get() + 1).unwrap(),
        );
        let mut wrong_batch = identity;
        wrong_batch.protocol_batch_id = oblivion_one::compositor::CompositorFrameBatchId::new(
            std::num::NonZeroU64::new(identity.protocol_batch_id.get() + 1).unwrap(),
        );
        let mut wrong_slot = identity;
        wrong_slot.slot = crate::native_output::scanout::OutputSlotId::new(2).unwrap();
        let mut wrong_framebuffer = identity;
        wrong_framebuffer.framebuffer_id =
            oblivion_one::native::kms::FramebufferId::new(43).unwrap();
        let mut wrong_render_generation = identity;
        wrong_render_generation.render_generation += 1;
        let mut wrong_pool_generation = identity;
        wrong_pool_generation.pool_generation += 1;
        let mut wrong_reservation = identity;
        let crate::native_output::presentation::transaction::FramePresentationReservation::Fixed(
            target,
        ) = identity.reservation
        else {
            panic!("test frame uses a fixed presentation reservation");
        };
        wrong_reservation.reservation =
            crate::native_output::presentation::transaction::FramePresentationReservation::Fixed(
                oblivion_one::native::presentation_deadline::PresentationTarget {
                    sequence: target.sequence + 1,
                    ..target
                },
            );

        for (label, mismatched_ready) in [
            ("output", wrong_output),
            ("frame", wrong_frame),
            ("transaction", wrong_transaction),
            ("batch", wrong_batch),
            ("slot", wrong_slot),
            ("framebuffer", wrong_framebuffer),
            ("render generation", wrong_render_generation),
            ("pool generation", wrong_pool_generation),
            ("reservation", wrong_reservation),
        ] {
            let expected = expected(identity);
            assert!(!expected.exactly_matches(mismatched_ready), "{label}");
            let mut ownership =
                RenderWorkerPhysicalOwnershipSnapshot::from_swapchain(expected, &swapchain);
            ownership.ready_identity = Some(mismatched_ready);
            let mut pacing = dispatched_pacing();
            let owner = pacing.active;

            let disposition = settle_render_worker_failure_ownership(
                &mut pacing,
                expected,
                crate::native_output::OutputEpochId::initial(),
                Some(ownership),
                22,
            )
            .expect("mismatched READY identity must be retained as a conflict");

            assert_eq!(
                disposition,
                RenderWorkerSettlementFailureDisposition::PhysicalOwnershipConflict,
                "{label}"
            );
            assert_eq!(pacing.active, owner, "{label}");
            assert!(pacing.has_unsettled_output_frame_ownership(), "{label}");
        }
    }

    #[test]
    fn settlement_error_with_slot_still_rendering_keeps_owner_pinned() {
        let identity = physical_identity(9, 1, 4);
        let mut pacing = dispatched_pacing();
        let mut ownership = empty_ownership(expected(identity));
        ownership.rendering_slot = Some(identity.slot);

        let disposition = settle_render_worker_failure_ownership(
            &mut pacing,
            expected(identity),
            crate::native_output::OutputEpochId::initial(),
            Some(ownership),
            22,
        )
        .expect("a still-rendering slot remains owned until fatal recovery");

        assert_eq!(
            disposition,
            RenderWorkerSettlementFailureDisposition::PhysicalOwnershipUnsettled
        );
        assert_eq!(pacing.unsubmitted_physical_identity(), None);
        assert!(pacing.has_unsettled_output_frame_ownership());
    }

    #[test]
    fn absent_swapchain_observations_do_not_prove_safe_frame_retirement() {
        let identity = physical_identity(9, 1, 4);
        let mut pacing = dispatched_pacing();
        let ownership = empty_ownership(expected(identity));

        let disposition = settle_render_worker_failure_ownership(
            &mut pacing,
            expected(identity),
            crate::native_output::OutputEpochId::initial(),
            Some(ownership),
            22,
        )
        .expect("an empty role snapshot is not terminalization evidence");

        assert_eq!(
            disposition,
            RenderWorkerSettlementFailureDisposition::NoTerminalRetirementProof
        );
        assert_eq!(pacing.unsubmitted_physical_identity(), None);
        assert!(pacing.has_unsettled_output_frame_ownership());
    }

    #[test]
    fn missing_or_replaced_swapchain_cannot_release_the_worker_owner() {
        let identity = physical_identity(9, 1, 4);
        let expected = expected(identity);
        let mut pacing = dispatched_pacing();
        let owner = pacing.active;

        assert_eq!(
            settle_render_worker_failure_ownership(
                &mut pacing,
                expected,
                crate::native_output::OutputEpochId::initial(),
                None,
                22,
            )
            .unwrap(),
            RenderWorkerSettlementFailureDisposition::SwapchainUnavailable
        );

        let mut replaced_pool = empty_ownership(expected);
        replaced_pool.pool_generation += 1;
        assert_eq!(
            settle_render_worker_failure_ownership(
                &mut pacing,
                expected,
                crate::native_output::OutputEpochId::initial(),
                Some(replaced_pool),
                22,
            )
            .unwrap(),
            RenderWorkerSettlementFailureDisposition::PoolGenerationMismatch
        );
        pacing.cancel_unsubmitted_render();
        assert_eq!(pacing.active, owner);
        assert!(pacing.has_unsettled_output_frame_ownership());
    }

    #[test]
    fn settlement_error_from_old_output_epoch_cannot_adopt_a_new_epoch_frame() {
        let identity = physical_identity(9, 1, 4);
        let mut pacing = dispatched_pacing();
        let mut ownership = empty_ownership(expected(identity));
        ownership.ready_identity = Some(identity);

        let disposition = settle_render_worker_failure_ownership(
            &mut pacing,
            expected(identity),
            crate::native_output::OutputEpochId::initial().next(),
            Some(ownership),
            22,
        )
        .expect("output epoch mismatch is retained as unsettled ownership");

        assert_eq!(
            disposition,
            RenderWorkerSettlementFailureDisposition::OutputEpochMismatch
        );
        assert_eq!(pacing.unsubmitted_physical_identity(), None);
        assert!(pacing.has_unsettled_output_frame_ownership());
    }

    #[test]
    fn settlement_error_with_slot_quarantined_keeps_owner_pinned() {
        let mut swapchain = test_swapchain(4);
        let slot = swapchain.acquire_render_slot().unwrap();
        let quarantine_fence = test_render_fence().take_timing_fd();
        swapchain
            .quarantine_rendering(
                quarantine_fence,
                crate::native_output::scanout::OutputQuarantineReason::PostDrawRenderFailure,
            )
            .unwrap();
        let identity = physical_identity(9, slot.get(), 4);
        let mut pacing = dispatched_pacing();
        let ownership =
            RenderWorkerPhysicalOwnershipSnapshot::from_swapchain(expected(identity), &swapchain);
        assert_eq!(ownership.quarantined_slot, Some(slot));

        let disposition = settle_render_worker_failure_ownership(
            &mut pacing,
            expected(identity),
            crate::native_output::OutputEpochId::initial(),
            Some(ownership),
            22,
        )
        .expect("a quarantined slot remains owned until fatal recovery");

        assert_eq!(
            disposition,
            RenderWorkerSettlementFailureDisposition::PhysicalOwnershipUnsettled
        );
        assert_eq!(pacing.unsubmitted_physical_identity(), None);
        assert!(pacing.has_unsettled_output_frame_ownership());
    }

    #[test]
    fn suspended_worker_queued_and_kernel_submitted_frames_stay_owned() {
        let current_epoch = crate::native_output::OutputEpochId::initial();
        let token = oblivion_one::native::kms::PageFlipToken::new(77).unwrap();

        let (mut suspended_swapchain, suspended_identity) = ready_swapchain(4);
        suspended_swapchain.suspend_abandon_ready().unwrap();
        let suspended_expected = expected(suspended_identity);
        let suspended = RenderWorkerPhysicalOwnershipSnapshot::from_swapchain(
            suspended_expected,
            &suspended_swapchain,
        );

        let (mut worker_swapchain, worker_identity) = ready_swapchain(4);
        worker_swapchain
            .take_ready_for_worker(token, crate::native_output::MonotonicTimestampNs::new(1))
            .unwrap();
        let worker_expected = expected(worker_identity);
        let worker_queued = RenderWorkerPhysicalOwnershipSnapshot::from_swapchain(
            worker_expected,
            &worker_swapchain,
        );

        let (mut submitted_swapchain, submitted_identity) = ready_swapchain(4);
        submitted_swapchain
            .take_ready_for_worker(token, crate::native_output::MonotonicTimestampNs::new(1))
            .unwrap();
        submitted_swapchain
            .promote_worker_queued(
                token,
                None,
                crate::native_output::MonotonicTimestampNs::new(2),
                crate::native_output::MonotonicTimestampNs::new(3),
            )
            .unwrap();
        let submitted_expected = expected(submitted_identity);
        let submitted = RenderWorkerPhysicalOwnershipSnapshot::from_swapchain(
            submitted_expected,
            &submitted_swapchain,
        );

        for (expected, ownership) in [
            (suspended_expected, suspended),
            (worker_expected, worker_queued),
            (submitted_expected, submitted),
        ] {
            let mut pacing = dispatched_pacing();
            let owner = pacing.active;
            assert_eq!(
                settle_render_worker_failure_ownership(
                    &mut pacing,
                    expected,
                    current_epoch,
                    Some(ownership),
                    22,
                )
                .unwrap(),
                RenderWorkerSettlementFailureDisposition::PhysicalOwnershipUnsettled
            );
            pacing.cancel_unsubmitted_render();
            assert_eq!(pacing.active, owner);
            assert!(pacing.has_unsettled_output_frame_ownership());
        }
    }

    #[test]
    fn successful_terminal_no_frame_outcome_releases_only_after_slot_cancellation() {
        let identity = physical_identity(1, 1, 4);
        let slots = crate::native_output::scanout::OutputSlotSet::new([
            crate::native_output::scanout::OutputSlotId::new(0).unwrap(),
            identity.slot,
            crate::native_output::scanout::OutputSlotId::new(2).unwrap(),
        ])
        .unwrap();
        let mut swapchain =
            crate::native_output::scanout::AtomicOutputSwapchain::from_presented_slots(
                slots,
                crate::native_output::scanout::OutputSlotId::new(0).unwrap(),
                identity.pool_generation,
            )
            .unwrap();
        let rendering_slot = swapchain.acquire_render_slot().unwrap();
        assert_eq!(rendering_slot, identity.slot);

        let worker_expected = expected(identity);
        let mut pacing = dispatched_pacing();
        let owner = pacing.active;
        let rendering =
            RenderWorkerPhysicalOwnershipSnapshot::from_swapchain(worker_expected, &swapchain);
        assert_eq!(rendering.rendering_slot, Some(identity.slot));
        assert_eq!(
            settle_render_worker_failure_ownership(
                &mut pacing,
                worker_expected,
                crate::native_output::OutputEpochId::initial(),
                Some(rendering),
                22,
            )
            .unwrap(),
            RenderWorkerSettlementFailureDisposition::PhysicalOwnershipUnsettled
        );
        pacing.cancel_unsubmitted_render();
        assert_eq!(pacing.active, owner);

        swapchain.cancel_render_before_gpu(rendering_slot).unwrap();
        assert_eq!(swapchain.rendering_slot(), None);

        let outcome =
            crate::native_output::scanout::AtomicFrameRenderOutcome::RetryPresentationState {
                render_us: 0,
            };
        settle_render_worker_after_terminal_no_frame(&mut pacing, &outcome)
            .expect("successful scanout settlement is explicit no-frame evidence");
        assert_eq!(pacing.active, owner);
        pacing.cancel_unsubmitted_render();
        assert!(!pacing.has_unsettled_output_frame_ownership());

        // Reconfiguration is now allowed to move the lifecycle to a new
        // output epoch and pool. A subsequent worker frame must be able to
        // complete the full READY -> worker -> pending -> pageflip path.
        let next_epoch = crate::native_output::OutputEpochId::initial().next();
        pacing.rebase_after_output_epoch();
        let mut next_swapchain = test_swapchain(5);
        pacing.queue_visual(30, 18);
        pacing
            .note_render_started(NativeOutputPacingMode::ReactiveDouble, false)
            .unwrap();
        pacing.mark_render_worker_dispatched().unwrap();
        let next_slot = next_swapchain.acquire_render_slot().unwrap();
        next_swapchain
            .finish_render(next_slot, 18, test_render_fence())
            .unwrap();
        let next_identity = next_swapchain.ready_identity().unwrap();
        let mut next_expected = expected(next_identity);
        next_expected.output_epoch = next_epoch;
        let next_ownership =
            RenderWorkerPhysicalOwnershipSnapshot::from_swapchain(next_expected, &next_swapchain);
        assert_eq!(
            settle_render_worker_failure_ownership(
                &mut pacing,
                next_expected,
                next_epoch,
                Some(next_ownership),
                31,
            )
            .unwrap(),
            RenderWorkerSettlementFailureDisposition::PhysicalReadyRetained
        );
        assert!(pacing.has_unsettled_output_frame_ownership());

        let token = oblivion_one::native::kms::PageFlipToken::new(78).unwrap();
        let (_submission_fence, _) = next_swapchain
            .take_ready_for_worker(token, crate::native_output::MonotonicTimestampNs::new(32))
            .unwrap();
        next_swapchain
            .promote_worker_queued(
                token,
                None,
                crate::native_output::MonotonicTimestampNs::new(33),
                crate::native_output::MonotonicTimestampNs::new(34),
            )
            .unwrap();
        pacing.note_submit(
            token.get(),
            34,
            true,
            NativeOutputPacingMode::ReactiveDouble,
        );
        let completed = next_swapchain.complete_pageflip(token, 5).unwrap();
        let completed_identity = OutputFrameIdentitySnapshot::from(&completed.frame);
        assert_eq!(completed_identity, next_identity);
        pacing.note_pageflip_exact(Some(completed_identity), 35, 34, token.get(), 16_666);
        assert!(!pacing.has_unsettled_output_frame_ownership());
        assert_eq!(next_swapchain.current(), next_slot);
    }
}

#[cfg(test)]
mod tests {
    use super::{effect_reload_admission_available, handoff_render_worker_ready};
    use crate::native_output::presentation::transaction::test_deferred_o1_reservation;
    use crate::native_output::runtime::{NativeFramePacing, NativeFrameScheduler};
    use crate::native_output::scanout::{OutputFrameIdentitySnapshot, OutputSlotId};
    use oblivion_one::native::scheduler::NativeOutputPacingMode;
    use std::num::NonZeroU64;

    fn identity(frame_id: u64) -> OutputFrameIdentitySnapshot {
        OutputFrameIdentitySnapshot {
            output_id: oblivion_one::core::OutputId::from_raw(1).unwrap(),
            frame_id,
            protocol_batch_id: oblivion_one::compositor::CompositorFrameBatchId::new(
                NonZeroU64::new(frame_id).unwrap(),
            ),
            transaction_id: crate::native_output::OutputTransactionId::new(
                NonZeroU64::new(frame_id).unwrap(),
            ),
            slot: OutputSlotId::new(1).unwrap(),
            framebuffer_id: oblivion_one::native::kms::FramebufferId::new(frame_id as u32).unwrap(),
            render_generation: frame_id,
            pool_generation: 1,
            reservation: test_deferred_o1_reservation(),
        }
    }

    #[test]
    fn worker_completion_hands_off_reactive_ready_without_lane_wait() {
        for waits_for_target in [false, true] {
            let mut pacing = NativeFramePacing::from_env();
            let mut scheduler = NativeFrameScheduler::new_with_refresh_interval_ns(16_666_667, 0);
            pacing.queue_visual(1, 1);
            scheduler.queue_visual_work();
            pacing
                .begin_render_attempt(NativeOutputPacingMode::ReactiveDouble, false)
                .unwrap();
            pacing.mark_render_worker_dispatched().unwrap();
            let first = identity(1);
            pacing.bind_physical_output_frame(first).unwrap();
            scheduler.consume_visual_work();

            handoff_render_worker_ready(&mut pacing, &mut scheduler, 2, waits_for_target, false)
                .unwrap();
            assert_eq!(pacing.active, None);
            assert!(pacing.ready.is_some());
            assert_eq!(pacing.unsubmitted_physical_identity(), Some(first));
            assert_eq!(pacing.active_physical_identity(), None);
            assert!(scheduler.ready_frame_queued());

            // The next render must be able to bind a different physical frame
            // without inheriting the completed worker's active identity.
            pacing.queue_visual(3, 2);
            pacing
                .begin_render_attempt(NativeOutputPacingMode::ReactiveDouble, false)
                .unwrap();
            pacing.bind_physical_output_frame(identity(2)).unwrap();
            assert_eq!(pacing.active_physical_identity(), Some(identity(2)));
            assert_eq!(pacing.unsubmitted_physical_identity(), Some(first));
        }
    }

    #[test]
    fn worker_completion_hands_off_untimed_ready_for_a_fixed_reservation() {
        use crate::native_output::presentation::transaction::FramePresentationReservation;
        use oblivion_one::native::presentation_deadline::{
            MonotonicTimestampNs, PresentationTarget, PresentationTargetReason,
            PrimaryRefreshClaim, TargetSelectionEvidence,
        };

        let at = MonotonicTimestampNs::new(10);
        let target = PresentationTarget {
            sequence: 1219,
            presentation_time: at,
            submit_not_before: at,
            render_start_deadline: at,
            refresh_interval: std::time::Duration::from_nanos(6_060_637),
            reason: PresentationTargetReason::PredictedPressure,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
            physical_claim: PrimaryRefreshClaim {
                sequence: 1219,
                presentation_time: at,
                clock_generation: 1,
            },
            selection_evidence: TargetSelectionEvidence::default(),
        };
        let mut pacing = NativeFramePacing::from_env();
        let mut scheduler = NativeFrameScheduler::new_with_refresh_interval_ns(6_060_637, 0);
        pacing.queue_visual(1, 1);
        scheduler.queue_visual_work();
        pacing
            .begin_render_attempt(NativeOutputPacingMode::ReactiveDouble, false)
            .unwrap();
        pacing.mark_render_worker_dispatched().unwrap();
        let mut fixed = identity(1);
        fixed.reservation = FramePresentationReservation::Fixed(target);
        pacing.bind_physical_output_frame(fixed).unwrap();
        scheduler.consume_visual_work();

        handoff_render_worker_ready(&mut pacing, &mut scheduler, 2, true, false).unwrap();
        assert!(scheduler.ready_frame_queued());
        // No scheduler timing claim, but the READY is known to be untimed so
        // ownership checks do not mistake it for stale target metadata.
        assert_eq!(scheduler.ready_target(), None);
        assert!(scheduler.ready_frame_untimed());
        assert_eq!(
            scheduler.decision(0),
            oblivion_one::native::scheduler::SchedulerDecision::SubmitReady
        );
    }

    #[test]
    fn effect_reload_admission_is_single_flight_and_requires_idle_worker() {
        assert!(effect_reload_admission_available(true, false, false));
        assert!(!effect_reload_admission_available(true, true, false));
        assert!(!effect_reload_admission_available(true, false, true));
        assert!(!effect_reload_admission_available(false, false, false));
    }
}
