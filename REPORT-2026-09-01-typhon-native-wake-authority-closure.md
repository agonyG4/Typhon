# Typhon Native Wake Authority v1 Closure

Date: 2026-09-01

Status: implemented and statically verified; native DRM/KMS qualification not run in this environment.

## Root cause

The native runtime had two independent wake authorities. The pipeline-aware
scheduler selected an action and a wait state, while runtime timer aggregation
later merged raw timestamps from the frame scheduler, presentation target,
control, pacing, XWayland, and other subsystems.

Several values represented immediate work as an absolute deadline equal to the
current time. More importantly, an old presentation render-start boundary could
remain eligible after the pipeline had moved to `WaitForWorkerQueue`,
`WaitForPageFlip`, or `WaitForBuffer`. The timerfd then woke the loop without
making the current owner progress, and the same timestamp could be armed again.

The native evidence is consistent with this split authority: O1 and DMA-BUF
were healthy, while `repeated_immediate_timer_wake_count` and scheduler wake
lateness were high.

## Before and after wake ownership

Before:

```text
pipeline decision ───────────────┐
raw scheduler deadline ──────────┼─> independent timer aggregation -> timerfd
presentation target deadline ────┤
input/Astrea/planning/XWayland now┘

blocked owner + expired visual deadline -> repeated timer wake
```

After:

```text
bounded immediate work -> fixed continuation bits -> one nonblocking eventfd
future physical/logical time -> one earliest owned absolute timerfd deadline
external owner -> that owner's existing fd/readiness

epoll_wait
  -> NativeWakeup contains every ready domain
  -> service external owners and one bounded continuation turn
  -> rederive the plan from current authoritative state
```

The loop now obeys:

```text
time is required       -> one genuine future CLOCK_MONOTONIC deadline
external owner required -> that owner's readiness
another turn required  -> one coalesced continuation eventfd signal
```

No expired deadline is clamped into the future, and no expired presentation
deadline is used to poll ownership.

## Current producer audit

The audit was performed against the current checkout before implementation.

| Producer | Classification | Final ownership |
| --- | --- | --- |
| Current frame-scheduler render/submit/protocol boundary | `REAL_FUTURE_DEADLINE` | Decision-linked scheduler wake contract |
| Legacy pageflip watchdog | `WATCHDOG_OR_TIMEOUT` | Frame-scheduler pageflip watchdog only |
| Atomic commit watchdog | `WATCHDOG_OR_TIMEOUT` | Atomic commit watchdog owner |
| Explicit-sync fallback | `WATCHDOG_OR_TIMEOUT` | Explicit-sync fallback owner |
| XWayland startup/backoff/adoption/resize timeout | `WATCHDOG_OR_TIMEOUT` | XWayland timeout owner |
| Cursor response timeout | `REAL_FUTURE_DEADLINE` | Cursor-response owner |
| Control-client idle timeout | `WATCHDOG_OR_TIMEOUT` | Future control deadline; expired debt uses continuation |
| Surface-pacing future boundary | `REAL_FUTURE_DEADLINE` | Surface-pacing owner, only when readiness is not pending |
| DMA-BUF retry debt | `WATCHDOG_OR_TIMEOUT` | DMA-BUF retry owner; release ownership and correlation are unchanged |
| Input backlog | `IMMEDIATE_CONTINUATION` | `InputBacklog` continuation reason |
| Pending Astrea publication | `IMMEDIATE_CONTINUATION` | `AstreaPublication` continuation reason |
| Commit-timing planning | `IMMEDIATE_CONTINUATION` | `CommitTimingPlanning` continuation reason; resulting target remains temporal |
| XWayland “continue now” | `IMMEDIATE_CONTINUATION` | `XwaylandContinuation` continuation reason |
| DRM, KMS worker, input, Wayland, sync, control, seat, and child fds | `EXTERNAL_READINESS_WAIT` | Existing epoll registrations |

The compositor APIs that expose pacing/planning timestamps remain available to
their existing callers. Native runtime timer authority no longer treats the
commit-planning “now” timestamp as a timer, and it excludes the surface-pacing
readiness-now compatibility value when readiness is already pending.

## Scheduler decision/deadline contract

`ExplicitAtomicSchedulerDecision` now carries `wake_deadline` beside `action`
and `wait_reason`. It is derived from the same current pipeline snapshot as the
decision:

