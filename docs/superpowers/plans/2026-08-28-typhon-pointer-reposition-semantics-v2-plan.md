# Typhon Pointer Reposition Semantics v2 Plan

## Objective

Close the remaining pointer reposition correctness gaps: confinement bypass,
same-client focus-crossing delivery, and modern `wl_pointer.warp` semantics,
while preserving typed constraint ownership and prior lock/serial regressions.

## Tasks

1. Add deterministic RED coverage for an out-of-region client warp and a
   same-client cross-surface warp that can expose leave-without-enter.
2. Add native backend RED coverage for locked, confined, and unconstrained
   `WarpPointer` requests.
3. Implement final-position resolution before compositor mutation and backend
   defense through `NativePointerConstraintState`.
4. Replace the warp-only legacy delivery helper with a narrow reposition path:
   preserve implicit grabs, use leave/enter for focus changes, and retain old
   motion delivery for pre-v11 resources.
5. Upgrade only the direct core Wayland bindings needed for `wl_pointer.warp`,
   add v11 event-log tests, and raise the advertised seat version only after
   the semantics are complete.
6. Integrate v11 lock-restore delivery without changing hint/anchor selection,
   backend acknowledgements, pending reveal ownership, or stale-generation
   behavior.
7. Run targeted RED-to-GREEN tests, full repository gates, two explicit review
   passes, and commit the independently reviewable changes.

## Verification requirements

Preserve active-lock, current-enter-serial, stale-enter, cursor-hint, no-hint,
pending-reveal, confinement, implicit-grab, and generation tests. Add explicit
assertions for compositor/native final-position agreement, focus event order,
per-resource version behavior, frame isolation, and absence of synthetic
relative motion.

## Scope guard

Do not enlarge generic serial history, defer client warps, disable pointer
warp, change relative deltas, implement surface-movement notifications, or
touch unrelated render/input architecture.
