# Typhon app launch handling design

## Problem

Eclipse app launches reach Typhon through the shell-control bridge, but the
compositor replies with error code 5, `compositor is shutting down`. The
current runtime source locates that reply in the helper that settles queued
launch requests during shutdown. The launch loop selects that helper while
`shutdown.is_running()` is true, which is the normal operating state.

## Design

In `NativeRuntime::run_cycle`, drain and start queued process launches while
the shutdown lifecycle is running. Only call
`fail_pending_process_launches_for_shutdown` after shutdown has begun. Keep
the current batching, launch environment, cursor configuration, and shutdown
error response unchanged.

This is a one-condition correction in Typhon. Eclipse already selects the
Typhon launch bridge and delivers the requests, so changing Eclipse would
leave the underlying compositor behavior broken for other clients too.

## Alternatives considered

- **Recommended: correct the shutdown condition.** Smallest change, preserves
  the existing queue and shutdown settlement behavior.
- **Refactor launch queue servicing.** Broader change with no demonstrated
  need for this failure.
- **Change Eclipse to use another launch route.** Masks the compositor bug
  and does not fix other clients using Typhon's launch protocol.

## Error handling and validation

During shutdown, queued requests must continue to receive error code 5 with
the existing shutdown message. During normal operation, queued requests must
reach the existing process-launch drain. After rebuilding Typhon into the
approved Aether build directory and running the updated compositor, verify
app launches from both Dock and Spotlight. The workspace instructions
prohibit adding or running tests unless requested, so this change will be
validated with the existing runtime launch flow instead.
