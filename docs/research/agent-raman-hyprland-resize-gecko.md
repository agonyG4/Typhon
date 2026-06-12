# Agent Raman: Hyprland Resize Behavior for Gecko

Date: 2026-06-11

Scope: read-only research for Oblivion. This document studies local reference
compositors, especially `WM para Referencia/Hyprland-main`, for interactive
resize behavior with Gecko/Firefox/Zen. It does not change production source.

## Executive Summary

Hyprland's resize behavior is not "resize by waiting for the client." It moves
the compositor-owned window target immediately, sends a new xdg configure, and
damages both the previous and next visual bounds. The important detail for
Gecko is that Hyprland avoids stretching a stale smaller client surface during
interactive resize. The frame/decorations can follow the pointer, but the old
client buffer is kept at its committed size until the client ACKs and commits a
compatible resized buffer.

For Oblivion, the ACK path is already aligned with Hyprland: the current code
promotes the latest resize serial not newer than the ACK serial. The remaining
risky area is the visual model. If Oblivion adds live resize visuals by updating
`RenderableSurface.width` / `height` before a compatible client commit, the CPU
renderer will scale the old buffer into the future target rect. That is the
Gecko/Zen artifact to avoid.

The safest path is:

1. Keep committed buffer geometry and interactive resize target geometry
   separate.
2. During resize, damage old and new visual bounds, but render stale client
   buffers at committed size or clipped size.
3. Draw frame/background/empty newly exposed area separately from the client
   buffer.
4. Promote the real surface size only when an ACKed resize commit has a
   compatible committed logical size.
5. Split protocol preparation from presentation completion so resize configures
   are flushed before paint decisions, while frame callbacks/presentation are
   completed after real presentation/pageflip.

## Hyprland Reference Behavior

### Interactive Resize Owns Window Geometry Immediately

Hyprland records the drag start geometry and pointer position in
`CDragStateController::updateDragWindow()`:

- `DragController.cpp:76-83` stores the original box, pointer, position, size,
  and last pointer.
- `DragController.cpp:86-165` starts a drag, chooses resize corner, damages the
  target, focuses/raises the window, and sets the special resize cursor.

Each pointer motion computes a new compositor-side box:

- `DragController.cpp:254-283` computes total and per-tick pointer delta and
  throttles repeated resize updates near monitor cadence.
- `DragController.cpp:285-292` damages the current target and captures the
  previous full window box before mutation.
- `DragController.cpp:306-366` computes floating resize position/size from the
  grabbed corner, clamps min/max, rounds, then calls `setPositionGlobal()`.
- `DragController.cpp:368-369` routes tiled resize through the layout manager.
- `DragController.cpp:373-387` records motion blur from previous to current
  full bounds and damages the target again.

The target update itself also damages around the geometry mutation:

- `WindowTarget.cpp:34-42` calls `updatePos()`, which damages the window before
  mutation and uses a scope guard to damage again after mutation.
- `WindowTarget.cpp:50-58` updates floating `m_position`, `m_size`,
  `m_realPosition`, `m_realSize`, sends the window size, and updates
  decorations.
- `WindowTarget.cpp:107-120` performs the same pre-damage path for tiled
  windows before calculating visual box.
- `WindowTarget.cpp:219-228` writes the rounded visual position/size, updates
  decorations, and sends the window size.

Takeaway for Oblivion: pointer resize should be compositor-owned visual state.
Do not wait for Gecko to commit before the resize frame/outline/decorations can
move. But also do not confuse that compositor-owned visual target with the
client buffer's committed size.

### Old Gecko Buffers Are Not Stretched During Interactive Resize

Hyprland's normal render setup starts from the real window size:

- `Renderer.cpp:573-583` builds `renderdata` from `m_realPosition` and
  `m_realSize`.
- `Renderer.cpp:702-715` traverses the current wl surface tree and adds a
  `CSurfacePassElement` for every committed texture.

Then the surface pass clamps old or undersized client buffers:

- `WLSurface.cpp:40-50` defines `small()` as reported size larger than current
  committed surface size.
- `WLSurface.cpp:53-61` computes a correction vector for undersized surfaces.
- `SurfacePassElement.cpp:20-27` detects whether an interactive resize is in
  progress.
- `SurfacePassElement.cpp:36-50` is the key: if the surface is smaller than the
  viewport, the normal path may translate and scale it to match the real window
  size, but the interactive-resize branch sets `windowBox.width = SIZE.x` and
  `windowBox.height = SIZE.y`. In other words, the old buffer stays at its
  committed size during the drag.
