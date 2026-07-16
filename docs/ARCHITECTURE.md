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

Atomic output ownership

On every effective Atomic KMS backend, the primary and cursor planes are one
output state. This includes the explicit EGL/GBM scanout, opaque EGL/GBM
compatibility, CPU GBM, and any asynchronous dumb-framebuffer pageflip path.
Discovery is part of the effective KMS startup plan: it enables universal
planes, selects a CRTC-compatible ARGB8888 cursor plane when available, records
its original property snapshot, and reads the cursor size capabilities with a
64×64 fallback. The cursor owner uses a pageflip-safe CPU/dumb ARGB buffer and
AddFB2; it never calls legacy cursor ioctls. NVIDIA therefore follows the same
visible hardware-cursor path when a linear cursor buffer is available, and
otherwise falls back visibly to a software cursor.

One output commit authority serializes composited-primary, direct-primary, and
cursor-only Atomic commits and owns the token, DRM generation, CRTC, and
watchdog for all three kinds. Compatibility scanouts use this same total
arbiter whenever their effective KMS backend is Atomic. Cursor-only completion
promotes cursor state only; it does not complete compositor frame batches,
presentation feedback, damage, or Direct Scanout transitions. Cursor movement
is coalesced behind the pending primary or cursor commit. Primary and cursor
state are validated together with `TEST_ONLY`, and framebuffer ownership ends
only at the matching pageflip (or through generation-aware recovery).

Direct Scanout remains available only to the explicit EGL/GBM scanout. Legacy
cursor ioctls are a Legacy-KMS-only implementation detail; compatibility
scanouts never combine an Atomic primary with a legacy cursor owner.

The effective KMS cursor state is centralized: software fallback, a latched
cursor-plane failure, and an unsupported client cursor keep the Atomic cursor
plane disabled until explicit recovery. Hidden logical cursor movement still
updates the next show position but is KMS-equivalent to the current hidden
state, so it does not create disabled-plane commits. Visible cursor planes
normalize alpha to the advertised maximum and use the discovered premultiplied
blend enum when present.

All compositor-owned pointer paths consume one immutable XCursor image loaded
at startup, including software composition, EGL fallback, Atomic cursor-plane
uploads, and Legacy cursor uploads.

While a valid Direct Scanout candidate remains active, predictive render-ahead
cannot select a composed primary path. Pointer movement selects a cursor-only
commit when the effective hardware cursor is usable, and an unchanged direct
scene is idle. A hidden pointer keeps Direct Scanout eligible even without a
hardware cursor. Popups, overlays, software cursors, unsupported client
cursors, and other genuine scene changes force one composited fallback; closing
them permits the next eligible direct re-entry.

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
