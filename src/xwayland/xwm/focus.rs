//! ICCCM focus and activation policy.

use crate::xwayland::{
    XwaylandGeneration,
    trace::{self, TraceFields},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusModel {
    Input,
    TakeFocusOnly,
    NoFocus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FocusTransitionId {
    pub(crate) generation: XwaylandGeneration,
    pub(crate) serial: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingX11Focus {
    pub(crate) id: FocusTransitionId,
    pub(crate) target: Option<u32>,
    pub(crate) model: FocusModel,
    pub(crate) timestamp: u32,
    pub(crate) request_sequence: Option<u64>,
    pub(crate) sent_set_input_focus: bool,
    pub(crate) sent_take_focus: bool,
    pub(crate) issued_at_ns: u64,
    pub(crate) repair_attempted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FocusRepair {
    pub(crate) id: FocusTransitionId,
    pub(crate) target: Option<u32>,
    pub(crate) model: FocusModel,
    pub(crate) timestamp: u32,
}

// Allow one normal async X event-loop turn while keeping launch-time focus
// failures visible well before a user would reasonably expect a click.
pub(crate) const FOCUS_CONFIRMATION_TIMEOUT_NS: u64 = 100_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum FocusReason {
    InitialMap,
    ClientRequest,
    UserActivation,
    FocusRestoration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum PointerConstraintActivationSource {
    FocusTransition,
    PointerEnter,
    PointerButton,
    BackendAcknowledgement,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum FocusMismatchReason {
    UnexpectedXid,
    RootOrNone,
    StaleGeneration,
    IneligibleMode,
    IneligibleDetail,
    SequenceMismatch,
    NoPendingTransition,
    NoFocusModel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum FocusRepairReason {
    Timeout,
    UnexpectedFocus,
    RootOrNone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum FocusTraceEvent {
    CompositorIntent {
        transition: FocusTransitionId,
        surface_id: Option<u32>,
        window_id: Option<u64>,
        xid: Option<u32>,
        reason: FocusReason,
    },
    WaylandKeyboardFocus {
        transition: FocusTransitionId,
        surface_id: Option<u32>,
    },
    PointerConstraintState {
        transition: FocusTransitionId,
        surface_id: Option<u32>,
        requested: bool,
        active: bool,
        source: PointerConstraintActivationSource,
    },
    XwmCommand {
        transition: FocusTransitionId,
        xid: Option<u32>,
        model: FocusModel,
        timestamp: u32,
    },
    XRequestIssued {
        transition: FocusTransitionId,
        xid: Option<u32>,
        sequence: Option<u64>,
        set_input_focus: bool,
        take_focus: bool,
    },
    ActivePropertyPublished {
        transition: FocusTransitionId,
        xid: Option<u32>,
    },
    FocusIn {
        generation: XwaylandGeneration,
        xid: u32,
        sequence: u16,
        mode: u8,
        detail: u8,
        matched_transition: Option<FocusTransitionId>,
    },
    FocusOut {
        generation: XwaylandGeneration,
        xid: u32,
        sequence: u16,
        mode: u8,
        detail: u8,
    },
    Confirmed {
        transition: FocusTransitionId,
        xid: Option<u32>,
        latency_us: u64,
    },
    Mismatch {
        transition: FocusTransitionId,
        desired: Option<u32>,
        observed: Option<u32>,
        reason: FocusMismatchReason,
    },
    Timeout {
        transition: FocusTransitionId,
        xid: Option<u32>,
        elapsed_us: u64,
    },
    Repair {
        transition: FocusTransitionId,
        xid: Option<u32>,
        reason: FocusRepairReason,
    },
}

const FOCUS_TRACE_CAPACITY: usize = 512;

#[derive(Debug)]
struct FocusTraceRing {
    enabled: bool,
    next: usize,
    len: usize,
    events: [Option<FocusTraceEvent>; FOCUS_TRACE_CAPACITY],
}

impl FocusTraceRing {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            next: 0,
            len: 0,
            events: [None; FOCUS_TRACE_CAPACITY],
        }
    }

    fn record(&mut self, event: FocusTraceEvent) {
        if !self.enabled {
            return;
        }
        self.events[self.next] = Some(event);
        self.next = (self.next + 1) % FOCUS_TRACE_CAPACITY;
        self.len = self.len.saturating_add(1).min(FOCUS_TRACE_CAPACITY);
    }

    #[cfg(test)]
    fn iter(&self) -> impl Iterator<Item = FocusTraceEvent> + '_ {
        let start = (self.next + FOCUS_TRACE_CAPACITY - self.len) % FOCUS_TRACE_CAPACITY;
        (0..self.len).filter_map(move |offset| self.events[(start + offset) % FOCUS_TRACE_CAPACITY])
    }
}

impl Default for FocusTraceRing {
    fn default() -> Self {
        Self::new(trace::enabled())
    }
}

const X11_TIME_HALF_RANGE: u32 = 0x8000_0000;
const ACTIVATION_WINDOW_MS: u32 = 10_000;

#[derive(Debug, Default)]
pub(crate) struct FocusTracker {
    confirmed_server_focus: Option<u32>,
    confirmed_server_sequence: Option<u16>,
    desired_focus: Option<u32>,
    published_active_window: Option<u32>,
    pending_focus: Option<PendingX11Focus>,
    current_generation: Option<XwaylandGeneration>,
    next_transition_serial: u64,
    current_time: Option<u32>,
    last_user_time: Option<u32>,
    trace: FocusTraceRing,
}

impl FocusTracker {
    #[cfg(test)]
    fn with_trace_enabled(enabled: bool) -> Self {
        Self {
            trace: FocusTraceRing::new(enabled),
            ..Self::default()
        }
    }

    pub(crate) fn note_server_timestamp(&mut self, timestamp: u32) {
        if timestamp == 0
            || self
                .current_time
                .is_some_and(|current| !x11_time_after_eq(timestamp, current))
        {
            return;
        }
        self.current_time = Some(timestamp);
    }

    pub(crate) fn note_user_time(&mut self, timestamp: Option<u32>) {
        if let Some(timestamp) = timestamp {
            self.note_server_timestamp(timestamp);
            self.last_user_time = Some(timestamp);
        }
    }

    pub(crate) fn note_activation_request(
        &mut self,
        _xid: u32,
        timestamp: u32,
    ) -> (u32, Option<u32>) {
        let current_time = self.current_time.unwrap_or_default();
        self.note_server_timestamp(timestamp);
        (current_time, self.last_user_time)
    }

    pub(crate) fn begin_focus_transition(
        &mut self,
        generation: XwaylandGeneration,
        target: Option<u32>,
        model: FocusModel,
        timestamp: u32,
    ) -> FocusTransitionId {
        let id = self.allocate_transition_id(generation);
        self.note_server_timestamp(timestamp);
        self.current_generation = Some(generation);
        self.desired_focus = target;
        self.pending_focus = Some(PendingX11Focus {
            id,
            target,
            model,
            timestamp,
            request_sequence: None,
            sent_set_input_focus: false,
            sent_take_focus: false,
            issued_at_ns: 0,
            repair_attempted: false,
        });
        self.record_trace(FocusTraceEvent::XwmCommand {
            transition: id,
            xid: target,
            model,
            timestamp,
        });
        id
    }

    fn record_trace(&mut self, event: FocusTraceEvent) {
        self.trace.record(event);
        trace::emit("focus_lifecycle", || {
            TraceFields::new().field("event", format!("{event:?}"))
        });
    }

    fn allocate_transition_id(&mut self, generation: XwaylandGeneration) -> FocusTransitionId {
        let mut serial = self.next_transition_serial.max(1);
        loop {
            self.next_transition_serial = serial.wrapping_add(1).max(1);
            if self
                .pending_focus
                .is_none_or(|pending| pending.id.serial != serial)
            {
                return FocusTransitionId { generation, serial };
            }
            serial = self.next_transition_serial;
        }
    }

    pub(crate) fn note_focus_request_issued(
        &mut self,
        transition: FocusTransitionId,
        request_sequence: Option<u64>,
        sent_set_input_focus: bool,
        sent_take_focus: bool,
    ) {
        self.note_focus_request_issued_at(
            transition,
            request_sequence,
            sent_set_input_focus,
            sent_take_focus,
            crate::native::event_loop::monotonic_now_ns().unwrap_or_default(),
        );
    }

    fn note_focus_request_issued_at(
        &mut self,
        transition: FocusTransitionId,
        request_sequence: Option<u64>,
        sent_set_input_focus: bool,
        sent_take_focus: bool,
        issued_at_ns: u64,
    ) {
        let target = if let Some(pending) = self.pending_focus.as_mut()
            && pending.id == transition
        {
            pending.request_sequence = request_sequence;
            pending.sent_set_input_focus = sent_set_input_focus;
            pending.sent_take_focus = sent_take_focus;
            pending.issued_at_ns = issued_at_ns;
            Some(pending.target)
        } else {
            None
        };
        if let Some(target) = target {
            self.record_trace(FocusTraceEvent::XRequestIssued {
                transition,
                xid: target,
                sequence: request_sequence,
                set_input_focus: sent_set_input_focus,
                take_focus: sent_take_focus,
            });
        }
    }

    pub(crate) fn next_focus_deadline_ns(&self) -> Option<u64> {
        self.pending_focus.and_then(|pending| {
            (pending.target.is_some()
                && pending.issued_at_ns != 0
                && !pending.repair_attempted
                && (pending.sent_set_input_focus || pending.sent_take_focus))
                .then(|| {
                    pending
                        .issued_at_ns
                        .saturating_add(FOCUS_CONFIRMATION_TIMEOUT_NS)
                })
        })
    }

    pub(crate) fn take_focus_repair(&mut self, now_ns: u64) -> Option<FocusRepair> {
        let deadline = self.next_focus_deadline_ns()?;
        if now_ns < deadline {
            return None;
        }
        let timestamp = self.current_focus_timestamp();
        let pending = self.pending_focus.as_mut()?;
        pending.repair_attempted = true;
        let elapsed_us = now_ns.saturating_sub(pending.issued_at_ns) / 1_000;
        let repair = FocusRepair {
            id: pending.id,
            target: pending.target,
            model: pending.model,
            timestamp,
        };
        self.record_trace(FocusTraceEvent::Timeout {
            transition: repair.id,
            xid: repair.target,
            elapsed_us,
        });
        self.record_trace(FocusTraceEvent::Repair {
            transition: repair.id,
            xid: repair.target,
            reason: FocusRepairReason::Timeout,
        });
        Some(repair)
    }

    pub(crate) fn note_focus_request_failed(&mut self, transition: FocusTransitionId) {
        if self
            .pending_focus
            .is_some_and(|pending| pending.id == transition)
        {
            self.pending_focus = None;
        }
    }

    pub(crate) fn update_pending_focus_model(
        &mut self,
        transition: FocusTransitionId,
        model: FocusModel,
    ) -> bool {
        let Some(pending) = self.pending_focus.as_mut() else {
            return false;
        };
        if pending.id != transition {
            return false;
        }
        pending.model = model;
        if matches!(model, FocusModel::NoFocus) {
            pending.request_sequence = None;
            pending.sent_set_input_focus = false;
            pending.sent_take_focus = false;
            pending.issued_at_ns = 0;
        }
        true
    }

    pub(crate) fn note_active_property_published(
        &mut self,
        transition: FocusTransitionId,
        xid: Option<u32>,
    ) {
        if self
            .pending_focus
            .is_some_and(|pending| pending.id == transition)
        {
            self.published_active_window = xid;
            self.record_trace(FocusTraceEvent::ActivePropertyPublished { transition, xid });
        }
    }

    pub(crate) fn note_focus_command(
        &mut self,
        generation: XwaylandGeneration,
        xid: Option<u32>,
        model: FocusModel,
        timestamp: u32,
    ) -> FocusTransitionId {
        self.begin_focus_transition(generation, xid, model, timestamp)
    }

    pub(crate) fn current_server_time(&self) -> u32 {
        self.current_time.unwrap_or(1)
    }

    pub(crate) fn current_focus_timestamp(&self) -> u32 {
        self.current_time.unwrap_or(0)
    }

    pub(crate) fn note_focus_in_event_with_root(
        &mut self,
        generation: XwaylandGeneration,
        root_xid: Option<u32>,
        xid: u32,
        sequence: u16,
        mode: u8,
        detail: u8,
    ) {
        let current_generation = self.current_generation == Some(generation);
        let mode_eligible = mode == 0;
        let detail_eligible = !matches!(detail, 5 | 6);
        let pending = self.pending_focus;
        let matched_transition = pending.and_then(|pending| {
            (current_generation
                && mode_eligible
                && detail_eligible
                && pending.id.generation == generation
                && pending.target == Some(xid)
                && !matches!(pending.model, FocusModel::NoFocus)
                && request_sequence_matches(pending.request_sequence, sequence))
            .then_some(pending.id)
        });
        self.record_trace(FocusTraceEvent::FocusIn {
            generation,
            xid,
            sequence,
            mode,
            detail,
            matched_transition,
        });
        if !current_generation {
            if let Some(pending) = pending {
                self.record_trace(FocusTraceEvent::Mismatch {
                    transition: pending.id,
                    desired: pending.target,
                    observed: Some(xid),
                    reason: FocusMismatchReason::StaleGeneration,
                });
            }
            return;
        }
        if !mode_eligible {
            if let Some(pending) = pending {
                self.record_trace(FocusTraceEvent::Mismatch {
                    transition: pending.id,
                    desired: pending.target,
                    observed: Some(xid),
                    reason: FocusMismatchReason::IneligibleMode,
                });
            }
            return;
        }
        if !detail_eligible {
            if let Some(pending) = pending {
                self.record_trace(FocusTraceEvent::Mismatch {
                    transition: pending.id,
                    desired: pending.target,
                    observed: Some(xid),
                    reason: FocusMismatchReason::IneligibleDetail,
                });
            }
            return;
        }
        if pending.is_some_and(|pending| {
            pending.target == Some(xid) && matches!(pending.model, FocusModel::NoFocus)
        }) {
            let pending = pending.expect("pending focus checked above");
            self.record_trace(FocusTraceEvent::Mismatch {
                transition: pending.id,
                desired: pending.target,
                observed: Some(xid),
                reason: FocusMismatchReason::NoFocusModel,
            });
            return;
        }
        self.confirmed_server_focus = Some(xid);
        self.confirmed_server_sequence = (sequence != 0).then_some(sequence);
        if let Some(transition) = matched_transition {
            let latency_us = self
                .pending_focus
                .and_then(|pending| {
                    (pending.issued_at_ns != 0).then(|| {
                        crate::native::event_loop::monotonic_now_ns()
                            .unwrap_or_default()
                            .saturating_sub(pending.issued_at_ns)
                            / 1_000
                    })
                })
                .unwrap_or_default();
            self.pending_focus = None;
            self.record_trace(FocusTraceEvent::Confirmed {
                transition,
                xid: Some(xid),
                latency_us,
            });
        } else if let Some(pending) = self.pending_focus
            && pending.id.generation == generation
        {
            self.record_trace(FocusTraceEvent::Mismatch {
                transition: pending.id,
                desired: pending.target,
                observed: Some(xid),
                reason: if root_xid == Some(xid) {
                    FocusMismatchReason::RootOrNone
                } else if pending.target == Some(xid)
                    && !request_sequence_matches(pending.request_sequence, sequence)
                {
                    FocusMismatchReason::SequenceMismatch
                } else {
                    FocusMismatchReason::UnexpectedXid
                },
            });
        }
    }

    pub(crate) fn note_focus_out_event(
        &mut self,
        generation: XwaylandGeneration,
        xid: u32,
        sequence: u16,
        mode: u8,
        detail: u8,
    ) {
        self.record_trace(FocusTraceEvent::FocusOut {
            generation,
            xid,
            sequence,
            mode,
            detail,
        });
        if self.current_generation != Some(generation) {
            return;
        }
        let sequence_is_current = self.confirmed_server_sequence.is_none_or(|confirmed| {
            sequence == 0 || confirmed == 0 || x11_sequence_after_eq(sequence, confirmed)
        });
        if mode == 0
            && !matches!(detail, 5 | 6)
            && sequence_is_current
            && self.confirmed_server_focus == Some(xid)
        {
            self.confirmed_server_focus = None;
            self.confirmed_server_sequence = None;
        }
    }

    pub(crate) fn reset_generation(&mut self, generation: XwaylandGeneration) {
        self.current_generation = Some(generation);
        self.confirmed_server_focus = None;
        self.confirmed_server_sequence = None;
        self.desired_focus = None;
        self.published_active_window = None;
        self.pending_focus = None;
        self.next_transition_serial = 1;
    }

    pub(crate) fn cancel_focus_transition(&mut self, transition: FocusTransitionId) {
        if self
            .pending_focus
            .is_some_and(|pending| pending.id == transition)
        {
            self.pending_focus = None;
        }
    }

    pub(crate) fn note_destroyed(&mut self, xid: u32) {
        self.note_target_gone(xid);
    }

    pub(crate) fn note_unmapped(&mut self, xid: u32) {
        self.note_target_gone(xid);
    }

    fn note_target_gone(&mut self, xid: u32) {
        if self.confirmed_server_focus == Some(xid) {
            self.confirmed_server_focus = None;
            self.confirmed_server_sequence = None;
        }
        if self.desired_focus == Some(xid) {
            self.desired_focus = None;
        }
        if self.published_active_window == Some(xid) {
            self.published_active_window = None;
        }
        if self
            .pending_focus
            .is_some_and(|pending| pending.target == Some(xid))
        {
            self.pending_focus = None;
        }
    }

    #[allow(dead_code)]
    pub(crate) fn desired_focus(&self) -> Option<u32> {
        self.desired_focus
    }

    pub(crate) fn confirmed_focus(&self) -> Option<u32> {
        self.confirmed_server_focus
    }

    #[allow(dead_code)]
    pub(crate) fn published_active_window(&self) -> Option<u32> {
        self.published_active_window
    }

    pub(crate) fn pending_focus(&self) -> Option<&PendingX11Focus> {
        self.pending_focus.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn trace_events(&self) -> impl Iterator<Item = FocusTraceEvent> + '_ {
        self.trace.iter()
    }
}

fn request_sequence_matches(request_sequence: Option<u64>, event_sequence: u16) -> bool {
    let Some(request_sequence) = request_sequence else {
        return true;
    };
    event_sequence.wrapping_sub(request_sequence as u16) < 0x8000
}

fn x11_sequence_after_eq(left: u16, right: u16) -> bool {
    left == right || left.wrapping_sub(right) < 0x8000
}

pub(crate) const fn x11_time_after_eq(left: u32, right: u32) -> bool {
    left == right || left.wrapping_sub(right) < X11_TIME_HALF_RANGE
}

fn x11_time_elapsed(now: u32, then: u32) -> Option<u32> {
    x11_time_after_eq(now, then).then(|| now.wrapping_sub(then))
}

pub(crate) fn focus_model(input: Option<bool>, take_focus: bool) -> FocusModel {
    match (input.unwrap_or(true), take_focus) {
        (true, _) => FocusModel::Input,
        (false, true) => FocusModel::TakeFocusOnly,
        (false, false) => FocusModel::NoFocus,
    }
}

pub(crate) fn activation_allowed(
    source_is_user: bool,
    timestamp: u32,
    now: u32,
    last_user_time: Option<u32>,
    current_focus: bool,
    valid_transient: bool,
    startup_token: bool,
) -> bool {
    let recent_request = timestamp != 0
        && x11_time_elapsed(now, timestamp).is_some_and(|elapsed| elapsed < ACTIVATION_WINDOW_MS);
    let recent_user_time = last_user_time.is_some_and(|last| {
        timestamp != 0
            && x11_time_after_eq(timestamp, last)
            && timestamp.wrapping_sub(last) < ACTIVATION_WINDOW_MS
    });
    source_is_user && recent_request
        || current_focus
        || valid_transient
        || startup_token
        || recent_user_time && recent_request
}

pub(crate) fn should_send_take_focus(input: Option<bool>, take_focus: bool) -> bool {
    matches!(focus_model(input, take_focus), FocusModel::TakeFocusOnly)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use crate::xwayland::XwaylandGeneration;

    use super::*;

    #[test]
    fn input_false_only_uses_take_focus_protocol() {
        assert_eq!(focus_model(Some(false), true), FocusModel::TakeFocusOnly);
        assert_eq!(focus_model(Some(false), false), FocusModel::NoFocus);
        assert!(should_send_take_focus(Some(false), true));
    }

    #[test]
    fn sync_request_fallback_time_is_nonzero() {
        assert_ne!(FocusTracker::default().current_server_time(), 0);
    }

    #[test]
    fn focus_timestamp_uses_current_time_or_x11_current_time() {
        let mut tracker = FocusTracker::default();
        assert_eq!(tracker.current_focus_timestamp(), 0);
        tracker.note_server_timestamp(77);
        assert_eq!(tracker.current_focus_timestamp(), 77);
    }

    #[test]
    fn x11_time_comparison_handles_wraparound() {
        assert!(x11_time_after_eq(2, u32::MAX - 2));
        assert!(!x11_time_after_eq(u32::MAX - 2, 2));
    }

    #[test]
    fn issuing_focus_command_remains_pending_until_focus_in() {
        let mut tracker = FocusTracker::default();
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        let transition =
            tracker.begin_focus_transition(generation, Some(10), FocusModel::Input, 105);

        tracker.note_focus_request_issued(transition, Some(42), true, true);

        assert_eq!(tracker.desired_focus(), Some(10));
        assert_eq!(tracker.confirmed_focus(), None);
        assert!(tracker.pending_focus().is_some());
    }

    #[test]
    fn matching_focus_in_confirms_only_the_current_generation_transition() {
        let mut tracker = FocusTracker::default();
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        let stale_generation = XwaylandGeneration::new(NonZeroU64::new(6).unwrap());
        let transition =
            tracker.begin_focus_transition(generation, Some(10), FocusModel::Input, 105);
        tracker.note_focus_request_issued(transition, Some(42), true, false);

        tracker.note_focus_in_event_with_root(stale_generation, None, 10, 42, 0, 0);
        assert_eq!(tracker.confirmed_focus(), None);
        assert!(tracker.pending_focus().is_some());

        tracker.note_focus_in_event_with_root(generation, None, 10, 42, 0, 0);
        assert_eq!(tracker.confirmed_focus(), Some(10));
        assert_eq!(tracker.pending_focus(), None);
    }

    #[test]
    fn newer_transition_supersedes_older_pending_transition() {
        let mut tracker = FocusTracker::default();
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        let old = tracker.begin_focus_transition(generation, Some(10), FocusModel::Input, 105);
        let new = tracker.begin_focus_transition(generation, Some(11), FocusModel::Input, 106);

        assert_ne!(old, new);
        assert_eq!(tracker.desired_focus(), Some(11));
        assert_eq!(tracker.pending_focus().map(|pending| pending.id), Some(new));
        tracker.cancel_focus_transition(old);
        assert_eq!(tracker.pending_focus().map(|pending| pending.id), Some(new));
    }

    #[test]
    fn transition_serial_wrap_skips_a_pending_serial() {
        let mut tracker = FocusTracker::default();
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        let first = tracker.begin_focus_transition(generation, Some(10), FocusModel::Input, 105);
        tracker.next_transition_serial = 1;
        let skipped = tracker.begin_focus_transition(generation, Some(11), FocusModel::Input, 106);

        assert_eq!(first.serial, 1);
        assert_eq!(skipped.serial, 2);
        tracker.next_transition_serial = u64::MAX;
        let wrapped = tracker.begin_focus_transition(generation, Some(12), FocusModel::Input, 107);
        assert_eq!(wrapped.serial, u64::MAX);
    }

    #[test]
    fn late_no_focus_model_cancels_request_deadline_without_synthesizing_focus() {
        let mut tracker = FocusTracker::default();
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        let transition =
            tracker.begin_focus_transition(generation, Some(10), FocusModel::Input, 105);
        tracker.note_focus_request_issued_at(transition, Some(42), true, false, 1_000);

        assert!(tracker.update_pending_focus_model(transition, FocusModel::NoFocus));
        assert_eq!(tracker.next_focus_deadline_ns(), None);
        assert!(tracker.pending_focus().is_some());
    }

    #[test]
    fn clearing_focus_does_not_schedule_a_target_repair() {
        let mut tracker = FocusTracker::default();
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        let transition = tracker.begin_focus_transition(generation, None, FocusModel::Input, 105);
        tracker.note_focus_request_issued_at(transition, Some(42), true, false, 1_000);

        assert_eq!(tracker.next_focus_deadline_ns(), None);
    }

    #[test]
    fn active_property_publication_is_not_server_focus_confirmation() {
        let mut tracker = FocusTracker::default();
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        let transition =
            tracker.begin_focus_transition(generation, Some(10), FocusModel::Input, 105);
        tracker.note_active_property_published(transition, Some(10));

        assert_eq!(tracker.published_active_window(), Some(10));
        assert_eq!(tracker.confirmed_focus(), None);
    }

    #[test]
    fn destroying_target_cancels_its_pending_transition() {
        let mut tracker = FocusTracker::default();
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        tracker.begin_focus_transition(generation, Some(10), FocusModel::Input, 105);

        tracker.note_destroyed(10);

        assert_eq!(tracker.desired_focus(), None);
        assert_eq!(tracker.published_active_window(), None);
        assert_eq!(tracker.pending_focus(), None);
    }

    #[test]
    fn unmapping_target_cancels_its_pending_transition() {
        let mut tracker = FocusTracker::default();
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        tracker.begin_focus_transition(generation, Some(10), FocusModel::Input, 105);

        tracker.note_unmapped(10);

        assert_eq!(tracker.desired_focus(), None);
        assert_eq!(tracker.pending_focus(), None);
    }

    #[test]
    fn disabled_trace_records_nothing() {
        let mut tracker = FocusTracker::with_trace_enabled(false);
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        tracker.begin_focus_transition(generation, Some(10), FocusModel::Input, 105);

        assert_eq!(tracker.trace_events().count(), 0);
    }

    #[test]
    fn enabled_trace_reconstructs_one_transition_in_order() {
        let mut tracker = FocusTracker::with_trace_enabled(true);
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        let transition =
            tracker.begin_focus_transition(generation, Some(10), FocusModel::Input, 105);
        tracker.note_focus_request_issued(transition, Some(42), true, false);
        tracker.note_active_property_published(transition, Some(10));
        tracker.note_focus_in_event_with_root(generation, None, 10, 42, 0, 0);

        let events: Vec<_> = tracker.trace_events().collect();
        assert!(
            matches!(events[0], FocusTraceEvent::XwmCommand { transition: id, .. } if id == transition)
        );
        assert!(
            matches!(events[1], FocusTraceEvent::XRequestIssued { transition: id, .. } if id == transition)
        );
        assert!(
            matches!(events[2], FocusTraceEvent::ActivePropertyPublished { transition: id, .. } if id == transition)
        );
        assert!(
            matches!(events[3], FocusTraceEvent::FocusIn { matched_transition: Some(id), .. } if id == transition)
        );
        assert!(
            matches!(events[4], FocusTraceEvent::Confirmed { transition: id, .. } if id == transition)
        );
    }

    #[test]
    fn trace_ring_keeps_only_the_most_recent_events() {
        let mut tracker = FocusTracker::with_trace_enabled(true);
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        for xid in 1..=600 {
            tracker.begin_focus_transition(generation, Some(xid), FocusModel::Input, xid);
        }

        let events: Vec<_> = tracker.trace_events().collect();
        assert_eq!(events.len(), FOCUS_TRACE_CAPACITY);
        assert!(matches!(
            events[0],
            FocusTraceEvent::XwmCommand { xid: Some(89), .. }
        ));
        assert!(matches!(
            events[FOCUS_TRACE_CAPACITY - 1],
            FocusTraceEvent::XwmCommand { xid: Some(600), .. }
        ));
    }

    #[test]
    fn pointer_detail_and_no_focus_model_do_not_confirm_keyboard_focus() {
        let mut tracker = FocusTracker::with_trace_enabled(true);
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        let input_transition =
            tracker.begin_focus_transition(generation, Some(10), FocusModel::Input, 105);
        tracker.note_focus_request_issued(input_transition, Some(42), true, false);
        tracker.note_focus_in_event_with_root(generation, None, 10, 42, 0, 5);
        assert_eq!(tracker.confirmed_focus(), None);
        assert!(tracker.pending_focus().is_some());

        let no_focus_transition =
            tracker.begin_focus_transition(generation, Some(11), FocusModel::NoFocus, 106);
        tracker.note_focus_request_issued(no_focus_transition, None, false, false);
        tracker.note_focus_in_event_with_root(generation, None, 11, 0, 0, 0);
        assert_ne!(tracker.confirmed_focus(), Some(11));
        assert!(tracker.pending_focus().is_some());
    }

    #[test]
    fn focus_in_sequence_must_follow_the_issued_request() {
        let mut tracker = FocusTracker::with_trace_enabled(true);
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        let transition =
            tracker.begin_focus_transition(generation, Some(10), FocusModel::Input, 105);
        tracker.note_focus_request_issued(transition, Some(100), true, false);

        tracker.note_focus_in_event_with_root(generation, None, 10, 99, 0, 0);

        assert_eq!(tracker.confirmed_focus(), Some(10));
        assert!(tracker.pending_focus().is_some());
    }

    #[test]
    fn root_focus_is_observed_as_a_mismatch() {
        let mut tracker = FocusTracker::with_trace_enabled(true);
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        let transition =
            tracker.begin_focus_transition(generation, Some(10), FocusModel::Input, 105);
        tracker.note_focus_request_issued(transition, Some(42), true, false);
        tracker.note_focus_in_event_with_root(generation, Some(1), 1, 42, 0, 0);

        assert_eq!(tracker.confirmed_focus(), Some(1));
        assert!(tracker.pending_focus().is_some());
        assert!(tracker.trace_events().any(|event| matches!(
            event,
            FocusTraceEvent::Mismatch {
                reason: FocusMismatchReason::RootOrNone,
                ..
            }
        )));
    }

    #[test]
    fn grab_focus_out_noise_does_not_clear_confirmed_focus() {
        let mut tracker = FocusTracker::with_trace_enabled(true);
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        let transition =
            tracker.begin_focus_transition(generation, Some(10), FocusModel::Input, 105);
        tracker.note_focus_request_issued(transition, Some(42), true, false);
        tracker.note_focus_in_event_with_root(generation, None, 10, 42, 0, 0);

        tracker.note_focus_out_event(generation, 10, 43, 1, 0);

        assert_eq!(tracker.confirmed_focus(), Some(10));
    }

    #[test]
    fn older_focus_out_cannot_clear_a_newer_confirmation() {
        let mut tracker = FocusTracker::with_trace_enabled(true);
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        let transition =
            tracker.begin_focus_transition(generation, Some(10), FocusModel::Input, 105);
        tracker.note_focus_request_issued(transition, Some(42), true, false);
        tracker.note_focus_in_event_with_root(generation, None, 10, 100, 0, 0);

        tracker.note_focus_out_event(generation, 10, 99, 0, 0);
        assert_eq!(tracker.confirmed_focus(), Some(10));
        tracker.note_focus_out_event(generation, 10, 101, 0, 0);
        assert_eq!(tracker.confirmed_focus(), None);
    }

    #[test]
    fn focus_timeout_allows_exactly_one_repair_for_a_transition() {
        let mut tracker = FocusTracker::with_trace_enabled(true);
        let generation = XwaylandGeneration::new(NonZeroU64::new(7).unwrap());
        let transition =
            tracker.begin_focus_transition(generation, Some(10), FocusModel::Input, 105);
        tracker.note_focus_request_issued_at(transition, Some(42), true, false, 1_000);

        assert_eq!(
            tracker.next_focus_deadline_ns(),
            Some(1_000 + FOCUS_CONFIRMATION_TIMEOUT_NS)
        );
        assert_eq!(
            tracker.take_focus_repair(1_000 + FOCUS_CONFIRMATION_TIMEOUT_NS - 1),
            None
        );

        let repair = tracker
            .take_focus_repair(1_000 + FOCUS_CONFIRMATION_TIMEOUT_NS)
            .expect("first timeout should repair");
        assert_eq!(repair.id, transition);
        assert_eq!(tracker.take_focus_repair(u64::MAX), None);
    }

    #[test]
    fn activation_uses_real_current_and_user_times() {
        assert!(!activation_allowed(true, 100, 0, None, false, false, false));
        assert!(activation_allowed(
            true, 100, 105, None, false, false, false
        ));
    }
}
