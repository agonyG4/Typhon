# Typhon PointerSceneHit Cache and Focus-Churn Closure Design

## Scope and preserved architecture

This closure addresses the remaining ordinary pointer scene-cache correctness
defect, client/server-side-decoration focus churn, and avoidable work in the
uncached hit-test path. It preserves `VisualStackGroup` as the shared
render/input ordering authority, `PointerSceneHit` as the ordinary pointer
scene ownership authority, exact `WindowId`/root ownership for decoration
actions, immediate interactive resize, the bounded latest-wins configure
pipeline, presentation-domain damage history, buffer-age handling, and
fullscreen frame-scene authority.

The actual Git checkout is `/home/agony/GitHub/Typhon`; the declared Codex
workspace directory is empty. The working tree at the actual checkout is the
implementation baseline and contains substantial unrelated prior closure work.

## Residual cache defect — CONFIRMED

`PointerSceneHitCache` currently keys a result by pointer coordinates and
`scene_render_generation`. `surface_accepts_input_at()` also reads committed
Wayland input-region state from `SurfaceData`. A surface can therefore change
pointer ownership without changing the render-visible scene generation.

The production path in
`src/compositor/state/surface_transactions.rs::apply_cached_subsurface_commit`
does this in the current baseline:

1. apply the committed input region;
2. publish any buffer/geometry state;
3. call `refresh_pointer_focus_at_last_position()` when the input region
   changed.

For an input-region-only commit that does not advance
`scene_render_generation`, the refresh reaches `pointer_scene_hit_at()` and
reuses the old coordinate/render-generation cache entry. That can continue
routing a stationary pointer to a surface whose input region now excludes the
point.

Exact reproducer:

```text
Window A is in front of Window B and both cover P.
P is stationary and A's input region contains P.
pointer_scene_hit_at(P) -> Client(A), cached at render generation N.
A commits set_input_region(empty).
the normal input-region-changed refresh runs at P.
pointer_scene_hit_at(P) sees the same coordinates and N -> stale Client(A).
```

The reverse excluded-to-included transition has the same failure in the other
direction: B remains cached even after A becomes input-capable at P.

## Cache validity contract

The cache is valid only when all of these values match the current state:

```text
coordinate x/y
scene_render_generation
pointer_hit_generation
```

The cache stores stable hit identity (`WindowId`, surface identity, root
surface identity, and `DecorationHit`) or an owned `PointerTarget`; it does not
store raw references whose lifetime can outlive a surface resource.

`scene_render_generation` remains authoritative for render-visible geometry,
surface placement, committed scene ordering, and visual WindowVisual changes.
`pointer_hit_generation` is a lightweight ownership/topology generation for
input state that is not guaranteed to be represented by render generation.

## Hit-test dependency audit

| Dependency | Current source | Generation classification | Invalidation decision |
| --- | --- | --- | --- |
| Surface input region | committed `SurfaceData` input region | input-only | advance `pointer_hit_generation` after state mutation, before refresh |
| WindowVisual geometry / SSD extents | `toplevel_visual_geometries`, renderable placements, decoration layout | `scene_render_generation` | existing placement/resize/mode generation remains authoritative; existing origin invalidation also advances pointer generation |
| Window stacking / surface order | `VisualStackGroup`, renderable surface order | `scene_render_generation` plus existing origin/group invalidation | advance pointer generation through the existing invalidation seam |
| Popup topology | popup maps/nodes and surface-tree grouping | render generation or existing topology invalidation where published | existing invalidation seam advances pointer generation |
| Subsurface topology | placements and committed subsurface stack | render generation or existing topology invalidation | existing invalidation seam advances pointer generation |
| SSD mode and geometry | decoration preference/mode and visual geometry | `scene_render_generation` | no second generation for render-visible geometry |
| Resize input extents | current visual WindowVisual geometry | `scene_render_generation` | add parity regression; do not add duplicate generation |
| Surface map/unmap/destruction | renderable surfaces, resources, unmap teardown | existing unmap/invalidation and resource identity checks | existing invalidation seam advances pointer generation; refresh removes stale ownership |
| Layer ordering | layer scene rank and renderable ordering | render generation / existing reorder invalidation | no layer-specific policy change |
| XWayland Shape input | Shape is negotiated and diagnosed but is not currently applied to compositor hit testing | not currently a hit-test dependency | no special case without an applied source; record as unproven for future X11 shape work |
| Pointer constraints, grabs, DND, compositor move/resize | routing precedence before ordinary scene hit | not ordinary cached ownership | preserve precedence; ordinary cache cannot override these routes |

The existing `invalidate_surface_origin_cache()` is the narrow shared seam for
surface order/placement/topology changes. It will stop directly dropping the
pointer cache and instead advance the pointer-hit generation, allowing the
same cache contract to invalidate it without scattered cache resets. Input
region commits use the dedicated seam because they do not necessarily need
origin-cache invalidation.

## Chosen invalidation architecture — CONFIRMED

Add `pointer_hit_generation: u64` to `CompositorState` and to
`PointerSceneHitCache`. Advance it with a non-zero wrapping serial whenever a
mutation can change ordinary hit ownership:

```text
mutate input ownership/topology
advance pointer_hit_generation
refresh stationary pointer focus
```

