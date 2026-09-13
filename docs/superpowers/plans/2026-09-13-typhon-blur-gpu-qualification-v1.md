# Blur GPU Qualification v1 Implementation Plan

> **For agentic workers:** Execute this plan inline in the current checkout. Do not dispatch sub-agents; preserve all pre-existing working-tree changes.

**Goal:** Add opt-in, bounded, non-blocking GLES timestamp instrumentation to Typhon's existing effect graph and document/test its asynchronous telemetry.

**Architecture:** Add a renderer-owned `EffectGpuProfiler` in `src/egl_renderer/effects/gpu_timing.rs`. Separate the glow calls from a deterministic `TimingState` that owns fixed span slots, FIFO pending samples, graph aggregates, recycling, disjoint invalidation, and formatting. Integrate collection at the renderer frame boundary, graph-total markers around `execute_graph_passes`, and pass markers immediately around `execute_pass`; retain source frame IDs from `EffectExecutionTrace`.

**Tech Stack:** Rust, `glow 0.17`, GLES `GL_EXT_disjoint_timer_query`, existing typed effect graph, Cargo unit/integration tests, `rtk` command wrapper.

## Global Constraints

- `TYPHON_EFFECT_GPU_TIMING=1` is the only opt-in gate; default behavior remains disabled.
- Do not modify blur shaders, blur radius/pass count/scale/capture footprint/color boundaries, graph planning, damage, resources, KMS, pacing, presentation, or native output behavior.
- Never use `glFinish`, `glFlush` for timing, fence/sync waits, busy polling, `QUERY_RESULT` before `QUERY_RESULT_AVAILABLE`, unbounded retries, or per-pass query allocation.
- Query objects belong to `GlesSceneRenderer`, are preallocated only when active, and are explicitly deleted exactly once from `destroy()`.
- Pool exhaustion and all timing failures affect instrumentation only; rendering continues unchanged.
- Collection is FIFO, checks the end query for availability, reads timestamps only after readiness, and resolves at most 64 spans per call.
- All telemetry durations are integer nanoseconds and identify the source `frame_id` and monotonic graph `scope`.
- Compile and run Cargo commands in `/home/agony/GitHub/Typhon` so `target/` is reused.
- Use `rtk` for repository, formatting, compilation, lint, and test commands.

---

### Task 1: Add failing deterministic profiler state-machine tests

**Files:**
- Create: `src/egl_renderer/effects/gpu_timing.rs`
- Modify: `src/egl_renderer/effects/mod.rs`
- Test: `src/egl_renderer/effects/gpu_timing.rs` unit tests

**Interfaces:**
- The new module will expose `pub(crate) struct EffectGpuProfiler` to the renderer module and keep lifecycle types private except test-visible items under `cfg(test)`.
- The pure state machine will use fixed slot tokens, `TimingCategory`, `TimingSpanMetadata`, `GraphTimingScope`, `SpanToken`, and `GpuTimingRecord`.

- [ ] **Step 1: Declare the module and write the first failing tests.**

Add `mod gpu_timing;` to `effects/mod.rs`, then add tests that reference the intended pure API:

```rust
#[test]
fn disabled_timing_does_not_allocate_or_issue_query_work() {
    let mut state = TimingState::disabled();
    assert!(state.begin_scope(Some(120)).is_none());
    assert_eq!(state.query_slots(), 0);
}

#[test]
fn unsupported_timing_degrades_without_pending_work() {
    let profiler = EffectGpuProfiler::unsupported_for_test("timestamp-query-unavailable");
    assert_eq!(profiler.state_kind(), ProfilerStateKind::Unsupported);
    assert_eq!(profiler.pending_span_count_for_test(), 0);
}

#[test]
fn unavailable_end_query_is_not_read() {
    let mut state = TimingState::active_for_test(2);
    let scope = state.begin_scope(Some(120)).expect("scope slot");
    let pass = state.begin_pass(scope, pass_metadata(7, RenderPassKind::Composite, 64));
    state.finish(pass.expect("pass slot"));
    assert_eq!(state.poll_front(false, Some((100, 140))), PollOutcome::NotReady);
    assert_eq!(state.read_count_for_test(), 0);
    assert_eq!(state.pending_span_count(), 1);
}
```

