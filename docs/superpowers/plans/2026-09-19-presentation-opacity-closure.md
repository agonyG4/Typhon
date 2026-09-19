# Typhon Presentation Opacity Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close WindowGroup PresentationOpacity lifetime and final-effect-alpha correctness, with focused regression coverage for rendering, inheritance, XWayland continuity, scanout, input, and CPU semantics.

**Architecture:** Capture the stable WindowGroup SceneNodeId before logical teardown and cancel all temporary PresentationEngine properties before scene-node removal. Resolve final effect presentation opacity once, apply it after effect alpha mode in the existing composite shader, and select source-over whenever the final contribution is translucent. Preserve canonical opacity, physical presentation evidence, root adapters, and existing rendering passes.

**Tech Stack:** Rust 2024, Cargo, glow/OpenGL ES 3 composite shaders, existing compositor state/render tests, Codebase Memory MCP, `rtk` command wrapper.

## Global Constraints

- `DesktopWindow` remains the persistent canonical opacity authority.
- `PresentationEngine` remains temporary opacity interpolation only.
- `WindowGroup SceneNodeId` remains semantic presentation owner identity.
- `root_surface_id` remains the frame/render/input adapter.
- `NativeSceneHistory` remains physical presentation authority.
- Logical destruction must cancel active tracks without destructive physical-root cleanup.
- PresentationOpacity is applied exactly once at each final WindowGroup contribution boundary.
- Internal effect targets retain replacement semantics; final translucent output uses premultiplied source-over.
- Add zero new offscreen framebuffers, mandatory render passes, window textures, animation threads, or timers.
- Do not implement Clip, migrate Lamp, or activate new visible fade effects.
- Preserve unrelated dirty work and stage only explicit closure paths in commits.
- Compile in the checkout’s existing Cargo target directory.

---

### Task 1: Retire all presentation properties during logical WindowGroup teardown

**Files:**
- Modify: `src/compositor/state/desktop_windows.rs:224-258`
- Test: `src/compositor/state/desktop_window_tests.rs`
- Test: `src/presentation_animation/transactions_tests.rs`

**Interfaces:**
- Consumes: `CompositorState::scene_node_id_for_window_group`, `PresentationEngine::cancel_all`, existing `PresentationFrameSnapshot` publication helpers.
- Produces: logical removal that cancels Geometry and Opacity tracks by stable WindowGroup SceneNodeId while retaining published physical evidence.

- [ ] **Step 1: Add the failing compositor teardown tests.**

Add tests that create an XDG `DesktopWindow`, capture its WindowGroup node, commit one mixed Geometry+Opacity transaction directly through `state.presentation_animator`, publish a frame snapshot containing the same root’s presented opacity, remove the window, and assert:

```rust
assert_eq!(state.presentation_animator.active_count(), 0);
assert_eq!(state.presentation_animator.transaction_count(), 0);
assert!(!state.presentation_animator.has_track(group));
assert_eq!(state.presented_presentation_frame_id(), frame_id);
assert!(state
    .presented_presentation
    .as_ref()
    .is_some_and(|frame| frame.opacities.iter().any(|entry| entry.root_surface_id == root)));
```

Add an opacity-only variant asserting the stable `group` has no opacity track and both counts decrease to zero. Use an existing `AnimationCurve::easing` and valid `PresentationRect` values.

- [ ] **Step 2: Run the focused tests and verify the expected red failure.**

Run:

```bash
rtk cargo test --locked desktop_window_tests::logical_window_removal_cancels_mixed_presentation_properties
rtk cargo test --locked desktop_window_tests::logical_window_removal_cancels_opacity_only_track
```

Expected: the mixed and opacity-only tests fail because logical removal currently calls `cancel_presentation_geometry_for_root` and leaves the opacity track/transaction member alive.

- [ ] **Step 3: Implement the minimal identity/order fix.**

In `remove_desktop_window`, capture:

```rust
let window_group_scene_node_id = self.scene_node_id_for_window_group(id);
```

