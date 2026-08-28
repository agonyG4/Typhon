# Typhon Hybrid Floating/Tiled Foundation

## Scope

This phase closes the remaining workspace/special scene-transition pointer-ordering defect, strengthens the fullscreen regression, and introduces the runtime foundation for switching a managed window between `Floating` and `Tiled`. It does not implement Dwindle, split trees, ratios, gaps, tiled placement, animations, or a second workspace ownership model.

The existing axes remain authoritative:

```rust
WorkspaceLocation { Regular(WorkspaceId), Special(SpecialWorkspaceId) }
LayoutMembership { Floating, Tiled }
```

`WorkspaceLocation` remains canonical WM membership. `ActiveSceneSelection` and `ActiveSceneView` remain derived presentation state. `SceneWorkIndex` remains derived bookkeeping. Layout transitions never mutate workspace location, EWMH desktop, ext-workspace state, parentage, or client identity.

## Preflight scene transition closure

Workspace and Special transitions enter a transition-scoped pointer-refresh deferral before mutating membership or selection. The transition computes the new selection, identifies departing managed roots/families, cancels terminal interaction, pointer constraints, implicit grabs, and popup grabs while refresh is deferred, then rebuilds `ActiveSceneView`. Focus and layer arbitration are repaired from the final scene. The deferral is released only immediately before one canonical pointer refresh at the final pointer position.

No intermediate `wl_pointer.leave`, enter, or hit-test target is emitted from cleanup. Existing terminal resize finalization still runs exactly once; the transition deferral only suppresses its intermediate pointer refresh. Regular interactions are not included in departing Special IDs and therefore remain active when an unrelated Special closes.

## Fullscreen regression

The native frame planning test will construct a real fullscreen Regular application and a managed Special application. With Special hidden, fullscreen solitude remains eligible. With Special visible, the fullscreen owner remains fullscreen but the Special application remains in native renderables, solitude is false, Direct Scanout is rejected, and no geometry configure is produced by the visibility change.

## Layout ownership

`WindowManagementState` owns the layout membership bit and exposes explicit accessors. `WindowChromePolicy` is a pure policy projection of `LayoutMembership`: Floating maps to full traditional chrome; Tiled maps to minimal/no traditional titlebar policy. The policy is independent of backend, protocol, workspace, and Special state. This phase establishes the boundary and tests the mapping without changing the existing visual decoration theme or geometry protocol behavior.

Floating geometry is compositor-side state on `DesktopWindow`, stored as the existing `WindowGeometry` snapshot. A Floating-to-Tiled transition captures the current visual geometry, including placement and size. Tiled mode is placement-neutral until a layout engine exists. A Tiled-to-Floating transition restores the saved snapshot through the existing backend-specific geometry path, without recreating the client and without changing `ToplevelVisualGeometry`, the resize configure pipeline, or `MAX_IN_FLIGHT_RESIZE_CONFIGURES`.

For XDG windows, restoration uses the existing surface placement and visual geometry assignment. For normal X11 windows, restoration updates the existing X11 frame authority and queues the normal backend configure/state path. Only the active layout authority changes: Floating uses the existing floating placement state; Tiled reserves the future layout authority slot but does no per-frame work.

## Input and runtime flow

The default exact binding is `Super+V`, press-only, non-repeating, and `Respect` for shortcut inhibition. The binding consumes the key event through the existing `NativeWindowAction` pipeline. Routing invokes the server's focused-window layout toggle, which changes only `LayoutMembership`, preserves focus/keyboard/pointer/grabs, and requests the normal visual update. No workspace transition or client recreation is created.

## Future Dwindle insertion point

The future layout engine will consume `LayoutMembership::Tiled` windows grouped by their canonical `WorkspaceLocation`, with independent regular and Special layout state. It will own placement only while a window is Tiled. No current code assumes all managed windows are Floating, and no current data structure combines workspace location with layout mode.

## Validation

Focused unit and integration tests cover pointer lifecycle during Special close, fullscreen/native frame planning, binding semantics, geometry preservation, Regular+Tiled, Special+Tiled, focus/grab preservation, and no-client-recreation behavior. Final validation runs formatting, locked all-target checking, source-layout checks, diff checks, focused tests, and the full locked test suite. Known pre-existing environment failures are reported separately from regressions.
