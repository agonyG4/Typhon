# Presentation Pacing Integration Gaps Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task with verification checkpoints.

**Goal:** Ensure every eligible READY binding frame is re-evaluated before worker reservation, and ensure only proven worker dispatch tails adapt the KMS dispatch guard.

**Architecture:** Keep the existing ready-target replacement transaction and worker timing model. Factor the ready pull-in operation so the runtime READY path can invoke it for both lane-waiting and lane-free frames, then pass an availability proof into the existing adaptive observer so upstream lateness is excluded without adding a second miss taxonomy.

**Tech Stack:** Rust, existing Atomic KMS runtime, `NativeFramePacing`, `PresentationDeadlinePlanner`, `KmsWorkerDispatchModel`, and the repository `rtk` command proxy.

## Global Constraints

- Do not redesign the pacing architecture.
- Do not modify Lamp, Blur/Effects, XWayland, input, Direct Scanout policy, or Predictive O1 semantics.
- Preserve `ReadyPresentationServiceEstimate`, queue-residency exclusion, P95 normal dispatch estimation, the 1 ms tail-guard cap, ReactiveDouble advisory behavior, Predictive O1 exact identity, ready-target replacement transaction, queued predecessor ordering, worker ABA protection, and submitted-worker ownership settlement.
- Compile in the repository folder; use the existing local Cargo target placement and run all Cargo/Git commands through `rtk`.
- Preserve unrelated dirty work and commit the focused changes because this is a Git repository.

### Task 1: Add RED regressions

**Files:**
- Modify: `src/native_output/runtime/presentation_ready.rs` tests or the nearest runtime test module for the READY-to-worker boundary.
- Modify: `src/native_output/kms_worker/timing_tests.rs`.

**Interfaces:**
- The runtime regression must exercise the runtime READY pull-in operation, inspect the replacement target and `KmsSubmitWindow`, then call `NativeFramePacing::reserve_worker_submission(false)` to cross the worker-reservation boundary.
- The timing regression must exercise the existing `KmsWorkerDispatchModel` observer with a job availability time after the planned worker wake and assert that both the adaptive guard and increase count remain unchanged.

- [ ] **Step 1: Write the lane-free runtime regression.** Build the existing 165 Hz planner scenario with a bound N+2 target, a free lane, `render_ahead = false`, and an unreserved normal pacing frame. Assert that the runtime READY path replaces N+2 with N+1, installs the N+1 window, and only then returns a worker ticket for the same frame. Include the old-path proof by asserting the worker ticket is taken after the runtime operation, not by directly testing the planner or swapchain replacement alone.

- [ ] **Step 2: Run the focused runtime test before production changes.** Run `rtk cargo test --locked presentation_ready` and confirm the new regression fails because the lane-free runtime handoff does not yet invoke the pull-in operation.

- [ ] **Step 3: Write the late-payload timing regression.** Use a commit-complete deadline D, a planned worker wake before D, a job availability timestamp after that wake, and a submit return after D. Assert the existing dispatch-tail guard and increase count are unchanged.

- [ ] **Step 4: Run the focused timing test before production changes.** Run `rtk cargo test --locked kms_worker` and confirm the new regression fails because the current observer treats every late successful submit as dispatch-tail evidence.

### Task 2: Move READY pull-in evaluation ahead of both handoff branches

**Files:**
- Modify: `src/native_output/runtime/presentation_ready.rs`.
- Modify: `src/native_output/runtime/presentation_cycle.rs`.

**Interfaces:**
- Preserve `pull_ready_frame_into_reachable_opportunity` as the production Atomic scanout entry point.
- Add only the smallest runtime-facing factoring needed to run the same replacement transaction against the ready swapchain before either lane waiting or worker reservation.

- [ ] **Step 1: Factor the existing ready pull-in transaction over the ready swapchain.** Keep all current eligibility checks and ownership preflight/rollback behavior, including non-Predictive-O1 identity, binding target, generation, physical frontier, service estimate, exact ownership, and advisory ReactiveDouble rejection.

- [ ] **Step 2: Call the factored operation for every non-deferred-O1 rendered READY frame.** For lane-waiting frames, retain the existing `note_ready_frame` and deferred callback accounting before evaluation. For lane-free frames, evaluate while the active normal frame is still available, before `reserve_worker_submission(false)` or any queue ownership transfer. Keep deferred Predictive O1 abandonment on its existing path and retain the existing pull-in diagnostics.

- [ ] **Step 3: Run the runtime regression and adjacent focused suites.** Run `rtk cargo test --locked presentation_ready`, `rtk cargo test --locked presentation_deadline`, `rtk cargo test --locked scheduler`, and `rtk cargo test --locked pacing`. Confirm both lane-free and lane-waiting behavior, queued-predecessor ordering, Predictive O1 exclusion, and ReactiveDouble advisory behavior remain covered.

### Task 3: Gate adaptive dispatch learning on availability evidence

**Files:**
- Modify: `src/native_output/kms_worker/timing.rs`.
- Modify: `src/native_output/kms_worker/thread.rs`.
- Modify: `src/native_output/kms_worker/timing_tests.rs`.

**Interfaces:**
- Keep `KmsPresentationOutcome` as the existing miss taxonomy; use a boolean/proof argument derived from the job’s recorded availability and planned worker wake rather than adding another miss enum.
- Preserve tail-guard increase, cap, clean-streak decay, and timing snapshot counters for proven dispatch observations.

- [ ] **Step 1: Add an availability-proof parameter to the existing observer.** Treat `job.queued_at <= planned_worker_wake_at` as the minimum fair-chance proof. A positive deadline overrun may raise the guard only with that proof; an unavailable/late job must not raise or reset the clean streak as a dispatch miss.

- [ ] **Step 2: Pass the proof from `run_worker`.** Keep queue residency out of the dispatch estimator. Continue recording normal timing samples and successful submission metrics exactly as before, but pass the job-availability evidence into the adaptive observer.

- [ ] **Step 3: Retain and update the positive regression.** The existing on-time payload plus approximately 12 us worker overrun must still increase the guard immediately, while the new late-payload case must leave the guard and increase counter unchanged.

- [ ] **Step 4: Run `rtk cargo test --locked kms_worker`.** Confirm the timing behavior and worker integration compile and pass.

### Task 4: Verify the complete request and commit

**Files:**
- Review all modified files and the final Git diff.

- [ ] **Step 1: Run the requested verification commands.**

```bash
rtk cargo test --locked kms_worker
rtk cargo test --locked presentation_deadline
rtk cargo test --locked scheduler
rtk cargo test --locked pacing
rtk cargo test --locked presentation
rtk cargo test --locked native_output
rtk cargo fmt --all -- --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

- [ ] **Step 2: Review the final diff.** Confirm lane-free READY pull-in precedes worker reservation, lane-waiting behavior remains, queued predecessors remain ordered, Predictive O1 remains excluded, ReactiveDouble remains advisory, late readiness cannot train the tail guard, genuine worker tails still train it, and no unrelated dirty work was changed.

- [ ] **Step 3: Commit the focused implementation.** Use a message such as `fix(pacing): close ready and dispatch integration gaps` after fresh verification succeeds.
