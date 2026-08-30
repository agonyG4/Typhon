# Native Input / Pointer-Constraint Causality Closure Plan

## Scope

Make native pointer-constraint semantics stable for each materialized input
epoch while retaining bounded input storage, motion coalescing, existing
Wayland behavior, and the completed pointer reposition and unlock-settlement
closures.

## Work items

1. Add deterministic RED tests for current-dispatch activation, pre-existing
   activation, mid-batch activation/deactivation, confined transitions, and
   budget continuation.  Assert the semantic generation/mode attached to each
   result, not only the absence of a large delta.
2. Add pointer-debug diagnostics for epoch identity, effective constraint,
   batch counts/timestamps, budget exhaustion, pending request timing, and
   transition application boundaries.
3. Introduce a small private native input epoch coordinator.  Apply requests
   before the read-side dispatch and after the epoch, while leaving protocol
   progression inside the batch intact and deferring only native constraint
   effects.
4. Preserve the 256-event hard drain bound.  If a backend stops at the bound,
   keep the current epoch open and schedule a bounded continuation so a
   libinput internal backlog cannot cross a constraint generation boundary.
5. Run focused, surrounding pointer/input, resource-efficiency, and full
   repository verification.  Review every prior pointer invariant and search
   every native constraint request application site.

## Deterministic evidence

The RED model tests demonstrate that the old ordering applies a queued
transition to later events in the same input snapshot.  The raw-evdev budget
test demonstrates that a 257-event queue is observed as 256 events followed by
one continuation event; the first bounded drain reports exhaustion.

The local `input` crate source confirms that `Libinput::next()` directly calls
`libinput_get_event()`, and its iterator is stopped by Typhon's budget before
`None`.  Therefore a continuation state and an explicit wake are required for
libinput as well as raw evdev.

## Acceptance criteria

- A native input epoch has one immutable effective constraint mode/generation.
- Pre-existing requests settle before the next epoch.
- Requests queued by current Wayland dispatch or mid-batch protocol progress
  settle only after the current epoch.
- The 256-event bound remains and its continuation cannot strand an internal
  input backlog across generations.
- Motion coalescing, timestamp ownership, relative exactness, cursor-only
  fast paths, batch flush coalescing, and no-extra-tick behavior remain intact.
- All existing pointer reposition, confinement, implicit-grab, and unlock
  settlement behavior remains unchanged.
- Sober/Roblox is not declared fixed without an interactive runtime test.
