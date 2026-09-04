# Typhon Pointer Constraint Surface Transaction v1.1 — Closure Report

## Result

The v1.1 closure preserves the accepted causal transaction architecture:

```text
protocol requests
    -> pending surface pointer state
    -> exact wl_surface.commit capture
    -> CachedSubsurfaceCommit
    -> surface publication
    -> native backend request
    -> NativeInputEpoch-safe settlement
```

The three reviewed defects are closed without changing the transition timing
model, input thread model, scheduler, motion values, timestamps, or native
acceleration behavior.

The closure commits are:

- `66c4477` — deterministic RED regressions and test seams;
- `62bd409` — lifecycle, captured-identity, cancellation, and report closure.
- `4dfb6e5` — locked-release restore ownership and transition provenance.

They build on the accepted prior commits `23a2d3e`, `45e9371`, and `72eaf2e`.

The previous defects were:

- resource death prematurely invalidated current effective routing;
- captured-but-unpublished installs escaped `AlreadyConstrained` ownership;
- region/hint payloads lacked stable constraint identity.

## Lifecycle and ownership closure

`PointerConstraint` now separates these state domains:

- `protocol_resource_alive` controls whether a protocol resource can produce
  future state or receive an event;
- `surface_constraint_pending` keeps protocol-pending and captured-but-
  unpublished installs as requested ownership;
- `committed` identifies the current committed surface constraint;
- `lifecycle_removal_pending` records a current constraint whose destroyed
  protocol resource has staged removal but whose removal commit is not yet
  published;
- `defunct` remains semantic defunctness for compositor-ended one-shot or
  forced teardown, not ordinary client resource destruction;
- `backend_pending` is cleared when activation work is canceled. Separate
  cancellation evidence preserves the existing one-shot compatibility warp
  without treating a canceled request as a live backend activation.

Therefore:

```text
dead protocol resource + current committed constraint
    -> no future protocol event
    -> current routing remains effective
    -> removal commit publishes the topology change
    -> native deactivation follows the existing settlement boundary
```

Registration derives `AlreadyConstrained` from the explicit constraint record,
not from the mutable pending surface-state map. A current constraint retains
the surface slot while removal is pending; ownership is released only when
that removal is published. A never-current install canceled before its first
effective commit is removed and cannot activate.

Protocol resource lifetime, committed surface constraint lifetime, and native effective routing lifetime are separate ownership domains.

## Captured identity closure

`CachedSubsurfaceCommit` carries a constraint-scoped
`CapturedPointerConstraintCommit` containing:

- `constraint_id`;
- lifecycle mutation;
- region mutation;
- cursor-position-hint mutation.

Merging preserves evidence only for the same constraint identity. An
uncommitted `Install(A)` followed by `Remove(A)` becomes an explicit
constraint-tagged cancellation whose region and hint are both `NoChange`.
It cannot leave unowned surface-wide evidence behind. Publication applies
region and hint only through the captured record's identity; there is no
semantic `ids.first()` selection.

Captured pointer-constraint region and cursor-hint state can never be attributed to a different constraint identity.

## Protocol and policy boundary

`set_region` and `set_cursor_position_hint` remain protocol-defined
double-buffered state. Their values are captured by the exact
`wl_surface.commit` that makes them current.

Commit-synchronized constraint install and removal remain Typhon architectural
policy inspired by current KWin behavior. This synchronization is not claimed
as an explicit pointer-constraints protocol statement. Focus changes continue
to reevaluate already-current constraint state only.

Native activation and deactivation continue to be issued only at the existing
`NativeInputEpoch::constraint_settlement_allowed()` boundaries. Late-bound
locked-pointer activation anchors and current-generation validation remain in
place.

## RED regressions

Commit `66c4477` added deterministic RED coverage before the production closure.
The pre-fix failures demonstrated:

- active locked routing disappeared immediately after resource destruction;
- delayed hint and region payloads crossed from A to later B;
- a captured synchronized install was accepted as a second constraint;
- `Install(A) + Remove(A)` preserved A's region and hint instead of canceling
  them.

