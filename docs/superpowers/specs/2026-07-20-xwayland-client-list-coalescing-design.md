# Xwayland Client-List Coalescing Design

## Problem

Steam can create and destroy popup windows within one native reactor cycle. Typhon currently turns every XWM event into commands, stores all commands for the cycle, and executes them only after the event queue is drained.

A `WindowReady` event therefore queues a `_NET_CLIENT_LIST` snapshot containing the popup. A later `WindowDestroyed` event queues the final snapshot without it. By execution time the popup has already been removed from the XWM registry. Strict handle validation rejects the earlier snapshot, and `XwaylandService::execute_managed_command` treats that local stale-snapshot error as an XWM connection failure. Managed Xwayland is terminated before the final snapshot or later popup commands can run.

## Decision

At the native runtime batch boundary, retain only the last `XwmCommand::SyncClientLists` command produced during that cycle. Preserve the relative order of every other XWM command and append the final client-list snapshot after them.

Client-list commands are declarative EWMH snapshots, not window operations. Intermediate snapshots have no required externally observable meaning; only the final compositor state for the drained event batch is authoritative. Other commands such as map, configure, focus, stack, and resize retain their existing strict handle validation and ordering.

## Data Flow

1. Drain all currently queued XWM events.
2. Apply each event to compositor state and collect its commands.
3. Collect compositor backend commands for the same cycle.
4. Coalesce client-list synchronization commands, retaining the last snapshot only.
5. Execute non-list commands in their original order.
6. Execute the final client-list snapshot and flush XWM output.

## Error Handling

No XWM error policy changes. A genuine invalid command or X11 connection failure remains fatal to the managed XWM generation. Coalescing removes only stale intermediate EWMH snapshots that Typhon itself superseded within the batch.

## Testing

A pure command-batch regression will model a Steam popup becoming ready and then destroyed in the same cycle. It must prove that:

- the stale list containing the popup is removed;
- the final list without the popup remains;
- unrelated commands retain their order;
- a batch without client-list synchronization is unchanged.

Focused XWM/runtime suites, strict Clippy, the full serial test suite, source-layout validation, and a fresh release build remain required. Live validation will open Steam popup windows and verify that Xwayland stays managed and dead popup XIDs disappear.
