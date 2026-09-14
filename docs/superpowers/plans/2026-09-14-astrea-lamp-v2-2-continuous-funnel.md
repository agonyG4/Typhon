# Astrea Lamp v2.2 Continuous Funnel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the sequential Lamp visual stage model with a continuous overlapping funnel model while preserving all v2.1 lifecycle, geometry-domain, renderer, mesh, and settlement guarantees.

**Architecture:** `WindowLifecycleAnimator` remains the linear raw-progress owner. `src/window_lifecycle_animation.rs` becomes the sole CPU authority for four temporal channels and the direction-normalized spatial warp. `src/egl_renderer.rs` uploads those CPU channels, while `src/egl_renderer/program.rs` performs only the matching spatial deformation over the complete visual rectangle.

**Tech Stack:** Rust 2024, Cargo tests, existing GLES/GLSL renderer, `PresentationRect`, `LifecycleVisualGroup`, deterministic unit tests, `rtk` command wrapper.

## Global Constraints

- CPU remains the sole authority for temporal channels.
- GLSL remains spatial-only; do not duplicate timing/easing equations in the shader.
- The first RED regression must prove actual aggregate mesh velocity collapse around the current internal stage boundary, not merely scalar channel behavior.
- Preserve exact `p=0` source geometry and exact `p=1` anchor mapping.
- Preserve exact positional continuity across arbitrary mid-flight reversal.
- `retreat_progress` is a pure bounded function of raw progress and never a sequential pre-animation phase.
- Contraction and translation overlap; no hidden stage machine survives under new names.
- Do not change mesh density, the 280 ms base duration, opacity policy, lifecycle ownership, settlement, SSD freezing, Effects, Blur, KMS, pacing, or the known subsurface-snapshot ownership gap.
- Preserve all existing dirty post-v2.1 Effects/native-input work exactly.
- Do not modify files outside `src/window_lifecycle_animation.rs`, `src/egl_renderer.rs`, and `src/egl_renderer/program.rs` unless a concrete compile/test dependency is documented before editing.
- Keep all builds and tests in `/home/agony/GitHub/Typhon` to avoid creating an additional SSD build tree.

## File map

- Modify `src/window_lifecycle_animation.rs`: remove sequential channel semantics, add continuous channels and pure spatial helpers, and update/add lifecycle geometry tests. Keep tests in this existing module; do not perform source-layout refactoring.
- Modify `src/egl_renderer.rs`: replace old Lamp uniform locations and uploads with CPU-computed continuous channels. Preserve mesh topology caching and all unrelated Effects/native-output code, including any concurrent dirty changes.
- Modify `src/egl_renderer/program.rs`: replace stage uniforms and sequential GLSL deformation with the spatial funnel equations; retain complete visual-domain mapping and add renderer/shader contract coverage.
- No other production files are planned. `WindowLifecycleAnimator`, lifecycle ownership, and `LifecycleVisualGroup` APIs remain unchanged.

## Task 1: Establish the clean boundary and baseline

**Files:** None.

- [ ] Run `rtk git status --short`.
- [ ] Run `rtk git log --oneline -12`.
- [ ] Run `rtk git diff --stat`.
- [ ] Record every dirty path, especially `src/effects/render_graph.rs` and native-input files. Do not stage, overwrite, reset, checkout, stash, clean, or revert any of them.
- [ ] Run the current Lamp baseline with `rtk cargo test --locked lamp` and record its result before adding tests.
- [ ] Confirm the codebase-memory project remains `home-agony-GitHub-Typhon`, generation is current, and coverage for the three planned paths has no recorded issue. Treat the graph as best-effort and use direct source reads for exact formulas.

## Task 2: TDD Gate 1 — prove the old aggregate velocity collapse

**Files:**
- Modify: `src/window_lifecycle_animation.rs` test module only

**Interfaces:** Use the current `LampStageChannels`, `lamp_stage_channels`, and `lamp_warp_point_directional` only for this characterization test. No production code changes are allowed in this task.

