# Typhon Generalized Presentation Engine — Gate and Design

Date: 2026-09-15
Status: approved design; Geometry Presentation Engine v2 hardened and closed;
Opacity closed as the second production property

## Decision

The canonical scene identity, visual topology, immutable frame evidence, and
physical promotion foundations are implemented. Geometry presentation now uses
the first production stage of the generalized engine: transactional,
SceneNode-owned sparse tracks with exact physical revision acknowledgement.
The geometry stage is now hardened for repeated pending mutations, immutable
output-qualified physical ACK evidence, and exact transaction-member
retirement. Opacity now uses the same transactional SceneNode/revision
architecture while retaining canonical final state on `DesktopWindow`.
Lifecycle/Lamp retention and additional generalized properties remain deferred.

Future implementation stages must preserve the physical frame authority and
the compatibility boundaries described below.

## Repository baseline and evidence

The audit was refreshed against:

```text
HEAD:   5184b400a686b43fcc3ebbc3b3e77aee61bbe5f2
branch: main
status: ahead of origin/main by 29 commits
```

The worktree also contains user-owned modifications and untracked compositor
work outside this design change. They changed during the audit, so the exact
final snapshot is reported in the handoff below. None of those paths may be
staged by the migration.

Codebase Memory was refreshed for this HEAD. It reports no skipped source
files. The only partial parse range relevant to this audit is
`src/native_output/runtime/presentation_cycle.rs:155`; that line was read
directly. The graph evidence was checked for every inspected source path, with
the best-effort caveat that the graph is not a proof of source completeness.

The original pre-Phase-1A audit recorded these relevant properties. The
current-state qualification below supersedes the output-identity portion of
that historical evidence; the remaining scene and presentation-engine gates
still apply.

* The typed logical `OutputId` foundation is implemented. It is distinct from
  `OutputTransactionId`, output/backend generation, and physical DRM IDs.
* `SceneNodeId` is now a stable, stale-safe typed identity. It is monotonically
  allocated and never reused during the compositor session. Geometry
  presentation is keyed by WindowGroup `SceneNodeId`; `root_surface_id` remains
  only a frame/render/input compatibility adapter. Lifecycle presentation
  remains keyed by `WindowId` until the later retained-visual migration.
* `ActiveSceneView` remains an ordered `Vec<RenderableSurface>` with its
  existing `HashMap<u32, usize>` indexing, and now carries derived
  `surface_id -> SceneNodeId` and reverse active-index mappings. It is still a
  projection, not a replacement renderer scene or presentation authority.
* `CanonicalSceneRegistry` now provides a sidecar identity/topology layer with
  `SceneOwner`, `SceneRole`, semantic-domain assignment, explicit
  `visual_parent`, and reverse visual-child indexing. Existing protocol,
  window, stacking, geometry, and render authorities remain unchanged.
* `CompositorState` currently owns separate
  SceneNode-owned geometry `PresentationEngine`, `WindowLifecycleAnimator`,
  `presented_presentation`, `presented_lifecycle`, and
  `layout_animation_epoch` state. This is useful behavior to migrate, but it
  is not the required generalized ownership model.
* Native frame resolution already uses the selected scheduled presentation time
  when one exists and falls back to monotonic time only when no target exists.
  That contract must be preserved.
* `NativeFrameSceneSnapshot` stores presentation and lifecycle snapshots
  separately. Pageflip promotion is physically authoritative, but the worker
  still publishes the two conceptual paths separately.
* The current HEAD does not contain the requested post-pending architecture
  documents for generic scene nodes, semantic domains,
  presentation, damage, frame flow, testing, and guardrails. Current
  architecture/effects/frame-pacing documents and the deleted historical
  presentation-foundation documents were treated as context, not as a
  substitute for the missing current-baseline foundation.

## Current prerequisite status

As of the logical-output and scene-identity foundation closures, the
prerequisite status is:

* **OutputId foundation:** implemented internally. `NativeRuntime`, scene
  history, transaction ledgers, Direct Scanout identities, cursor identities,
  KMS bundle identities, and exact pageflip acknowledgements are output-bound.
