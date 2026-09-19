# Suffix-Demand Verification Hardening Implementation Plan

> **For agentic workers:** This plan is executed inline in the current checkout because the user explicitly prohibited subagents. Steps use checkbox syntax for tracking.

**Goal:** Verify the existing suffix-demand executor and planner behavior at GLES execution level, adding only the missing conservative planner regression proof.

**Architecture:** Reuse the existing native-faithful fixtures, compiler-backed three-checkpoint fixture, shared trace events, and `SceneReplayWorkState`. Keep production replay geometry and pass-lifetime retirement unchanged; the only expected source change is a focused planner test that materializes the conservative trailing `SurfaceConsumerPlan` from baseline work.

**Tech Stack:** Rust 2024, Cargo unit tests in the `oblivion-one` binary target, GLES test harness, Codebase Memory MCP, `rtk` command wrapper.

## Global Constraints

- Keep all existing pure `SceneReplayWorkState` tests.
- Do not redesign `SceneReplayWorkPlan` or `SceneReplayWorkState`.
- Do not change Kawase, checkpoint capture domains, render-graph dependency semantics, damage planning, resource pools, or scene-work preservation.
- Retire a checkpoint requirement only after its direct framebuffer `execute_pass` succeeds.
- Use real GLES execution, real scene commands, real framebuffer checkpoint capture, real executor state transitions, and pixel comparison for executor regressions.
- Compile and run Cargo commands in `/home/agony/GitHub/Typhon`.
- Do not add large rectangle dumps or new trace fields unless an invariant cannot be observed with existing fields.
- Preserve unrelated working-tree changes and stage only task-owned files for commits.

---

### Task 1: Confirm current graph authority and focused baseline

**Files:**
- Read: `src/egl_renderer/effects/executor.rs`
- Read: `src/egl_renderer/effects/trace.rs`
- Read: `src/egl_renderer.rs`

**Interfaces:**
- Confirm Codebase Memory paths for `plan_effect_surface_consumers_with_debug_config`, `scene_replay_work_plan`, `SceneReplayWorkState`, `execute_graph_passes_inner`, `draw_effect_scene_range`, and `checkpoint_source_semantic_validity`.
- Confirm both planner and executor use `SceneReplayWorkState.active_work()` and that executor retirement follows successful `execute_pass`.

- [ ] **Step 1: Run graph status and targeted searches.**

Run Codebase Memory `list_projects`, `index_status`, `search_graph`, `trace_path`, `get_code_snippet`, and `check_index_coverage` for both target source files. Record the current generation and any coverage caveat.

- [ ] **Step 2: Run the focused existing tests.**

Run:

```bash
rtk cargo test --bin oblivion-one native_faithful_stacked_replay_baseline_matches_suffix_demand -- --nocapture
rtk cargo test --bin oblivion-one native_faithful_topbar_baseline_matches_suffix_demand -- --nocapture
rtk cargo test --bin oblivion-one native_three_checkpoint_suffix_demand_replays_pending_suffix -- --nocapture
rtk cargo test --bin oblivion-one native_same_anchor_suffix_demand_retains_later_checkpoint -- --nocapture
rtk cargo test --bin oblivion-one surface_consumer_plan_matches_scene_replay_work_state -- --nocapture
rtk cargo test --bin oblivion-one scene_replay_work_same_anchor_uses_pass_lifetime_not_cursor_position -- --nocapture
```

Expected: each command runs one test and exits successfully. If a focused test fails, stop implementation and report the concrete correctness failure before changing production code.

- [ ] **Step 3: Commit only if this task discovers no source gap.**

Do not create a no-op source commit. Keep the design commit separate from implementation commits.

### Task 2: Add the release-style planner under-planning regression

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs` near `surface_consumer_plan_trailing_pending_requirements_use_baseline_work`
- Test: the same Rust test module under `#[cfg(not(debug_assertions))]`

**Interfaces:**
- Consume `SceneReplayWorkPlan`, `SceneReplayWorkState`, `finalize_surface_consumer_trailing_work`, `add_surface_consumers_for_command_range`, and `SurfaceConsumerPlan`.
- Produce a focused assertion that the conservative trailing plan contains surfaces from presentation work and every still-required checkpoint region.

- [ ] **Step 1: Extend the failing regression assertion before changing production code.**

Build three real `EglDrawCommand` values whose bounds match the presentation, retired checkpoint, and still-pending checkpoint regions. Keep the intentionally inconsistent state:

```rust
let mut state = SceneReplayWorkState::new(&plan, SceneReplayWorkMode::SuffixDemand);
assert!(state.mark_capture_satisfied(GraphPassId::new(1).unwrap()));
assert_eq!(state.pending_checkpoint_requirements(), 1);
```

Finalize trailing work, pass it through `add_surface_consumers_for_command_range`, call `finish`, and assert that `surface_ids()` includes the surface IDs for presentation, checkpoint A, and checkpoint B. The test must exercise plan materialization, not only compare rectangles.

