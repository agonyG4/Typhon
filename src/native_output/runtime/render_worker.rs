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
        let outer = self
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
            crate::native_output::scanout::AtomicFrameRenderOutcome::Skipped {
                reason,
                render_us,
                dmabuf_gpu_release,
            } => {
                self.frame_pacing
                    .note_predictive_o1_other_safe_abandonment();
                self.frame_pacing.cancel_unsubmitted_render();
                if let Some((lease_id, release_fence)) = dmabuf_gpu_release {
                    let completion_fd = release_fence.duplicate_completion_fd();
                    match completion_fd {
                        Ok(completion_fd) => {
                            match self.dmabuf_gpu_release_registry.register_with_origin(
                                lease_id,
                                completion_fd,
                                &mut self.event_loop,
                                DmabufGpuReleaseOrigin::NoVisual,
                                0,
                                monotonic_now_ns().unwrap_or(0),
                            ) {
                                Ok(_) => {
                                    self.dmabuf_gpu_release_registry.note_no_visual_fence_only();
                                    if self.server.explicit_release_signal_retry_count() == 0 {
                                        self.dmabuf_gpu_release_registry.complete_retry();
                                    } else {
                                        self.dmabuf_gpu_release_registry.retry_after_failure(
                                            DmabufReleaseRetryReason::ExplicitReleaseSignalFailed,
                                            monotonic_now_ns()?,
                                        );
                                    }
                                }
                                Err(_) => {
                                    self.dmabuf_gpu_release_registry.note_registration_failure();
                                    self.server.requeue_dmabuf_gpu_release_lease(lease_id);
                                    self.dmabuf_gpu_release_registry.retry_after_failure(
                                        DmabufReleaseRetryReason::ReactorRegistrationFailed,
                                        monotonic_now_ns()?,
                                    );
                                }
                            }
                        }
                        Err(_) => {
                            self.dmabuf_gpu_release_registry
                                .note_completion_fd_failure();
                            self.server.requeue_dmabuf_gpu_release_lease(lease_id);
                            self.dmabuf_gpu_release_registry.retry_after_failure(
                                DmabufReleaseRetryReason::CompletionFdDuplicationFailed,
                                monotonic_now_ns()?,
                            );
                        }
                    }
                } else if self.server.explicit_release_signal_retry_count() > 0 {
                    self.dmabuf_gpu_release_registry.retry_after_failure(
                        DmabufReleaseRetryReason::ExplicitReleaseSignalFailed,
                        monotonic_now_ns()?,
                    );
                } else if self.server.retryable_deferred_dmabuf_release_count() > 0 {
                    let reason = if release_safety.permits_compositor_gpu_release() {
                        DmabufReleaseRetryReason::NoGpuProofAvailable
                    } else {
                        DmabufReleaseRetryReason::DirectKmsOwnershipBlocked
                    };
                    self.dmabuf_gpu_release_registry
                        .retry_after_failure(reason, monotonic_now_ns()?);
                }
                self.render_telemetry.record_skipped(render_us);
                self.frame_scheduler.note_immediate_completion();
                frame_completed = true;
                super::commit_timing::retire_logical_scene_generation(
                    &mut self.last_rendered_scene_generation,
                    outer.scene_generation,
                    true,
                );
                self.presentation_deadline.clear_scheduled_target();
                self.scheduled_presentation_target = None;
                if self.render_demand_coalesced_count == outer.coalesced_demand_count_at_dispatch {
                    self.queued_redraw_requested = false;
                }
                self.perf.log("native.atomic_render_skipped", || {
                    vec![
                        NativePerfField::str("reason", format!("{reason:?}")),
                        NativePerfField::u64("render_us", render_us),
                        NativePerfField::u64("scene_generation", outer.scene_generation),
                        NativePerfField::u64("cursor_epoch", outer.cursor_epoch),
                        NativePerfField::str("output_damage", "empty"),
                    ]
                });
            }
            crate::native_output::scanout::AtomicFrameRenderOutcome::RetryPresentationState {
                render_us,
            } => {
                self.frame_pacing
                    .note_predictive_o1_other_safe_abandonment();
                self.frame_pacing.cancel_unsubmitted_render();
                self.render_telemetry.record_skipped(render_us);
                self.frame_scheduler.note_immediate_completion();
                frame_completed = true;
                self.queued_redraw_requested = true;
                self.last_acquire_ready_at_ns = None;
                self.presentation_deadline.clear_scheduled_target();
                self.scheduled_presentation_target = None;
                self.perf.log("native.atomic_presentation_state_retry", || {
                    vec![
                        NativePerfField::u64("render_us", render_us),
                        NativePerfField::u64("scene_generation", outer.scene_generation),
                        NativePerfField::str("output_damage", "forced-full"),
                    ]
                });
            }
            crate::native_output::scanout::AtomicFrameRenderOutcome::LifecycleFallback {
                fallbacks,
                render_us,
            } => {
                let fallback_count = fallbacks.failed.len();
                for fallback in fallbacks.failed {
                    self.server.apply_lifecycle_render_fallback(fallback);
                }
                self.frame_pacing
                    .note_predictive_o1_other_safe_abandonment();
                self.frame_pacing.cancel_unsubmitted_render();
                self.frame_scheduler.note_immediate_completion();
                frame_completed = true;
                self.queued_redraw_requested = true;
                self.last_acquire_ready_at_ns = None;
                self.perf.log("native.atomic_lifecycle_fallback", || {
                    vec![
                        NativePerfField::usize("fallbacks", fallback_count),
                        NativePerfField::u64("render_us", render_us),
                        NativePerfField::bool("physical_lifecycle_ledger_changed", false),
                    ]
                });
            }
            crate::native_output::scanout::AtomicFrameRenderOutcome::Rendered {
                frame_id,
                transaction_id,
                protocol_batch_id,
                render_us,
                repaint_stats,
                lifecycle_snapshot,
                resolved_snapshot,
                resolved_scene_signature,
                render_damage_signature,
                repair_damage_signature,
                resolved_render_generation,
                framebuffer_slot,
                deferred_o1_binding_advanced_intervals,
                deferred_o1_binding_failure,
            } => {
                let physical_identity = self
                    .scanout
                    .explicit_output_swapchain()
                    .and_then(|swapchain| swapchain.ready_identity())
                    .ok_or_else(|| {
                        io::Error::other("worker-rendered Atomic frame has no physical identity")
                    })?;
                if let Err(error) = self.frame_pacing.bind_predictive_o1(physical_identity) {
                    self.server
                        .cancel_lifecycle_precapture_after_render_failure();
                    self.frame_pacing.note_predictive_o1_failed();
                    self.frame_pacing.cancel_unsubmitted_render();
                    return Err(io::Error::other(error).into());
                }
                self.frame_pacing.note_render_ready();
                if let Some(advanced_intervals) = deferred_o1_binding_advanced_intervals {
                    self.frame_pacing
                        .note_predictive_binding_after_render_completion(advanced_intervals);
                }
                let settled_lifecycle_releases =
                    self.server.settle_lifecycle_precapture_for_rendered_frame(
                        protocol_batch_id,
                        &lifecycle_snapshot,
                    );
                let mut lease_id = release_lease_id;
                if settled_lifecycle_releases > 0 && lease_id.is_none() {
                    lease_id = Some(self.dmabuf_gpu_release_registry.allocate_lease_id()?);
                }
                if let crate::native_output::scanout::NativeScanoutBackend::AtomicEglGbm(explicit) =
                    &*self.scanout
                {
                    let _ = arm_composited_dmabuf_release(
                        &mut self.dmabuf_gpu_release_registry,
                        &mut self.server,
                        explicit,
                        release_safety,
                        &mut self.event_loop,
                        protocol_batch_id,
                        lease_id,
                        transaction_id,
                    )?;
                } else {
                    return Err(io::Error::other(
                        "Atomic render worker completion reached a compatibility scanout",
                    )
                    .into());
                }
                if outer
                    .o1_admission
                    .is_some_and(|admission| admission.used_extra_credit)
                {
                    self.frame_pacing.note_o1_credit2_extra_credit_consumed();
                }
                let presented_frame_id = self.scene_history.presented_frame_id();
                super::presentation_worker::record_atomic_rendered_scene(
                    &mut self.scene_history,
                    &mut self.presentation_trace,
                    frame_id,
                    transaction_id,
                    resolved_render_generation,
                    resolved_snapshot,
                    outer.presentation_snapshot,
                    lifecycle_snapshot,
                    resolved_scene_signature,
                    render_damage_signature,
                    repair_damage_signature,
                    presented_frame_id,
                    framebuffer_slot,
                    outer.cursor_damage,
                    repaint_stats.buffer_age,
                )?;
                self.render_telemetry.record_rendered(
                    render_us,
                    repaint_stats,
                    outer.output_width,
                    outer.output_height,
                );
                self.frame_scheduler.consume_visual_work();
                frame_rendered = true;
                let waits_for_target =
                    oblivion_one::native::scheduler::rendered_primary_must_wait_for_lane(
                        outer.render_ahead,
                        self.atomic_commit_arbiter.atomic_commit_pending(),
                        outer.can_queue_worker_next,
                    );
                if let Some(failure) = deferred_o1_binding_failure {
                    if !outer.render_ahead || !waits_for_target {
                        return Err(io::Error::other(
                            "deferred O1 worker result became stale outside its guarded lane",
                        )
                        .into());
                    }
                    self.server
                        .note_frame_callbacks_deferred_ready(protocol_batch_id);
                    let ready_at_ns = monotonic_now_ns()?;
                    self.frame_pacing
                        .note_ready_frame(ready_at_ns, waits_for_target);
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
                            &mut self.scene_history,
                            &mut self.frame_pacing,
                            &mut self.frame_scheduler,
                            &mut self.server,
                            &mut self.output_transactions,
                            MonotonicTimestampNs::new(ready_at_ns),
                        )?,
                        _ => {
                            return Err(io::Error::other(
                                "Atomic render worker completion reached a compatibility scanout",
                            )
                            .into());
                        }
                    }
                    self.queued_redraw_requested = true;
                } else if waits_for_target {
                    self.server
                        .note_frame_callbacks_deferred_ready(protocol_batch_id);
                    self.frame_pacing
                        .note_ready_frame(monotonic_now_ns()?, waits_for_target);
                    if outer.render_ahead {
                        self.frame_pacing.note_predictive_unbound_ready();
                    }
                }
                self.presentation_trace
                    .push(PresentationTransactionEvent::TransactionBuilt {
                        transaction_id,
                        timestamp_ns: monotonic_now_ns()?,
                    });
                self.presentation_trace
                    .push(PresentationTransactionEvent::AcquireReady {
                        transaction_id,
                        timestamp_ns: monotonic_now_ns()?,
                    });
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
                    outer.output_generation,
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

    pub(super) fn process_render_worker_events(&mut self) -> NativeResult<()> {
        loop {
            let event = self.scanout.try_take_render_worker_event()?;
            let Some(event) = event else {
                return Ok(());
            };
            match event {
                crate::native_output::scanout::AtomicRenderWorkerEvent::Rendered(completion) => {
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
    use super::effect_reload_admission_available;

    #[test]
    fn effect_reload_admission_is_single_flight_and_requires_idle_worker() {
        assert!(effect_reload_admission_available(true, false, false));
        assert!(!effect_reload_admission_available(true, true, false));
        assert!(!effect_reload_admission_available(true, false, true));
        assert!(!effect_reload_admission_available(false, false, false));
    }
}
