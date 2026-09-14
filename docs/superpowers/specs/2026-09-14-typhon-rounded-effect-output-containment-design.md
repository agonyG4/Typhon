# Rounded Effect Output Containment Design

## Problem

Typhon's bounded `EffectRegion` representation is intentionally conservative for internal effect work, but its ordinary Cartesian `intersect()` can overflow the 128-rectangle budget while clipping a fragmented repair region to a rounded visible effect. `EffectRegion::push()` then replaces the exact fragments with a bounding rectangle. When that result reaches a final Composite or OutputPostProcess pass, pixels outside the authoritative rounded output influence can be drawn for one or more frames during animated geometry changes.

The confirmed reproduction is an Eclipse-style 16-segment rounded decomposition: each 500x80 or 650x80 region has about 33 rectangles, and the old/new transition creates enough pairwise intersections to exceed 128. A top-left point in the new rounded bounding box but outside its corner is then incorrectly included in planned output damage.

## Goals and constraints

- Preserve `MAX_EFFECT_REGION_RECTS = 128`.
- Keep bounding-box/full-domain fallback semantics for internal work and dependency demand where over-approximation is safe.
- Guarantee every visible execution region is contained by its authoritative output influence region.
- Preserve the existing per-pass demand architecture, single reverse dependency traversal, partial capture/replay, Dual Kawase demand, linear filtering, pruning, and `fccdc870`'s separation of internal work from final output.
- Do not change Eclipse, blur shaders, blur quality, capture footprints, alpha behavior, or renderer-specific Shell/Dock clipping.

## Chosen approach

Add a pure `EffectRegion::intersect_bounded_within(&self, clip: &EffectRegion) -> EffectRegion` operation. `clip` is authoritative. The operation handles conservative-full operands by returning the clip, otherwise emits exact pairwise intersections until the next rectangle would exceed the bound. On overflow it returns `clip.clone()` directly, bypassing normal `push()` coalescing.

Its contract is:

```text
exact(self ∩ clip) ⊆ result ⊆ clip
```

This gives bounded work and preserves holes/disconnected regions because the fallback is the exact authoritative clip, not its bounding rectangle. It remains pure and does not expose the region's internal fields.

## Affected data flow

Use the clip-preserving operation at every output-visible boundary:

1. `plan_effect_damage()` clips expanded source damage to the new output influence region.
2. `plan_effect_execution_demand()` clips direct repair demand to each instance output influence.
3. Backdrop dependency propagation clips a dependency's required output to that dependency's output influence.
4. Precise final-pass seed construction clips `pass.damage ∪ instance_demand.output_region` to the instance output influence before texture-domain clipping.
5. Conservative final-pass construction and compatibility/no-pass-plan paths keep final demand constrained to the instance output influence; internal passes retain full-domain conservative demand.

Where a path has to combine an existing output region with another required region, the authoritative output influence is the final clip. A fallback may therefore execute the entire valid influence region, but never the influence's bounding box.

## Diagnostics

Add bounded effect-trace diagnostics with three distinct semantic labels:

- `region_representation_overflow` for the generic bounded-region overflow condition;
- `work_region_bbox_coalesce` when ordinary internal `push()` fallback coalesces to a bounding rectangle;
- `visible_clip_fallback` when an authoritative output clip is returned after an overflowing visible intersection.

Visible fallback records include `frame_id`, instance, pass, input rectangle count, clip rectangle count, and `fallback=output_influence`, and are emitted only while the existing effect trace gate is enabled. GPU timer fields remain unchanged.

## Testing strategy

Follow TDD with a pure damage regression first. Build a 33-band rounded-region helper for 500x80 and 650x80 geometry, form old+new source damage, plan the new visible output, and assert that a clipped top-left corner is outside the visible region but leaks into current `output_damage` (RED). After the primitive and call-site updates, assert the corner is excluded and `output_damage` is a subset of `output_influence_region`.

Add focused semantic tests for:

- one rounded region, horizontal growth/shrink, animated radius, translation, output-edge clipping, and a 96-rectangle client region;
- fragmented repair intersecting rounded output, precise/conservative/precise transitions, disconnected regions with empty gaps, and rounded cutouts;
- internal work overflow retaining bounded conservative behavior;
- instance output demand and backdrop dependency demand containment;
- final Composite/OutputPostProcess seed containment in precise and conservative paths;
- executor scissor conversion receiving no rectangle outside the authoritative final output influence.

Run the requested focused and workspace-wide `rtk cargo` checks, formatting, clippy, diff check, and native RTX 3060 Ti qualification when the normal launch command is available. Native acceptance requires no rounded blur block/flash/gap fill and healthy GPU timing (`dropped_spans = 0`, `disjoint_invalidated_spans = 0`).

## Alternatives considered

1. Increase or remove the rectangle bound. Rejected: it postpones or removes bounded work guarantees and leaves the semantic collision unresolved.
2. Globally replace overflow with the authoritative clip. Rejected: internal demand is allowed to over-approximate and would lose useful conservative work semantics; only visibility-clipped operations need this contract.
3. Renderer-side clipping or Eclipse geometry changes. Rejected: it would be product-specific or reduce valid exact geometry. The planner must provide a correct generic execution region.