| Scheduler state | Timer authority |
| --- | --- |
| `WaitForRefresh` before visual render | Exact render-start boundary |
| `WaitForRefresh` with a ready frame | Exact submit-not-before boundary |
| `WaitForRefresh` for protocol work | Exact protocol refresh boundary |
| `WaitForWorkerQueue` | No visual timer; worker readiness |
| `WaitForPageFlip` | No visual timer; DRM readiness plus a genuine watchdog if present |
| `WaitForBuffer` | No visual timer; buffer/fence/pageflip readiness plus a genuine watchdog if present |
| `Render`, `RenderAhead`, `SubmitReady`, `SubmitReadyLate`, `CompleteProtocolOnly` | No rediscovery timer |
| `ReadyTargetInvalidated`, `PageFlipWatchdogExpired` | Handle the transition before rearming |
| `Idle` | No scheduler timer |

The runtime recomputes this contract at each arm boundary from current
pipeline ownership. It does not cache a scheduler decision across render,
worker, kernel, pageflip, target, or buffer transitions. An expired explicit
pageflip watchdog is now terminal before rearming; the legacy path preserves
the genuine watchdog without exposing a visual target deadline.

## Wake-plan module

`src/native_output/runtime/wake_plan.rs` contains the fixed-size wake contract:

* `NativeWakePlan` is `Copy` and contains a fixed continuation bitset plus one
  earliest owner-labelled deadline.
* Deadline owners are explicit: frame scheduler, presentation target, atomic
  watchdog, explicit-sync fallback, XWayland, cursor, control, surface pacing,
  and DMA-BUF retry.
* Selection is deterministic with stable first-owner tie breaking.
* There is no hash set, heap queue, thread, mutex, or per-wake allocation.
* The plan does not render, dispatch Wayland, perform KMS I/O, or complete
  protocol work.

The runtime installs the plan through the existing absolute timerfd and
requests the plan’s continuation bits through the event loop.

## Continuation eventfd and fairness

`NativeEventLoop` owns one internal nonblocking `eventfd` registered as
`NativeEventSource::RuntimeContinuation`. External registration of the
internal timer or continuation source is rejected.

`request_continuation(reason)` ORs the reason into the pending fixed bitset.
If the eventfd is already signaled, it performs no write and increments the
coalescing counter. Otherwise it writes one eventfd unit. On an epoll wake the
eventfd is drained once, the complete current bitset is taken, and the
signaled state is cleared.

The continuation source is an ordinary epoll member. A continuation wake does
not replace or hide DRM, KMS worker, input, Wayland, sync, control, or other
ready sources returned in the same epoll turn.

Input remains bounded by `NATIVE_INPUT_DRAIN_BUDGET`. If the bounded batch ends
with debt, the next cycle requests one `InputBacklog` continuation. The loop
returns to epoll between batches and therefore cannot starve DRM/KMS readiness.

## Timer lifecycle and observability

The temporal source remains one `CLOCK_MONOTONIC` timerfd using
`TFD_TIMER_ABSTIME`. `arm_deadline(None)` disarms it. After a one-shot timer is
drained, `armed_deadline_ns()` reports no active deadline; the fired deadline
is retained only long enough to calculate accurate lateness for that wake.

The wake-authority summary adds bounded counters for:

* timer arms and disarms;
* continuation requests, coalescing, and wakes;
* input, Astrea, commit-planning, XWayland, and control-timeout continuation
  requests;
* stale and past-deadline arms; and
* each deadline owner.

At most one `event=native_wake_authority_summary` line is emitted at runtime
drop when native debug summary logging is enabled. Metrics do not select the
wake plan.

## Pageflip cadence metrics

The legacy `pageflip_interval_p50_us`, `pageflip_interval_p95_us`, and
`pageflip_interval_p99_us` fields retain their existing all-sample semantics.
The new active fields use the existing bounded sample implementation and the
existing active-interval policy:

```text
active_pageflip_interval_p50_us
active_pageflip_interval_p95_us
active_pageflip_interval_p99_us
```

The deterministic test sequence records the known 60,000 us idle gap in the
legacy samples, excludes it from active samples, and increments
`idle_intervals_excluded`.

## RED/GREEN evidence

The scheduler authority tests were added before the contract implementation in
commit `829653f`; the initial focused run failed because the wake contract was
absent. The implemented scheduler contract and wake plan are now green.

Final focused results include:

