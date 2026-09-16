# Diagnostic Framebuffer Safety Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. This task is explicitly executed inline because the user forbids subagents.

**Goal:** Make framebuffer capture diagnostics preserve all scene-work pixels outside presentation repair, share scene-work geometry with surface resource planning, and exercise all four debug policies through deterministic GLES tests.

**Architecture:** Keep `effect_debug_config()` as the production `OnceLock` wrapper and add explicit-config executor/planner entry points for tests. Add a pure `SceneWorkRegions` helper in `executor.rs`; both scene replay and consumer planning use its `scene_work_rects`. During ordinary framebuffer diagnostics, an explicitly owned per-execution preservation texture captures the active output before extra scene work is cleared, restores only `extra_scene_work` after final replay, and is released before overlays.

**Tech Stack:** Rust, glow GLES3, surfaceless Mesa EGL pbuffer test harness, existing `EffectResourcePool`, existing `CompiledFrameGraph` and `SurfaceConsumerPlan`.

## Global Constraints

- Do not implement `PersistentBackdropCache`.
- Do not continue into the production backdrop architecture.
- Keep ordinary presentation damage unchanged.
- Do not broaden final `Composite` or `OutputPostProcess` clipping.
- Do not use `glFinish()` or `glFlush()` as a workaround.
- Preserve `SurfaceCapture` semantics, raster-aware coverage fixes, visible-output clipping, bounded regions, checkpoint ordering, and planner-level full-Kawase behavior.
- Compile in `/home/agony/GitHub/Typhon` so build artifacts stay in the current checkout.
- Do not mutate process environment variables concurrently in tests.
- Commit each independently verified task in this Git repository.

## File Map

- Modify `src/egl_renderer/effects/trace.rs`: add an explicit constructor for immutable test configs while retaining the process-scoped environment wrapper.
- Modify `src/egl_renderer/effects/executor.rs`: add explicit config threading, the shared scene-work helper, preservation capture/restore, and executor/planner regressions.
- Modify `src/egl_renderer/effects/mod.rs`: re-export explicit testable config and execution/planning entry points within the crate.
- Modify `src/egl_renderer.rs`: use one production debug config for effect demand, consumer planning, and execution; add deterministic GLES RED and matrix pixel tests using the existing harness.
- Modify `src/egl_renderer/damage.rs` only if the shared explicit config must be threaded through production Kawase-demand planning; retain the existing public wrapper behavior.
- Modify `src/egl_renderer/effects/resources.rs` only if a read-target binding helper is required for the preservation transfer; do not attach temporal meaning to generic pooled texture contents.

---

### Task 1: Add RED tests for explicit policies, consumer coverage, and destructive framebuffer output

**Files:**

- Modify: `src/egl_renderer/effects/executor.rs` tests near the existing scene-work and consumer-plan tests.
- Modify: `src/egl_renderer.rs` tests near `GlesEffectTestHarness` and existing effect execution tests.

**Interfaces:**

- Consumes: existing `GlesEffectTestHarness`, graph builders, `EffectExecutionSelection`, and `SurfaceConsumerPlan`.
- Produces: failing tests that name the explicit-config entry points and establish the pixel comparison helpers needed by later tasks.

- [ ] **Step 1: Add the consumer coverage test first.**

Create a selected `SceneCapture` graph whose capture domain covers the output, a command below the anchor at a small repair, and a command above the anchor outside that repair. Call the future explicit planner with `Framebuffer + Partial` and assert both surface IDs are consumers. Keep the capture index source assertion so the below-anchor surface remains covered by `SurfaceCapture` semantics.

- [ ] **Step 2: Run the focused executor test and confirm the expected missing-entry-point failure.**

Run: `rtk cargo test --lib egl_renderer::effects::executor::tests::effect_surface_consumer_plan_framebuffer_scene_work_includes_upper_surface -- --exact`

Expected: compilation fails because the explicit planner entry point is not implemented yet. This is the intentional first RED for the new test seam.

- [ ] **Step 3: Add the deterministic two-frame destructive-output GLES test.**

Use a non-uniform uploaded background texture, a translucent target surface, and a blur graph anchored before the target. Render frame one with a full repaint. Render frame two with a small repair and `Framebuffer + Partial`, with a capture domain larger than repair. Read back the framebuffer and assert that at least one pixel outside repair differs from the saved frame-one pixels while the test is marked as the current destructive RED behavior. Keep the test independent of process environment variables.

- [ ] **Step 4: Run the GLES RED test.**

Run: `rtk cargo test --lib egl_renderer::tests::diagnostic_framebuffer_extra_scene_work_is_currently_destructive -- --exact --nocapture`

Expected: the test fails on the outside-repair equality assertion once the explicit execution seam is present, demonstrating the confirmed review finding against the current implementation.

- [ ] **Step 5: Commit the RED tests.**

Run:

```bash
rtk git add src/egl_renderer/effects/executor.rs src/egl_renderer.rs
rtk git commit -m "test: expose diagnostic framebuffer regressions"
```

### Task 2: Thread explicit debug configuration and share scene-work geometry

**Files:**

- Modify: `src/egl_renderer/effects/trace.rs`.
- Modify: `src/egl_renderer/effects/executor.rs`.
- Modify: `src/egl_renderer/effects/mod.rs`.
- Modify: `src/egl_renderer.rs`.
- Modify: `src/egl_renderer/damage.rs` if needed by demand planning.

**Interfaces:**

- Consumes: `EffectDebugConfig`, `EffectDebugCaptureMode`, `EffectDebugKawaseMode`, `CompiledFrameGraph`, and current production wrappers.
- Produces: `EffectDebugConfig::new`, `execute_effect_graph_with_debug_config`, `plan_effect_surface_consumers_with_debug_config`, and `SceneWorkRegions`.

- [ ] **Step 1: Add the immutable config constructor and explicit-config test seam.**

Add `EffectDebugConfig::new(capture_mode, kawase_mode)`. Keep `effect_debug_config()` unchanged as the process-scoped `OnceLock` source. Add wrappers whose production forms pass `*effect_debug_config()` to explicit forms; thread the explicit value through direct-capture checks, capture damage, trace summaries, graph-pass execution, and scene-work calculation.

- [ ] **Step 2: Run the explicit-config and consumer RED tests.**

Run: `rtk cargo test --lib egl_renderer::effects::executor::tests::effect_surface_consumer_plan_framebuffer_scene_work_includes_upper_surface -- --exact`

Expected: the test now compiles and fails because planner command ranges still use the small repaint region.

- [ ] **Step 3: Replace duplicated scene-work geometry with one pure helper.**

Implement `SceneWorkRegions { scene_work_rects, extra_scene_work }` and a pure helper that takes repaint rectangles, graph, selection, output size, lifecycle state, and explicit config. Preserve the current selected `SceneCapture` domain expansion and disjoint bounded fallback. Compute `extra_scene_work` by subtracting every repaint rectangle from the returned scene-work rectangles.

- [ ] **Step 4: Use the helper in both planner and executor.**

Pass `scene_work_rects` to every command-range consumer call in `plan_effect_surface_consumers_with_debug_config`. Use the same returned rectangles for framebuffer clearing and all effect scene replay calls. Keep capture-index rectangle planning unchanged.

- [ ] **Step 5: Verify geometry and consumer tests.**

Run:

```bash
rtk cargo test --lib egl_renderer::effects::executor::tests::framebuffer_scene_work_includes_selected_backdrop_capture_domains -- --exact
rtk cargo test --lib egl_renderer::effects::executor::tests::effect_surface_consumer_plan_framebuffer_scene_work_includes_upper_surface -- --exact
rtk cargo test --lib egl_renderer::effects::executor::tests::effect_surface_consumer_plan_keeps_capture_only_source -- --exact
```

Expected: all three pass, with the upper surface and below-anchor capture source both present in the consumer plan.

- [ ] **Step 6: Commit the explicit-config and shared-geometry change.**

Run:

```bash
rtk git add src/egl_renderer/effects/trace.rs src/egl_renderer/effects/executor.rs src/egl_renderer/effects/mod.rs src/egl_renderer.rs src/egl_renderer/damage.rs
rtk git commit -m "fix: share diagnostic scene work with consumers"
```

### Task 3: Preserve and restore extra scene work in framebuffer mode

**Files:**

- Modify: `src/egl_renderer/effects/executor.rs`.
- Modify: `src/egl_renderer/effects/resources.rs` only if the transfer needs a read-target helper.

**Interfaces:**

- Consumes: `SceneWorkRegions::extra_scene_work`, `EffectGlResourceCache`, `PooledEffectTexture`, active output framebuffer binding, and existing framebuffer-origin mapping.
- Produces: a per-execution preservation owner and capture/restore helpers used only by ordinary framebuffer diagnostic execution.

- [ ] **Step 1: Add focused transfer mapping tests.**

Test the logical-to-physical blit rectangles for bottom-left and top-left output origins, including a partial extra rectangle. Assert capture writes canonical bottom-left texture coordinates and restore maps back to the corresponding output coordinates.

- [ ] **Step 2: Run the mapping tests.**

Run: `rtk cargo test --lib egl_renderer::effects::executor::tests::scene_work_preservation_maps_framebuffer_origins -- --exact`

Expected: FAIL because the preservation transfer mapping helpers do not exist.

- [ ] **Step 3: Implement explicit per-frame preservation ownership.**

Check out a named RGBA8 nearest texture sized to the current output only when `extra_scene_work` is non-empty. Fully overwrite it from the active output framebuffer before `clear_effect_scene_work`. Use explicit READ/DRAW framebuffer bindings and the existing output-origin contract. Do not use `glFlush` or `glFinish`.

- [ ] **Step 4: Restore only extra scene work after final replay.**

