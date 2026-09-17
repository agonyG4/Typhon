# Stacked Backdrop Checkpoint Validity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove or disprove semantic invalidity of direct framebuffer checkpoint captures under replay-plus-partial stacked backdrops, then apply the smallest pass-driven scene-work correction only if deterministic RED evidence confirms the defect.

**Architecture:** Keep graph-texture physical/current-execution validity separate from checkpoint-source semantic validity. Derive bounded internal scene work from the same direct-capture authority and graph texture domains used by execution, preserve only extra internal work, and share that plan with surface consumers, replay, validity diagnostics, and trace output.

**Tech Stack:** Rust, Cargo, GLES 3/EGL, `glow`, Typhon's typed render graph, existing renderer/effects/compositor test suites.

## Global Constraints

- Do not implement `PersistentBackdropCache` or begin a new production backdrop architecture.
- Do not force Full Kawase, global framebuffer capture, global full repaint, or global replay capture.
- Keep the existing capture-coordinate and fullscreen effect visibility fixes unchanged.
- Keep checkpoint dependencies and ordered composition semantics unchanged.
- Keep presentation damage and final Composite clipping unchanged.
- Do not change alpha, shell shape, or blend semantics for this experiment.
- Compile in `/home/agony/GitHub/Typhon` so build artifacts stay in the same folder.
- Use `rtk` for repository commands and test/build output.
- Do not use subagents.
- Preserve pre-existing user changes in the dirty worktree; stage only task-owned files when committing.

---