The `input_region_changed` branch in `apply_cached_subsurface_commit` will
advance the generation before calling the existing refresh function. Existing
surface-origin/order invalidation will advance it as a shared ownership
invalidation seam. Render-only events such as title changes, client damage,
frame callbacks, and presentation feedback do not advance it unless they pass
through an already ownership-affecting geometry/order mutation.

The ordinary hit path will use two phases:

1. borrow immutable scene data and calculate an uncached hit;
2. after those borrows end, store the hit and both generations in the cache.

No `unsafe` code and no whole origin-cache clone are needed. Root surface
indices will be stored in each `VisualStackGroup` at construction time, so the
hit test will use O(1) root lookup rather than a per-group linear scan through
all renderable surfaces.

## Stationary refresh ordering — CONFIRMED requirement

The input-region commit path will be tested directly for ordering. The test
must fail if refresh happens before generation advancement, because the first
refresh would consume the stale entry. The production sequence is therefore
state mutation, generation advancement, stationary refresh.

## Client-to-SSD focus model — CONFIRMED

Ordinary scene ownership transitions are:

```text
Client(A) -> Decoration(A): wl_pointer leave A; desktop focus remains A
Decoration(A) -> Client(A): desktop focus remains A; wl_pointer enter A
```

While the pointer is over A's SSD, `pointer_surface == None` is expected, but
`PointerSceneHit::Decoration { window_id: A, .. }` remains the exact ownership
identity. The same-window focus helper already has a `NoChange` fast path; the
stress test will exercise production pointer motion dispatch and count any
unexpected desktop focus, keyboard focus, or pointer-constraint reconciliation
work. Decoration hover, button hover, pressed state, and cursor changes remain
allowed to update render/input state.

The 1,000-boundary test will alternate client and titlebar coordinates through
real pointer motion commands and assert that A remains desktop/keyboard owner,
B receives no pointer enter or desktop activation, and focus generation does
not change. A smaller event-sequence assertion will require enter/leave/enter
for A with no B events and no duplicate leave while moving inside the SSD.

## Hot-path cost findings — CONFIRMED

The current `pointer_scene_hit_uncached()`:

- clones the complete `surface_origin_cache` for each uncached hit;
- searches `renderable_surfaces` with `.position()` for every visual group root;
- then performs the intended front-to-back group/surface traversal.

The first two costs are avoidable. `VisualStackGroup` is already constructed
from root indices, so retaining `root_surface_index` removes the repeated
root search. Borrowing `&self.surface_origin_cache` during computation and
writing the cache only after the immutable computation returns removes the
clone. Straightforward group/surface traversal remains intentionally intact;
no spatial index is justified by current surface-count evidence.

## Instrumentation and measurement — CONFIRMED

When `TYPHON_POINTER_DEBUG` is enabled, bounded counters will record hit-test
calls, cache hits/misses, groups inspected, surfaces inspected, origin-cache
clones, root linear searches, and CPU duration. Counters are disabled and
allocation-free in the normal path unless debugging is enabled. Deterministic
tests will prove zero origin clones and zero root searches after the refactor,
and will feed at least 10,000 motions across client, SSD, button, and client
coordinates without a wall-clock assertion.

## Resize and render-ahead boundary — CONFIRMED by deterministic tests; native behavior UNPROVEN

Immediate resize already changes visual geometry before frame presentation and
uses render generation for geometry. The regression will move the resize
target, hit the newly moved edge before presentation, and assert the current
target is used. The configure tests will be classified against the approved
bounded window of three, latest-wins behavior; assertions that require the old
one-configure serialization will be updated only when they do not protect
final geometry, ACK ownership, or `resizing=false` correctness.

Presentation-domain damage history, buffer ages 1/2/3, `ResolvedNativeFrameScene`,
and fullscreen frame-scene authority are out of scope for behavioral changes.
They receive regression verification only.

## Rejected alternatives

- **Use render generation alone:** rejected because input-region state is
  demonstrably input-only.
- **Clear the cache at every refresh call:** rejected because it obscures the
  ownership contract and makes ordinary motion pay for unrelated refreshes.
- **Scatter `pointer_scene_hit_cache = None` assignments:** rejected because
  mutation coverage becomes unreviewable and easy to miss for topology paths.
- **Increment both generations for every input event:** rejected because
  motion, frame callbacks, presentation feedback, and title-only changes do
  not alter ordinary hit ownership.
- **Clone origin state to satisfy borrow checking:** rejected because a
  two-phase immutable compute/cache-store boundary removes the ownership
  conflict cleanly.
- **Build an R-tree/quadtree/BVH:** rejected without evidence; the existing
  ordered traversal is simple and desktop surface counts are small.
- **Treat SSD transitions as desktop focus changes:** rejected; decoration
  ownership belongs to the same `WindowId` and must not churn application
  focus.
- **Special-case X11 Shape now:** rejected because source inspection shows the
  extension is negotiated/diagnosed but not applied to Typhon's current scene
  hit test.

## Qualification status

- CONFIRMED: stale cache root cause, refresh ordering requirement, clone, and
  per-group root lookup.
- CONFIRMED: dedicated generation design, same-window fast-path behavior under
  real routing, immediate resize hit parity, and deterministic hot-path metrics.
- NATIVE-PROVEN: none at design time; native DRM qualification must not be
  claimed unless it actually runs without `TEST_ONLY`/session blockers.
- UNPROVEN: final stress counters, resize failure resolution, buffer-age
  regressions, and native titlebar/resize behavior.
