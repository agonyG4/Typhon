# Per-Pass Effect Demand v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add bounded intra-instance pass demand so only graph texture regions that can influence the selected final repair execute, while preserving the existing instance dependency closure and rendering behavior.

**Architecture:** Extend `EffectExecutionDemand` with pass output regions and pass-level diagnostics. Derive those regions in `src/effects/render_graph.rs` using one reverse traversal over compiled passes, a validated texture-producer table, explicit stage rules, and a conservative per-instance fallback. Make the GLES executor consume the plan for selection, scissors, replay capture clears, surface consumers, physical capture metrics, and trace output.

**Tech Stack:** Rust, Cargo, GLES 3 via `glow`, `EffectRegion`, existing typed `CompiledFrameGraph`, `rtk` command wrapper.

## Global Constraints

- Preserve `plan_effect_execution_demand()` instance traversal and its direct/transitive/fragmented dependency behavior exactly.
- Do not add a global pass-demand fixpoint, `while changed`, repeated dependency propagation, or structural `EffectRegion` convergence test.
- Do not change graph texture domains, aggregate `EffectFootprint`, blur radius, pass count, scale, shaders, filtering, color, alpha, resource budget, or direct framebuffer blit behavior.
- Use the existing `MAX_EFFECT_REGION_RECTS` bounded `EffectRegion`; unrepresentable pass regions use conservative fallback.
- Compile and test in `/home/agony/GitHub/Typhon` so Cargo reuses the existing `target/` directory.
- Use `rtk` for Git, Cargo, and test commands.
- Do not stage or modify unrelated working-tree files; no subagents.

---

### Task 1: Build the pure pass-demand planner

**Files:**
- Modify: `src/effects/render_graph.rs:200-502` for pass-demand data, producer validation, mapping, and planner integration.
- Test: `src/effects/render_graph.rs` existing `#[cfg(test)]` module, adding renderer-independent fixtures beside the existing demand-closure tests.

**Interfaces:**
- Consumes: `CompiledFrameGraph`, `CompiledRenderPass`, `GraphTexturePlan`, `EffectRegion`, existing instance-level `EffectExecutionDemand`.
- Produces: `EffectPassExecutionDemand { id: GraphPassId, output_region: EffectRegion }`, `EffectExecutionDemand::pass_output_region(GraphPassId)`, pass-level `EffectDemandPlanStats` fields, and a planned pass entry for every pass belonging to a selected instance, including empty entries.

- [ ] **Step 1: Add the first failing planner test.**

Add a six-pass built-in blur fixture that compiles `Capture -> Downsample -> Downsample -> Upsample -> Upsample -> Composite` over a large domain, plans a small final repair, and asserts the composite is smaller than the capture domain while every required upstream pass has non-empty demand. Call `demand.pass_output_region(pass.id)` so the test names the production API that does not yet exist.

- [ ] **Step 2: Run the focused test and verify the expected red failure.**

Run:

```bash
rtk cargo test --locked effects::render_graph::tests::small_blur_repair_plans_partial_pass_demand
```

Expected: compilation failure because `EffectPassExecutionDemand` or `pass_output_region` is not defined. Fix only test-fixture typos if necessary; do not add production implementation before this failure is observed.

- [ ] **Step 3: Add the demand data model without changing instance traversal.**

Add:

```rust
#[derive(Clone, Debug, PartialEq)]
pub struct EffectPassExecutionDemand {
    pub id: GraphPassId,
    pub output_region: EffectRegion,
}
```

Extend `EffectExecutionDemand` with bounded pass entries and private per-instance fallback IDs. Add `pass_output_region`, a crate-visible `has_pass_plan`, and a crate-visible per-instance conservative query. Extend `EffectDemandPlanStats` with `pass_count_selected`, `partial_pass_count`, `full_domain_pass_count`, `pass_dependency_propagations`, `max_pass_region_rect_count`, and `pass_conservative_fallbacks`. Update all existing struct literals to use the new fields explicitly or `..Default::default()`.

- [ ] **Step 4: Implement the pure region mapping helper.**

Implement a bounded helper in `render_graph.rs` that maps a demanded output region to an input logical region using the actual `GraphTexturePlan` domains and physical dimensions. Convert each output rectangle to an outward physical interval, map normalized coordinates into the input texture, add one physical texel for linear filtering, add the requested per-axis sampling radius, round the logical bounds outward, clip with `input.domain`, and union through `EffectRegion::push`. Return a conservative failure signal if dimensions, radius, arithmetic, or region representation is invalid. Use normalized-domain mapping for Kawase/local stages and the explicit logical-domain mapping for `NormalizeInput`/composite-style passes.

