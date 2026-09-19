# Direct Scanout Effect Contribution Analysis Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Direct Scanout reject only effects that can contribute pixels to the exact frame replaced by an opaque full-output scanout source, while sharing presentation visibility with the renderer.

**Architecture:** Add a typed Direct Scanout effect analysis beside the existing scene blockers. Centralize lifecycle/fullscreen-plan filtering in `effects.rs`; the renderer applies transforms to that filtered scene, while Direct Scanout analyzes the canonical scene after discovering the exact presentation-coverage source. Store the analysis on `DirectScanoutSceneAnalysis` and pass it directly to doctor formatting.

**Tech Stack:** Rust, compositor scene/effect model, `EffectRegion`, presentation coverage, existing compositor and native-output tests, RTK command wrappers.

## Global Constraints

- Do not change `EffectSceneSummary.requires_composition`; it remains generic renderer state.
- Do not special-case blur, Eclipse, fullscreen applications, or program names.
- Use `FullscreenCompositionPlan::allows_presentation_root()` and lifecycle suppression through one shared helper.
- Use active scene `VisualGroupId`, `EffectAnchorScope`, `EffectAnchor`, and surface order; never raw IDs or creation order.
- Occlusion requires authoritative full-output coverage and proven opaque presentation coverage.
- `OutputPostProcess` always blocks when it intersects the output.
- Preserve all existing non-effect Direct Scanout blockers and the NVIDIA DMA-BUF/KMS path.
- Preserve a zero-effect fast path and bounded doctor detail storage; no per-frame formatting or logging.
- Compile in `/home/agony/GitHub/Typhon` and use `rtk` for repository commands.
- Preserve unrelated existing worktree changes and commit only task-owned files in each task.

---

### Task 1: Add canonical presentation filtering and typed effect-analysis primitives

**Files:**
- Modify: `src/compositor/effects.rs`
- Modify: `src/compositor/direct_scanout.rs`
- Test: `src/compositor/state/task_05_8_tests.rs`

**Interfaces:**
- `CompositorState::resolved_effect_scene_for_composition_plan(&FullscreenCompositionPlan) -> ResolvedEffectScene` filters lifecycle-suppressed and plan-disallowed roots.
- `CompositorState::resolved_effect_scene_with_presentation(...)` consumes the canonical filtered scene before applying transforms.
- `DirectScanoutEffectDisposition` is a typed enum with `PresentationCulled`, `OutsideOutput`, `OccludedByOpaqueScanoutSource`, `ContributingAboveSource`, `ContributingAtSource`, `OutputPostProcess`, and `UnknownOrder`.
- `DirectScanoutEffectAnalysis` stores raw, presentation, culled, outside, occluded, contributing counts, `requires_composition`, and a bounded vector of typed per-instance evidence.

- [ ] **Step 1: Write the failing canonical-filter and zero-path assertions.**

Extend the existing fullscreen presentation test to call the canonical-plan helper and assert that the panel-root effect is absent under the dominant fullscreen plan while the output post-process remains. Add a zero-effect assertion that the Direct Scanout effect analysis equals its default and does not require composition.

- [ ] **Step 2: Run the focused tests and verify the expected API/behavior failure.**

Run:

```bash
rtk test cargo test fullscreen_presentation_filters_anchored_effects_with_surface_visibility -- --exact
```

Expected: compilation fails because the canonical helper and typed analysis are not present.

- [ ] **Step 3: Extract the shared root-visibility predicate and canonical scene helper.**

Move the lifecycle/fullscreen root filter from `resolved_effect_scene_with_presentation` into one helper used by both `resolved_effect_scene_for_composition_plan` and the presentation-transforming renderer path. Keep `OutputPostProcess` unconditionally presentation-visible.

- [ ] **Step 4: Add typed Direct Scanout analysis data with a default zero state.**

Define the disposition enum, bounded per-instance evidence, and aggregate counts in `direct_scanout.rs`. Keep the existing generic summary untouched. Add a bounded maximum for retained detail evidence and make `requires_composition` derive solely from `contributing_instance_count > 0`.

- [ ] **Step 5: Run the focused tests and verify the canonical filter is green.**

Run:

```bash
rtk test cargo test fullscreen_presentation_filters_anchored_effects_with_surface_visibility -- --exact
```

Expected: PASS, including the existing renderer behavior and the new canonical-plan assertions.

- [ ] **Step 6: Commit the typed primitives and canonical filter.**

