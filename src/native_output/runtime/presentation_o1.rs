use oblivion_one::native::adaptive_buffering::{AdaptiveBufferingController, RenderPrediction};
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
        opportunity: target.physical_claim().opportunity_id(),
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
    let Some(successor_ns) = predecessor
        .physical_claim()
        .presentation_time
        .get()
        .checked_add(refresh_ns)
    else {
        return 0;
    };
    estimate.overlap_required_ns(
        predecessor.presentation_time,
        MonotonicTimestampNs::new(successor_ns),
    )
}

pub(super) fn pipeline_service_estimate_for_prediction(
    prediction: &RenderPrediction,
) -> PipelineServiceEstimate {
    PipelineServiceEstimate::new(
        prediction.main_event_loop_wake_guard_ns,
        prediction.render_risk_ns,
        prediction.kms_dispatch_budget_ns,
        prediction.kms_apply_guard_ns,
    )
    .with_selected_end_to_end_service_ns(prediction.total_cost_ns)
}

#[cfg(test)]
mod tests {
    use super::{observe_current_o1_opportunity, overlap_required_for_current_opportunity};
    use oblivion_one::native::adaptive_buffering::{
        AdaptiveBufferingController, AdaptiveTripleBufferPolicy, PredictionEstimatorMode,
        RenderPrediction, TripleCapability,
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
            physical_claim: oblivion_one::native::presentation_deadline::PrimaryRefreshClaim {
                sequence: 41,
                presentation_time: MonotonicTimestampNs::new(10_000_000),
                clock_generation: 7,
            },
            selection_evidence: Default::default(),
        }
    }

    #[test]
    fn current_positive_opportunity_grants_capacity_before_same_admission() {
        let mut adaptive = AdaptiveBufferingController::new(AdaptiveTripleBufferPolicy::Auto);
        adaptive.apply_capability(TripleCapability::Capable);

        let decision =
            observe_current_o1_opportunity(&mut adaptive, Some(predecessor()), 1_200_000);

        assert_eq!(decision.desired_credit_before, 1);
        assert_eq!(decision.desired_credit_after, 2);
        assert!(decision.granted_extra_credit);
        assert!(
            decision.desired_credit_after > 1,
            "RenderAhead must be allowed in this decision"
        );

        let retry = observe_current_o1_opportunity(&mut adaptive, Some(predecessor()), 0);
        assert_eq!(retry.desired_credit_before, 2);
        assert_eq!(retry.desired_credit_after, 2);
        assert!(!retry.granted_extra_credit);
        assert_eq!(adaptive.extra_credit_grants(), 1);
    }

    fn selected_prediction() -> RenderPrediction {
        RenderPrediction {
            ewma_render_ns: 10_000_000,
            upper_render_deviation_ns: 0,
            p90_recent_render_ns: 1_000_000,
            render_risk_ns: 10_000_000,
            p95_wake_lateness_ns: 1_000_000,
            p95_atomic_submit_ns: 1_000_000,
            p95_worker_queue_residency_ns: 0,
            p95_worker_pre_submit_ns: 0,
            p95_worker_dispatch_ns: 0,
            p95_atomic_ioctl_ns: 1_000_000,
            main_event_loop_wake_guard_ns: 1_000_000,
            kms_dispatch_budget_ns: 1_000_000,
            kms_apply_guard_ns: 1_000_000,
            kms_total_lead_ns: 2_000_000,
            p95_target_slip_ns: 0,
            paired_service_p95_ns: 4_500_000,
            paired_service_samples: 20,
            estimator_mode: PredictionEstimatorMode::WarmPaired,
            independent_total_cost_ns: 13_000_000,
            warm_paired_total_cost_ns: 6_500_000,
            independent_p90_floor_ns: 3_000_000,
            worker_non_ioctl_lead_ns: 0,
            miss_recovery_remaining: 0,
            total_cost_ns: 6_500_000,
            idle_wake_guard: false,
        }
    }

    #[test]
    fn o1_overlap_uses_selected_prediction_total_from_integration_boundary() {
        let prediction = selected_prediction();
        let estimate = super::pipeline_service_estimate_for_prediction(&prediction);
        let overlap_required_ns = overlap_required_for_current_opportunity(
            Some(predecessor()),
            Duration::from_millis(6),
            estimate,
        );

        assert_eq!(estimate.end_to_end_service_ns(), 6_500_000);
        assert_eq!(
            estimate.latest_successor_render_start(MonotonicTimestampNs::new(16_000_000)),
            MonotonicTimestampNs::new(9_500_000)
        );
        assert_eq!(overlap_required_ns, 500_000);
    }
}