The real Wayland-resource regression module covers:

- lock requested without a surface commit;
- create and destroy before the first effective commit;
- active locked destroy without commit;
- active confined destroy without commit;
- destroy plus commit and native deactivation;
- no events to destroyed resources;
- cancellation of stale backend activation;
- captured-but-unpublished `AlreadyConstrained` on a synchronized subsurface;
- delayed constraint-scoped hint and region publication.

The reducer suite directly covers canceled install payloads, including the
absence of unowned region and hint state.

## Historical v1.1 verification before region algebra

The focused transaction suite passed with 7 tests, and the adjacent legacy
pointer-constraint suite passed with 38 tests during implementation. Production
library verification also passed:

```text
rtk cargo check --locked
rtk cargo check --locked --lib
rtk cargo clippy --locked --lib -- -D warnings
```

The exact required `rtk cargo test --locked` command built and ran 2,005 tests:
2,004 passed and one unrelated native KMS test failed:
`native::kms::tests::explicit_atomic_flip_adopts_out_fence_and_closes_input_after_success`.

The all-target check and clippy commands are currently blocked by unrelated
working-tree edits in `src/native/adaptive_buffering.rs` and
`src/native_output/runtime/dmabuf_release.rs`. Their tests reference fields,
methods, and types absent from the corresponding production definitions.
Those edits are outside this pointer-constraint closure and were preserved.

Consequently, the following required commands cannot be reported as full
GREEN until that unrelated mismatch is resolved:

```text
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
```

At the final audit, `rtk git diff --check` passes. `rtk cargo fmt --check`
reports formatting drift in the unrelated `src/native/adaptive_buffering.rs`.
The pointer-constraint files remain formatted.

The current host is Linux rather than Windows, so no Windows-specific native
availability claim is made. Full Linux native qualification involving the
actual input backend remains not run here. The causal timing tests were
rerun successfully: 5 native transition-evidence tests, including
deactivation A followed by activation B and wall/thread-CPU association, and
16 input-epoch dispatch tests passed. No timing semantics were changed.

After those focused runs, the shared checkout received the unrelated
`adaptive_buffering.rs` test edits noted above; a subsequent focused Cargo
test invocation was blocked during test compilation by those edits. The
subsequent library check and clippy reruns were also blocked by a missing
`RenderPrediction` field initializer in that file. The pointer-constraint
commits remain unchanged.

## Region-algebra v1 closure

This region-algebra follow-up started from:

```text
cc9fb1c196558bfad8c958c1483951c17cc61fa3
```

The implementation commit is `9973fc2`, followed by the formatting and
warning-closure commit `1f5d5c1`. The ending implementation HEAD before this
report finalization is `1f5d5c1`; the final repository HEAD is the report
commit that contains this record.

The pre-fix defects and evidence were:

- resource death prematurely invalidated current effective routing;
- captured-but-unpublished installs escaped `AlreadyConstrained` ownership;
- region/hint payloads lacked stable constraint identity;
- the effective-region resolver rasterized every surface pixel and repeatedly
  locked the committed input-region state.

The old pointer-constraint effective-region resolver scaled with surface pixel
area and acquired the committed input-region mutex from inside that area scan.
For a `W × H` surface, the old candidate count was `W × H`: 1,783,680 points
for the Sober-shaped 1920×929 case and 33,177,600 points for 7680×4320.
`d665d3a` added the deterministic RED algebra tests before the resolver and
snapshot API existed; they failed at compile time because the expected
resolver and snapshot surface were absent. The subsequent implementation
replaced that path without changing the accepted transaction architecture.

The new resolver scales with region geometry complexity and snapshots
committed input-region state once per resolution. `SurfaceRectRegion` uses
positive-area, non-overlapping half-open rectangles with i64 edges. It applies
ordered `wl_region.add` union and subtraction, intersects the committed
constraint region with the committed input region, clips to
`[0,width) × [0,height)`, and translates deterministically top-to-bottom and
left-to-right into the existing `OutputRegion`/`OutputRect` types. Defaults
are full-surface for constraints and effectively infinite-for-input before
surface clipping; custom input regions start empty. The production geometry
path has no area loop, row loop, or input-region mutex access.

