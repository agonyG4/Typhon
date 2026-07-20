# XWM Authoritative MapNotify Design

## Problem

Steam CEF can expose a managed X11 window as already viewable while Typhon's
XWM has no recorded `MapRequest` for it. When the authoritative `MapNotify`
arrives, `confirm_external_map_notify` accepts that ordering only for
override-redirect windows. A managed window instead gets converted back into
a pending map request after its `MapNotify` has already been consumed.

Mapping an already-mapped X11 window does not produce a second notification,
so `mapped_notified` remains false forever. The window never emits
`WindowReady` and remains absent from `_NET_CLIENT_LIST` even though
`xwininfo` reports it as viewable.

## Design

Treat every valid, non-pending `MapNotify` as authoritative mapped state for
both managed and override-redirect windows. The registry transition sets
`map_requested`, `map_authorized`, and `mapped_notified`, clears
`map_operation_pending`, and derives the lifecycle from the existing surface
association. Event normalization then refreshes properties when needed and
attempts readiness emission without issuing a redundant map request.

Keep this state transition in `X11WindowRegistry`, which owns X11 mapping
lifecycle. Add a structured `xwm_map_notify` diagnostic for the external-map
path so a live XID can be correlated with readiness and admission.

A real `UnmapNotify` remains the withdrawal boundary. Existing unmap handling
continues to clear mapping and retained-buffer state, so this change cannot
reuse stale content during a later remap.

## Alternatives

Special-casing managed windows in `events.rs` would duplicate registry state
transitions and leave two definitions of mapped state. Querying X attributes
or issuing another map request would add a round trip and still depend on an
event the X server is not required to repeat for an already-mapped window.

## Verification

Add an event-level regression that observes a managed window, completes its
properties, association, and buffer readiness without recording a map
request, then delivers `MapNotify`. Require one `WindowReady`, no
`WindowMapRequested`, and mapped registry state.

Run the focused red-green test, all XWM tests, the native Xwayland reactor
test, formatting, compilation, Clippy, the complete test suite, source layout
validation, diff validation, and a release build. After restart, verify that
Steam's exact persistent main XID is viewable, appears in `_NET_CLIENT_LIST`,
and has matching `xwm_map_notify` and `xwayland_window_admitted` diagnostics.
