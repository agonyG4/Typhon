# Typhon Native Frame Pacing v2.1 Verification Report

Date: 2026-09-03

## Scope and outcome

Native Frame Pacing v2 remains unchanged as an accepted architecture. This closure made two independent qualification-integrity corrections:

1. Fast-client tail qualification now follows exact outstanding-demand evidence instead of censoring intervals longer than four refreshes.
2. Exact no-pageflip DMA-BUF correlation retirement is produced by centralized output-transaction terminal settlement and consumed by the runtime observer.

Prediction remains telemetry-only. No scheduling policy, target policy, O1 admission, READY ownership, worker ownership, wake authority, client buffer ownership, or Astrea behavior changed.

## A. Fast-client censorship root cause and correction

The v2 fast-client continuity predicate called:

```rust
is_active_refresh_interval(elapsed_us, refresh_interval_us)
```

which accepted only `elapsed_us <= refresh_interval_us * 4`. At 165 Hz this excluded any valid compositor tail longer than approximately 24.24 ms, including a fast client whose next exact visual commit was already outstanding.

The correction removes that elapsed-time gate only from the dedicated fast-client population. The candidate still requires the existing exact evidence: one callback surface, exclusive surface damage, callback admission, the next visual commit, a fast callback reaction, and no callback-handoff limitation. The global active-pageflip cutoff and `idle_intervals_excluded` semantics remain unchanged.

This makes the causal rule explicit:

```text
fast callback admission
    -> same-surface next visual commit
    -> exact captured/presented content
    -> any physical delay remains a measurable fast-client tail
```

### Deterministic causal results

- One-refresh fast-client cadence remains continuous.
- A five-refresh compositor stall with a 500 us callback reaction is retained: one continuous sample, a 30,303 us interval, and one `missed_3x_or_more` bucket increment.
- A true idle gap has no next timely callback-requesting commit and remains excluded; its delayed callback reaction is client-limited and starts a fresh baseline.
- A 30 ms slow-client reaction is excluded from the fast-client tail population.
- Ambiguous multi-surface damage remains excluded.

## B. DMA-BUF correlation gap and central terminal architecture

v2 retired correlations in the READY and worker-queued physical-claim overtake callers. The audit found that queued shutdown abandonment and other exact transaction settlement paths could reach a terminal without notifying the DMA-BUF correlation observer.

`OutputTransactionLedger` now emits a transaction-keyed `SettledOutputTerminal` only after successful terminal finalization. It contains an `OutputPhysicalTerminal` classification, while remaining independent of sync files, GPU leases, fences, reactor tokens, and DMA-BUF ownership.

The runtime drains these notifications and calls the existing idempotent correlation observer. Caller-specific overtake retirement was removed. Draining occurs before pageflip validation after worker events, immediately after physical-claim recovery, and at worker, session, and output teardown boundaries.

### Audited terminal classes

- `Presented`: normal pageflip ownership; no no-pageflip retirement is emitted.
- `SafeAbandonment`: exact physical-claim overtake and proven shutdown abandonment; emits `NoPageflip::SafeAbandonment`.
- `Superseded`: the ledger rejects supersession from `Submitted`, so accepted supersession is a proven no-pageflip terminal.
- `OutputDestroyed` and `SessionSuspended`: emitted only by teardown/recovery paths after KMS ownership has been quiesced or released.
- Pre-physical-submission failure terminals from `Built`, `Ready`, or `Queued`: emit `NoPageflip::SubmissionRejected`.
- `NoVisualChange`: emits no physical notification; it is not a rendered physical claim.
- Submitted-state failures and protocol-settlement failures whose physical outcome is uncertain: emit no no-pageflip notification, preserving conservative correlation ownership.

The notification is keyed by the exact transaction ID. A no-pageflip retirement only removes that observability record. It does not complete a GPU lease, release a client buffer, close client synchronization ownership, alter retry debt, cancel a completion FD, or change token validation. A later GPU signal therefore completes its independent client-release lease without recreating a correlation or fabricating a pageflip pair.

### DMA-BUF deterministic results

- Normal GPU-signal/pageflip ordering in both directions remains paired.
- Physical overtake retirement remains idempotent.
- Queued shutdown SafeAbandonment retires only its own correlation.
- A later GPU signal/pageflip notification does not recreate or pair the retired correlation.
- Accounting remains `armed = paired + abandoned_without_pageflip + unpairable_signal_timestamp + pending`.
- Correlation capacity, duplicate, timestamp, and pending accounting behavior remains bounded and unchanged.

