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

`bin/start-oblivion-one-tty` is the TTY-oriented entry point. It forces
`OBLIVION_ONE_OUTPUT=native`, `OBLIVION_ONE_PROFILE=release`, and the
`oblivion-one-tty` socket. It also defaults `OBLIVION_ONE_MODE` to
`1920x1080@165`, then delegates to `bin/start-oblivion-one`. Use it from a text
TTY such as `Ctrl+Alt+F3`; it still refuses native scanout from an existing
graphical session unless `OBLIVION_ONE_NATIVE_SCANOUT=1` is set.

The Rust CLI then binds the Oblivion-owned Wayland socket and enters
`src/native_output.rs`.

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
when libseat/libinput/GBM are installed, because `src/native_output.rs` now
uses a shared libseat session for input and DRM device ownership and attempts a
GBM/KMS pageflip scanout path, but the GBM path still fills buffers through the
CPU scene renderer instead of rendering with EGL/GLES directly into GBM.

## Native Backend Today

The native backend currently:

- discovers `/dev/dri/card*` and connected KMS connectors;
- selects KMS modes through `OBLIVION_ONE_MODE`: `auto`/`highres` prefer the
  largest resolution and then highest refresh, `highrr` prefers highest refresh
  and then largest resolution, `preferred` keeps the kernel's first mode, and
  `WIDTHxHEIGHT@HZ` selects an exact resolution with nearest refresh fallback;
- opens DRM through the shared libseat session when available, with a direct DRM
  fallback kept for development sessions;
- prefers a GBM/KMS pageflip scanout path with writable XRGB8888 scanout
  buffers, falling back to a simple KMS dumb framebuffer if GBM scanout is not
  available;
- attempts a GBM-backed DRM hardware cursor by default
  (`OBLIVION_ONE_CURSOR=auto`) and omits cursor pixels from full frame
  repaints while that path is active; set `OBLIVION_ONE_CURSOR=software` to
  force the older software cursor path for comparison;
- renders the compositor scene through the CPU scene path into the active
  scanout buffer;
- prefers `libseat + libinput udev` for keyboard, pointer motion, buttons, and
  scroll;
- suspends/resumes libinput when the libseat input owner receives
  disable/enable events;
- keeps direct libinput and the older `/dev/input/event*` raw evdev reader as
  fallback/diagnostic paths;
- forwards the normalized keyboard and pointer events into `OwnCompositorServer`;
- coalesces consecutive pointer motion events before applying input effects, so
  high-rate mice do less duplicate compositor work per native loop tick;
- uses a small activity-based wakeup policy and derives the active-frame wakeup
  interval from the selected KMS mode refresh rate instead of sleeping at a
  fixed 16 ms in all states;
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
  `write_bytes` records how much was submitted to the scanout backend. When
  native input or no-damage visible frame callbacks do not change local visuals,
  `perf native.frame_skip` records the skipped repaint batch. GBM pageflip
  pacing also logs `native.finish_frame reason=pageflip_complete` when DRM
  confirms that a queued pageflip completed, and `native.frame_skip
  reason=pageflip_pending` when the native loop intentionally holds new
  client dispatch/repaint work behind an outstanding pageflip.
- repairs the cached CPU scene from explicit same-layout surface damage instead
  of rebuilding every client surface on every small `wl_surface` commit. Bounds
  changes for the same surface, such as interactive move/resize, now repair the
  previous and current surface rectangles as partial scene damage. The
  dumb-framebuffer scanout copy is also damage-limited. The GBM scanout staging
  copy is damage-limited and falls back to one full-frame copy when overlapping
  damage rects would copy more than the output. Window movement and resize
  damage old and new surface bounds instead of the whole output.
  `gbm_bo_write` still submits the full staging buffer until native EGL/GLES
  rendering replaces this CPU write path.
- previews interactive resize with a compositor-owned target geometry while
  waiting for the client commit. Left/top edge resizes keep the opposite edge
  visually anchored. If the committed client buffer is smaller than the preview
  target, the CPU renderer does not upscale the stale buffer; it leaves the
  newly exposed region to the desktop background until a compatible commit
  arrives. Native `prepare_frame` flushes queued resize configure events before
  repaint/present, so browser clients can start producing the matching buffer a
  frame earlier. On the GBM/KMS path, buffer releases, frame callbacks, and
  presentation feedback now wait for DRM pageflip completion instead of being
  completed immediately after pageflip submission. The xdg configure/ACK path
  still decides the committed client size.
- keeps minimized toplevels hidden across later client commits. Hidden commits
  update the minimized surface snapshot so restore shows the latest buffer
  without letting active browsers redraw themselves back into the visible scene.

Keyboard clients receive an XKB keymap generated from
`OBLIVION_ONE_XKB_LAYOUT`, `OBLIVION_ONE_XKB_VARIANT`, and
`OBLIVION_ONE_XKB_OPTIONS`. The default is `br` with the `abnt2` variant.

The native emergency exit shortcut is `Alt+P`. `Ctrl+C` is forwarded to clients
again, so terminals and shells inside the session can use it normally.

## Production Gaps

This is not yet a Hyprland/KWin-class native compositor backend. The next
architecture milestones are:

- remove direct DRM/input fallbacks after the libseat path is stable under real
  SDDM/VT switching;
- replace CPU-filled GBM buffers with EGL/GLES rendering into GBM render
  targets;
- make the loop wake from DRM/libinput readiness instead of polling DRM and
  sleeping from the current refresh-derived timer approximation;
- parse DRM pageflip timestamps/sequences for precise presentation feedback;
- add VRR capability detection and a conservative `off/on/fullscreen` policy;
- centralize output suspend/resume and device revoke handling in the session
  abstraction;
- add a tight-damage software cursor fallback and retained output damage for
  resize, shell overlay, and GBM/EGL-native presentation.

Research notes for the current native push live in:

- `docs/research/kms-mode-refresh.md`
- `docs/research/gecko-resize-rendering.md`
- `docs/research/native-performance-mouse.md`
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