- `SurfacePassElement.cpp:68-73` only "squishes" oversized surfaces down to the
  window box; it is not a stale-buffer stretch path.

Hyprland can still use UV adjustment and edge extension for legitimate
viewport/fractional scale cases:

- `ElementRenderer.cpp:34-50` chooses expected size from viewport destination,
  viewport source, current surface size when the window/surface size is
  misaligned, or reported window size.
- `ElementRenderer.cpp:53-110` adjusts UVs for source rectangles, fractional
  scale misalignment, and undersized/oversized surfaces.
- `ElementRenderer.cpp:238-265` detects interactive resize while deciding the
  misaligned fractional scale fast path.

Takeaway for Oblivion: old-buffer stretching must be opt-in for real
viewport/scale semantics, not an accidental side effect of live resize. During
interactive toplevel resize, if committed buffer size differs from requested
window target, render the old buffer at committed size and fill/draw the new
exposed region separately.

### Damage Covers Old And New Bounds

Hyprland has several layers of conservative damage:

- `DragController.cpp:157` damages the target when drag begins.
- `DragController.cpp:285` damages before a motion update.
- `WindowTarget.cpp:40-42` damages before/after target position updates with a
  scope guard.
- `DragController.cpp:387` damages after the motion update.
- `Renderer.cpp:2754-2773` maps a full window bounding box into every monitor
  that should render the window.
- `Renderer.cpp:2698-2752` maps committed surface damage into monitor damage for
  normal client commits.
- `Renderer.cpp:2788-2803` provides arbitrary box damage for other old/new
  rects.

Takeaway for Oblivion: a live resize visual path needs an output damage
accumulator that can add:

- previous resize visual bounds;
- new resize visual bounds;
- decoration/frame/titlebar bounds;
- committed surface damage;
- software cursor old/new boxes;
- full-output fallback on unknown or overflow.

Surface damage alone is not enough when the compositor changes the window box.

### Configure, ACK, And Commit Replacement

Hyprland sends sizes and tracks serials:

- `Window.cpp:1633-1655` sends the xdg toplevel size when the reported size
  changes and stores `(serial, size)` in `m_pendingSizeAcks`.
- `Window.cpp:1414-1428` handles ACK by finding the newest pending size whose
  serial is `<= ack_serial`, applies that size to pending wl surface state, and
  erases older serials.

The client buffer replacement happens on wl surface commit:

- `protocols/core/Compositor.cpp:560-584` updates `m_current` surface state and
  current texture from the committed state.
- `protocols/core/Compositor.cpp:591-603` emits commit events through the surface
  tree.
- `Window.cpp:2575-2633` handles the window commit, clamps floating constraints,
  damages the committed wl surface at `m_realPosition`, and rechecks
  subsurfaces/popups.
- The next `Renderer.cpp:702-715` traversal sees the updated current texture.

Takeaway for Oblivion: do not special-case Gecko by forcing a visual resize on
ACK alone. ACK says "the client accepted a configure"; the resized visual
content is available only when a compatible buffer/window geometry commit
arrives.

### Frame Scheduling Keeps Gecko From Waiting Forever

Hyprland has explicit output-event scheduling:

- `Monitor.cpp:99-110` listens for output frame, commit, and needs-frame events.
- `Monitor.cpp:112-140` handles presented events and forwards timing to the
  presentation protocol.
- `Monitor.cpp:163-175` has an explicit Firefox note: if no monitor frame is
  pending, Hyprland sends frame callbacks on present because Firefox may wait for
  a new callback when nothing else schedules frames.
- `Renderer.cpp:2200-2205` sends frame callbacks to visible workspaces when
  there is no damage.
- `Renderer.cpp:2249-2263` adds frame damage to output state, commits, and
  reschedules if needed.
- `Renderer.cpp:2504-2510` sends wl frame callbacks to visible views.
- `MonitorFrameScheduler.cpp:14-18` enables the new scheduler only with explicit
  sync support and no active direct scanout.
- `MonitorFrameScheduler.cpp:20-83` renders early if sync says a frame is late,
  then commits on the next presented/doLater path.
- `MonitorFrameScheduler.cpp:86-139` renders on output frame and arms a sync fd
  callback after render.

Takeaway for Oblivion: Gecko resize smoothness depends on configure delivery,
frame callbacks, and presentation cadence. A no-damage visible-client callback
path is not optional for browser liveness.

## KWin Cross-Check

KWin shows the same broad separation between compositor geometry, configure
queue, ACK, and commit:

- `xdgshellwindow.cpp:85-90` schedules configure events on an idle timer instead
  of sending every transient change immediately.
- `xdgshellwindow.cpp:114-132` sends a configure event, carries forward flags,
  and stores it in `m_configureEvents`.
