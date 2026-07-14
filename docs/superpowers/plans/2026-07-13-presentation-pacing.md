# Explicit Atomic Presentation Pacing Matrix

The explicit Atomic path keeps the planned presentation target immutable after
rendering. A frame may be userspace-ready before its target boundary, but its
submission is held until `submit_not_before`. The target sequence is never
rewritten when a timer fires or when a boundary is missed.

## State-transition test matrix

| State transition | Ownership/timing rule | Regression coverage |
| --- | --- | --- |
| target planned for N+1 → render | The normal target remains immediately submit-eligible after rendering | `ready_frame_for_next_refresh_submits_immediately` |
| target planned for N+2 → render | The frame becomes ready without a KMS commit | `ready_frame_for_n_plus_two_waits_until_the_previous_boundary` |
| ready before `submit_not_before` → timer wake | Keep the exact ready frame and target; do not repaint or replace it | `ready_frame_for_n_plus_two_waits_until_the_previous_boundary` |
| ready at `submit_not_before` → KMS pending | Submit exactly one ready frame and transfer it to the pending slot | `ready_frame_submits_immediately_after_expected_pageflip` |
| ready after target presentation time → late submit | Submit with the original target identity and record lateness | `expired_ready_target_uses_explicit_late_submit_transition` |
| pending → presented | Advance logical presentation sequence from monotonic timestamps, not a zero DRM sequence | `presented_sequence_is_derived_from_timestamp_intervals`, `zero_drm_sequence_uses_timestamp_logical_sequence` |
| rendering/submit diagnostics → render journal | Start the sample inside the explicit backend after diagnostics and protocol preparation | `debug_log_delay_before_backend_start_does_not_change_render_sample` |
| trace queue full → diagnostic drop | `try_send` drops without blocking the compositor render path and increments the drop counter | `verbose_trace_drops_when_full_without_blocking` |
| suspend/recovery/teardown → waiting ready frame | Scheduler readiness is retired with the output slot; no target mutation or second ready frame is created | session and scanout suspend/recovery tests |

## Deterministic cadence model

`PresentationCadenceMetrics::record_with_refresh` retains the raw DRM sequence
for diagnostics, but derives Typhon's logical sequence from monotonic pageflip
timestamps. At 165 Hz it classifies approximately 6.060 ms, 12.121 ms, and
18.181 ms as one, two, and three refresh intervals respectively. A zero DRM
sequence activates the timestamp fallback and is not treated as a fatal kernel
condition.

