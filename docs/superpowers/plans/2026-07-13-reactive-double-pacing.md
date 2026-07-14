# Reactive Double Pacing Restoration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. This task must be executed inline without sub-agents.

**Goal:** Restore pre-triple pageflip-reactive pacing for `off` and Auto/Double while retaining the explicit Atomic ownership backend and isolating predictive scheduling to Triple mode.

**Architecture:** Resolve an explicit `NativeOutputPacingMode` from configuration and adaptive state. `ReactiveDouble` makes immediate render/submit decisions, creates non-gating N+1 target metadata, and exposes only current+pending capacity; `PredictiveTriple` alone plans render-ahead targets and may own one ready frame. Presentation target identity remains frame-owned, while pageflip completion remains the scheduling clock.

**Tech Stack:** Rust, Atomic DRM/KMS, EGL/GLES/GBM, the existing native scheduler/runtime/swapchain test modules.

## Global Constraints

- Preserve the complete dirty tree based on `f8b2997466e9c68623f0572fbf1a71ee297afae6`.
- Use `0e30f0a9e521f8c2549efdd6b4914dac7177d5b6` as the scheduler behavior oracle.
- Do not reset, clean, restore, remove explicit Atomic KMS, remove explicit sync, or weaken frame ownership.
- Do not commit until native Atomic `off` passes production-feel and lightweight-metrics validation.
- Do not use sub-agents.

---

### Task 1: Differential Reactive Scheduler Contract

**Files:**
- Modify: `src/native/scheduler.rs`

**Interfaces:**
- Consumes: existing `NativeFrameScheduler`, `SchedulerFrameContext`, and `SchedulerDecision`.
- Produces: `NativeOutputPacingMode::{ReactiveDouble, PredictiveTriple}` passed explicitly in `SchedulerFrameContext` and a test-only pre-triple reference decision model.

- [ ] Add the test-only old-reference model for visual work, protocol work, pending pageflip, effective target availability, ready state, completion, watchdog, and suspend.
- [ ] Add differential traces for idle render, pending coalescing, matching completion, protocol-only work, watchdog, suspend, and same-wake client work.
- [ ] Run `cargo test native::scheduler::tests -- --test-threads=1` and confirm failures show the current deadline-driven branch differs from the reference.
- [ ] Add the explicit pacing mode and a direct ReactiveDouble decision branch: pending visual work waits; non-pending visual work renders without inspecting a target deadline; ready state is rejected as an invariant violation.
- [ ] Re-run the focused scheduler tests and confirm the differential traces pass.

### Task 2: Reactive Target Identity Without Scheduling Authority

**Files:**
- Modify: `src/native/presentation_deadline.rs`

**Interfaces:**
- Consumes: last matching pageflip time, logical sequence, refresh interval, and current monotonic time.
- Produces: `PresentationTargetReason::ReactiveDouble` and `reactive_target(now) -> Option<PresentationTarget>` which never updates `scheduled`.

- [ ] Add failing tests proving a reactive target is N+1 even when prediction exceeds one refresh, has already-satisfied deadlines, and does not become the planner's scheduled owner.
- [ ] Run `cargo test native::presentation_deadline::tests -- --test-threads=1` and confirm the new tests fail because the API/reason is absent.
- [ ] Implement non-gating reactive target construction from the last pageflip anchor without calling `earliest_reachable` or `plan_normal`.
- [ ] Re-run the focused planner tests and confirm they pass.

### Task 3: Policy-Aware Explicit Slot Capacity

