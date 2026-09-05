# Typhon Native Session v2 — VT Switch Survival and Libseat Disable Boundary

## Status

Approved design for the current Typhon checkout. This is a narrow session-entry correctness change. The accepted Native Frame Pacing v3.x and Native Session Recovery v1 architectures remain unchanged.

## Problem

Typhon currently marks a libseat session inactive and queues `Disabled`, but returns from the libseat callback before the concurrent KMS worker has closed its submission boundary. That is unsafe for logind, whose `PauseDeviceComplete` follows the disable callback. The input path also propagates a failed `switch_session()` request as a compositor-fatal error, and a synchronous/reentrant `Disable` delivered during `switch_session()` can remain queued until a later reactor wake while the runtime still produces output.

The current legacy cursor suspension path can additionally issue a DRM cursor-clear ioctl after the callback-return boundary, and its `Drop` implementation can repeat that ioctl.

## Backend contract

The common Typhon contract is:

```text
libseat Disable callback
  -> active = false
  -> synchronously quiesce the current KMS worker submit boundary
  -> queue one Disabled event
  -> return
```

For logind, callback return is the pre-revocation boundary before device ownership may be revoked. For seatd, the existing later `seat.disable()` remains the explicit `CLIENT_DISABLE_SEAT` / `SERVER_SEAT_DISABLED` acknowledgement. Typhon does not detect or branch on the backend.

## Invariants

1. When the Disable callback returns, no KMS worker is executing an ioctl inside `submit_gate`, and no worker admission can begin another ioctl.
2. After any runtime-owned `switch_session()` returns, all seat events generated during that call are consumed before further output work.
3. A failed VT request is observable and nonfatal while the current session remains valid; it never emits a successful-request event and never escapes as a fatal `NativeResult`.
4. If a VT request both emits `Disabled` and returns an error, the `Disabled` transition wins and is consumed immediately.
5. The main-thread KMS path needs no extra lock: the callback runs on the runtime thread, so no main-thread ioctl can execute concurrently with it; event consumption still gates subsequent output work.
6. Session suspension retains the existing lifecycle, transaction, pageflip, DMA-BUF, cursor-recovery, and generation-barrier ownership rules.
7. Every active worker-backed session has exactly one current pre-revoke authority, and recovery installs the replacement worker authority before `Active` is restored.

## Chosen architecture

### Narrow worker authority

`KmsCommitWorkerHandle` retains ownership of its join handle. A cloneable `KmsWorkerQuiesceHandle` contains only `Arc<WorkerShared>` and exposes `request_quiesce()`. It cannot join, drain events, submit work, mutate queue ownership, or acknowledge pageflips.

`NativeSeatSession` owns a stable `Rc<RefCell<Option<KmsWorkerQuiesceHandle>>>` hook. The callback snapshots and invokes the installed authority synchronously. Runtime bootstrap installs the initial worker authority, and recovery replaces it when a new worker starts. No raw pointer, unsafe self-reference, global singleton, background thread, or callback re-entry into `NativeRuntime` is used. `None` is a correct no-worker no-op.

The existing `WorkerShared::request_quiesce()` remains the only synchronization boundary. Its `submit_gate` lock waits for an in-progress worker ioctl, sets lifecycle to `Quiescing`, rejects future admission, and wakes the worker. Repeated requests are idempotent and cannot reopen admission.

### Runtime-owned VT requests

`apply_native_input_effect()` only reports `vt_switch_requested` through `NativeInputApplication`. `dispatch_wayland_and_input()` carries the request in `NativeWaylandInputDispatchOutcome`; `NativeRuntime` invokes a narrow request helper, immediately consumes pending seat events using the same authoritative event consumer as normal seat dispatch, and re-checks `session.permits_output()` before any planning, presentation, cursor, or KMS work.

The helper records `native.vt_switch_request` with `outcome=requested` or `outcome=failed` and an errno/detail on failure. Failure without ownership loss logs and returns success to the cycle. A consumed `Disabled` event performs the normal `Active -> Suspending -> Suspended` transition before the helper returns, even when `switch_session()` returned an error.

The event consumer is split into `seat.dispatch()` plus `consume_pending_seat_events()`. The latter is reusable after `switch_session()` and contains the only lifecycle transition logic. The normal reactor path still uses libseat fd wakeups and does not add polling or timers.

### Cursor retirement

Session suspension changes the operation semantics from physical cursor clearing to state retirement. Atomic cursor state is suspended without a new KMS submission. Legacy cursor cleanup is disarmed before the object is dropped, so session loss never performs a cursor-disable ioctl after callback return and `Drop` cannot repeat it. Normal non-session destruction and frame-time cursor operations retain their existing physical behavior.

### Existing recovery remains authoritative

The accepted `G1 -> G2` generation barrier, synchronous recovery modeset, presented-plane rebase, `OutputTransactionDropReason::SessionSuspended`, direct ownership, pageflip quarantine, DMA-BUF release ownership, predictor/O1 policy, and Native Wake Authority are not redesigned. The new hook is updated as part of worker replacement before `Active` is restored.

## Ordering

```text
Disable callback
  -> active=false
  -> pre-revoke quiesce via submit_gate
  -> lifecycle=Quiescing / admission rejected
  -> queue Disabled once
  -> callback returns
  -> runtime consumes exact Disabled
  -> existing suspension bookkeeping
  -> seat.disable() acknowledgement (including seatd)
  -> Suspended
```

For an input-triggered request:

```text
input effect
  -> application.vt_switch_requested
  -> runtime switch_session()
  -> consume all reentrant seat events
  -> suspend immediately if Disabled was observed
  -> log request outcome
  -> continue only if output is still permitted
```

## Testing strategy

Deterministic tests will cover:

- failed VT request is nonfatal, leaves the runtime active, and emits no success event;
- successful and failed requests that synchronously produce `Disabled` consume it before return;
- the same-cycle output gate stops planning after reentrant disable;
- worker quiesce blocks behind an active fake ioctl, then closes admission after release;
- no-worker mode and duplicate Disable de-duplication;
- recovered worker replacement targets W2 rather than stale W1;
- legacy cursor session retirement and `Drop` perform no forbidden DRM ioctl;
- abstract session ordering includes `PreRevokeKmsQuiesce` before callback return and contains no post-callback physical cursor clear;
- existing generation-barrier, transaction, DMA-BUF, O1, predictor, and Wake Authority tests remain green.

## Upstream behavioral references

- libseat logind `handle_pause_device()` calls `set_active(false)`, whose listener callback returns before `PauseDeviceComplete`; `disable_seat()` is a no-op: <https://github.com/kennylevinsen/seatd/blob/master/libseat/backend/logind.c>
- libseat seatd queues disable events and its `disable_seat()` waits for `SERVER_SEAT_DISABLED`: <https://github.com/kennylevinsen/seatd/blob/master/libseat/backend/seatd.c>
- libseat’s Rust API documents that a successful switch request does not imply that a switch occurs: <https://docs.rs/libseat/latest/libseat/struct.Seat.html>
- Aquamarine and KWin provide behavioral references for invalidating stale physical work and treating session switching as a request, not a fatal compositor invariant: <https://github.com/hyprwm/aquamarine/blob/main/src/backend/drm/DRM.cpp>, <https://invent.kde.org/plasma/kwin>

## Out of scope

No backend-specific branching, full suspension inside the callback, unsafe runtime ownership, asynchronous session manager, new timer, KMS worker redesign, frame-pacing policy change, generation-barrier change, predictor/O1 change, transaction or DMA-BUF ownership change, Eclipse change, synthetic VT input, or hardware qualification claims without fresh evidence.
