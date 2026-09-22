# Composite Scene Replay Attribution v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add bounded GPU/CPU attribution for eligible Composite scene replay while preserving existing effect-pass and graph-gap semantics.

**Architecture:** Extend the existing `gpu_timing.rs` span metadata and aggregate pipeline with an explicit `CompositeSceneReplay` purpose, static work metadata, exact-token execution detail, availability state, and max attribution. In `executor.rs`, gate the measurement around the existing Composite `draw_effect_scene_range()` call, issue GPU END immediately after the draw returns, then build frame-stat deltas and attach them by token. Keep the renderer draw loop and scene-work planning unchanged.

**Tech Stack:** Rust, glow timer queries, existing `EffectGpuProfiler`/`TimingState`, Cargo unit tests, Markdown qualification documentation.

## Global Constraints

- Keep `TIMING_SPAN_POOL_CAPACITY = 2048`.
- Keep `TIMING_QUERY_OBJECT_CAPACITY = 4096`.
- Use the existing profiler and query pool; add exactly one timestamp pair per eligible Composite replay.
- Do not modify `draw_effect_scene_range()`, `draw_command_batch_range()`, visibility planning, shaders, EGL scheduling, capture, checkpoints, resources, or presentation behavior.
- Set `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target` for every Cargo build/test/check/clippy command and verify it before running the command.
- Preserve unrelated user commits and changes; commit only the logical attribution/doc changes.

### Task 1: Add failing profiler-model tests

**Files:**
- Modify: `src/egl_renderer/effects/gpu_timing.rs` test module

**Interfaces:**
- Consume the current `TimingState`, `GraphTimingScope`, and existing test timestamp helpers.
- Produce failing tests that define the replay-purpose API, aggregate fields, availability behavior, exact-token ownership, max tie behavior, formatter keys, and capacity proof.

- [ ] **Step 1: Add the failing tests**

Add tests named with the `composite_scene_replay_` prefix for span isolation, static work ownership, execution-detail ownership, slot reuse, same-frame scope separation, dropped spans, invalid timestamps, missing detail, no eligible replay, aggregation, deterministic max ties, formatter uniqueness, disabled timing, and capacity proof. Make the tests call the intended `begin_composite_scene_replay`, `attach_composite_scene_replay_execution_detail`, and resolve helpers, and assert that replay timing does not change `timed_passes`, pass totals, `max_effect_pass`, `max_capture_pass`, or graph-gap boundaries.

- [ ] **Step 2: Run the focused tests and verify the expected red state**

Run:

```bash
build_dir=/mnt/Aether/Desktop/GitHub/Typhon-target
test -n "$build_dir" && case "$build_dir" in /mnt/Aether/Desktop/GitHub/*) ;; *) exit 1 ;; esac
CARGO_TARGET_DIR="$build_dir" rtk cargo test --locked composite_scene_replay
```

Expected: compilation/test failure because the explicit replay-purpose types, APIs, record fields, and formatter keys do not yet exist.

### Task 2: Implement purpose-tagged replay spans and aggregates

**Files:**
- Modify: `src/egl_renderer/effects/gpu_timing.rs`

**Interfaces:**
- Consume `TimingSpanPurpose`, `CompositeSceneReplayWork`, and `CompositeSceneReplayExecutionDetail` from the same module.
- Produce `CompositeSceneReplayTimingSpan`, replay aggregate fields on `GraphAggregate`/`GpuTimingRecord`, exact-token attach APIs, and formatted telemetry.

- [ ] **Step 1: Add metadata/detail/work/max types and capacity proof**

Define the purpose enum and the static/execution/max structures with the requested fields. Replace the total boolean with the purpose enum, add one graph-total span plus `CURRENT_PASSES_PER_BLUR_INSTANCE + 1` per instance in the compile-time estimate, and retain the 2048/4096 constants.

- [ ] **Step 2: Add replay allocation, finish, resolution, and availability state**

Increment expected replay spans for every eligible allocation attempt, mark availability false on pool drop, invalid timestamps, or missing exact detail, count valid timestamp resolutions, aggregate saturating GPU/CPU/counter totals, and retain only the strict-`>` longest complete replay candidate. Dispatch `poll_front()` by purpose so replay spans never call `finish_pass()`.

- [ ] **Step 3: Make detail attachment exact-token based**

Find pending metadata by `SpanToken` rather than `pending.back_mut()`, and expose purpose-specific attach methods that reject stale slot generations. Keep pass and capture detail behavior intact while preventing slot reuse from mixing details.

- [ ] **Step 4: Extend `GpuTimingRecord` and canonical formatting**

Emit each required aggregate and max replay key once, with stable zero values when no max exists. Keep `pass_timed_ns`, `graph_unattributed_ns`, effect category totals, max pass attribution, and graph-gap fields unchanged.

- [ ] **Step 5: Run the focused profiler tests green**

Run the same `CARGO_TARGET_DIR`-verified `rtk cargo test --locked composite_scene_replay` command and fix production code until the new tests and existing `gpu_timing` tests pass.

