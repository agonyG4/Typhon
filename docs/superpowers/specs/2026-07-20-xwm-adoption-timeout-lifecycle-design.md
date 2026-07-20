# XWM Adoption Timeout Lifecycle Design

## Problem

Steam's CEF main window can remain between X11 mapping and Wayland surface readiness for more than five seconds. The XWM adoption deadline currently calls `mark_unmapped` for any unready record. That fabricates an unmap which did not occur on X11, clears `map_requested`, `mapped_notified`, and `buffer_ready`, and prevents a later association or buffer from producing `WindowReady`. Native evidence shows Steam's main window remains `IsViewable` while Typhon omits it from `_NET_CLIENT_LIST`; a later `xmessage` completes immediately.

## Selected Approach

Deadline expiration removes only the bounded adoption-tracker entry and emits a diagnostic containing the wait reason and XID. It must not cancel property discovery or mutate window lifecycle. `UnmapNotify` and `DestroyNotify` remain the only X11 events that withdraw or destroy a live window record.

Alternatives rejected:

- Increasing the timeout moves the failure threshold without correcting ownership of lifecycle state.
- Querying window attributes at expiration introduces another asynchronous reply and can race with real X11 events.

## Recovery Flow

1. A slow window exceeds the adoption diagnostic deadline.
2. The tracker removes its deadline entry and logs the expired gate.
3. The XWM record retains map, property, association, and buffer state.
4. A later serial association or buffer-ready event re-enters existing readiness evaluation.
5. `WindowReady` and normal EWMH publication occur when all real gates complete.

## Testing and Verification

- Add a regression with a mapped, property-ready record whose adoption deadline expires before association/buffer readiness; assert its lifecycle and map flags are unchanged.
- Complete association and buffer readiness after expiration and assert `WindowReady` is emitted.
- Run focused XWM tests, the native reactor test, all project checks, the full suite, and a release build.
- After restart, launch Steam and verify its normal window is `IsViewable`, present in `_NET_CLIENT_LIST`, and recorded by `xwayland_window_admitted`.
