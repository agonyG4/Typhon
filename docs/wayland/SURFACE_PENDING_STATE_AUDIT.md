# Typhon surface pending-state audit

This audit records the ownership boundary that is observable at one
`wl_surface.commit`. The implementation keeps request-local storage split in
`SurfaceData` for compatibility with the existing synchronization paths, but
`src/compositor/protocols/core.rs` captures all fields below before calling the
publication/validation transaction. Internal storage shape is therefore not
treated as a protocol gap by itself.

## Publication pipeline

The phases have directional dependencies:

```text
wl_surface.commit capture
        ↓
Admission / explicit-sync preparation
        ↓
SurfaceTransactionState
        ↓
local acquire polling or native acquire readiness
        ↓
SurfaceTree publication
        ↓
Canonical compositor state
```

`prepare_surface_tree_acquires()` consumes each cached commit's raw captured
explicit-sync state before the SurfaceTree transaction is queued. It transfers
the release point to the pending buffer and represents an unsignaled acquire
as a transaction dependency. Tree publication asserts that no raw captured
state remains and applies the prepared buffer through
`publish_admitted_surface_buffer()`. That publication path cannot queue
explicit-sync work or progress readiness. There is no separate standalone
explicit-sync commit queue: captured protocol state becomes either a release
point owned by the pending buffer or a `SurfaceTreeAcquireDependency` owned by
`SurfaceTransactionState`.

Subsurface relationship identity and pending position remain in
`SurfaceTransactionState`, while published placement and staged stack ordering
belong to `SurfaceTopologyState`. A parent commit removes its pending stack and
sets that exact order as the latched baseline; transaction capture qualifies
the ordered child IDs with current relationship identities. Publication
revalidates those identities before applying the committed stack and placements,
so a protocol relationship can exist before it appears in published topology.

| property | request-time storage owner | commit capture owner | publication owner | failure rollback | synchronized subsurface owner | teardown owner |
|---|---|---|---|---|---|---|
| attachment / NULL | `SurfaceData.pending_buffer` | `wl_surface::commit` + `surface_transactions` | `publish_admitted_surface_buffer` / unmap path | pending attachment remains unpublished; release target is terminally owned | `CachedSubsurfaceCommit.attachment` | `teardown_surface_resource` / shutdown release |
| surface damage | `SurfaceData.pending_surface_damage` | `take_pending_damage` | validated damage transaction | invalid commit never publishes damage | cached commit damage | surface teardown |
| buffer damage | `SurfaceData.pending_buffer_damage` | `take_pending_damage` | validated damage transaction after transform/scale | invalid commit never publishes damage | cached commit damage | surface teardown |
| offset | `SurfaceData.pending_offset` | `take_pending_offset` | role-specific commit | pending value is consumed only by its commit | cached commit offset where applicable | surface teardown |
| buffer scale | `SurfaceData.buffer_scale` | `take_pending_buffer_scale` | validated surface publication | invalid scale posts `wl_surface.invalid_scale` before publication | cached commit scale | surface teardown |
| buffer transform | `SurfaceData.buffer_transform` | `take_pending_buffer_transform` | validated surface publication and renderer client-buffer geometry | invalid enum/size posts protocol error; no partial publication | cached commit transform | surface teardown |
| opaque region | `SurfaceData.opaque_region` | `take_pending_opaque_region` | current published culling hint only | pending snapshot is discarded on failed transaction | cached commit opaque region | surface teardown |
| input region | `SurfaceData.input_region` | `take_pending_input_region` | current hit-test state | pending snapshot is discarded on failed transaction | cached commit input region | surface teardown |
| viewport source/destination | `SurfaceData.viewport` | `take_pending_viewport` and `viewport_for_change` | validated logical-size publication | invalid viewport remains unpublished | cached viewport change | surface teardown |
| frame callbacks | `SurfaceData.frame_callbacks` | `take_frame_callbacks` | frame-owned completion queues | failed commit completes/discards exactly once | cached commit callbacks | teardown/shutdown disposition |
| presentation feedback | `SurfaceData` | commit capture | frame-batch/presentation owner | discarded on failed or abandoned commit | cached feedback vector | teardown/shutdown disposition |
| explicit-sync acquire/release | `SurfaceData.explicit_sync` | `CapturedExplicitSyncState`; SurfaceTree preparation consumes it before queueing | release point on pending buffer; unsignaled acquire in `SurfaceTreeAcquireDependency` | protocol error leaves no unrelated fields published | SurfaceTree transaction in `SurfaceTransactionState` | acquire-watch and shutdown cleanup |
| XDG window geometry | `pending_surface_window_geometries` | commit removes one pending snapshot | XDG/window publication | invalid size posts `xdg_surface.invalid_size` | cached commit geometry | XDG/surface teardown |
| subsurface relationship identity/phase, pending position, and sync mode | `SurfaceTransactionState` relationship state | parent transaction capture | relationship-qualified topology application | invalid restack leaves current order unchanged | `SurfaceTransactionState` | role/client teardown |
| pending/latched/committed subsurface stack order and applied placement | `SurfaceTopologyState` | pending stack is removed and becomes the latched baseline at parent commit capture | captured child IDs are qualified by `SurfaceTransactionState`, then applied by `SurfaceTopologyState` at transaction publication | stale relationship identities are ignored; invalid restack leaves current order unchanged | `SurfaceTopologyState` | role/client teardown |

## Black-box invariants

- Requests after commit N are captured only by commit N+1.
- A failed validation does not publish a buffer, damage, geometry, callback,
  feedback, explicit-sync watch, region, transform, or scale from that commit.
- A synchronized child publishes with its parent transaction, including its
  callback, feedback, release, viewport, transform, scale, geometry, and
  damage ownership.
- A NULL attachment unmaps visible content while preserving permanent role
  identity and leaves output membership while the surface resource is alive.
- Teardown converges the pending attachment, regions, callbacks, feedback,
  explicit-sync watches, and buffer-release owners exactly once.

Evidence is provided by the `surface_frames`, `subsurface`, `protocol_error`,
and `output_keyboard_cursor` test modules, including the atomic commit and
unmap/remap cases. Future changes that move storage must preserve this table
and the same externally observable tests.
