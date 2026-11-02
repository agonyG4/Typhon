//! VRR cadence diagnostics (VRR Cadence Governor, Phases 0-2).
//!
//! Observes confirmed physical presentations per output and explains them in a
//! `native.vrr_cadence` trace. The recorded range also bounds production VRR
//! windows (Phase 2). Counters for behavior that does not exist yet (service
//! pulses, idle fallback, cursor piggybacking) stay at zero until the phase
//! that introduces that behavior wires them.

use crate::native_output::output::{EdidVrrRange, EdidVrrRangeSource};
use crate::native_output::perf::NativePerfField;
use crate::native_output::scanout::VariableReservationFallback;
use oblivion_one::compositor::{OutputPresentationMode, VrrPolicy};

const RECENT_INTERVALS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VrrRangeSource {
    /// Minimum refresh unknown; only the active mode's maximum is known.
    ActiveModeOnly,
    Edid(EdidVrrRangeSource),
    /// `OBLIVION_ONE_VRR_MIN_HZ`, for sinks with missing or wrong EDID data.
    UserOverride,
}

impl VrrRangeSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ActiveModeOnly => "active_mode_only",
            Self::Edid(source) => source.as_str(),
            Self::UserOverride => "user_override",
        }
    }
}

/// VRR window of one output in its active mode. The maximum always comes from
/// the exact active mode; the minimum is never invented when unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VrrRange {
    pub(crate) min_hz: Option<u32>,
    pub(crate) max_millihz: u32,
    pub(crate) source: VrrRangeSource,
}

impl VrrRange {
    pub(crate) fn discover(
        active_mode_refresh_millihz: u32,
        edid: Option<EdidVrrRange>,
        override_min_hz: Option<u32>,
    ) -> Self {
        // A minimum at or above the active refresh leaves no variable window.
        let below_active =
            |hz: u32| hz > 0 && u64::from(hz) * 1_000 < u64::from(active_mode_refresh_millihz);
        let (min_hz, source) = if let Some(hz) = override_min_hz.filter(|hz| below_active(*hz)) {
            (Some(hz), VrrRangeSource::UserOverride)
        } else if let Some(edid) = edid.filter(|edid| below_active(edid.min_hz)) {
            (Some(edid.min_hz), VrrRangeSource::Edid(edid.source))
        } else {
            (None, VrrRangeSource::ActiveModeOnly)
        };
        Self {
            min_hz,
            max_millihz: active_mode_refresh_millihz,
            source,
        }
    }

