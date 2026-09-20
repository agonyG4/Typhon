# Typhon Presentation Effect Influence Damage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make physical PresentationOpacity and PresentationClip damage cover the complete final output influence of effects owned by the changed WindowGroup.

**Architecture:** Factor final effect output expansion into `effects::damage`; freeze one merged owner influence region per stable WindowGroup `SceneNodeId` into `NativeSceneSnapshot` during native frame resolution; consume only frozen previous/current evidence in the shared Opacity and Clip helpers. Keep root surface IDs as per-frame adapters and keep global effect transition damage separate.

**Tech Stack:** Rust 2024, Cargo, Typhon native Wayland output, Codebase Memory MCP, `rtk` command wrapper.

## Global Constraints

- `DesktopWindow` = canonical presentation state.
- `PresentationEngine` = temporary interpolation.
- `PresentationFrameSnapshot` = immutable presentation-property evidence.
- `NativeSceneHistory` = sole physical ready/submitted/presented authority.
- The source repositories are under `/home/agony/GitHub/Typhon`.
- ALL compilation, tests, benchmarks, generated build artifacts, and temporary build output MUST use `/mnt/Aether/Desktop/GitHub`.
- For Cargo/Rust builds, explicitly set `CARGO_TARGET_DIR` to Aether.
- Do not run a build command unless its output directory has been explicitly verified to be under `/mnt/Aether/Desktop/GitHub`.
- Do not create another presentation ledger or reconstruct effect influence at pageflip time.
- Keep global `effect_damage`, effect identity, and effect transition damage unchanged.
- Add 0 new GPU passes, 0 new framebuffer copies, 0 new textures, 0 new animation threads, and 0 new timers.
- Preserve unrelated dirty work and stage only closure paths in commits.
- Use output outsets, not source sample radius, for final effect influence.
- Do not change renderer/effect shader semantics or force full repaint through signatures.
- Do not implement lifecycle/Lamp migration or new visible animation effects.

---

## Baseline

- Starting HEAD: `2373f1c43444ac92cc21f0a2923c86bf9783c408`.
- Branch: `main`.
- The starting dirty files are unrelated compositor test support and XWayland selection files; none of the effect damage, native frame, presentation damage, or scene history paths required by this closure are dirty.
- The exact graph project is `home-agony-GitHub-Typhon`, rooted at `/home/agony/GitHub/Typhon`, indexed at the starting HEAD with status `ready` and generation matching source metadata.
- `rtk run ./bin/check-source-layout` already reports pre-existing over-limit files, including compositor test support, compositor state, EGL effect execution, native runtime, and XWayland files. Preserve this baseline without raising limits.
- Focused filters: `effects::damage`, `presentation_damage`, `scene_history`, `presentation_worker`, `presentation_opacity`, `presentation_clip`, and `xwayland`.

## File responsibilities

- `src/effects/damage.rs`: one canonical helper for visible-region expansion by aggregate output outsets; `plan_effect_damage` reuses it.
- `src/native_output/output/damage.rs`: immutable owner-scoped effect evidence type and `NativeSceneSnapshot` field.
- `src/native_output/output/presentation_damage.rs`: stable-owner Opacity and Clip damage and regression fixtures.
- `src/native_output/runtime/frame.rs`: capture the frame's trusted registry generation, frame output bounds, and owner mapping; freeze the merged regions before producing the resolved frame scene.
- `src/compositor/server.rs`: expose the existing output dimensions through a small server getter if frame resolution cannot otherwise access them.
- `src/native_output/runtime/scene_history.rs`: verify submitted snapshot evidence remains immutable across newer ready state and backing replacement.
- `src/native_output/tests/output.rs`: update any explicit `NativeSceneSnapshot` literal for the new defaulted field.

## Tasks

### Task 1: Reproduce missing effect-halo damage

**Files:**
- Modify: `src/native_output/output/presentation_damage.rs` tests only.

- [ ] Add a Clip regression with a 100×100 surface at `(100,100)` and a frozen conceptual effect halo at `(80,80)` sized 140×140. For Unbounded→Rect(surface), assert damage contains the old halo-only pixels on all four sides.
- [ ] Add an Opacity regression for the same owner and halo, changing opacity `1.0→0.5`; assert damage includes pixels outside the surface.
- [ ] Verify the focused tests fail on the current implementation because only surface/SSD regions are damaged.

Run after verifying Cargo's reported target directory:

```bash
CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target rtk cargo test --locked --lib presentation_damage::clip_hides_old_effect_halo
CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target rtk cargo test --locked --lib presentation_damage::opacity_damages_effect_halo
```

### Task 2: Reuse one final-output influence calculation

**Files:**
- Modify: `src/effects/damage.rs`.

**Interface:**
- Add `effect_output_influence_region(footprint: EffectFootprint, visible_region: &EffectRegion, output_bounds: EffectRect) -> EffectRegion`.

