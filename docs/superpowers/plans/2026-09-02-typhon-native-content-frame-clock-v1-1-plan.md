# Physical Claim Propagation and Attribution Integrity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Propagate immutable physical primary claims through every native ownership boundary and make callback/content attribution exact to the surface and frame batch that produced it.
**Architecture:** Reuse the existing physical frontier representation as the canonical `PrimaryRefreshClaim`, attach it to `PresentationTarget`, compare claims in planner/swapchain/READY/worker/pageflip paths, and freeze surface-local callback timing in `CompositorFrameBatch`. Keep advisory ReactiveDouble metadata diagnostic-only and keep binding CommitTiming/Predictive targets reserved.
**Tech Stack:** Rust, existing Typhon native scheduler/output swapchain, bounded `BoundedSamples` telemetry, existing compositor frame-batch and callback ownership maps, `rtk cargo` verification.

## Global Constraints

* Execute inline; do not use subagents or worktrees.
* Use TDD: add a compiling behavioral RED test against current behavior before each production behavior change.
* Use `apply_patch` for source/document edits.
* Stage only files belonging to the current commit. Preserve unrelated user-owned report deletions.
* Do not change DMA-BUF, O1 admission/depth, KMS worker timeout ownership, wake authority, cursor liveness, input, pointer, Direct Scanout, SHM, callback terminal semantics, or shutdown behavior.
* Do not add a thread, mutex, sleep, polling loop, forced repaint, blocking GPU wait, unbounded history, or per-event logging.

---

## Task 1 — Add the real swapchain RED boundary

- [ ] Add a test using `OutputSwapchain::finish_render_owned()`, READY validation, and `validate_invariants()` for an advisory predecessor whose metadata is sequence 4 while the intended physical claim is sequence 2, followed by a reserved O1 successor at claim 3.
- [ ] Confirm the current raw-target implementation fails with the strict ordering error.
- [ ] Add the planner-stability RED test that calls `plan_visual_target_for_budget()` twice with the same advisory predecessor and already scheduled successor, asserting the scheduled target and abandon counter remain unchanged.
- [ ] Commit the RED tests separately.

## Task 2 — Introduce and propagate `PrimaryRefreshClaim`

- [ ] Add the fixed-size claim type using the existing clock generation, sequence, and monotonic presentation time representation.
- [ ] Attach the immutable claim to `PresentationTarget`; centralize target construction so reserved targets claim their exact target and ReactiveDouble claims the next phase-aligned physical opportunity after the current real frontier.
- [ ] Keep advisory target metadata unchanged for target-error and prediction diagnostics.
- [ ] Update planner physical predecessor/frontier derivation and O1 successor generation to use claims.
- [ ] Add claim consistency accessors and preserve generation validation.
- [ ] Run focused planner tests and commit.

## Task 3 — Make every output ownership check claim-aware

- [ ] Replace raw target ordering in `OutputSwapchain` invariant, READY, worker-queued, pending-target, and presentation-opportunity-frontier construction with physical claim ordering.
- [ ] Reject duplicate live claims and mixed generations using existing frontier validation.
- [ ] Track the last physically presented primary claim in the existing swapchain state, without creating a second ownership queue.
- [ ] Add pageflip revalidation for READY/worker claims overtaken by a predecessor; reject or route through existing explicit invalidate/replan behavior without mutating target identity.
- [ ] Add tests for valid advisory-then-reserved ordering, reserved out-of-order rejection, duplicate claims, worker mode, and predecessor miss.
- [ ] Run focused output swapchain/READY/KMS-worker tests and commit.

## Task 4 — Close planner churn and target identity observability

- [ ] Replace `plan_visual_target_for_budget()` raw predecessor/scheduled comparison with claim comparison.
- [ ] Verify two planning turns retain the exact scheduled target and do not call abandon/reallocate.
- [ ] Replace any literal `presentation_target_sequence_mutations` zero with real bounded counters for target reuse after abandonment, claim-order violations, or READY identity mutation.
- [ ] Add assertions/tests for immutable READY identity and explicit invalidation on physical miss.
- [ ] Run planner, pacing-mode, and O1 tests and commit.

## Task 5 — Make callback evidence surface-local and batch-local

- [ ] Add bounded live per-surface callback timing state keyed by the existing surface identity.
- [ ] Change callback commit accounting to receive the committing `surface_id`, calculate reaction only from that surface’s prior admission, and return fixed-size evidence.
- [ ] Carry timing evidence with pending callback ownership and freeze it in `CompositorFrameBatch` at capture.
- [ ] Update exact callback admission to write admission timestamps back to the owning surface state before removing callback ownership mappings.
- [ ] Make native render paths consume batch-local evidence instead of global latest callback metrics.
- [ ] Add cross-surface RED/GREEN tests in both callback orderings and a rendered-frame batch-local evidence test.
- [ ] Run frame callback, compositor batch, and native render attribution tests and commit.

## Task 6 — Correct attribution semantics

- [ ] Add immutable `TargetSelectionEvidence` to target selection or the exact rendered-frame observation, including earliest feasible distance and binding status.
- [ ] Change `TargetLimited` to require frozen binding evidence for an intentionally later selection and physical presentation at that selected opportunity with no later-stage miss.
- [ ] Change the existing advisory selected-3/actual-1 test to expect prediction overestimation/target hit rather than `TargetLimited`.
- [ ] Add a real binding target-limited test and preserve separate Reactive/Predictive early/late diagnostics.
- [ ] Add real counters replacing any hard-coded target-mutation metric and expose them in the bounded summary.
- [ ] Run pacing/content attribution tests and commit.

## Task 7 — Full regression verification and report

- [ ] Run focused tests for PresentationDeadlinePlanner, OutputSwapchain, pacing modes, O1/triple buffering, KMS worker, frame callbacks, content attribution, wake authority/event loop, DMA-BUF, output transactions, and shutdown.
- [ ] Run `rtk cargo fmt --check`, `rtk cargo check`, `rtk cargo clippy --all-targets --all-features -- -D warnings`, `rtk cargo test`, and `git diff --check`.
- [ ] Inspect the final diff for raw Advisory ordering, silent READY mutation, callback global contamination, cursor PlaneDelta phase advancement, DMA-BUF changes, and O1 disablement.
- [ ] Create `REPORT-2026-09-02-typhon-native-content-frame-clock-v1-1.md` in English with root cause, claim propagation, attribution evidence, RED/GREEN results, verification, non-regression, and the fact that native qualification was not run.
- [ ] Commit the report and source changes in narrow commits.

## Handoff

The source is ready for the user's real sustained 1920x1080@165 Hz Atomic EGL/GBM + KMS-worker qualification only after the swapchain-level advisory predecessor test is green and all static/focused verification results are recorded. No hardware qualification is performed in this environment.

