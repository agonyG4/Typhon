# Known Issues

## Native SDDM session is experimental

Status: installable for testing; not yet production-grade

`bin/install-start-oblivion-one --sddm-session` can install the Wayland session
entry and the launcher now runs native sessions from the release binary by
default. This makes SDDM testing reproducible. The native backend now has a
`native-egl-gbm` GPU scanout path, but it is still experimental until real
TTY/KMS runs validate it across local hardware and drivers.

`oblivion-one doctor` reports the current native-session matrix: runtime dir,
KMS/render devices, connected output, seat/libinput/GBM/EGL prerequisites, and
raw input fallback availability. Treat `host prerequisites available; backend
still experimental` as a good host signal, not as a claim that the native
backend has already reached production parity.

Current limits:

- input now prefers libseat + libinput, but `NativeRuntime` owns libseat
  enable/disable for all input backends, coalesces native pointer motion batches, and uses `Alt+P` as the
  native emergency exit shortcut, but direct libinput/raw fallbacks remain for
  diagnostics;
- DRM/KMS is opened through the shared libseat session when possible. After a
  managed seat is selected, auto mode does not fall back to direct DRM;
- Atomic KMS is selected by default when capability discovery and the initial
  test-only transaction succeed. `OBLIVION_ONE_KMS_MODE=atomic` requires it and
  `OBLIVION_ONE_KMS_MODE=legacy` retains the old path. Real atomic takeover,
  mixed legacy-cursor/atomic-primary operation, VT cycling, and exact restore
  still require physical TTY validation on the target RTX 3060 Ti;
- KMS mode selection now supports `OBLIVION_ONE_MODE` and the installed
  SDDM/TTY paths default to `1920x1080@165` for this test cycle;
- native scanout now tries `native-egl-gbm` first in `auto`. That path renders
  with the shared EGL/GLES scene renderer into a GBM-backed EGL surface, locks
  the GBM front buffer, and pageflips the cached DRM framebuffer. The
  `gbm-cpu-write` and KMS dumb framebuffer paths remain explicit fallback/debug
  modes. Legacy values such as `gbm` and `gbm-egl` still select the CPU-write
  path;
- compositor-owned window shadows are disabled. Interactive resize uses the
  compositor-owned visual target without adding a backdrop, border, tint,
  shadow, or outline; richer decorations/shadows remain deferred to a later
  milestone;
- startup fallback is limited to backend creation and the first paint before
  clients are launched. Once the session is running, GPU scanout failures are
  fatal runtime errors with stage/backend/frame diagnostics and a recommended
  restart command such as
  `OBLIVION_ONE_SCANOUT_BACKEND=cpu OBLIVION_ONE_NATIVE_APP_GPU=cpu`;
- native app GPU policy is resolved from the active backend. CPU-write and dumb
  fallback sessions do not publish GPU buffer globals and default apps to the
  software profile; `OBLIVION_ONE_NATIVE_APP_GPU=gpu` requires
  `native-egl-gbm`;
- `wl_output.mode` and presentation feedback follow the selected KMS refresh
  rate. The native loop now blocks on DRM, Wayland, input, and absolute timerfd
  readiness; GBM/KMS frame callbacks, buffer releases, and presentation
  feedback complete only after a token-matched DRM pageflip completion and use
  its kernel timestamp and sequence. Accurate real-hardware validation across
  drivers remains outstanding;
- `libseat` 0.2.4 exposes a pollable seat event fd through its safe API. The
  runtime registers and dispatches it independently of input. Suspend/resume
  hardware validation across physical TTY/KMS drivers remains outstanding.
  Explicit-sync acquire points use syncobj
  eventfds on supported native DRM devices. Drivers returning `ENOTTY`,
  `EOPNOTSUPP`, or `ENOSYS` use a bounded absolute retry deadline only while a
  fallback point remains blocked. Cross-driver real-hardware coverage remains
  outstanding;
- VRR is not implemented yet: the compositor does not query `vrr_capable`, set
  the DRM `VRR_ENABLED` property, or expose an `off/on/fullscreen` policy;
- Atomic property discovery does not imply later display features: direct
  scanout, cursor-plane migration, overlay planes, KMS in/out fences,
  `FB_DAMAGE_CLIPS`, hotplug policy, and multi-output commits are not
  implemented;
- interactive resize now frame-paces raw pointer targets, keeps visual geometry
  independent from committed client content, and uses a bounded sent-configure
  ledger with coalesced ACKs so newer browser sizes can be reported without
  waiting for every older buffer commit. It crops stale content rather than
  stretching it and damages old/new bounds even when a browser commit changes
  logical size. Rapid re-resize uses interaction IDs so obsolete unsent final
  state from an older drag cannot outrank a newer active resize, and delayed
  older commits cannot clear a newer active visual resize. Intermediate
  `resizing=true` commits preserve the visual box; final `resizing=false`
  commits complete it. Zen/Gecko
  and Kitty rapid release/re-grab cycles still need live TTY validation against
  NVIDIA and real browser/terminal buffers;
- `wl_surface.damage` and `wl_surface.damage_buffer` are now stored separately
  and converted at commit for the integer-scale and viewport-destination paths
  Typhon implements. Unsupported or ambiguous mappings conservatively repaint
  the full surface. Arbitrary buffer transforms and viewport source cropping
  remain unsupported protocol breadth, not an under-damage path;
- partial GLES repaint remains opt-in with
  `OBLIVION_ONE_ENABLE_PARTIAL_REPAINT=1`. The software swapchain oracle passes,
  but the legacy/full, Atomic/full, legacy/partial, and Atomic/partial real-TTY
  matrix has not been run in this development environment. Full repaint is the
  default for every real visual frame until that hardware validation is done;
- synchronized subsurface buffers, placement, stacking, callbacks, feedback,
  and acquire dependencies now publish as one parent-latched tree generation.
  Bufferless commits after a blocked dmabuf root now merge into the ordered
  waiting surface tree instead of canceling the inherited acquire-backed
  attachment. This addresses the Kitty resize/input freeze where later
  damage/geometry/frame-callback commits could make the original ready wakeup
  stale before the new buffer became current. Real-driver explicit-sync subtree
  waits, deeply nested libdecor trees, and the legacy/Atomic KMS Kitty matrix
  still require live TTY validation on supported DRM hardware;
- `clipboard_source_disconnect_clears_focused_target_selection` is a known
  baseline failure: the focused target does not receive `selection(None)` when
  the source client disconnects. TASK 05.2 reproduces this unchanged on its
  starting commit `4791f55`; TASK 05.3 also reproduces it on starting commit
  `d67fc35`. Neither task alters clipboard behavior;
- nested output size and refresh are configurable through
  `start-oblivion-one --width W --height H --refresh R -- app`, but the selected
  nested refresh is a target advertised to clients and used for scheduling.
  Actual physical presentation can still be capped by the host compositor,
  host monitor refresh, or application rendering cadence;
- output revoke/suspend/resume is runtime-owned and deterministically tested;
  physical VT/libseat recovery still needs validation across target drivers.

Expected direction:

Validate native EGL/GLES and the implemented revoke/resume recovery over
GBM/KMS on real TTY sessions, then wire direct scanout before treating SDDM
performance as comparable to KWin, Hyprland, or other mature compositors.

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
