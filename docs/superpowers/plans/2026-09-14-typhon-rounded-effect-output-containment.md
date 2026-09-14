# Rounded Effect Output Containment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent bounded effect-region overflow from widening any visible Composite or OutputPostProcess execution beyond its authoritative output influence region.

**Architecture:** Add a pure clip-preserving bounded intersection in `src/effects/damage.rs`. Use its exact-or-authoritative-clip semantics at effect damage, instance demand, backdrop dependency, and final-pass seed boundaries, while retaining current bounding/full-domain coalescing for internal work. Carry bounded planner diagnostics into the existing effect trace and verify the planner-to-executor scissor boundary.

**Tech Stack:** Rust 2024, Cargo, existing `EffectRegion`/render-graph planner, GLES executor, unit tests, effect execution trace.

## Global Constraints

- Keep `MAX_EFFECT_REGION_RECTS = 128`.
- Preserve the single reverse pass traversal and one propagation per producer edge.
- Preserve partial Capture clear/replay, Dual Kawase pass demand, linear-filter support, resource pruning, surface-consumer planning, and the `a26780fe`/`fccdc870` architecture.
- Internal work may use conservative bounding/full-domain fallback; visible output must remain a subset of `output_influence_region`.
- Do not modify Eclipse, blur shaders, radius, pass count, scale, alpha mode, capture footprint, filtering, shell colors, or geometry.
- Use `rtk` for repository commands and compile in `/home/agony/GitHub/Typhon` so Cargo reuses the existing local target directory.
- Preserve the five pre-existing dirty files exactly and stage only files belonging to this task.
- Do not use subagents.

---

### Task 1: Add the RED rounded-hover regression and semantic test helpers

**Files:**
- Modify: `src/effects/damage.rs` test module near the existing region tests

**Interfaces:**
- Consumes: existing `EffectRect`, `EffectRegion`, `EffectFootprint`, `plan_effect_damage`, and `MAX_EFFECT_REGION_RECTS`.
- Produces: a deterministic Eclipse-style 33-band rounded-region helper and a failing test that demonstrates current bounding-box leakage.

- [ ] **Step 1: Add a 33-band rounded-region helper in the test module**

Implement `rounded_test_region(x, y, width, height, radius, segments) -> EffectRegion` using 16 one-pixel top strips, one full-width middle strip, and 16 mirrored bottom strips. Compute each corner strip's inset from the circle equation, round the inset outward, and push exactly 33 rectangles. Keep every rectangle inside the bounding box and assert the helper returns 33 rectangles.

```rust
fn rounded_test_region(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    radius: u32,
    segments: u32,
) -> EffectRegion {
    let mut region = EffectRegion::empty();
    let middle_height = height - segments * 2;
    let corner_width = width - radius * 2;
    for band in 0..segments {
        let inset = (f64::from(radius)
            - (f64::from(radius).powi(2)
                - (f64::from(radius) - (f64::from(band) + 0.5)
                    * f64::from(radius) / f64::from(segments))
                    .powi(2))
                .sqrt())
            .ceil() as u32;
        let strip_width = corner_width + (radius - inset) * 2;
        region.push(EffectRect::new(
            x + inset as i32,
            y + band as i32,
            strip_width,
            1,
        ).unwrap());
    }
    region.push(EffectRect::new(x, y + segments as i32, width, middle_height).unwrap());
    for band in 0..segments {
        let inset = (f64::from(radius)
            - (f64::from(radius).powi(2)
                - (f64::from(radius) - (f64::from(band) + 0.5)
                    * f64::from(radius) / f64::from(segments))
                    .powi(2))
                .sqrt())
            .ceil() as u32;
        let strip_width = corner_width + (radius - inset) * 2;
        region.push(EffectRect::new(
            x + inset as i32,
            y + segments as i32 + middle_height + band as i32,
            strip_width,
            1,
        ).unwrap());
    }
    assert_eq!(region.rects().len(), usize::try_from(segments * 2 + 1).unwrap());
    region
}
```

- [ ] **Step 2: Write the failing rounded hover-transition test**

Add `rounded_hover_transition_currently_leaks_a_corner` with old `500x80` and new `650x80` regions, both at radius 20 and 16 segments. Form `source_damage = old.union(&new)`, call `plan_effect_damage(EffectFootprint::ZERO, &new, &source_damage, output_bounds)`, and assert the exact rounded region excludes a top-left clipped point while current `output_damage` includes it.

