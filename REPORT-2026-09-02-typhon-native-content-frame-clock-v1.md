# Typhon Native Content Frame Clock v1 Closure Report

Date: 2026-09-02

Status: source and deterministic-test closure complete; native DRM/KMS qualification pending

## Root cause

The deterministic RED case confirmed the structural failure hypothesized from the
165 Hz evidence. With a physical primary at sequence 1, a 17 ms ReactiveDouble
prediction selected sequence 4. Before this closure, `plan_render_ahead()` treated
that advisory target as the predecessor and produced a successor after sequence 4.
One in-flight primary could therefore reserve multiple future refresh opportunities
because of prediction metadata rather than physical or binding ownership.

The current KMS worker source also confirms that `ReactiveDouble` does not make the
worker sleep on its presentation target: the worker wait-arm predicate excludes that
target reason. The primary pageflip path excludes `AtomicCommitKind::PlaneDelta`
from primary completion, so cursor-only pageflips do not advance the primary pacing
phase or the primary cadence samples.

The hardware cadence numbers supplied in the task remain evidence requiring a new
native qualification run. No native DRM/KMS run was attempted in this environment.

## Wake and target ownership

Before:

```text
physical primary phase
        -> ReactiveDouble target with conservative timestamp
        -> pending target lookup
        -> O1 predecessor = target metadata
        -> successor = metadata sequence + 1
```

After:

```text
physical primary presentation
        -> stable per-output phase anchor
        -> PrimaryRefreshFrontier
             ├─ advisory ReactiveDouble: next physical opportunity
             └─ binding target: exact reserved opportunity
        -> O1 successor after the real frontier
        -> exact READY target identity
        -> KMS admission
        -> physical pageflip
        -> phase advancement
```

`PresentationTargetAuthority` is explicit. `ReactiveDouble` is `Advisory`; Commit
Timing, predictive READY targets, recovery targets, and other exact ownership paths
remain `Reserved`. Advisory timestamps remain available for feasibility and
diagnostics, but they do not reserve a physical refresh. `plan_render_ahead()`,
`reactive_target_after()`, and successor planning now use the physical predecessor
for this distinction. No parallel ownership queue was introduced.

Commit Timing still uses `plan_not_before()` and retains its explicit client lower
bound. READY identity and target mutation rules were not relaxed.

## Content-cadence attribution

`NativeFramePacing` now keeps bounded samples for:

```text
primary presentation interval
callback admission -> next commit
client commit -> render start
render start -> READY
READY -> submit
submit -> pageflip
selected target distance
actual primary distance
```

The shutdown event `native_content_frame_clock_summary` also reports separate
Reactive and predictive early/late target counters, fast/slow client samples, and
the diagnostic attribution classes:

```text
callback_handoff_limited
client_limited
target_limited
render_limited
submit_limited
kms_limited
target_hit
```

The fast-client threshold is telemetry-only:
`min(refresh_interval / 2, 2 ms)`. It does not affect protocol or scheduling
behavior. Callback timing was not changed; the implementation records the existing
admission and subsequent commit timestamps.

The existing active pageflip policy remains unchanged. Its active samples continue
to be collected only from the primary completion path, while legacy pageflip sample
fields retain their old semantics and idle exclusions remain visible.

## Predictor audit

No predictor constants were lowered and no protection against measured tails was
removed. The shutdown content summary exposes the current prediction snapshot:

```text
EWMA render
upper render deviation
p90 recent render
render risk
p95 wake lateness
p95 worker queue residency
p95 worker pre-submit and dispatch
p95 atomic ioctl and submit
p95 target slip
KMS dispatch budget
KMS apply guard
KMS total lead
total cost
idle wake guard
```

The paired end-to-end service model was not added because the available evidence did
not prove that independent percentile summation, rather than target ownership, was
the limiting cause. The next native run can now attribute that decision using the
bounded stage data.

## Wake-authority diagnostics

The existing aggregate `stale_deadline_rearms`, `past_deadline_arms`, and
`deadline_owner_cursor` fields remain unchanged. `NativeWakeAuthorityMetrics` now
also records fixed per-owner stale and past counters for every
`NativeDeadlineOwner`, without a map, allocation, or per-wake logging. This makes
the six residual deadlines from the previous hardware run attributable during the
next run.

SurfacePacing was audited but not changed: no deterministic or native evidence in
this closure proved that its matured timestamp was the residual owner. It remains a
real future deadline while future and becomes due state after maturity under the
accepted wake-plan contract.

