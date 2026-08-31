use oblivion_one::control_snapshots::ResourceEfficiencyPerformanceSnapshot;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ResourceEfficiencyMetrics {
    pub(super) native_cycles: u64,
    pub(super) input_ready: u64,
    pub(super) raw_input_events: u64,
    pub(super) coalesced_input_events: u64,
    pub(super) pointer_samples: u64,
    pub(super) primary_scene_attempts: u64,
    pub(super) primary_scene_renders: u64,
    pub(super) primary_scene_submits: u64,
    pub(super) cursor_only_opportunities: u64,
    pub(super) cursor_only_submits: u64,
    pub(super) protocol_only_completions: u64,
    pub(super) pure_input_completions: u64,
    pub(super) input_only_cycles: u64,
    pub(super) wayland_read_dispatch_cycles: u64,
    pub(super) server_tick_calls: u64,
    pub(super) client_flushes: u64,
    pub(super) hit_test_locality: u64,
    pub(super) hit_test_full_scans: u64,
    pub(super) xwayland_sync_requests: u64,
    pub(super) xwayland_reconciliations: u64,
    pub(super) xwayland_unchanged_skips: u64,
    pub(super) xwayland_environment_materializations: u64,
    pub(super) pacing_progressions: u64,
    pub(super) acquire_prepare_runs: u64,
    pub(super) acquire_prepare_skips: u64,
    pub(super) explicit_sync_service_runs: u64,
    pub(super) frame_prepare_runs: u64,
    pub(super) surface_pacing_service_runs: u64,
    pub(super) commit_timing_planning_replans: u64,
    pub(super) presentation_planning_runs: u64,
    pub(super) presentation_planning_skips: u64,
}

impl ResourceEfficiencyMetrics {
    pub(super) const fn snapshot(&self) -> ResourceEfficiencyPerformanceSnapshot {
        ResourceEfficiencyPerformanceSnapshot {
            native_cycles: self.native_cycles,
            input_ready: self.input_ready,
            raw_input_events: self.raw_input_events,
            coalesced_input_events: self.coalesced_input_events,
            pointer_samples: self.pointer_samples,
            primary_scene_attempts: self.primary_scene_attempts,
            primary_scene_renders: self.primary_scene_renders,
            primary_scene_submits: self.primary_scene_submits,
            cursor_only_opportunities: self.cursor_only_opportunities,
            cursor_only_submits: self.cursor_only_submits,
            protocol_only_completions: self.protocol_only_completions,
            pure_input_completions: self.pure_input_completions,
            input_only_cycles: self.input_only_cycles,
            wayland_read_dispatch_cycles: self.wayland_read_dispatch_cycles,
            server_tick_calls: self.server_tick_calls,
            client_flushes: self.client_flushes,
            hit_test_locality: self.hit_test_locality,
            hit_test_full_scans: self.hit_test_full_scans,
            xwayland_sync_requests: self.xwayland_sync_requests,
            xwayland_reconciliations: self.xwayland_reconciliations,
            xwayland_unchanged_skips: self.xwayland_unchanged_skips,
            xwayland_environment_materializations: self.xwayland_environment_materializations,
            pacing_progressions: self.pacing_progressions,
            acquire_prepare_runs: self.acquire_prepare_runs,
            acquire_prepare_skips: self.acquire_prepare_skips,
            explicit_sync_service_runs: self.explicit_sync_service_runs,
            frame_prepare_runs: self.frame_prepare_runs,
            surface_pacing_service_runs: self.surface_pacing_service_runs,
            commit_timing_planning_replans: self.commit_timing_planning_replans,
            presentation_planning_runs: self.presentation_planning_runs,
            presentation_planning_skips: self.presentation_planning_skips,
        }
    }

