# Adaptive KMS pacing holes

## Context

The native Atomic KMS path has two independent, evidence-backed pacing holes:

1. The worker's P95 dispatch estimate plus its fixed guard can undershoot a rare
   submit tail, allowing `submit_returned_at` to cross the hard
   commit-complete deadline even when the rendered payload was ready in time.
2. A non-predictive frame can be rendered early enough to reach an intermediate
   refresh after it was conservatively reserved for a later refresh. Its old
   target's submit window then deliberately waits until just after that
   intermediate opportunity.

The existing physical-target and worker-ownership models are authoritative and
must remain so. In particular, queue residency is observable pipeline waiting,
Reactive Double targets are advisory, and Predictive O1 has exact attempt-to-
physical-frame identity.

## Design

### A: adaptive dispatch-tail guard

`KmsWorkerDispatchModel` retains its bounded P95 wake-lateness and full
post-wake dispatch-duration estimator. Its exported budget becomes:

```text
p95_wake_lateness
+ p95_full_dispatch_duration
+ DISPATCH_GUARD_NS
+ adaptive_tail_guard_ns
```

The adaptive guard starts at zero. After every real worker submission, the
worker computes the saturating deadline overrun from the already available
submit-return and commit-complete timestamps. A positive overrun increases the
guard by `overrun + 50_000 ns`, capped at `1_000_000 ns`, before the next
submission budget is exported. This feedback is attached only to dispatch
deadline misses; it never changes the KMS apply guard or render prediction
components.

Thirty-two consecutive on-time submissions form one clean decay window. Each
window subtracts `50_000 ns` from the adaptive guard, saturating at zero. A
miss resets the clean streak. The model exposes the current guard, latest
overrun, increase/decay counts, and cap-hit count through the existing worker
timing snapshot. The existing submission budget continues to flow through the
worker event and `AdaptiveRenderJournal`, so the prediction layer gets one
authoritative dispatch budget and does not add queue residency.

### B: ready-time opportunity pull-in

`PresentationDeadlinePlanner` gains a ready-service estimate containing only
the KMS dispatch budget and KMS apply guard. It provides a ready-time candidate
operation that:

- starts strictly after the physical presented frontier;
- finds the earliest refresh reachable after `now + ready_remaining_service`;
- requires that refresh to be earlier than the current reserved target;
- creates a fresh exact `PresentationTarget` and physical claim; and
- does not mutate the old target.

The runtime evaluates this candidate after render readiness and before any
worker pacing reservation, queue ownership, or kernel submission. It first
rejects advisory Reactive Double and exact Predictive O1 identities, then
validates generation, frame readiness, target identity, physical ordering, and
ownership against the swapchain and transaction ledger. A successful operation
explicitly releases the old ready reservation, replaces the ledger reservation
and swapchain frame target, and installs a newly generated `KmsSubmitWindow`
from the current KMS timing authorities. The planner state is committed only
after both ownership holders have passed preflight, keeping the target/window
pair consistent at every observable state.

Worker-reserved, worker-queued, submitted, stale-generation, already-owned,
regressive, and unsafe candidates are rejected without identity mutation.
Predictive O1 lifecycle stages are untouched. Pull-in summary counters and
focused trace events record attempts, successes, rejection classes, and the
number of refresh intervals advanced.

## Diagnostics

The existing performance snapshot is extended with dispatch-tail feedback and
ready-pull-in counters. Per-frame logging remains trace-mode-only. The old
dispatch miss/apply guard miss classification remains separate.

## Verification

Part A is developed first with tests for reproduction/classification, immediate
learning, saturation, gradual decay, and queue-residency exclusion. Part B
starts with the 165 Hz planner/scheduler reproduction showing an N+2 target and
an N+1 submit boundary, then adds successful explicit replacement and unsafe
ownership/generation/O1 rejection cases. Existing Reactive Double, target
identity, physical-claim, worker-ticket, ABA, and Predictive O1 tests remain
unchanged and must stay green.

The two production changes are committed separately after focused and complete
Rust verification. Hardware acceptance is not inferred from unit tests; the
Blur-disabled native Lamp workload remains a separate acceptance run.
