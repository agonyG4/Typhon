# Native-Faithful Stacked Backdrop Regression Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task with inline review checkpoints. Sub-agents are prohibited for this task.

**Goal:** Replace the synthetic stacked GLES checkpoint fixture with native-faithful heterogeneous Dock and TopBar regressions, prove legacy presentation-only scene work is detected, and remove unused `SceneWorkRegions` state without changing the production checkpoint architecture.

**Architecture:** Keep `compile_frame_execution_plan()` as the sole dependency compiler, `scene_work_regions()` as the shared authority used by both execution and `SurfaceConsumerPlan`, and the existing full-output preservation path. Build independent test-only effect instances and independent scene command topologies so compiler ordering, exact domains, semantic validity, and pixel behavior are all observed through the normal renderer path.

**Tech Stack:** Rust, Cargo, GLES/EGL test harness, `EffectRegion`, compiled effect graphs, renderer trace events, `rtk` command proxy.

## Global Constraints

- Keep the current pass-driven checkpoint scene-work architecture unchanged unless the native-faithful regression exposes a concrete correctness defect.
- Do not redesign blur, implement `PersistentBackdropCache`, optimize preservation, replace replay capture globally, or force Full Kawase in production.
- Compile and run tests in `/home/agony/GitHub/Typhon` so build artifacts stay in the same folder.
- Preserve the existing unstaged user changes; stage only task-owned files for each commit.
- Use no sub-agents.
- Do not add a public environment variable or debug production path for legacy sensitivity.
- Final production behavior must remain partial Kawase with replay capture policy unchanged.

---

### Task 1: Add failing native-fixture regression entry points

**Files:**
- Modify: `src/egl_renderer.rs` in the `#[cfg(test)] mod tests` helper/test area around the current stacked regression at lines 6957-8101.

**Interfaces:**
- Consumes: existing `GlesEffectTestHarness`, `EffectDebugConfig`, `RepaintPlan`, pixel readers, trace helpers, and compiler APIs.
- Produces: test names `native_faithful_stacked_dock_checkpoint_replay_partial_matches_full_current_reference` and `native_faithful_topbar_checkpoint_replay_partial_matches_full_current_reference`, both initially referring to the new helper names before those helpers exist.

- [ ] **Step 1: Write the failing tests first**

Add test bodies that request a `native_stacked_backdrop_scene(...)` result and a `native_stacked_diagnostic_scene(...)` installation, assert the compiled graph has two captures, and assert B has one dependency. Use the specified Dock dimensions `(1920, 1080)`, `TopLeftScanout`, replay capture, partial Kawase, and the TopBar domain `(0, 0, 120, 65)`. Do not insert a dependency in the test graph.

- [ ] **Step 2: Run the focused tests to verify RED**

Run:

```bash
rtk cargo test native_faithful_stacked_dock_checkpoint_replay_partial_matches_full_current_reference -- --nocapture
```

Expected: compilation failure because the native fixture helper is not yet defined. If the test passes or fails for an unrelated reason, correct the test entry point before adding helpers.

- [ ] **Step 3: Commit the failing test entry points**

```bash
rtk git add src/egl_renderer.rs
rtk git commit -m "test: add native backdrop regression entry points"
```

### Task 2: Implement independent native-faithful scene and graph fixtures

**Files:**
- Modify: `src/egl_renderer.rs` in the test-only scene/program helper area around lines 6717-7000.

**Interfaces:**
- Consumes: the existing moving blur program construction and `ResolvedEffectInstance` fields.
- Produces: a test-only `NativeStackedBackdropEffectSpec`, a `native_stacked_backdrop_scene(...) -> (ResolvedEffectScene, EffectRegistry)` helper, a `native_stacked_diagnostic_scene(...)` renderer setup helper, and a dedicated TopBar scene setup using the same independent-instance graph helper.

- [ ] **Step 1: Add an independently specified effect spec and instance constructor**

Define a test-only spec containing at least `id`, `target_bounds`, `anchor`, `anchor_scope`, `visual_group`, and `scene_order`. Construct each `ResolvedEffectInstance` as a separate struct literal with its own ID/signature and `region` derived from its own target bounds. Do not clone A to create B.

- [ ] **Step 2: Share only the immutable blur program/registry**

Reuse one validated dual-Kawase program and registry for A and B. Keep the blur parameters `radius=4.0`, `passes=2`, and the caller-supplied scale. Let `ResolvedEffectScene::new()` sort the instances by the supplied scene order.

