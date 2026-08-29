# Typhon Pointer-Lock Warp Invariant Design

## Goal

Prevent an ordinary `wp_pointer_warp_v1.warp_pointer` request from changing
Typhon's authoritative compositor or native absolute cursor position while a
backend-confirmed locked-pointer generation owns that position, while
preserving relative motion and lock teardown restoration.

## Observed data flow

```text
native relative/absolute input
  -> NativeInputState::handle_pointer_motion
  -> locked input emits relative motion but keeps cursor_x/y fixed
  -> compositor send_pointer_motion_sample
  -> active lock suppresses absolute wl_pointer.motion

client wp_pointer_warp_v1
  -> validate resource/focus/serial/coordinates
  -> apply_pointer_warp
     -> currently updates last_pointer_x/y
     -> currently queues normal WarpPointer
     -> only then suppresses wl_pointer.motion for an active lock
  -> native request processor
  -> currently turns WarpPointer into cursor_position unconditionally
```

The compositor's active lock route is established only after the backend
activation acknowledgement. That acknowledgement binds the validated
constraint generation to `active_locked_pointer_routing`; the native backend
simultaneously owns an active `Locked` constraint. Lock teardown separately
selects the committed cursor-position hint or activation-anchor fallback and
is allowed to restore the absolute position.

## Invariant

For an active, backend-confirmed locked-pointer generation:

* ordinary input and client pointer-warp paths do not mutate
  `last_pointer_x`/`last_pointer_y`;
* an ordinary client warp does not enqueue `WarpPointer`;
* an ordinary client warp emits no `wl_pointer.motion` and creates no relative
  motion;
* relative input continues to dispatch its exact relative delta;
* the active lock and its generation remain unchanged;
* only lock teardown may perform the final absolute restoration associated
  with the lock.

The invariant does not disable all absolute movement for every constraint:
confined-pointer clamping remains unchanged, and teardown restoration is an
explicitly authorized transition.

## Implementation

1. Guard `CompositorState::apply_pointer_warp` before its position update and
   backend request enqueue. Use the existing
   `active_locked_pointer_binding` validation so stale, dead, defunct, or
   mismatched generations do not own the pointer. Log an explicit suppressed
   warp reason and return without dispatching motion.
2. Make the native backend's existing `active_locked` state query available to
   production code. In `NativePointerConstraintBackend::handle_request`, make
   normal `WarpPointer` a no-op while the backend constraint is `Locked`; keep
   the existing action when unlocked.
3. Do not add deferred post-unlock warp state. A client warp received during an
   active lock is intentionally ignored for this fix.

## Tests

The first test cycle adds a Wayland integration regression for:

```text
lock at A -> valid client warp to B -> backend/compositor processing
           -> absolute position remains A, no wl_pointer.motion
           -> relative delta D remains exactly D
           -> unlock restores existing hint/anchor behavior
```

The same integration test checks persistent lock activation and teardown,
while existing hint and no-hint restoration tests remain unchanged. Focused
native backend tests check that a locked backend returns no `cursor_position`
for `WarpPointer` and an unlocked backend still returns position B. Existing
confined-pointer tests remain the regression coverage for clamping behavior.

## Compatibility and follow-ups

This is the KWin-style immutable-position model for active locks. Hyprland's
corrective/recentering model is not adopted because Typhon already has typed
backend ownership, generation validation, and teardown-owned restoration.

The pointer-warp enter-serial surface/client conformance question is outside
this fix. Typhon's current validation is surface-specific in addition to
client ownership; the specification permits an enter serial from any surface
belonging to the same client. It will be independently tested and fixed, or
recorded as a follow-up, without being bundled into this change.

Runtime Sober/Roblox qualification is separate from deterministic tests. The
fix must not be described as an application-level resolution unless Sober is
actually run or the user validates it interactively.
