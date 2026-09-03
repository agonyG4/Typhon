# Typhon Native Frame Pacing v2 — Verification Report

Date: 2026-09-03

## Result

Deterministic qualification passed. The v2 implementation adds a separate,
surface-qualified fast-client cadence population, observability-only DMA-BUF
correlation retirement, and paired same-frame service telemetry. The physical
native cadence and predictor policy were not redesigned.

## Delivered changes

* `SurfaceDamagePresentation` now identifies an exclusive surface sample.
  Callback `surface_id` is carried from compositor callback evidence through the
  explicit rendered/presented frame into pacing observations.
* Fast-client samples require exact surface identity, exclusive visual damage,
  fast callback reaction, a same-surface preceding sample, an actual primary
  distance, and an active physical interval. The first sample establishes the
  baseline; no repaint or synthetic demand is generated.
* `native_content_frame_clock_summary` now exports bounded fast-client interval,
  distance, miss-bucket, and target/render/submit/KMS attribution fields.
* `retire_composited_without_pageflip` removes only a correlation-map entry after
  the exact SafeAbandonment recovery terminal. It does not touch GPU leases,
  protocol obligations, retry state, fences, buffers, current tokens, or KMS
  ownership. Retirement is idempotent, and later GPU completion still completes
  the real lease.
* Paired service observations use exact sync-file render completion plus the
  submit service interval. READY waits, binding/target waits, predecessor waits,
  client reaction, and idle gaps are not included. The 120-sample journal reports
  `ColdStart`, `WarmPaired`, and bounded `MissRecovery` telemetry.

## Evidence

The deterministic 165 Hz fast-client test used a virtual `6_060_606 ns` cadence,
500 µs callback reaction, repeated same-surface visual commits, and no wall-clock
sleep. It produced 127 continuous samples at a 6,060 µs p50 primary interval and
actual primary distance 1; all samples were target hits. Idle, slow, and ambiguous
observations did not increase the population.

DMA-BUF tests cover GPU-before-pageflip, pageflip-before-GPU, retirement-before-
either-event, duplicate retirement, unpairable timestamps, pending accounting,
and real lease completion after observability retirement. The accounting assertion
is:

```text
armed = paired + abandoned_without_pageflip
      + unpairable_signal_timestamp + currently_pending
```

Paired-predictor tests cover unrelated-tail separation, same-frame tail inclusion,
miss escalation/decay, and exclusion of the READY/binding/predecessor wait. The
RED evidence did not establish that the existing independent estimator materially
overestimates same-frame service, so the predictor remains measurement-only in
this change. No global-vs-fast cadence policy comparison was required because no
policy changed.

## Verification commands

All commands were run through `rtk`:

```text
rtk cargo fmt --check                         PASS
rtk cargo check                              PASS
rtk cargo clippy --all-targets --all-features -- -D warnings  PASS
rtk cargo test                                PASS — 3275 passed, 5 ignored
rtk git diff --check                          PASS
```

One intervening parallel-suite invocation exposed a single existing KMS fd
assertion (`expected -1, observed 1`). The named test passed five isolated runs
and the full suite passed with `--test-threads=1`; the required normal parallel
invocation then passed as well. No unrelated KMS code was changed.

Focused results included 31 DMA-BUF release tests, the fast-client pacing test,
and the paired adaptive-journal tests. The implementation commits are:

* `8526b05` — design and plan;
* `08b1580` — v2 observability implementation;
* `7e2c396` — verification-gate fixes; and
* `e5c8d8c` — fast-client summary assertions.

## Native qualification

`bin/qualify-presentation --dry-run` passed and enumerated all 18 labeled matrix
phases. The native launcher dry-run resolved the release binary at
`target/release/oblivion-one`. A live TTY/DRM run was not executed: this process
reports `not a tty`, `/dev/tty0` is unavailable, and the launcher also reports
that the user is not in the `input` group. `/dev/dri/card1` is present, but that
alone is insufficient for a native session qualification. This is therefore
`REAL NATIVE QUALIFICATION: DEFERRED`, not a native pass.

No Astrea source, compositor configuration, or external input automation was
changed.
