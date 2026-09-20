# Typhon Physical Effect Damage Closure Implementation Plan

> **For agentic workers:** Execute this plan inline with `superpowers:executing-plans`. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Commit effect-expanded physical transition damage and resolved-frame continuous dirty evidence to the buffer-age journal.

**Architecture:** Freeze a renderer-independent per-frame recipe while `ResolvedNativeFrameScene` owns the resolved effects, captured `EffectRegistryGeneration`, and output bounds. Copy that evidence into `NativeFrameSceneSnapshot`; at pageflip, reconstruct the source transition from PRESENTED and the token-selected SUBMITTED snapshot, add the current frame's frozen dirty region, and call `plan_effect_damage()` over both frozen recipes without recursively propagating generated output.

**Tech Stack:** Rust 2024, Cargo, Typhon native output, effects damage planner, Codebase Memory MCP, RTK.

## Global Constraints

- Keep compilation, tests, benchmarks, generated artifacts, and temporary build output under `/mnt/Aether/Desktop/GitHub`.
- Set `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target` for every Cargo build or test command.
- Verify Cargo's effective target directory before the first build command; stop if it resolves inside `/home/agony/GitHub/Typhon`.
- Use RTK for Rust, build, test, and Git work where available.
- Preserve the existing unrelated dirty files and stage only files belonging to this task.
- Do not use subagents.
- Preserve exact PRESENTED plus token-selected SUBMITTED pageflip authority; do not use READY state, RepaintPlan, or render-time final damage as physical history.
- Keep Presentation Clip/Opacity effect damage owner-scoped and do not globally invalidate effects.
- Use existing `plan_effect_damage()` semantics once per previous/current visible instance from the same unchanged source region.
- Freeze all footprints and frame-local dirty evidence before READY; perform no live compositor or registry query at pageflip.
- Preserve Direct Scanout, ACK ownership, `RenderableSurface::visual_clip`, effect dependency semantics, and buffer-age slot ownership.

## File Responsibilities

- Create `src/native_output/output/physical_effect_damage.rs` for frozen recipe evidence, recipe freezing, renderer-independent physical effect expansion, and focused unit tests.
- Modify `src/native_output/output/mod.rs` to declare and re-export the focused module.
- Modify `src/native_output/runtime/frame.rs` to freeze the recipe from the exact resolved scene and captured registry generation while output bounds are available.
- Modify `src/native_output/runtime/scene_history.rs` to carry frozen evidence and expand reconstructed physical transitions from the exact physical predecessor and submitted token.
- Modify `src/native_output/runtime/presentation_cycle.rs` to use the resolved frame's frozen dirty region for its render-time source damage, keeping it identical to pageflip evidence.
- Add pageflip evidence regressions in `src/native_output/runtime/physical_effect_damage_tests.rs` so READY/SUBMITTED/PRESENTED cases do not grow the scene-history module.
- Add the real age-2/age-3 repair regression in `src/egl_renderer/damage_tests.rs` at the existing `PartialRepaintPlanner` journal boundary.

## Plan

### Task 1: Reproduce omitted foreign-effect output at physical pageflip

**Files:**
- Create: `src/native_output/runtime/physical_effect_damage_tests.rs`
- Modify: `src/native_output/runtime/mod.rs` to include the test module.

- [ ] Build two frame snapshots with an A Clip transition and an unchanged B effect recipe whose capture/source region observes A's source pixels. Assert `clip_damage_for_frame_snapshots()` contains A's owner contribution and excludes B's output region.
- [ ] Assert `prepare_pageflip_transition()` contains B's expected final output region; keep the owner-scope assertion separate so a global Presentation invalidation cannot satisfy both checks.
- [ ] Add a second unchanged effect with a non-intersecting source/output domain and assert it remains absent from transition damage.
- [ ] Run the focused test with `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target rtk cargo test --locked --lib physical_pageflip_expands_clip_source_for_observing_foreign_effect`; confirm it fails because B's derived output is missing.

### Task 2: Freeze the per-frame effect recipe and resolved dirty evidence

**Files:**
- Create: `src/native_output/output/physical_effect_damage.rs`
- Modify: `src/native_output/output/mod.rs`
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/native_output/runtime/scene_history.rs`
- Test: `src/native_output/output/physical_effect_damage.rs`

**Evidence types:**

```rust
pub(crate) struct NativeEffectDamageInstanceSnapshot {
    pub(crate) instance_id: EffectInstanceId,
    pub(crate) region: EffectRegion,
    pub(crate) aggregate_footprint: EffectFootprint,
}

pub(crate) struct NativeEffectDamageFrameSnapshot {
    pub(crate) instances: Vec<NativeEffectDamageInstanceSnapshot>,
    pub(crate) frame_local_dirty: EffectRegion,
    pub(crate) conservative_full: bool,
}

pub(crate) fn freeze_native_effect_damage(
    effects: &ResolvedEffectScene,
    registry_generation: &EffectRegistryGeneration,
    output_bounds: EffectRect,
) -> NativeEffectDamageFrameSnapshot;

