# Hyprland Hardware Cursor and Gecko Resize Study

Date: 2026-06-11

Authoring context: Lagrange / focused engineering research.

Scope: how Hyprland-class compositors reduce pointer, cursor, and resize cost for Firefox/Zen/Gecko on Wayland/NVIDIA-like high-refresh systems, and what that implies for Oblivion's native backend.

Non-goals:

- Do not patch production compositor code in this document.
- Do not use Firefox flags to hide compositor bugs.
- Do not regress existing resize configure/ack tests or changes from other agents.

## Executive Summary

The fastest path toward Hyprland-like mouse feel is not another resize heuristic. It is separating cursor movement from full-scene repaint.

Current Oblivion has already moved in the right direction:

- pointer motion events are coalesced before dispatch (`src/native_output.rs:2548-2606`);
- the input path can theoretically skip frame repaint when `NativeCursorRenderMode::Hardware` is active (`src/native_output.rs:1141-1156`);
- cursor drawing can be omitted from the composed frame in hardware mode (`src/native_output.rs:1217-1224`);
- resize ACK handling now matches the Hyprland shape by promoting the latest resize serial not newer than the ACK (`src/compositor/mod.rs:2296-2310`);
- tests cover the newer ACK behavior (`src/compositor/tests/windows.rs:226-274`);
- perf logging exists through `OBLIVION_ONE_PERF_LOG` and reports paint/render/copy/write cost (`src/native_output.rs:35-39`, `src/native_output.rs:3098-3109`).

Implementation update: the first native hardware-cursor pass now wires
`OBLIVION_ONE_CURSOR=auto|hardware|software`, creates a GBM `ARGB8888` cursor
buffer with `CURSOR | WRITE`, activates it through the DRM cursor ioctl, moves
it through `move_cursor`, and omits software cursor pixels from the scene while
hardware cursor is active. Plain pointer motion can now move the hardware
cursor without forcing a full monitor repaint.

The remaining large cost is the frame path itself when a real repaint is still
needed: the GBM path renders a full CPU scene, converts/copies it into staging,
and writes the whole scanout BO. Resize, new client commits, shell changes, and
software-cursor fallback still need damage tracking and eventually direct
EGL/GLES rendering to get closer to Hyprland/KWin behavior.

For Gecko, this matters because resize is a protocol-paced loop: compositor sends configure, Gecko ACKs, Gecko resizes its Wayland/EGL surface and commits a new buffer. If the compositor burns the 6.06 ms budget at 165 Hz on pointer repaint/copy, resize commits arrive late and look "not real-time" even if configure/ack semantics are correct.

## What Hyprland Does

### Hardware Cursor First

Hyprland's pointer manager keeps software cursor as a fallback state, not the default rendering model.

Relevant evidence:

- `CPointerManager::softwareLockedFor()` returns true only when software cursor is locked or hardware failed (`WM para Referencia/Hyprland-main/src/managers/PointerManager.cpp:92-99`).
- `updateCursorBackend()` tries hardware cursor per monitor and falls back if the output rejects it (`PointerManager.cpp:284-315`).
- `onCursorMoved()` moves the cursor through the output backend when hardware cursor is active (`PointerManager.cpp:319-352`).
- `attemptHardwareCursor()` checks backend pointer capability, renders a cursor buffer, applies it, and marks failure when the backend rejects it (`PointerManager.cpp:360-397`).

Takeaway for Oblivion:

- cursor movement needs its own backend object;
- mouse motion should update cursor plane state, not the desktop framebuffer;
- software cursor must remain available, but it should be a fallback.

### Repaint Suppression on Pointer Motion

Hyprland only schedules a cursor-move render when it actually needs software cursor rendering.

Relevant evidence:

- Input motion checks software cursor lock before scheduling cursor move frame (`WM para Referencia/Hyprland-main/src/managers/input/InputManager.cpp:267-276`).
- A later path repeats the same guard for surfaces/layers below (`InputManager.cpp:552-560`).
- When software cursor is not needed, the cursor frame callback can still be delivered without drawing the cursor in the scene (`PointerManager.cpp:620-664`).

Takeaway for Oblivion:

- `wl_pointer.motion` delivery to clients is not the same as "render a monitor frame";
- pointer-only motion with hardware cursor should flush client input, move cursor plane, and stop;
- pointer motion during resize remains special because it drives `xdg_toplevel.configure`.

### Damage Tracking Instead of Full Monitor Repaint

Hyprland carries damage as regions.

Relevant evidence:

- Render begins with a damage region and only renders main scene when damage is non-empty (`WM para Referencia/Hyprland-main/src/render/Renderer.cpp:2136-2164`).
- `damageSurface()` computes surface damage, translates it into monitor coordinates, and adds damage only to affected monitors (`Renderer.cpp:2698-2752`).
- `damageWindow()` damages a window bounding box for old/new placement and animation cases (`Renderer.cpp:2754-2773`).
- Software cursor fallback damages only the cursor box (`PointerManager.cpp:1118-1130`).

Takeaway for Oblivion:

- the first correct damage implementation can be conservative: old window box + new window box + cursor boxes;
- full-screen damage should be an explicit fallback, not the steady-state response to mouse movement;
- software cursor damage should be two small rectangles, not a scene generation bump.

### Resize Configure/Ack Is Serial-Aware

Hyprland stores size configures with serials and accepts the newest pending size ACK not newer than the incoming ACK.

Relevant evidence:

- `sendWindowSize()` avoids configure spam, sends a toplevel size, and stores `(serial, size)` (`WM para Referencia/Hyprland-main/src/desktop/view/Window.cpp:1633-1655`).
- `onAck()` searches pending size ACKs in reverse and accepts `serial <= ack_serial` (`Window.cpp:1414-1428`).

Takeaway for Oblivion:

- Oblivion's current ACK code is now aligned with this pattern (`src/compositor/mod.rs:2296-2310`);
- keep the test `ack_configure_promotes_latest_resize_not_newer_than_ack_serial` as a regression guard (`src/compositor/tests/windows.rs:226-274`);
- next resize work should focus on frame timing, damage, and avoiding buffer stretch rather than reworking ACK again.

### Resize Does Not Stretch Old Client Buffers

Hyprland has an interactive-resize guard in the surface pass.

Relevant evidence:

- `SurfacePassElement::getTexBox()` detects resize drag (`WM para Referencia/Hyprland-main/src/render/pass/SurfacePassElement.cpp:20-27`).
- In normal mode it may correct/scale a small surface to the real window size, but during interactive resize it uses the current surface size instead (`SurfacePassElement.cpp:42-50`).

Takeaway for Oblivion:

- if Oblivion adds a live resize target rectangle, do not feed that target into the old-buffer blit path as `surface.width/height`;
- keep committed buffer size and interactive window target as separate concepts;
- draw frame/background/empty area separately while Gecko catches up.

## Oblivion Current State

### What Is Already Good

Pointer motion coalescing exists:

- the loop drains raw events and calls `coalesce_pointer_motion_events()` (`src/native_output.rs:670-676`);
- relative motion is accumulated until a non-motion event, and absolute motion keeps the latest position (`src/native_output.rs:2548-2606`).

Cursor repaint suppression has a model:

- `NativeCursorRenderMode::{Software,Hardware}` exists (`src/native_output.rs:843-856`);
- `NativeInputEffect::requires_frame_repaint()` skips repaint for pure cursor motion in hardware mode (`src/native_output.rs:1141-1156`);
- `NativeInputState::desktop_visual_state()` omits cursor pixels in hardware mode (`src/native_output.rs:1217-1224`);
- `apply_native_input_effect()` initializes `redraw_requested` from `effect.requires_frame_repaint(cursor_mode)` (`src/native_output.rs:2762-2770`).

Resize ACK is now Hyprland-like:

- `ack_xdg_surface_configure()` chooses latest resize `<= serial` and retains only newer serials (`src/compositor/mod.rs:2296-2310`);
- tests cover exact ACK and non-exact newer ACK (`src/compositor/tests/windows.rs:200-274`).

Native perf hooks exist:

- `OBLIVION_ONE_PERF_LOG` enables perf output (`src/native_output.rs:35-39`);
- paint stats include `paint_us`, `render_us`, `copy_us`, and `write_us` (`src/native_output.rs:3098-3109`);
- the loop logs native frame fields including raw/coalesced input counts (`src/native_output.rs:734-742`).

### What Still Costs Too Much

Hardware cursor needs real-session validation:

- `OBLIVION_ONE_CURSOR=auto` should report `native cursor backend active:
  hardware` and `perf native.cursor backend=hardware` when GBM/DRM accepts the
  cursor buffer;
- if the driver rejects the cursor buffer or cursor move ioctl, the compositor
  falls back to `cursor=software`, which intentionally restores full-frame
  cursor repaint behavior until damage tracking is added;
- `OBLIVION_ONE_CURSOR=software` is the comparison switch for measuring the old
  cursor path against the hardware path.