## Predictor and global cadence non-regression

The paired service estimator remains observability-only. `prediction.total_cost_ns`, render-start policy, target selection, and O1 admission were not changed. Existing paired-service tests continue to prove that READY, binding-target, and predecessor waits are excluded while same-frame work is included.

Global `primary_present_interval_*`, `active_pageflip_interval_*`, `missed_refresh_*`, and `idle_intervals_excluded` definitions were not redefined. The causal change applies only to the dedicated fast-client qualification population.

## Static verification

Targeted v2.1 Rust formatting check (with child-module traversal disabled so
unrelated shared-checkout edits are not included):

```text
rtk run -- rustfmt --edition 2024 --check --config skip_children=true <changed Rust files>: PASS
```

Production compilation in the shared checkout:

```text
rtk cargo check: BLOCKED
  unrelated pointer-constraint edits leave PointerConstraintRegionResolutionTiming
  fields private at src/native_output/input/routing.rs
```

Repository-wide checks were run fresh and are blocked by unrelated pre-existing pointer-constraint work in the shared checkout:

```text
rtk cargo fmt --check: BLOCKED
  existing formatting differences in pointer-constraint and native-pointer files

rtk cargo clippy --all-targets --all-features -- -D warnings: BLOCKED
  existing pointer-constraint integration reads private timing fields from
  src/native_output/input/routing.rs

rtk cargo test: BLOCKED
  same existing pointer-constraint integration compilation errors
```

The frame-pacing-specific test groups passed on the clean v2.1 tree before the
unrelated pointer-constraint edits made the shared checkout uncompilable:

```text
rtk cargo test native_output::pacing: 30 passed
rtk cargo test native_output::runtime::dmabuf_release: 32 passed
rtk cargo test native_output::tests::presentation_transactions: 62 passed
rtk git diff --check: PASS
```

## Native qualification

The existing qualification matrix was dry-run and resolved all 18 direct/triple/cursor phases. A real native attempt used the requested 1920x1080@165 configuration without ydotool or desktop screenshots. Typhon reached `/dev/dri/card1`, connector DP-1, exact 1920x1080@165 Hz, direct DRM fallback, and the explicit Atomic EGL/GLES GBM backend. It then stopped at the pre-render atomic TEST_ONLY commit with `Permission denied (os error 13)`. Consequently no Chromium/Electron sustained workload or native cadence/tail sample can be accepted from this environment.

The native attempt therefore provides startup-path evidence, not qualification evidence. A future run must execute the same matrix on a seat with permission to perform the TEST_ONLY commit and then inspect primary cadence, causal fast-client tails, clean DMA-BUF accounting, transaction terminal counters, physical recovery, and wake-authority counters.

## Adversarial review

- Can a five-refresh compositor stall disappear from the fast-client tail? No, when exact fast callback and next-commit evidence proves outstanding demand; elapsed duration no longer censors it.
- Can wall-clock duration alone classify idle? No.
- Can slow-client think time contaminate compositor tails? No; slow callback reaction fails the fast candidate.
- Can real idle contaminate fast-client tails? No; without a timely next visual commit it is excluded and establishes a new baseline later.
- Can queued shutdown SafeAbandonment retire its correlation? Yes, through the centralized transaction terminal drain.
- Can correlation retirement release a client buffer early? No; it only removes the observability correlation.
- Can a later GPU signal recreate a retired correlation? No.
- Can normal pageflip pairing be retired accidentally? No; presented terminals do not emit no-pageflip events.
- Can one no-pageflip terminal retire another transaction? No; the observer uses the terminal's exact transaction ID.
- Can an overtake double-retire? No; the caller-specific calls were removed and the observer is idempotent.
- Can the paired predictor become policy-authoritative? No; only telemetry fields were preserved.
- Can primary median cadence regress to every second refresh? The global cadence policy was untouched; native acceptance remains pending the permission-blocked hardware run.
- Can physical recovery become fatal? The existing recovery/fatal counters and paths were untouched; native acceptance remains pending.
- Can clean shutdown finish with an orphaned known physical correlation? Exact safe, superseded, teardown, and pre-physical failure terminals now retire centrally; uncertain submitted failures remain conservatively pending until their physical outcome is known.
