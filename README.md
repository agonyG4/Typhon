# Typhon

Typhon (the `oblivion-one` binary) is a native TTY/SDDM Wayland compositor.
It owns the Wayland server and starts through the native seat, DRM/KMS,
renderer, input, and shell runtime. It has no host-window execution path.

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

## Native configuration

Current native controls are environment-based:

- `OBLIVION_ONE_MODE` selects the KMS mode.
- `OBLIVION_ONE_KMS_MODE` selects atomic or legacy KMS policy.
- `OBLIVION_ONE_SCANOUT_BACKEND=auto|gpu|native-egl-gbm` selects the explicit
  Atomic EGL/GBM path. The old opaque path is rollback-only under the exact
  value `native-egl-gbm-opaque`; CPU GBM and dumb remain separate fallbacks.
- `OBLIVION_ONE_CURSOR` selects hardware or software cursor policy.
- `OBLIVION_ONE_NATIVE_APP_GPU` selects the GPU policy for launched clients.
- `OBLIVION_ONE_SHELL_COMMAND` starts the session shell.
- `OBLIVION_ONE_PERF_LOG=1` enables structured native performance logs.

The compositor command accepts `--check`, `--socket`, and an application after
`--`. Former product-mode commands and backend flags are intentionally rejected.

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