* **SceneNodeId foundation:** implemented internally with stable,
  monotonically allocated, never-reused IDs.
* **Canonical identity/topology metadata:** implemented for live surfaces,
  WindowGroup nodes, server-decoration nodes, semantic domains, and
  visual-parent relationships. Existing geometry and stacking authorities are
  deliberately not duplicated.
* **ActiveScene identity projection:** implemented through derived canonical
  node references while preserving existing surface-based APIs and ordering.
* **Immutable frame-level SceneNode evidence:** implemented. Native frame
  resolution freezes the ordered surface/node pairing, server-decoration node
  metadata, and client-cursor node metadata in the existing native scene
  snapshot package.
* **Physical SceneNode promotion contract:** implemented. `NativeSceneHistory`
  promotes the exact frozen evidence on immediate/pageflip promotion and does
  not reconstruct identity from current compositor state.
* **Presentation Engine v2 geometry stage:** implemented and hardened.
  Transaction IDs, SceneNode-owned geometry tracks, exact revision ACK,
  output-qualified frame samples, scheduled sample-time evidence, and atomic
  Dwindle geometry transactions are active. Pending duplicate mutations keep
  the first start and final target; physical ACKs use only immutable promoted
  frame evidence; and transaction members retire by exact
  node/property/revision identity. Opacity is implemented as a bounded,
  canonical-window-backed second property with inherited rendering, physical
  damage, and conservative Direct Scanout qualification. Clip, retained
  lifecycle/Lamp migration, and new animation effects remain pending.

Typhon remains a single-output product. This internal `OutputId` foundation
does not add hotplug, multi-output layout, or a multi-output product model.

## Architecture after the prerequisites

The target remains a sparse overlay over canonical state:

```text
Canonical Scene
       +
Sparse Presentation Overlay
       =
Resolved Frame Scene
       -> Renderer / KMS
       -> Physical Presentation Ledger
       -> exact ACK
```

There are four authorities, with no ownership shortcuts:

1. **Canonical Scene** contains logical layout/workspace state, canonical
   geometry, visibility, hierarchy, semantic role/domain metadata,
   canonical opacity/clip where applicable, and content/resource identity.
   Animation never writes intermediate values here.
2. **Active Presentation State** is a sparse overlay indexed by canonical
   `SceneNodeId`. It owns active tracks, transaction membership, exact track
   revisions, retarget state, retained visual sources, and specialized
   deformation evidence. It does not duplicate the scene tree.
3. **Frame Presentation Sample** is immutable evidence for one `OutputId` and
   one intended presentation timestamp. It contains sampled node state,
   transaction evidence, sampling-time source, and a signature.
4. **Physically Presented State** is updated only by pageflip or immediate
   presentation promotion. It is the authority for hit testing, settlement,
   damage history, Direct Scanout recovery, stale-frame rejection, and visual
   handoff.

The v1 generic properties are limited to one coherent axis-aligned spatial
contract, opacity, and clip. Translation and non-uniform scaling are enough for
the initial migration. No generic blur, shadow, material, color grading,
arbitrary effect parameter, or arbitrary 3D transform is introduced.

Parent presentation state is inherited by descendants exactly once. A node
may explicitly override an inherited property. Client content, subsurfaces,
server-side decorations, and root-owned effects resolve from the same sample;
they do not each apply a root transform independently.

## Foundation work that must land first

### Phase 1 — canonical output and scene foundations

1. **Completed — OutputId foundation.** A real typed `OutputId` and the
   existing output/backend generation are threaded through output projection,
   native frame planning, physical ledger entries, damage history, Direct
   Scanout diagnostics, and ACK qualification. The current one-output behavior
   remains a compatibility product policy, not an implicit identity default.
2. **Completed — SceneNodeId foundation.** A typed, monotonic,
   stale-safe allocator is owned by compositor scene state. IDs are never
   reused during the compositor session, so they cannot alias a later object.
