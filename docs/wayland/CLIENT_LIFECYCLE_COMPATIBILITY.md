# Wayland Client Lifecycle Compatibility

Typhon keeps protocol validation strict where accepting a request could cross a
client boundary, make object ownership ambiguous, or leave compositor state
corrupt. A small set of ordering mistakes has an unambiguous server-owned
cleanup path, so those mistakes are diagnosed and recovered without ending the
client connection.

## Recovery boundary

| Request sequence | Classification | Recovery |
| --- | --- | --- |
| `wl_surface.destroy` while an XDG toplevel, XDG popup, subsurface, layer surface, or XWayland role is live | `RECOVERABLE_LIFECYCLE` | Record a bounded diagnostic, then run `teardown_surface_resource` once. That path removes scene state, pending commits, buffers, role registrations, focus, and ownership maps. Role resources that remain on the wire become inert. Cursor and drag-icon roles do not trigger this compatibility path. |
| `xdg_surface.destroy` while its toplevel or popup object is live | `RECOVERABLE_LIFECYCLE` | Record a bounded diagnostic, then run `unregister_xdg_surface_role`. It retires the role and associated popup/configure/decoration state. Later role requests are ignored and their normal destroy requests are harmless. |
| Repeating `wl_data_device.set_selection` with the same source that is still the active clipboard source | `RECOVERABLE_LIFECYCLE` | Record a bounded diagnostic and preserve the current selection without another cancellation or generation change. |
| Reusing a source after selection clear/replacement, configuring an active clipboard source for DnD, using a clipboard source for DnD, or reusing a drag source | `FATAL` | Keep `wl_data_device.used_source` and `wl_data_source.invalid_source` fatal. Retired sources cannot be made active again. |
| Invalid data-offer lifecycle requests, including `finish` before a valid drop, selection-offer DnD requests, or receives after a drag offer is terminal | `FATAL` | Keep `invalid_finish` and `invalid_offer` errors. Invalid MIME offers, action masks, and preferred actions remain fatal too. |
| `wl_subsurface`, layer-surface, or XWayland role requests after their underlying `wl_surface` has been destroyed | `INERT` | The handler returns when the matching live surface resource is gone; it posts no protocol error and cannot mutate compositor state. Normal role-resource destruction remains idempotent. |
| `xdg_wm_base.destroy` with live `xdg_surface` objects, toplevel destruction with a live decoration object, or popup destruction out of stack order | `FATAL` | Preserve `defunct_surfaces`, `orphaned`, and `not_the_topmost_popup` protocol errors. An inert XDG surface left by surface-first recovery still counts as live until that XDG resource is destroyed or its client disconnects. |
| Cross-client object references, invalid object references, invalid configure acknowledgements, role reassignment, invalid popup parents, invalid DND action masks, and buffer ownership violations | `FATAL` | Keep existing protocol errors and client termination. |

Compatibility records are held in a fixed 64-record ring and are still dumped
at shutdown. Set `TYPHON_WAYLAND_COMPAT_TRACE=1` to emit records live; only the
exact value `1` enables it, and live output is capped at 64 records per
compositor run with one limit notice. The gate is available in release builds
and changes logging only. Each record includes a monotonic timestamp, Wayland
client identity, protocol object id, interface, surface id, violation, and
recovery action. Counters are separate for surface teardown, XDG role teardown,
and active clipboard source reuse. No client name changes the recovery rules.
Client disconnect cleanup removes any retained XDG base ownership entry for an
inert surface, along with the rest of that client's resources.

## Native-client qualification status

**Not run on 2026-10-02.** The active seat is a Wayland session on `tty1`, and
Hyprland (PID 2150) owns `/dev/dri/card1`; `/sys/class/tty/tty0/active` reports
`tty1`. The other logged-in session is an inactive text session on `tty3`, and
`/dev/tty2` is root-owned with mode `0600`. Starting Typhon through its native
TTY/DRM launcher would require taking over the active seat, so it was not
started. Firefox was not launched, `TYPHON_WAYLAND_COMPAT_TRACE=1` was not set
for a compositor process, and there is no native Firefox trace or survival
result to claim.

