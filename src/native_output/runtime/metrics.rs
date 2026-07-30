use super::planner::visual_target_deadline_for_mode;
use super::*;

impl NativeRuntime {
    pub(super) fn update_cycle_metrics(
        &mut self,
        cycle: &NativeCycleState,
        scheduler_decision: SchedulerDecision,
    ) -> NativeResult<()> {
        if let Some(worker) = self.kms_commit_worker.as_ref() {
            worker.record_runtime_queue_state(
                usize::from(self.atomic_commit_arbiter.worker_job_queued()),
                self.atomic_commit_arbiter.kernel_commit_submitted(),
            );
        }
        let perf = self.perf;
        perf.log("native.scheduler", || {
            let (
                xwm_drain_max_us,
                xwm_translation_max_us,
                xwm_command_execution_max_us,
                adoption_deadline_max_us,
                xwm_events_per_cycle_max,
                xwm_commands_per_cycle_max,
            ) = self.xwayland.xwayland_timing_snapshot();
            let fullscreen = self.server.fullscreen_render_plan_metrics();
            let transaction_counters = self.output_transactions.counters();
            let presented = self.scanout.direct_scanout_presented_info();
            let submitted = self.scanout.direct_scanout_submitted_info();
            // Queued identity belongs to the Atomic arbiter; physical resources
            // are reported separately from DirectPrimaryOwnership below.
            let direct_pending = self
                .atomic_commit_arbiter
                .pending_atomic_kind()
                .is_some_and(|kind| matches!(kind, AtomicCommitKind::DirectPrimary { .. }));
            let mut fields = vec![
                NativePerfField::str("decision", format!("{scheduler_decision:?}")),
                NativePerfField::str(
                    "cursor_scheduling_policy",
                    self.cursor_scheduling_policy.as_str(),
                ),
                NativePerfField::u64(
                    "perf_records_suppressed",
                    NativePerfLogger::suppressed_records(),
                ),
                NativePerfField::bool(
                    "dmabuf_feedback_egl_wayland2_compat_effective",
                    self.dmabuf_feedback_compatibility.effective
                        != DmabufFeedbackCompatibilityEffective::Off,
                ),
                NativePerfField::str(
                    "dmabuf_feedback_egl_wayland2_compat_mode",
                    self.dmabuf_feedback_compatibility.effective.as_str(),
                ),
                NativePerfField::u64(
                    "dmabuf_feedback_scanout_target_normalizations",
                    self.dmabuf_feedback_compat_metrics
                        .scanout_target_normalizations,
                ),
                NativePerfField::u64(
                    "dmabuf_feedback_scanout_target_normalization_rejections",
                    self.dmabuf_feedback_compat_metrics
                        .scanout_target_normalization_rejections,
                ),
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
                    "atomic_commits_submitted_total",
                    self.atomic_commit_arbiter.atomic_commits_submitted_total(),
                ),
                NativePerfField::u64(
                    "atomic_commits_completed_total",
                    self.atomic_commit_arbiter.atomic_commits_completed_total(),
                ),
                NativePerfField::u64(
                    "atomic_commit_watchdog_timeouts_total",
                    self.atomic_commit_arbiter
                        .atomic_commit_watchdog_timeouts_total(),
                ),
                NativePerfField::u64(
                    "atomic_cursor_watchdog_timeouts",
                    self.atomic_commit_arbiter.cursor_watchdog_timeouts(),
                ),
                NativePerfField::u64(
                    "atomic_primary_watchdog_timeouts",
                    self.atomic_commit_arbiter.primary_watchdog_timeouts(),
                ),
                NativePerfField::str(
                    "kms_commit_transport",
                    match self.kms_commit_worker_transport {
                        crate::native_output::kms_worker::KmsCommitWorkerTransport::Synchronous => {
                            "sync"
                        }
                        crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker => {
                            "worker"
                        }
                    },
                ),
                NativePerfField::usize(
                    "worker_queue_depth",
                    self.kms_commit_worker
                        .as_ref()
                        .map_or(0, |worker| worker.queue_depth()),
                ),
                NativePerfField::bool(
                    "worker_inflight",
                    self.kms_commit_worker
                        .as_ref()
                        .is_some_and(|worker| worker.inflight()),
                ),
                NativePerfField::bool(
                    "worker_submit_active",
                    self.kms_commit_worker
                        .as_ref()
                        .is_some_and(|worker| worker.submission_active()),
                ),
                NativePerfField::u64("cursor_pageflip_early_returns", 0),
                NativePerfField::u64(
                    "cursor_response_windows_opened",
                    self.cursor_output_arbitration.response_windows_opened(),
                ),
                NativePerfField::u64(
                    "cursor_changes_coalesced",
                    self.cursor_output_arbitration.changes_coalesced(),
                ),
                NativePerfField::u64(
                    "plane_delta_plans",
                    self.cursor_output_arbitration.plane_delta_plans(),
                ),
                NativePerfField::u64(
                    "plane_delta_submissions",
                    self.cursor_output_arbitration.plane_delta_submissions(),
                ),
                NativePerfField::u64(
                    "plane_delta_deferred_for_primary",
                    self.cursor_output_arbitration
                        .plane_delta_deferred_for_primary(),
                ),
                NativePerfField::u64(
                    "cursor_state_piggybacked",
                    self.cursor_output_arbitration.cursor_state_piggybacked(),
                ),
                NativePerfField::u64(
                    "cursor_idle_hardware_updates",
                    self.cursor_output_arbitration.idle_hardware_updates(),
                ),
                NativePerfField::u64(
                    "cursor_idle_software_updates",
                    self.cursor_output_arbitration.idle_software_updates(),
                ),
                NativePerfField::bool(
                    "cursor_response_window_open",
                    self.cursor_output_arbitration.pending(),
                ),
                NativePerfField::u64(
                    "cursor_response_deadline_ns",
                    self.cursor_output_arbitration.deadline_ns().unwrap_or(0),
                ),
                NativePerfField::str(
                    "atomic_pending_commit_kind",
                    self.atomic_commit_arbiter
                        .pending_atomic_kind()
                        .map(|kind| format!("{kind:?}"))
                        .unwrap_or_else(|| "none".to_string()),
                ),
                NativePerfField::u64(
                    "mismatched_pageflip_events",
                    self.mismatched_pageflip_events,
                ),
                NativePerfField::u64("stale_pageflip_events", self.stale_pageflip_events),
                NativePerfField::u64("presentations", self.presentation_cadence.presentations()),
                NativePerfField::usize("presentation_trace_events", self.presentation_trace.len()),
                NativePerfField::u64(
                    "presentation_trace_dropped",
                    self.presentation_trace.dropped(),
                ),
                NativePerfField::u64("xwm_drain_max_us", xwm_drain_max_us),
                NativePerfField::u64("xwm_translation_max_us", xwm_translation_max_us),
                NativePerfField::u64("xwm_command_execution_max_us", xwm_command_execution_max_us),
                NativePerfField::u64("adoption_deadline_max_us", adoption_deadline_max_us),
                NativePerfField::u64("xwm_events_per_cycle_max", xwm_events_per_cycle_max),
                NativePerfField::u64("xwm_commands_per_cycle_max", xwm_commands_per_cycle_max),
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
            if let Some(worker) = self.kms_commit_worker.as_ref() {
                let metrics = worker.metrics_snapshot();
                fields.extend([
                    NativePerfField::u64("worker_jobs_enqueued", metrics.jobs_enqueued),
                    NativePerfField::u64("worker_jobs_submitted", metrics.jobs_submitted),
                    NativePerfField::u64("worker_jobs_rejected", metrics.jobs_rejected),
                    NativePerfField::u64("worker_queue_full", metrics.queue_full),
                    NativePerfField::u64(
                        "worker_admission_contention",
                        metrics.admission_contention,
                    ),
                    NativePerfField::u64("worker_busy_deferrals", metrics.busy_deferrals),
                    NativePerfField::u64("worker_busy_retries", metrics.busy_retries),
                    NativePerfField::u64("worker_busy_exhausted", metrics.busy_exhausted),
                    NativePerfField::u64("worker_late_wakeups", metrics.late_wakeups),
                    NativePerfField::u64(
                        "worker_submit_duration_ns_total",
                        metrics.submit_duration_ns_total,
                    ),
                    NativePerfField::u64(
                        "worker_submit_duration_ns_max",
                        metrics.submit_duration_ns_max,
                    ),
                    NativePerfField::u64("worker_queue_wait_ns_total", metrics.queue_wait_ns_total),
                    NativePerfField::u64("worker_queue_wait_ns_max", metrics.queue_wait_ns_max),
                    NativePerfField::u64("worker_pageflip_timeouts", metrics.pageflip_timeouts),
                    NativePerfField::u64("worker_main_thread_stalls", metrics.main_thread_stalls),
                    NativePerfField::u64(
                        "worker_driver_timeout_suspicions",
                        metrics.driver_timeout_suspicions,
                    ),
                    NativePerfField::u64("worker_result_mismatches", metrics.result_mismatches),
                    NativePerfField::u64("worker_fatal_events", metrics.fatal_events),
                    NativePerfField::u64("worker_quiesce_count", metrics.quiesce_count),
                    NativePerfField::u64("worker_quiesce_ns_total", metrics.quiesce_ns_total),
                    NativePerfField::u64("worker_join_ns_total", metrics.join_ns_total),
                    NativePerfField::u64(
                        "worker_input_fence_retry_attempts",
                        metrics.input_fence_retry_attempts,
                    ),
                    NativePerfField::u64(
                        "worker_input_fence_retry_preserved",
                        metrics.input_fence_retry_preserved,
                    ),
                    NativePerfField::u64(
                        "worker_scheduler_queued_cancellations",
                        metrics.scheduler_queued_cancellations,
                    ),
                    NativePerfField::u64(
                        "worker_scheduler_cancel_mismatches",
                        metrics.scheduler_cancel_mismatches,
                    ),
                    NativePerfField::u64(
                        "worker_cursor_pageflip_acks",
                        metrics.cursor_pageflip_acks,
                    ),
                    NativePerfField::u64(
                        "worker_primary_pageflip_acks",
                        metrics.primary_pageflip_acks,
                    ),
                    NativePerfField::u64(
                        "worker_duplicate_pageflip_acks",
                        metrics.duplicate_pageflip_acks,
                    ),
                    NativePerfField::u64(
                        "worker_eventfd_notification_failures",
                        metrics.eventfd_notification_failures,
                    ),
                    NativePerfField::u64(
                        "worker_unnotified_fatal_health_checks",
                        metrics.unnotified_fatal_health_checks,
                    ),
                    NativePerfField::u64("worker_runtime_queue_depth", metrics.runtime_queue_depth),
                    NativePerfField::u64(
                        "worker_runtime_queue_depth_max",
                        metrics.runtime_queue_depth_max,
                    ),
                    NativePerfField::u64(
                        "worker_shutdown_admission_stops",
                        metrics.shutdown_admission_stops,
                    ),
                    NativePerfField::u64(
                        "worker_shutdown_queued_jobs_returned",
                        metrics.shutdown_queued_jobs_returned,
                    ),
                    NativePerfField::u64(
                        "worker_shutdown_queued_jobs_settled",
                        metrics.shutdown_queued_jobs_settled,
                    ),
                    NativePerfField::u64(
                        "worker_shutdown_ack_suppressed_next_submit",
                        metrics.shutdown_ack_suppressed_next_submit,
                    ),
                    NativePerfField::u64(
                        "worker_shutdown_inflight_abandons",
                        metrics.shutdown_inflight_abandons,
                    ),
                    NativePerfField::u64(
                        "cursor_worker_jobs_queued",
                        metrics.cursor_worker_jobs_queued,
                    ),
                    NativePerfField::u64(
                        "cursor_worker_submits_confirmed",
                        metrics.cursor_worker_submits_confirmed,
                    ),
                    NativePerfField::u64(
                        "cursor_worker_rejections_retryable",
                        metrics.cursor_worker_rejections_retryable,
                    ),
                    NativePerfField::u64(
                        "cursor_worker_rejections_fallback",
                        metrics.cursor_worker_rejections_fallback,
                    ),
                    NativePerfField::u64(
                        "cursor_worker_arbitration_consumed",
                        metrics.cursor_worker_arbitration_consumed,
                    ),
                    NativePerfField::u64(
                        "cursor_worker_epoch_mismatches",
                        metrics.cursor_worker_epoch_mismatches,
                    ),
                    NativePerfField::u64(
                        "worker_pacing_submits_confirmed",
                        metrics.worker_pacing_submits_confirmed,
                    ),
                    NativePerfField::u64(
                        "worker_pacing_pre_submit_rejections",
                        metrics.worker_pacing_pre_submit_rejections,
                    ),
                    NativePerfField::u64("worker_kernel_inflight", metrics.runtime_kernel_inflight),
                    NativePerfField::bool("worker_active", worker.submission_active()),
                ]);
            }
            if let Some(summary) = self.timing_scopes.get("wayland_dispatch") {
                fields.extend([
                    NativePerfField::u64("wayland_dispatch_count", summary.count),
                    NativePerfField::u64("wayland_dispatch_max_us", summary.max_ns / 1_000),
                ]);
            }
            if let Some(summary) = self.timing_scopes.get("xwm_dispatch") {
                fields.extend([
                    NativePerfField::u64("xwm_dispatch_count", summary.count),
                    NativePerfField::u64("xwm_dispatch_max_us", summary.max_ns / 1_000),
                ]);
            }
            if let Some(summary) = self.timing_scopes.get("prepare_frame") {
                fields.extend([
                    NativePerfField::u64("prepare_frame_count", summary.count),
                    NativePerfField::u64("prepare_frame_max_us", summary.max_ns / 1_000),
                ]);
            }
            if let Some(summary) = self.timing_scopes.get("egl_draw") {
                fields.extend([
                    NativePerfField::u64("egl_draw_count", summary.count),
                    NativePerfField::u64("egl_draw_max_us", summary.max_ns / 1_000),
                ]);
            }
            fields.extend([
                NativePerfField::bool(
                    "atomic_cursor_plane_available",
                    self.kms_backend
                        .atomic()
                        .is_some_and(|atomic| atomic.discovery().cursor_plane.is_some()),
                ),
                NativePerfField::bool(
                    "atomic_cursor_hardware_active",
                    self.atomic_cursor.is_some()
                        && self.cursor_render_mode == NativeCursorRenderMode::Hardware,
                ),
                NativePerfField::bool(
                    "direct_scanout_active",
                    self.confirmed_primary_assignment
                        .is_some_and(|assignment| assignment.is_direct()),
                ),
                NativePerfField::bool(
                    "direct_scanout_qualified",
                    self.direct_scanout_qualification.is_qualified(),
                ),
                NativePerfField::str(
                    "direct_scanout_qualification",
                    self.direct_scanout_qualification.status_str(),
                ),
                NativePerfField::bool("direct_scanout_pending", direct_pending),
                NativePerfField::bool(
                    "direct_scanout_fallback_active",
                    self.direct_fallback_tracker.is_some(),
                ),
                NativePerfField::str(
                    "direct_scanout_first_blocker",
                    self.scanout
                        .direct_scanout_counters()
                        .and_then(|counters| counters.first_blocker)
                        .unwrap_or("none"),
                ),
            ]);
            if let Some(pending) = self.atomic_commit_arbiter.pending_atomic_commit() {
                fields.extend([
                    NativePerfField::u64("atomic_pending_token", pending.token.get()),
                    NativePerfField::u64("atomic_pending_crtc", u64::from(pending.crtc_id)),
                    NativePerfField::u64("atomic_pending_generation", pending.generation),
                    NativePerfField::u64("atomic_pending_submitted_at_ns", pending.submitted_at_ns),
                    NativePerfField::u64(
                        "atomic_pending_watchdog_deadline_ns",
                        pending.watchdog_deadline_ns,
                    ),
                ]);
            }
            if let Some((_buffer, _framebuffer, format, modifier)) =
                self.scanout.direct_scanout_info()
            {
                fields.extend([
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
                    NativePerfField::u64(
                        "direct_scanout_real_submit_rejections",
                        counters.submit_rejections,
                    ),
                    NativePerfField::u64("direct_scanout_submissions", counters.submissions),
                    NativePerfField::u64("direct_scanout_presentations", counters.presentations),
                    NativePerfField::u64("direct_scanout_entries", counters.entries),
                    NativePerfField::u64(
                        "direct_scanout_replacements",
                        counters.direct_replacements,
                    ),
                    NativePerfField::u64("direct_scanout_exits", counters.exits),
                    NativePerfField::u64(
                        "direct_combined_cursor_rejection",
                        counters.combined_cursor_rejections,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_fallback_redraws",
                        counters.fallback_redraws,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_same_content_suppressed",
                        counters.same_buffer_suppressed,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_same_buffer_resubmissions",
                        counters.same_buffer_resubmissions,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_same_buffer_suppressed",
                        counters.same_buffer_suppressed,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_out_fences_received",
                        counters.out_fences_received,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_out_fence_missing",
                        counters.out_fence_missing,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_test_only_p50_us",
                        counters.test_only_timing.percentile_ns(50) / 1_000,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_test_only_p95_us",
                        counters.test_only_timing.percentile_ns(95) / 1_000,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_test_only_p99_us",
                        counters.test_only_timing.percentile_ns(99) / 1_000,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_test_only_max_us",
                        counters.test_only_timing.max_ns / 1_000,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_test_only_duration_ns_last",
                        counters.test_only_timing.last_ns,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_test_only_duration_ns_max",
                        counters.test_only_timing.max_ns,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_test_only_duration_ns_total",
                        counters.test_only_timing.total_ns,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_real_submit_p50_us",
                        counters.real_submit_timing.percentile_ns(50) / 1_000,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_real_submit_p95_us",
                        counters.real_submit_timing.percentile_ns(95) / 1_000,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_real_submit_p99_us",
                        counters.real_submit_timing.percentile_ns(99) / 1_000,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_real_submit_max_us",
                        counters.real_submit_timing.max_ns / 1_000,
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
                    NativePerfField::u64(
                        "direct_scanout_composited_render_ahead_suppressed",
                        counters.composited_render_ahead_suppressed,
                    ),
                    NativePerfField::u64("direct_scanout_blocker_set", counters.blocker_set),
                    NativePerfField::u64(
                        "direct_scanout_worker_admission_rejected",
                        counters.worker_admission_rejected,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_worker_queue_overflow",
                        counters.worker_queue_overflow,
                    ),
                    NativePerfField::u64("direct_scanout_live_leases", counters.live_leases),
                    NativePerfField::u64(
                        "direct_scanout_validation_cache_hits",
                        counters.validation_cache_hits,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_validation_cache_misses",
                        counters.validation_cache_misses,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_fallback_cycles",
                        counters.fallback_cycles,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_fallback_cycles_current",
                        counters.fallback_cycles_current,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_fallback_cycles_last",
                        counters.fallback_cycles_last,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_fallback_cycles_max",
                        counters.fallback_cycles_max,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_duplicate_feedback",
                        counters.duplicate_feedback,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_duplicate_settlement",
                        transaction_counters.duplicate_settlement_attempts,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_early_release_prevented",
                        counters.early_release_prevented,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_early_release_violations",
                        counters.early_release_violations,
                    ),
                    NativePerfField::u64(
                        "dmabuf_feedback_unchanged_rebuilds",
                        counters.dmabuf_feedback_unchanged_rebuilds,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_callback_owner_leak_events",
                        counters.callback_owner_leak_events,
                    ),
                    NativePerfField::u64(
                        "direct_scanout_callback_owner_leaked_callbacks",
                        counters.callback_owner_leaked_callbacks,
                    ),
                ]);
            }
            fields.extend([
                NativePerfField::str(
                    "direct_scanout_presented_surface",
                    presented.map_or_else(|| "none".to_string(), |value| value.0.to_string()),
                ),
                NativePerfField::str(
                    "direct_scanout_presented_buffer",
                    presented.map_or_else(|| "none".to_string(), |value| value.1.to_string()),
                ),
                NativePerfField::str(
                    "direct_scanout_presented_framebuffer",
                    presented.map_or_else(|| "none".to_string(), |value| value.2.to_string()),
                ),
                NativePerfField::str(
                    "direct_scanout_presented_content_epoch",
                    presented.map_or_else(|| "none".to_string(), |value| value.3.to_string()),
                ),
                NativePerfField::str(
                    "direct_scanout_submitted_surface",
                    submitted.map_or_else(|| "none".to_string(), |value| value.0.to_string()),
                ),
                NativePerfField::str(
                    "direct_scanout_submitted_buffer",
                    submitted.map_or_else(|| "none".to_string(), |value| value.1.to_string()),
                ),
                NativePerfField::str(
                    "direct_scanout_submitted_framebuffer",
                    submitted.map_or_else(|| "none".to_string(), |value| value.2.to_string()),
                ),
                NativePerfField::str(
                    "direct_scanout_submitted_content_epoch",
                    submitted.map_or_else(|| "none".to_string(), |value| value.3.to_string()),
                ),
            ]);
            if let Some(cursor) = self.atomic_cursor.as_ref() {
                fields.extend([
                    NativePerfField::u64(
                        "atomic_cursor_image_uploads",
                        cursor.counters.image_uploads,
                    ),
                    NativePerfField::u64(
                        "client_cursor_hw_image_uploads",
                        cursor.counters.client_image_uploads,
                    ),
                    NativePerfField::u64(
                        "client_cursor_image_cache_hits",
                        cursor.counters.image_cache_hits,
                    ),
                    NativePerfField::u64(
                        "client_cursor_hw_position_submissions",
                        cursor.counters.position_submissions,
                    ),
                    NativePerfField::u64(
                        "client_cursor_hw_primary_submissions",
                        cursor.counters.primary_submissions,
                    ),
                    NativePerfField::u64(
                        "atomic_cursor_updates_requested",
                        cursor.counters.updates_requested,
                    ),
                    NativePerfField::u64(
                        "atomic_cursor_updates_submitted",
                        cursor.counters.updates_submitted,
                    ),
                    NativePerfField::u64(
                        "atomic_cursor_updates_completed",
                        cursor.counters.updates_completed,
                    ),
                    NativePerfField::u64(
                        "atomic_cursor_updates_coalesced",
                        cursor.counters.updates_coalesced,
                    ),
                    NativePerfField::u64(
                        "atomic_cursor_hidden_updates_suppressed",
                        cursor.counters.hidden_updates_suppressed,
                    ),
                    NativePerfField::u64(
                        "atomic_cursor_test_failures",
                        cursor.counters.test_failures,
                    ),
                    NativePerfField::u64(
                        "atomic_cursor_submit_failures",
                        cursor.counters.submit_failures,
                    ),
                    NativePerfField::u64(
                        "atomic_cursor_software_fallbacks",
                        cursor.counters.software_fallbacks,
                    ),
                    NativePerfField::u64(
                        "composed_cursor_fallback",
                        cursor.counters.composed_cursor_fallbacks,
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
        let atomic_commit_deadline = self.atomic_commit_arbiter.watchdog_deadline_ns();
        self.frame_pacing.note_deadline_state(
            scheduler_decision,
            monotonic_now_ns()?,
            scheduler_deadline,
            visual_deadline,
            self.frame_scheduler.ready_frame_queued() || self.scanout.ready_frame_queued(),
            cycle.wakeup.reasons.timer(),
        );
        self.event_loop.arm_deadline(earliest_native_deadline(
            earliest_native_deadline(
                earliest_native_deadline(scheduler_deadline, visual_deadline),
                atomic_commit_deadline,
            ),
            earliest_native_deadline(
                self.acquire_watches.next_fallback_deadline_ns(),
                earliest_native_deadline(
                    self.xwayland.next_deadline_ns(),
                    self.cursor_output_arbitration.deadline_ns(),
                ),
            ),
        ))?;
        Ok(())
    }
}
