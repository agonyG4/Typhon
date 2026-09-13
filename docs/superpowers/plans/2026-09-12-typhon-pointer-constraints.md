# Pointer-constraint replacement implementation plan

## Goal

Make request ownership independent from ordered effective surface state while
preserving Typhon's synchronized-subsurface ordering and existing
generation-qualified backend identity.

## Red tests first

1. Add a protocol trace unit test mapping the generated
   `ZwpPointerConstraintsV1` resource to a dedicated pointer-constraints
   interface category.
2. Extend the surface-state merge tests to cover:
   - `Remove(A) + Install(B)` retaining both sides;
   - `Remove(A) + Install(B) + Cancel(B)` retaining only A retirement;
   - `Remove(A) + Install(B) + Cancel(B) + Install(C)` retaining A retirement
     and C installation;
   - replacement regions/hints remaining identity-local.
3. Add protocol integration tests for lock/confine replacement without an
   intermediate surface commit, both mode directions, and replacement
   cancellation. Assert the client remains connected and the backend route
   retires/activates in order.
4. Add a committed-A replacement test with a pending old backend deactivation
   and a late A completion; assert only B's generation can route input.
5. Preserve and run the synchronized-subsurface live duplicate test and the
   existing destroy-without-commit routing tests.

Run the smallest affected tests and confirm the new tests fail against the
single-mutation implementation before changing production code.

## Implementation

1. Add `PointerConstraints` to `ProtocolErrorInterface` and classify the
   generated manager resource in `for_resource`; update exhaustive trace
   matches and deterministic tests.
2. Replace `CapturedPointerConstraintSurfaceState::Mutation`'s one-identity
   representation with a bounded normalized transition containing an optional
   retired ID and optional install/update mutation. Implement and document the
   merge algebra in one place.
3. Update cached subsurface merge, pending hint lookup, transition snapshots,
   and all surface-state consumers to understand both transition slots.
4. Change duplicate registration to consider only live protocol/request
   ownership for the surface. Keep destroyed committed entries until their
   effective retirement/backend handshake completes.
5. Apply replacement transitions in retirement-then-install order, preserve
   region/hint identity, and resume a valid replacement after the retired
   generation's deactivation completion. Keep stale backend ID checks
   unchanged and add assertions only for impossible internal states.
6. Add pointer lifecycle trace messages for object destruction, pending old
   retirement, accepted replacement, applied transition, and backend
   activation/deactivation using the existing pointer trace gates.

## Verification

Run pointer-constraint transaction tests, synchronized-subsurface tests, and
the protocol trace tests. Then run formatting/check/clippy and the full suite
after the async plan is complete.

## Commit

Commit the pointer-only changes separately from the async publication changes
so the state-machine review remains bisectable.
