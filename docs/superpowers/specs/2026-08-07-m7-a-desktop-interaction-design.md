# M7-A Desktop Interaction Core Design

**Status:** Approved architecture; implementation pending

**Repository:** Typhon only

**Starting commit:** `62b15f63e10c79d99f83164e749388f4c1bcb7c`

## Goal

M7-A makes Typhon's desktop interaction policy explicit and deterministic:

- hover changes keyboard focus without changing stacking;
- an ordinary click focuses and raises the exact window that was hit;
- move and resize interactions retain exclusive ownership after the pointer crosses other surfaces;
- committed client-side decoration and shadow extents remain visible during resize preview;
- Typhon renders no normal visible application border.

M7-A is a Typhon-only milestone. M7-B and Eclipse M7-C remain untouched until the deterministic M7-A tests and the required Firefox/Kitty real-session gate pass.

## Constraints and invariants

The implementation must preserve these invariants:

1. Pointer button delivery remains tied to the surface captured by the original hit-test. `activate_desktop_window()` may change stacking, but it must not cause a second hit-test before the button is delivered.
2. Hover focus advances the desktop focus serial only when keyboard focus changes to a different managed `WindowId`. Pointer refreshes and repeated hover over the same window do not churn focus history.
3. Pointer focus, keyboard focus, and interaction motion target are separate concepts. The motion target captured at interaction start remains authoritative until an explicit terminal condition.
4. Hover focus never raises.
5. Click activation raises exactly once through the existing family-aware stacking machinery.
6. Generic `FocusLoss` never terminates an active move or resize.
7. Post-interaction pointer focus refresh occurs exactly once, after the terminal event is complete.
8. M7-A does not add generalized surface-tree shadow inference.
9. M7-A does not change XDG decoration negotiation mode.
10. Typhon adds no visible normal application border.
11. M7-B and Eclipse remain untouched until the M7-A real-session gate passes.
12. Pointer-constraint transitions are not interaction terminals by themselves. A move or
    resize ends only when the transition actually invalidates the captured interaction
    ownership or its protocol state.
13. Desktop focus and activation return typed outcomes: `Changed`, `NoChange`, or
    `Unavailable`. Repeated hover over the same managed `WindowId`, invalid targets, and
    true focus transitions must remain distinguishable without inspecting secondary state.

The existing clean `main` worktree and valid live-session fixes must be preserved. Work stays directly on `main`; no branch, worktree, reset, amend, squash, or history rewrite is permitted.

## Current seams

The design extends existing mechanisms rather than creating parallel compositor paths.

### Focus and stacking

- `src/compositor/state/surface_focus.rs` owns low-level keyboard and surface focus transitions.
- `src/compositor/state/windows.rs` already contains `focus_desktop_window`, `activate_root_window`, minimize/restore operations, and close behavior.
- `src/compositor/state/desktop_windows.rs` owns `WindowId` lookup, stacking, X11 transient-family ordering, and `raise_window_id`.
- `src/compositor/state/input_dispatch.rs` currently performs pointer button hit-testing, focus, implicit-grab setup, and button delivery.
- `src/compositor/state/input_resources.rs` owns ordinary pointer motion and interaction-target motion delivery.

Low-level `focus_surface()` remains necessary for popups, layer-shell, pointer constraints, and compositor-owned surfaces. It must not acquire desktop-window raise or restore semantics.

### Interaction ownership

- `src/compositor/interaction.rs` defines `WindowInteraction`, its source, trigger metadata, and terminal reasons.
- `src/compositor/state/window_interaction.rs` owns begin, motion, resize-preview updates, release finalization, cancellation, and cursor override state.
- `src/compositor/state/hit_testing.rs` owns pointer hit-testing and pointer-focus refresh helpers.
- `src/compositor/state/window_resize.rs` owns visual resize preview, resize configure flow, and render assignment.

The current interaction record already has the essential captured identity: `WindowInteractionId`, `WindowId`, root surface, trigger button/serial, and `pointer_motion_surface_id`. M7-A makes those fields authoritative instead of requiring the target to remain the current pointer-focused surface.

### Visual geometry and borders

- `RenderableSurface.width/height` is the committed root buffer size.
- `surface_window_geometries` stores committed XDG window geometry.
- `ToplevelVisualGeometry` stores compositor resize-preview geometry.
- `src/compositor/render.rs` converts surface targets and clips into CPU/GLES scene plans and native damage bounds.
- Existing server-frame primitives are dormant for normal XDG/CSD windows; `SERVER_FRAME_BORDER_THICKNESS` is an interaction hit thickness, not a visible border.
- X11 ConfigureRequest border width currently reaches the XWM Configure command and must be normalized for managed clients.

## Architecture