Native scanout still renders and copies full frames:

- the repaint condition includes `redraw_requested` and `pending_frame_work` (`src/native_output.rs:706-718`);
- GBM paint renders a full `NativeFrameRenderer` frame, converts ARGB to XRGB into staging, and writes to the scanout BO (`src/native_output.rs:3370-3395`);
- the dumb framebuffer path also renders full frame and copies into the mapping (`src/native_output.rs:3680-3690`).

Compositor render still works as full-frame CPU composition:

- `DesktopSceneRenderer::compose_request()` rebuilds/copies the scene and then blends shell/cursor (`src/compositor/render.rs:108-135`);
- `blit_surface_to_rect()` scales old buffer content when buffer size differs from target size (`src/compositor/render.rs:643-723`);
- there is no output damage API passed into `compose_request`.

Frame lifecycle is still too monolithic:

- `present_frame()` commits ready explicit sync buffers, flushes resize configure, releases buffers, completes callbacks, sends presentation feedback, and flushes clients in one method (`src/compositor/server.rs:222-229`);
- `has_pending_frame_work()` correctly includes pending resize configure (`src/compositor/mod.rs:2625-2629`), but completion is still tied to loop/present approximation rather than actual pageflip completion.

## Gecko / Firefox / Zen Notes

### Why Compositor Behavior Matters More Than Firefox Tweaks

Gecko on Wayland sizes its native surface after GTK/Wayland resize notifications and compositor widget updates. Mozilla source shows:

- GTK/Wayland display detection uses GDK display type, with XWayland detection based on a non-Wayland GDK display plus `WAYLAND_DISPLAY` ([Searchfox WidgetUtilsGtk.cpp](https://searchfox.org/firefox-main/source/widget/gtk/WidgetUtilsGtk.cpp#649-681)).
- GTK compositor widget configures the Wayland backend when `GdkIsWaylandDisplay()` is true ([Searchfox GtkCompositorWidget.cpp](https://searchfox.org/firefox-main/source/widget/gtk/GtkCompositorWidget.cpp#126-138)).
- Wayland-only EGL window sizing is explicit in `SetEGLNativeWindowSize()` ([Searchfox GtkCompositorWidget.cpp](https://searchfox.org/firefox-main/source/widget/gtk/GtkCompositorWidget.cpp#278-294)).
- `NotifyClientSizeChanged()` updates the compositor widget client size ([Searchfox GtkCompositorWidget.cpp](https://searchfox.org/firefox-main/source/widget/gtk/GtkCompositorWidget.cpp#204-217)).

That means the compositor should:

- send configure promptly;
- handle ACK serials correctly;
- avoid stretching an old buffer during the configure/commit gap;
- deliver frame callbacks/presentation at the right time;
- avoid spending the resize budget on cursor-only repaint.

### Flags and Envs Relevant to Gecko Validation

Use these as diagnostics, not permanent "fixes".

| Variable / pref | Use | Notes |
| --- | --- | --- |
| `MOZ_ENABLE_WAYLAND=1` | Force/ensure Firefox starts on Wayland in distro builds that still need it. | If GTK already selects Wayland by session, this may be redundant. Validate with `about:support` Window Protocol or logs. |
| `GDK_BACKEND=wayland` | Force GTK to choose Wayland backend. | Useful to avoid accidentally testing XWayland. `GDK_BACKEND=x11` is a negative control. |
| `WAYLAND_DISPLAY=<socket>` | Ensure Gecko connects to the Oblivion socket. | Oblivion already guards native scanout when host display vars exist (`src/native_output.rs:3817-3825`). |
| `MOZ_LOG=Widget:5,WidgetPopup:5` | Inspect GTK/Wayland widget configure, popup, resize, and compositor-widget messages. | Searchfox shows widget logging through `gWidgetLog`/`gWidgetPopupLog` in GTK code ([WidgetUtilsGtk.cpp](https://searchfox.org/firefox-main/source/widget/gtk/WidgetUtilsGtk.cpp#535-548), [GtkCompositorWidget.cpp](https://searchfox.org/firefox-main/source/widget/gtk/GtkCompositorWidget.cpp#78-89)). |
| `MOZ_LOG_FILE=/tmp/firefox-widget.log` | Save Gecko widget logs without flooding terminal. | Pair with `MOZ_LOG`. |
| `MOZ_WEBRENDER=1` | Historical/diagnostic only. | WebRender is generally default in modern Firefox; do not treat this as a compositor-side fix. |
| `gfx.webrender.compositor.force-enabled` | Diagnostic for Wayland native layer/WebRender compositor paths. | Searchfox shows Wayland NativeLayerRoot is conditional on `gfxVars::UseWebRenderCompositor()` ([GtkCompositorWidget.cpp](https://searchfox.org/firefox-main/source/widget/gtk/GtkCompositorWidget.cpp#317-344)). Keep this as an experiment, not a required user setting. |
| `widget.dmabuf.force-enabled` / dmabuf prefs | Diagnostic for buffer path/video/zero-copy issues. | Useful for NVIDIA/DMABUF experiments, but it does not solve pointer repaint or resize ACK timing by itself. |
| `MOZ_USE_XINPUT2=1` | X11/XWayland input diagnostic only. | Not a Wayland-native resize or cursor solution. Avoid using it in the main Wayland validation path. |

Suggested Gecko launch for reproducing Wayland resize behavior:

```sh
MOZ_ENABLE_WAYLAND=1 \
GDK_BACKEND=wayland \
MOZ_LOG=Widget:5,WidgetPopup:5 \
MOZ_LOG_FILE=/tmp/gecko-widget.log \
firefox
```

Suggested Oblivion-side native validation:

```sh
OBLIVION_ONE_NATIVE_SCANOUT=1 \
OBLIVION_ONE_MODE=1920x1080@165 \
OBLIVION_ONE_SCANOUT_BACKEND=gbm \
OBLIVION_ONE_CURSOR=auto \
OBLIVION_ONE_PERF_LOG=1 \
./target/debug/oblivion-one
```

Relevant Oblivion envs:

- `OBLIVION_ONE_MODE` parses native mode policy (`src/native_output.rs:293-307`).
- `OBLIVION_ONE_SCANOUT_BACKEND` chooses GBM/pageflip vs dumb framebuffer (`src/native_output.rs:3120-3138`).
- `OBLIVION_ONE_CURSOR` chooses native cursor policy: `auto`, `hardware`, or `software`.
- `OBLIVION_ONE_NATIVE_SCANOUT=1` bypasses host-display guard (`src/native_output.rs:3817-3825`).
- `OBLIVION_ONE_PERF_LOG=1` enables frame/input paint logs (`src/native_output.rs:35-39`).

## Recommendations

### P0: Validate Real Native Hardware Cursor Backend

Goal: prove the first `NativeCursorRenderMode::Hardware` implementation works on the target SDDM/TTY session and quantify the improvement.

Validation shape:

1. Run a release SDDM/TTY session with `OBLIVION_ONE_CURSOR=auto` and `OBLIVION_ONE_PERF_LOG=1`.
2. Confirm `native cursor backend active: hardware` and `perf native.cursor backend=hardware`.
3. Move only the pointer on an idle desktop and confirm motion does not emit full `native.frame phase=repaint` lines at mouse rate.
4. Repeat with `OBLIVION_ONE_CURSOR=software` as the baseline comparison.
5. If the driver falls back, keep the fallback log and inspect whether GBM cursor allocation, `set_cursor`, or `move_cursor` failed.

Validation:

- `native_input_pointer_motion_can_skip_frame_repaint_with_hardware_cursor` already proves the high-level input model (`src/native_output.rs:4447-4458`).
- Add integration/perf assertion: with hardware cursor enabled, moving mouse over an idle desktop should not emit `native.frame phase=repaint` for every motion event.

Risk:

- NVIDIA/DRM cursor plane support can be strict about format, size, modifiers, hotspot, and atomic/legacy APIs. Fallback must be automatic.

### P1: Add Software Cursor Damage-Only Fallback

Goal: when hardware cursor is unavailable, software cursor should damage old and new cursor boxes, not the full output.

Implementation shape:

- Track previous and current cursor rect in output coordinates.
- Feed those rects into a new output damage accumulator.
- Render/copy only those rects when no other damage exists.

Validation:

- Perf log should show pointer-only motion with much lower `bytes`, `copy_us`, and `paint_us` once dirty-copy is supported.
- A cursor move over a static client should update old/new cursor pixels without stale trails.

Risk:

- Current CPU scene renderer copies whole scene to frame (`src/compositor/render.rs:108-135`). Damage-only cursor requires either dirty compose or a retained front buffer model.

### P2: Introduce Output Damage for Window/Surface Commits

Goal: reduce resize cost from "full monitor every motion/commit" to "changed window regions".

Implementation shape:

- On surface commit: transform `RenderableSurfaceDamage` to output coordinates.
- On window move/resize target change: damage old and new window boxes.
- On shell overlay changes: damage topbar/dock/spotlight bounds.
- Keep full-output damage as a conservative fallback for unknown paths.

Validation:

- Resize Gecko while perf logging. `copy_us` should not scale with full 1920x1080 on every step once damage lands.
- Add unit tests for union of old/new resize bounds and cursor old/new bounds.

Risk:

- Under-damaging is visually worse than full repaint. Start conservative.

### P3: Split Frame Lifecycle Around Pageflip

Goal: send resize configure before render, but complete frame callbacks/presentation after actual pageflip.

Implementation shape:

- Split `present_frame()` into:
  - `prepare_frame_protocol()`: commit ready explicit sync, flush color, flush resize configure, flush clients;
  - `finish_presented_frame()`: release buffers, complete callbacks, complete presentation feedback.
- For GBM/pageflip, call finish only when `drain_page_flip_events()` observes completion.
- For dumb framebuffer, keep an immediate fallback with explicit docs.

Validation:

- Gecko logs should show configure/ACK/commit cadence close to pointer cadence.
- Presentation feedback timestamps should align with pageflip completion, not CPU paint.

Risk:

- Changes callback timing. Test with SHM clients, Gecko, popups, and explicit sync clients.

### P4: Guard Against Old-Buffer Stretch During Resize

Goal: preserve Hyprland behavior where old Gecko buffers are not stretched into a future size while waiting for a new commit.

Implementation shape:

- Keep committed surface size separate from desired toplevel target size.
- If interactive resizing and buffer size differs from target size, blit old buffer at committed size and fill/damage the extra area separately.
- Do not reuse `blit_surface_to_rect()` scaling fallback for interactive toplevel resize without an explicit viewport reason.

Validation:

- Checkerboard test client: resize without commit should not show scaled checkerboard.
- Gecko/Zen: during slow resize, old content should stay crisp or show empty/marginal area, not blurred stretched content.

Risk:

- Need to distinguish legitimate viewport/fractional-scale stretching from interactive resize stretching.

## Measurement Plan

Run four cases and compare `native.frame` perf output:

1. Idle desktop, mouse motion only.
2. Firefox open, mouse motion over content, no resize.
3. Firefox bottom-right resize.
4. Firefox top-left resize.

Metrics to extract:

- `raw_input_events` vs `coalesced_input_events`;
- `paint_us`, `render_us`, `copy_us`, `write_us`;
- frame count during pointer-only motion;
- time from resize configure to Gecko ACK to Gecko commit;
- whether frame budget stays below `~6060 us` for 165 Hz.

Expected end-state:

- pointer-only motion with hardware cursor emits no full repaint;
- software fallback emits only cursor-region damage;
- Gecko resize repaint cost is dominated by changed window damage, not whole output;
- frame callbacks and presentation feedback progress from pageflip completion.

## Practical Conclusion

For Firefox/Zen/Gecko on NVIDIA-class native sessions, the compositor has to stop treating cursor motion as scene damage. Hyprland wins here because hardware cursor and damage tracking remove pointer movement from the expensive render path. Oblivion now has the first real KMS cursor backend with automatic software fallback, but scanout still copies full frames when repaint is genuinely needed. The next engineering milestone should be software-cursor damage fallback, broader output damage, direct EGL/GLES scanout rendering, and pageflip-driven frame completion.

## Sources

Local source studied:

- `WM para Referencia/Hyprland-main/src/managers/PointerManager.cpp`
- `WM para Referencia/Hyprland-main/src/managers/input/InputManager.cpp`
- `WM para Referencia/Hyprland-main/src/render/Renderer.cpp`
- `WM para Referencia/Hyprland-main/src/desktop/view/Window.cpp`
- `WM para Referencia/Hyprland-main/src/render/pass/SurfacePassElement.cpp`
- `WM para Referencia/Hyprland-main/src/output/Monitor.cpp`
- `src/native_output.rs`
- `src/compositor/mod.rs`
- `src/compositor/server.rs`
- `src/compositor/render.rs`
- `src/compositor/tests/windows.rs`

External primary source references:

- Mozilla Searchfox: [WidgetUtilsGtk.cpp Wayland/XWayland detection and widget logging](https://searchfox.org/firefox-main/source/widget/gtk/WidgetUtilsGtk.cpp#535-681)
- Mozilla Searchfox: [GtkCompositorWidget.cpp Wayland backend, client size, EGL window size, NativeLayerRoot](https://searchfox.org/firefox-main/source/widget/gtk/GtkCompositorWidget.cpp#126-344)
