# Checkpoint Shader-Copy Execution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Connect the existing checkpoint shader-copy diagnostic to production execution through one authoritative execution plan, with trace, GPU timing, GLES pixel-equivalence, and native graph evidence.

**Architecture:** Keep the existing framebuffer-blit and shader-copy acquisition functions in `src/egl_renderer/effects/executor.rs`. Replace `CapturePathDecision` with one per-pass `CheckpointCaptureExecutionPlan` computed in `execute_graph_passes_inner`; pass that immutable result through trace, timing metadata, and `execute_pass` into capture execution. Keep the existing sampleable output capability, capture-copy program, fullscreen quad, graph texture target, semantic validity, domains, and Kawase unchanged.

**Tech Stack:** Rust, GLES 3.0 via `glow`, Cargo tests, existing native-faithful EGL test harness, Codebase Memory MCP, repository-local `rtk` command wrapper.

## Global Constraints

- Do not redesign the diagnostic.
- Do not implement zero-copy aliasing.
- Do not implement an internal composition framebuffer.
- Do not change checkpoint semantic validity.
- Do not change Kawase.
- Do not change capture domains.
- Keep `GraphTextureSource::Output` sampling rejected by `EffectExecutionInvariantError::SampledOutputTexture`.
- Keep compilation artifacts in the repository folder and use `rtk` for commands.
- Do not use subagents.
- Preserve unrelated existing worktree modifications.

---

### Task 1: Add the focused RED planner tests

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs` in the existing `#[cfg(test)]` executor test module near `capture_timing_metadata_uses_execution_authority`

**Interfaces:**
- Consumes: `EffectDebugCaptureMode`, `CheckpointCapturePath`, `RenderPassKind`.
- Produces: failing tests that define `CheckpointCaptureExecutionPlan`’s requested, executed, and fallback contract for eligible and unavailable-output checkpoint captures.

- [ ] **Step 1: Write the failing eligible-path test**

Add a test that calls the not-yet-created pure planner with `SceneCapture`, one checkpoint dependency, `lifecycle_backdrop=false`, `Replay`, `FramebufferShaderCopy`, and `active_output_texture_available=true`, then asserts:

```rust
assert_eq!(plan.requested, Some(CheckpointCapturePath::FramebufferShaderCopy));
assert_eq!(plan.executed, CaptureTimingMode::FramebufferShaderCopy);
assert_eq!(plan.fallback_reason, None);
```

- [ ] **Step 2: Add the no-output fallback test**

Use the same eligible inputs with `active_output_texture_available=false` and assert:

```rust
assert_eq!(plan.executed, CaptureTimingMode::FramebufferBlit);
assert_eq!(
    plan.fallback_reason,
    Some(CapturePathFallbackReason::NoSampleableOutputTexture),
);
```

- [ ] **Step 3: Run the focused tests and verify RED**

Run:

```bash
CARGO_TARGET_DIR=$PWD/target rtk cargo test -p oblivion-one egl_renderer::effects::executor::tests::checkpoint_capture_execution_plan -- --nocapture
```

Expected: compilation/test failure because the authoritative planner and its returned plan type do not exist yet. Do not implement production code until this failure is observed.

- [ ] **Step 4: Commit the RED tests**

```bash
git add src/egl_renderer/effects/executor.rs
git commit -m "test(effects): define checkpoint capture execution plan"
```

### Task 2: Implement one per-pass execution authority

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs`
- Modify: `src/egl_renderer/effects/trace.rs` only if the planner’s public-to-module diagnostic enums need an existing derive or string conversion

**Interfaces:**
- Consumes: pass kind, checkpoint dependency count, lifecycle-backdrop state, debug capture mode, requested checkpoint path, and `renderer.active_output_texture.is_some()`.
- Produces: `CheckpointCaptureExecutionPlan { requested, executed, fallback_reason }`, computed once for each pass in `execute_graph_passes_inner` and passed to all consumers.

- [ ] **Step 1: Add the pure planner and make the RED tests compile**

Define a small input struct and plan in `executor.rs`:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CheckpointCaptureExecutionPlan {
    requested: Option<CheckpointCapturePath>,
    executed: CaptureTimingMode,
    fallback_reason: Option<CapturePathFallbackReason>,
}

fn checkpoint_capture_execution_plan(
    pass_kind: RenderPassKind,
    checkpoint_dependency_count: usize,
    lifecycle_backdrop: bool,
    capture_mode: EffectDebugCaptureMode,
    requested_path: CheckpointCapturePath,
    active_output_texture_available: bool,
) -> CheckpointCaptureExecutionPlan { /* eligibility rules only */ }
```