- [ ] **Step 3: Encode the Dock geometry through ordinary compiler inputs**

Use an A region/target that expands by the existing blur footprint to `(66, 120, 1000, 937)` and B target/region `(786, 1000, 348, 56)` so the compiler derives `(762, 976, 396, 104)` for B. Set A to `BeforeSurface(11)`/`VisualGroup`, B to `BeforeSurface(7)`/`Surface`, and use distinct visual groups and scene order values that put A before B. Assert the two compiled capture domains exactly.

- [ ] **Step 4: Build independent scene identities and command order**

Create a dedicated diagnostic topology with `Surface(100)` as the non-uniform fullscreen background, `Surface(11)` as earlier A-associated content, and `Surface(7)` as translucent Dock content. Place command ranges in that order, assign A's command to A's visual group, and assign B's command to B's visual group without relying on IDs for ordering. Keep the background update helper keyed to the dedicated background identity.

- [ ] **Step 5: Add the TopBar topology**

Use the same independent-identity structure with a non-colliding TopBar owner and a B target region whose ordinary footprint expansion/clipping produces `(0, 0, 120, 65)`. Keep B as `BeforeSurface(<topbar owner>)`/`Surface`, replay capture, partial Kawase, and require a dependency from an earlier A effect.

- [ ] **Step 6: Run compiler/fixture-focused tests**

Run:

```bash
rtk cargo test native_faithful -- --nocapture
rtk cargo test replay_scene_work_includes_selected_checkpoint_framebuffer_domains -- --nocapture
rtk cargo test replay_scene_work_includes_topbar_checkpoint_framebuffer_domain -- --nocapture
```

Expected: the helper compiles, A and B are distinct, B has exactly one compiler-created dependency, the Dock domains are exact, the TopBar domain is exact, and no production defaults change.

- [ ] **Step 7: Commit the fixture implementation**

```bash
rtk git add src/egl_renderer.rs
rtk git commit -m "test: model heterogeneous stacked backdrop effects"
```

### Task 3: Complete Dock pixel, trace, advancement, validity, and consumer-plan evidence

**Files:**
- Modify: `src/egl_renderer.rs` in the native Dock regression area.
- Modify: `src/egl_renderer/effects/executor.rs` only if a test-visible diagnostic helper is required; keep `scene_work_regions()` behavior unchanged.

**Interfaces:**
- Consumes: `native_stacked_backdrop_scene`, `native_stacked_diagnostic_scene`, `execute_diagnostic_frame_with_origin`, `diagnostic_matrix_mismatch_counts_for_origin`, `EffectRegion` operations, and `plan_effect_surface_consumers_with_debug_config`.
- Produces: a native Dock two-frame regression that independently records previous, candidate, and full-current images, plus trace assertions proving actual B framebuffer capture and A-to-B scene advancement.

- [ ] **Step 1: Assert graph structure before rendering**

Find A/B `SceneCapture` passes by instance ID, assert there are exactly two captures, assert B has one dependency and that dependency is A's capture pass, assert B's output texture domain is `(762, 976, 396, 104)`, and compute `B.required_capture ∩ A.output_influence_region` with `EffectRegion::intersect`. Assert it is non-empty and is covered by A's semantically valid output region using `subtract`, not bounding-box containment.

- [ ] **Step 2: Assert different composition boundaries before rendering**

Use the actual command vector and native anchors to assert the scene ranges for A and B are different, with a real command index between them. The assertions must make a collapsed A/B boundary fail. Keep the A/B command identities explicit so this is evidence of topology rather than an ID comparison.

- [ ] **Step 3: Assert expanded checkpoint work reaches the consumer plan**

Build the partial demand and selection, call `plan_effect_surface_consumers_with_debug_config`, and assert the plan includes surfaces needed while replaying the expanded checkpoint region, including `Surface(100)`, `Surface(11)`, and `Surface(7)`. Keep this call on the existing `scene_work_regions()` authority.

- [ ] **Step 4: Execute a full previous frame and partial candidate**

Run a full frame at `1920x1080` with `TopLeftScanout`, store `previous_valid_framebuffer`, update only a small repair inside the Dock, optionally poison pooled textures with magenta or green, enable trace capture, and run the partial replay/partial-Kawase candidate. Locate B's execution and semantic-validity trace events and require `backdrop_capture_policy=replay`, `kawase_execution_policy=partial`, `capture_mode=framebuffer_blit`, `checkpoints=1`, and `missing_pixels=0`.

- [ ] **Step 5: Assert scene advancement ordering from trace**

