# XWM Pre-Map Buffer Readiness Design

## Problem

Steam CEF can commit and associate an Xwayland surface before its X11
`MapRequest`. The XWM records that retained buffer as ready, but
`X11WindowRegistry::mark_map_requested` unconditionally clears
`buffer_ready`. Because the association is already complete, no later
association event restores the flag. The X11 window becomes viewable but
never emits `WindowReady`, so it is absent from `_NET_CLIENT_LIST`.

## Design

Preserve the existing `buffer_ready` value when processing a map request.
This is safe for an initial map because the readiness belongs to the same
live association and surface content.

Real `UnmapNotify` remains the boundary for remapping: `mark_unmapped`
already clears `buffer_ready`, the snapshot, and mapping state. Therefore a
withdrawn window still requires a fresh buffer before a later remap can
become ready.

No association, compositor, timeout, or command interfaces change.

## Alternatives

Re-reading the XWM surface-ready cache during each `MapRequest` would also
restore the flag, but it would couple registry lifecycle transitions to XWM
association storage. Synthesizing readiness after the map command would make
event ownership ambiguous and risk duplicate readiness.

## Verification

Add a registry regression for the Steam ordering: observe, associate, mark
buffer ready, request map, complete properties, command map, confirm
`MapNotify`, and require one ready snapshot. Keep the existing
`managed_unmap_requires_a_fresh_buffer_before_remap` regression unchanged to
prove that withdrawal still clears readiness.

Run the focused red-green test, all XWM tests, the native Xwayland reactor
test, formatting, compilation, Clippy, the complete test suite, source layout
validation, diff validation, and a release build. After restart, verify the
exact Steam main XID is viewable, appears in `_NET_CLIENT_LIST`, and has a
matching `xwayland_window_admitted` diagnostic.