### Task 1: Establish the exact source-validity model and failing pure-region tests

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs:3450-3514` for the bounded pure region helpers and tests.
- Modify: `src/egl_renderer/effects/trace.rs:160-240,496-571` for bounded checkpoint validity fields and formatting.
- Test: `src/egl_renderer/effects/executor.rs` unit-test module.

**Interfaces:**
- Consumes: `CompiledFrameGraph`, `CompiledRenderPass`, `EffectExecutionSelection`, `EffectRegion`, `OutputRect`, and `is_direct_framebuffer_capture`.
- Produces: a pure `CheckpointSourceValidity`/`CheckpointSourceGap` calculation that accepts the authoritative capture domain and exact reconstructed checkpoint region, returning bounded rect counts, bboxes, and pixel counts; a bounded trace summary consumed by pass-boundary diagnostics.

- [ ] **Step 1: Write the failing exact-containment tests.**

  Add tests that use the native-shaped Dock domain `(762, 976, 396, 104)` and TopBar domain `(0, 0, 120, 65)` with two disjoint reconstructed rectangles. Assert that subtracting exact regions reports the correct non-empty missing region and pixel count even when the valid-region bbox contains the required bbox. Add the complementary containment case where exact coverage is complete and the gap is empty.

  Add a graph fixture with replay policy, a selected earlier capture A, a selected checkpoint-dependent SceneCapture B, B's output domain extending outside the ordinary presentation repair, and `TopLeftScanout` metadata. Assert B is selected, has one checkpoint dependency, resolves to direct framebuffer capture, and its required domain is not inferred from a bbox.

- [ ] **Step 2: Run the focused tests and confirm the intended RED.**

  Run:

  ```bash
  rtk cargo test -p oblivion-one --lib egl_renderer::effects::executor -- --nocapture
  ```

  Expected: the new semantic-gap assertions fail because the current executor has no checkpoint-source region calculation and `scene_work_regions` does not include checkpoint domains under global replay.

- [ ] **Step 3: Add the pure bounded region calculation and trace fields only.**

  Implement exact region subtraction/reduction using existing bounded/disjoint region helpers. Do not change scene-work behavior yet. Extend the trace payload with:

  ```text
  checkpoint_required_rects
  checkpoint_required_bbox
  checkpoint_valid_rects
  checkpoint_valid_bbox
  checkpoint_missing_rects
  checkpoint_missing_bbox
  checkpoint_missing_pixels
  ```

  Cap all emitted rectangle counts and use `none`/zero for empty regions. Keep these fields distinct from graph texture validity.

- [ ] **Step 4: Run the focused tests and commit the evidence helper.**

  Run the same focused command. The pure region tests must pass; the integration RED test remains failing until Task 2's execution instrumentation is present.

  ```bash
  rtk git add src/egl_renderer/effects/executor.rs src/egl_renderer/effects/trace.rs
  rtk git commit -m "test: model checkpoint source semantic validity"
  ```

### Task 2: Add the deterministic real-GLES RED A and RED B reproductions

**Files:**
- Modify: `src/egl_renderer.rs:6544-8843` test harness and renderer tests.
- Modify: `src/egl_renderer/effects/executor.rs` only where test-only diagnostics must observe checkpoint validity.

**Interfaces:**
- Consumes: Task 1's pure region calculation, existing `GlesEffectTestHarness`, graph compilation helpers, `EffectDebugConfig`, and `OutputFramebufferOrigin::TopLeftScanout`.
- Produces: deterministic tests for Dock and TopBar checkpoint domains, exact-region RED evidence, real two-frame pixel comparisons, poison independence checks, and trace assertions that B executed as a framebuffer blit.

- [ ] **Step 1: Build a two-frame stacked-checkpoint fixture before changing production behavior.**

  Add test-only helpers that:

  ```rust
  fn render_stacked_frame(
      harness: &mut GlesEffectTestHarness,
      graph: &CompiledFrameGraph,
      repair: &[OutputRect],
      full: bool,
      poison: Option<[f32; 4]>,
  ) -> Vec<u8>
  ```

  Use a non-uniform background, the existing translucent shell-like surface, two ordered effects, `TopLeftScanout`, replay capture, partial Kawase, and a small frame-2 source change. Keep the Dock case at the supplied `396x104` domain and add a TopBar case at `120x65`.

- [ ] **Step 2: Assert capture mode and dependency semantics.**

  Before pixel assertions, locate B in the compiled graph and assert `checkpoint_dependencies.len() > 0`. Enable the test trace and assert B's execute-boundary record contains `capture_mode=framebuffer_blit`, `backdrop_capture_policy=replay`, `kawase_execution_policy=partial`, `framebuffer_origin=top_left_scanout`, and the checkpoint diagnostic fields.

- [ ] **Step 3: Run RED A and confirm a non-empty exact semantic gap.**

  Execute Variant A with ordinary presentation scene work only. Assert the exact missing region/pixel count is non-zero for the Dock fixture, and repeat the same assertion for the TopBar fixture. If the helper reports zero, keep the test non-failing and record the disproven result; do not force a failure or change production behavior based on correlation.

- [ ] **Step 4: Run RED B and confirm temporal self-feedback.**

  Render full Frame 1 and save `previous_valid_framebuffer`. For Frame 2, change only a small repair region, render two candidates with distinct magenta/green and cyan/red poison patterns in unproven source contents, and independently render a full Frame 2 reference. Assert:

  ```text
  candidate outside repair == previous_valid_framebuffer
  candidate inside repair == full_current_reference
  candidate_with_poison_A == candidate_with_poison_B
  ```

  Use per-channel tolerance appropriate for the existing GLES readback format. Make the RED explicit: Variant A must fail inside the repair if B samples stale final-frame pixels. Do not accept a graph where B is replay capture.

- [ ] **Step 5: Run the new tests and record the result before a production fix.**

  Run:

  ```bash
  rtk cargo test -p oblivion-one --lib egl_renderer::tests::stacked_checkpoint -- --nocapture
  ```

  If RED A/B is not reproducible, stop production changes and report the disproven hypothesis with the exact trace and pixel evidence. If RED is confirmed, continue to Task 3.

### Task 3: Localize checkpoint-source validity without weakening graph-texture validity

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs:858-1278,1618-1702`.
- Modify: `src/egl_renderer/effects/trace.rs` checkpoint diagnostic types and pass formatting.
- Test: `src/egl_renderer/effects/executor.rs` validity tests and `src/egl_renderer.rs` trace assertions.

**Interfaces:**
- Consumes: exact capture domains, authoritative `scene_work` geometry, scene replay cursor, `is_direct_framebuffer_capture`, and the existing `valid_regions` map.
- Produces: a separate checkpoint-source assertion that runs before a direct capture is marked physically valid; existing `validate_current_frame_input_regions` remains unchanged.

- [ ] **Step 1: Write the failing semantic-validity assertion test.**

  Add a test that gives B a full capture domain while the reconstructed checkpoint region covers only the presentation repair. Assert the new error/diagnostic contains B's pass and instance, missing rect count, missing bbox, and missing pixel count. Assert a physically written output region alone does not make the semantic assertion pass.

