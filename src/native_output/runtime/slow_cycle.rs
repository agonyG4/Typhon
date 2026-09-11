use std::collections::VecDeque;

use super::perf::native_perf_log_value_enabled;

pub(super) const SLOW_CYCLE_RING_CAPACITY: usize = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SlowCycleClass {
    Slow,
    Severe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub(super) enum SlowCyclePhase {
    XwaylandReactor = 0,
    TimerDeadline = 1,
    WaylandInput = 2,
    CursorControl = 3,
    XwaylandScene = 4,
    AcquirePrepare = 5,
    RenderPresentKms = 6,
}

const SLOW_CYCLE_PHASE_COUNT: usize = 7;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct SlowCycleContext {
    pub(super) wake_reasons: u32,
    pub(super) continuation_reasons: u32,
    pub(super) work_domains: u32,
    pub(super) ready_sources: usize,
    pub(super) blocked_ns: u64,
    pub(super) timer_lateness_ns: Option<u64>,
    pub(super) x11_events_total: u64,
    pub(super) x11_property_replies_total: u64,
    pub(super) xwm_budget_exhaustions_total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SlowCycleRecord {
    pub(super) cycle_start_ns: u64,
    pub(super) total_ns: u64,
    pub(super) refresh_interval_ns: u64,
    pub(super) class: SlowCycleClass,
    phase_ns: [u64; SLOW_CYCLE_PHASE_COUNT],
    pub(super) wake_reasons: u32,
    pub(super) continuation_reasons: u32,
    pub(super) work_domains: u32,
    pub(super) ready_sources: usize,
    pub(super) blocked_ns: u64,
    pub(super) timer_lateness_ns: Option<u64>,
    pub(super) x11_events: usize,
    pub(super) x11_property_replies: usize,
    pub(super) xwm_budget_exhausted: bool,
    pub(super) xwm_events_translated: usize,
    pub(super) xwm_commands_executed: usize,
    pub(super) input_raw_events: usize,
    pub(super) input_coalesced_events: usize,
    pub(super) input_ready: bool,
    pub(super) input_backlog_pending: bool,
    pub(super) render_attempted: bool,
    pub(super) frame_rendered: bool,
    pub(super) frame_submitted: bool,
    pub(super) pageflip_pending: bool,
    pub(super) kms_queue_depth: usize,
    pub(super) kms_worker_active: bool,
}

impl Default for SlowCycleRecord {
    fn default() -> Self {
        Self {
            cycle_start_ns: 0,
            total_ns: 0,
            refresh_interval_ns: 0,
            class: SlowCycleClass::Slow,
            phase_ns: [0; SLOW_CYCLE_PHASE_COUNT],
            wake_reasons: 0,
            continuation_reasons: 0,
            work_domains: 0,
            ready_sources: 0,
            blocked_ns: 0,
            timer_lateness_ns: None,
            x11_events: 0,
            x11_property_replies: 0,
            xwm_budget_exhausted: false,
            xwm_events_translated: 0,
            xwm_commands_executed: 0,
            input_raw_events: 0,
            input_coalesced_events: 0,
            input_ready: false,
            input_backlog_pending: false,
            render_attempted: false,
            frame_rendered: false,
            frame_submitted: false,
            pageflip_pending: false,
            kms_queue_depth: 0,
            kms_worker_active: false,
        }
    }
}

impl SlowCycleRecord {
    #[cfg(test)]
    pub(super) fn phase(self, phase: SlowCyclePhase) -> u64 {
        self.phase_ns[phase as usize]
    }
}

#[derive(Debug, Clone, Copy)]
struct ActiveSlowCycle {
    record: SlowCycleRecord,
    x11_events_total: u64,
    x11_property_replies_total: u64,
    xwm_budget_exhaustions_total: u64,
}

#[derive(Debug)]
struct SlowCycleTraceState {
    records: VecDeque<SlowCycleRecord>,
    active: Option<ActiveSlowCycle>,
}

#[derive(Debug)]
pub(super) struct NativeSlowCycleTrace {
    state: Option<SlowCycleTraceState>,
}

impl NativeSlowCycleTrace {
    pub(super) fn from_env() -> Self {
        let enabled = std::env::var("TYPHON_SLOW_CYCLE_TRACE")
            .ok()
            .is_some_and(|value| native_perf_log_value_enabled(&value));
        Self::new(enabled)
    }

    pub(super) fn new(enabled: bool) -> Self {
        Self {
            state: enabled.then(|| SlowCycleTraceState {
                records: VecDeque::with_capacity(SLOW_CYCLE_RING_CAPACITY),
                active: None,
            }),
        }
    }

    pub(super) const fn enabled(&self) -> bool {
        self.state.is_some()
    }

    pub(super) fn start_cycle(
        &mut self,
        start_ns: u64,
        refresh_interval_ns: u64,
        context: SlowCycleContext,
    ) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        let record = SlowCycleRecord {
            cycle_start_ns: start_ns,
            refresh_interval_ns,
            wake_reasons: context.wake_reasons,
            continuation_reasons: context.continuation_reasons,
            work_domains: context.work_domains,
            ready_sources: context.ready_sources,
            blocked_ns: context.blocked_ns,
            timer_lateness_ns: context.timer_lateness_ns,
            ..SlowCycleRecord::default()
        };
        state.active = Some(ActiveSlowCycle {
            record,
            x11_events_total: context.x11_events_total,
            x11_property_replies_total: context.x11_property_replies_total,
            xwm_budget_exhaustions_total: context.xwm_budget_exhaustions_total,
        });
    }

    pub(super) fn note_work_domains(&mut self, work_domains: u32) {
        let Some(active) = self.state.as_mut().and_then(|state| state.active.as_mut()) else {
            return;
        };
        active.record.work_domains = work_domains;
    }

    pub(super) fn note_xwayland_totals(
        &mut self,
        x11_events_total: u64,
        x11_property_replies_total: u64,
        xwm_budget_exhaustions_total: u64,
    ) {
        let Some(active) = self.state.as_mut().and_then(|state| state.active.as_mut()) else {
            return;
        };
        active.record.x11_events =
            usize::try_from(x11_events_total.saturating_sub(active.x11_events_total))
                .unwrap_or(usize::MAX);
        active.record.x11_property_replies = usize::try_from(
            x11_property_replies_total.saturating_sub(active.x11_property_replies_total),
        )
        .unwrap_or(usize::MAX);
        active.record.xwm_budget_exhausted =
            xwm_budget_exhaustions_total > active.xwm_budget_exhaustions_total;
    }

    pub(super) fn record_phase(&mut self, phase: SlowCyclePhase, start_ns: u64, end_ns: u64) {
        let Some(active) = self.state.as_mut().and_then(|state| state.active.as_mut()) else {
            return;
        };
        active.record.phase_ns[phase as usize] =
            active.record.phase_ns[phase as usize].saturating_add(end_ns.saturating_sub(start_ns));
    }

    #[cfg(test)]
    pub(super) fn note_xwayland(
        &mut self,
        x11_events: usize,
        x11_property_replies: usize,
        xwm_budget_exhausted: bool,
    ) {
        let Some(active) = self.state.as_mut().and_then(|state| state.active.as_mut()) else {
            return;
        };
        active.record.x11_events = active.record.x11_events.saturating_add(x11_events);
        active.record.x11_property_replies = active
            .record
            .x11_property_replies
            .saturating_add(x11_property_replies);
        active.record.xwm_budget_exhausted |= xwm_budget_exhausted;
    }

    pub(super) fn note_xwayland_scene(
        &mut self,
        events_translated: usize,
        commands_executed: usize,
    ) {
        let Some(active) = self.state.as_mut().and_then(|state| state.active.as_mut()) else {
            return;
        };
        active.record.xwm_events_translated = active
            .record
            .xwm_events_translated
            .saturating_add(events_translated);
        active.record.xwm_commands_executed = active
            .record
            .xwm_commands_executed
            .saturating_add(commands_executed);
    }

    pub(super) fn note_input(
        &mut self,
        raw_events: usize,
        coalesced_events: usize,
        input_ready: bool,
        input_backlog_pending: bool,
    ) {
        let Some(active) = self.state.as_mut().and_then(|state| state.active.as_mut()) else {
            return;
        };
        active.record.input_raw_events = raw_events;
        active.record.input_coalesced_events = coalesced_events;
        active.record.input_ready = input_ready;
        active.record.input_backlog_pending = input_backlog_pending;
    }

    pub(super) fn note_output(
        &mut self,
        render_attempted: bool,
        frame_rendered: bool,
        frame_submitted: bool,
        pageflip_pending: bool,
        kms_queue_depth: usize,
        kms_worker_active: bool,
    ) {
        let Some(active) = self.state.as_mut().and_then(|state| state.active.as_mut()) else {
            return;
        };
        active.record.render_attempted = render_attempted;
        active.record.frame_rendered = frame_rendered;
        active.record.frame_submitted = frame_submitted;
        active.record.pageflip_pending = pageflip_pending;
        active.record.kms_queue_depth = kms_queue_depth;
        active.record.kms_worker_active = kms_worker_active;
    }

    pub(super) fn finish_cycle(&mut self, end_ns: u64) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        let Some(mut active) = state.active.take() else {
            return;
        };
        active.record.total_ns = end_ns.saturating_sub(active.record.cycle_start_ns);
        if active.record.total_ns <= active.record.refresh_interval_ns {
            return;
        }
        let severe_threshold = active.record.refresh_interval_ns.saturating_mul(2);
        active.record.class = if active.record.total_ns > severe_threshold {
            SlowCycleClass::Severe
        } else {
            SlowCycleClass::Slow
        };
        if state.records.len() == SLOW_CYCLE_RING_CAPACITY {
            state.records.pop_front();
        }
        state.records.push_back(active.record);
    }

    #[cfg(test)]
    pub(super) fn retained_records(&self) -> impl Iterator<Item = &SlowCycleRecord> {
        self.state
            .as_ref()
            .into_iter()
            .flat_map(|state| state.records.iter())
    }

    pub(super) fn dump(&self) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        for record in &state.records {
            eprintln!(
                "typhon_slow_cycle class={:?} start_ns={} total_ns={} refresh_ns={} phases_ns={:?} wake=0x{:x} continuation=0x{:x} domains=0x{:x} ready_sources={} blocked_ns={} timer_lateness_ns={:?} x11_events={} x11_property_replies={} xwm_budget_exhausted={} xwm_events_translated={} xwm_commands_executed={} input_raw={} input_coalesced={} input_ready={} input_backlog_pending={} render_attempted={} frame_rendered={} frame_submitted={} pageflip_pending={} kms_queue_depth={} kms_worker_active={}",
                record.class,
                record.cycle_start_ns,
                record.total_ns,
                record.refresh_interval_ns,
                record.phase_ns,
                record.wake_reasons,
                record.continuation_reasons,
                record.work_domains,
                record.ready_sources,
                record.blocked_ns,
                record.timer_lateness_ns,
                record.x11_events,
                record.x11_property_replies,
                record.xwm_budget_exhausted,
                record.xwm_events_translated,
                record.xwm_commands_executed,
                record.input_raw_events,
                record.input_coalesced_events,
                record.input_ready,
                record.input_backlog_pending,
                record.render_attempted,
                record.frame_rendered,
                record.frame_submitted,
                record.pageflip_pending,
                record.kms_queue_depth,
                record.kms_worker_active,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        NativeSlowCycleTrace, SLOW_CYCLE_RING_CAPACITY, SlowCycleClass, SlowCycleContext,
        SlowCyclePhase,
    };

    fn context() -> SlowCycleContext {
        SlowCycleContext {
            wake_reasons: 0x11,
            continuation_reasons: 0x22,
            work_domains: 0x44,
            ready_sources: 3,
            blocked_ns: 17,
            timer_lateness_ns: Some(19),
            ..SlowCycleContext::default()
        }
    }

    #[test]
    fn disabled_path_retains_no_records_or_output() {
        let mut trace = NativeSlowCycleTrace::new(false);
        trace.start_cycle(0, 1_000, context());
        trace.record_phase(SlowCyclePhase::WaylandInput, 0, 2_000);
        trace.finish_cycle(2_000);

        assert!(!trace.enabled());
        assert_eq!(trace.retained_records().count(), 0);
        trace.dump();
    }

    #[test]
    fn refresh_derived_threshold_classifies_representative_rates() {
        for (refresh_ns, slow_ns, severe_ns) in [
            (16_666_667, 16_666_668, 33_333_335),
            (8_333_333, 8_333_334, 16_666_667),
            (6_060_606, 6_060_607, 12_121_214),
            (4_166_667, 4_166_668, 8_333_335),
        ] {
            let mut trace = NativeSlowCycleTrace::new(true);
            trace.start_cycle(0, refresh_ns, SlowCycleContext::default());
            trace.finish_cycle(slow_ns);
            assert_eq!(trace.retained_records().count(), 1);
            assert_eq!(
                trace.retained_records().next().unwrap().class,
                SlowCycleClass::Slow
            );

            trace.start_cycle(10_000_000, refresh_ns, SlowCycleContext::default());
            trace.finish_cycle(10_000_000 + severe_ns);
            assert_eq!(trace.retained_records().count(), 2);
            assert_eq!(
                trace.retained_records().last().unwrap().class,
                SlowCycleClass::Severe
            );
        }
    }

    #[test]
    fn slow_record_captures_phase_and_context() {
        let mut trace = NativeSlowCycleTrace::new(true);
        trace.start_cycle(100, 1_000, context());
        trace.record_phase(SlowCyclePhase::XwaylandReactor, 100, 350);
        trace.record_phase(SlowCyclePhase::RenderPresentKms, 350, 1_250);
        trace.note_xwayland(7, 5, true);
        trace.note_xwayland_scene(7, 3);
        trace.note_input(11, 4, true, true);
        trace.note_output(true, true, true, true, 2, true);
        trace.finish_cycle(1_500);

        let record = trace.retained_records().next().unwrap();
        assert_eq!(record.total_ns, 1_400);
        assert_eq!(record.refresh_interval_ns, 1_000);
        assert_eq!(record.phase(SlowCyclePhase::XwaylandReactor), 250);
        assert_eq!(record.phase(SlowCyclePhase::RenderPresentKms), 900);
        assert_eq!(record.wake_reasons, 0x11);
        assert_eq!(record.continuation_reasons, 0x22);
        assert_eq!(record.work_domains, 0x44);
        assert_eq!(record.x11_events, 7);
        assert_eq!(record.x11_property_replies, 5);
        assert!(record.xwm_budget_exhausted);
        assert_eq!(record.xwm_events_translated, 7);
        assert_eq!(record.xwm_commands_executed, 3);
        assert_eq!(record.input_raw_events, 11);
        assert_eq!(record.input_coalesced_events, 4);
        assert!(record.input_ready);
        assert!(record.input_backlog_pending);
        assert!(record.render_attempted);
        assert!(record.frame_rendered);
        assert!(record.frame_submitted);
        assert!(record.pageflip_pending);
        assert_eq!(record.kms_queue_depth, 2);
        assert!(record.kms_worker_active);
    }

    #[test]
    fn ring_is_bounded_and_evicts_oldest_records() {
        let mut trace = NativeSlowCycleTrace::new(true);
        for index in 0..(SLOW_CYCLE_RING_CAPACITY + 5) {
            trace.start_cycle(index as u64 * 10, 1, SlowCycleContext::default());
            trace.finish_cycle(index as u64 * 10 + 2);
        }

        let records = trace.retained_records().collect::<Vec<_>>();
        assert_eq!(records.len(), SLOW_CYCLE_RING_CAPACITY);
        assert_eq!(records.first().unwrap().cycle_start_ns, 50);
        assert_eq!(
            records.last().unwrap().cycle_start_ns,
            50 + (SLOW_CYCLE_RING_CAPACITY as u64 - 1) * 10
        );
    }

    #[test]
    fn retained_record_has_fixed_numeric_shape() {
        fn assert_copy_eq<T: Copy + Eq>() {}

        assert_copy_eq::<super::SlowCycleRecord>();
    }

    #[test]
    fn phase_accounting_uses_supplied_timestamps() {
        let mut trace = NativeSlowCycleTrace::new(true);
        trace.start_cycle(1_000, 100, SlowCycleContext::default());
        trace.record_phase(SlowCyclePhase::WaylandInput, 1_100, 1_400);
        trace.record_phase(SlowCyclePhase::WaylandInput, 1_500, 1_650);
        trace.finish_cycle(1_701);

        assert_eq!(
            trace
                .retained_records()
                .next()
                .unwrap()
                .phase(SlowCyclePhase::WaylandInput),
            450
        );
    }
}
