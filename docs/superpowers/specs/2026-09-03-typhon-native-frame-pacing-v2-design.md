# Typhon Native Frame Pacing v2 — Stage-Coherent Prediction and Fast-Client Tail Qualification

## Status

Approved implementation design for 2026-09-03. The implementation is intentionally
staged: measurement and observability land before any predictor policy change.

## Problem and invariants

Typhon already has an accepted native physical clock, with a primary cadence near
165 Hz (`6_060_606 ns`). That clock, pageflip authority, explicit output
transactions, DMA-BUF release ownership, callback ownership, KMS worker ownership,
and the existing SHM/DMA-BUF, input, XWayland, and direct-scanout paths remain the
authority. v2 must not turn advisory prediction into a second clock or a second
release owner.

The implementation preserves these invariants:

* only the physical pageflip settles physical presentation;
* a target reservation and an O1 admission are advisory/ownership records, not
  presentation proof;
* DMA-BUF correlation retirement removes only an observability entry and never
  releases a GPU lease, obligation, fence, buffer, retry debt, current token, or
  KMS/direct-scanout ownership;
* callback attribution is surface-local, callback-admission-local, transaction-
  local, and rejected when a batch is ambiguous;
* no synthetic repaint is generated to improve a metric;
* READY, commit-timing lower bounds, predecessor physical waits, client think time,
  and idle gaps are not silently counted as compositor service time.

## Phase A — fast-client continuous-content oracle

`FrameCallbackTimingEvidence.surface_id` is the existing exact-surface source. The
native output frame will carry that identity through its pageflip observation. The
same observation will also carry whether the captured `SurfaceDamagePresentation`
contains only that surface. A frame is eligible for the fast-client population only
when all of the following hold:

1. callback admission and the next callback-requesting commit have the same exact
   surface identity;
2. the callback timing was not discarded as an ambiguous multi-surface batch;
3. the visual damage captured for the output is exclusively attributable to that
   surface;
4. callback reaction is at or below `min(refresh / 2, 2 ms)`;
5. the preceding eligible frame has the same surface and the physical interval is
   active, not an idle gap; and
6. the observation has a real next visual frame and an actual primary distance.

The first eligible frame establishes the continuous-demand baseline; only the next
same-surface eligible visual frame contributes to the population. Slow, ambiguous,
idle, direct-scanout, and multi-surface observations reset or do not extend that
baseline. This distinguishes an early-producing client from an idle scene without
forcing damage or repaint.

The existing global callback samples remain intact. The content summary gains a
separate bounded population with these fields:

* `fast_client_primary_present_interval_p50_us/p95_us/p99_us`;
* `fast_client_actual_primary_distance_p50/p95/p99`;
* `fast_client_missed_refresh_1x`, `_2x`, `_3x_or_more`;
* `fast_client_target_hit`, `_target_limited`, `_render_limited`,
  `_submit_limited`, `_kms_limited`; and
* `fast_client_continuous_samples`.

The existing bounded sample cap and nearest-rank percentiles are reused. The
deterministic test uses virtual 165 Hz timestamps only, a 250–500 µs callback
reaction, in-budget render/submit/KMS stages, repeated same-surface damage, and
explicitly checks that idle and slow observations are excluded.

## Phase B — DMA-BUF correlation retirement

`DmabufGpuReleaseObservability` gains an idempotent API:

```text
retire_composited_without_pageflip(
    transaction_id,
    reason: DmabufCorrelationNoPageflipReason,
)
```

The reason enum is deliberately terminal and small (`SafeAbandonment` and
`Superseded` are the supported meanings). Current overtaken READY and queued-worker
recovery paths call it only after the exact output transaction is safely abandoned.
No generic “dropped” hook is added, because a drop is not proof that a correlation
cannot still receive a physical pageflip or a GPU signal.

Retirement removes the correlation map entry and increments
`correlations_abandoned_without_pageflip` exactly once. A later GPU signal still
completes its actual registry lease normally; it simply finds no observability
correlation. A pageflip that paired first makes retirement a no-op. Timestamp
unavailability increments the existing unpairable counter only when an armed
correlation was actually removed.

The qualification summary exposes the abandoned counter and retains pending map
length. Its clean-state accounting is:

```text
armed = paired + abandoned_without_pageflip
      + unpairable_signal_timestamp + currently_pending
```

The pending map is never cleared merely to make a summary look clean.

## Predictor evidence and conditional policy

The current `AdaptiveRenderJournal` remains the compatibility estimator while v2
collects paired same-frame service observations. A paired observation is composed
from render and submit service boundaries only; it excludes READY target waits,
commit-timing lower bounds, predecessor physical waits, client reaction time, and
idle gaps. The journal keeps at most the existing 120-sample evidence window and
reports the paired tail and estimator mode (`ColdStart`, `WarmPaired`, or
`MissRecovery`) for diagnosis.

The first implementation is measurement-only unless deterministic production-API
tests demonstrate material overestimation from independently combining unrelated
tails. If that RED evidence exists, warm mode may use the paired p95 service tail
with the existing explicit KMS apply guard, while preserving the advisory render
start deadline and all client/event-loop opportunity semantics. Miss recovery may
temporarily fall back to the conservative legacy estimate and decay only through
bounded successful paired observations. Client slowness, binding waits, and
predecessor physical waits must not enter the paired service sample.

Required tests cover unrelated-tail non-inflation, real same-frame tail inclusion,
miss escalation/decay, client slowness isolation, binding-wait isolation, and
predecessor-wait isolation. The policy remains unchanged when those tests do not
prove a material error. Global cadence is compared with the fast-client cadence
after any policy change.

## Validation and native boundary

Static tests are deterministic and use no wall-clock sleeps. The full verification
set is:

```text
rtk cargo fmt --check
rtk cargo check
rtk cargo clippy --all-targets --all-features -- -D warnings
rtk cargo test
rtk git diff --check
```

Native qualification is attempted only if this checkout exposes a documented
native-output command and hardware/session access. Otherwise the final report
records the exact blocker and leaves Astrea, compositor configuration, and external
input tooling untouched.
