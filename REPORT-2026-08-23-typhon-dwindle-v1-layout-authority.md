# Typhon Dwindle v1 — Layout Authority Report

Date: 2026-08-23

## 1. Baseline HEAD

Baseline HEAD was `9d3fb34b45f6ce4ffc4582c3231e220b3643e959`.

## 2. Initial dirty-tree summary

The repository was already dirty before this task: 55 tracked files were modified or deleted, with additional untracked reports, plans, specifications, and source files. The pre-existing changes were preserved. No reset, clean, stash, commit, or branch operation was performed.

## 3. Actual current Hybrid foundation discovered

Typhon already had the relevant authority seams: `DesktopWindow`, `WindowManagementState`, `WorkspaceLocation`, `ToplevelVisualGeometry`-style visual geometry installation, XDG/X11 configure paths, workspace-family inheritance, active-scene ordering, and server-side decoration layout/hit testing. The pre-existing default admission policy remains floating.

## 4. Reference behavior studied

The reference behavior used for v1 was the familiar Hyprland-style binary split model: focus-aware insertion, deterministic axis selection, per-workspace topology, sibling promotion on removal, and layout-driven placement while floating windows remain independent. The implementation uses those behavioral constraints without importing compositor or protocol objects into the pure tree.

## 5. Architecture chosen

The implementation is split into:

- `src/wm/layout/geometry.rs`: integer layout geometry, shared boundaries, and split ratios.
- `src/wm/layout/dwindle.rs`: pure arena-backed topology.
- `src/wm/layout/plan.rs`: snapshots, constraints, per-location plans, and generation values.
- `src/compositor/state/tiled_layout.rs`: compositor integration and geometry application.

The compositor applies plans through the existing XDG/XWayland geometry and visual-geometry paths.

## 6. Arena/NodeId design

`DwindleTree` uses generational `DwindleNodeId { index, generation }` values. Removed arena slots are reused with a new generation; a slot at generation `u32::MAX` is never reused, preventing stale-id aliasing after generation exhaustion.

## 7. Tree invariants

`debug_validate()` checks root validity, parent links, child validity, cycles, duplicate leaves, orphan live nodes, `by_window` consistency, insertion-order consistency, and valid split ratios. Mutations invoke debug validation in debug/test builds.

## 8. Insertion behavior

Insertion chooses the focused existing leaf first, then a fallback leaf, then the first deterministic leaf. The axis is horizontal for a wider or square anchor rectangle and vertical for a taller rectangle. Pointer position can choose whether the new leaf is the first or second child. The compositor now derives the anchor rectangle from the selected existing plan target rather than blindly using the full output.

## 9. Removal behavior

Removal deletes one leaf and its split parent, promotes the sibling, repairs the grandparent link, and preserves the remaining topology. It does not rebuild the tree.

## 10. Deterministic rounding

`LayoutRect::split_boundary()` computes one rounded shared boundary. Both children use that same boundary, so integer rounding cannot create a one-pixel overlap or hole. Horizontal and vertical splits are tested.

## 11. Minimized-leaf behavior

Minimized leaves remain in the topology but are inactive during plan calculation. A split with one active child gives that child the entire parent rectangle. Restoring the leaf recalculates the original topology.

## 12. Per-WorkspaceLocation ownership

`TiledLayoutManager` owns independent lazy trees keyed by `WorkspaceLocation`, covering regular workspaces and special workspaces. It rejects a `WindowId` already present in another location tree.

## 13. Layout generation

`LayoutGeneration` is a non-zero monotonic value stored in `CompositorState`. A visible reflow advances it once when the layout batch produces a scene effect. Hidden-location topology changes do not eagerly configure or repaint hidden windows.

## 14. Multi-window batch application

`layout_batch_depth` and `layout_batch_scene_effect` suppress intermediate render-generation advances from configure, placement, visual-geometry, and related paths. A visible reflow ends with one `LayoutReflow` generation. Multi-location output reflow calculates all visible plans and refreshes active scene order once.

## 15. ToplevelVisualGeometry integration

Tiled plans apply through `apply_layout_geometry()`, which deduplicates against current visual/root geometry and then uses the existing XDG or X11 visual installation functions. No second preview geometry map was introduced.

## 16. Super+V real transition

Super+V now validates a focused, managed, normal, non-minimized window and performs a real Floating↔Tiled transition. Floating→Tiled captures floating geometry, inserts and calculates before committing membership, then applies a visible plan. Tiled→Floating removes the leaf with rollback on calculation failure, restores the saved floating geometry, and reflows survivors.

## 17. Workspace/Special move integration

Workspace-family transitions prepare source-tree removal and destination-tree insertion before committing location changes. Migration rolls back the manager on failure. Regular and special destinations retain `LayoutMembership::Tiled`; hidden destinations mutate topology without eager configure. Newly visible regular/special locations reflow before scene publication.

## 18. Fullscreen/maximize behavior

Fullscreen and maximize do not remove a tiled leaf. X11 normal-mode restoration prefers the current calculated tiled target over stale restore geometry. XDG normal restoration likewise returns to the current location plan. Focused integration tests cover tiled X11 restoration.

## 19. Work-area reflow

`set_output_size()` recalculates all visible tiled location plans after layer-shell/stateful-window reconfiguration. Output usable geometry is converted to validated integer `LayoutRect` coordinates and dimensions.

## 20. Real Minimal chrome implementation

`WindowChromePolicy` is derived from layout membership. Tiled server-side windows use Minimal chrome: no titlebar, titlebar buttons, or resize hit target. A configured border remains representable. Floating server-side windows retain Full chrome.

