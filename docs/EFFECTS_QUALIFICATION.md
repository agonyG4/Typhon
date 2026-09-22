# Effects qualification

Status: checkpoint shader-copy is the production-preferred path based on the
direct path qualification recorded below. The post-promotion native run with
the path override unset is blocked here because the process has no controlling
TTY; an available render node does not satisfy that prerequisite.

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

## Checkpoint capture path policy

Checkpoint-dependent Replay `SceneCapture` prefers framebuffer shader-copy
when the current output is sampleable. The execution planner uses framebuffer
blit as the capability fallback when no sampleable output texture is available.
This is limited to the eligible checkpoint Replay case and does not widen
shader-copy to ordinary Replay captures, lifecycle backdrop captures,
framebuffer debug capture mode, `SurfaceCapture`, or other direct framebuffer
captures.

`TYPHON_EFFECT_DEBUG_CHECKPOINT_CAPTURE_PATH=blit` forces diagnostic framebuffer
blit. `TYPHON_EFFECT_DEBUG_CHECKPOINT_CAPTURE_PATH=shader-copy` explicitly
requests the preferred shader-copy path and retains the no-sampleable-output
fallback. When unset, the parser selects the same shader-copy preference. An
invalid value emits the existing bounded warning and uses that production
default. These choices are capability-based and do not inspect GPU vendor.

The direct checkpoint-path comparison was qualified on an NVIDIA RTX 3060 Ti
at `1920x1080@165 Hz` using Replay Attribution v2. The measurements are
qualification evidence, not CI thresholds:

| Measurement | Framebuffer blit | Shader-copy |
| --- | ---: | ---: |
| p50 per pass | 40.96 us | 12.29 us |
| p99 per pass | 806.83 us | 327.54 us |
| total GPU time | 310.75 ms | 126.01 ms |
| weighted ns/pixel | 0.2423 | 0.0742 |

The shader-copy session processed more total checkpoint pixels while using
substantially less measured GPU time. With checkpoint blit removed, the
remaining heavy frames were dominated by large Replay work, Kawase, and
Composite.

## Repaint provenance trace

With `TYPHON_EFFECT_EXEC_TRACE=1`, each rendered frame emits one bounded
`event=effect_repaint_provenance` record keyed by `frame_id`. It snapshots the
sequential pipeline stages: input damage authority, resolved scene damage,
effect graph merged damage, the initial repaint plan, and the final repaint
plan after effect execution resolution. Damage kinds use `none`, `empty`,
`rects`, and `full`; an unrepresentable pixel total uses the existing
`u64::MAX` telemetry sentinel.

`first_full_stage` names the first of those observed stages whose
representation is full-output. It describes where full-output first appears
in this pipeline and does not establish the ultimate source of the underlying
damage. `promoted_to_full` records whether the initial plan was non-full and
the final plan was full.

Join this record with `effect_demand_plan_begin` and `effect_demand_plan_end`
using `frame_id` for the existing detailed demand and repair evidence. Join with
`effect_gpu_timing` by `frame_id` as well; GPU timing results are asynchronous
and retain the source frame identifier. The provenance event does not duplicate
demand-plan metrics or add GPU queries.

### Damage complexity shadow policy

The same record includes a diagnostic-only Damage Complexity Shadow Policy for
the initial planner's pre-fallback repair candidate. It applies only when the
current planner reason is `too_many_rectangles` and the candidate has more than
eight rectangles. Its reference algorithm is:

```text
more than 8 rects
    ↓
bbox area <= actual area × 2?
    YES → simulate one bbox rectangle
    NO  → simulate retaining the original rectangles
    ↓
apply Typhon's existing 75% output-area threshold
```

