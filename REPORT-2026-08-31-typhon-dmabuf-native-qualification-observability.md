# Typhon DMA-BUF Native Qualification Observability

Date: 2026-08-31

## Scope and checkout

This closure adds bounded observability only. The current local checkout was
authoritative; no public repository snapshot was used for implementation. The
pre-existing pointer constraint commit (`0a7005c`) was left untouched.

The v1-v1.3 DMA-BUF release coordinator remains the ownership authority. This
change does not alter release transfer, current-token revalidation, retry debt,
Direct/KMS exclusion, `NativeRenderFence` ownership, or protocol completion.

## Current root cause

Before this change, `DmabufGpuReleaseWatch` held only a lease ID and completion
FD. `service_ready()` dropped the FD and completed the exact lease, but did not
query the sync-file's physical signal timestamp. Normal composited release
arming also had no `OutputTransactionId` metadata. The Atomic pageflip path did
already have the kernel-derived `presented_at_ns`, but it was not connected to
DMA-BUF release metrics.

Consequently, the runtime could prove that GPU release leases worked, but could
not prove the physical ordering of GPU completion against the corresponding
pageflip. Event-loop callback order was intentionally not used as evidence.

## Before/after observability flow

Before:

```text
rendered frame -> release FD -> reactor -> protocol completion
                                             + no physical GPU timestamp
pageflip -> existing output transaction/presentation handling
             + no DMA-BUF correlation
```

After:

```text
normal Atomic composited frame
    -> exact OutputTransactionId + obligation count + registration timestamp
    -> bounded correlation ledger
    -> readable sync-file FD
    -> query_sync_file_info(signal_timestamp_ns)
    -> exact GPU timestamp recorded
    -> existing exact protocol completion

Atomic composited pageflip
    -> existing kernel-derived presented_at_ns
    -> lookup by the same OutputTransactionId
    -> timestamp comparison, independent of callback delivery order
```

`NoVisual` and `DeferredRetry` watches carry explicit origins and are counted
for GPU-fence completion/wait observability, but never create a physical
pageflip correlation.

## Implementation

Changed files:

- `src/native_output/runtime/dmabuf_release.rs`: added
  `DmabufGpuReleaseOrigin`, a fixed-capacity 256-entry transaction correlation
  ledger, bounded wait/lead/lag samples using the existing `BoundedSamples`,
  exact sync-file timestamp querying before FD disposal, and qualification
  counters/snapshots. Timestamp-query failures are recorded and do not prevent
  an already-readable GPU lease from completing.
- `src/native_output/runtime/presentation_cycle.rs`: attaches `NoVisual` and
  normal `Composited { transaction_id }` origin metadata; deferred retry watches
  are tagged `DeferredRetry`. Normal composited registration receives the exact
  `OutputTransactionId` from the rendered outcome.
- `src/native_output/runtime/cycle/pageflip.rs`: adds one metrics-only hook in
  the Atomic composited pageflip path using the existing `presented_at_ns` and
  exact completed transaction ID.
- `src/native_output/runtime/mod.rs`: emits one bounded
  `event=dmabuf_gpu_release_timing_summary` line at teardown, alongside the
  existing release summary.
- `docs/superpowers/plans/2026-08-31-typhon-dmabuf-native-qualification-observability-plan.md`:
  records the focused design and verification plan.

No changes were made to `src/egl_renderer/native_fence.rs`: its existing
independent completion-FD duplication remains the source used by the release
registry, while submission and timing FD ownership remain separate.

## Physical timestamp authority

GPU timestamps come only from `query_sync_file_info()` on the readable
completion FD. Approximate event-loop time is not substituted. If querying
fails, `signal_timestamp_unavailable` increments and the safe protocol release
still proceeds.

Pageflip timestamps come only from the existing DRM/kernel conversion into
`presented_at_ns` in `cycle/pageflip.rs`. The ledger pairs entries by
`OutputTransactionId`, never by surface, buffer, frame counter, pageflip order,
or latest transaction.

Both delivery orders are supported:

```text
GPU signal callback -> pageflip callback
pageflip callback   -> GPU signal callback
```

Classification uses the two physical timestamps. Equal timestamps are recorded
separately and are not classified as before-pageflip.

The ledger is capped at 256 entries. Overflow and duplicate transaction IDs
increment bounded counters and do not affect release behavior.

## Deterministic RED/GREEN coverage

