use crate::native_output::perf::NativePerfField;

#[derive(Debug, Clone, Copy)]
pub(crate) struct O1BranchTailInput {
    pub(crate) frame_id: u64,
    pub(crate) transaction_id: u64,
    pub(crate) target_sequence: u64,
    pub(crate) target_presentation_ns: u64,
    pub(crate) target_reason: &'static str,
    pub(crate) target_binding: bool,
    pub(crate) target_predicted_unreachable: bool,
    pub(crate) actual_sequence: u64,
    pub(crate) presented_at_ns: u64,
    pub(crate) refresh_ns: u64,
    pub(crate) composite_started_ns: u64,
    pub(crate) rendered_observed_ns: Option<u64>,
    pub(crate) worker_handoff_begin_ns: Option<u64>,
    pub(crate) fence_signal_ns: Option<u64>,
    pub(crate) fence_timestamp_quality: &'static str,
    pub(crate) raw_render_ready_ns: u64,
    pub(crate) raw_queue_ready_ns: u64,
    pub(crate) selected_render_ready_risk_ns: u64,
    pub(crate) selected_queue_ready_risk_ns: u64,
    pub(crate) estimator_mode: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct O1BranchTailEvent {
    input: O1BranchTailInput,
}

impl O1BranchTailEvent {
    pub(crate) fn new(input: O1BranchTailInput) -> Option<Self> {
        (input.raw_render_ready_ns > input.refresh_ns
            || input.raw_queue_ready_ns > input.refresh_ns)
            .then_some(Self { input })
    }