    pub(crate) fn override_min_hz_from_environment() -> Option<u32> {
        std::env::var("OBLIVION_ONE_VRR_MIN_HZ")
            .ok()
            .and_then(|value| value.trim().parse().ok())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VrrPresentKind {
    /// A commit that carried a new primary plane buffer (client content).
    Primary,
    /// A cursor/plane-delta commit with no new primary content.
    CursorOnly,
}

impl VrrPresentKind {
    const fn action(self) -> &'static str {
        match self {
            Self::Primary => "primary_present",
            Self::CursorOnly => "cursor_present",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VrrCapabilities {
    pub(crate) policy: VrrPolicy,
    pub(crate) connector_capable: bool,
    pub(crate) crtc_property_available: bool,
}

impl VrrCapabilities {
    /// Whether VRR could ever be enabled on this output.
    pub(crate) const fn relevant(self) -> bool {
        !matches!(self.policy, VrrPolicy::Off)
            && self.connector_capable
            && self.crtc_property_available
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VrrPresentObservation {
    pub(crate) kind: VrrPresentKind,
    pub(crate) presented_at_ns: u64,
    /// Mode of the last confirmed primary presentation, after this flip.
    pub(crate) confirmed_mode: OutputPresentationMode,
    pub(crate) capabilities: VrrCapabilities,
    pub(crate) output_generation: u64,
    pub(crate) primary_pending: bool,
    pub(crate) cursor_pending: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct IntervalRing {
    values: [u64; RECENT_INTERVALS],
    len: usize,
    next: usize,
}

impl IntervalRing {
    fn push(&mut self, value: u64) {
        self.values[self.next] = value;
        self.next = (self.next + 1) % RECENT_INTERVALS;
        self.len = (self.len + 1).min(RECENT_INTERVALS);
    }

    fn last(&self) -> Option<u64> {
        (self.len > 0).then(|| self.values[(self.next + RECENT_INTERVALS - 1) % RECENT_INTERVALS])
    }

    fn iter(&self) -> impl Iterator<Item = u64> + '_ {
        self.values[..self.len].iter().copied()
    }

    fn median(&self) -> Option<u64> {
        let mut sorted = self.values;
        let sorted = &mut sorted[..self.len];
        sorted.sort_unstable();
        sorted.get(self.len / 2).copied()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct VrrCadenceCounters {
    pub(crate) vrr_window_opportunities: u64,
    /// AdaptiveSync composited frames that still took a Fixed reservation.
    pub(crate) vrr_fixed_fallback_reservations: u64,
    pub(crate) vrr_cursor_piggybacks: u64,
    pub(crate) vrr_cursor_intermediate_updates: u64,
    pub(crate) vrr_service_pulses_attempted: u64,
    pub(crate) vrr_service_pulses_confirmed: u64,
    pub(crate) vrr_service_pulses_noop_or_rejected: u64,
    pub(crate) vrr_idle_fixed_fallback_entries: u64,
    pub(crate) vrr_idle_fixed_fallback_exits: u64,
    pub(crate) vrr_state_transitions: u64,
    pub(crate) vrr_range_floor_events: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct VrrCadenceRecorder {
    range: VrrRange,
    last_physical_present_ns: Option<u64>,
    last_primary_present_ns: Option<u64>,
    last_cursor_present_ns: Option<u64>,
    recent_physical_intervals: IntervalRing,
    recent_content_intervals: IntervalRing,
    quiescent_since_ns: Option<u64>,
    last_confirmed_vrr: Option<bool>,
    last_fixed_fallback: Option<VariableReservationFallback>,
    counters: VrrCadenceCounters,
}

impl VrrCadenceRecorder {
    pub(crate) fn new(range: VrrRange) -> Self {
        Self {
            range,
            last_physical_present_ns: None,
            last_primary_present_ns: None,
            last_cursor_present_ns: None,
            recent_physical_intervals: IntervalRing::default(),
            recent_content_intervals: IntervalRing::default(),
            quiescent_since_ns: None,
            last_confirmed_vrr: None,
            last_fixed_fallback: None,
            counters: VrrCadenceCounters::default(),
        }
    }

    #[cfg(test)]
    pub(crate) const fn counters(&self) -> VrrCadenceCounters {
        self.counters
    }

    /// Shortest physical interval the panel allows (1 / max refresh).
    fn shortest_interval_ns(&self) -> Option<u64> {
        (self.range.max_millihz > 0).then(|| 1_000_000_000_000 / u64::from(self.range.max_millihz))
    }

    /// Longest physical interval before the panel drops below its VRR floor.
    fn longest_interval_ns(&self) -> Option<u64> {
        self.range
            .min_hz
            .filter(|hz| *hz > 0)
            .map(|hz| 1_000_000_000 / u64::from(hz))
    }

    /// Longest physical interval as a VRR window bound (1 / min refresh).
    pub(crate) fn longest_interval(&self) -> Option<std::time::Duration> {
        self.longest_interval_ns()
            .map(std::time::Duration::from_nanos)
    }

    /// Records the timing domain chosen for one composited frame. Frames whose
    /// output is not in steady AdaptiveSync are not VRR decisions.
    pub(crate) fn note_composited_reservation(
        &mut self,
        outcome: Result<(), VariableReservationFallback>,
    ) {
        match outcome {
            Ok(()) => self.counters.vrr_window_opportunities += 1,
            Err(VariableReservationFallback::NotAdaptiveSync) => {}
            Err(fallback) => {
                self.counters.vrr_fixed_fallback_reservations += 1;
                self.last_fixed_fallback = Some(fallback);
            }
        }
    }

    /// Records one confirmed physical presentation. Out-of-order timestamps
    /// are recorded as presents but never produce intervals.
    pub(crate) fn observe_present(&mut self, observation: VrrPresentObservation) {
        let now = observation.presented_at_ns;
        let vrr_active = observation.confirmed_mode.uses_vrr();
        if let Some(previous) = self.last_physical_present_ns
            && now > previous
        {
            let interval = now - previous;
            self.recent_physical_intervals.push(interval);
            if vrr_active
                && self
                    .longest_interval_ns()
                    .is_some_and(|longest| interval > longest)
            {
                self.counters.vrr_range_floor_events += 1;
            }
        }
        self.last_physical_present_ns = Some(now);
        match observation.kind {
            VrrPresentKind::Primary => {
                if let Some(previous) = self.last_primary_present_ns
                    && now > previous
                {
                    self.recent_content_intervals.push(now - previous);
                }
                self.last_primary_present_ns = Some(now);
                self.quiescent_since_ns = None;
                if self
                    .last_confirmed_vrr
                    .is_some_and(|previous| previous != vrr_active)
                {
                    self.counters.vrr_state_transitions += 1;
                }
                self.last_confirmed_vrr = Some(vrr_active);
            }
            VrrPresentKind::CursorOnly => {
                self.last_cursor_present_ns = Some(now);
                if self.quiescent_since_ns.is_none() {
                    self.quiescent_since_ns = self.last_primary_present_ns;
                }
                if vrr_active {
                    self.counters.vrr_cursor_intermediate_updates += 1;
                }
            }
        }
    }

    /// Explains the most recent presentation's VRR state.
    fn reason(observation: &VrrPresentObservation) -> &'static str {
        let capabilities = observation.capabilities;
        if matches!(capabilities.policy, VrrPolicy::Off) {
            "policy_disabled"
        } else if !capabilities.connector_capable {
            "connector_not_vrr_capable"
        } else if !capabilities.crtc_property_available {
            "crtc_vrr_property_missing"
        } else if observation.confirmed_mode.uses_vrr() {
            "adaptive_sync_confirmed"
        } else {
            // Per-decision blockers are in native.output_presentation_policy.
            "fixed_vsync_confirmed"
        }
    }

    pub(crate) fn trace_fields(
        &self,
        output_id: u64,
        observation: &VrrPresentObservation,
    ) -> Vec<NativePerfField> {
        let longest = self.longest_interval_ns();
        let service_due = longest.is_some_and(|longest| {
            observation.confirmed_mode.uses_vrr()
                && self
                    .recent_physical_intervals
                    .last()
                    .is_some_and(|interval| interval > longest)
        });
        let counters = self.counters;
        vec![
            NativePerfField::u64("output_id", output_id),
            NativePerfField::u64("output_generation", observation.output_generation),
            NativePerfField::str(
                "configured_policy",
                observation.capabilities.policy.as_str(),
            ),
            NativePerfField::str("effective_mode", observation.confirmed_mode.as_str()),
            NativePerfField::bool(
                "vrr_connector_capable",
                observation.capabilities.connector_capable,
            ),
            NativePerfField::bool(
                "vrr_crtc_property_available",
                observation.capabilities.crtc_property_available,
            ),
            NativePerfField::u64("min_hz", self.range.min_hz.map_or(0, u64::from)),
            NativePerfField::u64("max_millihz", u64::from(self.range.max_millihz)),
            NativePerfField::str("range_source", self.range.source.as_str()),
            NativePerfField::u64(
                "last_physical_interval_ns",
                self.recent_physical_intervals.last().unwrap_or(0),
            ),
            NativePerfField::u64(
                "content_interval_estimate_ns",
                self.recent_content_intervals.median().unwrap_or(0),
            ),
            NativePerfField::u64(
                "recent_physical_min_ns",
                self.recent_physical_intervals.iter().min().unwrap_or(0),
            ),
            NativePerfField::u64(
                "recent_physical_max_ns",
                self.recent_physical_intervals.iter().max().unwrap_or(0),
            ),
            NativePerfField::u64(
                "shortest_interval_ns",
                self.shortest_interval_ns().unwrap_or(0),
            ),
            NativePerfField::u64("longest_interval_ns", longest.unwrap_or(0)),
            NativePerfField::u64("presented_at_ns", observation.presented_at_ns),
            NativePerfField::u64(
                "last_primary_present_ns",
                self.last_primary_present_ns.unwrap_or(0),
            ),
            NativePerfField::u64(
                "last_cursor_present_ns",
                self.last_cursor_present_ns.unwrap_or(0),
            ),
            NativePerfField::u64("quiescent_since_ns", self.quiescent_since_ns.unwrap_or(0)),
            NativePerfField::bool("primary_pending", observation.primary_pending),
            NativePerfField::bool("cursor_pending", observation.cursor_pending),
            NativePerfField::bool("service_due", service_due),
            NativePerfField::str("action", observation.kind.action()),
            NativePerfField::str("reason", Self::reason(observation)),
            NativePerfField::u64(
                "vrr_window_opportunities",
                counters.vrr_window_opportunities,
            ),
            NativePerfField::u64(
                "vrr_fixed_fallback_reservations",
                counters.vrr_fixed_fallback_reservations,
            ),
            NativePerfField::str(
                "last_fixed_fallback",
                self.last_fixed_fallback
                    .map_or("none", VariableReservationFallback::as_str),
            ),
            NativePerfField::u64("vrr_cursor_piggybacks", counters.vrr_cursor_piggybacks),
            NativePerfField::u64(
                "vrr_cursor_intermediate_updates",
                counters.vrr_cursor_intermediate_updates,
            ),
            NativePerfField::u64(
                "vrr_service_pulses_attempted",
                counters.vrr_service_pulses_attempted,
            ),
            NativePerfField::u64(
                "vrr_service_pulses_confirmed",
                counters.vrr_service_pulses_confirmed,
            ),
            NativePerfField::u64(
                "vrr_service_pulses_noop_or_rejected",
                counters.vrr_service_pulses_noop_or_rejected,
            ),
            NativePerfField::u64(
                "vrr_idle_fixed_fallback_entries",
                counters.vrr_idle_fixed_fallback_entries,
            ),
            NativePerfField::u64(
                "vrr_idle_fixed_fallback_exits",
                counters.vrr_idle_fixed_fallback_exits,
            ),
            NativePerfField::u64("vrr_state_transitions", counters.vrr_state_transitions),
            NativePerfField::u64("vrr_range_floor_events", counters.vrr_range_floor_events),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VRR_CAPABLE: VrrCapabilities = VrrCapabilities {
        policy: VrrPolicy::Auto,
        connector_capable: true,
        crtc_property_available: true,
    };

    fn present(
        kind: VrrPresentKind,
        presented_at_ns: u64,
        confirmed_mode: OutputPresentationMode,
    ) -> VrrPresentObservation {
        VrrPresentObservation {
            kind,
            presented_at_ns,
            confirmed_mode,
            capabilities: VRR_CAPABLE,
            output_generation: 1,
            primary_pending: false,
            cursor_pending: false,
        }
    }

    fn field<'a>(fields: &'a [NativePerfField], key: &str) -> &'a str {
        fields
            .iter()
            .find(|field| field.key == key)
            .map(|field| field.value.as_str())
            .unwrap_or_else(|| panic!("missing field {key}"))
    }

    #[test]
    fn physical_and_content_intervals_are_tracked_separately() {
        let mut recorder = VrrCadenceRecorder::new(VrrRange::discover(165_000, None, None));
        let mode = OutputPresentationMode::AdaptiveSync;
        recorder.observe_present(present(VrrPresentKind::Primary, 1_000_000, mode));
        recorder.observe_present(present(VrrPresentKind::CursorOnly, 4_000_000, mode));
        recorder.observe_present(present(VrrPresentKind::Primary, 11_000_000, mode));

        let observation = present(VrrPresentKind::Primary, 11_000_000, mode);
        let fields = recorder.trace_fields(7, &observation);
        assert_eq!(field(&fields, "last_physical_interval_ns"), "7000000");
        assert_eq!(field(&fields, "content_interval_estimate_ns"), "10000000");
        assert_eq!(field(&fields, "recent_physical_min_ns"), "3000000");
        assert_eq!(field(&fields, "recent_physical_max_ns"), "7000000");
        assert_eq!(field(&fields, "shortest_interval_ns"), "6060606");
        assert_eq!(field(&fields, "longest_interval_ns"), "0");
        assert_eq!(field(&fields, "action"), "primary_present");
        assert_eq!(field(&fields, "reason"), "adaptive_sync_confirmed");
    }

    #[test]
    fn cursor_only_presents_mark_quiescence_and_count_intermediate_updates_under_vrr() {
        let mut recorder = VrrCadenceRecorder::new(VrrRange::discover(165_000, None, None));
        let vrr = OutputPresentationMode::AdaptiveSync;
        recorder.observe_present(present(VrrPresentKind::Primary, 1_000, vrr));
        recorder.observe_present(present(VrrPresentKind::CursorOnly, 2_000, vrr));
        recorder.observe_present(present(VrrPresentKind::CursorOnly, 3_000, vrr));
        assert_eq!(recorder.quiescent_since_ns, Some(1_000));
        assert_eq!(recorder.counters().vrr_cursor_intermediate_updates, 2);

        recorder.observe_present(present(
            VrrPresentKind::CursorOnly,
            4_000,
            OutputPresentationMode::Vsync,
        ));
        assert_eq!(recorder.counters().vrr_cursor_intermediate_updates, 2);

        recorder.observe_present(present(VrrPresentKind::Primary, 5_000, vrr));
        assert_eq!(recorder.quiescent_since_ns, None);
    }

    #[test]
    fn state_transitions_count_confirmed_vrr_changes_only() {
        let mut recorder = VrrCadenceRecorder::new(VrrRange::discover(60_000, None, None));
        let vsync = OutputPresentationMode::Vsync;
        let vrr = OutputPresentationMode::AdaptiveSync;
        recorder.observe_present(present(VrrPresentKind::Primary, 1, vsync));
        recorder.observe_present(present(VrrPresentKind::Primary, 2, vsync));
        recorder.observe_present(present(VrrPresentKind::Primary, 3, vrr));
        recorder.observe_present(present(
            VrrPresentKind::Primary,
            4,
            OutputPresentationMode::AdaptiveAsync,
        ));
        recorder.observe_present(present(VrrPresentKind::Primary, 5, vsync));
        assert_eq!(recorder.counters().vrr_state_transitions, 2);
    }

    #[test]
    fn floor_events_require_a_known_minimum_refresh() {
        let mut recorder = VrrCadenceRecorder::new(VrrRange::discover(165_000, None, None));
        let vrr = OutputPresentationMode::AdaptiveSync;
        recorder.observe_present(present(VrrPresentKind::Primary, 0, vrr));
        recorder.observe_present(present(VrrPresentKind::Primary, 100_000_000, vrr));
        assert_eq!(recorder.counters().vrr_range_floor_events, 0);

        recorder.range.min_hz = Some(48);
        recorder.observe_present(present(VrrPresentKind::Primary, 200_000_000, vrr));
        assert_eq!(recorder.counters().vrr_range_floor_events, 1);
        let observation = present(VrrPresentKind::Primary, 200_000_000, vrr);
        let fields = recorder.trace_fields(1, &observation);
        assert_eq!(field(&fields, "service_due"), "true");
    }

    #[test]
    fn out_of_order_timestamps_produce_no_intervals() {
        let mut recorder = VrrCadenceRecorder::new(VrrRange::discover(60_000, None, None));
        let vsync = OutputPresentationMode::Vsync;
        recorder.observe_present(present(VrrPresentKind::Primary, 10, vsync));
        recorder.observe_present(present(VrrPresentKind::Primary, 5, vsync));
        assert_eq!(recorder.recent_physical_intervals.len, 0);
        assert_eq!(recorder.recent_content_intervals.len, 0);
    }

    #[test]
    fn reason_names_the_first_missing_capability() {
        let mut observation = present(VrrPresentKind::Primary, 1, OutputPresentationMode::Vsync);
        assert_eq!(
            VrrCadenceRecorder::reason(&observation),
            "fixed_vsync_confirmed"
        );
        observation.capabilities.crtc_property_available = false;
        assert_eq!(
            VrrCadenceRecorder::reason(&observation),
            "crtc_vrr_property_missing"
        );
        observation.capabilities.connector_capable = false;
        assert_eq!(
            VrrCadenceRecorder::reason(&observation),
            "connector_not_vrr_capable"
        );
        observation.capabilities.policy = VrrPolicy::Off;
        assert_eq!(VrrCadenceRecorder::reason(&observation), "policy_disabled");
        assert!(!observation.capabilities.relevant());
    }

    #[test]
    fn range_discovery_prefers_override_then_edid_and_never_invents_a_minimum() {
        let edid = EdidVrrRange {
            min_hz: 48,
            max_hz: 165,
            source: EdidVrrRangeSource::FreeSyncVsdb,
        };
        let range = VrrRange::discover(164_999, Some(edid), None);
        assert_eq!(range.min_hz, Some(48));
        assert_eq!(range.max_millihz, 164_999);
        assert_eq!(range.source.as_str(), "edid_freesync_vsdb");

        let range = VrrRange::discover(164_999, Some(edid), Some(60));
        assert_eq!(
            (range.min_hz, range.source),
            (Some(60), VrrRangeSource::UserOverride)
        );

        // An override or EDID minimum at or above the active refresh is unusable.
        let range = VrrRange::discover(48_000, Some(edid), Some(50));
        assert_eq!(
            (range.min_hz, range.source),
            (None, VrrRangeSource::ActiveModeOnly)
        );
        assert_eq!(VrrRange::discover(60_000, None, None).min_hz, None);
    }

    #[test]
    fn composited_reservations_count_windows_and_fixed_fallbacks() {
        let mut recorder = VrrCadenceRecorder::new(VrrRange::discover(165_000, None, None));
        recorder.note_composited_reservation(Ok(()));
        recorder.note_composited_reservation(Ok(()));
        recorder.note_composited_reservation(Err(VariableReservationFallback::NotAdaptiveSync));
        recorder.note_composited_reservation(Err(VariableReservationFallback::CommitTiming));

        let counters = recorder.counters();
        assert_eq!(counters.vrr_window_opportunities, 2);
        assert_eq!(counters.vrr_fixed_fallback_reservations, 1);
        assert_eq!(
            recorder.last_fixed_fallback,
            Some(VariableReservationFallback::CommitTiming)
        );
    }

    #[test]
    fn known_minimum_sets_the_longest_interval() {
        let edid = EdidVrrRange {
            min_hz: 48,
            max_hz: 165,
            source: EdidVrrRangeSource::FreeSyncVsdb,
        };
        let recorder = VrrCadenceRecorder::new(VrrRange::discover(165_000, Some(edid), None));
        assert_eq!(recorder.longest_interval_ns(), Some(20_833_333));
    }
}
