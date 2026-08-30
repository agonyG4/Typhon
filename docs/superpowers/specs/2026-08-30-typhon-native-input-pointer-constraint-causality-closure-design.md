# Typhon Native Input / Pointer-Constraint Causality Closure

## Status

Follow-up design for the Pointer Reposition Semantics v2 and locked-pointer
unlock-settlement closures.

## Problem

The native runtime can materialize a hardware-input batch and then apply a
client-requested pointer-constraint backend transition before all events in
that batch have been interpreted.  There are two versions of the same error:

1. a Wayland read-side dispatch can queue `ActivateLocked` before the pending
   native input queue is drained;
2. the narrow protocol progression used for native-only key/button input can
   queue a transition while an already-drained batch is still being iterated.

In either case, later hardware samples from one physical input snapshot can
be interpreted under a different native constraint generation than earlier
samples.  This can make a first locked-relative sample appear to contain
motion that happened before activation.  A large delta by itself is not proof
of that cause: it can also be genuine post-activation motion or input-service
latency.

## Invariant

Once a native input epoch begins, its effective pointer-constraint mode and
generation are immutable until the epoch ends.  Compositor protocol state may
continue to progress inside the epoch, but newly queued native pointer-
constraint effects are settled only at an epoch boundary.

This preserves the existing motion coalescing rules.  Consecutive relative
samples in one materialized batch still sum exactly, and absolute samples
still use the latest-position rule.  No magnitude threshold, first-sample
discard, reset, clamp, recenter, or application-specific behavior is used.

## Runtime ordering

Each reactor cycle follows this relevant ordering:

1. settle pointer-constraint requests that were already pending before the
   current Wayland read-side work;
2. dispatch readable Wayland work, allowing new requests to be queued;
3. begin a native input epoch with the currently effective backend mode;
4. drain, coalesce, and process the retained input batch;
5. permit the existing narrow protocol progression, but defer its native
   pointer-constraint effects;
6. end the epoch and settle queued pointer-constraint requests;
7. synchronize cursor state and flush the existing native-input batch.

The native request loop remains FIFO and continues to drain requests queued by
processing a request.  Thus a client warp and a visibility request retain the
effective native ordering `Deactivate`, `WarpPointer`, then visibility.

## Hard drain budget

The 256-event drain budget remains in force.  Both the libinput and raw-evdev
backends report when the budget was reached.  The runtime keeps the same
native input epoch open, schedules an immediate continuation wake, and drains
the next bounded chunk before settling pointer-constraint requests.

This is required because the `input` crate exposes libinput through a direct
iterator over `libinput_get_event()`: stopping the iterator before `None` can
leave already-dispatched events in libinput's internal queue.  A later wake
must therefore be an explicit continuation, not an assumption that the input
fd will become readable again.

## Diagnostics

Under `TYPHON_POINTER_DEBUG=1`, the runtime reports the private epoch id,
effective backend constraint id/generation/mode, raw and coalesced counts,
available oldest/newest hardware timestamps, budget exhaustion, pending
requests around Wayland and protocol progression, and the epoch boundary at
which Activate/Deactivate is applied.  Motion diagnostics include the epoch,
constraint id/generation, and sample timestamp.  The epoch id is not part of
Wayland protocol state.

## Relationship to unlock settlement

Unlock settlement remains a separate transaction.  A native deactivation
acknowledgement is a prerequisite, not final reveal settlement.  Client
pointer warps and fallback settlement retain their existing ownership and
same-client cross-surface validation.  Cursor surface/shape requests continue
to select what the cursor looks like; they do not settle where the pointer is.

## Protocol compatibility

The existing delivery matrix remains unchanged:

| Cause | `wl_pointer` v11+ | legacy `wl_pointer` |
| --- | --- | --- |
| Client warp | `warp` + frame | compatibility motion + frame |
| Active locked restore | `warp` + frame | no synthetic motion |
| Pending oneshot hint warp | `warp` + frame | compatibility motion + frame |

Active locked relative input remains exact and does not mutate absolute
position or emit ordinary pointer motion.  Restore still emits no relative
motion.

## References

- `2026-08-28-typhon-pointer-reposition-semantics-v2-design.md`
- `2026-08-29-typhon-locked-pointer-unlock-settlement-closure-design.md`
- `2026-08-25-typhon-resource-efficiency-v1-design.md`
