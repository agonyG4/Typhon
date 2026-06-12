# Known Issues

## Native SDDM session is experimental

Status: installable for testing; not yet production-grade

`bin/install-start-oblivion-one --sddm-session` can install the Wayland session
entry and the launcher now runs native sessions from the release binary by
default. This makes SDDM testing reproducible, but the native backend still uses
a transitional CPU-filled GBM/dumb scanout path.

`oblivion-one doctor` reports the current native-session matrix: runtime dir,
KMS/render devices, connected output, seat/libinput/GBM/EGL prerequisites, and
raw input fallback availability. Treat `host prerequisites available; backend
still experimental` as a good host signal, not as a claim that the native
backend has already reached production parity.

Current limits:

- input now prefers libseat + libinput and handles libseat enable/disable for
  libinput, coalesces native pointer motion batches, and uses `Alt+P` as the
  native emergency exit shortcut, but direct libinput/raw fallbacks remain for
  diagnostics;
- DRM/KMS is opened through the shared libseat session when possible, but direct
  DRM fallback remains for diagnostics;
- KMS mode selection now supports `OBLIVION_ONE_MODE` and the installed
  SDDM/TTY paths default to `1920x1080@165` for this test cycle;
- native scanout now attempts GBM/KMS pageflips, but the GBM buffers are still
  filled by the CPU renderer and may fall back to a KMS dumb framebuffer;
- active wakeups, `wl_output.mode`, and presentation feedback follow the
  selected KMS refresh rate. On GBM/KMS, frame callbacks, buffer releases, and
  presentation feedback are now completed after DRM pageflip completion, but
  the native loop still polls DRM with a zero-timeout check and then sleeps from
  a refresh-derived timer instead of using DRM/libinput readiness as the wake
  source;
- VRR is not implemented yet: the compositor does not query `vrr_capable`, set
  the DRM `VRR_ENABLED` property, or expose an `off/on/fullscreen` policy;
- interactive resize now keeps resize commits tied to compatible client sizes
  and promotes the latest acked resize serial, but Zen/Gecko still needs live
  TTY validation against NVIDIA and real browser buffers;
- output revoke/suspend/resume handling is not yet centralized around pageflip
  recovery.

Expected direction:

Move from CPU-filled GBM buffers to EGL/GLES rendering into GBM targets and wire
output revoke/resume before treating SDDM performance as comparable to KWin,
Hyprland, or other mature compositors.

## Brave/Chromium Vulkan warning on Wayland

Status: observed; no automatic compositor-side workaround

Observed command:

```bash
./target/release/oblivion-one compositor -- brave
```

Observed symptoms:

- Brave logs `--ozone-platform=wayland is not compatible with Vulkan`.
- The compositor stays on the native EGL/GLES renderer.
- The app creates an xdg toplevel and renderable surfaces inside the Oblivion
  socket.
- When the compositor is killed by a smoke-test timeout, Brave may log zygote or
  pipe errors during shutdown.

Likely cause:

Chromium can still decide to start its own Vulkan GPU process on Wayland. That
warning is from the client process, not from Oblivion's output renderer. The
Oblivion side now uses a native EGL/GLES Wayland output and imports
`create_immed` dmabuf buffers through EGLImage when clients send them.

Current policy:

The direct compositor launcher preserves the application's command-line
arguments. This matches the model used by production compositors: the
compositor exposes Wayland protocols/capabilities, while Chromium decides its
own GL/Vulkan/VAAPI path.

```bash
./target/release/oblivion-one compositor -- brave
```

Temporary browser flags can still be used manually for debugging, but they do
not belong in the compositor core.

Candidate fixes / hardening:

- Implement full dmabuf feedback table/tranche events once we move past
  `zwp_linux_dmabuf_v1` v3.
- Revisit hardware video decode once the compositor has more protocol coverage
  and the local NVIDIA/Brave VAAPI path is stable enough to measure.

## Legacy X11 apps do not run yet

Status: intentional; architecture scaffolded, runtime bridge not enabled

Apps launched through `oblivion-one compositor -- ...` do not inherit the host
`DISPLAY`. If an app cannot speak Wayland and only supports X11/XCB, it should
fail instead of opening on the host desktop. That keeps nested testing honest.

The future compatibility path is an Oblivion-owned rootless XWayland bridge. The
code now has an isolated app-environment policy and an XWayland launch plan with
`-listenfd`, `-wm`, and `-displayfd`, but real X11 windows still need an XWM
implementation before the bridge can be enabled for users.
