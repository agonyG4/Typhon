# Plan: Typhon Native XWayland Deadline Ownership v2.2

## Constraints

- Preserve unrelated working-tree edits.
- Do not use sub-agents.
- Use `rtk` for repository, build, test, and runtime commands.
- Keep the change limited to deadline ownership and bounded observability.
- Do not change physical clock, predictor tuning, O1 policy, DMA-BUF release
  ownership, or native test-machine configuration.

## Implementation steps

1. Confirm current source and graph evidence for `XwaylandService` deadline
   publication/consumption, XWM focus state, Native Wake Authority selection,
   fast-client admission, and predictive READY ownership.
2. Add `Xwm::next_deadline_ns` and `Xwm::handle_deadlines`; route the service's
   Running-state publication and timer handler through that aggregate while
   retaining startup/backoff/termination handling and the opportunistic event
   hook.
3. Add test-only XWM/service fixtures that create a real Running service state
   with a deterministic focus transition, then verify timer-only expiry,
   duplicate invocation, confirmation, and target disappearance.
4. Add the aggregate focus-plus-adoption test and the Native Wake Plan test
   proving a consumed XWayland T is disarmed rather than reselected as stale or
   past.
5. Add mutually exclusive fast-candidate rejection counters and continuity
   lifecycle counters without changing the qualification predicate or sample
   population. Export them in the native content summary.
6. Add one bounded predictive READY frame identity and terminal counters at
   submit, safe-ready overtake, worker-queued overtake, safe abandonment,
   failure, and shutdown boundaries. Export the reconciliation counters in the
   native summary without changing O1 decisions.
7. Run targeted tests, formatting, `cargo check`, Clippy, the full test suite,
   and diff checks. Treat unrelated pre-existing failures as such and do not
   repair them in this task.
8. Run the exact approved native command. Record whether DRM/session access
   permits the run and report all relevant summary counters without suppressing
   failures.
9. Stage only v2.2 files and create a focused commit; leave pointer-constraint
   and other user-owned edits unstaged.

## Files in scope

```text
src/xwayland/service.rs
src/xwayland/service_state.rs
src/xwayland/service_runtime.rs
src/xwayland/tests.rs
src/xwayland/xwm/api.rs
src/xwayland/xwm/events.rs
src/xwayland/xwm/events_regression_tests.rs
src/xwayland/xwm/focus.rs
src/xwayland/xwm/mod.rs
src/native_output/pacing.rs
src/native_output/runtime/wake_plan.rs
src/native_output/runtime/cycle/pageflip.rs
src/native_output/runtime/kms_worker/rejection.rs
src/native_output/runtime/mod.rs
```

The report is updated after verification with exact command results, native
evidence, limitations, and the resulting commit ID.