| Focus | Result |
| --- | ---: |
| `native::scheduler` | 46 passed |
| `native::scheduler::pipeline` | 6 passed |
| `native::event_loop` | 33 passed |
| `runtime::wake_plan` | 5 passed |
| `compositor::state::surface_pacing` | 26 passed |
| `control_snapshots` | 6 passed |
| native pacing | 61 passed |
| triple buffering model | 21 passed |
| KMS worker | 123 passed |
| presentation deadline | 19 passed |
| O1/buffering | 26 passed |
| input epoch | 3 passed |
| input batching | 1 passed |
| pointer constraints | 12 passed |
| commit timing | 5 passed |
| DMA-BUF GPU release | 1 passed |
| output transactions | 4 passed |
| shutdown | 28 passed |
| session I/O | 9 passed |

The scheduler tests cover expired worker-blocked visual targets, pageflip
blockers with watchdog ownership, exact render-start and submit boundaries,
actionable decisions without rediscovery timers, and terminal expired watchdog
behavior. Event-loop tests cover continuation coalescing, combined readiness,
external-source protection, and truthful timer consumption. Pacing tests cover
legacy versus active pageflip percentiles and idle exclusion.

## Verification

Fresh final commands:

```text
rtk cargo fmt --check                                      PASS
rtk cargo check                                           PASS
rtk cargo clippy --all-targets --all-features -- -D warnings PASS
rtk cargo test                                             PASS: 3185 passed, 5 ignored, 40 filtered out
rtk git diff --check                                       PASS
```

One earlier full-test attempt had a transient unrelated
`tests/sigchld.rs::one_child_exit_wakes_the_sigchld_signalfd_once` failure
(`0` observed versus `1` expected). The isolated `rtk cargo test --test
sigchld` rerun passed all 4 tests, and the subsequent full `rtk cargo test`
passed 3,185 tests.

## Non-regression evidence

The wake-authority changes do not alter DMA-BUF release ownership, exact
OutputTransactionId correlation, current-token revalidation, retry debt, or
release timing. The DMA-BUF retry deadline remains a genuine timer owner.
The full suite and focused KMS worker, O1, triple-buffering, output transaction,
shutdown, input epoch, pointer-constraint, and protocol-related tests remain
green.

No new thread, mutex, polling loop, sleep, GPU wait, `glFinish`, or per-event
stdout path was added. No 1 us deadline clamp was introduced.

## Native qualification

The real Atomic EGL/GBM + KMS 165 Hz qualification was not run by this agent.
This environment was not used to attempt DRM/TTY-seat qualification, and no
desktop automation or screenshots were used. Therefore this report makes no
native acceptance claim for:

```text
repeated_immediate_timer_wake_count == 0
expired_deadline_wait_count == 0
stale_deadline_rearms == 0
```

The user should run the specified sustained Chromium/Electron workload on the
required TTY/DRM seat and review the wake-authority summary alongside active
pageflip cadence, O1, KMS worker, output transaction, protocol, DMA-BUF, and
shutdown fields. The expected active 165 Hz cadence remains approximately
6,060 us when the workload actually produces sustained 165 FPS.

## Remaining limitations

* Native hardware qualification remains outstanding because it requires the
  real TTY/DRM seat and interactive workload.
* The working tree contains pre-existing user-owned deletions of historical
  reports and a concurrent formatting-only change in
  `src/native_output/runtime/metrics.rs`; these were not staged or changed by
  this closure.
* `past_deadline_arms` remains diagnostic for genuine deadlines that become due
  between planning and timer installation. The correctness target is that a
  stale deadline is never rearmed as an ownership poll; `stale_deadline_rearms`
  is covered by deterministic tests and is expected to remain zero in the
  native qualification.

## Closure commits

The wake-authority work was kept in reviewable commits, including:

```text
7237622 docs: design native wake authority closure
e9ea02c docs: plan native wake authority closure
829653f test: specify native wake authority contracts
9f843eb feat: add decision-aware native wake plan
ce1884c feat: add coalesced native continuation readiness
83724bc feat: centralize native wake authority
5d6f888 fix: guard continuation source registration
c72b76b fix: suppress obsolete legacy visual deadlines
a682059 fix: preserve native pageflip watchdog ownership
edfa8dc fix: make expired pageflip watchdog terminal
23ce528 obs: count native continuation reasons
9d080f3 fix: preserve snapshot decoding for active pageflip metrics
4e75ce4 fix: make initial watchdog deadline explicit
```
