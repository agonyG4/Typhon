# Ready-Frame Wake Ownership Design

## Status

This is a qualification-driven native runtime liveness fix discovered during
Pacing v3.1 qualification. It does not change the Pacing v3.1 estimators,
Predictive O1 target arithmetic, or scheduler decision semantics.

## Evidence and root cause

The native run showed approximately 2,712 content frames, 82.6% WarmPaired,
16.7% MissRecovery, 17 proven KmsDispatch misses, and 10 ExactRender misses.
Only three KmsDispatch misses were both binding and fair; those were correctly
absorbed by the dedicated dispatch-tail model. The remaining 11 binding but
unfair samples had 1.16 ms to 383.21 ms dispatch overruns, unlike genuine fair
worker tails in the tens-to-hundreds of microseconds. `fair_dispatch_chance =
false` must therefore continue to exclude unrelated upstream latency from
dispatch-tail training.

Frame 51 identifies the upstream failure. Its Predictive O1 successor was
physically READY at `render_end_ns = 754418390702`. The predecessor presented
at `754421401812 ns`, the DRM event arrived with only `10 us` receive delay,
and deferred binding correctly produced `submit_not_before = 754421501812`.
After predecessor completion, the runtime observed a binding-ready frame with
no scene debt and no queued visual work, but the frame was submitted at
`754432216404`, about 10.7 ms after its submit boundary. The worker received
the job late and consequently recorded an unfair KMS dispatch miss. This is a
lost wake owner in the native runtime, not a KMS worker dispatch-tail failure.

## Scheduler contract

The scheduler intentionally distinguishes a future wait from actionable work:

* A physically READY frame before `submit_not_before` returns `WaitForRefresh`
  and `SchedulerWakeDeadlineKind::SubmitNotBefore`.
* At or after `submit_not_before`, it returns `SubmitReady` or
  `SubmitReadyLate` and `wake_deadline = None`.

The latter is correct. An actionable scheduler decision has no future
rediscovery deadline; changing that contract or arming a fake past timer would
move runtime ownership into generic scheduler semantics and risk polling.

The existing Atomic commit lane guard remains authoritative. A ready decision
that becomes `WaitForPageFlip` because an Atomic commit occupies the lane and
the worker cannot queue the next job is not actionable and must not request a
continuation. Likewise, `WaitForWorkerQueue` leaves progress to worker
readiness.

## Typed runtime wake requirement

The runtime introduces a typed requirement near the native wake-authority
layer:

```text
None
Deadline(NativeDeadline)
ImmediatePresentation { action: SchedulerDecision }
```

`current_scheduler_wake_requirement()` computes the scheduler decision, applies
all runtime ownership and Atomic lane guards, and then classifies the guarded
action. Future `WaitForRefresh` deadlines remain exact. Main-thread-owned
pageflip watchdogs remain deadlines; worker-owned pageflip and
`WaitForWorkerQueue` remain externally owned. `Idle` is `None`. Actions that
remain immediately executable (`Render`, `RenderAhead`, `SubmitReady`,
`SubmitReadyLate`, `ReadyTargetInvalidated`, `CompleteProtocolOnly`, and
`PageFlipWatchdogExpired`) become `ImmediatePresentation`.

The wake plan carries the typed requirement through planning. A deadline is
installed in the timerfd; an immediate requirement is represented by the
existing continuation bitset and eventfd. The runtime never represents
“execute now” as a past or synthetic timer deadline.

## Continuation ownership and work domains

`NativeContinuationReason::FrameScheduler` identifies the owner of an
actionable frame-scheduler continuation. It is coalesced by the existing
eventfd mechanism, counted by the existing wake-authority metrics, and
reported in the wake-authority summary as `frame_scheduler_continuations`.

The continuation is presentation-only:

```text
FrameScheduler continuation
  -> NativeWorkDomains.presentation = true
  -> NativeWorkDomains.scene = false unless independently dirty
  -> NativeCycleOperationPlan.presentation_due = true
  -> no automatic service_acquire_and_prepare
  -> render_present_and_update_metrics()
```

This allows an already prepared and physically READY output frame to be
submitted when its boundary becomes actionable without rebuilding the Wayland
scene or manufacturing visual work. Existing SceneVisualDebt continuation
logic remains independent and continues to solve scene-generation ownership.

If a continuation cycle discovers that the action is no longer executable, the
next guarded requirement transfers ownership to the relevant pageflip, worker,
render-fence, or future timer state. No unconditional continuation is
reinstalled, so the design cannot create an immediate-cycle busy loop.

## Deferred Predictive O1 and pacing preservation

Deferred Predictive O1 binding remains unchanged:

```text
earliest_submit_ns = max(bind_at,
                          actual_claim.presentation_time + 100_000)
```

Physical frame identity, attempt identity, predecessor identity, physical
refresh claims, target sequences, submit windows, and deferred binding rules
are unchanged. WarmPaired, MissRecovery, render-risk, service prediction,
dispatch-tail adaptation, `fair_dispatch_chance`, KMS apply-guard adaptation,
adaptive buffering credit, overlap calculation, and presentation target
selection are outside this fix.

The `fair=false` KMS samples are a symptom of READY work arriving late at the
worker. They must not be accepted as dispatch-tail training data and must not
be hidden by tuning the worker budget or estimator recovery.

## Verification strategy

The regression suite covers the boundary before and after `submit_not_before`,
the explicit TOCTOU transition, presentation-only domain mapping, Atomic lane
blocking, worker-queue ownership, continuation coalescing, and the unchanged
`actionable_scheduler_decisions_have_no_rediscovery_deadline` contract. Unit
tests establish deterministic ownership behavior; they do not replace the
same blur-enabled native workload with Pacing v3.1 tracing. Native
qualification must check READY-frame submit lateness, continuation counts and
coalescing, fair/unfair dispatch samples, scheduler wake lateness,
WarmPaired/MissRecovery shares, submit/render-limited frames, fast-client
cadence, and Predictive O1 lifecycle transitions.
