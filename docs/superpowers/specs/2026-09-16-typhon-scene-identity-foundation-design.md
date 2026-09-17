# Typhon Scene Identity and Canonical Visual Topology Foundation

Date: 2026-09-16
Status: implemented and verified on the current repository baseline

## Decision

Add a small canonical scene-identity layer as metadata over the existing
compositor state. It will own stable `SceneNodeId` values, semantic metadata,
and visual-parent topology. Existing protocol, window, layout, stacking,
rendering, animation, presentation, input, KMS, and Direct Scanout authorities
remain unchanged.

The registry is a lifecycle/topology service, not a replacement renderer scene
tree and not a second physical-presentation authority.

## Repository baseline

Implementation starts from:

```text
HEAD: 14e955dd (Phase 2A frame-identity implementation and tests)
Codebase Memory: home-agony-GitHub-Typhon, indexed and ready
Graph: 27,994 nodes / 201,333 edges (current index status)
Source-layout baseline: 53 diagnostics
```

The worktree contains unrelated concurrent viewport/subsurface validation
edits. Their hunks do not overlap the scene integration seams selected below;
they will not be staged or modified by this work.

## Identity model

`SceneNodeId` is a separate typed namespace from `WindowId`, compositor
surface IDs, X11 IDs, `OutputId`, render IDs, and backend generations. It is a
copyable, hashable, orderable wrapper around `NonZeroU64`.

`SceneNodeIdAllocator` starts at one, advances with checked arithmetic, and
returns exhaustion instead of wrapping or recycling. IDs are never reused
during the compositor session. No generation bits, arena slots, free list, or
compositor state are stored in the ID.

## Canonical registry

The focused `src/compositor/scene/` module contains:

* `SceneOwner`, which describes lifetime authority (`Surface(u32)` or
  `Window(WindowId)`), independently of visual hierarchy;
* `SceneRole`, which describes generic visual meaning;
* `SceneDomain` and `SceneDomainAssignment`, supporting explicit domains and
  bounded inherited resolution with a fallback;
* `SceneNodeMetadata`, containing the node ID, owner, role, domain assignment,
  and optional `visual_parent`;
* `CanonicalSceneRegistry`, which maps stable source keys to metadata and
  maintains the reverse parent-to-children index.

The registry source key is distinct from owner so a window group and its
server decoration can share a `WindowId` owner without aliasing. Surface,
window-group, and decoration registrations are unique and duplicate source
registration is rejected.

`set_visual_parent` validates the complete proposed edge before mutating
either direction. It rejects missing parents, self-parenting, and cycles. A
removed parent deterministically detaches live children without recursively
destroying them. Removed IDs and their source keys are never reusable.

The registry does not copy geometry, opacity, clip, visibility, stacking,
damage, buffer, workspace, or output-membership state. Those remain with their
existing authorities.

## Lifecycle integration

`CompositorState` owns one registry. A canonical surface node is registered at
the existing `wl_surface` resource registration seam and removed only after
the existing surface teardown has removed protocol/lifecycle references. Test
helpers that create synthetic renderables explicitly create their test node;
the normal active-frame path never allocates scene metadata.

Existing permanent/live `SurfaceRole` state remains authoritative. Scene role,
domain, and visual-parent metadata are synchronized after successful role
changes and after role-instance deactivation/rollback. A live subsurface edge
is represented by the surface role's validated parent; destroying its live
role detaches the visual edge while retaining the surface node and permanent
role metadata.

Popup visual parenting uses the existing validated parent-surface relationship
only. Window transient relationships and popup/stack groups remain separate.

Layer-shell semantic domains are derived from authoritative committed layer
state without affecting `scene_rank`, stacking, or layout:

```text
Background -> Desktop
Top/Overlay -> Chrome
Bottom -> Chrome (conservative default)
```

Cursor and drag-icon surfaces use `Input`. Application/window groups use
`Content`; ordinary child surfaces inherit their visual ancestor's resolved
domain. Decorations use `Chrome`.

## Stable window anchors

Each `DesktopWindow` receives two compositor-owned nodes for its `WindowId`
lifetime:

```text
WindowGroup
├── root client surface
└── ServerDecoration
```

The group and decoration are metadata-only nodes. Decoration visibility stays
derived from existing decoration mode/state. Transient window relationships do
not become visual-parent edges.

When XWayland replaces a backing surface, the old surface node is detached and
the new surface node is attached to the unchanged WindowGroup. The old node is
not transferred to the new wl_surface, and the WindowGroup is not recreated.

## Active-scene projection

`ActiveSceneView` retains its current renderable-surface vector, surface-index
map, origins, popup IDs, ordering, visibility selection, and all consumers. It
adds derived mappings for `surface_id -> SceneNodeId` and
`SceneNodeId -> active surface index`. These mappings are materialized during
the already-required active-scene rebuild and do not alter render sorting or
introduce a registry traversal on stable frames.

`RenderableSurface`, `RenderSceneElementId`, `VisualStackGroup`, presentation
animators, `NativeSceneHistory`, damage snapshots, Direct Scanout policy, and
KMS/pageflip paths remain unchanged.

## Test strategy

Tests are split into focused modules to avoid growing near-limit integration
files:

* core identity tests cover zero rejection, monotonic allocation, ordering,
  hashability, namespace separation, exhaustion, and no reuse;
* pure registry tests cover source uniqueness, metadata, parent validation,
  cycle rejection, atomic failed mutation, reverse-index coherence, removal,
  domain inheritance, and distinct same-raw external owners;
* compositor integration tests cover surface lifetime, role synchronization and
  rollback, subsurface/popup/layer/cursor/drag metadata, WindowGroup and
  decoration stability, XWayland replacement, and ActiveScene projection;
* render-order parity tests assert existing stacking/order behavior remains
  independent from `visual_parent`.

The tests do not require hardware or a fake second output and do not assert
specific allocator numbers except in allocator-specific tests.

## Explicit non-goals

This foundation does not add `PresentationTransactionId`, frame-level
SceneNode evidence, retained presentation, geometry/opacity/clip migration,
workspace or domain root nodes, a new renderer scene tree, SceneNode-based
damage or Direct Scanout policy, multi-output rendering, hotplug, layout,
animation changes, or generalized Presentation Engine behavior.

## Review invariants

Before completion, the diff must demonstrate that IDs cannot alias or recycle,
visual-parent mutations are cycle-safe and reverse-indexed, parent removal
cannot leave dangling references, WindowGroup identity survives XWayland root
replacement, existing stacking and renderer paths remain authoritative, and
metadata-only changes do not advance render generations or schedule work.
