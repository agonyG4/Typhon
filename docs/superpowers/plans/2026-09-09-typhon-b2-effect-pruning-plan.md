# Typhon B2 Consumer-Driven Effect Graph Pruning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Execute only effect instances consumed by the final buffer-age repair region and their transitive backdrop dependencies, while preserving logical damage history and conservative fallback behavior.

**Architecture:** Keep `compile_frame_execution_plan` as the stage-1 validator and logical-damage compiler, but attach bounded per-instance output/capture/dependency metadata to the compiled graph. After `PartialRepaintPlanner::plan`, derive a separate `EffectExecutionDemand` from `repair_damage`; the executor skips dead instance passes before texture realization and widens repair-only live passes to conservative bounded domains. Dependency-only output writes are unioned into actual repaint/swap coverage without changing `render_damage`.

**Tech Stack:** Rust, Cargo, existing GLES renderer, `EffectRegion`, `PartialRepaintPlanner`, existing render-graph metadata and unit-test fixtures.

## Global Constraints

- Work on the current Typhon checkout and reuse the existing Cargo target/build directory.
- Do not create another checkout, target directory, benchmark build tree, or alternate Cargo target.
- Do not use subagents.
- Use `rtk` for repository search, file reads, Cargo commands, and Git commands where applicable.
- Preserve `MAX_PARTIAL_REPAINT_RECTS`, the 75% repair-area fallback, buffer-age history depth, effect quality, and all A1/A2/B1/B3 behavior.
- Do not implement persistent blur caching, capture narrowing, C1 upload deferral, C2 graph caching, C3 resource-policy changes, or a second occlusion/continuous-dirty system.
- Pruning must fail toward overdraw: full/uncertain repair, unknown metadata, unsupported topology, or conservative-region ambiguity executes the complete visible effect graph.
- Commit focused changes in the current Git repository; leave unrelated user edits unstaged.

---

### Task 1: Add repair-aware graph metadata and pure liveness planning

**Files:**
- Modify: `src/effects/render_graph.rs:123-158,431-522,605-892`
- Modify: `src/effects/damage.rs` only if a small shared region helper is required; do not duplicate footprint math
- Test: `src/effects/render_graph.rs` existing `#[cfg(test)]` module

**Interfaces:**
- Produces `CompiledEffectInstance { id, output_influence_region, capture_region, dependencies }` stored in `CompiledFrameGraph.instances`.
- Produces `EffectExecutionDemand { instances: Vec<EffectInstanceExecutionDemand>, execution_region: EffectRegion }`.
- Produces `plan_effect_execution_demand(graph, repair_region, conservative_full) -> EffectExecutionDemand`.
- `EffectInstanceExecutionDemand` contains an `EffectInstanceId` and the output region that its final composite must establish.

- [ ] **Step 1: Write the failing unrelated-effect test**

Add a render-graph test using the existing `blur_scene` fixture plus a second separated instance. Compile with logical source damage far from both effects, then call the wished-for planner with a partial repair region intersecting only the first instance. Assert the demand contains only the first instance and that its execution region is the intersection of repair and output influence. Before adding the planner, the test must fail because the API is absent.

```rust
let demand = plan_effect_execution_demand(
    &graph,
    &EffectRegion::from_rect(EffectRect::new(100, 80, 320, 180).unwrap()),
    false,
);
assert_eq!(demand.instances.len(), 1);
assert_eq!(demand.instances[0].id, first_id);
assert!(!demand.execution_region.is_empty());
```

- [ ] **Step 2: Run the test to verify the expected RED result**

Run: `rtk cargo test --locked --bin oblivion-one effects::render_graph::tests::unrelated_repair_prunes_separated_effect`

Expected: compile failure identifying the missing execution-demand planner, not a fixture or assertion error.

- [ ] **Step 3: Add graph metadata without changing logical compile behavior**

Extend `CompiledFrameGraph` with a bounded `instances` vector. During the existing visible-instance loop, retain the output influence and capture regions returned by the single `plan_effect_damage` call. Track each checkpoint's originating `EffectInstanceId`; convert the existing `checkpoint_dependencies` pass IDs into `dependencies: Vec<EffectInstanceId>` for that instance. Keep missing-program and compile errors unchanged, and keep `graph.final_damage` based only on logical `output_damage`.

- [ ] **Step 4: Implement the minimal pure demand planner**

Implement `plan_effect_execution_demand` with a vector-based fixed-point pass:

