# Typhon Explicit-Release Signal Ownership Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve every explicit-release obligation across failed DRM timeline signals, retry only the already-safe signal operation, and keep exact-token ownership auditable through runtime, idle wakeups, and shutdown.

**Architecture:** `ExplicitSyncPoint::signal` will return the DRM result. `CompositorState` will own a separate signal-only retry vector of complete `DmabufReleaseObligation` values, while the native DMA-BUF registry will provide the shared bounded retry deadline. A retry will revalidate exact-token activity before signaling; active tokens return to proof-required deferred ownership, inactive tokens retry directly without a new fence.

**Tech Stack:** Rust, Cargo, compositor frame-batch ownership, native event-loop wake deadlines, DRM syncobj timelines, existing unit/state/native integration tests.

## Global Constraints

- Work in `/home/agony/GitHub/Typhon` and reuse its existing Cargo `target` directory.
- Use `rtk` for shell searches, reads, builds, tests, and checks where applicable.
- Do not use subagents or create another checkout/build tree/target directory.
- Do not modify unrelated dirty work.
- Preserve SHM release behavior, presentation lineage, explicit-sync commit capture, protocol-proxy lifetime behavior, Direct Scanout policy, KMS worker behavior, and rendering policy.
- Commit the focused result with `fix(compositor): retain failed explicit release signals`.

### Task 1: Confirm the release authority graph and record the RED test seam

**Files:**
- Inspect: `src/compositor/explicit_sync.rs`, `src/compositor/state_data.rs`, `src/compositor/state/frames.rs`, `src/native_output/runtime/dmabuf_release.rs`, `src/native_output/runtime/metrics.rs`, `src/native_output/runtime/wake_plan.rs`, `src/compositor/state/shutdown.rs`.
- Test: `src/compositor/state/frame_tests.rs`, `src/native_output/runtime/dmabuf_release.rs` tests.

- [ ] **Step 1: Trace every explicit-release completion caller.**

Run the required `rg` searches for `release_target().release()`, `point.signal()`, release containers, and shutdown ownership. Use the graph callers for `ExplicitSyncPoint::signal`, `PendingSurfaceBuffer::release_target`, and `complete_dmabuf_release_if_inactive`, then read each reported source range directly.

- [ ] **Step 2: Add the lowest-level deterministic failure test before production edits.**

Extend the test-only explicit point seam so a point can be scripted to fail, and add a test asserting the current desired contract returns `Err` from `signal()`. Run the test before changing `signal`; record the expected compile/API failure or assertion failure as RED.

- [ ] **Step 3: Verify the RED failure is caused by discarded signal errors.**

Run the narrow test with `rtk cargo test --lib <exact-test-filter> -- --nocapture`; do not change production behavior until the failure demonstrates that the current `signal()` returns `()` or otherwise hides the injected error.

### Task 2: Make explicit signal completion fallible and ownership-preserving

**Files:**
- Modify: `src/compositor/explicit_sync.rs`, `src/compositor/state_data.rs`.
- Test: `src/compositor/state/frame_tests.rs`.

- [ ] **Step 1: Make `ExplicitSyncPoint::signal` return `io::Result<()>`.**

Keep `DrmSyncobjTimeline` unchanged and return `self.timeline.signal_point(self.point)` directly. Make `for_tests` default to a deterministic successful point and add an explicit failure/script helper under `cfg(test)` without changing production ioctl behavior.

- [ ] **Step 2: Replace the infallible release API with an explicit outcome.**

Implement a `SurfaceBufferRelease` completion method that returns a terminal/failed outcome: dead or failed `wl_buffer.release` remains discarded terminal behavior; explicit-sync signal failure returns the unchanged `DmabufReleaseObligation` for compositor-owned retry. Remove normal-runtime uses of an infallible explicit release sink.

- [ ] **Step 3: Run the low-level signal and legacy release tests.**

Confirm successful test points complete, injected errors remain observable, SHM/legacy Wayland release behavior is unchanged, and no explicit obligation is dropped.

### Task 3: Add compositor-owned signal-only retry and exact-token transfer

