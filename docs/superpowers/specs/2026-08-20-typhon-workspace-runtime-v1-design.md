# Typhon Workspace Runtime v1 Design

Date: 2026-08-20

## Goal

Turn the existing WM foundation into runtime workspaces with instantaneous
visibility switching, focus restoration, transient-family inheritance, native
number-row bindings, and XWayland EWMH desktop semantics while preserving the
existing mapped-window, WindowVisual, resize, rendering, input, and KMS
architecture.

## Scope

Workspace 1 is active initially. `Super+1..9` and `Super+0` switch to
workspaces 1..10. `Super+Shift+1..9` and `Super+Shift+0` move the focused
managed transient family without changing the active workspace.

Workspace visibility is a scene policy, never minimization. Inactive windows
remain mapped, alive, associated, geometrically intact, and present in the
global `DesktopWindow` registry. They are excluded only from active desktop
scene consumers.

Dwindle, tiled geometry, chrome policy, workspace animation, Spatial Canvas,
and multi-window geometry transactions remain out of scope.

## Architecture

`WorkspaceManager` remains a pure compositor-independent value object. It owns
known `WorkspaceId`s and the active identity, and exposes a typed activation
operation returning `Changed`, `NoChange`, or `UnknownWorkspace`. It does not
own windows, geometry, threads, timers, locks, renderer state, or output
state.

`WindowManagementState` gains a controlled workspace mutation operation. The
operation changes only `WorkspaceId`; it preserves `LayoutMembership`, so
Floating and future Tiled windows can both move between workspaces.

The compositor keeps one workspace visibility policy over its existing
`DesktopWindow` registry and scene traversal. Managed roots are visible when
their management workspace equals the active workspace and they are not
minimized. Popup/subsurface trees follow their owner root. Managed X11
dialogs/transients inherit their managed parent. Auxiliary X11 windows do not
gain independent management state; when attached to a managed transient
ancestor, their scene visibility follows that ancestor. Layer-shell remains
global/output-owned.

The policy is queried by existing render, hit-test, fullscreen, idle, frame
callback, and commit/damage paths. It does not rebuild or drain
`renderable_surfaces`, create a second scene graph, or add a second whole-window
scan per frame.

## Workspace transitions

`CompositorState::switch_workspace(WorkspaceId)` validates the target, cancels
interactive move/resize and disappearing-workspace grabs, updates the manager,
reconciles application focus and pointer constraints, preserves exclusive
layer-shell focus, updates fullscreen and idle policies, marks required
publication state dirty, advances one explicit workspace render-generation
cause, and requests one redraw. Same-workspace and unknown-workspace requests
produce no scene generation.

Switching never mutates `WindowState::minimized`, `ToplevelMode`,
`ToplevelVisualGeometry`, `SurfacePlacement`, floating geometry, X11 WM state,
or configure queues. X11 windows remain mapped and `_NET_WM_STATE_HIDDEN`
continues to mean only actual minimization.

## Focus and activation

All desktop focus/activation paths validate active-workspace visibility before
focusing a managed window. The focused-window invariant is:

```text
focused_window_id == Some(window)
    implies window.management.workspace == active_workspace
```

On a switch, focus restoration chooses the highest valid `last_focus_serial`
among visible, mapped, non-minimized managed windows, then falls back to the
topmost eligible window in `window_stacking`. Empty workspaces clear
application focus. An exclusive layer-shell focus remains authoritative during
switching; application fallback focus is updated without stealing layer focus.

Explicit authorized activation of an inactive window navigates to its workspace
before focusing it. Initial mapping of a child inheriting an inactive parent
workspace does not navigate or steal focus.

## Transient inheritance

XDG `set_parent` updates reconcile the child's management workspace to the
valid managed parent's workspace. Parent removal preserves the child's current
workspace. X11 transient relationship reconstruction performs the same
reconciliation for managed dialog/toplevel families. Cycles remain rejected.

Moving any focused managed family resolves its managed family root and applies
the target workspace to the root and managed descendants. It does not copy the
parent's layout membership: a tiled parent may retain a floating dialog.

## Native bindings

The existing exact-modifier binding system gains number-row constants and typed
actions carrying `WorkspaceId`. Workspace bindings fire on press, disable key
repeat, respect shortcut inhibition, are not reserved/emergency bindings, and
do not overlap the existing Ctrl+Shift+Alt session-switch bindings. Existing
deferred Super/Alt forwarding and consumed-key ledgers remain unchanged.

## XWayland EWMH

The existing XWM command/event boundary remains the only raw-X11 boundary.
The default root publication becomes ten desktops with current desktop equal to
active workspace minus one. Desktop viewport/workarea arrays are cardinality
consistent. A single tested conversion helper maps `WorkspaceId(1..10)` to
EWMH `0..9` and rejects zero-based values outside the configured range and
`0xFFFFFFFF`.

Managed X11 windows and dialogs publish `_NET_WM_DESKTOP`; auxiliary windows do
not. Runtime switches and family moves publish root/current and window desktop
state through typed XWM commands. Valid `_NET_CURRENT_DESKTOP` and supported
`_NET_WM_DESKTOP` client messages normalize into typed events, then pass through
compositor validation and workspace policy. No compositor code reaches raw X11
connections.

## Rendering, fullscreen, idle, and frame policy

Active-workspace visibility is applied at the existing scene authority, so
inactive roots, subsurfaces, popups, and SSD decorations do not render or hit
test. Layer-shell remains visible. Fullscreen eligibility requires the
fullscreen window to belong to the active workspace; an inactive fullscreen
window retains `ToplevelMode::Fullscreen` but cannot own active composition or
direct scanout.

Idle inhibition treats hidden application trees as ineffective while preserving
global layer-shell behavior. Hidden clients may continue committing buffers,
but hidden commits do not schedule useless visible output work where the
existing damage path permits that distinction. Ordinary output frame callbacks
are not completed as visible presentation for hidden application trees; normal
delivery resumes after the workspace becomes visible.

## Testing strategy

Implementation follows test-driven development in focused slices:

1. Pure workspace activation and controlled membership mutation.
2. Visibility predicates, one-generation transitions, no-configure behavior,
   and mapped/geometry/state preservation.
3. Focus restoration, activation guards, interaction/grab cancellation,
   transient inheritance, and family moves.
4. Native bindings and inhibition/repeat/forwarding behavior.
5. Fullscreen, idle/frame, control snapshot, and XWayland EWMH publication and
   request routing.

Each slice gets a failing test first, then minimal implementation, then focused
`rtk cargo test --locked` validation. Final validation includes `rtk cargo fmt
--check`, `rtk cargo check --locked --all-targets`, `rtk git diff --check`, the
focused suites, and an attempted `rtk cargo test --locked`. Existing
environment failures will be classified rather than hidden.

