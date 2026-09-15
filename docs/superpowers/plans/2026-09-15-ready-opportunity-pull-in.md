# Plan: Ready-time physical opportunity pull-in

## Goal

Allow a non-predictive ready frame to replace a still-unsubmitted conservative
target with the earliest newly reachable unowned physical refresh, while
preserving the exact target/window and ownership invariants.

## Scope

- `src/native/presentation_deadline.rs`
- `src/native_output/scanout/output_swapchain.rs`
- `src/native_output/presentation/transaction.rs`
- `src/native_output/presentation/ledger.rs`
- `src/native_output/runtime/presentation_cycle.rs`
- `src/native_output/pacing.rs` and performance snapshot plumbing only if
  required for the focused pull-in diagnostics

## Tasks

### 1. Add the RED planner/pipeline reproduction

- Use a 6,060,606 ns refresh interval and a prior physical presentation.
- Admit rendering too late for the full pre-render estimate to reach N+1 and
  assert the planner selects N+2.
- Make the frame ready much earlier and calculate KMS-only remaining service.
- Assert the current N+2 submit window remains gated at approximately N+1 plus
  100 us and the current ready-submit decision waits through N+1.

Run the focused deadline/scheduler/presentation tests and retain this test as
the deterministic regression scenario.

### 2. Add planner-owned ready-service and replacement candidate semantics

- Add a named `ReadyPresentationServiceEstimate` containing only dispatch and
  apply service and an overflow-safe total duration.
- Add a `PresentationDeadlinePlanner` API that searches strictly after the
  physical frontier and before the old target, returning a fresh exact target
  and the abandoned target without mutating the old value.
- Keep advisory Reactive Double targets non-gating and do not allow the
  planner operation to change Predictive O1 identities.
- Commit planner state only after runtime ownership preflight succeeds.

### 3. Add ownership-correct ready replacement

- Add read-only preflight and commit operations to the output transaction
  descriptor/ledger and Atomic output swapchain.
- Require the frame to still be ready and bound, the generation to match, the
  candidate to be strictly after the presented frontier, and all other worker
  or kernel owners to remain strictly ordered.
- Reject worker-reserved/queued/submitted, stale, duplicate, regressive, or
  already-owned candidates without mutation.
- On success, explicitly release the N+2 reservation, install a new N+1
  reservation, and replace the frame's `KmsSubmitWindow` from current dispatch
  and apply authorities in one coordinated transition.

### 4. Integrate before worker reservation and add diagnostics

- Invoke the operation after the frame is render-ready and before any worker
  pacing ticket or queue ownership is taken.
- Exclude frames with a live Predictive O1 attempt and preserve all existing
  O1 stage/ABA behavior.
- Update local diagnostics to the replacement target/window and expose bounded
  attempt, success, rejection, and advanced-interval counters.
- Add focused success and rejection tests for ownership, generation, readiness,
  target ordering, O1 identity, and exact old-target release.

### 5. Verify and commit Part B

Run the focused tests from Part A plus the full requested suite:

```text
rtk cargo test --locked kms_worker
rtk cargo test --locked presentation_deadline
rtk cargo test --locked scheduler
rtk cargo test --locked pacing
rtk cargo test --locked presentation
rtk cargo test --locked adaptive_buffering
rtk cargo test --locked native_output
rtk cargo fmt --all -- --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

Review the diff for target identity mutation, queue-residency accounting,
Reactive Double gating, and Predictive O1 changes, then commit:

```text
fix(pacing): pull ready frames into newly reachable refreshes
```
