# Native Session Architecture

This document explains the current native/SDDM path for agents working on
Oblivion One.

## Current Flow

`bin/install-start-oblivion-one --sddm-session` writes an
`oblivion-one.desktop` Wayland session entry. The entry runs
`bin/start-oblivion-one` with `OBLIVION_ONE_PROFILE=release` and
`OBLIVION_ONE_OUTPUT=native`. The installed SDDM entry currently also sets
`OBLIVION_ONE_MODE=1920x1080@165` as the first high-refresh test target. Pass
`--perf-log` to the installer when a real SDDM login should also capture the
structured `perf ...` lines in the native session log.

`bin/start-oblivion-one` resolves its project root, chooses native output when
there is no host `WAYLAND_DISPLAY`, points the session at the release binary,
sets the desktop/session environment, publishes activation variables to
systemd user and D-Bus when those tools are available, and logs native-session
startup to `~/.local/state/oblivion-one/session.log`.

When a host display is present, the launcher chooses nested output and forwards
compositor sizing flags without shell-side parsing:

```sh
./bin/start-oblivion-one --width 1600 --height 900 --refresh 165 -- zen-browser
```

The nested flags configure the initial logical host-window size and requested
nested refresh. The compositor advertises that refresh to Wayland clients and
uses it for active wakeup timing, but real presentation remains paced by the
host compositor and the monitor. On Hyprland hosts, `hyprctl monitors` is the
quick check for the monitor refresh underneath the nested window.

`bin/start-oblivion-one-tty` is the TTY-oriented entry point. It forces
`OBLIVION_ONE_OUTPUT=native`, `OBLIVION_ONE_PROFILE=release`, and the
`oblivion-one-tty` socket. It also defaults `OBLIVION_ONE_MODE` to
`1920x1080@165`, then delegates to `bin/start-oblivion-one`. Use it from a text
TTY such as `Ctrl+Alt+F3`; it still refuses native scanout from an existing
graphical session unless `OBLIVION_ONE_NATIVE_SCANOUT=1` is set.

The Rust CLI then binds the Oblivion-owned Wayland socket and enters
`src/native_output.rs`. For native output the socket is bound with only the base
Wayland globals first. After the scanout backend is opened and any startup
fallback has settled, the compositor resolves the effective app GPU policy and
publishes `linux-dmabuf`, explicit-sync, and `wl_drm` only when the active
backend is `native-egl-gbm`.

## Native Session Diagnostics

`oblivion-one doctor` now probes the native session in three layers:

- session/runtime: `XDG_RUNTIME_DIR`, SDDM entry, and log path;
- input prerequisites: seat manager presence, `libseat`, `libinput`,
  `xkbcommon`, and whether the current experimental backend can read at least
  one `/dev/input/event*` device;
- output prerequisites: KMS card, render node, connected DRM output, GBM, and
  EGL.

The diagnosis separates host prerequisites from implementation readiness. A
machine can report `host prerequisites available; backend still experimental`
when libseat/libinput/GBM/EGL are installed, because real TTY/KMS validation is
still required. `src/native_output.rs` now uses a shared libseat session for
input and DRM device ownership and, in `auto`, attempts native EGL/GLES
composition into a GBM surface before falling back to the explicitly named
`gbm-cpu-write` pageflip path. Legacy names such as `gbm` and `gbm-egl` remain
aliases for that CPU-write fallback.

## Native Backend Today

The native backend currently:

- discovers `/dev/dri/card*` and connected KMS connectors;
- selects KMS modes through `OBLIVION_ONE_MODE`: `auto`/`highres` prefer the
  largest resolution and then highest refresh, `highrr` prefers highest refresh
  and then largest resolution, `preferred` keeps the kernel's first mode, and
  `WIDTHxHEIGHT@HZ` selects an exact resolution with nearest refresh fallback.
  Native mode selection is separate from nested `--width`, `--height`, and
  `--refresh` flags;
- opens DRM through the shared libseat session when available, with a direct DRM
  fallback kept for development sessions;
- selects KMS through `OBLIVION_ONE_KMS_MODE=auto|atomic|legacy`. `auto`
  enables the atomic client capability, discovers connector/CRTC/primary-plane
  properties, validates the complete initial state with a test-only commit, and
  uses atomic takeover when that succeeds. It falls back to legacy only for
  capability, discovery, or test-only failure before takeover. `atomic`
  requires this path; `legacy` skips it;