- [ ] **Step 5: Implement producer metadata validation and one reverse traversal.**

Build a producer table for every captured/intermediate texture. Require exactly one producer and require its pass position to precede each consumer. Associate missing, duplicate, unknown, and non-topological producer edges with the selected instance that touches them. Seed the final `Composite`/`OutputPostProcess` pass with `pass.damage.union(instance.output_region)` clipped to its output domain. Traverse `graph.passes` once in reverse, propagate each input edge at most once, and apply these rules:

```rust
SceneCapture | SurfaceCapture => no input propagation
Composite | OutputPostProcess | NormalizeInput => pointwise mapped input demand
Fragment | fused local stages => unchanged mapped demand
Blend | Mask => propagate to every input
CustomFragment => mapped demand expanded by declared sample radius
DualKawaseDownsample | DualKawaseUpsample => mapped demand expanded by pass.blur_radius plus filtering support
```

On a selected-instance producer or region failure, set every pass in that instance to its output texture domain and increment `pass_conservative_fallbacks`. On existing graph-wide metadata failure, preserve `conservative_full` and fill all pass demands with full domains. Never alter the instance dependency counters or traversal.

- [ ] **Step 6: Run the first test green and then add the remaining pure planner tests.**

Run the single blur test again, then add deterministic tests for full repair, zero-footprint local stages, declared custom footprint, blend and mask fan-in, fragmented bounded regions, texture-domain clipping, odd dimensions, translated domains, conservative `EffectRegion`, malformed producer metadata, and empty-demand unreachable branches. Assert pass demand is clipped, non-empty required producer demand is larger than the final repair for blur, full repair reaches all domains, and `dependency_propagations` remains equal to the existing instance edge count.

Run:

```bash
rtk cargo test --locked effects::render_graph
```

- [ ] **Step 7: Commit the pure planner slice.**

```bash
rtk git add src/effects/render_graph.rs
rtk git commit -m "feat: plan effect demand per render pass"
```

### Task 2: Consume pass demand in executor selection and scissors

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs:387-548,707-1061`.
- Test: `src/egl_renderer/effects/executor.rs` coordinate and executor test modules.

**Interfaces:**
- Consumes: `EffectExecutionDemand::has_pass_plan`, `pass_output_region`, and per-instance conservative fallback.
- Produces: pass selection that excludes empty demand, no resource acquisition for excluded outputs, precise `effective_pass_damage`, and unchanged legacy behavior for manually constructed demands without pass entries.

- [ ] **Step 1: Add failing selection and effective-damage tests.**

Create a graph fixture with one selected instance containing an unused internal branch and a final pass. Construct a pass-planned demand with an empty branch entry and a small final entry. Assert the branch is absent from `executed_passes` and `acquired_texture_ids`, while the final pass is present. Add a precise `effective_pass_damage` assertion and a per-instance conservative fallback assertion that expects the old full output domain.

- [ ] **Step 2: Run the executor tests red.**

Run:

```bash
rtk cargo test --locked egl_renderer::effects::executor::tests::empty_demand_pass_is_not_selected
```

Expected: failure because selection still checks only instance membership and `effective_pass_damage` still substitutes full output domains for non-final passes.

- [ ] **Step 3: Update selection with a compatibility branch.**

In `select_effect_execution`, retain the current selected-instance loop for demands created by `EffectExecutionDemand::new` with no pass plan. For planned demands, require `demand.pass_output_region(pass.id)` to be present and non-empty before pushing the pass or its resources. Keep deterministic graph order and leave `executed_instances` ordered by first executed pass.

- [ ] **Step 4: Replace only the precise damage substitution.**

Change `effective_pass_damage` so a planned pass returns its stored output region, including the final seed’s existing `pass.damage` contribution. If the demand has no pass plan, retain the current compatibility behavior. If the demand is global or per-instance conservative, return the pass output texture domain. Keep the function renderer-side as a consumer; do not add another graph planner there.

- [ ] **Step 5: Run executor tests green and add coverage for selection/resource safety.**

Add tests for precise damage, global conservative full-domain behavior, per-instance fallback, empty pass omission, output resource omission, full repair selection, legacy `EffectExecutionDemand::new`, unchanged direct error handling, and GPU timing call eligibility through only selected passes. Run:

```bash
rtk cargo test --locked egl_renderer::effects
```

- [ ] **Step 6: Commit executor selection integration.**

```bash
rtk git add src/egl_renderer/effects/executor.rs
rtk git commit -m "feat: execute only demanded effect passes"
```

### Task 3: Make replay capture partial and metrics unit-safe

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs:2157-2269` for replay clear planning and direct-capture conservative execution damage.
- Modify: `src/egl_renderer.rs:417-419,2061-2067` for physical executed capture metrics.
- Test: `src/egl_renderer/effects/executor.rs` coordinate/executor tests and `src/egl_renderer/effects/gpu_timing.rs` metric fixtures if their literals require updates.

