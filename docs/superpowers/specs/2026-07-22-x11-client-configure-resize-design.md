# X11 Client Configure Resize Design

## Problem

Floating managed X11 windows such as Steam can implement client-side resize by issuing ordinary `ConfigureRequest` events rather than `_NET_WM_MOVERESIZE`. A recent placement-stability change accepts the requested width and height but replaces requested x and y with Typhon's current authoritative position.

That breaks the geometry transaction for left and top edges. For example, a window at `x=100`, width `640` has right edge `740`. A client request for `x=120`, width `620` must be applied as one box so the right edge remains `740`. Applying `x=100`, width `620` moves the right edge to `720`; the client then compensates and creates a configure feedback loop. Right-edge resize is less affected because its x coordinate is unchanged.

The session log corroborates this path: the bad operation produced no compositor interactive-resize updates and no X11 resize-sync transactions. KWin explicitly permits client-driven interactive move/resize through `ConfigureRequest`, and Hyprland applies the full requested box for floating X11 windows.

## Design

For an X11 `ConfigureRequest` received outside an active compositor-driven resize, preserve every requested geometry field after applying existing ICCCM size constraints. Continue to preserve unrequested fields from authoritative geometry. Continue rejecting configure geometry during an active compositor-driven resize, and retain all existing stacking behavior.

The change restores Typhon's behavior before the uncommitted x/y override and matches KWin and Hyprland. It does not add edge inference, a second resize state machine, or changes to synchronized compositor-driven resize.

## Regression coverage

Add a test representing a client-side left-edge resize. Starting with `(x=100, width=640)`, request `(x=120, width=620)` with x and width fields set. Assert that the emitted X11 configure keeps `x + width == 740`, and that Typhon's authoritative geometry is updated to the same complete box.

Retain the partial-field test to prove that unrequested y, width, and height remain authoritative when only x is requested.
