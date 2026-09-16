# Phase 1A Logical Output Identity Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove implicit logical-output qualification from production transactions and make exact KMS pageflip acknowledgement reject cross-output evidence without changing presentation behavior.

**Architecture:** `OutputTransaction` constructors receive the owning `OutputId` and pass it directly to the private builder; all runtime owners use their output-bound ledger or scanout identity. `KmsCommitWorkerHandle::ack_pageflip_identity` validates `OutputId` before generation/bundle checks or mutation, while `OutputTransactionLedger` remains a second-line ownership check. Focused test modules cover wrong-output ACK preservation and recovery identity validation.

**Tech Stack:** Rust, Cargo, Codebase Memory MCP, `rtk`, existing unit/integration test modules, repository source-layout gate.

## Global Constraints

- Preserve all rendering, scheduling, pacing, KMS policy, cursor policy, Direct Scanout, animation, layout, and visual behavior.
- Do not add `SceneNodeId`, generic scene metadata, multi-output product behavior, or Presentation Engine v2.
- Compile/test in `/home/agony/GitHub/Typhon` so build artifacts stay in the same folder.
- Do not use sub-agents, stash/reset/discard unrelated work, amend commits, or stage the whole repository.
- Preserve the eight pre-existing modified files and untracked `.codebase-memory/`; stage only files owned by this task.
- Starting source-layout output is 55 diagnostics; final output must be no greater and must introduce no new violating file.

---

### Task 1: Add and prove the wrong-output KMS ACK regression

**Files:**
- Create: `src/native_output/kms_worker/output_identity_tests.rs`
- Modify: `src/native_output/kms_worker/mod.rs` to register the focused test module under `#[cfg(test)]`
- Read only: `src/native_output/kms_worker/bundle.rs`, `src/native_output/kms_worker/queue.rs`, `src/native_output/kms_worker/thread.rs`, `src/native_output/kms_worker/tests.rs`

**Interfaces:**
- Consumes: existing `KmsCommitWorkerHandle`, `KmsCommitBundleIdentity`, `KmsCommitJob`, test worker transport, and exact `ack_pageflip_identity` API.
- Produces: a focused regression that expects `Err(KmsWorkerAckError::OutputMismatch)` for an ACK identity differing only in `OutputId`, then proves the exact correct identity still ACKs the same inflight commit.

- [ ] **Step 1: Write the failing test**

Build one inflight bundle using the existing test helpers or a focused local helper. Clone its identity and replace only `output_id` with a second nonzero logical ID. Assert the wrong ACK returns `OutputMismatch`, the inflight snapshot remains present, the established base is unchanged, queued dependents remain queued, and the correct identity subsequently succeeds.

- [ ] **Step 2: Run the focused test to verify it fails for the missing enum/branch**

Run:

```bash
rtk cargo test --locked native_output::kms_worker::output_identity_tests -- --nocapture
```

Expected: compilation or assertion failure because `KmsWorkerAckError::OutputMismatch` and the early validation branch do not yet exist.

- [ ] **Step 3: Commit the red regression**

```bash
git add src/native_output/kms_worker/output_identity_tests.rs src/native_output/kms_worker/mod.rs
git commit -m "test(identity): cover wrong-output KMS pageflip acknowledgement"
```

### Task 2: Reject cross-output KMS ACKs without invalidation

**Files:**
- Modify: `src/native_output/kms_worker/thread.rs:138-146` to add `KmsWorkerAckError::OutputMismatch`
- Modify: `src/native_output/kms_worker/validation.rs:130-263` to compare `inflight.bundle.output_id` and `identity.output_id` before any mutation or invalidation
- Modify: `src/native_output/kms_worker/output_identity_tests.rs` only if the minimal setup needs correction

**Interfaces:**
- Consumes: the Task 1 failing regression and existing `result_mismatches` metric.
- Produces: exact ACK behavior where `OutputMismatch` increments `result_mismatches`, preserves inflight/cursor/queue/validation-base state, does not call `invalidate_queued_dependents`, and permits a later exact ACK.

