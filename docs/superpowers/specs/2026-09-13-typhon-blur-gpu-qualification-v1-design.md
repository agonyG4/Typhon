# Typhon Blur GPU Qualification v1 Design

## Goal

Add opt-in, observational GPU timestamp instrumentation to the existing GLES
effect graph without changing rendering semantics, effect output, damage,
resource allocation, or presentation behavior.

## Architecture

`GlesSceneRenderer` owns an `EffectGpuProfiler` because query objects are tied
to its current GL context. The profiler has explicit `Disabled`, `Unsupported`,
and `Active` states. It detects `GL_EXT_disjoint_timer_query` (including the
NVIDIA GLES path) or a valid desktop core/ARB timer-query path, confirms
non-zero timestamp counter bits, and otherwise leaves rendering untouched.

When active, initialization preallocates 2,048 query-pair slots (4,096 query
objects). This is derived from two expected in-flight graph scopes, the
128-instance effect bound, and the current six-pass built-in blur instance
shape, rounded to a fixed bounded capacity. Custom graphs may exhaust the
pool; exhaustion drops only timing spans. No query is created in the pass
path.

The executor polls old pending spans at the start of each renderer frame. It
uses a FIFO queue, checks only the end query's `QUERY_RESULT_AVAILABLE`, reads
start/end timestamps only after that check, and resolves at most 64 spans per
collection call. A graph-total timestamp pair surrounds one
`execute_graph_passes` invocation. Each selected pass gets a pair immediately
around the existing `execute_pass` call. Span metadata retains source frame ID,
monotonic graph scope ID, pass/instance IDs, pass kind, and execution-region
pixel count.

## Aggregation and failures

Pass samples aggregate by graph scope and `RenderPassKind`; the total sample
emits one structured `event=effect_gpu_timing` line using integer nanoseconds.
The source `frame_id` is emitted even when collection occurs later. Scene and
surface capture share the stable `capture_*` output bucket, and output
post-processing uses `postprocess_*`; all other categories have explicit
stable fields. A valid timestamp with `end < start` is discarded.

For EXT disjoint timing, the profiler samples `GPU_DISJOINT_EXT` before new
scopes and during collection. A disjoint invalidates all pending measurements,
recycles their slots exactly once, increments the invalidation counter, and
prevents affected aggregates from being emitted. Timing errors produce one
bounded diagnostic and disable/degrade instrumentation; they never reach the
effect fallback or renderer error paths. Renderer teardown explicitly deletes
each owned query exactly once before other GL resources are torn down.

## Testing

The query lifecycle and aggregate state machine are independent of `glow` and
use deterministic unit tests for disabled/unsupported states, availability
gating, bounded FIFO collection, duration validation, exact recycling,
exhaustion, disjoint invalidation, frame/scope identity, category aggregation,
total completion, error ownership, and teardown accounting. Formatting is
unit-tested separately. Existing effect, blur, partial repaint, resource,
cache, and lifecycle tests remain unchanged.

## Documentation

`docs/EFFECTS_QUALIFICATION.md` documents `TYPHON_EFFECT_GPU_TIMING=1`,
supported/unsupported behavior, asynchronous source-frame identity, the
structured fields, disjoint invalidation, pool drops, and the limitation that
GPU timing alone is not hardware qualification.
