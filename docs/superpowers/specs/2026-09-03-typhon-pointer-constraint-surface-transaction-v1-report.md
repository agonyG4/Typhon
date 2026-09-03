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

## GREEN results and verification limits

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

## Non-claims

This closure does not claim that the Sober/Roblox pointer jump is fixed. That
claim requires manual native Linux qualification. It also does not introduce
an input thread, scheduler changes, readiness probes, sleeps, timers, motion
drops or clamps, timestamp filtering, application detection, or acceleration
changes.
