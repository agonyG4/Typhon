# Typhon Generalized Presentation Engine — Gate and Design

Date: 2026-09-15
Status: approved design; production implementation blocked by the prerequisite gate

## Decision

Do not implement the generalized Presentation Engine in the current checkout.
The canonical scene foundations required by the task are not present. Adding
presentation primitives now would require inventing a second scene identity or
an interim scene graph, which is explicitly prohibited.

The implementation may begin only after the Phase 1 and Phase 2 foundation work
described below has landed and is present at the new baseline.

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

The current checkout has these relevant properties:

* There is no typed `OutputId`. `PhysicalOutputId(u8)` is a private
  single-output membership helper, and `OutputTransactionId` identifies a KMS
  submission rather than a physical output. Output generation is currently a
  scalar in the native-output path.
* There is no stable or generational `SceneNodeId`. Presentation geometry is
  keyed by `root_surface_id: u32`; lifecycle presentation is keyed by
  `WindowId`; protocol surface and subsurface relationships use `u32` surface
  IDs.
* `ActiveSceneView` is an ordered `Vec<RenderableSurface>` with `HashMap<u32,
  usize>` indexing. It is not a canonical generic scene containing stable node
  metadata.
* The existing hierarchy is protocol/window-specific. `parent_surface_id`,
  `WindowVisualGroup`, and `VisualStackGroup` do not provide a generic
  `SceneNodeId` hierarchy with inherited presentation resolution across client
  content, subsurfaces, decorations, and compositor-owned effects.
* `CompositorState` currently owns separate
  `PresentationAnimator`, `WindowLifecycleAnimator`,
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
  documents for `OutputId`, generic scene nodes, semantic domains,
  presentation, damage, frame flow, testing, and guardrails. Current
  architecture/effects/frame-pacing documents and the deleted historical
  presentation-foundation documents were treated as context, not as a
  substitute for the missing current-baseline foundation.

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

1. Add a real typed `OutputId` and an output generation/epoch. Thread it
   through output projection, native frame planning, physical ledger entries,
   damage history, Direct Scanout diagnostics, and ACK qualification. Keep the
   current one-output behavior as a cheap adapter, but do not encode output
   zero in new APIs.
2. Add a stable/generational `SceneNodeId` allocator owned by the canonical
   scene. Stale node generations must be rejected rather than aliasing a new
   node.
3. Make canonical scene metadata explicit: node kind, semantic role/domain,
   content/resource identity, canonical geometry, visibility, opacity/clip,
   and parent/group relationship. The scene must represent the window root,
   client subtree, subsurfaces, decorations, and compositor-owned effect
   participation without requiring an animation-specific identity.
4. Make the canonical scene the source for the active-scene projection. Adapt
   `ActiveSceneView` and frame projections without changing layout behavior,
   configure behavior, X11 geometry, or existing buffer ownership semantics.
5. Add foundation tests for node generation reuse, parent/child ownership,
   output-qualified projection, semantic metadata, and no-animation parity.

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
ids.rs          OutputId, SceneNodeId, transaction and revision IDs
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

This design records the migration target and the exact reason implementation
must stop at the current HEAD. No production source, animation behavior,
renderer path, scheduler, input path, damage path, or KMS ownership was changed
by this task.
