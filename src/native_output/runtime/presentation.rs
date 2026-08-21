use super::commit_timing::{refreshed_published_state, reset_after_same_buffer};
use super::cycle::direct_fallback::{DirectFallbackReason, DirectFallbackTracker};
use super::frame::{
    NativeRepaintInputs, native_repaint_decision, plane_delta_allowed_at_deadline,
    update_cursor_output_arbitration,
};
use super::planner::{
    NativePresentationPath, NativePresentationPlanInput, pending_target_for_scanout,
    plan_native_presentation_path, plan_visual_target_for_mode,
    prepare_presentation_target_for_mode, reactive_or_commit_timing_target,
    take_reactive_or_commit_timing_target,
};
use super::presentation_cursor::*;
use super::presentation_direct::{
    DirectPresentationInputs, inspect_direct_presentation, log_prepared_primary_arbitration,
    suppress_direct_render_ahead,
};
use super::presentation_metrics::{
    PipelineSchedulingDiagnostics, log_output_pipeline_snapshot, note_same_buffer_suppressed,
};
use super::presentation_pipeline::build_output_pipeline_snapshot_with_presented;
use super::presentation_protocol::{
    ProtocolCycleMetrics, complete_protocol_only_tick, log_no_visual_work,
    log_wait_for_presentation,
};
#[cfg(test)]
use super::presentation_transactions::complete_immediate_output_transaction_with;
#[cfg(test)]
use super::presentation_transactions::present_compatibility_frame;
use super::presentation_transactions::{
    complete_immediate_output_transaction, register_primary_transaction,
};
use super::presentation_worker::*;
use super::*;
use crate::native_output::kms_worker::{KmsCommitWorkerTransport, KmsTestOnlyPolicy};
use oblivion_one::native::kms::KmsBackendKind;
use oblivion_one::native::scheduler::rendered_primary_must_wait_for_lane;
impl NativeRuntime {
    pub(super) fn render_present_and_update_metrics(
        &mut self,
        cycle: &mut NativeCycleState,
    ) -> NativeResult<()> {
        let perf = self.perf;
        let Self {
            server,
            cursor_image,
            cursor_manager,
            perf: _,
            kms,
            kms_backend,
            target,
            mode_label,
            refresh_hz,
            drm_file_generation,
            drm_timestamp_clock: _,
            presentation_clock: _,
            scanout,
            frame_renderer,
            input_state,
            cursor_preference,
            cursor_scheduling_policy,
            cursor_output_arbitration,
            direct_scanout_preference,
            cursor_render_mode,
            atomic_cursor,
            legacy_cursor,
            input_devices: _,
            acquire_notifier: _,
            acquire_watches,
            parked_acquire_watches: _,
            event_loop,
            drm_reactor_token: _,
            output_render_fence_token,
            kms_commit_worker,
            kms_commit_worker_transport,
            frame_scheduler,
            atomic_commit_arbiter,
            emergency_quarantined_worker_jobs,
            output_transactions,
            presented_planes,
            confirmed_output_presentation,
            presentation_timing,
            presentation_deadline,
            scheduled_presentation_target,
            render_journal,
            adaptive_buffering,
            triple_buffer_policy,
            pending_proven_deadline_miss: _,
            effective_app_gpu_policy: _,
            last_rendered_scene_generation,
            last_direct_candidate_key,
            direct_fallback_tracker,
            last_refresh_sequence,
            last_submitted_cursor_epoch,
            scene_history,
            last_client_cursor_damage,
            last_software_cursor_damage,
            last_client_cursor_path,
            queued_redraw_requested,
            frame_index,
            known_toplevels: _,
            pending_launches: _,
            mismatched_pageflip_events: _,
            stale_pageflip_events: _,
            presentation_cadence: _,
            frame_pacing,
            presentation_trace,
            last_acquire_ready_at_ns,
            resize_perf: _,
            pointer_constraint_backend: _,
            render_telemetry,
            seat_session: _,
            process_supervisor: _,
            shutdown: _,
            session,
            #[cfg(test)]
            native_io_recorder,
            ..
        } = self;
        #[rustfmt::skip]
        let validation_base_context = (kms_commit_worker.as_ref(), *presented_planes, *drm_file_generation, target.crtc_id);
        let wakeup = &cycle.wakeup;
        let worker_mode = *kms_commit_worker_transport == KmsCommitWorkerTransport::Worker;
        let mut frame_completed = cycle.frame_completed;
        let mut frame_rendered = cycle.frame_rendered;
        let mut frame_submitted = cycle.frame_submitted;
        let pageflip_drain_us = cycle.pageflip_drain_us;
        let pageflip_completed = cycle.pageflip_completed;
        let present_us = cycle.present_us;
        let pageflip_pending_at_tick = cycle.pageflip_pending_at_tick;
        let tick_us = cycle.tick_us;
        let accepted = cycle.accepted;
        let redraw_requested = cycle.redraw_requested;
        let skipped_input_repaints = cycle.skipped_input_repaints;
        let input_drain_us = cycle.input_drain_us;
        let raw_input_events = cycle.raw_input_events;
        let coalesced_input_events = cycle.coalesced_input_events;
        #[rustfmt::skip] let (render_generation, _scene_generation, scene_changed, pending_frame_work) = refreshed_published_state(server, *last_rendered_scene_generation);
        let pacing_now_ns = monotonic_now_ns()?;
        #[rustfmt::skip] synchronize_active_cursor_image(server, cursor_manager, cursor_image, frame_renderer, scanout, queued_redraw_requested);
        let (client_cursor, client_cursor_active, cursor_visible) =
            resolve_native_cursor_visibility(server, input_state);
        #[rustfmt::skip] prepare_legacy_cursor_for_frame(legacy_cursor, kms, target.crtc_id, cursor_image, cursor_render_mode, cursor_manager, client_cursor_active, perf)?;
        let (mut runtime_plane_plan, mut client_cursor_hardware_usable) = (None, false);
        if let Some(cursor) = atomic_cursor.as_mut() {
            let cursor_image_ready = prepare_cursor_image(
                cursor,
                cursor_image,
                cursor_manager.generation(),
                client_cursor,
                kms,
                input_state,
                cursor_manager,
                perf,
            );
            let plan = apply_cursor_policy_with_runtime_inputs(
                CursorPolicyContext {
                    cursor,
                    cursor_visible,
                    cursor_image_ready,
                    output_width: target.width,
                    output_height: target.height,
                    cursor_preference: *cursor_preference,
                    cursor_scheduling_policy: *cursor_scheduling_policy,
                    presented_primary: presented_planes.primary,
                    predictive_triple_active: adaptive_buffering.mode()
                        == AdaptiveBufferingMode::Triple,
                    client_cursor_active,
                    cursor_render_mode,
                    last_client_cursor_damage,
                },
                kms_commit_worker.as_ref(),
                *drm_file_generation,
                target.crtc_id,
                *scheduled_presentation_target,
                presented_planes.cursor,
                atomic_commit_arbiter.atomic_commit_pending(),
                perf,
            );
            client_cursor_hardware_usable = plan_uses_hardware_cursor(&plan);
            runtime_plane_plan = Some(plan);
        } else if client_cursor_active {
            *cursor_render_mode = NativeCursorRenderMode::SoftwareClient;
        } else if *cursor_preference == NativeCursorPreference::Software || legacy_cursor.is_none()
        {
            *cursor_render_mode = NativeCursorRenderMode::Software;
        } else {
            *cursor_render_mode = NativeCursorRenderMode::Hardware;
        }
        if let Some(cursor) = atomic_cursor.as_mut() {
            cursor.set_hardware_path_active(
                *cursor_render_mode == NativeCursorRenderMode::Hardware
                    && !cursor.capability_quarantined(),
            );
        }
        let (current_client_cursor_damage, current_software_cursor_damage) = cursor_damage_states(
            client_cursor,
            target.width,
            target.height,
            *cursor_render_mode,
            cursor_visible,
            client_cursor_active,
            input_state,
            cursor_image,
        );
        let client_cursor_software_work = planned_client_cursor_software_work(
            runtime_plane_plan.as_ref(),
            client_cursor_hardware_usable,
            last_client_cursor_damage.as_ref(),
            current_client_cursor_damage,
            client_cursor_active,
        );
        let mut effective_cursor = effective_cursor_for_plan(
            runtime_plane_plan.as_ref(),
            atomic_cursor.as_ref(),
            *cursor_render_mode,
            cursor_visible,
        );
        #[rustfmt::skip] let planned_cursor_delivery = presented_delivery_for_plan(runtime_plane_plan.as_ref(), &effective_cursor);
        let cursor_state_changed = atomic_cursor
            .as_ref()
            .is_some_and(|cursor| cursor.needs_submission_for(effective_cursor.as_ref()));
        let cursor_epoch = atomic_cursor.as_ref().map_or(
            *last_submitted_cursor_epoch,
            NativeAtomicCursor::desired_epoch,
        );
        let primary_cursor = plan_primary_cursor_presentation(runtime_plane_plan.as_ref());
        let hardware_cursor_work_pending = planned_hardware_cursor_work_pending(
            runtime_plane_plan.as_ref(),
            cursor_state_changed,
            atomic_cursor.as_ref(),
            *cursor_render_mode,
        );
        log_client_cursor_path_if_changed(
            last_client_cursor_path,
            client_cursor_active,
            client_cursor_hardware_usable,
            presented_planes
                .primary
                .is_some_and(|assignment| assignment.is_direct()),
            client_cursor,
            perf,
        );
        let (_cursor_epoch_changed, cursor_deadline_due, cursor_work_pending) =
            update_cursor_output_arbitration(
                cursor_output_arbitration,
                cursor_epoch,
                *last_submitted_cursor_epoch,
                pacing_now_ns,
                frame_scheduler,
                client_cursor_software_work,
                hardware_cursor_work_pending,
            );
        let primary_redraw_requested =
            redraw_requested || (cursor_deadline_due && client_cursor_software_work);
        let repaint_decision = native_repaint_decision(NativeRepaintInputs {
            accepted_clients: accepted > 0,
            render_generation_changed: scene_changed,
            pending_frame_work,
            only_pending_surface_frame_callbacks: server.has_only_pending_surface_frame_callbacks(),
            redraw_requested: primary_redraw_requested,
            cursor_work_pending,
            page_flip_pending: false,
        });
        if repaint_decision.repaint {
            frame_pacing.queue_visual(pacing_now_ns, render_generation);
            frame_scheduler.queue_visual_work();
            *queued_redraw_requested |= primary_redraw_requested;
        } else if repaint_decision.protocol_only_present {
            frame_scheduler.queue_protocol_work(monotonic_now_ns()?);
        }
        let scheduler_now = MonotonicTimestampNs::new(monotonic_now_ns()?);
        let refresh_interval = super::plane_cycle::output_refresh_interval(*refresh_hz);
        let prediction = render_journal.prediction_at_with_kms_guard(
            scheduler_now,
            refresh_interval,
            presentation_timing.apply_guard_ns(),
        );
        let predicted_total_cost = Duration::from_nanos(prediction.total_cost_ns);
        let explicit_output = matches!(&**scanout, NativeScanoutBackend::AtomicEglGbm(_));
        let triple_capability = match scanout.explicit_output_swapchain() {
            Some(swapchain) => derive_triple_capability(TripleCapabilityInputs {
                atomic_kms: kms_backend.effective_kind() == KmsBackendKind::Atomic,
                explicit_swapchain: true,
                slot_capacity: swapchain.slot_capacity(),
                primary_in_fence: kms_backend
                    .atomic()
                    .is_some_and(|atomic| atomic.discovery().optional.in_fence_fd),
                render_fence_export: true,
                submission_transport_healthy: match kms_commit_worker_transport {
                    KmsCommitWorkerTransport::Synchronous => true,
                    KmsCommitWorkerTransport::Worker => kms_commit_worker
                        .as_ref()
                        .is_some_and(|worker| worker.fatal_reason().is_none()),
                },
                session_active: session.permits_output(),
                output_generation_stable: swapchain.pool_generation() == *drm_file_generation,
                ordinary_vsync: true,
                swapchain_poisoned: swapchain.is_poisoned(),
                software_cursor_visible: cursor_visible && cursor_render_mode.is_software(),
            }),
            None if kms_backend.effective_kind() != KmsBackendKind::Atomic => {
                TripleCapability::Unavailable(TripleCapabilityBlocker::NonAtomicKms)
            }
            None => {
                TripleCapability::Unavailable(TripleCapabilityBlocker::ExplicitSwapchainUnavailable)
            }
        };
        adaptive_buffering.apply_capability(triple_capability);
        let render_ahead_allowed = adaptive_buffering.mode() == AdaptiveBufferingMode::Triple;
        let pacing_mode = adaptive_buffering.pacing_mode();
        *scheduled_presentation_target = prepare_presentation_target_for_mode(
            pacing_mode,
            explicit_output,
            presentation_deadline,
            server,
            frame_scheduler,
            *scheduled_presentation_target,
            scheduler_now,
            predicted_total_cost,
        );
        #[rustfmt::skip] let (render_generation, scene_generation, scene_changed, pending_frame_work) = refreshed_published_state(server, *last_rendered_scene_generation);
        #[rustfmt::skip] let pending_target = if explicit_output && frame_scheduler.visual_work_queued() && scheduled_presentation_target.is_none() { pending_target_for_scanout(scanout)? } else { None };
        *scheduled_presentation_target = plan_visual_target_for_mode(
            presentation_deadline,
            pacing_mode,
            pending_target,
            scheduler_now,
            predicted_total_cost,
            explicit_output,
            frame_scheduler.visual_work_queued(),
            *triple_buffer_policy == AdaptiveTripleBufferPolicy::Force,
            *scheduled_presentation_target,
        );
        let effective_render_target_available = if explicit_output {
            scanout.render_target_available_for(pacing_mode)
        } else {
            scanout.render_target_available()
        };
        let worker_queue_available = worker_mode
            && atomic_commit_arbiter.worker_slot_available()
            && kms_commit_worker
                .as_ref()
                .is_some_and(|worker| worker.admission_available());
        let pipeline_snapshot = if explicit_output {
            let swapchain =
                super::presentation_pipeline::require_explicit_output_swapchain(scanout)?;
            let pipeline = build_output_pipeline_snapshot_with_presented(
                *drm_file_generation,
                target.crtc_id,
                pacing_mode,
                swapchain,
                output_transactions,
                atomic_commit_arbiter,
                scanout.explicit_presented_direct_ownership(),
                *scheduled_presentation_target,
                triple_capability,
                *presented_planes,
            )
            .map_err(|error| {
                io::Error::other(format!(
                    "output pipeline snapshot mismatch: generation={} crtc={} kernel={:?} worker={:?} error={error}",
                    drm_file_generation,
                    target.crtc_id,
                    atomic_commit_arbiter.pending_atomic_commit(),
                    swapchain.worker_queued_identity(),
                ))
            })?;
            log_output_pipeline_snapshot(
                perf,
                *triple_buffer_policy,
                pacing_mode,
                &pipeline,
                PipelineSchedulingDiagnostics::new(
                    *scheduled_presentation_target,
                    render_ahead_allowed,
                    worker_queue_available,
                ),
                adaptive_buffering.force_unavailable_blocker(),
                output_transactions.validate_terminal_ownership().is_ok(),
            );
            Some(pipeline)
        } else {
            None
        };
        let mut scheduler_decision = if explicit_output {
            let decision = frame_scheduler.decision_with_pipeline_diagnostics(
                ExplicitAtomicSchedulerContext {
                    now: scheduler_now,
                    predicted_total_cost: Duration::from_nanos(prediction.total_cost_ns),
                    presentation_target: *scheduled_presentation_target,
                    render_ahead_allowed,
                    worker_queue_available,
                },
                pipeline_snapshot
                    .as_ref()
                    .expect("explicit output built a pipeline snapshot"),
            );
            if let Some(wait_reason) = decision.wait_reason {
                frame_pacing.note_pipeline_wait(wait_reason);
            }
            decision.action
        } else {
            frame_scheduler
                .decision_with_render_target(scheduler_now.get(), scanout.render_target_available())
        };
        if scheduler_decision == SchedulerDecision::PageFlipWatchdogExpired {
            perf.log("native.pageflip_watchdog", || {
                vec![
                    NativePerfField::u64("frame", *frame_index),
                    NativePerfField::u64("crtc", u64::from(target.crtc_id)),
                    NativePerfField::str("scanout", scanout.kind().metric_name()),
                    NativePerfField::str("kms_backend", kms_backend.effective_kind().as_str()),
                    NativePerfField::u64(
                        "pending_token",
                        scanout.pending_page_flip_token().unwrap_or(0),
                    ),
                    NativePerfField::u64("backend_generation", *drm_file_generation),
                    NativePerfField::u64("timeout_count", frame_scheduler.watchdog_timeout_count()),
                    NativePerfField::bool("drm_ready", wakeup.reasons.drm()),
                    NativePerfField::bool("final_drain_completed", pageflip_completed),
                ]
            });
            acquire_watches.shutdown(event_loop)?;
            return Err(io::Error::other(format!(
                "native page flip watchdog expired: backend={} crtc={} frame={} pending=true; final DRM drain found no completion",
                scanout.kind().metric_name(),
                target.crtc_id,
                frame_index
            ))
            .into());
        }
        if scheduler_decision == SchedulerDecision::ReadyTargetInvalidated {
            return Err(io::Error::other(
                "explicit Atomic ready frame belongs to an invalidated presentation target",
            )
            .into());
        }
        let mut cursor_hardware_usable = planned_cursor_hardware_usable(
            runtime_plane_plan.as_ref(),
            atomic_cursor.as_ref(),
            *cursor_render_mode,
            cursor_visible,
        );
        if client_cursor_active {
            cursor_hardware_usable = client_cursor_hardware_usable;
        }
        let cursor_plane_update_usable = cursor_hardware_usable
            || atomic_cursor.as_ref().is_some_and(|cursor| {
                cursor_state_changed && cursor.current().visible && !cursor_visible
            });
        let direct_inspection = inspect_direct_presentation(DirectPresentationInputs {
            server,
            kms_kind: kms_backend.effective_kind(),
            atomic_cursor: atomic_cursor.as_ref(),
            cursor_render_mode: *cursor_render_mode,
            cursor_visible,
            client_cursor_active,
            client_cursor_hardware_usable,
            legacy_cursor_available: legacy_cursor.is_some(),
            page_flip_pending: scanout.page_flip_pending(),
            atomic_commit_pending: atomic_commit_arbiter.atomic_commit_pending(),
            drm_file_generation: *drm_file_generation,
            effective_cursor: effective_cursor.as_ref(),
            last_direct_candidate_key,
            scene_changed,
            pending_frame_work,
            primary_redraw_requested,
            direct_active: presented_planes
                .primary
                .is_some_and(|assignment| assignment.is_direct()),
            plane_decision: runtime_plane_plan.as_ref().map(|plan| &plan.decision),
        });
        let cursor_direct_compatible = direct_inspection.cursor_direct_compatible;
        let atomic_primary_commit_pending = direct_inspection.atomic_primary_commit_pending;
        let direct_candidate_changed = direct_inspection.direct_candidate_changed;
        let direct_candidate_eligible = direct_inspection.direct_candidate_eligible;
        let primary_visual_work_pending = direct_inspection.primary_visual_work_pending;
        let composition_required = direct_inspection.composition_required;
        if scanout.ready_frame_queued()
            && direct_scanout_preference.enabled()
            && cursor_direct_compatible
            && direct_candidate_eligible
            && !composition_required
            && let Some(direct_key) = direct_inspection.candidate_key
        {
            log_prepared_primary_arbitration(
                perf,
                pipeline_snapshot.as_ref(),
                output_transactions,
                direct_key,
                cursor_direct_compatible,
                composition_required,
            );
        }
        let (can_queue_worker_cursor, sidecar_opportunity) = cursor_worker_opportunities(
            worker_mode,
            kms_commit_worker.as_ref(),
            atomic_commit_arbiter,
            runtime_plane_plan
                .as_ref()
                .and_then(|plan| plan.attachable_primary),
        );
        let atomic_commit_blocks_cursor = atomic_primary_commit_pending && !can_queue_worker_cursor;
        let primary_work_for_cursor = (primary_visual_work_pending && !sidecar_opportunity)
            || direct_candidate_changed
            || atomic_commit_blocks_cursor
            || scanout.ready_frame_queued();
        let plane_delta_allowed = plane_delta_allowed_at_deadline(
            cursor_output_arbitration,
            *cursor_scheduling_policy,
            scheduler_now.get(),
            primary_work_for_cursor,
            cursor_state_changed,
            cursor_plane_update_usable,
        );
        let presentation_path = plan_native_presentation_path(NativePresentationPlanInput {
            direct_active: presented_planes
                .primary
                .is_some_and(|assignment| assignment.is_direct()),
            direct_candidate_changed,
            direct_candidate_eligible,
            primary_visual_work_pending: primary_visual_work_pending && !sidecar_opportunity,
            cursor_changed: cursor_state_changed,
            cursor_hardware_usable,
            cursor_visible,
            composition_required,
            atomic_commit_pending: atomic_commit_blocks_cursor,
            plane_delta_allowed,
            render_ahead_requested: scheduler_decision == SchedulerDecision::RenderAhead,
        });
        let plane_delta_deferred = cursor_state_changed
            && !primary_visual_work_pending
            && !plane_delta_allowed
            && !scanout.ready_frame_queued();
        if plane_delta_deferred {
            frame_scheduler.note_immediate_completion();
            scheduler_decision = SchedulerDecision::Idle;
        }
        suppress_direct_render_ahead(presentation_path, &mut scheduler_decision, scanout, perf);
        if presentation_path == NativePresentationPath::PlaneDelta
            && let Some(cursor) = atomic_cursor.as_mut()
            && (!atomic_commit_arbiter.atomic_commit_pending() || can_queue_worker_cursor)
            && !scanout.ready_frame_queued()
        {
            let desired = effective_cursor.clone();
            let cursor_target = (*scheduled_presentation_target)
                .or_else(|| {
                    presentation_deadline.reactive_target(scheduler_now, predicted_total_cost)
                })
                .ok_or_else(|| {
                    io::Error::other("cursor-only Atomic output has no presentation target")
                })?;
            let cursor_submit_window = presentation_timing
                .submit_window(
                    cursor_target.presentation_time.get(),
                    cursor_target.submit_not_before().get(),
                    prediction.kms_dispatch_budget_ns,
                )
                .map(Some)
                .unwrap_or_else(|_| {
                    presentation_timing.record_unreachable_target();
                    None
                });
            #[rustfmt::skip] let validation_base = require_validation_base!(validation_base_context, queued_redraw_requested);
            let Some(decision) = present_cursor_for_presentation(
                worker_mode,
                kms_commit_worker.as_ref(),
                kms_backend,
                cursor,
                desired,
                atomic_commit_arbiter,
                output_transactions,
                presentation_trace,
                cursor_target,
                cursor_submit_window,
                target.crtc_id,
                *drm_file_generation,
                pacing_mode,
                cursor_epoch,
                validation_base,
                last_submitted_cursor_epoch,
                cursor_output_arbitration,
                frame_scheduler,
                pacing_now_ns,
                perf,
                client_cursor_active,
                cursor_render_mode,
                &mut effective_cursor,
                queued_redraw_requested,
                last_client_cursor_damage,
                last_software_cursor_damage,
                current_client_cursor_damage,
                current_software_cursor_damage,
                runtime_plane_plan.as_ref(),
            )?
            else {
                *queued_redraw_requested = true;
                return Ok(());
            };
            scheduler_decision = decision;
        }
        let can_queue_worker_next = super::presentation_worker::can_queue_worker_primary(
            worker_mode,
            scheduler_decision,
            pipeline_snapshot.as_ref(),
            kms_commit_worker.as_ref(),
        );
        scheduler_decision = oblivion_one::native::scheduler::apply_atomic_commit_lane_guard(
            scheduler_decision,
            atomic_commit_arbiter.atomic_commit_pending(),
            can_queue_worker_next,
        );
        if matches!(
            scheduler_decision,
            SchedulerDecision::SubmitReady | SchedulerDecision::SubmitReadyLate
        ) {
            let compatibility_target = (*scheduled_presentation_target).or_else(|| {
                presentation_deadline.reactive_target(scheduler_now, predicted_total_cost)
            });
            let compatibility_submit_window = compatibility_target.and_then(|target| {
                match presentation_timing.submit_window(
                    target.presentation_time.get(),
                    target.submit_not_before().get(),
                    prediction.kms_dispatch_budget_ns,
                ) {
                    Ok(window) => Some(window),
                    Err(_) => {
                        presentation_timing.record_unreachable_target();
                        None
                    }
                }
            });
            match super::presentation_ready::submit_ready_frame(
                scheduler_decision,
                worker_mode,
                kms_commit_worker.as_ref(),
                server,
                kms_backend,
                scanout,
                target.crtc_id,
                *drm_file_generation,
                mode_label,
                *refresh_hz,
                compatibility_target,
                compatibility_submit_window,
                render_generation,
                effective_cursor.as_ref(),
                cursor_epoch,
                *cursor_render_mode,
                planned_cursor_delivery,
                atomic_cursor,
                cursor_output_arbitration,
                last_submitted_cursor_epoch,
                frame_scheduler,
                frame_pacing,
                output_render_fence_token,
                event_loop,
                atomic_commit_arbiter,
                output_transactions,
                presentation_trace,
                pacing_mode,
                *presented_planes,
                scene_history,
                frame_index,
                &mut frame_submitted,
                perf,
                #[cfg(test)]
                native_io_recorder,
            )? {
                super::presentation_ready::ReadySubmissionResult::Submitted => {}
                super::presentation_ready::ReadySubmissionResult::Unavailable => {
                    presentation_deadline.clear_scheduled_target();
                    *scheduled_presentation_target = None;
                    *queued_redraw_requested = true;
                    return Ok(());
                }
            }
        } else if matches!(
            scheduler_decision,
            SchedulerDecision::Render | SchedulerDecision::RenderAhead
        ) {
            let render_ahead = scheduler_decision == SchedulerDecision::RenderAhead;
            let mut direct_submitted = false;
            let mut direct_suppressed = false;
            if !render_ahead
                && direct_scanout_preference.enabled()
                && (!cursor_visible || *cursor_render_mode == NativeCursorRenderMode::Hardware)
                && cursor_direct_compatible
                && !atomic_commit_arbiter.atomic_commit_pending()
                && !scanout.ready_frame_queued()
                && !scanout.output_render_in_progress()
                && !scanout.direct_scanout_inhibited()
                && direct_candidate_changed
            {
                let direct_target = match pacing_mode {
                    NativeOutputPacingMode::ReactiveDouble => reactive_or_commit_timing_target(
                        presentation_deadline,
                        *scheduled_presentation_target,
                        MonotonicTimestampNs::new(monotonic_now_ns()?),
                        predicted_total_cost,
                    ),
                    NativeOutputPacingMode::PredictiveTriple => scheduled_presentation_target
                        .or_else(|| {
                            presentation_deadline.reactive_target(
                                MonotonicTimestampNs::new(monotonic_now_ns().ok()?),
                                predicted_total_cost,
                            )
                        }),
                };
                if let Some(direct_target) = direct_target
                    && let Ok(direct_submit_window) = presentation_timing.submit_window(
                        direct_target.presentation_time.get(),
                        direct_target.submit_not_before().get(),
                        prediction.kms_dispatch_budget_ns,
                    )
                    && worker_mode
                    && kms_commit_worker.is_some()
                {
                    match scanout.try_direct_scanout(
                        kms_backend,
                        server,
                        output_transactions,
                        direct_target,
                        effective_cursor.as_ref(),
                        frozen_revision(effective_cursor.as_ref(), atomic_cursor.as_ref()),
                        cursor_epoch,
                        pacing_mode,
                        confirmed_output_presentation.content_type,
                        kms_commit_worker.as_ref(),
                    )? {
                        DirectScanoutAttempt::Unchanged => {
                            direct_suppressed = note_same_buffer_suppressed(perf);
                            #[rustfmt::skip] reset_after_same_buffer(server, presentation_deadline, scheduled_presentation_target);
                        }
                        #[rustfmt::skip] DirectScanoutAttempt::TimingDeferred => { super::commit_timing::defer_after_timing(presentation_deadline, scheduled_presentation_target, queued_redraw_requested); return Ok(()); }
                        DirectScanoutAttempt::WorkerQueued {
                            transaction_id,
                            token,
                            framebuffer_id,
                            cursor_revision,
                            lease,
                            admission,
                            mut test_only,
                        } => {
                            if atomic_cursor.as_ref().is_some_and(|cursor| {
                                cursor.scheduled_test_policy() == KmsCursorTestPolicy::Required
                            }) {
                                test_only = KmsTestOnlyPolicy::Required;
                            }
                            #[rustfmt::skip] let validation_base = require_validation_base!(validation_base_context, queued_redraw_requested);
                            let direct_result = finish_direct_worker_queued(
                                kms_commit_worker
                                    .as_ref()
                                    .expect("direct scanout requires KMS worker"),
                                scanout,
                                emergency_quarantined_worker_jobs,
                                direct_fallback_tracker,
                                server,
                                output_transactions,
                                atomic_commit_arbiter,
                                presentation_trace,
                                frame_scheduler,
                                cursor_output_arbitration,
                                effective_cursor.as_ref(),
                                cursor_revision,
                                worker_ctx(
                                    atomic_cursor.as_ref(),
                                    frame_pacing,
                                    validation_base,
                                    planned_cursor_delivery,
                                    primary_cursor,
                                ),
                                *drm_file_generation,
                                target.crtc_id,
                                scene_generation,
                                cursor_epoch,
                                last_rendered_scene_generation,
                                last_submitted_cursor_epoch,
                                frame_index,
                                &mut frame_submitted,
                                transaction_id,
                                token,
                                framebuffer_id,
                                direct_target,
                                direct_submit_window,
                                *lease,
                                admission,
                                test_only,
                            )?;
                            match direct_result {
                                DirectWorkerQueueResult::Queued => {
                                    direct_submitted = true;
                                }
                                DirectWorkerQueueResult::AdmissionRejected => {
                                    let _ = DirectFallbackTracker::start(
                                        direct_fallback_tracker,
                                        transaction_id,
                                        *last_refresh_sequence,
                                        DirectFallbackReason::WorkerAdmissionRejected,
                                    );
                                }
                            }
                        }
                        DirectScanoutAttempt::AdmissionRejected {
                            transaction_id,
                            reason: _reason,
                        } => {
                            let _ = DirectFallbackTracker::start(
                                direct_fallback_tracker,
                                transaction_id,
                                *last_refresh_sequence,
                                DirectFallbackReason::WorkerAdmissionRejected,
                            );
                        }
                        DirectScanoutAttempt::Rejected(rejection) => {
                            scanout.note_direct_blocker(rejection.as_str());
                            perf.log("native.direct_scanout", || {
                                vec![
                                    NativePerfField::str("transition", "fallback"),
                                    NativePerfField::str("rejection", rejection.as_str()),
                                ]
                            });
                        }
                        DirectScanoutAttempt::Fallback(reason) => {
                            scanout.note_direct_blocker(reason);
                            perf.log("native.direct_scanout", || {
                                vec![
                                    NativePerfField::str("transition", "fallback"),
                                    NativePerfField::str("reason", reason),
                                ]
                            });
                        }
                    }
                }
            }
            if direct_submitted {
                frame_pacing.log(
                    "render_complete",
                    vec![
                        frame_id_field(frame_pacing.active),
                        PacingField::u64("render_generation", render_generation),
                        PacingField::u64("gpu_draw_us", 0),
                        PacingField::bool("direct_scanout", true),
                    ],
                );
            } else if direct_suppressed {
                frame_scheduler.note_immediate_completion();
                frame_completed = true;
                *last_rendered_scene_generation = scene_generation;
                *queued_redraw_requested = false;
            } else {
                frame_pacing.note_render_started(pacing_mode, render_ahead);
                let render_observed_at_ns = monotonic_now_ns()?;
                let mut render_begin_fields = vec![
                    frame_id_field(frame_pacing.active),
                    PacingField::u64("render_generation", render_generation),
                    PacingField::u64("render_observed_at_ns", render_observed_at_ns),
                    PacingField::bool("render_ahead", render_ahead),
                    PacingField::str("buffering_mode", adaptive_buffering.mode().as_str()),
                    PacingField::u64("prediction_ewma_ns", prediction.ewma_render_ns),
                    PacingField::u64(
                        "prediction_upper_deviation_ns",
                        prediction.upper_render_deviation_ns,
                    ),
                    PacingField::u64("prediction_p90_ns", prediction.p90_recent_render_ns),
                    PacingField::u64("prediction_render_risk_ns", prediction.render_risk_ns),
                    PacingField::u64(
                        "prediction_worker_queue_residency_ns",
                        prediction.p95_worker_queue_residency_ns,
                    ),
                    PacingField::u64(
                        "prediction_worker_submit_wake_ns",
                        prediction.p95_wake_lateness_ns,
                    ),
                    PacingField::u64(
                        "prediction_worker_pre_submit_ns",
                        prediction.p95_worker_pre_submit_ns,
                    ),
                    PacingField::u64(
                        "prediction_worker_dispatch_ns",
                        prediction.p95_worker_dispatch_ns,
                    ),
                    PacingField::u64("prediction_atomic_ioctl_ns", prediction.p95_atomic_ioctl_ns),
                    PacingField::u64(
                        "prediction_kms_dispatch_budget_ns",
                        prediction.kms_dispatch_budget_ns,
                    ),
                    PacingField::u64(
                        "prediction_kms_apply_guard_ns",
                        prediction.kms_apply_guard_ns,
                    ),
                    PacingField::u64("prediction_kms_total_lead_ns", prediction.kms_total_lead_ns),
                    PacingField::u64(
                        "main_event_loop_wake_guard_ns",
                        prediction.main_event_loop_wake_guard_ns,
                    ),
                    PacingField::u64("predicted_total_cost_ns", prediction.total_cost_ns),
                    PacingField::u64("refresh_interval_ns", refresh_interval.as_nanos() as u64),
                    PacingField::bool("idle_wake_guard", prediction.idle_wake_guard),
                ];
                render_begin_fields.extend(snapshot_fields(scanout.buffer_snapshot()));
                frame_pacing.log("render_begin", render_begin_fields);
                let effective_redraw_requested = redraw_requested || *queued_redraw_requested;
                let render_cause = native_repaint_cause_label(
                    server.render_generation_cause(),
                    scene_changed,
                    accepted,
                    pending_frame_work,
                    effective_redraw_requested,
                );
                let atomic_backend = matches!(&**scanout, NativeScanoutBackend::AtomicEglGbm(_));
                if !atomic_backend {
                    server.capture_frame_callbacks_for_render();
                }
                let (resolved_scene, output_damage) = resolve_scene_and_damage(
                    presented_planes
                        .primary
                        .is_some_and(|assignment| assignment.is_direct()),
                    target.width,
                    target.height,
                    scene_history,
                    &*server,
                    (current_client_cursor_damage, current_software_cursor_damage),
                );
                let no_primary_work = output_damage.is_empty() && !effective_redraw_requested;
                if no_primary_work {
                    drop(resolved_scene);
                    perf.log("native.frame_skip", || {
                        let mut fields = output_damage.fields().to_vec();
                        fields.extend([
                            NativePerfField::str("reason", "no_logical_damage"),
                            NativePerfField::usize(
                                "skipped_input_repaints",
                                skipped_input_repaints,
                            ),
                            NativePerfField::u64("tick_us", tick_us),
                            NativePerfField::bool(
                                "pageflip_pending_at_tick",
                                pageflip_pending_at_tick,
                            ),
                            NativePerfField::u64("input_drain_us", input_drain_us),
                            NativePerfField::usize("raw_input_events", raw_input_events),
                            NativePerfField::usize(
                                "coalesced_input_events",
                                coalesced_input_events,
                            ),
                            NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
                            NativePerfField::bool("pageflip_completed", pageflip_completed),
                            NativePerfField::u64("present_us", present_us),
                            NativePerfField::str(
                                "kms_backend",
                                kms_backend.effective_kind().as_str(),
                            ),
                            NativePerfField::u64(
                                "pageflip_token",
                                scanout.pending_page_flip_token().unwrap_or(0),
                            ),
                            NativePerfField::u64("backend_generation", *drm_file_generation),
                            NativePerfField::u64("render_generation", render_generation),
                            NativePerfField::str("render_cause", render_cause),
                            NativePerfField::bool("pending_frame_work", pending_frame_work),
                        ]);
                        fields
                    });
                    if pending_frame_work {
                        let finish_frame_start = Instant::now();
                        server.finish_frame();
                        perf.log("native.finish_frame", || {
                            vec![
                                NativePerfField::str("reason", "empty_visible_damage"),
                                NativePerfField::u64(
                                    "elapsed_us",
                                    elapsed_micros(finish_frame_start),
                                ),
                                NativePerfField::usize(
                                    "surfaces",
                                    server.renderable_surfaces().len(),
                                ),
                                NativePerfField::u64(
                                    "render_generation",
                                    server.render_generation(),
                                ),
                            ]
                        });
                    }
                    frame_scheduler.note_immediate_completion();
                    *queued_redraw_requested = false;
                    *last_software_cursor_damage = current_software_cursor_damage;
                } else {
                    #[rustfmt::skip] let atomic_kms_lane_free = !atomic_commit_arbiter.atomic_commit_pending() && !scanout.ready_frame_queued();
                    if let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut **scanout {
                        let expected_scene_signature = resolved_scene.scene_identity_signature();
                        drop(resolved_scene);
                        let frame_target = match pacing_mode {
                            NativeOutputPacingMode::ReactiveDouble => {
                                take_reactive_or_commit_timing_target(
                                    presentation_deadline,
                                    scheduled_presentation_target,
                                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                                    predicted_total_cost,
                                )
                            }
                            NativeOutputPacingMode::PredictiveTriple => {
                                scheduled_presentation_target.take().or_else(|| {
                                    presentation_deadline.reactive_target(
                                        MonotonicTimestampNs::new(monotonic_now_ns().ok()?),
                                        predicted_total_cost,
                                    )
                                })
                            }
                        }
                        .ok_or_else(|| {
                            io::Error::other(
                                "explicit Atomic render started without a presentation target",
                            )
                        })?;
                        let submit_window = match presentation_timing.submit_window(
                            frame_target.presentation_time.get(),
                            frame_target.submit_not_before().get(),
                            prediction.kms_dispatch_budget_ns,
                        ) {
                            Ok(window) => window,
                            Err(_) => {
                                presentation_timing.record_unreachable_target();
                                presentation_deadline.clear_scheduled_target();
                                *queued_redraw_requested = true;
                                return Ok(());
                            }
                        };
                        presentation_deadline.clear_scheduled_target();
                        let (cursor_assignment, frozen_cursor_plane_owner) =
                            freeze_cursor_assignment_for_render(
                                effective_cursor.as_ref(),
                                cursor_epoch,
                                atomic_cursor.as_ref(),
                            )?;
                        #[rustfmt::skip] let render_outcome = explicit.render_frame(
                            frame_renderer,
                            server,
                            output_transactions,
                            input_state,
                            *cursor_render_mode,
                            &output_damage,
                            expected_scene_signature,
                            render_generation,
                            *drm_file_generation,
                            frame_target,
                            submit_window,
                            pacing_mode,
                            cursor_assignment,
                            direct_inspection
                                .candidate_key
                                .filter(|_| !composition_required),
                            frozen_cursor_plan_for_render(
                                planned_cursor_delivery,
                                primary_cursor,
                                runtime_plane_plan.as_ref(),
                            ),
                            frozen_cursor_plane_owner, AtomicAsyncPolicyInputs::new(cursor_state_changed, atomic_kms_lane_free, confirmed_output_presentation.content_type),
                        )?;
                        match render_outcome {
                            AtomicFrameRenderOutcome::Skipped { reason, render_us } => {
                                render_telemetry.record_skipped(render_us);
                                frame_scheduler.note_immediate_completion();
                                presentation_deadline.clear_scheduled_target();
                                *scheduled_presentation_target = None;
                                *queued_redraw_requested = false;
                                perf.log("native.atomic_render_skipped", || {
                                    vec![
                                        NativePerfField::str("reason", format!("{reason:?}")),
                                        NativePerfField::u64("render_us", render_us),
                                        NativePerfField::u64("scene_generation", scene_generation),
                                        NativePerfField::u64("cursor_epoch", cursor_epoch),
                                        NativePerfField::usize("accepted_clients", accepted),
                                        NativePerfField::bool(
                                            "pending_frame_work",
                                            pending_frame_work,
                                        ),
                                        NativePerfField::str("output_damage", "empty"),
                                    ]
                                });
                            }
                            #[rustfmt::skip]
                            AtomicFrameRenderOutcome::Rendered { frame_id, transaction_id, render_us, repaint_stats, resolved_snapshot, resolved_scene_signature, render_damage_signature, repair_damage_signature, resolved_render_generation, framebuffer_slot } => {
                                record_atomic_rendered_scene(
                                    scene_history,
                                    presentation_trace,
                                    frame_id,
                                    transaction_id,
                                    resolved_render_generation,
                                    resolved_snapshot,
                                    resolved_scene_signature,
                                    render_damage_signature,
                                    repair_damage_signature,
                                    scene_history.presented_frame_id(),
                                    framebuffer_slot,
                                    (current_client_cursor_damage, current_software_cursor_damage),
                                    repaint_stats.buffer_age,
                                )?;
                                render_telemetry.record_atomic(render_us, repaint_stats, *target);
                                frame_scheduler.consume_visual_work();
                                frame_rendered = true;
                                let trace_timestamp_ns = monotonic_now_ns()?;
                                presentation_trace.push(
                                    PresentationTransactionEvent::TransactionBuilt {
                                        transaction_id,
                                        timestamp_ns: trace_timestamp_ns,
                                    },
                                );
                                presentation_trace.push(
                                    PresentationTransactionEvent::AcquireReady {
                                        transaction_id,
                                        timestamp_ns: trace_timestamp_ns,
                                    },
                                );
                                let ready_at_ns = monotonic_now_ns()?;
                                let waits_for_target = rendered_primary_must_wait_for_lane(
                                    render_ahead,
                                    atomic_commit_arbiter.atomic_commit_pending(),
                                    can_queue_worker_next,
                                );
                                if waits_for_target {
                                    frame_pacing.note_ready_frame(ready_at_ns, render_ahead);
                                } else {
                                    #[rustfmt::skip] let async_render_fence_ready = super::presentation_ready::ensure_async_render_fence_ready(explicit, output_transactions, transaction_id, output_render_fence_token, event_loop)?;
                                    if !async_render_fence_ready {
                                        *queued_redraw_requested = true;
                                        return Ok(());
                                    }
                                    #[rustfmt::skip] let validation_base = require_validation_base!(validation_base_context, queued_redraw_requested);
                                    let Some((
                                        token,
                                        framebuffer_id,
                                        transaction_id,
                                        worker_queued,
                                    )) = submit_explicit_ready_for_presentation(
                                        worker_mode,
                                        kms_commit_worker.as_ref(),
                                        explicit,
                                        kms_backend,
                                        server,
                                        output_transactions,
                                        atomic_commit_arbiter,
                                        presentation_trace,
                                        transaction_id,
                                        *drm_file_generation,
                                        target.crtc_id,
                                        worker_ctx(
                                            atomic_cursor.as_ref(),
                                            frame_pacing,
                                            validation_base,
                                            planned_cursor_delivery,
                                            primary_cursor,
                                        ),
                                        false,
                                    )?
                                    else {
                                        *queued_redraw_requested = true;
                                        return Ok(());
                                    };
                                    // This token is the presentation identity
                                    // for the rendered scene. Queue the exact
                                    // ready snapshot only after Atomic accepts
                                    // the submission, so pageflip-time damage
                                    // can resolve the actual predecessor.
                                    scene_history.queue_submission_or_error(token)?;
                            let _atomic_primary_registered = if worker_queued {
                                        true
                                    } else {
                                        register_primary_transaction(
                                            atomic_commit_arbiter,
                                            server,
                                            kms_backend.effective_kind(),
                                            token,
                                            *drm_file_generation,
                                            target.crtc_id,
                                            Some(transaction_id),
                                            *frame_index,
                                            framebuffer_id,
                                            monotonic_now_ns()?,
                                            output_transactions,
                                            presentation_trace,
                                        )?
                                    };
                                    if !worker_queued
                                        && let Some(cursor) = atomic_cursor.as_mut()
                                        && cursor.needs_submission_for(effective_cursor.as_ref())
                                        && let Some(cursor_token) = PageFlipToken::new(token)
                                    {
                                        let state = effective_cursor.clone().unwrap_or_else(|| {
                                            let mut hidden = cursor.desired().clone();
                                            hidden.visible = false;
                                            hidden.framebuffer_id = None;
                                            hidden
                                        });
                                        cursor.begin_primary_submission(cursor_token, state);
                                    }
                                    if !worker_queued {
                                        frame_pacing.note_submit(
                                            token,
                                            monotonic_now_ns()?,
                                            false,
                                            pacing_mode,
                                        );
                                    }
                                    if !worker_queued
                                        && output_render_fence_token.is_none()
                                        && let Some(fd) = explicit.pending_timing_fd()
                                    {
                                        *output_render_fence_token =
                                            Some(event_loop.register(
                                                fd,
                                                NativeEventSource::OutputRenderFence,
                                            )?);
                                    }
                                    frame_submitted = true;
                                    *frame_index = frame_index.saturating_add(1);
                                }
                                frame_pacing.log(
                                    "render_complete",
                                    vec![
                                        PacingField::u64("frame_id", frame_id),
                                        PacingField::u64("render_generation", render_generation),
                                        PacingField::u64(
                                            "render_observed_at_ns",
                                            render_observed_at_ns,
                                        ),
                                        PacingField::u64("render_end_ns", ready_at_ns),
                                        PacingField::u64(
                                            "target_vblank_sequence",
                                            frame_target.sequence,
                                        ),
                                        PacingField::u64(
                                            "target_presentation_ns",
                                            frame_target.presentation_time.get(),
                                        ),
                                        PacingField::bool("render_ahead", render_ahead),
                                    ],
                                );
                                *queued_redraw_requested = false;
                                *last_rendered_scene_generation = scene_generation;
                                if !waits_for_target && !worker_mode {
                                    *last_submitted_cursor_epoch = cursor_epoch;
                                    cursor_output_arbitration.consume(cursor_epoch);
                                }
                            }
                        }
                    } else {
                        let cpu_before = perf
                            .enabled()
                            .then(NativeProcessCpuSample::read_current)
                            .flatten();
                        let paint_outcome = match scanout.paint_server_frame(
                            frame_renderer,
                            &resolved_scene,
                            server,
                            input_state,
                            *cursor_render_mode,
                            &output_damage,
                        ) {
                            Ok(outcome) => outcome,
                            Err(error) => {
                                server.restore_prepared_frame_batch_after_render_failure();
                                return Err(Box::new(error));
                            }
                        };
                        let paint_stats = paint_outcome.stats();
                        render_telemetry.record_native_paint(paint_stats);
                        frame_pacing.log(
                            "render_complete",
                            vec![
                                frame_id_field(frame_pacing.active),
                                PacingField::u64("render_generation", render_generation),
                                PacingField::u64("render_observed_at_ns", render_observed_at_ns),
                                PacingField::u64("render_end_ns", monotonic_now_ns()?),
                                PacingField::u64("gpu_draw_us", paint_stats.gpu_draw_us),
                                PacingField::u64("egl_swap_us", paint_stats.egl_swap_us),
                                PacingField::u64("render_total_us", paint_stats.total_us),
                            ],
                        );
                        if matches!(paint_outcome, NativePaintOutcome::Skipped(_)) {
                            frame_scheduler.note_immediate_completion();
                            server.finish_prepared_frame();
                            frame_completed = true;
                            perf.log("native.frame_skip", || {
                                let mut fields = paint_stats.fields();
                                fields.extend(output_damage.fields());
                                fields.extend([
                                    NativePerfField::str("reason", "renderer_no_logical_damage"),
                                    NativePerfField::bool("egl_swap_attempted", false),
                                    NativePerfField::bool("gbm_front_buffer_locked", false),
                                    NativePerfField::bool("ready_frame_created", false),
                                    NativePerfField::u64("render_generation", render_generation),
                                ]);
                                fields
                            });
                            *queued_redraw_requested = false;
                            *last_software_cursor_damage = current_software_cursor_damage;
                            *last_client_cursor_damage = current_client_cursor_damage;
                            *last_software_cursor_damage = current_software_cursor_damage;
                        } else {
                            frame_rendered = true;
                            #[rustfmt::skip] let resolved_scene_signature = replace_ready_scene_and_signature(scene_history, &resolved_scene, *frame_index, (current_client_cursor_damage, current_software_cursor_damage));
                            drop(resolved_scene);
                            server.complete_rendered_frame_callbacks_for_prepared();
                            let mut ready_fields = vec![
                                frame_id_field(frame_pacing.active),
                                PacingField::u64("render_generation", render_generation),
                            ];
                            ready_fields.extend(snapshot_fields(scanout.buffer_snapshot()));
                            frame_pacing.log("ready_queued", ready_fields);
                            let cpu_after = perf
                                .enabled()
                                .then(NativeProcessCpuSample::read_current)
                                .flatten();
                            let (cpu_user_us, cpu_system_us) = cpu_before
                                .zip(cpu_after)
                                .map(|(before, after)| after.delta_us_since(before))
                                .unwrap_or((0, 0));
                            let repaint_present_start = Instant::now();
                            #[rustfmt::skip] let (present_result, compatibility_transaction_id) = if render_ahead { (NativePresentResult::Noop, None) } else { present_composited_compatibility_frame(scanout, server, output_transactions, *drm_file_generation, target.crtc_id, presentation_deadline, *scheduled_presentation_target, scheduler_now, predicted_total_cost, pacing_mode, render_generation, effective_cursor.as_ref(), cursor_epoch, *frame_index, kms_backend, scene_history)? };
                            #[cfg(test)]
                            if !render_ahead {
                                native_io_recorder.record(NativeIoOperation::ScanoutPresent);
                            }
                            let repaint_present_us = elapsed_micros(repaint_present_start);
                            let acquire_ready_to_render_submit_us = last_acquire_ready_at_ns
                                .map(|ready_at| {
                                    monotonic_now_ns()
                                        .map(|now| now.saturating_sub(ready_at) / 1_000)
                                })
                                .transpose()?
                                .unwrap_or(0);
                            match present_result {
                                NativePresentResult::AsyncSubmitted {
                                    token,
                                    framebuffer_id,
                                    transaction_id,
                                } => {
                                    scene_history.queue_submission_or_error(token)?;
                                    #[rustfmt::skip] record_compatibility_scene_identity(presentation_trace, scanout, transaction_id, *frame_index, render_generation, resolved_scene_signature)?;
                                    server.mark_prepared_frame_submitted();
                                    #[cfg(test)]
                                    native_io_recorder.record(NativeIoOperation::PageflipSubmit);
                                    #[cfg(test)]
                                    native_io_recorder.record(match kms_backend.effective_kind() {
                                        oblivion_one::native::kms::KmsBackendKind::Atomic => {
                                            NativeIoOperation::AtomicCommit
                                        }
                                        oblivion_one::native::kms::KmsBackendKind::Legacy => {
                                            NativeIoOperation::LegacyCommit
                                        }
                                    });
                                    let atomic_primary_registered = register_primary_transaction(
                                        atomic_commit_arbiter,
                                        server,
                                        kms_backend.effective_kind(),
                                        token,
                                        *drm_file_generation,
                                        target.crtc_id,
                                        transaction_id,
                                        *frame_index,
                                        framebuffer_id,
                                        monotonic_now_ns()?,
                                        output_transactions,
                                        presentation_trace,
                                    )?;
                                    if let Some(cursor) = atomic_cursor.as_mut()
                                        && atomic_primary_registered
                                        && cursor.needs_submission_for(effective_cursor.as_ref())
                                        && let Some(cursor_token) = PageFlipToken::new(token)
                                    {
                                        let state = effective_cursor.clone().unwrap_or_else(|| {
                                            let mut hidden = cursor.desired().clone();
                                            hidden.visible = false;
                                            hidden.framebuffer_id = None;
                                            hidden
                                        });
                                        cursor.begin_primary_submission(cursor_token, state);
                                    }
                                    frame_scheduler
                                        .note_async_submission(token, monotonic_now_ns()?)
                                        .map_err(io::Error::other)?;
                                    if atomic_primary_registered {
                                        frame_scheduler
                                            .defer_page_flip_watchdog_to_atomic_arbiter();
                                    }
                                    frame_pacing.note_submit(
                                        token,
                                        monotonic_now_ns()?,
                                        false,
                                        pacing_mode,
                                    );
                                    frame_submitted = true;
                                }
                                NativePresentResult::Immediate => {
                                    scene_history.promote_immediate_or_error()?;
                                    let transaction_id = record_immediate_scene_identity(
                                        presentation_trace,
                                        compatibility_transaction_id,
                                        *frame_index,
                                        render_generation,
                                        resolved_scene_signature,
                                    )?;
                                    frame_scheduler.note_immediate_completion();
                                    let finish_frame_start = Instant::now();
                                    complete_immediate_output_transaction(
                                        output_transactions,
                                        presentation_trace,
                                        server,
                                        transaction_id,
                                        MonotonicTimestampNs::new(monotonic_now_ns()?),
                                    )?;
                                    frame_completed = true;
                                    perf.log("native.finish_frame", || {
                                        vec![
                                            NativePerfField::str("reason", "immediate_scanout"),
                                            NativePerfField::u64(
                                                "elapsed_us",
                                                elapsed_micros(finish_frame_start),
                                            ),
                                            NativePerfField::usize(
                                                "surfaces",
                                                server.renderable_surfaces().len(),
                                            ),
                                            NativePerfField::u64(
                                                "render_generation",
                                                server.render_generation(),
                                            ),
                                        ]
                                    });
                                }
                                NativePresentResult::Noop => {
                                    debug_assert!(compatibility_transaction_id.is_none());
                                    if render_ahead {
                                        frame_scheduler.note_render_ahead_ready();
                                        frame_pacing.note_render_ahead_ready(monotonic_now_ns()?);
                                    } else {
                                        return Err(io::Error::other(
                                    "native scanout rendered a frame but did not submit or complete it",
                                )
                                .into());
                                    }
                                }
                            }
                            if !render_ahead {
                                server.mark_render_damage_presented();
                                *last_client_cursor_damage = current_client_cursor_damage;
                                *last_software_cursor_damage = current_software_cursor_damage;
                            }
                            *last_acquire_ready_at_ns = None;
                            if !render_ahead {
                                *frame_index = frame_index.saturating_add(1);
                            }
                            perf.log("native.frame", || {
                                let mut fields = paint_stats.fields();
                                fields.extend(output_damage.fields());
                                fields.extend([
                                    NativePerfField::u64("index", *frame_index),
                                    NativePerfField::str(
                                        "phase",
                                        if render_ahead {
                                            "render-ahead"
                                        } else {
                                            "repaint"
                                        },
                                    ),
                                    NativePerfField::str("mode", mode_label.clone()),
                                    NativePerfField::str("cursor", cursor_render_mode.as_str()),
                                    NativePerfField::u64("refresh_hz", u64::from(*refresh_hz)),
                                    NativePerfField::usize(
                                        "surfaces",
                                        server.renderable_surfaces().len(),
                                    ),
                                    NativePerfField::u64("render_generation", render_generation),
                                    NativePerfField::bool("render_changed", scene_changed),
                                    NativePerfField::str("render_cause", render_cause),
                                    NativePerfField::u64("tick_us", tick_us),
                                    NativePerfField::bool(
                                        "pageflip_pending_at_tick",
                                        pageflip_pending_at_tick,
                                    ),
                                    NativePerfField::u64("input_drain_us", input_drain_us),
                                    NativePerfField::usize("raw_input_events", raw_input_events),
                                    NativePerfField::usize(
                                        "coalesced_input_events",
                                        coalesced_input_events,
                                    ),
                                    NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
                                    NativePerfField::bool("pageflip_completed", pageflip_completed),
                                    NativePerfField::u64("present_us", present_us),
                                    NativePerfField::u64("repaint_present_us", repaint_present_us),
                                    NativePerfField::bool("render_ahead", render_ahead),
                                    NativePerfField::bool(
                                        "render_ahead_ready",
                                        scanout.ready_frame_queued(),
                                    ),
                                    NativePerfField::u64(
                                        "acquire_ready_to_render_submit_us",
                                        acquire_ready_to_render_submit_us,
                                    ),
                                    NativePerfField::u64("cpu_user_us", cpu_user_us),
                                    NativePerfField::u64("cpu_system_us", cpu_system_us),
                                    NativePerfField::bool("pending_frame_work", pending_frame_work),
                                    NativePerfField::bool("redraw_requested", redraw_requested),
                                    NativePerfField::usize(
                                        "skipped_input_repaints",
                                        skipped_input_repaints,
                                    ),
                                    NativePerfField::usize("accepted_clients", accepted),
                                ]);
                                fields
                            });
                            *queued_redraw_requested = false;
                            *last_rendered_scene_generation = scene_generation;
                            *last_submitted_cursor_epoch = cursor_epoch;
                            cursor_output_arbitration.consume(cursor_epoch);
                        }
                    }
                }
            }
        } else if scheduler_decision == SchedulerDecision::CompleteProtocolOnly {
            complete_protocol_only_tick(
                server,
                frame_scheduler,
                perf,
                ProtocolCycleMetrics::from_cycle(
                    cycle,
                    render_generation,
                    effective_render_target_available,
                    scene_changed,
                    pending_frame_work,
                    redraw_requested,
                ),
            );
            frame_completed = true;
        } else if matches!(
            scheduler_decision,
            SchedulerDecision::WaitForPageFlip | SchedulerDecision::WaitForBuffer
        ) {
            log_wait_for_presentation(
                frame_pacing,
                scanout,
                perf,
                scheduler_decision,
                ProtocolCycleMetrics::from_cycle(
                    cycle,
                    render_generation,
                    effective_render_target_available,
                    scene_changed,
                    pending_frame_work,
                    redraw_requested,
                ),
            )?;
        } else if skipped_input_repaints > 0 {
            log_no_visual_work(
                perf,
                ProtocolCycleMetrics::from_cycle(
                    cycle,
                    render_generation,
                    effective_render_target_available,
                    scene_changed,
                    pending_frame_work,
                    redraw_requested,
                ),
            );
        }
        cycle.record_presentation_result(frame_completed, frame_rendered, frame_submitted);
        self.update_cycle_metrics(cycle, scheduler_decision)?;
        Ok(())
    }
}
#[cfg(test)]
mod pacing_mode_tests;
