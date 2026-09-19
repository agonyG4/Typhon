# Replay Capture Attribution v2 Implementation Plan

> **For agentic workers:** Execute this plan inline, task-by-task, because subagents are prohibited. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add fixed-size native Replay capture execution detail that distinguishes region/command amplification, host planning/submission time, and residual GPU/driver timing without changing rendering behavior.

**Architecture:** The existing Replay capture executor returns a fixed `ReplayCaptureExecutionDetail` populated from the materialization plan, the single canonicalized execution region, existing planner results, and deltas of the existing renderer frame counters. The detail is carried by the exact pass timing span and aggregated by scope; the GPU END query is issued before detail attachment. Host timing is gated by the existing active graph timing scope.

**Tech Stack:** Rust, Cargo, `glow` GLES timestamp queries, `std::time::Instant`, Typhon effect executor, `rtk` command wrapper.

## Global Constraints

- Compile in `/home/agony/GitHub/Typhon` so build artifacts stay in the same folder.
- Use `rtk` for Cargo and Git commands and output.
- Do not use subagents.
- Preserve all pre-existing dirty-worktree changes; stage only Replay-attribution files and the approved design/plan notes.
- Do not add GPU queries, timestamp classes, fences, `glFinish`, `glFlush`, synchronous query reads, or per-pass logs.
- Do not add a second traversal of `renderer.commands` or capture indices for telemetry.
- Do not change capture policy, replay policy, effect damage, checkpoint semantics, pacing, scheduler, KMS, target selection, shaders, or rendering output.
- Keep timing metadata and execution detail fixed-size and allocation-free.
- Keep `replay_capture_commands` equal to the legacy sum of selected Replay candidate indices once per capture pass.

---

### Task 1: Add RED tests for fixed Replay detail and region canonicalization

**Files:**
- Modify: `src/egl_renderer.rs` renderer test module and private Replay capture helpers.
- Modify: `src/egl_renderer/effects/executor.rs` executor stats tests.

**Interfaces:**
- Consumes: existing `EffectRegion::disjoint_bounded()`, `capture_materialization_plan()`, `EffectExecutionStats`, and existing renderer counters.
- Produces: failing tests for materialization versus execution-region counts, conservative overflow fallback, actual planner work, candidate/full-scene distinction, and actual executed-draw counters.

- [ ] **Step 1: Write a failing pure layout test.**

  Add a test for the helper used by `draw_capture_commands_for_regions()` with two non-overlapping `OutputRect` values. Assert `materialization_rects == 2`, `execution_regions == 2`, and `disjoint_overflowed == false`. Add an overflow-shaped `EffectRegion` fixture using the existing bounded-region constants and assert the helper reports `disjoint_overflowed == true` while the execution region has exactly one conservative bounding rectangle. The production helper must be the code under test; do not duplicate its canonicalization sequence inside the assertions.

- [ ] **Step 2: Write failing tests for candidate versus full-scene work.**

  Add an executor-level fixture with a scene command count `S`, selected Replay candidate count `N < S`, and two execution regions. Feed the existing `plan_capture_visibility()` path through the helper and assert the accumulated `planner_commands_visited`/`command_region_pairs` are the actual visits, while `scene_scan_pairs`/`commands_considered` reflect the full draw-loop scans. Include an invalid candidate index and assert the pair count follows returned planner statistics instead of `N * R`.

- [ ] **Step 3: Write failing tests for actual draw counters and zero defaults.**

  Extend the stats fixture with separate actual `commands_executed` and `draw_calls` values that are lower than candidate count, representing occluded, outside, and unavailable commands. Assert the Replay detail reports those physical draw-path values and does not infer them from candidates. Assert a default/framebuffer detail has zero Replay-only fields.

- [ ] **Step 4: Run the focused RED tests.**

  Run `rtk cargo test --locked egl_renderer::` and `rtk cargo test --locked executor`. Expected result: the new tests fail because the Replay detail/layout fields and aggregation helpers do not yet exist. Fix only test setup errors until the failure is specifically missing production behavior.