### Task 3: Add the Composite executor measurement boundary

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs`

**Interfaces:**
- Consume `CompositeSceneReplayWork`, `CompositeSceneReplayExecutionDetail`, `GlesSceneFrameStats`, and `EffectGpuProfiler` replay APIs.
- Produce Composite-only eligibility, before/after stats delta construction, exact GPU END ordering, and error-safe closing.

- [ ] **Step 1: Add the ordering regression test**

Extend the existing timing finalization helper tests with a replay helper test that records events and requires `draw_returns`, `gpu_end`, `detail_build`, and `detail_attach` order, with no detail-building callback before GPU END.

- [ ] **Step 2: Add the helper and frame-stat delta construction**

Implement a finalization helper that closes the span first, invokes a detail-builder second, and attaches the detail third. Compute saturating deltas for `commands_considered`, `commands_executed`, `draw_calls`, `texture_binds`, `scene_vbo_uploads`, and `scene_vbo_upload_bytes` from `GlesSceneFrameStats` snapshots; do not add loop-local counters.

- [ ] **Step 3: Instrument only the Composite `composite_advance` block**

Before the existing draw call, require a live graph scope, `pass.kind == Composite`, `draw_end > scene_cursor`, non-empty `scene_work_state.active_work()`, and `!renderer.capture_in_progress`. Capture work metadata from the existing active region and command vector, allocate the replay span, and only then start `Instant`/stats snapshots. Call the existing draw exactly once.

- [ ] **Step 4: Close on success and error without changing propagation**

Store the draw result, capture host end immediately before the profiler END call, run the finalization helper, then return the original draw error. Keep scene validity/cursor/trace behavior in its existing order and do not instrument checkpoint, framebuffer-capture, final-scene, or OutputPostProcess replay.

- [ ] **Step 5: Add executor gate and disabled-profiler tests**

Verify that OutputPostProcess, empty ranges, empty active work, capture-in-progress, and no graph scope do not allocate replay timing or take profiler-only snapshots, while ordinary rendering call structure remains unchanged.

- [ ] **Step 6: Run focused executor/profiler tests green**

Run:

```bash
build_dir=/mnt/Aether/Desktop/GitHub/Typhon-target
test -n "$build_dir" && case "$build_dir" in /mnt/Aether/Desktop/GitHub/*) ;; *) exit 1 ;; esac
CARGO_TARGET_DIR="$build_dir" rtk cargo test --locked composite_scene_replay
CARGO_TARGET_DIR="$build_dir" rtk cargo test --locked gpu_timing
CARGO_TARGET_DIR="$build_dir" rtk cargo test --locked effect_gpu_timing
```

### Task 4: Update qualification documentation

**Files:**
- Modify: `docs/EFFECTS_QUALIFICATION.md`

**Interfaces:**
- Consume the telemetry formatter contract and the existing graph-gap semantics.
- Produce durable documentation for eligibility, CPU/GPU distinction, work-shape metrics, physical counters, query cost, availability, and unchanged graph gaps.

- [ ] **Step 1: Document the new fields and units**

Add the aggregate and max field lists, explain logical `command_count` versus full-vector `scene_scan_pairs`, identify physical `commands_executed`/`draw_calls`/`texture_binds`, and state that CPU and GPU durations may overlap and must not be added.

- [ ] **Step 2: Document scope and semantics**

State Composite-only eligibility, no-work availability, dropped/invalid/missing-detail unavailability, exact owner attribution, one additional timestamp pair per eligible replay, unchanged pass totals and graph gaps, and the deliberate exclusion of other replay reasons.

### Task 5: Verify, inspect, and commit

**Files:**
- Modify: `src/egl_renderer/effects/gpu_timing.rs`
- Modify: `src/egl_renderer/effects/executor.rs`
- Modify: `docs/EFFECTS_QUALIFICATION.md`

- [ ] **Step 1: Format and run the complete deterministic suite with Aether output**

Verify `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target` before each command, then run the requested focused tests, `rtk cargo fmt --check`, locked all-target check, strict all-target Clippy, and locked full tests.

- [ ] **Step 2: Inspect source-level invariants**

Confirm the pool constants remain 2048/4096, `rg` finds exactly four `query_counter(... TIMESTAMP)` call sites before the implementation and exactly six afterward, replay spans do not reach `finish_pass()` or `record_pass_interval()`, and the renderer draw/visibility functions are unchanged.

- [ ] **Step 3: Attempt native qualification only if the environment supports it**

Check for the established controlling TTY/DRM workload. If unavailable, record the concrete blocker and do not claim native results. If available, run the exact requested environment and workload, collect the top-20 gap samples, and compute replay/gap ratios offline without adding runtime telemetry.

- [ ] **Step 4: Commit the logical change**

After fresh verification and review of `git diff`, commit only the three implementation/documentation files plus the approved design/plan artifacts with:

```bash
git add src/egl_renderer/effects/gpu_timing.rs src/egl_renderer/effects/executor.rs docs/EFFECTS_QUALIFICATION.md docs/superpowers/specs/2026-09-21-composite-scene-replay-attribution-v1-design.md docs/superpowers/plans/2026-09-21-composite-scene-replay-attribution-v1.md
git commit -m "feat(effects): attribute composite scene replay"
```
