# Typhon VRR Phase 2.2B Implementation Plan

> **For agentic workers:** Execute inline in this session. Do not use subagents. Each task uses test-first changes and keeps build output under `/mnt/Aether/Desktop/GitHub`.

**Goal:** Make KMS reservation ownership and presentation timing domain-neutral while preserving fixed-refresh production behavior and leaving variable presentation inactive.

**Architecture:** Add reservation metadata accessors, carry typed reservation and KMS timing through rendered frames and jobs, validate exact ownership, and branch worker behavior by timing domain. Add a separate planner method for one-step variable pageflip accounting without calling it from runtime.

**Tech Stack:** Rust, Cargo, Typhon native output presentation and KMS worker modules.

## Global Constraints

- Do not alter `KmsSubmitWindow` or fixed-refresh constants and thresholds.
- Do not construct variable reservation or timing values from production code.
- Reject `DeferredO1` from executable KMS jobs until it is bound to a fixed reservation.
- Keep O1 fixed-only and `ReactiveDouble` as the adaptive mode.
- Keep the presentation planner's existing clock generation and sequence as the sole authority.
- Put all Cargo build output in `/mnt/Aether/Desktop/GitHub/Typhon-target`.
- Preserve unrelated worktree edits and commit only Phase 2.2B files.

---

### Task 1: Reservation metadata and classification

**Files:**
- Modify: `src/native_output/presentation/transaction.rs`
- Modify: fixed-target call sites in `src/native_output/scanout/output_swapchain.rs`, `src/native_output/runtime/presentation_pipeline.rs`, `src/native_output/kms_worker/bundle.rs`, `src/native_output/presentation/ledger.rs`, and `src/native_output/scanout/atomic_egl_gbm.rs`
- Test: `src/native_output/presentation/transaction.rs`

- [ ] Add failing tests for `Fixed`, `Variable`, and `DeferredO1` metadata, and prove fixed-target absence is not used as deferred-O1 identity.
- [ ] Add `reason`, `opportunity`, `variable`, and explicit `is_deferred_o1` accessors; retain `fixed_target` and fixed-only compatibility methods where justified.
- [ ] Replace each ambiguous production optional-target classification with exact reservation matching or explicit deferred-O1 checks.
- [ ] Run the transaction-focused test filter and confirm existing fixed and O1 cases remain unchanged.

### Task 2: Typed timing through rendered frames

**Files:**
- Modify: `src/native_output/presentation/kms_timing.rs`
- Modify: `src/native_output/scanout/output_swapchain.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Test: `src/native_output/presentation/kms_timing.rs`, `src/native_output/scanout/output_swapchain.rs`

- [ ] Add failing tests for fixed timing accessors and variable earliest/preferred metadata with no fixed deadline.
- [ ] Add `KmsPresentationTiming::{Fixed, Variable}` without changing `KmsSubmitWindow`.
- [ ] Replace `RenderedOutputFrame.submit_window` storage with explicit typed KMS timing; wrap every current constructed window in `Fixed` and preserve its exact value.
- [ ] Update fixed-only frame operations to request `fixed_submit_window` or explicitly match the reservation.
- [ ] Verify fixed window wake, deadline, dispatch budget, and apply guard are byte-for-byte equal through the typed wrapper.

### Task 3: Domain-neutral KMS ownership

**Files:**
- Modify: `src/native_output/kms_worker/payload.rs`
- Modify: `src/native_output/kms_worker/bundle.rs`
- Modify: `src/native_output/kms_worker/queue.rs`
- Modify: `src/native_output/runtime/kms_worker.rs`
- Modify: fixed-only accounting reads in `src/native_output/runtime/cycle/pageflip.rs`
- Modify: `src/native_output/runtime/plane_cycle.rs`
- Modify: KMS worker test helpers and test call sites in `src/native_output/kms_worker/`
- Test: `src/native_output/kms_worker/payload.rs` and existing KMS worker tests

- [ ] Add failing tests that fixed and variable jobs validate only against exact matching owner reservations, and deferred O1 cannot be executable.
- [ ] Replace job target/window fields with one reservation and one typed timing value, with narrowly scoped fixed-only accessors.
- [ ] Validate transaction and owner reservations by exact equality and require the job timing variant to match the reservation domain.
- [ ] Update all production producers to construct only `Fixed(target)` plus `Fixed(existing_window)`.
- [ ] Update test builders and fixed-only queue/pageflip plumbing to use explicit fixed accessors.

### Task 4: Worker timing semantics

**Files:**
- Modify: `src/native_output/kms_worker/thread.rs`
- Modify: `src/native_output/kms_worker/payload.rs`
- Modify: `src/native_output/kms_worker/queue.rs`
- Test: `src/native_output/kms_worker/worker_pacing_tests.rs`, `src/native_output/kms_worker/timing.rs`

- [ ] Add failing tests for variable eligibility before and at `earliest_submit_ns`, no preferred-time miss, and no fixed deadline-tail observation.
- [ ] Preserve the fixed worker wake, wait-arm decision, deadline, dispatch fairness, adaptive model observation, and telemetry arguments exactly.
- [ ] For variable timing, wait only until its earliest-submit bound, skip fixed deadline/apply-guard/unreachable classification and tail observation, and retain generic timing telemetry.
- [ ] Run the focused worker pacing tests and compare fixed timing values with the pre-refactor expectations.

### Task 5: Phase-free variable pageflip preparation

**Files:**
- Modify: `src/native/presentation_deadline.rs`
- Test: `src/native/presentation_deadline.rs`

- [ ] Add failing tests for a 9.4 ms interval at 165 Hz and a 100 ms idle interval, expecting exactly one sequence increment for each variable presentation.
- [ ] Add `prepare_variable_presented` using the existing planner generation, sequence, and timestamp authority, without changing `prepare_presented` or pageflip runtime.
- [ ] Run fixed planner regressions to confirm fixed interval quantization and generation behavior are unchanged.

### Task 6: Validation and isolated commits

**Files:** all Phase 2.2B files only

- [ ] Run `cargo fmt --check` and the focused filters `presentation_timing`, `vrr_window`, `presentation_transactions`, `kms_timing`, `kms_worker`, and `buffering`, with the effective target directory verified under `/mnt/Aether/Desktop/GitHub` before compiling.
- [ ] Run `cargo test` and `cargo clippy --all-targets` using the same verified target directory.
- [ ] Run `git diff --check`; inspect production source and confirm variable reservation and timing constructor counts are zero.
- [ ] Review the twelve adversarial questions from the Phase 2.2B request, fix any violations, and report any unrelated baseline failures accurately.
- [ ] Commit the design/plan and the isolated implementation delta on `main`; do not stage unrelated worktree files.
