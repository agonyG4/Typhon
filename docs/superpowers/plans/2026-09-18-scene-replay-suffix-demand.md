# Scene Replay Suffix-Demand Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make replay-mode ordinary scene replays use only presentation work plus direct framebuffer checkpoint regions whose capture passes have not successfully executed, while preserving baseline clear, exact-region preservation, semantic validity, and final pixels.

**Architecture:** Build a static `SceneReplayWorkPlan` from the existing scene-work geometry authority, then drive one `SceneReplayWorkState` through executor and surface-consumer planning in graph-pass order. Replay uses suffix demand; framebuffer diagnostics and lifecycle execution use the existing global baseline. Retire requirements only after successful direct capture execution and replace, rather than union, scene validity after each non-empty scene advancement.

**Tech Stack:** Rust, GLES executor tests, existing `EffectRegion` and `OutputDamage` bounded rectangle machinery, existing native-faithful diagnostic fixtures, `rtk cargo` verification commands.

## Global Constraints

- `baseline_work` must remain exactly equivalent to the current `scene_work_rects` calculation.
- Initial scene clear continues to use baseline work.
- `extra_scene_work` and exact-region preservation remain unchanged.
- Requirements expire only after their direct framebuffer capture pass executes successfully.
- Active work must be exact-region monotonic and never use bounding-box subset checks.
- `checkpoint_source_semantic_validity` remains the safety mechanism and is not weakened.
- Capture-index consumer planning, Kawase, capture domains, preservation storage, graph dependencies, and unrelated compositor code are out of scope.
- Do not add a user-facing environment variable or production legacy mode.
- Compile in `/home/agony/GitHub/Typhon`.
- Do not revert or stage unrelated existing worktree changes.

---

### Task 1: Add the pure suffix-demand work model and planner-proof tests

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs:3660-3745` for the static plan, state, normalization helper, and unit tests in the existing `#[cfg(test)] mod tests`.

**Interfaces:**
- Produce `SceneCheckpointRequirement { capture_pass: GraphPassId, region: Vec<OutputRect> }`.
- Produce `SceneReplayWorkPlan { presentation_work: Vec<OutputRect>, baseline_work: Vec<OutputRect>, extra_scene_work: Vec<OutputRect>, checkpoint_requirements: Vec<SceneCheckpointRequirement> }`.
- Produce `SceneReplayWorkMode::{GlobalBaseline, SuffixDemand}` and `SceneReplayWorkState<'a>` with `new`, `active_work`, `pending_checkpoint_requirements`, `mark_capture_satisfied`, `force_baseline`, and exact monotonicity checking.
- Produce `scene_replay_work_plan(...) -> SceneReplayWorkPlan` using the existing direct-capture and region helpers.

- [ ] **Step 1: Write the failing pure model tests first.** Add tests that construct a small plan with presentation `P` and requirement regions `A`/`B`, then assert these exact transitions:

```rust
let mut state = SceneReplayWorkState::new(&plan, SceneReplayWorkMode::SuffixDemand);
assert_work_eq(&state.active_work(), &[p, a, b]);
state.mark_capture_satisfied(a_pass);
assert_work_eq(&state.active_work(), &[p, b]);
state.mark_capture_satisfied(b_pass);
assert_work_eq(&state.active_work(), &[p]);
```

Cover one checkpoint, disjoint checkpoints, overlapping checkpoints (the overlap survives while B is pending), same-anchor pass IDs (A retirement does not retire B), presentation containment, no checkpoints, exact subset monotonicity, and a strict reduction in active pixel count after A succeeds. Add a global-baseline assertion that active work never changes.

- [ ] **Step 2: Run only the new tests and confirm the expected feature-missing failure.**

Run:

```bash
rtk cargo test -p typhon egl_renderer::effects::executor::tests::scene_replay_work -- --nocapture
```

Expected: compilation/test failure because the new plan/state types and methods do not yet exist; fix only test naming or syntax issues if needed, and do not add production behavior before observing this failure.

- [ ] **Step 3: Implement the static plan with the existing geometry authority.** Replace the frame-global-only `SceneWorkRegions` representation with the plan. Enumerate selected passes using the current `is_direct_framebuffer_capture`, `RenderPassKind` filter, output texture lookup, and `clipped_output_rect`. Normalize baseline and active work by calling `OutputDamage::rects`, `disjoint_output_rects`, and `full_output_rect`; compute `extra_scene_work` by the current exact subtraction loop. Keep all normalized regions bounded by `MAX_EFFECT_REGION_RECTS` through `disjoint_output_rects` fallback.

