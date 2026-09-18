# Advisory ReactiveDouble Opportunity Slips Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task with review checkpoints. This task is executed inline because subagents are explicitly disallowed.

**Goal:** Keep ReactiveDouble KMS dispatch slips visible for physical and cadence analysis without treating them as estimator-confidence misses.

**Architecture:** Leave `KmsPresentationOutcome` and KMS timing observations as physical stage accounting. Add pageflip-local deadline assessment policy that preserves render evidence, gates KMS dispatch proof on target authority, and routes only proven assessments into existing recovery/O1 policy. Add explicit worker dequeue timing evidence and conditional advisory telemetry in the existing pacing surface.

**Tech Stack:** Rust, existing native compositor pacing modules, `KmsPresentationTimingModel`, `KmsWorkerDispatchModel`, `NativeFramePacing`, and the repository `rtk` command proxy.

## Global Constraints

- Keep `ColdStart`, `WarmPaired`, and `MissRecovery` unchanged.
- Preserve ReactiveDouble advisory target selection and non-gating render/submit semantics.
- Preserve exact/guarded render-readiness recovery, binding KMS cause-aware recovery, KmsApplyGuard behavior, content cadence attribution, KMS timing observations, Predictive O1, and READY-frame wake ownership.
- Do not change pacing constants, estimator arithmetic, scheduler wake semantics, target-selection arithmetic, or add a metrics subsystem.
- Compile in `/home/agony/GitHub/Typhon` and run Cargo/Git commands through `rtk`.
- Preserve unrelated dirty work; commit only the focused files explicitly changed by this task.

---

### Task 1: Add RED assessment and fairness regressions

**Files:**
- Modify: `src/native_output/runtime/cycle/pageflip_tests.rs`
- Modify: `src/native_output/kms_worker/timing.rs`
- Modify: `src/native_output/kms_worker/thread.rs` tests if the fairness helper is placed there

**Interfaces:**
- Pageflip tests consume the new assessment helper and existing recovery disposition helper.
- Worker timing tests consume `KmsWorkerDispatchTailObservation::dequeued_before_planned_wake` and the unchanged `fair_dispatch_chance` gate.

- [ ] **Step 1: Write the failing pageflip assessment tests.** Cover a non-binding ReactiveDouble `KmsDispatchMiss` becoming `Advisory(KmsDispatch)`, exact and guarded render misses remaining `Proven`, binding dispatch remaining `Proven`, and KmsApplyGuard remaining `Proven`.

- [ ] **Step 2: Run the assessment tests before production changes.** Run `rtk cargo test --locked pageflip`; expect compilation/test failure because the assessment type/helper and new diagnostic field do not yet exist.

- [ ] **Step 3: Write the failing recovery and cadence contract tests.** Assert advisory dispatch does not change a WarmPaired journal with zero remaining recovery, does not reset an active horizon, and can still feed `classify_content_frame(... submit_missed = true ...)` to `SubmitLimited`. Keep exact/guarded render misses and binding dispatch recovery assertions adjacent to the existing pageflip tests.

- [ ] **Step 4: Write the fairness decomposition tests.** Assert `(binding=false, dequeued_before=true) -> fair=false`, `(binding=true, dequeued_before=false) -> fair=false`, and `(binding=true, dequeued_before=true) -> fair=true`, while preserving the existing tail-guard training gate.

- [ ] **Step 5: Run the new worker tests before implementation.** Run `rtk cargo test --locked kms_worker`; expect failure at the new field/signature until production code is added.

### Task 2: Implement separate physical outcome assessment

**Files:**
- Modify: `src/native_output/runtime/cycle/pageflip.rs`
- Modify: `src/native_output/runtime/cycle/pageflip_tests.rs`

**Interfaces:**
- Add pageflip-local `PresentationDeadlineAssessment` and `AdvisoryOpportunitySlip` types, or repository-equivalent names.
- Add a focused helper that accepts `PresentationTarget`, `KmsPresentationOutcome`, and fence evidence, returning `None`, `Proven`, or `Advisory`.

- [ ] **Step 1: Implement the minimum assessment policy.** Keep `KmsPresentationOutcome::classify()` unchanged. Preserve pending render-miss precedence. Map render misses by existing fence quality, map binding/non-binding KMS dispatch by `target.is_binding()`, and leave KmsApplyGuard on its current proven path.

- [ ] **Step 2: Route only proven assessments into recovery and O1.** Replace the direct KMS-dispatch-to-proven mapping in `wait_for_events_and_pageflips`. Call `recovery_disposition`, `note_proven_deadline_miss`, and `observe_o1_outcome` only for `Proven`; advisory slips must not touch those flows or alter an active horizon.

