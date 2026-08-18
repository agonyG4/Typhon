# Typhon Instant Interactive Resize and WindowVisual Pointer Ownership Design

Date: 2026-08-18

Status: implementation design for the current dirty Typhon worktree

## Scope and observations

This closure addresses two coupled compositor input problems:

1. interactive resize geometry is visually delayed until `prepare_frame()` even though the pointer update has already selected the next geometry;
2. a server-side decoration is rendered as part of a top window but is absent from ordinary pointer surface hit testing, allowing a lower client surface to receive pointer focus through the titlebar.

The closure preserves the existing configure/commit ownership ledger, `ToplevelVisualGeometry`, WindowVisual scene generation, stale-buffer policy, explicit-sync capture, presentation-history damage, fullscreen frame-scene authority, and buffer-age behavior.

The user-visible acceptance target is direct manipulation: after a processed resize motion, the compositor-owned frame and titlebar use the new target during that input dispatch and are eligible for the next render opportunity. Client content may remain at its last committed extent until the client converges.

## Evidence classification

### CONFIRMED

- `CompositorState::update_window_interaction_by_id()` computes a resize target and stores `PendingInteractiveResizeUpdate`.
- `OwnCompositorServer::prepare_frame()` calls `apply_pending_interactive_resize_update()` before flushing resize configures.
- `ResizeConfigureFlow::take_sendable()` and `mark_sent()` reject a new configure whenever `in_flight_configure_count()` is nonzero.
- `in_flight_configure_count()` currently includes unACKed configures, an ACKed uncaptured configure, and captured commits waiting for completion.
- `pointer_target_at()` scans renderable client surfaces only.
- `decoration_hit_at()` is a separate query and therefore does not occlude a lower client during ordinary pointer focus routing.
- SSD active state is derived from `focused_window_id`, so a titlebar becoming inactive proves that desktop focus changed.

### STRONG HYPOTHESIS

- The visible resize lag is primarily the extra input-to-prepare boundary, compounded by one-configure serialization and latest-target coalescing behind an old capture.
- The observed titlebar hitch is caused by the false pointer-enter focus transition and its downstream keyboard, pointer, dirty-state, and redraw work.

### NATIVE-PROVEN

