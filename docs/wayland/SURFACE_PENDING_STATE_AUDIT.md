# Typhon surface pending-state audit

This audit records the ownership boundary that is observable at one
`wl_surface.commit`. The implementation keeps request-local storage split in
`SurfaceData` for compatibility with the existing synchronization paths, but
`src/compositor/protocols/core.rs` captures all fields below before calling the
publication/validation transaction. Internal storage shape is therefore not
treated as a protocol gap by itself.

| property | request-time storage owner | commit capture owner | publication owner | failure rollback | synchronized subsurface owner | teardown owner |
|---|---|---|---|---|---|---|
| attachment / NULL | `SurfaceData.pending_buffer` | `wl_surface::commit` + `surface_transactions` | `commit_surface_buffer_by_role` / unmap path | pending attachment remains unpublished; release target is terminally owned | `CachedSubsurfaceCommit.attachment` | `teardown_surface_resource` / shutdown release |
| surface damage | `SurfaceData.pending_surface_damage` | `take_pending_damage` | validated damage transaction | invalid commit never publishes damage | cached commit damage | surface teardown |
| buffer damage | `SurfaceData.pending_buffer_damage` | `take_pending_damage` | validated damage transaction after transform/scale | invalid commit never publishes damage | cached commit damage | surface teardown |
| offset | `SurfaceData.pending_offset` | `take_pending_offset` | role-specific commit | pending value is consumed only by its commit | cached commit offset where applicable | surface teardown |
| buffer scale | `SurfaceData.buffer_scale` | `take_pending_buffer_scale` | validated surface publication | invalid scale posts `wl_surface.invalid_scale` before publication | cached commit scale | surface teardown |
| buffer transform | `SurfaceData.buffer_transform` | `take_pending_buffer_transform` | validated surface publication and renderer client-buffer geometry | invalid enum/size posts protocol error; no partial publication | cached commit transform | surface teardown |
| opaque region | `SurfaceData.opaque_region` | `take_pending_opaque_region` | current published culling hint only | pending snapshot is discarded on failed transaction | cached commit opaque region | surface teardown |
| input region | `SurfaceData.input_region` | `take_pending_input_region` | current hit-test state | pending snapshot is discarded on failed transaction | cached commit input region | surface teardown |
| viewport source/destination | `SurfaceData.viewport` | `take_pending_viewport` and `viewport_for_change` | validated logical-size publication | invalid viewport remains unpublished | cached viewport change | surface teardown |
| frame callbacks | `SurfaceData.frame_callbacks` | `take_frame_callbacks` | frame-owned completion queues | failed commit completes/discards exactly once | cached commit callbacks | teardown/shutdown disposition |
| presentation feedback | `SurfaceData` / explicit-sync capture | commit capture | frame-batch/presentation owner | discarded on failed or abandoned commit | cached feedback vector | teardown/shutdown disposition |
| explicit-sync acquire/release | `SurfaceData.explicit_sync` | `CapturedExplicitSyncState` | pending explicit-sync commit queue | protocol error leaves no unrelated fields published | cached explicit-sync state | acquire-watch and shutdown cleanup |
| XDG window geometry | `pending_surface_window_geometries` | commit removes one pending snapshot | XDG/window publication | invalid size posts `xdg_surface.invalid_size` | cached commit geometry | XDG/surface teardown |
| subsurface position/stack/sync | subsurface pending maps and role lifecycle | parent transaction capture | `apply_cached_subsurface_commit` | invalid restack leaves current order unchanged | `SubsurfaceTransactionState` | role/client teardown |

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
