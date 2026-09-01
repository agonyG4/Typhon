# Typhon Native Wake Authority v1

## Status

Approved implementation design for the native scheduler/event-loop liveness
closure. This design preserves the accepted DMA-BUF GPU release authority,
O1 render-ahead, KMS worker ownership, presentation protocol, input epoch, and
shutdown architectures.

## Problem and root cause

The native runtime currently has two independent scheduler authorities. The
explicit scheduler derives an action and, for pipeline-aware output, a wait
reason. Runtime timer orchestration then separately merges raw timestamps from
the frame scheduler, the scheduled presentation target, and several unrelated
subsystems. In the current checkout, `NativeRuntime::arm_runtime_deadline` in
`src/native_output/runtime/metrics.rs` arms all of these values together.

That merge includes timestamps that are not temporal ownership:

* an input backlog is represented by `Some(now)`;
* pending Astrea publication is represented by `Some(now)`;
* commit-timing planning is represented by a timestamp equivalent to `now`;
* XWayland requests another turn by directly arming `Some(monotonic_now_ns())`;
* a presentation target's render-start timestamp remains eligible even when
  the pipeline decision has moved to `WaitForWorkerQueue`, `WaitForPageFlip`, or
  `WaitForBuffer`.

When one of those timestamps is already expired but its owner cannot make
progress, the timerfd is repeatedly consumed and re-armed without a state
transition. The latest native run's high immediate-timer count and scheduler
wake lateness are consistent with this split authority. The fix is ownership
based: one decision-aware temporal deadline, one coalesced continuation eventfd,
and readiness from the external owner for blocked pipeline states.

The event loop also currently retains `armed_deadline_ns` after draining a
one-shot timer. That timestamp is useful for lateness measurement but must not
be treated as an actively armed deadline after consumption.

## Current producer audit

The audit was performed against the current `main` checkout on 2026-09-01.
The following classification is the source-of-truth migration list.

| Producer | Current use | Classification | v1 treatment |
| --- | --- | --- | --- |
| `NativeFrameScheduler::next_deadline_ns` | refresh, ready-submit, or pageflip watchdog depending on scheduler-local state | real deadline, but incomplete pipeline authority | derive through the decision-aware scheduler wake contract |
| `visual_target_deadline_for_target` | raw scheduled target render-start timestamp | real only when the current decision is `WaitForRefresh` for visual work | remove from independent runtime aggregation; use the scheduler contract |
| `AtomicCommitArbiter::watchdog_deadline_ns` | timeout for a submitted atomic commit | watchdog/timeout | retain as an independent real timer owner |
| `ExplicitSyncWatchRegistry::next_fallback_deadline_ns` | acquire fallback deadline | watchdog/timeout | retain as a real timer owner |
| `XwaylandService`/adoption/resize `next_deadline_ns` | startup, backoff, adoption, and resize timeouts | watchdog/timeout | retain as real future deadlines; continuation uses eventfd |
| `NativeControlServer::next_deadline_ns` | idle-client/control timeout | watchdog/timeout | retain as a real future deadline |
| `NativeCursorOutputArbitration::deadline_ns` | cursor response window | real deadline | retain as a real future deadline |
| `CompositorState::next_surface_pacing_deadline_ns` | FIFO fallback and commit-timing release boundary, plus readiness-now compatibility | mixed | retain only its future temporal boundary; represent readiness-now through continuation/readiness |
| `CompositorState::next_commit_timing_planning_deadline_ns` | `Some(client_pacing_now_ns())` when planning is pending | immediate continuation | remove from timer aggregation; request `CommitTimingPlanning` |
| pending Astrea publication | `Some(now_ns)` in runtime aggregation | immediate continuation | request `AstreaPublication` |
| `NativeInputEpoch::backlog_pending` | `Some(now_ns)` in runtime aggregation | immediate continuation | request `InputBacklog`; keep the bounded drain and epoch semantics |
| XWayland reactor continuation | direct `arm_deadline(Some(monotonic_now_ns()))` | immediate continuation | request `XwaylandContinuation` |
| shutdown pageflip/suspended deadlines | shutdown safety boundary and suspended timeout | watchdog/timeout or external lifecycle deadline | retain existing shutdown-specific arming, with continuation for immediate publication only |
| libinput, DRM, KMS worker, sync-file, Wayland, control, and child fds | external readiness | external readiness wait | continue to use epoll; never poll them with a stale visual deadline |

The search also found test-only absolute timer arms and unrelated compositor
clock reads. They remain tests or protocol timestamp producers and are not
runtime wake ownership.

## Architecture

### Wake plan

Create `src/native_output/runtime/wake_plan.rs`. It contains deterministic,
allocation-free planning types and functions only; it does not perform
rendering, KMS I/O, Wayland dispatch, protocol completion, or eventfd/timerfd
I/O.

