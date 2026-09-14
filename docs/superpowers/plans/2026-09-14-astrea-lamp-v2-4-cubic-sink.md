# Astrea Lamp v2.4 Canonical Cubic Funnel Rails and Icon Sink Implementation Plan

> **For agentic workers:** Execute this plan inline in the current checkout. Do not dispatch subagents. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep the proven v2.2 temporal Lamp motion and v2.3 Dock occlusion unchanged while replacing the aspect-dependent portal endpoint with a fixed-depth sink and replacing the spatial power funnel with a canonical cubic rail profile.

**Architecture:** `LifecycleVisualGroup` remains the frozen CPU authority. It will retain `anchor_rect` for destination identity/damage, retain `portal_rect` as the aspect-preserved cross-axis aperture, and add one frozen `sink_rect` whose one-pixel main-axis sliver is placed at a source-aspect-independent depth inside the anchor. CPU code will calculate all spatial geometry and temporal channels; the GLES vertex shader will consume frozen rectangles and CPU channels and perform only the spatial warp. v2.3 Dock promotion and cache partitioning remain untouched.

**Tech Stack:** Rust unit tests and pure geometry helpers; GLES 3.0 ES vertex shader in `src/egl_renderer/program.rs`; existing `GlesSceneRenderer` uniform and geometry-key plumbing; `rtk cargo` verification.

## Global Constraints

- CPU remains the sole authority for temporal channels and spatial reference geometry.
- GLSL remains spatial-only; do not duplicate timing or easing equations in the shader.
- The first RED regression must prove actual aggregate mesh velocity collapse in the old model, where existing v2.2 coverage already protects that behavior; no scalar-only replacement is acceptable.
- Preserve exact `p=0` presented-source geometry, exact `p=1` sink mapping, and exact positional continuity across arbitrary mid-flight reversal.
- `retreat_progress` remains a pure bounded function of raw progress and never becomes a sequential pre-animation phase.
- Contraction and translation continue to overlap; no hidden sequential stage machine may survive under new names.
- Do not change v2.2 temporal constants, `lamp_motion_channels`, mesh density, the 280 ms duration, opacity policy, lifecycle ownership, settlement, SSD freezing, Effects, Blur, KMS, pacing, or the known live-subsurface snapshot-ownership gap.
- Preserve every unrelated dirty native-input, compositor, Effects, fullscreen, pacing, and screenshot change exactly.
- Do not modify Eclipse. No file outside the three identified Lamp files may be edited unless a failing test demonstrates a concrete compile or integration dependency; any such dependency must be reported before editing that file.
- Do not change v2.3 Dock ordering, multi-output matching, partition signature, or external-overlay behavior unless a v2.3 regression fails.

---

## Planned file boundaries

- Modify `src/window_lifecycle_animation.rs` for the frozen sink field, sink construction/validation/signatures, CPU cubic rail, CPU warp target, and geometry/TDD tests. Existing v2.2 temporal code and aggregate velocity tests stay unchanged.
- Modify `src/egl_renderer/program.rs` for the spatial-only `u_sink_rect` target, cubic rail constants/equation, and shader contract tests. No timing formula is added.
- Modify `src/egl_renderer.rs` for sink uniform lookup/upload, Lamp geometry-key identity, and renderer tests. Preserve all concurrent Effects/native-input code in the dirty file.
- No other production file is expected. In particular, Eclipse and compositor Dock ordering are already correct in v2.3; a newly required file must be justified by a test or compiler error before it is touched.
- Create this plan file only for the documented plan; it is not production behavior.

## TDD sequence and implementation tasks

### Task 1: Characterize the v2.3 terminal-depth defect

**Files:**
- Modify: `src/window_lifecycle_animation.rs` test module only.
- Test: the new focused Lamp regression in the same module.

**Interfaces:**
- Consume the existing `LifecycleVisualGroup::from_bounds`, `lamp_warp_visual_point`, `lamp_portal_rect`, and `PresentationRect` behavior.
- Produce a deterministic regression fixture for the old endpoint that will become the sink-depth independence test after production changes.

