# Typhon Locked Pointer Unlock Settlement Closure

## Goal

Close the Pointer Reposition Semantics v2 unlock lifecycle regression without
changing its active-lock, enter-serial, confinement, focus-crossing, implicit
grab, relative-motion, or per-resource delivery invariants.

## Root cause

`PendingLockedPointerReveal` currently records only the backend identity, the
lock pointer/surface, the fallback position, and the dispatch epoch at which
unlock began. Backend deactivation currently publishes the fallback
reposition and immediately clears the pending record. That treats the native
restore acknowledgement as the complete transaction even though a client can
send its final `wp_pointer_warp_v1` request in a later Wayland dispatch.

Cursor surface, cursor shape, and hidden-cursor requests also clear the pending
record. Cursor visual selection answers what should be rendered, not where the
pointer's unlock position is settled.

The pending oneshot compatibility path reuses the active-lock restore cause,
which suppresses legacy motion delivery even though the oneshot never became
active and historically used the ordinary synthetic motion behavior.

## Design

Keep the existing small `PendingLockedPointerReveal` ownership record and add
two independent settlement facts:

* `backend_restore_settled` plus `backend_settled_dispatch_epoch` records the
  matching native deactivation acknowledgement;
* `client_warp_position` records a valid matching client warp's final resolved
  output position.

Unlock creation initializes both facts as unsettled, keeps the lock-hidden
constraint active, preserves the committed hint/activation-anchor fallback,
and queues the existing native `Deactivate { restore_position }` request.

The matching backend deactivation callback validates the current
`PointerConstraintBackendId`/generation exactly as before, marks the matching
pending record backend-settled at the current dispatch epoch, and does not
publish fallback events or reveal visibility by itself. If a matching client
warp was already recorded, the common settlement helper may release the
lock-hidden state at that point. Otherwise dispatch-epoch fallback grace starts
at the acknowledgement epoch and remains blocked until the existing short
grace expires.

A valid client warp remains a normal `ClientWarp` reposition: validation,
active confinement resolution, compositor state update, native `WarpPointer`,
and existing per-resource event delivery all happen through the current common
path. The pending record then records the resolved final position. Its matcher
requires the same `wl_pointer` resource and a target surface belonging to the
same client as the original lock surface; it does not require the original
surface. A warp before the backend acknowledgement keeps visibility hidden
until the acknowledgement. A warp after the acknowledgement queues visibility
only after its native warp. The native request drain therefore preserves
`Deactivate/restore → WarpPointer(final) → ApplyCursorVisibility`.

If no matching client warp arrives, fallback grace is evaluated only after a
backend acknowledgement. On expiry, the fallback position is published once
with `ActiveLockedPointerRestore`: v11+ resources receive `warp + frame`,
legacy resources receive no synthetic event, and no relative motion is
created. The pending record is then finalized and normal desired visibility is
allowed to decide whether the native cursor is shown.

Cursor requests continue to update normal client cursor-choice and visibility
state while the pending record exists. They never settle or clear pointer
position ownership. `set_cursor(NULL)` remains represented by the ordinary
client-hidden state, so the existing `desired_visible()` calculation naturally
keeps the cursor hidden after settlement.

## Reposition causes

The common reposition delivery path distinguishes:

| Cause | v11+ resources | Legacy resources |
| --- | --- | --- |
| `ClientWarp` | `wl_pointer.warp + frame` | legacy `motion + frame` |
| `ActiveLockedPointerRestore` | `wl_pointer.warp + frame` when fallback is finally published | no synthetic event |
| `PendingOneshotHintWarp` | `wl_pointer.warp + frame` | legacy `motion + frame` |

Focus crossings continue to use leave/enter and frame isolation rather than an
additional reposition event. Active locks continue to suppress ordinary warps
and absolute native mutation, while relative motion remains exact and silent
on the absolute pointer route.

## Testing

Add RED-to-GREEN integration coverage for backend acknowledgement without an
immediate reveal, client warp before and after acknowledgement, cursor request
separation including `set_cursor(NULL)`, post-ack fallback settlement,
same-client cross-surface pending-unlock resolution, wrong-client rejection,
stale backend generations, and the legacy/v11 pending-oneshot delivery
matrix. Preserve and update existing lock-restore tests to acknowledge the
backend before asserting final settlement. Keep native backend ordering and
active-lock defense tests in place.

## Scope

This closure does not change Wayland dependency versions, active-lock warp
rejection, enter-serial authority, confinement, relative motion math, cursor
rendering architecture, XWayland, or unrelated rendering and DMA-BUF work.
Runtime Sober/Roblox behavior remains an interactive follow-up and is not
claimed from deterministic tests alone.
