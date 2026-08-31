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
  ledger, bounded registry-wait/lead/lag samples using the existing `BoundedSamples`,
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

Registry wait semantics are intentionally separate from physical correlation:
the registration timestamp is when the asynchronous reactor watch is installed,
not when the EGL fence is created. A sync-file signal timestamp earlier than
registration is recorded as `already_signaled_before_registration` and adds a
zero-microsecond registry-wait sample. It is not a clock anomaly. An exact
timestamp query failure removes the associated composited correlation as
unpairable while still allowing the already-safe protocol release to complete.

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

The v1.1 corrective focused suite added seven tests and passed as:

```text
running 26 tests
test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured; 1039 filtered out
```

## Static/unit verification

The v1.0 observability commit was fully verified before this v1.1 correction:

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

For v1.1, formatting still passed and the observability test executable passed
all 26 focused tests. The final full-suite rerun was interrupted by unrelated
pointer-constraint edits appearing concurrently in the working tree. Current
`cargo check` and strict all-target Clippy are blocked by that incomplete
pointer change, including missing `OutputPosition`/anchor state and mismatched
`ActivateLocked` fields. The independent pointer test reruns reproduce the same
source inconsistency. Those files are not part of this closure and were not
edited or staged.

The full-suite process that ran before the source inconsistency became visible
reported two unrelated pointer test failures:

```text
locked_activation_resolves_anchor_at_settlement_position: assertion mismatch
native_input_epoch_does_not_deliver_backlog_to_new_relative_pointer: assertion mismatch
```

No observability test failure was observed. The current worktree intentionally
retains the concurrent pointer files as user-owned changes.

## Native DRM/KMS verification

Not run. This environment did not provide a readable DRM card (`/dev/dri/card0`)
and the process did not have a TTY (`stdin_not_tty`). Although `/dev/tty0` was
present, the required real DRM/KMS access was unavailable. No native
qualification log was generated and no physical qualification result is
claimed.

Therefore the exact native summary lines requested for a hardware run are
unavailable for this checkout session. The new runtime will emit, at shutdown:

```text
typhon pacing: event=dmabuf_gpu_release_timing_summary composited_correlations_armed=... composited_correlations_paired=... release_before_pageflip_leases=... release_before_pageflip_obligations=... release_after_pageflip_leases=... release_after_pageflip_obligations=... release_same_timestamp_leases=... exact_signal_timestamps=... signal_timestamp_unavailable=... correlations_unpairable_signal_timestamp=... already_signaled_before_registration=... timestamp_order_anomalies=... correlation_pending=... correlation_overflows=... correlation_duplicates=... gpu_release_registry_wait_p50_us=... gpu_release_registry_wait_p95_us=... gpu_release_registry_wait_p99_us=... release_to_pageflip_lead_p50_us=... release_to_pageflip_lead_p95_us=... release_to_pageflip_lead_p99_us=... pageflip_to_release_lag_p50_us=... pageflip_to_release_lag_p95_us=... pageflip_to_release_lag_p99_us=...
```

The existing `TYPHON_FRAME_PACING_DEBUG=1` summary remains the authority for
O1 render-ahead and 165 Hz pacing metrics; no pacing fields were duplicated or
modified.

## v1.1 registration-timing correction

The first observability implementation treated
`signal_timestamp_ns < registered_at_ns` as `timestamp_order_anomalies`. The
current source ordering makes that case normal: `NativeRenderFence::create()`
can signal before the later composited release watch is installed. The metric
now records `already_signaled_before_registration` and contributes a zero
microsecond sample to the renamed `gpu_release_registry_wait_*` percentiles.
The registration timestamp remains explicitly documented as async-watch
registration time, not fence-creation time.

Physical ordering is unchanged and still compares only:

```text
exact sync-file signal_timestamp_ns
vs.
exact kernel-derived presented_at_ns
```

using the same `OutputTransactionId`. In particular, a GPU timestamp before
watch registration can still be classified as before the pageflip when its
physical timestamp is earlier.

An unavailable exact timestamp now receives the watch origin. For a
`Composited` watch, its correlation entry is removed immediately and counted as
`correlations_unpairable_signal_timestamp`; it cannot consume ledger capacity
or remain pending forever. `NoVisual` and `DeferredRetry` only increment the
normal unavailable-timestamp counter. In every case the already-safe protocol
release continues.

Additional v1.1 deterministic tests cover pre-signaled zero-wait accounting,
post-registration wait accounting, percentile inclusion of zero, physical
classification independence, unpairable correlation removal, repeated
unavailable timestamps without overflow, and successful-only correlation-arm
metrics.

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