```rust
if conservative_full || repair_region.bounding_rect().is_none() {
    return all_visible_instances_with_output_regions(graph);
}

for instance in &graph.instances {
    let direct = repair_region.intersect(&instance.output_influence_region);
    if !direct.is_empty() {
        mark_live(instance.id, direct);
    }
}
while changed {
    for live in current_live_instances {
        for dependency_id in live.dependencies {
            let required = graph.instance(dependency_id)
                .output_influence_region
                .intersect(&live.capture_region);
            mark_live_or_union(dependency_id, required);
        }
    }
}
```

Use existing semantic checkpoint dependencies rather than geometric overlap alone. Preserve `TargetContent` semantics because target captures do not populate checkpoint dependencies. Union every live instance output region into `execution_region` for later framebuffer/swap accounting. Unknown IDs or inconsistent metadata must return an all-visible conservative demand.

- [ ] **Step 5: Add and run pure RED-to-GREEN coverage**

Add tests for: partial repair of one of two separated effects, repair intersecting only a blur halo, clearly outside-halo pruning, full/uncertain repair keeping every visible instance, two-level and three-level dependency closure, overlapping backdrop retention, non-overlapping earlier backdrop pruning, and `TargetContent` not retaining an unrelated earlier effect. Run:

`rtk cargo test --locked --bin oblivion-one effects::render_graph::tests`

Expected: all render-graph tests pass, including the new liveness cases.

- [ ] **Step 6: Commit the pure planner slice**

Run `rtk cargo fmt --check` and `rtk git diff --check`, then commit only the graph files with:

`git add src/effects/render_graph.rs src/effects/damage.rs && git commit -m "perf(renderer): plan effect liveness from repaired consumers"`

### Task 2: Feed final repaint repair into execution demand and preserve swap accounting

**Files:**
- Modify: `src/egl_renderer.rs:120-140,760-870,909-923`
- Modify: `src/egl_renderer/damage.rs` only for a focused helper that converts/augments bounded effect repair coverage
- Test: `src/egl_renderer/damage_tests.rs` and `src/egl_renderer.rs` test seams if available

**Interfaces:**
- Consumes `plan_effect_execution_demand` and `EffectExecutionDemand` from Task 1.
- Produces a local `RepaintPlan` whose `repair_damage` includes dependency-only output writes, while `render_damage` remains exactly the logical candidate damage.
- Produces frame metrics for compiled versus executed instances/passes and executed capture/resource work.

- [ ] **Step 1: Write failing buffer-age demand tests**

Use `PartialRepaintPlanner` with real history. Add tests for age 2 and age 3 where current damage is far from a static blur but the accumulated historical repair intersects it. Assert the converted repair region produces a live effect demand even though the graph was compiled with empty logical effect output damage. Add a full-repair test for `BufferAge::Value(0)` and invalidated history that expects all visible instances live.

```rust
let plan = planner.plan(current_damage, BufferAge::Value(2));
let repair = effect_region_from_output_damage(&plan.repair_damage, WIDTH, HEIGHT);
let demand = plan_effect_execution_demand(&graph, &repair, plan.mode == RepaintMode::Full);
assert!(demand.contains(effect_id));
```

- [ ] **Step 2: Run the tests to verify the expected RED result**

Run: `rtk cargo test --locked --bin oblivion-one egl_renderer::damage_tests::age_2_repair_revives_unchanged_effect`

Expected: compile failure for the new demand integration or assertion failure showing the existing renderer has no repair-aware effect demand.

- [ ] **Step 3: Derive demand only after `PartialRepaintPlanner::plan`**

In `draw_scene_with_buffer_age`, keep the existing sequence that compiles all visible effects, merges only logical `graph.final_damage` into `output_damage`, and calls `self.repaint_planner.plan`. After that call and before the `RepaintMode::Skip` branch, convert `plan.repair_damage` to `EffectRegion`, set conservative mode from `plan.mode == RepaintMode::Full` or a conservative region, and call `plan_effect_execution_demand`. Never use `pass.damage.is_empty()` or current logical effect damage as the liveness authority.

- [ ] **Step 4: Account for dependency-only framebuffer writes conservatively**

Union `demand.execution_region` into a cloned `plan.repair_damage` using the existing bounded output-region conversion. Keep `plan.render_damage` unchanged. If the union becomes full/conservative or cannot be represented safely, replace the local plan with a full repaint plan and use all-visible demand. Pass the augmented plan to the executor and retain it in `EglSceneFrameCommit`, so `begin_effect_repaint`, EGL swap-with-damage, and explicit output repair agree on actual writes. Do not commit augmented repair into the presentation logical-damage journal; `commit_presented` continues to receive the logical transition selected by the existing presentation authority.