Select shader-copy only for `SceneCapture`, non-empty dependencies, no lifecycle backdrop, replay mode, shader-copy request, and available output texture. Select blit for all other direct captures; record `NoSampleableOutputTexture` only for an eligible shader-copy request without output texture. Return replay for non-direct capture passes.

- [ ] **Step 2: Run the focused planner tests and verify GREEN**

Run the Task 1 command. Expected: both planner tests pass.

- [ ] **Step 3: Compute the plan once and thread it through production consumers**

In `execute_graph_passes_inner`, compute the plan once per selected pass before the first trace boundary. Add a plan parameter to every `pass_trace_summary` call, `capture_timing_metadata`, `execute_pass`, and `execute_capture`. Remove `CapturePathDecision` and all independent calls to `capture_path_decision`.

- [ ] **Step 4: Preserve direct-capture behavior and defaults**

Keep `is_direct_framebuffer_capture` unchanged. Make the default/invalid config tests assert blit execution, keep the invalid-value warning once-only behavior, and preserve `Framebuffer` debug mode, lifecycle backdrop, `SurfaceCapture`, and ordinary replay semantics.

- [ ] **Step 5: Run executor diagnostic tests**

Run:

```bash
CARGO_TARGET_DIR=$PWD/target rtk cargo test -p oblivion-one egl_renderer::effects::executor -- --nocapture
```

Expected: existing semantic-validity, dependency, coordinate, filtering, and planner tests pass.

- [ ] **Step 6: Commit the planner wiring**

```bash
git add src/egl_renderer/effects/executor.rs src/egl_renderer/effects/trace.rs
git commit -m "fix(effects): centralize checkpoint capture path selection"
```

### Task 3: Route real shader-copy execution and strengthen state/invariant coverage

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs`
- Modify: `src/egl_renderer.rs` only for renderer capability/state tests or explicit output-texture argument plumbing
- Test: existing `capture_renderer_state_restores_active_output_texture` and `shader_copy_maps_each_capture_pixel_like_framebuffer_blit`

**Interfaces:**
- Consumes: the per-pass execution plan and `renderer.active_output_texture` capability.
- Produces: real production calls to `capture_output_region_to_graph_texture_shader_copy`, explicit source texture input, destination-only draw framebuffer binding, and ordinary scene-state restoration.

- [ ] **Step 1: Change `execute_capture` to match on `plan.executed`**

For direct capture, call the existing blit function for `FramebufferBlit` and the shader-copy function for `FramebufferShaderCopy`; record stats using the same executed mode. Leave replay capture materialization and scene drawing unchanged.

- [ ] **Step 2: Make shader-copy’s source capability explicit**

Pass the sampleable output texture obtained from the execution context into `capture_output_region_to_graph_texture_shader_copy`. Keep the destination as the exact pooled `PooledEffectTexture`, keep the existing coordinate mapping and uniforms, use `ensure_effect_quad`, and do not add per-capture geometry or synchronization calls.

- [ ] **Step 3: Preserve and test the feedback invariant**

At draw time require the bound draw framebuffer to differ from `renderer.active_output_framebuffer`; preserve the `GraphTextureSource::Output` rejection and `SampledOutputTexture` invariant. Keep the existing renderer state restoration test and strengthen it so nested/internal capture restores `active_output_texture=None` when no capability was active on entry.

- [ ] **Step 4: Run the focused coordinate/state tests**

Run:

```bash
CARGO_TARGET_DIR=$PWD/target rtk cargo test -p oblivion-one shader_copy_maps_each_capture_pixel_like_framebuffer_blit -- --nocapture
CARGO_TARGET_DIR=$PWD/target rtk cargo test -p oblivion-one capture_renderer_state_restores_active_output_texture -- --nocapture
```

Expected: the pure mapping and state-restoration tests pass, with no semantic-validity changes.

- [ ] **Step 5: Commit real execution wiring**

```bash
git add src/egl_renderer/effects/executor.rs src/egl_renderer.rs
git commit -m "fix(effects): execute checkpoint shader-copy captures"
```

### Task 4: Make trace and GPU timing consume executed paths

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs`
- Modify: `src/egl_renderer/effects/gpu_timing.rs`
- Modify: `src/egl_renderer/effects/trace.rs` only for event formatting fields

