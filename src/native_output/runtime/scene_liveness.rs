use super::*;

pub(super) const fn scene_visual_debt_has_external_wake_owner(
    page_flip_pending: bool,
    atomic_commit_pending: bool,
    output_render_fence_pending: bool,
    render_worker_busy: bool,
) -> bool {
    page_flip_pending || atomic_commit_pending || output_render_fence_pending || render_worker_busy
}

impl NativeRuntime {
    pub(super) fn arm_runtime_deadline(&mut self) -> NativeResult<()> {
        let now_ns = monotonic_now_ns()?;
        let pageflip_timeout_owner: NativePageflipTimeoutOwner =
            self.head.kms_commit_worker_transport.into();
        self.dmabuf_gpu_release_registry.update_retry_for_work(
            self.server.deferred_dmabuf_release_count(),
            self.server.retryable_deferred_dmabuf_release_count(),
            self.server.explicit_release_signal_retry_count(),
            DmabufReleaseRetryReason::NoGpuProofAvailable,
            now_ns,
        );
        let dmabuf_retry_deadline = self.dmabuf_gpu_release_registry.retry_deadline_ns();
        let surface_pacing_deadline = (!self.server.has_surface_pacing_readiness_pending())
            .then(|| self.server.next_surface_pacing_deadline_ns())
            .flatten();
        let control_timeout_deadline = self.control_server.next_deadline_ns();
        let system_action_transport_deadline = self.system_action_transport.next_deadline_ns();
        let scratch_deadline_now = std::time::Instant::now();
        let scratch_maintenance_deadline_ns = self
            .head
            .scanout
            .next_effect_scratch_maintenance_deadline()
            .map(|deadline| {
                super::wake_plan::instant_deadline_to_monotonic_ns(
                    deadline,
                    scratch_deadline_now,
                    now_ns,
                )
            });
        let cursor_animation_deadline_ns =
            self.cursor_manager
                .next_animation_deadline()
                .map(|deadline| {
                    super::wake_plan::instant_deadline_to_monotonic_ns(
                        deadline,
                        scratch_deadline_now,
                        now_ns,
                    )
                });
        let scheduler_wake_requirement = self.current_scheduler_wake_requirement(now_ns)?;
        let scheduler_wake_requirement = gate_scheduler_for_output_configuration(
            self.head.pending_output_configuration.is_some(),
            scheduler_wake_requirement,
        );
        let visual_scene_debt = super::commit_timing::logical_scene_changed(
            self.head.last_rendered_scene_generation,
            self.server.scene_render_generation(),
        );
        // An in-flight render-worker job wakes the reactor through its event
        // fd when it completes. Without counting it, every cycle during a
        // render re-requested an immediate continuation and the reactor spun
        // for the whole render, up to ~1 s while a game saturated the GPU.
        let scene_visual_debt_has_external_wake_owner = scene_visual_debt_has_external_wake_owner(
            self.head.scanout.page_flip_pending(),
            self.head.atomic_commit_arbiter.atomic_commit_pending(),
            self.head.output_render_fence_token.is_some(),
            self.head.scanout.render_worker_busy(),
        );
        // A held secondary-only repaint is owned by its hold deadline (or by
        // the fullscreen owner's next commit), not an immediate continuation.
        let secondary_hold_deadline_ns = self
            .head
            .vrr_cadence
            .secondary_hold()
            .wake_deadline_ns(now_ns);
        let scene_visual_debt_continuation = visual_scene_debt
            && scheduler_wake_requirement.is_none()
            && !scene_visual_debt_has_external_wake_owner
            && secondary_hold_deadline_ns.is_none();
        let plan = build_native_wake_plan(NativeWakePlanInputs {
            now_ns,
            primary_deadline: None,
            scheduler_wake_requirement,
            atomic_commit_watchdog_deadline_ns: atomic_commit_watchdog_deadline_for_timeout_owner(
                self.head.atomic_commit_arbiter.watchdog_deadline_ns(),
                pageflip_timeout_owner,
            ),
            explicit_sync_fallback_deadline_ns: self.acquire_watches.next_fallback_deadline_ns(),
            xwayland_timeout_deadline_ns: self.xwayland.next_deadline_ns(),
            cursor_response_deadline_ns: self
                .head
                .cursor_output_arbitration
                .wake_deadline_ns(now_ns),
            cursor_planning_deadline_ns: self
                .head
                .cursor_output_arbitration
                .planning_retry_wake_deadline_ns(now_ns),
            cursor_sync_admission_deadline_ns: (self.head.kms_commit_worker_transport
                == crate::native_output::kms_worker::KmsCommitWorkerTransport::Synchronous)
                .then(|| {
                    self.head
                        .cursor_output_arbitration
                        .synchronous_physical_admission_wake_deadline_ns(now_ns)
                })
                .flatten(),
            control_timeout_deadline_ns: control_timeout_deadline
                .filter(|deadline| *deadline > now_ns),
            system_action_transport_deadline_ns: system_action_transport_deadline
                .filter(|deadline| *deadline > now_ns),
            output_configuration_deadline_ns: match (
                self.head.output_configuration_transactions.deadline_ns(),
                self.head.output_reconfiguration_rearm_retry_deadline_ns,
            ) {
                (Some(transaction), Some(retry)) => Some(transaction.min(retry)),
                (transaction, retry) => transaction.or(retry),
            },
            surface_pacing_deadline_ns: surface_pacing_deadline,
            dmabuf_retry_deadline_ns: dmabuf_retry_deadline,
            keyboard_repeat_deadline_ns: self
                .shutdown
                .is_running()
                .then(|| self.input_state.keyboard_repeat_deadline_ns())
                .flatten(),
            scratch_maintenance_deadline_ns,
            cursor_animation_deadline_ns,
            secondary_hold_deadline_ns,
            input_backlog: self.input_epoch.backlog_pending(),
            astrea_publication: self.server.has_pending_astrea_toplevel_publication(),
            commit_timing_planning: self.server.has_pending_commit_timing_planning(),
            xwayland_continuation: self.xwayland.generation().is_some()
                && self.server.has_pending_xwayland_backend_commands(),
            control_timeout_pending: control_timeout_deadline
                .is_some_and(|deadline| deadline <= now_ns),
            scene_visual_debt_continuation,
        });
        if visual_scene_debt
            && plan.deadline.is_none()
            && !scene_visual_debt_has_external_wake_owner
            && !plan
                .continuation
                .contains(NativeContinuationReason::SceneVisualDebt)
        {
            self.wake_authority
                .note_scene_visual_debt_without_wake_owner();
        }
        if visual_scene_debt {
            let wake_owner = if let Some(deadline) = plan.deadline {
                match deadline.owner {
                    NativeDeadlineOwner::FrameScheduler => "deadline:frame_scheduler",
                    NativeDeadlineOwner::PresentationTarget => "deadline:presentation_target",
                    NativeDeadlineOwner::AtomicCommitWatchdog => "deadline:atomic_commit_watchdog",
                    NativeDeadlineOwner::ExplicitSyncFallback => "deadline:explicit_sync_fallback",
                    NativeDeadlineOwner::XwaylandTimeout => "deadline:xwayland_timeout",
                    NativeDeadlineOwner::CursorResponse => "deadline:cursor_response",
                    NativeDeadlineOwner::CursorPlanningRetry => "deadline:cursor_planning_retry",
                    NativeDeadlineOwner::CursorSynchronousAdmission => {
                        "deadline:cursor_sync_admission"
                    }
                    NativeDeadlineOwner::ControlTimeout => "deadline:control_timeout",
                    NativeDeadlineOwner::SystemActionTransport => {
                        "deadline:system_action_transport"
                    }
                    NativeDeadlineOwner::SurfacePacing => "deadline:surface_pacing",
                    NativeDeadlineOwner::DmabufRetry => "deadline:dmabuf_retry",
                    NativeDeadlineOwner::OutputConfiguration => "deadline:output_configuration",
                    NativeDeadlineOwner::KeyboardRepeat => "deadline:keyboard_repeat",
                    NativeDeadlineOwner::ScratchMaintenance => "deadline:scratch_maintenance",
                    NativeDeadlineOwner::CursorAnimation => "deadline:cursor_animation",
                    NativeDeadlineOwner::SecondaryHold => "deadline:secondary_hold",
                }
            } else if plan
                .continuation
                .contains(NativeContinuationReason::FrameScheduler)
            {
                "continuation:frame_scheduler"
            } else if plan
                .continuation
                .contains(NativeContinuationReason::SceneVisualDebt)
            {
                "continuation:scene_visual_debt"
            } else if scene_visual_debt_has_external_wake_owner {
                if self.head.scanout.page_flip_pending() {
                    "external:pageflip"
                } else if self.head.atomic_commit_arbiter.atomic_commit_pending() {
                    "external:atomic_commit"
                } else if self.head.scanout.render_worker_busy() {
                    "external:render_worker"
                } else {
                    "external:render_fence"
                }
            } else {
                "none"
            };
            self.perf.log("native.scene_liveness", || {
                vec![
                    NativePerfField::u64(
                        "last_rendered_scene_generation",
                        self.head.last_rendered_scene_generation,
                    ),
                    NativePerfField::u64(
                        "scene_render_generation",
                        self.server.scene_render_generation(),
                    ),
                    NativePerfField::bool("visual_scene_debt", true),
                    NativePerfField::str("wake_owner", wake_owner),
                ]
            });
        }
        self.install_native_wake_plan(plan, now_ns)
    }
}

