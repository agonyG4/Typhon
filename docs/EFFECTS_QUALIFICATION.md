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

## GLES effect GPU timing

GPU effect timing is a separate opt-in instrumentation capability. Enable it
for a renderer process with:

```bash
TYPHON_EFFECT_GPU_TIMING=1
```

The default is disabled. When disabled, Typhon allocates no timer-query
objects, issues no timestamp commands, polls no query results, and emits no
GPU-timing diagnostics. When explicitly requested, timing is active only on a
live context with a confirmed timestamp-query path: GLES
`GL_EXT_disjoint_timer_query` (including the NVIDIA GLES path), or a valid
desktop core/`GL_ARB_timer_query` path with non-zero timestamp counter bits.
Unsupported contexts emit one stable `event=effect_gpu_timing_unsupported`
diagnostic and continue rendering normally; this does not trigger effect
fallback, repaint, or renderer shutdown.

Results are asynchronous. Typhon polls old pending end queries at a renderer
frame boundary, stops at the first unavailable FIFO query, and reads the start
and end timestamps only after that end query reports
`QUERY_RESULT_AVAILABLE`. Collection is capped at 64 resolved spans per
boundary. A result retains the source effect-execution `frame_id`, which may be
older than the frame in which the record is logged, and a monotonic `scope`
that distinguishes multiple graph executions associated with one frame.

When a graph-total query resolves, one aggregate record is emitted for that
scope. Durations are integer nanoseconds. The `*_pixels` fields are bounded
effect-space demanded-region areas, not literal fragment counts and not
allocated texture area; scaled pyramid passes therefore do not report their
physical target-FBO fragment count:

```text
typhon effect: event=effect_gpu_timing frame_id=<u64|unknown> scope=<u64> total_ns=<u64> capture_ns=<u64> normalize_ns=<u64> blur_downsample_ns=<u64> blur_upsample_ns=<u64> fragment_ns=<u64> blend_ns=<u64> mask_ns=<u64> composite_ns=<u64> postprocess_ns=<u64> timed_passes=<usize> dropped_passes=<usize> capture_pixels=<u64> normalize_pixels=<u64> blur_downsample_pixels=<u64> blur_upsample_pixels=<u64> fragment_pixels=<u64> blend_pixels=<u64> mask_pixels=<u64> composite_pixels=<u64> postprocess_pixels=<u64> query_pool_capacity=<usize> query_pool_high_water=<usize> dropped_spans=<usize> disjoint_invalidated_spans=<usize>
```

`capture_ns` and `capture_pixels` combine `SceneCapture` and `SurfaceCapture`;
`postprocess_*` maps `OutputPostProcess`. The other duration/pixel fields map
to their corresponding `RenderPassKind` categories. `timed_passes` counts
valid pass samples included in the aggregate. `dropped_passes` counts pass
spans that could not be published, including pool exhaustion and invalid
timestamp data. `query_pool_capacity` and `query_pool_high_water` are query
object counts. `dropped_spans` and `disjoint_invalidated_spans` are bounded
cumulative diagnostics for the profiler lifetime.

The active profiler preallocates a fixed pool of 4,096 query objects (2,048
timestamp-pair slots), sized from Typhon's 128-instance bound, the current
six-pass built-in blur instance shape, and two expected in-flight graph
scopes. Pool exhaustion drops only timing spans; it never changes effect
execution or rendering. No query is allocated in the per-pass path.

With `GL_EXT_disjoint_timer_query`, a `GPU_DISJOINT_EXT` event invalidates all
affected pending measurements. Their query slots are recycled, the invalidated
count is recorded, and later non-disjoint boundaries resume collection. A
timestamp with `end < start` is discarded rather than being subtracted with
unsigned underflow. Timer-query records are useful qualification evidence, but
GPU timing alone is not hardware qualification: real hardware, display-mode,
presentation, and visual-acceptance runs remain required.

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
reports asynchronous per-graph GPU effect timing on supported contexts when
`TYPHON_EFFECT_GPU_TIMING=1` is set. It remains `UNAVAILABLE` when timing is
disabled, unsupported, disjoint-invalidated, or not enabled for the target
run.

## Current result

No live native run was performed for this checkout: `tty` reports `not a tty`,
`/dev/dri/renderD128` and an NVIDIA GeForce RTX 3060 Ti are available, and no
controlling `/dev/dri/card0` node is present. Therefore all native baseline,
blur, overlap, presentation-combination, custom-frame-demand, and Settings
visual-acceptance observations are `DEFERRED`, and no production-default
decision is claimed from performance data. Direct Scanout remains conservative
and effects continue to require composition whenever visible effect pixels are
present.