- [ ] Add a test named `current_lamp_stage_model_collapses_aggregate_velocity_at_internal_boundaries`.
- [ ] Define the separated fixture directly in the test: source `(300, 200, 640, 480)`, anchor `(700, 900, 64, 64)`, direction `Bottom`, shape `0.60`, and points `[620,680]`, `[620,440]`, `[620,200]`, `[300,440]`, `[940,440]`.
- [ ] Add a test-local `aggregate_velocity(progress, half_step, bump_distance)` helper that calls the real `lamp_warp_point_directional` for every point at `progress-half_step` and `progress+half_step`, then returns the RMS displacement divided by `2*half_step`. This observes mesh motion rather than scalar channels.
- [ ] Use hand-derived old boundaries: no-bump Stretch-to-Squash `0.42 / 1.42 = 0.29577464788732394`; overlap Bump-to-Stretch `0.12 / 1.54 = 0.07792207792207792`.
- [ ] Assert each boundary has normal movement immediately before and after, but the boundary velocity is less than 10% of the smaller neighboring velocity. This expected assertion describes the current defect and must fail only because the old implementation reaches zero derivative at both stage endpoints.
- [ ] Run `rtk cargo test --locked current_lamp_stage_model_collapses_aggregate_velocity_at_internal_boundaries`.
- [ ] Confirm RED: the test fails with the intended velocity-collapse assertion, not a compile/setup error. Save the observed output for the final report.
- [ ] Do not leave this old-behavior assertion in the final suite. In Task 4, replace it with the v2.2 no-stop regression after the new implementation exists.

## Task 3: Implement CPU-owned continuous motion channels

**Files:**
- Modify: `src/window_lifecycle_animation.rs`
- Test: `src/window_lifecycle_animation.rs` test module

**Interfaces:** Add `pub struct LampMotionChannels { pub temporal_progress: f64, pub contraction_progress: f64, pub translation_progress: f64, pub retreat_progress: f64 }` and `pub fn lamp_motion_channels(progress: f64, bump_distance: f64) -> LampMotionChannels`. Keep `lamp_warp_point_directional` as the spatial consumer, changing its channel parameter to `LampMotionChannels`.

- [ ] Delete the old stage-only declarations and helpers: `LampStageChannels`, `bump_progress`, `stretch_progress`, `squash_progress`, `stage_progress`, `stage_fractions`, `ASTREA_LAMP_BUMP_WEIGHT`, `ASTREA_LAMP_STRETCH_WEIGHT`, `ASTREA_LAMP_SQUASH_WEIGHT`, `ASTREA_LAMP_NEAR_EDGE_BIAS`, `ASTREA_LAMP_NECK_BASE`, and `ASTREA_LAMP_NECK_RANGE`.
- [ ] Keep `ASTREA_LAMP_BASE_DURATION_MS = 280`, the shape-factor bounds, and endpoint opacity constant unchanged.
- [ ] Centralize and document the one global curve as `in_out_cubic(p)`, with `p = normalized_progress(progress)` and `temporal_progress = q = in_out_cubic(p)`.
- [ ] Add centralized constants `ASTREA_LAMP_CONTRACTION_END = 0.42`, `ASTREA_LAMP_TRANSLATION_START = 0.15`, `ASTREA_LAMP_TRANSLATION_BLEND = 0.25`, `ASTREA_LAMP_RETREAT_END = 0.30`, `ASTREA_LAMP_STRETCH_POWER = 2.0`, `ASTREA_LAMP_SPATIAL_EXPONENT_MIN = 2.0`, and `ASTREA_LAMP_SPATIAL_EXPONENT_MAX = 3.0`.
- [ ] Add one `smoothstep01(value)` helper implementing `x*x*(3-2*x)` after clamping `x` to `[0,1]`.
- [ ] Implement `translation_soft_start(q)` exactly as follows, with `start = 0.15`, `b = 0.25`, `t = clamp((q-start)/(1-start),0,1)`, `normalizer = 1-b/2`, and:

  ```text
  raw(t) = t²/(2b)       when t < b
           t - b/2       when t >= b
  translation = clamp(raw(t) / normalizer, 0, 1)
  ```

  At `t=b`, both branches equal `b/2` and both derivatives equal `1`; division by the same positive normalizer preserves C1 continuity. At `q=start`, the value and derivative are zero; at `q=1`, the value is one.
- [ ] Implement `lamp_motion_channels` as `q`, `smoothstep01(q / 0.42)`, `translation_soft_start(q)`, and `0` when `bump_distance <= f64::EPSILON`, otherwise `smoothstep01(p / 0.30)`. Clamp all outputs and treat nonfinite raw progress as zero through `normalized_progress`.
- [ ] Add tests for literal channel contracts: `q` equals the existing global cubic at representative values, contraction reaches one at q-space end `0.42`, translation starts at zero at q `0.15` and reaches one at q `1`, the two translation branches have matching finite-difference slopes around `q = 0.15 + 0.85*0.25`, contraction and translation overlap, retreat is zero for no overlap, and retreat is monotonic and bounded for overlap.
- [ ] Check channel finite-difference continuity around every required transition: translation start `q=0.15`, quadratic-to-linear join `q=0.15+0.85*0.25`, contraction end `q=0.42`, and retreat end `p=0.30`. Compare slopes on both sides with absolute tolerances scaled to the finite-difference step; the contraction and retreat endpoint slopes may tend to zero, but neither may create an aggregate visual stop because another channel is active.
- [ ] Run `rtk cargo test --locked lamp_motion_channels` and the relevant existing lifecycle tests. These tests may fail until Task 4 updates spatial consumers; do not change the constants to make tests pass.