**Interfaces:**
- Consumes: precise capture pass `EffectRegion`, `GraphTexturePlan`, existing `effect_damage_to_texture_rects`, and direct-capture detection.
- Produces: testable physical capture clear rectangles, replay-only partial clear/draw behavior, full direct framebuffer capture behavior, and `effect_capture_pixels_executed` as physical demanded target area.

- [ ] **Step 1: Add failing pure capture-plan tests.**

Add a test with a 100x100 logical capture domain backed by a 50x50 texture and a small demanded rectangle. Assert the clear plan contains only the mapped physical target rectangle. Add a fragmented demand assertion that preserves holes and a full-domain assertion for a conservative region. Add a source-over preparation test that verifies the replay path’s clear plan is non-empty before any capture rectangles are submitted.

- [ ] **Step 2: Run the capture-plan tests red.**

Run:

```bash
rtk cargo test --locked egl_renderer::effects::executor::tests::replay_capture_clear_plan
```

Expected: failure because the pure clear-plan helper does not yet exist and replay capture still performs an unconditional full clear.

- [ ] **Step 3: Implement bounded physical clear rectangles.**

Factor a helper that maps `execution_damage` through the non-output target plan using bottom-left storage coordinates. In ordinary replay capture, enable scissoring, clear each mapped target rectangle to transparent black, disable scissoring, then replay the same logical demanded rectangles through `draw_capture_commands_for_regions`. Keep pooled pixels outside demand untouched and never depend on their previous contents.

- [ ] **Step 4: Preserve conservative direct framebuffer capture.**

Detect lifecycle/checkpoint direct capture in the execution loop and use the full output texture domain for that pass’s execution damage and physical metrics. Keep `capture_output_region_to_graph_texture` and its full-domain blit unchanged. Add trace-visible conservative classification for these passes.

- [ ] **Step 5: Make capture executed metrics physical and explicit.**

Rename the internal execution statistic to `capture_execution_pixels` or equivalent. For replay capture, sum the physical demanded target rectangles; for direct capture, sum the full target texture area. Continue recording the graph metric/`effect_capture_pixels` as allocated physical capture texture pixels. Do not change the GPU profiler query lifecycle or its effect-space `effect_region_pixels` metadata.

- [ ] **Step 6: Run capture, metrics, and timing tests green.**

Run:

```bash
rtk cargo test --locked egl_renderer::effects::executor
rtk cargo test --locked egl_renderer::effects::gpu_timing
rtk cargo test --locked egl_renderer
```

Update exact expected metric strings only to reflect the explicit physical/effect-space distinction, never by weakening assertions.

- [ ] **Step 7: Commit capture and metrics integration.**

```bash
rtk git add src/egl_renderer/effects/executor.rs src/egl_renderer.rs
rtk git commit -m "feat: partially clear demanded effect captures"
```

