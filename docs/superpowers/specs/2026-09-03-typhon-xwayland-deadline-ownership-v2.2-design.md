# Typhon Native XWayland Deadline Ownership v2.2

## Status

This is a narrow correctness and observability closure after Native Frame
Pacing v2.1. It does not change the physical clock, predictor tuning, O1
claim/frontier semantics, scheduler policy, DMA-BUF release ownership, or
focus policy.

## Problem and evidence

The accepted native 1920x1080@165 Hz run retained the correct physical cadence
(about 6.060606 ms per refresh) but reported:

```text
runtime_timer_arms = 90291
stale_deadline_rearms = 38651
past_deadline_arms = 38651
stale_xwayland = 38651
past_xwayland = 38651
```

All other stale and past deadline-owner counters were zero. This is a timer
ownership correctness failure, not evidence that the physical pacing design
should be reopened.

The source call graph is:

```text
XwaylandService::next_deadline_ns()
  Running -> min(XWM resize, focus, adoption)
    -> NativeWakePlanInputs::xwayland_timeout_deadline_ns
      -> Native Wake Authority timer arm
        -> XwaylandService::handle_deadline(now)
          -> adoption expiry + resize-sync timeout
          -> focus timeout was not consumed
            -> the same T remained published and was rearmed as stale/past
```

The focus event/scene path separately called
`XwaylandService::handle_focus_deadline`. That path was opportunistic and did
not satisfy the Native Wake Authority producer/consumer invariant.

## Deadline ownership contract

For every deadline published by `XwaylandService::next_deadline_ns()`, the
same service timer entry point must consume every due XWM deadline class. The
service still owns startup, backoff, and termination deadlines. Running state
owns one XWM aggregate:

```text
Xwm::next_deadline_ns()
  = min(resize-sync, focus, adoption)

Xwm::handle_deadlines(now)
  = collect adoption expirations
  + handle resize-sync deadline
  + handle focus deadline
```

The existing bounded resize expiration loop remains the loop that owns resize
records; no new timer, eventfd, poll, sleep, clamp, suppression, or immediate
wake loop is introduced. If multiple classes are due, all existing handlers
are attempted once in the aggregate call. A resize error is retained before a
focus error so the service preserves one fatal XWM failure transition.

The event-path `handle_focus_deadline` remains available for existing callers.
It is idempotent because focus repair marks the transition before issuing the
repair and the timer path now reaches the same XWM operation.

Terminal focus conditions remain unchanged: FocusIn confirmation, unmap, and
destroy cancel the pending transition and therefore remove its deadline. A
valid timeout emits at most one repair; the next deadline query cannot return
the consumed T.

## Fast-client diagnostics

The existing fast-client admission predicate is unchanged: callback reaction
must be within the existing threshold, the callback surface must be present
and exclusive, the exact client commit and callback-admission timestamps must
exist, and callback handoff exclusion must be false. The diagnostics classify
each fast-reaction sample in a fixed first-match order:

```text
fast_candidate_seen
  = fast_candidate_qualified
  + rejected_missing_surface
  + rejected_nonexclusive_surface
  + rejected_missing_commit
  + rejected_missing_admission
  + rejected_callback_handoff
```

Only fast reactions are candidates; slow or absent reactions do not enter this
accounting. Continuity is separately classified for every qualified candidate:

```text
fast_candidate_qualified
  = continuity_seeded
  + continuity_sampled
  + continuity_broken_surface_change
  + continuity_broken_nonmonotonic_commit
  + continuity_broken_missing_previous_present
```

`seeded` is the first qualified candidate after reset. `sampled` is an
unchanged exclusive surface with a strictly newer commit and a previous
present timestamp, preserving the existing continuous-sample predicate. The
three broken counters explain why a qualified candidate did not contribute to
continuous cadence. All counters are scalar and bounded by the run's sample
count; no per-sample ledger is retained.

## Predictive O1 lifecycle diagnostics

Existing O1 policy, admission, claim, overtake, recovery, and predictor logic
remain unchanged. A single frame ID tracks the predictive READY state. Its
terminal classification is recorded exactly once at the existing ownership
boundary:

```text
predictive_ready_created
  = predictive_ready_submitted
  + predictive_ready_overtaken_ready
  + predictive_ready_overtaken_worker_queued
  + predictive_ready_other_safe_abandonment
  + predictive_ready_failed
  + predictive_ready_current_at_shutdown
```

The tracker is cleared on every terminal transition, so duplicate cleanup
cannot duplicate a counter. The state is one optional frame ID, not an
unbounded ledger. Existing `predictive_render_ahead_ready` and
`predictive_ready_submits` remain intact for backwards-compatible summaries.

## Comparator principles

The study extracts ownership principles rather than constants or framework
machinery:

- delayed focus has one explicit owner;
- a timer is single-shot and is removed by its successful terminal;
- acknowledgement, unmap, and destroy cancel timeout ownership;
- duplicate terminal delivery is harmless.

These principles are consistent with the delayed-focus cleanup patterns in
[KWin's X11 activation code](https://github.com/KDE/kwin-x11/blob/Plasma/5.0/activation.cpp),
[Mutter's X11 window implementation](https://github.com/endlessm/mutter/blob/master/src/x11/window-x11.c),
and [Hyprland's compositor lifecycle](https://github.com/hyprwm/Hyprland/blob/main/src/Compositor.cpp).

## Verification contract

Deterministic tests cover timer-only focus expiry through real service APIs,
duplicate timer invocation, confirmation-before-timeout, destroyed-target
cancellation, simultaneous focus/adoption expiry, wake-plan disarm after the
same XWayland T, every fast-candidate rejection, all continuity outcomes, and
predictive READY submitted/overtaken/failed/shutdown reconciliation.

Native acceptance remains a separate hardware run. The hard boundary is:

```text
stale_deadline_rearms = 0
past_deadline_arms = 0
stale_xwayland = 0
past_xwayland = 0
no immediate timer storm
```

The established physical cadence and DMA-BUF ownership evidence are preserved
as regression constraints, not retuned by this change.
