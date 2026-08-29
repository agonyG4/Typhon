# Typhon DMA-BUF GPU Completion Release Authority v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Release retired composited DMA-BUFs after an asynchronous native GPU
completion proof, while keeping Direct Scanout on its existing KMS authority
and leaving O1, SHM, damage, buffer-age, and KMS scheduling unchanged.

**Architecture:** Retain exact `BufferId` plus protocol token in a
`DmabufReleaseObligation`; transfer obligations from the exact frame batch to
one compositor-owned GPU lease; let native runtime own only the duplicated
sync-file FD and reactor token; complete the lease asynchronously. NoVisualChange
uses a release-only fence when Atomic native-fence support exists and otherwise
requeues to the existing conservative terminal.

**Tech Stack:** Rust 2024, existing Wayland compositor state, `NativeRenderFence`,
Linux epoll reactor, Atomic EGL/GBM, deterministic state-machine tests, `rtk`.

## Global constraints

- Preserve uncommitted concurrent pointer work; stage only task files.
- Do not use subagents, `glFinish`, waits, sleeps, busy polling, or a new scene graph.
- Do not modify O1 admission/callback pacing, SHM release, DMA-BUF acquire,
  Direct Scanout policy/identity, KMS worker scheduling, regional damage,
  buffer-age repair, or resize behavior.
- Every obligation has exactly one owner and one terminal.
- GPU completion never advances physical scene history, output serials,
  pageflip sequence, `wp_presentation`, or logical surface-damage lineage.

## Task 1: Documentation and RED tests

Files: `src/compositor/state/frame_tests.rs`,
`src/egl_renderer/native_fence.rs`,
`src/native_output/runtime/dmabuf_release.rs` (test harness),
`src/native/event_loop.rs`, and focused native/compositor test modules.

- [ ] Add RED assertions that NoVisualChange does not complete a retired DMA-BUF
  without a GPU proof, while preserving physical state.
- [ ] Add RED native-fence tests for an independent non-consuming completion FD.
- [ ] Add RED registry tests for one fence/many obligations, rejected output,
  registration failure, cancellation, and no busy-loop wake behavior.
- [ ] Add RED ownership tests for current-buffer protection, exact-token
  reattachment, distinct explicit points, direct isolation, and failed render.
- [ ] Add the integrated client/output rotation plus topology/SSD oracle test
  with a real rejected candidate retry.
- [ ] Run the narrow focused tests and record the expected failures before
  production changes.

## Task 2: Preserve DMA-BUF identity in compositor ownership

Files: `src/compositor/state_data.rs`, `src/compositor/mod.rs`,
`src/compositor/frame_batch.rs`, `src/compositor/state/surface_commits.rs`,
`src/compositor/state/frames.rs`, `src/compositor/server.rs`,
`src/compositor/server_frames.rs`.

- [ ] Introduce `DmabufReleaseObligation` and `ActiveDmabufBuffer` with
  `BufferId` plus exact `SurfaceBufferRelease`.
- [ ] Change retirement/capture/restore/shutdown paths to move obligations,
  preserving duplicate-token semantics and SHM behavior.
- [ ] Add compositor-owned keyed GPU-lease storage and narrow public transfer,
  requeue, and completion APIs.
- [ ] Make GPU completion re-check active exact-token ownership before release.
- [ ] Run compositor buffer/frame-batch tests and commit this ownership phase.

## Task 3: Independent native completion FD

Files: `src/egl_renderer/native_fence.rs` and its tests.

- [ ] Add a non-consuming completion-FD duplicate using the existing sync-file
  descriptor without consuming submission or timing ownership.
- [ ] Exercise successful duplication and injected duplication failure.
- [ ] Run fence/Atomic scanout tests and commit this phase.

## Task 4: Dedicated asynchronous release registry

Files: `src/native_output/runtime/dmabuf_release.rs`,
`src/native/event_loop.rs`, `src/native_output/runtime/mod.rs`,
`src/native_output/runtime/work_domains.rs`, `src/native_output/runtime/bootstrap.rs`,
`src/native_output/runtime/cycle/pageflip.rs`.

- [ ] Add `NativeEventSource::DmabufGpuRelease`, a wake reason, and token
  collection independent of `OutputRenderFence`.
- [ ] Implement a bounded registry that owns lease IDs, FDs, tokens, and
  exact-once unregister/cancel behavior.
- [ ] Service ready release tokens before ordinary cycle classification without
  scheduling output work; complete exact server leases.
- [ ] Initialize and tear down the registry without synchronous GPU waits.
- [ ] Run native event-loop and registry tests and commit this phase.

## Task 5: Atomic composited integration and NoVisualChange

Files: `src/native_output/scanout/atomic_egl_gbm.rs`,
`src/native_output/scanout/output_swapchain.rs`,
`src/native_output/scanout/atomic_egl_gbm_transactions.rs`,
`src/native_output/runtime/presentation_cycle.rs`,
`src/native_output/runtime/presentation_ready.rs`,
`src/native_output/runtime/presentation_metrics.rs`.

- [ ] Duplicate the ready/rendered Atomic fence before KMS submission consumes
  `submission_fd`.
- [ ] Transfer only eligible composited obligations to one GPU lease per
  rendered frame and register it before output admission.
- [ ] Keep output submission rejection independent; a valid GPU lease survives
  rejected KMS ownership.
- [ ] Create a release-only fence for Atomic NoVisualChange with no draw and
  no physical terminal; requeue safely when unavailable.
- [ ] Leave compatibility EGL and Direct Scanout on conservative existing
  release authorities.
- [ ] Run focused Atomic, pageflip, frame-callback, and NoVisualChange tests;
  commit this integration.

## Task 6: Failure, reuse, and protocol closure

Files: touched ownership/runtime modules plus explicit-sync and cache tests.

- [ ] Close render failure before fence export, duplication/registration failure,
  dead resources, teardown, reattachment, current-buffer, and distinct-point cases.
- [ ] Prove cached EGLImage reuse follows a new valid commit/acquire after release.
- [ ] Re-run Direct Scanout, explicit-sync, SHM, O1, and KMS ownership suites.
- [ ] Commit the focused closure.

## Task 7: Integrated oracle and verification

Files: existing integrated swapchain/oracle test modules and English report.

- [ ] Strengthen the existing oracle with client rotation, output ages 1/2/3,
  topology, SSD overlap, real rejection, retry, and pixel equality.
- [ ] Add bounded release metrics without per-frame verbose logging.
- [ ] Review every required adversarial question against source and tests.
- [ ] Run `rtk cargo fmt --check`, `rtk cargo check`,
  `rtk cargo clippy --all-targets --all-features -- -D warnings`,
  `rtk cargo test`, `git diff --check`, and `git status --short`.
- [ ] Run native qualification only if a real DRM/KMS TTY is available.
- [ ] Write the final report, classify unrelated failures, and make the final
  task commit without staging concurrent pointer work.

