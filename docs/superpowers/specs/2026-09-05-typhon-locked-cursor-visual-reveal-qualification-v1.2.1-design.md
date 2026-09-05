# Typhon Locked Cursor Visual Reveal Qualification v1.2.1 Design

**Starting HEAD:** `798f5ef62aea6f23d41a356db5b55e146b561c9d`

## Goal

Close the remaining Gate 1 observability hole where an already-captured worker cursor reveal snapshot is lost when the cursor update cannot be attached as a sidecar and is prepared as an independent cursor-only submission.

## Scope

The correction is diagnostic ownership propagation only. It does not alter cursor state, geometry, visibility, scheduling, sidecar selection, worker admission, KMS ordering, pageflip handling, or pointer-lock behavior. The visual cursor teleport fix remains outside scope.

## Design

`prepare_plane_delta()` already receives the frozen `Option<CursorRevealTraceSnapshot>` and already forwards it for promoted sidecars. The independent `PlaneDeltaPreparationSubmit` branch will carry that same value unchanged instead of assigning `None`. The downstream existing worker path already places the preparation value into `KmsBundleOwners`, so no new state machine, lookup, or snapshot construction is needed.

Transport selection remains ownership-neutral:

```text
frozen snapshot R
    -> sidecar offer succeeds       -> CursorSidecar.trace_reveal = R
    -> sidecar offer returns None   -> PlaneDeltaPreparationSubmit.cursor_reveal_trace = R
```

An absent snapshot remains absent through preparation, worker ownership, physical binding, and pageflip tracing. Trace-disabled behavior therefore remains neutral by construction.

## Verification

Add deterministic coverage at the existing plane-cycle test seam for independent preparation retaining `Some(R)`, while preserving existing sidecar and promotion tests. Exercise the focused worker path and full first-visible trace path where existing test hooks permit; assert the exact snapshot is retained, absent snapshots remain absent, and disabled tracing does not manufacture ownership. Run the complete required Rust formatting, check, clippy, test, and diff checks.

## Reporting

Update the existing English v1.2 report to document that both worker transport choices retain one frozen reveal ownership unit, including the first hidden-to-visible worker cursor-only pageflip. Explicitly state that no Gate 2 semantic cursor correction was implemented.