- [ ] **Step 1: Add the minimal early branch**

Immediately after confirming `state.inflight` exists and before token/generation/bundle checks that can invalidate dependents, add:

```rust
if inflight.bundle.output_id != identity.output_id {
    self.shared
        .metrics
        .result_mismatches
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    return Err(KmsWorkerAckError::OutputMismatch);
}
```

- [ ] **Step 2: Run the focused regression to verify it passes**

Run:

```bash
rtk cargo test --locked native_output::kms_worker::output_identity_tests -- --nocapture
```

Expected: PASS, including preservation assertions and the later exact ACK.

- [ ] **Step 3: Run existing ACK/queue tests**

Run:

```bash
rtk cargo test --locked native_output::kms_worker::tests -- --nocapture
rtk cargo test --locked native_output::kms_worker::task4_tests -- --nocapture
```

Expected: existing mismatch invalidation tests retain their old behavior for generation/bundle/CRTC mismatches; only output routing mismatch avoids invalidation.

- [ ] **Step 4: Commit the KMS fix**

```bash
git add src/native_output/kms_worker/thread.rs src/native_output/kms_worker/validation.rs src/native_output/kms_worker/output_identity_tests.rs
git commit -m "fix(identity): reject cross-output KMS pageflip acknowledgements"
```

### Task 3: Make every transaction constructor output-explicit

**Files:**
- Modify: `src/native_output/presentation/transaction.rs` constructors `composited`, `composited_with_direct_equivalence`, `composited_deferred_o1`, `direct`, `compatibility_composited`, `compatibility_immediate`, `cursor_plane_delta`, `plane_delta`, and private `build`
- Modify: `src/native_output/presentation/ledger.rs` to keep `new_for_output`/`for_output` production paths and make `new`, `with_capacities`, and their `OutputId(1)` helper test-only where production usage is absent
- Modify: production callsites in `src/native_output/runtime/presentation_transactions.rs`, `src/native_output/runtime/plane_cycle.rs`, `src/native_output/runtime/presentation_ready.rs`, `src/native_output/runtime/presentation_pipeline.rs`, `src/native_output/runtime/dmabuf_release.rs`, `src/native_output/scanout/atomic_egl_gbm.rs`, `src/native_output/scanout/atomic_egl_gbm/direct.rs`, `src/native_output/runtime/kms_worker/rejection.rs`
- Modify: affected test callsites in `src/native_output/kms_worker/direct_lease_tests.rs`, `src/native_output/kms_worker/task4_tests.rs`, `src/native_output/runtime/cycle/pageflip_tests.rs`, `src/native_output/runtime/plane_cycle_tests.rs`, `src/native_output/runtime/presentation/pacing_mode_tests.rs`, `src/native_output/runtime/presentation_pipeline_direct_tests.rs`, `src/native_output/tests/direct_scanout_stage4.rs`, and `src/native_output/tests/presentation_transactions.rs`

**Interfaces:**
- Consumes: `OutputId` from the owning ledger (`output_transactions.output_id()`), scanout/direct state (`self.direct.output_id` or equivalent), and existing test ledger identities.
- Produces: no `OutputTransaction` can be constructed without an exact `OutputId`; `.with_output_id(...)` is removed; ledger mismatch rejection remains unchanged.

- [ ] **Step 1: Add/adapt the transaction identity regression first**

In `src/native_output/tests/presentation_transactions.rs`, construct output B directly through `OutputTransaction::composited(output_b, ...)`, assert `descriptor.output_id() == output_b`, insert into ledger B successfully, and assert insertion into ledger A returns `OutputTransactionError::OutputMismatch`. Remove the `.with_output_id()` repair from this test.

- [ ] **Step 2: Run the transaction regression against the old API**

Run:

```bash
rtk cargo test --locked native_output::tests::presentation_transactions::transaction_from_one_logical_output_cannot_enter_another_ledger -- --nocapture
```

Expected: FAIL to compile because the current constructor does not accept the required output argument and the old test still relies on post-construction repair.

- [ ] **Step 3: Change constructors and remove the repair API**

Add `output_id: OutputId` to every listed public constructor and to `build`; thread it through the internal delegation. Store the supplied value directly and delete `with_output_id`. Do not alter validation, content, planes, synchronization, obligations, release plans, or presentation fields.

- [ ] **Step 4: Migrate production owners**

Pass `output_transactions.output_id()` in compatibility and cursor transaction builders and plane-cycle paths. Pass the already-owned scanout/direct `OutputId` in compositor and Direct Scanout paths. Keep the dirty `presentation_cycle.rs` estimator change untouched because it is not a constructor callsite.

- [ ] **Step 5: Migrate tests without broad helper cleanup**

Pass each test ledger’s `output_id()` when a helper receives a ledger; use explicit synthetic IDs only where a test is constructing a standalone identity. Keep unrelated `OutputId::from_raw(1)` fixtures unchanged when they are test-only and do not create a production ownership default.

- [ ] **Step 6: Classify and constrain ledger defaults**

Search production source for `OutputId::from_raw(1)`, `default_output_id`, and `with_output_id`. Keep only test-only helpers or documented non-identity fixtures. If `OutputTransactionLedger::new`/`with_capacities` are test-only, add `#[cfg(test)]`; otherwise migrate production callers to `new_for_output(output_id)` and `for_output(output_id, ...)`.

- [ ] **Step 7: Run focused transaction/presentation suites**

Run:

```bash
rtk cargo test --locked native_output::tests::presentation_transactions -- --nocapture
rtk cargo test --locked native_output::runtime::presentation_transactions -- --nocapture
rtk cargo test --locked native_output::runtime::plane_cycle -- --nocapture
rtk cargo test --locked native_output::runtime::presentation_pipeline -- --nocapture
rtk cargo test --locked native_output::tests::direct_scanout_stage4 -- --nocapture
```

Expected: existing compatibility, cursor, Direct Scanout, pageflip, and ledger tests pass with unchanged behavior.

- [ ] **Step 8: Commit the constructor migration**

```bash
git add src/native_output/presentation/transaction.rs src/native_output/presentation/ledger.rs src/native_output/runtime/presentation_transactions.rs src/native_output/runtime/plane_cycle.rs src/native_output/runtime/presentation_ready.rs src/native_output/runtime/presentation_pipeline.rs src/native_output/runtime/dmabuf_release.rs src/native_output/scanout/atomic_egl_gbm.rs src/native_output/scanout/atomic_egl_gbm/direct.rs src/native_output/runtime/kms_worker/rejection.rs src/native_output/kms_worker/direct_lease_tests.rs src/native_output/kms_worker/task4_tests.rs src/native_output/runtime/cycle/pageflip_tests.rs src/native_output/runtime/plane_cycle_tests.rs src/native_output/runtime/presentation/pacing_mode_tests.rs src/native_output/runtime/presentation_pipeline_direct_tests.rs src/native_output/tests/direct_scanout_stage4.rs src/native_output/tests/presentation_transactions.rs
git commit -m "refactor(identity): require OutputId at transaction construction"
```

### Task 4: Strengthen session recovery identity validation

**Files:**
- Modify: `src/native_output/runtime/session_io.rs` at the pending-recovery validation seam and its test module

**Interfaces:**
- Consumes: current runtime `OutputId`, pending recovery `OutputId`, and existing DRM/backend generation fields.
- Produces: production validation that rejects a pending recovery for another logical output before rebind, while retaining the same logical ID across generation 41 → 42.

- [ ] **Step 1: Write the contract tests**

