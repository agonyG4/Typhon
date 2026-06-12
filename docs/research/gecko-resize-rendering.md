# Gecko Resize Rendering Investigation

## Summary

Zen/Gecko appears to expose a resize artifact in Oblivion where a window is drawn
or visually appears before the client has committed the resized content. The most
likely cause is not an explicit rubber-band or frame-preview path. Instead, the
renderer can draw the current client buffer into a resize target rectangle whose
logical size has advanced ahead of the client commit.

In short:

- Oblivion sends interactive resize configures through `xdg_toplevel.configure`
  and tracks the configure serial.
- Gecko may ack/configure later and commit content asynchronously.
- During the gap, if a frame is rendered, Oblivion can use the new visual target
  size with the old buffer.
- The CPU renderer path scales mismatched buffers to the target rectangle.
- Hyprland has an explicit interactive-resize guard that avoids stretching small
  or mismatched surfaces in the equivalent render path.

## Probable Cause

The probable artifact source is the combination of:

1. `RenderableSurface.width` and `RenderableSurface.height` representing the
   logical surface target used by the compositor renderer.
2. resize configure/ack state being tracked separately from actual client buffer
   arrival.
3. `blit_surface_to_rect()` scaling a buffer when the committed buffer size does
   not match the target rectangle.

Relevant Oblivion code:

- `src/compositor/interaction.rs:147` computes interactive resize geometry.
- `src/compositor/mod.rs:1752` updates the active window interaction.
- `src/compositor/mod.rs:1774` queues the requested resize size and placement.
- `src/compositor/mod.rs:2208` sends resize configures with the `Resizing` state.
- `src/compositor/mod.rs:2272` promotes acked configure serials into pending
  resize commits.
- `src/compositor/mod.rs:792` applies a committed buffer to the renderable
  surface.
- `src/compositor/render.rs:257` draws each surface using its current logical
  target rectangle.
- `src/compositor/render.rs:674` takes the fast path only when buffer and target
  sizes match.
- `src/compositor/render.rs:699` scales the old buffer into the target rectangle
  when sizes differ.

That last fallback is useful for viewport/scaling cases, but during live resize
it can look like a preview or phantom window: the compositor presents a new
window rectangle before Gecko has committed pixels for that rectangle.

## Oblivion Flow

### Begin Interaction

Resize starts through compositor input paths:

- `OwnCompositorServer::begin_window_resize_at()` forwards to compositor state:
  `src/compositor/server.rs:149`.
- `CompositorState::begin_window_resize_at()` finds the surface under the pointer:
  `src/compositor/mod.rs:1609`.
- `begin_window_interaction_for_root()` records the starting pointer, placement,
  width, and height in `WindowInteraction`: `src/compositor/mod.rs:1678`.

For client-requested resize, the xdg request also routes into the same
interaction model:

- `begin_client_window_resize()` validates the pointer serial and requested
  edge: `src/compositor/mod.rs:1659`.

### Pointer Motion

On motion:

- `update_window_interaction()` calculates pointer delta:
  `src/compositor/mod.rs:1752`.
- resize ignores tiny movements until the drag threshold is reached:
  `src/compositor/mod.rs:1768`.
- `interactive_resize_geometry()` calculates target x/y/width/height:
  `src/compositor/interaction.rs:147`.
- `queue_resize_root_window_to()` stores one pending resize configure:
  `src/compositor/mod.rs:2118`.

The pending configure is coalesced until the presentation path flushes it:

- `OwnCompositorServer::present_frame()` calls
  `flush_pending_resize_configure()`: `src/compositor/server.rs:212`.
- `flush_pending_resize_configure()` sends the latest pending configure:
  `src/compositor/mod.rs:2147`.

### Configure, Ack, Commit

Resize configures are sent as xdg toplevel size + xdg surface serial:

- `send_resize_configure_to()` adds `xdg_toplevel::State::Resizing` while active:
  `src/compositor/mod.rs:2208`.
- `send_configure_root_window_to()` sends `xdg_toplevel.configure` and then
  `xdg_surface.configure`: `src/compositor/mod.rs:2288`.

Ack handling:

- `ack_xdg_surface_configure()` removes the matching sent resize configure and
  stores it as the pending resize commit for that surface:
  `src/compositor/mod.rs:2272`.

Commit handling:

- `commit_surface_request()` configures the initial xdg surface if needed, then
  commits the pending buffer: `src/compositor/mod.rs:945`.
- `commit_surface_buffer()` applies pending resize placement only when a pending
  resize commit exists: `src/compositor/mod.rs:792`.
- `take_pending_resize_commit_placement()` uses committed geometry or buffer
  size to anchor top/left edge resizes correctly: `src/compositor/mod.rs:2249`.

### Rendering

Rendering draws surfaces with their current compositor target:

- `draw_client_surfaces_scaled()` builds a target rect from
  `surface.width` and `surface.height`: `src/compositor/render.rs:257`.
- `blit_surface_to_rect()` copies directly only when buffer size and target size
  match: `src/compositor/render.rs:674`.
- otherwise it scales source pixels into the target size:
  `src/compositor/render.rs:699`.

This is the likely visual mismatch for Gecko. The compositor may have a target
geometry that represents requested resize state while the client buffer still
represents the previous content size.

## Hyprland Flow Observed

Hyprland keeps separate concepts for:

- logical/window box,
- real draw size,
- reported size,
- pending configure acks,
- committed client texture.

Relevant files:

- `src/desktop/view/Window.hpp:150` defines `m_realPosition` and `m_realSize`,
  the real draw position and size.
- `src/desktop/view/Window.hpp:154` defines reported and pending reported size
  fields.
- `src/desktop/view/Window.cpp:1633` sends window size only when the reported
  size changes.