- [ ] **Step 4: Implement state transitions and the exact monotonic debug invariant.** Initialize every requirement as pending. In suffix mode, combine presentation plus pending requirement regions through the shared normalization helper. In global mode, expose baseline. `mark_capture_satisfied` must identify the pass ID, remove only that requirement, recompute active work, and `debug_assert` that `next.subtract(previous).is_empty()` using `output_rects_to_effect_region`; do not compare bounding boxes. `force_baseline` must be available for production fallback.

- [ ] **Step 5: Run the pure tests and confirm they pass with a strict reduction.**

Run:

```bash
rtk cargo test -p typhon egl_renderer::effects::executor::tests::scene_replay_work -- --nocapture
```

Expected: all work-lifetime tests pass, including the same-anchor test and the strict active-pixel reduction.

- [ ] **Step 6: Commit the pure model and tests.**

```bash
git add src/egl_renderer/effects/executor.rs
git commit -m "test: prove scene replay suffix demand"
```

### Task 2: Make surface-consumer planning use the same state transitions

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs:555-662`.
- Modify: `src/egl_renderer/effects/executor.rs` tests for planner parity.

**Interfaces:**
- `plan_effect_surface_consumers_with_debug_config` will build the same `SceneReplayWorkPlan` and use `SceneReplayWorkState` with `scene_replay_work_mode(false, debug_config.capture_mode())`.
- The existing `add_surface_consumers_for_capture_indices` call remains unchanged.

- [ ] **Step 1: Add a failing parity test.** Build an equivalent ordered pass fixture with two checkpoint captures and command ranges whose surfaces are unique to P, A, and B. Capture the active-work snapshots used by the planner and compare them to the pure executor-state snapshots at the same boundaries; assert the planner retains B after A is retired and retains same-anchor B after same-anchor A is retired. Also assert the consumer plan includes every surface that intersects any active range.

- [ ] **Step 2: Run the parity test and verify it fails before planner wiring.**

Run:

```bash
rtk cargo test -p typhon egl_renderer::effects::executor::tests::surface_consumer_plan -- --nocapture
```

Expected: the new parity assertion fails because the planner still passes global baseline work to every range.

- [ ] **Step 3: Wire planner ranges to the shared state.** Replace `scene_work.scene_work_rects` with `work_state.active_work()` for checkpoint-dependency ranges, composite ranges, and the final command range. After the planner’s direct-capture bookkeeping completes, call `mark_capture_satisfied(pass.id)` exactly as the executor will after successful capture execution. Keep capture-index materialization arguments and `indices_for_capture` unchanged.

- [ ] **Step 4: Preserve diagnostic planner behavior.** Select `GlobalBaseline` for framebuffer debug capture and `SuffixDemand` only for replay checkpoint captures. If a planner inconsistency leaves a requirement pending at completion, use baseline work for the trailing range and assert the invariant in debug/test builds.

- [ ] **Step 5: Run planner parity and existing consumer tests.**

```bash
rtk cargo test -p typhon egl_renderer::effects::executor::tests::surface_consumer_plan -- --nocapture
rtk cargo test -p typhon effect_surface_consumer_plan -- --nocapture
```

Expected: planner snapshots match the shared state and all existing consumer-planning tests remain green.

- [ ] **Step 6: Commit planner parity.**

```bash
git add src/egl_renderer/effects/executor.rs
git commit -m "perf: share suffix scene work with consumer planning"
```

### Task 3: Integrate suffix demand into executor replay and validity

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs:862-1389`.
- Modify: `src/egl_renderer/effects/executor.rs` executor tests.

**Interfaces:**
- Add a private policy selector that returns `GlobalBaseline` for lifecycle or framebuffer diagnostic capture and `SuffixDemand` for ordinary replay checkpoint execution.
- `execute_graph_passes_inner` will own one `SceneReplayWorkState` for all scene advancement reasons and final replay.
- Add `EffectExecutionInvariantError::PendingSceneCheckpointRequirements` for debug/test completion failures.

- [ ] **Step 1: Add failing executor-level tests for same-anchor lifetime and post-capture validity.** Use the existing graph/pass helpers to assert that after A executes, with `scene_cursor == B`’s anchor, B remains pending and its region is still active. Add a validity-focused test showing that after an active-work shrink, the next non-empty scene advance replaces `scene_valid_region` with the new active region instead of retaining expired A.

