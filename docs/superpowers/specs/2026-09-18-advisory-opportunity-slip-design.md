# Advisory ReactiveDouble Opportunity Slips

## Status

This qualification-driven refinement separates physical KMS stage attribution
from evidence strong enough to change estimator confidence. It preserves the
Pacing v3 WarmPaired, Pacing v3.1 cause-aware MissRecovery, READY-frame wake
ownership, and Predictive O1 contracts.

## Problem

ReactiveDouble targets describe reachable presentation opportunities for
cadence accounting, but they are deliberately advisory: their authority is
`Advisory`, `is_binding()` is false, they do not become the planner's reserved
target, and their render-start and submit-not-before values do not gate normal
ReactiveDouble work. A physical KMS dispatch slip on such a target therefore
means that a reachable opportunity was not reached; it does not prove that the
paired render-service estimator lost confidence.

The physical classifier still needs to retain its stage attribution. A
`KmsDispatchMiss` remains useful for KMS timing observations and content-clock
attribution, where it is a submission-limited event. The mistake is promoting
that physical result directly to `ProvenDeadlineMiss::KmsDispatch` without
considering target authority.

## Assessment boundary

Pageflip policy introduces an explicit assessment separate from the physical
outcome:

```text
TargetHit
    -> None
RenderReadinessMiss + exact fence
    -> Proven(ExactRender)
RenderReadinessMiss + guarded approximate fence
    -> Proven(GuardedApproximateRender)
RenderReadinessMiss without fence evidence
    -> current conservative render classification
KmsDispatchMiss + binding target
    -> Proven(KmsDispatch)
KmsDispatchMiss + advisory target
    -> Advisory(KmsDispatch)
KmsApplyGuardMiss
    -> current Proven(KmsApplyGuard) behavior
```

Pending render-readiness evidence continues to take precedence over later KMS
classification. Target authority is never used as a generic suppression rule:
an exact or guarded render-readiness miss on a ReactiveDouble target remains
proven and enters full recovery.

Only `Proven` assessments call `recovery_disposition`,
`AdaptiveRenderJournal::note_proven_deadline_miss`, and the existing O1
outcome path. An advisory dispatch slip leaves WarmPaired or an existing
MissRecovery horizon untouched; it cannot grant recovery capacity or train the
KMS dispatch-tail guard. Exact paired-service observation may still progress an
active recovery horizon according to the existing pageflip ordering.

## Diagnostics and metrics

Advisory dispatch slips emit `event=advisory_opportunity_slip` through the
existing conditional pacing trace. The event includes the frame and pageflip
identity, target reason/authority metadata, planned and actual sequences,
dispatch evidence and fairness decomposition, dispatch budget before/after
adaptation, deadline overrun, and recovery horizon/mode before and after. A
bounded `advisory_dispatch_slips` counter is exported in the existing pacing
summary.

The worker's `fair_dispatch_chance` remains exactly
`binding_target && dequeued_before_planned_wake`. The new
`dequeued_before_planned_wake` field makes the diagnostic distinction between
an advisory non-binding sample and a binding sample whose worker ownership was
late. Tail-guard training continues to require `fair_dispatch_chance`.

Proven render-miss trace events additionally expose the existing pageflip
readiness evidence: target reason and binding state, readiness source and
fence quality, payload-ready timestamp, commit-complete deadline, lateness,
composite start, and rendered-at timestamps. Missing evidence is represented
as `none`; no timestamps are inferred.

## Preserved behavior and scope

Content cadence continues to receive `submit_missed = true` for advisory KMS
dispatch slips, so the existing classifier may report `SubmitLimited`. Physical
KMS timing continues to consume the unmodified `KmsPresentationOutcome` and
`observe_pageflip_with_evidence` path. Binding KMS dispatch recovery retains
the existing complete-evidence rule: a fair, positive-overrun, uncapped guard
increase preserves estimator state; otherwise recovery resets. KmsApplyGuard
policy is intentionally unchanged and is not generalized to advisory targets
without native evidence.

No scheduler retune, estimator mode, pacing arithmetic, target selection,
Predictive O1 lifecycle, READY-frame wake ownership, KMS budget constant, or
apply-guard constant changes are part of this design.

## Verification

Tests cover advisory dispatch assessment and unchanged WarmPaired/active
MissRecovery state, exact and guarded render misses on advisory targets,
binding KMS recovery, unchanged KmsApplyGuard behavior, SubmitLimited content
attribution, and the three-way fairness decomposition. Focused Cargo tests,
format/check/clippy/full test verification, diff checks, and a repeat of the
blur-enabled native workload with pacing debug/trace are required. Native
qualification must inspect advisory slips, proven causes, recovery shares,
render readiness evidence, binding dispatch evidence, content attribution,
READY continuations, and Predictive O1 lifecycle output.
