# Typhon KMS Timing v2

## Status

Approved implementation design for the KMS presentation-timing and commit-worker
throughput closure. The detailed task brief supplied with this work is the design
authority; this document records its repository-local decisions and boundaries.

## Problem

The current timing path has three correctness problems:

1. Reactive Double can preserve an already missed N+1 presentation opportunity.
2. `submit_not_before` is used as if it were a latest-safe-submit deadline.
3. A worker can publish a late result before pre-submit work and the atomic ioctl
   have completed.

It also has one ownership problem: pageflip presentation feedback is sent back to
the worker, where it adjusts the same scalar that also represents worker wake and
ioctl cost. That makes render prediction and miss attribution ambiguous.

## Design

### Physical mode timing and presentation authority

Add an exact-mode timing identity derived from the selected `drm_mode_modeinfo`.
The identity includes the timing fields that determine scanout geometry and clock,
not only the rounded refresh rate. For valid progressive modes, derive the
vertical-blank duration from vertical blank lines, horizontal total, and pixel
clock with checked arithmetic. Invalid, ambiguous, zero-clock, and overflowing
modes produce an unknown blanking duration and use a bounded conservative fallback.

The runtime owns `KmsPresentationTimingModel`. It stores mode identity, base mode
guard, adaptive apply guard, and bounded pageflip-derived history. It is reset or
requalified when the mode or relevant output/DRM generation changes. It learns only
from an exact pageflip outcome associated with the frame and transaction that
produced it.

The runtime remains the sole presentation authority. A pageflip can classify an
outcome as target hit, render/readiness miss, KMS dispatch miss, or KMS apply-guard
miss. A dispatch miss has priority when both dispatch and presentation failed; an
on-time submit followed by a target miss updates apply protection instead.

### Worker dispatch timing

Replace the worker-owned presentation margin with `KmsWorkerDispatchModel`. Its
bounded observations are actual wait-return wake lateness, post-wake preparation,
atomic ioctl duration, and end-to-end dispatch duration. Queue residency remains a
separate observation and is not treated as CPU work.

The worker records the actual timeline:

```text
job enqueued
planned worker wake
actual wait return
pre-submit preparation complete
atomic ioctl start
atomic ioctl return
pageflip presentation
```

No worker presentation-feedback queue or early `SubmitLate` event remains. A
worker dispatch miss exists only after the actual submit-return timestamp is known.

### Submit window

Compute a `KmsSubmitWindow` before worker admission:

```text
T = target presentation
A = runtime apply guard
D = worker dispatch budget

commit_complete_deadline = T - A
worker_wake_at = max(earliest_submit, commit_complete_deadline - D)
```

An earliest-submit lower bound never becomes a latest deadline. If the lower bound
is after the commit-completion deadline, the target is explicitly unreachable and
is replanned. Arriving after `worker_wake_at` but before the completion deadline
means lost headroom, not a proven miss; submit immediately.

The synchronous path uses the same target, mode, apply guard, and completion
deadline. Worker-Off removes only the worker wake component from prediction; it
does not make ioctl or hardware timing free.

### Reachable Reactive Double

Reactive target planning accepts the predicted total cost and selects the first
refresh opportunity whose commit window is reachable. Rendering still starts
immediately and the ready frame is submitted as soon as it is legal. A skipped
historical opportunity is accounting metadata, not a fabricated worker or
presentation miss.

Predictive Triple continues to use predicted pressure. Proven misses are reserved
for actual render/readiness, dispatch, and pageflip evidence.

### Prediction coupling

Render prediction exports distinct values for render risk, compositor wake guard,
KMS dispatch budget, KMS apply guard, and their total KMS lead. The total predicted
cost is:

```text
render risk + compositor wake guard + KMS dispatch budget + KMS apply guard
```

Existing submission observations are corrected in place; no duplicate KMS margin is
added.

### Scope boundaries

The change preserves pageflip-authoritative presented state, transaction and
framebuffer identity, scene history, Direct Scanout eligibility, cursor ownership,
async/tearing behavior, Commit Timing lower bounds, and Triple/partial-repaint
contracts. It does not add commit coalescing, a synchronous deadline bypass,
busy-spinning, NVIDIA-specific tuning, or pre-armed reservations unless measured
post-v2 evidence proves the required handoff-dominant regression.

SCHED_RR is a later, Linux-only best-effort experiment. It is retained only if an
A/B measurement shows a material worker wake-tail or throughput benefit.

## Evidence and validation

The initial release-binary control runs are stored outside the repository under
`/home/agony/Typhon-perf/artifacts`. The interleaved run is authoritative for the
pre-change comparison; the earlier sequential run is retained as provisional
context because its Auto Worker-Off cell showed substantial P-state variance.

The implementation must add deterministic tests for target reachability, earliest
submit versus completion deadline, actual dispatch attribution, pageflip apply
attribution, exact mode identity and safe blanking derivation, generation reset,
worker timestamp phases, async/tearing exclusion, Commit Timing lower bounds, and
Triple pressure preservation. It must pass the repository validation commands and
be remeasured with the required alternating 2x2 benchmark and desktop/vkcube smoke
checks before closure.