before removing the `DesktopWindow`. After the logical removal and before `remove_window_scene_nodes(id)`, call `cancel_all` when the captured node exists. Remove only the old geometry-only call from this logical teardown path. Do not call `cancel_presentation_for_root` and do not mutate `presented_presentation`, `presented_window_geometries`, or other physical caches.

- [ ] **Step 4: Run the focused tests and verify green behavior.**

Run:

```bash
rtk cargo test --locked desktop_window_tests::logical_window_removal
rtk cargo test --locked presentation_animation::transactions_tests
```

Expected: teardown tests pass and existing transaction tests remain green.

- [ ] **Step 5: Commit only the teardown paths.**

```bash
rtk git add -- src/compositor/state/desktop_windows.rs src/compositor/state/desktop_window_tests.rs src/presentation_animation/transactions_tests.rs
rtk git commit -m "fix(animation): cancel all properties on logical window removal"
```

### Task 2: Make final effect blend policy opacity-aware

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs:491-507,3636-3922`
- Test: `src/egl_renderer/effects/executor.rs` test module

**Interfaces:**
- Consumes: `CompiledRenderPass`, `EffectAlphaMode`, `GlesSceneRenderer::presentation_opacity_for_visual_group`.
- Produces: `effect_pass_blend_mode(kind, output_is_framebuffer, alpha_mode, presentation_opacity)` and one resolved scalar reused for final uniform/blend selection.

- [ ] **Step 1: Extend the pure-policy tests before changing the helper.**

Update `effect_pass_blend_modes_are_explicit_for_each_pass_family` with these assertions:

```rust
assert_eq!(effect_pass_blend_mode(RenderPassKind::Composite, true, EffectAlphaMode::Opaque, 1.0), EffectPassBlendMode::Replace);
assert_eq!(effect_pass_blend_mode(RenderPassKind::Composite, true, EffectAlphaMode::Opaque, 0.5), EffectPassBlendMode::PremultipliedSourceOver);
assert_eq!(effect_pass_blend_mode(RenderPassKind::Composite, true, EffectAlphaMode::Opaque, 0.0), EffectPassBlendMode::PremultipliedSourceOver);
assert_eq!(effect_pass_blend_mode(RenderPassKind::Composite, true, EffectAlphaMode::Preserve, 1.0), EffectPassBlendMode::PremultipliedSourceOver);
assert_eq!(effect_pass_blend_mode(RenderPassKind::Composite, true, EffectAlphaMode::Preserve, 0.5), EffectPassBlendMode::PremultipliedSourceOver);
assert_eq!(effect_pass_blend_mode(RenderPassKind::Fragment, false, EffectAlphaMode::Preserve, 0.5), EffectPassBlendMode::Replace);
```

- [ ] **Step 2: Run the policy test and verify the expected red failure.**

Run:

```bash
rtk cargo test --locked effect_pass_blend_modes_are_explicit_for_each_pass_family
```

Expected: compile failure until the helper accepts the fourth argument, followed by the old Opaque/translucent policy failing once the signature is updated in the test.

- [ ] **Step 3: Implement the minimal blend-policy change.**

Add `presentation_opacity: f32` to the helper. Return `PremultipliedSourceOver` for final framebuffer Composite/OutputPostProcess passes when `alpha_mode == Preserve` or `presentation_opacity < 1.0`; otherwise preserve `Replace`. Pass identity opacity for internal targets.

- [ ] **Step 4: Resolve opacity once in final-pass execution.**

At the start of `execute_fullscreen_pass`, compute one local scalar:

```rust
let presentation_opacity = if output_is_framebuffer
    && matches!(pass.kind, RenderPassKind::Composite | RenderPassKind::OutputPostProcess)
{
    renderer.presentation_opacity_for_visual_group(pass.visual_group)
} else {
    1.0
};
```

Use this value for `effect_pass_blend_mode` and `u_presentation_opacity`. Let `establish_effect_pass_blend_state` own the source-over state; remove the old Preserve-only duplicate state setup.

- [ ] **Step 5: Run effect-policy and compile-focused tests.**

```bash
rtk cargo test --locked effect_pass_blend_modes_are_explicit_for_each_pass_family
rtk cargo test --locked effects
```

- [ ] **Step 6: Commit the policy change.**

```bash
rtk git add -- src/egl_renderer/effects/executor.rs
rtk git commit -m "fix(effects): source-over translucent final effect output"
```

### Task 3: Put PresentationOpacity outside effect alpha mode

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs:117-168`
- Test: `src/egl_renderer/effects/executor.rs` test module

