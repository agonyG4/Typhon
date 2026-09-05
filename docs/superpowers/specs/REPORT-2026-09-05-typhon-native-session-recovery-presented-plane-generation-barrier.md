# Typhon Native Session Recovery v1 — Presented Plane Physical-Provenance Generation Barrier Report

## Outcome

Implemented and committed the native session-recovery physical-provenance barrier. The accepted frame-pacing v3.x architecture was not redesigned, and the strict primary identity validator was not weakened.

Implementation commits:

- `d3cb373` — design and implementation plan
- `3fbdbe8` — deterministic stale-provenance RED tests
- `515b2e3` — snapshot and synchronous cursor-modeset primitives
- `463fe39` — one prepared generation through native recovery
- `510cba9` — recovery barrier, validation-base, slot-safety, direct-primary tests

The current checkout also contains unrelated concurrent keyboard/layout edits. They were not staged or modified by this task.

## Source root cause

The old runtime kept `PresentedPlaneSnapshot.primary` as a valid G1 `PresentedPrimaryState::Composed` or `Direct` after the synchronous recovery modeset. `rearm_explicit_sync` then allocated a new runtime DRM generation and rebound timing, scanout, explicit-sync watches, and the KMS worker. The first scheduler wake followed:

```text
current_scheduler_wake_deadline()
  -> validate_output_pipeline()
  -> build_output_pipeline_snapshot_with_presented()
  -> validate_presented_primary()
  -> current_composed/output_generation: G1 != G2
```

The validator was correct: pageflip provenance is immutable and a G1 pageflip cannot validate a G2 pipeline.

The observed XWayland `SIGKILL` was downstream unwind behavior. `NativeRuntime::Drop` performs emergency child cleanup and XWayland termination while the outer bootstrap prints the original runtime error afterward. The failing path did not contain the normal native-input exit request or `SafeDisable`; this task therefore added no XWayland or shutdown workaround.

## Before/after state

```text
Before:
G1 presented primary = exact G1 pageflip
  -> session suspend/resume
  -> synchronous recovery modeset
  -> old G1 primary remains in PresentedPlaneSnapshot
  -> runtime/swapchain/timing/worker become G2
  -> first scheduler wake rejects G1 as current G2 provenance

After:
G1 presented primary = exact G1 pageflip
  -> session suspend/resume
  -> synchronous recovery modeset programs framebuffer and cursor baseline
  -> retire quarantined pageflip and suspended transactions
  -> PresentedPlaneSnapshot: primary=None, exact cursor baseline, revision=R+1
  -> exact prepared generation G2 rebinds runtime/timing/swapchain/watches/worker
  -> scheduler rearm and immediate wake validate successfully
  -> first genuine G2 pageflip promotes exact G2 primary provenance
```

The recovery modeset creates no `PageFlipToken`, `OutputTransaction`, presentation serial, physical timestamp, or DMA-BUF presentation correlation. G1 identity is retired, never rewritten as G2.

## Exact implementation

Changed by this task:

- `src/native_output/presentation/plane.rs`
  - added `PresentedCursorState::hidden()`;
  - added `PresentedPlaneSnapshot::rebase_after_session_recovery`, which clears only old primary provenance, installs the supplied physical cursor baseline, and increments snapshot revision.
- `src/native_output/output/cursor.rs`
  - extracted the existing initial-modeset behavior into `mark_synchronous_modeset_submitted`; the initial wrapper remains compatible and recovery uses the accurately named primitive.
- `src/native_output/runtime/mod.rs`
  - changed pending recovery state to carry the scanout token, one prepared generation, and exact cursor baseline.
- `src/native_output/runtime/session_io.rs`
  - allocates the replacement generation exactly once during recovery preparation;
  - passes that same value to cursor preparation and later rebinds;
  - promotes the exact cursor state passed to the successful synchronous modeset;
  - commits the physical snapshot barrier during pageflip retirement, before G2 rebind;
  - clears pending recovery only after exact generation rebind succeeds;
  - emits one recovery-only `native.session_generation_barrier` event.
- `src/native_output/runtime/presentation_pipeline.rs`
  - deterministic stale-G1, rebase, first-G2, repeated-recovery, direct-primary, and slot-safety tests.
- `src/native_output/output/cursor_tests.rs`
  - exact synchronous cursor-state promotion test.