- [ ] **Step 2: Run the focused test to verify the intended missing-API failure.**

Run: `rtk cargo test --locked egl_renderer::effects::gpu_timing`

Expected: compilation fails because `TimingState`, `EffectGpuProfiler`, and the lifecycle methods do not exist yet. This confirms the tests are red for the feature rather than an accidental assertion failure.

- [ ] **Step 3: Commit only the test/module declaration if the red failure is correct.**

Run: `rtk git add src/egl_renderer/effects/mod.rs src/egl_renderer/effects/gpu_timing.rs && rtk git commit -m "test: define GPU timing lifecycle expectations"`

### Task 2: Implement the pure bounded timing state machine

**Files:**
- Modify: `src/egl_renderer/effects/gpu_timing.rs`

**Interfaces:**
- `TimingState::active_for_test(capacity: usize) -> TimingState` creates test slots without GL objects; production will use the same slot logic behind `EffectGpuProfiler`.
- `TimingState::begin_scope(frame_id: Option<u64>) -> Option<GraphTimingScope>` reserves one pair for the total and creates one bounded aggregate.
- `TimingState::begin_pass(scope: GraphTimingScope, metadata: TimingSpanMetadata) -> Option<SpanToken>` reserves a pair or increments that scope's dropped-pass count.
- `TimingState::finish(token: SpanToken) -> bool` transitions exactly one open slot to FIFO pending.
- `TimingState::poll_front(available: bool, timestamps: Option<(u64, u64)>) -> PollOutcome` must return `NotReady` without reading/consuming timestamps when unavailable, and must recycle the pair exactly once when ready.
- `TimingState::invalidate_pending() -> usize` recycles all pending pairs, removes affected aggregates, and returns the invalidated span count.
- `format_gpu_timing_line(&GpuTimingRecord) -> String` emits stable machine-parseable fields.

- [ ] **Step 1: Implement only enough slot/queue logic to make the disabled, unsupported, and unavailable-query tests pass.**

Use `SlotPhase::{Free, Open, Pending}`, a generation counter in `SpanToken`, a `Vec<usize>` free list, and a `VecDeque<PendingSpan>`. `finish` must reject stale, already-pending, or already-free tokens. `poll_front(false, ...)` must not increment read counters or inspect the supplied timestamps.

- [ ] **Step 2: Add failing tests for duration, recycling, exhaustion, and bounded collection.**

Add `ready_timestamps_publish_end_minus_start_and_recycle_pair`, asserting
timestamps `(100, 140)` produce `40` nanoseconds and restore the free-slot
count; add `end_before_start_is_discarded_and_recycled`, asserting no wrapped
duration is published; add
`double_finish_or_recycle_cannot_return_a_slot_twice`, asserting the free-slot
count is unchanged by the second operation; add
`pool_exhaustion_drops_only_the_new_timing_span`, asserting the graph remains
usable and only the drop counter changes; and add
`collection_bound_stops_before_a_large_ready_backlog`, asserting one poll
consumes no more than the fixed 64-entry bound.

The tests must assert slot counts, pending counts, `dropped_spans`, and the exact number of consumed ready entries rather than merely checking a mock call count.

- [ ] **Step 3: Implement aggregate state and bounded collection.**

Store graph frame ID, scope ID, per-category nanoseconds/pixels, `timed_passes`, and `dropped_passes`. Map all ten `RenderPassKind` values to `TimingCategory`; combine scene/surface capture into `capture_*` output fields and output post-process into `postprocess_*`. Treat a valid `end < start` result as an invalid pass/total and never subtract unsigned values.

- [ ] **Step 4: Run the focused tests and refactor only while green.**

Run: `rtk cargo test --locked egl_renderer::effects::gpu_timing`

