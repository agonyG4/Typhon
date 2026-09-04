# Typhon Native Frame Pacing v3.2 — Predictive O1 Observability Design

## Status

Approved for implementation from the v3.2 observability reconciliation request.

## Goal

Make Predictive O1 telemetry an exact per-frame lifecycle record without changing O1 admission, deferred binding, physical claims, KMS timing, wake ownership, client attribution, DMA-BUF ownership, or OutputTransaction policy.

## Root causes

`waits_for_target` describes a generic pipeline condition. It can be true for a normal composited frame that is waiting behind an Atomic/KMS lane, so using it as the Predictive O1 test inflates `predictive_render_ahead_ready` and `predictive_ready_created`.

The existing `predictive_ready_frame_id: Option<NativeOutputFrameId>` is also a single mutable pointer. The native pipeline can retain an older frame at a worker or submitted boundary while a newer frame reaches READY, so the newer ID can overwrite the older frame before its terminal callback arrives.

## Design

### Exact origin

`PreparedFrameOrigin` is computed at the render-start boundary from the existing pacing mode and render-ahead decision:

- `Normal` for ordinary composited work;
- `ReactiveDouble` for reactive-double work;
- `PredictiveO1` only for `PredictiveTriple` work with `render_ahead == true`.

The origin is stored beside the active pacing frame identity. A Predictive O1 lifecycle entry is inserted at that same boundary, so later code never infers origin from `waits_for_target`, a mutable pacing mode, or a current frame pointer.

### Bounded lifecycle ledger

`NativeFramePacing` owns a fixed four-entry array keyed by the exact `NativeOutputFrameId`. Four entries cover the existing pacing ownership boundaries (`active`, `ready`, `worker reservation`, and `pending`) without introducing an unbounded collection, a second transaction ledger, a thread, a timer, or retained per-frame history.

Each entry has one current stage and an observed-stage bitset. Stage transitions are recorded at the existing ownership boundaries:

1. `Rendering` at Predictive O1 admission;
2. `RenderReady` after the exact render completion;
3. `ReadyUnbound` only when the exact deferred frame remains unbound;
4. `Bound` only after the existing deferred binding succeeds;
5. `WorkerQueued` only when the exact worker reservation is accepted;
6. `Submitted` only after the existing kernel-submit ownership boundary succeeds;
7. `Presented` only after the exact physical pageflip terminal.

The stage counters are monotonic observations and may overlap. A binding that succeeds at render completion intentionally skips `ReadyUnbound`; its identity is recorded as `RenderReady -> Bound`. Immediate non-worker submission intentionally skips `WorkerQueued`.

### Terminal accounting

The same ledger entry is removed exactly once by one terminal outcome:

- physical pageflip: `presented`;
- stale predecessor identity: `abandoned_identity`;
- stale output/clock/pool generation: `abandoned_generation`;
- safe overtake, safe cancellation, or other non-fatal disposal: `other_safe_abandonment`;
- failed render/worker submission or duplicate-invalid lifecycle: `failed` where the existing path classifies it as failure;
- remaining exact entries during runtime drop: `current_at_shutdown`.

Duplicate terminal notifications find no live entry and do not increment a terminal counter. Existing legacy READY counters remain available with corrected exact-ID semantics; `predictive_ready_submitted` is no longer driven by a single global frame ID.

At summary time, terminal accounting is reported explicitly as:

```text
predictive_o1_created
= predictive_o1_presented
 + predictive_o1_abandoned_identity
 + predictive_o1_abandoned_generation
 + predictive_o1_other_safe_abandonment
 + predictive_o1_failed
 + predictive_o1_current_at_shutdown
```

Stage counters are separate from terminal counters. The implementation reports the active ledger population, peak population, terminal total, remainder, and an exact reconciliation boolean so unresolved ownership cannot be hidden.

### Event flow

```text
Predictive O1 origin on active frame
    -> ledger insert / created
    -> render completion / render_ready
    -> exact deferred frame state / ready_unbound
    -> existing bind / bound
    -> existing worker reservation / worker_queued
    -> existing kernel-submit acknowledgment / submitted
    -> existing physical pageflip / presented terminal
```

Normal READY waits increment their existing wait metric only. ReactiveDouble and Direct Scanout do not create Predictive O1 ledger entries or stage/terminal counts.

## Files and boundaries

- `src/native_output/pacing.rs`: owns the origin classification, bounded exact-ID ledger, stage counters, terminal counters, compatibility counter mappings, and summary fields/tests.
- `src/native_output/runtime/presentation_cycle.rs`: supplies the exact render-ready and failure/safe-abandonment boundaries and no longer uses generic `waits_for_target` as Predictive O1 identity.
- `src/native_output/runtime/cycle/pageflip.rs`: records exact bound and physical-presented transitions through the existing pageflip authority.
- `src/native_output/runtime/kms_worker/rejection.rs`: preserves safe worker cancellation while terminalizing the exact reserved identity.
- `src/native_output/runtime/mod.rs`: reconciles every remaining exact lifecycle entry during shutdown before printing the pacing summary.

`OutputTransaction`, `FramePresentationReservation`, `PrimaryRefreshClaim`, the predictor, scheduler policy, KMS worker policy, Direct Scanout, ReactiveDouble, fast-client attribution, DMA-BUF release ownership, and OutputTransaction semantics remain unchanged.

## Error handling and boundedness

Inserting a fifth live entry is an internal capacity violation and returns an error at the existing render-start boundary; it does not silently discard accounting. Exact-ID transition mismatches are rejected or ignored according to the existing lifecycle boundary, with no retargeting or physical-policy change. The fixed array is the only added state and is bounded by four existing pacing ownership slots.

## Verification

Tests cover normal waiting isolation, N Predictive O1 plus M normal-ready reconciliation, overlapping and reversed terminal ordering, duplicate submit terminalization, generation and identity abandonment, shutdown-current accounting, submitted-then-presented stage progression, ReactiveDouble/normal/direct isolation, and existing bound-overtake recovery. Static verification uses fresh `rtk cargo fmt --check`, `rtk cargo check`, `rtk cargo clippy --all-targets --all-features -- -D warnings`, `rtk cargo test`, and `rtk git diff --check`; native qualification uses the approved release build and launcher without modifying Eclipse or machine configuration.