- [ ] **Step 5: Add transition and continuous-demand coverage**

Verify existing effect transition damage remains logical-only for appear, move, and removal. Add a renderer/planner seam test proving a continuous effect's existing dirty region makes its output consumer live when ordinary client damage is empty. Do not add a timer or second dirty mechanism.

- [ ] **Step 6: Run Task 2 focused tests and commit**

Run:

`rtk cargo test --locked --bin oblivion-one egl_renderer::damage_tests`

`rtk cargo fmt --check`

`rtk git diff --check`

Then commit only the repaint integration files:

`git add src/egl_renderer.rs src/egl_renderer/damage.rs src/egl_renderer/damage_tests.rs && git commit -m "perf(renderer): feed buffer-age repair into effect demand"`

### Task 3: Skip dead passes before resource realization and expose actual execution metrics

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs:340-465,534-651,1184-1580`
- Modify: `src/egl_renderer.rs:120-140,817-850,909-923`
- Modify: `src/native_output/scanout/backend.rs:230-260` for the additional compact perf fields
- Modify: `src/egl_renderer/effects/resources.rs` only if a non-invasive acquisition counter seam is needed
- Test: `src/egl_renderer/effects/executor.rs` existing tests or a focused pure helper test module

**Interfaces:**
- Consumes `EffectExecutionDemand` from Task 2.
- Changes `execute_effect_graph(renderer, graph, framebuffer_origin, repaint_plan, demand)` to execute only live instance passes.
- Produces `EffectExecutionStats { instances, passes, scene_captures, capture_pixels, blur_downsamples, blur_upsamples, composites, resource_acquisitions }`.

- [ ] **Step 1: Write failing executor selection/resource tests**

Add a test seam that walks a compiled graph with a demand and returns the selected pass IDs, selected instance IDs, and texture IDs that would be acquired. Assert a separated pruned instance contributes no pass or non-output texture, while a repair-only live instance selects its entire internal chain. Also assert a dependency-only earlier instance is selected when a later backdrop instance is live.

```rust
let selection = select_effect_execution(&graph, &demand);
assert_eq!(selection.executed_instances, vec![first_id]);
assert!(selection.acquired_texture_ids.iter().all(|id| {
    graph.texture(*id).source != GraphTextureSource::Static(_) 
}));
assert_eq!(selection.executed_passes.len(), expected_first_pass_count);
```

- [ ] **Step 2: Run the tests to verify the expected RED result**

Run: `rtk cargo test --locked --bin oblivion-one egl_renderer::effects::executor::tests::pruned_instance_does_not_realize_textures`

Expected: compile failure for the new demand-aware selection seam or an assertion showing the current executor selects every graph pass.

- [ ] **Step 3: Thread demand through the executor**

Change `execute_effect_graph` and `execute_graph_passes` to accept `&EffectExecutionDemand`. Before `ensure_pass_textures`, skip passes whose `pass.instance` is not live. Count actual executed instances from the demand only after the graph executes successfully. Keep error handling, ordinary-scene fallback, release of realized textures, shader lookup, and generation quarantine unchanged.

- [ ] **Step 4: Supply non-empty execution damage for live repair-only passes**

Add a helper that returns the scissor region for a pass:

- capture, normalization, blur, fragment, blend, and mask passes use the existing bounded graph texture domain or conservative live-instance domain when their logical `pass.damage` is empty;
- final composite/output-post-process passes use the demand's per-instance output region, unioned with any dependency output region assigned to that instance;
- non-empty logical pass damage remains unchanged for ordinary logical-damage execution.

Pass this effective region to `execute_capture`/`draw_damage_scissors` without mutating `CompiledRenderPass.damage`. This keeps logical damage and execution demand separate and ensures repair-only blur levels issue draws.

- [ ] **Step 5: Preserve checkpoint capture ordering and ordinary scene drawing**

Keep the existing `scene_cursor` ordering. Skipped dead composites must not prevent the final ordinary scene range from being drawn. A live dependency-only earlier composite must establish its demand output before a later `checkpoint_dependencies` capture copies from the current output framebuffer. Use the augmented `RepaintPlan.repair_damage` for clear/scissor/swap coverage.

- [ ] **Step 6: Add actual execution counters**

Keep `effect_instances_visible`, `render_graph_passes`, `effect_capture_pixels`, and graph peak metrics as compiled/planned values. Add compact actual fields: `effect_instances_pruned`, `effect_passes_executed`, `effect_capture_pixels_executed`, and `effect_resource_acquisitions`. Increment acquisition count only in `ensure_pass_textures` when a pooled texture is acquired. Add the same fields to native perf emission without introducing a telemetry subsystem.

- [ ] **Step 7: Run focused execution/resource tests and commit**

Run:

`rtk cargo test --locked --bin oblivion-one egl_renderer::effects::executor::tests`

`rtk cargo test --locked --bin oblivion-one egl_renderer::effects::resources::tests`

`rtk cargo fmt --check`

`rtk git diff --check`

Then commit:

`git add src/egl_renderer/effects/executor.rs src/egl_renderer.rs src/native_output/scanout/backend.rs src/egl_renderer/effects/resources.rs && git commit -m "perf(renderer): skip unconsumed effect passes"`

### Task 4: Validate end-to-end behavior and close the B2 report

**Files:**
- Modify: `src/effects/render_graph.rs` tests if any required regression from the task spec is still absent
- Modify: `src/egl_renderer/damage_tests.rs` tests if age/full-repair coverage needs completion
- Modify: `src/egl_renderer/effects/executor.rs` tests if counter/resource coverage needs completion
- Create: none unless an existing deterministic test seam requires a small focused module

- [ ] **Step 1: Add remaining required regressions**

Ensure focused coverage exists for: age 1 unrelated pruning; age 2 historical revival; age 3 accumulated repair; full repair; halo boundary; clearly outside halo; separated effects; overlapping and transitive backdrop dependency; non-overlapping earlier effect; `TargetContent`; continuous effect; move/removal; no resource realization; and compile-failure fallback. Where no deterministic GLES fixture exists, keep pure graph/selection/resource tests and do not claim hardware pixel equivalence.

- [ ] **Step 2: Run the complete focused suite**

Run:

`rtk cargo test --locked --bin oblivion-one effects::render_graph::tests`

`rtk cargo test --locked --bin oblivion-one egl_renderer::damage_tests`

`rtk cargo test --locked --bin oblivion-one egl_renderer::effects::executor::tests`

`rtk cargo test --locked --bin oblivion-one egl_renderer::effects::resources::tests`

Expected: all focused tests pass, including actual selected-pass and resource-acquisition assertions.

- [ ] **Step 3: Run required repository verification**

Run each command in the current checkout using the existing target directory:

`rtk cargo fmt --check`

`rtk cargo check --locked --all-targets`

`rtk cargo clippy --locked --all-targets -- -D warnings`

`rtk cargo test --locked`

`./bin/check-source-layout`

`rtk git diff --check`

`rtk git status --short`

Record source-layout reports separately from test/build failures; do not refactor unrelated oversized modules.

- [ ] **Step 4: Run the final source audit**

Run:

`rtk rg -n 'compile_frame_execution_plan|plan_effect_damage|EffectDamagePlan|final_damage' src/effects src/egl_renderer.rs`

`rtk rg -n 'repair_damage|render_damage|PartialRepaintPlanner' src/egl_renderer.rs src/egl_renderer`

`rtk rg -n 'checkpoint_dependencies|SceneCapture|SurfaceCapture' src/effects src/egl_renderer/effects`

`rtk rg -n 'ensure_pass_textures|execute_graph_passes|execute_pass' src/egl_renderer/effects`

`rtk rg -n 'effect_instances_visible|effect_instances_executed|effect_passes_executed|blur_downsample_passes' src`

Use the results to state: logical effect damage is compiled before repaint planning and feeds presentation history; final `repair_damage` feeds execution demand and actual swap coverage; `render_graph_passes` is planned while `effect_passes_executed` is actual; skipped effects never reach texture acquisition.

- [ ] **Step 5: Commit any final focused test-only changes**

If Task 4 adds only missing regression tests, commit them as:

`git add src/effects/render_graph.rs src/egl_renderer/damage_tests.rs src/egl_renderer/effects/executor.rs && git commit -m "test(perf): close buffer-age effect pruning coverage"`

Do not stage unrelated worktree files.

- [ ] **Step 6: Prepare the final report**

Report the original unsafe ordering, the logical-versus-repair authority split, fixed-point dependency closure, conservative fallbacks, age 1/2/3 behavior, continuous/transition behavior, deferred capture narrowing, visible versus executed counts, compiled versus executed pass counts, capture/downsample/upsample/resource counters, RED evidence, focused/full verification, commit hashes, and whether deterministic GPU pixel equivalence was available. Explicitly answer every required yes/no question from the supplied B2 task without claiming unmeasured frame-time or GPU improvements.