- prefers `native-egl-gbm` scanout in `auto`: a GBM surface with
  `SCANOUT|RENDERING`, an EGLDisplay created from the `gbm_device`, the shared
  GLES scene renderer, `eglSwapBuffers`, `gbm_surface_lock_front_buffer`, a DRM
  framebuffer cache keyed by GBM BO metadata, and KMS pageflip;
- keeps `gbm-cpu-write` as an explicit fallback/debug path with writable
  XRGB8888 scanout buffers, and falls back further to a simple KMS dumb
  framebuffer when GBM scanout is not available;
- attempts a GBM-backed DRM hardware cursor by default
  (`OBLIVION_ONE_CURSOR=auto`) and omits cursor pixels from full frame
  repaints while that path is active; set `OBLIVION_ONE_CURSOR=software` to
  force the older software cursor path for comparison;
- renders normal native GPU frames through GLES. The CPU scene renderer is still
  used by `gbm-cpu-write` and dumb-framebuffer fallback modes;
- keeps compositor-owned window shadows disabled. Native damage and scene bounds
  include client content plus temporary resize preview backdrop/outline only;
  shadow extents are not generated or tracked in the active paths;
- prefers `libseat + libinput udev` for keyboard, pointer motion, buttons, and
  scroll;
- suspends/resumes libinput when the libseat input owner receives
  disable/enable events;
- keeps direct libinput and the older `/dev/input/event*` raw evdev reader as
  fallback/diagnostic paths;
- forwards the normalized keyboard and pointer events into `OwnCompositorServer`;
- coalesces consecutive pointer motion events before applying input effects, so
  high-rate mice do less duplicate compositor work per native loop tick;
- registers DRM, Wayland listener/client, libinput or raw evdev, and monotonic
  timer fds in a level-triggered `epoll` reactor. With no work or deadline the
  native thread blocks indefinitely in `epoll_wait`;
- registers pending explicit-sync acquire points with
  `DRM_IOCTL_SYNCOBJ_EVENTFD` on the active DRM file and adds each nonblocking,
  close-on-exec eventfd to epoll. Supported watches do not arm a refresh polling
  deadline. Unsupported implementations use one absolute refresh-derived retry
  deadline only while fallback points exist. Visual work renders immediately
  when no pageflip is pending, while queued work waits for the current pageflip;
- advertises the selected KMS refresh rate through `wl_output.mode` and
  presentation feedback instead of hardcoding 60 Hz for native clients.
- can emit structured native performance logs when `OBLIVION_ONE_PERF_LOG=1`
  is set. Events are prefixed with `perf` and include startup backend choices,
  KMS mode, frame paint/copy/write/present timings, input event counts,
  process CPU deltas, native app spawn time, first-toplevel latency, and resize
  begin/update/end timings. Frame logs include `cursor=hardware` or
  `cursor=software`, and `perf native.cursor` records the selected cursor
  backend/fallback. Native frame logs also include `render_changed`,
  `render_cause`, `scene_rebuild`, `frame_copy`, `damage_kind`,
  `damage_rects`, and `damaged_pixels` so repaint analysis can separate surface
  commits from window movement, pending frame work, accepted clients, partial
  scene repair, partial frame copies, and full-output fallbacks. `copy_bytes`
  records how much of the ARGB frame was converted into scanout memory, while
  `write_bytes` records how much was submitted to the scanout backend. The
  `backend` field distinguishes `native-egl-gbm`,
  `gbm-cpu-write-pageflip`, and `dumb-framebuffer`. GPU frames report
  `write_bytes=0`, `full_frame_cpu_copy_bytes=0`, `gpu_draw_us`,
  `egl_swap_us`, `shm_upload_bytes`, and dmabuf import/reuse/failure counts.
  When native input or
  no-damage visible frame callbacks do not change local visuals,
  `perf native.frame_skip` records the skipped repaint batch. GBM pageflip
  pacing also logs `native.finish_frame reason=pageflip_complete` when DRM
  confirms that a queued pageflip completed, and `native.frame_skip
  reason=pageflip_pending` when repaint is held behind an outstanding
  pageflip. Wayland client dispatch is still serviced while a pageflip is
  pending, and native frame logs include `pageflip_pending_at_tick` for that
  state. `perf native.wakeup`, `perf native.scheduler`, `perf native.deadline`,
  `perf native.pageflip_watchdog`, and `perf native.explicit_sync` report
  readiness masks, kernel wait time, deadlines, scheduling, watch counts,
  registrations, wakeups, cancellations, fallback activation, stale/duplicate
  tokens, and acquire latency. Reliable libinput motion timestamps also produce
  `perf native.input_dispatch` latency fields.
