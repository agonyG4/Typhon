# Typhon DMA-BUF GPU Release Authority v1.1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline execution is required; subagents are prohibited by the task request).

**Goal:** Close the worker-owned Direct Scanout exclusion hole and give deferred composited DMA-BUF release obligations bounded runtime liveness without creating visual render work.

**Architecture:** Compute one runtime `DmabufGpuReleaseSafety` snapshot from Atomic Direct ownership plus the KMS worker's queued, executing, and inflight Direct leases. Pass that snapshot to both normal rendered-frame and NoVisualChange release decisions. Keep deferred obligations in compositor ownership and add capped timer-backed retry debt in the existing native timer domain; retry creates a release-only GPU fence without drawing, while normal rendered-frame failures retain the existing pageflip fallback.

**Tech Stack:** Rust 2024, existing compositor frame-batch ownership, Atomic EGL/GBM, KMS worker state snapshots, Linux epoll/timerfd reactor, deterministic unit tests, `rtk`.

## Global Constraints

- Preserve `DmabufReleaseObligation`, exact release-token equality, GPU lease ownership, independent render-fence FDs, DirectReleaseProof, O1, SHM, regional damage, buffer age, and KMS scheduling.
- Do not release a DMA-BUF from a GL fence while any worker queued/executing/inflight Direct lease or Atomic submitted/presented/suspended Direct ownership exists.
- Deferred retry debt is runtime ownership work, not scene damage; it must not create a render, callback, `wp_presentation`, or KMS commit.
- Normal rendered-frame GPU-fence setup failure retains the existing physical-presentation fallback; retry debt is primarily for NoVisualChange/no-physical-terminal cases.
- No `glFinish`, busy polling, sleeping thread, pointer-reposition edits, scene-graph rewrite, or Direct Scanout redesign.

---

### Task 1: RED coverage for the two review findings

**Files:**
- Modify: `src/native_output/runtime/dmabuf_release.rs`
- Test: existing worker and scheduler test modules where ownership state is produced

- [x] **Step 1: Write failing safety-snapshot tests**

Add tests that construct each worker ownership tuple `(queued, executing, inflight)` and assert the unified safety snapshot blocks GPU release. Add a clearing case for an empty tuple.

- [x] **Step 2: Write failing retry-debt tests**

Add deterministic-clock tests that require deferred release retry deadlines, capped exponential backoff, no visual-work flag, and clearing after a successful arm.

- [x] **Step 3: Run the focused tests**

Run `rtk cargo test --locked dmabuf_release --bin oblivion-one` and the relevant worker/scheduler tests. Expected result is compilation failure because the v1.1 safety and retry interfaces do not yet exist.

### Task 2: Unify Direct/KMS ownership safety

**Files:**
- Modify: `src/native_output/runtime/dmabuf_release.rs`
- Modify: `src/native_output/runtime/presentation_cycle.rs`
- Modify: `src/native_output/runtime/cycle.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Test: KMS worker direct ownership and scheduler tests

- [x] **Step 1: Add the runtime safety snapshot**

Define `DmabufGpuReleaseSafety` and compute it from `AtomicEglGbmScanout::has_live_direct_kms_ownership()` plus `KmsCommitWorkerHandle::direct_content_keys()`. Treat any queued, executing, or inflight worker key as live.

- [x] **Step 2: Thread the snapshot through both entry points**

Replace the partial `has_live_direct_kms_ownership()` checks in normal `arm_composited_dmabuf_release()` and Atomic NoVisualChange release-only fencing with the same snapshot.

- [x] **Step 3: Run the worker overlap tests**

Run the direct lease, scheduler render-ahead, and focused DMA-BUF tests. Confirm the scheduler still returns `RenderAhead` for queued-worker overlap while the release safety snapshot remains live.

### Task 3: Add bounded deferred-release retry debt

**Files:**
- Modify: `src/native_output/runtime/dmabuf_release.rs`
- Modify: `src/native_output/runtime/mod.rs`
- Modify: `src/native_output/runtime/cycle.rs`
- Modify: `src/native_output/runtime/metrics.rs`
- Modify: `src/compositor/state/frames.rs`
- Modify: `src/compositor/server.rs`
- Modify: `src/compositor/server_frames.rs`

- [x] **Step 1: Expose deferred-only count and transfer**

Add compositor methods that distinguish deferred obligations from pending frame-batch obligations and transfer deferred obligations directly into a GPU lease without creating a visual frame batch.

- [x] **Step 2: Implement retry state and capped deadlines**

Add bounded reasons, deterministic `next_retry_deadline_ns`, exponential backoff capped at a documented maximum, and success/failure transitions. Do not mark deferred work as `has_unowned_frame_work()`.

- [x] **Step 3: Service due retry debt**

At the start of a native cycle, re-evaluate the complete Direct/KMS safety snapshot. If safe, create a release-only Atomic render fence, transfer exact deferred obligations, duplicate the independent completion FD, and register it. On failure, requeue and back off. Compatibility remains conservative.

- [x] **Step 4: Include the retry deadline in existing timer arming**

Extend `arm_runtime_deadline()` with the retry deadline only for Atomic deferred work. Timer service must not set redraw, presentation, callback, or KMS state.

- [x] **Step 5: Run liveness tests**

Run deterministic tests for retry without unrelated work, no visual work, persistent failure backoff, Direct worker clearing, registration failure retention, and dead-resource cleanup.

### Task 4: Protocol-boundary and non-regression tests

**Files:**
- Modify: existing compositor/native output protocol and ownership test modules
- Test: explicit sync, SHM, O1, Direct Scanout, cache reuse, and KMS ownership suites

- [ ] **Step 1: Add or strengthen protocol-boundary assertions**

Prove legacy release and explicit release points follow GPU completion independently of pageflip where the existing deterministic infrastructure permits; preserve current-buffer and reattachment protection.

- [x] **Step 2: Verify failure terminals**

Prove failed rendering, registration failure, KMS rejection, worker Direct ownership, and teardown retain or conservatively complete obligations exactly once.

- [x] **Step 3: Re-run unchanged closure suites**

Run O1 callback admission, SHM materialization, regional damage/buffer-age, Direct Scanout, KMS worker scheduling, and DMA-BUF cache tests.

### Task 5: Documentation and verification

**Files:**
- Create: `REPORT-2026-08-29-typhon-dmabuf-gpu-release-authority-v1.md`
- Modify: this plan

- [x] **Step 1: Record source evidence and fallbacks**

Document the worker safety barrier, retry ownership, all unchanged boundaries, and distinguish unit, protocol, and native KMS evidence.

- [x] **Step 2: Run final verification**

  `fmt`, `check`, Clippy, and diff checks pass. The full suite ran with one
  unrelated existing `tests/sigchld.rs::one_child_exit_wakes_the_sigchld_signalfd_once`
  failure; this is recorded in the final report and was not modified.

Run `rtk cargo fmt --check`, `rtk cargo check`, `rtk cargo clippy --all-targets --all-features -- -D warnings`, `rtk cargo test`, `git diff --check`, and `git status --short`. Do not hide unrelated failures.

- [ ] **Step 3: Commit only task files**

Stage the v1.1 source, tests, plan, and report. Preserve concurrent pointer-reposition work and do not stage unrelated changes.