## RED/GREEN evidence

The RED case `reactive_advisory_pending_target_does_not_reserve_a_false_o1_frontier`
exercises `reactive_or_commit_timing_target()`, the real pending-target planning
boundary used by the output path, `plan_visual_target_for_budget()`, and the
planner's O1 path. It demonstrates the pre-fix false frontier and now produces the
physical successor.

The virtual 165 Hz oracle uses a 6,060,606 ns refresh, O1 enabled, conservative
17 ms advisory metadata, 1 ms actual service, and 128 callback-driven iterations.
It reports 128 O1 successes and one-refresh primary spacing. A separate 8 ms
service test keeps a later opportunity legitimate.

The attribution test covers client, target, render, submit, KMS, and target-hit
classification. Existing pageflip tests continue to prove that PlaneDelta does not
produce a primary presentation completion.

## Verification

Fresh static verification:

```text
rtk cargo fmt --check                         PASS
rtk cargo check                              PASS
rtk cargo clippy --all-targets --all-features -- -D warnings
                                             PASS; no issues found
rtk cargo test                                PASS; 3225 passed, 5 ignored,
                                             0 failed, 40 filtered, 30 suites
git diff --check                              PASS
```

Focused results:

```text
PresentationDeadlinePlanner                  20 passed
NativeFramePacing / attribution               28 passed
Native wake plan / wake authority             12 passed
Presentation pacing-mode / O1                33 passed
KMS worker                                   110 passed
Native event loop                             34 passed
Adaptive buffering / O1                       32 passed
Output swapchain                              16 passed
Frame callbacks                               10 passed
Commit Timing                                  5 passed
FIFO                                           1 passed
O1                                             5 passed
DMA-BUF                                        32 passed
Cursor                                         173 passed
Output transactions                             4 passed
Shutdown                                       28 passed
```

The first full-suite attempt had one transient XWayland popup-order assertion.
The isolated test rerun passed, and the final fresh full suite passed with zero
failures. No XWayland production code was changed by this closure.

## Non-regression evidence

The closure changes are limited to presentation target authority/frontier logic,
bounded cadence telemetry, wake-owner attribution, and their tests. No DMA-BUF,
Direct Scanout, input routing, pointer, SHM, transaction ownership, worker timeout,
or shutdown ownership code was changed by these commits. O1 remains enabled in the
oracle and existing adaptive buffering paths.

No new thread, mutex, sleep, polling loop, forced periodic repaint, GPU blocking
wait, or timer clamp was added. The existing timerfd/eventfd wake architecture is
unchanged.

## Adversarial self-review

```text
Cursor PlaneDelta can change primary phase?              No; primary completion excludes it.
Reactive metadata can reserve refreshes?                 No; it is advisory in the frontier.
One in-flight advisory primary reserves many slots?       No; O1 follows physical/binding state.
READY target can silently mutate?                         No; existing identity checks remain.
Commit Timing lower bound preserved?                      Yes; plan_not_before remains binding.
Fast client systematically skips every other refresh?     No in the virtual oracle; requalify natively.
Idle client can fail 165 Hz qualification?                No; no forced repaint; active samples exclude idle gaps.
Predictor tails hidden by tuning?                         No; no constants were lowered.
Frame callback timing changed?                            No; only timestamps/metrics were added.
Matured SurfacePacing timestamp proven polling owner?      No evidence; unchanged.
DMA-BUF ownership changed?                                 No.
Direct Scanout ownership changed?                         No.
Cursor wake liveness regressed?                            No change to the accepted cursor closure.
Input pre-read semantics changed?                         No change to input logic here.
O1 disabled?                                               No; the oracle records 128 successful uses.
```

## Native qualification status and limitations

Native qualification was not run because this environment does not own the required
DRM/KMS TTY/seat. The user should run the accepted 1920x1080@165 Hz Atomic
EGL/GBM + KMS-worker command with a sustained content-producing workload and review:

```text
primary_present_interval_p50/p95/p99
callback_admission_to_next_commit_p50/p95/p99
content attribution counts
selected versus actual target distances
stale_<owner> and past_<owner>
O1 counters
KMS worker counters
DMA-BUF release counters
transaction/compliance counters
SafeDisable shutdown
```

The native acceptance target remains a zero aggregate stale/past deadline count,
one-refresh median primary cadence for a fast client whose measured service fits the
165 Hz budget, and preserved O1/DMA-BUF/transaction safety evidence.

