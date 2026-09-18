# Capture GPU Attribution Implementation Plan

> **For agentic workers:** Execute this plan inline, task-by-task, because subagents are prohibited. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Attribute existing GPU capture timestamp spans and physical execution work to capture kind, mode, checkpoint status, and graph scope without adding GPU instrumentation.

**Architecture:** The executor derives typed capture metadata from the existing `is_direct_framebuffer_capture` authority and attaches it to the existing pass span. The profiler aggregates valid spans and stores a bounded execution summary on the same `scope_id`; `execute_capture` fills that summary from the rectangles and command indices it already materializes.

**Tech Stack:** Rust, Cargo, GLES timestamp queries, `glow`, Typhon effects render graph, `rtk` command wrapper.

## Global Constraints

- Compile in `/home/agony/GitHub/Typhon` so build artifacts stay in the same folder.
- Use `rtk` for repository commands and test/build output.
- Do not use subagents.
- Preserve pre-existing user changes in the dirty worktree; stage only task-owned files.
- Do not allocate additional timestamp queries, add synchronization, read results synchronously, or enable execution tracing.
- Do not change capture behavior, graph compilation, checkpoint semantics, damage, shaders, pacing, KMS, scheduler behavior, or presentation policy.

---

### Task 1: Add failing profiler attribution tests

**Files:**
- Modify: `src/egl_renderer/effects/gpu_timing.rs` tests and test fixtures.

**Interfaces:**
- Consumes: existing `TimingState`, `TimingSpanMetadata`, `RenderPassKind`, and fixed query-pool test helpers.
- Produces: RED tests for typed capture metadata aggregation, checkpoint subsets, max capture pass selection, invalid spans, scope-owned summaries, and unchanged query allocation.

- [ ] **Step 1: Add tests for capture-kind and mode aggregation.**

  Extend the test metadata helper to accept optional `CaptureTimingMetadata` and add valid SceneCapture/SurfaceCapture spans with distinct durations and pixels. Add replay and framebuffer-blit SceneCapture spans and assert each duration/pixel/pass counter receives only its matching sample.

- [ ] **Step 2: Run the focused profiler tests and verify RED.**

  Run `rtk cargo test --locked gpu_timing`. Expected: compilation fails because capture metadata and the new aggregate fields do not yet exist.

- [ ] **Step 3: Add tests for checkpoint subset, max pass, invalid timing, scope ownership, and query capacity.**

  Add a checkpoint framebuffer span with `checkpoint_count=2` plus a non-checkpoint framebuffer span; assert only the checkpoint sample contributes to checkpoint duration/pass/pixel totals. Resolve multiple capture spans and assert the longest valid span owns all max fields. Resolve an invalid capture span and assert no duration, count, checkpoint, or max contribution. Attach different execution summaries to two same-frame scopes and assert they remain separate. Keep assertions for `TIMING_SPAN_POOL_CAPACITY`, `TIMING_QUERY_OBJECT_CAPACITY`, and allocated query count unchanged.

- [ ] **Step 4: Re-run and record the expected RED.**

  Run `rtk cargo test --locked gpu_timing`. The new tests must fail for missing production behavior, not for malformed test setup.

### Task 2: Implement typed metadata and fixed-size GPU aggregation

**Files:**
- Modify: `src/egl_renderer/effects/gpu_timing.rs` metadata, aggregate/record structs, resolution, formatting, and profiler methods.

**Interfaces:**
- Consumes: Task 1's failing tests.
- Produces: `CaptureTimingMode`, `CaptureTimingMetadata`, `CaptureExecutionTimingSummary`, `MaxCapturePassTiming`, scope-owned aggregation, and one-line integer/string telemetry.

- [ ] **Step 1: Add the minimal typed metadata and summary types.**

  Add fixed-size `CaptureTimingMode::{Replay, FramebufferBlit}` and `CaptureTimingMetadata { mode, checkpoint_count }`. Add a fixed-size execution summary containing total/replay/framebuffer/checkpoint physical pixels, scene/surface physical pixels, execution pass counts, replay command count, and checkpoint dependency-edge count. Add an optional max-capture record with duration, pass/instance IDs, kind, mode, effect pixels, and checkpoint count.

- [ ] **Step 2: Enrich existing aggregate and record state without changing query allocation.**

  Add the requested duration, pixel, and valid-pass counters to `GraphAggregate` and `GpuTimingRecord`, plus the execution summary and availability bit. Initialize them to zero/none. Do not alter `TimingState` slot counts, `create_query_pairs`, `begin_graph`, `begin_pass`, or collection polling.

- [ ] **Step 3: Aggregate only valid resolved capture spans.**

  In `finish_pass`, retain the existing category accumulation and add capture-kind, mode, checkpoint, and max updates from metadata. In `finish_total`, transfer the aggregate and scope-owned execution summary into the record. Invalid, dropped, disjoint-invalidated, and unavailable spans must continue through existing invalidation paths without aggregation.

- [ ] **Step 4: Attach summaries by scope before the total span is queued.**

  Extend `EffectGpuProfiler::end_graph` with an optional summary. Store it by `scope_id` in the matching aggregate before calling the existing `finish(scope.total)`. A `None` summary leaves availability false and all execution fields zero.

