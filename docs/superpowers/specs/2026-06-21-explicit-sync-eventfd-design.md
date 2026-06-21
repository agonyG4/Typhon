# Event-Driven Explicit Sync Acquire Design

Date: 2026-06-21

## Objective

Make a signaled client acquire timeline point wake the native compositor through
`DRM_IOCTL_SYNCOBJ_EVENTFD` and epoll. Readiness must apply only to the exact
surface commit that registered the watch, while unsupported kernels retain a
bounded, deadline-driven correctness fallback.

## Existing Behavior

An unsignaled explicit-sync dmabuf commit is stored in
`pending_explicit_sync_commits`. `OwnCompositorServer::prepare_frame()` scans
the entire collection and performs a nonblocking timeline wait for every
commit. Native scheduling treats the collection as protocol work and arms a
refresh-aligned timer until all points become ready. Consequently, a point that
signals while the compositor sleeps is not noticed until client, input, DRM, or
timer activity wakes epoll.

## Commit State And Identity

Every blocked commit receives a nonzero, monotonically allocated
`AcquireCommitId`. A pending commit records its surface, client ownership,
buffer, acquire timeline and point, release point, callbacks, damage, commit
receipt timestamp, and one of these states:

- waiting for native watch registration;
- waiting for an eventfd-backed watch;
- waiting for a fallback retry;
- acquire-ready.

Only an exact commit ID transition may change waiting state to acquire-ready.
The transition also validates the surface, timeline identity, and point. A
newer buffer commit for the same surface supersedes and removes an older
blocked commit before adding the new one. Removing unused blocked state never
signals its explicit release point or fabricates `wl_buffer.release`.

The compositor exposes narrow native integration operations: drain newly
created watch requests, drain cancellation notifications, mark an exact commit
as eventfd-backed or fallback-backed, mark an exact commit ready, and reject a
registration without pretending readiness. Nested/non-native operation keeps a
separate polling mode so this native integration does not couple general
surface state to epoll.

## DRM UAPI Boundary

`src/syncobj.rs` will contain a narrow `repr(C)` representation of
`drm_syncobj_eventfd` and one ioctl wrapper. It uses the audited DRM command
number, signal-wait flags only, zero padding, checked eventfd conversion, and
returns the original errno unchanged. Layout, full-width point preservation,
flags, padding, invalid fd handling, and unsupported-error classification will
be unit tested.

Native mode replaces the compositor's discovery-opened syncobj device before
advertising GPU buffer globals. The replacement is built from a close-on-exec
duplicate of the active native DRM file description. Imported handles and
eventfd registration therefore use the same DRM file description and backend
generation. Non-native setup retains current device discovery.

Capability is probed lazily with the first real pending point and is scoped to
one backend generation:

- `Supported`: registrations use eventfds;
- `Unsupported`: `ENOTTY`, `EOPNOTSUPP`, or `ENOSYS` activates fallback;
- `BrokenOrRejected(errno)`: a driver rejection is diagnosed and uses fallback
  only when the errno represents an operational incompatibility rather than a
  local invariant failure.

Invalid arguments remain errors. Resource exhaustion remains a per-watch
failure and leaves the commit blocked; it is not reported as readiness or
silently converted into permanent polling.

## Reactor Tokens

The native event loop gains dynamic registration and removal. A
`ReactorToken` encodes a slot and generation; it is never derived from the raw
fd. Epoll stores that token in `epoll_event.u64`. Removing a registration calls
`EPOLL_CTL_DEL` before the owner closes its fd. A queued token whose slot is
inactive or whose generation no longer matches is returned as stale rather
than attributed to a replacement source.

Fixed DRM, Wayland, input, and timer sources use the same registration
machinery. A wakeup includes explicit ready acquire tokens in addition to its
aggregate reason mask. Slot exhaustion and generation exhaustion return errors
instead of aliasing a live source.

## Watch Registry

`src/native/explicit_sync.rs` will own `ExplicitSyncWatchRegistry`. Each watch
contains:

- reactor token;
- owned nonblocking, close-on-exec eventfd;
- DRM backend generation;
- cloned timeline ownership and imported handle;
- exact timeline point;
- surface, client, buffer, and commit identity;
- commit receipt timestamp.

The registry indexes watches by reactor token and commit ID. It provides
register, cancel-by-commit, cancel-by-surface/client, handle-ready-token,
fallback retry, backend shutdown, metrics, and leak assertions. The watch owns
the eventfd; the event loop owns only its epoll registration.

Registration uses this race-proof sequence:

1. perform the existing nonblocking signaled check;
2. create `eventfd(0, EFD_NONBLOCK | EFD_CLOEXEC)`;
3. register that eventfd for the exact syncobj handle and point with the ioctl;
4. add the eventfd to epoll;
5. publish the watch in both registry indexes;
6. perform one final nonblocking point check.