```rust
#[test]
fn rounded_hover_transition_currently_leaks_a_corner() {
    let bounds = EffectRect::new(0, 0, 1920, 1080).unwrap();
    let old = rounded_test_region(500, 100, 500, 80, 20, 16);
    let new = rounded_test_region(500, 100, 650, 80, 20, 16);
    let source_damage = old.union(&new);
    assert_eq!(old.rects().len(), 33);
    assert_eq!(new.rects().len(), 33);
    assert!(source_damage.rects().len() <= MAX_EFFECT_REGION_RECTS);

    let plan = plan_effect_damage(EffectFootprint::ZERO, &new, &source_damage, bounds);
    let corner = (500, 100);
    assert!(!new.contains_point(corner.0, corner.1));
    assert!(plan.output_damage.contains_point(corner.0, corner.1));
}
```

- [ ] **Step 3: Run the RED test and verify the failure is the leakage assertion**

Run: `rtk cargo test --locked effects::damage::tests::rounded_hover_transition_currently_leaks_a_corner -- --exact`

Expected: FAIL at `assert!(plan.output_damage.contains_point(...))` only after the helper produces the intended 33/33 decomposition; do not proceed until the failure demonstrates current output damage contains the point outside `new`.

- [ ] **Step 4: Commit the RED test**

```bash
rtk git add src/effects/damage.rs
rtk git commit -m "test: reproduce rounded effect output leakage"
```

### Task 2: Implement bounded clip-preserving intersection and damage semantics

**Files:**
- Modify: `src/effects/damage.rs`

**Interfaces:**
- Consumes: the RED rounded transition from Task 1 and existing bounded `EffectRegion` behavior.
- Produces: `EffectRegion::intersect_bounded_within(&self, clip: &EffectRegion) -> EffectRegion`, plus pure overflow metadata usable by planner diagnostics.

- [ ] **Step 1: Add a pure exact-or-authoritative-clip result helper**

Add an internal result type carrying `region`, `overflowed`, `input_rect_count`, and `clip_rect_count`. Implement the bounded pairwise loop without calling normal `push()`: append directly while the result count is below 128; return `clip.clone()` as soon as the next exact intersection would exceed the limit. Handle `conservative_full` by returning the clip without widening it. Expose `intersect_bounded_within` as the pure region-returning method and use the result helper where a planner needs diagnostic metadata.

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BoundedIntersection {
    pub(crate) region: EffectRegion,
    pub(crate) overflowed: bool,
    pub(crate) input_rect_count: usize,
    pub(crate) clip_rect_count: usize,
}

impl EffectRegion {
    pub fn intersect_bounded_within(&self, clip: &Self) -> Self {
        self.intersect_bounded_within_result(clip).region
    }

