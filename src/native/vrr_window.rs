//! Pure timing planner for a phase-free VRR presentation window.
//!
//! A physical pageflip anchor supplies only the earliest physically valid
//! presentation time. After that lower bound, eligibility is continuous; this
//! planner does not model or advance a fixed-refresh phase.

use crate::native::presentation_deadline::MonotonicTimestampNs;
use crate::native::presentation_timing::PresentationTiming;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VrrWindowPlanInput {
    /// Timestamp of the latest matching, confirmed physical pageflip.
    pub anchor: MonotonicTimestampNs,
    /// Current monotonic time; must be at or after `anchor`.
    pub now: MonotonicTimestampNs,
    /// Optional client eligibility lower bound, never a refresh target.
    pub not_before: Option<MonotonicTimestampNs>,
    /// Physical interval implied by the display's maximum refresh rate.
    pub shortest_interval: Duration,
}

/// Stateless planner for the timing bounds of one VRR window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VrrWindowPlanner;

impl VrrWindowPlanner {
    pub fn plan(input: VrrWindowPlanInput) -> Option<PresentationTiming> {
        if input.now < input.anchor || input.shortest_interval.is_zero() {
            return None;
        }

        // Timestamp values are nanoseconds in u64. Reject intervals that do
        // not fit instead of inheriting the fixed planner's saturating helper.
        let shortest_interval_ns = u64::try_from(input.shortest_interval.as_nanos()).ok()?;
        let earliest_present =
            MonotonicTimestampNs::new(input.anchor.get().checked_add(shortest_interval_ns)?);
        let logical_eligible = input
            .not_before
            .map_or(input.now, |not_before| input.now.max(not_before));
        let preferred_present = earliest_present.max(logical_eligible);

        Some(PresentationTiming::VrrWindow {
            anchor: input.anchor,
            earliest_present,
            preferred_present,
            service_deadline: None,
            shortest_interval: input.shortest_interval,
            longest_interval: None,
        })
    }
}