- `src/native_output/kms_worker/payload.rs`
  - validation-base revision discontinuity test.
- the design, plan, and this report under `docs/superpowers/`.

No production changes were made to predictor logic, Predictive O1 admission/binding, `PrimaryRefreshClaim`, KMS worker scheduling policy, DMA-BUF release ownership, protocol handling, fast-client attribution, or Wake Authority policy.

## RED and GREEN evidence

The pre-fix RED was run against the binary test harness, not the library target. Before the method existed:

```text
rtk cargo test --bin oblivion-one session_recovery_rebase_retires_primary_and_advances_revision -- --exact
error[E0599]: no method named `rebase_after_session_recovery`
```

The companion stale-generation test preserves the intended strict failure:

```text
IdentityMismatch {
    owner: "current_composed",
    field: "output_generation",
    ...
}
```

After implementation:

```text
rtk cargo test --bin oblivion-one presentation_pipeline -- --nocapture
cargo test: 19 passed, 1198 filtered out

rtk cargo test --bin oblivion-one session_io -- --nocapture
cargo test: 9 passed, 1208 filtered out

rtk cargo test --bin oblivion-one synchronous_recovery -- --nocapture
cargo test: 2 passed, 1215 filtered out

rtk cargo test --bin oblivion-one pre_recovery_presented_validation_base -- --nocapture
cargo test: 1 passed, 1216 filtered out

rtk cargo test
cargo test: 3418 passed, 5 ignored, 40 filtered out (30 suites)
```

The tests prove:

- a valid G1 primary is accepted at G1 and rejected unchanged at G2;
- the recovery barrier sets `primary=None`, preserves the old G1 identity unchanged, and advances revision;
- immediate G2 pipeline validation succeeds without an intervening pageflip;
- a genuine G2 pageflip restores exact token/bundle, transaction, CRTC, slot, framebuffer, pool-generation, and presentation-serial provenance;
- repeated barriers retire every prior primary, including a direct primary and a no-prior-primary state;
- `AtomicOutputSwapchain::current()` remains excluded from render slots after `primary=None`;
- a pre-recovery presented validation base is `Invalidated` after snapshot revision changes;
- synchronous cursor promotion updates `current` and `submitted` to the exact state without a cursor pageflip or pending token;
- existing late-generation pageflip rejection and the existing transaction/direct/DMA-BUF/O1/Wake Authority tests remain green in the full suite.

## Cursor baseline findings

`NativeAtomicCursor::prepare_for_recovery` still creates the replacement cursor framebuffer and resets the physical bookkeeping to a hidden state before the modeset. The recovery path then derives the exact `cursor_kms_state` that will be passed to KMS, calls `recover_with_cursor`, and promotes that same state with `mark_synchronous_modeset_submitted`.

Therefore:

- visible hardware recovery makes cursor `current`, `submitted`, and presented snapshot agree on the replacement framebuffer and visible geometry;
- hidden/software recovery records a hidden hardware-plane baseline, with the existing software fallback policy unchanged;
- if the cursor plane disappears, hardware preference retains the existing hard error and auto/software preference retains the existing fallback; the presented baseline is hidden rather than stale;
- the old pre-suspend cursor framebuffer is never copied into the post-recovery presented physical state.

## Slot-aliasing proof

The barrier changes presentation provenance only. `AtomicOutputSwapchain::slot_is_free` continues to require `slot != self.current` and also excludes worker-queued, pending, and ready slots. `free_slot_count`, `render_target_available_for_limit`, and `validate_invariants_for_limit` remain unchanged. The new test observes two free slots from a three-slot swapchain while `current` remains non-renderable, and the acquired render slot is asserted to differ from `current`.

## Generation-allocation audit

Current writes are:

```text
bootstrap.rs: initial drm_file_generation = allocate_native_drm_file_generation()
session_io.rs: local recovery generation = allocate_native_drm_file_generation()
session_io.rs: final runtime drm_file_generation = pending generation
```

There is no longer an independent `saturating_add(1)` cursor guess and no second allocator call in `rearm_explicit_sync`. The pending generation is installed only after the synchronous modeset succeeds. A modeset failure returns before pending recovery is installed and before runtime `drm_file_generation` changes. Later rearm failure leaves the session on the existing resume-failure/inactive path; runtime generation is assigned only after the rebind/restart/parked-watch step succeeds, and failure cleanup clears pending recovery.