Strengthen the existing test to assert `OutputId(7)` survives old generation 41 to new generation 42, and add a pure seam-level test asserting current ID 7/pending ID 8 returns the same production validation error before rebind.

- [ ] **Step 2: Run the focused session tests to verify the new rejection test fails**

Run:

```bash
rtk cargo test --locked native_output::runtime::session_io -- --nocapture
```

Expected: the new wrong-output validation assertion fails until the shared production helper is added and used.

- [ ] **Step 3: Implement the shared pure validation helper and use it in recovery**

Add a small helper that compares current and pending `OutputId`, returns the existing recovery error type or a focused typed error, and invoke it at the current validation point without changing recovery ordering or rebinding behavior.

- [ ] **Step 4: Run the session suite and commit**

```bash
rtk cargo test --locked native_output::runtime::session_io -- --nocapture
git add src/native_output/runtime/session_io.rs
git commit -m "test(identity): strengthen session recovery identity contract"
```

Expected: PASS for same-ID generation change and wrong-ID rejection.

### Task 5: Update stale identity documentation

**Files:**
- Modify: `docs/superpowers/specs/2026-09-15-typhon-generalized-presentation-engine-design.md`
- Modify: `docs/wayland/PROTOCOL_SOURCE_MANIFEST.md`

**Interfaces:**
- Consumes: verified implementation state from Tasks 1–4.
- Produces: accurate wording that typed `OutputId` foundation is implemented internally, while Typhon remains a single-output product without hotplug/multi-output behavior; `SceneNodeId`, generic scene hierarchy, and Presentation Engine v2 remain gated.

- [ ] **Step 1: Replace stale “no typed OutputId” wording**

Update only identity-status statements; do not claim multi-output support or edit unrelated architecture history.

- [ ] **Step 2: Verify documentation search**

Run:

```bash
rtk rg -n "There is no typed `OutputId`|no `OutputId` exists|typed `OutputId`|multi-output|SceneNodeId|Presentation Engine v2" docs/superpowers/specs/2026-09-15-typhon-generalized-presentation-engine-design.md docs/wayland/PROTOCOL_SOURCE_MANIFEST.md
```

Expected: the two current documents distinguish the implemented internal output-ID foundation from deferred product/model work.

- [ ] **Step 3: Commit documentation**

```bash
git add docs/superpowers/specs/2026-09-15-typhon-generalized-presentation-engine-design.md docs/wayland/PROTOCOL_SOURCE_MANIFEST.md
git commit -m "docs(identity): mark logical output foundation implemented"
```

### Task 6: Full verification and handoff

**Files:**
- Read/verify: all task-owned source and documentation files plus the complete final diff

- [ ] **Step 1: Re-check concurrent state and owned diff**

Confirm HEAD, status, and changed paths. If unrelated files changed, preserve them. If HEAD changed, inspect overlap before proceeding.

- [ ] **Step 2: Run the required full commands in the checkout**

```bash
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk run ./bin/check-source-layout
```

Record exit codes and fresh output. The source-layout count must be ≤55 and no new violating path may appear.

- [ ] **Step 3: Re-run focused identity and behavior suites**

Run the KMS output-identity module, KMS queue/dependent tests, transaction ledger/construction tests, compatibility presentation, cursor plane path, Direct Scanout, pageflip promotion, and session recovery tests as named in Tasks 1–4.

- [ ] **Step 4: Audit remaining production defaults and inspect the diff**

Use `rtk rg` to classify every production `OutputId::from_raw(1)`, `default_output_id`, and `with_output_id` occurrence. Confirm no production transaction constructor defaults or post-construction identity repair remain, and that `NativeSceneHistory` remains the single physical authority.

- [ ] **Step 5: Report completion with exact evidence**

Report starting/ending HEAD, pre-existing dirty files, owned files, overlap status, KMS behavior, constructor migration, remaining default classifications, recovery contract, focused/full results, source-layout counts, documentation, and any limitation. Do not claim multi-output support.