Expected: all state-machine tests pass with no GL context.

- [ ] **Step 5: Commit the pure state machine.**

Run: `rtk git add src/egl_renderer/effects/gpu_timing.rs && rtk git commit -m "feat: add bounded GPU timing state machine"`

### Task 3: Add glow capability detection, query-pair pool, and diagnostics

**Files:**
- Modify: `src/egl_renderer/effects/gpu_timing.rs`

**Interfaces:**
- `EffectGpuProfiler::new(gl: &glow::Context) -> EffectGpuProfiler` reads the exact `TYPHON_EFFECT_GPU_TIMING=1` gate, detects a supported timestamp path, confirms `QUERY_COUNTER_BITS > 0`, and never returns an initialization error to rendering.
- `EffectGpuProfiler::collect(&mut self, gl: &glow::Context)` performs one bounded non-blocking collection pass.
- `EffectGpuProfiler::begin_graph(&mut self, gl: &glow::Context, frame_id: Option<u64>) -> Option<GraphTimingScope>` and `end_graph(...)` emit timestamp markers only in `Active` state.
- `EffectGpuProfiler::begin_pass(...) -> Option<PassTimingSpan>` and `end_pass(...)` issue a pair around an actual executor pass only when a slot is available.
- `EffectGpuProfiler::destroy(&mut self, gl: &glow::Context)` deletes every preallocated query once and clears ownership.

- [ ] **Step 1: Add failing pure tests for identity, categories, aggregates, disjoint invalidation, formatting, and teardown accounting.**

Cover these exact cases:

Add `multiple_frames_preserve_source_frame_ids` with two scopes whose source
frames are `120` and `121`, then collect them in FIFO order and assert each
record retains its own source frame. Add
`multiple_scopes_in_one_frame_remain_distinct` and assert two monotonically
increasing scope IDs for frame `120`. Add
`pass_categories_accumulate_in_correct_buckets` and feed every
`RenderPassKind`, asserting each duration/pixel pair lands in its category.
Add `total_completion_returns_one_graph_record` and assert pass completion
alone emits nothing while total completion emits exactly one aggregate. Add
`disjoint_invalidates_all_pending_measurements` and assert all pending slots
are free, the invalidation counter equals the pending span count, and no
record is emitted. Add `formatting_is_stable_and_integer_nanoseconds` and
compare the complete key/value line against the documented field order. Add
`timing_ownership_closes_on_error_path` and assert ending a span after a
simulated executor error returns its pair to pending ownership. Add
`teardown_deletes_each_query_object_once` and assert the teardown accounting
model has one delete per allocated query and zero deletes on a second teardown.

- [ ] **Step 2: Run the focused tests to verify these new expectations fail.**

Run: `rtk cargo test --locked egl_renderer::effects::gpu_timing`

Expected: the new assertions fail because the corresponding aggregate, diagnostic, and ownership behaviors are not implemented.

- [ ] **Step 3: Implement explicit profiler states and exact opt-in behavior.**

Use `ProfilerState::{Disabled, Unsupported(&'static str), Active(ActiveProfiler)}`. Disabled construction must not touch GL. Unsupported initialization emits one stable `event=effect_gpu_timing_unsupported reason=...` diagnostic for the profiler instance and never retries. Use a local `GPU_DISJOINT_EXT` constant (`0x8FBB`) because glow 0.17 exposes the query APIs but not that extension constant.

- [ ] **Step 4: Implement capability detection and fixed pool creation.**

Accept `GL_EXT_disjoint_timer_query` for GLES/NVIDIA and a valid desktop core/`GL_ARB_timer_query` path. After selecting the path, read `glow::QUERY_COUNTER_BITS`; a non-positive result is unsupported. Preallocate exactly 2,048 query-pair slots (4,096 query objects), derived from two expected in-flight scopes × 128 max effect instances × the current six-pass built-in blur instance shape, rounded up. On any create failure, delete already-created objects exactly once, emit one bounded diagnostic, and use `Unsupported`.