- [ ] **Step 1: Add the RED test before production edits.** Build one square anchor and three source visual groups with wide, portrait, and square aspect ratios, all moving toward the same Bottom Dock. Sample representative source points at `p=1`, compute the actual endpoint span of `lamp_warp_visual_point`, and assert that terminal main-axis depth is equal across aspects. The current v2.3 portal endpoint must fail this assertion because its fitted portal heights are different.
- [ ] **Step 2: Run only the new test and verify the expected behavior failure.** Run `rtk cargo test --locked v23_terminal_depth_varies_with_source_aspect_ratio` (or the final test name). Expected RED evidence is a normal assertion failure showing different wide/portrait/square endpoint depths, not a compile error.
- [ ] **Step 3: Record the baseline values in the test failure/implementation notes.** With a 64x64 square anchor, the representative fitted portal depths are expected to differ (for example, a 16:9 source produces a shallower vertical portal than a portrait source); do not weaken the test to match v2.3.

### Task 2: Add frozen sink geometry and preserve lifecycle identity

**Files:**
- Modify: `src/window_lifecycle_animation.rs` production geometry and signatures, plus its tests.

**Interfaces:**
- Add `LifecycleVisualGroup::sink_rect: PresentationRect`.
- Add `pub const ASTREA_LAMP_ABSORB_DEPTH: f64 = 0.60` and `pub const ASTREA_LAMP_SINK_THICKNESS: f64 = 1.0`.
- Add `pub fn lamp_sink_rect(anchor_rect: PresentationRect, portal_rect: PresentationRect, direction: LampDirection, absorb_depth: f64, sink_thickness: f64) -> Option<PresentationRect>`.

- [ ] **Step 1: Write sink tests first.** Cover wide/portrait/square sources, tiny valid anchors, negative coordinates, and all four directions. Assert finite positive dimensions, full containment in `anchor_rect`, cross-axis extent equal to the corresponding portal extent, bounded main-axis thickness, expected depth from the Dock-facing edge, and rotational equivalence.
- [ ] **Step 2: Run those tests to establish the missing `sink_rect` API failure.** Keep the Task 1 RED test failing for the old portal endpoint as well.
- [ ] **Step 3: Implement the one-rule sink construction.** Clamp depth to `[0,1]`; clamp thickness to a positive value no larger than the anchor movement-axis extent. Place the sink center at `near_edge + sign * anchor_extent * depth`, where Bottom/Right use `near_edge +` from the anchor’s low axis bound and Top/Left use `near_edge -` from the anchor’s high axis bound. Center only the cross extent through `portal_rect`; build a thin axis-aligned rect and clamp it inside the anchor without changing its thickness.
- [ ] **Step 4: Construct the sink once in `LifecycleVisualGroup::from_bounds`.** Continue computing `portal_rect` first; derive and store `sink_rect` from the same frozen anchor, portal, and direction. Extend `valid_visual_group` to require a finite positive contained sink.
- [ ] **Step 5: Extend deterministic identity.** Add all sink rectangle components to `lifecycle_snapshot_signature` and to `GlesSceneRenderer::lamp_geometry_key` in the renderer task; update equality/snapshot fixtures that construct or compare full visual groups.
- [ ] **Step 6: Re-run the geometry tests.** The old Task 1 assertion should still be RED until the warp target changes; sink-depth tests should pass once `from_bounds` stores the sink.

### Task 3: Replace only the spatial funnel profile with canonical cubic rails

**Files:**
- Modify: `src/window_lifecycle_animation.rs` spatial helper and tests.

**Interfaces:**
- Add `pub fn cubic_funnel_profile(t: f64, shape_factor: f64) -> f64`.
- Delete `ASTREA_LAMP_SPATIAL_EXPONENT_MIN`, `ASTREA_LAMP_SPATIAL_EXPONENT_MAX`, and `spatial_funnel_exponent`; these are obsolete sequential-v2 spatial-power semantics, not timing channels.
- Retain `ASTREA_LAMP_STRETCH_POWER` and the existing row-delay equation unchanged.

