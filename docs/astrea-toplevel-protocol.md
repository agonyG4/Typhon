# Astrea Toplevel Management v1

Astrea Toplevel Management v1 is a private, compositor-owned, read-only
Wayland protocol for the future Eclipse Dock and AltTab backends. Typhon is
the authority for the published window list; clients do not submit window
actions through this protocol.

The global is `astrea_toplevel_manager_v1`, advertised at version 1. The only
version-1 requests on both the manager and toplevel objects are their
destructor requests. Activation, minimize, restore, close, maximize,
fullscreen, workspace, output and thumbnail operations require a later
protocol version.

## Authorization

This is a private shell protocol. A manager bind requires both the peer UID to
match Typhon's effective UID and the peer PID to be the currently supervised
Astrea shell component or a verified descendant of one. The exact Wayland
`ClientId` is cached after admission; authorization is not transferred to a
later client merely because it reuses a PID, and it is removed on disconnect.
Application IDs, titles, environment variables and claimed client metadata are
never authorization inputs.

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

Every later reconciliation that changes the visible publication emits one
manager revision. All changed handle events, including additions and
terminal `closed` events, precede that manager `done`. Metadata storms are
coalesced to the authoritative state at the next publication point. A
reconciliation with no visible change emits no manager `done`.

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
it.

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
most 32 active manager bindings, four active managers per client, 4096
outstanding child handles per client, 16384 outstanding child handles
globally, and 4096 handles in one manager's deterministic window prefix.
Outstanding handles include live handles and closed handles whose client has
not yet destroyed the resource. Destroying a manager therefore does not
restore child-handle quota while its closed child resources remain alive.

When more eligible windows exist, each manager receives the deterministic
lowest-`WindowId` prefix, while `done.total` reports the full eligible count
saturated to `u32`. The `truncated` manager flag indicates that entries were
omitted. Omitted entries are admitted deterministically as lower-ID entries
leave the prefix. Manager admission reserves the complete known prefix before
creating any child resource; a resource-creation failure rolls the new
publisher records back and does not change another manager.

Each manager has independent protocol handles and lifecycle state. Destroying
a manager emits exactly one `closed` event for every still-live child, marks
those children inert, and retains their bookkeeping until each child resource
is destroyed. A window becoming permanently ineligible follows the same rule.
No metadata, state or `done` event follows `closed`. Client disconnect removes
all live and closed resources owned by that exact `ClientId` and releases its
quota idempotently.

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

Version 1 intentionally provides no workspaces, output assignment, icons,
thumbnails or window actions. Eclipse integration is a later milestone.
Future mutable requests require a protocol version increase rather than an
extension of the version-1 request surface.