- [ ] **Step 2: Run the new executor tests and confirm they fail against the global-work executor.**

```bash
rtk cargo test -p typhon egl_renderer::effects::executor::tests::same_anchor -- --nocapture
rtk cargo test -p typhon egl_renderer::effects::executor::tests::scene_valid -- --nocapture
```

Expected: the same-anchor or active-work assertion fails because execution currently uses one global region and accumulates scene validity.

- [ ] **Step 3: Use baseline work for clear and preservation.** Replace executor references used by `clear_effect_scene_work` and `capture_scene_work_preservation` with `plan.baseline_work` and `plan.extra_scene_work`. Do not change reconstruction conditions, preservation texture layout, blit domains, or restore ordering.

- [ ] **Step 4: Use state-owned active work for every ordinary replay.** Instantiate the state before the execution closure. Pass `work_state.active_work()` to all `draw_effect_scene_range` calls for checkpoint dependency, framebuffer capture, composite advance, and final replay. After each successful non-empty draw, assign `scene_valid_region = output_rects_to_effect_region(work_state.active_work())`; do not union stale work.

- [ ] **Step 5: Retire only after successful direct capture execution.** Leave all pre-execute semantic validity and resource validation unchanged. After `execute_pass(...)` returns `Ok(())`, call `work_state.mark_capture_satisfied(pass.id)` for direct framebuffer captures. On an error, return before any retirement. Keep effect-valid-region updates and current-frame output recording unchanged.

- [ ] **Step 6: Add completion handling.** After all selected passes and before final replay, if suffix mode has pending requirements, emit the invariant failure. In debug/test builds return the invariant error. In production call `force_baseline()` and use baseline for final replay. The normal successful path must assert no selected checkpoint requirement remains pending after graph pass execution.

- [ ] **Step 7: Run focused executor tests and semantic-validity tests.**

```bash
rtk cargo test -p typhon egl_renderer::effects::executor -- --nocapture
rtk cargo test -p typhon checkpoint_semantic_validity -- --nocapture
```

Expected: pure/state tests, existing scene-work tests, checkpoint validity tests, and coordinate tests pass.

- [ ] **Step 8: Commit executor integration.**

```bash
git add src/egl_renderer/effects/executor.rs
git commit -m "perf: expire completed checkpoint scene work"
```

### Task 4: Extend bounded scene replay tracing

**Files:**
- Modify: `src/egl_renderer/effects/trace.rs:321-360`.
- Modify: `src/egl_renderer/effects/trace.rs` trace tests.
- Modify: `src/egl_renderer/effects/executor.rs` call sites.

**Interfaces:**
- Extend `scene_replay_boundary` and `final_scene_replay_boundary` with active/baseline rectangle counts, active/baseline exact pixel totals, saved pixels, and pending checkpoint count.

- [ ] **Step 1: Add failing trace-format tests.** Assert a bounded ordinary replay event contains `active_work_rects`, `active_work_pixels`, `baseline_work_rects`, `baseline_work_pixels`, `saved_pixels`, and `pending_checkpoint_requirements`, and contains no `active_work=` or rectangle-list payload.

- [ ] **Step 2: Run the trace test and confirm it fails because the fields are absent.**

```bash
rtk cargo test -p typhon trace::tests::scene_replay -- --nocapture
```

- [ ] **Step 3: Add the fields and update executor call sites.** Pass precomputed exact pixel sums from disjoint work rectangles; never compute or emit bounding-box area. Keep existing cursor and command fields unchanged. Add the same metrics to final replay events, with pending count observed at that exact boundary.

- [ ] **Step 4: Run all trace tests.**

```bash
rtk cargo test -p typhon trace -- --nocapture
```

- [ ] **Step 5: Commit tracing.**

```bash
git add src/egl_renderer/effects/trace.rs src/egl_renderer/effects/executor.rs
git commit -m "perf: trace exact scene replay work"
```