- reports logical and repair damage, EGL buffer age, retained history depth,
  scissor passes, command replays, avoided pixels, swap-with-damage use, and
  conservative full-repaint reasons for native EGL frames;
- supports `OBLIVION_ONE_FORCE_FULL_REPAINT=1` to force a full GLES clear and
  replay for A/B validation. Partial repaint is disabled by default and is
  enabled only with `OBLIVION_ONE_ENABLE_PARTIAL_REPAINT=1`.
  `OBLIVION_ONE_DISABLE_BUFFER_AGE=1` leaves damage calculation active but
  disables buffer-age repair, conservatively falling back to full repaint.
  Precedence is force-full, then missing partial opt-in, then disabled/invalid
  buffer age, then the partial planner;
- distinguishes protocol/event-loop progress, a rendered frame, and a
  KMS-presented frame. Empty logical damage performs no GL execution, EGL swap,
  GBM front-buffer lock, ready-frame transition, or legacy/Atomic KMS submit.
  Native diagnostics include `frame_decision`, logical/repair rectangles,
  partial enablement, contradiction fallback, scene-snapshot commit, EGL swap,
  GBM lock, and ready-frame fields when performance logging is enabled;
- parses complete legacy or atomic DRM pageflip events and uses their kernel timestamp,
  sequence, CRTC ID, and unique submission token as native presentation
  metadata. `wp_presentation.clock_id` follows
  `DRM_CAP_TIMESTAMP_MONOTONIC`; feedback carries only the conservative
  `VSYNC` flag for synchronized legacy flips. Compositor receive time and
  submission duration are logged separately and never replace kernel metadata;
- repairs the cached CPU scene from explicit same-layout surface damage instead
  of rebuilding every client surface on every small `wl_surface` commit. Bounds
  changes for the same surface, such as interactive move/resize, now repair the
  previous and current surface rectangles as partial scene damage. The
  dumb-framebuffer scanout copy is also damage-limited. The GBM scanout staging
  copy is damage-limited and falls back to one full-frame copy when overlapping
  damage rects would copy more than the output. Window movement and resize
  damage old and new surface bounds instead of the whole output. Client commits
  that change logical bounds also combine commit damage with old/new bounds so a
  stale previous rectangle is repainted. `gbm_bo_write` is now limited to the
  explicit CPU-write fallback and is not used by `native-egl-gbm` frames.
- stores `wl_surface.damage` and `damage_buffer` separately until commit.
  Surface-coordinate rectangles map through the captured integer buffer scale
  or supported viewport destination; buffer-coordinate rectangles are already
  in attached-buffer space. Checked conversion and clipping union both spaces,
  and unsupported or ambiguous mapping becomes full surface damage. Applied
  commits remain journaled/accumulated until a real rendered frame succeeds;
- previews interactive resize with compositor-owned target geometry while
  waiting for client commits. Left/top edge resizes keep the opposite edge
  visually anchored. The shared CPU/GLES render plan draws stale client content
  at committed size without upscaling; when the visual target is smaller than
  committed content, it crops the sampled source UVs instead of squeezing the
  buffer. Temporary preview backdrop and outline layers show the pointer-
  following target until a compatible commit arrives. Native `prepare_frame`
  flushes the latest queued resize configure without waiting for an older
  resize commit, so slow clients reduce content freshness rather than pointer
  responsiveness. On the GBM/KMS path, buffer releases, frame callbacks, and
  presentation feedback now wait for DRM pageflip completion instead of being
  completed immediately after pageflip submission. The xdg configure/ACK path
  still decides the committed client size; ACK alone does not replace content.
