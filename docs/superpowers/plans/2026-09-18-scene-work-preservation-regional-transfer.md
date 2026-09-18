# Scene-Work Preservation Regional Transfer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace full-output scene-work preservation capture with exact-region framebuffer blits while retaining the output-sized pooled texture and all checkpoint semantics.

**Architecture:** `executor.rs` will build one pure `SceneWorkPreservationPlan` from `extra_scene_work` through the existing coordinate helper, store that plan with the single pooled preservation texture, and use it for both capture and restore. `trace.rs` will add one bounded preservation metric event per successful phase. Existing execution ordering, error cleanup, and scene-work geometry remain unchanged.

**Tech Stack:** Rust 2024, Glow GLES3, EGL surfaceless test harness, Cargo workspace, `rtk` command proxy.

## Global Constraints

- Keep acquiring `EffectTextureKey::new(output_width, output_height, Rgba8, Nearest, OutputEncodedSrgb)`.
- Do not introduce compact preservation textures, bounding-box conversion, fallback thresholds, per-checkpoint timelines, partial checkpoint capture, Kawase changes, or blur redesign.
- Use `scene_work_preservation_blit_rects` as the only framebuffer-origin coordinate conversion.
- Bind framebuffer state once per capture/restore phase and issue one blit per planned rectangle.
- Preserve cleanup and error precedence; never leak a checked-out `PooledEffectTexture`.
- Do not add timing assertions; report transfer quantity and keep native GPU timing observational.
- Use `rtk` for repository commands, compile in the repository's normal `target` directory, do not spawn subagents, and commit only explicitly staged files.

---

### Task 1: Add the pure preservation transfer plan with RED tests

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs:3044-3079` for the plan and resource representation.
- Modify: `src/egl_renderer/effects/executor.rs:5988-6007` for focused pure tests.

**Interfaces:**
- Consumes: `OutputRect`, `(u32, u32)`, `OutputFramebufferOrigin`, and `scene_work_preservation_blit_rects`.
- Produces: `SceneWorkPreservationPlan { transfers: Vec<GraphTextureCaptureBlit>, pixels: u64 }` and `SceneWorkPreservation { texture, plan }`.

- [ ] **Step 1: Write the failing pure-plan tests.** Add tests that call the planned constructor for `(1920,1080)` and assert: Dock `(762,976,396,104)` yields one transfer and `41_184` pixels for both origins; TopBar `(0,0,120,65)` yields `7_800`; Dock plus TopBar yields two transfers and `48_984` pixels; `(-10,-5,30,20)` clipped into `(100,80)` yields `300` pixels; and an empty slice yields no transfers and zero pixels. Keep `scene_work_preservation_maps_framebuffer_origins` and add plan-origin assertions if useful.
- [ ] **Step 2: Run the focused tests and verify the expected RED failure.**

  Run: `rtk cargo test --lib egl_renderer::effects::executor::tests::scene_work_preservation_plan -- --nocapture`

  Expected: compile/test failure because `SceneWorkPreservationPlan` and its constructor do not yet exist.
- [ ] **Step 3: Implement the minimal plan.** Add a pure constructor that calls `scene_work_preservation_blit_rects` once per input rectangle, retains each successful transfer in order, and accumulates transfer width × height with saturating `u64` arithmetic. Add a transfer-pixel helper based on the destination rectangle dimensions. Store the plan in `SceneWorkPreservation` without changing the pooled texture key.
- [ ] **Step 4: Run the focused plan tests and verify GREEN.**

  Run: `rtk cargo test --lib egl_renderer::effects::executor::tests::scene_work_preservation_plan -- --nocapture`

  Expected: all plan tests pass, including both framebuffer origins, clipping, disjoint rectangles, and empty work.
- [ ] **Step 5: Commit the pure plan.**

  Run: `rtk git add src/egl_renderer/effects/executor.rs`

  Run: `rtk git commit -m "perf: plan regional scene-work preservation"`

### Task 2: Add bounded preservation trace metrics with RED tests

**Files:**
- Modify: `src/egl_renderer/effects/trace.rs` near the existing bounded event methods and trace tests.

**Interfaces:**
- Consumes: phase (`capture` or `restore`), rectangle count, transferred pixels, and output pixels.
- Produces: one `event=effect_scene_work_preservation` line per successful phase when the existing effect trace is enabled.

- [ ] **Step 1: Write the failing trace-format test.** Add a trace test that enables the test trace, emits a capture event with two rectangles, `48_984` transferred pixels, and `2_073_600` output pixels, then asserts the event contains `phase=capture`, `rect_count=2`, `pixels=48984`, and `output_pixels=2073600` and contains no rectangle list.
- [ ] **Step 2: Run the trace test and verify RED.**

  Run: `rtk cargo test --lib egl_renderer::effects::trace::tests::scene_work_preservation_trace -- --nocapture`

  Expected: compile/test failure because the trace method does not yet exist.
- [ ] **Step 3: Implement the bounded trace method.** Add `EffectExecutionTrace::scene_work_preservation` using the existing `event` gate and a single bounded formatted line. Do not add an environment variable, coordinates, or an unbounded list.
- [ ] **Step 4: Run the trace test and verify GREEN.**

  Run: `rtk cargo test --lib egl_renderer::effects::trace::tests::scene_work_preservation_trace -- --nocapture`

  Expected: PASS with the exact bounded fields.
- [ ] **Step 5: Commit the trace metric.**

  Run: `rtk git add src/egl_renderer/effects/trace.rs`

  Run: `rtk git commit -m "feat: trace scene-work preservation quantity"`

### Task 3: Switch capture and restore to the stored plan

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs:892-918` to pass `extra_scene_work` into capture.
- Modify: `src/egl_renderer/effects/executor.rs:1352-1388` to restore from the stored plan.
- Modify: `src/egl_renderer/effects/executor.rs:3081-3181` to capture and restore each planned transfer.