- `xdgshellwindow.cpp:150-158` consumes configure events up to the ACKed serial.
- `xdgshellwindow.cpp:181-198` avoids synchronizing move-resize geometry from
  committed geometry when a geometry-changing configure is still pending.
- `xdgshellwindow.cpp:262-280` either updates geometry immediately if the
  rounded client size did not change, or schedules a configure when it did.
- `xdgshellwindow.cpp:820-831` stores toplevel configure bounds as
  `moveResizeGeometry()`.

KWin also shows why X11/XWayland should be considered separately:

- `window.cpp:1138-1203` updates interactive resize from pointer motion.
- `window.cpp:2401-2409` defaults Wayland interactive resize sync to a direct
  `moveResize(rect)`.
- `x11window.cpp:3888-3910` uses X11 sync-request logic and can wait for client
  ACK/timeout before applying more interactive resize steps.

Takeaway for Oblivion: the Wayland/Gecko path should not inherit X11 sync
behavior. Chromium under XWayland and native Gecko Wayland can look different
because the resize pacing contract is different.

## Oblivion Current State

### Already Aligned With Reference Behavior

Resize input and protocol state:

- `src/compositor/mod.rs:1702-1725` starts compositor resize from the surface
  under the pointer and picks edges from local position.
- `src/compositor/mod.rs:1771-1813` records root surface, kind, start pointer,
  start placement, start width/height, and `drag_committed`.
- `src/compositor/mod.rs:1845-1879` computes pointer deltas, applies the resize
  threshold, and queues a pending resize configure.
- `src/compositor/mod.rs:2223-2250` coalesces a single
  `pending_resize_configure`.
- `src/compositor/mod.rs:2252-2264` flushes the pending configure.
- `src/compositor/mod.rs:2313-2342` sends xdg configure with `Resizing` state and
  stores a `PendingResizeCommit` by `(surface_id, serial)`.
- `src/compositor/mod.rs:2381-2396` now promotes the latest sent resize whose
  serial is `<= ack_serial`, matching Hyprland/KWin-style ACK semantics.

Tests already cover important protocol guards:

- `src/compositor/tests/windows.rs:172-188` verifies resize motion coalescing
  until present/frame work.
- `src/compositor/tests/windows.rs:191-205` verifies pending resize configure is
  frame work.
- `src/compositor/tests/windows.rs:235-280` verifies non-exact newer ACK
  promotion.
- `src/compositor/tests/windows.rs:285-308` verifies pending resize commit waits
  for a matching committed size.
- `src/compositor/tests/windows.rs:372-387` verifies configure-only resize does
  not advance render generation.
- `src/compositor/tests/windows.rs:390-441` verifies tiny/same-size resize
  motion does not report repeated visual updates.
- `src/compositor/tests/windows.rs:462-479` verifies left-edge shrink keeps the
  old buffer origin until client commit.

Native cursor and partial damage work have also progressed:

- `src/native_output.rs:1340-1362` skips frame repaint for pure cursor motion
  when hardware cursor is active.
- `src/native_output.rs:1635-1653` still requests visual redraw for window
  interaction motion.
- `src/native_output.rs:3652-3715` implements a KMS/GBM hardware cursor buffer
  and move/disable calls.
- `src/native_output.rs:3449-3591` maps committed surface damage to native
  output damage when the render cause can use surface damage.
- `src/compositor/mod.rs:164-166` currently limits surface-damage use to
  `SurfaceCommit` and `SurfaceDamage`.

### Remaining Gecko Resize Gaps

Frame lifecycle is still monolithic:

- `src/compositor/server.rs:226-234` commits ready explicit sync buffers,
  flushes color, flushes resize configure, releases buffers, completes frame
  callbacks, completes presentation feedback, and flushes clients in one method.
- `src/native_output.rs:735-844` computes repaint need from accepted clients,
  render generation change, pending frame work, and redraw request.
- `src/native_output.rs:864-880` paints and presents when repaint is needed.
- `src/native_output.rs:926-930` calls `server.present_frame()` only after that
  loop stage when frame work is still pending.

This means resize configure, frame callbacks, and presentation feedback still
share one "present_frame" boundary instead of a prepare-before-render /
finish-after-pageflip split.

The renderer still has a stale-buffer stretch hazard if live resize geometry is
implemented naively:

- `src/compositor/render.rs:417-445` draws each surface into its snapshot target.
- `src/compositor/render.rs:470-484` builds snapshot targets from
  `surface.width` and `surface.height`.
- `src/compositor/render.rs:1011-1055` fast-copies only when buffer size and
  target size match.
