# Cursor feedback follows the frozen KMS owner

## Problem

The composited and Direct Scanout render paths currently put a visible hardware
client cursor's presentation key in the primary FrameBatch. A KMS worker may
replace that embedded cursor with a queued cursor sidecar after the primary
batch has been rendered and before submission. The primary batch then owns
feedback for content that was not physically displayed.

## Invariant

Ordinary composited/direct surfaces and software cursor content remain members
of the primary FrameBatch. Hardware cursor feedback is a separate,
presentation-only obligation. Exactly one active output transaction owns that
obligation, and ownership is rebound to the cursor transaction in the
submitted `KmsBundleOwners` when a sidecar replaces the embedded cursor.

The submitted bundle is authoritative at pageflip. The current cursor state is
never consulted to identify the cursor that was presented.

## Ownership transitions

- An embedded hardware cursor captures its exact
  `SurfacePresentationCommitKey` feedback into the primary transaction.
- A sidecar captures feedback before it is offered to the worker. This protects
  it from compositor supersession while it can still be physically displayed.
- A sidecar with the same exact presentation key transfers the existing batch;
  motion-only native replacement therefore keeps the feedback live.
- A sidecar with a different key settles the old transaction through the normal
  restore/requeue helper, which discards feedback when the old content is no
  longer current, and the new sidecar owns its own batch.
- When a worker submission freezes a sidecar, the primary obligation is
  transferred to that sidecar for the same key or restored/discarded for a
  different key. The ordinary primary scene membership is unchanged.
- Submitted ownership is immutable. Primary and cursor-sidecar completion use
  one `FramePresentation` produced by the physical pageflip.
- Sidecar return, rejection, worker fatal/recovery, output teardown, and
  generation cleanup settle the presentation-only obligation independently of
  callbacks, FIFO, Commit Timing, frame batches, and buffer-release state.

## Ledger shape

`PresentationFeedbackBatchId` remains the only payload of the
presentation-only obligation. The ledger gains narrow transfer/detach helpers
with exact-key checks so a batch cannot have two native owners or be silently
lost. Primary transactions may carry the obligation; compatibility-immediate
transactions may not.

## Flags

Cursor-plane feedback uses the existing pageflip presentation mode, timestamp,
sequence, clock, and async/vsync evidence. It is not marked `ZeroCopy` merely
because the cursor was placed on a hardware plane.

## Verification

Tests cover composited and Direct Scanout worker replacement, sidecar promotion,
motion-only and content-changing replacement, same-buffer commit identity,
submitted-owner immutability, independent PlaneDelta, rejection/return/fatal
cleanup, teardown, and callback/FIFO/Commit Timing non-interference.