**Interfaces:**
- Consumes: `SceneWorkPreservationPlan` from Task 1 and `EffectExecutionTrace::scene_work_preservation` from Task 2.
- Produces: one output-sized preservation texture per execution, regional capture/restore with one framebuffer bind per phase, and preserved cleanup/error precedence.

- [ ] **Step 1: Add a failing execution-level assertion for stored-plan usage.** Extend the plan/resource unit coverage so a `SceneWorkPreservation` owns the plan and restore has no `extra_scene_work` parameter to recompute. The test must construct two disjoint transfers and assert the stored plan remains unchanged after the planned restore iteration setup.
- [ ] **Step 2: Run the focused executor tests and verify RED.**

  Run: `rtk cargo test --lib egl_renderer::effects::executor::tests::scene_work_preservation -- --nocapture`

  Expected: compile failure from the old capture/restore signatures or missing stored-plan fields.
- [ ] **Step 3: Implement regional capture.** Change capture to receive `&[OutputRect]`, build the plan before acquiring the texture, acquire the unchanged output-sized key once, bind the active output framebuffer and preservation draw target once, and call `blit_framebuffer` once for each `plan.transfers` entry. Keep scissor disabled only once, restore ordinary state after the operation, release the texture on any capture error, and return the original error.
- [ ] **Step 4: Implement plan-based restore and trace calls.** Remove the independent `extra_scene_work` restore argument. Iterate `preservation.plan.transfers` exactly, keep the existing framebuffer-origin mapping unchanged, release the texture through the existing caller paths, and emit capture/restore metrics with `plan.transfers.len()`, `plan.pixels`, and output pixel count only after the corresponding phase completes.
- [ ] **Step 5: Run focused executor tests and verify GREEN.**

  Run: `rtk cargo test --lib egl_renderer::effects::executor::tests::scene_work_preservation -- --nocapture`

  Expected: PASS, with existing framebuffer-origin mapping still green and no ordinary empty-work preservation allocation path.
- [ ] **Step 6: Commit the execution-path change.**

  Run: `rtk git add src/egl_renderer/effects/executor.rs`

  Run: `rtk git commit -m "perf: capture only scene-work preservation regions"`

