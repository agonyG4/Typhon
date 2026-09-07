# Effects qualification

Status: deterministic implementation gates pass; native TTY/DRM qualification
is deferred.

This document records the reproducible procedure for the Typhon effects engine.
It does not convert unit tests, shader-source inspection, or the dry-run matrix
into hardware qualification.

## Deterministic gates

Run from the Typhon checkout so Cargo reuses `target/`:

```bash
rtk cargo fmt --all -- --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked effects
rtk cargo test --locked egl_renderer
rtk cargo test --locked native_output
rtk bin/qualify-presentation --dry-run
```

The dry-run enumerates 18 labeled combinations across direct scanout policy,
triple buffering, and cursor scheduling. It starts no compositor and measures no
GPU or presentation timing.

## Native matrix

The target profile is a real TTY/DRM session at `1920x1080@165`. The live
harness is:

```bash
OBLIVION_ONE_PERF_LOG=1 \
OBLIVION_ONE_QUALIFY_DURATION_SECONDS=120 \
OBLIVION_ONE_QUALIFY_COMMAND="$PWD/bin/start-oblivion-one-tty" \
  bin/qualify-presentation
```

The harness writes each labeled phase below
`$XDG_STATE_HOME/oblivion-one/qualifications/`, or below
`$HOME/.local/state/oblivion-one/qualifications/` when `XDG_STATE_HOME` is not
set. Inspect `session.log`, `environment.txt`, `summary.txt`, and the bounded
presentation trace for every phase.

Each native run must cover:

- no-effects idle baseline and fullscreen Direct Scanout eligibility;
- one static panel blur while idle, scrolling behind it, moving it, resizing
  its region, and repeatedly enabling/disabling it;
- overlapping blurred panels or popups with stable resource-cache growth;
- KMS worker policy, triple buffering, VRR/tearing mode, hardware/software
  cursor, explicit-sync clients, and Direct Scanout transitions;
- a localized continuous trusted shader while unrelated scene damage occurs.

Record CPU render p50/p95/p99, reliable GPU timing if available, target-slip or
missed-vblank counters, draw calls, texture binds, effect GPU-cache bytes, and
Direct Scanout state/blockers from the native perf lines. The effects renderer
currently has no reliable timer-query result to report, so GPU effect timing is
`UNAVAILABLE` unless the target run provides it independently.

## Current result

No real TTY/DRM run was performed for this checkout. Therefore all native
baseline, blur, overlap, presentation-combination, and custom-frame-demand
measurements are `DEFERRED`, and no production-default decision is claimed from
performance data. Direct Scanout remains conservative and effects continue to
require composition whenever visible effect pixels are present.