- [ ] **Step 5: Extend the legacy aggregate line.**

  Preserve every existing field and append the requested scene/surface, replay/framebuffer/checkpoint duration/pass/pixel, physical execution, replay-command, dependency-edge, max-pass, and summary-availability fields. Use `scene`/`surface`, `replay`/`framebuffer_blit`, and `none`/zero values; do not emit JSON or per-pass lines.

- [ ] **Step 6: Run the focused profiler tests GREEN.**

  Run `rtk cargo test --locked gpu_timing`. Confirm the new attribution tests and all existing invalid/dropped/query-capacity tests pass.

### Task 3: Add executor-owned mode authority and physical execution counters

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs` stats, pass timing call site, `execute_capture`, and focused tests.
- Modify: `src/egl_renderer/effects/mod.rs` only for the narrow profiler type re-export needed by the executor.
- Modify: `src/egl_renderer.rs` only if compile-time stats propagation needs the new fields; preserve existing frame-stat semantics.

**Interfaces:**
- Consumes: Task 2's metadata and summary types.
- Produces: authority-derived timing descriptors and `EffectExecutionStats::capture_timing_summary()`.

- [ ] **Step 1: Add executor RED tests for authority-derived metadata.**

  With debug capture mode Replay, assert a normal SceneCapture with no dependencies produces Replay and checkpoint count zero. With Replay plus one checkpoint dependency, assert FramebufferBlit and checkpoint count one. Add physical execution fixtures asserting replay pixels use the materialized partial rectangle area, direct pixels use the full target domain, replay commands equal the indices submitted to `draw_capture_commands_for_regions`, and direct captures add zero replay commands.

- [ ] **Step 2: Run the focused executor tests and verify RED.**

  Run `rtk cargo test --locked executor`. Expected: the metadata helper and new stats fields are unavailable or the new assertions fail before production changes.

- [ ] **Step 3: Derive timing metadata at the existing pass span call site.**

  For capture kinds only, call `is_direct_framebuffer_capture(pass, lifecycle_backdrop, debug_config)` and construct the typed mode plus `pass.checkpoint_dependencies.len()`. Pass `None` for all other kinds. Do not duplicate this policy in the profiler.

- [ ] **Step 4: Record physical work at the existing authoritative capture point.**

  Keep `capture_rects`, `output_rect_pixels(&capture_rects)`, `indices`, and `pass.checkpoint_dependencies` unchanged. Update total, mode, checkpoint, scene/surface physical pixels, execution pass counts, dependency edges, and replay command count from those exact values. Do not derive physical work from effect damage.

- [ ] **Step 5: Attach successful stats to `end_graph` by scope.**

  In `execute_graph_passes`, call `end_graph` with `Some(stats.capture_timing_summary())` only when `execute_graph_passes_inner` succeeds; call it with `None` on errors. Always call `end_graph` so the existing total slot closes. Return the original result unchanged.

- [ ] **Step 6: Run focused executor and effects tests GREEN.**

  Run `rtk cargo test --locked executor` and `rtk cargo test --locked effects`. Confirm existing capture geometry, error-path, checkpoint, and scene-work tests remain green.

### Task 4: Document semantics and verify the patch

**Files:**
- Modify: `docs/EFFECTS_QUALIFICATION.md` English GPU-timing section.
- Review only: all task-owned source and docs; do not modify unrelated dirty files.

- [ ] **Step 1: Document exact units and attribution rules.**

  Clarify legacy effect-space `capture_pixels` versus physical `capture_execution_pixels`, mode/checkpoint duration semantics, scope ownership, and that framebuffer-blit timing includes GL dependency/resolve/cache-ordering work rather than pure copy bandwidth.

- [ ] **Step 2: Run all focused verification commands.**

  Run `rtk cargo test --locked gpu_timing`, `rtk cargo test --locked executor`, `rtk cargo test --locked effects`, and `rtk cargo test --locked egl_renderer`.

- [ ] **Step 3: Run repository gates in the same checkout.**

  Run `rtk cargo fmt --all -- --check`, `rtk cargo check --locked --all-targets`, `rtk cargo clippy --locked --all-targets -- -D warnings`, `rtk cargo test --locked`, `rtk git diff --check`, and the source-layout gate if present.

- [ ] **Step 4: Review the final diff against the non-goals.**

  Confirm no new query objects/spans, synchronization, synchronous reads, trace requirement, unbounded profiler vectors, mode-policy duplicate, frame-ID join, rendering behavior, capture semantics, damage, shader, pacing, scheduler, KMS, or unrelated worktree changes.

- [ ] **Step 5: Run the approved native replay-policy qualification command.**

  Run the exact command supplied by the user when a controlling TTY/DRM session is available. Parse `effect_gpu_timing` records for the requested invariants, zero dropped/disjoint spans where hardware permits, and the new attribution fields. If native prerequisites remain unavailable, record the exact blocker without claiming native evidence.

- [ ] **Step 6: Commit only task-owned files.**

  Stage the design note, implementation plan, profiler/executor/module/docs changes, and tests only. Commit with `feat(effects): attribute capture GPU timing`.
