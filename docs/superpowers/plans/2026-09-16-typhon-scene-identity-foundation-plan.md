# Typhon Scene Identity and Canonical Visual Topology Foundation Plan

## Goal

Implement stable canonical scene identity and visual-topology metadata with no
intentional change to existing rendering, stacking, layout, input, animation,
presentation, KMS, Direct Scanout, damage, or protocol behavior.

## Constraints and ownership

* Preserve unrelated worktree changes. Own only the new scene/core/docs files
  and clean integration files needed by the implementation.
* Do not modify the concurrent viewport-validation hunks in
  `src/compositor/geometry.rs`, `src/compositor/protocols/core.rs`,
  `src/compositor/state/shutdown.rs`, `src/compositor/state/subsurfaces.rs`,
  `src/compositor/state/surface_transactions.rs`, `src/compositor/state_data.rs`,
  `src/compositor/subsurface.rs`, `src/compositor/tests/protocol_error.rs`, or
  `src/compositor/tests/surface_frames.rs`.
* Use existing protocol, role, window, popup, layer, stacking, render, and
  active-scene state as authority. Scene metadata is downstream and sidecar.
* Keep new test modules focused so the source-layout diagnostic count does not
  increase from the starting baseline of 53.

## Stage 1 — typed identity and pure registry (TDD)

### Red tests first

1. Add `src/core/scene_node_id.rs` tests for zero rejection, monotonic
   allocation, `Copy`/hash/order behavior, exhaustion without wrap, no reuse,
   and namespace separation from `WindowId`/`OutputId` raw values.
2. Add focused registry tests for unique source registration, duplicate source
   rejection, distinct same-raw external owners, parent existence, self-parent
   rejection, cycle rejection, atomic failed parent mutation, reverse-index
   coherence, parent removal detachment, domain inheritance, and no ID reuse.
3. Run the focused tests and confirm they fail because the identity and
   registry types do not yet exist.

### Minimal implementation

1. Add `SceneNodeId(NonZeroU64)` and `SceneNodeIdAllocator` under `src/core`,
   following `OutputId` allocator semantics: start at one, checked increment,
   explicit exhaustion, never wrap/recycle.
2. Export the ID publicly and keep the allocator crate-private.
3. Add focused `src/compositor/scene/` modules:
   `mod.rs`, `metadata.rs`, `registry.rs`, and `tests.rs`.
4. Define `SceneOwner`, `SceneRole`, `SceneDomain`,
   `SceneDomainAssignment`, `SceneNodeMetadata`, a source-key type, and
   `CanonicalSceneRegistry`.
5. Implement checked registration/update/removal APIs. Validate proposed
   visual-parent edges before changing either the parent field or reverse
   index. Resolve inherited domains with a bounded, cycle-safe walk.
6. Re-run the focused identity/registry tests and `cargo fmt --check`.

## Stage 2 — compositor-owned registry and surface lifecycle (TDD)

### Red tests first

1. Add focused compositor scene tests proving a surface node is allocated at
   resource registration, remains stable across buffer/content replacement,
   unmap/remap, and role-instance transitions, is removed at wl_surface
   destruction, and is not reused by a later surface.
2. Add role tests for XDG toplevel, popup, subsurface, layer, cursor, drag-icon,
   and XWayland metadata, including rollback after a failed role reservation.
3. Add parent/removal tests proving a destroyed live subsurface relationship
   detaches only its visual edge while retaining the surface node and
   permanent role metadata.

### Minimal implementation

1. Add a `CanonicalSceneRegistry` field to `CompositorState` and expose the
   narrow integration methods from a new `src/compositor/state/scene.rs`.
2. Register each surface node from the existing
   `register_surface_resource` seam. Remove it at the end of the existing
   surface teardown after protocol/lifecycle references are scrubbed.
3. Implement synchronization from `SurfaceRoleLifecycle` to generic scene
   role/domain/parent metadata. Call it only after successful role assignment,
   after live-role deactivation, and after rollback. Do not add a second role
   state machine.
4. Map generic roles as follows: unassigned, client/toplevel, popup,
   subsurface, layer, cursor, drag icon, and XWayland. Use explicit Content for
   app surfaces/groups, Chrome for decorations, Input for cursor/drag, and
   inherited domains for popup/subsurface descendants.
5. Use the already validated subsurface parent ID for visual parenting. Use
   the registered popup parent surface for popup visual parenting. Do not use
   transient window relationships or stacking groups as visual parents.
