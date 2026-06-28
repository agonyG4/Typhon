# XDG Resize Backpressure And Transactional Commit Design

Date: 2026-06-22

## Objective

Pace interactive XDG resize configure emission by output frame preparation while
keeping client progress bounded. Preserve enough sent configure history to
coalesce ACKs and attach the newest ACKed resize state to the next root commit,
including commits delayed by explicit-sync acquire fences.

## Configure Flow

Each root XDG toplevel owns one `ResizeConfigureFlow`:

- `active_interaction`: the current pointer-driven resize generation;
- `outstanding`: a bounded ledger of sent configures, including serial, request
  geometry, placement, edges, resizing state, interaction ID, sequence, and
  emission time;
- pending ACK state: the newest coalesced ACK state stored with the root
  surface pending state until the next relevant root commit consumes it;
- `queued_latest`: the newest unsent interactive geometry for the active
  interaction;
- `final_pending`: the newest required `resizing=false` geometry for the active
  interaction after the pointer grab ends.

Raw pointer updates replace one pending target. `prepare_frame()` consumes the
latest target at most once per output frame opportunity, updates visual
geometry, and queues at most one latest configure. A new resize interaction
discards obsolete unsent `queued_latest` and `final_pending` state from older
interactions, while preserving already-sent configures because protocol
messages cannot be retracted. The ledger allows a newer configure to be sent
without waiting for every older buffer commit. Storage remains bounded per
surface and intermediate pointer geometries may be skipped without losing the
current interaction's final target.

## ACK And Commit Transactions

ACK selects the newest outstanding resize configure whose serial is covered by
the ACK serial. Older outstanding configures are removed as coalesced; newer
ones remain outstanding. A newer ACK replaces an older uncaptured ACKed state.
Duplicate, stale, and unknown ACKs are classified and diagnosed.

The ACKed resize state is stored in pending root surface state. The next root
`wl_surface.commit` atomically consumes that pending ACK and attaches an
immutable snapshot to the concrete buffer, bufferless commit, explicit-sync
wait, or ordered surface-tree transaction. A captured transaction contains the
resize interaction ID, serial and sequence, requested geometry, placement,
edges, resizing state, actual committed content size when known, commit
sequence, and optional new `BufferId`. Child/subsurface commits cannot capture
root state. Commits received before ACK capture nothing. Once captured, later
configures or ACKs cannot change what the commit means, and the configure flow
does not retain that committed buffer for backpressure.

Immediate commits apply the snapshot directly. Explicit-sync pending commits
store the same snapshot and apply it after acquire readiness without consulting
mutable global resize state. Applying an intermediate `resizing=true` snapshot
updates committed content dimensions while preserving active toplevel visual
geometry. Only a matching `resizing=false` snapshot clears active visual resize
ownership and records completion. Applying an older snapshot may update
committed content, but it cannot clear visual resize ownership for a newer
interaction.

## Explicit-Sync Anti-Starvation

The current visible buffer remains presentable. Per surface, the compositor
retains the oldest waiting acquire commit as a progress candidate and one
newest waiting successor. Additional unready successors replace only the
newest waiting successor, never the oldest candidate. If a successor becomes
ready first, it explicitly supersedes older waits and cancels their watches
before application; a newer unready successor cannot discard a ready state.
Stale fence readiness is rejected by exact acquire commit identity.

This bounds waiting state at two commits per surface while preventing rapid
unready commits from indefinitely canceling every potential successor.

## Visual Resize Lifecycle

Visual resize starts when local desired geometry differs from committed content.
`ToplevelVisualGeometry` records the compositor-owned window box, and
`ActiveToplevelResize` records interaction ID, edges, flow sequence, and
activation time. Coalesced pointer motion updates desired visual geometry
without resetting its age. A rapid re-resize transfers visual ownership to the
newer interaction and starts from current visual geometry, not stale committed
XDG geometry. Applying a captured transaction clears active visual resize only
when the snapshot owns the current interaction and is the final
`resizing=false` commit. Unmap, role destruction, cancellation, maximize, and
fullscreen clear flow and active visual resize. Preview age is diagnostic; no
timeout drives synchronization. The compositor renders no resize-specific
border, backdrop, shadow, tint, or outline.

Renderable size domains stay separate. `RenderableSurface.width/height` always
describe committed `wl_surface` content after scale/viewport handling. XDG
window geometry remains shell geometry for CSD margins, placement, and configure
math; it aligns the root surface tree inside the visual box by subtracting the
XDG geometry offset from the visual origin. During interactive resize, client
surfaces render at committed 1:1 size with valid `[0,1]` UVs and are clipped by
the visual toplevel box. Growth does not stretch or edge-extend textures;
shrinking crops by clipping.

## Diagnostics And Metrics

Opt-in surface logging reports flow surface, in-flight serial, queued size,
final state, ACK/capture/apply decisions, commit sequence, buffer presence,
explicit-sync status, preview state, and age. Counters cover requested and sent
configures, coalescing, matched/stale/unknown ACKs, captured commits,
explicit-sync delays, preview activation/completion/age, and maximum retained
configure and pending acquire counts. Rapid-resize diagnostics additionally count
interaction starts, obsolete queued/final state discarded, stale interaction
commits applied while preserving newer preview, preview ownership transfers,
final configures sent, cancellations, raw pointer resize updates, replaced
pending targets, applied paced updates, unchanged rounded skips, duplicate
configure skips, and retained configure peaks. Preview frame rect count is
expected to remain zero.

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