### 1. Explicit desktop focus policy

Introduce an explicit reason-aware desktop focus operation, extending the existing `focus_desktop_window` seam:

```rust
pub(crate) enum WindowFocusReason {
    PointerEnter,
    PointerPress,
    ShellActivation,
    KeyboardNavigation,
    Restore,
}

pub(crate) fn focus_desktop_window(
    &mut self,
    window_id: WindowId,
    reason: WindowFocusReason,
) -> WindowFocusOutcome;
```

The exact visibility and naming may follow the current module conventions, but the operation must have these semantics:

- resolve the exact managed desktop `WindowId`;
- reject destroyed, unmapped, minimized, auxiliary, notification, support, DND, override-redirect, and non-keyboard-interactive targets;
- focus the target's root surface through the existing low-level focus path;
- update the logical focused window and active-state publication;
- queue X11 activation synchronization where applicable;
- advance the desktop focus serial once only when the focused managed `WindowId` changes;
- mark only the old and new affected toplevel snapshots dirty;
- never restore or raise.

The focus serial decision is keyed by the managed window transition, not by every surface-resource refresh. If the pointer moves across subsurfaces belonging to the same managed window, or a refresh re-enters the same window, the focus serial remains unchanged.

Low-level surface focus may still change for protocol reasons unrelated to desktop focus. Those changes must not be interpreted as a desktop-window focus transition.

### 2. Exact activation policy

Introduce or refactor an exact-window activation operation:

```rust
pub(crate) enum WindowActivationOutcome {
    Changed,
    NoChange,
    Unavailable,
}

pub(crate) fn activate_desktop_window(
    &mut self,
    window_id: WindowId,
    reason: WindowFocusReason,
) -> WindowActivationOutcome;
```

The operation must:

1. resolve the exact `WindowId`;
2. return `Unavailable` if it is no longer actionable;
3. restore it if minimized;
4. focus it through `focus_desktop_window`;
5. raise it through the existing `raise_window_id` / XDG root stacking path;
6. preserve X11 transient-family parent/dialog ordering and existing backend restack synchronization;
7. return `NoChange` when the window is already focused, restored, and topmost in the effective family;
8. avoid duplicate focus serial transitions and duplicate backend commands for a no-op activation.

The existing `activate_root_window(surface_id)` and focused-window helpers become compatibility wrappers or callers of the exact `WindowId` policy. They must not remain independent activation implementations.

### 3. Pointer press capture and delivery

For an ordinary press over an eligible desktop window, `send_pointer_button` follows this order:

```text
hit-test once
-> capture PointerTarget, root surface, and WindowId
-> establish pointer focus for the captured target
-> activate_desktop_window(WindowId, PointerPress)
-> create the press/implicit-grab record from the captured target
-> deliver wl_pointer.button to the captured surface client
```

The captured `PointerTarget` or an equivalent stable press record owns the button recipient. The activation step may reorder `renderable_surfaces` and `window_stacking`; it must not call `pointer_target_at` again and must not replace the captured surface with the newly topmost surface.

Popup grabs, layer-shell surfaces, session lock, compositor-owned surfaces, override-redirect X11 windows, and an already active move/resize interaction retain their existing exception paths and do not receive ordinary desktop activation.

Button release remains tied to the original implicit-grab/interaction ownership record. It must not be re-targeted by the current cursor location.

### 4. Focus follows mouse without raise

Normal pointer motion keeps the existing single hit-test and pointer enter/leave behavior. After the effective hit target is known, derive its managed root `WindowId` and apply desktop hover focus only when that `WindowId` differs from the currently focused desktop window.

Hover focus is suppressed while any ownership that requires a stable target is active:

- move or resize interaction;
- any held pointer button or implicit pointer grab;
- XDG popup grab;
- pointer lock or confinement;
- DND;
- session lock;
- exclusive layer-shell interaction.

Hover focus is also ineligible for auxiliary popups, notifications, support windows, override-redirect surfaces, cursor surfaces, minimized windows, unmapped windows, and non-keyboard-interactive layer-shell surfaces.

Hover invokes `focus_desktop_window(window_id, PointerEnter)` and never invokes a raise operation. Repeated pixels over the same effective `WindowId` do not call desktop focus again and do not update focus history.

After an interaction terminates, the compositor performs one current-position hit-test, restores ordinary pointer focus, and applies hover focus once. No other cleanup path may perform a second post-interaction refresh.

### 5. Exclusive interaction ownership

An active `WindowInteraction` contains three independent identities:

```text
pointer focus       -> Wayland pointer enter/leave state
keyboard focus      -> desktop keyboard focus and active state
motion target       -> captured WindowId/root/surface for move or resize
```