The focused lifecycle tests are synthetic wire tests against Typhon's in-process
test compositor. They are separate evidence from native-client or
cross-compositor runs. No native Firefox, GTK, or other external Wayland client
was exercised in this qualification session.

## Why these cases are recoverable

The surface and XDG cases have a single owning resource and an existing
canonical cleanup path. Continuing after that cleanup does not preserve a
partially live scene object, pending buffer, or role association. A late role
resource cannot affect a later role because requests are matched against the
currently registered resource identity; missing or stale role resources are
inert.

Clipboard repetition is narrower. It is accepted only when the submitted
source belongs to the calling client, is alive, has at least one MIME type,
has not had DnD actions configured, is marked as a selection source, and is
still the active clipboard selection. A source that has been replaced or
cleared is retired. DND remains separate because a drag source participates in
an active input-grab and action-negotiation lifecycle; treating it like
clipboard selection would allow the same source to be attached to multiple drag
sessions.

## Protocol and compositor comparison

The protocol baseline remains strict: destroying `wl_surface` before its role
is the defined `wl_surface.defunct_role_object` protocol error; destroying
`xdg_surface` before its role and destroying a non-topmost nested popup are
also specified protocol errors. Attempting to reuse a previously-used source
may send `wl_data_device.used_source`. Typhon deliberately recovers only the
narrow case where the same source is still the active clipboard selection.
The protocol-defined violation describes the request's validity; it does not
require every compositor to apply the same fatal or recovery policy. Typhon's
choice is safe because the active selection and source ownership remain
unambiguous. ([Wayland core](https://wayland.app/protocols/wayland), [xdg-shell](https://wayland.app/protocols/xdg-shell))

| Implementation | Source observation | Classification |
| --- | --- | --- |
| KWin | Warns if `xdg_surface.destroy` arrives before its role object, then destroys the XDG surface resource; its destructor deletes associated toplevel and popup objects. ([source](https://github.com/KDE/kwin/blob/master/src/wayland/xdgshell.cpp)) | `Interop convention` for this XDG ordering case; the protocol still requires role-first destruction. |
| Hyprland | Listens for the underlying surface's destruction, warns, unmaps the role, drops the surface link, and emits role destruction. ([source](https://github.com/hyprwm/Hyprland/blob/main/src/protocols/XDGShell.cpp)) | `Compositor-specific behavior` that supports recovering `wl_surface`-first XDG teardown. |
| wlroots | Its XDG surface implementation exposes explicit role-reset and destroy cleanup paths. This is a library mechanism; frontend policy determines whether a request is warned about or rejected. ([source](https://github.com/swaywm/wlroots/blob/master/types/xdg_shell/wlr_xdg_surface.c)) | `Compositor-specific behavior`; the source inspected does not establish one universal policy. |
| Mutter | The inspected implementation has explicit XDG reset/destructor paths and fatal checks for invalid or already-destroyed surface state. The inspected source did not establish equivalent recovery for these exact destroy orderings. ([source](https://github.com/GNOME/mutter/blob/main/src/wayland/meta-wayland-xdg-shell.c)) | `Unknown` for these exact lifecycle sequences; no recovery claim is made. |
| Smithay | The inspected selection code separates clipboard selection and DnD machinery, but did not establish an exception matching Typhon's active-source predicate. ([data-device source](https://docs.rs/smithay/0.7.0/src/smithay/wayland/selection/data_device/mod.rs.html)) | `Unknown` for identical source-reuse behavior; no broader reuse policy is inferred. |

These were source comparisons, not runtime controls. Hyprland and KWin were not
started in this session, and no Firefox request trace was available to compare
against them. In particular, KWin and Hyprland's observed recovery behavior is
an interoperability convention, not a reason to relax unrelated validation.

Typhon therefore recovers only lifecycle orderings for which it can perform
complete, deterministic cleanup and retain unambiguous ownership. It does not
adopt broader source-reuse behavior that the inspected sources did not prove.