Find A's composite replay boundary and B's `checkpoint_dependency` replay boundary. Assert their cursor starts differ, B's boundary advances over at least one command, and the B boundary occurs after the A composite. Require the B boundary's `scene_cursor_start`, `scene_cursor_end`, and `command_count` to describe a non-empty command range.

- [ ] **Step 6: Execute a separate full-current reference**

Construct a new harness and render the changed scene with a full repaint. Store `full_current_reference` separately; never use the previous frame as the reference.

- [ ] **Step 7: Assert the three-image pixel invariant**

Within the repair, compare the candidate to the full-current reference with the existing tolerance. Outside the repair, compare the candidate to the previous valid framebuffer with the same tolerance. Require zero mismatching pixels in both regions.

- [ ] **Step 8: Assert poison independence**

Run the candidate with incompatible magenta and green poison patterns. Assert both candidate outputs agree within the repair and each agrees with the full-current reference there. Keep the poison experiment tied to B's actual framebuffer checkpoint capture.

- [ ] **Step 9: Run focused Dock evidence tests**

Run:

```bash
rtk cargo test native_faithful_stacked_dock_checkpoint_replay_partial_matches_full_current_reference -- --nocapture
rtk cargo test checkpoint_semantic_validity_does_not_mask_dependency_gaps_with_base_scene -- --nocapture
rtk cargo test checkpoint_dependency_region_requires_prior_effect_influence_coverage -- --nocapture
rtk cargo test effect_surface_consumer_plan_replay_checkpoint_uses_checkpoint_scene_work -- --nocapture
```

Expected: GREEN, with the trace evidence and pixel counts asserted by the test.

- [ ] **Step 10: Commit Dock evidence**

```bash
rtk git add src/egl_renderer.rs
rtk git commit -m "test: prove native Dock checkpoint reconstruction"
```

### Task 4: Add TopBar GLES and Full-Kawase control evidence

**Files:**
- Modify: `src/egl_renderer.rs` in the test-only regression area.

**Interfaces:**
- Consumes: the shared native graph helper, independent TopBar scene topology, pixel comparison helpers, poison helper, and trace helper.
- Produces: `native_faithful_topbar_checkpoint_replay_partial_matches_full_current_reference` and a same-graph Full-Kawase control test for the Dock fixture.

- [ ] **Step 1: Implement the TopBar two-frame GLES test**

Use `1920x1080`, `TopLeftScanout`, replay capture, partial Kawase, a small TopBar repair, and the three independent images. Assert the compiled TopBar checkpoint domain is `(0, 0, 120, 65)`, B has a dependency, trace reports framebuffer blit and `missing_pixels=0`, and candidate pixels satisfy the outside/inside invariant.

- [ ] **Step 2: Add TopBar poison independence**

Render magenta and green poison candidates from identical previous frames and assert the repaired pixels match each other and the separately rendered full-current reference.

- [ ] **Step 3: Add a Full-Kawase Dock control**

Run the exact heterogeneous Dock graph and scene topology with replay capture and `EffectDebugKawaseMode::Full`. Assert graph domains, dependency, trace capture mode, and the same pixel/poison invariants. Do not alter the production debug default or the partial regression.

- [ ] **Step 4: Run focused controls and related suites**

Run:

```bash
rtk cargo test native_faithful_topbar_checkpoint_replay_partial_matches_full_current_reference -- --nocapture
rtk cargo test native_faithful_full_kawase_dock_control -- --nocapture
rtk cargo test full_kawase_debug_mode_expands_only_internal_passes -- --nocapture
rtk cargo test effect_trace_bounds_checkpoint_scene_advancement -- --nocapture
```

- [ ] **Step 5: Commit TopBar/control evidence**

```bash
rtk git add src/egl_renderer.rs
rtk git commit -m "test: add TopBar and Full Kawase checkpoint controls"
```

### Task 5: Prove RED sensitivity against legacy scene work

**Files:**
- Temporary local-only modification: `src/egl_renderer/effects/executor.rs` inside `scene_work_regions()`.
- Restore completely before the final commit.

**Interfaces:**
- Consumes: the GREEN native Dock regression from Task 3.
- Produces: an exact recorded failure result proving that `scene_work = presentation_work` is detected while B remains a framebuffer checkpoint capture.

- [ ] **Step 1: Make the minimal temporary legacy edit**

Locally replace the checkpoint-inclusive scene-work combination with presentation-only work while preserving the existing checkpoint capture predicate and all other production code. Do not add a configuration switch or public environment variable.

