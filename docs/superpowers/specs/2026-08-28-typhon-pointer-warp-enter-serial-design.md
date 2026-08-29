# Pointer-Warp Enter-Serial Authority Design

## Goal

Keep a current `wl_pointer.enter` serial valid for `wp_pointer_warp_v1` until
that pointer actually leaves focus or its resource is destroyed, regardless of
bounded generic input-serial history.

## Architecture

`pointer_enter_serials` is the authoritative per-pointer current-enter state.
The existing `pointer_has_current_enter_serial` helper remains unchanged for
`wl_pointer.set_cursor`, whose exact-surface semantics are intentional. The
pointer-warp path gets a dedicated validator that checks the live enter record,
pointer ownership, and same-client target-surface semantics without consulting
`recent_input_serials` or requiring the enter surface to equal the target.

The existing active-lock warp guards remain unchanged. Lock teardown continues
to own authorized absolute restoration.

## Validation and lifecycle

The warp request still rejects dead or unknown resources, wrong-client pointer
resources, missing focus, non-finite/out-of-surface coordinates, and stale
pointer-enter authority. A current enter record is replaced on a new enter and
removed on pointer leave, focus teardown, or pointer destruction. Generic
keyboard/button/other input serials do not alter it.

## Tests

Protocol tests will exercise real enter and unrelated button input, then warp
with the original serial. Existing stale-focus coverage remains. Additional
tests cover repeated lock teardown cycles, same-client target-surface use, and
wrong-client rejection. The prior active-lock warp tests remain in place.
