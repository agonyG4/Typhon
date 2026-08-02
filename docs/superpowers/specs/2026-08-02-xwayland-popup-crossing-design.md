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
cycle begins the batch before association dispatch and commits it before
collecting compositor backend commands:

1. XWayland association events;
2. buffer-level and buffer-ready events;
3. XWM window events;
4. scene-batch commit;
5. compositor backend-command collection;
6. terminal-window command normalization;
7. command execution and XWM flush.

The current native dispatch path is split so batch commit can produce
coalesced client-list synchronization, focus repair, or deliberate managed
window commands before backend commands are collected. No command produced by
commit is delayed until a later native cycle.

Logical state changes remain immediate. While a batch is active, scene side
effects set dirty state instead of refreshing pointer focus, reordering the
render stack, synchronizing client lists, or requesting duplicate repaint
work. Direct test/API calls remain immediate when no batch is active.

The guard has explicit, non-nestable ownership. `begin_xwayland_scene_batch()`
returns an exact batch token and epoch. The caller must pass that token to
`commit_xwayland_scene_batch(token)`. A mismatched token, double commit, or
nested begin is rejected and fails closed. Drop or failure cleanup only clears
active-batch bookkeeping; it performs no pointer, protocol, stack, or repaint
side effects. Logical mutations and dirty work remain available for a later
successful commit. Commit does not wait for X replies.

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
8. Schedule one repaint signal while preserving all damage and presentation
   work already recorded by the existing owners.

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
XWM sends `QueryTree(root)`, stores the exact sequence and request epoch, and
flushes without blocking. The next reactor wakeup reconstructs the x11rb
cookie and calls `reply_unchecked()`. `WouldBlock` preserves the pending
request.

The reply is authoritative only when its request epoch equals the current
dirty epoch. If a newer stack mutation occurred while the request was in
flight, XWM consumes the reply, counts it as superseded, emits no snapshot,
and issues exactly one follow-up query. This prevents an older root order from
becoming an intermediate compositor scene. A reply that omits a currently
live, mapped override-redirect record is treated as incomplete: it is not
used to remove or reorder that window, the current logical placement is
retained, and a fresh reconciliation is requested. Unknown XIDs may still be
pruned from an otherwise complete reply.

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
override-redirect windows known to XWM. Unknown XIDs present in the reply and
records that became stale, destroyed, or kind-changed are pruned and counted.
A currently live mapped record missing from the reply makes the reply
incomplete instead of producing a partial snapshot; its current logical
placement is retained and a fresh query is issued. Generation and epoch
checks occur in both XWM and the compositor.

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

Batch commit uses one atomic pointer-crossing primitive rather than composing
`clear_pointer_focus()`, `ensure_pointer_focus()`, and
`send_pointer_enter_if_needed()`. It captures the old target, resolves the
final target, queues all valid leave events, queues all valid enter events,
updates pointer bookkeeping, and sends frames only after the complete
crossing is queued. If the old surface was destroyed, its bookkeeping is
cleared without sending an event to the dead resource. Same-client leave and
enter are grouped into one pointer frame where protocol/resource support
allows it; different clients receive one final frame each. The primitive runs
only when pointer target, render-stack order, placement, association, or
relevant input state is dirty. Metadata-only mutations that cannot affect
hit-testing do not refresh the pointer.

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
unavailable reply is not an error and remains in flight. A malformed, stale,
superseded, or incomplete reply is consumed and counted without mutating
compositor state. A failed scene commit clears only active-batch bookkeeping;
logical state, existing damage journals, and dirty work remain available for a
later commit. It does not block the native cycle waiting for X11.

The scene batch does not create a second damage accumulator. Surface damage,
publication, and presentation state continue to be recorded immediately by
their existing journals and owners. The batch tracks only whether repaint
scheduling is needed and emits one scheduling signal at commit.

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
- superseded and incomplete QueryTree replies retaining logical order;
- same-batch temporary popup suppression and map/unmap cancellation;
- leave-before-enter sibling hover transitions;
- parent/submenu coexistence and unrelated popup families;
- focus preservation with actual popup pointer targeting;
- attachment replacement without an intermediate retired-surface enter;
- window kind transitions in both directions;
- mismatched, double, nested, and dropped/aborted scene-batch tokens;
- atomic leave/enter queueing and pointer-frame grouping;
- lifecycle trace retention through flush storms;
- deterministic 1000-cycle popup storms with bounded work;
- real XWM event drain to QueryTree reply to scene commit where the fixture
  supports it.

Performance assertions target O(N) state ingestion per batch, one final
pointer refresh only when pointer-affecting dirty state exists, one final
render-stack reorder, at most one active QueryTree, and zero
observation-driven RestackExact commands.

Full validation uses the repository's locked Cargo checks/tests/build and
source-layout checks. Hardware Steam qualification is reported only when
actually run; no hardware result is inferred from deterministic tests.