- [ ] **Step 2: Run the release-style test and inspect the result.**

Run:

```bash
rtk cargo test --release --bin oblivion-one surface_consumer_plan_trailing_pending_requirements_use_baseline_work -- --nocapture
```

Expected: the test passes with the current conservative fallback. If it fails, treat it as a concrete planner correctness defect and only then adjust the existing finalization path.

- [ ] **Step 3: Run the debug invariant test.**

Run:

```bash
rtk cargo test --bin oblivion-one surface_consumer_plan_trailing_pending_requirements_expose_invariant_failure -- --nocapture
```

Expected: the `should_panic` test passes, proving debug/test mode exposes pending requirements instead of silently shrinking work.

- [ ] **Step 4: Commit the focused planner regression.**

```bash
git add src/egl_renderer/effects/executor.rs
git commit -m "test: prove conservative trailing surface planning"
```

### Task 3: Verify executor regressions and RED sensitivity

**Files:**
- Read/modify temporarily: `src/egl_renderer.rs`
- Read/modify temporarily: `src/egl_renderer/effects/executor.rs`

**Interfaces:**
- Use existing `render_native_stacked_candidate_with_mode`, `render_native_three_checkpoint_candidate_with_mode`, `assert_native_baseline_matches_suffix_demand`, and bounded trace fields.

- [ ] **Step 1: Run Dock and TopBar A/B regressions.**

Verify final framebuffer equivalence, all checkpoint validity events with `missing_pixels=0`, baseline `saved_pixels=0`, suffix `saved_pixels > 0`, and TopBar checkpoint mode/policy/dependency assertions.

- [ ] **Step 2: Run the multi-checkpoint regression.**

Verify three compiler-produced captures, distinct A→C→B command positions, A dependency count zero, C dependency count at least one, B dependency count at least one (preferably at least two), progressive pending counts `>=2 → >=1 → 0`, active-work shrink after C, final presentation-only work, zero missing pixels, and baseline/suffix pixel equivalence.

- [ ] **Step 3: Run the same-anchor regression.**

Verify C and B share a cursor/anchor, C’s capture-satisfied event leaves one pending requirement, B’s leaves zero, both satisfaction events retain the same cursor range, later checkpoint validity is present with `missing_pixels=0`, and baseline/suffix pixels match.

- [ ] **Step 4: Perform the required temporary RED mutations.**

Temporarily mutate only the local checkout, run the affected focused test, and record the exact failure:

```text
multi-checkpoint: retire B when C succeeds, expecting missing_pixels > 0,
InvalidCheckpointSource, pixel mismatch, or trace invariant failure
same-anchor: retire by cursor/anchor position, expecting pending-count or
later-checkpoint pixel/validity failure
```

Restore the production implementation immediately after each RED run. Do not add flags, weaken assertions, or commit the unsafe mutation. Re-run both focused tests after restoration.

### Task 4: Run verification gates and authority audit

**Files:**
- Read: `src/egl_renderer/effects/executor.rs`
- Read: `src/egl_renderer/effects/trace.rs`
- Read: `src/egl_renderer.rs`

- [ ] **Step 1: Run the requested suffix-demand and semantic tests.**

Run the pure `SceneReplayWorkState` suite, planner parity tests, Dock/TopBar native-faithful tests, multi-checkpoint and same-anchor GLES tests, native checkpoint correctness, multi-dependency semantic validity, exact-region scene-work preservation, and framebuffer-origin preservation mapping.

- [ ] **Step 2: Run formatting, check, and clippy.**

```bash
rtk cargo fmt --check
rtk cargo check --workspace --all-targets
rtk cargo clippy --workspace --all-targets -- -D warnings
```

Record total clippy failures, failures in files modified by this task, and pre-existing failures elsewhere. Do not repair unrelated clippy debt.

- [ ] **Step 3: Re-run Codebase Memory after edits.**

Use `index_status`, then search/trace/coverage for the target files. Confirm both planner and executor remain connected to `SceneReplayWorkState`, and that no production `draw_effect_scene_range` path bypasses `scene_work_state.active_work()` except explicitly conservative baseline lifecycle/debug paths.

- [ ] **Step 4: Run the native workload if hardware is available.**

Use the requested `1920x1080@165`, replay capture, partial Kawase, effect execution trace, and GPU timing environment. Exercise Dock, TopBar, simultaneous blurred Shell elements, motion, partial repaint, and stacked checkpoint-dependent frames. Collect exact traced work-pixel totals and GPU timings separately; report visual correctness.

- [ ] **Step 5: Commit verified source changes.**

```bash
git status --short
git diff --check
git add src/egl_renderer/effects/executor.rs src/egl_renderer.rs src/egl_renderer/effects/trace.rs
git commit -m "test: harden suffix-demand execution verification"
```

Stage only files changed for this task; leave the pre-existing compositor/native-output edits and untracked logs untouched.
