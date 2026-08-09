# Astrea Toplevel Management v2

Astrea Toplevel Management v2 is a private, compositor-owned Wayland protocol
for the Eclipse Dock and AltTab backends. Typhon is the authority for the
published window list and exact window actions.

The global is `astrea_toplevel_manager_v1`, advertised at version 2. Version 1
clients remain read-only and compatible: the only version-1 requests on both
the manager and toplevel objects are their destructor requests. Version 2 adds
exact activate, minimize, restore, and close requests to toplevel handles, with
manager-owned `action_done` completion. Maximize, fullscreen, workspace,
output, and thumbnail operations remain outside this milestone.

## Version 2 actions

Each action is sent through the exact toplevel handle, whose immutable
`identifier` is Typhon's stable `WindowId`. The request carries only a
client-owned 64-bit token split into two `uint` values:

```text
activate(token_hi, token_lo)
minimize(token_hi, token_lo)
restore(token_hi, token_lo)
close(token_hi, token_lo)
```

Completion is always sent by the manager, never by the potentially terminal
handle:

```text
action_done(token_hi, token_lo, action, result)
```

The stable results are `accepted`, `no_change`, and `unavailable`. `accepted`
for `close` means only that Typhon issued the graceful XDG or existing XWM
close request. The normal handle `closed` event remains the disappearance
lifecycle boundary. `unavailable` is a semantic action result for an invalid
or non-actionable target, a duplicate token already pending on this manager,
or a manager that has reached its pending-action bound. It does not mean that
the request failed protocol authentication.

Typhon's manager-owned action tracker has at most 64 reserved entries. Normal
M7-B actions complete synchronously: reserve the token, execute the exact
WindowId primitive, emit `action_done` on the manager, and release the token in
that dispatch. There is no artificial production queue or delayed action.
Duplicate and beyond-bound requests emit `unavailable` without adding a new
entry. Completion makes a token immediately reusable and no completed-token
history is retained. Manager destruction or client disconnect clears any
tracker entries; longer-lived state is reserved for an existing production
action that genuinely requires deferred completion.

## Authorization

This is a private shell protocol. A client may authenticate once through
astrea_shell_auth_manager_v1 by presenting the opaque per-session capability
from the protected Astrea runtime handoff. On success Typhon authorizes the
exact Wayland ClientId for that connection; the grant is removed on
disconnect. The peer UID is checked during authentication but UID alone never
grants access.

For the compositor-launched shell path, the currently supervised Astrea shell
component and its verified descendants remain admitted without the capability.
PID ancestry is an admission path only; it is not required after successful
capability authentication, so systemd --user daemons can reconnect
independently. Application IDs, titles, environment variables and claimed
client metadata are never authorization inputs.

The current Wayland backend does not provide a state-aware global filter to the
compositor's publication model, so the global may remain visible to registry
clients. Bind is still checked defensively. An unauthorized bind receives the
protocol `unauthorized` error, creates no manager bookkeeping or child handles,
and exposes no window metadata.

## Initial enumeration and batches

An authorized manager receives one server-created handle for each eligible
window, followed by the handle's initial state in this order:

```text
identifier, app_id, title, pid, kind, state, focus_serial, handle.done
```

The manager then receives one `done` event. This is the explicit initial-list
completion boundary; clients must not use a display roundtrip as the semantic
boundary.

If a manager cannot be kept synchronized after admission, it receives one
terminal `failed` event with `resource_limit` or `publication_failure`. Its
children are closed before that event when possible, and no `toplevel` or
`done` event follows. The client connection and any other manager remain
usable. Bind-time authorization and quota violations are still fatal protocol
errors because they are client admission violations.

Every later publication is one explicit transaction. A transaction may be
processed in several bounded runtime cycles, but it receives exactly one
revision. At most 256 windows are processed in one cycle. All handle events,
including additions and terminal `closed` events, precede one matching manager
`done` for that revision. Clients must buffer handle changes until that manager
`done` arrives; the manager `done` is the atomic publication boundary.
Metadata storms are coalesced to the authoritative state at the next
publication point. A no-op publication emits no manager `done`.

State changes observed while a transaction is active are queued for a later
transaction. The current target remains frozen until its manager completions
have been sent and the canonical snapshot is committed.

Revisions and focus serials are unsigned 64-bit values encoded as high and
low 32-bit event arguments. Wayland event order remains authoritative when a
counter wraps.

## Identifiers and metadata

`identifier` is the decimal representation of Typhon's stable nonzero
`WindowId`. It is immutable for a handle and never contains a pointer, surface
ID, XID or hexadecimal prefix. A destroyed compositor window never reuses its
ID. An unmap/remap of the same still-live compositor record can create a new
protocol handle with the same identifier.

PID is metadata only. XDG PID metadata is captured from the Wayland client's
credentials when its toplevel role is created. X11 PID metadata comes from
the existing XWM metadata. PID is never used for identity or deduplication.

`app_id` and `title` are bounded UTF-8 strings. An empty app ID or title means
that the compositor has no value or that the value is intentionally empty.
The maximum app ID is 256 bytes and the maximum title is 1024 bytes; truncation
always preserves valid UTF-8. `pid = 0` means unavailable.

## Kinds and states

The stable kind values are:

```text
xdg_toplevel = 0
x11_toplevel = 1
x11_dialog   = 2
```

State is an append-only bitfield:

```text
active     = 1
minimized  = 2
maximized  = 4
fullscreen = 8
```

