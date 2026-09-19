# Replay Capture Attribution v2 Design

## Goal

Add bounded native observability that distinguishes Replay capture region and
command amplification, host CPU visibility planning, host GL/driver submission,
and GPU/driver dependency stalls. This is diagnostic instrumentation only; it
does not optimize Replay or change rendering behavior.

## Scope and invariants

The production Replay policy, capture ordering, scene replay policy, effect
damage, checkpoint semantics, pacing, scheduler, KMS, target selection,
shaders, and rendered output remain unchanged. The existing GPU timestamp query
pool and one begin/end timestamp pair per timed pass remain unchanged. Host
timing is read only while the existing effect GPU profiler is active.

The existing `replay_capture_commands` field remains the sum of candidate
command indices selected once per Replay capture pass. New fields are added for
the work that this legacy field does not describe.

## Architecture

### Fixed Replay execution detail

`ReplayCaptureExecutionDetail` is a small `Copy` value with no heap-owned
collections. It records:

- incoming materialization rectangle count;
- the actual post-`disjoint_bounded()` execution-region count and overflow bit;
- candidate-command count;
- actual planner visits/drawable decisions and their command-region pair count;
- total scene command count and actual full-scene scan iterations;
- actual commands considered/executed and draw calls from existing renderer
  frame-stat deltas;
- host wall time for selection, visibility planning, draw submission, and the
  whole Replay capture path.

The renderer’s existing draw loop remains the authority for full-scene scan,
considered, executed, and draw-call counters. No second traversal is added.
`plan_capture_visibility()` remains the authority for candidate planning
visits/drawable counts. `command_region_pairs` is the accumulated actual
planner visit count, not an arithmetic estimate.

### Timing-span ownership

The detail travels with the exact `PassTimingSpan` through the existing slot
generation and asynchronous polling lifecycle. It is attached to the
`TimingSpanMetadata` after the existing GPU END timestamp is queued. The graph
aggregate receives successful Replay totals by its existing scope ID. The
longest valid timed capture stores the exact detail in its max-pass record.
Dropped query slots, invalid timestamps, and disjoint-invalidated spans do not
produce max-pass detail.

### Host timing gate and phases

`Instant::now()` is called only when the active effect GPU profiler has begun a
graph scope. Replay capture uses bounded, saturating nanosecond conversion:

- `selection_cpu_ns`: layer/visual-group metadata construction and
  `indices_for_capture()`;
- `visibility_cpu_ns`: all `plan_capture_visibility()` calls across actual
  execution regions;
- `draw_submit_cpu_ns`: host time in the draw submission path after visibility
  planning, including GL/driver calls used to emit capture draws;
- `host_cpu_ns`: the whole Replay path, including materialization, target
  setup, clear/scissor/uniform setup, canonicalization, restoration, and the
  measured subphases.

Host timing does not claim pure CPU work when a GL call blocks; that blocking
time is intentionally part of the host phase that observed it.

### Bounded output

The existing `event=effect_gpu_timing` one-line-per-scope record is extended
with scalar graph totals and scalar `max_capture_*` fields. No per-pass logging,
trace requirement, arbitrary vectors, or extra GPU query is introduced.

## Testing strategy

Tests are written first and cover the following contracts:

1. Replay region detail reports incoming materialization rectangles, actual
   disjoint regions, and conservative bounding-region execution on overflow.
2. Candidate planner visits and command-region pairs come from actual planner
   results, while scene scan pairs and considered counts come from the existing
   full-scene draw loop; candidate count is intentionally distinct.
3. Executed commands and draw calls exclude occluded, outside, and unavailable
   commands.
4. Disabled GPU timing performs no new host clock reads and leaves CPU fields
   zero/unavailable.
5. Synthetic host timings survive asynchronous aggregation unchanged.
6. The exact slowest valid pass owns every max detail field, including when two
   same-frame scopes are active; framebuffer, invalid, and dropped spans emit
   no fabricated Replay detail.
7. Existing query pool capacities, timestamp count, capture aggregate
   identities, and scope ownership remain unchanged.

## Qualification interpretation

Comparable slow/fast Replay spans can now be classified by structure, host
phase, or residual GPU/driver behavior. The instrumentation does not prewarm,
synchronize, coalesce, cache, batch, or otherwise alter Replay execution.