### Task 4: Wire surface consumers, diagnostics, and documentation

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs:455-548` so capture consumers use precise capture demand and checkpoint/direct capture remains conservative.
- Modify: `src/egl_renderer/effects/trace.rs:44-287,523-541` for bounded pass-demand fields and direct-capture fallback diagnostics.
- Modify: `src/egl_renderer.rs:1931-1940,2173-2195` to pass the actual planner stats into trace summaries.
- Modify: `docs/EFFECTS.md:23-29,141-179` to document pass demand and pixel units.
- Modify: `docs/EFFECTS_QUALIFICATION.md:72-87,131-137` to document pass-demand evidence and metric units.
- Test: `src/egl_renderer/effects/executor.rs` surface-consumer tests and `src/egl_renderer/effects/trace.rs` trace tests.

**Interfaces:**
- Consumes: planned pass regions and direct-capture classification.
- Produces: bounded diagnostics proving partial/full/fallback pass behavior and precise ordinary capture consumer regions without changing synchronization.

- [ ] **Step 1: Add failing consumer and trace assertions.**

Add a surface-consumer fixture where the capture demand is smaller than the capture texture domain and assert only surfaces intersecting the precise capture rectangles are consumers. Add a trace test that expects `pass_count_selected`, `partial_pass_count`, `full_domain_pass_count`, `pass_dependency_propagations`, `max_pass_region_rect_count`, and `pass_conservative_fallbacks` fields.

- [ ] **Step 2: Run the diagnostics tests red.**

Run:

```bash
rtk cargo test --locked egl_renderer::effects::trace
rtk cargo test --locked egl_renderer::effects::executor::tests::surface_consumer_plan_uses_precise_capture_demand
```

Expected: failure because the new fields are not emitted and consumer planning still uses the old broad damage semantics.

- [ ] **Step 3: Integrate precise capture demand into surface consumers.**

Use the planned `effective_pass_damage` for ordinary replay capture. For passes with checkpoint dependencies, use the full capture texture domain in this planner because the direct framebuffer path remains conservative. Preserve cursor/lifecycle/resource synchronization and the existing command range calculations.

- [ ] **Step 4: Extend trace summaries without default verbose logging.**

Format the new bounded stats in the existing `effect_demand_plan` frame boundary. Add a pass summary boolean or stable field indicating direct framebuffer capture is conservative. Keep pass boundary logging opt-in through `TYPHON_EFFECT_EXEC_TRACE` and do not add per-pass logging by default.

- [ ] **Step 5: Update docs and run focused green tests.**

Document the two-layer architecture, Kawase mapping safety, partial replay clears, direct-capture fallback, and the distinction between allocated physical capture pixels, executed physical capture pixels, and GPU timing effect-space area. Run:

```bash
rtk cargo test --locked egl_renderer::effects
rtk cargo test --locked effects
```

- [ ] **Step 6: Commit diagnostics and docs.**

```bash
rtk git add src/effects/render_graph.rs src/egl_renderer.rs src/egl_renderer/effects/executor.rs src/egl_renderer/effects/trace.rs docs/EFFECTS.md docs/EFFECTS_QUALIFICATION.md
rtk git commit -m "docs: expose per-pass effect demand diagnostics"
```

### Task 5: Full verification and scoped handoff

**Files:**
- Inspect only: final Git diff and status; no unrelated file edits.

- [ ] **Step 1: Check the final diff for forbidden structures and scope.**

Run:

```bash
rtk git diff --check
rtk git diff --stat
rtk git diff -- src/effects/render_graph.rs src/egl_renderer/effects/executor.rs src/egl_renderer/effects/trace.rs src/egl_renderer.rs docs/EFFECTS.md docs/EFFECTS_QUALIFICATION.md
rtk rg -n "while changed|repeat until stable|full_output_texture_domain|EffectRegion.*==|dependency_propagations" src/effects/render_graph.rs src/egl_renderer/effects/executor.rs
```

Confirm no precise ordinary blur pass is replaced with a full domain, no new convergence loop exists, and only task files are staged/committed.

- [ ] **Step 2: Run the required verification commands from the current checkout.**

```bash
rtk cargo fmt --all -- --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked effects::render_graph
rtk cargo test --locked egl_renderer::effects
rtk cargo test --locked egl_renderer
rtk cargo test --locked effects
rtk cargo test --locked
rtk git diff --check
```

Record exact exit status and failures. If unrelated workspace failures occur, prove their scope from the output and report focused changed-module results separately.

- [ ] **Step 3: Check native qualification availability.**

Inspect only the documented native prerequisites. If a real controlling TTY/DRM session exists, run the requested `TYPHON_EFFECT_GPU_TIMING=1` scenario and compare capture/downsample/upsample execution demand, dropped spans, disjoint invalidation, and visual checks. Otherwise report native qualification deferred using the existing environment limitation and do not claim a performance win.

- [ ] **Step 4: Final status and handoff.**

Run:

```bash
rtk git status --short --branch
rtk git log --oneline -8
```

Report the root cause, planner and fallback architecture, exact sampling expansion, capture clear behavior, diagnostics, tests, verification results, native timing availability, and any aggregate-footprint/shader-support discrepancy left for follow-up.