3. **Completed — canonical identity/topology metadata.** The registry tracks
   scene source/owner, semantic role/domain assignment, stable WindowGroup and
   server-decoration identities, and validated visual-parent relationships for
   surfaces. Existing geometry, visibility, opacity/clip, stacking, damage,
   buffer, and effect authorities remain separate.
4. **Completed — ActiveScene identity projection.** `ActiveSceneView` retains
   its existing surface-based APIs and ordering while exposing derived
   canonical node mappings without changing layout, configure, X11 geometry,
   or buffer ownership semantics.
5. **Completed — immutable frame-level SceneNode evidence and physical
   promotion.** Native frame snapshots carry exact canonical node identity for
   surfaces, server decorations, and client cursors; `NativeSceneHistory`
   remains the sole physical promotion authority.

### Phase 2 — frame and physical contracts

1. Add typed `PresentationTransactionId` and a transaction membership record.
   A semantic compositor operation publishes all affected nodes atomically.
   Dwindle/reflow must use one transaction and one start time without
   mutating layout every animation frame.
2. Generalize the existing exact per-track `TransitionId` behavior into a
   node/property revision. Retargeting always creates a newer revision; an old
   frame may present but cannot retire the newer revision.
3. Add the sparse presentation overlay and immutable per-output frame sample
   interfaces. Sampling must take
   `(output_id, target_presentation_time, output_scene_projection)` and record
   whether the timestamp came from the scheduled target or an allowed
   fallback.
4. Add one frame-level physical presentation snapshot/ledger with immutable
   transition evidence. Pageflip and immediate paths promote through this
   common authority, qualify ACKs by output and generation, and reject stale,
   dropped, rejected, and superseded frames.
5. Define inherited transform inversion for input. Hit testing uses only the
   last physically presented state; retained lifecycle visuals are explicitly
   non-interactive, and non-invertible or near-zero transforms suppress
   presentation hit testing.
6. Make effects consume resolved presentation geometry while retaining the
   existing effects DAG. Make damage compare physical history against the new
   resolved footprint, with bounded conservative handling for malformed state.
   Direct Scanout keeps detailed blockers and remains blocked until the
   identity state is physically presented.
7. Add the required timing, transaction, hierarchy, input, ACK, damage,
   Direct Scanout, and two-logical-output tests before visual migration.

These phases are prerequisite work, not permission to create a temporary
presentation graph. They must land with zero visible animation behavior change.

## Staged migration once the gate is open

The migration then proceeds in independently reviewable commits:

* **Stage A — common evidence:** adapt existing geometry and lifecycle
  snapshots to the common frame evidence and transaction contracts while the
  old behavior remains active.
* **Stage B — geometry:** move programmatic move, resize, Dwindle reflow,
  maximize, and fullscreen to `SceneNodeId` sparse presentation state. Preserve
  the current easing, analytical spring sampling, velocity-preserving
  retargeting, input mapping, physical ACK, and Direct Scanout behavior.
* **Stage C — physical publication:** replace the conceptual split between
  `publish_presented_presentation` and `publish_presented_lifecycle` with one
  pageflip-promoted snapshot path. Keep lifecycle replacement/fallback rules
  until parity tests pass.
* **Stage D — retention/Lamp:** put minimize/restore and retained lifecycle
  sources under generalized transactions and physical ACK. Keep Lamp's
  frozen visual groups, anchors, portal/sink geometry, funnel deformation,
  shader behavior, conservative footprint, reversal, renderer evidence, and
  Dock anchor protocol as a specialized renderer-facing primitive.
* **Stage E — adapters:** remove duplicate root-surface presentation ownership,
  duplicate lifecycle ACK infrastructure, and duplicate snapshot fields only
  after geometry and Lamp regression suites pass. Keep compatibility
  re-exports when they avoid unrelated churn.

No new visible animation effect is part of this migration. Existing animation
control (`AnimationSlot`, stable effect IDs, persisted configuration, runtime
capabilities, speed scaling, Eclipse Settings, and Dock anchors) remains the
public semantic API and catalog authority.

## Proposed module boundary

