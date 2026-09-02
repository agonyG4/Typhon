# Typhon Native Content Frame Clock v1 Implementation Plan

## Method

Use test-driven development. Each behavioral change begins with a deterministic RED test, then the smallest production change, followed by focused tests and a narrow commit. Do not implement the conditional frontier or predictor change unless its preceding RED evidence fails.

## Task 1 — Baseline and source audit

Capture the current checkout evidence for the physical primary pageflip path, target lifecycle, O1 planning boundary, callback metrics, predictor components, and SurfacePacing deadline ownership. Record exact source locations in the design/report. Preserve unrelated staged report deletions.

## Task 2 — Add fixed per-owner wake diagnostics

Extend `NativeWakeAuthorityMetrics` with fixed-size per-owner stale and past counters. Keep aggregate `stale_deadline_rearms`, `past_deadline_arms`, and owner counters unchanged. Increment counters at the same plan-observation boundary, with deterministic enum-to-index mapping and no map allocation. Add unit coverage for a stale and past arm for each owner family through the shared observer.

## Task 3 — Add content-cadence telemetry

Extend the bounded native pacing/content telemetry with stage samples, target-kind classes, fast-client diagnostics, and predictor snapshots. Use existing timestamps and callback metric handoffs. Add only diagnostic accessors needed to correlate a prepared frame with client commit and callback admission. Emit one bounded shutdown summary and expose the predictor components without making them scheduling authority.

## Task 4 — RED production-boundary Reactive/O1 test

Build a virtual 165 Hz Atomic output test using an actual output-swapchain pending/worker target and the existing planner functions. Construct a conservative ReactiveDouble predecessor that is physically feasible at the next refresh. Exercise `reactive_or_commit_timing_target`, the real pending-target lookup boundary, `plan_visual_target_for_budget`, and `plan_render_ahead`. Assert the pre-fix successor inherits the advisory timestamp/reservation; this test must fail against the desired invariant before the production fix.

If this test does not fail, stop the authority-policy branch and continue with attribution tests rather than adding a frontier.

## Task 5 — RED callback-driven 165 Hz oracle

Add a deterministic no-sleep callback client model with 0–500 us reaction, KMS worker enabled, O1 enabled, and current depth/slot constraints. Assert one-refresh steady-state primary cadence, bounded depth, active O1, exact identity, and no callback duplication/burst. Add a slow-render case proving a later target remains legitimate.

## Task 6 — Conditional advisory/binding authority and physical frontier

Only after Task 4 fails, add explicit target authority and derive the primary refresh frontier from existing binding pipeline claims. Update O1 successor planning to skip advisory predecessor timestamps while preserving strict identity ordering, CommitTiming lower bounds, Direct timing, READY immutability, and proven-miss escalation. Add RED/GREEN tests for advisory predecessor, slow render, CommitTiming, READY identity, predecessor miss, and cursor PlaneDelta phase isolation.

## Task 7 — Conditional paired predictor evidence

Review Task 3 and the virtual oracle's component data. Only if independent p95 stage sums are proven to overestimate measured end-to-end service, add a bounded paired composite-start-to-submit sample. Retain all individual stage telemetry, KMS apply guard, and proven-miss escalation. Do not lower constants without evidence.

## Task 8 — Conditional SurfacePacing closure

Use per-owner native evidence and a deterministic matured-release test. Change SurfacePacing only if its matured timestamp is demonstrably reinstalled while another dependency owns progress. Preserve explicit future release constraints and readiness continuation semantics.

## Task 9 — Focused regression verification

Run focused tests for PresentationDeadlinePlanner, AdaptiveRenderJournal, NativeFrameScheduler/pipeline, O1/triple buffering, output swapchain/READY submission, KMS worker, frame callbacks, CommitTiming, FIFO, Wake Authority/event loop, cursor liveness, DMA-BUF release, output transactions, and shutdown. Inspect the diff for all adversarial failure modes in the task specification.

## Task 10 — Full static verification and handoff

Run fresh:

```text
rtk cargo fmt --check
rtk cargo check
rtk cargo clippy --all-targets --all-features -- -D warnings
rtk cargo test
git diff --check
git status --short
```

Report exact pass/fail counts. Do not run native DRM/KMS qualification from this environment and do not claim hardware qualification. Prepare the user-facing 165 Hz qualification command and review checklist only.

## Commit sequence

1. Documentation: approved design and implementation plan.
2. Wake owner diagnostics with RED/GREEN tests.
3. Content cadence attribution telemetry with RED/GREEN tests.
4. Reactive/O1 production-boundary RED test.
5. Conditional authority/frontier implementation, only if proven.
6. Conditional predictor or SurfacePacing closure, only if proven.
7. Virtual 165 Hz oracle and focused regression tests.
8. Final report and verification evidence.

Each commit stages only files belonging to that task. Existing staged report deletions remain untouched.

