# Typhon Native Input Pre-Read Freshness Closure v1

## Problem

The current post-routing-transition latency guard observes native input within microseconds, but a Wayland-only reactor snapshot can still read client requests after native input becomes readable and before native input is dispatched. A relative-pointer / pointer-constraint topology mutation can therefore precede the semantic epoch that owns already-serviceable physical input. The input is real and must remain exact; the defect is the stale ownership decision at the client-read boundary.

## Design

Keep the existing native semantic epochs, explicit `begin_semantic_epoch()` / `drain_epoch_chunk_into()` ingress boundary, non-consuming epoll input probe, Native Wake Authority, and post-transition latency guard. Add one late freshness arbitration point inside `NativeRuntime::dispatch_wayland_and_input()`, after the existing pre-epoch constraint settlement and cursor synchronization and immediately before the Wayland-only read branch.

The gate is entered only when `dispatch_wayland` is true and the current turn does not already own input. It performs exactly one `input_ready_nonblocking()` probe. A healthy `EPOLLIN` input registration promotes the turn to `service_input = true`, which reuses the existing native input epoch and then allows the originally requested Wayland read after that epoch completes. A negative result reads Wayland immediately. Combined input + Wayland readiness and input-only turns do not probe because their existing input epoch is already the arbitration snapshot. Input arriving after the probe belongs to the next epoch.

The promoted epoch remains bounded by `NATIVE_INPUT_DRAIN_BUDGET`; a retained libinput queue continues without a second dispatch or a Wayland read until exhausted. Raw evdev follows the same high-level ownership rule with its existing bounded reads. No polling loop, timer, sleep, unconditional libinput dispatch, thread, timestamp filtering, or motion transformation is introduced.

## Timing observability

The observer remains disabled with no clock read, formatting, allocation, or output. Enabled timing keeps a fixed-capacity record ring and emits at most one compact summary per completed transition. It records the real routing-commit boundary, actual input-service attempts, the libinput dispatch and queue-drain boundaries, and the late pre-read decision. Input-service duration is per attempt; an empty attempt cannot make a later attempt appear to block for the interval between them. A transition summary may include one bounded pre-read batch observation so the batch consumed before a new topology mutation is distinguishable from the first post-transition batch.

## Testing

Use strict RED -> GREEN. The primary regression exercises the production pre-read decision used by `dispatch_wayland_and_input()` and pairs it with existing real `OwnCompositorServer` relative-pointer / pointer-constraint resources. The matrix covers late promotion, unavailable input, combined readiness, input-only, continuous-input fairness, >256 continuation, raw evdev, transition-guard and anchor non-regression, input-peek non-consumption, and timing observer semantics. No test encodes a wall-clock delay, delta threshold, or application-specific behavior.

## Reference comparison

Current KWin separates libinput acquisition and higher-level processing with a dedicated connection thread and explicit queue, while Aquamarine/Hyprland, wlroots, and Weston keep fd readiness directly coupled to a short input dispatch turn. Typhon adopts the useful explicit snapshot and readiness-authority properties without copying KWin's thread or changing accepted semantic-epoch ownership.

## Non-goals

Do not change pointer anchor resolution, relative-motion arithmetic, coalescing, constraint semantics, rendering/KMS ownership, or Native Wake Authority. Do not claim application-level Sober closure until the user performs the native runtime qualification.
