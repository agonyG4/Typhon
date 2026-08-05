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

The manager binding uses Typhon's existing Astrea shell authorization policy.
Authorization is checked when the manager is admitted from the client's
kernel credentials and the compositor's authorized Astrea shell identities.
Unauthorized clients receive the protocol `unauthorized` error before any
toplevel handle or window metadata is created. Application IDs, titles,
environment variables and claimed client metadata are not authorization
inputs.

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
icons and other internal compositor surfaces are excluded. A detached X11
surface is not eligible.

## Limits and truncation

The protocol publisher retains bounded owned snapshots only. It supports at
most 32 manager bindings, four managers per client and 4096 toplevel handles
per manager. When more eligible windows exist, each manager receives the
deterministic lowest-`WindowId` prefix, while `done.total` reports the full
eligible count saturated to `u32`. The `truncated` manager flag indicates
that entries were omitted. Omitted entries are admitted deterministically as
lower-ID entries leave the prefix.

Each manager has independent protocol handles and lifecycle state. Destroying
a manager or handle does not alter a compositor window or another manager's
publication. Dead resources are pruned during bounded reconciliation. A
handle emits `closed` at most once and no metadata, state or `done` event
follows `closed`.

## Publication ownership

The canonical publication model belongs to `CompositorState`. It is derived
from authoritative XDG, XWayland, focus and window-state records and performs
no filesystem, `/proc`, renderer or DRM work. Publication runs after normal
Wayland dispatch and disconnect teardown, after input-driven state changes,
after an XWayland scene batch, and during suspended-session cycles that still
process safe Wayland work. Repeated calls are diff-based and harmless.

Version 1 intentionally provides no workspaces, output assignment, icons,
thumbnails or window actions. Eclipse integration is a later milestone.
Future mutable requests require a protocol version increase rather than an
extension of the version-1 request surface.
