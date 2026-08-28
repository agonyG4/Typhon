# Typhon Synchronous SHM Buffer Lifetime Design

## Goal

Separate synchronous `wl_shm` client backing lifetime from output-frame presentation lifetime while preserving O1 render-ahead, synchronized subsurface atomicity, cursor identity, XWayland behavior, and existing DMA-BUF ownership semantics.

## Current-source evidence

| Area | Current path | Finding |
| --- | --- | --- |
| Normal publication | `src/compositor/state/surface_commits.rs`: `commit_surface_buffer()` | Calls `track_committed_buffer_lifetime()` and stores the live `PendingSurfaceBuffer` in `current_surface_buffers`. |
| SHM lifetime routing | `src/compositor/state/surface_commits.rs`: `track_committed_buffer_lifetime()` -> `queue_buffer_release()` | SHM is queued for later release; DMA-BUF uses the separate active/deferred authority. |
| Frame ownership | `src/compositor/state/frames.rs`: `take_frame_batch_for_render()` | Takes `pending_buffer_releases` into `CompositorFrameBatch.shm_buffer_releases`. |
| Presentation terminal | `src/compositor/state/frames.rs`: `complete_frame_batch_releases()` | Emits those SHM releases when a frame batch reaches a render/presentation terminal. |
| Invalid reread | `src/compositor/state/surface_commits.rs`: `commit_surface_damage_only()` | Reads `current_surface_buffers[surface_id].data` again for a bufferless damage-only commit. |
| Cursor invalid reread | `src/compositor/state/surface_commit_cursor.rs`: `commit_cursor_surface_damage_only()` | Has the same reread dependency for cursor damage-only commits. |
| Unassigned adoption | `src/compositor/state/surface_commits.rs`: `commit_unassigned_surface_buffer()` / `adopt_current_surface_content_for_role()` | Retains a full `PendingSurfaceBuffer` until role adoption, so adoption can reread client SHM. |
| Synchronized subsurface | `src/compositor/subsurface.rs`, `src/compositor/state/surface_transactions.rs` | Cached attachments own live pending buffers until the parent transaction applies or supersedes them. |
| XWayland retirement | `src/compositor/state/xwayland_windows.rs`: `retire_xwayland_attachment()` | Removes current SHM and queues another release, so moving SHM release earlier needs an exactly-once transition. |
| Shutdown | `src/compositor/state/shutdown.rs` and `src/compositor/state/frames.rs` | Pending/cached/frame-owned resources converge through cleanup paths that must retain distinct DMA-BUF handling and avoid SHM duplication. |

The existing integration test `wayland_surface_damage_only_commit_updates_existing_shm_snapshot` currently expects mutation of the SHM file after the first commit to become visible without a new attach. That expectation conflicts with the protocol-safe early-release boundary.

## Protocol and reference constraints

The Wayland core protocol permits a compositor to release a buffer before the same commit's frame callback when it maintains its own copy of the surface contents, explicitly identifying this as an SHM optimization. Chromium/Ozone's `WaylandFrameManager` separately tracks frame-callback pacing, buffer readiness, and buffer release/submission bookkeeping. Hyprland and KWin both model surface content and renderer/presentation ownership as separate concerns, though their exact release policies differ. The supplied `session(9).zip`, `session(10).zip`, Hyprland, and KWin snapshots were not present in the current checkout or searched local source roots; native A/B facts supplied with this task remain the qualification baseline.

## Architecture

### 1. Explicit materialization boundary

Introduce a compositor-side current-content representation whose SHM variant contains only an owned `CommittedSurfaceBuffer::ShmSnapshot` and protocol identity metadata (`BufferId`/resource identity), never a readable `wl_buffer` or `ShmBufferData`. `RenderableSurface.buffer` remains the authoritative pixels consumed by rendering.

Keep `PendingSurfaceBuffer` as an unmaterialized client-use lease for attachments that still need to be validated, copied, adopted, or atomically applied. Add a consuming materialization operation that returns owned content plus a one-shot release decision only after a successful full/required-damage copy. A failed copy returns the lease without a release proof; cleanup releases it exactly once when it will never be read.

### 2. State transitions

* Normal XDG/layer/subsurface publication: validate -> copy SHM into owned content -> publish/update `RenderableSurface` -> release that attachment use immediately. Releasing one use does not deduplicate a later attach of the same `wl_buffer` object.
* Bufferless/damage-only: update committed metadata and the already-owned pixels; never obtain or reread a client backing lease.
* Unassigned: retain an explicit unmaterialized lease until role adoption, materialize at adoption, then release. Superseded/destroyed leases release without reading.
* Synchronized subsurface: cached attachments remain leases until parent transaction application; materialize before publishing the child atomically. Superseded cached attachments release without reading.
* Cursor and XWayland: use owned content after materialization and preserve their existing commit-sequence/retirement rules. A new valid attachment is a new use.
* DMA-BUF: retain the existing active/deferred release authority. No SHM early-release operation is applied to DMA-BUF.

### 3. Frame batches

Remove ordinary materialized SHM releases from `CompositorFrameBatch`. Keep frame-batch ownership for callbacks, presentation feedback, timing/FIFO claims, damage lineage, and DMA-BUF release authorities that still require the frame terminal. Shutdown and failed-render cleanup only drain those remaining authorities.

### 4. Metrics and invariants

Add bounded counters for materializations, failures, releases after materialization, deferred/superseded unmaterialized releases, presentation-bound SHM releases, and released-backing read attempts. The implementation will make released-backing reads structurally unavailable from current published state; the read-attempt counter is diagnostic evidence, not the safety mechanism.

## Tests

Add protocol-level tests for early release, released-backing mutation invisibility, same-object reuse, the deterministic two-buffer O1 oracle, synchronized subsurface application, superseded cached uses, unassigned adoption, copy failure, SHM/DMA-BUF/remove/destroy/disconnect/shutdown transitions, cursor, and XWayland. Replace the old damage-only mutation expectation with the owned-content expectation. Preserve partial-damage tests by asserting only the damaged rectangles are copied into owned pixels.

The O1 oracle uses a virtual clock with approximately `6_060_606 ns` refresh intervals and a READY/PENDING output pair. It must demonstrate successful render-ahead and continuous A/B rotation without making client release depend on presentation.

## Scope exclusions

This closure does not change O1 defaults, output refresh advertisement, scheduler policy, DMA-BUF GPU-completion authority, direct scanout/KMS ownership, popup topology damage, resize convergence, or trace volume. Trace rate limiting may be implemented as a separately reviewable commit only after ownership correctness is complete.
