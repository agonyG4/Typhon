# XDG Resize Backpressure And Transactional Commit Design

Date: 2026-06-22

## Objective

Bound interactive XDG resize configure emission by client progress. Preserve
every in-flight resize configure until its ACK is captured by a root surface
commit and that immutable transaction is applied, including commits delayed by
explicit-sync acquire fences.

## Configure Flow

Each root XDG toplevel owns one `ResizeConfigureFlow`:

- `in_flight`: the single sent configure, including serial, request geometry,
  placement, edges, resizing state, sequence, emission time, ACK state, and
  optional captured commit identity;
- `queued_latest`: the newest desired interactive geometry while progress is
  blocked;
- `final_pending`: the newest required `resizing=false` geometry after the
  pointer grab ends.

Pointer updates always update local preview geometry. They request a configure
but never create another in-flight configure. Repeated requests overwrite only
`queued_latest`. When an applied commit completes the in-flight transaction,
the compositor sends `final_pending` first, otherwise `queued_latest`. Thus
storage is constant per surface and intermediate pointer geometries may be
skipped without losing the final target.

## ACK And Commit Transactions

ACK marks the exact in-flight resize configure as acknowledged. It does not
change visible content or clear flow state. Duplicate, stale, and unknown ACKs
are classified and diagnosed.

The next root `wl_surface.commit` atomically captures the acknowledged resize
metadata exactly once. Buffer and bufferless paths use the same capture
operation. A captured transaction contains the resize serial and sequence,
requested geometry, placement, edges, resizing state, actual committed
geometry when known, commit sequence, and optional new `BufferId`.
Child/subsurface commits cannot capture root state. Commits received before ACK
capture nothing. Once captured, later configures or ACKs cannot change what the
commit means.

Immediate commits apply the snapshot directly. Explicit-sync pending commits
store the same snapshot and apply it after acquire readiness without consulting
mutable global resize state. Applying a matching snapshot clears preview,
completes the in-flight flow once, and emits the newest queued/final request.

## Explicit-Sync Anti-Starvation

The current visible buffer remains presentable. Per surface, the compositor
retains the oldest waiting acquire commit as the next progress candidate and
one newest waiting successor. Additional unready successors replace only the
newest waiting successor, never the oldest candidate. A newly ready commit may
supersede waiting commits because it is immediately presentable. Stale fence
readiness is rejected by exact acquire commit identity.

This bounds waiting state at two commits per surface while preventing rapid
unready commits from indefinitely canceling every potential successor.

## Preview Lifecycle

Preview starts when local desired geometry differs from committed content. It
records committed dimensions, anchor direction, flow sequence, and activation
time. Coalesced pointer motion updates desired geometry without resetting its
age. Applying the captured resize transaction clears preview once. Unmap, role
destruction, cancellation, maximize, and fullscreen clear both flow and
preview. Preview age is diagnostic; no timeout drives synchronization.

## Diagnostics And Metrics

Opt-in surface logging reports flow surface, in-flight serial, queued size,
final state, ACK/capture/apply decisions, commit sequence, buffer presence,
explicit-sync status, preview state, and age. Counters cover requested and sent
configures, coalescing, matched/stale/unknown ACKs, captured commits,
explicit-sync delays, preview activation/completion/age, and maximum in-flight
and pending acquire counts.

## Testing

Tests drive the flow model directly for 1,000 no-ACK updates, delayed ACK beyond
the former 32-entry window, coalescing, independent surfaces, destruction,
final resize, duplicate/stale/unknown ACKs, buffer and bufferless transaction
capture, child isolation, actual-size anchoring, explicit-sync delayed apply,
supersession, and anti-starvation. A regression sequence reproduces surface 8,
serial 308, 300+ pointer updates, delayed ACK/commit, and final
`resizing=false` completion.

## Non-Goals

No VRR, direct scanout, tearing control, KMS fences, cursor/overlay planes,
multi-output, hotplug, HDR, shell redesign, or broad renderer optimization is
included.
