# Buffering and worker timing study

## Findings and design

The adaptive render journal computes eleven nearest-rank percentiles for a
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
