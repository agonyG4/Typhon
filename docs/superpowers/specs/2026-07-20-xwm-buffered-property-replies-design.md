# XWM Buffered Property Reply Drain Design

## Problem

Steam creates a burst of X11 windows and property traffic. The XWM event drain samples raw socket readability before polling X events and only polls property replies when that sample is true. `x11rb` may read property replies into its internal buffer while `poll_for_event` drains X events. At that point the raw socket can be empty even though replies are available through `reply_unchecked`. Those replies are skipped indefinitely, window properties never become ready, map authorization is never emitted, and the adoption timeout withdraws the window. Once triggered, later clients such as `xmessage` fail at the same gate.

## Selected Approach

Within one reactor dispatch, alternate the existing bounded X-event drain and nonblocking property-reply poll until neither makes progress or each reaches its existing budget. `reply_unchecked` already returns `WouldBlock` when a reply is unavailable, so raw socket readability is neither necessary nor a correct proxy for packets buffered inside `x11rb`. Alternation is required because event polling can move more reply packets from the stream reader into X11RB's reply map after an earlier reply poll.

Alternatives rejected:

- Adding another reactor source or timer for property replies duplicates the existing XWM source and increases lifecycle complexity.
- Increasing the adoption timeout only delays failure and does not recover buffered replies.

## Data Flow

1. The XWM reactor reports readiness.
2. `drain_events` alternates X-event parsing and property-reply polling while either makes progress.
3. Each side retains its independent existing budget.
4. Available replies, including replies already buffered by `x11rb`, complete property discovery.
5. Unavailable replies return `WouldBlock` and remain pending without blocking; a no-progress pass ends the dispatch.
6. Existing readiness logic emits map authorization and later `WindowReady` when association and buffer gates are complete.

## Scope and Safety

The change is confined to XWM draining. It does not alter X11 properties, association rules, buffer readiness, adoption deadlines, or command execution. Work remains bounded by the current per-drain budget.

## Verification

- Add a regression that places an X event and property replies on the connection, drains the event so replies can reside in the client buffer, and verifies property discovery completes even when the raw socket no longer reports input.
- Run focused XWM and native reactor tests.
- Run formatting, all-target checks, clippy with warnings denied, the full locked test suite, source-layout validation, and diff validation.
- Build the release binary, restart Typhon, then verify Steam is `IsViewable`, admitted in Typhon logs, and listed in `_NET_CLIENT_LIST`.
