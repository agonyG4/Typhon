# XWM Buffered Property Reply Drain Design

## Problem

Steam creates a burst of X11 windows and property traffic. The XWM event drain samples raw socket readability before polling X events and only polls property replies when that sample is true. `x11rb` may read property replies into its internal buffer while `poll_for_event` drains X events. At that point the raw socket can be empty even though replies are available through `reply_unchecked`. Those replies are skipped indefinitely, window properties never become ready, map authorization is never emitted, and the adoption timeout withdraws the window. Once triggered, later clients such as `xmessage` fail at the same gate.

## Selected Approach

After draining X events, always invoke the existing nonblocking property-reply poll with the existing bounded budget. `reply_unchecked` already returns `WouldBlock` when a reply is unavailable, so raw socket readability is neither necessary nor a correct proxy for replies buffered inside `x11rb`.

Alternatives rejected:

- Adding another reactor source or timer for property replies duplicates the existing XWM source and increases lifecycle complexity.
- Increasing the adoption timeout only delays failure and does not recover buffered replies.

## Data Flow

1. The XWM reactor reports readiness.
2. `drain_events` drains at most the existing X event budget.
3. `poll_replies` attempts at most the same bounded reply budget regardless of raw-fd state.
4. Available replies, including replies already buffered by `x11rb`, complete property discovery.
5. Unavailable replies return `WouldBlock` and remain pending without blocking.
6. Existing readiness logic emits map authorization and later `WindowReady` when association and buffer gates are complete.

## Scope and Safety

The change is confined to XWM draining. It does not alter X11 properties, association rules, buffer readiness, adoption deadlines, or command execution. Work remains bounded by the current per-drain budget.

## Verification

- Add a regression that places an X event and property replies on the connection, drains the event so replies can reside in the client buffer, and verifies property discovery completes even when the raw socket no longer reports input.
- Run focused XWM and native reactor tests.
- Run formatting, all-target checks, clippy with warnings denied, the full locked test suite, source-layout validation, and diff validation.
- Build the release binary, restart Typhon, then verify Steam is `IsViewable`, admitted in Typhon logs, and listed in `_NET_CLIENT_LIST`.
