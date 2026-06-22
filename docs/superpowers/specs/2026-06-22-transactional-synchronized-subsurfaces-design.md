# Transactional Synchronized Subsurface Design

Date: 2026-06-22

## Confirmed Failure

The existing integration sequence publishes a default-synchronized child before
its parent:

```text
child wl_surface.commit -> commit surface=2 buffer_id=1 -> renderable now
parent wl_surface.commit -> commit surface=1 buffer_id=2 -> renderable later
```

`set_sync` and `set_desync` are ignored, `set_position` mutates current
placement immediately, and stacking is held in a separate partial pending map.
Consequently a decorated window can expose child and root nodes from different
logical transactions even during a full repaint.

## Chosen Model

Each subsurface has one invariant-enforcing role record containing its parent,
requested synchronization mode, cached surface commit, and parent-latched
position/stack requests. New roles default to synchronized. Effective
synchronization is computed by walking ancestors: a node is synchronized when
its own requested mode or any ancestor mode is synchronized. Parent links are
immutable for the role lifetime and creation rejects cycles.

Each `wl_surface.commit` first captures a complete protocol delta. The delta
contains attachment/removal state, damage, offset, viewport and scale changes,
input region changes, callbacks, and explicit-sync state. Capturing removes the
pending values from `SurfaceData` without modifying current state. An
effectively synchronized child merges the delta into its cached commit. A
desynchronized surface with no synchronized ancestor publishes independently.

An alternative journal of every child commit was rejected because replaying it
would create intermediate visual generations and complicate release semantics.
A buffer-only cache was rejected because it loses damage, callbacks, viewport,
scale, input state, and explicit-sync ownership.

## Tree Collection And Publication

A surface commit collects directly synchronized children and recursively
collects all effectively synchronized descendants. Parent-latched placement and
stack changes are included at the same boundary. Collection validates role
links, buffer geometry, and transaction ownership before current state changes.

The compositor reserves one tree generation, applies the root and collected
nodes while renderer observation is impossible, switches placement and stack
maps, and publishes that generation once. Root resize metadata remains attached
to the root node; only successful root/tree publication clears resize preview.
Cached child commits never independently advance generations, damage history,
callbacks, or presentation feedback.

## Cached Commit Merge

Multiple synchronized commits merge in protocol order. The latest attachment
wins; a superseded never-current buffer is released once. Damage accumulates
conservatively, callbacks append, and the latest value wins for double-buffered
offset, viewport, scale, transform, and region state. A content removal
supersedes a previous attachment. Cached resources remain alive until
publication, supersession, role destruction, or client teardown.

## Synchronization Transitions

`set_sync` changes requested mode immediately but does not publish cached state.
`set_desync` changes requested mode and recomputes effective synchronization.
When the node no longer has a synchronized ancestor, its eligible cached state
publishes through the normal commit preparation path exactly once. Descendants
that become effectively desynchronized are processed recursively. A requested
desynchronized descendant beneath a synchronized ancestor remains cached.

## Position And Stacking

Position and stacking requests are stored on the child role. They are current
only after the parent commit that consumes them. Multiple stack requests update
one pending parent stack in request order. Validation still requires the parent
or a sibling with the same parent. Publication damages conservatively and
switches renderer order and hit testing together.

## Explicit Sync

Cached commits own their acquire/release points. A prepared tree gathers every
acquire dependency. If any dependency is unready, the complete tree remains
pending and the old tree remains current. Readiness publishes the whole tree;
no child dependency can publish alone. Per root, the policy keeps the current
tree, the newest ready transaction, and one newest waiting transaction. A ready
transaction supersedes older waits and cancels their watches before publish;
an unready successor cannot discard the only ready successor. Stale watch
events are rejected by transaction identity.

## Damage, Callbacks, And Presentation

Preparation computes conservative damage for old/new node bounds, resource and
geometry changes, stack overlap, and preview removal. Unknown mapping uses full
damage. Cached or fence-waiting state contributes no render generation or
buffer-age history. Callbacks and presentation feedback move with the tree
generation and complete only after that generation is processed/presented.
Stable `BufferId` remains the renderer cache identity.

## Diagnostics And Metrics

Surface debug logging reports role mode, effective mode, cache/merge/prepare/
wait/publish/supersede decisions, commit/tree generations, buffer identity,
node/dependency/callback counts, and preview state. Counters track cached and
merged commits, prepared/published/waiting/superseded trees, maximum cached
nodes/depth/wait time, and forbidden immediate synchronized publishes.

## Scope Boundary

This design does not add VRR, direct scanout, tearing, KMS fences, cursor or
overlay planes, multi-output, hotplug, HDR, color management, XWayland work, or
unrelated renderer optimization. Surface and buffer damage coordinate spaces
remain separately documented follow-up work unless a synchronized-tree test
requires their distinction.