After Phase 1/2, a focused `src/presentation/` module may contain:

```text
mod.rs          public internal wiring and compatibility exports
time.rs         absolute animation/presentation time
ids.rs          transaction and revision IDs; core owns OutputId and SceneNodeId
curve.rs        easing, analytical spring, typed interpolation
state.rs        resolved properties and coordinate-space contracts
transaction.rs atomic semantic membership/publication
engine.rs       sparse active state, start, retarget, cancel, sample, ACK
frame.rs        immutable per-output samples and evidence/signatures
retention.rs    compositor-owned retained visual sources
```

The names are subordinate to the existing source-layout policy. No monolithic
animation engine is planned, and no dynamic `Box<dyn AnimatableProperty>` hot
path is introduced.

## Preserved invariants and acceptance gates

Before each migration commit, add the failing contract test first, implement
the minimum change, run focused tests, inspect ownership in the diff, and
commit. At minimum the suites must prove:

* canonical geometry/layout/workspace/configure state stays unchanged while a
  frame is sampled;
* transaction members activate atomically;
* arbitrary-time deterministic sampling and analytical spring behavior remain
  intact, including velocity-preserving retargeting and finite-value rejection;
* mathematical settlement does not retire a track before matching physical
  presentation; dropped/rejected/stale/output-generation-mismatched frames do
  not ACK;
* parent transforms affect descendants once, and inverse input mapping uses
  physical presentation rather than a future sample;
* retention and Lamp regressions remain intact, including fallback, teardown,
  reversal, missing/replaced roots, XWayland, anchors, off-output settlement,
  and complete footprints;
* physical-history damage and detailed Direct Scanout blockers remain
  conservative, with identity recovery only after physical promotion;
* two logical outputs sample and ACK independently;
* an empty overlay preserves the existing no-animation fast path, cursor-only
  hardware-plane behavior, client-buffer reuse, effects topology, and
  fullscreen Direct Scanout.

Observability is bounded and includes transaction/track starts, retargets,
cancellations, mathematical settlements, physical/stale ACKs, retained source
counts, per-output obligations, sample timestamps and timestamp source,
composition blockers, and sample/preparation timing.

Hardware qualification is required after the physical path changes. Unit tests
alone are not evidence of 1920x1080@165 Hz behavior, Direct Scanout recovery,
pageflip timing, or fullscreen gaming performance.

## Alternatives rejected

* **Big-bang rewrite:** rejected because it would mix ownership, animation,
  lifecycle, KMS, and behavior changes and would discard the current stale-ACK
  and Lamp semantics.
* **Adapter-only generalized engine before the foundations:** rejected because
  it would require mapping the new engine to `u32` root/window identities and
  would create the parallel identity layer forbidden by the prerequisite gate.
* **Per-frame layout or numerical integration:** rejected because canonical
  layout must remain immediate and deterministic absolute-time sampling and
  analytical springs already provide the required semantics.

## Current result

The first production geometry stage is complete and closed, and Opacity is
implemented as the second production property:

```text
OutputId foundation                         implemented
SceneNodeId foundation                      implemented
canonical visual topology                   implemented
immutable frame SceneNode evidence          implemented
physical SceneNode promotion contract       implemented

Presentation Engine v2 transaction IDs      implemented
Geometry SceneNode ownership                implemented
Geometry transactions                        implemented
exact revision physical ACK                 implemented
Dwindle atomic geometry transactions        implemented
duplicate batch semantics                   hardened
output-qualified immutable ACK               hardened
exact transaction-member retirement          hardened
geometry source-layout closure               complete
synthetic presentation OutputId defaults     removed
effective geometry no-op elision             implemented
Geometry v2                                  closed

Opacity                                      closed
Clip                                         pending
retained lifecycle/Lamp migration            pending
new animation effects                        pending
```

The implementation preserves the existing geometry curves, analytical spring
sampling, input handoff, effects/SSD adapters, Direct Scanout conservatism,
and `NativeSceneHistory` physical authority. It does not claim completion of
the generalized animation roadmap or lifecycle/Lamp migration.