6. Derive layer semantic domains from authoritative committed layer state:
   Background→Desktop, Top/Overlay→Chrome, and conservative Bottom→Chrome.
   Keep layer rank/layout untouched and refresh metadata when the layer changes.
7. Add explicit test-only setup for synthetic renderable surfaces so active
   scene tests never cause normal-frame lazy allocation.
8. Re-run focused scene/role/surface tests and inspect the diff for accidental
   render-generation or scheduler changes.

## Stage 3 — stable WindowGroup and ServerDecoration nodes (TDD)

### Red tests first

1. Add window tests proving every XDG and XWayland `DesktopWindow` creates a
   distinct WindowGroup and ServerDecoration node owned by the WindowId.
2. Prove the root surface and decoration point at the WindowGroup, while
   transient-window relationships do not become visual-parent edges.
3. Add the XWayland replacement regression: old root surface A detaches, new
   surface B attaches, WindowId and WindowGroup ID remain unchanged, and A/B
   retain separate surface-node IDs.
4. Add decoration stability coverage across SSD, CSD, fullscreen, and return to
   SSD wherever the existing policy exposes those transitions.

### Minimal implementation

1. Register WindowGroup and ServerDecoration source keys during the existing
   validated `insert_desktop_window` path. Keep both nodes metadata-only.
2. Attach the current root surface to the group after the window and nodes are
   established. Keep decoration visibility derived from existing decoration
   state; never create/destroy the decoration node during mode changes.
3. On window removal, remove only the group/decorations after existing window
   teardown; registry removal detaches children without recursively destroying
   surfaces.
4. In `attach_x11_surface`, detach the old surface’s visual-parent edge and
   attach the new surface node to the existing WindowGroup. In
   `detach_x11_surface`, detach only the edge. Do not transfer IDs or alter
   window/placement/presentation logic.
5. Run XDG/XWayland/window/decoration focused tests and existing render-order
   parity tests.

## Stage 4 — ActiveSceneView identity projection (TDD)

### Red tests first

1. Add focused projection tests proving every active renderable surface has a
   canonical SceneNodeId, content-only refresh preserves it, reorder changes
   only the active index, hidden workspace removes it from the projection but
   not the registry, and re-show restores the same ID.
2. Cover special-workspace overlays, popup selection, subsurfaces, and existing
   surface-id-to-index compatibility behavior.
3. Add render-order parity assertions that visual-parent metadata does not
   replace or alter existing window, popup, subsurface, decoration, layer, or
   X11 sorting.

### Minimal implementation

1. Add sidecar `surface_id -> SceneNodeId` and `SceneNodeId -> active index`
   mappings to `ActiveSceneView`; do not change `RenderableSurface` layout.
2. Build mappings from the canonical registry during the existing active-scene
   rebuild, alongside the already-materialized surface indices and origins.
   Avoid a registry traversal or allocation on stable frames.
3. Preserve all existing selection, visibility, order, origins, popup IDs,
   rebuild conditions, render IDs, scene-work, effects, and pointer-hit
   behavior.
4. Run projection, workspace, popup, cursor/drag, layer, and render-order
   focused suites.

## Stage 5 — documentation and final review

1. Update
   `docs/superpowers/specs/2026-09-15-typhon-generalized-presentation-engine-design.md`
   to state that OutputId, SceneNodeId, canonical identity/topology metadata,
   visual-parent hierarchy, and ActiveScene identity projection are
   implemented; frame-level SceneNode evidence and Presentation Engine v2
   remain pending. Replace “stable/generational” with the actual monotonic,
   never-reused, stale-safe invariant.
2. Run the full required verification commands:
   `cargo fmt --check`, `cargo check --locked --all-targets`,
   `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`,
   and `./bin/check-source-layout`.
3. Re-run focused scene registry, surface roles/lifetime, XDG/popup,
   subsurface, XWayland, decoration, active scene/workspace, layer, cursor,
   drag-icon, render-order, presentation, Direct Scanout, and frame/promotion
   suites.
4. Confirm final source-layout diagnostics are no greater than 53 and no new
   file violates configured limits.
5. Review the complete diff against the 18 architecture invariants in the
   task, confirm no renderer/presentation paths changed, and commit only owned
   files in reviewable stage commits.

## Commit sequence

1. `feat(scene): add canonical scene identity registry`
2. `refactor(scene): bind surface lifetimes to SceneNode identity`
3. `feat(scene): add stable window visual groups`
4. `refactor(scene): project active surfaces through canonical node identity`
5. `docs(scene): mark scene identity foundation implemented`

If a pure identity test and implementation are inseparable under the repository
workflow, keep their commit focused and explain the pairing in the handoff.

