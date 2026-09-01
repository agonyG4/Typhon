use super::*;

#[path = "cycle_direct.rs"]
mod cycle_direct;
#[path = "cycle/fallback.rs"]
pub(super) mod direct_fallback;
#[path = "cycle/pageflip.rs"]
mod pageflip;
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct AcquirePrepareOutcome {
    acquire_service_ran: bool,
    acquire_state_changed: bool,
    frame_prepare_ran: bool,
    visual_work_created: bool,
}

impl NativeRuntime {
    fn queued_visual_work_deadline_due(&self, now_ns: u64) -> bool {
        let scheduled_target_due = self
            .scheduled_presentation_target
            .is_some_and(|target| target.render_start_deadline.get() <= now_ns);
        if self.frame_scheduler.visual_work_queued() {
            scheduled_target_due || self.scheduled_presentation_target.is_none()
        } else {
            self.queued_redraw_requested
        }
    }

    fn native_runtime_state(&self, cycle: &NativeCycleState, now_ns: u64) -> NativeRuntimeState {
        NativeRuntimeState {
            scene_dirty: cycle.redraw_requested || self.server.has_pending_frame_prepare_work(),
            visual_work_deadline_due: self.queued_visual_work_deadline_due(now_ns),
            cursor_only_due: self.cursor_output_arbitration.pending()
                && self.cursor_output_arbitration.due(now_ns),
            explicit_sync_service_due: cycle.wakeup.reasons.explicit_sync_acquire()
                || !cycle.wakeup.explicit_sync_acquire_tokens.is_empty()
                || self.server.has_pending_acquire_watch_changes()
                || (cycle.wakeup.reasons.timer()
                    && self
                        .acquire_watches
                        .next_fallback_deadline_ns()
                        .is_some_and(|deadline| deadline <= now_ns)),
            astrea_publication_due: cycle.wakeup.reasons.timer()
                && self.server.has_pending_astrea_toplevel_publication(),
            commit_timing_planning_due: cycle.wakeup.reasons.timer()
                && self.server.has_pending_commit_timing_planning(),
            pacing_active: self.server.has_surface_pacing_work(),
            pacing_due: self.should_progress_surface_pacing(now_ns),
            xwayland_generation_changed: self.xwayland.reactor_registration_generation()
                != self.xwayland_reactor_generation,
            recovery_required: self.pending_session_recovery.is_some(),
            shutdown_requested: cycle.shutdown_requested,
            input_backlog_pending: self.input_epoch.backlog_pending(),
        }
    }

    pub(super) fn should_progress_surface_pacing(&self, now_ns: u64) -> bool {
        self.server.has_surface_pacing_readiness_pending()
            || self
                .server
                .next_surface_pacing_deadline_ns()
                .is_some_and(|deadline| deadline <= now_ns)
    }

    fn plan_pending_commit_timing(&mut self, now_ns: u64) {
        if !self.server.has_pending_commit_timing_planning() {
            return;
        }
        let predicted_total_cost = Duration::from_nanos(
            self.render_journal
                .prediction_with_kms_guard(
                    Duration::from_nanos(self.presentation_timing.mode().refresh_interval_ns()),
                    self.presentation_timing.apply_guard_ns(),
                )
                .total_cost_ns,
        );
        self.scheduled_presentation_target = super::planner::plan_commit_timing_target(
            &mut self.presentation_deadline,
            &mut self.server,
            &self.frame_scheduler,
            self.scheduled_presentation_target,
            MonotonicTimestampNs::new(now_ns),
            predicted_total_cost,
        );
    }

    pub(super) fn run_native_cycle(&mut self) -> NativeResult<()> {
        while !self.shutdown.is_complete() {
            self.run_cycle()?;
        }
        native_shutdown_debug_log("shutdown_complete");
        Ok(())
    }

