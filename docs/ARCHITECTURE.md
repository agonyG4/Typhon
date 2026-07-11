# Typhon Architecture

Typhon has one product runtime:

```text
main
  ↓
native bootstrap
  ↓
NativeRuntime
  ├─ libseat / seat session
  ├─ DRM / KMS
  ├─ EGL/GBM or native CPU fallback
  ├─ native input and cursor
  ├─ Wayland server and compositor state
  └─ session shell and client launches
```

The binary binds the owned Wayland server, then enters native bootstrap. The
bootstrap discovers the seat and DRM target, opens KMS, selects a connector and
mode, initializes scanout and rendering, opens input, and transfers ownership
to `NativeRuntime`. A failure is returned with the native phase that failed;
there is no alternate runtime to hide that failure.

## Source boundaries

- `src/compositor/` owns the Wayland server, protocol dispatch, compositor
  state, surface lifecycle, output description, and frame planning.
- `src/native/` owns reusable event-loop, DRM, KMS, explicit-sync, and
  scheduling primitives.
- `src/native_output/` owns the native runtime, input adapters, output target,
  cursor, damage, scanout, shell launch, presentation, recovery, and shutdown.
- `src/egl_renderer.rs` owns the shared native EGL/GLES scene renderer and
  dmabuf import helpers.
- `src/session/` describes native seat, input, output, and SDDM prerequisites.
- `src/core/geometry.rs` contains reusable geometry shared by the window
  manager and native-independent tests.

The source-layout check requires every retained Rust file to be connected to
the module tree and keeps production modules below the configured size limits.

## Native output choices

These are implementation choices inside the native product, not product
modes:

- atomic KMS or legacy KMS;
- native EGL/GBM, CPU GBM, or dumb framebuffer scanout;
- hardware or software cursor;
- accelerated or CPU-only application launch policy.

The server initially advertises the safe native protocol set. GPU buffer
protocols are enabled only after the active native scanout backend is known.

## Input and shell launch

Native input translates libinput or raw evdev events into compositor actions.
Astrea Shell shortcut events are dispatched to registered protocol owners first.
Only a zero-owner `spotlight_toggle` or `alt_tab_next` press may resolve an
optional external fallback. If that fallback cannot spawn, Typhon records and
logs `fallback_spawn_failed`, consumes the binding, and continues running.

Client launch policy removes host activation/display routes, sets Typhon's
Wayland socket, and keeps X11 disabled unless a Typhon-owned XWayland bridge is
explicitly implemented. This policy affects child environments only; it never
chooses the compositor runtime.

## Session boundary

TTY and SDDM are the supported launch environments. Running from a normal
terminal inside another graphical session is unsupported as a native session:
seat, DRM, KMS, renderer, or input acquisition may fail and the resulting
diagnostic identifies the native phase. Inherited graphical-session variables
do not trigger a host-backed compositor.
