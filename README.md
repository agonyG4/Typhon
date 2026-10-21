# Typhon

Typhon (the `oblivion-one` binary) is a native TTY/SDDM Wayland compositor
written in Rust. It is the graphical foundation of AstreaOS. It owns the
Wayland server and starts through the native seat, DRM/KMS, renderer, input,
and shell runtime. It has no host-window execution path.

## Supported launch model

Use a TTY or the installed SDDM Wayland session:

```bash
./bin/start-oblivion-one-tty
./bin/install-start-oblivion-one --sddm-session
```

The general launcher is also native-only:

```bash
./bin/start-oblivion-one -- kitty --class TyphonTerminal
```

Launching from inside another graphical session does not select a different
runtime. Typhon either acquires the native seat and DRM output or exits with a
native startup diagnostic. `WAYLAND_DISPLAY`, `WAYLAND_SOCKET`, and `DISPLAY`
are not product-mode selectors. The launch scripts remove them before starting
Typhon; clients launched by Typhon receive Typhon's own Wayland socket.

Native startup requires a usable `XDG_RUNTIME_DIR`, a seat manager or direct
native permissions, a DRM device with a connected connector, and usable input
and rendering support. `oblivion-one doctor` reports the detected prerequisites.

## What runs by default

With no environment overrides, Typhon starts with these policies. Each `auto`
policy is bounded by discovered capabilities and exact validation, and falls
back to a safer path when a capability or check is missing.

| Area | Default | What it means |
| --- | --- | --- |
| KMS | `auto` | Atomic KMS first; Legacy only if Atomic is unavailable at startup. |
| Scanout backend | `auto` | Explicit Atomic EGL/GBM, then CPU GBM, then dumb framebuffer. |
| KMS commit worker | `auto` | Atomic commits are submitted from a dedicated worker thread (bounded queue of one job). Startup failure degrades to synchronous submission with a warning. |
| Render worker | `auto` | Rendering can run on its own worker for the Atomic EGL/GBM path. |
| Buffering | `auto` | Starts double-buffered. Moves to triple buffering only when the deadline predictor or a proven readiness miss asks for it and every capability check passes. |
| Direct scanout | `auto` | Eligible candidates are validated with `TEST_ONLY` on the primary plane. A rejected candidate returns to composition. Needs the Atomic EGL/GBM path with a healthy KMS worker. |
| VRR | `auto` | Requests Adaptive Sync only for eligible solitary-fullscreen candidates. |
| Tearing | `auto` | Async page-flip is requested only for eligible fullscreen clients that ask for tearing and clear every async blocker. `off` disables it. |
| Cursor | `auto` | Hardware cursor when it can be established safely, software otherwise. |
| XWayland | `lazy` | The managed XWM starts on demand. `off`, `base`, and `eager` are also accepted. |
| Scheduling QoS | `auto` | Tries `SCHED_RR` for the reactor, then high-priority FAIR, then normal scheduling. `off` makes no scheduler changes. |

The effects engine ships canonical blur semantics and terminal Kawase fusion
enabled. Checkpoint dependency pruning, precomputed window blur, and the
regional demand planner are newer and stay off by default.

## Native configuration

Current native controls are environment-based. The native session guide is the
source of truth for accepted values, defaults, maturity, and fallback behavior
for presentation and runtime policies: [Native session configuration](docs/NATIVE_SESSION.md#presentation-and-runtime-policy-defaults).

Common controls:

- `OBLIVION_ONE_MODE` selects the display mode (`auto`, `preferred`, `highres`, `highrr`, or `WIDTHxHEIGHT[@HZ]`).
- `OBLIVION_ONE_KMS_MODE` selects the KMS API (`auto`, `atomic`, `legacy`).
- `OBLIVION_ONE_NATIVE_APP_GPU` selects the GPU policy for launched clients.
- `OBLIVION_ONE_SHELL_COMMAND` starts the session shell.
- `OBLIVION_ONE_PERF_LOG=1` enables structured native performance logs.

Presentation and runtime policies (`off`, `auto`, `force` unless noted):

- `OBLIVION_ONE_KMS_COMMIT_WORKER`
- `OBLIVION_ONE_RENDER_WORKER`
- `OBLIVION_ONE_TRIPLE_BUFFERING`
- `OBLIVION_ONE_DIRECT_SCANOUT` (`auto` or `off`)
- `OBLIVION_ONE_VRR` (`off`, `auto`, `on`)
- `OBLIVION_ONE_TEARING` (`off` or `auto`)
- `OBLIVION_ONE_CURSOR` (`auto`, `hardware`, `software`)
- `OBLIVION_ONE_SCHEDULING_QOS` (`off`, `auto`, `require`)
- `TYPHON_XWAYLAND` (`off`, `base`, `lazy`, `eager`)

`force` never bypasses a capability blocker: if the requirement cannot be met,
Typhon stays on the safer path and `oblivion-one doctor` reports a warning.
Invalid values for the worker and buffering policies are configuration errors.

The compositor command accepts `--check`, `--socket`, and an application after
`--`. Former product-mode commands and backend flags are intentionally rejected.

## Architecture in short

- **Presentation pipeline.** A predictor estimates render and submit cost per
  refresh, the frame scheduler targets a presentation opportunity, and the KMS
  worker submits the atomic commit at the planned wake time. Primary,
  cursor-plane, and direct-scanout commits are modeled as separate service
  classes.
- **Buffering.** Triple buffering is a bounded credit on top of double
  buffering, granted and revoked from overlap and miss evidence. It never
  retargets a frame that is already armed.
- **Direct scanout.** Candidates are validated against scene, device, format and
  modifier, plane, generation, synchronization, cursor, and presentation state,
  and again by the kernel through `TEST_ONLY`.
- **Effects.** A render graph with damage-aware execution, blur fusion, and
  checkpoint caching. See [Effects](docs/EFFECTS.md).
- **Ownership contract.** Typhon owns compositor authority, protocol handling,
  scene and rendering, security, and KMS. The shell owns presentation and
  user-interface behavior. See [Architecture](docs/ARCHITECTURE.md).

## Current scope

Typhon is a single-output compositor today: there is no multi-output layout,
hotplug lifecycle, or multi-GPU path. Color management is advertised as sRGB
only, and HDR is not supported. Direct scanout covers the primary plane of the
selected output and does not use overlay planes. See
[Known issues](docs/KNOWN_ISSUES.md) for the full list of limits and the
qualification status of each feature.

## Development checks

```bash
cargo fmt --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test
./bin/check-source-layout
```

The native session and SDDM integration remain experimental until they have
been validated on the target hardware and drivers.
