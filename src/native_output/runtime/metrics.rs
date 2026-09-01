use super::resource_efficiency::ResourceEfficiencyMetrics;
use super::*;
use crate::egl_renderer::{FullRepaintReason, GlesSceneFrameStats, RepaintMode};
use crate::native_output::{
    KmsTarget,
    kms_worker::{KmsCommitWorkerTransport, WorkerMetricsSnapshot, WorkerTimingSnapshot},
    scanout::NativePaintStats,
};
use oblivion_one::control_snapshots::{
    BufferingPerformanceSnapshot, KmsPerformanceSnapshot, PerformanceSnapshot,
    RepaintPerformanceSnapshot, SignedTimingSummarySnapshot, TimingSummarySnapshot,
    WorkerTimingPerformanceSnapshot,
};
use oblivion_one::native::scheduler::apply_atomic_commit_lane_guard;
use std::{collections::BTreeMap, time::Duration};

const RENDER_REPAINT_REASON_COUNT: usize = 12;
const RENDER_BUFFER_AGE_BUCKET_COUNT: usize = 6;

#[derive(Debug, Default)]
pub(super) struct NativeRenderTelemetry {
    compositor_cpu_render: TimingSummary,
    skip_frames: u64,
    partial_frames: u64,
    full_frames: u64,
    buffer_age_buckets: [u64; RENDER_BUFFER_AGE_BUCKET_COUNT],
    partial_repair_pixels: u64,
    full_output_pixels: u64,
    full_repaint_reasons: [u64; RENDER_REPAINT_REASON_COUNT],
    pub(super) resource_efficiency: ResourceEfficiencyMetrics,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct NativeRenderTelemetrySnapshot {
    pub(super) compositor_cpu_render: TimingSummary,
    pub(super) skip_frames: u64,
    pub(super) partial_frames: u64,
    pub(super) full_frames: u64,
    pub(super) buffer_age_buckets: [u64; RENDER_BUFFER_AGE_BUCKET_COUNT],
    pub(super) partial_repair_pixels: u64,
    pub(super) full_output_pixels: u64,
    pub(super) full_repaint_reasons: [u64; RENDER_REPAINT_REASON_COUNT],
}

impl NativeRenderTelemetry {
    pub(super) fn record_skipped(&mut self, render_us: u64) {
        self.record_render_us(render_us);
        self.record_repaint(RepaintMode::Skip, None, 0, 0, None);
    }

    pub(super) fn record_render_us(&mut self, render_us: u64) {
        self.compositor_cpu_render
            .record(render_us.saturating_mul(1_000));
    }

    pub(super) fn record_rendered(
        &mut self,
        render_us: u64,
        stats: GlesSceneFrameStats,
        output_width: u32,
        output_height: u32,
    ) {
        self.record_render_us(render_us);
        self.record_repaint(
            stats.repaint_mode,
            stats.buffer_age,
            u64::from(output_width).saturating_mul(u64::from(output_height)),
            stats.repair_damage_pixels,
            stats.fallback_reason,
        );
    }

    pub(super) fn record_atomic(
        &mut self,
        render_us: u64,
        stats: GlesSceneFrameStats,
        target: KmsTarget,
    ) {
        self.record_rendered(render_us, stats, target.width, target.height);
    }

    pub(super) fn record_native_paint(&mut self, stats: NativePaintStats) {
        self.record_render_us(stats.render_us);
        if let Some(repaint) = stats.gles_repaint {
            self.record_repaint(
                repaint.repaint_mode,
                repaint.buffer_age,
                u64::from(stats.width).saturating_mul(u64::from(stats.height)),
                repaint.repair_damage_pixels,
                repaint.fallback_reason,
            );
        }
    }