If the point signals between any steps, either the final check observes it or
the level-triggered readable eventfd remains observable by the next
`epoll_wait`. If the final check observes readiness, registration is removed
and the exact commit follows the same ready transition as an eventfd wake.

The eventfd drain retries `EINTR`, consumes exactly one `u64`, treats `EAGAIN`
as not yet notified, accepts counters greater than one as one watch wake, and
reports short reads as internal errors. After a wake, a defensive nonblocking
timeline check must still report signaled before readiness is applied.

## Runtime Ordering

One native cycle remains ordered as follows:

1. receive epoll readiness;
2. drain matching DRM pageflip completions;
3. dispatch already-readable Wayland requests;
4. route input, including the existing extra Wayland dispatch around pointer
   constraint changes;
5. process compositor watch registrations and cancellations;
6. apply exact acquire-ready transitions and due fallback checks;
7. prepare newly ready commits only when doing so cannot attach their frame
   completion state to an older outstanding pageflip;
8. reevaluate scheduler work and render only when no pageflip is pending;
9. arm the earliest scheduler, watchdog, or fallback absolute deadline;
10. return to epoll.

Acquire readiness during a pending pageflip records the commit as ready and
queues future visual work, but its buffer callbacks, presentation feedback,
and releases are not promoted into the outstanding frame. DRM pageflip events
remain the only asynchronous completion authority.

## Cancellation And Teardown

Compositor lifecycle paths emit cancellation before dropping a blocked commit:
supersession, pending-state discard, surface destruction, invalidating buffer
destruction or detach, explicit-sync surface destruction, timeline resource
destruction, client disconnect, and protocol rejection after commit identity
allocation. Native backend teardown cancels every registry entry before DRM
timeline ownership or the event loop is dropped.

Cancellation removes epoll registration, removes both registry indexes, drops
the eventfd, and detaches the watch from the exact commit. It never marks the
commit ready. Duplicate cancellation is harmless. Tokens already returned by
epoll become stale through the reactor generation check. Backend generation is
validated again when handling readiness, preventing an old file generation
from affecting a new import with the same numeric handle.

## Fallback

Unsupported registrations become separate fallback entries. The point is
checked immediately after the commit. While at least one fallback entry is
pending, the registry exposes one absolute monotonic retry deadline. Retries
use the output refresh interval as the initial conservative interval and may
back off up to a bounded multiple when points remain stuck. Each next deadline
is derived from the previous absolute deadline, skipping missed intervals, so
late wakeups do not accumulate drift.

The fallback deadline is disarmed immediately when the fallback set becomes
empty. Eventfd-backed watches are never included in fallback scans. A timeout
only triggers another nonblocking check and can diagnose a stuck point; it
never fabricates readiness. The main timerfd is armed to the minimum of the
frame scheduler deadline and fallback deadline, so input and Wayland sources
remain independently dispatchable.

## Release Semantics

Acquire readiness only changes buffer eligibility. It does not call
`finish_frame`, send presentation feedback, complete frame callbacks, signal an
explicit release point, or send `wl_buffer.release`. Existing buffer promotion
continues to create the release target, and existing immediate or pageflip
completion continues to release buffers after actual compositor use.

Canceled or superseded blocked commits were never used, so their release point
is left untouched as required by this task.

## Diagnostics

Low-overhead counters cover active eventfd and fallback watches,
registrations, already-signaled fast paths, eventfd wakeups, stale or duplicate
wakeups, cancellation reasons, registration errno classes, fallback
activation, maximum simultaneous watches, and leak assertions. Timestamps
measure commit-to-ready and ready-to-render-submit latency. Per-watch identity
is trace/performance-only and always includes internal commit, token, and
backend generation context.

## Testing

Implementation proceeds test-first in these slices:

1. UAPI layout, conversion, flags, errno, and capability classification.
2. Dynamic reactor registration, removal, stale generation, token exhaustion,
   and fd-number reuse.
3. Registry registration, exact lookup, cancellation scopes, duplicate wakes,
   backend mismatch, shutdown, and leak-free counts using an injected notifier.
4. Exact compositor commit identity, supersession, destruction, timeline and
   sync-surface cancellation, and separation from release behavior.
5. Registration races, already-signaled fast path, defensive pending result,
   scheduler wakeup, and pageflip-pending deferral.
6. Fallback activation, absolute deadlines, disarming, no idle timer, no
   eventfd-watch scans, and stuck-point correctness.
7. Existing explicit-sync, dmabuf release, callback, presentation, cursor,
   input, resize, fullscreen, and clipboard regressions.

Tests use fake notifier/readiness implementations where physical DRM behavior
is unavailable. Hardware-only tests remain conditional and are not counted as
portable verification.

## Non-Goals

This design does not add atomic KMS, KMS fences, VRR, direct scanout, tearing,
plane assignment, renderer changes, release-fence export redesign, XWayland,
multi-output, hotplug, multi-GPU synchronization, or presentation timestamp
changes.