- [ ] **Step 2: Run the test to verify it fails for the right reason.**

  ```bash
  rtk cargo test -p oblivion-one --lib egl_renderer::effects::executor::checkpoint -- --nocapture
  ```

  Expected: failure from the missing checkpoint-source coverage, not from `UninitializedInputRegion` or a texture allocation error.

- [ ] **Step 3: Add the separate assertion and diagnostics.**

  For each selected direct framebuffer SceneCapture, compute required capture region from the exact graph texture domain and valid source region from the actual reconstruction plan. Emit the bounded diagnostic before inserting the output into `valid_regions`. Keep the direct capture's physical output region insertion as the existing full graph-texture domain behavior only after the semantic check.

- [ ] **Step 4: Verify localization and preserve existing invariants.**

  Run both the new semantic test and the existing `current_frame_validity_guard_rejects_unproduced_input_texels` test. Confirm a graph-texture producer that writes the required texture region still passes the old tracker, while a direct framebuffer source with incomplete checkpoint reconstruction fails the new assertion.

  ```bash
  rtk cargo test -p oblivion-one --lib egl_renderer::effects::executor -- --nocapture
  ```

### Task 4: Implement Variant B as the smallest pass-driven scene-work correction

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs:551-658,858-1318,3444-3514`.
- Modify: `src/egl_renderer.rs` only if the shared plan needs a renderer-facing scene clear/replay interface adjustment.
- Test: `src/egl_renderer/effects/executor.rs` pure scene-work tests and `src/egl_renderer.rs` real-GLES tests.

**Interfaces:**
- Consumes: Task 1's region helpers, Task 3's semantic validity assertion, `capture_execution_damage`, and the same graph texture domains used by `execute_capture`.
- Produces: a pure `EffectSceneWorkPlan` containing `presentation_work`, `framebuffer_checkpoint_work`, `internal_work`, and `extra_internal_work`.

- [ ] **Step 1: Write the failing Variant B plan tests.**

  Under global replay, create a selected checkpoint-dependent B whose domain is outside the presentation repair. Assert:

  ```text
  presentation_work == repair
  framebuffer_checkpoint_work == exact B domain union
  internal_work == bounded union(presentation_work, framebuffer_checkpoint_work)
  extra_internal_work == internal_work - presentation_work
  ```

  Assert the Dock and TopBar domains are both represented exactly and region fragmentation remains bounded.

- [ ] **Step 2: Run the plan tests to confirm current behavior is RED.**

  ```bash
  rtk cargo test -p oblivion-one --lib egl_renderer::effects::executor::scene_work -- --nocapture
  ```

  Expected: replay policy returns only presentation work, so the checkpoint work assertion fails.

- [ ] **Step 3: Implement the shared pass-driven plan.**

  Replace the global `capture_mode == Framebuffer` gate in `scene_work_regions` with selected-pass evaluation through `is_direct_framebuffer_capture`. For each selected direct SceneCapture, use `capture_execution_damage` and the output texture's authoritative domain. Keep SurfaceCapture behavior unchanged unless its existing checkpoint semantics require the same direct source region. Use existing bounded/disjoint-region coalescing and preserve overflow fallback behavior.

- [ ] **Step 4: Drive every consumer from the shared plan.**

  Pass the plan to `plan_effect_surface_consumers_with_debug_config`, scene clear, all scene replay scissors, checkpoint validity, preservation, restore, and trace summaries. Do not recompute checkpoint geometry in any of these paths. Preserve `extra_internal_work` before destructive reconstruction and restore it after graph/final scene work but before ordinary presentation overlays; never restore inside presentation work.

- [ ] **Step 5: Run Variant B and prove GREEN.**

  Re-run RED A/B with Variant B. Require zero missing semantic source pixels, both poison candidates equal to each other and the full current reference inside repair, unchanged pixels outside repair, and B still executing as a framebuffer blit with checkpoint dependency count > 0.

  ```bash
  rtk cargo test -p oblivion-one --lib egl_renderer::tests::stacked_checkpoint -- --nocapture
  rtk cargo test -p oblivion-one --lib egl_renderer::effects::executor -- --nocapture
  ```

- [ ] **Step 6: Commit the minimal correction after GREEN.**

  ```bash
  rtk git add src/egl_renderer/effects/executor.rs src/egl_renderer/effects/trace.rs src/egl_renderer.rs
  rtk git commit -m "fix: validate stacked backdrop checkpoint sources"
  ```

### Task 5: Verify demand propagation and SurfaceConsumerPlan coverage

**Files:**
- Modify: `src/effects/render_graph.rs` only if a focused failing demand test proves source coverage is missing.
- Modify: `src/egl_renderer/effects/executor.rs` SurfaceConsumerPlan tests.
- Test: `src/effects/render_graph.rs` and `src/egl_renderer/effects/executor.rs`.

**Interfaces:**
- Consumes: pass-driven `EffectSceneWorkPlan`, graph instance dependency regions, and the existing reverse dependency demand propagation.
- Produces: regression evidence that B's checkpoint source intersecting A's output influence causes A's Composite to execute over the required region and that all scene commands drawn during expanded internal work were declared as consumers before resource synchronization.

- [ ] **Step 1: Write the failing coverage tests.**

  Build a graph where B's checkpoint-required domain intersects A's output influence. Assert A's executed Composite output region covers that intersection. Add a SurfaceConsumerPlan assertion that surfaces needed by expanded internal work are present before resource realization.

- [ ] **Step 2: Run the tests.**

  ```bash
  rtk cargo test -p oblivion-one --lib effects::render_graph -- --nocapture
  rtk cargo test -p oblivion-one --lib egl_renderer::effects::executor::surface -- --nocapture
  ```

- [ ] **Step 3: Fix only source demand propagation if RED.**

  If A coverage is missing, correct dependency-demand propagation at its graph source. Do not force A full-domain and do not compensate in the executor. If the tests are already GREEN, make no production change and record that the existing propagation is sufficient.

- [ ] **Step 4: Verify expanded-work consumer planning.**

  Re-run the checkpoint GLES test with resource synchronization enabled and assert no surface is sampled without a declared consumer. Keep the bounded work plan and checkpoint ordering unchanged.

### Task 6: Keep the controls and existing regressions green

**Files:**
- Modify: only test files if a missing assertion is required; no production policy shortcut.

- [ ] **Step 1: Add/retain the Full-Kawase control.**

  Run the same two-frame fixture with current checkpoint source behavior and Full Kawase. Record it as a control only; do not use it as the production fix or loosen the Variant A RED requirement.

- [ ] **Step 2: Run focused existing suites.**

  ```bash
  rtk cargo test -p oblivion-one --lib egl_renderer -- --nocapture
  rtk cargo test -p oblivion-one --lib effects -- --nocapture
  rtk cargo test -p oblivion-one --lib compositor -- --nocapture
  ```

  Include existing capture coordinate-space, fullscreen effect, stacked backdrop ordering, four explicit diagnostic configuration, and scene preservation tests. Do not hide unrelated failures.

### Task 7: Repository gates, native gate, and final commit

**Files:**
- No new files unless native trace capture requires a documented test fixture or bounded diagnostic output.

- [ ] **Step 1: Run formatting, checking, and linting in place.**

  ```bash
  rtk cargo fmt --all -- --check
  rtk cargo check --workspace
  rtk cargo clippy --workspace --all-targets -- -D warnings
  ```

- [ ] **Step 2: Run the complete relevant test set.**

  ```bash
  rtk cargo test --workspace --lib -- --nocapture
  ```

  Record unrelated pre-existing failures separately; do not suppress them.

- [ ] **Step 3: Run the native workload.**

  At 1920x1080@165 with `capture=replay` and `kawase=partial`, exercise overlapping Dock and TopBar blur movement. Require multiple partial frames where `checkpoint_dependencies > 0` and `capture_mode=framebuffer_blit`, and verify trace `checkpoint_missing_pixels=0` for those frames. Check both framebuffer IDs/swapchain reuse where practical and visually confirm no dark rectangular block.

- [ ] **Step 4: Review the diff and commit only task-owned changes.**

  ```bash
  rtk git diff --check
  rtk git status --short
  rtk git diff --stat
  rtk git add <task-owned-files>
  rtk git commit -m "fix: establish valid stacked backdrop checkpoints"
  ```

  Do not stage unrelated pre-existing worktree changes.