The record preserves original, bbox, and selected-candidate counts and pixel
totals, along with the bbox coordinates, acceptance flag, outcome, and
`damage_complexity_would_avoid_full`. A value of
`damage_complexity_would_avoid_full=1` means the reference complexity rule
would avoid the rectangle-count-triggered Full fallback while still applying
Typhon's 75% area threshold. It is an area-based counterfactual, not a measured
performance improvement. In particular, `partial_many_rects` means the
simulation keeps the original multi-rectangle region; it does not establish
that rendering an arbitrary number of rectangles is faster.

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
replay_capture_ns framebuffer_capture_ns framebuffer_blit_capture_ns framebuffer_shader_copy_capture_ns checkpoint_capture_ns
scene_capture_passes surface_capture_passes replay_capture_passes framebuffer_capture_passes framebuffer_blit_capture_passes framebuffer_shader_copy_capture_passes checkpoint_capture_passes
scene_capture_pixels surface_capture_pixels replay_capture_pixels framebuffer_capture_pixels framebuffer_blit_capture_pixels framebuffer_shader_copy_capture_pixels checkpoint_capture_pixels
capture_execution_summary_available
capture_execution_pixels scene_capture_execution_pixels surface_capture_execution_pixels replay_capture_execution_pixels framebuffer_capture_execution_pixels framebuffer_shader_copy_capture_execution_pixels checkpoint_capture_execution_pixels
replay_capture_execution_passes framebuffer_capture_execution_passes checkpoint_capture_execution_passes
replay_capture_commands checkpoint_dependency_edges
max_capture_pass_ns max_capture_pass_id max_capture_instance_id max_capture_kind max_capture_mode max_capture_pixels max_capture_checkpoint_count
pass_timed_ns graph_unattributed_ns max_effect_pass_ns max_effect_pass_id max_effect_instance_id max_effect_kind max_effect_capture_mode
max_effect_pixels max_effect_damage_rects max_effect_damage_bbox_pixels max_effect_target_width max_effect_target_height
graph_gap_attribution_available max_graph_gap_ns max_graph_gap_position
max_graph_gap_after_pass_id max_graph_gap_after_instance_id max_graph_gap_after_kind max_graph_gap_after_capture_mode max_graph_gap_after_checkpoint_count
max_graph_gap_before_pass_id max_graph_gap_before_instance_id max_graph_gap_before_kind max_graph_gap_before_capture_mode max_graph_gap_before_checkpoint_count
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
timestamp spans classified by the actual executor mode. `framebuffer_capture_ns`
is the aggregate framebuffer-backed capture category; its
`framebuffer_blit_capture_*` and `framebuffer_shader_copy_capture_*` fields are
the path-specific duration, pass-count, and pixel splits. The
`framebuffer_shader_copy_capture_execution_pixels` field is the corresponding
physical execution-pixel split already carried by the execution summary.
`checkpoint_capture_ns`
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
are `replay`, `framebuffer_blit`, and `framebuffer_shader_copy`; scopes without a valid capture pass emit
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
max_capture_replay_detail_available
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
fields and do not fabricate Replay detail. `max_capture_replay_detail_available=1`
means the max capture pass was Replay and its exact `ReplayCaptureExecutionDetail`
was attached; `0` means the max pass was framebuffer capture or Replay detail was
unavailable. The existing zero-valued max Replay fields remain for compatibility
when the availability bit is zero.

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

### Composite Scene Replay Attribution v1

Composite Scene Replay Attribution measures only the ordinary scene replay
immediately associated with `reason=composite_advance` for a
`RenderPassKind::Composite` pass. A replay span is eligible only when a GPU
graph timing scope exists, `draw_end > scene_cursor`, the authoritative
`SceneReplayWorkState::active_work()` region is non-empty, and capture is not in
progress. Empty command ranges, empty active work, `OutputPostProcess`,
`checkpoint_dependency`, `framebuffer_capture`, `final_scene_replay`, and
capture-in-progress paths are not measured by this span.

The aggregate fields appended to the same `event=effect_gpu_timing` line are:

```text
composite_scene_replay_timing_available
composite_scene_replay_expected_spans composite_scene_replay_resolved_spans
composite_scene_replay_gpu_ns composite_scene_replay_host_cpu_ns
composite_scene_replay_scene_scan_pairs
composite_scene_replay_commands_executed
composite_scene_replay_draw_calls composite_scene_replay_texture_binds
```

The bounded max fields are:

```text
max_composite_scene_replay_gpu_ns
max_composite_scene_replay_pass_id max_composite_scene_replay_instance_id
max_composite_scene_replay_command_start max_composite_scene_replay_command_end
max_composite_scene_replay_command_count
max_composite_scene_replay_scene_commands
max_composite_scene_replay_active_work_rects
max_composite_scene_replay_active_work_pixels
max_composite_scene_replay_command_region_pairs
max_composite_scene_replay_scene_scan_pairs
max_composite_scene_replay_pending_checkpoints
max_composite_scene_replay_host_cpu_ns
max_composite_scene_replay_commands_considered
max_composite_scene_replay_commands_executed
max_composite_scene_replay_draw_calls
max_composite_scene_replay_texture_binds
max_composite_scene_replay_scene_vbo_uploads
max_composite_scene_replay_scene_vbo_upload_bytes
```

