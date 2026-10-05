use oblivion_one::compositor::OutputPresentationMode;
use oblivion_one::native::buffering::{
    OpportunityLease, OpportunityLeaseReason, OpportunityLeaseTermination, PresentationOpportunity,
    PresentationOpportunityId,
};
use oblivion_one::native::presentation_deadline::{
    MonotonicTimestampNs, PresentationDeadlinePlanner, PresentationTarget,
    PresentationTargetReason, PrimaryRefreshClaim, TargetSelectionEvidence,
};
use oblivion_one::native::presentation_timing::{PresentationDomain, PresentationTiming};
use std::time::Duration;

#[test]
fn presentation_timing_domains_ids_and_successors_are_typed() {
    let id = PresentationOpportunityId::new(7, 23);
    let target_time = MonotonicTimestampNs::new(1_234_567);
    let interval = Duration::from_nanos(8_333_333);
    let fixed = PresentationOpportunity::fixed_vsync(id, target_time, interval);

    assert_eq!(fixed.id(), id);
    assert_eq!(
        fixed.timing(),
        PresentationTiming::FixedVsync {
            target_time,
            refresh_interval: interval,
        }
    );
    assert_eq!(fixed.domain(), PresentationDomain::FixedVsync);

    let fixed_successor = fixed.fixed_vsync_successor().unwrap();
    assert_eq!(fixed_successor.id(), PresentationOpportunityId::new(7, 24));
    assert_eq!(
        fixed_successor.timing(),
        PresentationTiming::FixedVsync {
            target_time: MonotonicTimestampNs::new(9_567_900),
            refresh_interval: interval,
        }
    );

    let variable = PresentationOpportunity::new(
        id,
        PresentationTiming::VrrWindow {
            anchor: MonotonicTimestampNs::new(1_000_000_000),
            earliest_present: MonotonicTimestampNs::new(1_002_000_000),
            preferred_present: MonotonicTimestampNs::new(1_004_000_000),
            service_deadline: Some(MonotonicTimestampNs::new(1_020_000_000)),
            shortest_interval: Duration::from_millis(5),
            longest_interval: None,
        },
    );
    assert_eq!(fixed.id(), variable.id());
    assert_eq!(variable.domain(), PresentationDomain::VrrWindow);
    assert_eq!(variable.fixed_vsync_successor(), None);

    let immediate = PresentationTiming::AsyncImmediate {
        not_before: MonotonicTimestampNs::new(4_200_000),
    };
    assert_eq!(immediate.domain(), PresentationDomain::AsyncImmediate);
    assert_eq!(
        PresentationOpportunity::new(id, immediate).fixed_vsync_successor(),
        None
    );
}

