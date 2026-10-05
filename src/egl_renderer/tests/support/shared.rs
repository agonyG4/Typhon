pub(in crate::egl_renderer::tests) fn assert_scene_work_preservation_trace_events(
    events: &[String],
    label: &str,
) {
    for phase in ["capture", "restore"] {
        let expected_phase = format!("phase={phase}");
        let phase_events = events
            .iter()
            .filter(|line| {
                line.contains("event=effect_scene_work_preservation")
                    && line
                        .split_whitespace()
                        .any(|field| field == expected_phase.as_str())
            })
            .collect::<Vec<_>>();
        assert!(
            !phase_events.is_empty(),
            "{label} has no scene-work preservation {phase} trace event"
        );
        assert!(
            phase_events.iter().any(|line| {
                line.split_whitespace()
                    .find_map(|field| field.strip_prefix("pixels="))
                    .and_then(|pixels| pixels.parse::<u64>().ok())
                    .is_some_and(|pixels| pixels > 0)
            }),
            "{label} has no non-zero scene-work preservation pixels in {phase}: {phase_events:?}"
        );
    }
}

pub(in crate::egl_renderer::tests) fn trace_event_index(
    events: &[String],
    terms: &[&str],
) -> usize {
    events
        .iter()
        .position(|line| terms.iter().all(|term| line.contains(term)))
        .unwrap_or_else(|| panic!("trace event not found: {terms:?}"))
}
