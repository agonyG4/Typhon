use super::*;
use crate::native_output::screen_capture::{CaptureDimensions, deliver_screen_capture_payload};

const MAX_SCREEN_CAPTURES_FAILED_PER_TURN: usize = 16;

pub(super) const fn screen_capture_delivery_is_allowed(
    shutdown_running: bool,
    session_permits_output: bool,
) -> bool {
    shutdown_running && session_permits_output
}

pub(super) const fn screen_capture_waits_for_render_worker(
    render_worker_active: bool,
    render_worker_busy: bool,
) -> bool {
    render_worker_active && render_worker_busy
}

pub(super) const fn screen_capture_backlog_needs_immediate_continuation(
    capture_pending: bool,
    render_worker_active: bool,
    render_worker_busy: bool,
) -> bool {
    capture_pending
        && !screen_capture_waits_for_render_worker(render_worker_active, render_worker_busy)
}

pub(super) const fn process_launch_backlog_needs_immediate_continuation(
    process_launch_backlog_pending: bool,
    shutdown_running: bool,
) -> bool {
    process_launch_backlog_pending && shutdown_running
}

#[path = "cycle_direct.rs"]
mod cycle_direct;
#[path = "cycle/fallback.rs"]
pub(super) mod direct_fallback;
#[path = "cycle/pageflip.rs"]
mod pageflip;
pub(super) use pageflip::abandon_overtaken_ready;
pub(crate) use pageflip::recover_or_arm_suspended_output_ownership;
pub(super) use pageflip::terminalize_suspended_frame_for_variable_retry;
pub(super) use pageflip::terminalize_unsubmitted_frame_for_reconfiguration;
#[cfg(test)]
pub(crate) use pageflip::validate_reconfiguration_frame_ownership_for_swapchain;
#[cfg(test)]
pub(crate) use pageflip::validate_reconfiguration_frame_ownership_for_swapchain_with_arbiter;
pub(crate) use pageflip::validate_reconfiguration_frame_ownership_with_arbiter;
pub(crate) use pageflip::{
    ExplicitWorkerReplanRequest, preflight_explicit_worker_replan, restore_explicit_worker_replan,
};
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

#[cfg(test)]
mod dmem_shutdown_tests {
    use super::NativeRuntime;

    #[test]
    fn shutdown_request_disables_new_foreground_targets() {
        assert!(NativeRuntime::foreground_dmem_is_eligible(true, true));
        assert!(!NativeRuntime::foreground_dmem_is_eligible(true, false));
        assert!(!NativeRuntime::foreground_dmem_is_eligible(false, true));
    }
}

#[cfg(test)]
mod screen_capture_lifecycle_tests {
    use super::super::{session::NativeSessionState, shutdown::NativeShutdownLifecycle};
    use super::*;

    #[test]
    fn screen_capture_delivery_requires_running_shutdown_and_active_session() {
        let shutdown = NativeShutdownLifecycle::new();

        assert!(screen_capture_delivery_is_allowed(
            shutdown.is_running(),
            NativeSessionState::Active.permits_output(),
        ));
        for session in [
            NativeSessionState::Suspending,
            NativeSessionState::Suspended,
            NativeSessionState::Resuming,
            NativeSessionState::Failed,
        ] {
            assert!(!screen_capture_delivery_is_allowed(
                shutdown.is_running(),
                session.permits_output(),
            ));
        }
    }

    #[test]
    fn shutdown_request_rejects_and_terminalizes_pending_capture_before_progress() {
        let mut shutdown = NativeShutdownLifecycle::new();
        shutdown.request_shutdown(1, None).unwrap();
        let mut pending_captures = MAX_SCREEN_CAPTURES_FAILED_PER_TURN + 1;
        let mut terminal_captures = 0;
        let mut backlog_continuations = 0;

        assert!(!screen_capture_delivery_is_allowed(
            shutdown.is_running(),
            NativeSessionState::Active.permits_output(),
        ));
        assert!(!process_launch_backlog_needs_immediate_continuation(
            true,
            shutdown.is_running(),
        ));
        while pending_captures > 0 {
            let failed_this_turn = pending_captures.min(MAX_SCREEN_CAPTURES_FAILED_PER_TURN);
            pending_captures -= failed_this_turn;
            terminal_captures += failed_this_turn;
            backlog_continuations += usize::from(pending_captures > 0);
        }

        assert_eq!(terminal_captures, MAX_SCREEN_CAPTURES_FAILED_PER_TURN + 1);
        assert_eq!(backlog_continuations, 1);
        assert_eq!(
            shutdown.advance_requested().unwrap().to,
            ShutdownState::StoppingChildren
        );
    }

    #[test]
    fn busy_render_worker_keeps_capture_pending_without_immediate_continuation() {
        let render_worker_busy = screen_capture_waits_for_render_worker(true, true);

        assert!(render_worker_busy);
        assert!(!screen_capture_backlog_needs_immediate_continuation(
            true, true, true,
        ));
        assert!(screen_capture_backlog_needs_immediate_continuation(
            true, true, false,
        ));
        assert!(!screen_capture_backlog_needs_immediate_continuation(
            false, false, false,
        ));
    }

