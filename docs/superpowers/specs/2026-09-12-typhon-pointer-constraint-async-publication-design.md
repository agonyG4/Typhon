# Typhon pointer-constraint replacement and async publication design

## Problem statement

Typhon has two lifetime failures above the renderer/KMS boundary:

1. `zwp_pointer_constraints_v1::AlreadyConstrained` is reported when a
   committed pointer-constraint protocol object has been destroyed but its
   ordered surface retirement has not yet been applied. The live request slot
   and the effective committed state are represented by the same map entry and
   duplicate detection treats the retirement tombstone as a live request.
2. An explicit-sync acquire can become ready after its owning Wayland client is
   wire-dead but before normal disconnected-client teardown has removed the
   compositor state. The ready path currently has commit/surface/acquire
   identity but no proof that the owner is still eligible for visual
   publication. The same gap exists in the SurfaceTree dependency path.

The current protocol trace classifies the pointer-constraints manager as
`Other`, so the first workstream also makes that diagnosis deterministic and
observable.

## Proven producer of the protocol error

The compositor has two `AlreadyConstrained` producers: the `lock_pointer` and
`confine_pointer` request arms in `protocols/advanced.rs`. Both call
`register_pointer_constraint`, whose duplicate predicate currently includes
`committed`, `surface_constraint_pending`, and
`lifecycle_removal_pending`. The generated pointer-constraints protocol maps
`AlreadyConstrained` to value 1. The resource is a
`ZwpPointerConstraintsV1`, but `ProtocolErrorInterface::for_resource` does not
classify that type and therefore records `Other`.

The implementation will add a focused `for_resource` test and a
`PointerConstraints` trace category. The existing synchronized-subsurface
duplicate test remains as request-level coverage.

## Pointer-constraint state model

`CapturedPointerConstraintSurfaceState` becomes a bounded normalized
transition with two independent optional slots:

```text
retire: Option<constraint_id>
install_or_update: Option<CapturedPointerConstraintCommit>
```

The retire slot identifies only the previously effective constraint. The
install slot contains only the replacement identity and its own region/hint;
it never inherits fields from the retired identity.

Merge rules are explicit:

- `Install(A)` followed by `Cancel(A)` clears the install slot, so A never
  becomes effective.
- `Remove(A)` followed by `Install(B)` retains both slots.
- `Remove(A)`, `Install(B)`, `Cancel(B)` retains only retirement of A.
- `Remove(A)`, `Install(B)`, `Cancel(B)`, `Install(C)` retains retirement of
  A and installs C.
- Updates for the same install identity use latest-wins region and hint
  semantics.
- A lifecycle transition for one identity never merges region or hint state
  from another identity.

Duplicate request occupancy is based on a live protocol/request owner that is
already committed or pending for the same surface. A destroyed object is no
longer a request occupant even when its committed effective state remains in
the map for ordered retirement and generation-qualified backend completion.
The existing `PointerConstraintBackendId { constraint_id, generation }` remains
the only backend identity.

Applying a replacement retires the old effective entry first, then applies the
new entry. An active old backend may therefore remain in the deactivation
handshake while the replacement is committed but inactive; its completion
resumes only the current generation. Synchronized-subsurface ordering is
unchanged.

## Async publication eligibility

Every queued direct explicit-sync commit captures:

- `surface_id`;
- the owning `ClientId`;
- the existing `surface_presentation_generations[surface_id]` value;
- the existing commit/acquire identity.

Every queued SurfaceTree acquire dependency captures the same surface owner
and presentation generation. A pending SurfaceTree transaction also captures
its root owner/generation so a transaction with no unsignaled dependency
cannot publish after its root has become invalid.

The state owns a `terminal_client_ids` set. It is inserted before
`post_protocol_error` posts a fatal error, including the deferred fatal-error
path, and removed by terminal client teardown. This closes the interval in
which `wayland-server` has already killed the wire but Typhon has not yet
drained `disconnected_clients`.

The async path performs a constant-time eligibility proof using the captured
surface ID, owner ID, current surface ownership map/resource, current
presentation generation, terminal marker, and existing publication sequence
ordering. The ordinary immediate-publication default remains unchanged. The
proof is applied to both direct explicit-sync readiness and SurfaceTree
dependency/transaction publication.

Eligibility failures are explicit outcomes (`SurfaceGone`, `OwnerGone`,
`TerminalClient`, or `StaleSurfaceGeneration`) and are recorded in the
surface-pipeline trace. They do not queue scene work, publish a buffer, emit
frame callbacks, or present feedback.

## Bookkeeping and ordering on discard

Acquire readiness still advances synchronization bookkeeping exactly once.
When visual eligibility fails:

- the external acquire watcher receives exactly one cancellation/settlement;
- the pending buffer releases its ownership and release point exactly once;
- resize captures are released;
- frame callbacks are completed without being sent to a dead owner;
- presentation feedback is discarded;
- SurfaceTree transactions are released in queue order and retain enough
  bookkeeping to avoid reordering later transactions.

Repeated or late readiness is idempotent through the existing pending-acquire
state transitions and identity matching.

## Verification strategy

Tests will be added before production changes for:

- pointer replacement and transition merge algebra;
- live synchronized duplicate rejection;
- pointer protocol-error interface classification;
- terminal direct explicit-sync readiness;
- terminal/dead/stale SurfaceTree dependency and transaction handling;
- teardown, repeated completion, and ownership settlement.

Focused tests will run during implementation, followed by formatting, locked
check/clippy, the focused compositor groups, and the full locked test suite.