- `src/desktop/view/Window.cpp:1654` stores xdg configure serials with the
  requested report size.
- `src/desktop/view/Window.cpp:1414` consumes configure acks and applies the
  acked size into pending surface state.
- `src/desktop/view/Window.cpp:2575` handles actual window commits.
- `src/desktop/view/Window.cpp:2613` damages the committed wl surface.

For drag resize:

- `src/layout/supplementary/DragController.cpp:86` begins a drag and records
  drag state.
- `src/layout/supplementary/DragController.cpp:235` handles drag motion.
- `src/layout/supplementary/DragController.cpp:278` throttles resize updates
  around monitor cadence.
- `src/layout/target/WindowTarget.cpp:40` damages before and after position
  updates.
- `src/layout/target/WindowTarget.cpp:50` updates floating window real position
  and real size, then sends the window size.

The most relevant render difference is in Hyprland's surface pass:

- `src/render/pass/SurfacePassElement.cpp:26` detects interactive resize in
  progress.
- `src/render/pass/SurfacePassElement.cpp:42` avoids the normal correction that
  stretches a smaller surface to the current real window size while interactive
  resize is active.

This is the closest observed equivalent to the fix Oblivion likely needs.
Hyprland does not appear to render a separate rubber-band preview for this path;
it renders current client textures and decorations while carefully controlling
when a surface is stretched.

## Recommended Tests

Add tests that reproduce the visual gap instead of only testing configure flow.
The existing resize tests cover important protocol state, but not the intermediate
render behavior with a stale Gecko-like buffer.

Recommended additions:

1. Acked resize without resized buffer must not stretch old content.

   Scenario:

   - create a 300x200 toplevel with a recognizable checker/pattern buffer.
   - begin resize to 500x350.
   - deliver/ack configure.
   - commit damage-only or a same-size old buffer.
   - render a frame.
   - assert the old buffer is drawn at old committed dimensions, or clipped, but
     not scaled to 500x350.

2. Final resized commit applies requested geometry.

   Scenario:

   - continue from the first test.
   - commit a 500x350 buffer.
   - assert the render target now uses 500x350.
   - assert top/left edge anchors still match expected placement.

3. Left/top edge resize keeps old committed origin until compatible client commit.

   Existing coverage:

   - `left_edge_resize_shrink_keeps_old_buffer_origin_until_client_commit`
     already checks one part of this behavior.

   Extend it with a render assertion to ensure no intermediate stretch appears.

4. Gecko-style configure coalescing.

   Scenario:

   - send several resize motions before present.
   - flush one configure.
   - ack an older configure after a newer one was sent.
   - ensure stale ack does not promote a stale render size.

5. Damage region covers old and new bounds during resize.

   Scenario:

   - when a visual resize finally applies, ensure output damage includes both
     previous bounds and new bounds to avoid leftover pixels.

## Patch Plan

### 1. Model committed size separately from requested resize size

Introduce explicit resize render state per root surface, for example:

```rust
struct SurfaceResizeState {
    requested_width: u32,
    requested_height: u32,
    requested_placement: SurfacePlacement,
    acked_serial: Option<u32>,
    resizing: bool,
}
```

Keep this separate from the current committed `RenderableSurface` size. The
renderer should not infer that a requested configure is drawable client content.

### 2. Promote resize geometry only on compatible client commit

In `commit_surface_buffer()` or near `take_pending_resize_commit_placement()`:

- check whether the committed logical size matches the acked/requested resize.
- if it matches, apply requested placement and size.
- if it does not match, keep the current committed render size and placement.

For Gecko, this prevents an ack or damage-only commit from advancing the visual
content rectangle before the real resized buffer exists.

### 3. Render stale buffers without live stretch during interactive resize

Adjust `blit_surface_to_rect()` or the call site in `draw_client_surfaces_scaled()`
so that resize-pending root surfaces do not use the scaled fallback.

Possible approaches:

- add a render flag to `RenderableSurface`, e.g. `resize_pending: bool`.
- build a target rect from committed buffer dimensions while `resize_pending`
  is true.
- or add a `SurfaceDrawMode::ExactOrClip` path that draws the current buffer at
  its committed size and clips to the requested window bounds.

The Hyprland analog is the interactive-resize guard in
`SurfacePassElement.cpp:26` and `SurfacePassElement.cpp:42`.

### 4. Damage both previous and new bounds

When the compatible client commit finally applies:

- damage the previous rendered bounds.
- damage the new rendered bounds.

This prevents stale pixels after clipped/no-stretch intermediate rendering.

### 5. Keep configure coalescing

Do not remove current coalescing behavior:

- `pending_resize_configure` should still keep only the latest pointer-derived
  configure before a frame.
- `sent_resize_commits` should still discard older serials when a newer ack is
  accepted.

The patch should change what becomes renderable, not spam configures.

## Validation

Validation already run during investigation:

```text
cargo test -p oblivion-one resize -- --nocapture
```

Result:

```text
23 tests passed
```

Covered by current tests:

- resize configure delivery,
- resize configure coalescing,
- configure ack promotion,
- resize end clears `Resizing` state,
- left-edge resize placement before client commit,
- scaled-buffer resize logical sizing.

Not yet covered:

- intermediate rendered pixels when Gecko acks configure but has not committed
  a compatible resized buffer,
- no-stretch behavior while resize is pending,
- output damage union of old and new visual bounds.

Manual validation after implementing the patch should include:

1. Launch Oblivion nested.
2. Launch Zen or another Gecko client inside it.
3. Resize with the existing interactive resize gesture.
4. Confirm there is no phantom/stretched pre-resize window.
5. Confirm final resize applies once Gecko commits the resized buffer.
6. Repeat for right/bottom and left/top edge resizes.
7. Repeat with a non-Gecko client to confirm no regression in normal live resize.
