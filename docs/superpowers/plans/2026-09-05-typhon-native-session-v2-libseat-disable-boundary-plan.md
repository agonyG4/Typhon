# Typhon Native Session v2 — Implementation Plan

**Goal:** Make VT/session relinquishment safe at the libseat callback-return boundary, make VT request failures nonfatal, consume reentrant seat events immediately, and prevent post-revoke cursor DRM I/O.

**Constraints:** Preserve the accepted frame-pacing and session-recovery architectures; compile in the existing checkout/`target`; use `rtk`; do not use sub-agents; preserve unrelated work; commit checkpoints.

## Task 1 — Documentation checkpoint

- [x] Record the approved design and backend-neutral invariants.
- [x] Record implementation ownership, test gates, static verification, and qualification/report gates.
- [ ] Run `rtk git diff --check`, stage only these two docs, and commit `docs: design native session v2 disable boundary`.

## Task 2 — Worker quiesce authority and callback boundary

Files: `src/native_output/kms_worker/queue.rs`, `src/native_output/kms_worker/thread.rs`, `src/native_output/input/routing.rs`, `src/native_output/runtime/bootstrap.rs`, `src/native_output/runtime/mod.rs`, and focused tests.

- [ ] Add `KmsWorkerQuiesceHandle` backed by `Arc<WorkerShared>` and expose it from `KmsCommitWorkerHandle` without exposing join/event/submission ownership.
- [ ] Add a stable `NativeSeatSession` pre-disable hook slot with install/replace support.
- [ ] Invoke the installed hook synchronously between `active=false` and queuing the de-duplicated `Disabled` event.
- [ ] Install the bootstrap worker authority and update it on every recovery worker replacement; clear it when the worker is retired or the transport is synchronous.
- [ ] Add RED/GREEN tests for active-ioctl serialization, no new admission after quiesce, no-worker mode, duplicate callback de-duplication, and W1→W2 replacement.

## Task 3 — Runtime seat-event consumer and VT request ownership

Files: `src/native_output/input/events.rs`, `src/native_output/input/routing.rs`, `src/native_output/runtime/cycle.rs`, `src/native_output/runtime/cycle_dispatch.rs`, and focused tests.

- [ ] Extend `NativeInputApplication` and `NativeWaylandInputDispatchOutcome` with an optional requested VT.
- [ ] Remove direct libseat invocation and the fatal `?` from `apply_native_input_effect()`.
- [ ] Split runtime seat handling into dispatch plus one reusable pending-event consumer.
- [ ] Add runtime-owned `request_native_vt_switch()`: request, consume reentrant events regardless of request result, log explicit requested/failed outcome, and suppress only the raw request error.
- [ ] Re-check output permission immediately after request handling and stop the current output-producing cycle when suspension began.
- [ ] Add RED/GREEN tests for failed request survival, no fake success event, reentrant Disable with success, and reentrant Disable with failure.

## Task 4 — Cursor session retirement

Files: `src/native_output/runtime/session_io.rs`, `src/native_output/output/legacy_cursor.rs`, `src/native_output/output/cursor.rs`, and focused tests.

- [ ] Rename the session operation to accurately describe retirement/suspension if required by existing semantics.
- [ ] Suspend atomic cursor bookkeeping without physical cursor submission.
- [ ] Disarm legacy DRM cleanup before dropping the retired object; ensure `Drop` cannot issue the forbidden cursor-clear ioctl.
- [ ] Preserve normal frame-time cursor disable/move behavior and normal non-session destruction behavior.
- [ ] Add RED/GREEN tests proving no legacy cursor ioctl occurs after the callback boundary or during retired-object drop.

## Task 5 — Session ordering and non-regression tests

Files: existing session recorder/lifecycle tests, KMS worker tests, runtime session tests, and the smallest relevant native-output test modules.

- [ ] Add explicit `SeatDisableCallback`, `PreRevokeKmsQuiesce`, and `SeatDisableCallbackReturned` operation points.
- [ ] Prove pre-revoke quiesce precedes callback return and all post-callback output operations avoid physical submission.
- [ ] Cover logind and seatd abstract timelines without detecting backend type in production.
- [ ] Re-run existing recovery-generation, presented-plane, transaction, DMA-BUF, O1, predictor, and Wake Authority suites without policy changes.

## Task 6 — Verification and report

- [ ] Run focused native seat, VT request, lifecycle, session I/O, worker submit-gate, cursor, recovery, and generation-barrier tests.
- [ ] Run fresh `rtk cargo fmt --check`, `rtk cargo check`, `rtk cargo clippy --all-targets --all-features -- -D warnings`, `rtk cargo test`, `rtk git diff --check`, and `rtk git status --short`.
- [ ] Build with `rtk cargo build --release` in the existing `target` directory and record `git rev-parse HEAD`, SHA-256, and `stat` for `target/release/oblivion-one`.
- [ ] Perform physical VT away/back qualification only if the host permits it safely; record three cycles and post-resume workload/counters, or explicitly mark it inconclusive.
- [ ] Create `docs/superpowers/specs/REPORT-2026-09-05-typhon-native-session-v2-libseat-disable-boundary.md` with source evidence, RED/GREEN results, static/build evidence, qualification evidence, and every adversarial-review answer.
- [ ] Run final diff/status checks and commit the implementation/report with `fix: close native session v2 disable boundary`.