**Interfaces:**
- Consumes: `CheckpointCaptureExecutionPlan` and `CaptureTimingMode` from the pass execution loop.
- Produces: authoritative requested/executed/fallback trace fields, separate blit/shader-copy aggregates, aggregate compatibility totals, and one bounded `effect_capture_gpu_timing` event per resolved timed capture span.

- [ ] **Step 1: Populate trace from the plan**

Use `plan.requested`, `plan.executed.as_str()`, and `plan.fallback_reason` in `PassTraceSummary`. Keep `capture_mode=framebuffer` for broad framebuffer semantics where appropriate and make `executed_capture_path` authoritative.

- [ ] **Step 2: Attribute timing metadata from the plan**

Make `capture_timing_metadata` accept the already-computed plan. Map blit to `CaptureTimingMode::FramebufferBlit` and shader-copy to `CaptureTimingMode::FramebufferShaderCopy`; do not recompute from config.

- [ ] **Step 3: Emit path-specific aggregate fields**

Extend the formatted `event=effect_gpu_timing` output with bounded fields for framebuffer blit and framebuffer shader-copy nanoseconds, passes, and pixels. Keep existing `framebuffer_capture_*` fields as compatibility aggregates including both paths.

- [ ] **Step 4: Emit per-capture timing events**

When GPU timing is enabled and a capture span resolves, format exactly one bounded event with `frame_id`, `pass`, `instance`, `kind`, `capture_path`, `checkpoint_count`, `pixels`, and `duration_ns`. Do not include region lists.

- [ ] **Step 5: Add pure timing/trace assertions**

Extend the executor and GPU timing tests to assert the shader-copy and unavailable-output paths report the expected mode, fallback, aggregate bucket, and event fields.

- [ ] **Step 6: Run timing/trace tests and commit**

```bash
CARGO_TARGET_DIR=$PWD/target rtk cargo test -p oblivion-one egl_renderer::effects::gpu_timing -- --nocapture
CARGO_TARGET_DIR=$PWD/target rtk cargo test -p oblivion-one egl_renderer::effects::executor -- --nocapture
git add src/egl_renderer/effects/executor.rs src/egl_renderer/effects/gpu_timing.rs src/egl_renderer/effects/trace.rs
git commit -m "feat(effects): attribute checkpoint capture timing by path"
```

### Task 5: Prove native GLES pixel equivalence

**Files:**
- Modify: `src/egl_renderer.rs`
- Keep: existing `shader_copy_maps_each_capture_pixel_like_framebuffer_blit` test in `src/egl_renderer/effects/executor.rs`

**Interfaces:**
- Consumes: the existing `GlesEffectTestHarness`, output framebuffer/texture setup, pooled graph targets, and both capture functions.
- Produces: exact readback equality for real GLES blit and shader-copy captures at both framebuffer origins and edge/interior domains.

- [ ] **Step 1: Extend the real GLES pattern fixture**

Render horizontal variation, vertical variation, checker detail, and non-trivial alpha into one texture-backed output image that is simultaneously the source texture and framebuffer attachment.

- [ ] **Step 2: Cover required domains/origins**

Run independent destination graph textures for `BottomLeft` and `TopLeftScanout`, covering dock-like interior/bottom, top-bar edge, and ordinary interior domains.

- [ ] **Step 3: Read back only in test code and assert exact equality**

Read both destinations after capture and assert the full pixel buffers are identical. Assert the shader-copy trace/path proof is not a fallback.

- [ ] **Step 4: Run the real GLES test**

