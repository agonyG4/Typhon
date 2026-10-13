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
) {
    frame_pacing.note_ready_frame(ready_at_ns, waits_for_target);
    frame_scheduler.note_ready_frame(None);
    if render_ahead {
        frame_pacing.note_predictive_unbound_ready();
    }
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
                self.frame_pacing.note_predictive_o1_failed();
                self.frame_pacing.cancel_unsubmitted_render();
                return Err(Box::new(error));
            }
        };

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
                        .note_ready_frame(settled.ready_at_ns, waits_for_target);
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
                    );
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
                        vec![
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
            let first = identity(1);
            pacing.bind_physical_output_frame(first).unwrap();
            scheduler.consume_visual_work();

            handoff_render_worker_ready(&mut pacing, &mut scheduler, 2, waits_for_target, false);
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
    fn effect_reload_admission_is_single_flight_and_requires_idle_worker() {
        assert!(effect_reload_admission_available(true, false, false));
        assert!(!effect_reload_admission_available(true, true, false));
        assert!(!effect_reload_admission_available(true, false, true));
        assert!(!effect_reload_admission_available(false, false, false));
    }
}