    #[test]
    fn repeated_capture_backlog_advances_one_request_per_turn() {
        let mut pending_captures = 128usize;
        let mut serviced_requests = 0;
        let mut continuations = 0;

        while pending_captures > 0 {
            pending_captures -= 1;
            serviced_requests += 1;
            continuations += usize::from(screen_capture_backlog_needs_immediate_continuation(
                pending_captures > 0,
                false,
                false,
            ));
        }

        assert_eq!(serviced_requests, 128);
        assert_eq!(continuations, 127);
        assert_eq!(pending_captures, 0);
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct AcquirePrepareOutcome {
    acquire_service_ran: bool,
    acquire_state_changed: bool,
    frame_prepare_ran: bool,
    visual_work_created: bool,
}

impl NativeRuntime {
    /// Admit a matured Auto cursor service before potentially expensive scene
    /// acquire/prepare work. This uses the ordinary arbiter and worker path;
    /// it does not resolve effects, render, or create a second KMS authority.
    fn service_matured_auto_cursor_before_prepare(
        &mut self,
        cycle: &NativeCycleState,
    ) -> NativeResult<bool> {
        if self.cursor_scheduling_policy != NativeCursorSchedulingPolicy::Auto
            || self.head.kms_commit_worker_transport
                != crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker
            || !self.shutdown.is_running()
            || !self.session.permits_output()
            || self.head.pending_output_configuration.is_some()
            || self.head.scanout.ready_frame_queued()
            || self.head.atomic_commit_arbiter.atomic_commit_pending()
        {
            return Ok(false);
        }
        let Some(cursor) = self.head.atomic_cursor.as_mut() else {
            return Ok(false);
        };
        if self.head.cursor_render_mode != NativeCursorRenderMode::Hardware
            || cursor.pending_token().is_some()
            || !self.head.cursor_output_arbitration.pending()
            || !cursor.needs_submission_for(Some(cursor.desired()))
            || (cursor.desired().visible && cursor.desired().framebuffer_id.is_none())
        {
            return Ok(false);
        }
        let Some(worker) = self.head.kms_commit_worker.as_ref() else {
            return Ok(false);
        };

        let now_ns = monotonic_now_ns()?;
        let now = MonotonicTimestampNs::new(now_ns);
        let pending_target = super::planner::pending_fixed_target_for_scanout(&self.head.scanout)?;
        let cursor_dispatch_budget_ns = worker.dispatch_budget_ns(
            crate::native_output::kms_worker::KmsServiceClass::CursorPlaneDelta,
            self.head.output_epoch.get(),
        );
        let candidate = super::planner::plan_runtime_cursor_service_opportunity(
            &self.head.presentation_deadline,
            &self.head.cursor_timing,
            now,
            super::planner::cursor_primary_ordering_target(
                self.head.scheduled_presentation_target,
                pending_target,
            ),
            cursor_dispatch_budget_ns,
            self.head.cursor_output_arbitration.service_opportunity(),
            crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker,
            self.head
                .confirmed_kms_presentation
                .vrr_request_confirmed_for(self.head.output_epoch.get()),
        );
        if let Some(candidate) = candidate {
            if self.head.cursor_output_arbitration.pending()
                && self.head.cursor_output_arbitration.desired_epoch() == cursor.desired_epoch()
            {
                self.head
                    .cursor_output_arbitration
                    .reconcile_pending_service_opportunity(candidate, now_ns);
            } else {
                self.head
                    .cursor_output_arbitration
                    .request_hardware_for_opportunity(cursor.desired_epoch(), now_ns, candidate);
            }
        } else if !self
            .head
            .cursor_output_arbitration
            .planning_retry_ns()
            .is_some_and(|retry_ns| retry_ns > now_ns)
        {
            // Do not fall through to a stale physical target when current
            // presentation authorities cannot produce a valid opportunity.
            self.head
                .cursor_output_arbitration
                .request_hardware_for_planning_retry(
                    cursor.desired_epoch(),
                    now_ns,
                    self.head.frame_scheduler.next_refresh_deadline_ns(now_ns),
                );
        }
        let Some(opportunity) = self.head.cursor_output_arbitration.service_opportunity() else {
            return Ok(false);
        };
        let attachable_primary = worker.attachable_primary(
            self.head.output_epoch.get(),
            self.head.target.crtc_id,
            opportunity.target,
        );
        let independent_kms_admission = worker.admission_available()
            && worker.queue_depth() == 0
            && !worker.submission_active()
            && !worker.inflight();
        let queued_kind = self.head.atomic_commit_arbiter.worker_queued_kind();
        let submitted_kind = self.head.atomic_commit_arbiter.kernel_submitted_kind();
        let primary_state = NativeCursorPrimaryState {
            primary_scene_work_pending: self.queued_redraw_requested
                || self.server.has_pending_frame_prepare_work()
                || self.head.frame_scheduler.visual_work_queued(),
            ready_primary_carrier_attachable: attachable_primary.is_some(),
            primary_target_owned: self.head.scanout.ready_frame_queued(),
            primary_worker_queued: queued_kind.is_some_and(|kind| {
                matches!(
                    kind,
                    AtomicCommitKind::CompositedPrimary { .. }
                        | AtomicCommitKind::DirectPrimary { .. }
                )
            }),
            primary_kernel_submitted: submitted_kind.is_some_and(|kind| {
                matches!(
                    kind,
                    AtomicCommitKind::CompositedPrimary { .. }
                        | AtomicCommitKind::DirectPrimary { .. }
                )
            }),
            independent_kms_admission,
            physical_admission: self
                .head
                .cursor_output_arbitration
                .physical_admission_for_transport(
                    crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker,
                    now_ns,
                ),
        };
        let desired = cursor.desired().clone();
        let hardware_usable =
            !desired.visible || desired.framebuffer_id.is_some() || cursor.current().visible;
        let disposition = super::frame::cursor_output_disposition_for_primary_state(
            &mut self.head.cursor_output_arbitration,
            NativeCursorSchedulingPolicy::Auto,
            now_ns,
            primary_state,
            hardware_usable,
        );
        self.cursor_responsiveness.record_arbitration_evidence(
            cycle.cursor_motion_sample_id,
            self.head.output_id,
            Some(cursor.desired_epoch()),
            now_ns,
            NativeCursorSchedulingPolicy::Auto,
            &self.head.cursor_output_arbitration,
            primary_state,
            Some(opportunity),
            disposition,
        );
        if disposition != super::frame::NativeCursorOutputDisposition::SubmitPlaneDelta {
            return Ok(false);
        }

        let cursor_epoch = cursor.desired_epoch();
        let mut effective_cursor = Some(desired.clone());
        let validation_base = crate::native_output::kms_worker::KmsValidationBase::Presented {
            output_id: self.head.output_id,
            snapshot: self.head.presented_planes,
            output_generation: self.head.output_epoch.get(),
            crtc_id: self.head.target.crtc_id,
        };
        let cursor_pending_since_ns = self.head.cursor_output_arbitration.pending_since_ns();
        let decision = super::presentation_worker::present_cursor_for_presentation(
            true,
            Some(worker),
            &self.head.kms_backend,
            cursor,
            &mut self.head.cursor_timing,
            Some(desired),
            &mut self.head.atomic_commit_arbiter,
            &mut self.head.output_transactions,
            &mut self.presentation_trace,
            opportunity.target,
            Some(opportunity.submit_window),
            self.head.target.crtc_id,
            self.head.output_epoch.get(),
            super::planner::pacing_mode_for_target(Some(opportunity.target)),
            cursor_epoch,
            validation_base,
            &mut self.head.last_submitted_cursor_epoch,
            &mut self.head.cursor_output_arbitration,
            &mut self.head.frame_scheduler,
            now_ns,
            self.perf,
            false,
            &mut self.head.cursor_render_mode,
            &mut self.server,
            &mut effective_cursor,
            &mut self.queued_redraw_requested,
            &mut self.head.last_client_cursor_damage,
            &mut self.head.last_software_cursor_damage,
            None,
            None,
            None,
            &mut self.cursor_reveal_trace,
            self.head.confirmed_kms_presentation,
        )?;
        if decision.is_some() {
            self.head
                .vrr_cadence
                .note_cursor_plane_delta_submitted(cursor_pending_since_ns);
            self.arm_runtime_deadline()?;
            return Ok(true);
        }
        // Admission can become unavailable between the lane snapshot and the
        // worker reservation. Back off until the next refresh while retaining
        // the original freshness deadline and pending cursor age.
        self.head
            .cursor_output_arbitration
            .set_retry_not_before(self.head.frame_scheduler.next_refresh_deadline_ns(now_ns));
        Ok(false)
    }

    fn queued_visual_work_deadline_due(&self, now_ns: u64) -> bool {
        let target = self
            .head
            .scheduled_presentation_target
            .or(self.head.render_latch_target);
        if self.head.frame_scheduler.visual_work_queued() {
            target.is_none_or(|target| target.render_start_deadline.get() <= now_ns)
        } else {
            self.queued_redraw_requested
        }
    }

    fn native_runtime_state(&self, cycle: &NativeCycleState, now_ns: u64) -> NativeRuntimeState {
        let visual_scene_debt = super::commit_timing::logical_scene_changed(
            self.head.last_rendered_scene_generation,
            self.server.scene_render_generation(),
        );
        NativeRuntimeState {
            scene_dirty: cycle.redraw_requested
                || self.server.has_pending_frame_prepare_work()
                || self.server.has_pending_gpu_screencopy()
                || self.head.scanout.has_staged_gpu_screencopy(),
            visual_scene_debt,
            callback_only_frame_work_pending: self
                .server
                .has_only_pending_surface_frame_callbacks(),
            visual_work_deadline_due: self.queued_visual_work_deadline_due(now_ns),
            cursor_only_due: self.head.cursor_output_arbitration.pending()
                && self.head.cursor_output_arbitration.due(now_ns),
            screen_capture_pending: self.server.has_pending_screen_capture(),
            explicit_sync_service_due: cycle.wakeup.reasons.explicit_sync_acquire()
                || !cycle.wakeup.explicit_sync_acquire_tokens.is_empty()
                || self.server.has_pending_acquire_watch_changes()
                || (cycle.wakeup.reasons.timer()
                    && self
                        .acquire_watches
                        .next_fallback_deadline_ns()
                        .is_some_and(|deadline| deadline <= now_ns)),
            astrea_publication_due: cycle
                .wakeup
                .continuation
                .contains(NativeContinuationReason::AstreaPublication)
                || (cycle.wakeup.reasons.timer()
                    && self.server.has_pending_astrea_toplevel_publication()),
            commit_timing_planning_due: cycle
                .wakeup
                .continuation
                .contains(NativeContinuationReason::CommitTimingPlanning)
                || (cycle.wakeup.reasons.timer()
                    && self.server.has_pending_commit_timing_planning()),
            pacing_active: self.server.has_surface_pacing_work(),
            pacing_due: self.should_progress_surface_pacing(now_ns),
            xwayland_generation_changed: self.xwayland.reactor_registration_generation()
                != self.xwayland_reactor_generation,
            xwayland_backend_commands_pending: !cycle.shutdown_requested
                && self.xwayland.generation().is_some()
                && self.server.has_pending_xwayland_backend_commands(),
            recovery_required: self.pending_session_recovery.is_some(),
            shutdown_requested: cycle.shutdown_requested,
            input_backlog_pending: self.input_epoch.backlog_pending(),
            keyboard_repeat_due: !cycle.shutdown_requested
                && self.input_state.keyboard_repeat_due(now_ns),
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
        super::planner::plan_commit_timing_targets(
            &self.head.presentation_deadline,
            &mut self.server,
            MonotonicTimestampNs::new(now_ns),
        );
    }

    pub(super) fn run_native_cycle(&mut self) -> NativeResult<()> {
        oblivion_one::native::scheduling_qos::promote_current_reactor()?;
        while !self.shutdown.is_complete() {
            self.termination.set_phase(RuntimePhase::RuntimeCycle);
            oblivion_one::native::scheduling_qos::observe_rt_budget_event();
            oblivion_one::native::scheduling_qos::observe_rt_worker_budget_events();
            self.run_cycle()?;
        }
        oblivion_one::native::scheduling_qos::demote_current_reactor();
        self.termination.set_phase(RuntimePhase::Complete);
        native_shutdown_debug_log("shutdown_complete");
        Ok(())
    }

    fn run_cycle(&mut self) -> NativeResult<()> {
        let (mut cycle, mut parked_cycles) = self.wait_for_events_and_pageflips()?;
        // A worker completion returns ownership to the reactor before any
        // planner can admit work for this cycle. The planner below therefore
        // always sees the settled swapchain/transaction state and current
        // compositor state.
        self.settle_pending_render_worker_completion(&mut cycle)?;
        for (index, parked_cycle) in parked_cycles.iter_mut().enumerate() {
            self.with_parked_head(index, |runtime| {
                runtime.settle_pending_render_worker_completion(parked_cycle)
            })?;
        }
        self.settle_gpu_screencopy()?;
        let now_ns = monotonic_now_ns()?;
        if self
            .system_action_transport
            .next_deadline_ns()
            .is_some_and(|deadline| deadline <= now_ns)
        {
            cycle.wakeup.reasons.mark_system_action_transport();
        }
        self.service_system_action_transport(&cycle, now_ns);
        if cycle.wakeup.reasons.system_action_transport() {
            self.arm_runtime_deadline()?;
        }
        let slow_cycle_enabled = self.slow_cycle_trace.enabled();
        let mut render_attempted = false;
        if self.pointer_timing.enabled() {
            self.pointer_timing.record_next_reactor_wake(now_ns);
            self.pointer_timing.record_reactor_wake_return(now_ns);
        }
        let runtime_state = self.native_runtime_state(&cycle, now_ns);
        let work_domains = NativeWorkDomains::classify(&cycle.wakeup, &runtime_state);
        if slow_cycle_enabled {
            self.slow_cycle_trace
                .note_work_domains(work_domains.diagnostic_bits());
        }
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
        if work_domains.only_controller_work()
            && !cycle.wakeup.reasons.timer()
            && !self.dmabuf_gpu_release_registry.retry_due(now_ns)
        {
            if self.session.permits_output() && self.shutdown.is_running() {
                self.service_controller_work(&cycle)?;
            }
            self.finish_slow_cycle(&cycle, render_attempted)?;
            return Ok(());
        }
        if work_domains.only_system_action_transport_work() {
            self.arm_runtime_deadline()?;
            self.finish_slow_cycle(&cycle, render_attempted)?;
            return Ok(());
        }
        self.service_due_dmabuf_release_retry(now_ns)?;
        self.server.set_commit_debug_pageflip_pending(
            self.head.scanout.page_flip_pending()
                || self.head.atomic_commit_arbiter.atomic_commit_pending(),
        );
        let mut signal_shutdown_requested = false;
        if work_domains.children || self.shutdown.state() == ShutdownState::StoppingChildren {
            let signal_poll_error = self.process_supervisor.poll_runtime_signals().err();
            if let Some(signal) = self.process_supervisor.take_termination_signal()
                && self.shutdown.is_running()
            {
                self.termination.record_signal_shutdown(signal.as_str());
                if let Err(error) = self.request_native_shutdown_before_child_processing() {
                    if let Some(signal_poll_error) = signal_poll_error {
                        self.termination.record_cleanup_error(&signal_poll_error);
                    }
                    return Err(error);
                }
                signal_shutdown_requested = true;
            }
            if let Some(error) = signal_poll_error {
                return Err(error.into());
            }
            self.reap_supervised_children(&cycle)?;
        }
        if signal_shutdown_requested {
            if !self.server.has_pending_screen_capture()
                && !self.server.has_pending_process_launches()
            {
                self.advance_shutdown_lifecycle_without_cycle()?;
            }
            self.finish_slow_cycle(&cycle, render_attempted)?;
            return Ok(());
        }
        self.sync_xwayland_selection_metadata()?;
        self.sync_xwayland_dnd()?;
        if work_domains.xwayland {
            let slow_phase_started_at_ns = slow_cycle_enabled.then(monotonic_now_ns).transpose()?;
            let xwm_drain_started = Instant::now();
            self.dispatch_xwayland_events(&cycle.wakeup)?;
            self.sync_xwayland_selection_metadata()?;
            self.sync_xwayland_dnd()?;
            self.service_xwayland_proxy_selection_data_requests()?;
            self.note_timing_scope("xwm_dispatch", xwm_drain_started.elapsed());
            if self.xwayland.generation().is_some() {
                self.attach_xwayland_private_client()?;
            } else {
                self.revoke_xwayland_private_client();
            }
            self.sync_xwayland_reactor_sources()?;
            if let Some(start_ns) = slow_phase_started_at_ns {
                self.slow_cycle_trace.record_phase(
                    SlowCyclePhase::XwaylandReactor,
                    start_ns,
                    monotonic_now_ns()?,
                );
            }
        }
        if cycle.wakeup.reasons.timer()
            || cycle
                .wakeup
                .continuation
                .contains(NativeContinuationReason::ControlTimeout)
        {
            oblivion_one::application_scope::poll_status();
            let slow_phase_started_at_ns = slow_cycle_enabled.then(monotonic_now_ns).transpose()?;
            self.control_server.expire_idle_clients(
                &mut self.event_loop,
                monotonic_now_ns()?,
                oblivion_one::native::control::MAX_CONTROL_OPERATIONS_PER_CYCLE,
            );
            self.xwayland
                .handle_deadline(monotonic_now_ns()?, &mut self.process_supervisor)?;
            self.sync_xwayland_selection_metadata()?;
            self.sync_xwayland_dnd()?;
            if self.xwayland.generation().is_none() {
                self.revoke_xwayland_private_client();
            }
            self.sync_xwayland_reactor_sources()?;
            if let Some(start_ns) = slow_phase_started_at_ns {
                self.slow_cycle_trace.record_phase(
                    SlowCyclePhase::TimerDeadline,
                    start_ns,
                    monotonic_now_ns()?,
                );
            }
        }
        if !screen_capture_delivery_is_allowed(
            self.shutdown.is_running(),
            self.session.permits_output(),
        ) && self.server.has_pending_screen_capture()
        {
            self.fail_pending_screen_captures("session_inactive");
            if self.server.has_pending_screen_capture() {
                self.finish_slow_cycle(&cycle, render_attempted)?;
                return Ok(());
            }
        }
        self.advance_shutdown_lifecycle(&cycle)?;
        self.sync_xwayland_selection_metadata()?;
        self.sync_xwayland_dnd()?;
        if cycle.wakeup.reasons.drm_hotplug() {
            self.service_drm_hotplug()?;
        }
        if !self.session.permits_output() {
            if work_domains.wayland_dispatch
                || work_domains.astrea_publication
                || work_domains.commit_timing_planning
                || work_domains.presentation
                || work_domains.surface_pacing
                || work_domains.explicit_sync
                || work_domains.screen_capture
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
                self.fail_pending_screen_captures("session_inactive");
                self.quiesce_control_server()?;
                self.finish_slow_cycle(&cycle, render_attempted)?;
                return Ok(());
            }
            if work_domains.control {
                self.service_control_events(&cycle.wakeup)?;
            }
            if work_domains.cursor {
                self.service_cursor_io_completions(&cycle.wakeup)?;
            }
            self.service_keyboard_persistence_completions(&cycle.wakeup)?;
            self.service_output_persistence_completions(&cycle.wakeup)?;
            self.reconcile_dmem_foreground();
            self.arm_suspended_deadline()?;
            self.fail_pending_screen_captures("session_inactive");
            self.finish_slow_cycle(&cycle, render_attempted)?;
            return Ok(());
        }
        if !self.shutdown.is_running() {
            self.quiesce_control_server()?;
            self.finish_slow_cycle(&cycle, render_attempted)?;
            return Ok(());
        }
        if operation_plan.service_controller {
            self.service_controller_work(&cycle)?;
        }
        let slow_wayland_phase_started_at_ns = (slow_cycle_enabled
            && (work_domains.wayland_dispatch || work_domains.input))
            .then(monotonic_now_ns)
            .transpose()?;
        let wayland_dispatch_started = Instant::now();
        let dispatch_outcome =
            if operation_plan.dispatch_wayland_read_side || operation_plan.service_input {
                self.dispatch_wayland_and_input(
                    &mut cycle,
                    operation_plan.service_input,
                    operation_plan.service_hardware_input,
                    operation_plan.service_keyboard_repeat,
                    operation_plan.dispatch_wayland_read_side,
                )?
            } else {
                NativeWaylandInputDispatchOutcome::default()
            };
        self.sync_xwayland_selection_metadata()?;
        self.sync_xwayland_dnd()?;
        self.submit_xwayland_selection_data_requests()?;
        if let Some(vt) = dispatch_outcome.vt_switch_requested {
            self.request_native_vt_switch(vt)?;
            if !self.session.permits_output() {
                self.fail_pending_screen_captures("session_inactive");
                self.quiesce_control_server()?;
                self.finish_slow_cycle(&cycle, render_attempted)?;
                return Ok(());
            }
        }
        let mut routing_guard = NativeInputTransitionLatencyGuard::default();
        if let Some(transition) = dispatch_outcome.routing_transition {
            routing_guard.arm(transition);
        }
        let mut wayland_pacing_readiness_changed = dispatch_outcome.pacing_readiness_changed;
        wayland_pacing_readiness_changed |= self.service_input_at_routing_guard_checkpoint(
            &mut routing_guard,
            &mut cycle,
            NativeInputRoutingGuardCheckpoint::AfterTransition,
        )?;
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
            let _ = self.service_input_at_routing_guard_checkpoint(
                &mut routing_guard,
                &mut cycle,
                NativeInputRoutingGuardCheckpoint::BeforeSurfacePacing,
            )?;
            let phase_started_at_ns = self
                .pointer_timing
                .enabled()
                .then(monotonic_now_ns)
                .transpose()?;
            self.resource_efficiency_mut()
                .record_surface_pacing_service_run();
            let result = self.server.progress_surface_pacing(monotonic_now_ns()?);
            let pacing_visual_work = result?;
            if let Some(start_ns) = phase_started_at_ns {
                self.pointer_timing.record_phase(
                    NativePointerTimingPhase::SurfacePacing,
                    start_ns,
                    monotonic_now_ns()?,
                );
            }
            pacing_visual_work
        } else {
            false
        };
        cycle.redraw_requested |= pacing_visual_work;
        // An animated theme cursor advances on its own frame deadline; the
        // presentation step swaps in the next frame image.
        cycle.redraw_requested |= self
            .cursor_manager
            .animation_frame_due(std::time::Instant::now());
        let post_service_state = self.native_runtime_state(&cycle, monotonic_now_ns()?);
        cycle.work_class =
            NativeWorkDomains::from_wakeup(&cycle.wakeup, &post_service_state).work_class;
        if work_domains.wayland_dispatch {
            self.note_timing_scope("wayland_dispatch", wayland_dispatch_started.elapsed());
        } else if work_domains.input {
            self.note_timing_scope("input_dispatch", wayland_dispatch_started.elapsed());
        }
        if let Some(start_ns) = slow_wayland_phase_started_at_ns {
            self.slow_cycle_trace.record_phase(
                SlowCyclePhase::WaylandInput,
                start_ns,
                monotonic_now_ns()?,
            );
        }
        let slow_cursor_phase_started_at_ns = (slow_cycle_enabled
            && (work_domains.control || work_domains.cursor))
            .then(monotonic_now_ns)
            .transpose()?;
        let cursor_control_started_at_ns = (self.pointer_timing.enabled()
            && (work_domains.control || work_domains.cursor))
            .then(monotonic_now_ns)
            .transpose()?;
        if work_domains.control || work_domains.cursor {
            let _ = self.service_input_at_routing_guard_checkpoint(
                &mut routing_guard,
                &mut cycle,
                NativeInputRoutingGuardCheckpoint::BeforeCursorAndControl,
            )?;
        }
        if work_domains.control {
            self.service_control_events(&cycle.wakeup)?;
        }
        if work_domains.cursor {
            self.service_cursor_io_completions(&cycle.wakeup)?;
        }
        self.service_keyboard_persistence_completions(&cycle.wakeup)?;
        self.service_output_persistence_completions(&cycle.wakeup)?;
        let output_configuration_cycle = output_configuration_cycle_is_due(
            self.head.pending_output_configuration.is_some(),
            self.head
                .output_configuration_transactions
                .deadline_ns()
                .is_some_and(|deadline| deadline <= monotonic_now_ns().unwrap_or(0)),
            self.output_persistence_compensation_required,
            self.head
                .output_reconfiguration_rearm_retry_deadline_ns
                .is_some_and(|deadline| deadline <= monotonic_now_ns().unwrap_or(0)),
        );
        if output_configuration_cycle {
            let now_ns = monotonic_now_ns()?;
            if self.advance_output_configuration(now_ns)?
                == super::output_configuration::OutputConfigurationAdvance::Waiting
            {
                self.report_stalled_output_configuration(now_ns);
            }
        }
        if let Some(start_ns) = cursor_control_started_at_ns {
            self.pointer_timing.record_phase(
                NativePointerTimingPhase::CursorAndControl,
                start_ns,
                monotonic_now_ns()?,
            );
        }
        if let Some(start_ns) = slow_cursor_phase_started_at_ns {
            self.slow_cycle_trace.record_phase(
                SlowCyclePhase::CursorControl,
                start_ns,
                monotonic_now_ns()?,
            );
        }
        let xwayland_scene_work = wayland_client_work || xwayland_work;
        if xwayland_scene_work {
            let _ = self.service_input_at_routing_guard_checkpoint(
                &mut routing_guard,
                &mut cycle,
                NativeInputRoutingGuardCheckpoint::BeforeXwaylandScene,
            )?;
        }
        if xwayland_scene_work {
            let slow_phase_started_at_ns = slow_cycle_enabled.then(monotonic_now_ns).transpose()?;
            let phase_started_at_ns = self
                .pointer_timing
                .enabled()
                .then(monotonic_now_ns)
                .transpose()?;
            self.dispatch_xwayland_client_disconnects()?;
            self.dispatch_xwayland_shell_binds()?;
            self.initialize_managed_xwayland()?;
            cycle.redraw_requested |= self.dispatch_xwayland_scene_batch()?;
            self.sync_xwayland_reactor_sources()?;
            if let Some(start_ns) = phase_started_at_ns {
                self.pointer_timing.record_phase(
                    NativePointerTimingPhase::XwaylandScene,
                    start_ns,
                    monotonic_now_ns()?,
                );
            }
            if let Some(start_ns) = slow_phase_started_at_ns {
                self.slow_cycle_trace.record_phase(
                    SlowCyclePhase::XwaylandScene,
                    start_ns,
                    monotonic_now_ns()?,
                );
            }
        }
        if cycle.shutdown_requested {
            self.request_native_shutdown()?;
        }
        self.reconcile_dmem_foreground();
        if !self.shutdown.is_running() || !self.session.permits_output() {
            if !self.shutdown.is_running() {
                self.quiesce_control_server()?;
            }
            self.finish_slow_cycle(&cycle, render_attempted)?;
            return Ok(());
        }
        if output_configuration_cycle || self.head.pending_output_configuration.is_some() {
            self.arm_runtime_deadline()?;
            self.finish_slow_cycle(&cycle, render_attempted)?;
            return Ok(());
        }
        if work_domains.wayland_dispatch
            || work_domains.control
            || xwayland_scene_work
            || cycle.wakeup.continuation.contains(
                oblivion_one::native::event_loop::NativeContinuationReason::ProcessLaunchBacklog,
            )
        {
            let launch_backlog_pending = if !self.shutdown.is_running() {
                self.fail_pending_process_launches_for_shutdown()
            } else {
                drain_pending_process_launches_with_xwayland_environment_and_cursor(
                    &mut self.server,
                    &mut self.process_supervisor,
                    &mut self.astrea_launch_tracker,
                    self.effective_app_gpu_policy,
                    self.perf,
                    &mut self.pending_launches,
                    self.xwayland.normal_app_environment(),
                    Some(self.cursor_manager.desired_configuration()),
                )
            };
            if process_launch_backlog_needs_immediate_continuation(
                launch_backlog_pending,
                self.shutdown.is_running(),
            ) {
                self.event_loop.request_continuation(
                    oblivion_one::native::event_loop::NativeContinuationReason::ProcessLaunchBacklog,
                )?;
            }
        }
        let commit_timing_planning_generation_before_prepare =
            self.server.commit_timing_planning_generation();
        let prepare_operation_plan = NativeWorkDomains::classify(
            &cycle.wakeup,
            &self.native_runtime_state(&cycle, monotonic_now_ns()?),
        )
        .operation_plan();
        if prepare_operation_plan.service_acquire_and_prepare {
            let _ = self.service_input_at_routing_guard_checkpoint(
                &mut routing_guard,
                &mut cycle,
                NativeInputRoutingGuardCheckpoint::BeforeAcquirePrepare,
            )?;
            let _ = self.service_matured_auto_cursor_before_prepare(&cycle)?;
        }
        let prepare_outcome = if prepare_operation_plan.service_acquire_and_prepare {
            let slow_phase_started_at_ns = slow_cycle_enabled.then(monotonic_now_ns).transpose()?;
            let phase_started_at_ns = self
                .pointer_timing
                .enabled()
                .then(monotonic_now_ns)
                .transpose()?;
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
            if let Some(start_ns) = phase_started_at_ns {
                self.pointer_timing.record_phase(
                    NativePointerTimingPhase::AcquirePrepare,
                    start_ns,
                    monotonic_now_ns()?,
                );
            }
            if let Some(start_ns) = slow_phase_started_at_ns {
                self.slow_cycle_trace.record_phase(
                    SlowCyclePhase::AcquirePrepare,
                    start_ns,
                    monotonic_now_ns()?,
                );
            }
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
            if !self.session.permits_output() {
                self.fail_pending_screen_captures("session_inactive");
            }
            self.finish_slow_cycle(&cycle, render_attempted)?;
            return Ok(());
        }
        let capture_service_attempted = prepare_operation_plan
            .service_screen_capture_if_pending(self.server.has_pending_screen_capture(), || {
                self.service_pending_screen_capture()
            })?;
        if capture_service_attempted
            && screen_capture_backlog_needs_immediate_continuation(
                self.server.has_pending_screen_capture(),
                self.head.scanout.render_worker_active(),
                self.head.scanout.render_worker_busy(),
            )
        {
            self.event_loop.request_continuation(
                oblivion_one::native::event_loop::NativeContinuationReason::ScreenCaptureBacklog,
            )?;
        }
        self.stage_gpu_screencopy_requests(&mut cycle)?;
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
        cycle.presentation_admitted = presentation_work;
        if presentation_work {
            let _ = self.service_input_at_routing_guard_checkpoint(
                &mut routing_guard,
                &mut cycle,
                NativeInputRoutingGuardCheckpoint::BeforePresentation,
            )?;
        }
        if presentation_work {
            render_attempted = true;
            let phase_started_at_ns = self
                .pointer_timing
                .enabled()
                .then(monotonic_now_ns)
                .transpose()?;
            let render_started = Instant::now();
            self.resource_efficiency_mut()
                .record_presentation_planning_run();
            self.render_present_and_update_metrics(&mut cycle)?;
            self.settle_gpu_screencopy()?;
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
            if let Some(start_ns) = phase_started_at_ns {
                self.pointer_timing.record_phase(
                    NativePointerTimingPhase::RenderPresentKms,
                    start_ns,
                    monotonic_now_ns()?,
                );
            }
        } else {
            self.resource_efficiency_mut()
                .record_presentation_planning_skip();
        }
        let parked_presentation_work = self.present_parked_heads(
            &mut parked_cycles,
            &cycle,
            prepare_outcome.visual_work_created,
        )?;
        let presentation_work = presentation_work || parked_presentation_work;
        let (mut visual_scene_debt, mut cursor_temporal_work) = self.head_cycle_tail_wake_work()?;
        for index in 0..self.parked_head_count() {
            let (debt, cursor) =
                self.with_parked_head(index, |runtime| runtime.head_cycle_tail_wake_work())?;
            visual_scene_debt |= debt;
            cursor_temporal_work |= cursor;
        }
        if super::wake_plan::cycle_tail_requires_runtime_wake_plan(
            presentation_work,
            self.input_epoch.backlog_pending(),
            visual_scene_debt,
            cursor_temporal_work,
        ) {
            self.arm_runtime_deadline()?;
        }
        if !cycle.shutdown_requested
            && self.xwayland.generation().is_some()
            && self.server.has_pending_xwayland_backend_commands()
        {
            self.request_native_continuation(
                oblivion_one::native::event_loop::NativeContinuationReason::XwaylandContinuation,
            )?;
        }
        cycle.fast_path_completed = !prepare_outcome.acquire_service_ran && !presentation_work;
        if self.pointer_timing.enabled() {
            self.pointer_timing.record_cycle_return(monotonic_now_ns()?);
        }
        self.finish_slow_cycle(&cycle, render_attempted)?;
        Ok(())
    }

    /// Per head: whether the cycle tail must arm a wake for scene debt this
    /// head has not rendered yet, or for its cursor temporal work.
    fn head_cycle_tail_wake_work(&self) -> NativeResult<(bool, bool)> {
        let visual_scene_debt = super::commit_timing::logical_scene_changed(
            self.head.last_rendered_scene_generation,
            self.server.scene_render_generation(),
        );
        let cursor_temporal_work = self
            .head
            .cursor_output_arbitration
            .has_future_wake_requirement(
                monotonic_now_ns()?,
                self.head.kms_commit_worker_transport,
            );
        Ok((visual_scene_debt, cursor_temporal_work))
    }

    /// Runs presentation planning for each parked head with the scene work
    /// the global phase produced. Returns whether any head was admitted.
    fn present_parked_heads(
        &mut self,
        parked_cycles: &mut [NativeCycleState],
        primary_cycle: &NativeCycleState,
        visual_work_created: bool,
    ) -> NativeResult<bool> {
        let mut admitted = false;
        for (index, parked_cycle) in parked_cycles.iter_mut().enumerate() {
            parked_cycle.redraw_requested |= primary_cycle.redraw_requested;
            parked_cycle.shutdown_requested |= primary_cycle.shutdown_requested;
            admitted |= self.with_parked_head(index, |runtime| -> NativeResult<bool> {
                let state = runtime.native_runtime_state(parked_cycle, monotonic_now_ns()?);
                parked_cycle.work_class =
                    NativeWorkDomains::from_wakeup(&parked_cycle.wakeup, &state).work_class;
                let work = NativeWorkDomains::classify(&parked_cycle.wakeup, &state)
                    .operation_plan()
                    .presentation_admitted(
                        parked_cycle.redraw_requested,
                        parked_cycle.frame_completed,
                        visual_work_created,
                    );
                parked_cycle.presentation_admitted = work;
                if work {
                    runtime.render_present_and_update_metrics(parked_cycle)?;
                }
                Ok(work)
            })?;
        }
        Ok(admitted)
    }

    fn finish_slow_cycle(
        &mut self,
        cycle: &NativeCycleState,
        render_attempted: bool,
    ) -> NativeResult<()> {
        let maintenance_now = std::time::Instant::now();
        let scratch_maintenance_due = self
            .head
            .scanout
            .next_effect_scratch_maintenance_deadline()
            .is_some_and(|deadline| deadline <= maintenance_now);
        if scratch_maintenance_due {
            // This shared cycle tail follows pageflip settlement, input and
            // protocol work, and any frame/render work selected for the wake.
            // One bounded batch runs here, then the normal wake arbiter is
            // refreshed before the reactor can wait on a now-stale deadline.
            let phase_start_ns = self
                .slow_cycle_trace
                .enabled()
                .then(monotonic_now_ns)
                .transpose()?;
            self.head.scanout.maintain_effect_scratch(maintenance_now)?;
            if let Some(start_ns) = phase_start_ns {
                self.slow_cycle_trace.record_phase(
                    SlowCyclePhase::PostWakeMaintenance,
                    start_ns,
                    monotonic_now_ns()?,
                );
            }
            self.arm_runtime_deadline()?;
        }
        self.flush_presentation_trace();
        if !self.slow_cycle_trace.enabled() {
            return Ok(());
        }
        let (kms_queue_depth, kms_worker_active) = self
            .head
            .kms_commit_worker
            .as_ref()
            .map(|worker| (worker.queue_depth(), worker.submission_active()))
            .unwrap_or_default();
        let xwayland_totals = self.xwayland.slow_cycle_totals();
        self.slow_cycle_trace.note_xwayland_totals(
            xwayland_totals.0,
            xwayland_totals.1,
            xwayland_totals.2,
        );
        self.slow_cycle_trace.note_input(
            cycle.raw_input_events,
            cycle.coalesced_input_events,
            cycle.wakeup.reasons.input(),
            self.input_epoch.backlog_pending(),
        );
        self.slow_cycle_trace
            .note_dispatch_timings(cycle.tick_us, cycle.input_drain_us);
        self.slow_cycle_trace.note_output(
            render_attempted,
            cycle.frame_rendered,
            cycle.frame_submitted,
            cycle.pageflip_pending_at_tick || self.head.scanout.page_flip_pending(),
            kms_queue_depth,
            kms_worker_active,
        );
        self.slow_cycle_trace.finish_cycle(monotonic_now_ns()?);
        Ok(())
    }

    /// Give newly readable native input one bounded microturn after a routing
    /// transition.  The checkpoint does not drain or acknowledge any other
    /// reactor source, so all cycle-tail work remains owned by this cycle.
    fn service_input_at_routing_guard_checkpoint(
        &mut self,
        guard: &mut NativeInputTransitionLatencyGuard,
        cycle: &mut NativeCycleState,
        checkpoint: NativeInputRoutingGuardCheckpoint,
    ) -> NativeResult<bool> {
        if !guard.armed() {
            return Ok(false);
        }
        let mut input_serviceable = false;
        let decision = guard.checkpoint_with_readiness(checkpoint, || {
            let ready = self.input_devices.ready_nonblocking()?;
            input_serviceable = ready;
            Ok::<bool, std::io::Error>(ready)
        })?;
        if self.pointer_timing.enabled() {
            self.pointer_timing.record_checkpoint(
                checkpoint.index(),
                input_serviceable,
                matches!(decision, NativeRoutingGuardDecision::ServiceFreshInput),
            );
        }
        if !matches!(decision, NativeRoutingGuardDecision::ServiceFreshInput) {
            return Ok(false);
        }
        let outcome = self.service_fresh_input_microturn_after_routing_transition(cycle)?;
        if let Some(transition) = outcome.routing_transition {
            guard.arm(transition);
        }
        Ok(outcome.pacing_readiness_changed)
    }

    fn service_fresh_input_microturn_after_routing_transition(
        &mut self,
        cycle: &mut NativeCycleState,
    ) -> NativeResult<NativeWaylandInputDispatchOutcome> {
        let baseline = cycle.microturn_baseline();

        let outcome = self.dispatch_wayland_and_input(cycle, true, true, false, false)?;
        cycle.merge_input_microturn(baseline);
        Ok(outcome)
    }

    fn flush_presentation_trace(&mut self) {
        if let Some(writer) = self.presentation_trace_writer.as_mut() {
            writer.submit(&self.presentation_trace);
        }
    }

    pub(super) fn advance_shutdown_lifecycle(
        &mut self,
        cycle: &NativeCycleState,
    ) -> NativeResult<()> {
        loop {
            match self.shutdown.state() {
                ShutdownState::Running | ShutdownState::Complete => return Ok(()),
                ShutdownState::Requested => {
                    self.termination.set_phase(RuntimePhase::ShutdownRequest);
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
                    self.termination.set_phase(RuntimePhase::PageflipDrain);
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
                                    self.head.scanout.page_flip_pending(),
                                ),
                            ]
                        });
                        continue;
                    }
                    self.arm_shutdown_deadline()?;
                    return Ok(());
                }
                ShutdownState::StoppingChildren => {
                    self.termination.set_phase(RuntimePhase::StoppingChildren);
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
                        primary_deadline: Some(NativeDeadline {
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
                    self.termination.set_phase(RuntimePhase::KmsTeardown);
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
            primary_deadline: self
                .shutdown
                .pageflip_deadline_ns()
                .map(|at_ns| NativeDeadline {
                    owner: NativeDeadlineOwner::AtomicCommitWatchdog,
                    at_ns,
                }),
            control_timeout_deadline_ns: control_timeout_deadline
                .filter(|deadline| *deadline > now_ns),
            output_configuration_deadline_ns: self
                .head
                .output_configuration_transactions
                .deadline_ns()
                .filter(|deadline| *deadline > now_ns),
            surface_pacing_deadline_ns: (!self.server.has_surface_pacing_readiness_pending())
                .then(|| self.server.next_surface_pacing_deadline_ns())
                .flatten(),
            control_timeout_pending: control_timeout_deadline
                .is_some_and(|deadline| deadline <= now_ns),
            ..NativeWakePlanInputs::default()
        });
        self.install_native_wake_plan(plan, now_ns)
    }

    fn arm_suspended_deadline(&mut self) -> NativeResult<()> {
        let now_ns = monotonic_now_ns()?;
        let control_timeout_deadline = self.control_server.next_deadline_ns();
        let system_action_transport_deadline = self.system_action_transport.next_deadline_ns();
        let plan = build_native_wake_plan(NativeWakePlanInputs {
            now_ns,
            primary_deadline: self.shutdown.suspended_reactor_deadline_ns().map(|at_ns| {
                NativeDeadline {
                    owner: NativeDeadlineOwner::ControlTimeout,
                    at_ns,
                }
            }),
            control_timeout_deadline_ns: control_timeout_deadline
                .filter(|deadline| *deadline > now_ns),
            system_action_transport_deadline_ns: system_action_transport_deadline
                .filter(|deadline| *deadline > now_ns),
            output_configuration_deadline_ns: self
                .head
                .output_configuration_transactions
                .deadline_ns()
                .filter(|deadline| *deadline > now_ns),
            surface_pacing_deadline_ns: (!self.server.has_surface_pacing_readiness_pending())
                .then(|| self.server.next_surface_pacing_deadline_ns())
                .flatten(),
            input_backlog: self.input_epoch.backlog_pending(),
            astrea_publication: self.server.has_pending_astrea_toplevel_publication(),
            commit_timing_planning: self.server.has_pending_commit_timing_planning(),
            control_timeout_pending: control_timeout_deadline
                .is_some_and(|deadline| deadline <= now_ns),
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

    fn dispatch_runtime_seat_events(&mut self, wakeup: &NativeWakeup) -> NativeResult<bool> {
        if !wakeup.reasons.seat() {
            return Ok(false);
        }
        let Some(seat) = self.seat_session.clone() else {
            return Ok(false);
        };
        NativeSessionIo::observe(self, NativeIoOperation::SeatDispatch);
        seat.dispatch()?;
        self.consume_pending_seat_events(&seat)
    }

    fn consume_pending_seat_events(&mut self, seat: &NativeSeatSession) -> NativeResult<bool> {
        let mut disabled_observed = false;
        for event in seat.drain_events() {
            if matches!(event, NativeSeatEvent::Disabled) {
                disabled_observed = true;
                let (disable_sequence, worker_present) = seat.pre_disable_observation();
                self.perf.log("native.seat_pre_revoke", || {
                    vec![
                        NativePerfField::bool("worker_present", worker_present),
                        NativePerfField::bool("worker_quiesced", true),
                        NativePerfField::u64("disable_sequence", disable_sequence),
                    ]
                });
            }
            match self.session.begin_for_event(event) {
                Some(NativeSessionTransition::BeginSuspend) => self.suspend_native_session(seat)?,
                Some(NativeSessionTransition::BeginResume) if self.shutdown.is_running() => {
                    self.resume_native_session()?
                }
                Some(NativeSessionTransition::AbortResumeForSuspend) => {
                    abort_native_output_recovery_for_suspend(self)?;
                    if seat.acknowledge_disable()? {
                        NativeSessionIo::observe(self, NativeIoOperation::SeatDisableAcknowledged);
                    } else {
                        return Err(io::Error::other(
                            "stale libseat disable acknowledgment during recovery abort",
                        )
                        .into());
                    }
                    self.session
                        .finish_resume_abort_for_suspend()
                        .ok_or_else(|| {
                            io::Error::other("native session recovery abort lost Resuming state")
                        })?;
                    self.log_session_transition("resuming", "suspended", "disable_during_recovery");
                    self.arm_suspended_deadline()?;
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
        Ok(disabled_observed)
    }

    fn request_native_vt_switch(&mut self, vt: u8) -> NativeResult<()> {
        let Some(seat) = self.seat_session.clone() else {
            self.perf.log("native.vt_switch_request", || {
                vec![
                    NativePerfField::u64("vt", u64::from(vt)),
                    NativePerfField::str("status", "failed"),
                    NativePerfField::bool("disabled_observed", false),
                    NativePerfField::str("error", "no seat session"),
                ]
            });
            eprintln!("native input: VT switch request vt={vt} failed: no seat session");
            return Ok(());
        };
        let outcome =
            request_native_vt_switch(&seat, vt, || self.consume_pending_seat_events(&seat))?;
        let status = match outcome.status {
            NativeVtSwitchRequestStatus::Requested => "requested",
            NativeVtSwitchRequestStatus::Failed => "failed",
        };
        self.perf.log("native.vt_switch_request", || {
            vec![
                NativePerfField::u64("vt", u64::from(vt)),
                NativePerfField::str("status", status),
                NativePerfField::bool("disabled_observed", outcome.disabled_observed),
                NativePerfField::str(
                    "error",
                    outcome.error.clone().unwrap_or_else(|| "none".to_string()),
                ),
            ]
        });
        if let Some(error) = outcome.error {
            eprintln!("native input: VT switch request vt={vt} failed: {error}");
        }
        Ok(())
    }

    fn reconcile_dmem_foreground(&mut self) {
        let target =
            Self::foreground_dmem_is_eligible(
                self.session.permits_output(),
                self.shutdown.is_running(),
            )
            .then(|| self.server.dmem_foreground_target())
            .flatten()
            .map(|(window_id, pid)| {
                oblivion_one::native::dmem_foreground::ForegroundTarget { window_id, pid }
            });
        self.dmem_foreground.submit(target);
    }

    fn foreground_dmem_is_eligible(session_permits_output: bool, shutdown_running: bool) -> bool {
        session_permits_output && shutdown_running
    }

    fn suspend_native_session(&mut self, seat: &NativeSeatSession) -> NativeResult<()> {
        self.fail_pending_screen_captures("session_inactive");
        self.dmem_foreground.submit(None);
        self.log_session_transition("active", "suspending", "seat_disable");
        self.perf.log("native.session_suspend", || {
            vec![
                NativePerfField::str("pageflip_policy", "quarantine_until_recovery_modeset"),
                NativePerfField::bool("pageflip_pending", self.head.scanout.page_flip_pending()),
                NativePerfField::str("explicit_sync_policy", "park_and_rearm"),
            ]
        });
        oblivion_one::native::scheduling_qos::prepare_current_reactor_for_session_suspend()
            .map_err(io::Error::other)?;
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
        match recover_native_output(self) {
            Ok(NativeSessionRecoveryProgress::Complete) => {
                self.finish_native_session_recovery()?;
            }
            Ok(NativeSessionRecoveryProgress::WaitingForSuspendedFence) => {}
            Err(error) => return self.fail_native_session_recovery(error),
        }
        Ok(())
    }

    fn finish_native_session_recovery(&mut self) -> NativeResult<()> {
        self.head.render_worker_admission_stopped = false;
        self.session.finish_resume();
        self.reconcile_dmem_foreground();
        oblivion_one::native::scheduling_qos::reacquire_current_reactor_after_session_resume()
            .map_err(|error| io::Error::other(error.to_string()))?;
        self.log_session_transition("resuming", "active", "output_recovered");
        Ok(())
    }

    fn fail_native_session_recovery(&mut self, error: Box<dyn Error>) -> NativeResult<()> {
        if let Some(token) = self.drm_reactor_token.take() {
            let _ = self.event_loop.unregister(token);
        }
        if let Some(token) = self.head.output_render_fence_token.take() {
            let _ = self.event_loop.unregister(token);
        }
        if let Ok(parked) = self
            .acquire_watches
            .park_for_session_suspend(&mut self.event_loop)
        {
            self.parked_acquire_watches.extend(parked);
        }
        if let Some(mut cursor) = self.head.legacy_cursor.take() {
            cursor.disarm_drm_cleanup();
        }
        self.pending_session_recovery = None;
        teardown_without_drm_io(self);
        self.session.fail_resume();
        self.log_session_transition("resuming", "failed", "recovery_failed");
        Err(error)
    }

    fn continue_native_session_recovery(&mut self) -> NativeResult<()> {
        match continue_native_output_recovery(self) {
            Ok(NativeSessionRecoveryProgress::Complete) => {
                self.finish_native_session_recovery()?;
                Ok(())
            }
            Ok(NativeSessionRecoveryProgress::WaitingForSuspendedFence) => Ok(()),
            Err(error) => self.fail_native_session_recovery(error),
        }
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
        self.head.output_reconfiguration_rearm_retry_deadline_ns = None;
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
                NativePerfField::bool("pageflip_pending", self.head.scanout.page_flip_pending()),
                NativePerfField::str("shutdown_state", self.shutdown.state().as_str()),
                NativePerfField::str("drm_backend", self.kms.kind().as_str()),
                NativePerfField::str("input_backend", self.input_devices.kind().as_str()),
            ]
        });
    }
    pub(super) fn fail_pending_screen_captures(&mut self, reason: &str) {
        self.fail_gpu_screencopy();
        if let Some(pending) = self.head.pending_render_worker_capture.as_mut()
            && !pending.cancelled
        {
            pending.pending.fail(reason);
            pending.cancelled = true;
        }
        for _ in 0..MAX_SCREEN_CAPTURES_FAILED_PER_TURN {
            let Some(pending) = self.server.take_pending_screen_capture() else {
                break;
            };
            pending.fail(reason);
        }
        if self.server.has_pending_screen_capture() {
            let _ = self.event_loop.request_continuation(
                oblivion_one::native::event_loop::NativeContinuationReason::ScreenCaptureBacklog,
            );
        }
    }

    fn service_pending_screen_capture(&mut self) -> NativeResult<()> {
        if !screen_capture_delivery_is_allowed(
            self.shutdown.is_running(),
            self.session.permits_output(),
        ) {
            self.fail_pending_screen_captures("session_inactive");
            return Ok(());
        }
        if screen_capture_waits_for_render_worker(
            self.head.scanout.render_worker_active(),
            self.head.scanout.render_worker_busy(),
        ) {
            // Keep the protocol request in the compositor backlog until the
            // serialized renderer command slot is available. The completion
            // fd will wake the reactor; no capture job is prepared here.
            return Ok(());
        }
        let Some(mut pending) = self.server.take_pending_screen_capture() else {
            return Ok(());
        };
        if !self
            .server
            .screen_capture_output_is_current(pending.output_id())
        {
            pending.fail("output_gone");
            return Ok(());
        }
        if !self.head.scanout.supports_gpu_buffer_protocols() {
            pending.fail("unsupported");
            return Ok(());
        }

        let (width, height) = self.head.scanout.dimensions();
        let mut dimensions = match CaptureDimensions::checked(width, height) {
            Ok(dimensions) => dimensions,
            Err(_) => {
                pending.fail("allocation_failed");
                return Ok(());
            }
        };
        let resolved_scene = if let Some(target) = pending.window_preview_target() {
            let source = match self.server.window_preview_source(
                target.window_id,
                target.max_width.min(width),
                target.max_height.min(height),
            ) {
                Ok(source) => source,
                Err(reason) => {
                    pending.fail(reason);
                    return Ok(());
                }
            };
            pending.set_window_preview_size(
                source.width,
                source.height,
                (source.window_width, source.window_height),
            );
            dimensions = CaptureDimensions::checked(source.width, source.height)?;
            ResolvedNativeFrameScene::for_window_preview(&self.server, source)
        } else {
            ResolvedNativeFrameScene::from_server(&self.server).into_owned()
        };
        let include_cursor = pending.overlay_cursor();
        let cursor_mode = self.head.cursor_render_mode.for_capture(
            include_cursor,
            self.server.client_cursor_render_state().is_some(),
        );
        if self.head.scanout.render_worker_active() {
            let job_id = super::render_worker::next_render_worker_job_id()?;
            let frozen_frame = match &*self.head.scanout {
                crate::native_output::scanout::NativeScanoutBackend::AtomicEglGbm(scanout) => {
                    scanout.freeze_capture_frame(
                        resolved_scene,
                        &self.server,
                        &self.input_state,
                        cursor_mode,
                        include_cursor,
                    )
                }
                _ => {
                    pending.fail("unsupported");
                    return Ok(());
                }
            };
            let control_epoch = match self
                .head
                .scanout
                .dispatch_render_worker_capture(job_id, frozen_frame)
            {
                Ok(epoch) => epoch,
                Err(error) => {
                    eprintln!("native screen capture worker admission failed: {error}");
                    pending.fail("render_failed");
                    return Ok(());
                }
            };
            if self.head.pending_render_worker_capture.is_some() {
                pending.fail("render_failed");
                return Err(io::Error::other(
                    "render worker admitted a second capture continuation",
                )
                .into());
            }
            self.head.pending_render_worker_capture =
                Some(super::render_worker::PendingWorkerCapture {
                    job_id,
                    control_epoch,
                    pending,
                    dimensions,
                    cancelled: false,
                });
            return Ok(());
        }
        let payload = match self.head.scanout.capture_scene(
            resolved_scene,
            &self.server,
            &self.input_state,
            cursor_mode,
            include_cursor,
        ) {
            Ok(payload) => payload,
            Err(error) => {
                eprintln!("native screen capture render failed: {error}");
                pending.fail("render_failed");
                return Ok(());
            }
        };
        deliver_screen_capture_payload(pending, &payload, dimensions);
        Ok(())
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
            head:
                OutputHead {
                    kms_backend,
                    target,
                    mode_label,
                    output_epoch,
                    scanout,
                    cursor_render_mode,
                    legacy_cursor,
                    frame_scheduler,
                    scene_history: _,
                    frame_index,
                    mismatched_pageflip_events,
                    stale_pageflip_events,
                    presentation_cadence: _,
                    last_acquire_ready_at_ns,
                    ..
                },
            perf: _,
            kms,
            drm_timestamp_clock,
            presentation_clock,
            frame_renderer,
            input_state,
            cursor_preference,
            input_devices,
            acquire_notifier,
            acquire_watches,
            parked_acquire_watches: _,
            event_loop,
            drm_reactor_token: _,
            effective_app_gpu_policy,
            queued_redraw_requested,
            known_toplevels,
            pending_launches,
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
                    *output_epoch,
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
