# Pointer Constraint Scene-Lifecycle Reconciliation

## Goal

Release locked and confined pointer routing when the owning application window
leaves Typhon's active scene, while keeping ordinary constrained pointer motion
unchanged for an eligible visible owner.

## Current defect

`clear_pointer_focus_state` protects an active locked or confined route before
it reaches `deactivate_pointer_constraints_for_surface_focus_loss`. Workspace
and minimize paths then refresh pointer focus, allowing the old route to pin an
invisible surface indefinitely. The existing backend deactivation transaction
already owns generation checks, queued-request cancellation, persistent versus
oneshot lifetime behavior, and locked-pointer reveal settlement.

## Design

Add a compositor-owned reconciliation helper in the pointer-constraint state
module. It accepts departing application window IDs and deactivates every
constraint whose constrained surface resolves through
`presentation_owner_root_for_surface` to one of those windows' canonical root
surfaces. This deliberately covers child/subsurface constraints without adding
another ownership model. A stable deactivation reason is passed for diagnostics
and test intent; it does not alter protocol lifetime semantics.

Workspace scene transitions will reconcile in this order:

1. determine the windows leaving the active scene;
2. mark `workspace_scene_transition_active` before scene mutations that can
   refresh pointer focus;
3. update memberships and rebuild the active scene;
4. deactivate constraints owned by departing roots while the transition flag is
   still set;
5. settle keyboard/window focus and related grabs;
6. clear the transition flag;
7. refresh pointer focus once against the new scene.

Regular workspace activation, special-workspace close/switch, and moving a
window family use this path. A no-op workspace activation returns before any
reconciliation. Minimize uses the same root-oriented helper after removing the
minimized surfaces from renderable scene state and before its final pointer and
window focus reconciliation. Lifecycle animation state remains independent, so
retained visual ghosts never become input authority.

The generic constrained-motion early returns remain intact. Only the explicit
scene-departure callers can invalidate a route. Deactivation calls the existing
`deactivate_pointer_constraint_by_id` transaction, preserving:

- persistent constraints as committed, eligible resources;
- compositor-driven oneshot constraints as defunct;
- pending activation cancellation and stale-generation rejection;
- locked-pointer cursor hints, reveal ownership, and backend settlement;
- confined-pointer focus transfer without forcing unrelated cursor state.

## Tests

Add deterministic compositor regressions covering persistent locked and
confined workspace departure, pending activation cancellation and stale
activation callbacks, oneshot non-reactivation, special-workspace close,
moving a constrained window family off the active workspace, minimizing locked
and confined windows, and child-surface ownership. Keep a negative regression
for ordinary locked/confined motion while the owner remains visible, and a
no-op workspace activation regression proving a valid route is retained.

Focused tests run first in the existing `relative_and_constraints` and
`pointer_constraint_transaction` modules, with workspace/window-state tests
for scene-departure and minimize ordering. Then run the repository's requested
format, check, clippy, full test, and source-layout gates. Direct Scanout,
Presentation Coverage, VRR, tearing, KMS presentation, and unrelated workspace
semantics remain out of scope.