    pub(super) fn record_repaint(
        &mut self,
        mode: RepaintMode,
        buffer_age: Option<u32>,
        output_pixels: u64,
        repair_pixels: u64,
        fallback_reason: Option<FullRepaintReason>,
    ) {
        match mode {
            RepaintMode::Skip => self.skip_frames = self.skip_frames.saturating_add(1),
            RepaintMode::Partial => {
                self.partial_frames = self.partial_frames.saturating_add(1);
                self.partial_repair_pixels =
                    self.partial_repair_pixels.saturating_add(repair_pixels);
            }
            RepaintMode::Full => {
                self.full_frames = self.full_frames.saturating_add(1);
                self.full_output_pixels = self.full_output_pixels.saturating_add(output_pixels);
            }
        }
        let age_bucket = match buffer_age {
            Some(0) => 0,
            Some(1) => 1,
            Some(2) => 2,
            Some(3) => 3,
            Some(_) => 4,
            None => 5,
        };
        self.buffer_age_buckets[age_bucket] = self.buffer_age_buckets[age_bucket].saturating_add(1);
        if let Some(reason) = fallback_reason {
            self.full_repaint_reasons[reason.histogram_index()] =
                self.full_repaint_reasons[reason.histogram_index()].saturating_add(1);
        }
    }

    pub(super) const fn snapshot(&self) -> NativeRenderTelemetrySnapshot {
        NativeRenderTelemetrySnapshot {
            compositor_cpu_render: self.compositor_cpu_render,
            skip_frames: self.skip_frames,
            partial_frames: self.partial_frames,
            full_frames: self.full_frames,
            buffer_age_buckets: self.buffer_age_buckets,
            partial_repair_pixels: self.partial_repair_pixels,
            full_output_pixels: self.full_output_pixels,
            full_repaint_reasons: self.full_repaint_reasons,
        }
    }