- [ ] **Step 1: Add cubic rail property tests.** For shape minimum, middle, and maximum, sample `t = 0.00, 0.10, 0.25, 0.50, 0.75, 0.90, 1.00`; assert finite values, `[0,1]` range, endpoint values, monotonicity, and the representative Apple-like profile stays wider through the mid-funnel than linear (`f(0.5) < 0.5`, meaning less early contraction at the middle than a linear spatial weight). Add golden representative values so the CPU implementation cannot drift.
- [ ] **Step 2: Run the rail tests before implementation and verify the intended API/property failure.** Do not add a second shader-equation implementation to make this test pass.
- [ ] **Step 3: Implement the cubic profile.** Clamp finite `t` to `[0,1]`; normalize finite `shape_factor` from `0.20..0.80` to `[0,1]`; derive `C1 = lerp(0.14, 0.04, s)` and `C2 = lerp(0.55, 0.24, s)`; compute `u=1-t` and `3u²tC1 + 3ut²C2 + t³`; clamp the finite result to `[0,1]`. Centralize and document the four rail-control bounds.
- [ ] **Step 4: Replace only `pow(m, spatial_funnel_exponent(shape_factor))`.** Keep the existing `early_contraction`, row translation, retreat displacement, source geometry domain, and cross-completion composition unchanged. Do not alter `lamp_motion_channels` or any v2.2 temporal constants.
- [ ] **Step 5: Re-run v2.2 aggregate-velocity tests unchanged.** They must remain green and must continue to demonstrate no full-object internal stop; do not adjust thresholds or sampling solely for the cubic rail.

### Task 4: Change CPU warp endpoint from portal to sink

**Files:**
- Modify: `src/window_lifecycle_animation.rs` warp helpers and existing Lamp geometry tests.

**Interfaces:**
- `lamp_warp_point_directional` and `lamp_warp_point` derive a sink from the source/anchor geometry; `lamp_warp_visual_point` consumes frozen `visual_group.sink_rect`.
- `lamp_warp_point_directional_to_target` continues to accept one explicit target rect, now the sink rect, so CPU tests and renderer semantics share one target authority.

- [ ] **Step 1: Update endpoint and reversal tests to target `sink_rect`.** At `p=0`, assert exact source; at `p=1`, map each normalized point into sink exactly; for restore and arbitrary reversal, compare the same pure function sampled at the corresponding raw progress and assert no positional jump.
- [ ] **Step 2: Add spatial invariants.** Preserve finite intermediate coordinates, near-edge-before-trailing behavior, eventual trailing-edge sink arrival, cross-axis cubic profile behavior, four-direction rotation equivalence, tiny/large/close/negative-coordinate fixtures, no-retreat behavior, and bounded retreat footprint. Keep `lamp_footprint` based on the full anchor plus retreat, never sink-only.
- [ ] **Step 3: Implement only the target replacement.** Use sink for both target axis and target cross coordinates; do not add a terminal temporal phase. The frozen portal remains available as the sink’s cross-aperture authority and CPU lifecycle geometry.
- [ ] **Step 4: Run Lamp and lifecycle focused tests.** The Task 1 RED test must become GREEN, and all existing v2.2/v2.3 endpoint, reversal, damage, and lifecycle tests must pass.

### Task 5: Update the spatial-only GLES contract

**Files:**
- Modify: `src/egl_renderer/program.rs`.

**Interfaces:**
- Replace `u_portal_rect` with `u_sink_rect`; remove portal as a shader terminal target.
- Replace GLSL power-profile constants/function with the CPU-matched cubic rail constants/function.

- [ ] **Step 1: Update shader contract tests before changing shader source.** Assert the contract requires `u_sink_rect`, rejects `u_portal_rect`, rejects old spatial exponent names, includes the cubic Bézier expression and the exact four control-bound literals, uses source visual geometry for source deformation, and keeps all temporal channels as uniforms without `smoothstep`/`in_out_cubic` timing code.
- [ ] **Step 2: Run the shader contract test to verify RED against v2.3 source.** Its failure must identify the old portal target or old power-profile source, not be hidden by a broad string assertion.
- [ ] **Step 3: Implement the shader target and rail replacement.** Upload/use `u_sink_rect` for endpoint and target axis/cross. Implement the same clamped shape normalization, `C1/C2` interpolation, and cubic scalar expression as CPU. Keep the existing visual source mapping through `u_canonical_visual_rect` and `u_source_visual_rect`, and keep the shader spatial-only.
- [ ] **Step 4: Add source-level CPU/GLSL parity evidence.** Assert representative CPU golden values and GLSL source formula/control constants; do not create an independent Rust timing or sink-depth implementation. Preserve exact p0 source and p1 sink endpoint assertions in the CPU-backed shader contract fixture.

