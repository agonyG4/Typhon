# X11 Initial Focus Design

## Problem

Steam Settings and Friends now remain mapped without terminating Xwayland, but Steam publishes them as independent `_NET_WM_WINDOW_TYPE_NORMAL` windows without `WM_TRANSIENT_FOR`, `_NET_WM_USER_TIME`, or `_NET_STARTUP_ID`. Typhon inserts them into stacking but does not focus or raise newly admitted X11 toplevels. The already-active main Steam window can therefore remain above a dialog whose geometry is completely inside the main window, making the dialog appear not to open.

Typhon already focuses newly registered XDG toplevels. Hyprland likewise gives a newly mapped focusable window initial focus by default. KWin applies a richer user-time/application-group policy, but Steam omits the metadata needed to reproduce that policy reliably here.

## Decision

After a `WindowReady` snapshot is admitted and its retained surface content is published, call the existing `focus_desktop_window(window_id)` path. That path already rejects auxiliary X11 roles and, for normal/dialog roles, updates compositor focus and raises the root window.

Do not focus auxiliary popup menus, notifications, override-redirect windows, or auxiliary support windows. Do not add Steam-specific matching or weaken existing activation-request validation.

The existing backend activation queue will translate the focus change into XWM `Focus` commands during the same native reactor cycle. Client-list synchronization will then publish stacking with the new normal window above the prior active window.

## Testing

A server-level regression will admit two renderable Steam-style normal X11 windows in sequence. It must prove the second window becomes the focused window, is last in stacking order, and produces backend focus commands that deactivate the first window and activate the second. A companion auxiliary-role test must prove popup menus remain non-focusable.

Focused compositor/XWM suites, strict Clippy, the full serial test gate, a fresh release, and live Settings/Friends validation are required.
