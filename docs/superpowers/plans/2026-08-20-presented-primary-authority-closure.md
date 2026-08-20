# Presented-Primary Authority Closure Implementation Plan

> **For agentic workers:** This plan is executed inline in the current checkout. Preserve all pre-existing worktree changes.

**Goal:** Make a physically presented Typhon primary independent of bounded transaction-history retention while preserving exact pageflip and resource-ownership validation.

**Architecture:** `PresentedPlaneSnapshot.primary` becomes the one persistent presented-primary identity. It stores immutable provenance and physical identity; `AtomicOutputSwapchain` or `DirectPrimaryOwnership.presented` owns the underlying resource. Active/queued/prepared transitions continue to use `OutputTransactionLedger`, while terminal history remains bounded diagnostics only.

**Tech Stack:** Rust 2024, Cargo, native-output unit/model tests, KMS worker validation models.

## Global Constraints

- Keep `DEFAULT_OUTPUT_TRANSACTION_HISTORY_CAPACITY` at 512 and allow tiny test capacities to evict records.
- Do not pin terminal records, widen history, downgrade physical mismatches, or add hot-path history scans.
- Preserve exact pageflip token, bundle, output-generation, and CRTC validation.
- Preserve worker/non-worker parity, direct lease retirement, swapchain ownership, cursor promotion, scene history, and presentation-domain buffer age.
- Do not reset, clean, restore, or overwrite unrelated dirty worktree changes.

## Task 1: Establish the failing composed eviction regression

**Files:**
- Modify: `src/native_output/runtime/presentation_pipeline.rs` tests

- [ ] Construct a real current composed swapchain slot and presented identity.
- [ ] Evict the origin transaction with a tiny terminal ledger capacity.
- [ ] Assert the origin is absent from `transaction_including_terminal` and the pipeline still validates; verify this test fails on the current history-dependent validator.

## Task 2: Refactor the presented identity

**Files:**
- Modify: `src/native_output/presentation/pipeline.rs`
- Modify: `src/native_output/presentation/plane.rs`
- Modify: `src/native_output/runtime/presentation_pipeline.rs`

- [ ] Replace transaction-dependent `ConfirmedPrimaryState` fields with immutable presented provenance and physical identity, including output generation and composited slot/generation/serial/framebuffer data.
- [ ] Make `PresentedPlaneSnapshot.primary` the only persistent presented-primary field and remove the duplicate `current_primary` snapshot field.
- [ ] Validate composed identity against `AtomicOutputSwapchain.current`, current framebuffer, pool generation, and presentation serial.
- [ ] Validate direct identity through `DirectPrimaryOwnership.presented` without consulting terminal history.
- [ ] Keep active transition validation on `record_for_active_owner` unchanged.

## Task 3: Move pageflip promotion to physical completion

**Files:**
- Modify: `src/native_output/runtime/cycle/pageflip.rs`
- Modify: `src/native_output/runtime/cycle_direct.rs`
- Modify: `src/native_output/runtime/presentation.rs`
- Modify: `src/native_output/runtime/presentation_worker.rs`
- Modify: `src/native_output/runtime/mod.rs`
- Modify: `src/native_output/runtime/session_io.rs`

- [ ] Construct composed presented identity from actual swapchain completion.
- [ ] Construct direct presented identity from the promoted direct lease.
- [ ] Preserve rollback ordering for direct-to-composed transitions and cursor/primary bundle promotion.
- [ ] Remove the long-lived runtime compatibility mirror and update scheduler, cursor, metrics, diagnostics, and recovery consumers to read `presented_planes.primary`.
- [ ] Ensure worker promotion uses immutable job/resource state and establishes the same presented snapshot revision/base.

## Task 4: Add ownership-boundary regressions

**Files:**
- Modify: `src/native_output/runtime/presentation_pipeline.rs`
- Modify: `src/native_output/scanout/atomic_direct_tests.rs`
- Modify: `src/native_output/tests/triple_buffering_model.rs`
- Modify: `src/native_output/tests/plane_scheduling_model.rs`
- Modify: `src/native_output/tests/presentation_transactions.rs`
- Modify: relevant worker/pageflip tests

- [ ] Add composed, cursor/plane-delta churn, direct eviction, worker, and non-worker history-independence tests.
- [ ] Add composed slot/framebuffer/pool-generation/serial/generation negative tests.
- [ ] Add direct missing-owner/key/framebuffer/surface/token/generation negative tests.
- [ ] Cover Direct→Composed, Composed→Direct, Direct→Direct, suspend/resume, and stale pageflip cases without early lease release.

## Task 5: Verify and document

**Files:**
- Create: `REPORT-2026-08-20-presented-primary-authority-closure.md`

- [ ] Run focused tests and the required Cargo/layout/diff checks using existing build caches.
- [ ] Run the broadest practical test suite and classify pre-existing blockers separately.
- [ ] Record native qualification status without claiming Sober validation unless it is actually run.
- [ ] Record final task-owned paths and final git status.
