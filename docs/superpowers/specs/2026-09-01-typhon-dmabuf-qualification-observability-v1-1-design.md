# Typhon DMA-BUF Qualification Observability v1.1 Design

Date: 2026-09-01

## Goal

Close the registration-timing semantics gap in the existing DMA-BUF GPU release
qualification observability without changing runtime ownership, release timing,
protocol completion, Direct/KMS safety, retry scheduling, O1, SHM, damage,
buffer age, or KMS scheduling.

## Preserved architecture

The accepted physical proof remains:

```text
Composited GPU release lease
    -> exact OutputTransactionId

readable sync-file
    -> query_sync_file_info()
    -> exact signal_timestamp_ns

matching composited pageflip
    -> kernel-derived presented_at_ns

same OutputTransactionId
    -> compare physical timestamps
```

`NativeRenderFence` is created by the rendered-frame path before
`AtomicFrameRenderOutcome::Rendered`. The later composited release arm
duplicates the ready frame's completion FD and records userspace async-watch
registration time. Registration time is therefore not fence creation time.

## Registration timing semantics

The existing registry-wait percentile fields remain named
`gpu_release_registry_wait_p50_us`, `gpu_release_registry_wait_p95_us`, and
`gpu_release_registry_wait_p99_us`. They measure remaining wait from async-watch
registration to the exact sync-file signal timestamp.

For an exact timestamp:

- `signal_timestamp_ns >= registered_at_ns` records the difference as a wait
  sample.
- `signal_timestamp_ns < registered_at_ns` increments
  `already_signaled_before_registration`, records a zero wait sample, and does
  not increment `timestamp_order_anomalies`.

Physical release-before/after/same classification continues to use only the
exact signal timestamp and kernel-derived pageflip timestamp. Registration time
never participates in that comparison.

## Unpairable timestamp failures

Timestamp lookup continues after the readable completion FD is serviced, and
protocol release continues exactly once even when lookup fails. The watch
origin is passed to timestamp-unavailable handling:

- `Composited { transaction_id }` removes that exact transaction from the
  bounded correlation ledger if present and increments the bounded
  `correlations_unpairable_signal_timestamp` counter.
- `NoVisual` and `DeferredRetry` increment only the normal
  `signal_timestamp_unavailable` counter and remain outside physical pairing.

No synthetic timestamp or physical classification is produced.

## Tests

Focused unit tests cover pre-signaled zero wait, post-registration wait, mixed
percentiles including zero, physical classification independence, unavailable
composited timestamp eviction, repeated unavailable timestamps beyond the
256-entry capacity, duplicate/capacity arm accounting, and exactly-once release
completion. The existing bounded ledger, exact transaction pairing, origin
exclusion, retry debt, and safety tests remain unchanged.

## Scope

Only `src/native_output/runtime/dmabuf_release.rs` and this design/plan
documentation may change. No native DRM/KMS qualification is run from this
environment.
