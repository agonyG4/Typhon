# Checkpoint Capture Path Diagnostic Implementation Plan

> **For agentic workers:** Execute this plan inline, task-by-task, because subagents are prohibited. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a deterministic framebuffer-blit versus exact shader-copy diagnostic for checkpoint-dependent native `SceneCapture` while preserving the blit default and all existing graph semantics.

**Architecture:** Propagate the Atomic EGL/GBM output texture as a narrow non-owning render-target capability and save/restore it with the active output framebuffer. Select shader-copy only for checkpoint-dependent direct `SceneCapture` under replay policy, copy exact source texels into the same pooled capture texture, and leave every downstream pass unchanged. Add bounded per-pass path traces and path-specific GPU timing fields/events.

**Tech Stack:** Rust, `glow`, GLES 3.0 shaders, EGL/GBM Atomic output, Typhon effect render graph, `rtk` command wrapper.

## Global Constraints

- Compile in `/home/agony/GitHub/Typhon` so build artifacts stay in the same folder.
- Use `rtk` for repository commands and test/build output.
- Do not use subagents.
- Preserve all pre-existing dirty-worktree changes; stage only this task’s hunks.
- Production default is `TYPHON_EFFECT_DEBUG_CHECKPOINT_CAPTURE_PATH=blit`.
- Accepted checkpoint-path values are exactly `blit` and `shader-copy`; invalid values warn once and fall back to `blit`.
- Do not implement zero-copy aliasing, an internal composition framebuffer, `PersistentBackdropCache`, Kawase changes, checkpoint-domain/validity changes, or partial-capture optimization.
- Do not weaken `GraphTextureSource::Output` validation or ordinary graph-input semantics.
- Do not add `glFinish`, `glFlush`, EGL waits, CPU readback, or `glReadPixels` to production capture.
- The supplied native texture is owned and deleted by `AtomicOutputSlot`, never by `GlesSceneRenderer`.
- No automated test may assert that shader-copy is faster.

---

## Files and responsibilities

- `src/egl_renderer.rs`: render-target capability, active output texture state, nested state restoration, and test-only state/GLES fixture support.
- `src/native_output/scanout/atomic_egl_gbm.rs`: pass `Some(slot.texture)` for the Atomic EGL/GBM target; do not change slot ownership.
- `src/egl_renderer/program.rs`: compile the dedicated exact copy vertex/fragment program and initialize its sampler binding.
- `src/egl_renderer/effects/executor.rs`: path policy, exact shader copy, explicit DRAW/texture invariant, existing blit control path, and capture execution stats.
- `src/egl_renderer/effects/trace.rs`: diagnostic environment parsing and bounded requested/executed path/fallback fields.
- `src/egl_renderer/effects/gpu_timing.rs`: path-specific aggregate metadata and bounded per-capture GPU timing events.
- `src/egl_renderer/effects/mod.rs`: narrow test/production re-exports only when required by the existing module boundary.

## Interfaces

The implementation will expose these internal concepts:

```rust
pub(crate) struct EglOutputRenderTarget {
    pub(crate) framebuffer: glow::Framebuffer,
    pub(crate) sampleable_texture: Option<glow::Texture>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) buffer_age: BufferAge,
    pub(crate) framebuffer_origin: OutputFramebufferOrigin,
}

pub(crate) enum CheckpointCapturePath {
    FramebufferBlit,
    FramebufferShaderCopy,
}

pub(crate) enum CapturePathFallbackReason {
    NoSampleableOutputTexture,
}
```

The exact visibility/naming may follow current module conventions, but the
default behavior and semantic boundaries above are fixed.

### Task 1: Add RED unit tests for configuration, state, mapping, and timing

**Files:**
- Modify: `src/egl_renderer.rs` tests and render-target/state definitions.
- Modify: `src/egl_renderer/effects/executor.rs` coordinate and path-policy tests.
- Modify: `src/egl_renderer/effects/trace.rs` environment/trace tests.
- Modify: `src/egl_renderer/effects/gpu_timing.rs` aggregation/event tests.

