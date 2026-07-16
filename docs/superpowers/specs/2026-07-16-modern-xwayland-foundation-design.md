# Modern XWayland Foundation Design

## Scope

This change adds the process, display, service, reactor, private Wayland identity,
and `xwayland-shell-v1` association foundations needed by a future XWM. It does
not launch X11 applications by default and does not implement window management,
clipboard, DnD, RandR, scaling, or renderer/KMS integration.

The task specification is authoritative because the referenced architecture
review document is not present in this checkout.

## Architecture

`NativeRuntime` owns one `XwaylandService`. The service owns its long-lived
`DisplayLease` (lock, filesystem and abstract listeners, Xauthority file), mode,
state machine, metrics, and the current generation. A generation owns only
restart-specific resources: private Wayland and WM socket pairs, displayfd,
mapped child descriptors, reactor registrations, startup deadline, and the
managed XWayland process identity.

`ChildSupervisor` remains the common child reaper. It assigns a monotonically
allocated `ManagedProcessId` and records PID plus verified dedicated PGID. Its
restart factory is retained for existing critical session components, but
XWayland is spawned once per service generation and is never generically
restarted by the supervisor.

The service state transitions are:

```text
Off -> Disabled
Base bootstrap -> Armed
listener readiness -> Starting(generation)
displayfd + authorized shell bind -> RunningBase
abnormal exit/timeout -> Backoff -> Armed or Failed
clean -terminate exit -> Armed
shutdown/fatal cleanup -> Disabled
```

Each start allocates a nonzero generation and rejects stale process exits,
reactor tokens, readiness payloads, and private-client notifications. The
service becomes `RunningBase` only after both independent readiness conditions
for that generation have succeeded.

## Process and FD ownership

Production commands use `std::process::Command` directly. `SpawnCommand` owns all
parent-side source descriptors until `spawn` returns and maps only deterministic,
collision-checked targets in `pre_exec`. The child setup uses only async-signal-safe
operations and clears CLOEXEC only on mapped targets. Parent descriptors remain
RAII-owned on every failure path.

Session-owned children use `ProcessGroupPolicy::Dedicated`; ordinary applications
continue to inherit their process group. Shutdown and emergency cleanup signal
only stored verified PGIDs for session-owned entries, then reap immediately
available exits without making `Drop` wait indefinitely. A bootstrap guard owns
the cleanup obligation for already-started session children until bootstrap
commits.

## Display and authentication security

`DisplayLease` scans a bounded display range and atomically claims a lock with
`O_CREAT|O_EXCL|O_CLOEXEC`. It verifies lock/socket safety, rejects symlink
substitution, creates both local listener forms before launch, and removes only
artifacts created by that lease. The Xauthority file is private to the lease,
mode `0600`, contains a random 128-bit-or-greater MIT-MAGIC-COOKIE-1 record, and
is never logged.

The child receives only the two display listeners, displayfd, private Wayland
socket, and WM socket through typed mappings. `WAYLAND_SOCKET` points at the
private socket; inherited `WAYLAND_DISPLAY`, `DISPLAY`, and host `XAUTHORITY`
are removed. TCP is disabled in the command line.

## Compositor identity and protocol

The compositor receives the parent side of the private Wayland socket and
inserts the client directly into `DisplayHandle`, recording
`(ClientId, XwaylandGeneration)` separately from UID/PID data. The compositor
publishes one `xwayland_shell_v1` global with visibility and bind authorization
restricted to the active exact `ClientId`; old generations are revoked.

`get_xwayland_surface` assigns the permanent XWayland surface role. Protocol
state holds only a pending nonzero serial, committed nonzero serial, generation,
and association-object lifetime. `set_serial` changes pending state; the next
`wl_surface.commit` atomically promotes it and emits exactly one normalized
registry event. Destroying the association object before commit drops pending
state; destroying it after commit does not undo the association. A second
commit, role conflict, zero serial, stale generation, disconnect, or surface
destruction follows the specified protocol/error and registry cleanup rules.

The future XWM consumes `AssociationRegistry` events keyed by
`(XwaylandGeneration, NonZeroU64)` and `SurfaceId`; no XIDs or window-management
state are introduced.

## Runtime and diagnostics

The reactor retains exact `ReactorToken` and epoll flags for XWayland events,
including normal HUP/RDHUP on displayfd. `NativeRuntime` dispatches these events
and XWayland child exits before generic launch handling, includes service
deadlines in timer planning, and calls service shutdown/emergency cleanup before
final supervisor completion or runtime-owned FD destruction.

`TYPHON_XWAYLAND` parses only `off`, `base`, and `eager`; absent and unknown
values resolve to `off`. `base` arms lazy listeners, `eager` invokes the same
generation start path immediately, and neither mode changes normal application
launch defaults. Diagnostic/test callers may request the service-owned
`DISPLAY` and `XAUTHORITY` through the existing `X11Bridge::IsolatedXWayland`
model.

## Testing strategy

Implementation proceeds test-first in independent commits:

1. Linux process identity, process groups, typed FD mapping, bootstrap guards,
   and fatal cleanup.
2. Focused XWayland module boundary and generation/display/auth unit tests.
3. Display lease artifact safety and parseable Xauthority tests.
4. Service state machine with deterministic fake children and stale-generation
   tests.
5. Reactor token/flag retention and native-runtime ownership tests.
6. Private client authorization, protocol role/serial semantics, and association
   registry tests.
7. Diagnostic environment sanitization and final validation.

The normal test suite remains independent of GPU, DRM, active TTY, host X, and
an installed Xwayland binary. Any real-binary test is opt-in and ignored.