fn gate_scheduler_for_output_configuration(
    configuration_pending: bool,
    scheduler: NativeSchedulerWakeRequirement,
) -> NativeSchedulerWakeRequirement {
    if configuration_pending {
        NativeSchedulerWakeRequirement::None
    } else {
        scheduler
    }
}

#[cfg(test)]
mod output_configuration_scheduler_tests {
    use super::*;

    #[test]
    fn pending_output_configuration_suppresses_scheduler_admission_wakes() {
        let scheduler = NativeSchedulerWakeRequirement::ImmediatePresentation {
            action: SchedulerDecision::Render,
        };
        assert_eq!(
            gate_scheduler_for_output_configuration(true, scheduler),
            NativeSchedulerWakeRequirement::None
        );
        assert_eq!(
            gate_scheduler_for_output_configuration(false, scheduler),
            scheduler
        );
    }
}

#[cfg(test)]
mod scene_visual_debt_wake_owner_tests {
    use super::scene_visual_debt_has_external_wake_owner;

    #[test]
    fn busy_render_worker_owns_the_scene_debt_wake() {
        assert!(scene_visual_debt_has_external_wake_owner(
            false, false, false, true
        ));
        assert!(!scene_visual_debt_has_external_wake_owner(
            false, false, false, false
        ));
        assert!(scene_visual_debt_has_external_wake_owner(
            true, false, false, false
        ));
    }
}
