use super::cursor_cycle::{
    NativeResolvedCursorSource, defer_cursor_after_busy, resolve_native_cursor_source,
};
use super::frame::{
    NativeRepaintInputs, cursor_only_allowed_at_deadline, native_repaint_decision,
    update_cursor_output_arbitration,
};
use super::planner::{
    NativePresentationPath, NativePresentationPlanInput, plan_native_presentation_path,
    plan_scheduled_target_for_mode,
};
use super::presentation_direct::{
    DirectPresentationInputs, inspect_direct_presentation, suppress_direct_render_ahead,
};
use super::presentation_protocol::{
    ProtocolCycleMetrics, complete_protocol_only_tick, log_no_visual_work,
    log_wait_for_presentation,
};
use super::*;
use oblivion_one::native::kms::{AtomicKmsErrorKind, KmsBackendKind};
impl NativeRuntime {
    #[allow(unused_variables)]
    pub(super) fn render_present_and_update_metrics(
        &mut self,
        cycle: &mut NativeCycleState,
    ) -> NativeResult<()> {
        let perf = self.perf;
        let Self {
            server,
            cursor_image,
            perf: _,
            kms,
            kms_backend,
            target,
            mode_label,
            refresh_hz,
            drm_file_generation,
            drm_timestamp_clock,
            presentation_clock,
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
            input_devices,
            acquire_notifier,
            acquire_watches,
            parked_acquire_watches: _,
            event_loop,
            drm_reactor_token: _,
            output_render_fence_token,
            frame_scheduler,
            atomic_commit_arbiter,
            presentation_deadline,
            scheduled_presentation_target,
            render_journal,
            adaptive_buffering,
            triple_buffer_policy,
            pending_proven_deadline_miss,
            effective_app_gpu_policy,
            last_rendered_scene_generation,
            last_direct_candidate_key,
            last_submitted_cursor_epoch,
            last_renderable_surfaces,
            last_client_cursor_damage,
            last_software_cursor_damage,
            last_client_cursor_path,
            queued_redraw_requested,
            frame_index,
            known_toplevels,
            pending_launches,
            mismatched_pageflip_events,
            stale_pageflip_events,
            presentation_cadence,
            frame_pacing,
            presentation_trace,
            last_acquire_ready_at_ns,
            resize_perf,
            pointer_constraint_backend,
            seat_session: _,
            process_supervisor: _,
            shutdown: _,
            session: _,
            #[cfg(test)]
            native_io_recorder,
            ..
        } = self;
        let wakeup = &cycle.wakeup;
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
        let render_generation = server.render_generation();
        let scene_generation = server.scene_render_generation();
        let scene_changed = scene_generation != *last_rendered_scene_generation;
        let pending_frame_work = server.has_unowned_frame_work();
        let pacing_now_ns = monotonic_now_ns()?;
        let theme_cursor_visible = input_state.cursor_visible();
        let client_cursor = server.client_cursor_render_state();
        let client_cursor_active = client_cursor.is_some();
        let resolved_cursor_source = resolve_native_cursor_source(
            client_cursor_active,
            server.interaction_cursor_override_active(),
            theme_cursor_visible,
        );
        let cursor_visible = !matches!(resolved_cursor_source, NativeResolvedCursorSource::Hidden);
        if client_cursor_active && let Some(cursor) = legacy_cursor.as_mut() {
            cursor.disable()?;
        }
        let mut client_cursor_hardware_usable = false;
        if let Some(cursor) = atomic_cursor.as_mut() {
            if let Some(client) = client_cursor {
                let source_key = NativeCursorImageKey::for_surface(
                    client.surface,
                    client.hotspot_x,
                    client.hotspot_y,
                );
                let image_ready = if cursor.client_image_matches(source_key) {
                    true
                } else if cursor.client_image_failure_matches(source_key) {
                    false
                } else if let Some(image) =
                    client_cursor_image(client.surface, client.hotspot_x, client.hotspot_y)
                {
                    match cursor.replace_image(kms.file(), image, source_key) {
                        Ok(()) => true,
                        Err(_) => {
                            cursor.note_client_image_failure(source_key);
                            false
                        }
                    }
                } else {
                    cursor.note_client_image_failure(source_key);
                    false
                };
                if image_ready
                    && !cursor.failure_latched()
                    && *cursor_scheduling_policy != NativeCursorSchedulingPolicy::Software
                    && *cursor_render_mode != NativeCursorRenderMode::Software
                {
                    let x = client
                        .logical_x
                        .saturating_add(client.surface.x)
                        .saturating_add(client.hotspot_x);
                    let y = client
                        .logical_y
                        .saturating_add(client.surface.y)
                        .saturating_add(client.hotspot_y);
                    cursor.set_position(x, y);
                    cursor.set_visible(cursor_visible);
                    *cursor_render_mode = NativeCursorRenderMode::Hardware;
                    client_cursor_hardware_usable = true;
                } else {
                    cursor.set_visible(false);
                    *cursor_render_mode = NativeCursorRenderMode::SoftwareClient;
                    *last_client_cursor_damage = None;
                    cursor.note_software_fallback();
                }
            } else {
                if !cursor.using_theme_image()
                    && let Err(error) = cursor.restore_theme_image(kms.file())
                {
                    cursor.set_visible(false);
                    *cursor_render_mode = NativeCursorRenderMode::Software;
                    perf.log("native.cursor", || {
                        vec![
                            NativePerfField::str("event", "theme_restore_failed"),
                            NativePerfField::str("error", error.to_string()),
                        ]
                    });
                }
                if *cursor_preference == NativeCursorPreference::Software
                    || *cursor_scheduling_policy == NativeCursorSchedulingPolicy::Software
                    || cursor.failure_latched()
                    || !cursor.using_theme_image()
                {
                    *cursor_render_mode = NativeCursorRenderMode::Software;
                } else {
                    *cursor_render_mode = NativeCursorRenderMode::Hardware;
                }
                let (x, y) = input_state.cursor_position();
                cursor.set_position(x, y);
                cursor.set_visible(cursor_visible);
            }
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
                    && !cursor.failure_latched(),
            );
        }
        let current_client_cursor_damage = client_cursor.map(|cursor| {
            NativeClientCursorDamageState::from_cursor(target.width, target.height, cursor)
        });
        let current_software_cursor_damage = (*cursor_render_mode
            == NativeCursorRenderMode::Software
            && cursor_visible
            && !client_cursor_active)
            .then(|| {
                native_theme_cursor_rect(
                    target.width,
                    target.height,
                    input_state.cursor_position(),
                    cursor_image,
                )
            })
            .flatten();
        let client_cursor_software_work = !client_cursor_hardware_usable
            && (*last_client_cursor_damage != current_client_cursor_damage)
            && (client_cursor_active || last_client_cursor_damage.is_some());
        let mut effective_cursor = atomic_cursor
            .as_ref()
            .and_then(|cursor| {
                effective_atomic_cursor_state(cursor, *cursor_render_mode, cursor_visible)
                    .kms_state()
            })
            .cloned();
        let cursor_state_changed = atomic_cursor
            .as_ref()
            .is_some_and(|cursor| cursor.needs_submission_for(effective_cursor.as_ref()));
        let cursor_epoch = atomic_cursor.as_ref().map_or(
            *last_submitted_cursor_epoch,
            NativeAtomicCursor::desired_epoch,
        );
        let hardware_cursor_work_pending = cursor_state_changed
            && *cursor_render_mode == NativeCursorRenderMode::Hardware
            && atomic_cursor
                .as_ref()
                .is_some_and(|cursor| !cursor.failure_latched());
        let resolved_client_cursor_path =
            resolve_client_cursor_path(client_cursor_active, client_cursor_hardware_usable);
        if *last_client_cursor_path != Some(resolved_client_cursor_path) {
            *last_client_cursor_path = Some(resolved_client_cursor_path);
            log_client_cursor_path(
                perf,
                resolved_client_cursor_path,
                client_cursor_hardware_usable,
                scanout.direct_scanout_active(),
                client_cursor,
            );
        }
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
        let refresh_interval =
            Duration::from_nanos(1_000_000_000 / u64::from((*refresh_hz).max(1)));
        let prediction = render_journal.prediction_at(scheduler_now, refresh_interval);
        let explicit_output = matches!(&**scanout, NativeScanoutBackend::AtomicEglGbm(_));
        let render_ahead_allowed = match triple_buffer_policy {
            AdaptiveTripleBufferPolicy::Off => false,
            AdaptiveTripleBufferPolicy::Force => true,
            AdaptiveTripleBufferPolicy::Auto => {
                adaptive_buffering.mode() == AdaptiveBufferingMode::Triple
            }
        };
        let pacing_mode = adaptive_buffering.pacing_mode();
        if pacing_mode == NativeOutputPacingMode::ReactiveDouble {
            *scheduled_presentation_target = None;
            presentation_deadline.clear_scheduled_target();
        }
        if explicit_output
            && frame_scheduler.visual_work_queued()
            && scheduled_presentation_target.is_none()
        {
            let pending_target = match &**scanout {
                NativeScanoutBackend::AtomicEglGbm(explicit) => {
                    explicit.swapchain()?.pending_target()
                }
                _ => None,
            };
            let reason = if *triple_buffer_policy == AdaptiveTripleBufferPolicy::Force {
                PresentationTargetReason::ForcedValidation
            } else {
                PresentationTargetReason::PredictedPressure
            };
            *scheduled_presentation_target = plan_scheduled_target_for_mode(
                presentation_deadline,
                pacing_mode,
                pending_target,
                scheduler_now,
                Duration::from_nanos(prediction.total_cost_ns),
                reason,
            );
        }
        let effective_render_target_available = if explicit_output {
            scanout.render_target_available_for(pacing_mode)
        } else {
            scanout.render_target_available()
        };
        let mut scheduler_decision = if explicit_output {
            let in_fence = kms_backend
                .atomic()
                .is_some_and(|atomic| atomic.discovery().optional.in_fence_fd);
            frame_scheduler.decision_with_context(SchedulerFrameContext {
                pacing_mode,
                capabilities: SchedulerCapabilities::explicit_atomic(in_fence, true),
                presentation_target: *scheduled_presentation_target,
                predicted_total_cost: Duration::from_nanos(prediction.total_cost_ns),
                now: scheduler_now,
                render_target_available: effective_render_target_available,
                render_ahead_allowed,
                ready_frame_present: scanout.ready_frame_queued(),
                ready_target_current: frame_scheduler
                    .ready_target()
                    .is_none_or(|target| presentation_deadline.is_current(target)),
            })
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
        let mut cursor_hardware_usable = atomic_cursor.as_ref().is_some_and(|cursor| {
            effective_atomic_cursor_state(cursor, *cursor_render_mode, cursor_visible)
                .hardware_usable()
        });
        if client_cursor_active {
            cursor_hardware_usable = client_cursor_hardware_usable;
        }
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
            direct_active: scanout.direct_scanout_active(),
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
            && scanout.discard_ready_frame_before_direct(server)?
        {
            frame_scheduler.discard_ready_frame();
            scheduler_decision = SchedulerDecision::Render;
            *scheduled_presentation_target = None;
            presentation_deadline.clear_scheduled_target();
            perf.log("direct_scanout", || {
                vec![NativePerfField::str(
                    "event",
                    "retired_pre_entry_composited_frame",
                )]
            });
        }
        let primary_work_for_cursor = primary_visual_work_pending
            || direct_candidate_changed
            || atomic_primary_commit_pending
            || scanout.ready_frame_queued()
            || frame_scheduler.ready_frame_queued();
        let cursor_only_allowed = cursor_only_allowed_at_deadline(
            cursor_output_arbitration,
            *cursor_scheduling_policy,
            scheduler_now.get(),
            primary_work_for_cursor,
            cursor_state_changed,
            cursor_hardware_usable,
        );
        let presentation_path = plan_native_presentation_path(NativePresentationPlanInput {
            direct_active: scanout.direct_scanout_active(),
            direct_candidate_changed,
            direct_candidate_eligible,
            primary_visual_work_pending,
            cursor_changed: cursor_state_changed,
            cursor_hardware_usable,
            cursor_visible,
            composition_required,
            atomic_commit_pending: atomic_primary_commit_pending,
            cursor_only_allowed,
            render_ahead_requested: scheduler_decision == SchedulerDecision::RenderAhead,
        });
        let cursor_only_deferred = cursor_state_changed
            && !primary_visual_work_pending
            && !cursor_only_allowed
            && !scanout.ready_frame_queued()
            && !frame_scheduler.ready_frame_queued();
        if cursor_only_deferred {
            frame_scheduler.note_immediate_completion();
            scheduler_decision = SchedulerDecision::Idle;
        }
        suppress_direct_render_ahead(presentation_path, &mut scheduler_decision, scanout, perf);
        if presentation_path == NativePresentationPath::CursorOnly
            && let Some(cursor) = atomic_cursor.as_mut()
            && !cursor.failure_latched()
            && !atomic_primary_commit_pending
            && !scanout.ready_frame_queued()
        {
            let desired = effective_cursor.clone();
            match kms_backend.test_atomic_cursor_flip(desired.as_ref()) {
                Ok(()) => {
                    let token = PageFlipToken::new(allocate_native_page_flip_token())
                        .expect("allocated native pageflip token is nonzero");
                    atomic_commit_arbiter
                        .reserve(
                            token,
                            *drm_file_generation,
                            target.crtc_id,
                            AtomicCommitKind::CursorOnly {
                                cursor_generation: cursor.generation,
                                framebuffer_id: desired
                                    .as_ref()
                                    .and_then(|state| state.framebuffer_id),
                            },
                            monotonic_now_ns()?,
                        )
                        .map_err(io::Error::other)?;
                    match kms_backend.submit_cursor_flip(desired.as_ref(), token) {
                        Ok(()) => {
                            let submitted_state = desired.unwrap_or_else(|| {
                                let mut hidden = cursor.desired().clone();
                                hidden.visible = false;
                                hidden.framebuffer_id = None;
                                hidden
                            });
                            cursor.begin_submission(token, submitted_state);
                            cursor_output_arbitration.note_cursor_only_submission();
                            *last_submitted_cursor_epoch = cursor_epoch;
                            cursor_output_arbitration.consume(cursor_epoch);
                            *last_client_cursor_damage = current_client_cursor_damage;
                            *last_software_cursor_damage = current_software_cursor_damage;
                            scheduler_decision = SchedulerDecision::WaitForPageFlip;
                            perf.log("native.cursor", || {
                                vec![
                                    NativePerfField::str("event", "submit"),
                                    NativePerfField::str("kind", "cursor_only"),
                                    NativePerfField::u64("generation", cursor.generation),
                                    NativePerfField::bool("visible", effective_cursor.is_some()),
                                    NativePerfField::str(
                                        "position",
                                        format!("{},{}", cursor.desired().x, cursor.desired().y),
                                    ),
                                ]
                            });
                        }
                        Err(error) if error.kind == AtomicKmsErrorKind::Busy => {
                            atomic_commit_arbiter.cancel(token);
                            defer_cursor_after_busy(
                                cursor_output_arbitration,
                                frame_scheduler,
                                pacing_now_ns,
                                perf,
                                "atomic_busy",
                            );
                            scheduler_decision = SchedulerDecision::Idle;
                        }
                        Err(error) => {
                            atomic_commit_arbiter.cancel(token);
                            cursor.note_submit_failure();
                            cursor.note_software_fallback();
                            cursor.set_visible(false);
                            *cursor_render_mode = if client_cursor_active {
                                NativeCursorRenderMode::SoftwareClient
                            } else {
                                NativeCursorRenderMode::Software
                            };
                            *last_client_cursor_damage = None;
                            effective_cursor = None;
                            *queued_redraw_requested = true;
                            scheduler_decision = SchedulerDecision::Render;
                            perf.log("native.cursor", || {
                                vec![
                                    NativePerfField::str("event", "fallback"),
                                    NativePerfField::str("reason", "cursor_submit_failed"),
                                    NativePerfField::str("error", error.to_string()),
                                ]
                            });
                        }
                    }
                }
                Err(error) if error.kind == AtomicKmsErrorKind::Busy => {
                    defer_cursor_after_busy(
                        cursor_output_arbitration,
                        frame_scheduler,
                        pacing_now_ns,
                        perf,
                        "cursor_test_busy",
                    );
                    scheduler_decision = SchedulerDecision::Idle;
                }
                Err(error) => {
                    cursor.note_test_failure();
                    cursor.note_software_fallback();
                    cursor.set_visible(false);
                    *cursor_render_mode = if client_cursor_active {
                        NativeCursorRenderMode::SoftwareClient
                    } else {
                        NativeCursorRenderMode::Software
                    };
                    *last_client_cursor_damage = None;
                    effective_cursor = None;
                    *queued_redraw_requested = true;
                    scheduler_decision = SchedulerDecision::Render;
                    perf.log("native.cursor", || {
                        vec![
                            NativePerfField::str("event", "fallback"),
                            NativePerfField::str("reason", "cursor_test_only_rejected"),
                            NativePerfField::str("error", error.to_string()),
                        ]
                    });
                }
            }
        }
        if atomic_commit_arbiter.atomic_commit_pending() {
            scheduler_decision = SchedulerDecision::WaitForPageFlip;
        }
        if matches!(
            scheduler_decision,
            SchedulerDecision::SubmitReady | SchedulerDecision::SubmitReadyLate
        ) {
            let repaint_present_start = Instant::now();
            let explicit_submission = matches!(&**scanout, NativeScanoutBackend::AtomicEglGbm(_));
            let present_result = if let NativeScanoutBackend::AtomicEglGbm(explicit) =
                &mut **scanout
            {
                let (token, framebuffer_id) =
                    explicit.submit_ready_frame(kms_backend, server, effective_cursor.as_ref())?;
                explicit.mark_composited_submission();
                NativePresentResult::AsyncSubmitted {
                    token,
                    framebuffer_id,
                }
            } else {
                scanout
                    .present(kms_backend, effective_cursor.as_ref())
                    .map_err(|error| {
                        native_runtime_error(
                            NativeRuntimeStage::Present,
                            scanout.kind(),
                            target.crtc_id,
                            *frame_index,
                            error,
                        )
                    })?
            };
            #[cfg(test)]
            native_io_recorder.record(NativeIoOperation::ScanoutPresent);
            let repaint_present_us = elapsed_micros(repaint_present_start);
            match present_result {
                NativePresentResult::AsyncSubmitted {
                    token,
                    framebuffer_id,
                } => {
                    let atomic_primary_registered = register_atomic_primary_submission(
                        atomic_commit_arbiter,
                        kms_backend.effective_kind(),
                        token,
                        *drm_file_generation,
                        target.crtc_id,
                        *frame_index,
                        framebuffer_id,
                        monotonic_now_ns()?,
                    )?;
                    if let Some(cursor) = atomic_cursor.as_mut()
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
                    *last_submitted_cursor_epoch = cursor_epoch;
                    cursor_output_arbitration.consume(cursor_epoch);
                    if !explicit_submission {
                        server.mark_prepared_frame_submitted();
                    }
                    #[cfg(test)]
                    native_io_recorder.record(NativeIoOperation::PageflipSubmit);
                    #[cfg(test)]
                    native_io_recorder.record(match kms_backend.effective_kind() {
                        KmsBackendKind::Atomic => NativeIoOperation::AtomicCommit,
                        KmsBackendKind::Legacy => NativeIoOperation::LegacyCommit,
                    });
                    frame_scheduler
                        .note_ready_submission(token, monotonic_now_ns()?)
                        .map_err(io::Error::other)?;
                    if atomic_primary_registered {
                        frame_scheduler.defer_page_flip_watchdog_to_atomic_arbiter();
                    }
                    frame_pacing.note_submit(token, monotonic_now_ns()?, true, pacing_mode);
                    if explicit_submission
                        && output_render_fence_token.is_none()
                        && let NativeScanoutBackend::AtomicEglGbm(explicit) = &**scanout
                        && let Some(fd) = explicit.pending_timing_fd()
                    {
                        *output_render_fence_token =
                            Some(event_loop.register(fd, NativeEventSource::OutputRenderFence)?);
                    }
                    frame_submitted = true;
                    if !explicit_submission {
                        server.mark_render_damage_presented();
                    }
                    *frame_index = frame_index.saturating_add(1);
                    perf.log("native.frame", || {
                        vec![
                            NativePerfField::u64("index", *frame_index),
                            NativePerfField::str("phase", "ready-submit"),
                            NativePerfField::str("mode", mode_label.clone()),
                            NativePerfField::str("cursor", cursor_render_mode.as_str()),
                            NativePerfField::u64("refresh_hz", u64::from(*refresh_hz)),
                            NativePerfField::u64("repaint_present_us", repaint_present_us),
                            NativePerfField::u64("pageflip_token", token),
                            NativePerfField::bool(
                                "render_ahead_ready",
                                scheduler_decision == SchedulerDecision::SubmitReady,
                            ),
                        ]
                    });
                }
                NativePresentResult::Immediate => {
                    frame_scheduler.note_immediate_completion();
                }
                NativePresentResult::Noop => {
                    perf.log("native.frame_skip", || {
                        vec![
                            NativePerfField::str("reason", "ready_submit_without_ready_frame"),
                            NativePerfField::bool("scanout_ready", scanout.ready_frame_queued()),
                        ]
                    });
                    frame_scheduler.note_immediate_completion();
                }
            }
        } else if matches!(
            scheduler_decision,
            SchedulerDecision::Render | SchedulerDecision::RenderAhead
        ) {
            let render_ahead = scheduler_decision == SchedulerDecision::RenderAhead;
            let mut direct_submitted = false;
            if !render_ahead
                && direct_scanout_preference.enabled()
                && (!cursor_visible || *cursor_render_mode == NativeCursorRenderMode::Hardware)
                && cursor_direct_compatible
                && !atomic_primary_commit_pending
                && !scanout.ready_frame_queued()
                && !scanout.output_render_in_progress()
                && !scanout.direct_scanout_inhibited()
                && direct_candidate_changed
            {
                let direct_target = match pacing_mode {
                    NativeOutputPacingMode::ReactiveDouble => presentation_deadline
                        .reactive_target(MonotonicTimestampNs::new(monotonic_now_ns()?)),
                    NativeOutputPacingMode::PredictiveTriple => scheduled_presentation_target
                        .or_else(|| {
                            presentation_deadline.reactive_target(MonotonicTimestampNs::new(
                                monotonic_now_ns().ok()?,
                            ))
                        }),
                };
                if let Some(direct_target) = direct_target {
                    match scanout.try_direct_scanout(
                        kms_backend,
                        server,
                        direct_target,
                        effective_cursor.as_ref(),
                    )? {
                        DirectScanoutAttempt::Submitted {
                            transaction_id,
                            token,
                        } => {
                            let trace_timestamp_ns = monotonic_now_ns()?;
                            presentation_trace.push(
                                PresentationTransactionEvent::TransactionBuilt {
                                    transaction_id,
                                    timestamp_ns: trace_timestamp_ns,
                                },
                            );
                            presentation_trace.push(
                                PresentationTransactionEvent::KmsSubmitReturned {
                                    transaction_id,
                                    timestamp_ns: trace_timestamp_ns,
                                },
                            );
                            if kms_backend.effective_kind() == KmsBackendKind::Atomic {
                                let commit_token = PageFlipToken::new(token).ok_or_else(|| {
                                    io::Error::other("Direct Atomic token is zero")
                                })?;
                                atomic_commit_arbiter
                                    .reserve(
                                        commit_token,
                                        *drm_file_generation,
                                        target.crtc_id,
                                        AtomicCommitKind::DirectPrimary {
                                            direct_token: commit_token,
                                            framebuffer_id: 0,
                                        },
                                        monotonic_now_ns()?,
                                    )
                                    .map_err(io::Error::other)?;
                            }
                            if let Some(cursor) = atomic_cursor.as_mut()
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
                            if kms_backend.effective_kind() == KmsBackendKind::Atomic {
                                frame_scheduler.defer_page_flip_watchdog_to_atomic_arbiter();
                            }
                            frame_pacing.note_submit(
                                token,
                                monotonic_now_ns()?,
                                false,
                                pacing_mode,
                            );
                            presentation_deadline.clear_scheduled_target();
                            *scheduled_presentation_target = None;
                            *last_rendered_scene_generation = scene_generation;
                            *last_submitted_cursor_epoch = cursor_epoch;
                            cursor_output_arbitration.consume(cursor_epoch);
                            *last_renderable_surfaces = server.renderable_surfaces().to_vec();
                            *last_software_cursor_damage = current_software_cursor_damage;
                            *frame_index = frame_index.saturating_add(1);
                            frame_submitted = true;
                            direct_submitted = true;
                            perf.log("native.direct_scanout", || {
                                vec![
                                    NativePerfField::str("transition", "submit"),
                                    NativePerfField::u64("token", token),
                                    NativePerfField::u64("gpu_draw_us", 0),
                                ]
                            });
                        }
                        DirectScanoutAttempt::Unchanged => {
                            presentation_deadline.clear_scheduled_target();
                            *scheduled_presentation_target = None;
                            perf.log("native.direct_scanout", || {
                                vec![NativePerfField::str("transition", "same_buffer_suppressed")]
                            });
                        }
                        DirectScanoutAttempt::Rejected(rejection) => {
                            perf.log("native.direct_scanout", || {
                                vec![
                                    NativePerfField::str("transition", "fallback"),
                                    NativePerfField::str("rejection", rejection.as_str()),
                                ]
                            });
                        }
                        DirectScanoutAttempt::Fallback(reason) => {
                            if reason == "cursor_test_only_rejected"
                                && let Some(cursor) = atomic_cursor.as_mut()
                            {
                                cursor.note_test_failure();
                                cursor.note_software_fallback();
                                cursor.set_visible(false);
                                *cursor_render_mode = if client_cursor_active {
                                    NativeCursorRenderMode::SoftwareClient
                                } else {
                                    NativeCursorRenderMode::Software
                                };
                                effective_cursor = None;
                                *queued_redraw_requested = true;
                            }
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
            } else {
                frame_pacing.note_render_started(pacing_mode, render_ahead);
                let render_observed_at_ns = monotonic_now_ns()?;
                let mut render_begin_fields = vec![
                    frame_id_field(frame_pacing.active),
                    PacingField::u64("render_generation", render_generation),
                    PacingField::u64("render_observed_at_ns", render_observed_at_ns),
                    PacingField::bool("render_ahead", render_ahead),
                    PacingField::str("buffering_mode", format!("{:?}", adaptive_buffering.mode())),
                    PacingField::u64("prediction_ewma_ns", prediction.ewma_render_ns),
                    PacingField::u64(
                        "prediction_upper_deviation_ns",
                        prediction.upper_render_deviation_ns,
                    ),
                    PacingField::u64("prediction_p90_ns", prediction.p90_recent_render_ns),
                    PacingField::u64("prediction_render_risk_ns", prediction.render_risk_ns),
                    PacingField::u64("dynamic_safety_margin_ns", prediction.safety_margin_ns),
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
                let output_damage = if scanout.direct_scanout_active() {
                    NativeOutputDamage::full_output(target.width, target.height)
                } else {
                    native_output_damage_for_scene_and_cursor(
                        target.width,
                        target.height,
                        last_renderable_surfaces,
                        server.renderable_surfaces(),
                        scene_changed,
                        NativeCursorDamageBounds {
                            previous_client: *last_client_cursor_damage,
                            client: current_client_cursor_damage,
                            previous_software: *last_software_cursor_damage,
                            software: current_software_cursor_damage,
                        },
                    )
                };
                let no_primary_work = output_damage.is_empty() && !effective_redraw_requested;
                if no_primary_work {
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
                    if let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut **scanout {
                        let frame_target = match pacing_mode {
                            NativeOutputPacingMode::ReactiveDouble => presentation_deadline
                                .reactive_target(MonotonicTimestampNs::new(monotonic_now_ns()?)),
                            NativeOutputPacingMode::PredictiveTriple => {
                                scheduled_presentation_target.take().or_else(|| {
                                    presentation_deadline.reactive_target(
                                        MonotonicTimestampNs::new(monotonic_now_ns().ok()?),
                                    )
                                })
                            }
                        }
                        .ok_or_else(|| {
                            io::Error::other(
                                "explicit Atomic render started without a presentation target",
                            )
                        })?;
                        presentation_deadline.clear_scheduled_target();
                        let render_outcome = explicit.render_frame(
                            frame_renderer,
                            server,
                            input_state,
                            *cursor_render_mode,
                            &output_damage,
                            render_generation,
                            frame_target,
                            pacing_mode,
                        )?;
                        match render_outcome {
                            AtomicFrameRenderOutcome::Skipped { reason, render_us } => {
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
                            AtomicFrameRenderOutcome::Rendered { frame_id } => {
                                frame_rendered = true;
                                let ready_at_ns = monotonic_now_ns()?;
                                let waits_for_target = render_ahead;
                                if waits_for_target {
                                    frame_scheduler.note_ready_frame(Some(frame_target));
                                    frame_pacing.note_ready_frame(ready_at_ns, render_ahead);
                                } else {
                                    let (token, framebuffer_id) = explicit.submit_ready_frame(
                                        kms_backend,
                                        server,
                                        effective_cursor.as_ref(),
                                    )?;
                                    let atomic_primary_registered =
                                        register_atomic_primary_submission(
                                            atomic_commit_arbiter,
                                            kms_backend.effective_kind(),
                                            token,
                                            *drm_file_generation,
                                            target.crtc_id,
                                            *frame_index,
                                            framebuffer_id,
                                            monotonic_now_ns()?,
                                        )?;
                                    if let Some(cursor) = atomic_cursor.as_mut()
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
                                    if output_render_fence_token.is_none()
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
                                if !waits_for_target {
                                    *last_submitted_cursor_epoch = cursor_epoch;
                                    cursor_output_arbitration.consume(cursor_epoch);
                                }
                                *last_renderable_surfaces = server.renderable_surfaces().to_vec();
                            }
                        }
                    } else {
                        server.capture_frame_callbacks_for_render();
                        let cpu_before = perf
                            .enabled()
                            .then(NativeProcessCpuSample::read_current)
                            .flatten();
                        let paint_outcome = match scanout.paint_server_frame(
                            frame_renderer,
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
                            // Settle the already-owned batch, not unowned work.
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
                            let present_result = if render_ahead {
                                NativePresentResult::Noop
                            } else {
                                scanout
                                    .present(kms_backend, effective_cursor.as_ref())
                                    .map_err(|error| {
                                        native_runtime_error(
                                            NativeRuntimeStage::Present,
                                            scanout.kind(),
                                            target.crtc_id,
                                            *frame_index,
                                            error,
                                        )
                                    })?
                            };
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
                                } => {
                                    server.mark_prepared_frame_submitted();
                                    #[cfg(test)]
                                    native_io_recorder.record(NativeIoOperation::PageflipSubmit);
                                    #[cfg(test)]
                                    native_io_recorder.record(match kms_backend.effective_kind() {
                                        KmsBackendKind::Atomic => NativeIoOperation::AtomicCommit,
                                        KmsBackendKind::Legacy => NativeIoOperation::LegacyCommit,
                                    });
                                    let atomic_primary_registered =
                                        register_atomic_primary_submission(
                                            atomic_commit_arbiter,
                                            kms_backend.effective_kind(),
                                            token,
                                            *drm_file_generation,
                                            target.crtc_id,
                                            *frame_index,
                                            framebuffer_id,
                                            monotonic_now_ns()?,
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
                                    frame_scheduler.note_immediate_completion();
                                    // Immediate presentation settles the owned batch exactly once.
                                    let finish_frame_start = Instant::now();
                                    server.finish_prepared_frame();
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
                            *last_renderable_surfaces = server.renderable_surfaces().to_vec();
                        }
                    }
                }
            }
        } else if scheduler_decision == SchedulerDecision::CompleteProtocolOnly {
            let protocol_metrics = ProtocolCycleMetrics {
                skipped_input_repaints,
                tick_us,
                pageflip_pending_at_tick,
                input_drain_us,
                raw_input_events,
                coalesced_input_events,
                pageflip_drain_us,
                pageflip_completed,
                present_us,
                render_generation,
                effective_render_target_available,
                scene_changed,
                pending_frame_work,
                redraw_requested,
            };
            complete_protocol_only_tick(server, frame_scheduler, perf, protocol_metrics);
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
                ProtocolCycleMetrics {
                    skipped_input_repaints,
                    tick_us,
                    pageflip_pending_at_tick,
                    input_drain_us,
                    raw_input_events,
                    coalesced_input_events,
                    pageflip_drain_us,
                    pageflip_completed,
                    present_us,
                    render_generation,
                    effective_render_target_available,
                    scene_changed,
                    pending_frame_work,
                    redraw_requested,
                },
            )?;
        } else if skipped_input_repaints > 0 {
            log_no_visual_work(
                perf,
                ProtocolCycleMetrics {
                    skipped_input_repaints,
                    tick_us,
                    pageflip_pending_at_tick,
                    input_drain_us,
                    raw_input_events,
                    coalesced_input_events,
                    pageflip_drain_us,
                    pageflip_completed,
                    present_us,
                    render_generation,
                    effective_render_target_available,
                    scene_changed,
                    pending_frame_work,
                    redraw_requested,
                },
            );
        }
        cycle.frame_completed = frame_completed;
        cycle.frame_rendered = frame_rendered;
        cycle.frame_submitted = frame_submitted;
        self.update_cycle_metrics(cycle, scheduler_decision)?;
        Ok(())
    }
}
#[cfg(test)]
#[path = "presentation_tests.rs"]
mod pacing_mode_tests;