### Task 2: Implement renderer-side Replay detail from existing work

**Files:**
- Modify: `src/egl_renderer.rs` imports, private Replay detail/layout types, `draw_capture_commands()`, `draw_capture_commands_for_regions()`, and no unrelated renderer paths.
- Modify: `src/egl_renderer/effects/executor.rs` `EffectExecutionStats`, `execute_pass()`, `execute_capture()`, and timing-gated host duration helpers.
- Modify: `src/egl_renderer/effects/mod.rs` narrow crate-visible re-export of the fixed detail type if needed by `egl_renderer.rs`.

**Interfaces:**
- Consumes: Task 1 RED tests; `EglVisibilityPlanStats`; existing frame-stat deltas; existing materialization and selection code.
- Produces: `ReplayCaptureExecutionDetail`, a single-use canonicalization layout helper, and `execute_capture() -> RendererResult<Option<ReplayCaptureExecutionDetail>>` for Replay only.

- [ ] **Step 1: Add the fixed-size detail type and saturating host timing helper.**

  Define `ReplayCaptureExecutionDetail` with scalar fields for materialization rects, execution regions, overflow, candidate commands, command-region pairs, scene command total, scene scan pairs, planner visited/drawable, considered/executed/draw calls, and the four nanosecond CPU durations. Add `duration_to_ns(Duration) -> u64` using checked conversion to `u64::MAX` on overflow. Make the type `Clone, Copy, Debug, Default, Eq, PartialEq`; do not add vectors, strings, or timestamps to timing state.

- [ ] **Step 2: Implement and use the one canonicalization helper.**

  Extract the existing `output_rects -> EffectRegion -> disjoint_bounded() -> bounding_rect fallback` sequence into a private helper returning the actual execution region, `materialization_rects`, `execution_regions`, and `disjoint_overflowed`. Call it exactly once from `draw_capture_commands_for_regions()` and iterate the returned region. Preserve the existing overflow fallback and empty-region behavior byte-for-byte in semantics.

- [ ] **Step 3: Return per-region planner and draw deltas without another traversal.**

  Change `draw_capture_commands()` to return a fixed partial Replay detail. Gate `Instant::now()` calls on the `host_timing_enabled` argument. Measure only `plan_capture_visibility()` for `visibility_cpu_ns`; measure the existing `draw_command_batch_with_visibility()` call for `draw_submit_cpu_ns`. Before and after that existing call, subtract `frame_stats.commands_considered`, `commands_executed`, and `draw_calls`. Accumulate the returned `EglVisibilityPlanStats.commands_visited` and `.commands_drawable`; set `command_region_pairs` from the accumulated actual visits. Do not add loops over `renderer.commands` or `command_indices`.

- [ ] **Step 4: Measure selection and whole Replay execution in `execute_capture()`.**

  Pass `graph_scope.is_some()` through `execute_pass()` to `execute_capture()` as the host-timing gate. For Replay only, start the whole-path `Instant` at capture entry, time the existing layer/visual-group metadata construction plus `indices_for_capture()` as `selection_cpu_ns`, call the renderer detail path, restore the existing framebuffer state, then set `host_cpu_ns` from the whole path. Keep direct framebuffer capture returning `None` detail and zero Replay-only stats.

- [ ] **Step 5: Preserve legacy stats while aggregating new successful Replay totals.**

  Add the requested Replay aggregate counters to `EffectExecutionStats`. Keep the existing `record_capture_execution()` call and its `replay_capture_commands += indices.len()` semantics unchanged. Add a `record_replay_capture_detail()` method that saturating-adds structural, planner, draw, and host fields only after the Replay draw path returns successfully. Return the detail through `execute_pass()` and pass it to `end_pass()`; non-capture and framebuffer branches return `None`.

- [ ] **Step 6: Run the renderer/executor tests GREEN.**

  Run `rtk cargo test --locked egl_renderer::` and `rtk cargo test --locked executor`. Confirm the region, overflow, candidate/full-scene, actual-draw, and existing capture-materialization tests pass. If formatting or type errors appear, fix implementation only; do not weaken assertions.

