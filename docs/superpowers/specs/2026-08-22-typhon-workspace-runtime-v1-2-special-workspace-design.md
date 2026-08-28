# Typhon Workspace Runtime v1.2 + Special Workspace v1 Design

## Goal

Close the remaining workspace quiescence debt and add one persistent Hyprland-style Special Workspace overlay without changing the workspace lifecycle authority, geometry authority, or O1/KMS scheduling model.

The result must represent all four future-compatible combinations:

```text
Regular(WorkspaceId) + Floating
Regular(WorkspaceId) + Tiled
Special(SpecialWorkspaceId) + Floating
Special(SpecialWorkspaceId) + Tiled
```

The implementation is one coherent runtime closure, internally phased as:

1. event-driven quiescence and ActiveScene correctness;
2. typed location and scene-selection foundation;
3. persistent Special overlay transitions;
4. focus/input/fullscreen and protocol integration;
5. review, instrumentation, and verification.

The supplied dirty working tree is authoritative. Existing changes are preserved, no unrelated files are restored or removed, no new worktree is created, and no commit is made unless explicitly requested.

## Current architecture constraints

- `WorkspaceManager` remains the authoritative regular/special workspace state owner.
- `WindowManagementState` remains the authoritative managed-window membership and layout state.
- `ActiveSceneView` remains a derived, event-driven presentation/input cache.
- `ToplevelVisualGeometry` remains the visual geometry authority.
- Surface/client lifecycle remains owned by the existing compositor state and protocol paths.
- Existing XDG, X11, popup, layer-shell, fullscreen, pointer-constraint, resize, and explicit-sync lifecycles are reused.
- O1 credit admission, KMS opportunity scheduling, presentation ledgers, render-ahead scheduling, and worker-lane simulation are not redesigned.
- Special is not `WorkspaceId(11)`, is absent from regular workspace counts/iterators, and is absent from ext-workspace-v1 and EWMH numbered desktop publication.

## Domain model

