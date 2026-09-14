# Astrea Lamp v2.2 Continuous Overlapping Funnel Design

**Status:** Approved for implementation

## Goal

Replace the sequential Lamp Bump/Stretch/Squash visual model with one continuous, reversible funnel deformation while preserving the v2.1 lifecycle and complete-visual-domain contracts.

## Design

`WindowLifecycleAnimator` continues to own only raw lifecycle progress `p`, with `0.0` as the presented source and `1.0` as the exact anchor endpoint. The CPU helper `lamp_motion_channels(p, bump_distance)` is the sole temporal authority. It computes one global `InOutCubic` value `q`, overlapping contraction and translation channels, and an optional retreat channel derived from raw `p`.

The exact initial constants are centralized in `src/window_lifecycle_animation.rs`:

```text
contraction end       = 0.42 in q-space
translation start     = 0.15 in q-space
translation blend     = 0.25 of normalized translation time
retreat end           = 0.30 in raw-p space
spatial exponent      = 2.0..3.0 from the frozen shape factor
stretch power         = 2.0
```

For translation, `t = clamp((q - 0.15) / 0.85, 0, 1)`. With blend `b = 0.25`, the unnormalized soft-start ramp is `t²/(2b)` for `t < b`, otherwise `t - b/2`. It is divided by `1 - b/2`, so its endpoint is one. Both branches have value `b/2` and derivative one at `t=b`; normalization preserves that derivative equality. The resulting channel is C1 at the quadratic-to-linear join and at the start boundary, while the global easing supplies endpoint slowdown.

Spatially, each point uses the complete presented visual rectangle and a direction-normalized movement coordinate `m`. The funnel weight is `pow(m, exponent)`, with exponent linearly mapped from the frozen shape factor range `0.20..0.80` to `2.0..3.0`. Cross-axis completion is `1 - (1 - contraction * funnel_weight) * (1 - row_translation)`. Main-axis movement lerps from a bounded, direction-aware retreated source axis to the target axis using `row_translation`, where `row_translation = pow(translation, 1 + 2 * contraction * (1-m))`.

The GLSL vertex shader receives CPU-computed temporal channels and contains only the spatial mapping/deformation. It maps canonical visual coordinates to presented source visual coordinates, uses the visual rectangle for all domain calculations, and removes every legacy stage uniform and stage constant.

## Preserved contracts

- Exact source identity at `p=0` and normalized anchor mapping at `p=1`.
- Exact positional continuity when the animator reverses at any raw progress.
- Frozen lifecycle visual groups, SSD decorations, anchors, directions, transition IDs, settlement, render evidence, and retained surfaces.
- Existing mesh resolution, bounded admission, 280 ms duration, opacity interval, Effects/Blur behavior, KMS/pacing, and ignored subsurface ownership gate.

## Testing strategy

Before removing the old implementation, a RED test will exercise actual warped mesh points around the old no-bump Stretch-to-Squash boundary and bump Bump-to-Stretch boundary. It will assert that aggregate finite-difference velocity must not collapse; the current implementation must fail with near-zero boundary velocity. After replacement, that test is replaced by dense aggregate-motion coverage and C1 channel-boundary tests, plus exact endpoint, reversal, direction-rotation, retreat, funnel-ordering, boundedness, and shader-uniform contract tests.