### Task 3: Carry exact detail through GPU timing and max-pass attribution

**Files:**
- Modify: `src/egl_renderer/effects/gpu_timing.rs` fixed metadata, scope aggregates, max-pass record, `end_pass()`, resolution, formatting, and tests.
- Modify: `src/egl_renderer/effects/executor.rs` only at the existing `end_pass()` call site to pass the returned exact detail.

**Interfaces:**
- Consumes: Task 2 `ReplayCaptureExecutionDetail`, `CaptureExecutionTimingSummary`, and `Option` returned by `execute_pass()`.
- Produces: exact timing-span detail ownership, graph-level Replay totals, and scalar `max_capture_*` output.

- [ ] **Step 1: Add failing GPU-timing aggregation tests.**

  Extend the synthetic pass metadata helper to accept optional Replay detail. Add tests that attach synthetic host/structural values to a Replay span, resolve it asynchronously, and assert every value survives unchanged in the max record. Add a graph-summary test asserting the new totals accumulate from successful execution summaries. Add a framebuffer max-pass case and assert all Replay-specific max fields are zero.

- [ ] **Step 2: Add failing exact-association and rejection tests.**

  Resolve two Replay spans in one scope with distinct pass IDs, durations, region counts, pair counts, scene scans, and host timings; make the second span slower and assert every `max_capture_*` value comes from the second span. Keep the same-frame/two-scope test and attach distinct detail to each scope. Add invalid timestamp and query-slot exhaustion cases asserting no fake max detail. Preserve existing dropped-pass, disjoint invalidation, and query-capacity assertions.

- [ ] **Step 3: Extend fixed timing metadata and aggregates.**

  Add `replay_execution: Option<ReplayCaptureExecutionDetail>` to `TimingSpanMetadata` and add the new graph totals to `CaptureExecutionTimingSummary`, `GpuTimingRecord`, and `GraphAggregate`. Initialize all new values to zero/none. Do not change `TIMING_SPAN_POOL_CAPACITY`, `TIMING_QUERY_OBJECT_CAPACITY`, query creation, polling, or begin/end query calls.

- [ ] **Step 4: Attach detail after the existing GPU END query.**

  Change `EffectGpuProfiler::end_pass(gl, span, detail)` so it first finishes timing ownership, queues the existing END timestamp, then copies the fixed detail into the pending span identified by its exact `SpanToken` and generation. A dropped or reused token must not accept detail. Do not join by frame, instance, or pass IDs.

- [ ] **Step 5: Aggregate valid Replay details and format bounded fields.**

  In `finish_pass()`, update graph totals from `metadata.replay_execution` only for valid Replay capture spans. Extend `MaxCapturePassTiming` with optional Replay detail and update it only when the valid capture duration is strictly longer. At total resolution, copy scope-owned totals into the record. Append graph-level fields and `max_capture_execution_pixels`, materialization rects, execution regions, overflow bit, Replay candidate commands, command-region pairs, scene commands, scene scans, planner counts, executed commands, draw calls, and host phase durations to the existing one-line formatter. For framebuffer max spans, emit zero/none Replay fields.

- [ ] **Step 6: Run GPU timing tests GREEN.**

  Run `rtk cargo test --locked gpu_timing`. Confirm synthetic durations, exact max association, scope ownership, invalid/dropped handling, stable formatting, and unchanged query capacity all pass.

### Task 4: Add disabled-timing and execution-summary regression coverage

**Files:**
- Modify: `src/egl_renderer/effects/gpu_timing.rs` tests and test helpers.
- Modify: `src/egl_renderer/effects/executor.rs` stats tests.

**Interfaces:**
- Consumes: Task 2 and Task 3 APIs.
- Produces: explicit guards for no host timing when the GPU profiler is inactive and legacy aggregate invariants.

- [ ] **Step 1: Write and run the disabled-host-timing RED test.**

  Add a test that constructs the Replay timing helper with `host_timing_enabled == false`, completes it, and asserts all four CPU fields remain zero. Assert that the existing disabled/unsupported profiler paths still have no pending work and no timing allocation. The test must exercise the gate used by production code, not a separately invented test-only path.

