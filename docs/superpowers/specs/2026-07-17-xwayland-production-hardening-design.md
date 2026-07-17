# Typhon XWayland Production Hardening Design

Date: 2026-07-17

Status: approved design; implementation is divided into seven follow-up commits.

## Objective

Turn the existing XWayland foundation into a managed, generation-safe XWayland
service that cannot block or terminate Typhon's native compositor loop when the
XWayland process, XWM, X11 client, display socket, GPU protocol, selection
transfer, or native session changes state.

The compositor remains the top-level authority. XWayland is a contained child
service and the XWM is an adapter that translates bounded X11 work into
generation-bound compositor events and commands.

## Architectural decisions

### One native authority and one immutable GPU contract

Native bootstrap selects the DRM/KMS/render device used by scanout and dmabuf
import/export. It creates one immutable GpuProtocolCapabilities record before
GPU globals are published. The record contains:

- selected KMS and render-node paths;
- device and inode identity for the selected device;
- render-node character-device validation and openability;
- importer/exporter format and modifier intersection;
- dmabuf protocol level (none, v1, v3, or v4) and the evidence for it;
- syncobj timeline creation/import support for that same device;
- wl_drm support, PRIME capability, and the reason it is disabled when false;
- structured diagnostics suitable for startup and recovery logs.

zwp_linux_dmabuf_v1, wp_linux_drm_syncobj_manager_v1, and wl_drm are
independent publication decisions. A global is registered only when its complete
bind-time and request-time contract is supported. A card node is never
advertised as a render device and wl_drm authentication is never acknowledged
without real DRM-magic support for the selected master.

The capability record is not recomputed by a global device search and is not
silently replaced after publication. Software rendering may leave all GPU
globals disabled.

### Incremental, reactor-owned X11 transport

The existing x11rb protocol types remain the protocol representation. The
synchronous RustConnection::connect_to_stream call is replaced by an
incremental setup driver:

1. The WM socket is set nonblocking before ownership is transferred to the
   reactor.
2. x11rb_protocol::connect::Connect owns the bounded setup parser and setup
   request bytes.
3. Each reactor dispatch performs a bounded amount of nonblocking read/write
   work and returns WouldBlock without waiting.
4. Once setup is complete, RustConnection::for_connected_stream is created
   from the parsed setup and the same nonblocking stream.
5. Extension version queries, atom interning, root claiming, and existing-window
   adoption are represented as explicit startup stages with bounded pending
   request/reply tables.

The stream's read and write methods are never allowed to block. EPOLLOUT is
registered only while output is queued. Per-cycle work, total queued output,
pending replies, property requests, and startup time are bounded. A flush that
would block leaves the output queued and resumes on EPOLLOUT. A flush failure,
malformed reply, protocol error, HUP/RDHUP/ERR, or sequence failure terminates
only the active XWayland generation.

If x11rb exposes an unavoidable synchronous dependency during implementation,
the work stops at that dependency and reports the exact call path. No worker
thread or replacement codec is introduced without a new design decision.

### Generation ownership

Every process, descriptor, reactor token, X11 handle, Wayland surface
association, selection transfer, drag transfer, timer, pending request, and
diagnostic record carries an XwaylandGeneration. Stale objects may be
observed for cleanup but cannot mutate current state.

Reactor registrations are retired before their descriptors are dropped. Stable
registration tokens and explicit registration identities are the correctness
mechanism; kcmp is diagnostic-only and EPERM/ENOSYS never make teardown fail.

The service always keeps a bounded stderr ring for the active and latest failed
generation. TYPHON_XWAYLAND_LOG=1 controls forwarding only.

### Managed process lifecycle

The service retains the existing displayfd race fix, deferred lease teardown,
private WAYLAND_SOCKET, private-client authorization, xwayland-shell-v1
association, stale-generation checks, and persistent crash backoff.

The XWM startup stages are:

~~~text
SocketConnected
  -> SetupReceived
  -> ExtensionsDiscovered
  -> AtomsInterned
  -> RootClaimed
  -> ExistingWindowsAdopted
  -> Running
~~~

The process exit classifier distinguishes expected shutdown after Running,
expected idle exit after Running, startup exit before readiness, crash/signal,
and compositor-requested termination. A clean exit before readiness is a
startup failure. Only an expected post-ready exit resets the crash budget.

Termination is SIGTERM, a bounded grace deadline, then one SIGKILL
escalation. Failed state has an explicit reset operation, accepted only after
complete old-generation teardown. Launch requests are bounded during Backoff,
each has an absolute deadline, and are visibly rejected in Failed or after
expiry. Listener, ready, and status environments are separate typed values;
none can expose a dead DISPLAY.

### X11 window lifecycle

