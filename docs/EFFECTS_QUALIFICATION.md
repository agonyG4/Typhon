# Effects qualification

Status: effects source closure is implemented and the library/strict-library
checks pass, but fresh all-target/test closure evidence is currently blocked by
uncommitted unrelated explicit-sync test edits in this checkout. Native
TTY/DRM qualification is also blocked because the process has no controlling
TTY. The available DRM node and NVIDIA GPU do not satisfy the real-TTY
prerequisite.

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

The closure also reruns the focused `resources`, `static_texture`, trusted
wrapper GLES-link, component-range, and effect-reload filters.
The previous checkpoint recorded: `effects` 103 passed, `egl_renderer` 124
passed, `resources` 18 passed, `static_texture` 3 passed, `native_output` 1162
passed, and the full suite 3658 passed with 5 ignored. Those are historical
checkout evidence, not fresh evidence for this closure.

The fresh attempt reached `cargo check --locked --lib` and
`cargo clippy --locked --lib -- -D warnings` successfully. The required
all-target/test commands cannot currently compile the checkout because
uncommitted explicit-sync changes leave `ExplicitSyncPoint` test constructors
and struct literals inconsistent (`src/compositor/state/frame_tests.rs` and
`src/compositor/explicit_sync.rs`). No fresh suite count is claimed until that
pre-existing checkout condition is resolved.

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
although `/dev/dri/card0`, `/dev/dri/renderD128`, and an NVIDIA GeForce RTX
3060 Ti are available. Therefore all native baseline, blur, overlap,
presentation-combination, and custom-frame-demand measurements are `DEFERRED`,
and no production-default decision is claimed from performance data. Direct
Scanout remains conservative and effects continue to require composition
whenever visible effect pixels are present.
