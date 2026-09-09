# Typhon Presentation Animation Foundation Design

## Snapshot and evidence boundary

This design targets the current `/home/agony/GitHub/Typhon` checkout at `main`,
including the pre-existing working-tree changes. The codebase-memory index is
ready at the current watched generation; the only recorded partial parses are
at `src/native_output/runtime/cycle_dispatch.rs:1333` and
`src/native_output/runtime/presentation_cycle.rs:154`, so those ranges remain
source-read fallback boundaries.

The current source includes the previously audited performance/resource work:

`EffectResourcePool` uses consumable eviction IDs, presentation traces export
incrementally, and SHM snapshots use copy-on-write shared pixel storage. The
working tree also contains unrelated compositor/protocol changes; this design
does not reinterpret or revert them.

## Current authority graph

```text
Wayland/X11 input and protocol actions
        |
        v
CompositorState + DesktopWindow + WorkspaceManager + TiledLayoutManager
        |  canonical membership, mode, logical geometry, stacking
        v
surface_placements / RenderableSurface committed content
        |  compositor visual preview only for active resize/mode transitions
        v
ActiveSceneView (stable ordered scene view)
        |
        v
ResolvedNativeFrameScene -> native damage/history -> EGL renderer or scanout
        |                                             |
        +--> visual stack groups / SSD render plan   +--> GPU/KMS ownership

NativeFrameScheduler -> PresentationTarget.presentation_time
```

Canonical geometry remains the `WindowGeometry` produced by WM/layout and the
committed root `SurfacePlacement`/size owned by compositor state. The existing
`ToplevelVisualGeometry` remains a one-mutation compositor preview for resize
and mode transitions; it is not repurposed as animation storage. `ActiveSceneView`
continues to own an ordered snapshot of visible surfaces and is rebuilt only
when scene membership/order/content requires it.

The new presentation layer sits between the resolved canonical scene and the
frame renderer. It owns only temporary transition state and produces an
immutable frame-local sample. It never calls a layout solve, sends a configure,
updates `ToplevelVisualGeometry`, or mutates `RenderableSurface` placement.

## Selected approach

The implementation uses a small, top-level `presentation_animation` module with
typed value objects and no layout abstraction. A `PresentationAnimator` stores
per-root-window transitions keyed by the existing root surface ID. A transition
contains immutable start state, target state, curve, creation time, and a
settlement policy. Sampling accepts an absolute `AnimationTime` and returns a
`PresentationWindowSample`; it does not mutate the transition. This keeps
render-ahead and multiple future timestamps deterministic.

Two curve families are supported:

1. bounded easing with explicit duration and a typed easing curve;
2. an analytical damped spring with mass fixed at one, supporting
   under-damped, critical, and over-damped configurations.

Spring evaluation uses the closed-form position and velocity for the absolute
elapsed time. Retargeting samples the old transition at the retarget timestamp,
then creates the new transition from that position and velocity. It never resets
velocity merely because the destination changed. Displacement and velocity
epsilon settle the transition; timestamps past settlement return the exact
target and zero velocity.

The integration seam is `PresentationSceneSample`: it contains the sample
timestamp, per-window samples, a `PresentationGroupTransform` for each active
root, and a bounded geometry signature. It intentionally does not own an
independent damage authority. Renderer-facing surfaces, SSD instances, and
effect regions are derived from the same sample; native scene history compares
the previously presented scene with the newly sampled scene. Stable
buffer/resource handles remain shared; only small placement/geometry records
are copied.

## Timing, scheduling, and lifecycle

The native scheduler remains the only frame-time authority. Once a
`PresentationTarget` is selected, its `presentation_time` is converted to
`AnimationTime` and passed to the animator. No frame counter, timer, sleep, or
animation thread advances state. Visible stored transitions remain frame work
until their exact `TransitionId` and final sample are acknowledged by a
physically presented frame. A mathematically settled transition therefore
still emits its exact target sample. Hidden roots do not request frames. If
animations are disabled, targets are applied immediately and no transition
remains active.

Transitions are created when a programmatic compositor geometry mutation changes
the canonical root target. Interactive move/resize owns geometry directly and
bypasses the animator. When the interaction ends, a transition is not created if
the sampled presentation rectangle already equals the canonical target.

The first engine lifecycle covers floating changes, layout reflow, maximize,
restore, fullscreen, fullscreen exit, and surviving windows in an atomic layout
batch. Window open/close, workspace retention, destroyed-client snapshots,
scrolling layouts, camera transforms, and configuration DSLs remain out of v1.

