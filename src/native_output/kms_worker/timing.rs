//! Deadline-aware timing for submit-only Atomic jobs.

use oblivion_one::native::presentation_deadline::{PresentationTarget, PresentationTargetReason};
use std::collections::VecDeque;
use std::time::Duration;

const MIN_SAFETY_MARGIN_NS: u64 = 100_000;
const INITIAL_SAFETY_MARGIN_NS: u64 = 1_000_000;
const MAX_SAFETY_MARGIN_NS: u64 = 3_000_000;
const SAMPLE_CAPACITY: usize = 120;
const PRESENTATION_FLOOR_DECAY_SAMPLES: u8 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KmsTimingDecision {
    pub(crate) submit_deadline_ns: u64,
    pub(crate) submit_at_ns: u64,
    pub(crate) late: bool,
    pub(crate) late_by_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KmsSubmissionBudget {
    pub(crate) submit_wake_lateness_ns: u64,
    pub(crate) ioctl_duration_ns: u64,
    pub(crate) submission_budget_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KmsCommitTimingModel {
    safety_margin_ns: u64,
    presentation_margin_floor_ns: u64,
    presentation_floor_early_samples: u8,
    refresh_interval_ns: u64,
    submit_wake_lateness_ns: VecDeque<u64>,
    ioctl_duration_ns: VecDeque<u64>,
}

impl KmsCommitTimingModel {
    pub(crate) fn new(refresh_interval: Duration) -> Self {
        let refresh_interval_ns = u64::try_from(refresh_interval.as_nanos())
            .unwrap_or(u64::MAX)
            .max(1);
        let mut model = Self {
            safety_margin_ns: INITIAL_SAFETY_MARGIN_NS,
            presentation_margin_floor_ns: 0,
            presentation_floor_early_samples: 0,
            refresh_interval_ns,
            submit_wake_lateness_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
            ioctl_duration_ns: VecDeque::with_capacity(SAMPLE_CAPACITY),
        };
        model.safety_margin_ns = model.clamp_margin(model.safety_margin_ns);
        model
    }

    pub(crate) const fn safety_margin_ns(&self) -> u64 {
        self.safety_margin_ns
    }

    pub(crate) fn reconfigure_refresh_interval(&mut self, refresh_interval: Duration) {
        let refresh_interval_ns = u64::try_from(refresh_interval.as_nanos())
            .unwrap_or(u64::MAX)
            .max(1);
        if refresh_interval_ns == self.refresh_interval_ns {
            return;
        }
        self.refresh_interval_ns = refresh_interval_ns;
        self.submit_wake_lateness_ns.clear();
        self.ioctl_duration_ns.clear();
        self.safety_margin_ns = self.clamp_margin(self.safety_margin_ns);
        self.presentation_margin_floor_ns = if self.presentation_margin_floor_ns == 0 {
            0
        } else {
            self.clamp_margin(self.presentation_margin_floor_ns)
        };
        self.presentation_floor_early_samples = 0;
    }

    pub(crate) fn submission_budget(&self) -> KmsSubmissionBudget {
        KmsSubmissionBudget {
            submit_wake_lateness_ns: nearest_rank(&self.submit_wake_lateness_ns, 95),
            ioctl_duration_ns: nearest_rank(&self.ioctl_duration_ns, 95),
            submission_budget_ns: self.safety_margin_ns,
        }
    }

    pub(crate) fn submit_at(&self, target: PresentationTarget, now_ns: u64) -> KmsTimingDecision {
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
            submit_deadline_ns: desired,
            submit_at_ns,
            late: now_ns > desired,
            late_by_ns: now_ns.saturating_sub(desired),
        }
    }

    pub(crate) fn observe_submit_delta_ns(&mut self, delta_ns: i64) {
        if delta_ns > 0 {
            let late_ns = u64::try_from(delta_ns).unwrap_or(u64::MAX);
            let required_margin_ns =
                self.clamp_margin(late_ns.saturating_add(MIN_SAFETY_MARGIN_NS));
            self.presentation_floor_early_samples = 0;
            self.safety_margin_ns = self
                .safety_margin_ns
                .max(self.presentation_margin_floor_ns)
                .max(required_margin_ns);
            return;
        }
        let difference = self.safety_margin_ns.saturating_sub(MIN_SAFETY_MARGIN_NS);
        self.safety_margin_ns = self
            .safety_margin_ns
            .saturating_sub(difference / 16)
            .max(MIN_SAFETY_MARGIN_NS);
        if self.presentation_margin_floor_ns > MIN_SAFETY_MARGIN_NS {
            self.presentation_floor_early_samples =
                self.presentation_floor_early_samples.saturating_add(1);
            if self.presentation_floor_early_samples >= PRESENTATION_FLOOR_DECAY_SAMPLES {
                let difference = self
                    .presentation_margin_floor_ns
                    .saturating_sub(MIN_SAFETY_MARGIN_NS);
                self.presentation_margin_floor_ns = self
                    .presentation_margin_floor_ns
                    .saturating_sub(difference / 16)
                    .max(MIN_SAFETY_MARGIN_NS);
                self.presentation_floor_early_samples = 0;
            }
            self.safety_margin_ns = self.safety_margin_ns.max(self.presentation_margin_floor_ns);
        }
    }

    pub(crate) fn observe_submit_result(
        &mut self,
        submit_returned_ns: u64,
        submit_deadline_ns: u64,
    ) {
        let delta_ns = if submit_returned_ns >= submit_deadline_ns {
            i64::try_from(submit_returned_ns - submit_deadline_ns).unwrap_or(i64::MAX)
        } else {
            -i64::try_from(submit_deadline_ns - submit_returned_ns).unwrap_or(i64::MAX)
        };
        self.observe_submit_delta_ns(delta_ns);
    }

    pub(crate) fn observe_missed_target(
        &mut self,
        submit_returned_ns: u64,
        target_presentation_ns: u64,
    ) {
        let observed_headroom_ns = target_presentation_ns.saturating_sub(submit_returned_ns);
        let required_margin_ns = observed_headroom_ns.saturating_add(MIN_SAFETY_MARGIN_NS);
        self.presentation_margin_floor_ns = self
            .presentation_margin_floor_ns
            .max(self.clamp_margin(required_margin_ns));
        self.presentation_floor_early_samples = 0;
        if required_margin_ns > self.safety_margin_ns {
            self.safety_margin_ns = self
                .safety_margin_ns
                .max(self.clamp_margin(required_margin_ns));
        }
    }

    pub(crate) fn observe_submission(
        &mut self,
        submit_wake_lateness_ns: u64,
        ioctl_duration_ns: u64,
    ) {
        push_bounded(&mut self.submit_wake_lateness_ns, submit_wake_lateness_ns);
        push_bounded(&mut self.ioctl_duration_ns, ioctl_duration_ns);
        let budget = nearest_rank(&self.submit_wake_lateness_ns, 95)
            .saturating_add(nearest_rank(&self.ioctl_duration_ns, 95))
            .saturating_add(MIN_SAFETY_MARGIN_NS);
        self.safety_margin_ns = self
            .clamp_margin(budget)
            .max(self.presentation_margin_floor_ns);
    }

    fn clamp_margin(&self, value: u64) -> u64 {
        let maximum = MAX_SAFETY_MARGIN_NS.min(self.refresh_interval_ns / 2);
        if maximum < MIN_SAFETY_MARGIN_NS {
            value.min(maximum)
        } else {
            value.clamp(MIN_SAFETY_MARGIN_NS, maximum)
        }
    }
}

fn push_bounded(samples: &mut VecDeque<u64>, sample: u64) {
    if samples.len() == SAMPLE_CAPACITY {
        samples.pop_front();
    }
    samples.push_back(sample);
}

fn nearest_rank(samples: &VecDeque<u64>, percentile: usize) -> u64 {
    let mut sorted: Vec<_> = samples.iter().copied().collect();
    if sorted.is_empty() {
        return 0;
    }
    sorted.sort_unstable();
    let rank = (percentile * sorted.len()).div_ceil(100).max(1);
    sorted[rank - 1]
}
