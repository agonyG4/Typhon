# Typhon client frame-clock admission design

Date: 2026-08-28

Status: approved for implementation

## Problem

Typhon currently completes visible `wl_surface.frame` callbacks at raw render
completion. In the explicit Atomic path,
`AtomicEglGbmScanout::render_frame` calls
`OwnCompositorServer::complete_rendered_frame_callbacks` immediately after
`finish_render_owned` succeeds. `presentation_cycle` only afterward evaluates
`rendered_primary_must_wait_for_lane`, so an O1 frame can be READY while its
client callback has already been sent. The compatibility path has the same
ordering through `complete_rendered_frame_callbacks_for_prepared`.

The exact callback owner already exists: `CompositorFrameBatch`. The missing
state is whether rendering has succeeded, whether the exact output frame has
been admitted, and whether the protocol callback has reached a terminal.

## Source evidence

| Evidence | Current source | Finding |
| --- | --- | --- |
| Exact callback capture | `src/compositor/state/frames.rs::take_frame_batch_for_render` | Visible pending callbacks are captured into a batch with the exact frame identity. |
| Premature protocol terminal | `src/compositor/state/frame_callbacks.rs::complete_rendered_frame_callbacks` | Drains and sends every live callback at renderer completion. |
| Explicit render call | `src/native_output/scanout/atomic_egl_gbm.rs::render_frame` | Calls the premature terminal after `finish_render_owned`. |
| READY decision | `src/native_output/runtime/presentation_cycle.rs` | Decides after rendering whether the primary waits in READY or is submitted immediately. |
| Exact READY identity | `src/native_output/runtime/presentation_ready.rs::submit_ready_frame` | Reads the exact `protocol_batch_id` from the explicit READY identity or prepared compatibility batch. |
| Physical terminal | `src/compositor/state/frames.rs::complete_presented_frame_batch` | Presentation owns feedback and DMA-BUF completion, but currently does not drain callbacks left in the batch. |
| Direct scanout | `src/native_output/runtime/cycle_direct.rs` and `src/native_output/runtime/kms_worker.rs` | Direct scanout has separate KMS ownership and callback handling; it is not changed by the composited READY policy. |
| SHM ownership | `src/compositor/state/surface_commits.rs` and `src/compositor/state_data.rs` | The prior materialization-bound SHM release closure remains independent of frame callback timing. |

The current official Wayland protocol defines `wl_surface.frame` as a
throttling hint for a good time to start drawing, while `wl_buffer.release`
defines when the compositor no longer uses a buffer. Those are separate from
physical presentation feedback.

## Design

Each `CompositorFrameBatch` keeps its callbacks and records an explicit
callback pacing state:

```text
Captured -> RenderedAwaitingAdmission -> Completed
```

The render terminal only changes `Captured` to
`RenderedAwaitingAdmission` and records render timing. It never sends the
protocol event.

The admission terminal is selected by presentation runtime code, where the
exact output transaction is known:

* an immediately submitted frame completes its exact batch after successful
  KMS/worker admission and all required ownership bookkeeping succeeds;
* a READY frame completes its exact batch after that READY frame is accepted
  into the KMS/worker presentation lane;
* failed, unavailable, stale, or rolled-back admission leaves callbacks in
  their batch and records retention telemetry;
* if a batch reaches physical presentation with callbacks still present, the
  presentation path sends them exactly once as a safety fallback and records
  the fallback counter.

The settlement ledger is renamed to describe actual terminals rather than
render completion. It continues to enforce:

```text
completed + transferred + cancelled + unresolved == originally_owned
```

Callback completion timestamps are recorded at admission. Render timestamps
remain available for latency telemetry. No callback is moved to an unkeyed
global queue.

Protocol-only/no-visual work remains on its existing independent terminal.
Direct scanout remains governed by its KMS submission/presentation ownership
path. Presentation feedback and DMA-BUF release authority remain physical/GPU
terminals, respectively.

## Test boundary

The implementation adds deterministic tests for render-ahead READY deferral,
successful immediate and READY admission, failed admission retry, the
165 Hz two-refresh virtual client oracle, READY invalidation, presentation
fallback, render failure restoration, no-visual callbacks, direct scanout,
and shutdown/resource cleanup. Tests also verify that the existing SHM
materialization closure and callback identity are not weakened.

## Explicit non-goals

This change does not disable O1, alter refresh advertisement, add a client or
Electron branch, modify DMA-BUF GPU-completion ownership, change popup damage
topology, retune resize convergence, or add synchronous high-volume tracing.
