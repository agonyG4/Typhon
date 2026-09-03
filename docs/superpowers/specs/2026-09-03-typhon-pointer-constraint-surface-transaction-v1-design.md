# Typhon Pointer Constraint Surface Transaction v1

## Status

Approved for implementation on 2026-09-03.

Starting HEAD: `053946abe5b78a5955e719fec4ef5537bf4e3bf9`

This design closes the remaining pointer-transition timing association defect and makes pointer-constraint surface state commit-exact without changing the accepted native input epoch contract.

## Goals and boundaries

The compositor will have four explicit layers:

1. Protocol resource state: live or defunct Wayland resources and their request handlers.
2. Pending surface pointer-constraint state: client requests waiting for the next `wl_surface.commit`.
3. Captured/current surface pointer-constraint state: immutable state captured by a commit and the state currently published by the surface tree.
4. Native effective routing: the lock/confine state installed in the native backend and the active compositor routing record.

The change must preserve one `libinput.dispatch()` per `NativeInputEpoch`, the bounded raw drain and continuation debt, exact relative deltas, locked absolute immutability, existing acceleration behavior, the pre-read gate, targeted nonblocking readiness probe, transition-latency guard, late-bound locked activation anchor, one-shot compatibility warp, and epoch/generation/token revalidation.

This change does not add an input thread, polling loop, timer, sleep, busy wait, mutex ingress queue, scheduler policy, or realtime-priority policy. It does not drop, clamp, reorder, or reclassify physical motion. It does not claim that every scheduler-induced physical-input backlog race is closed.

## Chosen architecture

The implementation will use a small, explicit captured-state value carried by `CachedSubsurfaceCommit`.

Pending state uses delta types with an explicit `NoChange` variant. Region and hint values are represented separately from their absence, so an explicit default/null region and a concrete cursor hint cannot be confused with an omitted request. The lifecycle delta similarly distinguishes no request from an install or removal.

Conceptually, the captured payload contains:

```text
CapturedPointerConstraintSurfaceState {
    lifecycle: CapturedPointerConstraintLifecycle,
    region: CapturedPointerConstraintRegion,
    cursor_position_hint: CapturedPointerConstraintHint,
}
```

The install payload contains the internal constraint identity and the initial requested region. Later `set_region` and hint requests become deltas against the same pending surface state. The payload is immutable after the `wl_surface.commit` that captured it.

`CachedSubsurfaceCommit::merge` will merge this payload with the same commit semantics as the other double-buffered surface fields: a newer explicit value replaces an older value, while `NoChange` preserves the older captured value. Lifecycle combinations are resolved explicitly. An install followed by removal before publication collapses to no effective native installation; removal of an already-current constraint remains a removal. Internal identities prevent a later resource from being mistaken for an earlier resource.

The protocol request path will therefore do the following:

1. Create the Wayland resource and internal identity immediately.
2. Register the resource as a pending install on its surface.
3. Record subsequent region/hint requests as pending surface deltas.
4. At `wl_surface::Request::Commit`, atomically take the pending deltas into the new `CachedSubsurfaceCommit` and reset the pending delta to `NoChange`.
5. Never read mutable pending fields while publishing a previously captured commit.

`PointerConstraint` will retain resource/liveness and native-routing metadata, while current surface state will be updated only from captured commit payloads. Activation eligibility may be reevaluated after current state publication, but focus changes cannot publish pending state.

## Surface-tree publication ordering

For every published tree node, the compositor will apply the node's ordinary captured surface state first, including input region, geometry, placement, mapping, scale, and subsurface position. It will then apply that same node's captured pointer-constraint state. This makes a commit containing region R2 and input region I2 evaluate against R2 ∩ I2, never R2 ∩ I1.

The synchronized-subsurface path will retain each child commit's captured pointer payload for the full cached lifetime. Publication will apply the payload for every node, not only the root. The current code's root-only pending application will be removed.

Pointer-state publication and native routing publication remain separate. The first operation changes the compositor's current surface state. The second only queues backend work and is settled at the existing `NativeInputEpoch::constraint_settlement_allowed()` boundaries.

## Lifecycle and race rules

Client-requested lifecycle changes are commit-synchronized policy in Typhon:

- A lock or confine resource exists immediately, but no native activation or locked/confined event occurs before the matching commit is captured and published.
- `AlreadyConstrained` considers both current effective constraints and pending installs for the same surface, seat, pointer identity, and client relationship.
- Client destruction immediately makes the protocol resource defunct and prevents future events. It stages removal of the effective surface state for the next matching commit.
- A destroy before native activation invalidates the queued activation by liveness and generation/token checks.
- An active client-requested destroy keeps the current native constraint effective until the removal commit. That commit queues deactivation, and the backend operation settles at an allowed native epoch boundary.
- A create followed by destroy before the first commit produces no activation, no locked event, and no ghost constraint.
- Install commit A followed by removal commit B before backend settlement cannot allow stale A to win.
- A hint and removal in one commit use the hint captured in that commit when restore/warp policy selects it; removal does not synthesize relative motion.

Compositor-driven one-shot deactivation remains distinct from client lifecycle mutation and keeps its existing immediate policy. The existing `PendingOneshotHintWarp` compatibility behavior is preserved.

Forced teardown for surface, client, pointer, seat, shutdown, and fatal cleanup bypasses client commit synchronization. It invalidates and cancels pending/current/backend/active routing state, cursor visibility ownership, and reveal state immediately, with no access to dead resources.

## Native transition timing

`process_native_pointer_constraint_backend_requests` currently selects the first routing transition and the first activation timing independently. That permits a deactivation for constraint A to be reported with activation timing for constraint B from the same settlement.

The settlement result will instead carry transition-local evidence:

```text
NativePointerTransitionEvidence {
    transition: NativeInputRoutingTransition,
    action_timing: NativePointerConstraintActionTiming,
}
```

Each backend action will create its own timing record. When the settlement selects a transition, it selects the evidence created by that same action. If that action has no activation phase, its activation timing remains unknown even when a neighboring action in the same settlement was an activation. `cycle_dispatch` will pass the selected evidence to `pointer_timing` as one unit. Trace-disabled execution will not add clock calls, formatting, per-event allocations, or unbounded metadata.

The existing transition identity, pre-read observations, thread CPU measurements, and ring behavior remain intact. Tests will explicitly cover deactivate-A/activate-B, activate-A/deactivate-B, selected non-activation transitions, and same-action wall-clock/thread-CPU association.

## Test strategy

Tests will be written RED before production implementation and made GREEN in focused stages. The coverage will include:

- lock/confine without commit and lock/confine with an empty commit;
- pending region/hint not published by focus;
- delayed commit isolation from future region/hint requests;
- synchronized child capture and publication;
- pure cached-commit merge cases, including no pointer state, R1 then R2, install/removal, and explicit defaults;
- destroy without commit, destroy plus commit, and create/destroy before first commit;
- install/remove before backend settlement and pending-install `AlreadyConstrained`;
- same-commit hint plus removal and region plus input-region ordering;
- same-transaction geometry and placement;
- forced teardown cancellation and dead-resource event suppression;
- more than 256 raw events with continuation debt preserved;
- real protocol resource boundaries for `wl_surface`, `wl_pointer`, `zwp_relative_pointer`, and locked/confined pointer resources;
- all transition-timing association matrix cases.

No synthetic Sober desktop automation will be used as proof of the protocol behavior.

## Expected files and verification

The implementation is expected to touch the compositor surface commit/caching/publication modules, pointer-constraint protocol and state modules, focus handling, native routing settlement, pointer timing, and the existing compositor/native-input tests. The final report will be English Markdown under `docs/superpowers/` and will include the exact starting and ending HEADs, protocol requirements versus Typhon policy, the KWin precedent, inspected sources, changed files, tests run, tests not run, and the remaining physical-input race boundary.

Before completion, run these commands through `rtk` on Windows:

```text
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

Focused tests will be run before the full suite, and each coherent implementation stage will receive its own commit.

## Protocol versus policy statements

`set_region` and `set_cursor_position_hint` are protocol-defined double-buffered state and are now owned by the exact `wl_surface.commit` that captured them.

Commit-synchronized creation/client-requested destruction is Typhon architectural policy chosen for coherent surface generations/KWin parity, not an explicit protocol requirement.

The task does not claim every scheduler-induced physical-input backlog race is closed.