## Ownership and non-regression review

- Direct scanout: `primary=None` retires both composed and direct pageflip provenance; existing direct callback-owner, lease, safe-abandonment, and `SessionSuspended` settlement paths are untouched.
- Output transactions: recovery still settles active transactions through `OutputTransactionDropReason::SessionSuspended`, including frame-batch completion and terminal reconciliation; no recovery modeset is represented as a client transaction.
- DMA-BUF: no release-plan, fence, correlation, direct lease, or physical pageflip-correlation code changed.
- O1/frame pacing: no predictor, admission, predecessor, ReadyUnbound, physical claim, worker queue, or presentation policy changed. The full suite is green; no native counters were available in this environment.
- 165 Hz cadence: no cadence/timing policy code changed; hardware cadence was not re-measured because native qualification could not be run here.
- Wake Authority: no deadline ownership code changed; full unit coverage is green, but hardware counters were not available.
- protocol/fast client: no protocol or client attribution code changed; workload qualification was not available here.
- clean shutdown: no shutdown code changed; `SafeDisable` could not be observed without a physical native run.

## Static verification and build provenance

Fresh commands:

```text
rtk cargo fmt --check                         BLOCKED
```

Exact unrelated diff: `src/control_tests.rs:2`, where the concurrent control/layout work has a rustfmt-only import wrapping difference.

```text
rtk cargo check                               PASS: 0 errors, 4 warnings
```

The four warnings are `private_interfaces` for the concurrent public keyboard-layout methods returning the `pub(crate)` `KeyboardLayoutControlError`.

```text
rtk cargo clippy --all-targets --all-features -- -D warnings   BLOCKED
```

Exact failure: the same four `private_interfaces` diagnostics in `src/compositor/server_toplevel.rs:169`, `175`, `186`, and `196`, referring to `src/compositor/mod.rs:235`.

```text
rtk cargo test                                PASS: 3418 passed, 5 ignored
rtk git diff --check                          PASS
rtk cargo build --release                     PASS: Finished release profile
```

Build provenance captured after the successful release build:

```text
git rev-parse HEAD
7669ecd2f391a83ea83d5e28d05e116e31734ff8

sha256sum target/release/oblivion-one
9aaf568e942f701e2e45b3a71a3c9c9349d6093ba5369a3caf5ee5fb1e9b7606

stat target/release/oblivion-one
Size: 19357000 bytes
Mode: 0755
Modify: 2026-09-05 14:06:29 -0300
```

The final status was dirty from unrelated concurrent files, including keyboard/layout/control and runtime dispatch edits. Those files were not staged by this task.

## Native VT qualification

Status: inconclusive, not a failed recovery result.

The required manual procedure was not executed because this agent session reports `not a tty`, has no physical keyboard control, and had no running Typhon native process. No synthetic keyboard, ydotool, screenshot, configuration change, or Eclipse operation was used.

The procedure for a physical operator is the approved baseline:

```bash
TYPHON_FRAME_PACING_DEBUG=1 \
OBLIVION_ONE_SHELL_COMMAND=/home/agony/GitHub/Eclipse/build/release/Shell/astrea-shell \
ASTREA_COMPOSITOR_BACKEND=typhon \
TYPHON_XWAYLAND=eager \
OBLIVION_ONE_MODE=1920x1080@165 \
OBLIVION_ONE_KMS_MODE=atomic \
OBLIVION_ONE_SCANOUT_BACKEND=native-egl-gbm \
OBLIVION_ONE_CURSOR=auto \
OBLIVION_ONE_KMS_COMMIT_WORKER=auto \
OBLIVION_ONE_TRIPLE_BUFFERING=auto \
OBLIVION_ONE_DIRECT_SCANOUT=off \
./bin/start-oblivion-one-tty
```

While active, use only the physical keyboard to switch to another VT and back, record `active -> suspending -> suspended -> resuming -> active`, record the recovery event's G1→G2 fields, verify the first real G2 pageflip, and repeat three cycles where practical. Exercise hover/magnification, settings, an XWayland client, scrolling/popups, idle, and interaction after each resume, then terminate normally and record transaction, DMA-BUF, protocol, O1, cadence, Wake Authority, and `SafeDisable` summaries.

