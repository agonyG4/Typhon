# Astrea Lamp v1 Design

## Goal

Implement Astrea Lamp as the first lifecycle presentation animation in Typhon. A
semantic minimize or restore remains logically immediate, while retained live
window surfaces are presented through a reversible, direction-independent,
non-linear mesh deformation toward the frozen Eclipse Dock anchor. Lifecycle
ownership retires only after the exact endpoint frame has been physically
presented and acknowledged.

The existing Animation Control Plane v1.1, Dock anchor protocol, canonical
window/layout state, and `PresentationAnimator` remain separate authorities.
`minimize.lamp` becomes available only after the runtime and focused
regressions are complete.

## Architecture

### Lifecycle authority

Add `src/window_lifecycle_animation.rs` and export it from the Typhon crate.
The module owns:

- `LifecycleTransitionId`, allocated independently from geometry
  `TransitionId`.
- `WindowLifecycleAnimator`, keyed by exact `WindowId` and retaining bounded
  transitions for minimize and restore.
- `LifecycleSceneSample` and `LampWindowSample`, which contain only frame-local
  metadata: identity, frozen canonical/source rectangles, frozen Dock anchor,
  normalized progress, opacity, and endpoint state.
- `LifecycleFrameSnapshot`, a metadata-only pageflip ledger with a stable
  signature and a `blocks_direct_scanout` classification.
- CPU reference math for the Astrea warp and late opacity collapse.

The animator samples a linear progress timeline using the effective speed read
at semantic-operation start. Minimize starts at `t = 0` and targets `t = 1`;
restore starts at `t = 1` and targets `t = 0`. Reversal samples the active
transition at the reversal timestamp, allocates a fresh lifecycle identity,
and uses `base_duration * abs(target - current)` for the new segment. A
mathematically settled transition remains active until a matching endpoint
snapshot is published from native scene history.

The compositor keeps the last pageflip-confirmed lifecycle snapshot separately
from active logical lifecycle state. Cancelling logical state does not erase
the physical ledger; the next physically presented replacement snapshot clears
it. Teardown is the one destructive identity-removal path.

### Semantic operation integration

The existing window action methods remain the logical authority. Minimize
captures the physical source and Dock anchor before calling the existing
logical mutation. Restore samples any active lifecycle transition before
performing the existing logical restore, then starts or reverses the lifecycle
presentation using the restored canonical visual group as the endpoint when
there is no existing transition.

Source selection is pageflip-confirmed `PresentedWindowGeometry`, then current
presented visual geometry, then current visual/canonical root geometry. When a
geometry transition is active, the captured canonical-to-presented affine is
frozen as the Lamp source basis and the old geometry animator is cancelled
using its existing cancellation semantics. The last confirmed geometry ledger
is preserved.

No lifecycle operation reinserts a minimized root into canonical scene state.
Minimize continues to move the root-owned surfaces into
`WindowState::minimized_surfaces`; restore continues to move them back. During
an active restore Lamp, frame resolution filters that root from canonical
surfaces, decorations, and window-owned effect instances. Pointer hit testing
does not use the warped mesh and remains suppressed for the restoring root
until the endpoint is acknowledged.

### Native frame data and physical settlement

Extend `ResolvedNativeFrameScene` with lifecycle metadata and the retained
surface/decorations needed to draw active Lamp windows. The lifecycle retained
surfaces are collected from the existing `renderable_surfaces` or
`minimized_surfaces` representation according to the active direction; they
are never added to canonical scene authority.

Extend `NativeFrameSceneSnapshot` with `LifecycleFrameSnapshot`. Both immediate
promotion and pageflip promotion publish this snapshot through the existing
native scene history pipeline. Only an exact, renderer-consumed endpoint
sample with the current `LifecycleTransitionId` acknowledges and retires a
transition. A stale snapshot can remain in history but cannot settle a newer
reversed transition.

### Renderer path

Extend `EglSceneDrawRequest` with lifecycle data. The EGL renderer maintains a
separate bounded lifecycle mesh cache and VBO/program state; ordinary progress
changes update uniforms and do not alter the canonical scene-cache key or
re-upload topology. Mesh topology is rebuilt only when a lifecycle visual group
or its source geometry materially changes.

Each retained client/subsurface rectangle and each SSD primitive is tessellated
in one root visual-group coordinate system. The Lamp vertex shader maps the
canonical vertex through the frozen affine source basis and then applies the
Astrea warp. The existing texture/resource sampling path supplies live surface
content and decoration resources. Lamp draw commands have empty opaque-region
metadata.