- keeps minimized toplevels hidden across later client commits. Hidden commits
  update the minimized surface snapshot so restore shows the latest buffer
  without letting active browsers redraw themselves back into the visible scene.

Keyboard clients receive an XKB keymap generated from
`OBLIVION_ONE_XKB_LAYOUT`, `OBLIVION_ONE_XKB_VARIANT`, and
`OBLIVION_ONE_XKB_OPTIONS`. The default is `br` with the `abnt2` variant.

The native emergency exit shortcut is `Alt+P`. `Ctrl+C` is forwarded to clients
again, so terminals and shells inside the session can use it normally.

## Backend Selection

- `OBLIVION_ONE_KMS_MODE=auto` (default): attempt atomic discovery and initial
  `TEST_ONLY | ALLOW_MODESET`; use legacy only if capability, discovery, or
  test-only validation fails before takeover.
- `OBLIVION_ONE_KMS_MODE=atomic`: require atomic capability, required
  properties, compatible primary plane, successful test-only validation, and a
  successful real initial commit.
- `OBLIVION_ONE_KMS_MODE=legacy`: retain legacy `set_crtc` and `page_flip` for
  recovery and regression comparison.

For direct comparison:

```sh
OBLIVION_ONE_KMS_MODE=atomic cargo run --release
OBLIVION_ONE_KMS_MODE=legacy cargo run --release
```

- `OBLIVION_ONE_SCANOUT_BACKEND=auto` (default): try `native-egl-gbm`, then
  `gbm-cpu-write`, then `dumb`.
- `OBLIVION_ONE_SCANOUT_BACKEND=gpu` or `native-egl-gbm`: require native
  EGL/GLES over GBM/KMS and fail if it cannot be created.
- `OBLIVION_ONE_SCANOUT_BACKEND=cpu` or `gbm-cpu-write`: force the old GBM
  CPU-write pageflip path. Legacy `gbm`, `egl`, `pageflip`, `gbm-egl`, and
  `gbm-egl-pageflip` values remain aliases for this fallback.
- `OBLIVION_ONE_SCANOUT_BACKEND=dumb`: force the KMS dumb framebuffer fallback.
- `OBLIVION_ONE_NATIVE_APP_GPU=auto` or unset: derive the app profile from the
  active backend. `native-egl-gbm` launches accelerated apps; CPU-write and dumb
  launch apps with the software recovery profile.
- `OBLIVION_ONE_NATIVE_APP_GPU=gpu`: require `native-egl-gbm`. If startup
  fallback lands on CPU/dumb, session startup fails with a clear error instead
  of silently converting the override to CPU.
- `OBLIVION_ONE_NATIVE_APP_GPU=cpu`: force compositor-launched apps into the
  software recovery profile even when `native-egl-gbm` is active.

The initial app command is launched exactly once after the Wayland socket,
scanout backend, effective feedback/protocols, initial modeset, and input
backend are ready:

```sh
./bin/start-oblivion-one-tty -- kitty
```

Spotlight launches and the initial command use the same spawn path and the same
effective app GPU policy. Their perf events carry `source=startup` or
`source=spotlight`.

Startup fallback and runtime recovery are different. `auto` can fall back from
`native-egl-gbm` to CPU/dumb while opening the backend or painting the first
frame. After the session is presenting frames, EGL swap, GBM lock, framebuffer,
pageflip, and DRM event failures are treated as fatal runtime errors with
structured diagnostics and an explicit CPU restart recommendation. The
compositor does not remove already-published Wayland globals or hot-swap to a
CPU backend mid-pageflip.

Atomic KMS follows the same rule. `auto` may choose legacy only before a real
atomic takeover. A failed real initial atomic commit is rolled back and fails
startup; a runtime atomic flip error is fatal and never silently changes the
owned device to legacy. Normal atomic flips submit only primary-plane `FB_ID`
with `NONBLOCK | PAGE_FLIP_EVENT`; the existing token-matched DRM completion,
watchdog, protocol callback, buffer release, and presentation metadata path is
shared with legacy flips.

On orderly shutdown, atomic mode first test-validates and commits the captured
connector/CRTC/primary-plane state. If an external saved framebuffer or mode
blob can no longer be restored, it commits an atomic safe-disable state. The
hardware cursor remains on the legacy cursor IOCTL path; atomic primary-plane
requests do not touch cursor-plane properties.