- [ ] **Step 2: Run the native Dock regression and record RED evidence**

Run:

```bash
rtk cargo test native_faithful_stacked_dock_checkpoint_replay_partial_matches_full_current_reference -- --nocapture
```

Record the exact failing assertion and result, which must be one of: non-zero `missing_pixels`, a candidate/full-reference mismatch, poison-dependent output, or a checkpoint semantic invariant failure. If the test stays green, stop the task and report that the fixture does not prove the suspected root cause.

- [ ] **Step 3: Revert the temporary edit and verify the diff**

Restore `scene_work_regions()` to the checkpoint-inclusive implementation, then run:

```bash
rtk git diff -- src/egl_renderer/effects/executor.rs
```

Expected: no task-created diff remains in that file except the intentional dead-state cleanup from Task 6, and the native Dock regression is GREEN again.

- [ ] **Step 4: Record the sensitivity result in the task report**

Add the exact RED assertion/result and the restored GREEN command/result to the final handoff; do not commit the legacy edit.

### Task 6: Remove dead `SceneWorkRegions` state

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs` around `SceneWorkRegions` and `scene_work_regions()` lines 3583-3653.

**Interfaces:**
- Consumes: the existing local `presentation_work`, `checkpoint_work`, `scene_work_rects`, and `extra_scene_work` calculation.
- Produces: `SceneWorkRegions` containing only `scene_work_rects` and `extra_scene_work`; local presentation/checkpoint vectors remain available while constructing the result.

- [ ] **Step 1: Write the focused structural test expectation**

Update the existing scene-work tests only as needed so they continue to assert the consumed fields and exact checkpoint-domain expansion; do not add tests that inspect removed fields.

- [ ] **Step 2: Remove the two unused stored fields**

Delete `presentation_work` and `framebuffer_checkpoint_work` from `SceneWorkRegions` and omit them from the returned struct literal. Leave their local variables and combination logic intact.

- [ ] **Step 3: Run focused scene-work and planner tests**

Run:

```bash
rtk cargo test replay_scene_work -- --nocapture
rtk cargo test scene_work -- --nocapture
rtk cargo test effect_surface_consumer_plan -- --nocapture
```

- [ ] **Step 4: Commit the cleanup**

```bash
rtk git add src/egl_renderer/effects/executor.rs
rtk git commit -m "refactor: remove unused scene work state"
```

### Task 7: Run verification gates and prepare native-gate evidence

**Files:**
- No new production files.
- Modify only task report/documentation files if a durable test result record is required.

**Interfaces:**
- Consumes: all committed regression fixtures and the restored production scene-work implementation.
- Produces: verified test results, clean formatting/lint/check status, and an explicit hardware-gate outcome.

- [ ] **Step 1: Run focused correctness gates**

Run the native Dock and TopBar tests, poison tests, exact-region validity tests, different-anchor advancement tests, dependency influence tests, SurfaceConsumerPlan tests, capture-coordinate tests, fullscreen tests, stacked ordering tests, and the four explicit diagnostic configurations using `rtk cargo test` with exact names where available.

- [ ] **Step 2: Run existing renderer/effects/compositor suites**

Use the repository’s existing focused suite commands and record failures without masking them. Keep all compilation in the checkout’s `target` directory.

- [ ] **Step 3: Run required static gates**

```bash
rtk cargo fmt --check
rtk cargo check --workspace --all-targets
rtk cargo clippy --workspace --all-targets -- -D warnings
```

Do not claim a gate passed unless its command exits successfully.

- [ ] **Step 4: Inspect the final diff and status**

```bash
rtk git diff --check
rtk git status --short --branch
rtk git diff HEAD~4..HEAD --stat
```

Ensure no `PersistentBackdropCache`, preservation optimization, production Full-Kawase default, public legacy switch, or temporary sensitivity edit is present. Ensure pre-existing user modifications remain intact.

- [ ] **Step 5: Run the native hardware gate only after automated GREEN plus RED sensitivity**

Run the requested `1920x1080@165`, replay/partial session and exercise Dock movement, TopBar movement, multiple blurred Shell elements, and repeated transitions. Accept it only if trace contains real partial checkpoint captures with `repaint_mode=partial`, replay policy, partial Kawase policy, dependency count greater than zero, framebuffer blit mode, and `missing_pixels=0`, with no dark/translucent-black Dock or TopBar block. If hardware access is unavailable, report that explicitly instead of treating a no-capture session as valid.
