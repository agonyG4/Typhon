# Typhon Native Output Damage Authority v1.1.1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the compatibility renderer's successful `NativePaintOutcome::Skipped` terminal retire the same logical scheduling state as the pre-render and Atomic NoVisualChange terminals.

**Architecture:** Extract the existing compatibility skip arm into a focused production helper used directly by `render_present_and_update_metrics`. The helper will complete the already-prepared NoVisualChange batch, retire the logical scene generation, clear the queued redraw state, and update each logical cursor baseline once; it will not touch physical presentation authorities. Tests will exercise this helper with the actual `NativePaintOutcome::Skipped` value and separately preserve the production no-primary, bounded-journal, Direct Scanout, and XWayland release contracts.

**Tech Stack:** Rust, Cargo tests, existing Typhon compositor/native-output test harnesses, `rtk` verification workflow.

**Spec:** `/home/agony/.codex/attachments/69c81f5e-eac9-4e3c-b318-40c722000fa7/pasted-text.txt`

## Global Constraints

- Preserve the current dirty checkout and unrelated changes.
- Do not redesign surface damage authority, output repair, O1, KMS worker, buffering, Direct Scanout, cursor scheduling, workspaces, Dwindle, XWayland, VRR, tearing, or renderer behavior.
- Keep physical presentation history unchanged for NoVisualChange terminals.
- Preserve pending SHM/DMABUF release ownership and the XWayland callback-plus-release contract.
- Keep all task-specific Markdown and new explanatory comments in English.
- Do not claim real TTY/DRM/KMS/165 Hz qualification.

---

### Task 1: Re-audit the compatibility terminal and add RED coverage

**Files:**
- Read: `src/native_output/runtime/presentation_cycle.rs`
- Read: `src/native_output/runtime/commit_timing.rs`
- Read: `src/native_output/runtime/presentation_metrics.rs`
- Read: `src/native_output/scanout/egl_gbm.rs`
- Read: `src/compositor/state/frames.rs`
- Read: `src/compositor/state/frame_callbacks.rs`
- Read: `src/compositor/state/frame_tests.rs`
- Test: existing native-output and compositor test modules

**Steps:**

- [ ] Reconfirm every production NoVisualChange terminal and the compatibility skipped-render branch.
- [ ] Confirm the stale logical-generation assignment and duplicate software cursor-baseline assignment.
- [ ] Add a deterministic test that constructs a prepared batch and actual `NativePaintOutcome::Skipped(FrameSkipReason::NoLogicalDamage)` terminal input, then asserts the logical baseline is retired, the batch is removed, surface lineage settles, cursor baselines are updated once, and physical state is untouched.
- [ ] Add assertions for the following unchanged state and later real mutation.
- [ ] Run the focused test and observe the expected pre-fix failure before production changes.

### Task 2: Extract and wire the compatibility NoVisualChange terminal

**Files:**
- Modify: `src/native_output/runtime/presentation_cycle.rs`
- Modify: `src/native_output/runtime/commit_timing.rs` only if a shared logical helper needs a narrow adjustment

**Steps:**

- [ ] Extract a focused helper from the existing `NativePaintOutcome::Skipped` arm, preserving the prepared-batch ownership and cursor-baseline semantics.
- [ ] Call `retire_logical_scene_generation` from that helper with the current scene generation.
- [ ] Replace the inline compatibility skip arm with the helper and remove the duplicate software baseline assignment.
- [ ] Keep `complete_no_visual_change_frame_batch`, not presented-frame completion, as the batch terminal.
- [ ] Run the new focused regression and the existing NoVisualChange/runtime tests.

### Task 3: Correct proof boundaries and historical documentation

**Files:**
- Modify: `src/compositor/state/frame_tests.rs`
- Modify: `REPORT-2026-08-27-typhon-native-output-damage-authority-v1-1.md` only for the minimal stale sentence if repository convention permits
- Create: `REPORT-2026-08-27-typhon-native-output-damage-authority-v1-1-1.md`

**Steps:**

- [ ] Rename the direct state-machine 128-entry test to describe bounded journal/helper coverage accurately.
- [ ] Add or strengthen the smaller production no-primary Empty/no-protocol wiring regression if the current harness exposes it without a large integration fixture.
- [ ] Preserve and rerun Direct identical-candidate and XWayland release/callback regressions.
- [ ] Write the English v1.1.1 closure report with the terminal matrix, root cause, RED evidence, verification results, two focused reviews, and hardware-qualification statement.

### Task 4: Verify and review

**Steps:**

- [ ] Run focused compatibility renderer, NoVisualChange, frame-batch, surface-damage, commit-timing, Direct Scanout, and XWayland tests.
- [ ] Run `rtk cargo fmt --all -- --check`, `rtk cargo check --locked`, `TMPDIR=/tmp rtk cargo test --locked`, `rtk cargo clippy --locked --all-targets --all-features -- -D warnings`, `rtk git diff --check`, and `rtk run "bin/check-source-layout"` when present.
- [ ] Classify any failures as task-owned or pre-existing and fix task-owned failures only.
- [ ] Perform correctness/ownership review for stale generations, batch failure, physical history, exact surface lineage, cursor baselines, releases, Direct Scanout, and later scene changes.
- [ ] Perform locality/scope review for accidental renderer or cursor refactors and for duplicate terminal work.
- [ ] Mark the implementation plan complete only after the report and verification evidence are final.
