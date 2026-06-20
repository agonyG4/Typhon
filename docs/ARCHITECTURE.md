# Oblivion One Architecture

Oblivion One is not a Hyprland theme and not a shell-only prototype. The core
goal is an owned Wayland compositor and window manager, with the desktop shell
built after real app hosting is reliable.

## Layer Contract

```text
core
  Shared primitives: geometry, paths, shell quoting, tool lookup.

render_backend
  Declares renderer capabilities, committed surface buffer types, and the target
  split between the native EGL/GLES backend and the CPU fallback/debug path.

compositor
  Owns the Wayland display, socket, protocol dispatch, client lifecycle, and
  the nested render loop used while the real EGL/GLES render/input backend
  grows.
  `plan.rs` holds the public architecture/protocol plan.
  `surface.rs` holds renderable surface, damage, and placement types.
  `interaction.rs` holds floating-window hit-testing and move/resize geometry.
  `window_state.rs` holds minimize, maximize, fullscreen, and restore state.
  `shell.rs` is the temporary compositor-rendered shell overlay root used for
  testing real apps before the full DE shell exists. Its overlay-specific code
  lives under `shell/`: `dock.rs` tracks open app affordances,
  `spotlight.rs` owns lab app launching, and `canvas.rs` owns the shared
  overlay drawing/blending helpers.
  `render.rs` holds scene composition helpers and CPU render fallback logic.

wm
  Owns window policy: focus, floating placement, move, resize, maximize,
  minimize, close, and future workspace rules.

shell
  Owns DE surfaces: dock, topbar, launcher, notifications, settings, wallpaper.
  The real shell is still deferred, but the compositor has a small built-in
  dock/Spotlight lab overlay so real app launching and open-app visibility can
  be tested before a separate shell protocol exists.

session
  Owns launch lifecycle: nested during development, experimental SDDM/native
  startup, then the production logind/libseat/libinput/GBM path.

xwayland
  Owns the future X11 compatibility bridge: rootless XWayland launch planning,
  isolated DISPLAY policy, compositor-owned listen/wm/display file descriptors,
  and the future XWM lifecycle.
```

## Current Milestone

The current milestone is deliberately small:

```text
bind our own Wayland socket      done
accept clients                   done
add xdg-shell registry/toplevel  done
render committed shm surfaces    done
draw simple desktop/cursor       done
add output/seat/frame baseline   done
add subcompositor compatibility  done
add xdg-popup menu baseline      done
add data-device compatibility    done
forward basic keyboard/pointer   done
move and resize a floating window done
import dmabuf into EGL textures   done
```

`./bin/oblivion-one compositor --check` already validates the first part of that
path without calling Hyprland or Gamescope.

`./bin/oblivion-one compositor -- wayland-info` validates the next part: a real
client connects to the Oblivion socket and reads the compositor registry. The
test suite also creates an `xdg_toplevel` through `wayland-client`, so the server
path is no longer just a socket stub.

The compositor also tracks `wl_shm` pools and buffers. When a client attaches a
shm buffer to a `wl_surface` and commits it, Oblivion copies that buffer into a
renderable snapshot. The nested output window composites those snapshots over a
simple procedural desktop wallpaper, then presents through a native EGL/GLES
Wayland window when available. The older `softbuffer` renderer remains as an
explicit CPU fallback.
The nested output keeps the host cursor visible by default and treats it like a
hardware-cursor-style path. Pointer motion is still forwarded to clients, but
mouse movement no longer forces a full scene redraw unless a client commits new
content or a window interaction changes geometry.

The nested renderer caches the wallpaper per output size and redraws on output
resize or client render generation changes. The default compositor path is now
native GPU `egl-gles`; CPU is an explicit fallback/debug path. EGL/GLES keeps
wallpaper, cursor fallback, and client surfaces as GL textures, then composites
them as quads. It detects
`EGL_KHR/EXT_swap_buffers_with_damage` and passes conservative output damage to
the host compositor when supported. `wl_shm` client buffers are still copied
from CPU memory when committed, but same-size commits track
`wl_surface.damage`/`damage_buffer` and update only the damaged CPU pixels and
GPU texture rectangles. Narrow damaged rectangles are packed densely before GL
texture upload, so the upload does not carry full-window row padding.

