# Wayland FIFO v1 and Commit Timing v1

Typhon implements the production frame-pacing path behind the qualified native
capability profile. The safe compositor baseline does not advertise either
global. The native profile advertises exactly version 1 of
`wp_fifo_manager_v1` and `wp_commit_timing_manager_v1`.

## Protocol and state model

`wp_fifo_v1` and `wp_commit_timer_v1` are one-per-`wl_surface` resources. Their
requests are staged in double-buffered `SurfaceData` state and become part of
the exact `wl_surface.commit` that follows them. Invalid timestamps, duplicate
resources, duplicate pending timestamps, and requests after surface teardown
use the protocol-defined error values.

The captured pacing state travels with the ordered surface-tree transaction.
Explicit acquire readiness, FIFO barrier readiness, and Commit Timing readiness
are evaluated together immediately before publication; no independent pacing
timer publishes a surface.

FIFO barriers receive a monotonically increasing generation and retain the
surface presentation generation, commit sequence, and fallback deadline. A
frame batch carries exact claims for the active barriers it owns. Presentation,
the no-visual-change latching deadline, direct scanout of the claimed surface,
surface teardown, or the bounded forward-progress fallback can clear a claim.
Stale generation/sequence claims are ignored and counted.

Commit Timing timestamps preserve the full 64-bit seconds field and validate
`tv_nsec < 1_000_000_000`. Monotonic timestamps are used directly. Realtime
timestamps are converted with a paired monotonic/realtime clock sample and are
never released before the requested lower bound.

## Ordering and bounded resources

Pacing-protected transactions are ordered boundaries. Ordinary explicit-sync
transactions may be superseded only before a pacing boundary; neither a pacing
boundary nor an older pacing-protected transaction is silently merged or
discarded. Synchronized-subsurface FIFO waits are marked ignored at the commit
boundary, as required by the effective synchronization state at that commit.

Each root has at most eight queued surface-tree transactions. Ordinary pressure
can retire an unready coalescible transaction. If the queue contains only
pacing-protected work, a new pacing boundary is rejected with a fatal resource
error after its resources are released; the boundary is never silently
superseded.

An active FIFO barrier is refresh-aware but finite: the fallback is at least
34 ms and otherwise one and one-quarter output refresh intervals. The fallback
is a forward-progress guard, not an independent presentation clock.

## Native integration

The native loop folds surface pacing deadlines into its existing event-loop
deadline arbitration during normal operation, output suspension, shutdown, and
session recovery. Commit Timing also calls
`PresentationDeadlinePlanner::plan_not_before`, which chooses the first
refresh target reachable after both the requested timestamp and predicted
render cost. The selected target is retained through reactive and predictive
paths, including direct-scanout target selection.

The same frame-batch claim and completion path is used for composited output,
direct scanout, no-visual-change settlement, release completion, and
presentation feedback. Explicit-sync acquire readiness remains a prerequisite;
FIFO and Commit Timing do not bypass it.

## Evidence

Deterministic coverage includes capability-negative and qualified registry
tests, real socket bindings, exact duplicate/invalid/destroyed protocol errors,
one-shot timestamp state, FIFO ordering, hidden-surface finite progress,
refresh-aware fallback, full timestamp storage, realtime conversion, ordered
prefix boundaries, synchronized-subsurface behavior, frame-batch claims, and
the Commit Timing planner at 60/120/165 Hz. Run the focused checks with:

```text
cargo test --locked compositor::tests::frame_pacing -- --nocapture
cargo test --locked presentation_deadline -- --nocapture
cargo test --locked surface_pacing -- --nocapture
```

Real TTY/DRM qualification remains an environment-dependent gate. It must be
reported separately from deterministic Linux tests and must exercise direct
scanout, composited output, explicit-sync buffers, hidden surfaces, and
60/120/165 Hz outputs on the target hardware.