**Interfaces:**
- Consumes: existing `COMPOSITE_FRAGMENT_SHADER` and sanitizer/color-conversion helpers.
- Produces: final composite shader order `effect conversion -> EffectAlphaMode -> PresentationOpacity -> sanitize`.

- [ ] **Step 1: Add the shader-order regression first.**

Add a test that finds `if (u_effect_force_opaque != 0)` and `result *= clamp(u_presentation_opacity, 0.0, 1.0)` in `COMPOSITE_FRAGMENT_SHADER`, asserts both exist, and asserts the force-opaque position precedes the opacity multiplication. Also assert the final line sanitizes `result`.

- [ ] **Step 2: Run the shader contract test and verify it fails.**

```bash
rtk cargo test --locked composite_shader_applies_presentation_opacity_after_effect_alpha_mode
```

Expected: failure because the current shader multiplies opacity before color conversion and force-opaque.

- [ ] **Step 3: Reorder only the existing shader statements.**

Keep the existing texture sampling and color-conversion calls. Move the PresentationOpacity multiplication below `u_effect_encode_srgb` and `u_effect_force_opaque`, then retain `typhon_sanitize_premultiplied(result)` as the output.

- [ ] **Step 4: Add semantic unit coverage for zero and half opacity.**

In the Rust test module, compute a premultiplied source after force-opaque semantics and assert the expected source values for opacity `1.0`, `0.5`, and `0.0`; include a destination source-over reference proving zero source leaves destination unchanged. This protects the shader contract without requiring physical GPU hardware.

- [ ] **Step 5: Run effect tests and commit.**

```bash
rtk cargo test --locked composite_shader_applies_presentation_opacity_after_effect_alpha_mode
rtk cargo test --locked effects
rtk git add -- src/egl_renderer/effects/executor.rs
rtk git commit -m "fix(effects): apply presentation opacity after effect alpha mode"
```

### Task 4: Verify CPU premultiplied semantics and renderer inheritance

**Files:**
- Test: `src/compositor/render.rs` test module
- Test: `src/compositor/state/desktop_window_tests.rs`
- Test: `src/compositor/tests/xwayland.rs`

**Interfaces:**
- Consumes: existing `scale_premultiplied_argb`, `scale_premultiplied_rgba`, `WindowVisualGroup`, and `PresentationGroupOpacity` paths.
- Produces: regression protection for client/popup/SSD owner inheritance, occlusion clearing, CPU scaling, canonical XWayland continuity, and input/layout separation.

- [ ] **Step 1: Add failing CPU helper tests.**

Test premultiplied `[R,G,B,A]` and ARGB samples at opacity `1.0`, `0.5`, and `0.0`; assert every channel, including alpha, is scaled and the `1.0` path preserves exact bytes. Add a source-over test with a non-opaque premultiplied source and known destination.

- [ ] **Step 2: Run the CPU tests and verify red if behavior regresses.**

```bash
rtk cargo test --locked compositor::render::tests::premultiplied_presentation_opacity
```

- [ ] **Step 3: Add compositor inheritance/occlusion tests.**

Use existing scene/command helpers to assert owner opacity `0.5` is used for an ordinary command, a popup visual group, and an SSD decoration; assert the same popup is not multiplied again and that opacity below `1.0` clears opaque regions while `1.0` retains them.

- [ ] **Step 4: Add XWayland canonical/backing continuity coverage.**

Create one XWayland `DesktopWindow`, set canonical opacity `0.5`, start an opacity transition on its stable WindowGroup node, sample with root A, replace the attachment with root B, and sample again. Assert WindowId, node, revision, canonical opacity, and continuous sampled value remain stable. Keep old frame evidence immutable by asserting the previously frozen root-A sample is unchanged.

