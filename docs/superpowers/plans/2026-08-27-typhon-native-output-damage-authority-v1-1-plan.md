# Typhon Native Output Damage Authority v1.1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `NoVisualChange` a complete logical terminal outcome without claiming physical presentation or creating empty compatibility frame ownership.

**Architecture:** Keep physical presentation history unchanged. Add a small logical-scene settlement boundary that advances the scheduling/coalescing baseline, capture exact surface lineage whenever a scene generation is terminally accounted, and let compatibility frame-batch ownership be conditional on actual protocol/release work. Normalize Atomic no-visual ledger settlement to the dedicated terminal transition while preserving Direct Scanout policy.

**Tech Stack:** Rust, existing compositor frame-batch state, native output runtime, output transaction ledger, deterministic unit/state-machine tests, `rtk` verification workflow.

**Spec:** `/home/agony/.codex/attachments/28e14af9-0458-46c5-bd40-9d9c7200e9df/pasted-text.txt`

## Global Constraints

- Preserve the dirty checkout; do not reset, clean, stash, discard, or rewrite unrelated changes.
- Do not change O1, KMS-worker policy/default, triple buffering, Direct Scanout admission/default, VRR, tearing, workspace, Dwindle, XWayland, cursor policy, or output buffer-age repair.
- `NoVisualChange` may settle logical surface damage and protocol work but must not advance physical presentation history or create fake presentation.
- Exact surface lineage remains independent from protocol frame-batch ownership.
- All task-specific Markdown, comments, and reports are English.
- No wall-clock threshold is a complexity proof.

---

### Task 1: Add RED coverage for logical terminal ownership

**Files:**
- Modify: `src/native_output/runtime/commit_timing.rs`
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/native_output/runtime/work_domains.rs`
- Test: `src/native_output/tests/frame.rs`

**Interfaces:**
- Produces a tested logical-scene settlement helper and explicit cycle-completion predicate for later runtime use.

- [ ] **Step 1: Write failing tests** for a logical baseline that advances after `NoVisualChange`, returns unchanged on the following stable cycle, returns changed after a later mutation, and does not treat an idle tick as completed work.
- [ ] **Step 2: Run the focused tests** and verify they fail because the baseline remains behind or the completion predicate reports the wrong result.
- [ ] **Step 3: Add the smallest pure helpers** needed to express logical settlement and distinguish logical/protocol completion from idle work.
- [ ] **Step 4: Run the focused tests** and verify they pass without changing physical presentation state.

### Task 2: Add RED coverage for independent surface lineage and runtime Empty sequences

**Files:**
- Modify: `src/compositor/state/frame_tests.rs`
- Modify: `src/native_output/tests/integrated_swapchain_oracle.rs`

**Interfaces:**
- Consumes the existing exact `SurfaceDamagePresentation` capture and `NoVisualChange` settlement APIs.
- Produces deterministic tests proving scene-only Empty accounting, bounded frame ownership, and later Partial preservation.

- [ ] **Step 1: Write failing state-machine tests** that execute 128+ Empty logical commits without constructing a protocol batch, assert no frame-batch leak, keep physical presentation unchanged, and preserve a later small Partial.
- [ ] **Step 2: Write failing compatibility ownership tests** for Empty with no protocol work and Empty with callback/feedback work.
- [ ] **Step 3: Run the focused tests** and record the expected failures: no-batch path cannot settle lineage and compatibility pre-capture leaves an orphan batch.

### Task 3: Delay compatibility ownership and complete no-visual runtime work

**Files:**
- Modify: `src/native_output/runtime/presentation_cycle.rs`
- Modify: `src/native_output/runtime/presentation_metrics.rs`
- Modify: `src/compositor/state/frame_callbacks.rs`
- Modify: `src/compositor/state/frames.rs`
- Modify: `src/compositor/server_frames.rs`

**Interfaces:**
- `finish_no_primary_work` receives exact lineage independently from protocol-work presence.
- Compatibility rendering captures a frame batch only after the output decision proves a real render or protocol ownership is needed.

- [ ] **Step 1: Remove the pre-resolution compatibility capture.**
- [ ] **Step 2: Capture exact lineage for scene-terminal NoVisualChange even when `pending_frame_work` is false.**
- [ ] **Step 3: Add a no-batch surface-damage settlement path and make protocol batch completion conditional on actual batch ownership.**
- [ ] **Step 4: Capture a compatibility batch immediately before real compatibility rendering, preserving existing failure restoration.**
- [ ] **Step 5: Update `frame_completed` from actual logical/protocol terminal work and advance only the logical scheduling baseline.**
- [ ] **Step 6: Run the focused runtime/state tests and confirm no orphan batch, no fake presentation, and no lost callback/feedback ownership.**

### Task 4: Normalize Atomic no-visual terminalization

**Files:**
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Modify: `src/native_output/presentation/ledger.rs` only if the existing dedicated transition requires a narrowly scoped assertion.
- Test: `src/native_output/tests/presentation_transactions.rs`

**Interfaces:**
- Atomic EGL `NoLogicalDamage` uses `settle_no_visual_change_output_transaction` exactly once.
- The resulting transaction is terminal and cannot later submit or present.

- [ ] **Step 1: Add a failing ledger/transaction regression** for exact NoVisualChange terminal state and rejected follow-up submission.
- [ ] **Step 2: Run it to confirm the current Atomic path is not covered by the dedicated transition.**
- [ ] **Step 3: Replace only the Atomic NoLogicalDamage generic dropped transition.**
- [ ] **Step 4: Run the focused Atomic and ledger tests.**

### Task 5: Audit Direct Scanout and write closure evidence

**Files:**
- Modify: `src/native_output/tests/scanout.rs` or the narrow existing Direct Scanout test module.
- Create: `REPORT-2026-08-27-typhon-native-output-damage-authority-v1-1.md`

**Interfaces:**
- Direct Scanout keeps its identical-candidate lightweight NoVisualChange behavior only with a tested lineage invariant.
- The report records the pre-change flows, changed files, RED/GREEN evidence, physical-authority invariants, and two adversarial reviews.

- [ ] **Step 1: Add or strengthen the deterministic identical-key lineage invariant test.**
- [ ] **Step 2: Run all focused domains: NoVisualChange, frame batch, surface journal, presentation transaction, compatibility, Atomic EGL, output damage, Direct Scanout.**
- [ ] **Step 3: Perform ownership review covering failed render/KMS, queued/superseded work, newer commits, callbacks/feedback/releases, and physical history.**
- [ ] **Step 4: Perform locality/repetition review covering Empty x128+, Empty-to-Partial, and remaining capture/batch scans.**
- [ ] **Step 5: Run fmt, check, full tests, clippy, diff check, and source-layout command; classify every failure as task-owned or pre-existing.**
- [ ] **Step 6: Write the English closure report with exact command outcomes and explicitly state that no real TTY/DRM/KMS/165 Hz qualification was run.**
