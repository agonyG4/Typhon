# Native TTY/SDDM Session

Typhon is supported as a native TTY/SDDM Wayland compositor. The supported
session entry points are:

```bash
./bin/start-oblivion-one-tty -- kitty
./bin/install-start-oblivion-one --sddm-session
```

The installer writes an SDDM Wayland-session entry that runs the release
binary and may enable `OBLIVION_ONE_PERF_LOG=1` for diagnostics. The TTY
launcher defaults to a release build, the `oblivion-one-tty` socket, and
`OBLIVION_ONE_MODE=1920x1080@165`.

## Startup requirements

Native bootstrap needs:

- a valid `XDG_RUNTIME_DIR`;
- a usable libseat/logind/seatd seat or direct native permissions;
- a readable DRM card with a connected connector and usable CRTC;
- native EGL/GBM or an available CPU framebuffer fallback;
- keyboard and pointer input through libinput or an allowed raw fallback.

`oblivion-one doctor` reports these prerequisites. Missing seat, DRM, KMS,
renderer, or input resources cause startup to fail with the failed native
phase and the underlying error.

`WAYLAND_DISPLAY`, `WAYLAND_SOCKET`, and `DISPLAY` are ignored for runtime
selection. The native launchers unset them before starting Typhon. Children
launched after the Wayland socket is created receive the Typhon socket through
the compositor launch environment.

## Native settings

- `OBLIVION_ONE_MODE=auto|preferred|highres|highrr|WIDTHxHEIGHT[@HZ]`
- `OBLIVION_ONE_KMS_MODE=auto|atomic|legacy`
- `OBLIVION_ONE_SCANOUT_BACKEND=auto|gpu|native-egl-gbm|native-egl-gbm-opaque|gbm-cpu-write|cpu|dumb`

`auto`, `gpu`, and `native-egl-gbm` select the explicit Atomic EGL/GBM path.
The exact `native-egl-gbm-opaque` value is the rollback-only opaque window
surface implementation and is never selected by `auto`.
- `OBLIVION_ONE_CURSOR=auto|hardware|software`
- `OBLIVION_ONE_NATIVE_APP_GPU=auto|gpu|cpu`
- `OBLIVION_ONE_SHELL_COMMAND='...'`
- `OBLIVION_ONE_PERF_LOG=1`

With Atomic KMS, `OBLIVION_ONE_CURSOR=auto` selects the discovered universal
cursor plane when its ARGB8888 storage can be allocated safely. This applies to
the explicit EGL/GBM, opaque EGL/GBM compatibility, CPU GBM, and asynchronous
dumb-framebuffer scanout paths. `hardware` fails startup if that plane or its
linear cursor buffer cannot be established; `auto` uses a visible software
cursor instead. Atomic cursor state participates in every primary `TEST_ONLY`
and commit, including compatibility scanouts, while cursor-only pageflips
preserve Direct Scanout and do not complete compositor frame batches. A
client-provided cursor that cannot be reproduced exactly in the Atomic cursor
buffer blocks Direct Scanout and is composed.

All nonblocking Atomic commits share one CRTC ownership slot and watchdog,
including Atomic commits submitted by compatibility scanouts. Cursor-only
timeouts follow the same final-drain and recovery path as primary timeouts. A
hidden pointer disables the cursor plane without blocking Direct Scanout; a
visible software or unsupported client cursor forces composition. Legacy cursor
ioctls are used only when the effective KMS backend is Legacy.

The compositor CLI retains only native configuration: `--check`, `--socket`,
and an optional application after `--`. Former host-window and demo commands
are invalid.

## Runtime sequence

```text
Wayland server bind
  → native bootstrap
  → seat/session acquisition
  → DRM device open
  → KMS target and mode selection
  → scanout/renderer initialization
  → input and shell startup
  → NativeRuntime event cycle
```

The event cycle owns seat lifecycle, DRM pageflips, Wayland dispatch, input,
frame scheduling, presentation feedback, child supervision, recovery, and
shutdown. Optional application utility failures are non-fatal; failures in
native bootstrap and required runtime components are returned.

## Validation

Run before a hardware session:

```bash
bash -n bin/start-oblivion-one bin/start-oblivion-one-tty
OBLIVION_ONE_DRY_RUN=1 ./bin/start-oblivion-one-tty
cargo test native_input_backend_plan --bin oblivion-one
cargo test native_wakeup_uses --bin oblivion-one
```

Real TTY/KMS validation is still required across the supported drivers. SDDM
integration is intentionally documented as experimental rather than complete.