- [ ] **Step 3: Preserve KMS timing and cadence.** Continue calling `observe_pageflip_with_evidence` for every synchronous physical outcome. Continue setting `submit_missed` from `KmsDispatchMiss`, so advisory dispatch can remain `SubmitLimited`.

- [ ] **Step 4: Run the focused pageflip/pacing tests.** Run `rtk cargo test --locked pageflip`, `rtk cargo test --locked pacing`, and `rtk cargo test --locked pacing_o1`; confirm the assessment and content contracts pass.

### Task 3: Add diagnostics and bounded advisory summary accounting

**Files:**
- Modify: `src/native_output/runtime/cycle/pageflip.rs`
- Modify: `src/native_output/pacing.rs`

**Interfaces:**
- Add `NativeFramePacing::note_advisory_dispatch_slip()` and the `advisory_dispatch_slips` summary field.
- Extend the existing `proven_deadline_miss` render trace with source/timestamp fields.
- Emit `advisory_opportunity_slip` only through `frame_pacing.log`, preserving its existing debug/trace gating.

- [ ] **Step 1: Add the bounded counter and summary regression.** Initialize the counter, increment it from the advisory path, export it from `summary_line`, and assert the zero-valued field in the existing summary test.

- [ ] **Step 2: Add advisory trace fields.** Include frame/pageflip identity, `slip_cause`, target reason/binding/sequence metadata, dispatch evidence decomposition, budget/overrun values, and recovery before/after/mode values. Use `none` fields when evidence is unavailable.

- [ ] **Step 3: Add exact render readiness observability.** Extend the existing proven render miss event with readiness source, fence quality, payload-ready/deadline/lateness, composite-start, and rendered-at values without changing classification.

- [ ] **Step 4: Run focused pacing verification.** Run `rtk cargo test --locked pacing`, `rtk cargo test --locked presentation`, and `rtk cargo test --locked native_output`.

### Task 4: Decompose worker dispatch evidence without changing adaptation

**Files:**
- Modify: `src/native_output/kms_worker/timing.rs`
- Modify: `src/native_output/kms_worker/thread.rs`
- Modify: `src/native_output/runtime/cycle/pageflip_tests.rs` as needed for struct literals

**Interfaces:**
- Add `dequeued_before_planned_wake: bool` to `KmsWorkerDispatchTailObservation`.
- Preserve `fair_dispatch_chance = binding_target && dequeued_before_planned_wake` and use it as the only tail-guard adaptation gate.

- [ ] **Step 1: Add the field to the observation and populate it in `run_worker`.** Compute dequeue timing beside the existing binding/fairness values and carry it through `KmsSubmittedOwnership` unchanged.

- [ ] **Step 2: Update all test fixtures and assert the decomposition.** Keep binding dispatch recovery tests requiring both binding and fair evidence; add all three boolean combinations.

- [ ] **Step 3: Run the worker suite.** Run `rtk cargo test --locked kms_worker` and confirm proven fair tails still adapt while non-binding/late-dequeue observations do not.

### Task 5: Verify, inspect, document native qualification, and commit

**Files:**
- Review: all focused source/test files above
- Review: `docs/superpowers/specs/2026-09-18-advisory-opportunity-slip-design.md`

- [ ] **Step 1: Run the requested focused tests.**

```bash
rtk cargo test --locked presentation_deadline
rtk cargo test --locked adaptive_buffering
rtk cargo test --locked kms_timing
rtk cargo test --locked kms_worker
rtk cargo test --locked pageflip
rtk cargo test --locked pacing
rtk cargo test --locked pacing_o1
rtk cargo test --locked presentation
rtk cargo test --locked native_output
```

- [ ] **Step 2: Run full verification in the repository folder.**

```bash
rtk cargo fmt --all -- --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

- [ ] **Step 3: Review the diff against the failure-mode checklist.** Confirm no generic non-binding suppression, no render-miss escape, no advisory recovery/O1/tail-guard update, unchanged fairness gate, unchanged cadence/KMS accounting, unchanged KmsApplyGuard, and no READY/O1/scheduler edits.

- [ ] **Step 4: Run the repository source-layout gate if available.** Record whether it is absent, passes, or fails pre-existing checks.

- [ ] **Step 5: Run the same blur-enabled native workload with the requested environment.** Record the exact command and inspect advisory slips, proven causes, recovery shares, render evidence, fair/unfair dispatch decomposition, content attribution, READY continuations, and Predictive O1 lifecycle. Deterministic tests do not substitute for this native gate.

- [ ] **Step 6: Commit only the focused patch.** Use `rtk git diff --check`, `rtk git status --short`, then stage only the new design note, implementation plan if intended as part of the deliverable, and focused source files. Commit with `fix(pacing): separate advisory dispatch slips from recovery`; verify the commit and unrelated dirty changes remain present.
