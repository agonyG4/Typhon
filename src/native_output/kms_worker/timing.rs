//! Deadline-aware timing for submit-only Atomic jobs.

use oblivion_one::native::presentation_deadline::{PresentationTarget, PresentationTargetReason};
use std::time::Duration;

const MIN_SAFETY_MARGIN_NS: u64 = 100_000;
const INITIAL_SAFETY_MARGIN_NS: u64 = 1_000_000;
const MAX_SAFETY_MARGIN_NS: u64 = 3_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KmsTimingDecision {
    pub(crate) submit_at_ns: u64,
    pub(crate) late: bool,
    pub(crate) late_by_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KmsCommitTimingModel {
    safety_margin_ns: u64,
    refresh_interval_ns: u64,
}

impl KmsCommitTimingModel {
    pub(crate) fn new(refresh_interval: Duration) -> Self {
        let refresh_interval_ns = u64::try_from(refresh_interval.as_nanos())
            .unwrap_or(u64::MAX)
            .max(1);
        let mut model = Self {
            safety_margin_ns: INITIAL_SAFETY_MARGIN_NS,
            refresh_interval_ns,
        };
        model.safety_margin_ns = model.clamp_margin(model.safety_margin_ns);
        model
    }

    pub(crate) const fn safety_margin_ns(self) -> u64 {
        self.safety_margin_ns
    }

    pub(crate) fn submit_at(self, target: PresentationTarget, now_ns: u64) -> KmsTimingDecision {
        let desired = if matches!(target.reason, PresentationTargetReason::ReactiveDouble) {
            target.submit_not_before.get()
        } else {
            target
                .presentation_time
                .get()
                .saturating_sub(self.safety_margin_ns)
                .max(target.submit_not_before.get())
        };
        let submit_at_ns = desired.max(now_ns);
        KmsTimingDecision {
            submit_at_ns,
            late: now_ns > desired,
            late_by_ns: now_ns.saturating_sub(desired),
        }
    }

    pub(crate) fn observe_submit_delta_ns(&mut self, delta_ns: i64) {
        if delta_ns > 0 {
            let late_ns = u64::try_from(delta_ns).unwrap_or(u64::MAX);
            self.safety_margin_ns = self.clamp_margin(late_ns.saturating_add(MIN_SAFETY_MARGIN_NS));
            return;
        }
        let difference = self.safety_margin_ns.saturating_sub(MIN_SAFETY_MARGIN_NS);
        self.safety_margin_ns = self
            .safety_margin_ns
            .saturating_sub(difference / 16)
            .max(MIN_SAFETY_MARGIN_NS);
    }

    fn clamp_margin(self, value: u64) -> u64 {
        let maximum = MAX_SAFETY_MARGIN_NS.min(self.refresh_interval_ns / 2);
        if maximum < MIN_SAFETY_MARGIN_NS {
            value.min(maximum)
        } else {
            value.clamp(MIN_SAFETY_MARGIN_NS, maximum)
        }
    }
}
