# Typhon Surface and Presentation Locality v2

**Date:** 2026-08-27

## Scope

This closure makes surface content, frame sampling, presentation settlement,
and topology changes obey separate ownership boundaries. The existing dirty
checkout remains authoritative. The ordered compositor render vector remains
the source of stacking order; no unordered collection replaces it.

## Indexed renderable authority

`CompositorState` owns a sidecar `HashMap<u32, usize>` mapping each
`RenderableSurface.surface_id` to its position in `renderable_surfaces`. Narrow
helpers own indexed lookup, append, in-place replacement, removal, and full
rebuild. Content paths use the sidecar without rebuilding it. Topology paths
rebuild it after a successful reorder, retain/removal, drain, or replacement
that changes vector membership or order.

Debug/test-only invariant validation checks length, uniqueness, and both
directions of the map/vector relationship. It is not run on every release
content commit. Active-scene indices remain a separate cache of cloned active
scene positions.

## Content versus topology

Wayland buffer and damage-only commits resolve their existing renderable once,
derive all buffer, mapping, and damage decisions from that state, and mutate
that indexed entry. Initial mapping is the only content-path insertion.

Mapped XWayland buffer commits preserve the existing renderable position and
visual assignment when surface identity, placement, stack membership, root
ownership, and visual geometry are unchanged. They continue to use
conservative full XWayland damage because this closure does not add an X11
partial-damage authority. Reordering, tree reassignment, minimization,
restacking, placement changes, and attachment/topology changes remain explicit
cold paths.

## Presentation lineage

The final `ResolvedNativeFrameScene` supplies the exact primary surface IDs for
a composited frame. A keyed sample-set capture follows that resolution and
deduplicates IDs with a set, so capture work is proportional to the sampled
surfaces. The token is owned by the rendered frame or compatibility frame
batch and is dropped on render failure, submission failure, replacement, or
abandonment. Only the matching confirmed presentation settles it.

Settlement verifies the frozen surface generation, advances the sampled commit
only, obtains the journal by key, and updates either the global renderable by
index or the client cursor map by surface ID. History loss remains conservative
`Full` damage.

Software cursor samples are added only when the final primary plan actually
embeds the current client cursor render state. Hardware client cursor samples
use the frozen `NativeCursorImageKey`; primary-bundled and independent cursor
plane transactions own their samples separately. Theme and hidden cursor
states carry no client surface sample. A superseded cursor transaction drops
its token without settling it.

Direct Scanout keeps its candidate-only capture rule.

## Verification boundary

Deterministic operation counters and tests cover index maintenance, hot-path
locality, final-scene membership, generation safety, cursor delivery, failed
or superseded presentations, and the integrated client/output swapchain model.
No O1, scheduler, KMS-worker policy, buffering, Direct Scanout admission, VRR,
tearing, deadline, workspace, or Dwindle policy is changed.