#[test]
fn presentation_timing_targets_keep_the_fixed_grid_at_four_refresh_rates() {
    const PRESENTED_AT_NS: u64 = 1_000_000_000;
    const NOW_NS: u64 = PRESENTED_AT_NS + 100_000;
    const PREDICTED_COST: Duration = Duration::from_nanos(200_000);

    for interval_ns in [16_666_667, 8_333_333, 6_944_444, 6_060_606] {
        let interval = Duration::from_nanos(interval_ns);
        let first_target_ns = PRESENTED_AT_NS + interval_ns;
        let second_target_ns = PRESENTED_AT_NS + 2 * interval_ns;

        let mut normal_planner = PresentationDeadlinePlanner::new(interval);
        normal_planner.note_presented(MonotonicTimestampNs::new(PRESENTED_AT_NS));
        let normal = normal_planner
            .plan_normal(MonotonicTimestampNs::new(NOW_NS), PREDICTED_COST)
            .unwrap();
        assert_eq!(
            (normal.sequence(), normal.presentation_time.get()),
            (2, first_target_ns)
        );

        let mut commit_planner = PresentationDeadlinePlanner::new(interval);
        commit_planner.note_presented(MonotonicTimestampNs::new(PRESENTED_AT_NS));
        let commit = commit_planner
            .plan_not_before(
                MonotonicTimestampNs::new(NOW_NS),
                MonotonicTimestampNs::new(first_target_ns + 1),
                PREDICTED_COST,
            )
            .unwrap();
        assert_eq!(
            (commit.sequence(), commit.presentation_time.get()),
            (3, second_target_ns)
        );
        assert_eq!(commit.reason, PresentationTargetReason::CommitTiming);

        let mut reactive_planner = PresentationDeadlinePlanner::new(interval);
        reactive_planner.note_presented(MonotonicTimestampNs::new(PRESENTED_AT_NS));
        let reactive = reactive_planner
            .reactive_target(MonotonicTimestampNs::new(NOW_NS), PREDICTED_COST)
            .unwrap();
        assert_eq!(
            (reactive.sequence(), reactive.presentation_time.get()),
            (2, first_target_ns)
        );
        assert_eq!(reactive.reason, PresentationTargetReason::ReactiveDouble);

        let mut successor_planner = PresentationDeadlinePlanner::new(interval);
        successor_planner.note_presented(MonotonicTimestampNs::new(PRESENTED_AT_NS));
        let pending = successor_planner
            .plan_normal(MonotonicTimestampNs::new(NOW_NS), PREDICTED_COST)
            .unwrap();
        let successor = successor_planner
            .plan_successor_after(
                pending,
                MonotonicTimestampNs::new(NOW_NS),
                PREDICTED_COST,
                PresentationTargetReason::Normal,
            )
            .unwrap();
        assert_eq!(
            (successor.sequence(), successor.presentation_time.get()),
            (3, second_target_ns)
        );
    }
}

#[test]
fn presentation_timing_target_conversions_and_leases_keep_timing_immutable() {
    let interval = Duration::from_nanos(6_944_444);
    let target = PresentationTarget {
        sequence: 9,
        presentation_time: MonotonicTimestampNs::new(1_062_500_000),
        submit_not_before: MonotonicTimestampNs::new(1_050_000_000),
        render_start_deadline: MonotonicTimestampNs::new(1_040_000_000),
        refresh_interval: interval,
        reason: PresentationTargetReason::Normal,
        clock_generation: 4,
        estimated: false,
        predicted_unreachable: false,
        physical_claim: PrimaryRefreshClaim {
            sequence: 8,
            presentation_time: MonotonicTimestampNs::new(1_055_555_556),
            clock_generation: 4,
        },
        selection_evidence: TargetSelectionEvidence::default(),
    };

    let logical = target.opportunity();
    assert_eq!(logical.id(), PresentationOpportunityId::new(4, 9));
    assert_eq!(
        logical.timing(),
        PresentationTiming::FixedVsync {
            target_time: target.presentation_time,
            refresh_interval: interval,
        }
    );

    let physical = target.physical_opportunity();
    assert_eq!(physical.id(), PresentationOpportunityId::new(4, 8));
    assert_eq!(
        physical.timing(),
        PresentationTiming::FixedVsync {
            target_time: target.physical_claim.presentation_time,
            refresh_interval: interval,
        }
    );

    let original = physical;
    let mut lease = OpportunityLease::arm(physical, OpportunityLeaseReason::VisualWork);
    lease.abandon(OpportunityLeaseTermination::PresentationDomainChanged);
    assert_eq!(lease.opportunity().id(), original.id());
    assert_eq!(lease.opportunity().timing(), original.timing());
}

#[test]
fn presentation_timing_preserves_the_four_output_mode_domains() {
    for (mode, domain) in [
        (
            OutputPresentationMode::Vsync,
            PresentationDomain::FixedVsync,
        ),
        (
            OutputPresentationMode::AdaptiveSync,
            PresentationDomain::VrrWindow,
        ),
        (
            OutputPresentationMode::Async,
            PresentationDomain::AsyncImmediate,
        ),
        (
            OutputPresentationMode::AdaptiveAsync,
            PresentationDomain::VrrWindow,
        ),
    ] {
        assert_eq!(mode.presentation_domain(), domain);
    }
}
