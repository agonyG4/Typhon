# Evidence-Gated Cause-Aware MissRecovery

## Status

This is a qualification-driven refinement to the WarmPaired service-estimator
design. The earlier WarmPaired note remains the historical design record; this
note records the recovery policy refined after native qualification.

## Problem

The independent estimator is intentionally conservative during cold start and
after a render-service miss. The original recovery implementation applied that
fallback to every proven miss, including failures already absorbed by a
dedicated KMS timing model. That was safe, but over-conservative: a KMS miss
could resurrect the independent render tail even when paired render service
remained valid.

## Policy

Recovery continues to use exactly the existing `ColdStart`, `WarmPaired`, and
`MissRecovery` modes. Exact sync-file render misses and guarded approximate
render misses always reset the full independent recovery horizon because they
are direct evidence against the paired service confidence.

KMS recovery is evidence-gated. A dispatch miss preserves the current
estimator state only when the exact submitted worker ownership proves that the
target was binding, the worker had a fair dispatch chance, the submission
overran its commit-complete deadline, the adaptive dispatch tail guard
increased, and the increase was not clipped by its cap. Missing, blocked,
non-binding, capped, or incomplete evidence resets the independent horizon.

An apply-guard miss preserves state only when the authoritative presentation
timing model accepted the matching pageflip observation and actually increased
its adaptive apply guard without clipping the requested correction at its cap.
Stale timing identity, rejected observations, saturated guards, and partially
clipped increases retain the conservative fallback. Preservation therefore
requires the responsible adaptive model to absorb its requested correction
completely, not merely to report a positive observed increase.

Every proven miss still increments total miss accounting. Preservation means
preservation exactly: an already-active `MissRecovery` horizon is neither
cleared nor reset by a successfully self-recovered KMS miss. Only exact paired
service observations advance that horizon. An earlier render miss stored in
`pending_proven_deadline_miss` retains precedence over later KMS classification
for the same frame.

## Evidence ownership and service semantics

Dispatch evidence is created beside the worker's exact dequeue and submission
timestamps and is carried through `KmsSubmittedOwnership` for the matching
pageflip token. Recovery never reconstructs fair dispatch from aggregate
timing snapshots. The pageflip trace distinguishes the submit-window dispatch
budget used by that job from the dispatch budget after the worker observes and
adapts to the miss. Queue residency remains upstream ownership evidence and
is excluded from paired service and selected KMS service cost.

Apply evidence is returned at the timing-model observation boundary with its
acceptance result and adaptive-guard before/after values. The pageflip path
uses that exact result rather than an aggregate snapshot.

## Predictive O1

Estimator selection is unchanged. `RenderPrediction::total_cost_ns` remains
the selected end-to-end authority and continues through
`PipelineServiceEstimate` into Predictive O1 overlap feasibility. KMS misses
do not masquerade as render-readiness misses, and physical, attempt, and target
identity rules are unchanged.

## Native qualification

The policy is designed for the next native run to report each proven miss's
cause, recovery disposition, recovery horizon before and after, and the exact
dispatch or apply evidence that justified the disposition. Automated tests
prove the ownership and gating rules; they do not constitute hardware
qualification.
