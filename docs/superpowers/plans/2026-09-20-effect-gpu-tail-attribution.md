# Effect GPU Tail Attribution Implementation Plan

> **For agentic workers:** Execute inline in this checkout; subagents are prohibited. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Attribute the slowest valid timed render pass and its bounded workload in the existing one-line-per-graph GPU timing record.

**Architecture:** Carry an immutable `PassTimingWork` in each existing timestamp span. On valid resolution, `TimingState::finish_pass()` independently selects `MaxEffectPassTiming` before capture filtering, while existing `MaxCapturePassTiming` and Replay detail remain untouched. Derive graph coverage from the category-duration array and append scalar fields to the canonical formatter.

**Tech Stack:** Rust, GLES timestamp queries through `glow`, deterministic unit tests in `src/egl_renderer/effects/gpu_timing.rs`.

## Global Constraints

- Add zero GPU timing queries; preserve `TIMING_SPAN_POOL_CAPACITY = 2048` and `TIMING_QUERY_OBJECT_CAPACITY = 4096`.
- Preserve the existing `gl.query_counter(..., glow::TIMESTAMP)` call-site count and all START/END ordering.
- Compute workload metadata only inside the `graph_scope.and_then(...)` profiler-active branch.
- Do not change pass execution, selection, drawing, capture, allocation, shaders, EGL priority, or scheduling.
- Keep effect-space demanded-region pixels unchanged and report planned target dimensions separately.
- Keep `MaxCapturePassTiming` and Replay Attribution v2 semantics unchanged.
- Use strict greater-than selection for deterministic first-winner ties; invalid spans never become candidates.
- Run Cargo commands from `/mnt/Aether/Desktop/GitHub` with `--manifest-path /home/agony/GitHub/Typhon/Cargo.toml`, retaining the current checkout's `target/` output.

---

### Task 1: Add a failing max-effect formatter regression

**Files:**
- Modify: `src/egl_renderer/effects/gpu_timing.rs` test module.

**Interfaces:**
- Consumes: Existing `TimingState`, `TimingSpanMetadata` test helper, `GpuTimingRecord`, and `format_gpu_timing_line()`.
- Produces: A regression proving a valid non-capture pass is reported as `max_effect_*` independently of `max_capture_*`.

- [x] **Step 1: Write the failing test.** Extended the canonical formatting regression and added the 40/300/100/80 ns selection fixture.

- [x] **Step 2: Run the focused test and confirm the expected assertion failure.** The new tests compiled and failed on missing `pass_timed_ns` and `max_effect_pass_ns` before implementation.

### Task 2: Carry workload and select the independent graph max

**Files:**
- Modify: `src/egl_renderer/effects/gpu_timing.rs`.

**Interfaces:**
- Consumes: The failing regression from Task 1 and existing `CaptureTimingMetadata`.
- Produces: `PassTimingWork { effect_pixels, damage_rect_count, damage_bbox_pixels, target_width, target_height }`, `MaxEffectPassTiming`, and stable `RenderPassKind` names.

- [x] **Step 1: Add `PassTimingWork` and `MaxEffectPassTiming`.** Added span workload, independent graph candidate, and test-helper values.

- [x] **Step 2: Select after a valid resolution and before the capture filter.** `finish_pass()` updates max effect before the capture-only early return and uses strict greater-than comparison.

- [x] **Step 3: Add deterministic selection and ownership regressions.** Added non-capture and capture winners, distinct workload attribution, invalid timestamp, and same-frame scope tests.

- [x] **Step 4: Format and verify stable values.** Added the explicit kind mapping and appends; capture keys remain in their original order and values.

- [x] **Step 5: Run focused regressions and confirm they pass.** `gpu_timing`: 44 passed; `effect_gpu_timing`: 1 passed. A later rerun was blocked by the live unrelated deletion noted under Task 5.

### Task 3: Compute work only in the active timing branch

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs`.
- Modify: `src/egl_renderer/effects/gpu_timing.rs` profiler `begin_pass()` signature.

**Interfaces:**
- Consumes: `PassTimingWork` from Task 2, the existing `execution_damage.region`, `CompiledRenderPass::output`, and `CompiledFrameGraph::textures`.
- Produces: Exact execution damage shape and planned output dimensions carried with the existing timed pass.

- [x] **Step 1: Build the workload inside `graph_scope.and_then(...)`.** Implemented inside the existing active timing closure after the capture-in-progress return.

- [x] **Step 2: Pass the workload into the existing profiler begin call.** Kept the existing timestamp boundaries and direct planned-texture lookup.

- [x] **Step 3: Run disabled-timing and pool regressions.** Both focused filters passed before the shared checkout became uncompilable; source retains capacities and four timestamp call sites.

### Task 4: Expose graph coverage and document units

**Files:**
- Modify: `src/egl_renderer/effects/gpu_timing.rs`.
- Modify: `docs/EFFECTS_QUALIFICATION.md`.

**Interfaces:**
- Consumes: Existing ten category duration totals and `total_ns`.
- Produces: `pass_timed_ns` and saturating `graph_unattributed_ns` in the canonical record, with field definitions in the qualification guide.

- [x] **Step 1: Add category-sum and remainder accessors.** Added saturating category sum and remainder tests, including a sum greater than total.

- [x] **Step 2: Finish the formatter regression.** Covers unique keys, exactly-once new fields, empty defaults, exact existing capture prefix, and no string replacement.

- [x] **Step 3: Update `docs/EFFECTS_QUALIFICATION.md`.** Documented the units, max authorities, and remainder limitation.

### Task 5: Verify the completed observability closure

**Files:**
- Inspect: `src/egl_renderer/effects/gpu_timing.rs` query capacities and timestamp calls.
- Inspect: final source diff and qualification documentation.

- [x] **Step 1: Run focused GPU timing tests first.** Both focused filters passed: 44 and 1 tests respectively.

- [x] **Step 2: Run all requested deterministic checks.** Formatting and all-target check passed (the check emitted four compositor warnings). Strict Clippy failed on 12 unrelated warnings promoted to errors; the full test run had 10 failures outside GPU timing. A later timing-test rerun could not compile after `selection.rs` was deleted concurrently.

- [x] **Step 3: Inspect bounded-query invariants.** Confirmed 2048 spans, 4096 objects derived as 2048 × 2, and four timestamp call sites before and after.

- [x] **Step 4: Attempt the approved native diagnostic run.** The host reports `tty: not a tty`, and no Typhon/compositor process is visible; the documented TTY/DRM qualification prerequisite is unavailable, so no native samples are claimed.

- [x] **Step 5: Commit only task-owned changes.** The implementation plan, telemetry, tests, and qualification documentation are committed; unrelated shared-checkout changes remain unstaged.
