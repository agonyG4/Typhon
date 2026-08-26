# Typhon Atomic Cursor Liveness v1 Design

**Date:** 2026-08-26

**Scope:** Restore liveness for the atomic hardware cursor after pointer-protocol quiescence without coupling hardware cursor motion to primary-scene repaint.

## Problem

`NativeAtomicCursor` records pointer movement in desired state, but the current native input path intentionally suppresses primary repaint for hardware cursors. Cursor arbitration is normally discovered during presentation planning, so an input-only wake can leave the desired cursor epoch unarmed with no runtime deadline. Later unrelated presentation work then publishes the newest position as a visible teleport.

## Design

The atomic cursor owns an O(1) output-debt predicate. It compares the desired KMS-visible state with the state already committed to become output state, in this order:

1. worker-queued cursor state;
2. submitted state while a cursor pageflip is pending;
3. current presented state.

The comparison reuses `AtomicCursorVisualState::kms_equivalent()`, so hidden-to-hidden movement is not output debt while visible movement, image changes, and visibility transitions remain debt.

`NativeCursorOutputArbitration` gains independent tracking for hardware cursor debt. The native input completion boundary calls a single observer after final cursor synchronization. When debt exists, the observer requests the existing arbitration with the latest desired epoch and a refresh-derived deadline from `NativeFrameScheduler`. When debt disappears, it clears only hardware debt; software-overlay work remains intact.

The observer does not inspect scene state, dispatch Wayland, reconcile compositor state, select cursor delivery policy, or submit KMS. Existing presentation and plane-policy code continues to choose primary piggyback, cursor-only plane delta, worker ownership, and software fallback.

## Integration point

`dispatch_wayland_and_input()` observes the atomic cursor once after the batch-end pointer-constraint processing and final `synchronize_cursor_state_for_server()` call, before ending the native input batch. This gives coalesced input and cursor-source synchronization one final state boundary while leaving `cycle.redraw_requested` unchanged for hardware-only motion.

## Tests

Tests will cover:

- the pre-fix starvation model and post-fix input-side arbitration request;
- visible movement, hidden movement, show/hide, and cursor visual changes;
- A→B→A before submission and A/current→B in-flight→A desired-state debt;
- stable first deadlines and latest-epoch coalescing;
- cursor-only maturity and primary piggyback;
- software and legacy cursor paths;
- floating/tiled interaction preservation through existing regressions;
- allocation-free, non-primary input-path behavior through the narrow observer interface.

## Constraints

- Preserve all unrelated dirty work in the current branch.
- Do not create a second cursor scheduler.
- Do not restore per-sample primary-scene repaint.
- Do not submit atomic KMS state from the raw input handler.
- Do not change pageflip, worker, transaction, direct-scanout, or cursor-delivery ownership.
- Keep surface pacing, eager debug formatting, shortcut inhibition, and narrow key/button full-tick behavior as separate follow-ups.