The owned nested output is configured through
`oblivion-one compositor --output nested --width W --height H --refresh R`.
`W` and `H` are initial logical host-window dimensions; after the user resizes
the host window, the compositor follows the actual Winit window size and updates
`wl_output.mode` size without forcing the window back to the CLI dimensions.
`R` is validated as a numeric target refresh, stored in `NestedOutputConfig`,
advertised through `wl_output.mode` as millihertz, and used to derive the active
wakeup interval with integer nanosecond division. This target remains
host-paced: unchanged scenes do not render merely because the interval elapsed,
and the host compositor/monitor can still cap physical presentation below the
advertised nested refresh.

The compositor advertises `zwp_linux_dmabuf_v1` version 3 with ARGB/XRGB linear
and implicit modifiers. Async `create` still uses the protocol's non-fatal
`failed` event, but `create_immed` creates `wl_buffer` resources carrying typed
`DmabufBufferHandle` metadata and FDs through `wl_surface.attach`/`commit`.
The native EGL/GLES renderer builds modifier-aware
`EGL_LINUX_DMA_BUF_EXT` attributes for those handles, creates EGLImages, and
binds them as GL textures through `GL_OES_EGL_image`.
Committed dmabuf buffers stay owned by the compositor until the surface commits
a replacement buffer or is destroyed, so the client cannot recycle a buffer that
is still backing an active GL texture.
Window shadows are disabled in all active compositor render paths. The current
window visual model treats visible bounds as client content plus temporary
resize preview backdrop/outline only; shadow extents do not participate in hit
testing, damage, scene bounds, or GPU command generation.
Clients receive `wl_surface.frame.done` after the nested output presents a frame,
and the registry now includes `wl_subcompositor`, `wl_data_device_manager`, one
`wl_output`, plus a pointer/keyboard-capable `wl_seat`. Keyboard clients can
request an initial XKB keymap, and the nested output forwards basic physical
keyboard events plus pointer enter/motion/button events into the focused xdg
surface.
The xdg-shell path also handles the first real `xdg_popup` menu flow:
positioners track anchor rect, anchor, gravity, offset, parent size, and
constraint adjustment; popups receive configure events; `xdg_popup.reposition`
sends `repositioned` plus a fresh configure; and committed popup buffers are
rendered as child surfaces above their parent toplevel. The popup path also
tracks `xdg_surface.set_window_geometry`, honors `wl_surface.offset`, and sends
`wl_surface.enter` on committed surfaces so real clients receive an output
association like they do on mature compositors. This is enough for the browser
menu baseline, but popup grabs are still intentionally minimal.

The CPU renderer keeps a wallpaper cache by output size and a composed-scene
cache by client render generation. Cursor-only redraws reuse the composed scene
and draw only the cursor overlay. Same-layout surface commits with explicit
partial damage repair only the damaged output rectangles in the cached scene,
redrawing intersecting surfaces in stacking order. Layout, size, scale, stacking,
and full-damage changes still fall back to a conservative full scene rebuild.
When a client commit changes logical window bounds, native output damage now
combines committed surface damage with old/new visible bounds so stale pixels
from the previous rectangle are repainted.
Native scanout now has three explicit paths. `native-egl-gbm` is the normal
target in `auto`: the shared `GlesSceneRenderer` draws the same wallpaper,
surface, frame, cursor, and shell-overlay layers used by nested EGL/GLES into a
GBM-backed EGL window surface, then KMS pageflips the locked GBM front buffer.
`gbm-cpu-write` is the retained diagnostic fallback: it converts damaged CPU
scene rectangles into a staging buffer but still submits the full staging BO via
`gbm_bo_write`. `dumb-framebuffer` remains the last-resort KMS mapping path.
Legacy environment aliases such as `gbm` and `gbm-egl` continue to select the
CPU-write fallback for compatibility.