- [ ] **Step 2: Verify legacy capture identities.**

  Extend executor and GPU timing assertions for `capture_ns == scene_capture_ns + surface_capture_ns` and `capture_ns == replay_capture_ns + framebuffer_capture_ns`, plus unchanged legacy `replay_capture_commands` values. Assert graph totals may describe physical Replay work even when the corresponding timing span is dropped, while max attribution remains absent.

- [ ] **Step 3: Run all focused effects tests GREEN.**

  Run `rtk cargo test --locked gpu_timing`, `rtk cargo test --locked executor`, `rtk cargo test --locked egl_renderer`, and `rtk cargo test --locked effects`.

### Task 5: Document, inspect, and verify the complete patch

**Files:**
- Modify: `docs/EFFECTS_QUALIFICATION.md` GPU timing section.
- Review only: `src/egl_renderer.rs`, `src/egl_renderer/geometry.rs`, `src/egl_renderer/effects/executor.rs`, `src/egl_renderer/effects/capture.rs`, `src/egl_renderer/effects/gpu_timing.rs`, `src/egl_renderer/effects/trace.rs`, and task-owned docs.

**Interfaces:**
- Consumes: all implementation and test changes.
- Produces: exact English field semantics, verification evidence, source-layout result, and a focused implementation commit.

- [ ] **Step 1: Document units and distinctions.**

  Add exact definitions for `replay_capture_commands`, `replay_capture_command_region_pairs`, `replay_capture_scene_scan_pairs`, materialization rectangles versus execution regions, disjoint overflow, planner/draw counters, and all host nanosecond fields. State that `replay_capture_ns` is a GPU timestamp interval that may include GPU idle/order dependency while the host is constructing/submitting commands, and that `host_cpu_ns` includes fixed Replay setup and GL/driver blocking observed on the host.

- [ ] **Step 2: Run formatting and all requested verification gates.**

  Run, from `/home/agony/GitHub/Typhon`, `rtk cargo fmt --all -- --check`, `rtk cargo check --locked --all-targets`, `rtk cargo clippy --locked --all-targets -- -D warnings`, `rtk cargo test --locked`, and `rtk git diff --check`. Also run the repository source-layout gate if present (`rtk cargo test --locked --test source_layout` when that target exists). Record exact exit status and test counts.

- [ ] **Step 3: Review the final diff against the non-goals.**

  Inspect only task-owned staged and unstaged hunks. Confirm no new query class/object/capacity, no host clock reads outside the active graph-scope gate, no second command/index traversal, no legacy field reinterpretation, no pre-END aggregation/logging, no frame-ID join, no fake invalid/dropped max pass, no framebuffer Replay detail, no per-pass output, and no changes in rendering, capture policy, pacing, scheduler, KMS, or unrelated dirty files.

- [ ] **Step 4: Run the approved native Replay qualification command.**

  Run the exact production Replay command from the request with `TYPHON_EFFECT_EXEC_TRACE=0`, `TYPHON_EFFECT_GPU_TIMING=1`, and the existing pacing/native environment when a controlling TTY/DRM session is available. Exercise the listed workload and collect fast/slow recurring samples. If the environment lacks the required TTY/DRM prerequisite, record that exact blocker and do not claim native qualification.

- [ ] **Step 5: Stage only task-owned changes and commit.**

  Stage `docs/superpowers/specs/2026-09-19-replay-capture-attribution-v2-design.md`, `docs/superpowers/plans/2026-09-19-replay-capture-attribution-v2.md`, `docs/EFFECTS_QUALIFICATION.md`, `src/egl_renderer.rs`, `src/egl_renderer/effects/executor.rs`, `src/egl_renderer/effects/gpu_timing.rs`, and any narrowly required `geometry.rs`/`effects/mod.rs` changes. Do not stage the pre-existing compositor/native-output changes, the unrelated opacity changes in `executor.rs`, or the unrelated untracked cursor-feedback files. Commit with `feat(effects): attribute replay capture amplification`.