pub(crate) fn expand_physical_effect_damage(
    base_damage: NativeOutputDamage,
    previous: &NativeEffectDamageFrameSnapshot,
    current: &NativeEffectDamageFrameSnapshot,
    output_bounds: EffectRect,
) -> NativeOutputDamage;
```

- [ ] Add freeze tests for all non-empty resolved instances, including `OutputPostProcess`; exclude empty regions exactly as `compile_frame_execution_plan()` does.
- [ ] Assert each record uses the aggregate footprint from the captured `EffectRegistryGeneration`, not a lookup deferred to pageflip.
- [ ] Assert the snapshot copies `ResolvedEffectScene::frame_demand_snapshot().dirty_region` from the represented resolved scene.
- [ ] Assert a missing validated program marks the frame recipe `conservative_full` without silently dropping the instance.
- [ ] Run the new freezer tests and confirm the missing freezer/evidence is the only failure.
- [ ] Implement the evidence and freezer in `physical_effect_damage.rs`; add the snapshot to `ResolvedNativeFrameScene`, freeze it beside the captured generation and bounds, and copy it into `NativeFrameSceneSnapshot`.
- [ ] Run the freezer tests plus `native_output::runtime::frame_tests` and `native_output::runtime::scene_history` tests.

### Task 3: Expand one physical source region through old and new recipes

**Files:**
- Modify: `src/native_output/output/physical_effect_damage.rs`
- Test: `src/native_output/output/physical_effect_damage.rs`

- [ ] Add a test for same-source expansion through previous and current recipes; include a previous-only effect with non-zero outsets and a current-only effect, and assert both old and new output extents are covered.
- [ ] Add an unrelated-effect test whose source/capture domain does not intersect the input transition; assert its output stays clean.
- [ ] Add an `OutputPostProcess` case with a non-zero frozen footprint and assert the output expands.
- [ ] Add zero/empty, empty instance region, full source, and incomplete-recipe cases. Empty source stays empty; full source stays full; incomplete evidence turns a non-empty transition into bounded full-output damage.
- [ ] Add renderer-parity cases that feed identical source damage, visible regions, and footprints to `compile_frame_execution_plan()` and physical expansion, then compare final damaged output regions.
- [ ] Verify the unit tests fail for missing expansion, then implement `expand_physical_effect_damage()` using `plan_effect_damage()` for every previous and current recipe entry, always passing the original source region and unioning only `output_damage` results.
- [ ] Run all `physical_effect_damage` unit tests and the `effects::damage` and `effects::render_graph` suites.

### Task 4: Integrate expanded evidence at confirmed pageflip

**Files:**
- Modify: `src/native_output/runtime/scene_history.rs`
- Modify: `src/native_output/runtime/presentation_cycle.rs`
- Test: `src/native_output/runtime/physical_effect_damage_tests.rs`

- [ ] Keep the existing exact token lookup and actual `self.presented` predecessor unchanged.
- [ ] Keep scene, effect transition, cursor, Presentation Opacity/Clip, and lifecycle damage as the base physical transition.
- [ ] Add only the token-selected current frame's frozen `frame_local_dirty` to the source transition.
- [ ] Expand the source through both previous and current frozen recipes, then return the expanded result in `PreparedNativePresentationTransition.damage`.
- [ ] Change render-time continuous dirty union to use the resolved frame's frozen local dirty region, not the separate current compositor query; retain the compositor demand snapshot for scheduling decisions.
- [ ] Extend the foreign-backdrop test and verify the existing `presentation_effect_damage_does_not_include_unrelated_owners` regression remains unchanged and passes.
- [ ] Add delayed-pageflip coverage where C is READY but B's token presents; assert B's footprint and local dirty are used.
- [ ] Add render-ahead coverage where B is submitted/rejected and C presents after A; assert transition damage is based on A→C and contains no B-only footprint.
- [ ] Run the two focused history cases and the existing Presentation Clip/Opacity damage tests.

### Task 5: Verify the expanded physical journal repairs aged buffers

**Files:**
- Modify: `src/egl_renderer/damage_tests.rs`
- Test: `src/egl_renderer/damage_tests.rs`

- [ ] Connect a prepared physical transition to `PartialRepaintPlanner::commit_presented_transition()` in the test; commit the derived effect output region, then request an age-2 repaint and assert that region is present in `repair_damage`.
- [ ] Repeat with an age-3 slot after two committed transitions, asserting the derived output from two physical frames ago is restored.
- [ ] Include a current continuous dirty region with unchanged effect identity, region, and scene identity; assert it is in the prepared physical transition and later age repair.
- [ ] Run the journal regressions plus all `egl_renderer::damage_tests` and render-ahead/pageflip predecessor tests.

### Task 6: Fresh verification and task-only commit

- [ ] Verify the target path with `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target rtk cargo metadata --format-version 1 --no-deps`; confirm its `target_directory` is under `/mnt/Aether/Desktop/GitHub` before any build or test.
- [ ] Run `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target rtk cargo fmt --check`.
- [ ] Run focused suites for `physical_effect_damage`, native physical scene history, Presentation damage, effect damage/render graph, EGL partial repaint/buffer age, and pageflip/render-ahead.
- [ ] Run `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target rtk cargo check --locked --all-targets`.
- [ ] Run `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target rtk cargo clippy --locked --all-targets -- -D warnings`.
- [ ] Run `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target rtk cargo test --locked`.
- [ ] Run `rtk run ./bin/check-source-layout`; compare results with the pre-existing baseline and do not change limits.
- [ ] Review the final diff against the ten adversarial questions in the task, confirm only task files are staged, and commit the implementation as a separate logical commit.

## Regression Coverage Map

- Foreign backdrop versus owner isolation: Task 1 and Task 4.
- Unrelated effect exclusion: Task 1 and Task 3.
- Age-2/age-3 physical journal repair: Task 5.
- Actual physical predecessor and rejected render-ahead: Task 4.
- Delayed pageflip ignores READY: Task 4.
- Removed and introduced effects: Task 3.
- Output postprocess: Tasks 2 and 3.
- Continuous resolved-frame dirty evidence: Tasks 2, 4, and 5.
- Empty/full/incomplete evidence behavior: Task 3.
- Renderer parity: Task 3.