`focus_serial = 0` means the window has not been focused since publication
tracking began. Application focus transitions update the focused and
previously focused windows in one publication revision. Reasserting focus on
the same window does not advance its serial, and losing focus does not reset
it. Serial advancement skips zero when the counter wraps; zero remains
reserved for never-focused windows.

## Eligible windows

Typhon publishes only task-relevant compositor-managed toplevels:

* XDG toplevels are published after their authoritative lifecycle is mapped.
  Minimized XDG windows remain published. A null-buffer unmap closes the
  handle; a later remap creates a new handle.
* Managed X11 windows with a live XWayland surface are published when their
  role is `Toplevel` or `Dialog`.

XDG popups, layer-shell surfaces, cursor surfaces, notifications,
override-redirect windows, auxiliary popups, auxiliary support windows, DND
icons and other internal compositor surfaces are excluded. Typhon v1 has no
separate authoritative skip-taskbar or urgent bit beyond these role and
lifecycle filters, so it does not publish such a claim. A detached X11
surface is not eligible.

## Limits and truncation

The protocol publisher retains bounded owned snapshots only. It supports at
most 32 active, pending-initial or terminal manager bindings in total, four
such managers per client, 4096 active child handles per client, 16384 active
child handles globally, and 4096
handles in one manager's deterministic window prefix. Retired resources have
separate hard limits of 8192 per client and 32768 globally. Admission also
reserves space for active resources to become retired, so later destruction
cannot exceed the retired-resource budget. Outstanding resources include live
handles and closed handles whose client has not yet destroyed the resource.
Terminal managers use the same four-per-client and 32-total bounds. Destroying
a terminal manager or disconnecting its client releases the manager slot.
Destroying a manager restores its active-manager slot, but does not restore
child-handle quota while its closed child resources remain alive.

When more eligible windows exist, each manager receives the deterministic
lowest-`WindowId` prefix, while `done.total` reports the full eligible count
saturated to `u32`. The `truncated` manager flag indicates that entries were
omitted. Omitted entries are admitted deterministically as lower-ID entries
leave the prefix. The publisher tracks the complete eligible identity set up
to a hard bound of 65,536 windows without retaining omitted metadata. Above
that bound publication fails closed with a bounded resource error. A
client-destroyed handle remains suppressed while its window remains eligible,
even if it leaves and later re-enters the bounded prefix; suppression is
cleared only after authoritative ineligibility. Manager admission reserves
the complete known prefix before creating any child resource. A resource or
initial-event failure retires every resource created by that attempt and
removes the affected manager without changing another manager.

Each manager has independent protocol handles and lifecycle state. Destroying
a manager emits exactly one `closed` event for every still-live child, marks
those children inert, moves them into publisher-owned retired bookkeeping, and
retains that bookkeeping until each child resource is destroyed. A window
becoming permanently ineligible follows the same rule. No metadata, state or
`done` event follows `closed`. A stale child destroy is matched by the exact
client, manager resource, child resource and `WindowId`, so it cannot affect a
new handle. Clients should destroy terminal handles promptly, but Typhon
remains bounded if they delay destruction. Client disconnect removes all live
and retired resources owned by that exact `ClientId` and releases its quota
idempotently.

If initial resource creation or a later event send fails after manager
admission, the affected manager is removed from healthy publication, all of
its live children are terminally retired, and the manager receives one
terminal `failed` event when delivery is possible. The failure is client-local:
it does not post a fatal protocol error and does not disconnect another manager
or another protocol owned by the same Wayland client. A failed manager remains
tracked until the client destroys it. Bind-time authorization, manager-limit
and known resource-limit violations remain protocol errors.

A manager that binds while a transaction is active is admitted as a bounded
pending-initial manager. It receives no partial state. After the transaction
commits, it is enumerated from the committed canonical snapshot and receives
one complete initial `done`. Pending managers count toward manager and handle
limits; their bounded initial handle reservation is made before admission.

The publisher schedules a native continuation wake whenever a transaction has
remaining IDs, including while the session is suspended. It does not depend on
pointer activity, rendering damage or another client request.

## Publication ownership

The canonical publication model belongs to `CompositorState`. It is derived
from authoritative XDG, XWayland, focus and window-state records and performs
no filesystem, `/proc`, renderer or DRM work. Publication runs after normal
Wayland dispatch and disconnect teardown, after input-driven state changes,
after an XWayland scene batch, and during suspended-session cycles that still
process safe Wayland work.

Publication is dirty-driven. Title, app ID, PID, focus, state, eligibility and
window lifecycle mutation sites mark only the affected `WindowId`; pointer
motion, cursor animation, pageflips, rendering damage and frame scheduling do
not mark toplevel state. Repeated changes to one window coalesce before the
next safe publication point, and one bounded owned snapshot is fanned out to
all managers. At most 256 dirty or removed windows are processed per cycle,
with removals ordered before ordinary updates. Remaining work is retained for
the next cycle and publishes the latest authoritative value rather than an
unbounded event history.

Initial enumeration and structural corrections may perform one complete
bounded eligibility scan. Ordinary input hot paths do not perform that scan.
Each non-empty batch advances one monotonic revision; an unchanged
reconciliation emits no manager `done`.

Version 2 intentionally provides no workspaces, output assignment, icons or
thumbnails. Future mutable requests require another protocol version increase
rather than an extension of the version-1 request surface.