The RED run was performed before the implementation API existed. The focused
test command failed with 28 compile errors for the missing observability type,
correlation capacity, origin registration, timestamp-aware service path, and
qualification summary. This established that the new tests were exercising
new behavior rather than passing against an existing implementation.

The added deterministic tests cover:

1. GPU timestamp before pageflip, including obligation count and lead time.
2. Pageflip before GPU timestamp, including lag time.
3. Equal timestamp classification.
4. Opposite event delivery orders producing timestamp-based results.
5. Unavailable timestamp recording while the safe lease completion still runs.
6. `NoVisual` and `DeferredRetry` exclusion from physical correlation.
7. Existing bounded percentile semantics for fence wait samples.
8. Registration/signal timestamp inversion exclusion from wait percentiles.
9. Fixed correlation capacity and overflow accounting.
10. Isolation of multiple transaction IDs.
11. Duplicate correlation detection without replacing the original entry.
12. Actual registry readiness completion exactly once with metrics-only pairing.

Focused GREEN result:

```text
cargo test: 19 passed, 3123 filtered out (19 suites, 0.01s)
```

## Static/unit verification

Completed successfully:

```text
rtk cargo fmt --check
rtk cargo check
rtk cargo clippy --all-targets --all-features -- -D warnings
rtk cargo test
git diff --check
```

Full test result:

```text
cargo test: 3147 passed, 5 ignored, 40 filtered out (30 suites, 45.83s)
```

Strict Clippy reported no issues. Formatting and diff checks produced no
diagnostics. The final working tree before commit contained only the four
runtime source files and the two closure documents listed above.

## Native DRM/KMS verification

Not run. This environment did not provide a readable DRM card (`/dev/dri/card0`)
and the process did not have a TTY (`stdin_not_tty`). Although `/dev/tty0` was
present, the required real DRM/KMS access was unavailable. No native
qualification log was generated and no physical qualification result is
claimed.

Therefore the exact native summary lines requested for a hardware run are
unavailable for this checkout session. The new runtime will emit, at shutdown:

```text
typhon pacing: event=dmabuf_gpu_release_timing_summary composited_correlations_armed=... composited_correlations_paired=... release_before_pageflip_leases=... release_before_pageflip_obligations=... release_after_pageflip_leases=... release_after_pageflip_obligations=... release_same_timestamp_leases=... exact_signal_timestamps=... signal_timestamp_unavailable=... timestamp_order_anomalies=... correlation_pending=... correlation_overflows=... correlation_duplicates=... gpu_release_fence_wait_p50_us=... gpu_release_fence_wait_p95_us=... gpu_release_fence_wait_p99_us=... release_to_pageflip_lead_p50_us=... release_to_pageflip_lead_p95_us=... release_to_pageflip_lead_p99_us=... pageflip_to_release_lag_p50_us=... pageflip_to_release_lag_p95_us=... pageflip_to_release_lag_p99_us=...
```

The existing `TYPHON_FRAME_PACING_DEBUG=1` summary remains the authority for
O1 render-ahead and 165 Hz pacing metrics; no pacing fields were duplicated or
modified.

## Non-regression evidence

Source review and the full test suite show that this diff:

- does not change DMA-BUF ownership or protocol release terminals;
- does not change current-token revalidation or deferred retry/backoff;
- does not change Direct/KMS release authority or worker safety;
- does not change `NativeRenderFence` submission/timing FD semantics;
- does not change O1 callback admission or KMS scheduling;
- does not change SHM materialization-bound release;
- does not change regional damage or output buffer-age repair;
- adds no per-frame stdout or per-pageflip logging;
- adds no thread, lock, blocking wait, `glFinish`, busy polling, global surface
  scan, or unbounded ledger.

The new sync-file ioctl runs only when a DMA-BUF release completion FD is
already reported readable. Observability failures are non-fatal after the
existing safe completion point.

## Residual limitations and follow-up

- Real DRM/KMS qualification remains outstanding and must be run on an
  appropriate Linux TTY with the specified O1 configuration. It must verify
  `composited_correlations_paired > 0`, exact signal timestamps, and
  `release_before_pageflip_leases > 0` while reviewing the existing pacing
  summary.
- Unmatched transactions are retained only within the bounded ledger and are
  reported as `correlation_pending` at shutdown; no fake pageflip classification
  is generated for dropped or superseded transactions.
- `NoVisual` and deferred-retry fences intentionally do not participate in
  GPU-vs-pageflip pairing because they have no corresponding physical output
  transaction requirement.