### Task 5: Add baseline-vs-suffix GLES equivalence and multi-checkpoint regressions

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs` for the test-only/internal replay strategy hook.
- Modify: `src/egl_renderer.rs` around the existing native stacked diagnostic fixture and GLES tests.

**Interfaces:**
- Add an internal/test-only execution selector for `GlobalBaseline` versus `SuffixDemand`; do not read an environment variable and do not expose production configuration.
- Keep `execute_effect_graph_with_debug_config` production behavior selecting suffix demand in replay mode.

- [ ] **Step 1: Add the failing native equivalence regression before the selector is wired.** Run the existing heterogeneous TopLeftScanout partial-Kawase stacked fixture twice with the same scene and repair: one global-baseline run and one suffix-demand run. Assert final pixel equality using the existing GLES tolerance and assert `missing_pixels=0` for every selected framebuffer checkpoint. Reuse the existing Dock and TopBar expected domains and do not change their reference pixels.

- [ ] **Step 2: Add the failing multi-checkpoint work regression.** Use a non-uniform source scene with checkpoint A and B at different composition positions, with at least one ordinary command between them. Collect replay trace events and assert both regions are active before A, A-exclusive work disappears after A, B remains active through the intervening update, and B capture has `missing_pixels=0` and equals the global-baseline output.

- [ ] **Step 3: Run the new focused GLES tests and observe the expected failure.**

```bash
rtk cargo test -p typhon native_faithful -- --nocapture
rtk cargo test -p typhon multi_checkpoint -- --nocapture
```

Expected: the new baseline/suffix comparison or work-metric assertion fails before the production selector and state are integrated.

- [ ] **Step 4: Wire the test-only strategy without changing production configuration.** Thread the private mode through the internal executor entry used by tests, while the normal entry maps replay to suffix demand and diagnostic/lifecycle paths to global baseline. Ensure both modes share plan construction, pass ordering, capture execution, semantic validity, preservation, and final rendering.

- [ ] **Step 5: Run the focused GLES regressions green.**

```bash
rtk cargo test -p typhon native_faithful_stacked_dock_checkpoint_replay_partial_matches_full_current_reference -- --nocapture
rtk cargo test -p typhon native_faithful_topbar_checkpoint_replay_partial_matches_full_current_reference -- --nocapture
rtk cargo test -p typhon native_faithful_full_kawase_dock_control -- --nocapture
rtk cargo test -p typhon multi_checkpoint -- --nocapture
```

Expected: Dock, TopBar, full-Kawase control, multi-checkpoint, semantic-validity, and missing-pixel assertions pass without changing fixture expectations.

- [ ] **Step 6: Commit the correctness regressions.**

```bash
git add src/egl_renderer/effects/executor.rs src/egl_renderer.rs
git commit -m "test: prove suffix replay pixel equivalence"
```

### Task 6: Run the requested verification and inspect the diff

**Files:**
- Verify: `src/egl_renderer/effects/executor.rs`, `src/egl_renderer/effects/trace.rs`, `src/egl_renderer.rs`, and the two committed docs.

- [ ] **Step 1: Run focused correctness suites.**

```bash
rtk cargo test -p typhon scene_replay_work -- --nocapture
rtk cargo test -p typhon effect_surface_consumer_plan -- --nocapture
rtk cargo test -p typhon checkpoint_semantic_validity -- --nocapture
rtk cargo test -p typhon scene_work_preservation -- --nocapture
rtk cargo test -p typhon framebuffer_origin -- --nocapture
rtk cargo test -p typhon fullscreen -- --nocapture
rtk cargo test -p typhon capture -- --nocapture
```

- [ ] **Step 2: Run formatting, workspace check, and Clippy exactly as requested.**

```bash
rtk cargo fmt --check
rtk cargo check --workspace --all-targets
rtk cargo clippy --workspace --all-targets -- -D warnings
```

Record the total Clippy failures and whether any diagnostic originates in modified files. Do not repair unrelated pre-existing warnings.

- [ ] **Step 3: Inspect repository state and diff.**

```bash
git diff --check
git status --short
git diff --stat HEAD~5..HEAD
```

Confirm no unrelated worktree files were staged and no out-of-scope subsystem changed.

- [ ] **Step 4: Run the native benchmark gate only if automated correctness is green.** Exercise the requested 1920x1080@165 replay/partial-Kawase workload with execution trace and GPU timing enabled. Collect matching partial checkpoint frames, `missing_pixels=0`, exact baseline/active/saved replay pixels, distributions, and observational GPU timing. Report visual results without claiming pixel reduction equals time reduction. If the traces show little or no reduction, report that honestly.

- [ ] **Step 5: Commit the verified implementation if checks permit.**

```bash
git add src/egl_renderer/effects/executor.rs src/egl_renderer/effects/trace.rs src/egl_renderer.rs docs/superpowers/specs/2026-09-18-scene-replay-suffix-demand-design.md docs/superpowers/plans/2026-09-18-scene-replay-suffix-demand.md
git commit -m "perf: expire completed checkpoint replay work"
```

