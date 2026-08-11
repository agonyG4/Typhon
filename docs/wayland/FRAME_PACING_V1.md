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
timer publishes a surface. Commit Timing additionally carries an explicit
readiness plan: the requested lower bound, selected presentation target, and
render-release deadline. The native planner arms that plan before the target's
render-start deadline, while completion still validates the requested lower
bound.

FIFO barriers receive a monotonically increasing generation and retain the
surface presentation generation, commit sequence, and fallback deadline. A
frame batch carries exact claims for the active barriers it owns. Presentation
of the claimed surface, surface teardown, or the bounded forward-progress
fallback can clear a claim. A no-visual-change or same-buffer direct-scanout
result is not itself a FIFO content-latching event. Stale
generation/sequence claims are ignored and counted.

Commit Timing timestamps preserve the full 64-bit seconds field and validate
`tv_nsec < 1_000_000_000`. Comparisons use the seconds/nanoseconds tuple and
total-nanosecond calculations use `u128`; contemporary monotonic timestamps are
used directly. Realtime timestamps are converted with a paired
monotonic/realtime clock sample. A target beyond the native `u64` monotonic
horizon receives a finite re-evaluation wake and is never treated as due.

## Ordering and bounded resources

Pacing-protected transactions are ordered boundaries. Ordinary explicit-sync
transactions may be superseded only before a pacing boundary; neither a pacing
boundary nor an older pacing-protected transaction is silently merged or
discarded. Synchronized-subsurface FIFO waits are marked ignored at the commit
boundary, as required by the effective synchronization state at that commit.

Each root has at most eight queued surface-tree transactions. Ordinary pressure
can retire an unready coalescible transaction. If the queue contains only
pacing-protected work, a new pacing boundary is released and the owning client
is terminated through the Wayland display no-memory/resource-exhaustion error
path after dispatch; the boundary is never silently superseded and no
`wl_surface` semantic error is fabricated.

An active FIFO barrier is refresh-aware but finite: the fallback is at least
34 ms and otherwise one and one-quarter output refresh intervals. The fallback
is a forward-progress guard, not an independent presentation clock.

## Native integration

The native loop folds surface pacing deadlines into its existing event-loop
deadline arbitration during normal operation, output suspension, shutdown, and
session recovery. A pending timed transaction is itself frame-prepare work;
planning is not gated on an already-queued visual flag. Commit Timing calls
`PresentationDeadlinePlanner::plan_not_before`, which chooses the first
refresh target reachable after both the requested timestamp and predicted
render cost. The selected target is armed on the transaction before release
and retained in the frame batch/output transaction, including direct-scanout
target selection.

The same frame-batch claim and completion path is used for composited output,
direct scanout, no-visual-change settlement, release completion, and
presentation feedback. Explicit-sync acquire readiness remains a prerequisite;
FIFO and Commit Timing do not bypass it.

## Evidence

Deterministic coverage includes capability-negative and qualified registry
tests, real socket bindings, exact duplicate/invalid/destroyed protocol errors,
one-shot timestamp state, FIFO ordering with fresh post-publication barrier
readiness, stale FIFO generations, timed-head ordering, hidden-surface finite
progress, refresh-aware fallback, full timestamp storage including the maximum
seconds value, realtime conversion, selected-target readiness/ownership,
frame-batch claims, and the Commit Timing planner at 60/120/165 Hz. Run the
focused checks with:

```text
cargo test --locked compositor::tests::frame_pacing -- --nocapture
cargo test --locked presentation_deadline -- --nocapture
cargo test --locked surface_pacing -- --nocapture
```

Real TTY/DRM qualification remains an environment-dependent gate. It must be
reported separately from deterministic Linux tests and must exercise direct
scanout, composited output, explicit-sync buffers, hidden surfaces, and
60/120/165 Hz outputs on the target hardware.