### Task 4: Add the real GLES regional pixel regression

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs` to expose only the smallest crate-test-visible capture/restore/release seam needed by the existing GLES test module.
- Modify: `src/egl_renderer.rs` near the existing `GlesEffectTestHarness` tests.

**Interfaces:**
- Consumes: the real existing `GlesEffectTestHarness`, `capture_scene_work_preservation`, `restore_scene_work_preservation`, and the stored plan.
- Produces: a non-uniform framebuffer regression for two disjoint regions under both `TopLeftScanout` and `BottomLeft`.

- [ ] **Step 1: Write the failing GLES regression.** Use an 8×6 harness output, fill a non-uniform original pattern, capture logical rectangles `(1,1,2,2)` and `(5,3,2,2)`, overwrite the whole framebuffer with a distinct solid color, restore, and assert through the existing origin-aware pixel helper that every pixel inside either region equals the original and every outside pixel equals the overwrite for both origins. Release the single preservation resource through the test-visible seam.
- [ ] **Step 2: Run the regression and verify RED.**

  Run: `rtk cargo test --lib egl_renderer::tests::real_gles_scene_work_preservation_restores_only_planned_regions -- --nocapture`

  Expected: compile failure until the test-visible seam and the new capture signature are available. The test must also assert the returned stored plan has two transfers and `8` pixels before the overwrite, so the old full-output capture cannot pass by relying only on restore's existing regional loop.
- [ ] **Step 3: Add the minimal test-visible seam.** Keep production execution unchanged in ownership and cleanup; expose only crate-test-visible access needed to obtain, restore, and release one preservation resource. Do not duplicate framebuffer mapping or introduce a second implementation.
- [ ] **Step 4: Run the GLES test and verify GREEN.**

  Run: `rtk cargo test --lib egl_renderer::tests::real_gles_scene_work_preservation_restores_only_planned_regions -- --nocapture`

  Expected: PASS for both framebuffer origins, demonstrating preserved regions match the original and all other pixels retain the overwrite.
- [ ] **Step 5: Commit the GLES regression.**

  Run: `rtk git add src/egl_renderer/effects/executor.rs src/egl_renderer.rs`

  Run: `rtk git commit -m "test: cover regional scene-work preservation"`

### Task 5: Verify correctness, work quantity, lint ownership, and native evidence

**Files:**
- Read and verify the committed changes; do not modify unrelated dirty-worktree files.

- [ ] **Step 1: Run focused automated correctness tests.** Exercise the plan/origin/GLES tests, Dock and TopBar native-faithful checkpoint regressions, full-Kawase control, multi-dependency checkpoint validity, SurfaceConsumerPlan, capture coordinate-space regressions, fullscreen regressions, and all four diagnostic capture/Kawase configurations. Record exact pass/fail counts and any environment-gated skips.
- [ ] **Step 2: Run formatting and workspace compilation.**

  Run: `rtk cargo fmt --check`

  Run: `rtk cargo check --workspace --all-targets`

  Expected: both commands exit successfully without changing the normal repository-local target directory.
- [ ] **Step 3: Run repository-wide Clippy and classify failures.**

  Run: `rtk cargo clippy --workspace --all-targets -- -D warnings`

  Record total failures and whether any diagnostic points into `src/egl_renderer/effects/executor.rs`, `src/egl_renderer/effects/trace.rs`, or the directly related test changes. Do not repair unrelated pre-existing lint debt.
- [ ] **Step 4: Inspect the final diff and graph coverage.** Confirm no changes to render-graph semantics, resource-pool architecture, Kawase shaders, damage planner, Shell/Eclipse, checkpoint geometry, dependency propagation, or scene cursor behavior. Use `check_index_coverage` for every modified source path and read any reported missed ranges directly.
- [ ] **Step 5: Run the native hardware gate.** With `TYPHON_EFFECT_EXEC_TRACE=1 TYPHON_EFFECT_GPU_TIMING=1`, exercise the known `1920x1080@165`, `capture=replay`, `Kawase=partial` workload across Dock, TopBar, simultaneous Shell blur, and checkpoint-dependent partial frames. Collect matching frames requiring `repaint_mode=partial`, `backdrop_capture_policy=replay`, `kawase_execution_policy=partial`, `checkpoint_dependencies>0`, `capture_mode=framebuffer_blit`, and `missing_pixels=0`; record preservation `rect_count`, `pixels`, full-output equivalent pixels, and reduction ratio. Report GPU timing only as observation, not as a claimed speedup.
- [ ] **Step 6: Finish with the verified commits.**

  Run: `rtk git status --short`

  Run: `rtk git diff --check HEAD~3..HEAD`

  Do not stage unrelated existing edits. If verification requires a code correction, repeat the relevant RED-GREEN cycle and make a focused follow-up commit containing only the preservation implementation/test files.