Because that procedure was not available in this environment, the number of successful native recovery cycles is `0 observed / not evaluated`, first post-resume hardware pageflip evidence is unavailable, and hardware O1/cadence/Wake Authority/transaction/DMA-BUF/protocol/SafeDisable counters are explicitly not claimed.

## Adversarial final review

| Question | Evidence-backed answer |
|---|---|
| Did the original fatal occur before Drop? | Yes. The source path validates during scheduler wake; `Drop` is unwind cleanup after the outer bootstrap receives the error. |
| Is XWayland SIGKILL a consequence rather than the cause? | Yes. Drop owns XWayland cleanup; no XWayland hook was added. |
| What changes `drm_file_generation`? | Bootstrap allocates the initial value; recovery assigns the one pending allocator value at the end of explicit-sync rearm. |
| Can old G1 provenance survive after G2 becomes authoritative? | No in the fixed state machine: retirement calls the snapshot barrier before rebind, and the deterministic stale/rebase tests pass. |
| Can a recovery modeset manufacture pageflip provenance? | No. It only promotes cursor synchronous-modeset bookkeeping; primary remains `None` until a real pageflip. |
| Does `primary=None` keep the current slot non-renderable? | Yes. Existing `slot_is_free` excludes `current`; the new slot-safety test passes. |
| Does the first G2 pageflip restore exact provenance? | Yes in the deterministic physical-layer test, including token/bundle, transaction, slot, framebuffer, pool generation, serial, and generation. |
| Can a late G1 pageflip overwrite G2 state? | No. Existing stale-generation pageflip rejection remains covered by the full suite; no promotion path was relaxed. |
| Can a G1 Direct primary remain authoritative? | No. The barrier clears either primary variant; the direct-primary identity test passes and existing direct settlement remains unchanged. |
| Does recovery cursor state match the KMS modeset? | Yes. The same cloned `cursor_kms_state` is passed to KMS and synchronous cursor promotion; the exact-state test passes. |
| Can an old cursor framebuffer remain physically current? | No in the fixed state bookkeeping: recovery prepares a replacement and promotes the exact modeset state; the cursor test passes. |
| Does snapshot revision reflect the baseline change? | Yes. The barrier increments it; the revision test passes and a pre-recovery base becomes `Invalidated`. |
| Can a pre-recovery KMS validation base survive as post-recovery? | No. Revision and generation must match; the validation-base test passes. |
| Is generation identity derived coherently? | Yes. One recovery allocator call is carried through cursor preparation and every rebind; no `+1` guess or second rearm allocation remains. |
| Can recovery failure leave G2 partially authoritative? | A failed modeset does not install pending state or runtime G2. Later rearm failure remains on the existing inactive resume-failure path, with runtime assignment after successful rearm and cleanup clearing pending state. |
| Can G1→G2→G3 recovery repeat? | The snapshot state test passes repeated barriers; full runtime hardware cycles were not available. |
| Does immediate scheduler wake validate? | Yes in the deterministic G2 builder test, without a pageflip. |
| Did O1 policy change? | No production O1 files or policy paths changed; full tests pass; hardware counters not measured. |
| Did PrimaryRefreshClaim semantics change? | No. |
| Did predictor behavior change? | No. |
| Did KMS worker scheduling policy change? | No. |
| Did fast-client attribution change? | No. |
| Did DMA-BUF release ownership change? | No. |
| Did OutputTransaction terminal semantics change? | No. |
| Did the half-refresh bug return? | No relevant timing code changed; hardware cadence was not re-measured. |
| Did Native Wake Authority remain clean? | No policy code changed and unit tests pass; hardware counters were not available. |
| Did final clean shutdown reach SafeDisable? | Not evaluated here because physical VT qualification was unavailable; shutdown code was not changed. |

## Reference comparison

The architectural lesson agrees with current upstream behavior: Aquamarine clears stale page-flip bookkeeping before VT restoration, and KWin treats DRM device pause/resume as an explicit ownership transition. Typhon expresses the same principle through its generation barrier and typed physical snapshot rather than importing either structure.

- [Aquamarine DRM session restore](https://github.com/hyprwm/aquamarine/blob/main/src/backend/drm/DRM.cpp)
- [KWin DRM backend](https://github.com/KDE/kwin/blob/master/src/backends/drm/drm_backend.cpp)