- `src/compositor/render.rs:1077-1101` scales the buffer into the target when
  they differ.
- `src/compositor/render.rs:670-679` returns no server-side frame rects today,
  so there is no separate frame/background resize visual to absorb newly exposed
  area.

Partial scene rebuild is layout-stable only:

- `src/compositor/render.rs:488-530` returns partial damage only when previous
  and current snapshots have the same layout and changed surfaces have partial
  damage.

Takeaway: Oblivion is currently safe from old-buffer stretch mainly because it
does not advance the renderable surface size on configure-only resize. If a
future change makes the resize visual live by mutating `RenderableSurface`
dimensions early, the current blit path will stretch stale Gecko buffers unless
an explicit interactive-resize guard is added.

## Gecko Versus Chromium Notes

### Gecko / Firefox / Zen

Gecko Wayland resize is configure/ACK/commit paced. The compositor sends a
toplevel configure, Gecko ACKs it, GTK/Gecko resizes its native Wayland/EGL
surface, and a later wl surface commit carries the new buffer. During that gap,
the compositor must:

- keep sending frame callbacks so Gecko does not wait forever;
- not stretch the old buffer to the requested size;
- keep top/left anchors stable until a compatible commit;
- avoid wasting the resize frame budget on cursor-only full repaint.

Useful diagnostics for a manual run:

- `MOZ_ENABLE_WAYLAND=1` to force/confirm Wayland path when needed.
- `WAYLAND_DISPLAY=<oblivion-socket>` to target Oblivion.
- `MOZ_LOG=Widget:5,WidgetPopup:5` and `MOZ_LOG_FILE=/tmp/firefox-widget.log`
  for configure/resize/widget logging.

Avoid treating Firefox flags as the fix. If old buffers stretch or frame
callbacks stall, that is compositor behavior.

### Chromium / Brave / Chrome

Native Chromium Wayland still follows the same xdg configure/commit contract,
but Chromium often has a different launch surface:

- more command-line knobs for Ozone/Wayland/GPU;
- more frequent dmabuf/GPU buffer paths once native GPU is enabled;
- different throttling around frame callbacks and damage.

The compositor invariant is the same: ACK is not a resized buffer, and a stale
buffer must not be stretched into the future resize target. If Chromium is
running through XWayland, compare it separately because X11 sync-request
behavior is closer to KWin's `x11window.cpp:3888-3910` path, not Gecko Wayland.

## Proposed Gradual Enablement For Oblivion

### Phase 1: Instrument Resize Without Changing Visual Semantics

Add logs/tests around the existing model first:

- resize begin/update/end with root surface, edges, target x/y/width/height;
- configure serial and whether `Resizing` state is set;
- ACK serial and promoted resize serial;
- commit logical size and buffer size;
- render generation cause;
- output damage kind/rect count/pixels;
- frame callback and presentation completion timing.

Manual pass condition: Zen resize logs show configure -> ACK -> compatible
commit ordering, and configure-only resize still does not alter renderable
surface size.

### Phase 2: Introduce Separate Resize Visual State

Add a per-root-surface state shaped roughly like:

```text
ResizeVisualState {
  root_surface_id,
  active,
  requested_rect,
  previous_visual_rect,
  edges,
  latest_configure_serial,
  committed_buffer_size,
}
```

This state should be owned by WM/compositor policy, not by the committed
surface buffer. The committed `RenderableSurface` should still describe pixels
that actually exist.

Manual pass condition: dragging a resize edge can move a frame/outline or fill
newly exposed area without changing the old client buffer's sampled size.

### Phase 3: Add Old/New Resize Damage

Before applying a new visual target, add damage for the previous visual rect.
After applying it, add damage for the new visual rect. Include decorations and
newly exposed background.

Implementation should start conservative:

- whole old root window bounds;
- whole new root window bounds;
- full-output fallback if rect math overflows or effects are unknown.

Only after correctness is proven should it shrink to tighter client/frame
regions.

Manual pass condition: no stale pixels around right/bottom growth, left/top
movement, titlebar/frame, popups, or CSD margins.

### Phase 4: Guard The Renderer Against Interactive Resize Stretch

Add a render-time distinction:

- legitimate scale/viewport/fractional scale can use the scaling fallback;
- interactive toplevel resize with stale committed buffer must not use the
  scaling fallback just because the target changed.

Expected behavior:

- old buffer remains crisp at committed dimensions, or is clipped;
- new exposed area is wallpaper/frame/background, not stretched old content;
- when Gecko commits a matching buffer, the resized content replaces the stale
  visual in the next frame.