## Production Gaps

This is not yet a Hyprland/KWin-class native compositor backend. The next
architecture milestones are:

- remove direct DRM/input fallbacks after the libseat path is stable under real
  SDDM/VT switching;
- harden EGL/GBM rendering under real SDDM/TTY hardware runs across drivers;
- complete protocol-owned pointer constraints and keyboard-shortcuts-inhibit
  activation before advertising them in normal sessions;
- add VRR capability detection and a conservative `off/on/fullscreen` policy;
- centralize output suspend/resume and device revoke handling in the session
  abstraction;
- add a tight-damage software cursor fallback, suspend recovery, direct scanout,
  and driver-specific validation for GBM/EGL-native presentation.

Atomic KMS is only a foundation here. VRR policy, direct scanout, atomic cursor
planes, overlay promotion, KMS in/out fences, framebuffer damage clips,
hotplug, and multi-output commits remain unimplemented.

The current `libseat` crate API does not expose the seat event fd, so libseat
lifecycle dispatch runs before other work whenever another registered source
wakes the reactor. No periodic idle timer is used as a workaround. Explicit
sync acquire points are independent epoll sources where syncobj eventfd is
supported; the bounded retry timer is armed only for blocked points on kernels
or drivers that reject that ioctl.

Research notes for the current native push live in:

- `docs/research/kms-mode-refresh.md`
- `docs/research/gecko-resize-rendering.md`
- `docs/research/native-performance-mouse.md`
- `docs/research/native-explicit-sync-eventfd.md`
- `docs/research/hyprland-resize-refresh-mouse.md`
- `docs/research/agent-raman-hyprland-resize-scale-followup.md`
- `docs/research/agent-gauge-resize-followup-perf.md`
- `docs/research/agent-pulse-resize-cpu-gpu-followup.md`
- `docs/research/agent-kernel-resize-state-followup.md`
- `docs/research/agent-keystone-resize-scale-architecture.md`

## Validation

Use these checks before real SDDM/VT testing:

```sh
bash -n bin/start-oblivion-one
bash -n bin/start-oblivion-one-tty
bash -n bin/install-start-oblivion-one
OBLIVION_ONE_DRY_RUN=1 ./bin/start-oblivion-one
env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET -u DISPLAY OBLIVION_ONE_DRY_RUN=1 ./bin/start-oblivion-one-tty
OBLIVION_ONE_PERF_LOG=1 start-oblivion-one-tty
OBLIVION_ONE_CURSOR=software OBLIVION_ONE_PERF_LOG=1 start-oblivion-one-tty
OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM=1 OBLIVION_ONE_SCANOUT_BACKEND=auto ./bin/start-oblivion-one-tty -- kitty
grep '^perf ' ~/.local/state/oblivion-one/session.log
./bin/install-start-oblivion-one --sddm-session --target-dir target/sddm-smoke --perf-log
cargo run -- doctor
cargo test --test start_launcher
cargo test session --lib
cargo test native_input_backend_plan --bin oblivion-one
cargo test keyboard_layout
cargo test native_wakeup_uses
```

For real SDDM testing, build release first:

```sh
cargo build --release
./bin/install-start-oblivion-one --sddm-session
./bin/install-start-oblivion-one --sddm-session --perf-log # verbose performance run
```

Manual TTY/KMS validation still needs to cover:

- modes: 1920x1080@60, 1920x1080 high refresh, 2560x1440 where available, and
  4K where available;
- drivers: AMD Mesa, Intel Mesa, and the local NVIDIA stack when present;
- clients: a simple SHM client, an EGL Wayland client, GTK, Qt/Qt Quick,
  Firefox/Zen, Chromium/Electron, and video playback;
- interactions: move, resize from each edge, maximize/restore, fullscreen,
  minimize/restore, popups, rapid commits, hardware/software cursor modes,
  VT/session switch, and shutdown during pageflip activity;
- metrics: `backend=native-egl-gbm`, `write_bytes=0`,
  `full_frame_cpu_copy_bytes=0`, non-llvmpipe GL renderer unless intentionally
  testing software EGL, pageflip completion logs, bounded framebuffer cache
  growth, and stable memory use.
