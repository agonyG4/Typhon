use super::cursor_cycle::{
    apply_cursor_position, complete_cursor_only_pageflip, complete_primary_cursor_pageflip,
};
use super::*;
pub fn run(
    server: OwnCompositorServer,
    app: Vec<String>,
    app_gpu_preference: CompositorAppGpuPreference,
) -> NativeResult<()> {
    let mut runtime = NativeRuntime::bootstrap(NativeRuntimeConfig {
        server,
        app,
        app_gpu_preference,
    })?;
    runtime.run()
}
impl NativeRuntime {
    pub(super) fn run_native_cycle(&mut self) -> NativeResult<()> {
        while !self.shutdown.is_complete() {
            self.run_cycle()?;
        }
        native_shutdown_debug_log("shutdown_complete");
        Ok(())
    }

    fn run_cycle(&mut self) -> NativeResult<()> {
        let mut cycle = self.wait_for_events_and_pageflips()?;
        self.server.set_commit_debug_pageflip_pending(
            self.scanout.page_flip_pending() || self.atomic_commit_arbiter.atomic_commit_pending(),
        );
        self.reap_supervised_children(&cycle)?;
        let xwm_drain_started = Instant::now();
        self.dispatch_xwayland_events(&cycle.wakeup)?;
        self.note_timing_scope("xwm_dispatch", xwm_drain_started.elapsed());
        if self.xwayland.generation().is_some() {
            self.attach_xwayland_private_client()?;
        } else {
            self.revoke_xwayland_private_client();
        }
        if cycle.wakeup.reasons.timer() {
            self.xwayland
                .handle_deadline(monotonic_now_ns()?, &mut self.process_supervisor)?;
            if self.xwayland.generation().is_none() {
                self.revoke_xwayland_private_client();
            }
            self.sync_xwayland_reactor_sources()?;
        }
        self.sync_xwayland_reactor_sources()?;
        self.advance_shutdown_lifecycle(&cycle)?;
        if !self.session.permits_output() {
            self.dispatch_suspended_sources(&cycle)?;
            return Ok(());
        }
        if !self.shutdown.is_running() {
            return Ok(());
        }
        let wayland_dispatch_started = Instant::now();
        self.dispatch_wayland_and_input(&mut cycle)?;
        self.note_timing_scope("wayland_dispatch", wayland_dispatch_started.elapsed());
        self.dispatch_xwayland_client_disconnects()?;
        self.dispatch_xwayland_shell_binds()?;
        self.initialize_managed_xwayland()?;
        let association_events = self.server.take_xwayland_association_events();
        self.xwayland.record_association_events(&association_events);
        self.dispatch_xwayland_association_events();
        self.dispatch_xwayland_buffer_ready();
        self.dispatch_xwayland_window_events()?;
        self.sync_xwayland_reactor_sources()?;
        if cycle.shutdown_requested {
            self.request_native_shutdown()?;
        }
        if !self.shutdown.is_running() || !self.session.permits_output() {
            return Ok(());
        }
        drain_pending_process_launches_with_xwayland_environment(
            &mut self.server,
            &mut self.process_supervisor,
            &mut self.astrea_launch_tracker,
            self.effective_app_gpu_policy,
            self.perf,
            &mut self.pending_launches,
            self.xwayland.normal_app_environment(),
        );
        let prepare_started = Instant::now();
        self.process_acquire_and_prepare(&cycle)?;
        self.note_timing_scope("prepare_frame", prepare_started.elapsed());
        if !self.shutdown.is_running() || !self.session.permits_output() {
            return Ok(());
        }
        let render_started = Instant::now();
        self.render_present_and_update_metrics(&mut cycle)?;
        self.note_timing_scope("egl_draw", render_started.elapsed());
        self.flush_presentation_trace()?;
        Ok(())
    }

