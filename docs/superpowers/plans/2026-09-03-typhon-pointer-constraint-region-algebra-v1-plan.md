# Typhon Pointer Constraint Region Algebra v1 Implementation Plan

## Starting point

- Starting HEAD: `cc9fb1c196558bfad8c958c1483951c17cc61fa3`
- Existing unrelated worktree edits are preserved, including native-pacing
  source changes and its untracked design/plan documents.

## Task 1: Add RED algebra and snapshot coverage

Add deterministic tests for the planned rectangle-region abstraction and
committed input-region snapshot API before replacing the resolver. Cover
constant work at small, Sober-shaped, and large dimensions; defaults and
custom ordered add/subtract operations; clipping, extreme coordinates,
overlap/no-overlap, holes, islands, empty regions; deterministic output order;
and closest-point equivalence to a cfg(test)-only raster oracle. Add a direct
snapshot test that proves committed state is cloned and the input lock is not
used by geometry after the snapshot.

Run the focused RED tests and record their pre-fix failure. Commit the tests
separately from the implementation.

## Task 2: Implement the region algebra

Create the private i64 half-open rectangle region type with empty/full
constructors, exact ordered union, subtraction, intersection, clipping,
deterministic ordering, and output translation. Keep operation accounting
test-only and ensure the production path has no area-scaled loops.

Implement the legacy raster oracle only under `cfg(test)`. Use the same
half-open membership semantics as the old resolver and compare both membership
and closest-point probes, including fractional positions and equal-distance
ties.

## Task 3: Snapshot committed input state once

Add `SurfaceData::committed_input_region_snapshot`, preserving fail-open
behavior for a poisoned input-region mutex. Replace the pointer resolver's
per-point `input_region_contains` calls with one snapshot, materialize both
regions against the surface bounds, intersect, and translate to the existing
`OutputRegion` without changing its public semantics.

## Task 4: Preserve transition evidence and integration behavior

Attach resolver duration and thread-CPU duration to the existing selected
`NativePointerTransitionEvidence`, without adding clocks, formatting,
allocation, or counters when tracing is disabled. Preserve activation,
deactivation, confinement updates, late-bound anchors, generations, and
NativeInputEpoch settlement boundaries.

Add/retain deterministic real Wayland-resource regressions for default/custom
regions, exact input/constraint commit capture, delayed publication, real
pointer constraints, synchronized subsurfaces, and the Sober-shaped 1920×929
case. Verify no pointer motion or backend ordering changes.

## Task 5: Audit and verify

Audit all resolver callers and remove semantic dependence on area scans or
surface-wide input ownership. Run focused region, pointer-constraint, and
causal timing tests, then run the exact required checks through `rtk`:

```text
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

Document every actual result, unrelated blockers, anything not run, and the
remaining requirement for manual native Linux qualification. Update the
existing English v1.1 closure report with the region-algebra closure and the
non-claim that the Sober/Roblox camera jump is fixed.