- [ ] **Step 5: Add input/layout independence coverage.**

Set canonical/presented opacity to zero on a live test window, run existing hit-test/input resolution, and assert the window remains interactive. Record layout/configure counters before and after opacity sampling and assert no geometry/configure mutation occurs.

- [ ] **Step 6: Run focused renderer/XWayland tests and commit only clean paths.**

```bash
rtk cargo test --locked compositor::render::tests::premultiplied_presentation_opacity
rtk cargo test --locked desktop_window_tests::presentation_opacity
rtk cargo test --locked xwayland
rtk git add -- src/compositor/render.rs src/compositor/state/desktop_window_tests.rs src/compositor/tests/xwayland.rs
rtk git commit -m "test(renderer): cover opacity inheritance and occlusion"
```

### Task 5: Add direct-scanout opacity and physical-recovery regressions

**Files:**
- Test: `src/compositor/state/desktop_window_tests.rs`
- Test: `src/compositor/tests/direct_scanout.rs`
- Test: `src/native_output/output/presentation_damage.rs`

**Interfaces:**
- Consumes: existing `DirectScanoutSceneAnalysis`, `PresentationCoverageAnalysis`, `publish_presented_presentation`, and opacity damage helpers.
- Produces: visibility-aware canonical/active/presented opacity blockers, coverage/opacity separation, hidden-track non-blocking behavior, physical recovery, and opacity damage protection.

- [ ] **Step 1: Add direct-scanout failing tests.**

For an otherwise valid fullscreen XRGB candidate assert canonical opacity `0.5` rejects with `PresentationOpacity`; assert a visible active opacity track rejects; assert an unrelated hidden track does not reject the valid candidate. Assert `PresentationCoverageOpacity::OpaqueRgb8888` remains opaque independently of WindowGroup presentation opacity.

- [ ] **Step 2: Add physical recovery and damage tests.**

Publish physically presented opacity `0.5`, change canonical opacity to `1.0`, and settle the active track without publishing a new frame. Assert scanout remains blocked until a frame containing opacity `1.0` is published, then assert the opacity blocker clears. Add/extend physical opacity-damage tests for `1.0`, `0.5`, and `0.0` changes.

- [ ] **Step 3: Run focused scanout/damage tests and fix only production behavior if a new test exposes a gap.**

```bash
rtk cargo test --locked desktop_window_tests::direct_scanout_opacity
rtk cargo test --locked compositor::tests::direct_scanout
rtk cargo test --locked presentation_damage
```

- [ ] **Step 4: Commit scanout coverage.**

```bash
rtk git add -- src/compositor/state/desktop_window_tests.rs src/compositor/tests/direct_scanout.rs src/native_output/output/presentation_damage.rs
rtk git commit -m "test(scanout): cover opacity physical recovery"
```

### Task 6: Final verification and documentation handoff

**Files:**
- Modify: `docs/` status document only if an existing Opacity status file is present and the fresh evidence supports closure.

- [ ] **Step 1: Re-run all focused suites from the final tree.**

```bash
rtk cargo test --locked task_05_8
rtk cargo test --locked effects
rtk cargo test --locked xwayland
rtk cargo test --locked direct_scanout
rtk cargo test --locked presentation_transactions
rtk cargo test --locked presentation_damage
```

- [ ] **Step 2: Run the required full verification commands.**

```bash
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk run ./bin/check-source-layout
```

- [ ] **Step 3: Inspect the final diff and status.**

Confirm no Clip/Lamp/new fade effect/new GPU pass was added, no unrelated dirty path was staged, and source-layout diagnostics did not increase relative to the recorded baseline.

- [ ] **Step 4: Commit only documentation/status changes.**

```bash
rtk git add -- docs/
rtk git commit -m "docs(animation): close presentation opacity"
```

- [ ] **Step 5: Report starting/ending HEAD, preserved dirty paths, Codebase Memory generation, exact focused/full results, source-layout comparison, and deferred Presentation Engine work.**

