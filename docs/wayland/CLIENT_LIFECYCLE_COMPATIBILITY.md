# Wayland Client Lifecycle Compatibility

Typhon keeps protocol validation strict where accepting a request could cross a
client boundary, make object ownership ambiguous, or leave compositor state
corrupt. A small set of ordering mistakes has an unambiguous server-owned
cleanup path, so those mistakes are diagnosed and recovered without ending the
client connection.

## Recovery boundary

| Request sequence | Classification | Recovery |
| --- | --- | --- |
| `wl_surface.destroy` while an XDG, popup, subsurface, or other role is live | Recoverable lifecycle ordering | Record a bounded diagnostic, then run `teardown_surface_resource` once. That path removes scene state, pending commits, buffers, role registrations, focus, and ownership maps. Role resources that remain on the wire become inert. |
| `xdg_surface.destroy` while its toplevel or popup object is live | Recoverable lifecycle ordering | Record a bounded diagnostic, then run `unregister_xdg_surface_role`. It retires the role and associated popup/configure/decoration state. Later role requests are ignored and their normal destroy requests are harmless. |
| Repeating `wl_data_device.set_selection` with the same source that is still the active clipboard source | Recoverable client reuse | Record a bounded diagnostic and preserve the current selection without another cancellation or generation change. |
| Reusing a source after selection clear/replacement, or using a drag source again | Fatal protocol violation | Keep `wl_data_device.used_source` fatal. Retired sources cannot be made active again. Drag-source validation remains strict. |
| Cross-client object references, invalid object references, invalid configure state, role reassignment, popup destruction out of stack order, invalid DND actions, and buffer ownership violations | Fatal protocol/state violation | Keep existing protocol errors and client termination. |

Compatibility records are held in a fixed-size ring and summarized at shutdown;
the request path does not emit an unbounded log line for each event. Counters
are separate for surface teardown, XDG role teardown, and active clipboard
source reuse. No client name or compositor-wide compatibility switch changes
the rules.

## Why these cases are recoverable

The surface and XDG cases have a single owning resource and an existing
canonical cleanup path. Continuing after that cleanup does not preserve a
partially live scene object, pending buffer, or role association. A late role
resource cannot affect a later role because requests are matched against the
currently registered resource identity; missing or stale role resources are
inert.

Clipboard repetition is narrower. It is accepted only when the submitted
resource is the source currently committed as the clipboard selection. A source
that has been replaced or cleared is retired. DND remains separate because a
drag source participates in an active input-grab and action-negotiation
lifecycle; treating it like clipboard selection would allow the same source to
be attached to multiple drag sessions.

These boundaries follow production compositor practice without generalizing it
to all invalid state. Hyprland explicitly logs and tears down a surface when it
is destroyed before its XDG role ([source](https://github.com/hyprwm/Hyprland/blob/main/src/protocols/XDGShell.cpp)).
KWin warns when an `xdg_surface` is destroyed while its role object remains and
resource destruction cleans up the role handles ([source](https://github.com/KDE/kwin/blob/master/src/wayland/xdgshell.cpp)).
Mutter likewise destroys the XDG resource and clears its client association,
while retaining fatal errors for invalid XDG state ([source](https://github.com/GNOME/mutter/blob/main/src/wayland/meta-wayland-xdg-shell.c)).
Smithay documents that real clients reuse selection sources and tracks their
use, though its data-device policy is broader than Typhon's chosen clipboard
exception ([source](https://github.com/Smithay/smithay/blob/master/src/wayland/selection/data_device/device.rs)).
Smithay's XDG shell also makes destruction callbacks explicit while noting
that implicit resource destruction order is not guaranteed ([source](https://github.com/Smithay/smithay/blob/master/src/wayland/shell/xdg/mod.rs)).

Typhon therefore recovers only lifecycle orderings for which it can perform
complete, deterministic cleanup and retain unambiguous ownership. It does not
adopt the broader source-reuse tolerance visible in some framework code.