**Files:**
- Modify: `src/native_output/scanout/mod.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Modify: `src/native_output/scanout/output_swapchain.rs`
- Modify: `src/native_output/tests/scanout.rs`

**Interfaces:**
- Consumes: `NativeOutputPacingMode`.
- Produces: `render_target_available_for(mode)` and `acquire_render_slot_for(mode)`; ReactiveDouble rejects rendering/ready ownership while pending.

- [ ] Add failing swapchain and backend tests proving ReactiveDouble reports no target and cannot acquire while pending, while PredictiveTriple can acquire exactly one third slot.
- [ ] Run the focused scanout/swapchain tests and confirm failures are caused by physical three-slot availability leaking into double mode.
- [ ] Implement central mode-aware availability/acquisition and `validate_invariants_for(mode)` without changing generation, fence, quarantine, or protocol batch ownership.
- [ ] Re-run focused tests and confirm they pass.

### Task 4: Runtime Mode Split and Immediate Reactive Submission

**Files:**
- Modify: `src/native/adaptive_buffering.rs`
- Modify: `src/native_output/runtime/presentation.rs`
- Modify: `src/native_output/runtime/cycle.rs`
- Modify: `src/native_output/runtime/mod.rs`

**Interfaces:**
- Consumes: policy, adaptive state, safe ownership boundary, planner, scheduler, and mode-aware scanout availability.
- Produces: `pacing_mode()` resolution where Off and Auto/Double are ReactiveDouble, Force and Auto/Triple are PredictiveTriple.

- [ ] Add failing unit tests proving Off/Auto-Double never call normal planning, never wait on normal deadlines, and pageflip completion plus queued visual work renders/submits in one cycle.
- [ ] Run focused runtime/cycle tests and confirm the current normal target/timer path fails those assertions.
- [ ] Resolve pacing mode before planning. In ReactiveDouble clear scheduled predictive state, create reactive metadata at render time, render immediately, submit immediately, and never call `note_ready_frame` for a normal frame.
- [ ] Keep predictive planning only for pending+visual in PredictiveTriple, transfer the single target to ready ownership, and submit ready N+1 immediately after the matching N pageflip.
- [ ] Gate adaptive transitions on no ready/rendering ownership, preserving deterministic hysteresis.
- [ ] Re-run focused runtime/cycle tests and confirm they pass.

### Task 5: Timer Ownership and Pacing Counters

**Files:**
- Modify: `src/native_output/pacing.rs`
- Modify: `src/native_output/runtime/presentation.rs`
- Modify: `src/native_output/runtime/session_io.rs`

**Interfaces:**
- Consumes: resolved pacing mode and scheduler/planner ownership transitions.
- Produces: the required reactive, predictive, normal-ready, scheduled-normal, expired-deadline, immediate-wake, and multiple-owner counters.

- [ ] Add failing metric tests for all required zero relationships in Off and `predictive_render_ahead_ready <= predictive_render_ahead_attempts`.
- [ ] Run focused pacing tests and confirm missing counters/incorrect render-ahead accounting fail.
- [ ] Add counters at ownership transitions, assert a single deadline owner, and remove normal visual target deadlines from ReactiveDouble event-loop arming.
- [ ] Re-run focused pacing tests and state-machine stress tests and confirm they pass.

### Task 6: Existing Correctness Regression Sweep

**Files:**
- Modify only a directly implicated test or source file when a regression test demonstrates a pacing-induced failure.

**Interfaces:**
- Consumes: frame-owned callbacks/releases/damage, explicit fences, orientation, pageflip identity, and timestamp fallback.
- Produces: no change to their ownership semantics.

- [ ] Run focused scheduler, swapchain, frame-batch, damage, explicit-sync, pacing, zero-sequence, and orientation tests.
- [ ] For each failure, reproduce it independently and add a failing regression before the minimal correction.
- [ ] Confirm all focused tests pass with no duplicate release, unpublished callback, sequence mutation, or stale/mismatched pageflip regression.

### Task 7: Automated and Native Validation Boundary

**Files:**
- No source changes unless a verification command exposes a reproduced defect with a new failing test.

**Interfaces:**
- Consumes: completed implementation.
- Produces: exact automated counts and a handoff for mandatory real-TTY validation.

- [ ] Run `cargo fmt --check`.
- [ ] Run `cargo check --all-targets`.
- [ ] Run `cargo clippy --all-targets -- -D warnings`.
- [ ] Run `cargo test -- --test-threads=1`.
- [ ] Run `./bin/check-source-layout`.
- [ ] Run `git diff --check`.
- [ ] Run `cargo build --release`.
- [ ] Preserve the uncommitted result and report that native Atomic `off` production-feel and lightweight-metrics runs remain authoritative and require the real TTY/NVIDIA output.