### Task 6: Update renderer uniform and topology identity plumbing

**Files:**
- Modify: `src/egl_renderer.rs` only in Lamp uniform lookup/upload, Lamp geometry identity, and focused Lamp renderer tests.

**Interfaces:**
- Rename `LampUniformLocations::portal_rect` to `sink_rect` and look up `u_sink_rect`.
- Upload the frozen `sample.visual_group.sink_rect`.
- Include sink rectangle bits in `lamp_geometry_key`; progress-only changes still update uniforms without rebuilding topology.

- [ ] **Step 1: Update renderer tests to require the new uniform and reject the removed one.** Keep assertions for CPU-owned temporal channels, bounded mesh, source visual domain, and static topology.
- [ ] **Step 2: Run focused renderer tests to verify RED against old plumbing.** Confirm failure is the missing sink uniform/old portal authority.
- [ ] **Step 3: Apply the minimal plumbing changes.** Do not edit concurrent Effects/native-input code in `src/egl_renderer.rs`; preserve v2.3 scene-cache partition identity and Dock ordering exactly.
- [ ] **Step 4: Run focused Lamp, lifecycle, and renderer tests together.** Verify no stale portal target remains in production paths and no topology rebuild occurs on progress-only sampling.

### Task 7: Full verification, dirty-tree audit, and commit

**Files:**
- Modify only the three Lamp files and this plan file; no Eclipse production change is expected.

- [ ] **Step 1: Format only the three changed production Rust files as needed, then run `rtk cargo fmt --all -- --check`.** Do not format unrelated dirty files in a way that changes their content.
- [ ] **Step 2: Run focused verification:**
  ```bash
  rtk cargo test --locked lamp
  rtk cargo test --locked lifecycle_animation
  rtk cargo test --locked egl_renderer
  ```
  Record any unrelated pre-existing Effects failures separately.
- [ ] **Step 3: Run repository verification:**
  ```bash
  rtk cargo check --locked --all-targets
  rtk cargo clippy --locked --all-targets -- -D warnings
  rtk cargo test --locked
  rtk git diff --check
  ```
  Do not claim failures in concurrent pacing/compositor/Effects work as Lamp regressions.
- [ ] **Step 4: Run the source-layout checker if present and report its exact existing oversized-file failures.** Do not refactor source layout for this task.
- [ ] **Step 5: Re-run `rtk git status --short`, `rtk git diff --stat`, and `rtk git diff --cached --stat` before staging.** Stage only the plan and v2.4 Lamp changes; leave every unrelated dirty path unstaged.
- [ ] **Step 6: Commit with a focused message such as `feat: add Lamp cubic rails and icon sink`.** Verify the commit contains no unrelated paths.
- [ ] **Step 7: Report that native visual acceptance was not performed unless an actual 1920x1080@165 Hz Blur-disabled run was executed.** Explicitly retain the known intermittent micro-stutter, Blur, and live-subsurface snapshot gaps as independent issues.

## Self-review against the specification

- The plan covers the old v2.3 aspect-dependent terminal-depth RED test before production changes, then sink independence, direction symmetry, cubic rails, complete warp invariants, and renderer contract updates.
- CPU owns both temporal channels and authoritative spatial geometry; GLSL receives channels/rectangles and contains no timing/easing or aspect-fit calculation.
- The soft-start translation ramp, temporal overlap, retreat raw-progress semantics, row delay, duration, opacity, mesh, lifecycle, Dock ordering, Effects, Blur, and pacing are explicitly frozen.
- `portal_rect` remains a CPU aperture authority; `sink_rect` becomes the only shader terminal target. Obsolete spatial exponent constants/function and obsolete portal uniform are explicitly deleted, while v2.2 row `STRETCH_POWER` remains.
- Exact p0 source, p1 sink, reversal continuity, frozen identity/signature, damage authority, negative coordinates, and all four directions are assigned concrete tests.
- No unlisted production file is needed. If the compiler or a failing integration test proves otherwise, the dependency must be explained before editing it.
- The verification section distinguishes focused results, unrelated pre-existing failures, source-layout failures, dirty-tree preservation, and native acceptance.