**Interfaces:**
- Consumes: current `CaptureRendererState`, `plan_graph_texture_capture`, `EffectDebugConfig`, `CaptureTimingMetadata`, and existing test GLES harness.
- Produces: failing tests for the new output-texture state, parser, exact source mapping, path metadata, and path-specific profiler fields.

- [ ] **Step 1: Write the failing state-restoration test.**

  Add a focused test helper that creates a renderer state snapshot with a
  non-`None` active framebuffer and output texture, changes both fields to a
  nested target state, restores the snapshot, and asserts both original handles
  are restored. The test must also assert a target with no sampleable texture
  leaves the active texture `None` during the nested operation. Keep ownership
  out of the renderer test; the handles are borrowed GL names.

- [ ] **Step 2: Write failing configuration tests.**

  Extend `EffectDebugConfig::from_env_values` tests with:

  ```rust
  assert_eq!(config(None), CheckpointCapturePath::FramebufferBlit);
  assert_eq!(config(Some("blit")), CheckpointCapturePath::FramebufferBlit);
  assert_eq!(config(Some("shader-copy")), CheckpointCapturePath::FramebufferShaderCopy);
  assert_eq!(config(Some("auto")), CheckpointCapturePath::FramebufferBlit);
  ```

  Capture the warning through the existing test convention if available; the
  parser must not warn for missing or accepted values and must not accept
  `auto` or any other value.

- [ ] **Step 3: Write failing pure mapping tests.**

  Add a helper that returns the integer source texel for destination physical
  coordinates and compare it to the existing `GlBlitRect` mapping. Cover
  `(1920,1080)` domains `(762,976,396,104)`, `(0,0,120,65)`,
  `(1610,0,310,65)`, and an interior domain such as `(700,400,320,180)` for
  `BottomLeft` and `TopLeftScanout`. Test the four edge domains separately and
  require that every destination pixel maps to the same source pixel as the
  blit planner.

- [ ] **Step 4: Write failing trace/timing tests.**

  Add assertions for `requested_capture_path`, `executed_capture_path`, and
  `fallback_reason=no-sampleable-output-texture`. Extend profiler fixtures with
  `FramebufferShaderCopy` and assert that blit and shader-copy durations,
  passes, and pixels aggregate independently while legacy aggregate fields stay
  populated.

- [ ] **Step 5: Run the focused new tests and verify RED.**

  Run:

  ```bash
  rtk cargo test --locked egl_renderer::tests:: --lib
  rtk cargo test --locked executor --lib
  rtk cargo test --locked trace --lib
  rtk cargo test --locked gpu_timing --lib
  ```

  Expected: compilation/test failures are caused by the missing new path/state/
  timing behavior, not malformed fixtures.

### Task 2: Implement render-target capability and exact copy program