The plan has a fixed continuation bitset and at most one selected deadline:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeDeadlineOwner {
    FrameScheduler,
    PresentationTarget,
    AtomicCommitWatchdog,
    ExplicitSyncFallback,
    XwaylandTimeout,
    CursorResponse,
    ControlTimeout,
    SurfacePacing,
    DmabufRetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeDeadline {
    pub(crate) owner: NativeDeadlineOwner,
    pub(crate) at_ns: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct NativeContinuationReasons(u32);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct NativeWakePlan {
    pub(crate) continuation: NativeContinuationReasons,
    pub(crate) deadline: Option<NativeDeadline>,
}
```

The exact visibility and bit constants follow repository conventions. The
bitset includes at least `InputBacklog`, `AstreaPublication`,
`CommitTimingPlanning`, and `XwaylandContinuation`. It also has explicit OR,
contains, and union operations, so no hash set, queue, mutex, or per-wake heap
structure is introduced.

The plan accepts current authoritative state, not a cached prior decision. Its
pure selection rule is:

1. Add continuation reasons for immediate bounded work.
2. Add only genuine future deadlines from independent owners.
3. Add the scheduler deadline supplied by the current scheduler wake contract.
4. Select the earliest timestamp with deterministic owner tie-breaking.
5. Do not clamp an expired timestamp and do not convert an external blocker to
   a timer.

### Decision-aware scheduler contract

Extend `ExplicitAtomicSchedulerDecision` in
`src/native/scheduler/pipeline.rs` with a wake deadline contract. The contract
contains the scheduler-owned deadline kind and absolute timestamp, for example
`RenderStart`, `SubmitNotBefore`, `ProtocolRefresh`, or
`PageFlipWatchdog`. The scheduler produces the contract beside `action` and
`wait_reason` from the same current pipeline snapshot.

Required mapping:

| Action/state | Scheduler timer authority |
| --- | --- |
| `WaitForRefresh` before visual render | exact current render-start boundary |
| `WaitForRefresh` with a ready frame | exact current submit-not-before boundary |
| `WaitForRefresh` for protocol-only work | exact refresh boundary |
| `WaitForWorkerQueue` | no visual deadline; KMS worker readiness is progress authority |
| `WaitForPageFlip` | no visual deadline; DRM readiness is progress authority; a genuine pageflip watchdog may remain |
| `WaitForBuffer` | no obsolete visual deadline; buffer/fence/pageflip readiness is progress authority |
| `Render`, `RenderAhead`, `SubmitReady`, `SubmitReadyLate`, `CompleteProtocolOnly` | no rediscovery timer |
| `ReadyTargetInvalidated`, `PageFlipWatchdogExpired` | handle the terminal transition before any rearm |
| `Idle` | no scheduler deadline |

The scheduler contract is recomputed after ownership transitions. No runtime
cache may carry a scheduler deadline over render completion, ready-frame
creation, worker admission, commit submission, pageflip completion, target
replanning, or buffer availability unless it carries and validates an exact
state generation/identity. The implementation uses pure recomputation at the
runtime arm boundary, which is preferred over caching a small calculation.

### Continuation eventfd

`NativeEventLoop` owns one `eventfd(EFD_CLOEXEC | EFD_NONBLOCK)` and registers
it as an internal `NativeEventSource::RuntimeContinuation`. External callers
cannot register that source, just as they cannot register the internal timer
source.

`request_continuation(reason)` ORs the reason into the pending fixed bitset. If
the eventfd is not currently signaled, it writes one unit and marks it
signaled. If already signaled, it only increments the coalescing metric. The
write is bounded to one unit per pending signal, and `EAGAIN` is treated as
the already-signaled case after preserving the reason bits.

During `epoll_wait`, the continuation readiness is processed alongside every
other ready source. The eventfd is drained once, its current bitset is taken,
and the signaled flag is cleared. `NativeWakeup` exposes both the regular
readiness reasons and the continuation reasons. A continuation cannot suppress
DRM, KMS worker, input, Wayland, sync, or control readiness returned in the
same epoll result.

The runtime services all observed native readiness first according to its
existing bounded domain plan. If bounded input draining still reports debt,
the runtime requests exactly one next `InputBacklog` continuation after the
current bounded turn. It never loops over the internal queue without returning
to epoll.

### Timer lifecycle

The existing nonblocking `CLOCK_MONOTONIC` absolute timerfd remains the sole
temporal wake source. `arm_deadline(None)` disarms it. A successful one-shot
timer drain clears the active `armed_deadline_ns` state while preserving the
fired deadline separately for lateness calculation in that `NativeWakeup`.

Each runtime rearm builds one `NativeWakePlan`, records its selected owner,
arms the timer with its absolute timestamp, and requests the plan's
continuation reasons. Disarming when no real deadline exists is explicit.
The plan and metrics never cause a wake on their own.

### Fairness

The intended sequence for simultaneous input debt and KMS readiness is:

```text
input debt -> one continuation eventfd unit
DRM/KMS readiness -> their existing fds

epoll_wait -> NativeWakeup contains all ready domains
    -> service KMS/DRM ownership
    -> service one bounded input batch
    -> request one next continuation only if input debt remains
    -> return to epoll
```

The continuation fd is an ordinary epoll member, not a userspace scheduler
loop. Coalescing prevents eventfd write amplification and the single bounded
drain preserves DRM/KMS fairness.

## Runtime integration

Move timer orchestration out of `metrics.rs`. The metrics file will retain
metrics collection and export; the wake plan module and runtime cycle own wake
planning. The central runtime rearm path will collect:

* the current decision-aware scheduler deadline;
* atomic commit watchdog;
* explicit-sync fallback;
* real XWayland timeout;
* cursor response deadline;
* control timeout;
* future surface-pacing boundary;
* DMA-BUF retry deadline.

It will separately request continuation reasons for input backlog, Astrea
publication, commit-timing planning, and XWayland continuation. Immediate
reasons may be requested directly at the point they are discovered, but all
runtime-level rearming is represented by the same fixed contract and metrics.

`NativeRuntimeState` will treat continuation readiness as equivalent to the
corresponding serviceable work, while genuine timer readiness continues to
service timeouts and future-boundary operations. In particular, Astrea and
commit-timing planning must not depend on `reasons.timer()` after migration.

The accepted DMA-BUF release registry and retry debt remain unchanged. The
DMA-BUF retry deadline remains a genuine future timer owner and does not become
continuation work.

## Observability

Preserve the existing compatibility counters:

* `expired_deadline_wait_count`;
* `repeated_immediate_timer_wake_count`;
* `multiple_deadline_owner_violation_count`.

Add bounded counters for runtime arms/disarms, continuation requests,
coalesced requests, continuation wakes, each migrated continuation source,
stale re-arms, past-deadline arms, and deadline-owner selection. Counters are
updated on the native hot path without stdout. At most one shutdown summary is
emitted as `event=native_wake_authority_summary` when summary logging is
enabled. Metrics describe the plan; they never decide it.

The compatibility deadline counters will observe the decision-aware deadline,
not the removed raw visual aggregate. A stale same-deadline rearm remains
observable in the planner/arming boundary and is expected to be zero for
normal scheduler visual work.

## Pageflip cadence metrics

Keep the legacy `pageflip_intervals` samples and percentile fields unchanged.
Add a second bounded `active_pageflip_intervals` sample set. In
`note_pageflip`, record every interval in the legacy set, and record the same
interval in the active set only when the existing `is_active_refresh_interval`
policy accepts it. Keep incrementing `idle_intervals_excluded` for excluded
intervals. Use the existing `BoundedSamples` percentile implementation for both
sets and export `active_pageflip_interval_p50_us`,
`active_pageflip_interval_p95_us`, and `active_pageflip_interval_p99_us`.

## Tests

Tests are written first and run to a real failure before the corresponding
production implementation. The deterministic suites cover:

* expired visual target with `WaitForWorkerQueue`;
* expired visual target with `WaitForPageFlip`;
* exact render-start and ready-frame submit boundaries;
* no timer for actionable render/render-ahead/submit/protocol actions;
* input backlog continuation and bounded multi-turn draining;
* continuation reason coalescing and eventfd readiness;
* continuation plus real fd readiness in one `NativeWakeup`;
* no continuation when debt is exhausted;
* all representative real timer owners remain absolute timerfd deadlines;
* stale same-deadline rearm accounting;
* truthful consumed timer state with preserved lateness;
* legacy versus active pageflip percentile samples.

Existing scheduler, pipeline, event-loop, input, pointer-constraint,
surface-pacing, commit-timing, KMS worker, O1, output-transaction, DMA-BUF,
session, and shutdown tests remain regression gates. No native qualification is
claimed from unit tests; the required TTY/DRM-seat run remains a user-operated
qualification step.

## Non-goals and safety boundaries

This change does not modify DMA-BUF release ownership, correlation, retry
semantics, Direct Scanout, O1 admission, KMS worker ownership, output
transactions, protocol semantics, input epoch semantics, cursor constraints,
XWayland ownership, session safety, or shutdown SafeDisable behavior. It does
not add threads, mutexes, sleeps, polling loops, GPU waits, `glFinish`, or
per-event logging.

## Architecture diagram

```text
                         current authoritative state
                                      |
                    +-----------------+------------------+
                    |                                    |
       explicit pipeline scheduler                independent native owners
       action + wait reason + deadline            watchdogs / future boundaries
                    |                                    |
                    +-----------------+------------------+
                                      |
                              NativeWakePlan
                         one deadline + bitset reasons
                           /                    \
                          /                      \
          absolute CLOCK_MONOTONIC timerfd       coalesced continuation eventfd
                    (future time)                 (next bounded turn)
                          \                      /
                           +----------+---------+
                                      |
                                 epoll_wait
                                      |
             DRM/KMS/input/Wayland/sync/control + continuation all visible
                                      |
                              bounded fair runtime cycle
```