**Files:**
- Modify: `src/compositor/mod.rs`, `src/compositor/state/frames.rs`, `src/compositor/state_data.rs`, `src/compositor/state/surface_commits.rs`, `src/compositor/state/subsurfaces.rs`, `src/compositor/state/surfaces.rs`, `src/compositor/state/shutdown.rs`.
- Test: `src/compositor/state/frame_tests.rs`, relevant state tests.

- [ ] **Step 1: Add a distinct `explicit_release_signal_retries` owner.**

Store complete `DmabufReleaseObligation` values. Add count/accessor helpers separate from proof-required `pending`/`deferred` counts, and include this owner in exact-token duplicate accounting and dead-resource scrubbing without adding it to visual-work classification.

- [ ] **Step 2: Centralize fallible release completion.**

Update `complete_dmabuf_release`/`complete_dmabuf_release_if_inactive` and every direct safe-release path so successful explicit signaling increments completion metrics only after `Ok(())`; failed explicit signaling queues exactly the same obligation once and increments failure/retry metrics. SHM and dead Wayland resources retain existing semantics.

- [ ] **Step 3: Revalidate exact-token activity before retry.**

When retrying, if the exact token is active again, move the obligation back into proof-required deferred ownership and do not signal it. Otherwise attempt the same exact point directly. Ensure no path contains both owners simultaneously and that a different point on the same buffer remains independent.

- [ ] **Step 4: Include signal-only ownership in shutdown collection.**

Move signal retry obligations into `ShutdownDmabufReleaseSet` exactly once. Final shutdown makes one bounded signal attempt, records failure without successful-completion accounting, and remains idempotent.

- [ ] **Step 5: Run direct/supersede/token/shutdown state regressions.**

Add and run tests for direct safe-release failure, repeated `Err, Err, Err, Ok`, mixed GPU lease outcomes, exact-token reactivation, duplicate ownership, legacy `wl_buffer`, and shutdown deduplication.

### Task 4: Integrate signal-only retry with the existing native deadline authority

**Files:**
- Modify: `src/native_output/runtime/dmabuf_release.rs`, `src/native_output/runtime/metrics.rs`, `src/native_output/runtime/wake_plan.rs`, `src/native_output/runtime/cycle.rs`, `src/native_output/runtime/presentation_cycle.rs` only where existing retry completion is routed.
- Test: `src/native_output/runtime/dmabuf_release.rs` tests and wake-plan tests.

- [ ] **Step 1: Add signal-failure retry reason and metrics.**

Extend the existing bounded backoff with `ExplicitReleaseSignalFailed` and add metrics for signal failures, attempts, retry completions, and requeue-to-proof-required transitions. Keep lease/fence metrics distinct from protocol obligation metrics.

- [ ] **Step 2: Reconcile both retry classes.**

Make retry scheduling remain pending when either signal-only debt or proof-required deferred debt exists. `update_retry_for_deferred_work` must not clear signal-only debt, and the shared deadline must be armed even with no visual work or Atomic GPU backend.

- [ ] **Step 3: Service signal-only debt first.**

At a due deadline, ask the compositor for signal-only obligations and retry them directly before ordinary GPU-proof work. Do not allocate a lease, duplicate a fence FD, create a render fence, submit output work, or require AtomicEglGbm for this branch. Then service ordinary deferred work using the current path.

- [ ] **Step 4: Run idle/no-extra-GPU regressions.**

Verify signal-only debt keeps a deadline, does not set `has_unowned_frame_work`, does not create visual/GPU work, succeeds without a new lease/fence/watch, and does not starve ordinary deferred retry debt.

### Task 5: Verify, audit, and commit

**Files:**
- Verify all changed source and test files only; leave unrelated dirty files untouched.

- [ ] **Step 1: Run focused tests and inspect the exact owner counts/metrics.**

- [ ] **Step 2: Run `rtk cargo fmt --check`, `rtk cargo check --all-targets`, `rtk cargo clippy --all-targets -- -D warnings`, `rtk cargo test --all-targets`, `./bin/check-source-layout`, and `git diff --check`.**

- [ ] **Step 3: Run final searches for silent signal sinks and classify every remaining release call.**

- [ ] **Step 4: Call `check_index_coverage` for every operated source/test path and disclose any parse-partial limitations.**

- [ ] **Step 5: Stage only task files, commit with the required message, and report the commit hash, RED/GREEN evidence, tests, verification limitations, and hardware qualification without claiming induced DRM signal failure.**
