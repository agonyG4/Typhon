# Rare Resize Interaction Latch: Investigation and Fix Design

## Scope

Investigate the rare state where a native move/resize interaction remains globally active after the physical trigger is no longer held, then fix only the lifecycle boundary proven by deterministic fault-injection evidence. Independently preserve the visible geometry of an active resize when entering fullscreen or maximized mode.

The completed cursor override and exact-target interaction motion work remain outside the change surface. Sober frame pacing, configure pacing, pointer-lock policy, protocol semantics, and arbitrary interaction timeouts are non-goals.

## Architecture

`NativeInputState` owns only physical input truth: modifier state, cursor state, pointer constraints, and a deduplicated set of physically pressed pointer buttons. It never owns `WindowInteraction`.

`CompositorState` remains the sole owner of `WindowInteraction`. It exposes a read-only snapshot for diagnostics, records reasoned begin decisions and explicit cleanup reasons, and keeps interaction lifetime independent of resize configure ACK/commit completion.

The input lifecycle is observable as:

```text
hardware button
  -> NativeInputState physical state update
  -> binding match and NativeInputEffect
  -> compositor effect application
  -> compositor interaction begin/end
  -> resize protocol flow (independent)
```

`TYPHON_RESIZE_DEBUG=1` enables lazy, single-line records beginning with `typhon resize:`. Normal sessions do not format or emit these records.

## Investigation gate

Instrumentation is added before recovery behavior. Deterministic tests model consumed/missing trigger releases, session suspend/discard, surface destruction, pointer constraint transitions, delayed configure flow, and missing ACK/commit. The confirmed root cause is selected from the observed boundary; no speculative cleanup is enabled.

## Lifecycle fix

If physical state reports the trigger released while the compositor effect lacks a release, reconciliation ends the interaction with `TriggerButtonNoLongerHeld`. Reconciliation runs after each hardware event and at the end of an input batch, plus after resume/discard, but never before the begin action for a consumed press is applied. If the evidence instead identifies suspend, button identity conversion, or pointer-lock transition as the source, only that source fix is implemented and the irrelevant reconciliation regression is replaced by the confirmed-boundary regression.

Session suspension explicitly cancels active interactions and clears physical pointer state. Surface destruction, focus loss, pointer constraint transitions, and mode transitions use explicit cleanup reasons. No inactivity, ACK, commit, frame-delay, or configure timeout is introduced.

## Mode transition

Before fullscreen or maximize cleanup, capture `current_visual_root_window_geometry(surface_id)` and fall back to committed geometry only when no visual preview exists. Clear resize state with `ModeTransition`, then store the captured floating geometry as restore geometry before applying the target mode. Existing absolute fullscreen/maximized placement remains unchanged, and ordinary transitions without a preview retain their current behavior.

## Verification

Red regressions precede production changes for each behavior. Focused lifecycle, resize-flow, session, and fullscreen/maximize tests are followed by the full matrix: formatting, all-target checking, clippy with warnings denied, all tests, source-layout validation, and diff whitespace validation. Manual native reproduction is reported separately and honestly; deterministic tests are not presented as manual reproduction.
