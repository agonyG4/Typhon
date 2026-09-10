# Typhon C2a Post-Fusion Peak-Live Accounting

## Goal

Make `peak_live_intermediates` exact after local-stage fusion while preserving
stable `GraphPassId` values for graph relationships, effect execution, and
resource lifetime comparisons.

## Root cause

Fusion removes a pass from `GraphBuilder::passes` without renumbering the
surviving passes. For example, removing pass ID 3 leaves an execution sequence
such as `[1, 2, 4, 5]`. The current peak-live helper converts IDs with `id - 1`,
so a stable ID no longer names its compact execution position. The existing
test reference repeats that invalid conversion and can therefore agree with
the broken production implementation.

## Chosen design

Change the production helper to receive the final ordered pass slice and the
texture plans. After all fusion and lifetime rewrites, build one bounded vector
that maps each surviving `GraphPassId` to its compact position in
`passes`. Traverse intermediate textures once, resolve each complete lifetime
through that map, and insert an inclusive interval into the existing delta
array. Sweep the surviving positions once to obtain the maximum live count.

The lookup is bounded by `MAX_GRAPH_PASSES`; stable IDs remain unchanged. A
missing endpoint is inconsistent post-fusion metadata: in debug builds it
triggers `debug_assert!`, and the production-safe telemetry fallback counts the
interval over the full surviving pass range so malformed metadata cannot be
silently undercounted. Existing incomplete-lifetime handling remains explicit.

The test reference will accept the actual pass sequence and resolve each
endpoint with a simple `passes.iter().position(...)` lookup. It is deliberately
slower and semantically independent from the optimized production map.

## Alternatives considered

1. **Recommended: bounded ID-to-position vector.** It is one pass-map build,
   one texture interval traversal, and one sweep: `O(P + T)`, with no changes
   to graph IDs or execution semantics.
2. **Search the pass sequence for every texture endpoint.** This is easy to
   read but regresses production accounting to `O(P × T)`.
3. **Renumber the graph after fusion.** This restores compact IDs but requires
   auditing every stable-ID relationship and unnecessarily expands the
   correctness surface. It is rejected.

## Regression coverage

Tests will cover a real compiled local-stage fusion with a mid-graph ID gap,
multiple fused instances and multiple gaps, lifetimes beginning after a gap,
IDs numerically greater than `passes.len()`, inclusive endpoints after gaps,
compact no-fusion fixtures, malformed-ID fallback behavior as applicable, and
the existing deterministic large synthetic workload. The real compiled tests
will assert both fusion and agreement with the semantic reference.

## Non-goals

This change does not renumber `GraphPassId`, alter fusion eligibility or
texture rewiring, change effect execution or texture release semantics, modify
snapshot/presentation behavior, add persistent caches, or implement C2b.
`peak_live_intermediates` remains a telemetry/measurement statistic in the
current source.