The XWM keeps X11 map authorization independent of rendering readiness:

~~~text
Observed
  -> MapRequested
  -> PropertiesPending
  -> MapCommanded
  -> MappedAwaitingAssociation
  -> AssociatedAwaitingBuffer
  -> Renderable
  -> Withdrawn
  -> Destroyed
~~~

For managed windows, bounded core-property discovery precedes the compositor's
map decision and XwmCommand::Map. MapNotify and mapping authorization do not
wait for a Wayland serial or a first buffer. For override-redirect windows,
the client maps directly; Typhon adopts the window after MapNotify without a
normal map command or normal keyboard-focus policy.

At startup, QueryTree, GetWindowAttributes, geometry, and bounded property
requests adopt pre-existing windows. Duplicate map requests preserve valid
associations and buffers. Remap creates a new surface generation only when the
X11/Wayland identity actually changes.

The association join tracks serial-before-map, serial-after-map, buffer-before-
serial, and duplicate/stale cases. All waits have deadlines and bounded GC for
serials, associations, first buffers, and late events after destruction.

### ICCCM/EWMH boundary

ICCCM and EWMH are implemented in separate modules. Supported properties are
validated for atom type, format, and bounded length before transactional
application. Safe defaults are used after an initial-property deadline; valid
late replies are applied as deltas.

Configure requests preserve explicit value-mask flags and apply only requested
fields. Geometry constraints, all ICCCM stack modes, sibling-relative stacking,
transient-parent ordering, synthetic ConfigureNotify, focus models, recent
activation timestamps, startup tokens, independent horizontal/vertical
maximization, restore state, WM state, client lists, work area, and active-window
properties are represented at the XWM boundary.

Unknown state actions are rejected. Override-redirect windows are excluded from
normal EWMH client lists. _NET_SUPPORTED contains only features implemented and
covered by tests. Composite remains required for rootless redirection;
XFixes, Shape, RandR, and Sync are optional until their active behavior and
version checks exist. Shape has a rectangular fallback.

Resize synchronization uses a WM_PROTOCOLS ClientMessage containing
_NET_WM_SYNC_REQUEST, timestamp, and the low/high counter words. Timeout,
destruction, generation retirement, and normal completion restore
_XWAYLAND_ALLOW_COMMITS exactly once.

### Secure display and authentication artifacts

Display allocation uses verified directory descriptors and no-follow relative
filesystem operations. XDG_RUNTIME_DIR and /tmp/.X11-unix are validated for
ownership, type, permissions, sticky-bit policy, and symlink safety. Typhon-
created socket files are explicitly mode 0666; MIT-MAGIC-COOKIE authorization
provides access control independent of umask.

The lease retains parent directory descriptors and file identities. Cleanup
uses the retained descriptor, basename, and expected identity and never falls
back to an all-zero identity. Xauthority field encoding returns Result and
rejects fields larger than the protocol's 16-bit length.

Filesystem and abstract sockets remain available where interoperable. Stale
locks, concurrent allocation, replacement-before-cleanup, symlink swaps, and
umask independence are tested.

### Wayland selection and drag-and-drop adapter

CLIPBOARD, PRIMARY, and Xdnd are implemented only after startup, lifecycle,
ICCCM/EWMH, and filesystem phases. The existing Wayland data-device and
selection state remains the sole authority. XWayland owns only adapter state,
bounded nonblocking fds, XFixes/Xdnd protocol objects, and generation-bound
transfers.

Transfers support TARGETS, TIMESTAMP, MULTIPLE, the required text/URI/image/
octet MIME forms, and INCR. Data is streamed through bounded owned fds and is
never accumulated without a hard limit in compositor memory. Ownership changes,
cancellation, source destruction, timeouts, XWayland crash, shutdown, and
reflection-loop prevention are explicit cleanup paths.

Xdnd supports source and target negotiation, copy/move/link/ask/none actions,
coordinate conversion, target changes, representable drag icons, and bounded
cancel/crash/timeout cleanup. XEmbed and system-tray behavior are out of scope
and are not advertised.

### Output, cursor, and validation surface

RandR publication reports coherent root dimensions, output rectangles, primary
output, work area, and runtime changes under one explicit global X11 DPI/scale
policy. Mixed per-monitor DPI is not advertised.

X11 cursor requests participate in Typhon's cursor ownership and generation
teardown; an older X11 generation cannot override a newer Wayland owner.

bin/check-xwayland-session reports DISPLAY, XAUTHORITY, process argv, socket
forms, xdpyinfo, XWM state, generation, client count, selection bridge state,
and recent failure diagnostics. The native compatibility matrix is documented
in docs/XWAYLAND.md and executable checks are added without advertising
hardware-dependent features when hardware is absent.