- [ ] Add a failing unit test with unequal sample radii and output outsets; expected influence expands only by `max(left,right)` and `max(top,bottom)`, then clamps to output bounds.
- [ ] Run the exact focused test and confirm the missing-helper failure.
- [ ] Implement the helper by calling the existing bounded `EffectRegion::expand_clamped_xy` with output outsets only.
- [ ] Replace only the output-influence expansion inside `plan_effect_damage`; retain capture and source-dependency calculations unchanged.
- [ ] Run `effects::damage` tests.

### Task 3: Freeze owner-scoped effect influence in the resolved frame

**Files:**
- Modify: `src/native_output/output/damage.rs`.
- Modify: `src/native_output/runtime/frame.rs`.
- Modify: `src/compositor/server.rs` only if a public output-dimensions wrapper is needed.
- Modify: `src/native_output/tests/output.rs` only for explicit snapshot literals.
- Test: native frame tests.

**Interface:**
- Add `NativePresentationEffectInfluenceSnapshot { scene_node_id: SceneNodeId, presentation_owner_root_surface_id: u32, region: EffectRegion }`.
- Add `presentation_effect_influences: Vec<NativePresentationEffectInfluenceSnapshot>` to `NativeSceneSnapshot`.

- [ ] Add a failing focused builder test for merge-by-owner, anchor-to-root-to-SceneNode resolution, OutputPostProcess exclusion, output-outset expansion, and missing-program fallback to output bounds.
- [ ] Capture `server.trusted_effect_registry().current()` adjacent to `resolved_effect_scene_for_presentation` and use only that generation to read each `ValidatedEffectProgram.aggregate_footprint`.
- [ ] Resolve BeforeSurface, ReplaceSurface, and AfterSurface anchors using `presentation_owner_root_for_surface(surface)` followed by `presentation_scene_node_id_for_root(root)`; retain the root on the evidence record.
- [ ] Merge every owned final effect instance into one bounded `EffectRegion` per SceneNodeId. Skip `OutputPostProcess`.
- [ ] If a resolved program is missing, record output bounds for its resolved owner and emit a diagnostic; do not panic or drop the influence.
- [ ] Freeze the records into the frame's `NativeSceneSnapshot` before it reaches the ready/submitted/presented history.
- [ ] Run the new frame evidence test and native frame tests.

### Task 4: Include owned influence in shared Opacity and Clip damage

**Files:**
- Modify: `src/native_output/output/presentation_damage.rs`.

- [ ] Match changed Opacity groups by `scene_node_id`, use each frame's root ID only to locate its surfaces/decorations, and union each frame's matching frozen effect region.
- [ ] For Clip changes, union previous and current matching effect regions after intersecting each with that frame's physical Clip. Treat Unbounded as no mask and zero-area Rect as empty.
- [ ] Add Clip reveal and Rect-A→Rect-B tests with hand-derived old/new intersections.
- [ ] Add Opacity and Clip unrelated-owner isolation tests.
- [ ] Add a stable SceneNode test where frame A uses root A/effect EA and frame B uses root B/effect EB; assert both old and new influence are damaged for the changed owner.
- [ ] Run `presentation_damage` tests.

### Task 5: Verify physical history retains submitted evidence

**Files:**
- Modify: `src/native_output/runtime/scene_history.rs` tests.

- [ ] Extend a submitted-frame/backing-replacement regression so frame A's SceneNode, root, property state, and effect region remain the evidence used after newer root B/effect EB/property state has become ready.
- [ ] Assert pageflip transition damage consumes submitted A's frozen evidence and does not inspect the current compositor registry or scene.
- [ ] Run `scene_history` and `presentation_worker` focused tests; both paths must use the shared helpers.

### Task 6: Fresh verification and commits

**Files:**
- Modify the relevant animation documentation with the completed physical-damage closure after fresh verification.

- [ ] Verify `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target` using Cargo metadata before compiling.
- [ ] Run `rtk cargo fmt --check`.
- [ ] Run `rtk cargo check --locked --all-targets` with the verified Aether target directory.
- [ ] Run `rtk cargo clippy --locked --all-targets -- -D warnings` with the verified Aether target directory.
- [ ] Run `rtk cargo test --locked` with the verified Aether target directory.
- [ ] Run the focused filters: `effects::damage`, `presentation_damage`, `scene_history`, `presentation_worker`, `presentation_opacity`, `presentation_clip`, and `xwayland`.
- [ ] Run `rtk run ./bin/check-source-layout`; compare failures to the captured baseline and do not change limits.
- [ ] Review that renderer signatures, shaders, executor semantics, GPU resources, and global effect transition authority are unchanged.
- [ ] Commit only closure files and documentation in reviewable logical commits. Never stage or include the pre-existing dirty files.

---

## Acceptance

Physical Presentation damage covers previous and current surface, SSD, popup-owned, and effect output contributions for changed owners; remains isolated by stable WindowGroup SceneNodeId; survives XWayland backing replacement and delayed pageflip; and uses immutable frame evidence on both scene-history and presentation-worker paths.
