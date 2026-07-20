# Typhon modern XWayland profile

Typhon's modern XWayland implementation is an opt-in, local-only profile. It
does not claim support for X11 applications until the managed profile is
running; the compositor remains usable when XWayland is absent, crashes, or is
rejected by the protocol checks.

## Modes

`TYPHON_XWAYLAND` accepts these values:

- `off` (the default): no display lease, no XWayland process, and Wayland-only
  application environments.
- `base`: allocate the authenticated display lease and arm the foundation
  service. This is for development and readiness diagnostics; it is never
  exported to normal application launches.
- `lazy`: enable the managed profile. The service starts on the first local
  display connection and exports `DISPLAY` and `XAUTHORITY` only after the
  private XWayland client, committed shell binding, and XWM startup have all
  completed for the same generation.
- `eager`: use the same managed startup path during bootstrap. This is useful
  for diagnostics and integration tests.

`TYPHON_XWAYLAND_BINARY` may point at a test or development XWayland binary;
the production default is `Xwayland` resolved through `PATH`.

## Security and readiness

Each enabled service owns one display number, its lock, both local Unix
listeners, and a private `0600` Xauthority file under
`$XDG_RUNTIME_DIR/typhon/xwayland/`. TCP listeners and inherited host
`DISPLAY`/`XAUTHORITY` values are not used. XWayland receives a private
`WAYLAND_SOCKET` and explicitly mapped listener, WM, and `displayfd`
descriptors.

A generation is ready only after `displayfd` reports the leased display and
the exact private Wayland client binds `xwayland-shell-v1`. Managed startup
then requires the pure-Rust XWM connection and Composite. XFixes, Shape,
RandR, and Sync are optional; their absence does not block `Running`, and
their adapter/model foundations are not advertised as end-to-end application
support. Old generations cannot satisfy a newer readiness barrier.

Surface association uses committed `WL_SURFACE_SERIAL` values only. There is
no `WL_SURFACE_ID` fallback. Clipboard, primary selection, and drag-and-drop
bridges are intentionally deferred.

## Application testing

The managed launch environment is narrow and opt-in. It removes stale host
X11 routing before applying the service-owned `DISPLAY`, `XAUTHORITY`, and
Wayland-first toolkit settings. To test a toolkit's X11 path, use its own
documented override in the diagnostic launch environment, for example
`GDK_BACKEND=x11`, `QT_QPA_PLATFORM=xcb`, or `SDL_VIDEODRIVER=x11`.

The current implementation is a modern profile: legacy rootful/TCP clients,
clipboard and primary selection, Xdnd, XEmbed, mixed-DPI scaling, and
application-specific compatibility workarounds are not supported.

## Diagnostics

The XWayland metrics track state transitions, generations, lazy triggers,
startup duration, readiness failures, crashes/backoff, stale reactor events,
unauthorized shell binds, association commits/removals, XWM event budget
exhaustion, resize synchronization, and cleanup. Authentication cookies and
raw inherited descriptor contents are never logged.