- Hyprland's current `CDragStateController::mouseMove()` computes floating resize geometry and synchronously calls `setPositionGlobal()` followed by `warpPositionSize()`; its target implementation sends the new client size from the target update path. References: [DragController.cpp](https://github.com/hyprwm/Hyprland/blob/main/src/layout/supplementary/DragController.cpp), [WindowTarget.cpp](https://github.com/hyprwm/Hyprland/blob/main/src/layout/target/WindowTarget.cpp).
- KWin's current `Window::updateInteractiveMoveResize()` computes `nextMoveResizeGeom` directly from the pointer and contains an explicit synchronization gate; this design applies the Wayland visual target immediately and keeps synchronization policy in the backend/configure path. References: [window.cpp](https://github.com/KDE/kwin/blob/master/src/window.cpp), [xdgshellwindow.cpp](https://github.com/KDE/kwin/blob/master/src/xdgshellwindow.cpp).
- KWin's top-level hit test calls a decorated window's `hitTest()`, and the input handler derives decoration ownership from the already selected hover window outside `clientGeometry()`. References: [window.cpp](https://github.com/KDE/kwin/blob/master/src/window.cpp), [input.cpp](https://github.com/KDE/kwin/blob/master/src/input.cpp).
- Hyprland's view hit tester selects a window using unified input/reserved extents and its input manager separately checks the selected window's decoration input before forwarding pointer actions. References: [ViewHitTester.cpp](https://github.com/hyprwm/Hyprland/blob/main/src/desktop/state/ViewHitTester.cpp), [InputManager.cpp](https://github.com/hyprwm/Hyprland/blob/main/src/managers/input/InputManager.cpp).

### UNPROVEN

- The exact native p50/p95 pointer-to-pageflip improvement is not known until the supplied launcher is run on the high-refresh output.
- No static test can establish that resize is subjectively “Hyprland-equivalent”; native qualification must separately judge compositor frame attachment and client content convergence.

## Current resize pipeline

The current path is:

```text
pointer motion
  -> compute clamped target
  -> store PendingInteractiveResizeUpdate
  -> return from input dispatch
  -> prepare_frame()
  -> apply_pending_interactive_resize_update()
  -> queue_resize_root_window_to()
  -> update ToplevelVisualGeometry / render assignment
  -> render
```

The client path is separately serialized:

```text
queue target
  -> wait for all prior protocol/capture state
  -> send one configure
  -> ACK
  -> capture one commit
  -> wait for application
  -> send the next queued target
```

The visual path must no longer depend on the client path. The client path remains bounded and latest-wins.

## Current SSD pointer fall-through pipeline

The confirmed path is:

```text
pointer motion
  -> update_pointer_position()
  -> pointer_target_at()
  -> renderable client surface B under A's titlebar
  -> focus_desktop_window_at_pointer_target(B)
  -> focused_window_id A -> B
  -> A decoration becomes inactive
```

`decoration_hit_at()` can find A's titlebar, but it is not part of the ordinary pointer scene query. A decoration therefore cannot occlude a lower surface consistently, and independent decoration/client queries can disagree about stacking.

## Selected resize architecture

Interactive resize has two coordinated authorities:

```text
processed pointer target
  -> immediate queue_resize_root_window_to()
      -> ToplevelVisualGeometry (visual target)
      -> WindowVisual render placement and size
      -> SSD layout and hit testing
      -> X11 frame geometry where applicable
      -> render generation and existing damage/repaint path

same interaction dispatch
  -> bounded ResizeConfigureFlow queue
  -> server flushes sendable configures
  -> XDG serial/ACK/capture ownership
```

`PendingInteractiveResizeUpdate` is removed as a visual-authority cache. `queue_resize_root_window_to()` remains the single immediate mutation path, and the existing render-target clearing policy remains intact so stale client content is not stretched by default.

The native input order becomes:

```text
if an interaction is active:
  compute/apply visual resize target
  update pointer position and decoration hover using the new geometry
  dispatch interaction-owned pointer motion
else:
  update pointer position
  resolve normal pointer scene hit
```

The server flushes resize configures after pointer motion and after the public interaction-update entry point. `prepare_frame()` retains a flush as a safety net for queued work from non-pointer paths, but it is no longer the first point at which visual resize can occur.

## Bounded configure ledger

The selected hard bound is:

```rust
const MAX_IN_FLIGHT_RESIZE_CONFIGURES: usize = 3;
```

Three is the smallest tested bound that gives a responsive client a short sliding window while keeping a stopped client bounded. The bound is measured in protocol pressure, not all retained internal records:

- sent but not ACKed entries count;
- the newest ACKed-but-uncaptured entry counts;
- captured commits do not count against the send window because their serial ownership is already exact and retained in `captured` until apply/release;
- at most one ACKed-but-uncaptured entry is retained because a newer ACK supersedes older ACK state for the next client commit;
- one unsent latest intermediate target is retained when the window is full;
- one final `resizing = false` target supersedes the unsent intermediate target and has send priority.

Each sent entry retains serial, sequence, interaction ID, target geometry, resize state, and send timestamp. Serial transitions are explicit:

1. ACKing serial C retires older outstanding resize serials A/B and makes C the only eligible uncaptured ACK.
2. ACKing a newer serial replaces an older uncaptured ACK; the replaced serial is marked stale/retired rather than silently forgotten.
3. A commit captures only the newest eligible ACK and becomes a retained `ResizeCommitSnapshot`.
4. A captured old commit may complete while a newer interaction target is active; completion updates committed content state but never replaces the newer `ToplevelVisualGeometry`.
5. A final target is queued from the current visual geometry, sent with `resizing = false` as soon as bounded pressure permits, and retires preview ownership only when the matching final commit is applied.
6. If final priority requires it, the oldest unACKed intermediate entry is superseded and retired to make room while keeping the hard bound.

The flow exposes separate diagnostics for protocol pressure, retained captures, queued/coalesced targets, final priority, and maximum pressure. This prevents a captured commit from being misreported as a configure-starvation cause.

## Pointer scene-hit architecture

The compositor gains one ordered query:

```rust
pub(in crate::compositor) enum PointerSceneHit {
    Client { target: PointerTarget },
    Decoration {
        window_id: WindowId,
        root_surface_id: u32,
        hit: DecorationHit,
    },
    None,
}
```

`pointer_scene_hit_at(x, y)` walks the existing renderable scene from top to bottom. It preserves layer-shell, popup, subsurface, grab, constraint, and stacking precedence. At a managed window root it evaluates the current `ToplevelVisualGeometry` and SSD `DecorationLayout` in the same scene walk. A child/popup/input-capable surface above the root wins first; an interactive titlebar/button/resize region then returns an owning `Decoration` hit; a root client input region returns `Client`; shadows and other non-input visual extensions continue walking.

Existing `pointer_target_at()` becomes the client-only projection of this query. Existing decoration actions become projections of the same query rather than independent z-order decisions.

Decoration routing rules:

- keyboard/desktop focus uses the decoration's owning `WindowId` and the existing `PointerEnter` policy;
- an already focused owner produces no focus-generation change;
- compositor-owned decoration space clears Wayland client pointer focus instead of fabricating a `wl_surface`;
- returning to the owning client surface sends the normal leave/enter transition without involving a lower window;
- pointer grabs, locked/confined pointers, popup grabs, active move/resize, and drag-and-drop continue to run before ordinary scene resolution;
- titlebar buttons and resize margins capture the owning window ID for clicks/drags;
- CSD windows have no SSD input region and continue to use client surfaces;
- layer-shell and legitimate popups above the window continue to win because they are encountered earlier in the renderable scene.

## Latency metrics

Existing bounded resize diagnostics remain disabled by default and gain bounded histories/fields for:

- input hardware timestamp;
- interaction-update timestamp;
- visual geometry applied timestamp;
- configure queued and sent timestamps/serials;
- frame resolution/start, queue/submit, and pageflip timestamps;
- ACK, matching commit capture, and matching commit applied timestamps.

Derived p50/p95 metrics are computed from bounded samples for pointer-to-visual, pointer-to-configure-send, pointer-to-frame-resolution, pointer-to-KMS-submit, pointer-to-pageflip, configure-send-to-ACK, and ACK-to-commit. Structural tests prove pointer-to-visual has no `prepare_frame()` boundary; native qualification supplies the actual pageflip and subjective results.

## Rejected alternatives

- Delaying nothing but retaining `PendingInteractiveResizeUpdate`: rejected because it leaves the visual authority in the input-to-frame boundary.
- Stretching every stale client buffer: rejected because it hides client convergence latency and can violate the existing damage/content policy.
- Sending unlimited configure requests: rejected because a stalled client must remain bounded.
- Counting captured commits as protocol pressure: rejected because captured ownership is already exact and must not starve newer communication.
- Disabling pointer-enter focus globally: rejected because exposed client surfaces must retain current focus-follow behavior.
- Returning early whenever `decoration_hit_at()` reports a hit: rejected because independently ordered decoration and surface queries can swallow popups/layers.
- Creating a fake Wayland decoration surface: rejected because SSD is compositor-owned and must not fabricate client protocol ownership.
- Making active SSD depend on hover: rejected because it masks, rather than fixes, a desktop focus transition.
- Disabling triple buffering, buffer age, explicit sync, KMS pacing, or render-ahead: rejected because the interaction fix must preserve the preceding rendering closures.

## Regression strategy

TDD cycles cover:

1. same-dispatch visual geometry for all resize edges/corners, including X11 preview and immediate MacTahoe layout;
2. three-entry configure pressure, latest-wins coalescing, slow-client 1,000-motion boundedness, fast-client throughput, newest ACK, ACK replacement, captured old commit, and final priority/convergence;
3. unified A-over-B scene hit for client-to-titlebar, buttons, resize margins, pointer leave/return, unchanged focus generation, and exact click owner;
4. popup-above-decoration and layer-shell-above-decoration ordering;
5. CSD and XWayland behavior;
6. existing implicit-grab/constraint paths;
7. old/new complete WindowVisual damage and render-ahead/buffer-age 1/2/3 oracle tests.

Focused suites run after each task. Full validation records the environment-limited baseline and the post-change result without relabeling newly introduced failures as pre-existing. Native qualification uses the supplied launcher on the high-refresh output and distinguishes compositor frame latency from client content commit latency.
