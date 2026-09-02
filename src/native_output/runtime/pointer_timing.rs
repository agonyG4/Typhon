use std::sync::OnceLock;

const TIMING_RING_CAPACITY: usize = 8;

static TIMING_ENABLED: OnceLock<bool> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativePointerTimingTransition {
    LockedActivated,
    LockedDeactivated,
    ConfinedActivated,
    ConfinedDeactivated,
}

impl NativePointerTimingTransition {
    fn as_str(self) -> &'static str {
        match self {
            Self::LockedActivated => "locked_activated",
            Self::LockedDeactivated => "locked_deactivated",
            Self::ConfinedActivated => "confined_activated",
            Self::ConfinedDeactivated => "confined_deactivated",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct NativePointerTimingBatch {
    pub(crate) raw_events: u32,
    pub(crate) coalesced_events: u32,
    pub(crate) oldest_hardware_timestamp_us: Option<u64>,
    pub(crate) newest_hardware_timestamp_us: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct NativePointerPreReadObservation {
    pub(crate) probe_performed: bool,
    pub(crate) input_promoted: bool,
    pub(crate) batch: Option<NativePointerTimingBatch>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativePointerTimingPhase {
    SurfacePacing,
    CursorAndControl,
    XwaylandScene,
    AcquirePrepare,
    RenderPresentKms,
}

impl NativePointerTimingPhase {
    const COUNT: usize = 5;

    const fn index(self) -> usize {
        match self {
            Self::SurfacePacing => 0,
            Self::CursorAndControl => 1,
            Self::XwaylandScene => 2,
            Self::AcquirePrepare => 3,
            Self::RenderPresentKms => 4,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::SurfacePacing => "surface_pacing",
            Self::CursorAndControl => "cursor_control",
            Self::XwaylandScene => "xwayland_scene",
            Self::AcquirePrepare => "acquire_prepare",
            Self::RenderPresentKms => "render_present_kms",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct NativePointerTimingRecord {
    transition: Option<NativePointerTimingTransition>,
    routing_transition_committed_at_ns: u64,
    reactor_wake_return_at_ns: Option<u64>,
    active_input_service_start_at_ns: Option<u64>,
    first_input_service_attempt_at_ns: Option<u64>,
    first_nonempty_input_service_start_at_ns: Option<u64>,
    first_nonempty_input_service_end_at_ns: Option<u64>,
    libinput_dispatch_start_at_ns: Option<u64>,
    libinput_dispatch_end_at_ns: Option<u64>,
    queue_drain_start_at_ns: Option<u64>,
    queue_drain_end_at_ns: Option<u64>,
    native_batch_materialized_at_ns: Option<u64>,
    wayland_read_start_at_ns: Option<u64>,
    wayland_read_end_at_ns: Option<u64>,
    cursor_sync_start_at_ns: Option<u64>,
    cursor_sync_end_at_ns: Option<u64>,
    dispatch_return_at_ns: Option<u64>,
    cycle_return_at_ns: Option<u64>,
    next_reactor_wake_at_ns: Option<u64>,
    phase_spans: [Option<(u64, u64)>; NativePointerTimingPhase::COUNT],
    checkpoint_count: u8,
    first_serviceable_checkpoint: Option<u8>,
    fresh_input_microturn: bool,
    superseded_incomplete_transition_observations: u64,
    first_batch: Option<NativePointerTimingBatch>,
    first_batch_materialized_at_ns: Option<u64>,
    pre_read_probe: bool,
    pre_read_input_promoted: bool,
    pre_transition_input: Option<NativePointerTimingBatch>,
    complete: bool,
    summary_emitted: bool,
}

#[derive(Debug)]
pub(crate) struct NativePointerTimingTrace {
    enabled: bool,
    emit_summaries: bool,
    records: [Option<NativePointerTimingRecord>; TIMING_RING_CAPACITY],
    next_slot: usize,
    active_slot: Option<usize>,
    completed_record_count: u64,
    superseded_incomplete_transition_observations: u64,
    formatted_summary_count: u64,
    emitted_summary_count: u64,
}

impl NativePointerTimingTrace {
    pub(crate) fn from_env() -> Self {
        Self::new(
            *TIMING_ENABLED
                .get_or_init(|| std::env::var_os("TYPHON_POINTER_TIMING_TRACE").is_some()),
            true,
        )
    }

    fn new(enabled: bool, emit_summaries: bool) -> Self {
        Self {
            enabled,
            emit_summaries,
            records: [None; TIMING_RING_CAPACITY],
            next_slot: 0,
            active_slot: None,
            completed_record_count: 0,
            superseded_incomplete_transition_observations: 0,
            formatted_summary_count: 0,
            emitted_summary_count: 0,
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn record_routing_transition_committed(
        &mut self,
        transition: NativePointerTimingTransition,
        at_ns: u64,
    ) {
        self.record_routing_transition_committed_with_pre_read(
            transition,
            at_ns,
            NativePointerPreReadObservation::default(),
        );
    }

    pub(crate) fn record_routing_transition_committed_with_pre_read(
        &mut self,
        transition: NativePointerTimingTransition,
        at_ns: u64,
        pre_read: NativePointerPreReadObservation,
    ) {
        if !self.enabled {
            return;
        }

        let superseded_incomplete_transition_observations = if let Some(active_slot) =
            self.active_slot
            && self.records[active_slot]
                .is_some_and(|record| !record.complete && record.transition.is_some())
        {
            self.superseded_incomplete_transition_observations = self
                .superseded_incomplete_transition_observations
                .saturating_add(1);
            self.superseded_incomplete_transition_observations
        } else {
            self.superseded_incomplete_transition_observations
        };
        let slot = self.next_slot;
        self.next_slot = (slot + 1) % TIMING_RING_CAPACITY;
        self.records[slot] = Some(NativePointerTimingRecord {
            transition: Some(transition),
            routing_transition_committed_at_ns: at_ns,
            superseded_incomplete_transition_observations,
            pre_read_probe: pre_read.probe_performed,
            pre_read_input_promoted: pre_read.input_promoted,
            pre_transition_input: pre_read.batch,
            ..Default::default()
        });
        self.active_slot = Some(slot);
    }

    pub(crate) fn observe_first_batch(&mut self, batch: NativePointerTimingBatch, at_ns: u64) {
        if !self.enabled {
            return;
        }

        let Some(slot) = self.active_slot else {
            return;
        };
        let Some(record) = self.records[slot].as_mut() else {
            return;
        };
        if record.complete || record.first_batch.is_some() || batch.raw_events == 0 {
            return;
        }

        record.first_nonempty_input_service_start_at_ns =
            record.active_input_service_start_at_ns;
        record.first_batch = Some(batch);
        record.first_batch_materialized_at_ns = Some(at_ns);
        record.complete = true;
        self.completed_record_count += 1;
    }

    pub(crate) fn record_reactor_wake_return(&mut self, at_ns: u64) {
        self.set_record(|record| record.reactor_wake_return_at_ns = Some(at_ns));
        self.emit_completed_summaries();
    }

    pub(crate) fn record_input_service_start(&mut self, at_ns: u64) {
        self.set_record(|record| {
            record.active_input_service_start_at_ns = Some(at_ns);
            record.first_input_service_attempt_at_ns.get_or_insert(at_ns);
        });
    }

    pub(crate) fn record_input_service_end(&mut self, at_ns: u64) {
        self.set_record(|record| {
            if record.first_nonempty_input_service_start_at_ns.is_some()
                && record.first_nonempty_input_service_end_at_ns.is_none()
            {
                record.first_nonempty_input_service_end_at_ns = Some(at_ns);
            }
            record.active_input_service_start_at_ns = None;
        });
    }

    pub(crate) fn record_libinput_dispatch(&mut self, start_ns: u64, end_ns: u64) {
        self.set_record(|record| {
            record.libinput_dispatch_start_at_ns = Some(start_ns);
            record.libinput_dispatch_end_at_ns = Some(end_ns);
        });
    }

    pub(crate) fn record_queue_drain(&mut self, start_ns: u64, end_ns: u64) {
        self.set_record(|record| {
            record.queue_drain_start_at_ns = Some(start_ns);
            record.queue_drain_end_at_ns = Some(end_ns);
        });
    }

    pub(crate) fn record_native_batch_materialized(&mut self, at_ns: u64) {
        self.set_record(|record| record.native_batch_materialized_at_ns = Some(at_ns));
    }

    pub(crate) fn record_wayland_read(&mut self, start_ns: u64, end_ns: u64) {
        self.set_record(|record| {
            record.wayland_read_start_at_ns = Some(start_ns);
            record.wayland_read_end_at_ns = Some(end_ns);
        });
    }

    pub(crate) fn record_cursor_sync(&mut self, start_ns: u64, end_ns: u64) {
        self.set_record(|record| {
            record.cursor_sync_start_at_ns = Some(start_ns);
            record.cursor_sync_end_at_ns = Some(end_ns);
        });
    }

    pub(crate) fn record_dispatch_return(&mut self, at_ns: u64) {
        self.set_record(|record| record.dispatch_return_at_ns = Some(at_ns));
    }

    pub(crate) fn record_cycle_return(&mut self, at_ns: u64) {
        self.set_record(|record| record.cycle_return_at_ns = Some(at_ns));
    }

    pub(crate) fn record_next_reactor_wake(&mut self, at_ns: u64) {
        self.set_record(|record| record.next_reactor_wake_at_ns = Some(at_ns));
    }

    pub(crate) fn record_checkpoint(
        &mut self,
        checkpoint: u8,
        input_serviceable: bool,
        fresh_input_microturn: bool,
    ) {
        self.set_record(|record| {
            record.checkpoint_count = record.checkpoint_count.saturating_add(1);
            if input_serviceable {
                record
                    .first_serviceable_checkpoint
                    .get_or_insert(checkpoint);
            }
            record.fresh_input_microturn |= fresh_input_microturn;
        });
    }
    pub(crate) fn record_phase(
        &mut self,
        phase: NativePointerTimingPhase,
        start_ns: u64,
        end_ns: u64,
    ) {
        self.set_record(|record| record.phase_spans[phase.index()] = Some((start_ns, end_ns)));
    }

    fn set_record(&mut self, update: impl FnOnce(&mut NativePointerTimingRecord)) {
        if !self.enabled {
            return;
        }
        if let Some(slot) = self.active_slot
            && let Some(record) = self.records[slot].as_mut()
        {
            update(record);
        }
    }

    fn emit_completed_summaries(&mut self) {
        if !self.enabled {
            return;
        }
        for slot in 0..TIMING_RING_CAPACITY {
            let Some(record) = self.records[slot] else {
                continue;
            };
            if !record.complete || record.summary_emitted {
                continue;
            }
            let summary = format_summary(&record);
            if let Some(record) = self.records[slot].as_mut() {
                record.summary_emitted = true;
            }
            self.formatted_summary_count += 1;
            if self.emit_summaries {
                self.emitted_summary_count += 1;
                eprintln!("typhon pointer timing: {summary}");
            }
        }
    }

    #[cfg(test)]
    fn disabled_for_test() -> Self {
        Self::new(false, false)
    }

    #[cfg(test)]
    fn enabled_for_test() -> Self {
        Self::new(true, false)
    }

    #[cfg(test)]
    fn formatted_summary_count(&self) -> u64 {
        self.formatted_summary_count
    }

    #[cfg(test)]
    fn emitted_summary_count(&self) -> u64 {
        self.emitted_summary_count
    }

    #[cfg(test)]
    fn retained_capacity(&self) -> usize {
        TIMING_RING_CAPACITY
    }

    #[cfg(test)]
    fn completed_record_count(&self) -> u64 {
        self.completed_record_count
    }

    #[cfg(test)]
    fn oldest_retained_transition_timestamp(&self) -> Option<u64> {
        self.records
            .iter()
            .flatten()
            .filter(|record| record.complete)
            .map(|record| record.routing_transition_committed_at_ns)
            .min()
    }
}

fn format_summary(record: &NativePointerTimingRecord) -> String {
    let transition = record
        .transition
        .map(NativePointerTimingTransition::as_str)
        .unwrap_or("unknown");
    let batch = record.first_batch.unwrap_or_default();
    let span_us = batch
        .oldest_hardware_timestamp_us
        .zip(batch.newest_hardware_timestamp_us)
        .map(|(oldest, newest)| newest.saturating_sub(oldest));
    let hardware_timestamp_span_us = span_us
        .map(|span| span.to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    let pre_transition_input_raw = record
        .pre_transition_input
        .map(|batch| batch.raw_events.to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    let pre_transition_input_coalesced = record
        .pre_transition_input
        .map(|batch| batch.coalesced_events.to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    let pre_transition_input_hw_span_us = record
        .pre_transition_input
        .and_then(|batch| {
            batch
                .oldest_hardware_timestamp_us
                .zip(batch.newest_hardware_timestamp_us)
                .map(|(oldest, newest)| newest.saturating_sub(oldest).to_string())
        })
        .unwrap_or_else(|| "unknown".to_owned());
    let largest_phase = record
        .phase_spans
        .iter()
        .enumerate()
        .filter_map(|(index, span)| span.map(|(start, end)| (index, end.saturating_sub(start))))
        .max_by_key(|(_, duration)| *duration)
        .map(|(index, duration)| {
            let phase = match index {
                0 => NativePointerTimingPhase::SurfacePacing,
                1 => NativePointerTimingPhase::CursorAndControl,
                2 => NativePointerTimingPhase::XwaylandScene,
                3 => NativePointerTimingPhase::AcquirePrepare,
                4 => NativePointerTimingPhase::RenderPresentKms,
                _ => unreachable!(),
            };
            format!("{}:{duration}", phase.as_str())
        })
        .unwrap_or_else(|| "unknown".to_owned());
    let first_nonempty_input_service_duration_ns = format_duration(
        record.first_nonempty_input_service_start_at_ns,
        record.first_nonempty_input_service_end_at_ns,
    );
    let libinput_dispatch_duration_ns = format_duration(
        record.libinput_dispatch_start_at_ns,
        record.libinput_dispatch_end_at_ns,
    );
    let queue_drain_duration_ns =
        format_duration(record.queue_drain_start_at_ns, record.queue_drain_end_at_ns);
    let wayland_read_duration_ns = format_duration(
        record.wayland_read_start_at_ns,
        record.wayland_read_end_at_ns,
    );
    let cursor_sync_duration_ns =
        format_duration(record.cursor_sync_start_at_ns, record.cursor_sync_end_at_ns);
    let reactor_wait_ns =
        format_duration(record.cycle_return_at_ns, record.next_reactor_wake_at_ns);

    format!(
        "transition={transition} routing_transition_committed_at_ns={} transition_to_dispatch_return_ns={} transition_to_cycle_return_ns={} transition_to_next_reactor_wake_ns={} transition_to_first_input_service_attempt_ns={} reactor_wait_ns={} first_nonempty_input_service_duration_ns={} libinput_dispatch_duration_ns={} queue_drain_duration_ns={} wayland_read_duration_ns={} cursor_sync_duration_ns={} pre_read_probe={} pre_read_input_promoted={} pre_transition_input_raw={} pre_transition_input_coalesced={} pre_transition_input_hw_span_us={} raw={} coalesced={} hw_span_us={} checkpoint_count={} first_serviceable_checkpoint={} fresh_input_microturn={} superseded_incomplete_transition_observations={} largest_phase={largest_phase}",
        record.routing_transition_committed_at_ns,
        format_transition_duration(record.dispatch_return_at_ns, record),
        format_transition_duration(record.cycle_return_at_ns, record),
        format_transition_duration(record.next_reactor_wake_at_ns, record),
        format_transition_duration(record.first_input_service_attempt_at_ns, record),
        reactor_wait_ns,
        first_nonempty_input_service_duration_ns,
        libinput_dispatch_duration_ns,
        queue_drain_duration_ns,
        wayland_read_duration_ns,
        cursor_sync_duration_ns,
        record.pre_read_probe,
        record.pre_read_input_promoted,
        pre_transition_input_raw,
        pre_transition_input_coalesced,
        pre_transition_input_hw_span_us,
        batch.raw_events,
        batch.coalesced_events,
        hardware_timestamp_span_us,
        record.checkpoint_count,
        record
            .first_serviceable_checkpoint
            .map(|checkpoint| checkpoint.to_string())
            .unwrap_or_else(|| "unknown".to_owned()),
        record.fresh_input_microturn,
        record.superseded_incomplete_transition_observations,
    )
}

fn format_transition_duration(at_ns: Option<u64>, record: &NativePointerTimingRecord) -> String {
    at_ns
        .map(|at| {
            at.saturating_sub(record.routing_transition_committed_at_ns)
                .to_string()
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

fn format_duration(start_ns: Option<u64>, end_ns: Option<u64>) -> String {
    start_ns
        .zip(end_ns)
        .map(|(start, end)| end.saturating_sub(start).to_string())
        .unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_transition() -> NativePointerTimingTransition {
        NativePointerTimingTransition::LockedActivated
    }

    fn test_batch() -> NativePointerTimingBatch {
        NativePointerTimingBatch {
            raw_events: 28,
            coalesced_events: 1,
            oldest_hardware_timestamp_us: Some(9_405_366_325),
            newest_hardware_timestamp_us: Some(9_405_393_323),
        }
    }

    #[test]
    fn disabled_timing_probe_does_not_format_or_emit() {
        let mut trace = NativePointerTimingTrace::disabled_for_test();
        trace.record_routing_transition_committed(test_transition(), 10);
        trace.observe_first_batch(test_batch(), 20);

        assert_eq!(trace.formatted_summary_count(), 0);
        assert_eq!(trace.emitted_summary_count(), 0);
        assert_eq!(trace.retained_capacity(), 8);
    }

    #[test]
    fn timing_ring_replaces_oldest_record_deterministically() {
        let mut trace = NativePointerTimingTrace::enabled_for_test();
        for timestamp in 1..=9 {
            trace.record_routing_transition_committed(test_transition(), timestamp);
            trace.observe_first_batch(test_batch(), timestamp + 1);
        }

        assert_eq!(trace.completed_record_count(), 9);
        assert_eq!(trace.oldest_retained_transition_timestamp(), Some(2));
    }

    #[test]
    fn timing_probe_records_only_one_summary_per_transition() {
        let mut trace = NativePointerTimingTrace::enabled_for_test();
        trace.record_routing_transition_committed(test_transition(), 10);
        trace.observe_first_batch(test_batch(), 20);
        trace.observe_first_batch(test_batch(), 30);
        trace.record_reactor_wake_return(40);

        assert_eq!(trace.formatted_summary_count(), 1);
        assert_eq!(trace.emitted_summary_count(), 0);
    }

    #[test]
    fn timing_probe_does_not_change_recorded_batch_values() {
        let mut trace = NativePointerTimingTrace::enabled_for_test();
        trace.record_routing_transition_committed(test_transition(), 10);
        trace.record_phase(NativePointerTimingPhase::RenderPresentKms, 20, 42);
        trace.observe_first_batch(test_batch(), 50);

        let record = trace.records[0].expect("completed record");
        assert_eq!(record.first_batch, Some(test_batch()));
        assert_eq!(
            record.phase_spans[NativePointerTimingPhase::RenderPresentKms.index()],
            Some((20, 42))
        );
    }

    #[test]
    fn actual_input_service_time_is_not_reactor_wake_time() {
        let mut trace = NativePointerTimingTrace::enabled_for_test();
        trace.record_routing_transition_committed(test_transition(), 100);
        trace.record_next_reactor_wake(200);
        trace.record_input_service_start(450);

        let record = trace.records[0].expect("active record");
        assert_eq!(record.next_reactor_wake_at_ns, Some(200));
        assert_eq!(record.first_input_service_attempt_at_ns, Some(450));
        assert_eq!(record.first_input_service_attempt_at_ns.unwrap() - 100, 350);
    }

    #[test]
    fn empty_input_service_does_not_complete_transition_observation() {
        let mut trace = NativePointerTimingTrace::enabled_for_test();
        trace.record_routing_transition_committed(test_transition(), 100);
        trace.observe_first_batch(NativePointerTimingBatch::default(), 200);

        assert_eq!(trace.completed_record_count(), 0);

        trace.observe_first_batch(test_batch(), 300);
        assert_eq!(trace.completed_record_count(), 1);
    }

    #[test]
    fn incomplete_transition_observation_is_counted_when_replaced() {
        let mut trace = NativePointerTimingTrace::enabled_for_test();
        trace.record_routing_transition_committed(test_transition(), 100);
        trace.record_routing_transition_committed(
            NativePointerTimingTransition::LockedDeactivated,
            200,
        );

        assert_eq!(trace.superseded_incomplete_transition_observations, 1);
        assert_eq!(
            trace.records[1]
                .expect("replacement record")
                .superseded_incomplete_transition_observations,
            1
        );
    }

    #[test]
    fn timing_summary_uses_actual_service_and_unknown_phase_values() {
        let mut trace = NativePointerTimingTrace::enabled_for_test();
        trace.record_routing_transition_committed(test_transition(), 100);
        trace.record_next_reactor_wake(200);
        trace.record_input_service_start(450);
        trace.observe_first_batch(test_batch(), 500);

        let summary = format_summary(&trace.records[0].expect("completed record"));
        assert!(summary.contains("routing_transition_committed_at_ns=100"));
        assert!(summary.contains("transition_to_first_input_service_attempt_ns=350"));
        assert!(summary.contains("transition_to_next_reactor_wake_ns=100"));
        assert!(summary.contains("first_nonempty_input_service_duration_ns=unknown"));
        assert!(summary.contains("largest_phase=unknown"));
    }

    #[test]
    fn timing_summary_does_not_invent_a_hardware_timestamp_span() {
        let mut trace = NativePointerTimingTrace::enabled_for_test();
        trace.record_routing_transition_committed(test_transition(), 100);
        trace.observe_first_batch(
            NativePointerTimingBatch {
                raw_events: 1,
                coalesced_events: 1,
                oldest_hardware_timestamp_us: None,
                newest_hardware_timestamp_us: None,
            },
            200,
        );

        let summary = format_summary(&trace.records[0].expect("completed record"));
        assert!(summary.contains("hw_span_us=unknown"));
    }

    #[test]
    fn timing_summary_separates_service_attempts() {
        let mut trace = NativePointerTimingTrace::enabled_for_test();
        trace.record_routing_transition_committed(test_transition(), 100);
        trace.record_input_service_start(200);
        trace.observe_first_batch(NativePointerTimingBatch::default(), 225);
        trace.record_input_service_end(250);
        trace.record_input_service_start(450);
        trace.observe_first_batch(test_batch(), 475);
        trace.record_input_service_end(500);

        let summary = format_summary(&trace.records[0].expect("completed record"));
        assert!(summary.contains("transition_to_first_input_service_attempt_ns=100"));
        assert!(summary.contains("first_nonempty_input_service_duration_ns=50"));
        assert!(!summary.contains(" input_service_duration_ns="));
    }

    #[test]
    fn timing_summary_records_pre_read_input_promotion() {
        let mut trace = NativePointerTimingTrace::enabled_for_test();
        trace.record_routing_transition_committed_with_pre_read(
            test_transition(),
            100,
            NativePointerPreReadObservation {
                probe_performed: true,
                input_promoted: true,
                batch: Some(test_batch()),
            },
        );

        let summary = format_summary(&trace.records[0].expect("active record"));
        assert!(summary.contains("pre_read_probe=true"));
        assert!(summary.contains("pre_read_input_promoted=true"));
        assert!(summary.contains("pre_transition_input_raw=28"));
        assert!(summary.contains("pre_transition_input_coalesced=1"));
        assert!(summary.contains("pre_transition_input_hw_span_us=26998"));
    }
}