```bash
CARGO_TARGET_DIR=$PWD/target rtk cargo test -p oblivion-one gles_framebuffer_blit_and_shader_copy_capture_pixels_match_at_edges -- --nocapture
```

Expected: exact pixel equality for every listed domain/origin.

- [ ] **Step 5: Commit GLES coverage**

```bash
git add src/egl_renderer.rs
git commit -m "test(effects): compare native GLES capture paths"
```

### Task 6: Prove Dock, TopBar, and multi-dependency graph A/B

**Files:**
- Modify: `src/egl_renderer.rs` only where existing native-faithful fixtures/assertions need stronger path proof
- Modify: `src/egl_renderer/effects/executor.rs` only if a failing graph assertion exposes a regression in existing semantic validity

**Interfaces:**
- Consumes: `native_dock_fixture`, `native_topbar_fixture`, `native_three_checkpoint_fixture`, and existing rendering helpers.
- Produces: full-graph A/B evidence for final pixels, shader-copy execution, semantic validity, checkpoint ordering, and presentation repair.

- [ ] **Step 1: Run and strengthen Dock A/B**

Run blit and shader-copy configs through the native-faithful Dock graph; require equal final pixels, `executed_capture_path=framebuffer_shader_copy`, `missing_pixels=0`, unchanged checkpoint ordering, and the same presentation repair.

- [ ] **Step 2: Run and strengthen TopBar A/B**

Repeat the same assertions using the native-faithful TopBar graph with a real checkpoint dependency.

- [ ] **Step 3: Run multi-dependency A/B**

Run both paths through the existing three-checkpoint fixture; require final pixel equality, all checkpoint semantic validity events with `missing_pixels=0`, and unchanged dependency ordering.

- [ ] **Step 4: Run the three graph tests**

```bash
CARGO_TARGET_DIR=$PWD/target rtk cargo test -p oblivion-one native_faithful_topbar_blit_and_shader_copy_are_pixel_equivalent -- --nocapture
CARGO_TARGET_DIR=$PWD/target rtk cargo test -p oblivion-one native_faithful_stacked_dock_blit_and_shader_copy_are_pixel_equivalent -- --nocapture
CARGO_TARGET_DIR=$PWD/target rtk cargo test -p oblivion-one native_three_checkpoint_blit_and_shader_copy_preserve_dependencies -- --nocapture
```

Expected: shader-copy is selected and executed; no test may pass by fallback to blit.

- [ ] **Step 5: Commit graph evidence**

```bash
git add src/egl_renderer.rs src/egl_renderer/effects/executor.rs
git commit -m "test(effects): prove native checkpoint shader-copy A/B"
```

### Task 7: Full verification and Codebase Memory authority audit

**Files:**
- No new production files; inspect the committed implementation and current graph

- [ ] **Step 1: Run all diagnostic-specific tests**

Run the planner, executor, GPU timing, real GLES, Dock, TopBar, and multi-dependency test filters used above, then run the broader `egl_renderer` test target if the focused tests pass.

- [ ] **Step 2: Run required repository checks with local artifacts**

```bash
CARGO_TARGET_DIR=$PWD/target rtk cargo fmt --check
CARGO_TARGET_DIR=$PWD/target rtk cargo check --workspace --all-targets
CARGO_TARGET_DIR=$PWD/target rtk cargo clippy --workspace --all-targets -- -D warnings
```

Record exact exit codes/output. Report unrelated pre-existing Clippy debt separately and do not claim a pass without output.

- [ ] **Step 3: Refresh/query Codebase Memory after implementation**

Use `index_status` and, if needed, `index_repository` for the current commit. Use `search_graph`, `trace_path`, `get_code_snippet`, and `check_index_coverage` to verify:

```text
EffectDebugConfig::checkpoint_capture_path()
  -> execution planner
GlesSceneRenderer.active_output_texture
  -> shader-copy capture implementation
shader-copy program
  <- production checkpoint execution
one path-selection authority
```

- [ ] **Step 4: Review the final diff and commit verification documentation if needed**

```bash
rtk git diff --check HEAD~1..HEAD
rtk git status --short
```

Preserve the four unrelated pre-existing modifications and commit only implementation/test changes that belong to this task.
