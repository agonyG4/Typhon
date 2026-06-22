# Interactive Resize Buffer Identity Design

Date: 2026-06-22

## Objective

Keep the newest client content visible across interactive resize. A replacement
`wl_buffer` must never select an EGLImage imported for an older buffer merely
because the kernel reused a numeric file descriptor, and an acknowledged XDG
resize must accept valid client-selected geometry instead of waiting forever
for exact configure dimensions.

## Confirmed Existing Failure Paths

The EGL renderer currently keys inactive dmabuf resources by surface ID,
dimensions, format, plane layout, modifier, and raw plane fd. Kernel fd reuse
therefore makes a newly created `wl_buffer` indistinguishable from a destroyed
buffer with matching metadata. A full repaint still samples the stale texture,
so repaint policy cannot correct this identity error.

The resize state currently promotes a configure on ACK but removes the pending
transaction only when committed window/content dimensions exactly equal the
configure dimensions. Cell-aligned terminal geometry can remain pending
indefinitely, preserving resize preview state and suppressing normal size
progression even though commits and rendering continue.

## Stable Buffer Identity

`BufferId` is a nonzero, monotonically allocated compositor identity attached
to each server-side `wl_buffer` at creation. SHM and dmabuf userdata both carry
it. Clones of a buffer representation preserve the ID and a shared lifecycle
token; a new protocol buffer receives a new ID even if it contains duplicated
fds or receives a recently reused numeric fd.

Committed and renderable buffer representations carry `BufferId`, making
buffer replacement explicit independently of surface generation. Raw fds are
used only to build EGL import attributes and optional diagnostics.

## Dmabuf Cache Identity And Lifetime

The dmabuf cache key consists of `BufferId` plus immutable import compatibility
metadata: size, format, plane index, offset, stride, and modifier. Raw fds are
excluded. The renderer separates cache policy from GL/EGL destruction so key,
reuse, eviction, import-failure, and generation behavior can be tested without
a physical EGL device.

An active surface owns its selected image. Replaced images may enter the
inactive cache so a still-live swapchain buffer can be reused. Each cached
entry has a weak reference to the buffer lifecycle token. A sweep destroys
entries whose protocol/current/renderable ownership has ended, while a small
per-surface bound prevents retained live swapchains from growing without
limit. Removal transfers ownership exactly once, so each texture/EGLImage is
destroyed once with the renderer context current. Renderer recreation starts
with an empty cache generation; failed imports publish no entry.

Explicit-sync acquire identity, release points, `wl_buffer.release`, frame
callbacks, and presentation feedback remain independent of texture identity.

## XDG Resize Transactions

Sent resize configures retain serial, requested geometry, placement, edges,
and resizing state. ACK processing selects the newest sent resize not newer
than the ACK serial. An ACK older than the already acknowledged transaction is
ignored, and sent entries through the ACK serial are pruned.

An acknowledged transaction is accepted by the next valid commit that applies
new content geometry or committed XDG window geometry. Acceptance does not
require exact equality with requested dimensions. The actual logical surface
or window-geometry size drives final placement, preserving the opposite edge
for left/top anchored resize. A bufferless commit with neither new geometry nor
other relevant surface state does not consume the transaction.

Window geometry is treated as commit-associated state for resize decisions so
a geometry request cannot retroactively satisfy an older surface commit.
Repeated configures supersede older sent state through serial ordering; a
commit after an older ACK cannot complete a newer unacknowledged configure.
Successful acceptance removes the pending transaction and resize preview.

## Rendering Safety

`BufferId` participates in surface scene signatures. Replacing a visible
buffer therefore rebuilds commands even when geometry is unchanged. A changed
buffer identity receives full surface damage, invalidates unsafe partial
repaint history, and selects the resource associated with the new key. Unknown
or invalid damage continues to use the existing full-repaint fallback, but
full repaint is not the cache identity fix.

## Diagnostics And Metrics

Existing opt-in surface/performance logging will include surface ID, protocol
buffer object ID, `BufferId`, raw plane fds for observation only, complete
dmabuf layout, cache hit/miss/creation/eviction, selected resource identity,
configure and ACK serials, requested and actual geometry, viewport state, and
resize decision. Default logging remains quiet.

Renderer metrics include current and peak inactive dmabuf entries, imports,
reuses, failures, and evictions.

## Lifecycle

```text
wl_buffer creation
  -> allocate BufferId + lifecycle token
  -> attach/commit carries identity
  -> renderer lookup by BufferId + immutable layout
       -> active hit: reuse selected image
       -> inactive live hit: transfer cached image to surface
       -> miss: import and publish only after complete success
  -> surface replacement moves old live image to bounded inactive cache
  -> wl_buffer/current/renderable ownership ends
  -> weak-token sweep destroys inactive image once
  -> renderer shutdown drains active and inactive images with current context
```

## Testing

Tests are written failing-first for allocation, raw-fd reuse isolation,
same-buffer reuse, metadata incompatibility, exact-once eviction, bounded
swapchain growth, renderer generation invalidation, import failure, and
explicit-sync commit isolation.

Resize tests cover exact and cell-aligned sizes, commit-before-ACK, stale ACKs,
configure bursts, left/top anchoring with actual dimensions, geometry-only and
bufferless commits, viewport state, SHM/dmabuf parity, preview cleanup, and
newest-buffer visibility. An integration-style fake sequence recreates a Kitty
swapchain with reused raw fd values and verifies B, C, and D become renderer
inputs in order with no residual resize or renderer resource state.

## Non-Goals

This design does not add VRR, direct scanout, tearing control, KMS fences,
cursor or overlay planes, damage clips, batching, multi-output, hotplug, or a
broad XDG shell redesign.
