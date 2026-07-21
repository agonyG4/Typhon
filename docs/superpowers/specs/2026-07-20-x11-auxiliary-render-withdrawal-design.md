# X11 Auxiliary Render Withdrawal Design

## Problem

Steam maps small X11 support windows and assigns their identifying properties
after Typhon has admitted and published their associated XWayland surface.
Typhon's late auxiliary reconciliation now removes those windows from the
desktop registry and EWMH client lists, but `WindowWithdrawn` leaves the
already-adopted `RenderableSurface` in the scene. The result is a black 10x10
surface at the helper's X11 geometry that can receive pointer input but cannot
participate in normal move or resize behavior.

The live failure confirms this split state: Steam helper `0xc00001` is mapped at
`10,10` and absent from `_NET_CLIENT_LIST`, while its associated scene content
remains visible. This is not a popup-buffer failure; Steam popup-menu windows
remain override-redirect windows with their own map lifecycle.

## Reference Model

KWin keeps managed clients and unmanaged override-redirect windows in distinct
collections while still tracking their X11 map lifecycle. Hyprland similarly
keeps `m_isMapped`/visibility separate from whether a window participates in
normal layout and focus policy. Both models avoid treating desktop membership
as the only authority for whether previously published pixels remain visible.

Typhon needs the same separation at its existing boundary: XWM lifecycle decides
desktop admission, while compositor state owns publication of retained Wayland
buffers into the render scene.

## Design

Add a compositor-state operation that withdraws the published renderable tree
for an XWayland root surface without destroying protocol state. It will:

- remove renderable surfaces rooted at the withdrawn XWayland surface;
- retain `current_surface_buffers`, XWayland association state, placements, and
  surface roles;
- invalidate cached surface origins and output membership;
- advance render generation so native damage covers the removed bounds;
- return whether visible scene content changed.

`OwnCompositorServer` will resolve the X11 handle's root surface before removing
the `DesktopWindow`, invoke render withdrawal, then perform the existing desktop
removal and EWMH client-list synchronization.

The existing `WindowReady` admission path remains the inverse transition. It
inserts a new desktop identity and calls
`adopt_current_xwayland_surface_content`, which republishes the retained buffer.
No property-settling timer or Steam-specific class rule is introduced.

## Invariants

- Auxiliary helpers are neither desktop clients nor render/input targets.
- Override-redirect popup menus remain renderable while mapped.
- Withdrawal does not release or forget the latest committed buffer.
- A later typed/input-capable identity can be admitted and rendered again.
- Destroy and disconnect paths remain the owners of final protocol and buffer
  teardown.

## Verification

An automated state-level regression will publish an XWayland root, withdraw its
renderable content, and prove that the current buffer and association survive.
It will then reinsert the desktop identity and prove that existing adoption
restores the renderable surface. Existing popup, XWM lifecycle, renderer, and
native damage tests must remain green.

Live verification after restart will confirm that mapped 10x10 Steam helpers
remain absent from `_NET_CLIENT_LIST` and do not appear visually, while opening
a Steam popup maps and renders only the popup window.