    fn run_cycle(&mut self) -> NativeResult<()> {
        let mut cycle = self.wait_for_events_and_pageflips()?;
        let now_ns = monotonic_now_ns()?;
        self.service_due_dmabuf_release_retry(now_ns)?;
        let runtime_state = self.native_runtime_state(&cycle, now_ns);
        let work_domains = NativeWorkDomains::classify(&cycle.wakeup, &runtime_state);
        let operation_plan = work_domains.operation_plan();
        let wayland_client_work = work_domains.wayland_protocol;
        let xwayland_work = work_domains.xwayland;
        let work_decision = NativeWorkDomains::from_wakeup(&cycle.wakeup, &runtime_state);
        cycle.work_class = work_decision.work_class;
        {
            let metrics = self.resource_efficiency_mut();
            metrics.record_native_cycle();
            if cycle.wakeup.reasons.input() {
                metrics.record_input_ready();
            }
            metrics.record_work_decision(work_decision);
            if operation_plan.service_input && !operation_plan.dispatch_wayland_read_side {
                metrics.record_input_only_cycle();
            }
            if operation_plan.dispatch_wayland_read_side {
                metrics.record_wayland_read_dispatch_cycle();
            }
        }
        self.server.set_commit_debug_pageflip_pending(
            self.scanout.page_flip_pending() || self.atomic_commit_arbiter.atomic_commit_pending(),
        );
        if work_domains.children || self.shutdown.state() == ShutdownState::StoppingChildren {
            self.reap_supervised_children(&cycle)?;
        }
        if work_domains.xwayland {
            let xwm_drain_started = Instant::now();
            self.dispatch_xwayland_events(&cycle.wakeup)?;
            self.note_timing_scope("xwm_dispatch", xwm_drain_started.elapsed());
            if self.xwayland.generation().is_some() {
                self.attach_xwayland_private_client()?;
            } else {
                self.revoke_xwayland_private_client();
            }
            self.sync_xwayland_reactor_sources()?;
        }
        if cycle.wakeup.reasons.timer() {
            self.control_server.expire_idle_clients(
                &mut self.event_loop,
                monotonic_now_ns()?,
                oblivion_one::native::control::MAX_CONTROL_OPERATIONS_PER_CYCLE,
            );
            self.xwayland
                .handle_deadline(monotonic_now_ns()?, &mut self.process_supervisor)?;
            if self.xwayland.generation().is_none() {
                self.revoke_xwayland_private_client();
            }
            self.sync_xwayland_reactor_sources()?;
        }
        self.advance_shutdown_lifecycle(&cycle)?;
        if !self.session.permits_output() {
            if work_domains.wayland_dispatch
                || work_domains.astrea_publication
                || work_domains.commit_timing_planning
                || work_domains.presentation
                || work_domains.surface_pacing
                || work_domains.explicit_sync
            {
                self.dispatch_suspended_sources(&cycle)?;
            }
            work_domains.service_surface_pacing_if_due(|| -> NativeResult<bool> {
                self.resource_efficiency_mut()
                    .record_surface_pacing_service_run();
                self.server
                    .progress_surface_pacing(monotonic_now_ns()?)
                    .map_err(Into::into)
            })?;
            if !self.shutdown.is_running() {
                self.quiesce_control_server()?;
                return Ok(());
            }
            if work_domains.control {
                self.service_control_events(&cycle.wakeup)?;
            }
            if work_domains.cursor {
                self.service_cursor_io_completions(&cycle.wakeup)?;
            }
            self.arm_suspended_deadline()?;
            return Ok(());
        }
        if !self.shutdown.is_running() {
            self.quiesce_control_server()?;
            return Ok(());
        }
        let wayland_dispatch_started = Instant::now();
        let wayland_pacing_readiness_changed =
            if operation_plan.dispatch_wayland_read_side || operation_plan.service_input {
                self.dispatch_wayland_and_input(
                    &mut cycle,
                    operation_plan.service_input,
                    operation_plan.dispatch_wayland_read_side,
                )?
            } else {
                false
            };
        if work_domains.commit_timing_planning
            || (work_domains.wayland_dispatch && self.server.has_pending_commit_timing_planning())
        {
            self.plan_pending_commit_timing(now_ns);
        }
        if work_domains.astrea_publication && !work_domains.wayland_dispatch {
            self.server.service_pending_astrea_toplevel_updates();
            self.server.flush_wayland_clients()?;
            self.resource_efficiency_mut().record_client_flush();
        }
        let pacing_visual_work = if work_domains
            .should_service_surface_pacing_after_wayland(wayland_pacing_readiness_changed)
        {
            self.resource_efficiency_mut()
                .record_surface_pacing_service_run();
            self.server.progress_surface_pacing(monotonic_now_ns()?)?
        } else {
            false
        };
        cycle.redraw_requested |= pacing_visual_work;
        let post_service_state = self.native_runtime_state(&cycle, monotonic_now_ns()?);
        cycle.work_class =
            NativeWorkDomains::from_wakeup(&cycle.wakeup, &post_service_state).work_class;
        if work_domains.wayland_dispatch {
            self.note_timing_scope("wayland_dispatch", wayland_dispatch_started.elapsed());
        } else if work_domains.input {
            self.note_timing_scope("input_dispatch", wayland_dispatch_started.elapsed());
        }
        if work_domains.control {
            self.service_control_events(&cycle.wakeup)?;
        }
        if work_domains.cursor {
            self.service_cursor_io_completions(&cycle.wakeup)?;
        }
        let xwayland_scene_work = wayland_client_work || xwayland_work;
        if xwayland_scene_work {
            self.dispatch_xwayland_client_disconnects()?;
            self.dispatch_xwayland_shell_binds()?;
            self.initialize_managed_xwayland()?;
            cycle.redraw_requested |= self.dispatch_xwayland_scene_batch()?;
            self.sync_xwayland_reactor_sources()?;
        }
        if cycle.shutdown_requested {
            self.request_native_shutdown()?;
        }
        if !self.shutdown.is_running() || !self.session.permits_output() {
            if !self.shutdown.is_running() {
                self.quiesce_control_server()?;
            }
            return Ok(());
        }
        if work_domains.wayland_dispatch || work_domains.control || xwayland_scene_work {
            drain_pending_process_launches_with_xwayland_environment_and_cursor(
                &mut self.server,
                &mut self.process_supervisor,
                &mut self.astrea_launch_tracker,
                self.effective_app_gpu_policy,
                self.perf,
                &mut self.pending_launches,
                self.xwayland.normal_app_environment(),
                Some(self.cursor_manager.desired_configuration()),
            );
        }
        let commit_timing_planning_generation_before_prepare =
            self.server.commit_timing_planning_generation();
        let prepare_operation_plan = NativeWorkDomains::classify(
            &cycle.wakeup,
            &self.native_runtime_state(&cycle, monotonic_now_ns()?),
        )
        .operation_plan();
        let prepare_outcome = if prepare_operation_plan.service_acquire_and_prepare {
            self.resource_efficiency_mut().record_acquire_prepare_run();
            if prepare_operation_plan.explicit_sync_service {
                self.resource_efficiency_mut()
                    .record_explicit_sync_service_run();
            }
            let prepare_started = Instant::now();
            let outcome = self.process_acquire_and_prepare(
                &cycle,
                prepare_operation_plan.explicit_sync_service,
            )?;
            self.note_timing_scope("prepare_frame", prepare_started.elapsed());
            outcome
        } else {
            self.resource_efficiency_mut().record_acquire_prepare_skip();
            AcquirePrepareOutcome::default()
        };
        if prepare_outcome.frame_prepare_ran {
            self.resource_efficiency_mut().record_frame_prepare_run();
        }
        if self.server.commit_timing_planning_generation()
            != commit_timing_planning_generation_before_prepare
            && self.server.has_pending_commit_timing_planning()
        {
            self.resource_efficiency_mut()
                .record_commit_timing_planning_replan();
            self.plan_pending_commit_timing(monotonic_now_ns()?);
        }
        if !self.shutdown.is_running() || !self.session.permits_output() {
            return Ok(());
        }
        let presentation_operation_plan = NativeWorkDomains::classify(
            &cycle.wakeup,
            &self.native_runtime_state(&cycle, monotonic_now_ns()?),
        )
        .operation_plan();
        let presentation_work = presentation_operation_plan.presentation_admitted(
            cycle.redraw_requested,
            cycle.frame_completed,
            prepare_outcome.visual_work_created,
        );
        if presentation_work {
            let render_started = Instant::now();
            self.resource_efficiency_mut()
                .record_presentation_planning_run();
            self.render_present_and_update_metrics(&mut cycle)?;
            let metrics = self.resource_efficiency_mut();
            match cycle.work_class {
                NativeWorkClass::PrimaryScene => {
                    if cycle.frame_rendered {
                        metrics.record_primary_scene_render();
                    }
                    if cycle.frame_submitted {
                        metrics.record_primary_scene_submit();
                    }
                }
                NativeWorkClass::CursorOnly if cycle.frame_submitted => {
                    metrics.record_cursor_only_submit();
                }
                NativeWorkClass::NoOutputWork | NativeWorkClass::ProtocolOnly => {}
                NativeWorkClass::CursorOnly => {}
            }
            self.note_timing_scope("egl_draw", render_started.elapsed());
        } else {
            self.resource_efficiency_mut()
                .record_presentation_planning_skip();
        }
        if !presentation_work || self.input_epoch.backlog_pending() {
            self.arm_runtime_deadline()?;
        }
        cycle.fast_path_completed = !prepare_outcome.acquire_service_ran && !presentation_work;
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
                    let now_ns = monotonic_now_ns()?;
                    let control_timeout_deadline = self.control_server.next_deadline_ns();
                    let plan = build_native_wake_plan(NativeWakePlanInputs {
                        now_ns,
                        scheduler_deadline: Some(NativeDeadline {
                            owner: NativeDeadlineOwner::ControlTimeout,
                            at_ns: now_ns.saturating_add(50_000_000),
                        }),
                        control_timeout_deadline_ns: control_timeout_deadline
                            .filter(|deadline| *deadline > now_ns),
                        surface_pacing_deadline_ns: (!self
                            .server
                            .has_surface_pacing_readiness_pending())
                        .then(|| self.server.next_surface_pacing_deadline_ns())
                        .flatten(),
                        ..NativeWakePlanInputs::default()
                    });
                    self.install_native_wake_plan(plan, now_ns)?;
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
        let now_ns = monotonic_now_ns()?;
        let control_timeout_deadline = self.control_server.next_deadline_ns();
        let plan = build_native_wake_plan(NativeWakePlanInputs {
            now_ns,
            scheduler_deadline: self
                .shutdown
                .pageflip_deadline_ns()
                .map(|at_ns| NativeDeadline {
                    owner: NativeDeadlineOwner::AtomicCommitWatchdog,
                    at_ns,
                }),
            control_timeout_deadline_ns: control_timeout_deadline
                .filter(|deadline| *deadline > now_ns),
            surface_pacing_deadline_ns: (!self.server.has_surface_pacing_readiness_pending())
                .then(|| self.server.next_surface_pacing_deadline_ns())
                .flatten(),
            control_timeout_pending: control_timeout_deadline.is_some_and(|deadline| deadline <= now_ns),
            ..NativeWakePlanInputs::default()
        });
        self.install_native_wake_plan(plan, now_ns)
    }

