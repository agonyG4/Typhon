# XWayland Retained Buffer Readiness Design

## Problem

Steam can commit a `wl_surface` buffer before Xwayland assigns the
`xwayland_surface_v1` role and serial. Typhon retains that unassigned buffer,
but the pre-role commit cannot emit an XWayland buffer-ready event. When the
serial is committed later without another buffer attachment, the association
completes while the XWM continues waiting for a buffer that already exists.
The X11 window is mapped and viewable in Xwayland but never reaches
`WindowReady`, `DesktopWindow` admission, `RenderableSurface`, or
`_NET_CLIENT_LIST` publication.

## Design

When an XWayland surface serial commits, Typhon will check whether that surface
already has retained current buffer content. If it does, Typhon will enqueue
the same generation-bound buffer-ready event used by normal XWayland buffer
commits. Buffer-ready event insertion will be idempotent for a generation and
surface so a serial and a new buffer in the same Wayland commit cannot create
duplicate notifications.

The XWM remains responsible for joining the Wayland serial to the X11 window
and for enforcing all other readiness gates. Association alone will not imply
buffer readiness, and no non-XWayland role behavior will change.

## Regression Coverage

Add a compositor protocol regression that performs the production ordering:

1. Create an unassigned `wl_surface`.
2. Attach and commit a real SHM buffer.
3. Assign `xwayland_surface_v1` and commit its serial without another buffer.
4. Assert that association and exactly one buffer-ready event are emitted.
5. Deliver `WindowReady` and assert the retained buffer becomes a
   `RenderableSurface`.

The test must fail before the fix because step 4 has no buffer-ready event.
After the focused red-green cycle, run the installed-Xwayland reactor test,
the repository validation suite, rebuild release, and repeat the native Steam
launch. Native success requires Steam's main XID in `_NET_CLIENT_LIST` and a
Typhon `xwayland_window_admitted` log entry.

## Scope

This change will not alter Steam configuration, relax XWM readiness, publish
unassigned surfaces, or address unrelated Steam logger, `SLSTEAM`, or
`CLOUDREDIRECT` messages.
