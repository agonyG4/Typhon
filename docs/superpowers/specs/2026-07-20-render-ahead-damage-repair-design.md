# Render-Ahead Damage Repair Design

## Problem

Steam move and resize traces show one coherent X11 window geometry while the
native output displays several stale copies. The defect is therefore below XWM
geometry and compositor window policy, in reuse of native output buffers.

Typhon's predictive triple-buffer path may render a ready frame while an older
frame is still awaiting page-flip completion. After each render, runtime state
advances `last_renderable_surfaces` to the newly rendered geometry. However,
`PartialRepaintPlanner` deliberately commits damage history only after confirmed
presentation. A second render-ahead frame therefore computes current damage
from the first candidate's geometry without having the first candidate's damage
in the presented journal. When the third output buffer is reused, old window
pixels outside the second frame's damage survive and later appear as duplicate
windows.

## Reference Model

KWin's DRM path accumulates the acquired buffer's age through its damage journal
and falls back to an infinite/full region when the journal cannot cover that
age. Hyprland similarly asks its monitor damage tracker for all damage required
by the acquired swapchain buffer before drawing. Both treat incomplete buffer
history as a correctness boundary, not as permission for a partial repaint.

## Design

For the explicit atomic output path, report buffer age zero to the repaint
planner whenever a composited render begins while another output frame is
pending presentation. `BufferAge::Value(0)` already means that the acquired
buffer's contents cannot be repaired from authoritative presented history and
selects the existing full-repaint path.

Add a pure helper beside `software_buffer_age`:

```rust
pub(crate) fn render_target_buffer_age(
    presentation_serial: u64,
    last_presented_serial: Option<u64>,
    presentation_pending: bool,
) -> BufferAge
```

It returns `BufferAge::Value(0)` when `presentation_pending` is true and
otherwise delegates to `software_buffer_age`. `AtomicOutputSlot::buffer_age`
will accept the pending flag, and `AtomicEglGbmScanout::render_to_slot` will
derive it from `swapchain.pending_slot().is_some()` before drawing.

This does not alter swapchain ownership, presentation ordering, X11 geometry,
surface damage, or scheduler decisions. It only makes repaint planning
conservative during the interval where presented-only history is necessarily
behind rendered geometry.

## Trade-off

Render-ahead frames use a full-output repaint rather than partial scissoring.
Normal double-buffered frames and predictive frames rendered with no pending
page flip retain buffer-age partial repaint. Correct candidate-aware partial
damage can be added later as a separate ownership design; it is not required to
remove stale content safely.

## Verification

- A unit regression proves a reusable slot with ordinary age greater than one
  is normalized to age zero while presentation is pending.
- Existing software-age and partial-repaint planner tests remain unchanged.
- Atomic swapchain, native output damage, frame scheduling, and renderer suites
  remain green.
- Full repository gates pass and a clean release is built.
- Native Steam validation repeats rapid move and all-edge resize under adaptive
  triple buffering and shows no stale copies.
