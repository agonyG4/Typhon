# Typhon Pointer Constraint Region Algebra v1

## Status

Approved for implementation on 2026-09-03 from the supplied region-algebra
requirements.

## Scope

Replace only the production effective-region resolver used by pointer
constraints. The accepted transaction and native-input architecture remains:

```text
protocol requests
    -> pending surface pointer state
    -> exact wl_surface.commit capture
    -> CachedSubsurfaceCommit
    -> surface publication
    -> native backend request
    -> NativeInputEpoch-safe settlement
```

This change does not alter pointer motion values, confinement warp semantics,
locked absolute immutability, native input epochs, event ordering, readiness,
transition timing, scheduling, or backend acceleration. It does not include
Sober-specific fixes.

## Current defect

`pointer_constraint_output_region` currently scans every integer point in the
renderable surface. Each point evaluates the committed constraint region and
locks the committed input-region mutex through `input_region_contains`. The
work therefore scales with surface pixel area and couples geometry resolution
to repeated shared-state locking.

The effective region is the exact set:

```text
committed constraint region ∩ committed surface input region
```

both clipped to the surface-local half-open bounds `[0,width) × [0,height)`.

## Representation

Add a private `SurfaceRectRegion` implementation for pointer-constraint
geometry. It stores positive-area, non-overlapping half-open rectangles with
i64 edges. Input `i32` coordinates and widths are widened before edge
arithmetic; clipping and subtraction therefore remain safe for extreme
protocol values.

The representation has these operations:

- empty and full-surface construction;
- ordered add/union;
- ordered subtract;
- pairwise intersection;
- bounds clipping;
- deterministic rectangle ordering;
- translation into the existing `OutputRect`/`OutputRegion` types.

Rectangles are ordered top-to-bottom and left-to-right, with stable edge
tiebreakers. The resolver does not coalesce arbitrary adjacent geometry: the
canonical set is already sufficient for bounded geometry work, and preserving
deterministic fragments makes `OutputRegion::closest_point` tie behavior
auditable against the test-only raster oracle.

`SurfaceInputRegion::Default` materializes as the full surface basis for both
constraint and input regions. A custom input region starts empty and applies
`wl_region.add` unions and subtracts in protocol order. A null/default input
region is effectively infinite before surface clipping. No opaque-region
semantic defaults are reused.

## Snapshot and resolution

`SurfaceData::committed_input_region_snapshot` clones the committed input
region while holding the input-region mutex once, then releases the lock.
Poisoned state remains fail-open as the default/infinite input region, matching
the existing hit-testing behavior.

The pointer resolver obtains the committed constraint region and this one
input-region snapshot, materializes both with the same surface bounds,
intersects them, and translates the resulting deterministic rectangles to
output coordinates. No area loop and no input-region mutex operation occur in
the geometry path.

## Timing evidence

The existing `NativePointerTransitionEvidence` architecture remains the owner
of transition timing. Resolution duration and resolver thread-CPU duration are
recorded as function-level fields attached to the selected transition
evidence, alongside optional dimensions and rectangle/operation counts. The
trace-disabled path retains the existing zero-cost behavior: it does not add
clocks, formatting, allocation, or counters when tracing is disabled.

## Verification strategy

Before production replacement, deterministic RED coverage will establish:

- constant resolver operation work for 40×30, 1920×929, and 7680×4320 full
  regions;
- one committed input-region snapshot per resolution;
- exact defaults, custom regions, clipping, overlaps, holes, islands, empty
  results, and extreme coordinates;
- `OutputRegion::closest_point` equivalence, including fractional probes and
  ties, against a cfg(test)-only legacy raster oracle;
- real Wayland-resource coverage for lock/confine, input regions, exact
  commit capture, delayed and synchronized publication, and Sober-shaped
  dimensions.

The old resolver is retained only as test code. The focused algebra suite,
pointer-constraint integration suite, causal timing suite, and the required
repository checks will be run through `rtk`.

## Upstream design references and rejected alternative

The algorithm follows the standard compositor region approach used by
Wayland-family implementations: ordered rectangle unions, subtraction by
splitting intersecting rectangles, and pairwise intersections over region
geometry. Pixman was considered but rejected because this is a small integer
surface-local set problem and introducing an external raster/region dependency
would add dependency, lifetime, and conversion complexity without improving
the required exact semantics.