    pub(crate) fn intersect_bounded_within_result(&self, clip: &Self) -> BoundedIntersection {
        let input_rect_count = self.rects.len();
        let clip_rect_count = clip.rects.len();
        if self.conservative_full || clip.conservative_full {
            return BoundedIntersection {
                region: clip.clone(),
                overflowed: false,
                input_rect_count,
                clip_rect_count,
            };
        }
        let mut rects = Vec::new();
        for left in &self.rects {
            for right in &clip.rects {
                let Some(intersection) = left.intersect(*right) else { continue };
                if rects.len() == MAX_EFFECT_REGION_RECTS {
                    return BoundedIntersection {
                        region: clip.clone(),
                        overflowed: true,
                        input_rect_count,
                        clip_rect_count,
                    };
                }
                rects.push(intersection);
            }
        }
        BoundedIntersection {
            region: EffectRegion { rects, conservative_full: false },
            overflowed: false,
            input_rect_count,
            clip_rect_count,
        }
    }
}
```

- [ ] **Step 2: Use the result helper in `plan_effect_damage`**

Replace the output-damage `.intersect(&output_influence_region)` call with the bounded result helper. Keep `capture_region`, `source_query_region`, and `dependency_region` behavior unchanged. Preserve the result's authoritative clip fallback and expose only the minimal metadata needed for the graph's bounded diagnostics.

- [ ] **Step 3: Add direct primitive and semantic-direction tests**

Add tests that assert exact disjoint holes survive below the limit, overflowed visible clipping equals the authoritative disconnected clip rather than its bounding rectangle, and more than 128 internal `push()` operations remain bounded with the existing conservative semantics.

```rust
#[test]
fn bounded_intersection_overflow_returns_authoritative_clip() {
    let mut fragmented = EffectRegion::empty();
    for _ in 0..MAX_EFFECT_REGION_RECTS {
        fragmented.push(EffectRect::new(0, 0, 500, 1).unwrap());
    }
    let mut clip = EffectRegion::from_rect(EffectRect::new(0, 0, 1, 1).unwrap());
    clip.push(EffectRect::new(400, 0, 1, 1).unwrap());
    let result = fragmented.intersect_bounded_within(&clip);
    assert_eq!(result, clip);
    assert!(!result.contains_point(200, 0));
}
```

- [ ] **Step 4: Run the damage tests and verify GREEN**

Run: `rtk cargo test --locked effects::damage`

Expected: the RED rounded-transition test now passes, existing exact-intersection and capture-padding tests remain green, and no damage-region test fails.

- [ ] **Step 5: Commit the bounded primitive and damage correction**

```bash
rtk git add src/effects/damage.rs
rtk git commit -m "fix: preserve authoritative effect output clips"
```

### Task 3: Route render-graph demand and final-pass seeds through containment-safe clipping

**Files:**
- Modify: `src/effects/render_graph.rs`
- Modify: `src/egl_renderer.rs` only if compile/demand trace stats need forwarding

**Interfaces:**
- Consumes: `EffectRegion::intersect_bounded_within_result`, `EffectDamagePlan` metadata, `CompiledFrameGraph`, and existing demand stats.
- Produces: output-contained instance demand, dependency demand, precise/conservative final-pass demand, and bounded diagnostic counters.

- [ ] **Step 1: Extend bounded planner statistics with distinct diagnostic counters**

Add counters to `EffectDemandPlanStats` for `region_representation_overflows`, `visible_clip_fallbacks`, and `work_region_bbox_coalesces`, preserving `Copy`, `Default`, and existing trace construction with `..Default::default()`. Count ordinary internal `push()` coalescing only where planner code knowingly accepts it; do not reclassify internal full-domain fallback as a visible clip fallback.

- [ ] **Step 2: Replace direct instance clipping**

In `plan_effect_execution_demand`, replace `repair_region.intersect(&instance.output_influence_region)` with the bounded result helper. Store the returned region, increment overflow/visible-clip counters when the result falls back, and keep the instance selected with the complete output influence in that case.

- [ ] **Step 3: Replace backdrop dependency clipping**

Replace `dependency.output_influence_region.intersect(&consumer.capture_region)` with the bounded operation using `dependency.output_influence_region` as the authoritative clip. This preserves dependency visibility containment while allowing the complete lower output influence on overflow. Keep the existing single reverse traversal and one edge propagation.

- [ ] **Step 4: Constrain precise final-pass seeds**

In `plan_effect_pass_execution_demand`, form `pass.damage.union(&instance_demand.output_region)` and clip it with `instance.output_influence_region` using the bounded operation before intersecting the output texture domain. If the bounded result overflows, mark the instance conservative and seed its final pass with the complete output influence, never the bounding rectangle.

```rust
let seed_candidate = pass.damage.union(&instance_demand.output_region);
let instance_output_influence = demand
    .output_region(instance_demand.id)
    .expect("selected instance has an output influence region");
let bounded_seed = seed_candidate.intersect_bounded_within_result(instance_output_influence);
let seed = bounded_seed
    .region
    .intersect_rect(output_plan.domain);
