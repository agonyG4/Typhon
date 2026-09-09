# Typhon B2 Effect Subgraph Pruning Design

**Goal:** Make effect GPU work proportional to visible effect subgraphs consumed by the final repaired output slot, while keeping logical damage as the only presentation-history authority.

**Scope:** B2 only. This design does not add persistent effect-result caching, capture-domain narrowing, upload/import deferral, graph-topology caching, resource-budget changes, or quality reductions.

## Current problem

Typhon currently validates and compiles every visible effect instance from current logical damage before the repaint planner knows the acquired slot's buffer-age repair region. The resulting `CompiledRenderPass.damage` is therefore a logical-damage field, not a safe execution-liveness authority. In particular, an unchanged effect can have empty current logical damage while its pixels are required to reconstruct an older acquired slot.

## Design

`compile_frame_execution_plan` remains the stage-1 validator and logical-damage compiler. It still resolves every visible instance and fails conservatively for missing programs, invalid graphs, unsupported nodes, and graph limits. It records one bounded `CompiledEffectInstance` per visible instance with:

- stable instance identity;
- output influence region from `plan_effect_damage`;
- conservative capture region from `plan_effect_damage`;
- earlier effect-instance dependencies derived from existing backdrop checkpoint dependencies.

The compiled graph continues to retain all planned passes and textures as CPU metadata. No pooled GPU texture is acquired until execution.

After `PartialRepaintPlanner::plan` returns, the renderer converts the final `RepaintPlan.repair_damage` to an `EffectRegion`. A new pure planner computes `EffectExecutionDemand`:

1. Full, invalidated, or conservative repair marks every visible instance live.
2. Partial repair initially marks an instance live when repair intersects its output influence region.
3. Each live instance propagates liveness backward through its validated backdrop/checkpoint dependencies until stable.
4. Dependency-only instances receive the intersection of their output influence and the dependent capture region as an execution output region.
5. A live instance executes its complete compiled internal chain conservatively; only its final composite uses its execution output region. Internal pass scissor regions are widened to the bounded capture/output domain when logical pass damage is empty.

`TargetContent` captures do not create checkpoint dependencies, so geometric overlap alone cannot retain an earlier backdrop effect. Existing semantic scene ordering, anchors, visual groups, and checkpoint metadata remain authoritative.

Dependency-only output writes can extend beyond the planner's initial repair rectangles because a later checkpoint capture may copy those pixels from the current output framebuffer. The renderer unions this bounded dependency output region into the actual repair/swap damage before execution. `render_damage` remains unchanged and is still the value committed to presentation damage history.

The executor skips dead-instance passes before `ensure_pass_textures`, so pruned instances perform no shader lookup, pass execution, or capture/intermediate resource acquisition. Ordinary scene drawing still runs for the repaint region even when no effect instance is live. Full or uncertain repair bypasses pruning and executes the complete graph.

## Metrics

Keep compiled metrics separate from execution metrics:

- `effect_instances_visible` and `render_graph_passes` describe the validated/compiled graph;
- `effect_instances_executed`, `effect_instances_pruned`, and `effect_passes_executed` describe actual execution;
- existing blur/capture counters are actual execution counts, with planned capture pixels kept separate from executed capture pixels;
- per-frame effect resource acquisition count is incremented only when a graph texture is actually acquired.

## Test strategy

Pure render-graph tests cover unrelated damage pruning, age-independent demand, halo intersection, full-repair fallback, independent effects, transitive backdrop closure, non-overlapping backdrop pruning, and `TargetContent` non-dependency. Partial repaint tests cover age 2/3 repair regions. Executor tests use a test seam around pass selection/scissor demand and resource realization to prove skipped instances acquire no textures and repair-only instances execute their full chain. Existing effect transition and continuous-demand tests remain authoritative; focused tests add move/removal and continuous liveness assertions where the current seams permit. No large GPU harness is introduced unless an existing deterministic fixture can be extended without changing renderer scope.

## Fallback and deferred work

Any unknown topology, missing metadata, conservative region, full repair, or planner ambiguity falls back to complete graph execution. Persistent pixel caching and capture narrowing remain separate follow-up optimizations. B1, A1, A2, and B3 behavior remains untouched.
