# XWayland Popup Crossing Design

## Goal

Make rapid XWayland override-redirect popup transitions deterministic. One
logical XWayland mutation batch must settle to one scene stack and one final
pointer hit-test, producing at most one leave/enter transition. Managed X11
policy remains compositor-owned while the X server remains authoritative for
the relative order of live override-redirect windows.

This change is limited to XWayland lifecycle, X11 stacking, compositor scene
mutation, pointer crossing, focus policy, tracing, and their tests. KMS,
direct scanout, buffering, cursor planes, output transactions, pageflip
ownership, and explicit-sync ownership are out of scope.

## Current Root Cause

The existing path exposes temporary scene states during a popup burst:

- XWM, association, buffer-ready, and surface-attachment paths refresh pointer
  focus independently.
- An observed override-redirect `ConfigureNotify` updates compositor stacking
  and emits a complete `RestackExact` writeback.
- Transient-family rebuilding and raising include override-redirect windows.
- XWayland trace output is first-N limited, and normal flushes consume one
  record per call.

These behaviors allow stale or intermediate popup states to participate in
pointer crossing and can create a ConfigureNotify/restack feedback loop.

## Design Decisions

### Explicit scene batch

`CompositorState` gains an explicit XWayland scene-batch guard. The native
cycle begins the batch before association dispatch and ends it after:

1. XWayland association events;
2. buffer-level and buffer-ready events;
3. XWM window events;
4. XWayland backend-command collection.

Logical state changes remain immediate. While a batch is active, scene side
effects set dirty state instead of refreshing pointer focus, reordering the
render stack, synchronizing client lists, or requesting duplicate repaint
work. Direct test/API calls remain immediate when no batch is active.

The guard is fail-safe. A scoped guard commits on normal drop or explicitly
returns its commit outcome, and commit state cannot leave the compositor in a
permanent active-batch state after an error or early return. Commit does not
wait for X replies.

### Commit order

Batch commit performs the following steps:

1. Finish admission, withdrawal, destruction, association removal, and
   attachment replacement.
2. Apply only the newest valid override-redirect root-stack snapshot already
   available.
3. Normalize compositor ordering once and reorder renderable surfaces once.
4. Publish one coalesced XWayland client-list update.
5. Perform one hit-test at the last physical pointer position, respecting
   locked-pointer, confined-pointer, and implicit-grab ownership.
6. Send leave for the previous target before enter for the final target.
7. Send one pointer frame per affected client.
8. Request repaint once while preserving the union of all required damage and
   presentation work.

If a snapshot reply is not ready, the current logical state is committed and
the dirty/query state remains pending for a later batch. Applying the same
snapshot twice is a no-op and cannot trigger X stack writeback.

### Override-redirect root stacking

The XWM maintains one root-stack reconciliation state per XWayland
generation:

- a dirty bit and monotonically increasing stack epoch;
- at most one pending QueryTree sequence;
- the epoch captured by that request;
- the newest applied snapshot epoch.

Relevant X events mark the state dirty. If dirty and no request is pending,
XWM sends `QueryTree(root)`, stores the exact sequence, and flushes without
blocking. The next reactor wakeup reconstructs the x11rb cookie and calls
`reply_unchecked()`. `WouldBlock` preserves the pending request. If more
changes arrive while it is pending, the reply is consumed and exactly one
follow-up query is issued.

The X11 protocol defines QueryTree children in bottom-to-top stacking order.
The emitted event documents this explicitly:

```rust
XwmEvent::OverrideRedirectStackSnapshot {
    generation: XwaylandGeneration,
    epoch: u64,
    bottom_to_top: Vec<X11WindowHandle>,
}
```

The vector contains only current-generation, live, mapped,
override-redirect windows known to XWM. Unknown, missing, stale, destroyed,
or kind-changed XIDs are pruned and counted rather than invalidating the
complete snapshot. Generation and epoch checks occur in both XWM and the
compositor.

Snapshot application changes only the relative order of override-redirect
desktop windows. Managed X11 ordering, XDG ordering, layer-shell ordering,
and non-X11 scene layers remain compositor-owned.

### Transient relationships and commands

`WM_TRANSIENT_FOR` remains available for metadata, parent lookup, application
relationships, diagnostics, and focus-policy checks where relevant. It is
not used as stacking authority for override-redirect windows.