## Task 4: Replace CPU sequential spatial warp with the continuous funnel

**Files:**
- Modify: `src/window_lifecycle_animation.rs`
- Test: `src/window_lifecycle_animation.rs` test module

**Interfaces:** `lamp_warp_visual_point` continues to accept raw `p` and a frozen `LifecycleVisualGroup`. `lamp_warp_point` and `lamp_warp_point_directional` remain the CPU authority used by tests and renderer reasoning.

- [ ] Change `lamp_warp_visual_point` and `lamp_warp_point` to call `lamp_motion_channels(raw_progress, bump_distance)` exactly once for their spatial sample. Preserve their exact `p<=0` identity and `p>=1` normalized anchor branches.
- [ ] In the directional warp, derive `m` and `c` from the source visual rectangle with the existing direction rotation: Bottom `(m=v,c=u)`, Top `(m=1-v,c=u)`, Right `(m=u,c=v)`, Left `(m=1-u,c=v)`.
- [ ] Derive the bounded spatial exponent from the frozen shape factor: clamp shape to `0.20..0.80`, normalize it to `[0,1]`, and use `2.0 + normalized_shape * (3.0-2.0)`. Compute `funnel_weight = pow(m, exponent)`.
- [ ] Compute `early_contraction = contraction_progress * funnel_weight`.
- [ ] Compute `stretch = 2.0 * contraction_progress * (1.0-m)` and `row_translation = pow(translation_progress, 1.0 + stretch)`. Explicitly return `1.0` when translation is exactly one so the endpoint is exact for every row.
- [ ] Compute `cross_completion = 1.0 - (1.0-early_contraction) * (1.0-row_translation)` and cross-axis `lerp(source_cross, target_cross, cross_completion)`.
- [ ] Compute bounded retreat distance as `min(max(bump_distance,0), movement_extent(source,direction)) * retreat_progress`. Offset only the source main-axis coordinate away from the target: Top/Left positive, Bottom/Right negative. Do not introduce a retreat stage or early return.
- [ ] Compute main-axis `lerp(retreated_source_axis, target_axis, row_translation)`. Keep all four directions as rotations of this one model and retain finite-coordinate guards.
- [ ] Replace the RED test from Task 2 with `lamp_continuous_funnel_has_no_internal_aggregate_stop`. Sample the complete no-overlap transition densely over `[0.08,0.92]`, compute aggregate RMS finite-difference velocity for the same five points, and assert every sample exceeds a small positive fraction of the maximum and that both old boundary neighborhoods have no full-object velocity valley. The test must allow individual trailing points to remain nearly stationary.
- [ ] Add tests for exact source identity at `p=0`, exact normalized anchor mapping at `p=1`, finite intermediate coordinates for tiny/huge geometry, exact same pure-function output when sampling the same arbitrary progress before and after reversal, rotational equivalence for all four directions, monotonic no-retreat main-axis movement, near-edge-before-trailing-edge motion, cross-axis early near-edge narrowing, and exact trailing-edge target arrival.
- [ ] Add tiny-window, huge-window, tiny-anchor, close-anchor, no-overlap, and overlap fixtures. Assert overlap retreat never exceeds `bump_distance` and moves away from the target; assert no-overlap `retreat_progress == 0` for the entire transition.
- [ ] Run `rtk cargo test --locked lamp_continuous_funnel` and `rtk cargo test --locked lifecycle_animation`.

## Task 5: Update GLSL and renderer uniform plumbing

**Files:**
- Modify: `src/egl_renderer.rs`
- Modify: `src/egl_renderer/program.rs`
- Test: `src/egl_renderer/program.rs` and existing EGL renderer tests

**Interfaces:** `draw_lamp_overlay` receives raw lifecycle samples as before, calls `lamp_motion_channels`, and uploads only CPU-produced channel values. `LAMP_VERTEX_SHADER` receives the global eased value and three spatial-channel values; it does not calculate time/easing.

