# Typhon Native Session v2 — Disable Boundary Closure Report

Date: 2026-09-05

Status: implementation and deterministic verification complete; physical VT qualification remains pending.

## Root causes and implementation

Typhon previously queued `NativeSeatEvent::Disabled` and returned from the libseat callback before closing the concurrent KMS worker submission boundary. A VT request error also escaped `apply_native_input_effect()` as a fatal `NativeResult`, and a disable callback delivered reentrantly by `switch_session()` could remain queued until a later seat-fd wakeup.

The implementation now:

- adds cloneable `KmsWorkerQuiesceHandle`, backed by the existing `WorkerShared::submit_gate`; it exposes only `request_quiesce()`;
- installs a stable `NativeSeatPreDisableHook` slot in `NativeSeatSession`, replacing its worker authority at bootstrap and after recovery;
- orders the disable callback as `active=false`, synchronous pre-revoke quiesce, one deduplicated `Disabled` event, then callback return;
- extracts `consume_pending_seat_events()` and invokes it immediately after every runtime-owned VT request;
- carries `NativeInputEffect.vt_switch` through `NativeInputApplication` and `NativeWaylandInputDispatchOutcome` to `NativeRuntime`;
- records `native.vt_switch_request` with `status=requested|failed`, `disabled_observed`, and an error string, while keeping request failure nonfatal;
- rechecks `session.permits_output()` before continuing the current output-producing cycle;
- renames suspension cursor work to `retire_hardware_cursor_for_session`; atomic cursors are suspended as state, while legacy cursors are disarmed before drop without a cursor-clear ioctl;
- preserves the later explicit `seat.disable()` acknowledgment and all existing recovery, transaction, DMA-BUF, pacing, wake, and generation-barrier machinery.

## Backend-neutral ownership model

