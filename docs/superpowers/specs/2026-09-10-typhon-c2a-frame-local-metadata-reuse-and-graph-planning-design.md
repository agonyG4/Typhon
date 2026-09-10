# Typhon C2a: Frame-Local Metadata Reuse and Graph Planning

## Scope

C2a addresses only metadata work that is provably redundant within one exact
resolved presentation frame, plus the render-graph compiler's diagnostic
peak-live statistic. It does not add a persistent scene cache, effect-graph
topology cache, shared scene-history ownership, or any change to presentation
animation policy, damage authority, resource realization, B2 execution demand,
or Direct Scanout.

The current source confirms two safe transformations:

* `ResolvedNativeFrameScene::from_server_at()` already owns the exact-frame
  surfaces and decorations, but its stored `NativeSceneSnapshot` is incomplete.
  `snapshot()` clones it and reconstructs popup IDs, external overlays,
  visibility, and effect metadata for each consumer. `scene_identity_signature()`
  then invokes that path again before hashing.
* `compile_frame_execution_plan()` calculates `peak_live_intermediates` by
  scanning every texture for every post-fusion pass even though each texture's
  inclusive `first_use..=last_use` interval is already available.

The remaining scene/effect reconstruction is measurement-only in C2a. The
report will classify whether a later C2b should pursue validated-program
metadata reuse, effect-frame topology reuse, or native-scene topology
projection reuse.

## Frame-local snapshot architecture

`ResolvedNativeFrameScene` will contain a complete immutable snapshot and an
exact-frame `scene_identity_signature` computed by `from_server_at()` after:

1. canonical surfaces, presentation targets, presentation sample, and
   presentation-transformed decorations/surfaces are resolved;
2. resolved effects are computed for that same presentation sample;
3. the base native snapshot is built once; and
4. popup IDs, external overlay IDs, visibility signature, effect damage, and
   effect identity signature are installed before the frame scene is published.

Visibility signature mixing remains byte-for-byte equivalent to the existing
metrics-based implementation. Effect damage remains the union of visible
resolved effect instance regions, while effect identity remains the resolved
effect scene signature; neither is replaced with compiled graph damage.

The consumer API will make ownership explicit:

* `snapshot_ref()` borrows the finalized exact-frame snapshot and is used by
  damage comparison and other same-frame consumers.
* `snapshot_owned()` (or an equivalent explicit boundary) performs one clone
  only when a frozen snapshot must outlive the resolved-frame borrow, notably
  `NativeFrameSceneSnapshot` history ownership.
* `scene_identity_signature()` returns the cached value in O(1), with no
  snapshot clone, metadata finalization, or surface/decorations hash walk.

The ready/submitted/presented history remains independently owned and
immutable. It never points into mutable compositor state. Previous presented
metadata remains frozen while current-frame damage borrows only the current
resolved frame's finalized snapshot.

Atomic and compatibility paths consume this same API. They do not receive a
separate snapshot construction rule, and no scene identity is substituted
with `scene_render_generation`.

## Graph lifetime statistic

After `fuse_compatible_local_stages()` has rewritten pass and texture
lifetime metadata, each intermediate texture with valid inclusive bounds is
represented by two events:

```text
delta[first_use - 1] += 1
delta[last_use] -= 1       (when last_use < pass_count)
```

The one-based `GraphPassId` is converted to zero-based pass indices. A sweep
over the bounded `MAX_GRAPH_PASSES` domain accumulates live textures and takes
the maximum. A one-pass lifetime contributes exactly once. Removed/fused
intermediates with cleared lifetimes contribute nothing. Malformed metadata is
handled conservatively without a production panic; debug assertions retain
the invariant signal.

The production result remains the same `RenderGraphCompileStats` field, while
tests retain a brute-force reference for deterministic equality checks,
including post-fusion graphs. The helper will also expose bounded test-only
work accounting sufficient to distinguish `P × T` candidate checks from
`T` event insertions plus `P` sweep steps.

## Observability and qualification

Instrumentation will use bounded counters and the existing opt-in native perf
infrastructure for elapsed CPU timing where it already applies. Disabled perf
logging will not pay for per-frame `Instant::now()` calls. Counters cover
snapshot builds, owned clones, identity computations, surface projection
visits, visual-group builds, effect graph/instance compiles, node visits,
lookup-map construction, and lifetime events/sweep steps. Allocation byte
claims will not be fabricated; allocator tooling will be reported as
unavailable if it cannot be run.

Deterministic fixtures and representative workloads will cover one, 16, 100+,
and optionally 256 surfaces; deep popup/subsurface trees; active presentation
animation at distinct timestamps; one and many effects; uniform-only
parameter animation; and changing effect geometry. The required evidence will
separate same-frame duplicate work from legitimate per-sample work and will
report exact counters plus median/p95 CPU phases when native timing is
available. Hardware timing is explicitly omitted when the local environment
cannot execute native output.

## Regression contract

The tests will establish and preserve:

* finalized snapshots contain popup IDs, overlays, visibility, effect damage,
  and effect identity immediately after resolution;
* the new borrowed/owned snapshot semantics equal the old finalized snapshot
  semantics for ordinary surfaces, popups, overlays, effects, fullscreen
  filtering, and presentation-transformed geometry;
* cached and pre-change scene identity signatures are identical;
* repeated signature reads perform no additional finalization, clone, or
  surface hash walk;
* resolved-scene damage compares the borrowed current snapshot and preserves
  damage output;
* frozen history remains unchanged after the current scene advances;
* two animation timestamps produce distinct exact snapshots when transformed
  geometry differs despite stable canonical generation;
* interval sweep and brute-force peak-live definitions match across fixture
  lifetimes and after compatible-stage fusion; and
* graph work accounting is linear in pass count plus texture count.

The implementation will follow RED → GREEN → REFACTOR. The first RED test
will record the current duplicate snapshot finalization/hash behavior before
the new API is migrated. Focused tests will run after each migration, then
the full required Rust/source-layout verification will run from the existing
Cargo target directory.

## C2b decision gate

The final report will rank, with measurements, the fraction of structurally
reusable effect frames, remaining compiler CPU/work-unit cost, invalidating
fields, and whether key construction/materialization would outweigh reuse.
Uniform-only parameter changes must not be treated as topology changes, while
future Footprint/Structure parameters must either enter a structural key or
bypass caching. Presentation-transformed bounds, decoration geometry, effect
regions, current damage, content generations, and commit evidence remain
frame-local unless a later design proves their invalidation rules.

No persistent graph or scene cache will be implemented in C2a without that
evidence and a complete key, and the default expected outcome is a separate
C2b recommendation rather than speculative cache state.
