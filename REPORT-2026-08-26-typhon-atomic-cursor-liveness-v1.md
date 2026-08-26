# Typhon Atomic Cursor Liveness v1 — Closure Report

Date: 2026-08-26

## Result

The atomic hardware cursor starvation path is closed. Native input now observes
the final coalesced atomic cursor state once per input batch and independently
arms the existing cursor-output arbitration window. Hardware cursor movement
remains independent from primary-scene repaint.

Commits:

- `1690eab docs: design atomic cursor liveness closure`
- `2345342 docs: plan atomic cursor liveness closure`
- `303b859 fix: model atomic cursor output debt`
- `038c364 fix: arm atomic cursor liveness from input`
- `e0261ae fix: preserve atomic cursor liveness across input`

## Root cause

The source audit confirmed the supplied circular dependency:

```text
input changed NativeAtomicCursor.desired
→ primary repaint was correctly suppressed
→ no other work admitted presentation_cycle
→ presentation_cycle never discovered the cursor epoch
→ NativeCursorOutputArbitration stayed unarmed
→ arm_runtime_deadline had no cursor deadline
→ no cursor-only atomic presentation occurred
```

The eventual unrelated presentation submitted the newest desired state, which
explains the freeze followed by a teleport. The legacy cursor path remains
immediate and was not routed through atomic arbitration.

## Implementation

### Cursor-owned liveness predicate

`NativeAtomicCursor::needs_output_liveness_for()` in
`src/native_output/output/cursor.rs` compares effective desired state with the
existing KMS-equivalence semantics. Its future-output baseline is selected in
ownership order:

1. worker-owned queued cursor state;
2. submitted/in-flight cursor state;
3. current/presented cursor state.

This preserves newer desired work while an older cursor submission is in
flight, and handles A → B → A cancellation before B is committed. Hidden to
hidden movement is suppressed because `kms_equivalent()` already treats hidden
states as equivalent.

### Input-boundary observer

`observe_atomic_cursor_output_liveness()` in
`src/native_output/runtime/cursor_cycle.rs` runs after the final
`synchronize_cursor_state_for_server()` in
`dispatch_wayland_and_input()`.

The observer uses the existing `atomic_cursor_visibility_policy()` to avoid
creating redundant atomic work for ordinary software-cursor movement, while
still arming a clear when a visible hardware plane must be removed. It calls
`NativeCursorOutputArbitration::request_hardware()` with
`NativeFrameScheduler::next_refresh_deadline_ns(now_ns)`.

No raw-event loop, scene redraw request, Wayland dispatch, Astrea reconciliation,
or direct KMS submission was added.

### Arbitration ownership

`NativeCursorOutputArbitration` now tracks hardware cursor pending work
separately from software-overlay work. Stale hardware debt can be reconciled
without clearing a legitimate software-overlay response window. Existing
first-deadline stability, latest-epoch coalescing, piggyback selection, exact
epoch consumption, and re-arming behavior remain in the arbitration layer.

The existing runtime deadline path already includes
`cursor_output_arbitration.deadline_ns()`. Work-domain classification therefore
remains `NoOutputWork` before maturity and becomes `CursorOnly` at the cursor
deadline; pointer motion does not promote primary-scene work.

## Deterministic tests

TDD RED runs were recorded before production implementation:

- `rtk cargo test cursor_output_liveness -- --nocapture` initially failed because
  the cursor-owned liveness predicate was absent.
- `rtk cargo test input_boundary_observer -- --nocapture` initially failed because
  the observer and hardware-source arbitration APIs were absent.

Passing focused results:

- `rtk cargo test native_output::output::cursor -- --nocapture` — 41 passed.
- `rtk cargo test native_output::runtime::frame -- --nocapture` — 2 passed.
- `rtk cargo test native_output::tests::frame -- --nocapture` — 50 passed.
- `rtk cargo test native_output::runtime::work_domains -- --nocapture` — 10 passed.
- `rtk cargo test native_output::tests::input -- --nocapture` — 76 passed.
- `rtk cargo test input_boundary_liveness_rearms_after_an_older_inflight_cursor_completes -- --nocapture` — 1 passed.
- `rtk cargo test software_cursor_only_arms_atomic_liveness -- --nocapture` — 1 passed.

The tests cover visible movement, hidden-to-hidden suppression, show/hide and
software transitions, stale A → B → A cancellation, newer desired state during
an older in-flight submission, scheduler-derived deadlines, cursor-only
maturity, primary non-promotion, software-overlay preservation, and 1,000
coalesced hardware cursor updates retaining one response window and the latest
epoch.

Broader verification:

- `rtk cargo fmt --check` — passed.
- `rtk cargo check` — 0 errors, 7 existing dead-code warnings outside this
  closure.
- `rtk cargo test` — 2,970 passed, 5 ignored, 40 filtered out.
- `rtk git diff --check` — passed.
- `rtk cargo clippy --all-targets --all-features -- -D warnings` — blocked by 22
  existing lint errors and 1 warning in unrelated compositor/layout/resource
  work; no diagnostic was reported in the closure's changed cursor files.

## Review Pass 1 — correctness and ownership

1. Input gained liveness-notification authority only; final KMS policy remains
   in presentation/arbitration/KMS code.
2. The first deadline is derived from `NativeFrameScheduler`.
3. Continuous samples replace the latest epoch while retaining the first
   response-window deadline.
4. Hidden-to-hidden movement is suppressed through `kms_equivalent()`.
5. Software-only movement does not create redundant atomic cursor work.
6. A software transition can still arm the clear of an already-visible hardware
   plane.
7. Newer desired epochs remain observable against worker or in-flight state.
8. Exact submitted-epoch consumption does not clear a newer pending epoch.
9. Legacy `move_to()` behavior remains immediate.
10. Hardware cursor movement is non-primary before cursor deadline maturity.
11. Existing primary piggyback arbitration remains unchanged.
12. Pageflip, worker, atomic transaction, framebuffer, and lineage ownership was
    not moved into input.

## Review Pass 2 — adversarial cases

The deterministic arbitration/cursor suites and full suite were rerun against
the following cases: 1,000 Hz input without primary frames; pending cursor
pageflips; worker-owned state; primary-pending piggyback; hidden/show transitions;
software fallback and plane clearing; client cursor state changes; and A → B → A
both before submission and while B is in flight. The high-rate test confirms one
refresh-derived response window, 999 coalesced changes, and the latest epoch.

Existing interaction, direct-scanout, atomic-busy, output-recovery, pause/resume,
shutdown, floating move/resize, and tiled/Dwindle resize tests remain green in
the full 2,970-test run. No new broad work was added to those paths.

## Hardware qualification

No Linux KMS/TTY qualification run was available in this session. Therefore this
report makes no real-hardware latency or freeze/teleport claim and invents no
cadence measurements. The closure is qualified by source audit and deterministic
tests only.

## Follow-ups intentionally kept separate

The following residuals remain outside this one-root-cause closure:

- `should_progress_surface_pacing()` readiness coupling can still service
  surface-pacing work on input-only wakes.
- Eager `format!("event_index={event_index}")` allocation remains in the input
  loop.
- Shortcut-inhibition raw-frequency scanning remains a separate input hot-path
  concern.
- The key/button narrow full-tick residual remains separate.

The working tree remains otherwise dirty as provided. Only closure files were
committed; unrelated current Typhon work and the pre-existing deletion in
`src/native_output/runtime/frame.rs` were preserved and not staged by the final
closure commit.
