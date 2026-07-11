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
        self.reap_supervised_children(&cycle)?;
        self.advance_shutdown_lifecycle(&cycle)?;
        if !self.session.permits_output() {
            self.dispatch_suspended_sources(&cycle)?;
            return Ok(());
        }
        if !self.shutdown.is_running() {
            return Ok(());
        }
        self.dispatch_wayland_and_input(&mut cycle)?;
        if cycle.shutdown_requested {
            self.request_native_shutdown()?;
        }
        if !self.shutdown.is_running() || !self.session.permits_output() {
            return Ok(());
        }
        drain_pending_process_launches(
            &mut self.server,
            &mut self.process_supervisor,
            &mut self.astrea_launch_tracker,
            self.effective_app_gpu_policy,
            self.perf,
            &mut self.pending_launches,
        );
        self.process_acquire_and_prepare(&cycle)?;
        if !self.shutdown.is_running() || !self.session.permits_output() {
            return Ok(());
        }
        self.render_present_and_update_metrics(&mut cycle)?;
        Ok(())
    }

    fn reap_supervised_children(&mut self, cycle: &NativeCycleState) -> NativeResult<()> {
        self.astrea_launch_tracker.prune_dead();
        if !cycle.wakeup.reasons.child_signal()
            && self.shutdown.state() != ShutdownState::StoppingChildren
        {
            return Ok(());
        }
        for exit in self.process_supervisor.reap_exited()? {
            let finished_status = astrea_launch_finished_status(exit.status);
            self.perf.log("process.exit", || {
                vec![
                    NativePerfField::str("kind", exit.kind.as_str()),
                    NativePerfField::u64("pid", u64::from(exit.pid)),
                    NativePerfField::str("exit_code", finished_status.to_string()),
                    NativePerfField::u64("restarted_pid", exit.restarted_pid.map_or(0, u64::from)),
                ]
            });
            if self.astrea_launch_tracker.complete(exit.pid, exit.status) {
                self.perf.log("shell_control.finished", || {
                    vec![
                        NativePerfField::u64("pid", u64::from(exit.pid)),
                        NativePerfField::str("status", finished_status.to_string()),
                    ]
                });
            }
        }
        Ok(())
    }

    fn request_native_shutdown(&mut self) -> NativeResult<()> {
        let now_ns = monotonic_now_ns()?;
        let pending_pageflip_token = self
            .scanout
            .pending_page_flip_token()
            .or_else(|| self.frame_scheduler.pending_page_flip_token());
        match self
            .shutdown
            .request_shutdown(now_ns, pending_pageflip_token)
        {
            Some(transition) => {
                native_shutdown_debug_log("shortcut_exit_requested");
                native_shutdown_debug_log("shutdown_begin");
                println!("native input exit requested; shutting down cleanly");
                self.process_supervisor.begin_quiesce();
                self.log_shutdown_transition(transition);
            }
            None => {
                native_shutdown_debug_log("shortcut_exit_requested_duplicate");
            }
        }
        self.advance_shutdown_lifecycle_without_cycle()
    }

    fn advance_shutdown_lifecycle_without_cycle(&mut self) -> NativeResult<()> {
        let cycle = NativeCycleState {
            wakeup: NativeWakeup {
                reasons: Default::default(),
                ready_sources: 0,
                blocked_ns: 0,
                timer_lateness_ns: None,
                explicit_sync_acquire_tokens: Vec::new(),
            },
            pageflip_drain_us: 0,
            pageflip_completed: false,
            completed_pageflip_token: None,
            frame_completed: false,
            frame_rendered: false,
            frame_submitted: false,
            present_us: 0,
            pageflip_pending_at_tick: self.scanout.page_flip_pending(),
            tick_us: 0,
            accepted: 0,
            redraw_requested: false,
            skipped_input_repaints: 0,
            input_drain_us: 0,
            raw_input_events: 0,
            coalesced_input_events: 0,
            shutdown_requested: false,
        };
        self.advance_shutdown_lifecycle(&cycle)
    }

    fn advance_shutdown_lifecycle(&mut self, cycle: &NativeCycleState) -> NativeResult<()> {
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

    fn restore_kms_for_shutdown(&mut self) -> NativeResult<()> {
        let Some(reason) = self.shutdown.begin_kms_restore() else {
            return Ok(());
        };
        native_shutdown_debug_log("input_backend_stop");
        self.acquire_watches.shutdown(&mut self.event_loop)?;
        if !self.session.permits_output() {
            teardown_without_drm_io(self);
            self.parked_acquire_watches.clear();
            self.perf.log("native.shutdown_session", || {
                vec![NativePerfField::str(
                    "action",
                    "skip_kms_restore_while_seat_inactive",
                )]
            });
            if let Some(transition) = self.shutdown.note_kms_restore_complete() {
                self.log_shutdown_transition(transition);
            }
            return Ok(());
        }
        if let Some(cursor) = self.hardware_cursor.as_mut() {
            let _ = cursor.disable();
        }
        native_shutdown_debug_log("kms_restore_begin");
        NativeSessionIo::observe(self, NativeIoOperation::KmsRestore);
        let restoration = self.kms_backend.restore()?;
        native_shutdown_debug_log("kms_restore_end");
        self.perf.log("native.kms_restore", || {
            vec![
                NativePerfField::str("backend", self.kms_backend.effective_kind().as_str()),
                NativePerfField::str("outcome", restoration.as_str()),
                NativePerfField::str("shutdown_pageflip", reason.as_str()),
                NativePerfField::bool("pageflip_pending", self.scanout.page_flip_pending()),
            ]
        });
        native_shutdown_debug_log("drm_release");
        native_shutdown_debug_log("vt_restore");
        if let Some(transition) = self.shutdown.note_kms_restore_complete() {
            self.log_shutdown_transition(transition);
        }
        self.event_loop.arm_deadline(None)?;
        Ok(())
    }

    fn arm_shutdown_deadline(&mut self) -> NativeResult<()> {
        self.event_loop
            .arm_deadline(self.shutdown.pageflip_deadline_ns())?;
        Ok(())
    }

    fn log_shutdown_transition(&self, transition: ShutdownTransition) {
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
                || (wakeup.reasons.timer() && self.scanout.page_flip_pending()))
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
            hardware_cursor,
            input_devices,
            seat_session: _,
            session: _,
            acquire_notifier,
            acquire_watches,
            parked_acquire_watches: _,
            event_loop,
            drm_reactor_token: _,
            frame_scheduler,
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
            perf.log("native.deadline", || {
                vec![
                    NativePerfField::u64(
                        "lateness_us",
                        wakeup.timer_lateness_ns.unwrap_or(0) / 1_000,
                    ),
                    NativePerfField::str("scheduler_state", format!("{scheduler_state_before:?}")),
                    NativePerfField::bool("pageflip_watchdog", frame_scheduler.page_flip_pending()),
                ]
            });
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
                    || shutdown.state() == ShutdownState::Draining));
        let pageflip_drain = if should_drain_pageflips {
            scanout
                .drain_page_flip_events(kms.file().as_raw_fd())
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
        let pageflip_completed = pageflip_drain.completion.is_some();
        let mut completed_pageflip_token = None;
        let mut frame_completed = false;
        let frame_rendered = false;
        let frame_submitted = false;
        if let Some(pageflip) = pageflip_drain.completion {
            completed_pageflip_token = Some(pageflip.user_data);
            let compositor_receive_ns = monotonic_now_ns()?;
            let scheduler_state_at_completion = frame_scheduler.state();
            let completion = frame_scheduler
                .note_page_flip_completion(pageflip.user_data, compositor_receive_ns);
            if let PageFlipCompletionResult::Completed { submitted_at_ns } = completion {
                let presentation = FramePresentation::synchronized(
                    *presentation_clock,
                    pageflip.timestamp.seconds,
                    pageflip.timestamp.microseconds,
                    pageflip.sequence,
                )?;
                let compositor_receive_us = sample_clock_microseconds(*drm_timestamp_clock)?;
                let kernel_timestamp_us = u64::from(pageflip.timestamp.seconds)
                    .saturating_mul(1_000_000)
                    .saturating_add(u64::from(pageflip.timestamp.microseconds));
                let cadence = presentation_cadence.record(pageflip.sequence, kernel_timestamp_us);
                let finish_frame_start = Instant::now();
                server.finish_frame_with_presentation(presentation);
                if !server.has_pending_frame_work() {
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
            if let Some(mut cursor) = self.hardware_cursor.take() {
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
            hardware_cursor,
            input_devices,
            acquire_notifier,
            acquire_watches,
            parked_acquire_watches: _,
            event_loop,
            drm_reactor_token: _,
            frame_scheduler,
            effective_app_gpu_policy,
            last_render_generation,
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
            hardware_cursor,
            *cursor_render_mode,
        )?;
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
        for event in coalesced_events {
            let may_change_pointer_constraints = event.may_change_pointer_constraints();
            let effect = input_state.handle_hardware_input_event(event);
            let effect_requested_redraw = effect.redraw_requested;
            if let Some((cursor_x, cursor_y)) = effect.cursor_position
                && let Some(cursor) = hardware_cursor.as_mut()
                && let Err(error) = cursor.move_to(cursor_x, cursor_y)
            {
                if *cursor_preference == NativeCursorPreference::Hardware {
                    acquire_watches.shutdown(event_loop)?;
                    return Err(error.into());
                }
                eprintln!("native cursor: hardware cursor move failed: {error}; using software");
                *hardware_cursor = None;
                *cursor_render_mode = NativeCursorRenderMode::Software;
                perf.log("native.cursor", || {
                    vec![
                        NativePerfField::str("backend", cursor_render_mode.as_str()),
                        NativePerfField::str("policy", cursor_preference.as_str()),
                        NativePerfField::str("fallback", "move_failed"),
                        NativePerfField::str("error", error.to_string()),
                    ]
                });
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
            if may_change_pointer_constraints {
                let _ = server.tick()?;
                redraw_requested |= process_native_pointer_constraint_backend_requests(
                    server,
                    pointer_constraint_backend,
                    input_state,
                    hardware_cursor,
                    *cursor_render_mode,
                )?;
            }
        }
        redraw_requested |= process_native_pointer_constraint_backend_requests(
            server,
            pointer_constraint_backend,
            input_state,
            hardware_cursor,
            *cursor_render_mode,
        )?;
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
            hardware_cursor,
            input_devices,
            acquire_notifier,
            acquire_watches,
            parked_acquire_watches: _,
            event_loop,
            drm_reactor_token: _,
            frame_scheduler,
            effective_app_gpu_policy,
            last_render_generation,
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
                ]
            });
        }
        if !scanout.page_flip_pending() && server.has_pending_frame_prepare_work() {
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
                    NativePerfField::bool("pending_frame_work", server.has_pending_frame_work()),
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
