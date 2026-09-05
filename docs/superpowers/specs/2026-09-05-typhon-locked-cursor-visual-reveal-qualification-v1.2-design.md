# Typhon Locked Cursor Visual Reveal Qualification v1.2 Design

## Goal

Close the remaining Gate 1 trace-attribution gap by freezing reveal ownership,
cursor revision, and cursor source at the same semantic boundary as the cursor
presentation state, then carrying that immutable snapshot through render-ahead,
ready-frame, worker, sidecar, synchronous, KMS, pageflip, and presented-cursor
paths.

## Scope and invariants

- Preserve the v1.1 bounded ledger and exact physical key
  `(output_generation, crtc_id, PageFlipToken)`.
- Keep trace state observational only; no input, scheduling, visibility-policy,
  cursor-placement, KMS-ordering, or application-specific behavior changes.
- Keep Gate 1 only. No semantic cursor correction or Gate 2 teleport fix.
- Keep the existing worker and synchronous transports, sidecar replacement, and
  pageflip first-visible matching rules.
- When tracing is disabled, do not create or propagate trace-only reveal
  authority or snapshots.

## Architecture

`CursorRevealTraceSnapshot` becomes a presentation-plane data carrier so it can
be stored by `RenderedOutputFrame` without making the lower-level plane module
depend on the trace-ledger module. The snapshot is captured alongside
`FrozenPrimaryCursorPlan`: promoted `PresentedCursorState.source` is
authoritative for software/hidden primary presentation, while hardware state
captures the source once at freeze time. `RenderedOutputFrame` exposes the
snapshot to the ready-frame boundary; worker queue extraction moves it into
`KmsBundleOwners`, preferring the cursor owner when present and the primary
owner otherwise. Sidecar replacement continues to replace the cursor state and
its snapshot together.

Immediate cursor-only and direct-worker paths capture their snapshot before
their physical submission boundary and pass it forward. Token binding only
adds the physical identity to that already-frozen snapshot. Synchronous primary
KMS evidence receives the frozen epoch/revision from the ready frame rather than
reconstructing it from current transaction or cursor state.

The canonical KMS payload retains the exact raw `CRTC_X/Y` values and labels
them `CRTC_X_RAW/CRTC_Y_RAW`; it additionally emits
`pointer_position`, `hotspot`, and signed `plane_origin_signed`. The signed
origin is computed from the pointer position minus hotspot while raw DRM values
remain the exact signed-i64-as-u64 representation currently submitted.

## Components

1. Presentation plane and trace carrier: own the immutable snapshot type and
   make presented-state source authoritative.
2. Explicit output frame ownership: store and expose the frozen snapshot through
   render, ready, worker-queued, and synchronous submission transitions.
3. Worker ownership: retain the snapshot in primary/cursor bundle owners and
   sidecars, use it for KMS epoch/revision fields, and bind it after physical
   submission.
4. Immediate submission paths: replace late reconstruction in primary,
   cursor-only, compatibility, and direct-worker paths with freeze-time capture.
5. Compositor trace authority: gate trace-only authority creation, completion,
   terminal no-visible retirement, and trace-only work on the trace feature flag.
6. KMS observability: extend the canonical assignment vocabulary and tests
   without changing atomic request writes.
7. Qualification evidence: add focused RED/GREEN tests and update the English
   v1.1 report to v1.2 with the exact closure claims and verification results.

## Test strategy

Focused tests will cover the four overlapping A/B ownership orderings, frozen
source and revision surviving later mutation, ready-frame and worker-owner
propagation, sidecar replacement, synchronous primary and cursor-only capture,
pageflip promotion, disabled neutrality, no-visible retirement, bounded ledger
behavior, and both positive-hotspot and negative-origin KMS geometry. Existing
v1.1 tests remain regression coverage. Verification runs in the checkout's
normal local target directory through `rtk`.

## Explicit non-goals

No cursor teleport correction, cursor rescheduling, policy change, input change,
new unbounded map, second state machine, or native A/B claim is part of v1.2.