`SurfaceData::committed_input_region_snapshot` clones committed state under
one lock and releases it before rectangle resolution. A poisoned lock remains
fail-open as the default/infinite input region, matching existing hit-testing
behavior. The test-only snapshot lock counter reports exactly one lock for a
resolution snapshot.

The implementation deliberately does not reuse `SurfaceOpaqueRegion` or its
different defaults. Pixman was rejected: this is a small integer,
surface-local set problem, and a pure Rust rectangle algebra avoids an
additional dependency and conversion/lifetime boundary while preserving the
required exact ordered operations.

The algorithm follows the established Wayland-family region model represented
by [wlroots committed input-region state](https://github.com/swaywm/wlroots/blob/master/include/wlr/types/wlr_surface.h),
[wlroots pointer-constraint region handling](https://github.com/swaywm/wlroots/blob/master/rootston/cursor.c),
and [Weston compositor region state](https://github.com/krh/weston/blob/master/src/compositor.c):
regions are maintained as rectangle geometry, clipped in surface-local
coordinates, and consumed at the constraint transition boundary.

## Region RED and GREEN coverage

The cfg(test)-only legacy raster oracle remains limited to small surfaces. It
is used for membership and closest-point probes, including fractional probes
and equal-distance ordering cases. The algebra suite covers defaults/custom
constraint and input regions, union, ordered subtract, overlap and disjoint
sets, clipping at negative and extreme i32 coordinates, holes, islands,
empty results, deterministic rectangle ordering, and constant operation work
at 40×30, 1920×929, and 7680×4320. The original v1 focused region suite was
GREEN with 7 tests; the v1.1 closure below adds the canonical-history tests.

Real Wayland-resource coverage is also GREEN and retains the v1.1 regressions:

- lock and confine requests without a commit;
- active locked and confined destruction without a commit;
- destroy plus commit and native deactivation queueing;
- create and destroy before the first effective commit;
- no events to destroyed resources and cancellation of stale backend
  activation;
- captured-but-unpublished `AlreadyConstrained`, including synchronized
  subsurface ownership;
- delayed constraint-scoped region and cursor-hint identity;
- exact `wl_surface.commit` capture for constraint and input-region state;
- synchronized/delayed commit ownership and the Sober-shaped 1920×929
  default region.

At the v1 closure revision, the real confined-pointer subset passed 5 tests,
the pointer-constraint transaction subset passed 7 tests, and the full test
command passed 3,290 tests with 5 ignored. Existing active-confined-region updates use the same
algebraic resolver, while activation carries optional resolver timing through
the existing native request/action/evidence path.

## Timing and non-claims

`NativePointerTransitionEvidence` now carries
`constraint_region_resolution_duration_ns` and
`constraint_region_resolution_thread_cpu_ns`. The fields are selected with the
same transition evidence as activation and are included in the existing
timing summary. Resolver clocks are gated by the existing pointer timing trace
switch; tracing disabled adds no resolver clocks, formatting, allocations, or
operation counters. The routing-transition suite remains GREEN with 6 tests,
and the pointer timing suite remains GREEN with 17 tests, including
deactivation A followed by activation B and wall/thread-CPU association.

The required repository verification on the current checkout is:

```text
rtk cargo fmt --check                                  BLOCKED: pre-existing
                                                         import-order drift in
                                                         src/native_output/tests/input.rs
rtk cargo check --locked --all-targets                 GREEN
rtk cargo clippy --locked --all-targets -- -D warnings GREEN
rtk cargo test --locked                                GREEN: 3290 passed,
                                                         5 ignored
rtk git diff --check                                   GREEN
```

The formatting blocker is outside the pointer-constraint region change and
was preserved with the concurrent native-pacing checkout. Linux-only
dependency/target blockers (`wayland-server`, `libudev`, DRM, pkg-config, or
the Linux target) did not occur for these checks.

Protocol resource lifetime, committed surface constraint lifetime, and native effective routing lifetime are separate ownership domains.

Captured pointer-constraint region and cursor-hint state can never be attributed to a different constraint identity.

This task does not alter pointer motion semantics, Native Input Semantic Epoch semantics, pointer-constraint transaction ownership, or physical-event ordering.

The current verification host is Linux; no Windows build or Windows-specific
availability claim was made. Native Linux manual qualification against the
physical input backend, including the Sober/Roblox scenario, was not run.
This task does not claim the Sober/Roblox camera jump is fixed until native post-change qualification confirms it.

## Region-algebra v1.1 canonical geometry and locked timing closure

This v1.1 follow-up started from the clean v1 closure revision
`8e24c60b03b214e57b61f12abce53009cf390a3e` and preserves the accepted
constraint transaction and native settlement architecture.

### Defect A: fragmentation-dependent behavior

The review defect was that the v1 algebra preserved geometric membership but
could preserve artificial rectangle fragmentation. The minimal reproduction
was:

```text
Add(0, 0, 2, 1)
Add(0, 1, 2, 4)
```

compared with `Add(0, 0, 2, 5)`. The two sets are identical, but the old
fragmented `OutputRegion` made `OutputRect::closest_point()` observe separate
integer-edge clamps at the fractional probe `(0.0, 0.5)`. Deterministic
sorting could only make that wrong decomposition repeatable; it could not
make equivalent geometry behaviorally equivalent.

The resolver now canonicalizes from the geometric set after ordered algebra
and intersection. It collects unique rectangle y-edges, derives merged x
intervals for each vertical band, then merges vertically adjacent bands with
identical intervals before constructing `OutputRegion`. The work and storage
depend on rectangle/edge complexity, not physical surface rows or pixels.

> Pointer-constraint effective geometry is canonicalized from the geometric set, not from wl_region operation history.

Equivalent adjacent-add histories and equivalent hole histories now produce
the same canonical rectangles and `closest_point()` results across integer,
fractional, outside, between-island, and equal-distance probes. Separate
islands remain disconnected and holes remain absent. The cfg(test) raster
oracle continues to prove membership, clipping, and ordered set semantics;
its incidental row decomposition is not treated as canonical output identity.
The default constraint plus default input on a 1920×929 surface remains one
rectangle with the one-rectangle fast path.

### Defect B: locked resolver timing ownership

The locked settlement path previously called the untimed region resolver, so
`LockedActivated` evidence could report unknown resolver timing even though
late-bound activation had just resolved the effective region and anchor. The
internal `ResolvedPointerConstraintBackendRequest` now carries the unchanged
semantic `ActivateLocked { id }` request, its late-bound anchor, and timing
metadata from that exact resolution. The native settlement loop passes this
metadata into the same selected `NativePointerTransitionEvidence` that owns
`LockedActivated(id)`. Confined activation retains its existing action timing;
deactivation remains unknown unless that exact action resolves a region.

The locked regression uses wall `37ns` and thread CPU `19ns`, verifies both
fields on `LockedActivated(A)`, and verifies that a later `ActivateLocked(B)`
cannot attach to a preceding `Deactivate(A)` evidence record. Resolver clocks
remain gated by `TYPHON_POINTER_TIMING_TRACE`; disabled tracing does not add
timing clocks, formatting, timing-only allocations, or operation counters.

> Locked pointer region-resolution timing is attached to the exact LockedActivated transition that consumed that resolution.

### v1.1 verification

Focused suites run through `rtk`:

```text
pointer_constraint_region::tests                 GREEN: 10 passed
pointer_constraint_transaction                   GREEN: 7 passed
confined_pointer                                 GREEN: 5 passed
pointer_lock_warp                                GREEN: 1 passed
native_pointer_constraint_backend                GREEN: 12 passed
routing_transition_tests                          GREEN: 7 passed
pointer_timing::tests                             GREEN: 17 passed
epoch::tests                                      GREEN: 1 passed
cycle_dispatch::tests                             GREEN: 16 passed
relative_and_constraints                          GREEN: 38 passed
```

Required repository commands on the current Linux host:

```text
rtk cargo fmt --check                                  BLOCKED: unrelated
                                                         import-order drift in
                                                         src/native_output/tests/input.rs
rtk cargo check --locked --all-targets                 GREEN
rtk cargo clippy --locked --all-targets -- -D warnings GREEN
rtk cargo test --locked                                GREEN: 3294 passed,
                                                         5 ignored, 40 filtered
                                                         out of 30 suites
rtk git diff --check                                   GREEN
```

An earlier full-test run transiently observed the unrelated
`native::event_loop::tests::xwayland_retiring_registration_tolerates_closed_fd_and_counts_it`
failure at `src/native/event_loop.rs:1486` (`left: 0`, `right: 1`); the final
exact rerun passed and the event-loop source was not modified. No Linux
dependency or target blocker (`wayland-server`, `libudev`, DRM, pkg-config,
or the Linux target) occurred. The current host is Linux, so no Windows build
result is claimed, and manual physical-input/native Sober qualification was
not run.

The production audit confirms no area rasterization, per-row materialization,
or input-region mutex acquisition inside the geometry loop, and the locked
settlement path uses the timed resolver. No NativeInputEpoch, motion,
acceleration, scheduling, readiness, cursor-restoration, or lifecycle
semantics were changed.

This task does not claim the Sober/Roblox camera jump is fixed until native post-change qualification confirms it.

## Region-algebra v1.2 empty locked-region activation closure

This narrow closure starts from `3c6036c43cbfdd943852b3458f2ed781b219cfe6`.
The Region Algebra v1.1 canonicalization and locked resolver timing
architecture remain unchanged.

The defect was that the locked activation caller collapsed two different
resolver outcomes into the anchor helper's unrestricted `None` input:

```text
resolver result with empty effective region
    -> region: None
    -> current pointer position anchor
    -> LockedActivated
```

The locked path now keeps the resolver result intact. An unavailable result
aborts with `region_unresolved`; a successfully evaluated empty effective
region aborts with `region_empty`; only a non-empty `OutputRegion` is passed
to the existing late-bound anchor helper. Successful non-empty locked
resolutions continue to carry their exact wall and thread-CPU timing to the
same `LockedActivated` transition evidence. No anchor API semantics were
changed broadly.

The final invariant is:

```text
current committed constraint
        -> resolve effective geometry
        -> unavailable: no activation
        -> empty: no activation
        -> non-empty: late-bound anchor, backend activation, LockedActivated
```

A successfully evaluated empty effective pointer-constraint region is
distinct from a default/null protocol region and cannot activate a locked
pointer. Default/null constraint and input regions still materialize through
the region algebra as the full mapped surface geometry, so a pointer inside
that surface continues to activate at its current valid position.

### v1.2 RED and GREEN coverage

Before the production change, the new real Wayland-resource regressions failed
because both an explicit region outside a 100x100 surface and a disjoint
constraint/input-region intersection activated (`locked_count == 1`). The
tests use a test-only deterministic backend-settlement seam that drains the
queued request, invokes the real locked resolver, and settles only a resolved
activation; they do not use desktop automation or native timing changes.

After the fix, the real-resource suite covers:

- an empty locked constraint region that produces no lock event, active locked
  routing, or active anchor;
- an empty effective intersection between committed constraint and input
  regions;
- the default/null control, which activates successfully at the current
  pointer position;
- the existing v1.1 real-resource lifecycle, ownership, identity, delayed
  publication, cancellation, and synchronized-subsurface regressions.

The existing canonical geometry, resolver timing, transition evidence,
`NativeInputEpoch`, and deactivation-A/activation-B timing regressions remain
covered by their prior focused suites.

The v1.2 focused transaction suite passed with 10 tests. The exact repository
commands below were run against the current shared checkout. The all-target
check, all-target clippy, and full test are blocked by unrelated uncommitted
Xwayland metadata edits; those files were preserved. Formatting is also
blocked by unrelated formatting drift, while `git diff --check` is GREEN.

### v1.2 repository verification

```text
rtk cargo fmt --check                                  BLOCKED: unrelated drift
                                                         in Xwayland/native-pacing
                                                         files, including
                                                         src/native_output/tests/input.rs
rtk cargo check --locked --all-targets                 BLOCKED: 25 unrelated
                                                         Xwayland compile errors
rtk cargo clippy --locked --all-targets -- -D warnings BLOCKED: 18 unrelated
                                                         Xwayland compile errors
rtk cargo test --locked                                BLOCKED: same unrelated
                                                         Xwayland compile mismatch
rtk git diff --check                                   GREEN
```

The current Xwayland blocker is the unrelated uncommitted addition of
`X11MetadataDelta::DecorationHints` and `X11DecorationHints` fields without
the corresponding existing snapshot fields, match arms, and test initializers.
No Linux dependency or target blocker (`wayland-server`, `libudev`, DRM,
pkg-config, or the Linux target) occurred. The current host is Linux, so no
Windows build result is claimed, and manual physical-input/native Sober
qualification was not run.

## Locked pointer release cursor-restore investigation v1

This investigation starts from exact checkout HEAD `5911195` (`style: format
keyboard action routing`) and is implemented by `4dfb6e5` (`fix: close locked
pointer release restore ownership`). It preserves the accepted commit-exact
surface transaction architecture and the existing NativeInputEpoch settlement
boundaries.

### Symptom and root cause

The release path was found to treat the late-bound locked activation anchor as
an implicit cursor-restoration target. When a locked pointer had no committed
`cursor_position_hint`, release could therefore enqueue a native restore to
the activation anchor. The compositor's logical pointer, the native cursor,
the protocol resource, and the effective surface constraint were not all
owned by the same transition, which made the resulting cursor relocation
look like an input transition rather than a committed hint application.

The corrected release decision is:

```text
committed hint for the exact active constraint generation
    -> validate and apply that hint as the explicit restore target
no committed hint
    -> preserve the current logical pointer position; request no restore
invalid or unresolved committed hint
    -> preserve the current logical pointer position; request no restore
```

There is no activation-anchor fallback in the release path. A release restore
is queued only when `locked_pointer_release_restore_decision()` accepts the
committed hint for the exact current constraint. The native deactivation path
also requires an explicit restore position and carries its
`PointerWarpOrigin`; it cannot manufacture a target from an activation anchor.

The protocol's double-buffered hint remains authoritative only after the exact
`wl_surface.commit` that captured it. Commit-synchronized installation and
removal remain Typhon architectural policy inspired by current KWin behavior,
not a claim that the pointer-constraints protocol explicitly requires delayed
lifecycle activation or removal.

### State and mutation map

The closure keeps these ownership domains distinct:

- `PointerConstraint::protocol_resource_alive` controls protocol resource
  usability and event delivery;
- `surface_constraint_pending`, `committed`, and
  `lifecycle_removal_pending` control requested/current surface topology;
- `defunct` remains semantic one-shot/compositor-ended state rather than an
  eager alias for ordinary client resource destruction;
- `NativeInputEpoch` and the native backend action control when routing and
  native activation/deactivation become effective;
- the logical compositor pointer and the native cursor are observed and
  traced separately.

Protocol destruction clears the resource references and cancels backend-pending
activation bookkeeping immediately, so a dead resource receives no future
event and cannot produce ghost native activation. If the constraint was
already committed, destruction instead stages removal while leaving the
current constraint slot and effective routing intact. A later exact surface
commit publishes the removal; native deactivation then follows the existing
settlement boundary. A new constraint cannot overlap that pending removal.

Protocol resource lifetime, committed surface constraint lifetime, and native effective routing lifetime are separate ownership domains.

The relevant absolute-position mutation sites were traced as follows:

- `src/compositor/state/hit_testing.rs::locked_pointer_release_restore_decision`
  is the only locked-release restore decision. It updates logical compositor
  position only for a valid committed hint and otherwise preserves it.
- `src/compositor/state/pointer_constraints.rs::apply_pointer_warp` is the
  compositor-originated pointer-warp queue. One-shot compatibility and
  protocol warps carry explicit origins; locked-release hint warps carry
  `LockedPointerCursorHint`.
- `src/native_output/runtime/frame.rs::deactivate` forwards only an explicit
  restore position and origin; it does not use `activation_anchor` as a
  fallback.
- `src/native_output/input/routing.rs` records the request/action boundary and
  never synthesizes relative motion for a compositor warp.
- native cursor processing remains in the existing pointer input state and
  cursor-restore paths; no new input thread, scheduler, timer, readiness probe,
  motion filter, clamp, or acceleration path was introduced. No Rust production
  `XWarpPointer`/`warp_pointer` path was found in the reviewed pointer-input
  surface.

Locked-pointer activation anchors and cursor-restoration targets are separate ownership domains.

### Transition evidence and origins

`PointerWarpOrigin` now identifies the cause of each compositor/native warp:
`LockedPointerCursorHint`, `OneshotCompatibility`, `PointerWarpProtocol`, or
`ConfinedRegionCorrection`. The native transition evidence carries a
`NativePointerTransitionContext` with the exact constraint id and generation,
before/after active ids and generations, logical and native cursor positions,
activation anchor, committed and pending hint presence/value, restore
decision, requested/applied warp origin and target, cursor visibility, and
focus surface. Unavailable values remain `unknown`; they are not zero-filled.

The transition context is attached to the same evidence record that owns the
native routing transition and remains gated by `TYPHON_POINTER_TIMING_TRACE`.
The existing causal timing design, including wall/thread-CPU association and
the deactivation-A/activation-B separation, is unchanged.

A client-provided cursor position hint may influence release restoration only when it belongs to the exact committed active constraint generation.

Compositor-induced cursor restoration must never generate synthetic relative pointer motion.

### Mature-compositor comparison

The protocol contract is consistent with the [pointer-constraints unstable v1
XML](https://cgit.freedesktop.org/wayland/wayland-protocols/tree/unstable/pointer-constraints/pointer-constraints-unstable-v1.xml):
the hint is double-buffered by `wl_surface.commit`, a lock may be inactive,
relative motion remains the locked-pointer channel, and release may use a
committed hint without requiring a relative-motion event. The protocol does
not prescribe Typhon's delayed lifecycle publication policy.

KWin's current pointer-input implementation reads the committed
`cursorPositionHint` when invalidating an active lock and warps only when that
hint is valid; without a hint it does not substitute the activation anchor.
Its resource-destruction path preserves the hint before the resource is gone.
See [KWin pointer input](https://sources.debian.org/src/kwin/4:6.3.6-1/src/pointer_input.cpp)
and the related [KWin cursor restoration commit](https://git.yourcmc.ru/vitalif/kwin/commit/054ccc389899acf1f15001105e8b03a4ffb203d0).
The [wlroots pointer-constraints example](https://github.com/swaywm/wlroots/blob/master/examples/pointer-constraints.c)
and [Sway cursor handling](https://github.com/swaywm/sway/blob/master/sway/input/cursor.c)
likewise keep constraint and cursor state in compositor-owned state rather
than treating an activation anchor as a release hint. [Weston's input path](https://code.tokarch.uk/mainnika/weston/src/commit/5580cb13c900a3af595f305daa390b5314fca963/libweston/input.c)
also keeps committed constraint state and cursor motion policy distinct.

These implementations are comparison evidence for ownership and hint
semantics, not code copied into Typhon.

### Constraint-scoped captured evidence

`CapturedPointerConstraintCommit` stores the exact `constraint_id` beside its
lifecycle mutation, region, and cursor hint. Cached synchronized commits and
the transition snapshot use that identity directly; no surface iteration or
`ids.first()` selects an owner. The lifecycle reducer also treats
`Install(A) + Remove(A)` before first effective publication as cancellation,
clearing region and hint mutations rather than leaving surface-wide evidence
that a later B could consume.

`AlreadyConstrained` is derived from explicit constraint lifecycle ownership,
not from the mutable pending surface-state map. An install remains requested
through protocol-pending, captured, cached, synchronized/explicit-sync delayed,
and committed states. A current A whose protocol resource is destroyed still
owns the slot while removal is pending; `AlreadyConstrained` stops counting A
only after the removal commit is published and the lifecycle record is gone.
This also preserves compositor-ended one-shot defunct semantics separately.

Captured pointer-constraint region and cursor-hint state can never be attributed to a different constraint identity.

### RED regressions and GREEN results

The real Wayland-resource regressions cover the required lifecycle and delayed
publication cases in
`src/compositor/tests/input_output/pointer_constraint_transaction.rs`:

- lock request without commit and create-plus-destroy before the first
  effective commit;
- active locked and active confined destruction without a surface commit,
  including continued effective routing, locked relative routing/absolute
  locking, confined pointer motion, and no premature native deactivation;
- destroy plus commit, with removal becoming current only at publication;
- destroyed-resource event counters and stale backend activation cancellation;
- captured-but-unpublished `AlreadyConstrained` for the same surface/seat;
- delayed hint and delayed region identity, including publication before B's
  install is current;
- synchronized/cached commit ownership.

The reducer regression directly checks `Install(A)`, `Region(R_A)`,
`Hint(H_A)`, then `Remove(A)`: the collapsed state contains neither an
unowned region nor an unowned hint, not merely `lifecycle == NoChange`.
Equivalent confinement coverage is included. The pointer cursor and relative
constraint suites additionally assert that no-hint unlock preserves logical
position without a generic warp, while an exact committed hint carries the
`LockedPointerCursorHint` origin.

The pre-change RED behavior was deterministic: no-hint unlock/deactivation
restored the activation anchor, and the compositor release request carried a
generic restore target despite the absence of a committed hint. The focused
post-change suites are GREEN: pointer-constraint transaction `10 passed`,
pointer cursor `26 passed`, relative-and-constraints `38 passed`, and the
library-wide run `2058 passed, 2 ignored`. The full library check and clippy
also pass with `-D warnings`.

### Repository verification and remaining qualification

On this checkout's Linux host, the required commands produced:

```text
rtk cargo fmt --check                                  BLOCKED: unrelated
                                                         rustfmt drift in
                                                         presentation/scanout files
rtk cargo check --locked --all-targets                 BLOCKED: unrelated
                                                         presentation transaction
                                                         compile errors
rtk cargo clippy --locked --all-targets -- -D warnings BLOCKED: same unrelated
                                                         presentation/scanout errors
rtk cargo test --locked                                BLOCKED: same unrelated
                                                         presentation transaction
                                                         compile errors
rtk git diff --check                                   GREEN
```

The independently scoped commands `rtk cargo check --locked --lib`,
`rtk cargo clippy --locked --lib -- -D warnings`, and `rtk cargo test
--locked --lib` are GREEN. No `wayland-server`, `libudev`, DRM, pkg-config, or
Linux-target availability blocker occurred. The current host is Linux, so no
Windows verification result is claimed. The unrelated working-tree
presentation/scanout changes were preserved and remain outside the two
closure commits.

Manual native qualification remains required: run Typhon on Linux with
`TYPHON_POINTER_TIMING_TRACE=1`, exercise a persistent locked pointer with and
without a committed hint, destroy before and after the removal commit, test
confined release, and inspect the transition evidence for exact generation,
origin, logical/native cursor, and relative-motion fields. Repeat with the
native Sober/Roblox workload and record camera jump, cursor teleport, focus
changes, and `session.log` evidence. No desktop automation or native Sober
qualification was run here.

The solved ~28 ms pointer-constraint region-resolution stall was not reopened by this task.

## Non-claims

This closure does not claim that the Sober/Roblox pointer jump is fixed. That
claim requires manual native Linux qualification. It also does not introduce
an input thread, scheduler changes, readiness probes, sleeps, timers, motion
drops or clamps, timestamp filtering, application detection, or acceleration
changes.
