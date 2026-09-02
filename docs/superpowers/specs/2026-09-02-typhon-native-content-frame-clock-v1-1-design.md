# Typhon Native Content Frame Clock v1.1

## Status

Approved design for propagating physical primary claims through the native output ownership path and making content-cadence attribution frame-local.

## Scope

The v1 content frame-clock work correctly separated advisory ReactiveDouble prediction from binding physical refresh reservations in the planner. This closure propagates that distinction through `OutputSwapchain`, READY ownership, worker-queued ownership, and physical pageflip revalidation. It also replaces global “latest callback” timing as the source of rendered-frame attribution with exact surface-local evidence frozen into the relevant compositor frame batch.

The accepted DMA-BUF release authority, O1 admission model, KMS worker ownership, native wake authority, cursor wake closure, input epochs, pointer behavior, Direct Scanout, SHM, protocol callback ownership, surface pacing, commit timing, and shutdown behavior remain unchanged.

## Problem statement

The planner can now represent a valid state in which an in-flight ReactiveDouble frame has diagnostic target sequence 4 but owns physical claim sequence 2, while an O1 frame owns reserved claim sequence 3. `OutputSwapchain` still compares raw `PresentationTarget` sequence/time metadata, so it rejects the valid successor because metadata 3 is not later than advisory metadata 4. The same raw comparison in visual planning can abandon and recreate an already-owned successor on every planning turn.

The native content telemetry also still copies global callback timing into a rendered frame. An admission on surface A followed by a commit on surface B can therefore be paired with A's later commit. This makes client reaction and content attribution unreliable on a real desktop containing multiple independently paced surfaces.

## Goals

* Attach one immutable `PrimaryRefreshClaim` to each future-primary target as it enters ownership.
* Keep advisory `PresentationTarget` metadata available for prediction and diagnostics without using it as physical ordering authority.
* Compare claims consistently in the planner, swapchain, READY validation, worker queue, and pageflip revalidation paths.
* Preserve binding target identity and clock-generation checks.
* Explicitly detect a predecessor that physically overtakes a READY or worker-queued successor; revalidate through existing invalidation/replan paths rather than mutating a target in place.
* Freeze callback admission and same-surface reaction evidence in the compositor frame batch and carry that evidence to the rendered output frame.
* Classify `TargetLimited` only from frozen binding selection evidence plus the resulting physical presentation, never from advisory selected/actual distances alone.
* Replace the literal target-mutation metric with bounded invariant counters.
* Preserve bounded telemetry and keep all metrics diagnostic-only.

## Non-goals

This change does not tune prediction constants, force repainting, change `wl_surface.frame` timing, redesign the scheduler, alter DMA-BUF or Direct Scanout ownership, change KMS worker timeout behavior, change O1 depth/admission, or introduce an ownership queue parallel to the existing swapchain and scheduler state.

## Physical claim model

`PrimaryRefreshClaim` is the canonical physical ordering value:

```rust
struct PrimaryRefreshClaim {
    clock_generation: u64,
    sequence: u64,
    presentation_time: MonotonicTimestampNs,
}
```

The existing `PrimaryRefreshFrontier` representation is reused where its semantics already match this value. A `PresentationTarget` carries the claim immutably. Reserved targets use a claim equal to their target reservation. ReactiveDouble targets retain their conservative diagnostic target but receive the single phase-aligned physical opportunity actually owned by that in-flight primary. O1 successors are allocated after the real claim frontier, not after the advisory timestamp.

Ordering rules are:

```text
same clock generation
later.claim.sequence > earlier.claim.sequence
later.claim.presentation_time > earlier.claim.presentation_time
```

Reserved target metadata must remain consistent with its claim. Advisory metadata may differ from its claim by design. Duplicate claims and mixed generations remain hard validation errors.

Claims follow the existing frame identity through rendering, READY, worker queue, kernel pending, and pageflip completion. A physical pageflip advances the primary phase only for a primary commit; cursor `PlaneDelta` remains outside the primary phase.

When the physical frontier overtakes a live READY/worker claim, the live target is not retargeted in place. The existing generation/identity validation path must explicitly reject, invalidate, abandon, or replan it, retaining exact batch and transaction ownership rules.

## Target selection evidence

Each selected target freezes bounded diagnostic evidence describing the earliest feasible phase-aligned opportunity known at selection time and whether the selection is binding. `TargetLimited` requires a binding selection that intentionally chose a later opportunity despite an earlier measured-feasible opportunity, followed by presentation at the selected later opportunity with no render, submit, or KMS miss. ReactiveDouble prediction overestimation remains separately observable and never becomes `TargetLimited` solely because its selected diagnostic distance exceeds the actual distance.

## Surface-local callback evidence

Live per-surface callback timing state records the last exact admission and the next callback-requesting visual commit on that same surface. The commit path supplies the surface identity. Callback ownership maps carry the corresponding timing evidence with currently pending callbacks. At frame-batch capture, evidence is selected only from callbacks in that exact batch and is frozen into the batch. Rendering reads batch-local evidence; it never copies the global latest callback event into a frame.

Legacy aggregate callback metrics may remain for compatibility and shutdown summaries, but they are not content-frame attribution authority. State is bounded by live surfaces and currently pending callbacks, with no unbounded historical ledger.

## Before/after ownership

```text
Before:
  physical primary phase
      -> Reactive advisory target metadata
      -> swapchain/planner raw metadata comparison
      -> valid O1 claim can be rejected or recreated
  global latest callback timestamps
      -> unrelated rendered frame attribution

After:
  physical primary phase
      -> immutable PrimaryRefreshClaim frontier
      -> planner, swapchain, READY, worker, and pageflip compare claims
  surface callback admission
      -> same-surface commit evidence
      -> exact CompositorFrameBatch evidence
      -> RenderedOutputFrame attribution
```

The accepted wake contract remains:

```text
bounded immediate work -> coalesced RuntimeContinuation eventfd
future temporal work   -> one absolute CLOCK_MONOTONIC timerfd deadline
external ownership     -> that owner's readiness
```

## Evidence gates

The implementation begins with RED tests at the real swapchain and planning boundaries:

1. advisory metadata sequence 4 with physical claim 2 plus reserved O1 claim 3 must pass `finish_render_owned()` and invariant validation;
2. repeated planning must retain an already scheduled claim-3 target;
3. a physical predecessor miss must explicitly reject/revalidate a colliding successor;
4. cross-surface callback ordering must compute reaction from the same surface;
5. advisory selected/actual distance must not classify `TargetLimited`, while a binding intentionally-late selection must.

If a required RED test does not fail against the current source, the corresponding policy change is not made; the result is recorded as an attribution finding instead.

## Performance and safety

No new thread, mutex, sleep, polling loop, forced repaint, blocking GPU wait, per-event logging, or unbounded history is introduced. Claims and selection evidence are fixed-size `Copy` values. Existing frame callback and batch collections remain bounded by their current live ownership. DMA-BUF identities, release ordering, worker timeout authority, and shutdown SafeDisable are untouched.

