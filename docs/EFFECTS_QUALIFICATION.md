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

The capture attribution fields append the following integer values to the same
one-line record:

```text
scene_capture_ns surface_capture_ns
replay_capture_ns framebuffer_capture_ns checkpoint_capture_ns
scene_capture_passes surface_capture_passes replay_capture_passes framebuffer_capture_passes checkpoint_capture_passes
scene_capture_pixels surface_capture_pixels replay_capture_pixels framebuffer_capture_pixels checkpoint_capture_pixels
capture_execution_summary_available
capture_execution_pixels scene_capture_execution_pixels surface_capture_execution_pixels replay_capture_execution_pixels framebuffer_capture_execution_pixels checkpoint_capture_execution_pixels
replay_capture_execution_passes framebuffer_capture_execution_passes checkpoint_capture_execution_passes
replay_capture_commands checkpoint_dependency_edges
max_capture_pass_ns max_capture_pass_id max_capture_instance_id max_capture_kind max_capture_mode max_capture_pixels max_capture_checkpoint_count
```

`capture_pixels` and its SceneCapture/SurfaceCapture and replay/framebuffer/
checkpoint splits retain the legacy unit: bounded effect-space demanded-region
area. `capture_execution_pixels` is a separate physical-work unit: the sum of
the pixels in the materialized capture rectangles actually executed. Direct
framebuffer capture uses its full target-texture domain; replay capture uses
the existing materialization rectangles. The mode-specific execution pixel
fields use the same physical unit. `capture_execution_summary_available=0`
means execution metadata was unavailable, such as after an effect execution
error; the execution fields are then zero.

`replay_capture_ns` and `framebuffer_capture_ns` are the existing capture pass
timestamp spans classified by the actual executor mode. `checkpoint_capture_ns`
is the subset whose pass has one or more `checkpoint_dependencies`, regardless
of capture mode. The corresponding `*_capture_passes` and `*_capture_pixels`
fields count and sum only valid resolved GPU spans. The
`*_capture_execution_passes` fields count successful physical capture passes
in the scope-owned execution summary. `replay_capture_commands` is the number
of command indices submitted to `draw_capture_commands_for_regions`; direct
framebuffer captures contribute zero. `checkpoint_dependency_edges` is the
sum of the dependency-list lengths for physically executed capture passes.

`max_capture_*` describes the single slowest valid timed capture pass in the
scope. Its stable kind values are `scene` and `surface`, and its mode values
are `replay` and `framebuffer_blit`; scopes without a valid capture pass emit
`none` and zero values. All capture mode and checkpoint metadata is derived at
the executor pass-timing call site from `is_direct_framebuffer_capture` and
`checkpoint_dependencies`, then carried by the existing timestamp span.

Replay Capture Attribution v2 appends bounded structural and host-side fields
to the same line. The units are counts unless a field ends in `_ns`, which is
integer nanoseconds:

```text
replay_capture_materialization_rects replay_capture_execution_regions replay_capture_disjoint_overflows
replay_capture_command_region_pairs replay_capture_scene_scan_pairs
replay_capture_planner_commands_visited replay_capture_planner_commands_drawable
replay_capture_commands_executed replay_capture_draw_calls
replay_capture_host_cpu_ns replay_capture_selection_cpu_ns replay_capture_visibility_cpu_ns replay_capture_draw_submit_cpu_ns
max_capture_execution_pixels max_capture_materialization_rects max_capture_execution_regions max_capture_disjoint_overflow
max_capture_replay_commands max_capture_command_region_pairs max_capture_scene_commands max_capture_scene_scan_pairs
max_capture_planner_commands_visited max_capture_planner_commands_drawable max_capture_commands_executed max_capture_draw_calls
max_capture_host_cpu_ns max_capture_selection_cpu_ns max_capture_visibility_cpu_ns max_capture_draw_submit_cpu_ns
```

`replay_capture_commands` retains its legacy meaning: the number of candidate
command indices selected once for each Replay capture pass. It is not a region
multiplier. `replay_capture_command_region_pairs` is the actual accumulated
number of candidate-command visibility evaluations returned by
`plan_capture_visibility()` across all post-canonicalization execution
regions. It can differ from `replay_capture_commands * execution_regions` when
indices are malformed or future filtering changes the work.

`replay_capture_scene_scan_pairs` is the actual number of scene-command
iterations performed by the existing draw phase across all Replay execution
regions. It measures full-scene scanning and is intentionally independent of
candidate planning. `max_capture_scene_commands` is the total scene command
count observed by the max pass, so native analysis can compare it with
`max_capture_scene_scan_pairs`. `*_planner_commands_*` come from the existing
visibility planner, while `*_commands_executed` and `*_draw_calls` come from
the existing renderer draw counters; occluded, outside-damage, and unavailable
commands are not counted as executed draws.

`replay_capture_materialization_rects` counts incoming Replay materialization
rectangles. `replay_capture_execution_regions` counts the actual rectangles
iterated after the existing bounded disjointification. A value of one for
`replay_capture_disjoint_overflows` means that bounded disjointification
overflowed and execution used its existing conservative bounding rectangle;
the fallback behavior itself is unchanged. The corresponding
`max_capture_*` fields are copied from the exact valid timed pass that became
the longest capture span. Framebuffer-blit max passes emit stable zero Replay
fields and do not fabricate Replay detail.

`replay_capture_host_cpu_ns` measures the whole Replay capture path while GPU
effect profiling is active. It includes fixed setup such as materialization,
render-target binding, clear/scissor/uniform setup, region canonicalization,
resource restoration, and any host time spent inside GL/driver calls.
`replay_capture_selection_cpu_ns` covers CaptureLayer/VisualGroup metadata and
`indices_for_capture()`. `replay_capture_visibility_cpu_ns` sums
`plan_capture_visibility()` across actual regions. `replay_capture_draw_submit_cpu_ns`
covers the existing visibility-driven draw submission after planning,
including GL/driver calls. These clocks are not read when effect GPU profiling
is disabled, so the CPU fields remain zero/unavailable outside the active
profiler gate.

Capture timing includes any GPU idle or ordering dependency that occurs between
the existing begin-pass and end-pass timestamp commands. In particular,
`framebuffer_capture_ns` is not a pure memory-copy bandwidth measurement: a
framebuffer blit may include GL dependency, resolve, and cache-ordering costs
required by the command stream.

The same interpretation applies to `replay_capture_ns`: it is the existing GPU
timestamp interval around Replay capture execution, not necessarily pure shader
execution time. It may include GPU idle or ordering/dependency waits while the
host is still constructing or submitting Replay commands. Host CPU phase fields
are provided specifically to distinguish that ambiguity; they do not prewarm,
synchronize, or otherwise alter client textures or capture policy.

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