The dedicated shader receives output size, framebuffer origin, canonical/source
rectangles, Dock anchor, progress, opacity, and pull constant (`K = 2.5`). It
uses the same finite CPU-reference formulas. Shader initialization follows the
normal EGL lifecycle; failure causes the semantic operation to remain
immediate and releases any suppression or lifecycle state.

Both LegacyScene and EffectGraph execution paths are ordered explicitly as:

1. canonical application scene and effect composition;
2. Lamp lifecycle overlay;
3. external Dock/layer-shell overlays and cursor.

The Dock is never included in the Lamp mesh, and the cursor remains above it.
Lifecycle surfaces join the existing lazy resource-consumer union, so only
actually drawn retained surfaces are realized. Live minimized commits update
the same retained `RenderableSurface` resources consumed by Lamp.

### Damage, scheduler, and scanout

The compositor reports lifecycle work to `has_unowned_frame_work()` while an
active transition needs a frame. Lifecycle damage is the clipped output-space
union of each source and frozen anchor rectangle, with multiple windows
unioned through existing damage infrastructure. Exact endpoint frames remain
pending until physically published.

Direct Scanout gains a stable `lifecycle_animation` rejection reason. It is
blocked by a visible active transition or by a last physically presented
non-identity/visible lifecycle snapshot. Exact minimize (`t = 1`, opacity zero)
and restore (`t = 0`, identity) snapshots are non-blocking after matching ACK,
so no unrelated future frame is required to clear the blocker.

## Warp mathematics

For a valid source visual-group rectangle `S`, anchor rectangle `A`, canonical
vertex `p`, and timeline progress `t`:

1. Map `p` through the frozen canonical-to-presented affine to obtain the
   physical source point.
2. Compute `n = normalize(center(A) - center(S))`. If the center distance is
   effectively zero, use `s = 0.5` for every vertex.
3. Project all four source corners onto `n` and normalize the point projection
   into `[0, 1]`, where `s = 1` is nearest the Dock.
4. Compute `exponent = 1 + 2.5 * s` and
   `g = 1 - (1 - t)^exponent`.
5. Map the vertex's normalized `(u, v)` in `S` into `A`, and linearly interpolate
   from the source point to that anchor point by `g`.

All inputs are validated and every intermediate is finite. The reference tests
cover all anchor directions, diagonal anchors, endpoint and intermediate
progress values, tiny and large valid rectangles, small anchors, and nearly
coincident centers. Tests prove identity at `t = 0`, exact anchor mapping at
`t = 1`, bounded/monotonic pull, deterministic output, and reversible sampling.

Opacity uses the same timeline:

```text
1 - smoothstep(0.90, 1.00, t)
```

It is full at restore endpoint `t = 0` and zero at minimize endpoint `t = 1`.

## Mesh policy

Use a bounded adaptive grid with a target cell size near 48 physical pixels.
Large groups receive more subdivisions, tiny groups receive the minimum useful
grid, and total rows, columns, and vertices are capped globally. If concurrent
windows exceed the preferred budget, grids are coarsened deterministically.
The mesh cache exposes test-only upload/rebuild counters proving that progress
sampling does not cause one VBO upload per frame.

## Failure and teardown

An invalid or missing Dock anchor, invalid source geometry, or unavailable Lamp
shader produces the existing immediate logical minimize/restore behavior. No
synthetic anchor is generated. Restore suppression, active lifecycle metadata,
and temporary mesh/resource ownership are cleared on fallback.

Window teardown cancels active lifecycle state, removes suppression, damages the
last lifecycle footprint, and removes only the dead root's physical lifecycle
identity. It never dereferences removed surfaces or leaks the ordinary
`minimized_surfaces` state.

## Verification scope

Add focused Typhon tests for pure math, transition reversal and stale ACKs,
physical source selection, frozen anchors, missing anchors, endpoint
settlement, suppression, scheduler work, direct scanout diagnostics, retained
surface generations, visual-group sharing, mesh upload reuse, damage bounds,
renderer pass ordering, tiled/maximized/fullscreen behavior, managed X11 parity,
teardown, disabled animations, and endpoint changes during reversal.

The final catalog change is last: `AnimationEffect::MinimizeLamp::is_available()`
returns true, causing the existing Astrea requested/effective slots to resolve
to `minimize.lamp`. KDE and macOS retain `none` for lifecycle slots. Eclipse
production code remains unchanged unless an existing fixture explicitly asserts
the old planned catalog state.

No Scale, Glide, Squash, workspace, open, close, generic plugin framework,
additional settings model, Dock protocol change, timer thread, animation worker,
per-frame configuration parsing, CPU full-window capture, or per-frame mesh
topology upload is part of this design.