    pub(crate) fn fields(self) -> Vec<NativePerfField> {
        let input = self.input;
        let composite_to_rendered_observed_ns = input
            .rendered_observed_ns
            .and_then(|rendered| rendered.checked_sub(input.composite_started_ns));
        let post_render_handoff_ns = input
            .rendered_observed_ns
            .zip(input.worker_handoff_begin_ns)
            .and_then(|(rendered, handoff)| handoff.checked_sub(rendered));
        let fence_after_rendered_observed_ns = input
            .rendered_observed_ns
            .zip(input.fence_signal_ns)
            .and_then(|(rendered, fence)| fence.checked_sub(rendered))
            .filter(|elapsed| *elapsed > 0);
        let rendered_observed_after_fence_ns = input
            .rendered_observed_ns
            .zip(input.fence_signal_ns)
            .and_then(|(rendered, fence)| rendered.checked_sub(fence))
            .filter(|elapsed| *elapsed > 0);
        let queue_before_fence = input
            .worker_handoff_begin_ns
            .zip(input.fence_signal_ns)
            .is_some_and(|(handoff, fence)| handoff < fence);
        let fence_before_queue = input
            .worker_handoff_begin_ns
            .zip(input.fence_signal_ns)
            .is_some_and(|(handoff, fence)| fence < handoff);
        let equal_queue_and_fence = input
            .worker_handoff_begin_ns
            .zip(input.fence_signal_ns)
            .is_some_and(|(handoff, fence)| fence == handoff);

        vec![
            NativePerfField::u64("frame_id", input.frame_id),
            NativePerfField::u64("transaction_id", input.transaction_id),
            NativePerfField::u64("target_sequence", input.target_sequence),
            NativePerfField::u64("target_presentation_ns", input.target_presentation_ns),
            NativePerfField::str("target_reason", input.target_reason),
            NativePerfField::bool("target_binding", input.target_binding),
            NativePerfField::bool(
                "target_predicted_unreachable",
                input.target_predicted_unreachable,
            ),
            NativePerfField::u64("actual_sequence", input.actual_sequence),
            NativePerfField::u64("presented_at_ns", input.presented_at_ns),
            NativePerfField::u64("refresh_ns", input.refresh_ns),
            NativePerfField::u64("composite_started_ns", input.composite_started_ns),
            optional_ns_field("rendered_observed_ns", input.rendered_observed_ns),
            optional_ns_field("worker_handoff_begin_ns", input.worker_handoff_begin_ns),
            optional_ns_field("fence_signal_ns", input.fence_signal_ns),
            NativePerfField::str("fence_timestamp_quality", input.fence_timestamp_quality),
            NativePerfField::u64("raw_render_ready_ns", input.raw_render_ready_ns),
            NativePerfField::u64("raw_queue_ready_ns", input.raw_queue_ready_ns),
            optional_ns_field(
                "composite_to_rendered_observed_ns",
                composite_to_rendered_observed_ns,
            ),
            optional_ns_field("post_render_handoff_ns", post_render_handoff_ns),
            optional_ns_field(
                "fence_after_rendered_observed_ns",
                fence_after_rendered_observed_ns,
            ),
            optional_ns_field(
                "rendered_observed_after_fence_ns",
                rendered_observed_after_fence_ns,
            ),
            NativePerfField::bool("queue_before_fence", queue_before_fence),
            NativePerfField::bool("fence_before_queue", fence_before_queue),
            NativePerfField::bool("equal_queue_and_fence", equal_queue_and_fence),
            NativePerfField::u64(
                "selected_render_ready_risk_ns",
                input.selected_render_ready_risk_ns,
            ),
            NativePerfField::u64(
                "selected_queue_ready_risk_ns",
                input.selected_queue_ready_risk_ns,
            ),
            NativePerfField::str("estimator_mode", input.estimator_mode),
        ]
    }
}

fn optional_ns_field(key: &'static str, value: Option<u64>) -> NativePerfField {
    value.map_or_else(
        || NativePerfField::str(key, "unavailable"),
        |value| NativePerfField::u64(key, value),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(raw_render_ready_ns: u64, raw_queue_ready_ns: u64) -> O1BranchTailInput {
        O1BranchTailInput {
            frame_id: 7,
            transaction_id: 9,
            target_sequence: 11,
            target_presentation_ns: 1_000_000,
            target_reason: "reactive_double",
            target_binding: true,
            target_predicted_unreachable: false,
            actual_sequence: 12,
            presented_at_ns: 1_005_000,
            refresh_ns: 6_060_606,
            composite_started_ns: 100,
            rendered_observed_ns: Some(3_100),
            worker_handoff_begin_ns: Some(4_100),
            fence_signal_ns: Some(5_100),
            fence_timestamp_quality: "exact_sync_file",
            raw_render_ready_ns,
            raw_queue_ready_ns,
            selected_render_ready_risk_ns: 8_000_000,
            selected_queue_ready_risk_ns: 7_000_000,
            estimator_mode: "warm_parallel",
        }
    }

    fn field<'a>(fields: &'a [crate::native_output::perf::NativePerfField], key: &str) -> &'a str {
        &fields.iter().find(|field| field.key == key).unwrap().value
    }

    #[test]
    fn exact_outlier_event_has_frame_correlation_and_branch_decomposition() {
        let event = O1BranchTailEvent::new(sample(7_000_000, 4_000_000)).unwrap();
        let fields = event.fields();

        assert_eq!(field(&fields, "frame_id"), "7");
        assert_eq!(field(&fields, "transaction_id"), "9");
        assert_eq!(field(&fields, "raw_render_ready_ns"), "7000000");
        assert_eq!(field(&fields, "raw_queue_ready_ns"), "4000000");
        assert_eq!(field(&fields, "composite_to_rendered_observed_ns"), "3000");
        assert_eq!(field(&fields, "post_render_handoff_ns"), "1000");
        assert_eq!(field(&fields, "fence_after_rendered_observed_ns"), "2000");
        assert_eq!(
            field(&fields, "rendered_observed_after_fence_ns"),
            "unavailable"
        );
        assert_eq!(field(&fields, "queue_before_fence"), "true");
        assert_eq!(field(&fields, "selected_render_ready_risk_ns"), "8000000");
        assert_eq!(field(&fields, "selected_queue_ready_risk_ns"), "7000000");
        assert_eq!(field(&fields, "estimator_mode"), "warm_parallel");
    }

    #[test]
    fn normal_sub_refresh_frame_has_no_tail_event() {
        assert!(O1BranchTailEvent::new(sample(6_000_000, 5_000_000)).is_none());
    }
}
