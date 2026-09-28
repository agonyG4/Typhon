# F11-C1 Cross-Layer DND Closure

## Goal

Close the C1 semantic contract between the canonical compositor drag state and the future XWM XDND adapter. Preserve every required edge in order, allow only exact-identity continuous updates to coalesce, support non-terminal X11 target changes, and ensure cancellation ends Wayland source events with `cancelled`.

## Current problem

`CompositorState` currently stores one `Option<XwaylandDndTransition>`. A target switch calls `leave_drag_target()` and then publishes `TargetEntered`; the second assignment replaces `TargetLeft` before any consumer can drain it. Positions and feedback also replace earlier transitions. The XWM `DndManager` has no operation for leaving a target while retaining its canonical session. The shared cancellation path emits `wl_data_source.cancelled` before `leave_drag_target()` may emit `target(NULL)`.

## Design

Replace the single slot with `XwaylandDndOutbox`, backed by `VecDeque<XwaylandDndTransition>` and a named capacity of 64. The compositor exposes a bounded sequence-drain API. Adjacent `TargetPositioned` events may replace one another only for the same exact canonical session and X11 target. Adjacent `SourceFeedback` events may replace one another only for the same exact offer ID. Edge and terminal variants never coalesce. The outbox carries its exact session, target, offer, and generation identities in each transition.

When a required transition cannot be admitted after safe coalescing, the producer fails closed for that transition's exact canonical session: use the shared cancellation path to reconcile the current target and source, discard unusable pending adapter history, and publish one `Retired` transition for that session and generation. This prevents the canonical drag from continuing while the future adapter lacks a required edge.

Add `DndManager::leave_target(adapter_id, exact_target)`. It succeeds only for the exact active adapter session and target, clears target-local position/action/drop progress, returns to `AwaitingEnter`, and preserves the adapter ID, source identity, and unused terminal authority.

The shared cancellation path captures source/session/target identities first, leaves or withdraws the target (including `target(NULL)` when required), records retirement, then sends `cancelled` exactly once and completes the canonical session. No source event follows `cancelled` for that drag.

Generation teardown removes only queued transitions that name the retired generation. It preserves unrelated Wayland state and replacement-generation transitions.

## Alternatives

Separate queues for edges and continuous updates would bound storage but require a merge mechanism to restore semantic order. A larger unbounded queue preserves order but has no structural memory limit. The single bounded `VecDeque` is selected because it keeps order explicit and makes coalescing local and reviewable.

## Validation

Regression coverage will prove same-call X11 A-to-B ordering, enter-before-position, bounded position coalescing with latest coordinates, exact-session overflow retirement, source feedback offer identity and terminal preservation, adapter leave/re-enter identity, generation cleanup, and exact cancellation event order for X11 and native Wayland targets. No live XDND ClientMessages or end-to-end interoperability behavior is part of C1.

## Milestone boundary

This work closes F11-C1. F11-C2, F11-C3, F11-C4, and F11-D remain not started.
