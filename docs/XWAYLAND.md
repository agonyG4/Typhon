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

F11-C2 implements the Wayland → X11 XDND hover, data, and terminal bridge. The
XWM discovers an XDND-aware target asynchronously, uses a per-drag internal
source proxy, translates Enter/Position/Status/Leave, synchronizes physical
Drop with the final Status, keeps `XdndSelection` available through
`XdndFinished`, and retires the exact source proxy after canonical completion.
TARGETS, TIMESTAMP, MULTIPLE, offered MIME requests, direct transfers, and INCR
transfers are supported. Copy terminal interoperability is implemented. Move
terminal interoperability is implemented when the target does not require the
unsupported DELETE conversion. XdndDrop/Finished terminal synchronization is
implemented. The XDND v5 Ask terminal-resolution state machine is implemented
and tested, but the current deterministic Wayland→XDND requested-action policy
does not actively select Ask. Active Ask selection remains unqualified until a
canonical user/modifier action policy exists. Synthetic Ask tests validate the
terminal state machine, not production reachability or end-to-end active Ask
interoperability. DELETE remains unsupported and is not advertised. Real-
application interoperability qualification remains pending F11-D.

F11-C3 implements the canonical X11 → Wayland XDND bridge. F11-C3-A covers
incoming Enter/Position/Leave metadata, Wayland offer delivery, XdndStatus
feedback, and direct/INCR XdndSelection reads into Wayland-provided
descriptors. F11-C3-B adds the production v5 Drop/Finished terminal bridge.
The root `XdndProxy` is published only when Typhon owns the root proxy slot; a
valid foreign proxy is preserved and blocks reverse XDND for that generation
while Wayland → X11 remains available. Root ownership loss disables reverse
discovery for that generation without an automatic reclaim attempt.

The production reverse terminal bridge accepts v5 Copy and Move, and Ask when
the Wayland destination resolves it to Copy or Move. Move performs the XDND
`DELETE` selection conversion before successful `XdndFinished`. F11-C4 reviewed
and intentionally retained v5 as the minimum version for productive terminal
interoperability. v2-v4 metadata and messages remain parseable for safe
inspection and rejection, but those sources cannot receive accepted terminal
Status or enter canonical productive Drop/Finished handling. Before v5,
`XdndFinished` cannot truthfully encode late failure and action semantics.
Real GTK/Qt/Electron/Wine/Proton and application qualification remains F11-D;
deterministic tests do not claim that qualification. The current
root-coordinate mapping is 1:1 pending a future output-layout mapping for
multi-output support.

After the canonical Drop acknowledgement starts `AwaitingWaylandFinish`, its
terminal progress lease is 60 seconds and renews only for concrete, validated
activity on an exact live transfer for that offer: successful post-Drop data
request admission, a valid non-None `SelectionNotify`, a validated direct or
INCR property reply (including each chunk, read continuation, and empty INCR
terminator), or a positive-byte write into the Wayland sink. Merely issuing
`ConvertSelection`/`GetProperty`, raw `PropertyNotify`, the INCR
`DeleteProperty` handshake, sink readiness, zero writes, `WouldBlock`,
`EINTR`, malformed or stale replies, and X-side draining after sink loss do not
renew it.

Renewal cannot move the lease past the immutable 10-minute absolute lifetime
cap, which begins at the successful canonical Drop acknowledgement. At either
deadline Typhon submits the existing canonical `CancelAfterDrop` request; the
one-second cancellation fallback remains available for a queued canonical
`SourceFinished` transition to win before failure `XdndFinished` is sent. The
independent per-transfer idle timeout remains 30 seconds and only retires that
data transaction. Root proxy authority becoming `Lost` rejects new reverse
XDND admission but does not abort or freeze already committed Drop, terminal,
or Move DELETE work. Move DELETE retains its separate bounded timeout.

F11-C4 is the progress-aware bounded terminal lifetime and reviewed v5
compatibility boundary. F11-D is real-application qualification.

INCR teardown distinguishes source/session cancellation from an idle transfer
deadline. Cancellation retires the exact transfer and may send the empty
terminal property marker. An idle deadline drops the exact transfer state
without a zero-length marker, so a partial payload is not reported as a
successful end of transfer.

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

The 2026-10-05 F11-D environment review found an active Hyprland session and
Hyprland-managed Xwayland, with no native Typhon XWM available. No real-client
case was qualified. F11-D remains partially qualified pending a managed native
Typhon session; the installed-client inventory and blocked matrix are recorded
in [XWAYLAND_INTEROP_QUALIFICATION.md](XWAYLAND_INTEROP_QUALIFICATION.md).
