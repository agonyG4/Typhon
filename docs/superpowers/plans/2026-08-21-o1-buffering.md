# O1 Buffering — Opportunity-Locked Elastic Buffering

## Goal

Replace binary Auto Double/Triple scheduling decisions with immutable fixed-VSync
presentation-opportunity leases and a bounded future-primary credit of one or
two. Preserve pageflip authority, existing frame-callback timing, KMS Timing v2,
Direct Scanout eligibility, async/tearing paths, Commit Timing lower bounds, and
the existing swapchain/transaction ownership model.

## Constraints and evidence gates

- Treat current HEAD as authoritative and preserve unrelated worktree edits.
- Reproduce current Auto versus forced Double behavior before changing policy.
- Instrument target drift, sequence gaps, target abandonment, ready-frame
  residency, credit decisions, and exact miss attribution before calling any
  hypothesis proven.
- Add deterministic failing tests before each production behavior change.
- Keep the KMS worker ownership and KMS Timing v2 split intact; do not add a
  pre-armed worker, commit merging, VRR changes, vendor-specific policy, or
  power-state workaround.
- Do not train service estimates from intentional future-frame queue latency.
- Keep live future-primary depth at or below two.

## Work sequence

### 1. Evidence and source boundaries

- Record the current HEAD, worktree state, graph generation/coverage limits,
  journal findings, reference-study limitations, and the failed live-launch
  artifact in the external O1 implementation journal.
- Read the current target planner, scheduler/pipeline, swapchain, presentation
  runtime/pageflip, frame callback, KMS timing, and worker ownership paths in
  bounded source ranges.
- Capture the current target lifecycle and sequence-gap counters with a
  behavior-preserving instrumentation commit when the native session can run.
- Perform the source-layout audit for all files that will be touched.

### 2. Behavior-preserving module extraction

- Split buffering policy/journal/credit-model responsibilities from
  `src/native/adaptive_buffering.rs` into `src/native/buffering/` without
  changing behavior.
- Split presentation opportunity/planner responsibilities from the oversized
  runtime presentation module into `src/native/presentation/` or the nearest
  existing ownership boundary.
- Move focused tests with their domain and keep every extraction commit
  compiling with focused tests.

### 3. Deterministic integrated model (TDD)

- Add a pure virtual-time O1 model covering opportunity clocks, visual work,
  callback-driven work, rendering/fence readiness, worker/synchronous dispatch,
  apply/pageflip outcomes, ownership, immutable leases, and credit.
- First add failing property/state-machine tests for lease immutability,
  strict ordering, depth <= 2, low-load equivalence, overlap-required credit,
  no unnecessary overlap, no mode-switch drop, queue-latency isolation,
  worker equivalence, explicit abandonment, and transient recovery.
- Implement the smallest model needed to satisfy those tests and run the
  required 165 Hz/60 Hz, transient, miss, generation, Commit Timing,
  Direct Scanout-boundary, render-failure, and depth-two scenarios.

### 4. Opportunity identity and immutable leases

- Introduce fixed-VSync `PresentationOpportunityId`,
  `PresentationOpportunity`, and `OpportunityLease` using checked timestamp
  arithmetic and the existing monotonic clock domain.
- Lock a lease at arm time, carry it unchanged through rendering, readiness,
  worker/synchronous submission, kernel submission, and pageflip settlement.
- Narrow or remove armed-frame uses of `plan_target_after`; retain planning only
  for a not-yet-armed successor or Commit Timing lower-bound selection.
- Add explicit terminal pre-render abandonment with a new lease identity for
  unreachable, constraint-advanced, generation-changed, or domain-changed
  cases. Never retarget after render begins.
- Add a read-only `PresentationOpportunityFrontier` projection over existing
  ownership states and assert duplicate live claims rather than silently
  advancing a target.

### 5. Typed service estimates and overlap

- Expose a typed service estimate separating main-loop wake guard, render risk,
  KMS dispatch budget, and KMS apply guard.
- Add checked overlap calculation for each predecessor/successor pair:
  `successor_deadline = T_s - A`,
  `latest_start = successor_deadline - D - R`,
  `overlap_required = T_p - latest_start`.
- Keep prediction proactive and actual pageflip/dispatch outcomes evidentiary;
  do not synthesize proven misses from a prediction.

### 6. Elastic credit controller

- Add a bounded credit controller whose only policy output is future-primary
  capacity one or two; retain compatibility diagnostics for Double/Triple if
  useful, but do not use them to plan targets.
- Grant credit two immediately for positive overlap/proven pressure, do not
  revoke while an extra future primary remains owned, and revoke after a short
  stable negative-slack sequence chosen through simulator evidence.
- Keep capability blockers at credit one and keep `force` as a qualification
  control. Ensure credit changes never mutate an armed lease or create a drop.

### 7. Pipeline integration and estimator audit

- Integrate credit into scheduler decisions, render-ahead admission, ready-frame
  submission, worker pre-admission, swapchain depth checks, and target planning.
- Preserve callbacks at rendered-frame completion and preserve existing direct,
  async/tearing, Commit Timing, partial-repaint, buffer-age, and scene-history
  ownership contracts.
- Track intentional queue latency separately and prove it does not affect
  render, wake, dispatch, or apply service estimates.
- Add bounded internal metrics for credit, lease lifecycle, sequence gaps,
  forward-replan attempts, render-ahead usefulness, overlap slack, and the
  four terminal outcome categories.

### 8. Validation and qualification

- Run focused deterministic suites continuously, then format/check/clippy/full
  tests/source-layout/diff-check using the existing target cache.
- Reproduce the low-pressure alternating 2x2: O1 Auto versus forced credit one,
  worker on/off. Capture the existing harness's raw artifacts and environment.
- Run one repeatable near/over-refresh pressure workload and compare O1 with
  forced credit one, including credit usefulness and recovery.
- Run native vkcube and Kitty/window smoke checks when a clean tty session is
  available. Do not treat the failed session launch as a performance result.
- Perform the mandatory temporal/ownership review, then the independent
  simplicity/performance review; record findings and corrections in the journal.

### 9. Commits and handoff

- Keep logical commits reviewable: extraction, model/tests, leases/planner,
  service/credit policy, integration/telemetry, and qualification/docs.
- Stage only O1 files for each commit; leave unrelated pre-existing edits
  untouched.
- Before claiming completion, run verification commands and report exact
  baseline/final data, simulator/property counts, metrics, limitations, commit
  list, and one of the required O1 verdicts.