This mirrors `SurfacePassElement.cpp:20-50`.

### Phase 5: Split Frame Lifecycle

Split `present_frame()` into protocol preparation and presentation completion:

- prepare before render:
  - commit ready explicit sync buffers;
  - flush color;
  - flush pending resize configure;
  - flush clients.
- finish after presentation/pageflip:
  - release buffers used by the frame;
  - complete frame callbacks;
  - complete presentation feedback with real timing;
  - flush clients.

For native GBM, call finish from pageflip completion. For dumb/fallback paths,
document the immediate completion behavior.

Manual pass condition: Gecko gets resize configures before the compositor spends
the frame budget on paint/copy, and frame callbacks/presentation are not
completed before the output has actually presented the relevant frame.

## Manual Test Plan

### Checkerboard Test Client

Purpose: prove the renderer does not stretch stale buffers.

1. Launch a simple SHM client with a high-contrast checkerboard at `300x200`.
2. Start resize to `500x350`.
3. ACK the configure but commit either no new buffer or another `300x200`
   buffer.
4. Render a frame.
5. Pass: the checkerboard remains `300x200` or clipped; it is not scaled to
   `500x350`.
6. Commit a real `500x350` buffer.
7. Pass: the new buffer replaces the stale visual and anchors are correct.

### Zen / Firefox Wayland

Suggested diagnostics:

```sh
MOZ_ENABLE_WAYLAND=1 \
MOZ_LOG=Widget:5,WidgetPopup:5 \
MOZ_LOG_FILE=/tmp/firefox-widget.log \
WAYLAND_DISPLAY=<oblivion-socket> \
zen
```

Run:

1. Bottom-right resize slowly.
2. Bottom-right resize quickly.
3. Top-left resize slowly.
4. Top-left resize quickly.
5. Resize while a popup/menu is open.
6. Repeat with hardware cursor active and with software cursor fallback.

Pass:

- no blurred/stretched stale page content;
- resize frame/empty area follows pointer without stale trails;
- final content size replaces the old buffer after compatible commit;
- `Resizing` state clears on release;
- Gecko logs show configure/ACK/commit cadence continuing during drag;
- no visible stall where Firefox waits indefinitely for frame callbacks.

### Chromium / Brave

Run both native Wayland and any XWayland fallback separately:

```sh
WAYLAND_DISPLAY=<oblivion-socket> brave --ozone-platform=wayland
```

Pass criteria are the same for native Wayland. If the behavior differs under
XWayland, classify it as a separate sync-request/XWayland path instead of using
it to judge Gecko Wayland behavior.

### Native Perf / Damage Checks

With `OBLIVION_ONE_PERF_LOG` enabled, collect:

- `resize.begin`, `resize.update`, `resize.end`;
- `native.frame` with damage kind/rects/pixels;
- `native.present_frame` timing;
- `render_generation` and `render_cause`;
- cursor backend.

Pass:

- resize motion with no compatible client commit does not mutate committed
  surface dimensions;
- compatible commit damages old and new visual bounds;
- software cursor fallback does not leave trails;
- hardware cursor motion without resize does not repaint frames;
- resize frame cost trends with changed window region, not full output, once
  old/new damage exists.

## Concrete Takeaways

1. Keep the current ACK behavior. It already matches the reference model:
   `src/compositor/mod.rs:2381-2396` plus
   `src/compositor/tests/windows.rs:235-280`.
2. Do not implement live resize by simply changing `RenderableSurface.width` and
   `height` before the client commits. That feeds directly into
   `src/compositor/render.rs:470-484` and the scaling path at
   `src/compositor/render.rs:1077-1101`.
3. Add explicit resize visual state and draw the compositor-owned frame/empty
   area separately, since `server_frame_rects_for_surface()` is empty today
   (`src/compositor/render.rs:670-679`).
4. Add old/new output damage for move/resize. Current surface-damage use is
   limited to `SurfaceCommit` and `SurfaceDamage`
   (`src/compositor/mod.rs:164-166`), not compositor-owned resize bounds.
5. Preserve configure-only safety. The existing test at
   `src/compositor/tests/windows.rs:372-387` should remain true.
6. Add render-pixel tests for stale-buffer no-stretch. Current tests cover
   protocol and placement, but not rendered pixels during the ACK/commit gap.
7. Split frame lifecycle so resize configures are prepared before paint and
   callbacks/presentation complete after presentation. Hyprland's Firefox
   no-damage callback path (`Monitor.cpp:163-175`) is a direct warning for this.

The core rule is simple: the compositor may resize the window visual; it may not
pretend Gecko has submitted resized pixels before Gecko actually commits them.