## Rendering, effects, and input

Client roots, their ordinary subsurface trees, SSD decorations, and
policy-owned popup groups are represented through the shared visual-stack
policy. A root group receives one sampled transform; canonical
`RenderableSurface` and decoration state are not mutated. Effect bounds and
consumer geometry use the same frame-local transform, while graph topology and
stable resources are reused when only the transform changed.

Presentation damage is based on the previous geometry that was physically
presented and the new sampled geometry, expanded by the effect footprint and
rounded outward to integer output rectangles. A rendered or abandoned sample is
not promoted to the previous-presented authority. Pageflip/presentation
completion promotes the exact sample token that became visible. Existing
buffer-age repair remains authoritative for slot history.

Input uses the last physically presented `PresentationFrameSnapshot`, not a
fresh monotonic sample. Hit testing inverse-maps the presented group to
canonical surface-local space, so render-ahead frames cannot move input ahead
of the displayed projection. SSD hit regions use the same canonical geometry
and inverse mapping. Window stacking and pointer/grab ownership are unchanged;
active move/resize remains pointer-immediate.

Visible pending transitions and any non-identity physically presented
projection add the typed `AnimationTransform` Direct Scanout blocker. Existing
capability, TEST_ONLY, device identity, and ownership checks remain unchanged.
Only a successful immediate/pageflip promotion publishes the projection and
performs exact transition acknowledgement; rejected, abandoned, or stale
frames cannot become input authority.

## Performance foundation

The confirmed low-risk fixes are included before animation sampling:

* Replace effect eviction history with a consumable pending-ID queue plus a
  scalar cumulative eviction count. GL checkout drains pending IDs exactly once.
* Export presentation trace incrementally: unchanged rings do no work, ordinary
  appends append only new JSONL, and ring overwrite/drop events trigger a full
  bounded rewrite. There is no worker or unbounded queue.
* Store compositor-owned SHM pixels in `Arc<[u32]>`. Cloning a committed buffer,
  renderable surface, or scene view is O(1) in pixel payload size. A write to a
  published snapshot uses copy-on-write, retaining early client-buffer release
  and historical immutability.

Uniform-location caching and consumer-driven effect pruning remain bounded
follow-up implementations only where their existing dependency proof is
explicit. Metadata reconstruction is instrumented first; no speculative stable
topology cache is added without measurements.

## Observability

The animation core exposes bounded transition-start, retarget, cancellation,
acknowledgement, stale-acknowledgement, and sampled-window counters. The EGL
scene cache carries a separate presentation geometry signature, so geometry
rebuilds are observable without pretending that content generations changed.
Existing CPU preparation, scene/effect preparation, GPU completion, KMS queue
residence, submit/ioctl, pageflip, and deadline fields remain separate. No
single 165 Hz microsecond budget is invented; 6.06 ms is only the physical
interval used by hardware qualification.

## Deterministic test contract

The pure module tests use explicit nanosecond timestamps and cover easing bounds,
analytical spring position/velocity for all damping regimes, arbitrary-time and
repeated sampling, settlement, long gaps, retarget position/velocity
continuity, translation/resize/subpixel geometry, finite-value validation, and
outward damage rounding, plus fractional forward/inverse group transforms and
stale physical acknowledgements after retargeting. Authority tests assert that
sampling does not change canonical geometry, layout generations, XDG configure
counts, scene-view rebuild counts, or SHM payload identity. Scene-history and
runtime promotion tests cover ready/submitted/presented ownership and the
physically presented projection. Input and scanout tests cover inverse mapping,
immediate interactive resize/move, popup/SSD group motion, the animation
scanout blocker, and recovery after physical settlement.

Hardware qualification is separate: run before/after at 165 Hz on the available
DRM device, record distributions rather than averages only, and report CPU
preparation, GPU, KMS, submit, actual pageflip, missed-deadline, wake, and
SHM-copy evidence independently.

## Explicit drift and non-goals

Historical documents describe the current compositor before the latest explicit
sync, scene-view, resize-preview, and native presentation work. Where those
documents imply a mutable per-frame visual target or a CPU-composition renderer,
the current source wins: `ActiveSceneView`, `ResolvedNativeFrameScene`, the
existing preview assignment, and native EGL/KMS path are the authorities.

This design does not add a second scene tree, frame clock, layout engine, timer,
thread, retained destroyed-client content, or public animation scripting API.