- [ ] Update `LampUniformLocations`: retain `progress`, `shape_factor`, `bump_distance`, geometry, direction, opacity, and output uniforms; replace `bump_progress`, `stretch_progress`, and `squash_progress` with `contraction_progress`, `translation_progress`, and `retreat_progress`.
- [ ] Query `u_progress`, `u_contraction_progress`, `u_translation_progress`, and `u_retreat_progress`; remove all lookup strings for `u_bump_progress`, `u_stretch_progress`, and `u_squash_progress`.
- [ ] In `draw_lamp_overlay`, call `lamp_motion_channels(sample.progress, sample.visual_group.bump_distance)` and upload `channels.temporal_progress` as `u_progress`, then upload the three remaining fields. Do not put `in_out_cubic`, smoothstep, ramp, or retreat timing formulas in Rust outside the shared CPU helper or in GLSL.
- [ ] Keep the v2.1 complete visual mapping: `source_point = source_visual.xy + ((a_position-canonical_visual.xy)/canonical_visual.zw)*source_visual.zw`, clamp group UV, and use `u_source_visual_rect` for every spatial domain calculation.
- [ ] Replace GLSL stage constants and equations with only spatial constants and the matching funnel equations: shape-derived exponent, `pow(m, exponent)`, early contraction, row translation, cross completion, and direction-aware bounded retreat. Use `u_progress` only as the already-eased endpoint/branch value; do not re-ease it.
- [ ] Ensure shader endpoint behavior is exact: at `u_progress <= 0`, use `source_point`; at `u_progress >= 1`, use anchor-normalized target; at intermediate values, use the spatial channels. Keep output-origin and UV behavior unchanged.
- [ ] Add/update renderer contract tests proving all old stage uniform names and old stage constants are absent, all new uniform names are present, visual-domain uniforms drive movement extent/axis/cross calculations, and source mapping remains complete-visual based.
- [ ] Add a CPU/renderer plumbing test or existing harness assertion that the uploaded temporal values are exactly the fields returned by `lamp_motion_channels`; do not add an independent Rust copy of shader equations.
- [ ] Preserve `lamp_geometry_key`, static topology, `lamp_geometry_dirty`, vertex limits, and progress-only behavior. Add/update a regression that changes only progress and confirms geometry key/topology identity remains unchanged while the temporal values change.
- [ ] Run `rtk cargo test --locked egl_renderer` and `rtk cargo test --locked lamp`.

## Task 6: Final invariant review and verification

**Files:** Only the three Lamp files above should be staged. Any other current or newly dirty file is unrelated and must remain untouched.

- [ ] Review the final diff for absence of `LampStageChannels`, `lamp_stage_channels`, old stage fields, old stage constants, old stage uniform fields, old stage GLSL uniforms, and hidden sequential timing branches.
- [ ] Review the final diff to confirm no changes to `WindowLifecycleAnimator`, lifecycle ownership/settlement, SSD freezing, Effects/Blur, KMS/pacing, mesh constants, duration, opacity, or the ignored subsurface gate.
- [ ] Run focused tests in order:

  ```bash
  rtk cargo test --locked lamp
  rtk cargo test --locked lifecycle_animation
  rtk cargo test --locked egl_renderer
  ```

- [ ] Run full verification in the repository directory:

  ```bash
  rtk cargo fmt --all -- --check
  rtk cargo check --locked --all-targets
  rtk cargo clippy --locked --all-targets -- -D warnings
  rtk cargo test --locked
  rtk git diff --check
  ```

- [ ] Run the source-layout checker and report its exact existing failures without modifying oversized modules.
- [ ] Review `rtk git diff --cached --name-only`; stage only `src/window_lifecycle_animation.rs`, `src/egl_renderer.rs`, and `src/egl_renderer/program.rs`.
- [ ] Commit the implementation with `rtk git commit -m "feat: add continuous Lamp funnel motion"`.
- [ ] After committing, run `rtk git status --short` and `rtk git show --stat --oneline HEAD` to prove unrelated dirty Effects/native-input work remains preserved and the implementation commit contains only the three planned files.
- [ ] Report that native 1920×1080@165 Hz acceptance was or was not performed; automated tests are not hardware visual acceptance.

## Plan self-review

- The plan contains the required RED aggregate-mesh regression before production changes.
- The soft-start normalization and C1 derivative equality are specified numerically and symbolically.
- CPU temporal authority and spatial-only GLSL ownership are explicit in Tasks 3 and 5.
- Exact endpoints, reversal continuity, overlap/retreat bounds, direction rotation, funnel ordering, and no internal aggregate stop are covered in Tasks 3–4.
- Legacy sequential fields/constants/uniforms are explicitly deleted in Tasks 3 and 5.
- No task changes mesh density, duration, opacity, lifecycle/settlement, Effects, Blur, KMS, pacing, or subsurface ownership.
- No production file outside the three Lamp files is required; any dependency discovered during verification must be reported before editing.
- Dirty post-v2.1 files are inspected and preserved before any edit or stage operation.