    #[inline]
    pub(super) fn record_native_cycle(&mut self) {
        self.native_cycles = self.native_cycles.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_input_ready(&mut self) {
        self.input_ready = self.input_ready.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_raw_input_event(&mut self) {
        self.raw_input_events = self.raw_input_events.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_coalesced_input_event(&mut self) {
        self.coalesced_input_events = self.coalesced_input_events.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_pointer_sample(&mut self) {
        self.pointer_samples = self.pointer_samples.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_primary_scene_attempt(&mut self) {
        self.primary_scene_attempts = self.primary_scene_attempts.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_primary_scene_render(&mut self) {
        self.primary_scene_renders = self.primary_scene_renders.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_primary_scene_submit(&mut self) {
        self.primary_scene_submits = self.primary_scene_submits.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_cursor_only_opportunity(&mut self) {
        self.cursor_only_opportunities = self.cursor_only_opportunities.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_cursor_only_submit(&mut self) {
        self.cursor_only_submits = self.cursor_only_submits.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_protocol_only_completion(&mut self) {
        self.protocol_only_completions = self.protocol_only_completions.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_pure_input_completion(&mut self) {
        self.pure_input_completions = self.pure_input_completions.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_input_only_cycle(&mut self) {
        self.input_only_cycles = self.input_only_cycles.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_wayland_read_dispatch_cycle(&mut self) {
        self.wayland_read_dispatch_cycles = self.wayland_read_dispatch_cycles.saturating_add(1);
    }

    #[inline]
    #[cfg(test)]
    pub(super) fn record_server_tick_call(&mut self) {
        self.server_tick_calls = self.server_tick_calls.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_client_flush(&mut self) {
        self.client_flushes = self.client_flushes.saturating_add(1);
    }

    #[inline]
    #[cfg(test)]
    pub(super) fn record_hit_test_locality(&mut self) {
        self.hit_test_locality = self.hit_test_locality.saturating_add(1);
    }

    #[inline]
    #[cfg(test)]
    pub(super) fn record_hit_test_full_scan(&mut self) {
        self.hit_test_full_scans = self.hit_test_full_scans.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_xwayland_sync_request(&mut self) {
        self.xwayland_sync_requests = self.xwayland_sync_requests.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_xwayland_reconciliation(&mut self) {
        self.xwayland_reconciliations = self.xwayland_reconciliations.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_xwayland_unchanged_skip(&mut self) {
        self.xwayland_unchanged_skips = self.xwayland_unchanged_skips.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_xwayland_environment_materialization(&mut self) {
        self.xwayland_environment_materializations =
            self.xwayland_environment_materializations.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_pacing_progression(&mut self) {
        self.pacing_progressions = self.pacing_progressions.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_acquire_prepare_run(&mut self) {
        self.acquire_prepare_runs = self.acquire_prepare_runs.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_acquire_prepare_skip(&mut self) {
        self.acquire_prepare_skips = self.acquire_prepare_skips.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_explicit_sync_service_run(&mut self) {
        self.explicit_sync_service_runs = self.explicit_sync_service_runs.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_frame_prepare_run(&mut self) {
        self.frame_prepare_runs = self.frame_prepare_runs.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_surface_pacing_service_run(&mut self) {
        self.surface_pacing_service_runs = self.surface_pacing_service_runs.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_commit_timing_planning_replan(&mut self) {
        self.commit_timing_planning_replans = self.commit_timing_planning_replans.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_presentation_planning_run(&mut self) {
        self.presentation_planning_runs = self.presentation_planning_runs.saturating_add(1);
    }

    #[inline]
    pub(super) fn record_presentation_planning_skip(&mut self) {
        self.presentation_planning_skips = self.presentation_planning_skips.saturating_add(1);
    }

    pub(super) fn record_work_decision(&mut self, decision: NativeWorkDecision) {
        match decision.work_class {
            NativeWorkClass::NoOutputWork => self.record_pure_input_completion(),
            NativeWorkClass::ProtocolOnly => self.record_protocol_only_completion(),
            NativeWorkClass::CursorOnly => self.record_cursor_only_opportunity(),
            NativeWorkClass::PrimaryScene => self.record_primary_scene_attempt(),
        }
        if decision.service_xwayland {
            self.record_xwayland_sync_request();
        }
        if decision.service_pacing {
            self.record_pacing_progression();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeWorkClass {
    NoOutputWork,
    ProtocolOnly,
    CursorOnly,
    PrimaryScene,
}

impl NativeWorkClass {
    pub(super) const fn from_flags(
        protocol_work: bool,
        cursor_work: bool,
        primary_scene_work: bool,
    ) -> Self {
        if primary_scene_work {
            Self::PrimaryScene
        } else if cursor_work {
            Self::CursorOnly
        } else if protocol_work {
            Self::ProtocolOnly
        } else {
            Self::NoOutputWork
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeWorkDecision {
    pub(super) work_class: NativeWorkClass,
    pub(super) service_xwayland: bool,
    pub(super) service_pacing: bool,
    pub(super) service_explicit_sync_acquire: bool,
    pub(super) service_astrea_publication: bool,
    pub(super) service_commit_timing_planning: bool,
    pub(super) service_primary_scene: bool,
    pub(super) service_control: bool,
    pub(super) service_children: bool,
    pub(super) service_session: bool,
    pub(super) service_shutdown: bool,
}

impl NativeWorkDecision {
    #[expect(
        clippy::too_many_arguments,
        reason = "each service lane is an independent decision bit"
    )]
    pub(super) const fn new(
        work_class: NativeWorkClass,
        service_xwayland: bool,
        service_pacing: bool,
        service_explicit_sync_acquire: bool,
        service_astrea_publication: bool,
        service_commit_timing_planning: bool,
        service_primary_scene: bool,
        service_control: bool,
        service_children: bool,
        service_session: bool,
        service_shutdown: bool,
    ) -> Self {
        Self {
            work_class,
            service_xwayland,
            service_pacing,
            service_explicit_sync_acquire,
            service_astrea_publication,
            service_commit_timing_planning,
            service_primary_scene,
            service_control,
            service_children,
            service_session,
            service_shutdown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_work_classifies_output_work_by_priority() {
        assert_eq!(
            NativeWorkClass::from_flags(false, false, false),
            NativeWorkClass::NoOutputWork
        );
        assert_eq!(
            NativeWorkClass::from_flags(true, false, false),
            NativeWorkClass::ProtocolOnly
        );
        assert_eq!(
            NativeWorkClass::from_flags(false, true, false),
            NativeWorkClass::CursorOnly
        );
        assert_eq!(
            NativeWorkClass::from_flags(false, true, true),
            NativeWorkClass::PrimaryScene
        );
    }

    #[test]
    fn every_efficiency_counter_is_independently_recorded() {
        let mut metrics = ResourceEfficiencyMetrics::default();

        metrics.record_native_cycle();
        metrics.record_input_ready();
        metrics.record_raw_input_event();
        metrics.record_coalesced_input_event();
        metrics.record_pointer_sample();
        metrics.record_primary_scene_attempt();
        metrics.record_primary_scene_render();
        metrics.record_primary_scene_submit();
        metrics.record_cursor_only_opportunity();
        metrics.record_cursor_only_submit();
        metrics.record_protocol_only_completion();
        metrics.record_pure_input_completion();
        metrics.record_input_only_cycle();
        metrics.record_wayland_read_dispatch_cycle();
        metrics.record_server_tick_call();
        metrics.record_client_flush();
        metrics.record_hit_test_locality();
        metrics.record_hit_test_full_scan();
        metrics.record_xwayland_sync_request();
        metrics.record_xwayland_reconciliation();
        metrics.record_xwayland_unchanged_skip();
        metrics.record_xwayland_environment_materialization();
        metrics.record_pacing_progression();
        metrics.record_acquire_prepare_run();
        metrics.record_acquire_prepare_skip();
        metrics.record_explicit_sync_service_run();
        metrics.record_frame_prepare_run();
        metrics.record_surface_pacing_service_run();
        metrics.record_commit_timing_planning_replan();
        metrics.record_presentation_planning_run();
        metrics.record_presentation_planning_skip();

        assert_eq!(
            metrics.snapshot(),
            ResourceEfficiencyPerformanceSnapshot {
                native_cycles: 1,
                input_ready: 1,
                raw_input_events: 1,
                coalesced_input_events: 1,
                pointer_samples: 1,
                primary_scene_attempts: 1,
                primary_scene_renders: 1,
                primary_scene_submits: 1,
                cursor_only_opportunities: 1,
                cursor_only_submits: 1,
                protocol_only_completions: 1,
                pure_input_completions: 1,
                input_only_cycles: 1,
                wayland_read_dispatch_cycles: 1,
                server_tick_calls: 1,
                client_flushes: 1,
                hit_test_locality: 1,
                hit_test_full_scans: 1,
                xwayland_sync_requests: 1,
                xwayland_reconciliations: 1,
                xwayland_unchanged_skips: 1,
                xwayland_environment_materializations: 1,
                pacing_progressions: 1,
                acquire_prepare_runs: 1,
                acquire_prepare_skips: 1,
                explicit_sync_service_runs: 1,
                frame_prepare_runs: 1,
                surface_pacing_service_runs: 1,
                commit_timing_planning_replans: 1,
                presentation_planning_runs: 1,
                presentation_planning_skips: 1,
            }
        );
    }

    #[test]
    fn efficiency_snapshot_round_trips_through_json() {
        let mut metrics = ResourceEfficiencyMetrics::default();
        metrics.record_native_cycle();
        metrics.record_pure_input_completion();

        let encoded = serde_json::to_value(metrics.snapshot()).unwrap();
        let decoded: ResourceEfficiencyPerformanceSnapshot =
            serde_json::from_value(encoded).unwrap();

        assert_eq!(decoded, metrics.snapshot());
    }
}