Native runtime scheduling is split between `src/native/event_loop.rs` and
`src/native/scheduler.rs`. The reactor registers the DRM fd, Wayland listening
socket, Wayland backend dispatch fd, libinput or raw evdev fds, and one
`CLOCK_MONOTONIC` timerfd. The scheduler owns visual/protocol work, pageflip
pending state, absolute refresh deadlines, and the pageflip watchdog. A timer
can advance protocol-only work or report a watchdog failure, but it cannot
invent presentation completion; asynchronous frame completion is accepted only
after the scanout backend drains a DRM flip-complete event.

Legacy pageflip submissions carry a unique nonzero `u64` user-data token. The
DRM event parser validates each event length, matches that token, and preserves
the kernel seconds, microseconds, and finite-width sequence through compositor
frame completion. Native setup queries `DRM_CAP_TIMESTAMP_MONOTONIC` and
advertises the matching `CLOCK_MONOTONIC` or `CLOCK_REALTIME` ID through
`wp_presentation`. Legacy synchronized flips report `VSYNC`; `HW_CLOCK`,
`HW_COMPLETION`, and `ZERO_COPY` remain unset because the current path does not
establish those semantics conservatively.

For native output, optional GPU buffer protocols are bound after the active
scanout backend is known. The base Wayland socket starts without
`zwp_linux_dmabuf_v1`, explicit sync, or `wl_drm`; those globals are published
only when the final backend is `native-egl-gbm`. If `auto` falls back to
`gbm-cpu-write` or `dumb-framebuffer`, new clients see a CPU-safe registry and
the app launch profile resolves to software unless the user explicitly asked
for CPU already. An explicit `OBLIVION_ONE_NATIVE_APP_GPU=gpu` fails on those
CPU backends instead of silently degrading.

The `render_backend` module records graphics capabilities explicitly:
`egl-gles` is the main GPU target for dmabuf import, feedback, explicit sync,
direct scanout, and multi-GPU support. This keeps graphics backend choices out
of WM policy code and leaves a clean place for a future non-GL renderer without
shipping a Vulkan option before it is product-quality.
Committed client content is also typed at this boundary:
`ShmBufferSnapshot` keeps the current CPU fallback path, while
`DmabufBufferHandle` preserves the future zero-copy import metadata and FDs. The
compositor no longer treats every surface as an unconditional `Vec<u32>`.
The EGL/GLES import contract now builds dmabuf EGL import attributes that never
require CPU pixels. The runtime renderer owns the actual EGLImage import and
texture lifetime for both nested and native GPU composition. Explicit sync is
present in the protocol/buffer lifecycle and still needs real native GPU
hardware validation before claiming driver-complete synchronization behavior;
direct scanout remains a planned optimization, not an enabled scanout path.

The remaining gap before using normal desktop apps comfortably is protocol and
WM breadth: real floating placement/move/resize, richer focus policy, and then
decoration/activation details for bigger toolkits.

## XWayland Policy

Oblivion One is Wayland-first. Apps launched by the owned compositor path do not
inherit the host `DISPLAY`; the default launch environment removes it and points
`WAYLAND_DISPLAY` at the Oblivion socket. This is intentional while the
compositor is nested inside another desktop, because otherwise X11/XCB fallback
would open windows outside Oblivion.

X11 compatibility must be an Oblivion-owned bridge. The `xwayland` layer now
records the rootless launch shape expected by real compositors: XWayland runs as
a Wayland client on the Oblivion socket and receives compositor-owned
`-listenfd`, `-wm`, and `-displayfd` file descriptors. The app environment also
has an explicit isolated-XWayland mode that exposes only the bridge display and
marks it as `OBLIVION_ONE_XWAYLAND_DISPLAY`.

Runtime X11 app hosting is still intentionally disabled. Rootless XWayland needs
the compositor to act as an X window manager over the `-wm` socket before X11
windows can be mapped with product-quality behavior. Adding that XWM is the next
real implementation step; exporting the host `DISPLAY` is not an acceptable
shortcut.

## Legacy Backends

Hyprland and Gamescope are legacy lab paths. They remain useful for comparison
and quick visual tests, but they are not the architecture target.

## Native Session

The native SDDM session is installable for testing, but the current backend is
still a transitional implementation. See `docs/NATIVE_SESSION.md` for the
session launcher, logging path, input/keymap contract, and the production gaps
that remain before it should be treated like a mature KWin/Hyprland-class TTY
backend.
