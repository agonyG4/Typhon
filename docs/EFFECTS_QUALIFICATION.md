# Effects qualification

Status: fresh deterministic closure gates pass; native TTY/DRM qualification is
blocked in this environment because the process has no controlling TTY. The
available render node and NVIDIA GPU do not satisfy the real-TTY prerequisite.

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
rtk cargo test --locked
rtk bin/qualify-presentation --dry-run
```

The Surface/VisualGroup effect-surface prerequisite is included in this
deterministic closure. The four direct regressions cover production public and
trusted resolution, exact Surface versus complete VisualGroup composition
ranges, child scene order independent of effect identifiers, and overlapping
child blur checkpoint dependencies including the lower child content.

Fresh final results for the 2026-09-09 effect-surface verification are:
`effects` 132 passed (90 library plus 42 main-target tests), `egl_renderer`
153 passed, `native_output` 1175 passed, and the full serial suite 3755
passed with 5 ignored and 40 filtered across 31 suites. Formatting, locked
all-target compilation, and strict all-target Clippy passed. An intermediate
isolated `native_output` attempt reproduced a timing-sensitive failure; its
final isolated rerun passed 1175/1175, and the fresh serial suite also passed.
The dry-run enumerated all 18 phases.

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

No live native run was performed for this checkout: `tty` reports `not a tty`,
`/dev/dri/renderD128` and an NVIDIA GeForce RTX 3060 Ti are available, and no
controlling `/dev/dri/card0` node is present. Therefore all native baseline,
blur, overlap, presentation-combination, custom-frame-demand, and Settings
visual-acceptance observations are `DEFERRED`, and no production-default
decision is claimed from performance data. Direct Scanout remains conservative
and effects continue to require composition whenever visible effect pixels are
present.