The motion target is captured once at interaction start and remains authoritative until an explicit terminal condition. Pointer focus or keyboard focus changes never transfer or cancel this ownership.

While the interaction is active:

```text
update global pointer position
-> update captured interaction geometry
-> send motion to captured motion target
-> preserve interaction cursor
```

The interaction motion dispatcher must validate only the captured target's continued viability:

- surface is alive;
- surface remains under the captured root;
- target remains renderable or otherwise protocol-valid for the interaction.

It must not require `pointer_surface == pointer_motion_surface_id` and must not re-resolve the target from a new hit-test. Client motion delivery remains tied to the captured target's pointer resources and interaction-grab state.

Pointer refresh helpers are interaction-aware:

- `refresh_pointer_focus_at_last_position` does not re-hit-test or clear an active interaction;
- `commit_pointer_crossing_at_last_position` does not replace interaction ownership;
- implicit-grab refresh paths do not terminate or retarget an active interaction;
- XWayland metadata/configure, XDG commit, resize commit, scene reconciliation, and session transitions use the same ownership guard.

Remove the generic `WindowInteractionEndReason::FocusLoss` path. A pointer-focus clear is not a valid interaction terminal condition. If the captured surface disappears, use an explicit surface-destruction/unmap/client-disconnect reason and clear the associated resize preview safely.

Terminal behavior is split into two explicit paths:

- normal completion: trigger button release or explicit end, apply the final resize update, send the final configure, clear interaction state, finish the terminal event, then refresh pointer focus exactly once;
- cancellation: target destruction/unmap, session loss, input-device removal, or protocol cancellation, clear pending preview/ownership state exactly once, clear the cursor override, finish cleanup, then perform one safe pointer refresh if the session remains active.

The interaction ID remains useful for rejecting stale updates and proving that a second terminal event is a no-op.

### 6. Conservative visual extents during resize

