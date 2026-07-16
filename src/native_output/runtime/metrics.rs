use super::presentation::visual_target_deadline_for_mode;
use super::*;

impl NativeRuntime {
    pub(super) fn update_cycle_metrics(
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
                    NativePerfField::u64(
                        "direct_scanout_composited_render_ahead_suppressed",
                        counters.composited_render_ahead_suppressed,
                    ),
                ]);
            }
            if let Some(cursor) = self.atomic_cursor.as_ref() {
                fields.extend([
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
                self.xwayland.next_deadline_ns(),
            ),
        ))?;
        Ok(())
    }
}
