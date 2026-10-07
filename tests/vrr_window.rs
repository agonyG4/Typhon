use oblivion_one::native::presentation_deadline::{
    MonotonicTimestampNs, PresentationDeadlinePlanner,
};
use oblivion_one::native::presentation_timing::{PresentationDomain, PresentationTiming};
use oblivion_one::native::vrr_window::{VrrWindowPlanInput, VrrWindowPlanner};
use std::time::Duration;

const ANCHOR_NS: u64 = 1_000_000_000;
const REFRESH_165_HZ_NS: u64 = 6_060_606;

fn input(now: u64, not_before: Option<u64>) -> VrrWindowPlanInput {
    VrrWindowPlanInput {
        anchor: MonotonicTimestampNs::new(ANCHOR_NS),
        now: MonotonicTimestampNs::new(now),
        not_before: not_before.map(MonotonicTimestampNs::new),
        shortest_interval: Duration::from_nanos(REFRESH_165_HZ_NS),
    }
}

fn vrr_fields(
    timing: PresentationTiming,
) -> (
    MonotonicTimestampNs,
    MonotonicTimestampNs,
    Option<MonotonicTimestampNs>,
    Duration,
    Option<Duration>,
) {
    match timing {
        PresentationTiming::VrrWindow {
            earliest_present,
            preferred_present,
            service_deadline,
            shortest_interval,
            longest_interval,
            ..
        } => (
            earliest_present,
            preferred_present,
            service_deadline,
            shortest_interval,
            longest_interval,
        ),
        other => panic!("expected a VRR window, got {other:?}"),
    }
}

#[test]
fn vrr_window_at_165_hz_preserves_non_grid_now_and_unknown_minimum() {
    let timing = VrrWindowPlanner::plan(input(1_009_400_000, None)).unwrap();

    assert_eq!(timing.domain(), PresentationDomain::VrrWindow);
    assert_eq!(
        vrr_fields(timing),
        (
            MonotonicTimestampNs::new(1_006_060_606),
            MonotonicTimestampNs::new(1_009_400_000),
            None,
            Duration::from_nanos(6_060_606),
            None,
        )
    );
    let (earliest, preferred, _, _, _) = vrr_fields(timing);
    assert!(MonotonicTimestampNs::new(ANCHOR_NS) <= earliest);
    assert!(earliest <= preferred);
    assert_ne!(
        MonotonicTimestampNs::new(1_009_400_000),
        MonotonicTimestampNs::new(1_012_121_212),
    );
}

#[test]
fn vrr_window_before_physical_earliest_uses_earliest_as_preferred() {
    let timing = VrrWindowPlanner::plan(input(1_002_000_000, None)).unwrap();

    assert_eq!(
        vrr_fields(timing).1,
        MonotonicTimestampNs::new(1_006_060_606)
    );
}

#[test]
fn vrr_window_preserves_non_grid_commit_timing_lower_bound() {
    let not_before = 1_010_333_333;
    let timing = VrrWindowPlanner::plan(input(1_002_000_000, Some(not_before))).unwrap();

    assert_eq!(vrr_fields(timing).1, MonotonicTimestampNs::new(not_before));
}

#[test]
fn vrr_window_current_time_dominates_stale_not_before() {
    let timing = VrrWindowPlanner::plan(input(1_009_400_000, Some(1_003_000_000))).unwrap();

    assert_eq!(
        vrr_fields(timing).1,
        MonotonicTimestampNs::new(1_009_400_000)
    );
}

#[test]
fn vrr_window_after_long_idle_uses_now_without_refresh_cycle_search() {
    let now = ANCHOR_NS + 100_000_000;
    let timing = VrrWindowPlanner::plan(input(now, None)).unwrap();

    assert_eq!(vrr_fields(timing).1, MonotonicTimestampNs::new(now));
}

#[test]
fn vrr_window_rejects_zero_shortest_interval() {
    let mut input = input(ANCHOR_NS, None);
    input.shortest_interval = Duration::ZERO;

    assert_eq!(VrrWindowPlanner::plan(input), None);
}

#[test]
fn vrr_window_rejects_interval_outside_timestamp_domain() {
    let mut input = input(ANCHOR_NS, None);
    input.shortest_interval = Duration::from_secs(u64::MAX);

    assert_eq!(VrrWindowPlanner::plan(input), None);
}

#[test]
fn vrr_window_rejects_timestamp_overflow() {
    let anchor = MonotonicTimestampNs::new(u64::MAX - REFRESH_165_HZ_NS + 1);
    let input = VrrWindowPlanInput {
        anchor,
        now: anchor,
        not_before: None,
        shortest_interval: Duration::from_nanos(REFRESH_165_HZ_NS),
    };

    assert_eq!(VrrWindowPlanner::plan(input), None);
}

#[test]
fn vrr_window_rejects_now_before_physical_anchor() {
    let mut input = input(ANCHOR_NS, None);
    input.now = MonotonicTimestampNs::new(ANCHOR_NS - 1);

    assert_eq!(VrrWindowPlanner::plan(input), None);
}

#[test]
fn fixed_grid_and_vrr_window_choose_distinct_165_hz_times() {
    let now = MonotonicTimestampNs::new(1_009_400_000);
    let mut fixed_planner =
        PresentationDeadlinePlanner::new(Duration::from_nanos(REFRESH_165_HZ_NS));
    fixed_planner.note_presented(MonotonicTimestampNs::new(ANCHOR_NS));
    let fixed = fixed_planner.plan_normal(now, Duration::ZERO).unwrap();
    let vrr = VrrWindowPlanner::plan(input(now.get(), None)).unwrap();

    assert_eq!(
        fixed.presentation_time,
        MonotonicTimestampNs::new(1_012_121_212)
    );
    assert_eq!(vrr_fields(vrr).1, MonotonicTimestampNs::new(1_009_400_000));
}
