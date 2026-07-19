# Typhon XWayland compatibility

Typhon runs XWayland as a managed, generation-bound child. The native reactor
owns the displayfd, private Wayland socket, XWM socket, stderr ring, and all
retirement tokens. X11 setup and replies are progressed incrementally with
x11rb protocol types over a nonblocking stream.

The supported X11 window contract is:

- normal managed windows map after bounded property discovery and MapNotify;
- override-redirect windows are adopted without a normal WM map or focus
  decision;
- X11 mapping is independent of `wl_surface` association and first-buffer
  readiness;
- ICCCM input focus, `WM_TAKE_FOCUS`, configure masks, transient validation,
  stacking requests, `WM_STATE`, and the implemented EWMH root/client
  properties are generation-cleaned;
- Composite is required for the current rootless redirection architecture;
  XFixes, Shape, RandR, and Sync are optional and version-gated;
- one global X11 DPI policy is used. Mixed per-monitor DPI is not advertised.

CLIPBOARD, PRIMARY, and Xdnd are adapted to the existing Wayland data-device
authority. The adapter uses bounded MIME lists and nonblocking owned-fd
transfers; it does not maintain a second selection owner or buffer arbitrary
contents in compositor memory.

Diagnostics are available through `TYPHON_XWAYLAND_LOG=1` for forwarding the
bounded stderr ring and through `bin/check-xwayland-session` for a session
snapshot. `DISPLAY` and `XAUTHORITY` are private to the managed generation;
the filesystem socket is conventionally mode `0666` and MIT-MAGIC-COOKIE-1 is
the authorization boundary.

The native matrix remains environment-dependent. Hardware KMS ownership,
GTK/Qt/Steam/Proton availability, and a running X11 client suite must be
validated on the target session; ignored tests report skips rather than
claiming those external programs are installed.
