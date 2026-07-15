use super::frame::{NativeRepaintInputs, native_repaint_decision};
use super::*;

fn plan_scheduled_target_for_mode(
    planner: &mut PresentationDeadlinePlanner,
    pacing_mode: NativeOutputPacingMode,
    pending_target: Option<PresentationTarget>,
    now: MonotonicTimestampNs,
    predicted_total_cost: Duration,
    reason: PresentationTargetReason,
) -> Option<PresentationTarget> {
    if pacing_mode != NativeOutputPacingMode::PredictiveTriple {
        return None;
    }
    planner.plan_render_ahead(pending_target?, now, predicted_total_cost, reason)
}

fn visual_target_deadline_for_mode(
    pacing_mode: NativeOutputPacingMode,
    scheduled_target: Option<PresentationTarget>,
) -> Option<u64> {
    (pacing_mode == NativeOutputPacingMode::PredictiveTriple)
        .then(|| scheduled_target.map(|target| target.render_start_deadline.get()))
        .flatten()
}

impl NativeRuntime {
    #[allow(unused_variables)]
    pub(super) fn render_present_and_update_metrics(
        &mut self,
        cycle: &mut NativeCycleState,
    ) -> NativeResult<()> {
        let perf = self.perf;
        let Self {
            server,
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
            direct_scanout_preference,
            cursor_render_mode,
            hardware_cursor,
            input_devices,
            acquire_notifier,
            acquire_watches,
            parked_acquire_watches: _,
            event_loop,
            drm_reactor_token: _,
            output_render_fence_token,
            frame_scheduler,
            presentation_deadline,
            scheduled_presentation_target,
            render_journal,
            adaptive_buffering,
            triple_buffer_policy,
            pending_proven_deadline_miss,
            effective_app_gpu_policy,
            last_render_generation,
            last_renderable_surfaces,
            queued_redraw_requested,
            frame_index,
            known_toplevels,
            pending_launches,
            mismatched_pageflip_events,
            stale_pageflip_events,
            presentation_cadence,
            frame_pacing,
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
        let render_generation_changed = render_generation != *last_render_generation;
        let render_generation_cause = server.render_generation_cause();
        let pending_frame_work = server.has_pending_frame_work();
        let repaint_decision = native_repaint_decision(NativeRepaintInputs {
            accepted_clients: accepted > 0,
            render_generation_changed,
            pending_frame_work,
            only_pending_surface_frame_callbacks: server.has_only_pending_surface_frame_callbacks(),
            redraw_requested,
            page_flip_pending: false,
        });
        let pacing_now_ns = monotonic_now_ns()?;
        if repaint_decision.repaint {
            frame_pacing.queue_visual(pacing_now_ns, render_generation);
            frame_scheduler.queue_visual_work();
            *queued_redraw_requested |= redraw_requested;
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
        let scheduler_decision = if explicit_output {
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
        let mut decision_fields = vec![
            frame_id_field(frame_pacing.active),
            PacingField::u64("render_generation", render_generation),
            PacingField::str("scheduler_decision", format!("{scheduler_decision:?}")),
            PacingField::bool("render_target_available", effective_render_target_available),
            PacingField::u64(
                "pageflip_token",
                scanout.pending_page_flip_token().unwrap_or(0),
            ),
        ];
        decision_fields.extend(snapshot_fields(scanout.buffer_snapshot()));
        frame_pacing.log("decision", decision_fields);
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
        if matches!(
            scheduler_decision,
            SchedulerDecision::SubmitReady | SchedulerDecision::SubmitReadyLate
        ) {
            let repaint_present_start = Instant::now();
            let explicit_submission = matches!(&**scanout, NativeScanoutBackend::AtomicEglGbm(_));
            let present_result =
                if let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut **scanout {
                    let token = explicit.submit_ready_frame(kms_backend, server)?;
                    explicit.mark_composited_submission();
                    NativePresentResult::AsyncSubmitted { token }
                } else {
                    scanout.present(kms_backend).map_err(|error| {
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
                NativePresentResult::AsyncSubmitted { token } => {
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
                && hardware_cursor.is_some()
                && !scanout.page_flip_pending()
                && !scanout.ready_frame_queued()
                && !scanout.output_render_in_progress()
                && !scanout.direct_scanout_inhibited()
                && (render_generation_changed || pending_frame_work || redraw_requested)
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
                    match scanout.try_direct_scanout(kms_backend, server, direct_target)? {
                        DirectScanoutAttempt::Submitted { token } => {
                            frame_scheduler
                                .note_async_submission(token, monotonic_now_ns()?)
                                .map_err(io::Error::other)?;
                            frame_pacing.note_submit(
                                token,
                                monotonic_now_ns()?,
                                false,
                                pacing_mode,
                            );
                            presentation_deadline.clear_scheduled_target();
                            *scheduled_presentation_target = None;
                            *last_render_generation = render_generation;
                            *last_renderable_surfaces = server.renderable_surfaces().to_vec();
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
                        DirectScanoutAttempt::Rejected(rejection) => {
                            perf.log("native.direct_scanout", || {
                                vec![
                                    NativePerfField::str("transition", "fallback"),
                                    NativePerfField::str("rejection", rejection.as_str()),
                                ]
                            });
                        }
                        DirectScanoutAttempt::Fallback(reason) => {
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
                    render_generation_cause,
                    render_generation_changed,
                    accepted,
                    pending_frame_work,
                    effective_redraw_requested,
                );
                let output_damage = if scanout.direct_scanout_active() {
                    NativeOutputDamage::full_output(target.width, target.height)
                } else {
                    native_output_damage_for_repaint(
                        target.width,
                        target.height,
                        last_renderable_surfaces,
                        server.renderable_surfaces(),
                        render_generation_cause,
                        render_generation_changed,
                    )
                };
                let skip_empty_visible_damage = output_damage.is_empty()
                    && render_generation_changed
                    && accepted == 0
                    && !effective_redraw_requested;
                if skip_empty_visible_damage {
                    perf.log("native.frame_skip", || {
                        let mut fields = output_damage.fields().to_vec();
                        fields.extend([
                            NativePerfField::str("reason", "empty_visible_damage"),
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
                    *last_render_generation = render_generation;
                    *last_renderable_surfaces = server.renderable_surfaces().to_vec();
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
                        let frame_id = explicit.render_frame(
                            frame_renderer,
                            server,
                            input_state,
                            *cursor_render_mode,
                            &output_damage,
                            render_generation,
                            frame_target,
                            pacing_mode,
                        )?;
                        frame_rendered = true;
                        let ready_at_ns = monotonic_now_ns()?;
                        let waits_for_target = render_ahead;
                        if waits_for_target {
                            frame_scheduler.note_ready_frame(Some(frame_target));
                            frame_pacing.note_ready_frame(ready_at_ns, render_ahead);
                        } else {
                            let token = explicit.submit_ready_frame(kms_backend, server)?;
                            frame_scheduler
                                .note_async_submission(token, monotonic_now_ns()?)
                                .map_err(io::Error::other)?;
                            frame_pacing.note_submit(
                                token,
                                monotonic_now_ns()?,
                                false,
                                pacing_mode,
                            );
                            if output_render_fence_token.is_none()
                                && let Some(fd) = explicit.pending_timing_fd()
                            {
                                *output_render_fence_token = Some(
                                    event_loop
                                        .register(fd, NativeEventSource::OutputRenderFence)?,
                                );
                            }
                            frame_submitted = true;
                            *frame_index = frame_index.saturating_add(1);
                        }
                        frame_pacing.log(
                            "render_complete",
                            vec![
                                PacingField::u64("frame_id", frame_id),
                                PacingField::u64("render_generation", render_generation),
                                PacingField::u64("render_observed_at_ns", render_observed_at_ns),
                                PacingField::u64("render_end_ns", ready_at_ns),
                                PacingField::u64("target_vblank_sequence", frame_target.sequence),
                                PacingField::u64(
                                    "target_presentation_ns",
                                    frame_target.presentation_time.get(),
                                ),
                                PacingField::bool("render_ahead", render_ahead),
                            ],
                        );
                        *queued_redraw_requested = false;
                        *last_render_generation = render_generation;
                        *last_renderable_surfaces = server.renderable_surfaces().to_vec();
                    } else {
                        server.capture_frame_callbacks_for_render();
                        let cpu_before = perf
                            .enabled()
                            .then(NativeProcessCpuSample::read_current)
                            .flatten();
                        let paint_outcome = scanout.paint_server_frame(
                            frame_renderer,
                            server,
                            input_state,
                            *cursor_render_mode,
                            &output_damage,
                        )?;
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
                            if server.has_pending_frame_work() {
                                server.finish_frame();
                                frame_completed = true;
                            }
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
                            *last_render_generation = render_generation;
                            *last_renderable_surfaces = server.renderable_surfaces().to_vec();
                        } else {
                            frame_rendered = true;
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
                                scanout.present(kms_backend).map_err(|error| {
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
                                NativePresentResult::AsyncSubmitted { token } => {
                                    server.mark_prepared_frame_submitted();
                                    #[cfg(test)]
                                    native_io_recorder.record(NativeIoOperation::PageflipSubmit);
                                    #[cfg(test)]
                                    native_io_recorder.record(match kms_backend.effective_kind() {
                                        KmsBackendKind::Atomic => NativeIoOperation::AtomicCommit,
                                        KmsBackendKind::Legacy => NativeIoOperation::LegacyCommit,
                                    });
                                    frame_scheduler
                                        .note_async_submission(token, monotonic_now_ns()?)
                                        .map_err(io::Error::other)?;
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
                                    if server.has_pending_frame_work() {
                                        let finish_frame_start = Instant::now();
                                        server.finish_frame();
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
                                    NativePerfField::bool(
                                        "render_changed",
                                        render_generation_changed,
                                    ),
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
                            *last_render_generation = render_generation;
                            *last_renderable_surfaces = server.renderable_surfaces().to_vec();
                        }
                    }
                }
            }
        } else if scheduler_decision == SchedulerDecision::CompleteProtocolOnly {
            perf.log("native.frame_skip", || {
                vec![
                    NativePerfField::str("reason", "frame_callback_no_damage"),
                    NativePerfField::usize("skipped_input_repaints", skipped_input_repaints),
                    NativePerfField::u64("tick_us", tick_us),
                    NativePerfField::bool("pageflip_pending_at_tick", pageflip_pending_at_tick),
                    NativePerfField::u64("input_drain_us", input_drain_us),
                    NativePerfField::usize("raw_input_events", raw_input_events),
                    NativePerfField::usize("coalesced_input_events", coalesced_input_events),
                    NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
                    NativePerfField::bool("pageflip_completed", pageflip_completed),
                    NativePerfField::u64("present_us", present_us),
                    NativePerfField::u64("render_generation", render_generation),
                ]
            });
            let finish_frame_start = Instant::now();
            server.finish_frame();
            frame_scheduler.complete_protocol_only();
            frame_completed = true;
            perf.log("native.finish_frame", || {
                vec![
                    NativePerfField::str("reason", "frame_callback_no_damage"),
                    NativePerfField::u64("elapsed_us", elapsed_micros(finish_frame_start)),
                    NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                    NativePerfField::u64("render_generation", server.render_generation()),
                ]
            });
        } else if matches!(
            scheduler_decision,
            SchedulerDecision::WaitForPageFlip | SchedulerDecision::WaitForBuffer
        ) {
            let snapshot = scanout.buffer_snapshot();
            let event = if scheduler_decision == SchedulerDecision::WaitForBuffer {
                frame_pacing.wait_for_buffer_count += 1;
                "wait_for_buffer"
            } else {
                "wait_for_pageflip"
            };
            let now_ns = monotonic_now_ns()?;
            let mut fields = vec![
                frame_id_field(frame_pacing.active),
                PacingField::bool("pageflip_pending", scanout.page_flip_pending()),
                PacingField::u64(
                    "time_since_last_pageflip_us",
                    frame_pacing
                        .last_pageflip_ns()
                        .map_or(0, |last| now_ns.saturating_sub(last) / 1_000),
                ),
                PacingField::u64(
                    "time_since_visual_queued_us",
                    frame_pacing
                        .active_queued_ns
                        .map_or(0, |queued| now_ns.saturating_sub(queued) / 1_000),
                ),
            ];
            fields.extend(snapshot_fields(snapshot));
            frame_pacing.log(event, fields);
            perf.log("native.frame_skip", || {
                vec![
                    NativePerfField::str(
                        "reason",
                        if scheduler_decision == SchedulerDecision::WaitForBuffer {
                            "render_target_unavailable"
                        } else {
                            "pageflip_pending"
                        },
                    ),
                    NativePerfField::usize("skipped_input_repaints", skipped_input_repaints),
                    NativePerfField::u64("tick_us", tick_us),
                    NativePerfField::bool("pageflip_pending_at_tick", pageflip_pending_at_tick),
                    NativePerfField::bool(
                        "render_target_available",
                        effective_render_target_available,
                    ),
                    NativePerfField::u64("input_drain_us", input_drain_us),
                    NativePerfField::usize("raw_input_events", raw_input_events),
                    NativePerfField::usize("coalesced_input_events", coalesced_input_events),
                    NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
                    NativePerfField::bool("pageflip_completed", pageflip_completed),
                    NativePerfField::u64("present_us", present_us),
                    NativePerfField::u64("render_generation", render_generation),
                    NativePerfField::bool("render_changed", render_generation_changed),
                    NativePerfField::bool("pending_frame_work", pending_frame_work),
                    NativePerfField::bool("redraw_requested", redraw_requested),
                ]
            });
        } else if skipped_input_repaints > 0 {
            perf.log("native.frame_skip", || {
                vec![
                    NativePerfField::str("reason", "input_forwarded_no_visual"),
                    NativePerfField::usize("skipped_input_repaints", skipped_input_repaints),
                    NativePerfField::u64("tick_us", tick_us),
                    NativePerfField::bool("pageflip_pending_at_tick", pageflip_pending_at_tick),
                    NativePerfField::u64("input_drain_us", input_drain_us),
                    NativePerfField::usize("raw_input_events", raw_input_events),
                    NativePerfField::usize("coalesced_input_events", coalesced_input_events),
                    NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
                    NativePerfField::bool("pageflip_completed", pageflip_completed),
                    NativePerfField::u64("present_us", present_us),
                    NativePerfField::u64("render_generation", render_generation),
                ]
            });
        }
        cycle.frame_completed = frame_completed;
        cycle.frame_rendered = frame_rendered;
        cycle.frame_submitted = frame_submitted;
        self.update_cycle_metrics(cycle, scheduler_decision)?;
        Ok(())
    }

    fn update_cycle_metrics(
        &mut self,
        cycle: &NativeCycleState,
        scheduler_decision: SchedulerDecision,
    ) -> NativeResult<()> {
        let perf = self.perf;
        perf.log("native.scheduler", || {
            let fullscreen = self.server.fullscreen_render_plan_metrics();
            let mut fields = vec![
                NativePerfField::str("decision", format!("{scheduler_decision:?}")),
                NativePerfField::str("state_after", format!("{:?}", self.frame_scheduler.state())),
                NativePerfField::bool("pageflip_pending", self.frame_scheduler.page_flip_pending()),
                NativePerfField::bool(
                    "ready_frame_queued",
                    self.frame_scheduler.ready_frame_queued(),
                ),
                NativePerfField::bool("scanout_ready_frame", self.scanout.ready_frame_queued()),
                NativePerfField::bool(
                    "visual_work_queued",
                    self.frame_scheduler.visual_work_queued(),
                ),
                NativePerfField::bool(
                    "protocol_work_queued",
                    self.frame_scheduler.protocol_work_queued(),
                ),
                NativePerfField::bool("frame_rendered", cycle.frame_rendered),
                NativePerfField::bool("frame_submitted", cycle.frame_submitted),
                NativePerfField::bool("frame_completed", cycle.frame_completed),
                NativePerfField::u64(
                    "watchdog_timeout_count",
                    self.frame_scheduler.watchdog_timeout_count(),
                ),
                NativePerfField::u64(
                    "mismatched_pageflip_events",
                    self.mismatched_pageflip_events,
                ),
                NativePerfField::u64("stale_pageflip_events", self.stale_pageflip_events),
                NativePerfField::u64("presentations", self.presentation_cadence.presentations()),
                NativePerfField::u64(
                    "presentation_sequence_gaps",
                    self.presentation_cadence.sequence_gaps(),
                ),
                NativePerfField::bool("fullscreen_active", fullscreen.fullscreen_active),
                NativePerfField::str(
                    "fullscreen_owner_root",
                    fullscreen
                        .owner_root_surface_id
                        .map(|owner| owner.to_string())
                        .unwrap_or_else(|| "none".to_string()),
                ),
                NativePerfField::bool("solitary_tree_active", fullscreen.solitary_tree_active),
                NativePerfField::usize(
                    "fullscreen_culled_surfaces",
                    fullscreen.culled_surface_count,
                ),
                NativePerfField::bool("fullscreen_wallpaper_culled", fullscreen.wallpaper_culled),
                NativePerfField::usize(
                    "fullscreen_visible_overlays",
                    fullscreen.visible_overlay_count,
                ),
                NativePerfField::str(
                    "fullscreen_rejection",
                    fullscreen
                        .rejection
                        .map(FullscreenPresentationRejection::as_str)
                        .unwrap_or("none"),
                ),
            ];
            fields.extend([
                NativePerfField::bool(
                    "direct_scanout_active",
                    self.scanout.direct_scanout_active(),
                ),
                NativePerfField::bool(
                    "direct_scanout_pending",
                    self.scanout.direct_scanout_pending(),
                ),
                NativePerfField::u64(
                    "direct_scanout_surface",
                    u64::from(self.scanout.direct_scanout_surface().unwrap_or(0)),
                ),
            ]);
            if let Some((buffer, framebuffer, format, modifier)) =
                self.scanout.direct_scanout_info()
            {
                fields.extend([
                    NativePerfField::u64("direct_scanout_buffer", buffer),
                    NativePerfField::u64("direct_scanout_framebuffer", u64::from(framebuffer)),
                    NativePerfField::u64("direct_scanout_format", u64::from(format)),
                    NativePerfField::u64("direct_scanout_modifier", modifier),
                ]);
            }
            if let Some(counters) = self.scanout.direct_scanout_counters() {
                fields.extend([
                    NativePerfField::u64(
                        "direct_scanout_candidate_checks",
                        counters.candidate_checks,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_candidates_accepted",
                        counters.candidates_accepted,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_import_attempts",
                        counters.import_attempts,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_import_cache_hits",
                        counters.import_cache_hits,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_import_failures",
                        counters.import_failures,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_test_only_attempts",
                        counters.test_only_attempts,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_test_only_rejections",
                        counters.test_only_rejections,
                    ),
                    NativePerfField::u64("direct_scanout_submissions", counters.submissions),
                    NativePerfField::u64("direct_scanout_presentations", counters.presentations),
                    NativePerfField::u64("direct_scanout_entries", counters.entries),
                    NativePerfField::u64("direct_scanout_exits", counters.exits),
                    NativePerfField::u64(
                        "direct_scanout_same_buffer_resubmissions",
                        counters.same_buffer_resubmissions,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_composited_fallbacks",
                        counters.composited_fallbacks,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_stale_candidate_rejections",
                        counters.stale_candidate_rejections,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_cleanup_failures",
                        counters.cleanup_failures,
                    ),
                ]);
            }
            fields
        });
        let scheduler_deadline = self.frame_scheduler.next_deadline_ns();
        let visual_deadline = visual_target_deadline_for_mode(
            self.adaptive_buffering.pacing_mode(),
            self.scheduled_presentation_target,
        );
        self.frame_pacing.note_deadline_state(
            scheduler_decision,
            monotonic_now_ns()?,
            scheduler_deadline,
            visual_deadline,
            self.frame_scheduler.ready_frame_queued() || self.scanout.ready_frame_queued(),
            cycle.wakeup.reasons.timer(),
        );
        self.event_loop.arm_deadline(earliest_native_deadline(
            earliest_native_deadline(scheduler_deadline, visual_deadline),
            self.acquire_watches.next_fallback_deadline_ns(),
        ))?;
        Ok(())
    }
}

#[cfg(test)]
mod pacing_mode_tests {
    use super::*;

    #[test]
    fn reactive_double_never_schedules_a_normal_visual_target() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_nanos(6_060_606));
        planner.note_presented(MonotonicTimestampNs::new(6_060_606));

        let target = plan_scheduled_target_for_mode(
            &mut planner,
            NativeOutputPacingMode::ReactiveDouble,
            None,
            MonotonicTimestampNs::new(7_000_000),
            Duration::from_millis(100),
            PresentationTargetReason::PredictedPressure,
        );

        assert_eq!(target, None);
        assert_eq!(planner.scheduled_target(), None);
    }

    #[test]
    fn predictive_triple_only_schedules_pending_plus_one() {
        let mut planner = PresentationDeadlinePlanner::new(Duration::from_millis(10));
        planner.note_presented(MonotonicTimestampNs::new(10_000_000));
        let pending = planner
            .reactive_target(MonotonicTimestampNs::new(11_000_000))
            .unwrap();

        assert_eq!(
            plan_scheduled_target_for_mode(
                &mut planner,
                NativeOutputPacingMode::PredictiveTriple,
                None,
                MonotonicTimestampNs::new(12_000_000),
                Duration::from_millis(2),
                PresentationTargetReason::PredictedPressure,
            ),
            None
        );
        let ready = plan_scheduled_target_for_mode(
            &mut planner,
            NativeOutputPacingMode::PredictiveTriple,
            Some(pending),
            MonotonicTimestampNs::new(12_000_000),
            Duration::from_millis(2),
            PresentationTargetReason::PredictedPressure,
        )
        .unwrap();
        assert_eq!(ready.sequence, pending.sequence + 1);
    }

    #[test]
    fn reactive_double_visual_target_never_owns_an_event_loop_deadline() {
        let target = PresentationTarget {
            sequence: 1,
            presentation_time: MonotonicTimestampNs::new(10),
            submit_not_before: MonotonicTimestampNs::new(9),
            render_start_deadline: MonotonicTimestampNs::new(8),
            refresh_interval: Duration::from_millis(1),
            reason: PresentationTargetReason::ReactiveDouble,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
        };

        assert_eq!(
            visual_target_deadline_for_mode(NativeOutputPacingMode::ReactiveDouble, Some(target)),
            None
        );
        assert_eq!(
            visual_target_deadline_for_mode(NativeOutputPacingMode::PredictiveTriple, Some(target)),
            Some(8)
        );
    }
}
