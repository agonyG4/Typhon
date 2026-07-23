# X11 Client Resize Render Target Design

## Problem

Steam does not advertise `_NET_WM_SYNC_REQUEST_COUNTER` and resizes its floating X11 window through ordinary `ConfigureRequest` events. Typhon immediately updates the authoritative X11 frame position and size, but its renderer continues using the last committed Wayland surface extent until Xwayland attaches the next buffer. During a rapid drag, the frame and buffer repeatedly alternate between latest-requested and previously-committed extents. The result is a visible old window rectangle, flicker, and sluggish resize.

Hyprland handles this case by damaging the old window box, immediately updating `m_realPosition` and `m_realSize`, configuring Xwayland, damaging the new window box, and scaling a temporarily smaller surface into the new viewport. KWin similarly separates frame/buffer geometry and clips surface content inside a window-sized container.

## Design

Add `render_target_size: Option<BufferSize>` to `RenderableSurface`. This is compositor visual state only; `width` and `height` remain the immutable logical extent of the currently committed `wl_surface` content. Rendering uses `render_target_size` as the destination extent when present while retaining the full source UV, which scales the latest committed texture into the requested box.

When `set_x11_geometry` handles an ordinary X11 client configure and the root surface is renderable, set its `render_target_size` to the constrained requested width and height and update its placement. This immediately makes the visual destination match the X11 frame. The override remains current across commits; when a matching buffer arrives, source and destination naturally become 1:1. Compositor-driven X11 resize, XDG resize, popups, subsurfaces, and Wayland viewport protocol state do not set this field.

Render snapshot and damage comparison already operate on render targets. Once the destination extent participates in `surface_render_space_assignments`, a requested-size change invalidates both old and new bounds. Direct scanout identity checks see destination/source mismatch and reject scanout while temporary scaling is active.

## Testing

- A state regression proves an X11 configure changes `render_target_size` without mutating committed `width` and `height`.
- A render regression proves a target override changes destination size while keeping full source UV.
- A damage regression proves old and new destination bounds are both repainted.
- Existing interactive resize tests prove native Wayland and compositor-driven clipping remain unchanged.

## Non-goals

- No XSync emulation for clients that do not advertise a sync counter.
- No configure throttling.
- No changes to attachment publication order or native output buffering.
- No scaling for native Wayland interactive resize.