Override-redirect windows are excluded from transient-family reordering,
family raising, automatic parent-above-child correction, and managed-family
promotion. A legitimate menu/submenu chain may therefore remain mapped while
the root-tree snapshot determines its relative order.

`RestackExact`, `Raise`, `RaiseFamily`, `Stack`, and `StackFamily` remain
available for deliberate compositor-owned operations and managed/client
ConfigureRequest handling. A normal override-redirect map, configure, or
observed restack never emits observation-driven `RestackExact`.

### Pointer and keyboard focus

Pointer targeting always uses the actual topmost renderable surface under the
last physical pointer position. It never substitutes the keyboard-focused
managed window for a non-focusable popup.

Auxiliary and override-redirect roles do not become active managed windows.
Popup, menu, dropdown, combo, tooltip, notification, and DND types do not
request keyboard focus. X11 FocusIn/FocusOut events caused by grabs or
ungrabs cannot replace Typhon's active managed-window identity.

The final batch hit-test honors existing pointer constraints and implicit
grabs. Suppressing intermediate crossings does not transfer pointer
ownership. Stable targets do not receive leave/re-enter churn; a changed
target receives leave before enter.

### Lifecycle settlement

Unmap and destroy remove XWM and compositor popup state immediately. A popup
that is associated and then terminally unmapped or destroyed before
`WindowReady` admission is recorded as a pre-admission cancellation, releases
its pending association/buffer state once, and cannot publish a renderable
surface, client-list entry, repaint loop, or pointer enter.

Teardown tracing distinguishes first effective destruction from redundant
idempotent cleanup and records the exact terminal reason: X11 unmap, X11
destroy, Wayland association removal, attachment replacement, generation
teardown, compositor client teardown, pre-admission cancellation, or
redundant cleanup.

### Trace retention and metrics

The per-call `x11_resize_command_order command_order=flush` record is removed.
Flush activity may be represented by an aggregate counter only. Other
high-frequency records are classified so lifecycle records retain bounded
recent coverage instead of being permanently evicted by startup noise.

Relevant lifecycle records include generation, XID, Wayland surface ID,
association serial, X11 lifecycle, override-redirect state, window types,
transient parent, client leader, geometry, map serial, root-stack epoch and
index, render-stack index, old and new pointer targets, keyboard-focused XID,
and mutation source. Trace retention is bounded by category/ring capacity.

Metrics cover scene batches and mutations, deferred and committed pointer
refreshes, suppressed intermediate targets, root-stack query issuance,
coalescing, replies, applied/stale snapshots, pruned entries, prevented
restack writebacks, render-stack and client-list coalescing, pre-admission
cancellations, and redundant popup cleanup.

## Error Handling

QueryTree transport failures use the existing XWM error path. A temporarily
unavailable reply is not an error and remains in flight. A malformed or stale
reply is discarded and counted without mutating compositor state. A failed
scene commit leaves logical state intact, releases the batch guard, and does
not block the native cycle waiting for X11.

Terminal lifecycle events and required protocol replies are never coalesced
away. Client-list synchronization, render-stack normalization, pointer
refresh, identical metadata-driven rebuilds, and repaint requests are
coalesced only within the active batch.

## Testing Strategy

Tests are written before each implementation slice and must demonstrate the
failure before the fix. Coverage includes:

- root-stack snapshots `[A, B]` and `[B, A]` preserving managed order;
- no ConfigureNotify-to-RestackExact feedback;
- one in-flight QueryTree request with one follow-up after concurrent dirties;
- stale epoch and generation rejection;
- same-batch temporary popup suppression and map/unmap cancellation;
- leave-before-enter sibling hover transitions;
- parent/submenu coexistence and unrelated popup families;
- focus preservation with actual popup pointer targeting;
- attachment replacement without an intermediate retired-surface enter;
- window kind transitions in both directions;
- lifecycle trace retention through flush storms;
- deterministic 1000-cycle popup storms with bounded work;
- real XWM event drain to QueryTree reply to scene commit where the fixture
  supports it.

Performance assertions target O(N) state ingestion per batch, one final
pointer refresh, one final render-stack reorder, at most one active QueryTree,
and zero observation-driven RestackExact commands.

Full validation uses the repository's locked Cargo checks/tests/build and
source-layout checks. Hardware Steam qualification is reported only when
actually run; no hardware result is inferred from deterministic tests.
