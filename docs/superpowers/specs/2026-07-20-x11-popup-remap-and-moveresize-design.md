# X11 popup remap and client move/resize design

## Evidence

Steam reuses override-redirect popup XIDs while Xwayland creates a fresh
Wayland surface association for each mapping. Live logs show the first mapping
being admitted, followed by later mappings emitting `WindowReady` with the old
destroyed surface and failing admission with `SurfaceNotXwayland`.

Typhon also publishes `_NET_WM_MOVERESIZE` and `_NET_MOVERESIZE_WINDOW`, but its
client-message normalizer does not handle either atom. Steam client-side titlebar
drags therefore have no effect unless the compositor's Super binding is used.

## Popup lifecycle

On `UnmapNotify`, retire the X11 window's association from both the association
join and the window record. Clear buffer readiness and the published snapshot so
a remap must satisfy the new serial, association, buffer, map, and property gates.
Also reconcile a Wayland-side association removal into the owning window record
in case surface destruction arrives before the X11 unmap.

The XID and its reusable metadata record remain alive until `DestroyNotify`.

## Client move and resize

Decode `_NET_MOVERESIZE_WINDOW` as a one-shot configure request using EWMH mask
bits 8 through 11.

Decode `_NET_WM_MOVERESIZE` into a typed compositor event. Directions 0 through
7 map to the corresponding resize edges, direction 8 starts a move, and direction
11 cancels the matching interaction. Keyboard directions remain unsupported.

As KWin does, accept an interactive request only when its X11 button number maps
to a mouse button currently held on the requesting window. This prevents arbitrary
clients from initiating pointer-driven window operations.

## Verification

Regression tests cover popup XID remapping to a new surface, Wayland-first
dissociation, both EWMH decoders, held-button validation, direction mapping, and
cancel. Focused suites run before the complete repository gate and a native Steam
test.