if bounded_seed.overflowed {
    mark_instance_conservative(demand, &mut conservative_instances, instance_demand.id);
}
pass_regions[pass_index] = seed;
```

- [ ] **Step 5: Constrain conservative final-pass and compatibility paths**

Update `conservative_pass_region` and the no-pass-plan branch of `effective_pass_damage` so Composite and OutputPostProcess always use the instance output influence as the final authoritative clip. Keep internal pass kinds full-domain. Ensure output texture-domain clipping does not replace an authoritative region with a bounding box.

- [ ] **Step 6: Add render-graph invariant tests**

Add `region_is_subset_of`/`region_covers_region` assertions for fragmented repairs, disconnected output regions, rounded corners, direct instance demand, backdrop dependency propagation, precise final-pass seed, conservative fallback, and precise → conservative → precise planning. Assert that output regions contain no point in gaps or rounded cutouts and that overflow results equal the authoritative clip.

- [ ] **Step 7: Run render-graph tests and commit**

Run: `rtk cargo test --locked effects::render_graph`

Expected: all render-graph demand, dependency, pass-demand, and existing `fccdc870` conservative-output tests pass.

```bash
rtk git add src/effects/render_graph.rs src/egl_renderer.rs
rtk git commit -m "fix: constrain effect demand to visible output"
```

### Task 4: Make trace diagnostics and executor scissors observable

**Files:**
- Modify: `src/egl_renderer/effects/trace.rs`
- Modify: `src/egl_renderer/effects/executor.rs`

**Interfaces:**
- Consumes: demand diagnostic counters and authoritative per-instance output regions.
- Produces: trace-gated overflow/fallback events and an executor regression at the scissor boundary.

- [ ] **Step 1: Add trace fields for bounded-region diagnostics**

Include the three distinct names in effect trace output: `region_representation_overflow`, `visible_clip_fallback`, and `work_region_bbox_coalesce`. Extend the existing demand-plan event with bounded counters and add an event formatter/test that includes `frame_id`, `instance`, `pass`, `input_rect_count`, `clip_rect_count`, and `fallback=output_influence` for visible clipping fallback records.

- [ ] **Step 2: Preserve the trace gate**

Emit diagnostics through `EffectExecutionTrace::event`, keeping expensive formatting inside the closure so disabled tracing does not evaluate it. Do not change GPU timer-query output or emit per-frame diagnostic noise when `TYPHON_EFFECT_EXEC_TRACE` is disabled.

- [ ] **Step 3: Add executor-level scissor containment test**

Build a final Composite graph with a disconnected/rounded authoritative output region and a fragmented demand that would overflow ordinary intersection. Pass the resulting execution damage to `effect_damage_to_texture_rects`/`draw_damage_scissors` test seams and assert every texture scissor is contained by the authoritative output influence, including no rounded-corner point and no disconnected-gap point.

- [ ] **Step 4: Run focused executor and trace tests and commit**

Run: `rtk cargo test --locked egl_renderer::effects`

Expected: trace gating, pass summary, executor damage, and scissor tests pass.

```bash
rtk git add src/egl_renderer/effects/trace.rs src/egl_renderer/effects/executor.rs
rtk git commit -m "test: guard final effect scissors against clip leakage"
```

### Task 5: Full verification and native qualification

**Files:**
- No additional files unless a verification-discovered task-local fix is required

**Interfaces:**
- Consumes: all committed task changes.
- Produces: fresh verification evidence and native acceptance report.

- [ ] **Step 1: Run formatting and compilation checks in the repository folder**

```bash
rtk cargo fmt --all -- --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
```

- [ ] **Step 2: Run all requested focused and full tests**

```bash
rtk cargo test --locked effects::damage
rtk cargo test --locked effects::render_graph
rtk cargo test --locked egl_renderer::effects
rtk cargo test --locked egl_renderer
rtk cargo test --locked effects
rtk cargo test --locked
```

If a workspace-wide command is blocked by unrelated pre-existing changes, record the exact blocker and do not modify the unrelated files.

- [ ] **Step 3: Run diff and worktree checks**

```bash
rtk git diff --check
rtk git status --short
```

Confirm only task commits/files are present in the task diff and the five original dirty files remain unchanged.

- [ ] **Step 4: Run native RTX 3060 Ti acceptance when the normal launch command is available**

Launch with:

```bash
TYPHON_EFFECT_GPU_TIMING=1 TYPHON_EFFECT_EXEC_TRACE=1 <normal Typhon launch command>
```

Exercise Dock hover enter/slow/rapid/collapse/repeated enter-leave, horizontal and vertical positions when practical, rounded Bar/ContextMenu blur, simultaneous components, and ordinary blurred-window movement/resize. Inspect trace events for visible clip fallback records and verify no final Composite bounding-box execution outside the authoritative region, no rounded-corner/gap fill, no capture-padding leakage, no stale edge/freeze, `dropped_spans = 0`, and `disjoint_invalidated_spans = 0`.

- [ ] **Step 5: Commit any final task-local formatting/test adjustment and report evidence**

Use a task-only commit if needed, then report the RED reproduction, overflow counts and leaked point, primitive semantics, retained internal fallback semantics, all updated call sites, diagnostics, focused/full results, and native result or the precise reason native qualification was unavailable.
