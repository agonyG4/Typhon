# Typhon Native Frame Pacing v3.2 — Predictive O1 Observability Reconciliation Report

Date: 2026-09-04

## Outcome

Predictive O1 observability is now reconciled by exact `NativeOutputFrameId`.
The accepted v3/v3.1 physical architecture remains unchanged. Normal READY
waits no longer inflate Predictive O1 metrics, and overlapping Predictive O1
frames no longer compete for a single mutable terminal identity.

Source closure is complete in commit `7a08256`. The design and execution plan
are recorded in commits `0465dd0` and the preceding plan documentation.

## Root causes and implementation

The 130/138 anomaly came from using the generic `waits_for_target` condition as
Predictive O1 identity. That condition also describes normal composited work
waiting behind an Atomic/KMS lane. `PreparedFrameOrigin` is now classified at
render start and stored with the active pacing identity; only
`PredictiveTriple` plus `render_ahead == true` creates a Predictive O1 record.

The 130/100 divergence came from the former single
`predictive_ready_frame_id: Option<_>` tracker. `NativeFramePacing` now owns a
fixed four-entry exact-ID lifecycle ledger. It records each applicable stage:

```text
Rendering -> RenderReady -> ReadyUnbound -> Bound -> WorkerQueued
           -> Submitted -> Presented
```

Valid architectural skips are `RenderReady -> Bound` and `Bound -> Submitted`.
Each stage is recorded at its existing ownership boundary, and each live entry
is removed exactly once by `Presented`, identity abandonment, generation
abandonment, other safe abandonment, failure, or shutdown-current accounting.
The ledger is bounded, contains no retained per-frame history, and adds no
thread, timer, or hot-path log.

The authoritative terminal equation is exposed in the existing summary:

```text
predictive_o1_created
= predictive_o1_presented
 + predictive_o1_abandoned_identity
 + predictive_o1_abandoned_generation
 + predictive_o1_other_safe_abandonment
 + predictive_o1_failed
 + predictive_o1_current_at_shutdown
```

The summary also reports active and peak ledger population, terminal total,
remainder, and an exact reconciliation boolean. Legacy READY counters remain
available: `predictive_ready_created` is exact-origin only,
`predictive_ready_submitted` is incremented at exact submission, and the
existing v3 unbound/binding counters retain their names and meanings.

Relevant source boundaries are:

- `src/native_output/pacing.rs:1551` — origin and bounded ledger;
- `src/native_output/pacing.rs:2144` — exact render-start admission;
- `src/native_output/pacing.rs:2291` — exact render-ready stage;
- `src/native_output/pacing.rs:2437` — exact worker reservation stage;
- `src/native_output/pacing.rs:2700` — physical pageflip terminal;
- `src/native_output/pacing.rs:3500` — summary reconciliation;
- `src/native_output/runtime/presentation_cycle.rs:976` — origin admission;
- `src/native_output/runtime/presentation_cycle.rs:1174` — render failure;
- `src/native_output/runtime/presentation_cycle.rs:1182` — skipped-render safe abandonment;
- `src/native_output/runtime/presentation_cycle.rs:1260` — render completion;
- `src/native_output/runtime/mod.rs:692` — shutdown drain.

No changes were made to O1 admission thresholds, deferred binding selection,
physical claims, KMS timing or worker policy, predictor behavior, scheduler or
wake ownership, ReactiveDouble, Direct Scanout, DMA-BUF ownership,
OutputTransaction semantics, or fast-client attribution.

## Deterministic coverage

The pacing suite has 43 passing tests. The added tests cover:

- normal READY waiting versus Predictive O1 isolation;
- N Predictive O1 plus M normal READY reconciliation;
- overlapping exact worker identities;
- reversed terminal ordering;
- duplicate submit handling;
- generation and identity abandonment;
- shutdown-current draining of every live identity;
- submitted-to-presented progression;
- ReactiveDouble, normal, and Direct-path isolation;
- existing bound-overtake recovery behavior.

## Static verification

All commands were run in `/home/agony/GitHub/Typhon`, using the existing
checkout and `target/` directory.

```text
rtk cargo fmt --check: PASS
rtk cargo check: PASS
rtk cargo clippy --lib --all-features -- -D warnings: PASS
rtk cargo clippy --all-targets --all-features -- -D warnings: BLOCKED
  unrelated existing user change: unused ClientCommand::CaptureKeyboard in
  src/native_output/tests/input_protocol.rs:44
rtk cargo test native_output::pacing --no-fail-fast: PASS (43 passed)
rtk git diff --check: PASS before report creation
```

Two fresh repository-wide test runs each encountered one different unrelated
flaky test. Their exact isolated reruns passed:

```text
rtk cargo test: 2065 passed, 1 failed, 2 ignored
  failed once: native::kms::tests::explicit_atomic_flip_adopts_out_fence_and_closes_input_after_success
isolated rerun: 1 passed

rtk cargo test: 2066 passed, 1 failed, 2 ignored
  failed once: native_output::kms_worker::task4_tests::dependent_cursor_waits_for_the_exact_predecessor_pageflip_before_testing
isolated rerun: 1 passed
```

The final post-report verification repeated the same pattern:

```text
rtk cargo test: 2065 passed, 1 failed, 2 ignored
  failed once: native::kms::tests::explicit_atomic_flip_closes_kernel_written_out_fence_on_ioctl_failure
isolated rerun: 1 passed
```

The full suite therefore has not produced a clean single-run result in this
environment; the failures were outside the touched files and did not recur in
isolated reruns. The release build passed:

```text
rtk cargo build --release: PASS
sha256(target/release/oblivion-one):
5595c5f61c1f671b2676a73329f49b2bc6856a91d094e6c278ad507050d8ca26
```

## Native qualification

The approved launcher was run with the requested 1920x1080@165, Atomic,
native-egl-gbm, eager-XWayland, worker-auto, triple-auto, and Direct Scanout
off environment. Typhon reached:

```text
/dev/dri/card1
DP-1
1920x1080@165Hz (exact)
direct DRM
explicit Atomic EGL/GLES GBM
```

It then stopped at the pre-render Atomic TEST_ONLY commit with:

```text
Permission denied (os error 13)
```

No workload ran, so hardware counters for Predictive O1, cadence, wake,
DMA-BUF, OutputTransaction, protocol, and SafeDisable are unavailable and
must not be inferred from this attempt. No ydotool, desktop screenshot, Eclipse
modification, or machine-configuration change was used.

## Acceptance status

The source-level v3.2 observability closure is verified. Native hardware
acceptance remains inconclusive until the launcher can perform the pre-render
Atomic TEST_ONLY commit on a seat with the required permissions. The next
qualification should confirm nonzero submitted/presented O1 counts, exact
terminal reconciliation, no normal-wait inflation, zero physical recovery
failures/fatal violations, and the already accepted cadence, wake, DMA-BUF,
transaction, protocol, and SafeDisable invariants.