`command_start`, `command_end`, and `command_count` describe the logical
command range supplied to the scene replay. In particular,
`command_count = command_end - command_start` is not a count of physical
draws. `scene_commands_total` is the complete scene command-vector population.
The current `draw_command_batch_range()` implementation scans that complete
vector once for each active work rectangle, so `scene_scan_pairs` describes
`scene_commands_total * active_work_rects`. It is a workload metric for the
current full-vector scan and must not be interpreted as executed draws.

`command_region_pairs` describes the logical command range multiplied by the
active work-rectangle count. `active_work_rects`, `active_work_pixels`, and
`pending_checkpoint_requirements` are copied from the same normalized
`SceneReplayWorkState` region used by `draw_effect_scene_range()`; they are not
reconstructed by the profiler. `commands_considered`, `commands_executed`,
`draw_calls`, `texture_binds`, `scene_vbo_uploads`, and
`scene_vbo_upload_bytes` are physical execution counters derived from the
existing `GlesSceneFrameStats` counters. The profiler does not add duplicate
counting to the GL rendering loop.

`composite_scene_replay_gpu_ns` is the saturating sum of valid GPU timestamp
spans. `composite_scene_replay_host_cpu_ns` is the saturating sum of host time
around only `draw_effect_scene_range()`, with the required frame-stat snapshots
outside the draw boundary. Host CPU time is measured only after the replay span
is successfully allocated, and no host clock or profiler-only stats snapshot is
taken when GPU timing is disabled. The GPU END timestamp is issued immediately
after the draw returns and before execution-detail construction, so detail
building, aggregation, trace formatting, validation, and scene-validity updates
are outside the replay GPU interval.

CPU and GPU durations are independent measurements. CPU command submission and
GPU execution may overlap; the two durations must not be added together as a
decomposition of wall-clock frame time.

Availability is graph-scoped and complete only when every eligible replay span
has a valid timestamp result and exact execution detail. A graph with no
eligible replay reports `composite_scene_replay_timing_available=1` with
`expected_spans=0`, `resolved_spans=0`, and zero replay totals/max fields. Pool
exhaustion increments the expected count but leaves the span unresolved and
sets availability to zero. An invalid timestamp or missing execution detail
also sets availability to zero. Partial raw values may remain visible for
diagnostics, but native qualification must filter on availability before using
the attribution as complete evidence. The max replay record is selected with
strict `>` duration comparison, so equal-duration spans keep the first
resolved physical winner and all static/execution metadata comes from that
same span.

Each eligible replay costs one additional timestamp pair from the existing
2,048-span/4,096-query pool. The compile-time bound now proves two expected
in-flight graph scopes times one graph-total span plus 128 instances times the
current six built-in blur pass spans plus one Composite Scene Replay span per
instance: `2 * (1 + 128 * 7) = 1,794`, which remains within the unchanged
2,048-span pool. No second profiler or query pool is created.

Composite Scene Replay spans have their own timing purpose and never enter
effect-pass category durations, `timed_passes`, `max_effect_*`,
`max_capture_*`, `TimedPassBoundary`, or `record_pass_interval()`. Consequently
`pass_timed_ns`, `graph_unattributed_ns`, and existing Graph Gap Attribution
retain their prior meanings. In particular, the graph gap remains the largest
space between existing individually timed effect-pass boundaries; replay timing
is compared with that remainder and does not subtract from or redefine it.

### Pass-level tail attribution and graph coverage

The same one-line record appends `pass_timed_ns` and
`graph_unattributed_ns`, plus a single `max_effect_*` attribution:

```text
pass_timed_ns graph_unattributed_ns
max_effect_pass_ns max_effect_pass_id max_effect_instance_id max_effect_kind max_effect_capture_mode
max_effect_pixels max_effect_damage_rects max_effect_damage_bbox_pixels max_effect_target_width max_effect_target_height
```