- [ ] **Step 5: Implement timestamp commands, FIFO availability collection, and disjoint handling.**

Use `gl.query_counter(query, glow::TIMESTAMP)` for start/end markers. In `collect`, read `GPU_DISJOINT_EXT` when using the extension; if set, invalidate all pending spans and resume only on a later non-disjoint boundary. Otherwise inspect only the front end query with `get_query_parameter_u32(..., QUERY_RESULT_AVAILABLE)`. Stop immediately when unavailable; only then read both timestamps with `get_query_parameter_u64(..., QUERY_RESULT)`, resolve, recycle, and possibly emit the aggregate. Never call `glFlush`, `glFinish`, sync, fence, or a wait primitive.

- [ ] **Step 6: Run the focused tests and commit the profiler core.**

Run: `rtk cargo test --locked egl_renderer::effects::gpu_timing`

Expected: all deterministic profiler tests pass.

Run: `rtk git add src/egl_renderer/effects/gpu_timing.rs && rtk git commit -m "feat: add non-blocking GLES GPU timing profiler"`

### Task 4: Integrate the profiler at existing renderer and graph boundaries

**Files:**
- Modify: `src/egl_renderer.rs`
- Modify: `src/egl_renderer/effects/executor.rs`
- Modify: `src/egl_renderer/effects/trace.rs`
- Modify: `src/egl_renderer/effects/mod.rs`

**Interfaces:**
- `GlesSceneRenderer` owns one `EffectGpuProfiler` beside `effect_trace` and `effect_resources`.
- `new_current` constructs it after the live `glow::Context` exists without propagating instrumentation failure.
- `draw_scene_with_buffer_age` calls `collect` once at the deterministic start-of-frame boundary before new effect timing markers.
- `execute_graph_passes` begins a graph-total scope using `renderer.effect_trace.frame_id()` and finishes it on both success and error through a result wrapper.
- The selected-pass loop calls `begin_pass` immediately before the existing `execute_pass` call and `end_pass` immediately after it, including error results.
- `destroy()` calls profiler teardown before the GL context becomes invalid.

- [ ] **Step 1: Add the trace frame-ID accessor and renderer field/init/teardown references.**

Add `EffectExecutionTrace::frame_id(&self) -> Option<u64>`, import/re-export `EffectGpuProfiler`, initialize it from the live context, collect at the frame boundary, and call `destroy` from the existing renderer `destroy()` path. Do not add frame stats fields or alter effect fallback behavior.

- [ ] **Step 2: Add a failing integration assertion for source frame/scope wiring where a renderer-free test can cover it.**

Extend profiler tests with the exact `Option<u64>` frame identity passed into `begin_graph`; then run `rtk cargo test --locked egl_renderer::effects::gpu_timing` and confirm red until the accessor/wiring API exists.

- [ ] **Step 3: Wrap `execute_graph_passes` with total timing and preserve all existing result/release behavior.**

Use a local `GraphTimingScope` token and an inner result-returning body or equivalent. Finish the total marker after the body returns, before returning its original `RendererResult`. Do not move resource release, ordinary scene-state restoration, or overlay calls across existing control-flow boundaries.

- [ ] **Step 4: Place pass markers around only `execute_pass`.**

Compute `pixels` by summing `execution_damage.rects()` width×height with saturating `u64` arithmetic. Build metadata from `pass.id.get()`, `pass.instance.get()`, `pass.kind`, and the current scope; call `end_pass` even when `execute_pass` returns an error. No shader, capture, or blur helper receives timing code.

- [ ] **Step 5: Run focused executor/renderer tests and commit integration.**

Run: `rtk cargo test --locked egl_renderer`

Expected: existing renderer/effects tests pass and the new profiler tests pass. Commit only the four integration files:

```bash
rtk git add src/egl_renderer.rs src/egl_renderer/effects/executor.rs src/egl_renderer/effects/trace.rs src/egl_renderer/effects/mod.rs
rtk git commit -m "feat: instrument effect graph GPU boundaries"
```

