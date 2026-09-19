# Direct Scanout Physical Presentation Owner Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Qualify Direct Scanout physical Geometry and Opacity recovery by candidate WindowGroup `SceneNodeId` across XWayland backing replacement.

**Architecture:** Keep root-qualified physical helpers for input. Add SceneNode-qualified helpers backed directly by the immutable presented `PresentationFrameSnapshot`, then reuse one candidate SceneNode in Direct Scanout for active, canonical, and physical presentation qualification.

**Tech Stack:** Rust, Cargo, Typhon compositor tests, Codebase Memory MCP, `rtk` command wrapper.

## Global Constraints

- Do not modify renderer, effects, input semantics, NativeSceneHistory architecture, Presentation Engine transactions, Clip, or lifecycle/Lamp.
- Preserve `root_surface_id` in physical frame evidence and preserve root-qualified input helpers.
- Compile and test in `/home/agony/GitHub/Typhon` (the available Aether checkout) to avoid extra SSD writes.
- Preserve unrelated dirty work; do not stash, reset, discard, amend, or silently include it.
- Use TDD: each new behavior test must fail before its production fix.
- Commit reviewable changes because this is a Git repository.

---

### Task 1: Add the failing physical-owner regressions

**Files:**
- Modify: `src/compositor/state/direct_scanout_tests.rs`
- Test: existing focused Direct Scanout test target

**Interfaces:**
- Consumes: existing XWayland fixture helpers, `PresentationSceneSample`, `publish_presented_presentation`, and backing replacement APIs.
- Produces: independent tests proving root A physical Geometry/Opacity evidence still blocks current root B, identity G/root B clears those blockers, and unrelated SceneNode evidence is ignored.

- [ ] **Step 1: Add a helper that publishes a SceneNode-qualified physical sample with configurable root, transform, and opacity.**
- [ ] **Step 2: Add the combined A→B regression with no active tracks and canonical opacity 1.0.**
- [ ] **Step 3: Add the identity G/root B recovery assertion.**
- [ ] **Step 4: Add the unrelated physical-owner isolation case.**
- [ ] **Step 5: Run the focused tests and confirm the old root-qualified implementation fails because root B cannot find root A evidence.**

Run: `rtk cargo test --locked --all-targets direct_scanout_tests`

Expected before production changes: the new A→B physical-owner assertions fail while existing focused tests compile and run.

- [ ] **Step 6: Commit the red tests.**

```bash
git add src/compositor/state/direct_scanout_tests.rs
git commit -m "test(scanout): cover physical presentation across XWayland backing replacement"
```

### Task 2: Add SceneNode-qualified physical presentation helpers

**Files:**
- Modify: `src/compositor/state/active_scene.rs`
- Modify: `src/compositor/state/fullscreen.rs`
- Test: `src/compositor/state/direct_scanout_tests.rs`

**Interfaces:**
- Consumes: `PresentationFrameSnapshot::transform_for_scene_node` and `opacity_for_scene_node`.
- Produces: compositor-local SceneNode-qualified physical transform, Geometry identity, opacity, and Opacity identity helpers.

- [ ] **Step 1: Add `presented_presentation_transform_for_scene_node(SceneNodeId)` using only `presented_presentation` and `transform_for_scene_node`.**
- [ ] **Step 2: Add `presented_presentation_geometry_is_non_identity_for_scene_node(SceneNodeId)` with missing-transform-as-identity semantics.**
- [ ] **Step 3: Add `presented_presentation_opacity_for_scene_node(SceneNodeId)` with missing-opacity-as-opaque semantics.**
- [ ] **Step 4: Add `presented_presentation_opacity_is_non_identity_for_scene_node(SceneNodeId)`.**
- [ ] **Step 5: Keep root-qualified helpers unchanged and run the new focused tests to verify helper-level behavior.**
- [ ] **Step 6: Commit the helper implementation.**

```bash
git add src/compositor/state/active_scene.rs src/compositor/state/fullscreen.rs src/compositor/state/direct_scanout_tests.rs
git commit -m "fix(scanout): qualify physical presentation by candidate SceneNode"
```

### Task 3: Migrate Direct Scanout physical checks to the candidate SceneNode

**Files:**
- Modify: `src/compositor/state/direct_scanout.rs`
- Test: `src/compositor/state/direct_scanout_tests.rs`

**Interfaces:**
- Consumes: one candidate `SceneNodeId` resolved from the covering root, active track queries, canonical DesktopWindow opacity, and SceneNode-qualified physical helpers.
- Produces: candidate-causal Geometry/Opacity blockers that survive backing replacement without changing current root diagnostics or input behavior.

- [ ] **Step 1: Resolve the candidate SceneNode once after `covering_application_group` succeeds.**
- [ ] **Step 2: Use that value for active Geometry and Opacity checks.**
- [ ] **Step 3: Use that value for physical Geometry and Opacity checks.**
- [ ] **Step 4: Preserve canonical opacity lookup through the candidate root’s DesktopWindow.**
- [ ] **Step 5: Run candidate, physical recovery, canonical opacity, and XWayland replacement tests.**
- [ ] **Step 6: Commit the Direct Scanout migration.**

```bash
git add src/compositor/state/direct_scanout.rs
git commit -m "fix(scanout): use candidate SceneNode for physical presentation"
```

### Task 4: Verify boundaries and clean up only unused root blockers

**Files:**
- Inspect: `src/compositor/state/active_scene.rs`
- Inspect: `src/compositor/state/fullscreen.rs`
- Inspect: `src/native_output/runtime/scene_history.rs`
- Inspect: input callers of root-qualified physical helpers

**Interfaces:**
- Consumes: graph traces, source-layout checks, and focused/full verification output.
- Produces: evidence that NativeSceneHistory, input routing, root evidence, renderer/effects, transactions, Clip, and lifecycle/Lamp remain unchanged.

- [ ] **Step 1: Search all root-qualified physical helper callers.**
- [ ] **Step 2: Remove a root-specific identity helper only if no production or input caller remains; retain lower-level root getters needed by input.**
- [ ] **Step 3: Run Codebase Memory coverage for every operated-on path.**
- [ ] **Step 4: Run focused suites.**

```bash
rtk cargo test --locked --all-targets direct_scanout_tests
rtk cargo test --locked --all-targets presentation_animation
rtk cargo test --locked --all-targets xwayland_backing_replacement
rtk cargo test --locked --all-targets submitted_opacity_history_preserves_old_backing_evidence
```

- [ ] **Step 5: Run full repository verification.**

```bash
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk run ./bin/check-source-layout
```

- [ ] **Step 6: Commit any scoped cleanup or documentation update separately.**

```bash
git add <only-task-files>
git commit -m "docs(animation): close physical presentation owner qualification"
```