`max_effect_*` identifies the longest valid individually timed pass of any
kind in that graph scope. Its stable `max_effect_kind` values are `scene`,
`surface`, `normalize`, `kawase_down`, `kawase_up`, `fragment`, `blend`,
`mask`, `composite`, and `postprocess`. Its capture mode is `replay`,
`framebuffer_blit`, or `framebuffer_shader_copy` only when that winning pass is
a capture with valid capture metadata; otherwise it is `none`. If the scope has
no valid timed pass, its max-effect values are zero and both kind and mode are
`none`. `max_capture_*` remains an independent selection of the longest valid
capture pass, including its existing Replay Attribution v2 detail. Both max
records can identify the same physical pass when that capture is also the
longest pass overall.

`max_effect_pixels` retains the existing unit of effect-space demanded-region
area. It does not imply physical fragments or target allocation size. The
target width and height come from the pass's planned output `GraphTexturePlan`;
passes without an output target report zero dimensions. `max_effect_damage_rects`
counts the exact execution damage rectangles supplied to the pass (bounded by
the existing 128-rectangle region policy) and describes damage shape, not draw
calls. Replay and specialized capture paths can have different physical draw
semantics. `max_effect_damage_bbox_pixels` is the area of the bounding
rectangle around that same execution damage; empty damage
reports zero. These values let native analysis compare demanded effect-space
pixels, physical target dimensions, and fragmented or sparse damage shape
without conflating their units.

`pass_timed_ns` is the saturating sum of the valid resolved pass-category
durations: capture, normalize, Kawase downsample, Kawase upsample, fragment,
blend, mask, composite, and postprocess. `graph_unattributed_ns` is
`total_ns.saturating_sub(pass_timed_ns)`. It is only the graph-total remainder
outside valid individually timed pass spans. It may include GPU work or idle
time associated with resource realization, state/setup, scene reconstruction
outside pass spans, driver scheduling gaps, CPU submission gaps between GPU
timestamp commands, or other currently untimed graph work. It does not by
itself identify resource allocation, driver overhead, or CPU overhead.

Graph Gap Attribution reuses the absolute start and end timestamps from those
existing graph and pass query pairs. `graph_gap_attribution_available=1` means
the graph has a valid TOTAL interval and at least one valid timed pass, every
expected timed pass resolved without a drop, pass intervals are ordered, and
the first and last pass intervals lie inside TOTAL. Otherwise attribution is
unavailable and `max_graph_gap_ns=0`,
`max_graph_gap_position=none`, and all boundary fields use stable zero/`none`
values. `max_graph_gap_ns` is the largest single contiguous timestamp interval
outside the valid individually timed pass spans when attribution is available;
its position is `pre` (TOTAL START to first pass START), `inter` (one pass END
to the next pass START), or `post` (last pass END to TOTAL END). Equal gaps keep
the earliest physical interval in graph order.

The `max_graph_gap_after_*` and `max_graph_gap_before_*` fields identify the
timed pass boundaries adjacent to the selected interval. A `pre` interval has
no `after` pass, and a `post` interval has no `before` pass. Boundary identity
includes pass and instance IDs, stable pass kind, capture mode, and capture
checkpoint count. These fields describe neighboring timed passes; they do not
assign causal ownership to either pass.

A gap may contain untimed GPU commands; GPU idle time while later commands are
waiting to be submitted; driver or context scheduling delay; CPU submission
delay visible as GPU timeline idle time; or resource and setup work outside
pass timestamps. The interval alone does not identify which of these, if any,
accounts for its duration.

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

No post-promotion native acceptance run was performed for this checkout:
`tty` reports `not a tty`, `/dev/dri/renderD128` and an NVIDIA GeForce RTX 3060
Ti are available, but there is no controlling TTY for a live DRM session. The
required run with `TYPHON_EFFECT_DEBUG_CHECKPOINT_CAPTURE_PATH` unset is
therefore deferred. The production preference follows the direct checkpoint
path qualification above; this environment does not add an unset-override trace
from the live Atomic EGL/GBM backend. Broader native baseline, blur, overlap,
presentation-combination, custom-frame-demand, and Settings visual-acceptance
observations also remain deferred. Direct Scanout remains conservative and
effects continue to require composition whenever visible effect pixels are
present.