```bash
rtk git add src/compositor/effects.rs src/compositor/direct_scanout.rs src/compositor/state/task_05_8_tests.rs
rtk git commit -m "refactor: share presentation effect visibility authority"
```

### Task 2: Implement source-relative contribution classification and integrate scene eligibility

**Files:**
- Modify: `src/compositor/state/direct_scanout.rs`
- Modify: `src/compositor/direct_scanout.rs`
- Test: `src/compositor/tests/direct_scanout.rs`
- Test: `src/compositor/state/task_05_8_tests.rs`

**Interfaces:**
- `CompositorState::direct_scanout_effect_analysis(&FullscreenCompositionPlan, BufferSize, Option<DirectScanoutEffectSource>) -> DirectScanoutEffectAnalysis` uses the canonical filtered scene and exact source ordering.
- `DirectScanoutEffectSource` carries the exact source surface, optional active visual-group/surface order, and the proven opaque/full-output occlusion gate.
- `DirectScanoutSceneAnalysis.effects` is the single effect-contribution result used by eligibility and doctor.

- [ ] **Step 1: Write failing source-relative tests.**

Update `resolved_blur_scene_controls_direct_scanout_transition` so the resolved `BeforeSurface(scanout_source)` blur has raw/presentation/occluded/contributing counts of `1/1/1/0` and the candidate is allowed. Add focused tests for source `BeforeSurface`, `ReplaceSurface`, and `AfterSurface`; lower and higher Surface-scope anchors; same-group VisualGroup-scope phases; lower and higher visual groups; outside, partial, and conservative-full regions; and unknown/translucent source coverage.

- [ ] **Step 2: Run the focused tests to verify they fail for the old raw-summary blocker.**

Run:

```bash
rtk test cargo test resolved_blur_scene_controls_direct_scanout_transition -- --exact
rtk test cargo test direct_scanout -- --exact
```

Expected: the updated resolved-blur test fails with the old `EffectRequiresComposition` result, and new focused tests fail because contribution analysis is not integrated.

- [ ] **Step 3: Add exact source order and opaque-coverage gating.**

After `PresentationCoverageAnalysis` identifies `covering_surface`, derive the source group order from `visual_group_for_surface(source_surface_id)` and source surface order from `active_scene_surfaces()`. Set the occlusion gate only when the source is the exact full-output coverage surface, `coverage.can_occlude_behind_content()` is true, and the source buffer format proves opaque RGB8888.

- [ ] **Step 4: Implement output intersection and semantic disposition rules.**

For each raw effect instance, use the shared plan filter. Count filtered instances as presentation-visible; classify non-empty `instance.region.intersect_rect(output_bounds)`. Apply group order first. For equal groups, compare Surface-scope anchored surface order; for VisualGroup-scope use only anchor phase. `BeforeSurface(source)` occludes, while source `ReplaceSurface` and `AfterSurface` contribute. Missing order or missing occlusion proof produces `UnknownOrder`; post-process produces `OutputPostProcess` and is never occluded.

- [ ] **Step 5: Move the effect blocker after exact source discovery.**

Remove the early raw-summary call to `direct_scanout_scene_rejection_for_effects`. Once the exact source and source proof are available, add `EffectRequiresComposition` only when the typed analysis requires composition. Preserve every existing blocker and early-return path; when no exact source exists, keep presentation-visible effects conservative without authorizing occlusion.

- [ ] **Step 6: Run the focused tests and verify source-relative behavior.**

Run:

```bash
rtk test cargo test resolved_blur_scene_controls_direct_scanout_transition -- --exact
rtk test cargo test direct_scanout -- --exact
rtk test cargo test state::task_05_8_tests -- --exact
```

Expected: the resolved blur below an opaque full-output source no longer blocks; contributing phases, partial intersections, conservative-full regions, and unknown opacity remain conservative.

- [ ] **Step 7: Commit the eligibility integration.**

```bash
rtk git add src/compositor/state/direct_scanout.rs src/compositor/direct_scanout.rs src/compositor/tests/direct_scanout.rs src/compositor/state/task_05_8_tests.rs
rtk git commit -m "feat: analyze direct scanout effect contribution"
```

### Task 3: Make doctor consume the exact typed analysis

