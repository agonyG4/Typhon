# Plan: Adaptive KMS dispatch-tail guard

## Goal

Teach the native KMS worker's existing bounded-P95 budget to protect itself
from proven commit-complete deadline tails without adding queue residency or
changing apply-guard adaptation.

## Scope

- `src/native_output/kms_worker/timing.rs`
- `src/native_output/kms_worker/thread.rs`
- `src/native_output/kms_worker/metrics.rs`
- the existing worker timing/control snapshot propagation files, if required

## Tasks

### 1. Add the RED reproduction and learning tests

- Reproduce a low-P95 history with one submit return just beyond the
  commit-complete deadline while payload readiness remains early.
- Assert the existing classifier reports `KmsDispatchMiss`.
- Add a failing model test requiring one proven overrun to increase the next
  exported dispatch budget enough for an equal recurrence.
- Add failing bound/decay tests for cap saturation, 32-clean-submit decay,
  non-underflow, clean-streak reset, and queue-residency exclusion.

Run the focused worker tests and record the expected RED failure for the
learning assertion before changing production code.

### 2. Implement bounded tail feedback in the worker model

- Keep the four bounded sample histories and nearest-rank P95 estimator.
- Add centralized constants for the 50 us safety quantum, 32 clean submits,
  50 us decay step, and 1 ms maximum guard.
- Add saturating miss feedback using
  `max(submit_returned - commit_complete_deadline, 0) + safety_quantum`.
- Reset the clean streak on a miss and decay only after a complete clean
  window.
- Keep queue residency out of the model and preserve the single `budget()`
  export.

### 3. Feed real worker observations and expose diagnostics

- In the successful real submission path, observe the commit-complete deadline
  against `submit_returned_at` before exporting the next submission budget.
- Preserve `KmsPresentationOutcome` classification and apply-guard adaptation.
- Publish guard, latest overrun, increase, decay, and cap-hit values through
  the existing worker timing metrics and control snapshot conversion.
- Ensure the `submission_budget_ns` carried by the worker event remains the
  same authoritative value consumed by `AdaptiveRenderJournal`.

### 4. Verify and commit Part A

Run:

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
rtk git diff --check
```

Review the diff for queue-residency and apply-guard separation, then commit:

```text
fix(pacing): adapt KMS dispatch tail deadline guard
```