Add a root-level value type:

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct WindowVisualExtents {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}
```

For an XDG root, derive the value only from:

- committed root buffer bounds; and
- authoritative committed `xdg_window_geometry`.

Treat the root buffer as `[0, 0, buffer_width, buffer_height]` and the logical geometry as the signed rectangle `[x, y, x + width, y + height]`. Use checked or saturating arithmetic. The extents are the committed pixels outside that logical rectangle, with negative or offset geometry handled without unsigned underflow or double application of the XDG offset.

Do not infer extents from unrelated subsurfaces, titles, colors, alpha patterns, magic shadow sizes, or application identity.

During interactive preview, keep three concepts separate:

1. desired logical window geometry used for configure and resize anchoring;
2. committed root buffer content, which is never scaled or rewritten;
3. committed root visual extents, which may remain visible around the logical aperture.

The renderer must express the preview as a bounded root-only visual aperture: the desired logical content remains clipped to the new logical geometry, while only the committed root pixels that were outside the previous logical geometry are retained as visual extents. If the existing single rectangular clip cannot represent this union without exposing stale client content, add the smallest internal root-only region/rect representation necessary to express the bounded union. Use extent strips only if the repository's existing region representation cannot express it safely. Do not widen the existing clip for every subsurface.

The visual aperture must:

- preserve existing committed CSD/shadow pixels;
- preserve left/top anchor semantics for edge and corner resize;
- avoid scaling stale content;
- prevent old logical client content from leaking beyond the desired logical window;
- clear cleanly when the client commits the final resized buffer.

`WindowVisualExtents` does not participate in configure sizing, focus eligibility, or ordinary frame hit-testing. Existing logical window geometry remains the source of truth for those policies.

CPU, GLES, scene snapshots, and native damage must consume the same resolved root visual aperture. Old and new visible bounds must both contribute to damage when the preview changes.

### 7. Border policy

M7-A does not add a visible border, titlebar, separator, or compositor shadow around ordinary application windows.

Add or retain tests proving:

- normal XDG composition emits no Typhon-owned visible application border;
- normal managed X11 composition emits no Typhon-owned visible application border;
- the resize hit thickness remains an invisible interaction margin only;
- server-frame primitives are not emitted for ordinary application windows.

For managed X11 ConfigureRequest handling, normalize the effective client border width to zero before constructing the XWM Configure command. Preserve the existing geometry and ConfigureNotify contract; do not create a second X11 stacking or configure path.

Do not change the existing XDG decoration negotiation mode. M7-A does not force server-side decorations and does not remove client-side GTK, Qt, Firefox, or other application titlebar/shadow pixels.

## Test design

All production changes follow red/green/refactor. Each regression is written first, run to confirm the expected failure, implemented minimally, then rerun with focused and broader tests.

### Focus and activation tests

Add deterministic coverage for:

- hover from managed window A to B focuses B without changing `window_stacking`;
- repeated hover over B does not change its focus serial or emit duplicate focus transitions;
- pointer refresh over B does not churn focus history;
- click on B focuses and raises B exactly once;
- activation of an already focused/topmost B returns `NoChange` without duplicate backend restack or focus commands;
- click activation restores a minimized B before focus and raise;
- XDG and managed X11 windows share the policy;
- managed X11 transient-family ordering remains parent/dialog correct;
- popup, layer-shell, lock, override-redirect, and compositor-owned exceptions do not raise ordinary desktop windows.

The click regression must deliberately arrange overlapping windows so that activation changes render order, then assert that the button event still arrives at the surface captured before the raise.

### Interaction tests

Add deterministic coverage for:

- move/resize starting on A and crossing B, empty desktop, and A again while retaining one interaction ID and one `WindowId` owner;
- no `FocusLoss` termination during pointer-focus changes;
- XDG commit, XWayland metadata/configure, scene reconciliation, and explicit pointer refresh during resize;
- interaction motion delivery while the captured surface is no longer the current pointer-focused surface;
- explicit trigger release finalization followed by exactly one pointer refresh;
- target destruction/unmap ending the interaction once, clearing cursor override and preview state;
- stale terminal events and stale interaction IDs being harmless;
- pointer focus, keyboard focus, and interaction motion target remaining independently observable in test snapshots.

Required sequences include pointer travel inside A, over B, outside both, back over A, and release. B must never become the interaction owner.

### Visual extent tests

Add tests for a committed root buffer of `332 x 242` with signed/offset logical geometry such as `(16, 10, 300, 200)`, including the derived extents and resolved preview aperture.

Explicitly cover:

- grow and shrink from the right and bottom edges;
- left-edge resize with a changed placement and preserved left extent;
- top-edge resize with a changed placement and preserved top extent;
- top-left resize with both placement axes changing;
- negative XDG geometry offsets;
- all four corners;
- final client commit clearing the preview aperture without losing the committed root render placement;
- root visual extent preservation without extending the same extent clip to unrelated subsurfaces;
- stale logical client content not appearing in an extent strip;
- CPU/GLES render-plan parity where both paths are available;
- old and new native damage bounds covering the visual aperture change.

These tests must assert actual aperture/target values, not only a boolean `resize_preview_active` flag.

### Border tests

Add coverage for:

- normal XDG no-border rendering;
- normal managed X11 no-border rendering;
- managed X11 ConfigureRequest with a non-zero requested border width resulting in an effective width of zero;
- correct ConfigureNotify geometry after normalization.

### Stress and real-session gate

After focused deterministic tests pass, run the M7-A stress groups without arbitrary sleeps:

```text
100 click-focus-raise cycles
100 hover-focus-without-raise cycles
100 mixed hover/click cycles
100 XDG resize-across-window cycles
100 X11 resize-across-window cycles
100 pointer-refresh-during-resize cycles
100 CSD extent resize cycles
```

Only then run the real Firefox/Kitty gate on the native Typhon session:

- hover Kitty while Firefox remains above it: Kitty focuses, Firefox remains above;
- click Kitty: Kitty focuses and raises;
- hover Firefox: Firefox focuses, Kitty remains above;
- resize Kitty across Firefox, empty desktop, and Kitty until release;
- repeat left, right, top, bottom, and corner resizes;
- verify visible client-side shadow/CSD extents remain present during preview.

Record observed results. Automated tests, TTY/DRM qualification, and application behavior must be reported separately. No real-session result may be claimed unless it was observed in this milestone run.

## Review gates

Before the implementation is considered complete, inspect the diff for:

- hover paths that call raise;
- click paths that re-hit-test after activation;
- focus serial changes caused by same-window pointer refresh;
- motion ownership derived from current hit-test or pointer focus;
- generic `FocusLoss` cancellation;
- more than one post-interaction pointer refresh;
- expanded clips that leak stale client content;
- generalized surface-tree shadow inference;
- XDG decoration mode changes;
- visible normal application borders;
- a second unrelated X11 stacking/configure path;
- new `unsafe` without a local precise `// SAFETY:` explanation.

## Planned implementation slices

The implementation plan will keep the following logical slices independently testable:

1. Red/green exclusive interaction ownership and refresh suppression.
2. Red/green desktop focus policy, serial invariants, and click capture/raise.
3. Red/green conservative root visual extents and resize aperture, including left/top/top-left cases.
4. Red/green border invariants and managed X11 border-width normalization.
5. Focused deterministic suite, stress groups, and the M7-A real-session gate.

Each successful slice receives an atomic commit. No M7-B protocol mutation requests, Eclipse edits, or unrelated compositor redesign are included.
