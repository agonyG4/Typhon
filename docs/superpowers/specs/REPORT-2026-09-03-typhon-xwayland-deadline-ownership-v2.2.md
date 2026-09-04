# Report: Typhon Native XWayland Deadline Ownership v2.2

## Scope

Implemented the narrow v2.2 correctness and observability closure. The
physical frame clock, predictor, O1 policy/claim semantics, DMA-BUF release
ownership, and unrelated pointer-constraint work were left unchanged.

## Root cause

`XwaylandService::next_deadline_ns()` published the minimum of XWM resize-sync,
focus, and adoption deadlines. The Native Wake Authority invoked
`XwaylandService::handle_deadline()`, whose Running path consumed adoption and
resize-sync but not focus. Focus timeout handling existed only as a separate
opportunistic event-path call. An expired focus T therefore remained the
published deadline and was repeatedly selected as stale/past.

## Code closure

- Added the single XWM aggregate deadline query and handler.
- Routed the service Running timer path through the aggregate.
- Preserved the event-path focus handler as an idempotent compatibility path.
- Added real service fixtures for focus timeout, confirmation, destruction,
  duplicate invocation, and XWM aggregate focus/adoption expiry.
- Added the Native Wake Plan boundary test for disarming a consumed XWayland T.
- Added mutually exclusive fast-candidate rejection diagnostics and continuity
  lifecycle diagnostics, with accounting identities in the design.
- Added bounded predictive READY lifecycle counters using one optional frame ID
  and terminal classification at existing ownership boundaries.

## Tests

Targeted service/XWM and binary pacing tests passed during implementation:

```text
xwayland::tests::timer_only_focus_deadline_is_consumed_by_the_service_timer_api
xwayland::tests::focus_confirmation_before_timeout_removes_the_timer_deadline
xwayland::tests::destroyed_focus_target_cancels_the_timer_deadline_without_repair
xwayland::xwm::events::regression_tests::aggregate_deadline_handler_services_focus_and_adoption_due_together
native_output::pacing::tests::fast_client_population_requires_continuous_exact_surface_content
native_output::pacing::tests::fast_candidate_diagnostics_account_for_qualification_and_continuity_outcomes
native_output::pacing::tests::predictive_ready_lifecycle_reconciles_safe_overtake_failure_and_shutdown
```

The Native Wake Plan boundary test was added after the initial targeted pass
and could not be rerun after unrelated shared-checkout edits made the crate
unbuildable. It is included in the staged source and was checked for scoped
formatting before those edits appeared.

## Final verification

Final fresh verification in the shared checkout:

```text
cargo fmt --check: failed on unrelated shared-checkout keyboard/decoration formatting
cargo check: failed on unrelated X11 PropertyKind exhaustiveness edits
cargo clippy --all-targets --all-features -- -D warnings: failed with 18 unrelated X11 decoration/keyboard errors and 1 warning
cargo test: failed with 46 unrelated X11 decoration/keyboard errors and 2 warnings
git diff --check: passed
```

The scoped v2.2 rustfmt check passed before the later shared-checkout edits.
The failed full checks are outside v2.2: the working tree contains unrelated
`decoration_hints`/`no_decorations` and keyboard edits in X11 decoration
metadata and its consumers. Those files are not staged by this task.

## Native qualification

The approved native command exited 1 after opening `/dev/dri/card1` and
selecting the exact 1920x1080@165 Hz output, but the pre-render atomic TEST_ONLY
commit failed with `Permission denied (os error 13)`. No Native Wake Authority
or pacing summary was emitted. This is an environment limitation, not a pass
for the hard stale/past boundary.