    fn control_snapshot(&self) -> RepaintPerformanceSnapshot {
        let snapshot = self.snapshot();
        let age_names = ["0", "1", "2", "3", "4_plus", "unknown"];
        let buffer_age_buckets = age_names
            .into_iter()
            .zip(snapshot.buffer_age_buckets)
            .map(|(name, count)| (name.to_string(), count))
            .collect();
        let reasons = [
            FullRepaintReason::CurrentDamageFull,
            FullRepaintReason::FirstFrameOrInvalidated,
            FullRepaintReason::BufferAgeUnsupported,
            FullRepaintReason::PartialRenderRepairUnsupported,
            FullRepaintReason::BufferAgeZero,
            FullRepaintReason::BufferAgeInvalid,
            FullRepaintReason::BufferAgeQueryFailed,
            FullRepaintReason::InsufficientHistory,
            FullRepaintReason::TooManyRectangles,
            FullRepaintReason::DamageAreaThreshold,
            FullRepaintReason::ForcedFull,
            FullRepaintReason::PartialRepaintDisabled,
        ];
        let full_repaint_reasons = reasons
            .into_iter()
            .zip(snapshot.full_repaint_reasons)
            .map(|(reason, count)| (reason.as_str().to_string(), count))
            .collect();
        RepaintPerformanceSnapshot {
            skip_frames: snapshot.skip_frames,
            partial_frames: snapshot.partial_frames,
            full_frames: snapshot.full_frames,
            buffer_age_buckets,
            partial_repair_pixels: snapshot.partial_repair_pixels,
            full_output_pixels: snapshot.full_output_pixels,
            full_repaint_reasons,
        }
    }
}

fn timing_summary_snapshot(summary: TimingSummary) -> TimingSummarySnapshot {
    let total_us = summary.total_ns / 1_000;
    TimingSummarySnapshot {
        count: summary.count,
        total_us,
        last_us: summary.last_ns / 1_000,
        mean_us: if summary.count == 0 {
            0
        } else {
            total_us / summary.count
        },
        p50_us: summary.percentile_ns(50) / 1_000,
        p95_us: summary.percentile_ns(95) / 1_000,
        p99_us: summary.percentile_ns(99) / 1_000,
        max_us: summary.max_ns / 1_000,
    }
}

fn worker_timing_summary_snapshot(
    summary: crate::native_output::kms_worker::TimingSummarySnapshot,
) -> TimingSummarySnapshot {
    TimingSummarySnapshot {
        count: summary.count,
        total_us: summary.total_ns / 1_000,
        last_us: summary.last_ns / 1_000,
        mean_us: summary.mean_ns / 1_000,
        p50_us: summary.p50_ns / 1_000,
        p95_us: summary.p95_ns / 1_000,
        p99_us: summary.p99_ns / 1_000,
        max_us: summary.max_ns / 1_000,
    }
}

fn worker_signed_timing_summary_snapshot(
    summary: crate::native_output::kms_worker::SignedTimingSummarySnapshot,
) -> SignedTimingSummarySnapshot {
    SignedTimingSummarySnapshot {
        count: summary.count,
        total_us: summary.total_ns / 1_000,
        last_us: summary.last_ns / 1_000,
        mean_us: summary.mean_ns / 1_000,
        p50_us: summary.p50_ns / 1_000,
        p95_us: summary.p95_ns / 1_000,
        p99_us: summary.p99_ns / 1_000,
        min_us: summary.min_ns / 1_000,
        max_us: summary.max_ns / 1_000,
    }
}

fn worker_timing_snapshot(snapshot: WorkerTimingSnapshot) -> WorkerTimingPerformanceSnapshot {
    WorkerTimingPerformanceSnapshot {
        submit_wake_lateness: worker_signed_timing_summary_snapshot(snapshot.submit_wake_lateness),
        pre_submit_duration: worker_timing_summary_snapshot(snapshot.pre_submit_duration),
        dispatch_duration: worker_timing_summary_snapshot(snapshot.dispatch_duration),
        ioctl_duration: worker_timing_summary_snapshot(snapshot.ioctl_duration),
        queue_residency: worker_timing_summary_snapshot(snapshot.queue_residency),
        submit_earliness: worker_signed_timing_summary_snapshot(snapshot.submit_earliness),
        submit_return_earliness: worker_signed_timing_summary_snapshot(
            snapshot.submit_return_earliness,
        ),
        submit_ack_delay: worker_timing_summary_snapshot(snapshot.submit_ack_delay),
        pageflip_ack_delay: worker_timing_summary_snapshot(snapshot.pageflip_ack_delay),
        test_only_duration: worker_timing_summary_snapshot(snapshot.test_only_duration),
        dispatch_budget_us: snapshot.dispatch_budget_ns / 1_000,
        late_before_ioctl: snapshot.late_before_ioctl,
        late_after_ioctl: snapshot.late_after_ioctl,
        test_only_count: snapshot.test_only_count,
    }
}

impl NativeRuntime {
    pub(super) fn performance_snapshot(&self) -> PerformanceSnapshot {
        let buffering = self.frame_pacing.buffering_metrics();
        let pacing = self.frame_pacing.timing_metrics();
        let worker = self
            .kms_commit_worker
            .as_ref()
            .map_or_else(WorkerMetricsSnapshot::default, |worker| {
                worker.metrics_snapshot()
            });
        let presentation_timing = self.presentation_timing;
        let presentation_timing_snapshot = presentation_timing.snapshot();
        let service_prediction = self.render_journal.prediction_with_kms_guard(
            Duration::from_nanos(presentation_timing.mode().refresh_interval_ns()),
            presentation_timing.apply_guard_ns(),
        );
        let compositor_cpu_render =
            timing_summary_snapshot(self.render_telemetry.snapshot().compositor_cpu_render);
        let mut timing_scopes = BTreeMap::new();
        for (name, summary) in &self.timing_scopes {
            timing_scopes.insert((*name).to_string(), timing_summary_snapshot(*summary));
        }
        PerformanceSnapshot {
            compositor_cpu_render,
            repaint: self.render_telemetry.control_snapshot(),
            buffering: BufferingPerformanceSnapshot {
                reactive_double_frames: buffering.reactive_double_frames,
                predictive_triple_frames: buffering.predictive_triple_frames,
                future_primary_credit: self.adaptive_buffering.future_primary_credit(),
                extra_credit_grants: self.adaptive_buffering.extra_credit_grants(),
                extra_credit_revokes: self.adaptive_buffering.extra_credit_revokes(),
                o1_credit2_useful_hits: buffering.o1_credit2_useful_hits,
                o1_credit2_unnecessary_hits: buffering.o1_credit2_unnecessary_hits,
                o1_credit2_ineffective_misses: buffering.o1_credit2_ineffective_misses,
                o1_credit2_granted_not_consumed: buffering.o1_credit2_granted_not_consumed,
                o1_credit2_drain_events: buffering.o1_credit2_drain_events,
                o1_credit2_refill_suppressed_while_draining: buffering
                    .o1_credit2_refill_suppressed_while_draining,
                pre_render_abandoned: self.presentation_deadline.pre_render_abandoned(),
                predicted_render_ready_service_ns: service_prediction
                    .main_event_loop_wake_guard_ns
                    .saturating_add(service_prediction.render_risk_ns),
                predicted_kms_lead_ns: service_prediction.kms_total_lead_ns,
                predicted_total_service_ns: service_prediction.total_cost_ns,
                last_overlap_required_ns: self.adaptive_buffering.last_overlap_required_ns(),
                positive_overlap_observations: self
                    .adaptive_buffering
                    .positive_overlap_observations(),
                nonpositive_overlap_observations: self
                    .adaptive_buffering
                    .nonpositive_overlap_observations(),
                render_ahead_attempts: buffering.render_ahead_attempts,
                render_ahead_ready: buffering.render_ahead_ready,
                ready_submits: buffering.ready_submits,
                triple_entries_predicted: buffering.triple_entries_predicted,
                triple_entries_render_miss: buffering.triple_entries_render_miss,
                triple_entries_submit_miss: buffering.triple_entries_submit_miss,
                triple_entries_presentation_miss: buffering.triple_entries_presentation_miss,
                triple_exits: buffering.triple_exits,
            },
            kms: KmsPerformanceSnapshot {
                mode_refresh_interval_ns: presentation_timing.mode().refresh_interval_ns(),
                mode_blanking_interval_ns: presentation_timing.mode().blanking_interval_ns(),
                base_apply_guard_ns: presentation_timing.base_mode_guard_ns(),
                adaptive_apply_guard_ns: presentation_timing.adaptive_apply_guard_ns(),
                total_apply_guard_ns: presentation_timing.apply_guard_ns(),
                target_hits: presentation_timing_snapshot.target_hits,
                pre_render_unreachable: presentation_timing_snapshot.unreachable_targets,
                render_readiness_misses: presentation_timing_snapshot.render_readiness_misses,
                dispatch_misses: presentation_timing_snapshot.dispatch_misses,
                apply_guard_misses: presentation_timing_snapshot.apply_guard_misses,
                worker_jobs_enqueued: worker.jobs_enqueued,
                worker_jobs_submitted: worker.jobs_submitted,
                worker_jobs_rejected: worker.jobs_rejected,
                worker_late_wakeups: worker.late_wakeups,
                worker_submit_duration_max_us: worker.submit_duration_ns_max / 1_000,
                worker_queue_residency_max_us: worker.queue_wait_ns_max / 1_000,
                worker_queue_depth_max: worker.runtime_queue_depth_max,
                worker_timing: worker_timing_snapshot(worker.timing),
                main_loop_wake_lateness_p50_us: pacing.wake_lateness.0,
                main_loop_wake_lateness_p95_us: pacing.wake_lateness.1,
                main_loop_wake_lateness_p99_us: pacing.wake_lateness.2,
                main_loop_target_slip_p50_us: pacing.target_error.0,
                main_loop_target_slip_p95_us: pacing.target_error.1,
                main_loop_target_slip_p99_us: pacing.target_error.2,
                pageflip_interval_p50_us: pacing.pageflip_interval.0,
                pageflip_interval_p95_us: pacing.pageflip_interval.1,
                pageflip_interval_p99_us: pacing.pageflip_interval.2,
                active_pageflip_interval_p50_us: pacing.active_pageflip_interval.0,
                active_pageflip_interval_p95_us: pacing.active_pageflip_interval.1,
                active_pageflip_interval_p99_us: pacing.active_pageflip_interval.2,
                commit_to_present_p50_us: pacing.commit_to_present.0,
                commit_to_present_p95_us: pacing.commit_to_present.1,
                commit_to_present_p99_us: pacing.commit_to_present.2,
                missed_refresh_1x: pacing.missed_refresh_1x,
                missed_refresh_2x: pacing.missed_refresh_2x,
                missed_refresh_3x_or_more: pacing.missed_refresh_3x_or_more,
            },
            resource_efficiency: self.resource_efficiency().snapshot(),
            timing_scopes,
        }
    }
}

fn xwayland_scene_metric_fields(
    metrics: oblivion_one::compositor::XwaylandSceneMetricsSnapshot,
    snapshots_emitted: u64,
) -> Vec<NativePerfField> {
    vec![
        NativePerfField::u64("xwayland_stack_snapshots_emitted", snapshots_emitted),
        NativePerfField::u64("xwayland_scene_batches", metrics.xwayland_scene_batches),
        NativePerfField::u64("xwayland_scene_mutations", metrics.xwayland_scene_mutations),
        NativePerfField::u64(
            "pointer_refreshes_deferred",
            metrics.pointer_refreshes_deferred,
        ),
        NativePerfField::u64(
            "pointer_refreshes_committed",
            metrics.pointer_refreshes_committed,
        ),
        NativePerfField::u64(
            "intermediate_pointer_targets_suppressed",
            metrics.intermediate_pointer_targets_suppressed,
        ),
        NativePerfField::u64(
            "render_stack_reorders_coalesced",
            metrics.render_stack_reorders_coalesced,
        ),
        NativePerfField::u64(
            "client_list_syncs_coalesced",
            metrics.client_list_syncs_coalesced,
        ),
        NativePerfField::u64(
            "override_redirect_stack_snapshots_applied",
            metrics.override_redirect_stack_snapshots_applied,
        ),
        NativePerfField::u64(
            "override_redirect_stack_snapshots_rejected_stale",
            metrics.override_redirect_stack_snapshots_rejected_stale,
        ),
        NativePerfField::u64(
            "override_redirect_stack_snapshots_rejected_generation",
            metrics.override_redirect_stack_snapshots_rejected_generation,
        ),
        NativePerfField::u64(
            "override_redirect_restack_writebacks_prevented",
            metrics.override_redirect_restack_writebacks_prevented,
        ),
        NativePerfField::u64(
            "pre_admission_popup_cancellations",
            metrics.pre_admission_popup_cancellations,
        ),
        NativePerfField::u64(
            "popup_lifecycle_redundant_cleanup",
            metrics.popup_lifecycle_redundant_cleanup,
        ),
    ]
}

impl NativeRuntime {
    fn current_scheduler_wake_deadline(
        &mut self,
        now_ns: u64,
    ) -> NativeResult<Option<NativeDeadline>> {
        let predicted_total_cost = Duration::from_nanos(
            self.render_journal
                .prediction_with_kms_guard(
                    Duration::from_nanos(self.presentation_timing.mode().refresh_interval_ns()),
                    self.presentation_timing.apply_guard_ns(),
                )
                .total_cost_ns,
        );
        let explicit_output = matches!(&*self.scanout, NativeScanoutBackend::AtomicEglGbm(_));
        if explicit_output {
            let pipeline = self
                .validate_output_pipeline()
                .map_err(|error| {
                    io::Error::other(format!("wake pipeline validation failed: {error:?}"))
                })?
                .ok_or_else(|| io::Error::other("explicit output has no pipeline snapshot"))?;
            let worker_mode = self.kms_commit_worker_transport == KmsCommitWorkerTransport::Worker;
            let worker_queue_available = worker_mode
                && self.atomic_commit_arbiter.worker_slot_available()
                && self
                    .kms_commit_worker
                    .as_ref()
                    .is_some_and(|worker| worker.admission_available());
            let render_ahead_allowed =
                self.adaptive_buffering.desired_credit() > 1 && pipeline.triple_capable();
            let decision = self.frame_scheduler.decision_with_pipeline_diagnostics(
                ExplicitAtomicSchedulerContext {
                    now: MonotonicTimestampNs::new(now_ns),
                    predicted_total_cost,
                    presentation_target: self.scheduled_presentation_target,
                    render_ahead_allowed,
                    worker_queue_available,
                },
                &pipeline,
            );
            let can_queue_worker_next = super::presentation_worker::can_queue_worker_primary(
                worker_mode,
                decision.action,
                Some(&pipeline),
                self.kms_commit_worker.as_ref(),
            );
            let action = apply_atomic_commit_lane_guard(
                decision.action,
                self.atomic_commit_arbiter.atomic_commit_pending(),
                can_queue_worker_next,
            );
            return Ok((action == decision.action)
                .then_some(decision.wake_deadline)
                .flatten()
                .map(native_deadline_from_scheduler));
        }

        let decision = self
            .frame_scheduler
            .decision_with_context(SchedulerFrameContext {
                pacing_mode: self.adaptive_buffering.pacing_mode(),
                capabilities: SchedulerCapabilities::legacy(),
                presentation_target: self.scheduled_presentation_target,
                predicted_total_cost,
                now: MonotonicTimestampNs::new(now_ns),
                render_target_available: self.scanout.render_target_available(),
                render_ahead_allowed: false,
                ready_frame_present: self.frame_scheduler.ready_frame_queued(),
                ready_target_current: true,
                worker_queue_available: false,
            });
        let deadline = match decision {
            SchedulerDecision::WaitForRefresh => {
                if let Some(target) = self.frame_scheduler.ready_target() {
                    (now_ns < target.submit_not_before().get()).then_some(NativeDeadline {
                        owner: NativeDeadlineOwner::PresentationTarget,
                        at_ns: target.submit_not_before().get(),
                    })
                } else if self.frame_scheduler.visual_work_queued() {
                    self.scheduled_presentation_target
                        .filter(|target| now_ns < target.render_start_deadline.get())
                        .map(|target| NativeDeadline {
                            owner: NativeDeadlineOwner::PresentationTarget,
                            at_ns: target.render_start_deadline.get(),
                        })
                } else {
                    self.frame_scheduler
                        .next_deadline_ns()
                        .map(|at_ns| NativeDeadline {
                            owner: NativeDeadlineOwner::FrameScheduler,
                            at_ns,
                        })
                }
            }
            // Buffer and page-flip ownership is external readiness.  Preserve
            // only the genuine page-flip watchdog; never reuse a visual
            // target deadline as a poll for either owner.
            SchedulerDecision::WaitForBuffer | SchedulerDecision::WaitForPageFlip => self
                .frame_scheduler
                .page_flip_watchdog_deadline_ns()
                .map(|at_ns| NativeDeadline {
                    owner: NativeDeadlineOwner::FrameScheduler,
                    at_ns,
                }),
            SchedulerDecision::Idle
            | SchedulerDecision::Render
            | SchedulerDecision::RenderAhead
            | SchedulerDecision::SubmitReady
            | SchedulerDecision::SubmitReadyLate
            | SchedulerDecision::ReadyTargetInvalidated
            | SchedulerDecision::CompleteProtocolOnly
            | SchedulerDecision::WaitForWorkerQueue
            | SchedulerDecision::PageFlipWatchdogExpired => None,
        };
        Ok(deadline)
    }

