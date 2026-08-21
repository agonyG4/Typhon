use super::PresentationOpportunityId;

const NEGATIVE_SLACK_OBSERVATIONS_TO_REVOKE: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum O1CreditDemandReason {
    PredictedOverlap,
    ProvenRenderReadinessMiss,
    ForcedValidation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct O1CreditDemandController {
    desired_credit: u8,
    ceiling: u8,
    negative_slack_streak: u8,
    demand_reason: Option<O1CreditDemandReason>,
    last_observed_opportunity: Option<PresentationOpportunityId>,
    grants: u64,
    revokes: u64,
}

impl Default for O1CreditDemandController {
    fn default() -> Self {
        Self::new()
    }
}

impl O1CreditDemandController {
    pub const fn new() -> Self {
        Self::with_ceiling(2)
    }

    pub const fn with_ceiling(ceiling: u8) -> Self {
        let ceiling = clamp_credit_ceiling(ceiling);
        Self {
            desired_credit: 1,
            ceiling,
            negative_slack_streak: 0,
            demand_reason: None,
            last_observed_opportunity: None,
            grants: 0,
            revokes: 0,
        }
    }

    pub const fn desired_credit(self) -> u8 {
        self.desired_credit
    }

    pub const fn effective(self) -> u8 {
        self.desired_credit()
    }

    pub const fn ceiling(self) -> u8 {
        self.ceiling
    }

    pub const fn grants(self) -> u64 {
        self.grants
    }

    pub const fn revokes(self) -> u64 {
        self.revokes
    }

    pub const fn demand_reason(self) -> Option<O1CreditDemandReason> {
        self.demand_reason
    }

    pub const fn last_observed_opportunity(self) -> Option<PresentationOpportunityId> {
        self.last_observed_opportunity
    }

    pub fn observe_opportunity(
        &mut self,
        opportunity: PresentationOpportunityId,
        overlap_required_ns: u64,
    ) {
        if self.last_observed_opportunity == Some(opportunity) {
            return;
        }
        self.last_observed_opportunity = Some(opportunity);
        self.observe_slack(
            overlap_required_ns,
            Some(O1CreditDemandReason::PredictedOverlap),
        );
    }

    pub fn observe_overlap(&mut self, overlap_required_ns: u64) {
        self.observe_slack(
            overlap_required_ns,
            Some(O1CreditDemandReason::PredictedOverlap),
        );
    }

    pub fn observe_render_readiness_miss(&mut self) {
        self.negative_slack_streak = 0;
        self.grant(O1CreditDemandReason::ProvenRenderReadinessMiss);
    }

    pub fn force(&mut self) {
        self.negative_slack_streak = 0;
        self.grant(O1CreditDemandReason::ForcedValidation);
    }

    pub fn set_ceiling(&mut self, ceiling: u8) {
        self.ceiling = clamp_credit_ceiling(ceiling);
        if self.desired_credit > self.ceiling {
            self.desired_credit = self.ceiling;
            self.revokes = self.revokes.saturating_add(1);
            self.demand_reason = None;
        }
    }

    fn observe_slack(
        &mut self,
        overlap_required_ns: u64,
        positive_reason: Option<O1CreditDemandReason>,
    ) {
        if overlap_required_ns > 0 {
            self.negative_slack_streak = 0;
            if let Some(reason) = positive_reason {
                self.grant(reason);
            }
            return;
        }

        self.negative_slack_streak = self.negative_slack_streak.saturating_add(1);
        if self.desired_credit > 1
            && self.negative_slack_streak >= NEGATIVE_SLACK_OBSERVATIONS_TO_REVOKE
        {
            self.desired_credit = 1;
            self.revokes = self.revokes.saturating_add(1);
            self.demand_reason = None;
        }
    }

    fn grant(&mut self, reason: O1CreditDemandReason) {
        if self.desired_credit < self.ceiling {
            self.desired_credit = self.ceiling;
            self.grants = self.grants.saturating_add(1);
        }
        if self.desired_credit == 2 {
            self.demand_reason = Some(reason);
        }
    }
}

pub type ElasticFuturePrimaryCredit = O1CreditDemandController;

const fn clamp_credit_ceiling(ceiling: u8) -> u8 {
    if ceiling < 1 {
        1
    } else if ceiling > 2 {
        2
    } else {
        ceiling
    }
}

#[cfg(test)]
mod tests {
    use super::{O1CreditDemandController, O1CreditDemandReason};
    use crate::native::buffering::PresentationOpportunityId;

    #[test]
    fn unique_positive_opportunity_grants_once() {
        let id = PresentationOpportunityId::new(3, 7);
        let mut controller = O1CreditDemandController::new();

        controller.observe_opportunity(id, 1);
        controller.observe_opportunity(id, 1);

        assert_eq!(controller.desired_credit(), 2);
        assert_eq!(controller.grants(), 1);
        assert_eq!(
            controller.demand_reason(),
            Some(O1CreditDemandReason::PredictedOverlap)
        );
    }

    #[test]
    fn negative_observations_revoke_without_ownership_input() {
        let mut controller = O1CreditDemandController::new();
        controller.observe_overlap(1);
        for _ in 0..3 {
            controller.observe_overlap(0);
        }

        assert_eq!(controller.desired_credit(), 1);
        assert_eq!(controller.revokes(), 1);
    }

    #[test]
    fn readiness_miss_grants_but_kms_has_no_policy_api() {
        let mut controller = O1CreditDemandController::new();
        controller.observe_render_readiness_miss();

        assert_eq!(controller.desired_credit(), 2);
        assert_eq!(
            controller.demand_reason(),
            Some(O1CreditDemandReason::ProvenRenderReadinessMiss)
        );
    }
}
