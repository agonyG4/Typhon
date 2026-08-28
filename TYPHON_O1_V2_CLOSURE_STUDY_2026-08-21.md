# Typhon O1 v2 Closure Study

Date: 2026-08-21

## Scope

This study reviews the O1 v2 implementation at repository HEAD `2f3b346214c0af1530fa5064d7d046a9f23ee6f0` and defines the smallest architecture required to close the remaining runtime/simulator parity defects before any further performance campaign.

This is not a KMS Timing v2 redesign, not a KMS worker redesign, and not a new buffering algorithm. O1's ownership and demand model are retained. The goal is to make the implementation obey its own causal contract.

## Repository observations

The inspected O1 v2 commits are:

- `a078d42 refactor(native): separate O1 credit demand from ownership`
- `0854b8a fix(native): decouple O1 admission from pacing mode`
- `2f3b346 feat(native): record useful O1 credit outcomes`

The implementation successfully fixed two earlier architectural problems:

1. `desired_credit` is no longer the same thing as physical `owned_future_primary_depth`.
2. KMS dispatch/apply misses no longer directly grant future render credit. Only predicted overlap and proven render-readiness misses can demand credit 2.

The runtime also moved away from deriving a frame's pacing mode directly from the old global Double/Triple state. `pacing_mode_for_target()` derives timing behavior from the frame's immutable `PresentationTarget` reason.

These are correct foundations and should be preserved.

---

# Finding 1 — P0: same-opportunity demand is observed too late

The critical ordering in `src/native_output/runtime/presentation_cycle.rs` is currently:

```text
compute overlap_required
    ↓
read desired_credit into render_ahead_allowed
    ↓
prepare / plan target using that old capacity
    ↓
compute render-target availability using old capacity
    ↓
build pipeline snapshot using old capacity
    ↓
observe_overlap_for_target()
    ↓
desired_credit may change 1 → 2
    ↓
scheduler still receives old render_ahead_allowed
```

The code contains the equivalent sequence:

```rust
let render_ahead_allowed = adaptive_buffering.desired_credit() > 1;
...
plan_visual_target_for_budget(render_ahead_allowed, ...);
...
build_output_pipeline_snapshot_with_presented(
    ...,
    adaptive_buffering.desired_credit(),
    ...,
);
...
adaptive_buffering.observe_overlap_for_target(pending_target, overlap_required_ns);
...
ExplicitAtomicSchedulerContext {
    ...,
    render_ahead_allowed,
    ...
}
```

This violates O1's central causal rule.

If opportunity `N+1` proves that overlap is required, the capacity granted by that proof must be available to the scheduling decision for `N+1`. Granting the credit only after the decision inputs have been frozen means the system can detect exactly why it must render before the predecessor's pageflip, then still choose `WaitForPageFlip` because the old credit was captured earlier.

The simulator does the opposite: it observes the opportunity first, then derives `render_ahead` and admission from the updated demand. The simulator and production runtime therefore model different state machines at the most important transition, credit 1 → 2.

## Required invariant

For each distinct presentation opportunity:

```text
measure demand
    ↓
observe demand exactly once
    ↓
resolve desired capacity
    ↓
resolve admission/render-ahead permission
    ↓
plan and schedule the frame
```

Pressure detection and use of the resulting capacity belong to one atomic scheduling decision.

This does not mean repeatedly training the controller during event-loop retries. Existing `PresentationOpportunityId` deduplication should remain.

---

# Finding 2 — P1: credit-usefulness telemetry is not frame-local

Current pageflip accounting calls roughly:

```rust
frame_pacing.note_o1_credit2_outcome(
    frame.target.reason,
    adaptive_buffering.last_overlap_required_ns(),
    proven_miss.is_none(),
);
```

`last_overlap_required_ns()` is mutable controller-global state. With future depth greater than one, the controller can observe a newer opportunity before an older frame pageflips.

Example:

```text
Frame A admission:
    overlap = +800 us
    actually consumes second future credit

Frame B planning occurs before A presents:
    overlap = 0
    controller.last_overlap = 0

Frame A pageflips:
    pageflip reads controller.last_overlap
    A can be classified as unnecessary
```

The metric is then answering the wrong question.

A credit outcome must be evaluated from the exact observation that authorized that exact physical frame.

## Better ownership boundary

`RenderedOutputFrame` is the most natural carrier. It already travels through:

```text
render
→ ready
→ worker/synchronous submission
→ kernel-submitted ownership
→ pageflip
```

and already carries immutable target, transaction, slot, framebuffer, generation, and timing information.

Add a small immutable O1 admission record, conceptually:

```rust
struct O1AdmissionObservation {
    opportunity: PresentationOpportunityId,
    desired_credit: u8,
    owned_future_depth_before: u8,
    overlap_required_ns: u64,
    used_extra_credit: bool,
}
```

Exact naming may follow project conventions.

The important properties are:

- it is captured when the render admission decision is made;
- it belongs to one physical `RenderedOutputFrame`;
- it is never reconstructed from the controller at pageflip time;
- `used_extra_credit` is true only when this frame actually consumed the extra future capacity, not merely because credit 2 existed or the target reason was `PredictedPressure`.

Pageflip usefulness metrics should only classify frames that actually consumed extra credit.

A normal frame rendered while desired credit happens to be 2 is not automatically a credit-2 outcome.

---

# Finding 3 — P1: simulator still permits physically impossible lifecycle behavior

O1 v2's simulator is substantially better than v1 because it is event-driven and models demand, owned depth, worker state, render events, submit events, and pageflips.

However, its physical lanes are still approximated too loosely.

## 3.1 Pageflips are scheduled before submission exists

Every frame receives a `PageFlip` event at its target timestamp during simulator initialization.

At the event:

```rust
if !frame.submitted {
    continue;
}
```

If a frame misses that target and is not submitted by then, its pageflip event disappears permanently. A real frame does not simply vanish: if it is later submitted successfully, it should normally be presented at a later valid refresh opportunity unless invalidated/cancelled.

This prevents the simulator from accurately modeling:

```text
miss target N
→ present at N+1
→ release kernel primary
→ drain future depth
→ allow next primary
```

which is directly relevant to the historical multi-refresh cadence problem.

## 3.2 Kernel primary ownership is a boolean, not an identity

The simulator uses:

```text
kernel_submitted: bool
worker_queued: bool
prepared: bool
```

but does not make the frame identity owning each lane authoritative.

A second `SubmitReturned` can conceptually set `kernel_submitted = true` while a previous primary is already awaiting pageflip. Production Typhon's pipeline explicitly permits at most one kernel-submitted primary.

## 3.3 Render/prepared ownership is not fully serialized

`prepared` is tracked but `RenderStarted` does not use it as a hard single-render-lane ownership gate. Multiple scheduled render-start events can therefore model concurrency that the explicit output swapchain does not permit.

Production has typed, bounded lanes:

- at most one rendering/ready prepared primary;
- at most one worker-queued primary;
- at most one kernel-submitted primary;
- total future-primary depth at most 2.

The simulator should model the same bounds with frame identities, not loose booleans.

---

# Recommended architecture — O1 v2 Closure

Do not redesign O1. Close the causal gaps with three focused boundaries.

## A. Opportunity decision boundary

Extract the O1-specific part of the giant presentation cycle into a focused runtime module, for example:

```text
src/native_output/runtime/presentation_o1.rs
```

The boundary should ensure one ordering:

```text
apply O1 capability
compute overlap for current predecessor/opportunity
observe that opportunity exactly once
capture desired credit after observation
plan target/admission using that current desired credit
```

`presentation_cycle.rs` is already approximately 1495 lines, so this task should reduce or hold its size, not grow it beyond the source-layout limit.

A small result object can make ordering explicit, conceptually:

```rust
struct O1CycleDemandDecision {
    overlap_required_ns: u64,
    desired_credit_before: u8,
    desired_credit_after: u8,
    grant: bool,
    revoke: bool,
}
```

Do not create a new global scheduler mode.

Do not mutate an already armed presentation target.

## B. Frame-local admission evidence

Capture immutable evidence at actual render admission.

The most direct owner is `RenderedOutputFrame` or an equivalent exact physical-frame object.

Conceptually:

```rust
struct O1AdmissionObservation {
    opportunity: PresentationOpportunityId,
    desired_credit: u8,
    owned_future_depth_before: u8,
    overlap_required_ns: u64,
    used_extra_credit: bool,
}
```

The pageflip path consumes this exact evidence.

Metrics become causal:

```text
used extra credit + target hit + overlap > 0
    = useful

used extra credit + target hit + overlap == 0
    = unnecessary

used extra credit + target miss
    = ineffective

never used extra credit
    = not a credit-2 outcome
```

## C. Simulator lane model

Use frame IDs for the physical lanes:

```text
rendering: Option<FrameId>
next_primary: Option<FrameId>
kernel_submitted: Option<FrameId>
```

If keeping worker-specific state improves fidelity:

```text
worker_queued: Option<FrameId>
```

but do not model more capacity than production.

Pageflip events must be scheduled from accepted submission, not pre-created blindly for every target.

