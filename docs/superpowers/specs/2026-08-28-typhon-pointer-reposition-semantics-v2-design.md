# Typhon Pointer Reposition Semantics v2

## Goal

Make every explicit compositor-side absolute pointer reposition pass through
the active typed pointer-constraint authority, then deliver it as either a
focus crossing or a compositor-reposition event. Preserve the previously
established active-lock and current-enter-serial invariants.

## Authority and final position

`apply_pointer_warp` validates the request before resolving the final output
position. A backend-confirmed locked binding rejects the request before
logical state, backend work, or client events change. A backend-confirmed
confined binding resolves the requested output point with its existing
`OutputRegion::closest_point` authority. With no active binding, the requested
validated point is used.

The compositor updates `last_pointer_x/y` and queues the same resolved point
to the backend. The native backend repeats the authority check: locked
requests produce no cursor action, confined requests are clamped through the
existing native constraint state, and unconstrained requests are exact. This
protects the invariant from stale work and future callers without creating a
second region source.

## Delivery

Explicit repositions use a narrow reposition delivery path distinct from
physical motion. An implicit pointer grab retains its grab surface and gets
coordinates local to that surface; it cannot cross focus solely because the
global point is elsewhere.

Without a grab, a focus change is delivered by the existing leave/enter
crossing transaction. Leave and enter each retain their existing frame
sequencing, and enter supplies the new local coordinates. No additional
motion or warp is emitted for that crossing.

When focus is unchanged, a pointer resource at version 11 or newer receives
`wl_pointer.warp` with surface-local coordinates followed by its own frame.
Older resources retain the current legacy motion delivery. Events are chosen
per pointer resource, not from the global seat version. A warp is never put
in the same frame as enter or motion, and repositions never create relative
motion.

## Unlock restoration

Lock teardown remains the authorized absolute restoration path. Existing
committed cursor-position hint, activation-anchor fallback, backend
acknowledgement, and `PendingLockedPointerReveal` ownership remain in place.
The restored point is delivered through the same reposition path after the
focus/constraint transition, with no relative motion. Legacy resources keep
their existing behavior; v11 resources receive `wl_pointer.warp` when focus
is unchanged. If the restore crosses focus, the normal leave/enter crossing
is used instead.

## Core protocol boundary

The direct core binding update is the minimum `wayland-client`/`wayland-server`
release pair exposing the Wayland 1.26 `wl_pointer.warp` event. The audited
changes from the currently advertised seat version 8 through version 11 are:

* pointer version 9: `axis_relative_direction` exists in the current bindings,
  but Typhon's normalized axis frame does not carry physical direction
  metadata, so this milestone leaves that optional metadata event unsynthesized;
* keyboard version 10: the `repeated` key-state enum already exists and is
  unaffected;
* pointer version 11: `wl_pointer.warp` is implemented for explicit
  compositor repositions;
* seat, keyboard, and touch have no additional version-11 event/request;
* Typhon continues advertising no touch capability, so `get_touch` remains a
  missing-capability error.

The seat is advertised at version 11 only after the per-resource delivery,
frame isolation, and old-version tests are in place.

## Non-goals

This change does not implement surface-movement warp notifications, deferred
post-unlock client warps, touch input, or application-specific behavior.
