use super::*;

impl NativeRuntime {
    pub(super) fn arm_runtime_deadline(&mut self) -> NativeResult<()> {
        let now_ns = monotonic_now_ns()?;
        let pageflip_timeout_owner: NativePageflipTimeoutOwner =
            self.kms_commit_worker_transport.into();
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
        let scheduler_deadline = self.current_scheduler_wake_deadline(now_ns)?;
        let visual_scene_debt = super::commit_timing::logical_scene_changed(
            self.last_rendered_scene_generation,
            self.server.scene_render_generation(),
        );
        let scene_visual_debt_has_external_wake_owner = self.scanout.page_flip_pending()
            || self.atomic_commit_arbiter.atomic_commit_pending()
            || self.output_render_fence_token.is_some();
        let scene_visual_debt_continuation = visual_scene_debt
            && scheduler_deadline.is_none()
            && !scene_visual_debt_has_external_wake_owner;
        let plan = build_native_wake_plan(NativeWakePlanInputs {
            now_ns,
            scheduler_deadline,
            atomic_commit_watchdog_deadline_ns: atomic_commit_watchdog_deadline_for_timeout_owner(
                self.atomic_commit_arbiter.watchdog_deadline_ns(),
                pageflip_timeout_owner,
            ),
            explicit_sync_fallback_deadline_ns: self.acquire_watches.next_fallback_deadline_ns(),
            xwayland_timeout_deadline_ns: self.xwayland.next_deadline_ns(),
            cursor_response_deadline_ns: self.cursor_output_arbitration.wake_deadline_ns(now_ns),
            control_timeout_deadline_ns: control_timeout_deadline
                .filter(|deadline| *deadline > now_ns),
            surface_pacing_deadline_ns: surface_pacing_deadline,
            dmabuf_retry_deadline_ns: dmabuf_retry_deadline,
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
                    NativeDeadlineOwner::ControlTimeout => "deadline:control_timeout",
                    NativeDeadlineOwner::SurfacePacing => "deadline:surface_pacing",
                    NativeDeadlineOwner::DmabufRetry => "deadline:dmabuf_retry",
                }
            } else if plan
                .continuation
                .contains(NativeContinuationReason::SceneVisualDebt)
            {
                "continuation:scene_visual_debt"
            } else if scene_visual_debt_has_external_wake_owner {
                if self.scanout.page_flip_pending() {
                    "external:pageflip"
                } else if self.atomic_commit_arbiter.atomic_commit_pending() {
                    "external:atomic_commit"
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
                        self.last_rendered_scene_generation,
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