For fixed VSync, compute the actual first presentation opportunity consistent with submission/apply timing. A simple deterministic model can use:

```text
earliest_physical_apply = submit_returned + apply_delay
actual_presentation = first refresh opportunity >= earliest_physical_apply
```

while retaining the planned target for hit/miss classification.

After a pageflip:

- clear the exact kernel owner;
- decrement future depth for that exact frame;
- promote/submit the waiting next primary according to Worker-On/Off transport;
- continue the event chain.

Every accepted submitted frame must either:

1. eventually pageflip; or
2. be explicitly invalidated/cancelled by a modeled lifecycle transition.

It must never disappear because its pre-scheduled target event already passed.

---

# Why this is better than copying KWin or Hyprland

## KWin

The supplied KWin scheduler contains two valuable principles:

1. when triple-depth becomes necessary, it reacts immediately;
2. after a frame is already scheduled, a more pessimistic estimate may move render start earlier but must not push the target vblank farther away unnecessarily.

KWin also documents that switching Double → Triple itself can drop a frame, and therefore uses global hysteresis around `pageflipsInAdvance`.

O1 should retain immutable opportunities but avoid needing a global buffering-mode transition. Correct same-opportunity admission is essential to achieving that.

## Hyprland

The supplied Hyprland scheduler is simpler and reactive. When explicit sync proves the current cadence is late, `onSyncFired()` sets `m_pendingThird` and begins the third render immediately in that same event path. It does not discover the need for overlap and defer the actual overlap permission until a later pageflip.

O1 is more predictive and more strongly typed, but should preserve the same causal property:

> the evidence that extra overlap is required can authorize that overlap immediately.

The solution is not to copy `pendingThird`; it is to make O1's opportunity decision atomic.

---

# What must not change

Preserve:

- KMS Timing v2 semantics;
- Worker-On/Off throughput equivalence in credit-1;
- pageflip-authoritative presented state;
- Opportunity target immutability;
- `desired_credit` vs physical depth separation;
- no direct KMS-miss → credit grant path;
- max future-primary depth = 2;
- one kernel-submitted primary;
- one worker-queued primary;
- one prepared/rendering primary;
- Direct Scanout rules;
- Commit Timing semantics;
- async/tearing rules;
- partial repaint and slot lineage;
- frame callback timing.

Do not introduce:

- another global Double/Triple mode;
- KMS worker changes;
- SCHED_RR experiments;
- pre-armed worker reservations;
- queue growth;
- commit merging;
- vendor-specific tuning;
- busy waits;
- GPU clock hacks.

---

# Validation strategy

This closure should be dominated by deterministic tests.

Do not run a long vkmark campaign.

Required proof should include:

## Same-cycle admission

A test must demonstrate:

```text
desired credit before = 1
current opportunity overlap > 0
observe current opportunity
current decision sees desired credit = 2
scheduler can choose RenderAhead before predecessor pageflip
```

The opposite old ordering must fail the test.

## Retry deduplication

Multiple event-loop evaluations of the same opportunity must not create multiple demand observations.

## Drain semantics

```text
desired = 1
owned = 2
```

must suppress refill but preserve existing frames until natural pageflip drainage.

## Frame-local telemetry

Observe frame A with positive overlap, then observe frame B with zero overlap before A presents. A's pageflip must still use A's positive admission observation.

A frame that did not consume extra credit must not increment useful/unnecessary/ineffective credit-2 counters.

## Simulator physical invariants

- at most one rendering/prepared primary;
- at most one worker-queued primary;
- at most one kernel-submitted primary;
- total future depth <= 2;
- no target mutation;
- missed target reschedules physical pageflip to a later refresh;
- no submitted frame silently disappears;
- Worker On and Worker Off preserve the same opportunity ordering when dispatch service is equivalent;
- credit grant in the simulator is usable in the same opportunity, matching runtime ordering.

---

# Benchmark policy

No benchmark campaign is required for this task.

Do not run the historical 2x2 or 5-run-per-cell campaign.

Do not schedule poweroff/reboot after completion.

A future performance qualification can be a separate task after deterministic parity is closed.

If a live smoke is already available and cheap, a short non-benchmark native Wayland smoke may be run, but it is not a success criterion.

---

# Closure criterion

This task succeeds when O1's production runtime and deterministic simulator implement the same causal state machine for:

```text
opportunity observation
→ desired credit update
→ current admission decision
→ physical frame ownership
→ submission
→ actual pageflip
→ exact frame-local outcome accounting
```

Performance is intentionally not re-qualified here.

The expected final verdict is therefore about architecture/correctness, not vkmark score.