## 21. XDG decoration semantics

`DecorationMode` remains decoration negotiation/ownership state. It is not conflated with `WindowChromePolicy`. Decoration layout, hit testing, native render instances, and X11 extents all use the effective chrome policy.

## 22. X11 frame-extents behavior

X11 Minimal chrome exposes no invisible titlebar/button/resize target while preserving the intended border/extents behavior. X11 configure requests from a normal tiled window are answered with current authoritative geometry and do not mutate the tiled frame.

## 23. Hybrid Tiled/Floating scene ordering

Scene ordering distinguishes regular tiled, regular floating, special tiled, and special floating owners. Unmanaged auxiliary/popup roots use a global application band so they continue to stack above ordinary application roots. Canonical scene-owner resolution follows parent/transient ancestry for scene-band calculation without changing the broader workspace-owner semantics used by pointer/grab paths.

## 24. Interaction gating

Move and resize interaction begins are rejected for normal tiled windows before activation or capture. This covers native move/resize, XDG requests, and X11 moveresize routes that share the root interaction entry point.

## 25. Configure counts

Focused minimize/restore integration tests assert that a visible tiled collapse or expansion advances render generation exactly once. Geometry application skips unchanged windows, and hidden locations do not send tiled configure operations.

## 26. Render-generation counts

Visible tiled minimize/restore uses `RenderGenerationCause::LayoutReflow` with one generation per batch. Nested layout operations are coalesced by the batch depth rather than advancing once per window.

## 27. Idle-work evidence

The pure tree and plan are invoked only from transitions, workspace/location changes, minimize/restore, output work-area changes, and explicit reflow paths. Render and frame paths do not calculate Dwindle plans, allocate layout vectors, or scan the tree every frame.

## 28. Pure tree tests

The `wm::layout` focused suite passed: 15 tests covering geometry, shared boundaries, insertion, removal, stale-node protection, minimized leaves, independent locations, duplicate cross-location membership, constraints, and invariants.

## 29. Integration tests

Focused integration suites passed:

- `compositor::decoration`: 20 tests.
- `compositor::state::desktop_window_tests`: 74 tests.
- `compositor::state::window_interaction_tests`: 58 tests.
- tiled X11 configure-authority regression: 1 test.

These include Super+V, workspace and special transitions, tree migration, removal, minimize/restore, mode restoration, scene bands, chrome behavior, and interaction rejection.

## 30. Full validation results

Passed:

- `rtk cargo fmt --check`
- `rtk cargo check --locked --all-targets`
- `rtk git diff --check`
- all task-focused suites above

The final `rtk run cargo test --locked` attempt ran 1,827 tests: 1,789 passed, 36 failed, and 2 were ignored.

## 31. Environment/pre-existing failures

The 36 full-suite failures are environment-sensitive failures, not failures in the task-focused layout/decorations/interactions suites. Most fail during test socket/bootstrap setup with `path must be shorter than SUN_LEN`; subsequent Xwayland display-lock tests cascade into `PoisonError`. One cursor-persistence lock test observed `Busy` where it expected `Insecure`. The brief’s instruction not to weaken tests was followed. No hardware DRM/KMS qualification was claimed.

`bin/check-source-layout` also reports existing dirty-tree over-limit files: `src/compositor/tests/windows.rs`, `src/compositor/tests/xwayland.rs`, `src/compositor/state/desktop_windows.rs`, `src/compositor/state/windows.rs`, `src/compositor/state/desktop_window_tests.rs`, `src/compositor/mod.rs`, `src/compositor/server.rs`, `src/native_output/runtime/bootstrap.rs`, and `src/xwayland/xwm/events.rs`. No source-layout limits were raised, and the Dwindle implementation itself is in focused new modules.

## 32. Review Pass 1 — correctness/ownership findings and fixes

The independent ownership review found and fixed:

- popup/global scene bands falling below regular floating windows;
- X11 client configure requests escaping tiled authority;
- insertion anchors using the full output instead of the selected leaf;
- cross-location duplicate `WindowId` membership;
- stale hidden-tree geometry when a workspace/special becomes visible;
- stale arena generations potentially being reused at exhaustion.

The review also verified no tiled leaf is silently removed by fullscreen, minimize, or mode restoration; floating geometry is not overwritten by tiled geometry; and tree membership is migrated before workspace location commit.

## 33. Review Pass 2 — performance/latency/future-interaction findings and fixes

The independent hot-path review found duplicate active-scene refreshes when reflowing visible regular and special locations together. The reflow path now applies all plans inside one batch and refreshes active scene order once. No render/frame-path Dwindle calculation, mutex, background worker, timer, client barrier, or per-window generation storm was introduced. Unchanged geometry is skipped, hidden trees are lazy, and the pure nodes contain no compositor/protocol resources.

The topology remains suitable for the next phase: pointer delta → nearest ancestor split → normalized ratio mutation → one plan → one coalesced visual batch.

## 34. Final git status

The working tree remains intentionally uncommitted. The original dirty modifications, deletions, and untracked artifacts remain present alongside the new Dwindle modules, compositor integration, focused tests, design/plan documents, and this report. Nothing was staged, committed, reset, stashed, cleaned, or moved to another worktree.

## 35. Explicit future work

The next phase should add split-ratio pointer resizing, richer constraint propagation/clamping, directional focus and movement, directional reinsertion, gaps, split controls, and interaction coalescing. Later work can address tiled-by-default policy, window rules, multi-output ownership policy, animations, and broader protocol-level XDG authority tests. O1/KMS, presentation scheduling, direct scanout, physical renderer behavior, and native scheduling remain outside this task.
