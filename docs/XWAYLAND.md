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
  XFixes, Shape, RandR, and Sync are optional and version-gated. Their absence
  does not prevent XWM from reaching `Running`;
- one global X11 DPI policy is used. Mixed per-monitor DPI is not advertised.

The current selection bridge implements both CLIPBOARD/PRIMARY directions:

- X11 external CLIPBOARD → Wayland selection/data-control offers and payloads;
- X11 external PRIMARY → Wayland primary-selection/data-control offers and
  payloads.
- Wayland CLIPBOARD/PRIMARY → X11 selection ownership and requests.

Both channels use generation-bound XFixes ownership, bounded TARGETS/MIME
catalogs, and direct-property or incoming INCR payload reads through bounded
nonblocking sinks. Wayland → X11 ownership is claimed with a server timestamp
and confirmed through GetSelectionOwner before SelectionRequest serving begins.
TARGETS, TIMESTAMP, MULTIPLE, direct properties, and outgoing INCR are active.
External X11 takeover revokes proxy serving while preserving inbound discovery.
The canonical compositor DND state now has the F11-C1 authority and semantic
contract for generation-qualified XWayland origins and X11 targets. The F11-C1
canonical cross-layer DND authority is complete.

F11-C2-A implements a bounded Wayland → X11 hover and data bridge: the XWM
discovers an XDND-aware target asynchronously, uses a per-drag internal source
proxy, translates Enter/Position/Status/Leave, and serves `XdndSelection`
TARGETS, TIMESTAMP, MULTIPLE, and offered MIME requests through direct or INCR
transfers. XDND action feedback is independent of MIME requests. This milestone
does not implement successful `XdndDrop`/`XdndFinished`; physical release over
an X11 target is rejected at the C2-A boundary. Move/DELETE behavior is not
complete: DELETE is unsupported and is not advertised. Native interoperability
qualification remains pending F11-D.

F11-C2-B, the Wayland → X11 Drop/Finished terminal bridge, is not implemented.
F11-C3, X11 → Wayland XDND, is not implemented.

The following remain adapter/model foundations rather than end-to-end support:

- runtime RandR publication: inactive foundation; no live output publication;
- X11 cursor ownership integration: inactive foundation.

The compositor's canonical Wayland selection state remains authoritative for
publication to Wayland clients. Bidirectional CLIPBOARD/PRIMARY transport is
implementation-active; native interoperability qualification remains pending
F11-D.

Diagnostics are available through `TYPHON_XWAYLAND_LOG=1` for forwarding the
bounded stderr ring and through `bin/check-xwayland-session` for a session
snapshot. `DISPLAY` and `XAUTHORITY` are private to the managed generation;
the filesystem socket is conventionally mode `0666` and MIT-MAGIC-COOKIE-1 is
the authorization boundary.

The native matrix remains environment-dependent. Hardware KMS ownership,
GTK/Qt/Steam/Proton availability, and a running X11 client suite must be
validated on the target session; ignored tests report skips rather than
claiming those external programs are installed.