    fn flush_presentation_trace(&self) -> NativeResult<()> {
        let Some(path) = self.presentation_trace_path.as_ref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.presentation_trace.export_jsonl())?;
        Ok(())
    }

    pub(super) fn advance_shutdown_lifecycle(
        &mut self,
        cycle: &NativeCycleState,
    ) -> NativeResult<()> {
        loop {
            match self.shutdown.state() {
                ShutdownState::Running | ShutdownState::Complete => return Ok(()),
                ShutdownState::Requested => {
                    if let Some(transition) = self.shutdown.advance_requested() {
                        self.log_shutdown_transition(transition);
                        if self.shutdown.state() == ShutdownState::Draining {
                            native_shutdown_debug_log("pageflip_drain_begin");
                            self.arm_shutdown_deadline()?;
                            return Ok(());
                        }
                    }
                }
                ShutdownState::Draining => {
                    if let Some(token) = cycle.completed_pageflip_token
                        && let Some(transition) = self.shutdown.note_pageflip_event(token)
                    {
                        native_shutdown_debug_log("pageflip_drain_confirmed");
                        self.log_shutdown_transition(transition);
                        continue;
                    }
                    if cycle.wakeup.reasons.drm() && !cycle.pageflip_completed {
                        let _ = self.shutdown.note_empty_nonblocking_drm_read();
                    }
                    let now_ns = monotonic_now_ns()?;
                    if let Some(transition) = self.shutdown.advance_pageflip_timeout(now_ns) {
                        native_shutdown_debug_log("pageflip_drain_forced_timeout");
                        self.log_shutdown_transition(transition);
                        self.perf.log("native.shutdown_pageflip_timeout", || {
                            vec![
                                NativePerfField::u64(
                                    "expected_token",
                                    self.shutdown.expected_pageflip_token().unwrap_or(0),
                                ),
                                NativePerfField::bool(
                                    "scanout_pageflip_pending",
                                    self.scanout.page_flip_pending(),
                                ),
                            ]
                        });
                        continue;
                    }
                    self.arm_shutdown_deadline()?;
                    return Ok(());
                }
                ShutdownState::StoppingChildren => {
                    if self.shutdown.mark_child_stop_started() {
                        native_shutdown_debug_log("shell_children_stop");
                        self.perf.log("native.shutdown_children", || {
                            vec![NativePerfField::str("stage", "begin")]
                        });
                        self.revoke_xwayland_private_client();
                        self.xwayland.begin_shutdown(&mut self.process_supervisor)?;
                        self.sync_xwayland_reactor_sources()?;
                        self.process_supervisor.begin_shutdown(Instant::now())?;
                    }
                    if self.process_supervisor.advance_shutdown(Instant::now())?
                        && let Some(transition) = self.shutdown.note_session_children_stopped()
                    {
                        self.log_shutdown_transition(transition);
                        continue;
                    }
                    self.event_loop
                        .arm_deadline(Some(monotonic_now_ns()?.saturating_add(50_000_000)))?;
                    return Ok(());
                }
                ShutdownState::Restoring => {
                    self.restore_kms_for_shutdown()?;
                    return Ok(());
                }
            }
        }
    }

    fn arm_shutdown_deadline(&mut self) -> NativeResult<()> {
        self.event_loop
            .arm_deadline(self.shutdown.pageflip_deadline_ns())?;
        Ok(())
    }

    pub(super) fn log_shutdown_transition(&self, transition: ShutdownTransition) {
        self.perf.log("native.shutdown_transition", || {
            vec![
                NativePerfField::str("from", transition.from.as_str()),
                NativePerfField::str("to", transition.to.as_str()),
                NativePerfField::str("reason", transition.reason.as_str()),
                NativePerfField::u64(
                    "pending_pageflip_token",
                    self.shutdown.expected_pageflip_token().unwrap_or(0),
                ),
            ]
        });
        native_shutdown_debug_log(&format!(
            "state_{}_to_{}",
            transition.from.as_str(),
            transition.to.as_str()
        ));
    }

    #[allow(unused_variables)]
    fn wait_for_events_and_pageflips(&mut self) -> NativeResult<NativeCycleState> {
        let wakeup = self.event_loop.wait()?;
        self.dispatch_runtime_seat_events(&wakeup)?;
        if self.session.permits_output()
            && (wakeup.reasons.drm()
                || (wakeup.reasons.timer()
                    && (self.scanout.page_flip_pending()
                        || self.atomic_commit_arbiter.atomic_commit_pending())))
        {
            NativeSessionIo::observe(self, NativeIoOperation::PageflipDrain);
        }
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
            cursor_render_mode,
            atomic_cursor,
            legacy_cursor,
            input_devices,
            seat_session: _,
            session: _,
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
            pending_proven_deadline_miss,
            effective_app_gpu_policy,
            last_primary_presented_at_ns,
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
            process_supervisor: _,
            shutdown,
            ..
        } = self;
        let scheduler_state_before = frame_scheduler.state();
        perf.log("native.wakeup", || {
            vec![
                NativePerfField::u64("ready_mask", u64::from(wakeup.reasons.bits())),
                NativePerfField::usize("ready_sources", wakeup.ready_sources),
                NativePerfField::u64("blocked_us", wakeup.blocked_ns / 1_000),
                NativePerfField::u64(
                    "deadline_late_us",
                    wakeup.timer_lateness_ns.unwrap_or(0) / 1_000,
                ),
                NativePerfField::str("scheduler_before", format!("{scheduler_state_before:?}")),
                NativePerfField::bool("pageflip_pending", scanout.page_flip_pending()),
            ]
        });
        if wakeup.reasons.timer() {
            let wake_lateness_ns = wakeup.timer_lateness_ns.unwrap_or(0);
            render_journal.record_wake_lateness(wake_lateness_ns);
            frame_pacing.note_wake_lateness(wake_lateness_ns);
            perf.log("native.deadline", || {
                vec![
                    NativePerfField::u64("lateness_us", wake_lateness_ns / 1_000),
                    NativePerfField::u64("scheduler_wakeup_lateness_ns", wake_lateness_ns),
                    NativePerfField::str("scheduler_state", format!("{scheduler_state_before:?}")),
                    NativePerfField::bool("pageflip_watchdog", frame_scheduler.page_flip_pending()),
                ]
            });
        }
        if wakeup.reasons.output_render_fence() {
            if let Some(token) = output_render_fence_token.take() {
                event_loop.unregister(token)?;
            }
            if let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut **scanout
                && let Some(timing) = explicit
                    .sample_pending_timing(MonotonicTimestampNs::new(monotonic_now_ns()?))?
            {
                frame_pacing.note_fence_timestamp_quality(timing.quality);
                render_journal.record_render_sample(
                    timing
                        .signaled_at
                        .get()
                        .saturating_sub(timing.composite_started_at.get()),
                    timing.signaled_at,
                );
                let before = render_journal.prediction(timing.target.refresh_interval);
                *pending_proven_deadline_miss = match timing.quality {
                    FenceTimestampQuality::ExactSyncFile
                        if timing.signaled_at > timing.target.presentation_time =>
                    {
                        Some(ProvenDeadlineMiss::ExactRender)
                    }
                    FenceTimestampQuality::ObservedApproximate
                        if approximate_observation_is_late(
                            timing.signaled_at.get(),
                            timing.target.presentation_time.get(),
                            before.p95_wake_lateness_ns,
                        ) =>
                    {
                        Some(ProvenDeadlineMiss::GuardedApproximateRender)
                    }
                    _ => None,
                };
                perf.log("native.render_fence", || {
                    vec![
                        NativePerfField::u64("frame_id", timing.frame_id),
                        NativePerfField::u64("signal_ns", timing.signaled_at.get()),
                        NativePerfField::u64("target_ns", timing.target.presentation_time.get()),
                        NativePerfField::u64(
                            "render_fence_signal_latency_ns",
                            timing
                                .signaled_at
                                .get()
                                .saturating_sub(timing.composite_started_at.get()),
                        ),
                        NativePerfField::str("quality", format!("{:?}", timing.quality)),
                    ]
                });
            }
        }
        if !self.session.permits_output() {
            return Ok(NativeCycleState {
                wakeup,
                pageflip_drain_us: 0,
                pageflip_completed: false,
                completed_pageflip_token: None,
                frame_completed: false,
                frame_rendered: false,
                frame_submitted: false,
                present_us: 0,
                pageflip_pending_at_tick: false,
                tick_us: 0,
                accepted: 0,
                redraw_requested: false,
                skipped_input_repaints: 0,
                input_drain_us: 0,
                raw_input_events: 0,
                coalesced_input_events: 0,
                shutdown_requested: false,
            });
        }
        let pageflip_drain_start = Instant::now();
        let should_drain_pageflips = wakeup.reasons.drm()
            || (wakeup.reasons.timer()
                && (frame_scheduler.page_flip_pending()
                    || atomic_commit_arbiter.atomic_commit_pending()
                    || shutdown.state() == ShutdownState::Draining));
        let pageflip_drain = if should_drain_pageflips {
            scanout
                .drain_page_flip_events(kms.file().as_raw_fd(), kms_backend.effective_kind())
                .map_err(|error| {
                    native_runtime_error(
                        NativeRuntimeStage::DrainPageFlipEvents,
                        scanout.kind(),
                        target.crtc_id,
                        *frame_index,
                        error,
                    )
                })?
        } else {
            NativePageFlipDrain::default()
        };
        let pageflip_drain_us = elapsed_micros(pageflip_drain_start);
        *mismatched_pageflip_events =
            mismatched_pageflip_events.saturating_add(pageflip_drain.mismatched_events);
        *stale_pageflip_events = stale_pageflip_events.saturating_add(pageflip_drain.stale_events);
        if pageflip_drain.mismatched_events > 0 || pageflip_drain.stale_events > 0 {
            perf.log("native.pageflip_event_error", || {
                vec![
                    NativePerfField::u64("mismatched", pageflip_drain.mismatched_events),
                    NativePerfField::u64("stale", pageflip_drain.stale_events),
                    NativePerfField::u64(
                        "expected_token",
                        pageflip_drain.last_mismatch.map_or(0, |value| value.0),
                    ),
                    NativePerfField::u64(
                        "received_token",
                        pageflip_drain.last_mismatch.map_or(0, |value| value.1),
                    ),
                    NativePerfField::u64(
                        "stale_token",
                        pageflip_drain.last_stale_token.unwrap_or(0),
                    ),
                    NativePerfField::str("kms_backend", kms_backend.effective_kind().as_str()),
                    NativePerfField::u64("backend_generation", *drm_file_generation),
                ]
            });
        }
        let wrong_crtc_pageflip = pageflip_drain
            .completion
            .is_some_and(|event| event.crtc_id != target.crtc_id);
        if wrong_crtc_pageflip {
            *mismatched_pageflip_events = mismatched_pageflip_events.saturating_add(1);
        }
        let pageflip_event = pageflip_drain
            .completion
            .filter(|event| event.crtc_id == target.crtc_id);
        let (pageflip_event, atomic_completion, atomic_watchdog_kind) = validate_atomic_pageflip(
            atomic_commit_arbiter,
            kms_backend.effective_kind(),
            pageflip_event,
            *drm_file_generation,
            monotonic_now_ns()?,
            mismatched_pageflip_events,
            stale_pageflip_events,
        )?;
        if let Some(kind) = atomic_watchdog_kind {
            perf.log("native.atomic_commit_watchdog", || {
                vec![
                    NativePerfField::str("kind", format!("{kind:?}")),
                    NativePerfField::u64(
                        "token",
                        atomic_commit_arbiter
                            .pending_atomic_token()
                            .map_or(0, PageFlipToken::get),
                    ),
                    NativePerfField::u64("crtc", u64::from(target.crtc_id)),
                    NativePerfField::u64("generation", *drm_file_generation),
                    NativePerfField::bool("final_drain_completed", false),
                ]
            });
            acquire_watches.shutdown(event_loop)?;
            return Err(io::Error::other(
                "native Atomic commit watchdog expired; final DRM drain found no completion",
            )
            .into());
        }
        let pageflip_completed = pageflip_event.is_some();
        let mut completed_pageflip_token = None;
        let mut frame_completed = false;
        let frame_rendered = false;
        let frame_submitted = false;
        if let Some(pageflip) = pageflip_event {
            completed_pageflip_token = Some(pageflip.user_data);
            let cursor_commit = atomic_completion.is_some_and(|completion| {
                matches!(
                    completion,
                    AtomicCommitCompletion::Completed(AtomicCommitKind::CursorOnly { .. })
                )
            });
            if cursor_commit {
                // Cursor-only completion is not a complete compositor cycle.
                // Continue through protocol and input dispatch so a primary
                // producer can be observed and scheduled immediately.
                let _ = complete_cursor_only_pageflip(
                    atomic_cursor,
                    pageflip.user_data,
                    *drm_file_generation,
                    perf,
                )?;
            }
            let compositor_receive_ns = monotonic_now_ns()?;
            let scheduler_state_at_completion = frame_scheduler.state();
            let direct_pending = scanout.direct_scanout_pending();
            let completion = frame_scheduler
                .note_page_flip_completion(pageflip.user_data, compositor_receive_ns);
            if matches!(completion, PageFlipCompletionResult::Completed { .. })
                && let Some(token) = pageflip_drain.deferred_promotion_token
            {
                scanout.promote_page_flip(
                    PageFlipToken::new(token)
                        .ok_or_else(|| io::Error::other("pageflip promotion token is zero"))?,
                )?;
            }
            if let PageFlipCompletionResult::Completed { submitted_at_ns } = completion {
                let completed_frame_id = frame_pacing.pending;
                let presentation = if direct_pending {
                    FramePresentation::synchronized_zero_copy(
                        *presentation_clock,
                        pageflip.timestamp.seconds,
                        pageflip.timestamp.microseconds,
                        pageflip.sequence,
                    )?
                } else {
                    FramePresentation::synchronized(
                        *presentation_clock,
                        pageflip.timestamp.seconds,
                        pageflip.timestamp.microseconds,
                        pageflip.sequence,
                    )?
                };
                let compositor_receive_us = sample_clock_microseconds(*drm_timestamp_clock)?;
                let kernel_timestamp_us = u64::from(pageflip.timestamp.seconds)
                    .saturating_mul(1_000_000)
                    .saturating_add(u64::from(pageflip.timestamp.microseconds));
                let receive_delay_us = compositor_receive_us.saturating_sub(kernel_timestamp_us);
                let presented_at_ns =
                    compositor_receive_ns.saturating_sub(receive_delay_us.saturating_mul(1_000));
                *last_primary_presented_at_ns = Some(presented_at_ns);
                if direct_pending {
                    let completed = scanout.complete_direct_pageflip(
                        PageFlipToken::new(pageflip.user_data)
                            .ok_or_else(|| io::Error::other("pageflip token is zero"))?,
                        presentation,
                        server,
                    )?;
                    self.presentation_trace
                        .push(PresentationTransactionEvent::PageflipPresented {
                            transaction_id: completed.prepared.transaction_id,
                            timestamp_ns: compositor_receive_ns,
                        });
                    complete_primary_cursor_pageflip(
                        atomic_cursor,
                        pageflip.user_data,
                        *drm_file_generation,
                    )?;
                    let presented_at = MonotonicTimestampNs::new(presented_at_ns);
                    let actual_logical_sequence =
                        presentation_deadline.note_presented(presented_at);
                    render_journal.note_matching_presentation(presented_at);
                    frame_pacing.note_explicit_present(ExplicitPresentationObservation {
                        planned_sequence: completed.prepared.target.sequence,
                        actual_sequence: actual_logical_sequence,
                        target_ns: completed.prepared.target.presentation_time.get(),
                        presented_ns: presented_at_ns,
                        composite_started_ns: completed.submit_started_at.get(),
                        rendered_ns: completed.submit_returned_at.get(),
                        submit_started_ns: completed.submit_started_at.get(),
                        submit_returned_ns: completed.submit_returned_at.get(),
                        reactive_double: completed.prepared.target.reason
                            == PresentationTargetReason::ReactiveDouble,
                    });
                    *scheduled_presentation_target = None;
                } else if let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut **scanout {
                    if let Some(token) = output_render_fence_token.take() {
                        event_loop.unregister(token)?;
                    }
                    let completed = explicit.complete_pageflip(
                        PageFlipToken::new(pageflip.user_data)
                            .ok_or_else(|| io::Error::other("pageflip token is zero"))?,
                        presentation,
                        server,
                    )?;
                    complete_primary_cursor_pageflip(
                        atomic_cursor,
                        pageflip.user_data,
                        *drm_file_generation,
                    )?;
                    let presented_at = MonotonicTimestampNs::new(presented_at_ns);
                    let actual_logical_sequence =
                        presentation_deadline.note_presented(presented_at);
                    render_journal.note_matching_presentation(presented_at);
                    render_journal.record_atomic_submit(
                        completed
                            .submit_returned_at
                            .get()
                            .saturating_sub(completed.submit_started_at.get()),
                    );
                    let refresh = completed.target.refresh_interval;
                    let before_sample = render_journal.prediction(refresh);
                    let mut proven_miss = pending_proven_deadline_miss.take();
                    if let Some((signaled_at, quality)) = completed.fence_signal {
                        frame_pacing.note_fence_timestamp_quality(quality);
                        render_journal.record_render_sample(
                            render_sample_duration_ns(completed.composite_started_at, signaled_at),
                            signaled_at,
                        );
                        let target_ns = completed.target.presentation_time.get();
                        proven_miss = match quality {
                            FenceTimestampQuality::ExactSyncFile
                                if signaled_at.get() > target_ns =>
                            {
                                Some(ProvenDeadlineMiss::ExactRender)
                            }
                            FenceTimestampQuality::ObservedApproximate
                                if approximate_observation_is_late(
                                    signaled_at.get(),
                                    target_ns,
                                    before_sample.p95_wake_lateness_ns,
                                ) =>
                            {
                                Some(ProvenDeadlineMiss::GuardedApproximateRender)
                            }
                            _ => None,
                        };
                    }
                    if completed.submit_returned_at.get() > completed.target.presentation_time.get()
                    {
                        proven_miss = Some(ProvenDeadlineMiss::AtomicSubmit);
                    }
                    let prediction = render_journal.prediction(refresh);
                    if !scanout.third_slot_owned() {
                        let buffering_mode_before = adaptive_buffering.mode();
                        adaptive_buffering.observe(
                            prediction.total_cost_ns,
                            refresh,
                            proven_miss,
                            completed.target.sequence,
                            presented_at,
                            server.has_unowned_frame_work() || frame_scheduler.visual_work_queued(),
                        );
                        frame_pacing.note_adaptive_transition(
                            buffering_mode_before,
                            adaptive_buffering.mode(),
                            proven_miss,
                        );
                    }
                    frame_pacing.note_explicit_present(ExplicitPresentationObservation {
                        planned_sequence: completed.target.sequence,
                        actual_sequence: actual_logical_sequence,
                        target_ns: completed.target.presentation_time.get(),
                        presented_ns: presented_at_ns,
                        composite_started_ns: completed.composite_started_at.get(),
                        rendered_ns: completed.rendered_at.get(),
                        submit_started_ns: completed.submit_started_at.get(),
                        submit_returned_ns: completed.submit_returned_at.get(),
                        reactive_double: completed.target.reason
                            == PresentationTargetReason::ReactiveDouble,
                    });
                    *scheduled_presentation_target = None;
                } else {
                    complete_primary_cursor_pageflip(
                        atomic_cursor,
                        pageflip.user_data,
                        *drm_file_generation,
                    )?;
                    server.finish_frame_with_presentation(presentation);
                }
                frame_pacing.note_pageflip(
                    presented_at_ns,
                    submitted_at_ns,
                    pageflip.user_data,
                    1_000_000u64 / u64::from((*refresh_hz).max(1)),
                );
                let mut pacing_fields = vec![
                    frame_id_field(completed_frame_id),
                    PacingField::u64("render_generation", server.render_generation()),
                    PacingField::u64("pageflip_token", pageflip.user_data),
                    PacingField::u64("pageflip_complete_ns", presented_at_ns),
                ];
                pacing_fields.extend(snapshot_fields(scanout.buffer_snapshot()));
                frame_pacing.log("frame_complete", pacing_fields);
                let refresh_interval_us = 1_000_000u64 / u64::from((*refresh_hz).max(1));
                let cadence = presentation_cadence.record_with_refresh(
                    pageflip.sequence,
                    presented_at_ns / 1_000,
                    refresh_interval_us,
                );
                let finish_frame_start = Instant::now();
                if !server.has_unowned_frame_work() {
                    frame_scheduler.complete_protocol_only();
                }
                frame_completed = true;
                perf.log("native.finish_frame", || {
                    vec![
                        NativePerfField::str("reason", "pageflip_complete"),
                        NativePerfField::u64("elapsed_us", elapsed_micros(finish_frame_start)),
                        NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                        NativePerfField::u64("render_generation", server.render_generation()),
                        NativePerfField::u64("pageflip_token", pageflip.user_data),
                        NativePerfField::str("kms_backend", kms_backend.effective_kind().as_str()),
                        NativePerfField::u64("backend_generation", *drm_file_generation),
                        NativePerfField::u64("kernel_sequence", u64::from(pageflip.sequence)),
                        NativePerfField::u64("kernel_timestamp_us", kernel_timestamp_us),
                        NativePerfField::u64("presentations", cadence.presentations),
                        NativePerfField::u64(
                            "presentation_interval_us",
                            cadence.interval_us.unwrap_or(0),
                        ),
                        NativePerfField::u64(
                            "presentation_sequence_delta",
                            cadence.sequence_delta.map(u64::from).unwrap_or(0),
                        ),
                        NativePerfField::u64(
                            "logical_presentation_sequence",
                            cadence.logical_sequence,
                        ),
                        NativePerfField::u64(
                            "logical_presentation_sequence_delta",
                            cadence.logical_sequence_delta.unwrap_or(0),
                        ),
                        NativePerfField::bool(
                            "timestamp_sequence_fallback",
                            cadence.timestamp_sequence_fallback,
                        ),
                        NativePerfField::bool("presentation_sequence_gap", cadence.sequence_gap),
                        NativePerfField::u64(
                            "presented_hz_millihz",
                            cadence.estimated_hz_millihz.unwrap_or(0),
                        ),
                        NativePerfField::u64("presentation_sequence_gaps", cadence.sequence_gaps),
                        NativePerfField::u64("compositor_receive_us", compositor_receive_us),
                        NativePerfField::u64(
                            "receive_delay_us",
                            compositor_receive_us.saturating_sub(kernel_timestamp_us),
                        ),
                        NativePerfField::u64(
                            "submit_to_completion_us",
                            compositor_receive_ns.saturating_sub(submitted_at_ns) / 1_000,
                        ),
                        NativePerfField::str(
                            "scheduler_state",
                            format!("{scheduler_state_at_completion:?}"),
                        ),
                    ]
                });
            }
        }
        Ok(NativeCycleState {
            wakeup,
            pageflip_drain_us,
            pageflip_completed,
            completed_pageflip_token,
            frame_completed,
            frame_rendered,
            frame_submitted,
            present_us: 0,
            pageflip_pending_at_tick: false,
            tick_us: 0,
            accepted: 0,
            redraw_requested: false,
            skipped_input_repaints: 0,
            input_drain_us: 0,
            raw_input_events: 0,
            coalesced_input_events: 0,
            shutdown_requested: false,
        })
    }
    fn dispatch_runtime_seat_events(&mut self, wakeup: &NativeWakeup) -> NativeResult<()> {
        if !wakeup.reasons.seat() {
            return Ok(());
        }
        let Some(seat) = self.seat_session.clone() else {
            return Ok(());
        };
        NativeSessionIo::observe(self, NativeIoOperation::SeatDispatch);
        seat.dispatch()?;
        for event in seat.drain_events() {
            match self.session.begin_for_event(event) {
                Some(NativeSessionTransition::BeginSuspend) => {
                    self.suspend_native_session(&seat)?
                }
                Some(NativeSessionTransition::BeginResume) if self.shutdown.is_running() => {
                    self.resume_native_session()?
                }
                Some(NativeSessionTransition::BeginResume) => {
                    self.session.cancel_resume_for_shutdown();
                    self.log_session_transition(
                        "suspended",
                        "suspended",
                        "enable_ignored_after_shutdown",
                    )
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn suspend_native_session(&mut self, seat: &NativeSeatSession) -> NativeResult<()> {
        self.log_session_transition("active", "suspending", "seat_disable");
        self.perf.log("native.session_suspend", || {
            vec![
                NativePerfField::str("pageflip_policy", "quarantine_until_recovery_modeset"),
                NativePerfField::bool("pageflip_pending", self.scanout.page_flip_pending()),
                NativePerfField::str("explicit_sync_policy", "park_and_rearm"),
            ]
        });
        quiesce_and_acknowledge(self, |io| {
            if seat.acknowledge_disable()? {
                io.observe(NativeIoOperation::SeatDisableAcknowledged);
                Ok(())
            } else {
                Err(io::Error::other("stale libseat disable acknowledgment").into())
            }
        })?;
        self.session.finish_suspend();
        self.log_session_transition("suspending", "suspended", "disable_acknowledged");
        self.event_loop
            .arm_deadline(self.shutdown.suspended_reactor_deadline_ns())?;
        Ok(())
    }
    fn resume_native_session(&mut self) -> NativeResult<()> {
        self.log_session_transition("suspended", "resuming", "seat_enable");
        let result = recover_native_output(self);
        if let Err(error) = result {
            if let Some(token) = self.drm_reactor_token.take() {
                let _ = self.event_loop.unregister(token);
            }
            if let Ok(parked) = self
                .acquire_watches
                .park_for_session_suspend(&mut self.event_loop)
            {
                self.parked_acquire_watches.extend(parked);
            }
            if let Some(mut cursor) = self.legacy_cursor.take() {
                cursor.disarm_drm_cleanup();
            }
            self.pending_session_recovery = None;
            teardown_without_drm_io(self);
            self.session.fail_resume();
            self.log_session_transition("resuming", "failed", "recovery_failed");
            return Err(error);
        }
        self.session.finish_resume();
        self.log_session_transition("resuming", "active", "output_recovered");
        Ok(())
    }
    pub(super) fn rearm_parked_acquire_watches(&mut self) -> NativeResult<()> {
        let now_ns = monotonic_now_ns()?;
        let parked = std::mem::take(&mut self.parked_acquire_watches);
        let already_ready = match self.acquire_watches.rearm_parked_requests(
            parked,
            &mut self.event_loop,
            now_ns,
            &self.acquire_notifier,
        ) {
            Ok(already_ready) => already_ready,
            Err(failure) => {
                let (error, parked) = failure.into_parts();
                self.parked_acquire_watches = parked;
                return Err(error.into());
            }
        };
        for request in already_ready {
            let _ = self.server.mark_acquire_commit_ready(
                request.commit_id,
                request.surface_id,
                &request.acquire,
            );
        }
        Ok(())
    }
    fn dispatch_suspended_sources(&mut self, cycle: &NativeCycleState) -> NativeResult<()> {
        service_suspended_sources(
            self,
            NativeSuspendedReadiness {
                wayland: cycle.wakeup.reasons.wayland_listener()
                    || cycle.wakeup.reasons.wayland_clients(),
                input: cycle.wakeup.reasons.input(),
                drm: cycle.wakeup.reasons.drm(),
                timer: cycle.wakeup.reasons.timer(),
                explicit_sync: cycle.wakeup.reasons.explicit_sync_acquire(),
                redraw: false,
                cursor: false,
            },
        )
    }
    fn log_session_transition(&self, from: &str, to: &str, reason: &str) {
        self.perf.log("native.session_transition", || {
            vec![
                NativePerfField::str("from", from),
                NativePerfField::str("to", to),
                NativePerfField::str("reason", reason),
                NativePerfField::bool("pageflip_pending", self.scanout.page_flip_pending()),
                NativePerfField::str("shutdown_state", self.shutdown.state().as_str()),
                NativePerfField::str("drm_backend", self.kms.kind().as_str()),
                NativePerfField::str("input_backend", self.input_devices.kind().as_str()),
            ]
        });
    }
    #[allow(unused_variables)]
    fn dispatch_wayland_and_input(&mut self, cycle: &mut NativeCycleState) -> NativeResult<()> {
        if cycle.wakeup.reasons.input() {
            NativeSessionIo::observe(self, NativeIoOperation::RawInputAction);
        }
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
            cursor_render_mode,
            atomic_cursor,
            legacy_cursor,
            input_devices,
            acquire_notifier,
            acquire_watches,
            parked_acquire_watches: _,
            event_loop,
            drm_reactor_token: _,
            frame_scheduler,
            effective_app_gpu_policy,
            last_renderable_surfaces,
            queued_redraw_requested,
            frame_index,
            known_toplevels,
            pending_launches,
            mismatched_pageflip_events,
            stale_pageflip_events,
            presentation_cadence: _,
            last_acquire_ready_at_ns,
            resize_perf,
            pointer_constraint_backend,
            seat_session,
            process_supervisor,
            shutdown: _,
            session: _,
            ..
        } = self;
        let present_us = 0;
        let pageflip_pending_at_tick = scanout.page_flip_pending();
        let tick_start = Instant::now();
        let accepted = server.tick()?;
        let tick_us = elapsed_micros(tick_start);
        let mut redraw_requested = process_native_pointer_constraint_backend_requests(
            server,
            pointer_constraint_backend,
            input_state,
            *cursor_render_mode,
        )?;
        synchronize_cursor_state_for_server(server, atomic_cursor, legacy_cursor, input_state)?;
        let current_toplevels = server.xdg_toplevels();
        if current_toplevels > *known_toplevels {
            for _ in *known_toplevels..current_toplevels {
                let app_id = server.last_app_id().unwrap_or("unknown").to_string();
                if let Some(launch) = pending_launches.pop_front() {
                    perf.log("app.first_toplevel", || {
                        vec![
                            NativePerfField::str("program", launch.program.clone()),
                            NativePerfField::str("command", launch.command.clone()),
                            NativePerfField::str("source", launch.source.as_str()),
                            NativePerfField::u64("pid", u64::from(launch.pid)),
                            NativePerfField::str("app_id", app_id.clone()),
                            NativePerfField::u64("spawn_us", launch.spawn_us),
                            NativePerfField::u64("elapsed_us", elapsed_micros(launch.started_at)),
                            NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                        ]
                    });
                } else {
                    perf.log("app.toplevel", || {
                        vec![
                            NativePerfField::str("app_id", app_id.clone()),
                            NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                            NativePerfField::usize("total_toplevels", current_toplevels),
                        ]
                    });
                }
            }
            *known_toplevels = current_toplevels;
        }
        if accepted > 0 {
            println!(
                "accepted {accepted} client(s); total {}",
                server.accepted_clients()
            );
        }
        let mut skipped_input_repaints = 0usize;
        let input_drain_start = Instant::now();
        let raw_events = input_devices.drain_events();
        let input_drain_us = elapsed_micros(input_drain_start);
        let raw_input_events = raw_events.len();
        let input_event_timestamp_usec = matches!(
            input_devices.kind(),
            NativeInputBackendKind::LibseatLibinputUdev
                | NativeInputBackendKind::DirectLibinputUdev
        )
        .then(|| {
            raw_events
                .iter()
                .filter_map(|event| event.timestamp_usec())
                .max()
        })
        .flatten();
        let coalesced_events = coalesce_pointer_motion_events(raw_events);
        let coalesced_input_events = coalesced_events.len();
        for (event_index, event) in coalesced_events.into_iter().enumerate() {
            let may_change_pointer_constraints = event.may_change_pointer_constraints();
            let effect = input_state.handle_hardware_input_event(event);
            let effect_requested_redraw = effect.redraw_requested;
            let cursor_visible = server.client_cursor_render_state().is_some()
                || server.interaction_cursor_override_active()
                || input_state.cursor_visible();
            if let Err(error) = apply_cursor_position(
                atomic_cursor,
                legacy_cursor,
                effect.cursor_position,
                cursor_visible,
                *cursor_preference,
                cursor_render_mode,
                perf,
            ) {
                if *cursor_preference == NativeCursorPreference::Hardware {
                    acquire_watches.shutdown(event_loop)?;
                    return Err(error.into());
                }
                return Err(error.into());
            }
            let application = apply_native_input_effect(
                effect,
                NativeInputApplyContext {
                    server,
                    perf,
                    resize_perf,
                    cursor_mode: *cursor_render_mode,
                    app_gpu_policy: *effective_app_gpu_policy,
                    seat_session: seat_session.as_ref(),
                    process_supervisor,
                },
            )?;
            if application.exit_requested {
                cycle.shutdown_requested = true;
                break;
            }
            if let Some(launch) = application.launch {
                log_native_app_spawn(perf, &launch);
                pending_launches.push_back(launch);
            }
            if effect_requested_redraw && !application.redraw_requested {
                skipped_input_repaints = skipped_input_repaints.saturating_add(1);
            }
            redraw_requested |= application.redraw_requested;
            let interaction_reconciled = reconcile_trigger_liveness(
                server,
                input_state,
                &format!("event_index={event_index}"),
            );
            redraw_requested |= interaction_reconciled;
            if may_change_pointer_constraints {
                let _ = server.tick()?;
                redraw_requested |= process_native_pointer_constraint_backend_requests(
                    server,
                    pointer_constraint_backend,
                    input_state,
                    *cursor_render_mode,
                )?;
                synchronize_cursor_state_for_server(
                    server,
                    atomic_cursor,
                    legacy_cursor,
                    input_state,
                )?;
            }
        }
        let interaction_reconciled = reconcile_trigger_liveness(server, input_state, "batch_end");
        redraw_requested |= interaction_reconciled;
        redraw_requested |= process_native_pointer_constraint_backend_requests(
            server,
            pointer_constraint_backend,
            input_state,
            *cursor_render_mode,
        )?;
        synchronize_cursor_state_for_server(server, atomic_cursor, legacy_cursor, input_state)?;
        if let Some(event_timestamp_us) = input_event_timestamp_usec {
            let dispatch_latency_us = monotonic_now_ns()?
                .saturating_div(1_000)
                .saturating_sub(event_timestamp_us);
            perf.log("native.input_dispatch", || {
                vec![
                    NativePerfField::usize("events", coalesced_input_events),
                    NativePerfField::u64("event_timestamp_us", event_timestamp_us),
                    NativePerfField::u64("dispatch_latency_us", dispatch_latency_us),
                ]
            });
        }
        cycle.present_us = present_us;
        cycle.pageflip_pending_at_tick = pageflip_pending_at_tick;
        cycle.tick_us = tick_us;
        cycle.accepted = accepted;
        cycle.redraw_requested = redraw_requested;
        cycle.skipped_input_repaints = skipped_input_repaints;
        cycle.input_drain_us = input_drain_us;
        cycle.raw_input_events = raw_input_events;
        cycle.coalesced_input_events = coalesced_input_events;
        Ok(())
    }

    #[allow(unused_variables)]
    fn process_acquire_and_prepare(&mut self, cycle: &NativeCycleState) -> NativeResult<()> {
        let wakeup = &cycle.wakeup;
        NativeSessionIo::observe(self, NativeIoOperation::ExplicitSyncNotifier);
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
            cursor_render_mode,
            legacy_cursor,
            input_devices,
            acquire_notifier,
            acquire_watches,
            parked_acquire_watches: _,
            event_loop,
            drm_reactor_token: _,
            frame_scheduler,
            effective_app_gpu_policy,
            last_renderable_surfaces,
            queued_redraw_requested,
            frame_index,
            known_toplevels,
            pending_launches,
            mismatched_pageflip_events,
            stale_pageflip_events,
            presentation_cadence: _,
            last_acquire_ready_at_ns,
            resize_perf,
            pointer_constraint_backend,
            seat_session: _,
            process_supervisor: _,
            shutdown: _,
            session: _,
            ..
        } = self;
        let acquire_changes = server.take_acquire_watch_changes();
        let acquire_change_count = acquire_changes.len();
        let acquire_ready_token_count = wakeup.explicit_sync_acquire_tokens.len();
        let mut acquire_ready_count = 0usize;
        for change in acquire_changes {
            match change {
                AcquireWatchChange::Register(request) => {
                    match acquire_watches.register(
                        request,
                        event_loop,
                        monotonic_now_ns()?,
                        acquire_notifier,
                    )? {
                        AcquireRegistrationResult::AlreadyReady(request) => {
                            if server.mark_acquire_commit_ready(
                                request.commit_id,
                                request.surface_id,
                                &request.acquire,
                            ) {
                                acquire_ready_count = acquire_ready_count.saturating_add(1);
                            }
                        }
                        AcquireRegistrationResult::EventfdBacked(commit_id) => {
                            let _ = server.mark_acquire_commit_eventfd_backed(commit_id);
                        }
                        AcquireRegistrationResult::FallbackBacked(commit_id) => {
                            let _ = server.mark_acquire_commit_fallback_backed(commit_id);
                        }
                    }
                }
                AcquireWatchChange::Cancel { commit_id, reason } => {
                    let _ = acquire_watches.cancel_commit(commit_id, reason, event_loop)?;
                }
            }
        }
        for token in wakeup.explicit_sync_acquire_tokens.iter().copied() {
            match acquire_watches.handle_ready(
                token,
                event_loop,
                *drm_file_generation,
                acquire_notifier,
            )? {
                AcquireReadyResult::Ready(request) => {
                    if server.mark_acquire_commit_ready(
                        request.commit_id,
                        request.surface_id,
                        &request.acquire,
                    ) {
                        acquire_ready_count = acquire_ready_count.saturating_add(1);
                    }
                }
                AcquireReadyResult::BackendMismatch(_) => {}
                AcquireReadyResult::Pending | AcquireReadyResult::Stale => {}
            }
        }
        for request in acquire_watches.retry_fallback(monotonic_now_ns()?, acquire_notifier) {
            if server.mark_acquire_commit_ready(
                request.commit_id,
                request.surface_id,
                &request.acquire,
            ) {
                acquire_ready_count = acquire_ready_count.saturating_add(1);
            }
        }
        if acquire_change_count > 0 || acquire_ready_token_count > 0 || acquire_ready_count > 0 {
            if acquire_ready_count > 0 {
                *last_acquire_ready_at_ns = Some(monotonic_now_ns()?);
            }
            let metrics = acquire_watches.metrics();
            perf.log("native.explicit_sync", || {
                vec![
                    NativePerfField::usize("changes", acquire_change_count),
                    NativePerfField::usize("ready_tokens", acquire_ready_token_count),
                    NativePerfField::usize("ready_commits", acquire_ready_count),
                    NativePerfField::usize(
                        "active_eventfd_watches",
                        metrics.active_eventfd_watches,
                    ),
                    NativePerfField::usize(
                        "active_fallback_watches",
                        metrics.active_fallback_watches,
                    ),
                    NativePerfField::u64("registrations", metrics.registrations),
                    NativePerfField::u64("already_signaled", metrics.already_signaled),
                    NativePerfField::u64("eventfd_wakeups", metrics.eventfd_wakeups),
                    NativePerfField::u64("stale_wakeups", metrics.stale_wakeups),
                    NativePerfField::u64("duplicate_wakeups", metrics.duplicate_wakeups),
                    NativePerfField::u64("cancellations", metrics.cancellations),
                    NativePerfField::u64("registration_failures", metrics.registration_failures),
                    NativePerfField::u64(
                        "last_registration_errno",
                        metrics.last_registration_errno.max(0) as u64,
                    ),
                    NativePerfField::u64(
                        "commit_to_acquire_ready_us",
                        metrics.last_commit_to_ready_ns / 1_000,
                    ),
                    NativePerfField::u64("fallback_activations", metrics.fallback_activations),
                    NativePerfField::usize(
                        "maximum_simultaneous_watches",
                        metrics.maximum_simultaneous_watches,
                    ),
                    NativePerfField::u64(
                        "leaked_watch_assertions",
                        metrics.leaked_watch_assertions,
                    ),
                    NativePerfField::u64("canceled_superseded", metrics.cancellations_by_reason[0]),
                    NativePerfField::u64(
                        "canceled_surface_destroyed",
                        metrics.cancellations_by_reason[1],
                    ),
                    NativePerfField::u64(
                        "canceled_buffer_destroyed",
                        metrics.cancellations_by_reason[2],
                    ),
                    NativePerfField::u64(
                        "canceled_sync_surface_destroyed",
                        metrics.cancellations_by_reason[3],
                    ),
                    NativePerfField::u64(
                        "canceled_timeline_destroyed",
                        metrics.cancellations_by_reason[4],
                    ),
                    NativePerfField::u64(
                        "canceled_client_disconnected",
                        metrics.cancellations_by_reason[5],
                    ),
                    NativePerfField::u64(
                        "canceled_role_destroyed",
                        metrics.cancellations_by_reason[8],
                    ),
                ]
            });
        }
        if server.has_pending_frame_prepare_work() {
            let prepare_frame_start = Instant::now();
            let before_generation = server.render_generation();
            server.prepare_frame();
            let after_generation = server.render_generation();
            let resize = server.resize_flow_metrics();
            let subsurface = server.subsurface_transaction_metrics();
            perf.log("native.prepare_frame", || {
                vec![
                    NativePerfField::u64("elapsed_us", elapsed_micros(prepare_frame_start)),
                    NativePerfField::u64("render_generation", after_generation),
                    NativePerfField::bool("render_changed", after_generation != before_generation),
                    NativePerfField::bool("pending_frame_work", server.has_unowned_frame_work()),
                    NativePerfField::u64(
                        "resize_configures_requested",
                        resize.configures_requested,
                    ),
                    NativePerfField::u64("resize_configures_sent", resize.configures_sent),
                    NativePerfField::u64(
                        "resize_geometries_coalesced",
                        resize.geometries_coalesced,
                    ),
                    NativePerfField::u64("resize_acks_matched", resize.acks_matched),
                    NativePerfField::u64("resize_acks_stale", resize.acks_stale),
                    NativePerfField::u64("resize_acks_unknown", resize.acks_unknown),
                    NativePerfField::u64("resize_commits_captured", resize.commits_captured),
                    NativePerfField::u64(
                        "resize_interactions_started",
                        resize.resize_interactions_started,
                    ),
                    NativePerfField::u64(
                        "resize_rapid_reresize_interactions",
                        resize.rapid_reresize_interactions,
                    ),
                    NativePerfField::u64(
                        "resize_obsolete_finals_discarded",
                        resize.obsolete_finals_discarded,
                    ),
                    NativePerfField::u64(
                        "resize_obsolete_queued_targets_discarded",
                        resize.obsolete_queued_targets_discarded,
                    ),
                    NativePerfField::u64(
                        "resize_obsolete_in_flight_configures_discarded",
                        resize.obsolete_in_flight_configures_discarded,
                    ),
                    NativePerfField::u64(
                        "resize_stale_interaction_commits_applied",
                        resize.stale_interaction_commits_applied,
                    ),
                    NativePerfField::u64(
                        "resize_stale_commits_preserved_preview",
                        resize.stale_commits_preserved_preview,
                    ),
                    NativePerfField::u64(
                        "resize_preview_ownership_transfers",
                        resize.preview_ownership_transfers,
                    ),
                    NativePerfField::u64(
                        "resize_final_configures_sent",
                        resize.final_configures_sent,
                    ),
                    NativePerfField::u64(
                        "resize_interactions_completed",
                        resize.resize_interactions_completed,
                    ),
                    NativePerfField::u64(
                        "resize_interactions_canceled",
                        resize.resize_interactions_canceled,
                    ),
                    NativePerfField::u64(
                        "resize_visual_geometry_starts",
                        resize.visual_geometry_resize_starts,
                    ),
                    NativePerfField::u64(
                        "resize_raw_pointer_updates",
                        resize.raw_pointer_resize_updates,
                    ),
                    NativePerfField::u64(
                        "resize_pending_updates_replaced",
                        resize.pending_resize_updates_replaced,
                    ),
                    NativePerfField::u64("resize_updates_applied", resize.resize_updates_applied),
                    NativePerfField::u64(
                        "resize_updates_skipped_unchanged",
                        resize.resize_updates_skipped_unchanged,
                    ),
                    NativePerfField::u64(
                        "resize_duplicate_configures_skipped",
                        resize.duplicate_configure_sizes_skipped,
                    ),
                    NativePerfField::usize(
                        "resize_max_retained_configures",
                        resize.maximum_retained_configures,
                    ),
                    NativePerfField::u64("resize_preview_max_age_ms", resize.max_preview_age_ms),
                    NativePerfField::usize("resize_max_in_flight", resize.max_in_flight_configures),
                    NativePerfField::usize(
                        "resize_max_pending_explicit_sync",
                        resize.max_pending_explicit_sync_commits,
                    ),
                    NativePerfField::u64(
                        "subsurface_commits_cached",
                        subsurface.synchronized_child_commits_cached,
                    ),
                    NativePerfField::u64(
                        "subsurface_commits_merged",
                        subsurface.cached_commits_merged,
                    ),
                    NativePerfField::u64(
                        "subsurface_trees_published",
                        subsurface.tree_transactions_published,
                    ),
                    NativePerfField::u64(
                        "subsurface_trees_waiting_acquire",
                        subsurface.tree_transactions_waiting_on_acquire,
                    ),
                    NativePerfField::u64(
                        "subsurface_bufferless_tree_commits_merged",
                        subsurface.bufferless_tree_commits_merged,
                    ),
                    NativePerfField::u64(
                        "subsurface_metadata_only_nodes_merged",
                        subsurface.metadata_only_nodes_merged,
                    ),
                    NativePerfField::u64(
                        "subsurface_attachments_replaced",
                        subsurface.attachments_replaced,
                    ),
                    NativePerfField::u64(
                        "subsurface_explicit_detaches",
                        subsurface.explicit_detaches,
                    ),
                    NativePerfField::u64(
                        "subsurface_acquire_dependencies_preserved",
                        subsurface.acquire_dependencies_preserved,
                    ),
                    NativePerfField::u64(
                        "subsurface_acquire_dependencies_replaced",
                        subsurface.acquire_dependencies_replaced,
                    ),
                    NativePerfField::u64(
                        "subsurface_ready_preserved_from_newer_unready",
                        subsurface.ready_transactions_preserved_from_newer_unready,
                    ),
                    NativePerfField::u64(
                        "subsurface_ready_preserved_from_newer_ready",
                        subsurface.ready_transactions_preserved_from_newer_ready,
                    ),
                    NativePerfField::u64(
                        "explicit_sync_queue_overflow",
                        subsurface.explicit_sync_queue_overflow,
                    ),
                    NativePerfField::u64(
                        "subsurface_callbacks_merged",
                        subsurface.callbacks_merged,
                    ),
                    NativePerfField::u64(
                        "subsurface_feedbacks_merged",
                        subsurface.feedbacks_merged,
                    ),
                    NativePerfField::u64(
                        "subsurface_resize_snapshots_preserved",
                        subsurface.resize_snapshots_preserved,
                    ),
                    NativePerfField::u64(
                        "subsurface_resize_snapshots_replaced",
                        subsurface.resize_snapshots_replaced,
                    ),
                    NativePerfField::u64(
                        "subsurface_root_wide_supersessions",
                        subsurface.root_wide_supersessions,
                    ),
                    NativePerfField::u64(
                        "subsurface_waiting_transactions_published",
                        subsurface.waiting_transactions_published,
                    ),
                    NativePerfField::usize(
                        "subsurface_max_ready_slots_per_root",
                        subsurface.maximum_ready_slots_per_root,
                    ),
                    NativePerfField::usize(
                        "subsurface_max_waiting_slots_per_root",
                        subsurface.maximum_waiting_slots_per_root,
                    ),
                    NativePerfField::usize(
                        "subsurface_max_cached_nodes",
                        subsurface.maximum_cached_nodes,
                    ),
                    NativePerfField::usize(
                        "subsurface_max_tree_depth",
                        subsurface.maximum_tree_depth,
                    ),
                    NativePerfField::u64(
                        "subsurface_max_wait_ms",
                        subsurface.maximum_transaction_wait_ms,
                    ),
                ]
            });
        }
        Ok(())
    }
}
fn reconcile_trigger_liveness(
    server: &mut OwnCompositorServer,
    input_state: &NativeInputState,
    after_event: &str,
) -> bool {
    let Some(snapshot) = server.window_interaction_debug_snapshot() else {
        return false;
    };
    let trigger_pressed = snapshot
        .trigger_button
        .is_none_or(|button| input_state.is_pointer_button_pressed(button));
    if let Some(trigger_button) = snapshot.trigger_button
        && !trigger_pressed
    {
        resize_debug_log(|| {
            format!(
                "event=trigger_mismatch interaction_id={} trigger_button={} physical_pressed=false pressed_buttons={:?} after_event={after_event}",
                snapshot.interaction_id,
                trigger_button,
                input_state.pressed_pointer_buttons_snapshot(),
            )
        });
    };
    server.reconcile_window_interaction_trigger(trigger_pressed)
}
