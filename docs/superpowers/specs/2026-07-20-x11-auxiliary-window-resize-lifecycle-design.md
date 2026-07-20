# X11 Auxiliary Window and Resize Lifecycle Design

## Problem

Live Steam testing exposes three related X11 desktop-lifecycle defects.

First, Steam maps a 10x10 client-leader support window. It is a self-led
window with a separate `_NET_WM_USER_TIME_WINDOW`, no declared window type,
and no `WM_HINTS` input model. Typhon currently admits it as a normal desktop
window, producing the clickable black cube at the top-left.

Second, interactive resize uses the generic visual preview for X11 windows,
but only the XDG configure/commit path clears that preview. An X11 resize
therefore leaves `active_toplevel_resizes`, the visual geometry, and its clip
active after button release. Later move and resize operations use that stale
preview and appear as clipped, offset, or duplicated windows.

Third, `x11_client_lists` exports every X11 desktop record, including
override-redirect popup and helper windows. Live root properties consequently
advertise short-lived Steam and Proton helpers as normal clients.

## Design

### Auxiliary client-leader classification

Extend XWM property collection with `WM_CLIENT_LEADER` and
`_NET_WM_USER_TIME_WINDOW`. Classify a window as an auxiliary client-leader
support window only when all of these conditions hold:

- it is managed rather than override-redirect;
- both dimensions are at most 16 pixels;
- `WM_CLIENT_LEADER` points to the window itself;
- `_NET_WM_USER_TIME_WINDOW` points to a distinct window;
- `_NET_WM_WINDOW_TYPE` is absent; and
- `WM_HINTS` does not declare an input model.

When the complete property snapshot matches this signature, keep the X11
window mapped and associated but do not emit `WindowReady`. Mark its XWM
lifecycle as auxiliary so diagnostics describe the decision. The predicate
is protocol-derived and application-independent; ordinary small dialogs,
typed menus, input-capable windows, and override-redirect surfaces remain
eligible for desktop admission.

### X11 resize finalization

At the non-XDG branch of `send_resize_end_configure`, send the final backend
configure and then finalize the preview locally. Commit the final placement,
remove the matching `active_toplevel_resizes` entry, set the visual
geometry's `active_resize` to `None`, and refresh the render assignment.

X11 has no XDG configure serial to complete this state later. The final
client buffer may arrive asynchronously, but without an active preview clip
it replaces the previous buffer normally. Keeping the target visual geometry
provides the correct dimensions and placement for a subsequent move or
resize.

### EWMH client lists

Filter `_NET_CLIENT_LIST` and `_NET_CLIENT_LIST_STACKING` to
`DesktopWindowKind::Managed`. Override-redirect windows remain compositor
desktop records so their content can be rendered at X11 geometry, but they
are excluded from focus cycling and EWMH client publication.

## Alternatives

A Steam `WM_CLASS` denylist would remove the visible cube but encode an
application quirk in compositor policy. Filtering every tiny window would
hide legitimate launchers and dialogs. Clearing all visual resize state on
interaction end would break XDG's intentionally asynchronous final-configure
flow.

## Verification

Use test-driven regressions for:

- a complete Steam-shaped client-leader support record never producing
  `WindowReady`, while a normal tiny typed/input window remains eligible;
- ending an X11 resize removing the active preview and visual clip, retaining
  final geometry, and queuing one final backend configure;
- override-redirect X11 windows being absent from both EWMH client lists
  while managed stacking order remains unchanged.

Run focused red-green tests, all XWM and compositor state tests, the native
Xwayland reactor integration, formatting, all-target checks, Clippy with
warnings denied, the complete test suite, source-layout validation, diff
validation, and a release build. After restart, launch Steam and verify that
only its real managed window appears in `_NET_CLIENT_LIST`, no black support
cube is admitted, and move/resize release leaves the main window with one
normal unclipped representation.
