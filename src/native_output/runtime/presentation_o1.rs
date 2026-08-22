use oblivion_one::native::adaptive_buffering::AdaptiveBufferingController;
use oblivion_one::native::buffering::{O1AdmissionObservation, PipelineServiceEstimate};
use oblivion_one::native::presentation_deadline::{MonotonicTimestampNs, PresentationTarget};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct O1CycleDemandDecision {
    pub(super) overlap_required_ns: u64,
    pub(super) desired_credit_before: u8,
    pub(super) desired_credit_after: u8,
    pub(super) granted_extra_credit: bool,
    pub(super) revoked_extra_credit: bool,
}

pub(super) fn observe_current_o1_opportunity(
    adaptive_buffering: &mut AdaptiveBufferingController,
    predecessor: Option<PresentationTarget>,
    overlap_required_ns: u64,
) -> O1CycleDemandDecision {
    let desired_credit_before = adaptive_buffering.desired_credit();
    adaptive_buffering.observe_overlap_for_target(predecessor, overlap_required_ns);
    let desired_credit_after = adaptive_buffering.desired_credit();
    O1CycleDemandDecision {
        overlap_required_ns,
        desired_credit_before,
        desired_credit_after,
        granted_extra_credit: desired_credit_before == 1 && desired_credit_after == 2,
        revoked_extra_credit: desired_credit_before == 2 && desired_credit_after == 1,
    }
}

pub(super) fn admission_observation_for_frame(
    target: PresentationTarget,
    desired_credit: u8,
    owned_future_depth_before: u8,
    overlap_required_ns: u64,
    render_ahead: bool,
) -> O1AdmissionObservation {
    O1AdmissionObservation {
        opportunity: target.opportunity().id(),
        desired_credit,
        owned_future_depth_before,
        overlap_required_ns,
        used_extra_credit: render_ahead && desired_credit == 2 && owned_future_depth_before == 1,
    }
}

pub(super) fn overlap_required_for_current_opportunity(
    predecessor: Option<PresentationTarget>,
    refresh_interval: Duration,
    estimate: PipelineServiceEstimate,
) -> u64 {
    let Some(predecessor) = predecessor else {
        return 0;
    };
    let Ok(refresh_ns) = u64::try_from(refresh_interval.as_nanos()) else {
        return 0;
    };
    let Some(successor_ns) = predecessor.presentation_time.get().checked_add(refresh_ns) else {
        return 0;
    };
    estimate.overlap_required_ns(
        predecessor.presentation_time,
        MonotonicTimestampNs::new(successor_ns),
    )
}

#[cfg(test)]
mod tests {
    use super::observe_current_o1_opportunity;
    use oblivion_one::native::adaptive_buffering::{
        AdaptiveBufferingController, AdaptiveTripleBufferPolicy, TripleCapability,
    };
    use oblivion_one::native::presentation_deadline::{
        MonotonicTimestampNs, PresentationTarget, PresentationTargetReason,
    };
    use std::time::Duration;

    fn predecessor() -> PresentationTarget {
        PresentationTarget {
            sequence: 41,
            presentation_time: MonotonicTimestampNs::new(10_000_000),
            submit_not_before: MonotonicTimestampNs::new(9_000_000),
            render_start_deadline: MonotonicTimestampNs::new(8_000_000),
            refresh_interval: Duration::from_nanos(8_333_333),
            reason: PresentationTargetReason::Normal,
            clock_generation: 7,
            estimated: false,
            predicted_unreachable: false,
        }
    }

    #[test]
    fn current_positive_opportunity_grants_capacity_before_same_admission() {
        let mut adaptive = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        adaptive.apply_capability(TripleCapability::Capable);

        let decision = observe_current_o1_opportunity(&mut adaptive, Some(predecessor()), 1_200_000);

        assert_eq!(decision.desired_credit_before, 1);
        assert_eq!(decision.desired_credit_after, 2);
        assert!(decision.granted_extra_credit);
        assert!(decision.desired_credit_after > 1, "RenderAhead must be allowed in this decision");

        let retry = observe_current_o1_opportunity(&mut adaptive, Some(predecessor()), 0);
        assert_eq!(retry.desired_credit_before, 2);
        assert_eq!(retry.desired_credit_after, 2);
        assert!(!retry.granted_extra_credit);
        assert_eq!(adaptive.extra_credit_grants(), 1);
    }
}
