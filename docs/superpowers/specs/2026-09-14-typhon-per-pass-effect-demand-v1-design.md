# Per-Pass Effect Demand v1 Design

## Goal

Remove avoidable full-domain execution inside an already-selected effect
instance while preserving the existing effect-instance dependency closure,
visual output, texture domains, blur footprint, and GLES timing lifecycle.

## Architecture

The implementation remains a two-layer demand pipeline:

```text
repaint repair
  -> EffectExecutionDemand instance closure
  -> EffectPassExecutionDemand reverse texture closure
  -> executor scissors and capture rectangles
```

The existing `plan_effect_execution_demand()` traversal remains structurally
unchanged. After it has selected instances and calculated their output regions,
the render-graph module derives bounded output demand for the selected passes.
The planner stores one `EffectPassExecutionDemand` for each pass belonging to a
selected instance, including empty regions so pass selection can prune unused
branches deterministically.

## Pure pass planner

The planner builds a producer table for captured and intermediate graph
textures. Each such texture must have exactly one producing pass. Output and
static textures are external inputs and are not pass-produced. Missing,
duplicate, unknown, or non-topological producer relationships identify the
affected instance; that instance receives the existing full output-domain
semantics for all of its passes. A graph-wide instance-metadata failure keeps
the existing global conservative fallback.

For a precise plan, the planner finds the final `Composite` or
`OutputPostProcess` pass for every selected instance and seeds it with the
existing final-pass demand: `pass.damage` union the instance output region,
clipped to its output texture domain. It then walks the compiled pass order
once in reverse. A non-empty pass demand is retained, and each graph-texture
input receives a mapped upstream demand exactly once for that consumer edge.
The producer receives the union of all downstream requirements before it is
visited in reverse order. No structural region-equality loop or retry-until-
stable algorithm is introduced.

Dependency rules are explicit:

- Scene and surface capture have no graph inputs; their demand is the required
  capture region.
- Composite and output post-process map their pointwise final demand to the
  input texture.
- NormalizeInput uses its explicit output-domain to input-domain mapping with
  conservative resampling support.
- ColorMatrix, Tint, Noise, and fused local stages propagate pointwise demand.
- Blend and Mask propagate demand to every graph-texture input.
- CustomFragment expands upstream demand by its validated declared sampling
  radius.
- Dual Kawase passes map output physical pixels to input normalized pixels,
  expand by the current pass blur radius and one physical texel of linear
  filtering support, then round outward and clip to the input logical domain.

The mapping uses the actual input and output texture dimensions, so odd pyramid
dimensions and non-unit scale do not truncate required samples. All region
unions use the existing `MAX_EFFECT_REGION_RECTS` bounded `EffectRegion`; an
unrepresentable pass region marks the affected selected instance conservative.

## Executor integration

`select_effect_execution()` uses pass demand when a planned demand is present:
empty pass demand means no pass and no output-resource acquisition. A manually
constructed legacy demand without pass entries keeps the old selected-instance
behavior for compatibility. `effective_pass_damage()` returns the planned
output region for precise passes and retains the old full-domain fallback for
legacy/global-conservative or per-instance-conservative passes.

Ordinary replay capture clears only the physical target rectangles mapped from
the demanded capture region, then replays the same logical regions. This makes
transparent black available for every sampled demanded texel without relying
on pooled texture contents. Direct framebuffer capture remains a full-domain
blit in v1; its pass trace is marked as a conservative framebuffer-capture
fallback.

Surface-consumer planning consumes the precise capture demand, except for the
intentionally conservative direct checkpoint path. Existing scene replay,
orientation, resource lifetime, fallback, and GPU query begin/end behavior are
unchanged.

## Demand diagnostics and metrics

`EffectDemandPlanStats` gains bounded pass-level counters for selected passes,
partial/full-domain passes, pass dependency propagations, maximum pass-region
rectangle count, and pass conservative fallbacks. Trace output reports these
alongside the existing instance counters without default per-pass verbosity.

The allocated capture metric remains physical allocated texture pixels. The
executed capture metric becomes physical demanded target-rectangle pixels;
direct framebuffer capture reports its full physical target area. GPU timing
`*_pixels` remains the existing effect-space execution-region area and is
documented as such, so allocated, executed physical, and timing effect-space
quantities are not mixed.

## Testing

Renderer-independent tests cover the six-pass built-in blur demand shape,
full-demand execution, zero-footprint local stages, custom footprints, blend
and mask fan-in, fragmented bounded regions, domain edges, odd dimensions,
translated domains, conservative regions, malformed producers, and preservation
of the existing instance dependency counters and closure tests.

Executor tests cover precise versus conservative damage, empty-pass selection
and resource pruning, replay partial-clear planning, transparent source-over
capture preparation, precise surface consumers, conservative direct capture,
GPU timing only for executed passes, capture metric units, and unchanged error
fallback behavior. Existing effects, EGL, lifecycle, screenshot, native-output,
compositor, and timing regressions remain in the verification scope.

## Non-goals

This design does not alter the instance dependency algorithm, graph texture
domains, aggregate blur footprint, shaders, blur radius/pass count/scale,
filtering mode, alpha or color policy, direct framebuffer blit behavior,
resource budgets, KMS, pacing, scanout, screenshot, Lamp, or protocol work.