**Files:**
- Modify: `src/egl_renderer.rs`.
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`.
- Modify: `src/egl_renderer/program.rs`.

**Interfaces:**
- Consumes: Task 1 state and mapping RED tests.
- Produces: `sampleable_texture`, `active_output_texture`, snapshot restoration,
  and a dedicated copy program that never owns/deletes the output texture.

- [ ] **Step 1: Add the optional render-target texture and Atomic wiring.**

  Add `sampleable_texture: Option<glow::Texture>` to
  `EglOutputRenderTarget`. Set it to `Some(slot.texture)` in
  `AtomicEglGbmScanout::render_to_slot`; set it to `None` in screenshot/test
  targets. Do not create or delete an extra texture and do not modify
  `AtomicOutputSlot` ownership.

- [ ] **Step 2: Add active texture state and restoration.**

  Add `active_output_texture: Option<glow::Texture>` beside
  `active_output_framebuffer` in `GlesSceneRenderer` and `CaptureRendererState`.
  In `draw_scene_to_target`, save the previous active handles, set both target
  handles before `draw_scene_with_buffer_age`, and restore the previous handles
  after the draw while rebinding the prior framebuffer state. Ensure the
  `CaptureRendererState::take/restore` pair includes the texture. The ordinary
  renderer constructor initializes it to `None`.

- [ ] **Step 3: Compile a dedicated GLES copy program.**

  Add `create_capture_copy_program` in `program.rs` using GLES 3.0 shaders. The
  fragment shader must use `texelFetch(u_output_texture, ivec2(source_x,
  source_y), 0)` and compute `source_x/source_y` from `gl_FragCoord`, output
  size, capture domain, target size, and an explicit bottom-left-origin flag.
  The formula must map graph physical bottom-left pixels to logical capture
  top-left pixels exactly, matching `plan_graph_texture_capture` for both
  origins. Initialize the sampler uniform to texture unit zero. Do not use
  filtering, interpolation, gamma conversion, alpha modification, or vendor
  orientation inference.

- [ ] **Step 4: Run the state/mapping tests GREEN.**

  Run the focused commands from Task 1. Confirm the mapping tests and state
  restoration test pass before wiring execution into the graph.

### Task 3: Add deterministic path selection and shader-copy execution

**Files:**
- Modify: `src/egl_renderer/effects/trace.rs`.
- Modify: `src/egl_renderer/effects/executor.rs`.
- Modify: `src/egl_renderer/effects/mod.rs` only for required internal exports.

**Interfaces:**
- Consumes: `active_output_texture`, the copy program, current direct blit
  planner, `EffectDebugConfig`, and existing `execute_capture`.
- Produces: checkpoint-only shader-copy selection with blit fallback and GL
  state restoration.

- [ ] **Step 1: Add the checkpoint-path parser and policy.**

  Introduce `TYPHON_EFFECT_DEBUG_CHECKPOINT_CAPTURE_PATH`. Parse only
  `blit`/`shader-copy`, default to blit, and warn once on invalid values. Keep
  `TYPHON_EFFECT_DEBUG_CAPTURE_MODE` independent. Add a helper whose true case
  requires all of: `SceneCapture`, non-empty checkpoint dependencies, direct
  capture due to checkpoint semantics, `lifecycle_backdrop == false`, global
  capture mode `Replay`, requested shader-copy, and an active output texture.
  SurfaceCapture, lifecycle backdrop, ordinary replay capture, and global
  framebuffer diagnostic mode must select their current behavior.

- [ ] **Step 2: Preserve the framebuffer-blit control path.**

  Leave `capture_output_region_to_graph_texture` rendering and coordinate
  planning unchanged. Move only the selection around it so `blit` and all
  fallback cases call the same function and record `framebuffer_blit`.

- [ ] **Step 3: Implement the shader-copy pass.**

  Bind the pooled target as DRAW via `bind_draw_target`; reject if that DRAW
  framebuffer equals the active output framebuffer. Disable blend and scissor,
  set the target viewport, use the dedicated copy program, bind
  `active_output_texture` on texture unit zero, set output/domain/target/origin
  uniforms, and draw the existing six-vertex effect quad. Restore ordinary
  scene state in a cleanup path whether the draw succeeds or fails. Never bind
  the active output framebuffer as DRAW while its texture is sampled.

- [ ] **Step 4: Record bounded execution-path trace fields.**

  Extend capture pass trace summaries with the requested path, executed path,
  and optional bounded fallback reason. For a successful shader copy emit
  `requested_capture_path=shader-copy executed_capture_path=framebuffer_shader_copy`.
  For unavailable capability emit
  `executed_capture_path=framebuffer_blit fallback_reason=no-sampleable-output-texture`.
  Keep ordinary non-checkpoint capture traces unchanged except for a stable
  `executed_capture_path` value where the existing capture metadata already
  identifies the path.

- [ ] **Step 5: Run shader-copy unit tests and the existing effect suite.**

  Run:

  ```bash
  rtk cargo test --locked executor --lib
  rtk cargo test --locked effects --lib
  rtk cargo test --locked egl_renderer::tests:: --lib
  ```

  Confirm no graph validator, `GraphTextureSource::Output`, Kawase, or
  checkpoint-validity code changed.

### Task 4: Extend path-specific GPU timing and per-capture events

**Files:**
- Modify: `src/egl_renderer/effects/gpu_timing.rs`.
- Modify: `src/egl_renderer/effects/executor.rs`.
- Modify: `src/egl_renderer/effects/trace.rs`.

**Interfaces:**
- Consumes: executed path from Task 3 and existing timestamp-pass metadata.
- Produces: `FramebufferShaderCopy` timing attribution and bounded
  `event=effect_capture_gpu_timing` lines only when GPU timing is enabled.

- [ ] **Step 1: Add the shader-copy timing mode and fields.**

  Extend `CaptureTimingMode` with `FramebufferShaderCopy` and keep
  `framebuffer_capture_ns` as the aggregate of blit plus shader-copy. Add
  explicit duration/pass/pixel fields for framebuffer blit and framebuffer
  shader-copy to the aggregate/record output, preserving existing field names.
  Add corresponding execution-summary pixels/pass counters without changing
  timestamp query allocation.

- [ ] **Step 2: Attach executed-path metadata to pass spans.**

  At the existing capture timing metadata call site, derive the mode from the
  actual executed path chosen by the executor, not from the requested env
  value. A fallback must be recorded as `FramebufferBlit`. Non-capture passes
  still carry no capture metadata.

- [ ] **Step 3: Emit one bounded GPU timing event per resolved capture pass.**

  When a pass timestamp resolves and GPU timing is enabled, call the trace event
  formatter with frame ID, pass ID, instance ID, render-pass kind, path string,
  checkpoint count, physical pixels, and duration. Do not include regions or
  emit anything when the profiler is disabled. Preserve invalid/disjoint/drop
  behavior and legacy aggregate records.

- [ ] **Step 4: Add profiler tests and run them GREEN.**

  Test independent blit/shader-copy aggregation, aggregate compatibility,
  checkpoint counters, max-pass metadata, invalid timing, and disabled-event
  behavior. Run:

  ```bash
  rtk cargo test --locked gpu_timing --lib
  rtk cargo test --locked trace --lib
  ```

### Task 5: Add real GLES A/B capture and state invariants

**Files:**
- Modify: `src/egl_renderer.rs` GLES test fixture/tests.
- Modify: `src/egl_renderer/effects/executor.rs` test-only helpers if needed.

**Interfaces:**
- Consumes: both capture implementations and exact mapping helper.
- Produces: pixel-level proof that blit and shader-copy produce identical
  capture textures and that output-texture lifetime/state rules hold.

- [ ] **Step 1: Add a texture-backed output fixture.**

  Create an RGBA8 texture and framebuffer in the existing GLES fixture, attach
  the texture as COLOR_ATTACHMENT0, and set the renderer’s active output
  framebuffer/texture for the test. Keep cleanup in the test fixture’s Drop
  implementation. Render horizontal/vertical gradients, checker detail, and
  distinct alpha values into this output image.

- [ ] **Step 2: Capture the same domains by both paths.**

  For each Dock, TopBar-left/right, interior, and all four edge-touching domains,
  invoke the existing blit capture and shader-copy capture into separate pooled
  graph textures with a BottomLeft target. Read back only in the test, compare
  every RGBA byte exactly, and report the first differing coordinate/channel.
  Repeat for `BottomLeft` and `TopLeftScanout` output origins. The test must
  assert the copy DRAW framebuffer is the graph target, never the output FBO.

- [ ] **Step 3: Run the GLES A/B tests and fix production code only.**

  Run the named tests under `rtk cargo test --locked egl_renderer::tests:: --lib`.
  If equality fails, correct the explicit mapping/state handling; do not add
  tolerance or synchronization without documenting a driver/format reason.

### Task 6: Add full-effect and multi-dependency A/B regressions

**Files:**
- Modify: `src/egl_renderer.rs` native-faithful effect fixture tests.
- Modify: `src/egl_renderer/effects/trace.rs` only if test event helpers need a
  bounded path assertion.

**Interfaces:**
- Consumes: completed path selector, capture implementations, timing/trace
  metadata, and existing native stacked-backdrop fixtures.
- Produces: Dock, TopBar, and multi-dependency equivalence coverage without
  changing references or validity semantics.

- [ ] **Step 1: Run Dock and TopBar fixtures with both explicit paths.**

  Render each fixture once with `FramebufferBlit` and once with
  `FramebufferShaderCopy`, using replay policy, partial Kawase, non-empty
  checkpoint dependencies, and `TopLeftScanout`. Compare final framebuffer
  bytes exactly, require `missing_pixels=0`, identical checkpoint ordering, and
  identical presentation repair. Assert traces prove the requested and
  executed paths.

- [ ] **Step 2: Run the existing multi-checkpoint fixture with both paths.**

  Require the later checkpoint with at least two dependencies to retain the same
  validity events (`missing_pixels=0`), checkpoint ordering, and final pixels.
  Keep same-anchor coverage and the existing scene-work preservation assertions.

- [ ] **Step 3: Run focused correctness gates.**

  Run:

  ```bash
  rtk cargo test --locked egl_renderer::tests:: --lib
  rtk cargo test --locked effects --lib
  rtk cargo test --locked gpu_timing --lib
  rtk cargo test --locked trace --lib
  ```

### Task 7: Verify, document benchmark protocol, and commit task-owned changes

**Files:**
- Modify: `docs/EFFECTS_QUALIFICATION.md` if the repository’s existing timing
  documentation has a matching section; otherwise add the diagnostic protocol
  to the design/plan handoff only.
- Review: all production/test files listed above.

- [ ] **Step 1: Run formatting, compilation, and lint gates.**

  Run in `/home/agony/GitHub/Typhon`:

  ```bash
  rtk cargo fmt --check
  rtk cargo check --workspace --all-targets
  rtk cargo clippy --workspace --all-targets -- -D warnings
  ```

  Record task-owned diagnostics separately from pre-existing findings outside
  the touched hunks. Do not repair unrelated Clippy debt.

- [ ] **Step 2: Run final correctness and diff checks.**

  Run the focused suites from Tasks 5–6, `rtk git diff --check`, and inspect the
  diff for forbidden zero-copy aliases, graph-validator changes, Kawase/domain/
  validity changes, explicit synchronization, output-texture deletion, or
  automatic selection. Ensure the working tree’s unrelated edits remain
  unstaged.

- [ ] **Step 3: Record the two native benchmark commands.**

  Provide separate sessions that differ only in the path variable:

  ```bash
  TYPHON_EFFECT_DEBUG_CHECKPOINT_CAPTURE_PATH=blit \
  TYPHON_EFFECT_EXEC_TRACE=1 TYPHON_EFFECT_GPU_TIMING=1 \
  TYPHON_EFFECT_DEBUG_CAPTURE_MODE=replay \
  TYPHON_EFFECT_DEBUG_KAWASE_MODE=partial <native-session-command>

  TYPHON_EFFECT_DEBUG_CHECKPOINT_CAPTURE_PATH=shader-copy \
  TYPHON_EFFECT_EXEC_TRACE=1 TYPHON_EFFECT_GPU_TIMING=1 \
  TYPHON_EFFECT_DEBUG_CAPTURE_MODE=replay \
  TYPHON_EFFECT_DEBUG_KAWASE_MODE=partial <native-session-command>
  ```

  Do not invent a workload result if the native EGL/GBM/DRM session is
  unavailable. A valid comparison requires multiple partial/replay/partial-
  Kawase/checkpoint/missing-zero events and no shader-copy fallback in Run B.

- [ ] **Step 4: Commit only this task’s changes.**

  Stage the design/plan and task-owned hunks only. Preserve unrelated dirty
  files and use a commit message such as:

  ```bash
  git add <task-owned paths and selected hunks>
  git commit -m "feat(effects): diagnose checkpoint capture paths"
  ```

## Self-review checklist

- [ ] Every requirement maps to a task above: capability lifetime, path policy,
  exact mapping, GL state, traces, timing, GLES A/B, edge origins, full-effect
  equivalence, multi-dependency validity, verification, and native protocol.
- [ ] No task changes graph texture semantics, checkpoint domains/validity,
  Kawase, partial capture, resource-pool ownership, or output mutation order.
- [ ] No performance assertion is automated; native results are evidence only.
- [ ] The final default and invalid-value fallback are both FramebufferBlit.