Add a compact, non-zero typed `SpecialWorkspaceId` with `Copy`, `Eq`, `Ord`, `Hash`, and a `DEFAULT` constant. The representation follows `WorkspaceId` and remains extensible to multiple configured special IDs later.

Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WorkspaceLocation {
    Regular(WorkspaceId),
    Special(SpecialWorkspaceId),
}
```

Change `WindowManagementState` to store `location: WorkspaceLocation` plus the existing orthogonal `LayoutMembership`. The ambiguous numeric `workspace()` accessor is removed. Callers use:

- `location()`;
- `regular_workspace() -> Option<WorkspaceId>`;
- `special_workspace() -> Option<SpecialWorkspaceId>`;
- `with_location(...)`.

`WindowManagementState::new(WorkspaceId)` remains a regular-workspace admission convenience. A separate location constructor supports tests and future policies.

`WorkspaceManager` stores:

- active regular workspace;
- the configured regular workspace list;
- configured Special IDs, initially only `DEFAULT`;
- optional visible Special ID.

Regular activation keeps its existing `WorkspaceSwitchOutcome`. Special toggling gets a typed outcome distinguishing opened, closed, no-change, and unknown Special IDs. Toggling Special never changes the active regular workspace.

## Canonical ownership and derived scene state

`ActiveSceneSelection` is derived from `WorkspaceManager` and contains the active regular workspace plus the optional visible Special overlay. It is stored in `ActiveSceneView` as a cache key/selection snapshot, never as window-management authority.

Scene visibility is resolved through canonical managed roots:

- managed toplevel root: its own `WorkspaceLocation` and minimized state;
- managed XDG/X11 descendants, popups, and auxiliary surfaces: the canonical managed root's location and visibility;
- layer-shell trees and cursor surfaces: explicit `SceneWorkOwner::Global` ownership;
- ownerless/legacy cases: handled by an explicit bounded policy, never by assigning an independent workspace location.

No path reinterprets a Special window as belonging to the active regular workspace. Parentage changes and X11 `WM_TRANSIENT_FOR` changes are folded into the same transition that updates inherited management locations and pending-work ownership.

## SceneWorkIndex and true quiescence

Introduce a focused bookkeeping module, preferably `src/compositor/state/scene_work.rs`, with:

```rust
enum SceneWorkOwner {
    Global,
    Location(WorkspaceLocation),
}
```

The index owns no duplicated protocol resources. It maintains owner buckets/indices and exact counters for:

- pending frame callbacks;
- pending presentation feedback;
- pending explicit-sync/frame-prepare records;
- pending surface-tree transaction readiness;
- active FIFO barriers.

Each callback/feedback has one indexed owner and one authoritative resource. `take_visible_pending_*` drains only buckets selected by the current `ActiveSceneSelection`; hidden buckets remain parked. A small monotonic sequence can preserve deterministic visible-bucket order without scanning hidden buckets.

Every ownership-changing operation uses one transition boundary:

1. compute canonical old/new root ownership;
2. compute the full family/inheritance membership transition;
3. mutate `WindowManagementState` locations;
4. migrate all affected SceneWorkIndex records;
5. update the active selection and visible counters;
6. rebuild ActiveScene only if final visible IDs/order changed.

The same boundary is used for regular switches, Special toggles, family moves, relationship changes, map/unmap/minimize/restore, and teardown. Bulk explicit-sync/surface-tree drain paths use scoped index reconciliation so predicates never observe partial index state.

Scheduler-facing predicates read indexed visible-ready counts. They do not iterate hidden callback, feedback, explicit-sync, surface-tree, or FIFO collections. Total pending explicit-sync work remains separately countable for readiness servicing without making it visible-output work.

Discard paths remove only the affected resource IDs and decrement the visible counter only when that exact owner was visible. Debug assertions validate index/count consistency at event boundaries.

## ActiveScene correctness and generation ownership

Remove the test-only fallback that returns all renderable surfaces when the active scene cache is empty. `CompositorState::new` and test fixtures explicitly initialize/rebuild the cache.

`refresh_active_scene_surface_order()` returns an explicit outcome indicating whether the final active-scene order changed. `reorder_renderable_surfaces_by_window_stack()` advances `scene_render_generation` only when that outcome is changed. Hidden-only global renderable reorder therefore has zero active-scene generation delta.

ActiveScene rebuilds remain event-driven: workspace selection, membership, map/unmap, minimize/restore, relationship, popup, stacking, and relevant layer events. Native frame resolution borrows the stable cached scene between events.

Visible active ordering preserves canonical stacking and subsurface order within each application layer, but places visible Special application trees above visible regular application trees. Layer-shell scene ranks remain authoritative so Special cannot jump above Top/Overlay surfaces.

## Special transitions

Add compositor/server methods for:

- toggling `SpecialWorkspaceId::DEFAULT`;
- moving the focused managed family to Special;
- moving a focused Special family to the active regular workspace.

Family resolution always canonicalizes a child to its root and moves every managed family member together. Layout membership, mode, minimized state, geometry, buffers, constraints, and client state are unchanged.

The transition computes departing/entering scene ownership. Only windows that actually leave the active scene are passed to the existing interaction/grab/constraint termination lifecycle. Global layer/cursor ownership and unaffected regular-window interactions survive unrelated Special changes.

Opening/closing Special:

- does not map/unmap or minimize/restore clients;
- does not configure XDG or X11 geometry;
- does not change EWMH current/numbered desktop state;
- does not publish a fake ext-workspace handle;
- advances scene generation at most once when visible content changes.

Regular switching while Special is visible changes only the regular layer and retains Special visibility.

## Focus and input

On Special open:

- an exclusive layer-shell surface keeps actual keyboard focus;
- the application fallback is updated to the best eligible Special window;
- otherwise the most recently focused eligible Special window wins, followed by topmost Special stacking order;
- an empty Special preserves/restores valid regular focus.

On Special close, a focused Special application is replaced by the best eligible window in the active regular workspace. A valid regular focus remains intact. No hidden Special window may remain focused.

Pointer hit testing uses the cached active scene, so exposed regular content remains clickable while Special stays open. A regular focus change does not auto-hide Special.

Existing canonical termination is used for interactions, pointer constraints, implicit grabs, popup grabs, and active resize. Hidden-window transitions do not use a broad low-level state clear.

Authorized XDG activation of a hidden Special toplevel opens Special before focusing it. Token and focus-steal authorization is unchanged; rejected activation remains rejected.

## Fullscreen and scanout

Visible Special application content disables regular solitary-tree culling and regular direct-scanout qualification. A regular fullscreen window retains `ToplevelMode::Fullscreen` and remains composed beneath Special.

Special fullscreen content is conservatively composed. Direct scanout is rejected whenever Special application content is visible unless existing invariants independently prove a safe path; the implementation does not weaken scanout checks.

## Input bindings

Add canonical evdev `KEY_S` and typed actions:

```text
BindingAction::ToggleSpecialWorkspace(SpecialWorkspaceId)
NativeWindowAction::ToggleSpecialWorkspace(SpecialWorkspaceId)
BindingAction::MoveFocusedWindowToSpecialWorkspace(SpecialWorkspaceId)
NativeWindowAction::MoveFocusedWindowToSpecialWorkspace(SpecialWorkspaceId)
```

Default bindings:

- Super+S: press-only, repeat disabled, inhibition respected, exact modifier match, toggle default Special;
- Super+Shift+S: press-only, repeat disabled, inhibition respected, exact modifier match, move focused family to/from default Special.

Consumed key press/release forwarding continues through the existing ledger so Super/S events do not leak to clients.

## XDG, X11, EWMH, ext-workspace, and control publication

XDG and X11 managed admissions default to the active regular workspace. Inherited children use the exact parent location while retaining their own layout policy.

Extend `WindowBackendCommand` and `XwmCommand` with typed `ClearWorkspace`. XWM implements it by deleting `_NET_WM_DESKTOP`. Regular-to-Special emits ClearWorkspace; Special-to-Regular emits SetWorkspace using the existing 1-based Typhon/0-based EWMH conversion.

Special toggles never publish root desktop-state changes. Valid EWMH desktop requests for Special windows are interpreted as moves to a requested regular workspace; EWMH cannot request Special.

The regular ext-workspace-v1 bridge remains unchanged and publishes only regular workspaces. The control snapshot labels Regular locations numerically and the default Special location as `"special"`; mapped remains independent from active-scene visibility.

Existing Astrea toplevel dirty/structure publication is marked for location changes without redesigning the publication protocol.

## Tests and evidence

Pure domain tests cover typed IDs, invalid zero IDs, ordering/hash behavior, location/layout orthogonality, and all four state combinations.

Manager tests cover regular activation independence, Special open/close, persistent overlay during regular switches, unknown IDs, and regular-only count/iteration.

Quiescence tests cover:

- empty active scene without the former test fallback;
- active/hidden callback and feedback buckets with no repeated hidden scans;
- exact visible counters after hidden destruction;
- hidden explicit-sync/FIFO/surface-tree work not becoming active output work;
- hidden-only stacking generation delta zero;
- visible stacking generation delta one.

Special tests cover:

- overlay ordering and persistent client state;
- hidden commits without scene generation/repaint;
- Regular↔Special family moves preserving geometry/mode/layout;
- transient inheritance and relationship migration;
- focus, layer-shell focus, pointer hit testing, grabs, constraints, and resize termination;
- regular and Special fullscreen composition/scanout rejection;
- Super+S and Super+Shift+S matching, repeat/inhibition, and forwarding ledger;
- XDG activation, X11 ClearWorkspace, EWMH, ext-workspace, control labels, and toplevel publication.

Test-only instrumentation reports ActiveScene rebuilds, indexed hidden inspections, scene-generation deltas, and XWayland publication command counts across repeated native-frame resolution.

Validation uses the existing target directory and `rtk` wrappers where available:

```text
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk git diff --check
rtk cargo test --locked <focused filters>
rtk cargo test --locked
```

Environment-only socket/DRM failures are reported separately. No native DRM/KMS qualification is claimed unless appropriate hardware actually ran.

## Review gates

Correctness review checks that Special is not numeric, counted, published, unmapped, geometry-mutating, layout-coupled, focus-stale, or able to split families. It also checks layer-shell ordering, XDG/X11 parity, and exact input ownership cancellation.

Performance/future-layout review checks zero per-frame workspace reconstruction/filtering, zero hidden callback/feedback partition scans, indexed scheduler predicates, zero hidden-only scene-generation churn, no toggle-wide X11 publication, no locks/threads/timers, and natural future Dwindle keys by `WorkspaceLocation`.
