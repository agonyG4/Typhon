# Typhon Workspace Runtime v1.1 — Quiescence Closure Design

## Goal

Make inactive workspaces quiescent in native presentation and input while preserving `renderable_surfaces` and `DesktopWindow` as the canonical mapped/lifecycle authorities. Workspace membership changes remain scene transitions, not geometry or minimization changes.

## Architecture

Add a derived `ActiveSceneView` in `src/compositor/state/active_scene.rs`. It owns an event-driven, workspace-filtered clone view of currently presentable `RenderableSurface` values and an ordered active popup-ID list. It is not a protocol registry, does not own Wayland/X11 resources, and is rebuilt or incrementally updated only at state-change boundaries. Native frame resolution borrows this stable view; ordinary frames no longer scan, clone, or allocate because inactive windows exist.

The mapped scene remains authoritative:

```text
renderable_surfaces + DesktopWindows + popup/lifecycle state
                         |
                         v
                 ActiveSceneView
                 /      |       \
          native frame  input   identity
```

Content commits update the canonical mapped state first. A central surface-publication helper advances `render_generation` for every valid publication but advances `scene_render_generation` and updates the active view only when the affected surface tree is visible in the active workspace. Workspace/topology changes rebuild the active view once.

## Active-scene representation and invalidation

`ActiveSceneView` contains:

- `surfaces: Vec<RenderableSurface>` in canonical render order;
- `popup_surface_ids: Vec<u32>` containing only alive, mapped popups visible in the active workspace;
- test-only/diagnostic counters for rebuilds and incremental surface updates.

The view is refreshed on initial map, unmap/destroy, minimize/restore, active workspace changes, actual workspace membership changes that cross the active boundary, popup map/unmap/destroy/reparent, renderable reorder, role changes, placement/visual assignment changes, and visible content publication. A content-only commit on an already-hidden surface does not rebuild it. The native API continues to return `Cow<[RenderableSurface]>`; the normal path returns a borrowed slice of `ActiveSceneView`.

## Generation, damage, and feedback ownership

Introduce a central `publish_surface_generation(surface_id, generation, cause)` boundary. It always records global render generation. It consults the active-scene authority once for the affected tree and only publishes scene generation, active-view content, and visible-scene damage participation when visible. Existing visible `SurfaceCommit` and `SurfaceDamage` damage semantics remain unchanged; the damage planner is not weakened.

Pending presentation feedback uses its existing `surface_id` ownership. Visible feedback is taken into a frame batch; hidden feedback remains in the pending collection. Scheduler-facing frame-work predicates count only visible pending feedback, visible frame callbacks, and visible presentation work. Destroyed feedback keeps existing discard behavior. Explicit-sync/fence handling remains enabled for protocol/state progress but hidden readiness does not alone request primary repaint.

## Workspace membership transaction

Workspace inheritance and family moves use one planned transaction:

```text
WorkspaceMembershipTransition {
    changes: Vec<WindowWorkspaceChange>,
    active_scene_changed: bool,
}
```

The transaction computes all actual changes first, returns immediately for an empty change set, then cancels only interaction/grab ownership belonging to changed windows that are leaving the active scene. It applies all membership updates, publishes `SetWorkspace` only for changed managed X11 windows, updates the active view once when needed, reconciles focus/pointer/idle state, marks toplevel state, and advances at most one scene generation. Moves between inactive workspaces do not affect unrelated active interactions or scene generation. Parent removal preserves the child’s existing workspace.

Workspace switches use the same ownership-aware cleanup for the disappearing active workspace. Resize termination goes through `end_window_interaction_by_id_with_reason`, allowing the existing final visual reconciliation and `send_resize_end_configure` lifecycle to run exactly once.

## Focus and layer shell

Layer focus acquisition stores the prior application surface, then routes layer focus through `set_desktop_focus` so `focused_window_id`, activation publication, focus serials, and keyboard focus stay canonical. While a layer owns focus, application identity is cleared from desktop focus. On workspace changes the stored application candidate is discarded if it is not alive, mapped, non-minimized, application-toplevel, and visible in the new active workspace. Release restores through the canonical desktop focus helper; otherwise it uses the visible managed fallback.

## XWayland publication

`WorkspaceId::from_ewmh` becomes pure checked identity conversion (`0 -> 1`, `9 -> 10`, `10 -> 11`); `WorkspaceManager::contains` remains policy validation. Workspace manager length supplies `_NET_NUMBER_OF_DESKTOPS`. Active-workspace switches publish one root desktop-state command without looping over windows. Per-window `SetWorkspace` is emitted only on initial managed admission or actual membership change. Auxiliary X11 windows receive no fabricated desktop membership; a typed clear command removes stale membership if a window is reclassified out of managed desktop policy.

## Geometry and scope constraints

Workspace changes do not mutate `SurfacePlacement`, `ToplevelVisualGeometry`, client configure state, or buffers. The only configure permitted during a transition is the existing terminal configure needed to close an already-active interactive resize. Fullscreen/direct scanout, SSD decorations, pointer hit testing, control snapshots, render-ahead, and buffer-age behavior consume the active scene without changing their existing visual policies. Dwindle, Hybrid Chrome, and Spatial Canvas remain out of scope.

## Testing strategy

Tests are added before each production change for: borrowed stable active-scene resolution, hidden commit generation/damage quiescence, latest-buffer reveal, hidden feedback/callback ownership, active popup identity, dynamic XDG/X11 inheritance, inactive-to-inactive membership, true no-op moves, ownership-aware cancellation, terminal resize lifecycle, layer focus restoration, extensible EWMH conversion, incremental publication, auxiliary admission, fullscreen isolation, and zero geometry traffic. Focused suites run with `rtk`; the full locked suite is attempted and environment failures are classified separately.