After final scene replay and before ordinary overlays, blit only `extra_scene_work` from the preserved canonical texture back to the active output framebuffer. Never restore repaint rectangles. Release the owned texture after restoration. On graph execution errors, perform the same best-effort restoration and release before returning the error so fallback drawing starts from a valid framebuffer.

- [ ] **Step 5: Run the destructive GLES regression and verify it is GREEN.**

Run: `rtk cargo test --lib egl_renderer::tests::diagnostic_framebuffer_extra_scene_work_is_currently_destructive -- --exact --nocapture`

Expected: update the test name/assertion to the permanent invariant, then PASS because every outside-repair pixel equals the pre-frame framebuffer.

- [ ] **Step 6: Commit preservation.**

Run:

```bash
rtk git add src/egl_renderer/effects/executor.rs src/egl_renderer/effects/resources.rs
rtk git commit -m "fix: preserve framebuffer pixels outside diagnostic repair"
```

### Task 4: Add the four-policy pixel matrix and ordering diagnostics

**Files:**

- Modify: `src/egl_renderer.rs`.
- Modify: `src/egl_renderer/effects/executor.rs`.
- Modify: `src/egl_renderer/effects/trace.rs` only if explicit config is missing from an event.

**Interfaces:**

- Consumes: explicit execution entry point, deterministic GLES harness, preservation invariant, and existing trace event helpers.
- Produces: one in-process test covering replay/framebuffer × partial/full and supplemental ordering/full-Kawase assertions.

- [ ] **Step 1: Add pixel comparison helpers and the four-case matrix test.**

Render a full frame one and save RGBA readback. Render a full-reference frame two in a separate harness with the same scene. For each explicit config, render the partial frame two into a framebuffer containing frame one. Compare every pixel outside repair with frame one and every pixel inside repair with the full-reference frame using a small per-channel GLES readback tolerance. Assert the capture domain is substantially larger than repair and the target surface has non-opaque alpha.

- [ ] **Step 2: Run the matrix test.**

Run: `rtk cargo test --lib egl_renderer::tests::diagnostic_framebuffer_policy_matrix_preserves_partial_output -- --exact --nocapture`

Expected: PASS for all four configurations after Task 3. If a case fails, inspect pixel coordinates and trace output before changing code.

- [ ] **Step 3: Add or retain trace assertions for capture ordering, stacked backdrops, and final clipping.**

Run existing effect trace tests with explicit configs where needed. Assert framebuffer capture scene advancement occurs before capture execution, later stacked captures observe earlier results, final replay happens before preservation restore and overlays, and full-Kawase internal pass regions expand while final visible output remains constrained.

- [ ] **Step 4: Commit matrix diagnostics.**

Run:

```bash
rtk git add src/egl_renderer.rs src/egl_renderer/effects/executor.rs src/egl_renderer/effects/trace.rs
rtk git commit -m "test: verify all diagnostic framebuffer policies"
```

### Task 5: Run focused and full verification

**Files:**

- No source changes unless a verification failure identifies a required correction.

- [ ] **Step 1: Run focused regressions.**

Run:

```bash
rtk cargo test --lib egl_renderer::effects::executor::tests::framebuffer_scene_work_includes_selected_backdrop_capture_domains -- --exact
rtk cargo test --lib egl_renderer::effects::executor::tests::effect_surface_consumer_plan_framebuffer_scene_work_includes_upper_surface -- --exact
rtk cargo test --lib egl_renderer::effects::executor::tests::framebuffer_capture_orders_scene_advance_by_capture_policy -- --exact
rtk cargo test --lib egl_renderer::tests::effect_trace_bounds_visual_group_scene_and_final_replay -- --exact --nocapture
rtk cargo test --lib egl_renderer::tests::effect_trace_bounds_checkpoint_scene_advancement -- --exact --nocapture
rtk cargo test --lib egl_renderer::tests::diagnostic_framebuffer_policy_matrix_preserves_partial_output -- --exact --nocapture
rtk cargo test --lib effects::tests:: -- --nocapture
```

- [ ] **Step 2: Run the existing focused renderer/effects suites.**

Run:

```bash
rtk cargo test --lib egl_renderer:: -- --nocapture
rtk cargo test --lib effects:: -- --nocapture
```

- [ ] **Step 3: Run formatting and compilation checks in the current folder.**

Run:

```bash
rtk cargo fmt -- --check
rtk cargo check --lib
```

- [ ] **Step 4: Review the final diff and forbidden-scope checks.**

Run:

```bash
rtk git diff HEAD~4..HEAD --stat
rtk rg -n "PersistentBackdropCache|glFinish|glFlush|presentation_damage.*scene_work|full-monitor|full repaint" src/egl_renderer src/egl_renderer.rs
rtk git status --short
```

Confirm the only uncommitted files are pre-existing user changes or graph artifacts, no persistent backdrop cache was added, and no native gate claim is made.