### Task 5: Document telemetry and qualify the source-level safety boundary

**Files:**
- Modify: `docs/EFFECTS_QUALIFICATION.md`

- [ ] **Step 1: Replace the stale unavailable-timer statement.**

Document `TYPHON_EFFECT_GPU_TIMING=1`, disabled default, supported GLES extension/core behavior, one-time unsupported diagnostics, asynchronous collection, preserved source `frame_id`, monotonic `scope`, and no current-frame attribution.

- [ ] **Step 2: Document the structured record fields.**

Document the stable line shape:

```text
typhon effect: event=effect_gpu_timing frame_id=<u64|unknown> scope=<u64> total_ns=<u64> capture_ns=<u64> normalize_ns=<u64> blur_downsample_ns=<u64> blur_upsample_ns=<u64> fragment_ns=<u64> blend_ns=<u64> mask_ns=<u64> composite_ns=<u64> postprocess_ns=<u64> timed_passes=<usize> dropped_passes=<usize> capture_pixels=<u64> normalize_pixels=<u64> blur_downsample_pixels=<u64> blur_upsample_pixels=<u64> fragment_pixels=<u64> blend_pixels=<u64> mask_pixels=<u64> composite_pixels=<u64> postprocess_pixels=<u64> query_pool_capacity=<usize> query_pool_high_water=<usize> dropped_spans=<usize> disjoint_invalidated_spans=<usize>
```

Explain that capture combines `SceneCapture` and `SurfaceCapture`, postprocess maps `OutputPostProcess`, and all values are integer nanoseconds/pixels.

- [ ] **Step 3: Document disjoint/drop semantics and qualification limits.**

State that a disjoint event invalidates affected pending samples, recycles them, and prevents publication; pool exhaustion drops instrumentation only; timestamps are not hardware qualification alone; no performance claim is made without a real supported GLES run.

- [ ] **Step 4: Run documentation/source checks and commit docs.**

Run: `rtk git diff --check; rtk rg -n 'glFinish|glFlush|wait_sync|client_wait_sync|fence|QUERY_RESULT' src/egl_renderer/effects/gpu_timing.rs src/egl_renderer/effects/executor.rs`

Expected: no forced-completion primitive or pre-availability result read is present in the timing path; any `QUERY_RESULT` match is the post-availability read only. Commit: `rtk git add docs/EFFECTS_QUALIFICATION.md && rtk git commit -m "docs: describe effect GPU timing qualification"`.

### Task 6: Run complete verification and inspect the final diff

**Files:**
- Verify: all task files and the pre-existing dirty-tree file set

- [ ] **Step 1: Run the focused tests first.**

```bash
rtk cargo test --locked effects
rtk cargo test --locked egl_renderer
```

- [ ] **Step 2: Run the required deterministic closure in the same checkout.**

```bash
rtk cargo fmt --all -- --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked effects
rtk cargo test --locked egl_renderer
rtk cargo test --locked
rtk git diff --check
```

- [ ] **Step 3: Inspect scope and unchanged blur/graph semantics.**

Run: `rtk git status --short; rtk git diff HEAD~4 --stat; rtk git diff HEAD~4 -- src/egl_renderer/effects/blur.rs src/effects/render_graph.rs src/native_output src/bin bin`

Confirm the final task commits touch only the profiler integration/docs/spec/plan files, the six pre-existing compositor modifications remain unstaged, and `blur.rs`, render-graph lowering, native/KMS, `bin/`, and `src/bin/` have no task changes. If the user or another process changes the pre-existing dirty set, treat the latest status as authoritative and do not restore anything.

- [ ] **Step 4: Check the commit contents and report hardware status.**

Run: `rtk git log --oneline --decorate -8; rtk git diff origin/main..HEAD --stat`

Report exact command exit codes and test counts. State explicitly that no real GPU timing smoke test was performed unless a supported real GLES context was actually run; do not claim hardware performance or qualification from deterministic tests.