The [libseat logind backend](https://raw.githubusercontent.com/kennylevinsen/seatd/master/libseat/backend/logind.c) invokes the disable listener before completing `PauseDevice("pause")`; its `disable_seat()` path is effectively a no-op. Therefore callback return is Typhon’s pre-revocation boundary for logind. The [libseat seatd backend](https://raw.githubusercontent.com/kennylevinsen/seatd/master/libseat/backend/seatd.c) still has an explicit `CLIENT_DISABLE_SEAT` / `SERVER_SEAT_DISABLED` exchange, so Typhon retains the later `seat.disable()` acknowledgment. One stronger callback contract serves both backends without backend detection.

The resulting timeline is:

```text
Disable callback
  -> active=false
  -> pre-revoke worker quiesce through submit_gate
  -> queue Disabled once
  -> return
  -> runtime consumes Disabled
  -> suspend input / park sync / join worker / quarantine pageflip
  -> retire cursor state without a session-loss DRM clear
  -> seat.disable() acknowledgment
  -> Suspended
  -> existing synchronous recovery and generation barrier
  -> new worker hook installed before Active
```

Main-thread KMS operations cannot execute concurrently with the callback because both callback delivery and the runtime’s main-thread work occur on the native runtime thread. The worker path is the only concurrent submission path, and it reuses the existing submit gate; no second submission lock, raw runtime pointer, unsafe self-reference, background session thread, timer, or polling loop was added.

## Files changed

Implementation and tests:

- `src/native_output/input/routing.rs`
- `src/native_output/kms_worker/mod.rs`
- `src/native_output/kms_worker/thread.rs`
- `src/native_output/kms_worker/tests.rs`
- `src/native_output/runtime/bootstrap.rs`
- `src/native_output/runtime/cycle.rs`
- `src/native_output/runtime/cycle_dispatch.rs`
- `src/native_output/runtime/kms_worker.rs`
- `src/native_output/runtime/mod.rs`
- `src/native_output/runtime/session.rs`
- `src/native_output/runtime/session_io.rs`
- `src/native_output/tests/binding_launch.rs`
- `src/native_output/tests/input.rs`
- `src/native_output/tests/input_interaction_liveness.rs`

Documentation:

- `docs/superpowers/specs/2026-09-05-typhon-native-session-v2-libseat-disable-boundary-design.md`
- `docs/superpowers/plans/2026-09-05-typhon-native-session-v2-libseat-disable-boundary-plan.md`
- this report

Commits: `72986ca` (design), `368d4df` (RED tests), `3fa899f` (implementation), `8d4f48a` (test stabilization).

## RED and GREEN evidence

The RED checkpoint introduced deterministic coverage for:

- an active worker submit blocked by the pre-revoke authority;
- nonfatal VT request failure;
- reentrant `Disabled` during a failed request;
- cursor session retirement without `CursorDisable`;
- callback ordering and backend-neutral lifecycle modeling.

GREEN coverage additionally proves no-worker hook behavior, recovered-worker authority replacement, callback de-duplication, runtime event consumption, session lifecycle recovery ordering, and legacy cursor cleanup disarming. The gate test accepts the existing worker’s valid immediate `Quiescing` → `Stopped` finalization, while requiring admission rejection and no return to `Running`.

## Verification

Fresh commands completed successfully:

- `rtk summary cargo fmt -- --check`
- `rtk cargo check`
- `rtk cargo clippy --all-targets --all-features -- -D warnings`
- `rtk cargo test` — `3447 passed, 5 ignored, 40 filtered out`
- `rtk git diff --check`
- `rtk cargo build --release`

The release build used the repository’s existing `target` directory. At the release capture point, `git rev-parse HEAD` was `8d4f48add83b348ea3b9a0973f7a7582f6d38eea`; `target/release/oblivion-one` was 19,340,408 bytes, mode `0755`, and had SHA-256 `7bf70bf583e71c5499bde95ef4650775a53214cd948bb899d5e215e724aa185b`.

## Acceptance evidence and limits

Deterministic source/test evidence proves the KMS submit-gate contract, nonfatal VT errors, reentrant disable consumption, immediate output-permission recheck, cursor post-revoke safety, explicit seatd acknowledgment, duplicate-disable handling, worker-hook replacement, unchanged recovery generation architecture, and no changes to Predictive O1, `PrimaryRefreshClaim`, `PresentedPlaneSnapshot`, OutputTransaction ownership, DMA-BUF release ownership, or Native Wake Authority policy.

No physical VT roundtrip was performed in this environment. The qualification command was not launched, synthetic input was not used, screenshots were not taken, and Eclipse was not modified. Consequently there is no claimed G1 → G2 → G3 → G4 hardware trace, 165 Hz cadence sample, first-switch survival trace, post-resume workload trace, metric dump, or `SafeDisable` shutdown trace. Those remain the required manual follow-up using the supplied physical-keyboard configuration.

The pending manual checklist is:

- perform at least three physical away/back VT cycles;
- confirm `native.vt_switch_request`, `native.seat_pre_revoke`, and suspend/resume transitions;
- confirm synchronous recovery, generation-barrier provenance retirement, and continued output;
- exercise hover, settings, XWayland, scrolling, popups, idle/resume interaction;
- inspect O1 terminal reconciliation, 165 Hz cadence, wake counters, OutputTransaction, DMA-BUF, protocol, and final `SafeDisable` metrics.

## Adversarial review

| Question | Evidence / answer |
| --- | --- |
| Does logind complete `PauseDevice` after the disable callback? | Yes; verified in current upstream `logind.c`. |
| Is logind `disable_seat()` effectively a no-op? | Yes; verified in current upstream source. |
| Does seatd still need explicit `seat.disable()`? | Yes; retained and verified against current `seatd.c`. |
| Can a worker ioctl be active or begin at callback return? | No: the callback blocks on the existing `submit_gate`; admission is then rejected. |
| Can the callback access `NativeRuntime` unsafely? | No raw pointer or recursive runtime callback was added. |
| Can reentrant `Disabled` remain queued while output continues? | The shared consumer drains it immediately after `switch_session()` and the cycle rechecks output permission. |
| Can VT request failure become fatal or log success? | No: the helper records `failed`, consumes events regardless of result, logs failure, and returns nonfatal success. |
| Can failure-after-disable lose the event? | No: event consumption runs independently of request result. |
| Can legacy cursor cleanup issue post-revoke DRM I/O? | Session retirement disarms it before drop; normal non-session `disable()` behavior remains. |
| Is seatd acknowledgment preserved? | Yes, after runtime suspension bookkeeping. |
| Can duplicate disable reopen admission? | No; callback de-duplication is retained and quiesce is monotonic/idempotent. |
| Does recovery replace the worker authority? | Yes; restart installs the new authority before resume becomes Active. |
| Is no-worker mode safe? | Yes; the hook is a no-op while callback/event/lifecycle handling remains active. |
| Did accepted recovery, pacing, O1, claims, DMA-BUF, transactions, protocol, or wake policy change? | No; the diff is limited to session entry, VT ownership, cursor retirement semantics, tests, and fixture field removal. |
| Did real first-switch, three-cycle, cadence, workload, metric, or SafeDisable acceptance pass? | Not measured here; manual physical qualification is explicitly pending. |