## Phase boundaries and commits

Each phase is test-first, independently reviewable, and must leave the full
repository validation green before the next phase starts. Native validation is
performed at the phase checkpoint; unavailable hardware is reported as an
unmet gate rather than treated as success.

### Preparatory design commit

This document is committed separately and does not change the required order of
the seven implementation commits.

### Phase 1: GPU protocol publication

Commit:

~~~text
fix(xwayland): advertise only usable GPU protocol globals
~~~

Add GpuProtocolCapabilities, bind publication to it, remove unrelated syncobj
device search from the publication path, validate dmabuf feedback levels,
implement truthful wl_drm behavior, and add missing/empty/nonexistent/
regular-file/inaccessible/card/render/mismatch/missing-feedback tests.

Checkpoint: on real hardware, native eager startup reaches managed Running,
validates displayfd, xdpyinfo succeeds, incomplete device contracts omit
wl_drm, and no path depends on XWAYLAND_NO_GLAMOR.

### Phase 2: startup and failure containment

Commit:

~~~text
fix(xwayland): contain XWM startup and reactor failures
~~~

Add the incremental XWM driver, bounded output and pending requests, diagnostics,
stderr rings, stable registration identities, lifecycle exit classification,
SIGTERM/grace/SIGKILL escalation, typed environments, launch deadlines, and
controlled Failed reset.

Checkpoint: corrupt, stall, disconnect, flush-fail, and kill XWayland/XWM cases
leave Typhon running and reach bounded Backoff/Failed behavior without storms.

### Phase 3: map and surface lifecycle

Commit:

~~~text
fix(xwayland): separate X11 mapping from surface readiness
~~~

Refactor adoption and map state, pre-existing-window discovery, override-redirect
handling, serial association, deadlines, and late-event GC.

Checkpoint: real xmessage maps before association/buffer, closes, and reopens.

### Phase 4: ICCCM/EWMH semantics

Commit:

~~~text
fix(xwayland): complete core ICCCM and EWMH window semantics
~~~

Implement the supported ICCCM/EWMH, focus, state, shape, property, configure,
stacking, transient, and resize-sync contracts and tests.

Checkpoint: GTK/X11, Qt/XCB, dialogs, menus, focus, state changes, and resize
behave correctly; unsupported features remain absent from _NET_SUPPORTED.

### Phase 5: socket and Xauthority hardening

Commit:

~~~text
fix(xwayland): harden display sockets and Xauthority ownership
~~~

Implement descriptor-relative allocation, identity-safe cleanup, explicit modes,
validated runtime directories, and fallible Xauthority encoding.

Checkpoint: filesystem and abstract connections work under varied umasks and
replacement/symlink/concurrency tests cannot remove another process's files.

### Phase 6: selections and Xdnd

Commit:

~~~text
feat(xwayland): bridge X11 selections and drag and drop
~~~

Add the bounded selection and Xdnd adapter over Typhon's Wayland authority.

Checkpoint: CLIPBOARD, PRIMARY, and Xdnd work in both directions with bounded
cleanup on cancellation, crash, restart, and shutdown.

### Phase 7: native compatibility and recovery matrix

Commit:

~~~text
test(xwayland): add native compatibility and recovery matrix
~~~

Add RandR/output/cursor policy, documentation, validation tooling, and the
native matrix for eager/lazy startup, X11 clients, Steam, Proton, fullscreen,
resize, focus, clipboard, DnD, crash, VT/suspend, shutdown, and stability.

Checkpoint: every claim in the compatibility report is backed by a runnable
test or explicitly marked unavailable/unsupported.

## Validation contract

After every phase:

~~~text
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
./bin/check-source-layout
git diff --check
~~~

Targeted unit/integration tests run before the full suite. Native gates run at
the phase checkpoint rather than being deferred to Phase 7. A failed checkpoint
stops progression and records the exact failure, generation, resource state,
and diagnostic ring.

## Pre-implementation review

The document was reviewed before implementation for unresolved ambiguity,
contradictory ordering, placeholders, and uncontrolled scope. No unresolved
item remains. The transport choice is explicit: x11rb protocol types remain in
use, setup is incremental through `Connect`, and the work stops with a blocking
call-path report if that boundary cannot satisfy the nonblocking contract. The
seven implementation commits remain ordered after this preparatory document
commit, and the selection/Xdnd work is isolated to Phase 6.

## Explicit non-goals

This design does not add WL_SURFACE_ID, rootful X11, TCP X11, XEmbed, system
tray support, Steam-specific title/class hacks, mixed-DPI support, arbitrary
clipboard buffering, global error suppression, or a worker-thread fallback for
the XWM transport.
