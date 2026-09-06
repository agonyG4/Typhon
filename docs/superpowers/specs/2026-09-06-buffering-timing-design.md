# Buffering and worker timing study

## Findings and design

The adaptive render journal computes ten nearest-rank percentiles for a
fully populated prediction. Each currently allocates a vector and sorts it.
The KMS worker repeats this pattern for four dispatch percentiles. Both sample
histories are bounded at 120 entries. Pipeline snapshot validation also creates
vectors for at most four slot owners and three presentation targets.

Use a 120-element stack scratch array for percentile selection, copying both
segments of the sample deque and selecting only the required rank. Keep sample
eviction, percentile rounding, empty-history results, and timing guards intact.
Use fixed optional arrays for pipeline owners and targets, preserving validation
order and comparisons across absent entries.

This improves common timing work in reactive double and predictive triple
buffering without changing credit hysteresis, fence ownership, queue capacity,
or KMS submission policy. Retuning those policies needs hardware traces; a new
cached estimator would add invalidation state without being necessary here.

## Validation and constraints

Prove allocation removal with a thread-local counting allocator around actual
prediction and worker budget calls. Compare percentile results against a sorted
reference across empty, partially filled, wrapped, duplicate, and extreme-value
histories. Run existing buffering, swapchain, pipeline, and worker tests, then
the broader suite and Clippy. Build in this checkout's existing target directory.
Use rtk, no subagents, and commit only this work; preserve existing compositor
edits. Do not infer visible latency or FPS gains from allocation measurements.

## Results

- The allocation regressions failed before the change with nine allocations for
  the populated render fixture (its paired-service history is empty) and four
  for the worker budget. Both now report zero.
- Percentile selection matches a sorted reference through three ring capacities,
  including empty histories, duplicates, zero, and `u64::MAX`; journals retain
  their original sample order.
- Added pipeline regressions for target ordering and slot aliasing across absent
  queue positions. Existing double/triple admission behavior is preserved.
- `rtk cargo test --locked`: 3,503 passed, five ignored, 40 filtered out across
  31 suites. Targeted adaptive-buffering run: 36 passed. Changed Rust files pass
  rustfmt, and `git diff --check` passes.
- Strict all-target Clippy is blocked by the pre-existing unused
  `set_effect_scene_summary` method in `src/compositor/effects.rs`.
  The follow-up with `-D warnings -A dead-code` passes across all targets/features.
- Validation used the existing checkout's `target` directory. No live display
  benchmark or FPS/latency claim is made.
