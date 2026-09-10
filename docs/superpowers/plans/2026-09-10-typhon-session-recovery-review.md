# Typhon Session Recovery Review Fixes Implementation Plan

> **Execution note:** Work is being performed inline in the existing repository; subagents and worktrees are prohibited. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Close the asynchronous resume seat race, preserve completion proofs on failed worker ownership transfers, and make retained XWayland diagnostics byte-bounded without changing the established recovery mechanisms.

**Architecture:** A `Resuming + Disabled` event becomes an explicit abort-to-suspended transition. Runtime cleanup unregisters only the recovery watch, discards the prepared recovery record, re-parks cursor state, and acknowledges the disable while leaving unsafe output ownership untouched. Fence transfer APIs use mutable optional owners so validation failures leave the FD with its original job/emergency record; successful transfers consume it exactly once. Lifecycle trace retention exposes bounded accounting for tests and skips the unlimited verbose rendering path when tracing is disabled.

**Tech Stack:** Rust, libseat/session lifecycle, event-driven reactor, explicit Atomic EGL/GBM swapchain, KMS worker, deterministic unit/model tests, Cargo.

## Global Constraints

- Reuse the existing build directory and target artifacts.
- Do not create another build directory, target directory, or git worktree.
- Preserve unrelated working-tree changes.
- Use TDD: add and run a failing regression before each production behavior change.
- Do not block, poll, sleep, weaken ownership invariants, disable KMS/worker/triple-buffering/explicit-sync, or retain dead Wayland resources.
- Do not change Incident B/XWayland behavior beyond the requested diagnostic bounding and disabled-trace efficiency fixes.

---

### Task 1: Abort asynchronous resume when a second seat disable arrives

**Files:**
- Modify: `src/native_output/runtime/session.rs`
- Modify: `src/native_output/runtime/session_io.rs`
- Modify: `src/native_output/runtime/cycle.rs`
- Modify: `src/native_output/runtime/cycle/pageflip.rs`
- Test: the unit tests in those modules

**Interfaces:**
- Add an explicit `NativeSessionTransition` for `Resuming + Disabled` and a lifecycle completion method that returns `Suspended` only after runtime abort cleanup and disable acknowledgment.
- Add a `NativeSessionIo` recovery-abort operation used by the runtime to unregister the recovery fence, discard the prepared recovery generation, and re-suspend atomic cursor state without touching unsafe scanout ownership.
- Return whether seat dispatch observed a disable so pageflip processing suppresses the same-batch recovery continuation.

- [x] **Step 1: Write the failing lifecycle and event-batch tests.** Assert `Resuming + Disabled` is not silently `None`, and assert a disable-observed flag suppresses a resuming recovery wake.
- [x] **Step 2: Run the targeted session tests and verify they fail for the missing transition/suppression.**
- [x] **Step 3: Implement the explicit abort transition, runtime cleanup, exactly-once acknowledgment, and seat-wins wake ordering.** Keep suspended/pending ownership and the presented framebuffer intact; do not restart worker/DRM/scheduler/input.
- [x] **Step 4: Add and run runtime/session recorder tests for one and two pending fences, stale/duplicate wakes, fresh recovery after abort, and fatal query errors.**
- [x] **Step 5: Run the relevant session lifecycle and suspend/recovery suites.**

### Task 2: Make worker completion-proof transfers transactional

**Files:**
- Modify: `src/egl_renderer/native_fence.rs`
- Modify: `src/native_output/scanout/output_swapchain.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm/worker.rs`
- Modify: `src/native_output/scanout/worker.rs`
- Modify: `src/native_output/runtime/kms_worker.rs`
- Modify: `src/native_output/runtime/kms_worker/rejection.rs`
- Modify: `src/native_output/runtime/kms_worker_teardown.rs`
- Test: `src/native_output/tests/scanout.rs` and existing worker tests

**Interfaces:**
- Transfer methods accept mutable optional FD owners, consuming them only after all fallible state/token checks pass.
- Pre-submit restoration puts an unconsumed FD back in the returned job on failure and retains the complete job in the appropriate quarantine/emergency owner.
- Submitted fatal/quarantine handling retains `KmsSubmittedOwnership` with its out-fence intact when scanout transfer fails.

- [x] **Step 1: Add red tests for wrong token, missing queued ownership, and invalid suspend state while a completion/input FD is owned by the caller/job.** Assert the FD remains open and associated with exactly one owner.
- [x] **Step 2: Run the targeted scanout/worker tests and verify the old by-value APIs drop the FD on failure.**
- [x] **Step 3: Change the NativeRenderFence, swapchain, backend, submitted-ownership, rejection, and teardown transfer paths to transactional mutable-owner APIs.** Audit every pre-submit returned-job path, including teardown event drains and fatal-job deferral.
- [x] **Step 4: Run the targeted tests, including successful timing-FD duplication and forced `EMFILE` paths.**
- [x] **Step 5: Run output swapchain, KMS worker, presentation, and pageflip/recovery suites.**

### Task 3: Exercise and preserve the XWayland diagnostic byte budget

**Files:**
- Modify: `src/xwayland/trace.rs`
- Test: `src/xwayland/trace.rs` unit tests

**Interfaces:**
- Add a test-only retained-byte accessor; `take_recent_lifecycle_trace()` must reset both records and bytes.
- Keep full `render_line` output unchanged when verbose tracing is enabled, while disabled tracing renders only the bounded lifecycle line.

- [x] **Step 1: Expand the pathological-text regression to exceed 512 KiB and assert aggregate bytes, eviction, newest-record retention, record count, and reset.**
- [x] **Step 2: Run the trace tests and verify the strengthened assertions expose the old test’s missing aggregate-budget coverage or fail compilation for the accessor.**
- [x] **Step 3: Move full-line construction behind the enabled-trace branch; retain the bounded line independently for lifecycle events.**
- [x] **Step 4: Run XWayland trace tests and the relevant lifecycle suite.**

### Task 4: Full verification and focused integration

**Files:**
- Review all changed files and call sites; no unrelated source changes.

- [x] **Step 1: Run `cargo fmt --check` through the repository convention.**
- [x] **Step 2: Run locked `cargo check --all-targets`, locked clippy with warnings denied, locked tests, and `./bin/check-source-layout` using the existing target.**
- [x] **Step 3: Inspect the final diff and status, stage only task files, and create one focused commit if safe.**
- [x] **Step 4: Report any unrelated existing failures separately and provide exact hardware qualification commands; do not claim hardware completion without repeated Typhon → Hyprland → Typhon testing under active GPU/KMS-worker load.**
