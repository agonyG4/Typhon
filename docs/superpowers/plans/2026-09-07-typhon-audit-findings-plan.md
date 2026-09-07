# Typhon Confirmed Audit Fixes Implementation Plan

> **For agentic workers:** Inline execution is required for this task because the user explicitly prohibited sub-agents. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Correct the four confirmed Typhon runtime timing/ownership defects with focused regression coverage.

**Architecture:** Preserve existing runtime boundaries and add only durable ownership signals: XWayland command pending state, frame-owned fence evidence, complete worker dispatch samples, and an absolute inflight watchdog deadline.

**Tech Stack:** Rust 2024, Cargo, existing unit/integration test fixtures, `rtk` command proxy, codebase-memory graph.

## Global Constraints

- Reuse the repository’s existing Cargo target directory; do not create a second build tree.
- Preserve unrelated working-tree changes and stage only task hunks.
- Do not change buffering thresholds, retry policy, queue depth, guards, Direct Scanout policy, or explicit-sync ownership.
- Implement findings in order: XWayland continuation, fence evidence, worker dispatch budget, absolute watchdog.
- Run each regression test red before the production fix and green after it.

---

### Task 1: Guarantee XWayland backend-command continuation

**Files:**
- Modify: `src/compositor/state/desktop_windows.rs`
- Modify: `src/compositor/server_backend.rs` or the existing server façade module
- Modify: `src/native_output/runtime/cycle.rs`
- Modify: `src/native_output/runtime/work_domains.rs`
- Modify: `src/native_output/runtime/wake_plan.rs`
- Test: existing compositor XWayland resize tests and runtime work-domain/wake-plan tests

**Interfaces:**
- Produces `has_pending_xwayland_backend_commands()` as a non-destructive, X11-accurate predicate.
- Produces runtime state and wake-plan inputs that preserve `XwaylandContinuation` ownership.

- [ ] **Step 1: Add failing tests for mixed-queue ownership and continuation planning.** Assert that an X11 configure/finalize command is XWayland work, an XDG-only command is not, and `xwayland_continuation` produces `NativeContinuationReason::XwaylandContinuation`.
- [ ] **Step 2: Run the focused compositor/work-domain/wake-plan tests and confirm the new assertions fail for the missing predicate/continuation.
- [ ] **Step 3: Implement the filtered pending-command predicate and server façade without taking or mutating the queue.
- [ ] **Step 4: Thread pending XWayland work through `NativeRuntimeState`, `NativeWorkDomains::classify`, and `arm_runtime_deadline`; at cycle tail request one coalesced continuation only for a running runtime with a live managed generation.
- [ ] **Step 5: Add/extend the post-dispatch resize fixture so render-admission queueing is followed by a continuation-owned scene-batch drain; cover final resize when the fixture supports it.
- [ ] **Step 6: Run focused tests, neighboring compositor/native runtime tests, formatting, and inspect the diff.
- [ ] **Step 7: Commit with subject `fix(xwayland): guarantee backend command dispatch continuation`.

### Task 2: Retain render-fence timing evidence

**Files:**
- Modify: `src/native_output/scanout/output_swapchain.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Modify: `src/native_output/runtime/cycle/pageflip.rs`
- Test: scanout timing/pageflip tests and existing output swapchain fixtures

**Interfaces:**
- Produces frame-owned `RenderFenceTimingEvidence` with timestamp, quality, and accounting provenance.
- Produces completion metadata that distinguishes an early-accounted observation from a pageflip-first observation.

- [ ] **Step 1: Add deterministic failing tests for fence-first/pageflip-first timestamp equivalence, retained evidence after timing-FD consumption, and one-shot estimator accounting.
- [ ] **Step 2: Run the focused scanout/pageflip tests and confirm the pre-fix fence-first path loses physical timing or double/omits accounting as expected.
- [ ] **Step 3: Store evidence on `RenderedOutputFrame` when `sample_pending_timing` samples it and take the timing FD immediately afterward.
- [ ] **Step 4: Make pageflip completion reuse stored evidence or sample once when no evidence exists; carry accounting provenance into `PresentedOutputFrame`.
- [ ] **Step 5: Guard pageflip-side estimator/quality insertion with the provenance flag while always using physical evidence for classification and frame-service observations.
- [ ] **Step 6: Run focused tests, neighboring scanout/runtime tests, formatting, and inspect FD/resource ownership in the diff.
- [ ] **Step 7: Commit with subject `fix(presentation): retain render fence timing evidence`.

### Task 3: Train the worker budget from complete dispatch duration

**Files:**
- Modify: `src/native_output/kms_worker/timing.rs`
- Modify: `src/native_output/kms_worker/thread.rs`
- Test: `src/native_output/kms_worker/timing.rs` tests and existing scripted worker tests

**Interfaces:**
- `KmsWorkerDispatchModel::record` accepts the measured full dispatch duration independently of diagnostic components.

- [ ] **Step 1: Add a failing synthetic timing-model test where full dispatch exceeds reconstructed pre-submit plus ioctl time.
- [ ] **Step 2: Run the timing-model test and confirm the budget is trained from the smaller reconstructed value.
- [ ] **Step 3: Pass `dispatch_duration_ns` from the existing worker measurement into the model while leaving retry/backoff behavior unchanged.
- [ ] **Step 4: Extend scripted worker coverage for success and Busy retry sequences to assert complete dispatch accounting, successful ioctl duration, and retry counts/backoffs.
- [ ] **Step 5: Run focused worker tests, neighboring runtime timing tests, formatting, and inspect the diff.
- [ ] **Step 6: Commit with subject `fix(kms-worker): budget complete dispatch duration`.

### Task 4: Anchor the worker pageflip watchdog

**Files:**
- Modify: `src/native_output/kms_worker/thread.rs`
- Modify: `src/native_output/kms_worker/queue.rs` only if the existing test-access boundary requires it
- Test: `src/native_output/kms_worker/tests.rs` and existing quiesce/shutdown tests

**Interfaces:**
- The watchdog derives its deadline from `WorkerInFlight.submit_returned_at_ns` and emits one timeout for the matching token.

- [ ] **Step 1: Add a failing timeout test that emits repeated legitimate worker notifications while the same inflight token remains pending.
- [ ] **Step 2: Run it and confirm the relative wait is postponed by notifications.
- [ ] **Step 3: Replace the relative one-second loop with remaining-duration waits against the absolute submission deadline; after timeout, wait ordinarily for ack/quiesce.
- [ ] **Step 4: Verify acknowledgement-before-deadline, exactly-once timeout, quiesce, and shutdown behavior.
- [ ] **Step 5: Run focused worker tests, neighboring runtime tests, formatting, and inspect the diff.
- [ ] **Step 6: Commit with subject `fix(kms-worker): anchor pageflip timeout to submission`.

### Final verification

- [ ] Run the complete relevant Cargo test/check suite through the configured target directory using `rtk`.
- [ ] Inspect `rtk git status`, `rtk git diff`, and each commit to verify unrelated changes were preserved.
- [ ] Confirm the four logical commits exist and report any pre-existing warnings/failures separately.
