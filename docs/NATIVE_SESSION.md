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
- `OBLIVION_ONE_CURSOR_THEME=<theme>` and `OBLIVION_ONE_CURSOR_SIZE=<pixels>`
- `OBLIVION_ONE_NATIVE_APP_GPU=auto|gpu|cpu`
- `OBLIVION_ONE_SHELL_COMMAND='...'`
- `OBLIVION_ONE_PERF_LOG=1`

### Presentation and runtime policy defaults

These are the native parser values and runtime fallbacks. `auto` policies remain
bounded by discovered capabilities and transaction validation.

| Policy | Accepted values | Default | Effective fallback and status |
| --- | --- | --- | --- |
| `OBLIVION_ONE_KMS_MODE` | `auto`, `atomic`, `legacy` | `auto` | `auto` selects Atomic first and falls back to Legacy only for Atomic capability, discovery, or initial `TEST_ONLY` failures. Later Atomic failures remain fatal. Explicit `atomic` failures are fatal. |
| `OBLIVION_ONE_SCANOUT_BACKEND` | `auto`; Atomic aliases `gpu`, `native`, `native-gpu`, `native-egl-gbm`, `egl-gbm`, `gles-gbm`, `egl-gles-gbm`; rollback `native-egl-gbm-opaque`; CPU GBM aliases `gbm-cpu-write`, `gbm-cpu-write-pageflip`, `cpu-gbm-write`, `cpu-gbm-pageflip`, `cpu`, `cpu-gbm`, `gbm`, `egl`, `pageflip`, `gbm-egl`, `gbm-egl-pageflip`; dumb aliases `dumb`, `framebuffer`, `legacy` | `auto` | Auto chooses explicit Atomic EGL/GBM, then CPU GBM, then dumb when those capabilities are available. Explicit backend choices do not walk that fallback list. Unknown values warn and use `auto`. The opaque path is rollback-only and is never selected automatically. |
| `OBLIVION_ONE_KMS_COMMIT_WORKER` | `off`, `auto`, `force` | `auto` | Auto uses the worker on Atomic KMS; worker startup failure warns and continues synchronously. Legacy KMS remains synchronous. `force` fails on Legacy KMS or Atomic worker startup failure; `off` is intentionally synchronous. Invalid explicit values are configuration errors. |
| `OBLIVION_ONE_TRIPLE_BUFFERING` | `auto`, `off`, `force` | `auto` | Auto selects triple buffering only with proven capabilities and useful overlap; otherwise it stays double-buffered. `force` does not bypass capability blockers and falls back to double buffering with a doctor warning. Invalid values are configuration errors. |
| `OBLIVION_ONE_DIRECT_SCANOUT` | `auto`, `off`; deprecated alias `experimental-auto` | `auto` | Direct Scanout is project-qualified for production automatic use. Auto attempts eligible candidates opportunistically; exact scene, device, format/modifier, plane, generation, sync, cursor, presentation-state, and KMS `TEST_ONLY` proof still apply. A candidate that cannot be proven falls back to composition. Unknown values, including `force`, warn and resolve to `off`. Runtime candidate qualification starts unproven and is separate from project-level feature qualification. |
| `OBLIVION_ONE_VRR` | `off`, `auto`, `on`; aliases `0`/`false`/`no`/`disable`/`disabled` and `1`/`true`/`yes`/`enable`/`enabled` | `auto` | Phase 1 is implemented. Auto requests Adaptive Sync only for eligible solitary-fullscreen candidates; Atomic connector/CRTC capability and exact request validation still govern. Unknown values warn and resolve to `auto`. Further scheduler and final target-hardware qualification work is intentionally deferred; see [Presentation Modes v1](wayland/PRESENTATION_MODES_V1.md#phase-1-qualification-boundary). |
| `OBLIVION_ONE_TEARING` | `off`, `auto` | `off` | Auto may request async page-flip only when its surface hint, fullscreen, cursor/plane, sync, timing, format, and exact KMS checks allow it. Missing or unrecognized values resolve to `off`. |
| `OBLIVION_ONE_CURSOR` | `auto`, `hardware` (`hw`, `drm`), `software` (`sw`, `cpu`) | `auto` | Auto uses an available safe hardware cursor and falls back to software. Explicit `hardware` fails startup if its requested hardware path cannot be established. Unknown values warn and use `auto`. |
| `OBLIVION_ONE_CURSOR_SCHEDULING` | `auto`, `piggyback`, `software` | `auto` | Auto uses normal cursor/output arbitration; `piggyback` prioritizes primary work, and `software` selects software cursor scheduling. Unknown values warn and use `auto`. |
| `OBLIVION_ONE_DMABUF_KMS_PREFERRED` | `auto`, `off`, `force` | `auto` | Auto prefers exact same-FOURCC renderer/KMS modifier intersections only on NVIDIA EGL. Force removes incompatible advertised choices only when a safe replacement exists; it cannot create a missing intersection. Unknown values warn and use `off`. |

`experimental-auto` is accepted temporarily for existing launch scripts and emits
a deprecation warning; `auto` is the canonical Direct Scanout policy. The
runtime's `not_qualified` telemetry describes current candidate proof, not the
production maturity of the feature.

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

Typhon loads the compositor-owned `left_ptr` once at native startup. Theme and
size overrides use `OBLIVION_ONE_CURSOR_THEME`/`OBLIVION_ONE_CURSOR_SIZE`, then
`XCURSOR_THEME`/`XCURSOR_SIZE`, with the XCursor default theme and size 24 as
fallback. `XCURSOR_PATH` and standard XDG icon paths, including
`index.theme` inheritance, are resolved by the pinned pure-Rust `xcursor`
dependency. Its immutable ARGB8888 image and hotspot are shared by software
composition, EGL, Atomic, and Legacy cursor uploads without `libXcursor` FFI.

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
  → input, shell capability handoff, and shell startup
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