    pub(super) fn install_native_wake_plan(
        &mut self,
        plan: NativeWakePlan,
        now_ns: u64,
    ) -> NativeResult<()> {
        self.wake_authority
            .observe_plan(plan, now_ns, self.event_loop.armed_deadline_ns());
        for reason in [
            NativeContinuationReason::InputBacklog,
            NativeContinuationReason::AstreaPublication,
            NativeContinuationReason::CommitTimingPlanning,
            NativeContinuationReason::XwaylandContinuation,
            NativeContinuationReason::ControlTimeout,
        ] {
            if plan.continuation.contains(reason) {
                self.wake_authority.note_continuation(reason);
                self.event_loop.request_continuation(reason)?;
            }
        }
        self.event_loop
            .arm_deadline(plan.deadline.map(|deadline| deadline.at_ns))?;
        Ok(())
    }

    pub(super) fn request_native_continuation(
        &mut self,
        reason: NativeContinuationReason,
    ) -> NativeResult<()> {
        self.wake_authority.note_continuation(reason);
        self.event_loop.request_continuation(reason)?;
        Ok(())
    }

    pub(super) fn arm_runtime_deadline(&mut self) -> NativeResult<()> {
        let now_ns = monotonic_now_ns()?;
        let dmabuf_retry_deadline =
            if matches!(&*self.scanout, NativeScanoutBackend::AtomicEglGbm(_)) {
                self.dmabuf_gpu_release_registry
                    .update_retry_for_deferred_work(
                        self.server.deferred_dmabuf_release_count(),
                        self.server.retryable_deferred_dmabuf_release_count(),
                        DmabufReleaseRetryReason::NoGpuProofAvailable,
                        now_ns,
                    );
                self.dmabuf_gpu_release_registry.retry_deadline_ns()
            } else {
                None
            };
        let surface_pacing_deadline = (!self.server.has_surface_pacing_readiness_pending())
            .then(|| self.server.next_surface_pacing_deadline_ns())
            .flatten();
        let control_timeout_deadline = self.control_server.next_deadline_ns();
        let plan = build_native_wake_plan(NativeWakePlanInputs {
            now_ns,
            scheduler_deadline: self.current_scheduler_wake_deadline(now_ns)?,
            atomic_commit_watchdog_deadline_ns: self.atomic_commit_arbiter.watchdog_deadline_ns(),
            explicit_sync_fallback_deadline_ns: self.acquire_watches.next_fallback_deadline_ns(),
            xwayland_timeout_deadline_ns: self.xwayland.next_deadline_ns(),
            cursor_response_deadline_ns: self.cursor_output_arbitration.deadline_ns(),
            control_timeout_deadline_ns: control_timeout_deadline
                .filter(|deadline| *deadline > now_ns),
            surface_pacing_deadline_ns: surface_pacing_deadline,
            dmabuf_retry_deadline_ns: dmabuf_retry_deadline,
            input_backlog: self.input_epoch.backlog_pending(),
            astrea_publication: self.server.has_pending_astrea_toplevel_publication(),
            commit_timing_planning: self.server.has_pending_commit_timing_planning(),
            xwayland_continuation: false,
            control_timeout_pending: control_timeout_deadline.is_some_and(|deadline| deadline <= now_ns),
        });
        self.install_native_wake_plan(plan, now_ns)
    }

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
            fields.extend(xwayland_scene_metric_fields(
                self.server.xwayland_scene_metrics(),
                self.xwayland
                    .xwayland_override_redirect_stack_snapshots_emitted(),
            ));
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
                        "cursor_sidecars_materialized",
                        metrics.cursor_sidecars_materialized,
                    ),
                    NativePerfField::u64(
                        "cursor_sidecars_replaced",
                        metrics.cursor_sidecars_replaced,
                    ),
                    NativePerfField::u64(
                        "cursor_sidecars_claimed",
                        metrics.cursor_sidecars_claimed,
                    ),
                    NativePerfField::u64(
                        "cursor_sidecars_promoted",
                        metrics.cursor_sidecars_promoted,
                    ),
                    NativePerfField::u64(
                        "cursor_sidecars_missed_freeze",
                        metrics.cursor_sidecars_missed_freeze,
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
                    self.presented_planes
                        .primary
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
        let now_ns = monotonic_now_ns()?;
        let scheduler_deadline = self
            .current_scheduler_wake_deadline(now_ns)?
            .map(|deadline| deadline.at_ns);
        self.frame_pacing.note_deadline_state(
            scheduler_decision,
            now_ns,
            scheduler_deadline,
            None,
            self.frame_scheduler.ready_frame_queued() || self.scanout.ready_frame_queued(),
            cycle.wakeup.reasons.timer(),
        );
        self.arm_runtime_deadline()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xwayland_scene_metrics_export_each_counter_without_conflation() {
        let snapshot = oblivion_one::compositor::XwaylandSceneMetricsSnapshot {
            xwayland_scene_batches: 15,
            xwayland_scene_mutations: 16,
            pointer_refreshes_deferred: 17,
            pointer_refreshes_committed: 18,
            intermediate_pointer_targets_suppressed: 19,
            render_stack_reorders_coalesced: 20,
            client_list_syncs_coalesced: 21,
            override_redirect_stack_snapshots_applied: 22,
            override_redirect_stack_snapshots_rejected_stale: 23,
            override_redirect_stack_snapshots_rejected_generation: 24,
            override_redirect_restack_writebacks_prevented: 25,
            pre_admission_popup_cancellations: 26,
            popup_lifecycle_redundant_cleanup: 27,
        };
        let fields = xwayland_scene_metric_fields(snapshot, 14);

        assert_eq!(fields.len(), 14);
        for (field, expected) in fields.iter().zip(14_u64..=27) {
            assert_eq!(field.value, expected.to_string(), "field {}", field.key);
        }
        assert_eq!(fields[0].key, "xwayland_stack_snapshots_emitted");
        assert_eq!(fields[0].value, "14");
        assert_eq!(
            fields
                .iter()
                .filter(|field| field.key.contains("applied"))
                .count(),
            1
        );
        assert_eq!(
            fields
                .iter()
                .filter(|field| field.key.contains("rejected"))
                .count(),
            2
        );
        assert_ne!(fields[3].value, fields[4].value);
    }

    #[test]
    fn render_telemetry_keeps_bounded_timing_repaint_and_age_aggregates() {
        use crate::egl_renderer::{FullRepaintReason, RepaintMode};

        let mut telemetry = NativeRenderTelemetry::default();
        telemetry.record_render_us(1_500);
        telemetry.record_render_us(2_500);
        telemetry.record_repaint(RepaintMode::Partial, Some(2), 100, 20, None);
        telemetry.record_repaint(
            RepaintMode::Full,
            Some(0),
            100,
            100,
            Some(FullRepaintReason::BufferAgeZero),
        );
        telemetry.record_repaint(RepaintMode::Skip, None, 100, 0, None);

        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.compositor_cpu_render.count, 2);
        assert_eq!(snapshot.partial_frames, 1);
        assert_eq!(snapshot.full_frames, 1);
        assert_eq!(snapshot.skip_frames, 1);
        assert_eq!(snapshot.buffer_age_buckets[2], 1);
        assert_eq!(snapshot.buffer_age_buckets[0], 1);
        assert_eq!(snapshot.buffer_age_buckets[5], 1);
        assert_eq!(snapshot.partial_repair_pixels, 20);
        assert_eq!(snapshot.full_output_pixels, 100);
        assert_eq!(snapshot.full_repaint_reasons[4], 1);
    }
}
