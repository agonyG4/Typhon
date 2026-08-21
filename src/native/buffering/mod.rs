//! Opportunity-locked buffering primitives.
//!
//! This module intentionally starts with pure scheduling contracts.  Runtime
//! ownership remains in the native output pipeline; these values carry only
//! timing identity and bounded policy decisions.

#![allow(dead_code)]

use crate::native::presentation_deadline::MonotonicTimestampNs;
use std::time::Duration;

mod credit;
mod simulator;

pub use credit::{ElasticFuturePrimaryCredit, O1CreditDemandController, O1CreditDemandReason};
pub use simulator::{
    SimulatedO1Config, SimulatedO1Event, SimulatedO1EventKind, SimulatedO1EventModel,
    SimulatedO1Result, SimulatedO1State, simulate_o1, simulate_o1_with_render_services,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PresentationOpportunityId {
    clock_generation: u64,
    sequence: u64,
}

impl PresentationOpportunityId {
    pub const fn new(clock_generation: u64, sequence: u64) -> Self {
        Self {
            clock_generation,
            sequence,
        }
    }

    pub const fn clock_generation(self) -> u64 {
        self.clock_generation
    }

    pub const fn sequence(self) -> u64 {
        self.sequence
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationDomain {
    FixedVsync,
    VrrWindow,
    AsyncImmediate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationOpportunity {
    id: PresentationOpportunityId,
    target_time: MonotonicTimestampNs,
    refresh_interval: Duration,
    domain: PresentationDomain,
}

impl PresentationOpportunity {
    pub const fn fixed_vsync(
        id: PresentationOpportunityId,
        target_time: MonotonicTimestampNs,
        refresh_interval: Duration,
    ) -> Self {
        Self {
            id,
            target_time,
            refresh_interval,
            domain: PresentationDomain::FixedVsync,
        }
    }

    pub const fn id(self) -> PresentationOpportunityId {
        self.id
    }

    pub const fn target_time(self) -> MonotonicTimestampNs {
        self.target_time
    }

    pub const fn refresh_interval(self) -> Duration {
        self.refresh_interval
    }

    pub const fn domain(self) -> PresentationDomain {
        self.domain
    }

    pub fn successor(self) -> Option<Self> {
        Some(Self {
            id: PresentationOpportunityId::new(
                self.id.clock_generation,
                self.id.sequence.checked_add(1)?,
            ),
            target_time: MonotonicTimestampNs::new(
                self.target_time
                    .get()
                    .checked_add(duration_ns(self.refresh_interval))?,
            ),
            ..self
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpportunityLeaseReason {
    VisualWork,
    RenderAhead,
    CommitTiming,
    Recovery,
    ForcedValidation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpportunityLeaseTermination {
    UnreachableBeforeRender,
    ConstraintAdvanced,
    OutputGenerationChanged,
    PresentationDomainChanged,
    RenderFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpportunityLease {
    opportunity: PresentationOpportunity,
    reason: OpportunityLeaseReason,
    terminal: Option<OpportunityLeaseTermination>,
}

impl OpportunityLease {
    pub const fn arm(opportunity: PresentationOpportunity, reason: OpportunityLeaseReason) -> Self {
        Self {
            opportunity,
            reason,
            terminal: None,
        }
    }

    pub const fn opportunity(self) -> PresentationOpportunity {
        self.opportunity
    }

    pub const fn reason(self) -> OpportunityLeaseReason {
        self.reason
    }

    pub const fn terminal_reason(self) -> Option<OpportunityLeaseTermination> {
        self.terminal
    }

    pub const fn is_terminal(self) -> bool {
        self.terminal.is_some()
    }

    pub fn abandon(&mut self, reason: OpportunityLeaseTermination) {
        self.terminal = Some(reason);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationOpportunityFrontierError {
    DuplicateLiveClaim(PresentationOpportunityId),
    MixedClockGeneration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PresentationOpportunityFrontier {
    latest: Option<PresentationOpportunityId>,
}

impl PresentationOpportunityFrontier {
    pub fn from_claims<I>(claims: I) -> Result<Self, PresentationOpportunityFrontierError>
    where
        I: IntoIterator<Item = PresentationOpportunityId>,
    {
        let mut frontier = Self::default();
        for claim in claims {
            if let Some(latest) = frontier.latest {
                if latest.clock_generation != claim.clock_generation {
                    return Err(PresentationOpportunityFrontierError::MixedClockGeneration);
                }
                if latest == claim {
                    return Err(PresentationOpportunityFrontierError::DuplicateLiveClaim(
                        claim,
                    ));
                }
            }
            frontier.latest = Some(frontier.latest.map_or(claim, |latest| latest.max(claim)));
        }
        Ok(frontier)
    }

    pub const fn latest(self) -> Option<PresentationOpportunityId> {
        self.latest
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PipelineServiceEstimate {
    pub main_wake_guard_ns: u64,
    pub render_risk_ns: u64,
    pub kms_dispatch_budget_ns: u64,
    pub kms_apply_guard_ns: u64,
}

impl PipelineServiceEstimate {
    pub const fn new(
        main_wake_guard_ns: u64,
        render_risk_ns: u64,
        kms_dispatch_budget_ns: u64,
        kms_apply_guard_ns: u64,
    ) -> Self {
        Self {
            main_wake_guard_ns,
            render_risk_ns,
            kms_dispatch_budget_ns,
            kms_apply_guard_ns,
        }
    }

    pub const fn render_ready_service_ns(self) -> u64 {
        self.main_wake_guard_ns.saturating_add(self.render_risk_ns)
    }

    pub const fn kms_lead_ns(self) -> u64 {
        self.kms_dispatch_budget_ns
            .saturating_add(self.kms_apply_guard_ns)
    }

    pub const fn end_to_end_service_ns(self) -> u64 {
        self.render_ready_service_ns()
            .saturating_add(self.kms_lead_ns())
    }

    pub fn latest_successor_render_start(
        self,
        successor_presentation: MonotonicTimestampNs,
    ) -> MonotonicTimestampNs {
        MonotonicTimestampNs::new(
            successor_presentation
                .get()
                .saturating_sub(self.kms_apply_guard_ns)
                .saturating_sub(self.kms_dispatch_budget_ns)
                .saturating_sub(self.render_ready_service_ns()),
        )
    }

    pub fn overlap_required_ns(
        self,
        predecessor_presentation: MonotonicTimestampNs,
        successor_presentation: MonotonicTimestampNs,
    ) -> u64 {
        predecessor_presentation.get().saturating_sub(
            self.latest_successor_render_start(successor_presentation)
                .get(),
        )
    }
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{
        ElasticFuturePrimaryCredit, OpportunityLease, OpportunityLeaseReason,
        PipelineServiceEstimate, PresentationDomain, PresentationOpportunity,
        PresentationOpportunityFrontier, PresentationOpportunityId, SimulatedO1Config, simulate_o1,
        simulate_o1_with_render_services,
    };
    use crate::native::presentation_deadline::MonotonicTimestampNs;
    use std::time::Duration;

    #[test]
    fn armed_lease_keeps_immutable_opportunity_identity() {
        let opportunity = PresentationOpportunity::fixed_vsync(
            PresentationOpportunityId::new(4, 12),
            MonotonicTimestampNs::new(120_000_000),
            Duration::from_millis(10),
        );
        let lease = OpportunityLease::arm(opportunity, OpportunityLeaseReason::VisualWork);

        assert_eq!(lease.opportunity(), opportunity);
        assert_eq!(
            lease.opportunity().id(),
            PresentationOpportunityId::new(4, 12)
        );
        assert_eq!(
            lease.opportunity().target_time(),
            MonotonicTimestampNs::new(120_000_000)
        );
        assert_eq!(lease.opportunity().domain(), PresentationDomain::FixedVsync);
        assert!(!lease.is_terminal());
    }

    #[test]
    fn frontier_rejects_duplicate_live_opportunity_claims() {
        let frontier = PresentationOpportunityFrontier::from_claims([
            PresentationOpportunityId::new(1, 9),
            PresentationOpportunityId::new(1, 10),
        ])
        .unwrap();
        assert_eq!(
            frontier.latest(),
            Some(PresentationOpportunityId::new(1, 10))
        );

        assert!(
            PresentationOpportunityFrontier::from_claims([
                PresentationOpportunityId::new(1, 10),
                PresentationOpportunityId::new(1, 10),
            ])
            .is_err()
        );
    }

    #[test]
    fn overlap_requires_second_credit_only_when_successor_must_overlap() {
        let estimate = PipelineServiceEstimate::new(1_000, 3_000, 500, 500);
        assert_eq!(
            estimate.overlap_required_ns(
                MonotonicTimestampNs::new(100_000),
                MonotonicTimestampNs::new(104_000),
            ),
            1_000
        );
        assert_eq!(
            estimate.overlap_required_ns(
                MonotonicTimestampNs::new(100_000),
                MonotonicTimestampNs::new(108_000),
            ),
            0
        );
    }

    #[test]
    fn credit_change_is_capacity_only() {
        let mut credit = ElasticFuturePrimaryCredit::new();
        let lease = OpportunityLease::arm(
            PresentationOpportunity::fixed_vsync(
                PresentationOpportunityId::new(1, 2),
                MonotonicTimestampNs::new(20_000),
                Duration::from_micros(10),
            ),
            OpportunityLeaseReason::VisualWork,
        );
        credit.observe_overlap(1);
        assert_eq!(credit.effective(), 2);
        assert_eq!(
            lease.opportunity().id(),
            PresentationOpportunityId::new(1, 2)
        );
    }

    #[test]
    fn extra_credit_revokes_after_three_negative_observations() {
        let mut credit = ElasticFuturePrimaryCredit::new();
        credit.observe_overlap(1);
        assert_eq!(credit.effective(), 2);

        for _ in 0..3 {
            credit.observe_overlap(0);
        }
        assert_eq!(credit.effective(), 1);
        assert_eq!(credit.revokes(), 1);
    }

    fn config(render_service_ns: u64) -> SimulatedO1Config {
        SimulatedO1Config {
            refresh_interval_ns: 6_060_606,
            render_service_ns,
            dispatch_service_ns: 300_000,
            apply_guard_ns: 500_000,
            apply_delay_ns: 500_000,
            frames: 120,
            worker_enabled: true,
        }
    }

    #[test]
    fn simulator_low_load_matches_one_credit_and_worker_transport_is_irrelevant() {
        let worker = simulate_o1(config(500_000));
        let mut synchronous_config = config(500_000);
        synchronous_config.worker_enabled = false;
        let synchronous = simulate_o1(synchronous_config);

        assert_eq!(worker.target_hits, 120);
        assert_eq!(worker.credit_two_observations, 0);
        assert_eq!(worker.max_future_primary_depth, 1);
        assert_eq!(worker, synchronous);
    }

    #[test]
    fn simulator_grants_overlap_credit_for_sustained_pressure_without_exceeding_two() {
        let result = simulate_o1(config(7_500_000));

        assert_eq!(result.render_readiness_misses, 0);
        assert_eq!(result.dispatch_misses, 0);
        assert_eq!(result.apply_guard_misses, 0);
        assert!(result.credit_two_observations > 0);
        assert_eq!(result.max_future_primary_depth, 2);
        assert_eq!(result.target_mutations, 0);
    }

    #[test]
    fn simulator_separates_apply_failure_from_dispatch_failure() {
        let mut config = config(500_000);
        config.apply_delay_ns = 1_500_000;
        let result = simulate_o1(config);

        assert_eq!(result.dispatch_misses, 0);
        assert_eq!(result.apply_guard_misses, 120);
    }

    #[test]
    fn simulator_transient_pressure_recovers_to_one_credit() {
        let config = config(3_000_000);
        let mut services = vec![3_000_000; config.frames as usize];
        services[0] = 12_000_000;
        let result = simulate_o1_with_render_services(config, &services);

        assert!(result.credit_two_observations > 0);
        assert!(result.credit_one_observations > 0);
        assert_eq!(result.target_mutations, 0);
    }

    #[test]
    fn simulator_handles_alternating_near_refresh_service_without_retargeting() {
        let config = config(4_500_000);
        let services: Vec<_> = (0..config.frames as usize)
            .map(|index| if index % 2 == 0 { 4_500_000 } else { 6_500_000 })
            .collect();
        let result = simulate_o1_with_render_services(config, &services);

        assert_eq!(result.render_readiness_misses, 0);
        assert_eq!(result.dispatch_misses, 0);
        assert!(result.credit_two_observations > 0);
        assert_eq!(result.target_mutations, 0);
    }

    #[test]
    fn simulator_preserves_low_pressure_equivalence_at_sixty_hz() {
        let mut worker_config = config(3_000_000);
        worker_config.refresh_interval_ns = 16_666_667;
        worker_config.frames = 60;
        let worker = simulate_o1(worker_config);

        let mut synchronous_config = worker_config;
        synchronous_config.worker_enabled = false;
        let synchronous = simulate_o1(synchronous_config);

        assert_eq!(worker, synchronous);
        assert_eq!(worker.target_hits, 60);
        assert_eq!(worker.credit_two_observations, 0);
    }

    #[test]
    fn pre_render_generation_change_is_terminal_and_not_a_retarget() {
        let opportunity = PresentationOpportunity::fixed_vsync(
            PresentationOpportunityId::new(7, 20),
            MonotonicTimestampNs::new(200_000),
            Duration::from_micros(10),
        );
        let mut lease = OpportunityLease::arm(opportunity, OpportunityLeaseReason::VisualWork);

        lease.abandon(super::OpportunityLeaseTermination::OutputGenerationChanged);

        assert!(lease.is_terminal());
        assert_eq!(lease.opportunity(), opportunity);
        assert_eq!(lease.opportunity().id().sequence(), 20);
    }
}
