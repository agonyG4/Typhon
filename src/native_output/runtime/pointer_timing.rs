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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativePointerTimingPhase {
    SurfacePacing,
    CursorAndControl,
    XwaylandScene,
    AcquirePrepare,
    Render,
    PresentKms,
}

impl NativePointerTimingPhase {
    const COUNT: usize = 6;

    const fn index(self) -> usize {
        match self {
            Self::SurfacePacing => 0,
            Self::CursorAndControl => 1,
            Self::XwaylandScene => 2,
            Self::AcquirePrepare => 3,
            Self::Render => 4,
            Self::PresentKms => 5,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::SurfacePacing => "surface_pacing",
            Self::CursorAndControl => "cursor_control",
            Self::XwaylandScene => "xwayland_scene",
            Self::AcquirePrepare => "acquire_prepare",
            Self::Render => "render",
            Self::PresentKms => "present_kms",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct NativePointerTimingRecord {
    transition: Option<NativePointerTimingTransition>,
    transition_at_ns: u64,
    reactor_wake_return_at_ns: Option<u64>,
    input_service_start_at_ns: Option<u64>,
    input_service_end_at_ns: Option<u64>,
    libinput_dispatch_start_at_ns: Option<u64>,
    libinput_dispatch_end_at_ns: Option<u64>,
    native_batch_materialized_at_ns: Option<u64>,
    wayland_read_start_at_ns: Option<u64>,
    wayland_read_end_at_ns: Option<u64>,
    activation_resolution_at_ns: Option<u64>,
    native_activation_at_ns: Option<u64>,
    cursor_sync_start_at_ns: Option<u64>,
    cursor_sync_end_at_ns: Option<u64>,
    dispatch_return_at_ns: Option<u64>,
    cycle_return_at_ns: Option<u64>,
    next_reactor_wake_at_ns: Option<u64>,
    next_input_service_at_ns: Option<u64>,
    phase_spans: [Option<(u64, u64)>; NativePointerTimingPhase::COUNT],
    first_batch: Option<NativePointerTimingBatch>,
    first_batch_materialized_at_ns: Option<u64>,
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
            formatted_summary_count: 0,
            emitted_summary_count: 0,
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn observe_transition(
        &mut self,
        transition: NativePointerTimingTransition,
        at_ns: u64,
    ) {
        if !self.enabled {
            return;
        }

        let slot = self.next_slot;
        self.next_slot = (slot + 1) % TIMING_RING_CAPACITY;
        self.records[slot] = Some(NativePointerTimingRecord {
            transition: Some(transition),
            transition_at_ns: at_ns,
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
        if record.complete || record.first_batch.is_some() {
            return;
        }

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
            record.input_service_start_at_ns.get_or_insert(at_ns);
            if record.transition_at_ns <= at_ns {
                record.next_input_service_at_ns.get_or_insert(at_ns);
            }
        });
    }

    pub(crate) fn record_input_service_end(&mut self, at_ns: u64) {
        self.set_record(|record| record.input_service_end_at_ns = Some(at_ns));
    }

    pub(crate) fn record_libinput_dispatch(&mut self, start_ns: u64, end_ns: u64) {
        self.set_record(|record| {
            record.libinput_dispatch_start_at_ns = Some(start_ns);
            record.libinput_dispatch_end_at_ns = Some(end_ns);
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

    pub(crate) fn record_activation_resolution(&mut self, at_ns: u64) {
        self.set_record(|record| record.activation_resolution_at_ns = Some(at_ns));
    }

    pub(crate) fn record_native_activation(&mut self, at_ns: u64) {
        self.set_record(|record| record.native_activation_at_ns = Some(at_ns));
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

    pub(crate) fn record_next_input_service(&mut self, at_ns: u64) {
        self.set_record(|record| {
            record.next_input_service_at_ns.get_or_insert(at_ns);
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
            .map(|record| record.transition_at_ns)
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
    let phase = record
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
                4 => NativePointerTimingPhase::Render,
                5 => NativePointerTimingPhase::PresentKms,
                _ => unreachable!(),
            };
            format!("{}:{duration}", phase.as_str())
        })
        .unwrap_or_else(|| "none:0".to_owned());
    let input_service_duration_ns = duration(
        record.input_service_start_at_ns,
        record.input_service_end_at_ns,
    );
    let native_dispatch_duration_ns = duration(
        record.libinput_dispatch_start_at_ns,
        record.libinput_dispatch_end_at_ns,
    );
    let wayland_read_duration_ns = duration(
        record.wayland_read_start_at_ns,
        record.wayland_read_end_at_ns,
    );
    let cursor_sync_duration_ns =
        duration(record.cursor_sync_start_at_ns, record.cursor_sync_end_at_ns);
    let reactor_wait_ns = record
        .cycle_return_at_ns
        .zip(record.next_reactor_wake_at_ns)
        .map(|(cycle_return, wake)| wake.saturating_sub(cycle_return))
        .unwrap_or(0);

    format!(
        "transition={transition} activation_to_dispatch_return_ns={} activation_to_cycle_return_ns={} activation_to_next_wake_ns={} activation_to_next_input_service_ns={} reactor_wait_ns={} input_service_duration_ns={} native_dispatch_duration_ns={} wayland_read_duration_ns={} cursor_sync_duration_ns={} raw={} coalesced={} hw_span_us={} largest_phase={phase}",
        record
            .dispatch_return_at_ns
            .map(|at| at.saturating_sub(record.transition_at_ns))
            .unwrap_or(0),
        record
            .cycle_return_at_ns
            .map(|at| at.saturating_sub(record.transition_at_ns))
            .unwrap_or(0),
        record
            .next_reactor_wake_at_ns
            .map(|at| at.saturating_sub(record.transition_at_ns))
            .unwrap_or(0),
        record
            .next_input_service_at_ns
            .map(|at| at.saturating_sub(record.transition_at_ns))
            .unwrap_or(0),
        reactor_wait_ns,
        input_service_duration_ns,
        native_dispatch_duration_ns,
        wayland_read_duration_ns,
        cursor_sync_duration_ns,
        batch.raw_events,
        batch.coalesced_events,
        span_us.unwrap_or(0),
    )
}

fn duration(start_ns: Option<u64>, end_ns: Option<u64>) -> u64 {
    start_ns
        .zip(end_ns)
        .map(|(start, end)| end.saturating_sub(start))
        .unwrap_or(0)
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
        trace.observe_transition(test_transition(), 10);
        trace.observe_first_batch(test_batch(), 20);

        assert_eq!(trace.formatted_summary_count(), 0);
        assert_eq!(trace.emitted_summary_count(), 0);
        assert_eq!(trace.retained_capacity(), 8);
    }

    #[test]
    fn timing_ring_replaces_oldest_record_deterministically() {
        let mut trace = NativePointerTimingTrace::enabled_for_test();
        for timestamp in 1..=9 {
            trace.observe_transition(test_transition(), timestamp);
            trace.observe_first_batch(test_batch(), timestamp + 1);
        }

        assert_eq!(trace.completed_record_count(), 9);
        assert_eq!(trace.oldest_retained_transition_timestamp(), Some(2));
    }

    #[test]
    fn timing_probe_records_only_one_summary_per_transition() {
        let mut trace = NativePointerTimingTrace::enabled_for_test();
        trace.observe_transition(test_transition(), 10);
        trace.observe_first_batch(test_batch(), 20);
        trace.observe_first_batch(test_batch(), 30);
        trace.record_reactor_wake_return(40);

        assert_eq!(trace.formatted_summary_count(), 1);
        assert_eq!(trace.emitted_summary_count(), 0);
    }

    #[test]
    fn timing_probe_does_not_change_recorded_batch_values() {
        let mut trace = NativePointerTimingTrace::enabled_for_test();
        trace.observe_transition(test_transition(), 10);
        trace.record_phase(NativePointerTimingPhase::Render, 20, 42);
        trace.observe_first_batch(test_batch(), 50);

        let record = trace.records[0].expect("completed record");
        assert_eq!(record.first_batch, Some(test_batch()));
        assert_eq!(
            record.phase_spans[NativePointerTimingPhase::Render.index()],
            Some((20, 42))
        );
    }
}