    fn arm_suspended_deadline(&mut self) -> NativeResult<()> {
        let now_ns = monotonic_now_ns()?;
        let control_timeout_deadline = self.control_server.next_deadline_ns();
        let plan = build_native_wake_plan(NativeWakePlanInputs {
            now_ns,
            scheduler_deadline: self.shutdown.suspended_reactor_deadline_ns().map(|at_ns| {
                NativeDeadline {
                    owner: NativeDeadlineOwner::ControlTimeout,
                    at_ns,
                }
            }),
            control_timeout_deadline_ns: control_timeout_deadline
                .filter(|deadline| *deadline > now_ns),
            surface_pacing_deadline_ns: (!self.server.has_surface_pacing_readiness_pending())
                .then(|| self.server.next_surface_pacing_deadline_ns())
                .flatten(),
            input_backlog: self.input_epoch.backlog_pending(),
            astrea_publication: self.server.has_pending_astrea_toplevel_publication(),
            commit_timing_planning: self.server.has_pending_commit_timing_planning(),
            control_timeout_pending: control_timeout_deadline.is_some_and(|deadline| deadline <= now_ns),
            ..NativeWakePlanInputs::default()
        });
        self.install_native_wake_plan(plan, now_ns)
    }

    fn quiesce_control_server(&mut self) -> NativeResult<()> {
        self.control_server.shutdown(&mut self.event_loop)?;
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
        self.arm_suspended_deadline()?;
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
                input: cycle.wakeup.reasons.input()
                    || cycle
                        .wakeup
                        .continuation
                        .contains(NativeContinuationReason::InputBacklog),
                drm: cycle.wakeup.reasons.drm(),
                timer: cycle.wakeup.reasons.timer(),
                explicit_sync: cycle.wakeup.reasons.explicit_sync_acquire(),
                redraw: false,
                cursor: false,
            },
        )?;
        if (cycle.wakeup.reasons.timer()
            || cycle
                .wakeup
                .continuation
                .contains(NativeContinuationReason::AstreaPublication))
            && self.server.has_pending_astrea_toplevel_publication()
        {
            self.server.service_pending_astrea_toplevel_updates();
            self.server.flush_wayland_clients()?;
            self.resource_efficiency_mut().record_client_flush();
        }
        Ok(())
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
    fn process_acquire_and_prepare(
        &mut self,
        cycle: &NativeCycleState,
        service_explicit_sync: bool,
    ) -> NativeResult<AcquirePrepareOutcome> {
        let wakeup = &cycle.wakeup;
        let mut outcome = AcquirePrepareOutcome {
            acquire_service_ran: service_explicit_sync,
            ..AcquirePrepareOutcome::default()
        };
        if service_explicit_sync {
            NativeSessionIo::observe(self, NativeIoOperation::ExplicitSyncNotifier);
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
            legacy_cursor,
            input_devices,
            acquire_notifier,
            acquire_watches,
            parked_acquire_watches: _,
            event_loop,
            drm_reactor_token: _,
            frame_scheduler,
            effective_app_gpu_policy,
            scene_history: _,
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
        let acquire_changes = if service_explicit_sync {
            server.take_acquire_watch_changes()
        } else {
            Default::default()
        };
        let acquire_change_count = acquire_changes.len();
        let acquire_ready_token_count = if service_explicit_sync {
            wakeup.explicit_sync_acquire_tokens.len()
        } else {
            0
        };
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
        if service_explicit_sync {
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
        }
        if service_explicit_sync {
            for request in acquire_watches.retry_fallback(monotonic_now_ns()?, acquire_notifier) {
                if server.mark_acquire_commit_ready(
                    request.commit_id,
                    request.surface_id,
                    &request.acquire,
                ) {
                    acquire_ready_count = acquire_ready_count.saturating_add(1);
                }
            }
        }
        outcome.acquire_state_changed = acquire_change_count > 0 || acquire_ready_count > 0;
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
            outcome.frame_prepare_ran = true;
            let prepare_frame_start = Instant::now();
            let before_scene_generation = server.scene_render_generation();
            let before_generation = server.render_generation();
            server.prepare_frame();
            let after_scene_generation = server.scene_render_generation();
            outcome.visual_work_created = after_scene_generation != before_scene_generation;
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
                    NativePerfField::u64(
                        "resize_configure_capacity_blocked",
                        resize.resize_configure_capacity_blocked,
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
        Ok(outcome)
    }
}
