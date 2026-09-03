# Typhon Pointer Constraint Surface Transaction v1.1

## Status

Approved for implementation on 2026-09-03 from the supplied v1.1 closure
requirements.

## Goal and boundary

Close three correctness defects in the existing pointer-constraint surface
transaction path without changing its accepted timing architecture:

```text
protocol requests
    -> pending surface pointer state
    -> exact wl_surface.commit capture
    -> CachedSubsurfaceCommit
    -> surface publication
    -> native backend request
    -> NativeInputEpoch-safe settlement
```

This work does not add an input thread, scheduler policy, readiness probe,
sleep, timer, motion rewrite, timestamp filter, application detection, or
acceleration change. It does not include Sober-specific fixes.

## Lifecycle model

Protocol resource lifetime, committed surface constraint lifetime, and native
effective routing lifetime are separate ownership domains.

`PointerConstraint` will distinguish:

- protocol resource liveness: whether the locked/confined protocol object can
  receive events or produce requests;
- surface ownership: pending install, captured-but-unpublished install,
  current committed state, or removal pending;
- semantic `defunct` state: compositor-ended one-shot state and forced teardown,
  not ordinary client resource destruction;
- native backend state: active, backend-pending, or settled.

Destroying a client resource immediately makes it unable to receive events and
cancels queued backend activation. If its install is not current, its captured
install is canceled and no ghost activation may occur. If its install is
current, destruction only stages removal; current routing and `AlreadyConstrained`
ownership remain until that removal commit is published. Publication then
queues deactivation at the existing native settlement boundary. A new
constraint may reuse the slot only after removal publication, while backend
deactivation may still settle independently. Compositor-driven one-shot
defunctness and forced teardown retain their existing policies.

## Captured ownership model

`CachedSubsurfaceCommit` will carry an identity-bearing pointer record:

```text
CapturedPointerConstraintCommit {
    constraint_id,
    lifecycle,
    region,
    cursor_position_hint,
}
```

The surface-level no-op state has no owner. Any lifecycle, region, or hint
mutation carries the originating constraint identity through pending state,
cached synchronized commits, explicit-sync delayed commits, and publication.
Publication never selects an owner by surface iteration. An install followed
by removal before the install becomes current becomes an explicit tagged
cancellation with no surviving region or hint payload. Region and hint values
can never be attributed to a different constraint identity.

`set_region` and `set_cursor_position_hint` remain protocol-defined
double-buffered state. Commit-synchronized install and client-requested
removal remain Typhon architectural policy inspired by current KWin behavior,
not an explicit protocol statement.

## Test strategy

RED tests will be added before production changes for:

- real locked and confined resource destruction without a surface commit;
- destroy plus commit and native deactivation queueing;
- captured-but-unpublished `AlreadyConstrained`, including a synchronized
  subsurface;
- delayed identity isolation for cursor hints and regions;
- create/destroy before the first effective commit;
- dead-resource event suppression and stale backend activation cancellation;
- pure reducer cancellation that drops install-owned region and hint data.

The existing causal native timing tests, including deactivate-A/activate-B
and wall/thread-CPU association, will be rerun unchanged. No desktop
automation is used as protocol evidence.

## Verification and reporting

Supported host checks will be run through `rtk`. Linux-only failures caused by
missing `wayland-server`, `libudev`, DRM, pkg-config, or a Linux target will be
reported as environment blockers; Windows success will not be presented as
full Linux verification. The English closure report will state the three
previous defects, this lifecycle and captured-ownership model, RED/GREEN
results, blockers, anything not run, and the non-claim that the Sober/Roblox
pointer jump is fixed.
