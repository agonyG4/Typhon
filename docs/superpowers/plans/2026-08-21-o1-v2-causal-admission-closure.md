# Typhon O1 v2 Causal Admission and Simulator Closure Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task with verification checkpoints.

**Goal:** Make runtime O1 demand usable by the same scheduling decision that observes it, bind usefulness metrics to the exact admitted frame, and make the deterministic simulator obey bounded production ownership.

**Architecture:** Add a focused `presentation_o1` decision boundary that observes the current opportunity before deriving admission inputs and returns one post-observation credit snapshot. Carry a small immutable `O1AdmissionObservation` through `RenderedOutputFrame` to pageflip. Replace simulator boolean lanes and prequeued pageflips with frame-identity lanes and submission-driven refresh scheduling, while reusing the production demand controller.

**Tech Stack:** Rust, existing native output pipeline and scheduler types, deterministic virtual-time `BinaryHeap` event model, locked Cargo tests.

## Global Constraints

- Preserve unrelated dirty compositor, cursor, XWayland, and report changes.
- Do not reset, clean, delete `target/`, run vkmark, or run a long benchmark campaign.
- Preserve desired credit versus physical ownership, immutable targets, pageflip-authoritative presentation, KMS-only miss attribution, and existing KMS Timing v2 behavior.
- Keep physical future-primary depth, render/prepared ownership, worker-next ownership, and kernel-submitted ownership bounded to production capacity.

### Task 1: Apply current-opportunity demand before admission

**Files:**
- Create: `src/native_output/runtime/presentation_o1.rs`
- Modify: `src/native_output/runtime/mod.rs`
- Modify: `src/native_output/runtime/presentation_cycle.rs`
- Test: `src/native_output/runtime/presentation_o1.rs`

**Interfaces:**
- `O1CycleDemandDecision` records overlap, credit before/after, and grant/revoke transitions.
- `observe_current_o1_opportunity` observes one predecessor opportunity and returns the post-observation credit snapshot.
- The presentation cycle uses that one `desired_credit_after` for render-ahead permission, visual target planning, render-target availability, pipeline snapshot, and scheduler context.

- [ ] Write and run a failing test proving positive overlap changes credit 1 to 2 before the same admission decision and duplicate evaluation is deduplicated.
- [ ] Implement the focused decision boundary and move the runtime observation ahead of all admission derivation.
- [ ] Run the focused runtime/scheduler tests and commit `fix(native): apply O1 demand to the current opportunity`.

### Task 2: Bind O1 usefulness to frame admission

**Files:**
- Modify: `src/native/buffering/mod.rs`
- Modify: `src/native_output/scanout/output_swapchain.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Modify: `src/native_output/runtime/presentation_cycle.rs`
- Modify: `src/native_output/runtime/cycle/pageflip.rs`
- Modify: `src/native_output/pacing_o1.rs`
- Test: buffering/pacing tests

**Interfaces:**
- `O1AdmissionObservation` is immutable metadata attached to `RenderedOutputFrame`.
- `used_extra_credit` is true only when the frame admission actually consumes the second future-primary allowance.
- Pageflip passes only that frame-local observation to the bounded outcome counters; mutable controller overlap is not consulted.

- [ ] Add failing tests for later-opportunity overwrite, ordinary credit-2 frames, and extra-credit miss classification.
- [ ] Capture admission metadata at the real render decision and carry it through ready/worker/submitted/pageflip ownership.
- [ ] Update metrics and run focused tests; commit `fix(native): bind O1 credit outcomes to frame admission`.

### Task 3: Model physical simulator lanes and liveness

**Files:**
- Modify: `src/native/buffering/simulator.rs`
- Modify: `src/native/buffering/mod.rs` only if shared value exports are required
- Test: `src/native/buffering/simulator.rs`

**Interfaces:**
- Simulator lanes hold frame identities: one rendering/prepared, one worker-next, and one kernel-submitted primary.
- Pageflip events are created only after accepted submission and use the first refresh at or after submit return plus apply delay.
- Missed targets remain eligible for later physical presentation; generation changes explicitly invalidate stale work.

- [ ] Add failing lane, later-refresh, submitted-frame-liveness, and runtime/simulator ordering-parity tests.
- [ ] Implement serialized identity-owned transitions without busy loops or unbounded queues.
- [ ] Run bounded refresh/service/dispatch/apply sweeps and commit `test(native): model O1 physical scheduling lanes`.

### Task 4: Review and validate closure

**Files:**
- Review all O1 closure files; change only defects caused by this task.

- [ ] Perform a causal-order review and a runtime/simulator/telemetry parity review.
- [ ] Run focused tests, `cargo fmt --check`, `cargo check --locked --all-targets`, `cargo clippy --locked --all-targets -- -D warnings`, full locked tests, source-layout, and `git diff --check` via `rtk` where applicable.
- [ ] Document exact unrelated/environment-only failures and report that no vkmark campaign was run.