**Files:**
- Modify: `src/compositor/direct_scanout_doctor.rs`
- Modify: `src/compositor/effects.rs`
- Modify: `src/compositor/server.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Test: `src/native_output/runtime/cycle_dispatch.rs`

**Interfaces:**
- `CompositorState::direct_scanout_effect_doctor_details(&DirectScanoutSceneAnalysis) -> DirectScanoutEffectDoctorDetails` formats the passed analysis and trusted program registry; it does not rebuild an effect scene.
- `DirectScanoutDoctorScene` stores raw, presentation, culled, outside, occluded, contributing counts, `effect_requires_composition`, and bounded details.

- [ ] **Step 1: Write failing doctor consistency assertions.**

Extend doctor formatting tests for raw/presentation/culling, raw/presentation/occluded, and raw/presentation/contributing cases. Assert `effect_requires_composition == contributing_instance_count > 0`, and assert bounded detail output includes `presentation_culled`, `occluded_by_scanout_source`, or `contributing_*` dispositions.

- [ ] **Step 2: Run the focused doctor tests and verify the old output fails the new assertions.**

Run:

```bash
rtk test cargo test direct_scanout_doctor -- --exact
```

Expected: existing output lacks the new fields/dispositions.

- [ ] **Step 3: Replace raw-scene doctor reconstruction with analysis formatting.**

Pass the already-created `DirectScanoutSceneAnalysis` from the `Doctor` control path through the server into the doctor formatter. Resolve program names only for bounded entries, retain the existing raw count, preserve the existing maximum detail bound, and add the new aggregate fields to the formatted output.

- [ ] **Step 4: Run doctor tests and verify the shared decisions.**

Run:

```bash
rtk test cargo test direct_scanout_doctor -- --exact
rtk test cargo test cycle_dispatch -- --exact
```

Expected: doctor counts and `effect_requires_composition` match the scene analysis used by eligibility.

- [ ] **Step 5: Commit doctor observability.**

```bash
rtk git add src/compositor/direct_scanout_doctor.rs src/compositor/effects.rs src/compositor/server.rs src/native_output/runtime/cycle_dispatch.rs
rtk git commit -m "feat: report direct scanout effect contribution evidence"
```

### Task 4: Complete regression coverage and perform repository validation

**Files:**
- Modify: `src/compositor/state/task_05_8_tests.rs`
- Modify: `src/compositor/tests/direct_scanout.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs` (only if test fixtures require it)

- [ ] **Step 1: Add the remaining behavior tests.**

Cover fullscreen-plan culling without an effect blocker, output post-process remaining blocking, zero-effect default analysis, Surface versus VisualGroup semantics, different visual groups, output intersection, conservative-full regions, and translucent/unknown coverage. Keep the existing presentation-filter and DMA-BUF/KMS tests intact.

- [ ] **Step 2: Run focused test groups.**

Run:

```bash
rtk test cargo test direct_scanout
rtk test cargo test task_05_8_tests
rtk test cargo test cycle_dispatch
```

Expected: all focused groups pass with no effect-specific regressions.

- [ ] **Step 3: Run formatting, compilation, tests, and clippy in the repository folder.**

Run:

```bash
rtk err cargo fmt --check
rtk err cargo check
rtk test cargo test
rtk err cargo clippy --all-targets --all-features -- -D warnings
```

Record exact exit codes and distinguish unrelated pre-existing failures from task failures.

- [ ] **Step 4: Inspect the final diff and graph impact, then commit task changes.**

Run:

```bash
rtk git diff --check
rtk git status --short
rtk git diff HEAD~3..HEAD --stat
```

Commit only remaining task-owned changes:

```bash
rtk git add src/compositor/effects.rs src/compositor/direct_scanout.rs src/compositor/direct_scanout_doctor.rs src/compositor/state/direct_scanout.rs src/compositor/state/task_05_8_tests.rs src/compositor/tests/direct_scanout.rs src/compositor/server.rs src/native_output/runtime/cycle_dispatch.rs
rtk git commit -m "test: cover presentation-aware effect scanout decisions"
```

- [ ] **Step 5: Report hardware validation separately.**

Do not claim hardware success from unit tests. On the RTX 3060 Ti, run with:

```bash
OBLIVION_ONE_DMABUF_KMS_PREFERRED=auto OBLIVION_ONE_DIRECT_SCANOUT=experimental-auto <known-native-wine-wayland-launch-command>
```

Keep Eclipse and normal blur enabled; collect doctor evidence while Cyberpunk is fullscreen, while a real shell overlay is above it, after closing the overlay, and on normal desktop blur. Record submissions/presentations and any unrelated environmental limitation separately.
