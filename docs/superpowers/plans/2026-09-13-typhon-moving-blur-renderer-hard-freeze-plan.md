# Typhon Moving-Blur Renderer Hard-Freeze Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Localize the native Kitty moving-blur hard freeze, enforce proven effect-graph safety contracts, add deterministic regressions, and apply only the smallest evidence-backed renderer correction.

**Architecture:** Add a cached, opt-in trace recorder at the renderer/effect boundary and reuse the existing effect fallback path for validation failures. Keep logical domains, physical pooled resources, FBO identities, and coordinate orientation as explicit contracts tested by pure models and the existing real GLES pbuffer harness. Native Atomic EGL/GBM remains unchanged except for trace observation and the correction proven by the trace.

**Tech Stack:** Rust 2024, glow GLES 3, khronos-egl, GBM/Atomic KMS, existing `EffectResourcePool`/`EffectGlResourceCache`, Cargo tests, and `rtk` command wrappers.

## Global Constraints

- Use `TYPHON_EFFECT_EXEC_TRACE=1` as the single effect-execution trace gate.
- When disabled, do not build trace strings, allocate per-pass trace payloads, or take a trace logging lock.
- Do not use `glFinish()` or `eglWait*()` between production effect passes.
- Preserve WaylandAuto blur, client-requested blur, public Eclipse effects, partial repaint, buffer age, Dual Kawase, output orientation, lifecycle capture, Magic Lamp capture, Atomic EGL/GBM, and the existing resource budget.
- Do not modify Eclipse unless native evidence proves a protocol defect.
- Use the existing effect fallback path for ordinary validation errors and never submit a partially rendered effect frame as successful.
- Compile and test in `/home/agony/GitHub/Typhon`; never run `cargo clean`.
- Use `rtk` for repository commands and commit each logically complete implementation milestone.
- Do not use subagents.

---

### Task 1: Add the bounded effect-execution flight recorder

**Files:**
- Create: `src/egl_renderer/effects/trace.rs`
- Modify: `src/egl_renderer/effects/mod.rs`
- Modify: `src/egl_renderer/effects/executor.rs`
- Modify: `src/egl_renderer.rs`
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Test: `src/egl_renderer/effects/trace.rs` and existing renderer test modules

**Interfaces:**
- `TraceConfig::from_env(value: Option<&OsStr>) -> TraceConfig` reports whether the exact value `1` enables tracing.
- `EffectExecutionTrace::new(frame_id: Option<u64>, render_generation: Option<u64>, scene_generation: u64, scene_signature: u64) -> Self` creates a trace context without formatting strings.
- `EffectExecutionTrace::event<F>(&self, make_line: F)` invokes `make_line` only when enabled and writes one bounded line to stderr.
- `EffectExecutionTrace::frame_boundary(&self, phase: &'static str, boundary: &'static str, summary: TraceSummary)` records begin/end frame events.
- `EffectExecutionTrace::pass_boundary(&self, boundary: &'static str, pass: &CompiledRenderPass, graph: &CompiledFrameGraph, resources: &HashMap<GraphTextureId, PooledEffectTexture>, summary: PassTraceSummary)` records bounded pass metadata.

- [ ] **Step 1: Write the failing gate and lazy-formatting tests**

  Add tests proving `None`, empty, and non-`1` values disable tracing; `Some("1")` enables it; and a closure passed to `event` is not invoked while disabled. Add a formatter test proving pass lines omit shader source and full command/region dumps.

- [ ] **Step 2: Run the trace tests and verify the intended RED**

  Run:

  ```bash
  rtk cargo test --locked egl_renderer::effects::trace
  ```

  Expected: the new tests fail because the trace module and lazy event API do not exist.

- [ ] **Step 3: Implement the cached opt-in trace module**

  Cache one process-level `bool` from `TYPHON_EFFECT_EXEC_TRACE`. Keep event construction behind the enabled check, use a closure for formatting, bound list fields to fixed scalar counts, and write only one-line summaries. Do not include shader text, scene command arrays, or unbounded region strings.

- [ ] **Step 4: Add renderer and native boundary context propagation**

  Thread optional frame ID/render generation from `NativeFrameRenderer::egl_scene_draw_request` into `EglSceneDrawRequest`. Create the trace context at the renderer frame boundary and share the trace emitter with the Atomic render-fence export observation without changing scheduling, KMS, cursor, or fence semantics.

- [ ] **Step 5: Instrument frame and pass boundaries**

  Emit begin/end around scene resolve, graph compile, demand plan, effect resource sync, graph execute, graph release, renderer draw completion, and render-fence export. Emit pass begin immediately before each selected pass and pass end after CPU-side GL submission and state restoration. Include frame/render/scene identity, repaint and damage signatures, visible/selected counts, graph stats, pass identity, anchor/scope/group, graph/physical texture IDs, domains/dimensions/origins/flips, damage summary, checkpoint count, and replay versus framebuffer-blit capture metadata.

- [ ] **Step 6: Run focused tests and commit the recorder**

  Run:

  ```bash
  rtk cargo test --locked egl_renderer::effects::trace
  rtk cargo test --locked egl_renderer::effects::blur
  rtk git diff --check
  ```

  Expected: all focused tests pass, and disabled trace tests prove no formatting closure is evaluated. Commit with:

  ```bash
  rtk git add src/egl_renderer src/native_output/runtime/frame.rs src/native_output/scanout/atomic_egl_gbm.rs
  rtk git commit -m "feat(renderer): add bounded effect execution trace"
  ```

### Task 2: Add pre-draw effect-graph safety invariants

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs`
- Modify: `src/egl_renderer/effects/resources.rs`
- Modify: `src/effects/render_graph.rs`
- Test: `src/egl_renderer/effects/executor.rs`
- Test: `src/egl_renderer/effects/resources.rs`

**Interfaces:**
- `validate_effect_pass_resources(graph, pass, textures, gl_textures, framebuffer_state) -> Result<(), EffectExecutionInvariantError>` validates the pass without GPU synchronization.
- `EffectExecutionInvariantError` identifies feedback alias, invalid domain/dimensions/scissor mapping, invalid checkpoint transfer, or incomplete target metadata and implements `Error`.
- `EffectGlResourceCache::physical_texture_id(&self, texture: &PooledEffectTexture) -> Option<u64>` exposes only diagnostic identity; it does not change ownership.

- [ ] **Step 1: Write failing alias/lifetime tests**

  Build two graph texture IDs with the same pool key and assert that concurrently checked-out resources have different physical IDs. Build a deliberately invalid pass whose input and output map to the same physical resource and assert the validator rejects it. Add a lifetime fixture proving sequential same-key reuse is accepted only after the prior pass releases the texture.

- [ ] **Step 2: Run the invariant tests and verify RED**

  Run:

  ```bash
  rtk cargo test --locked egl_renderer::effects::resources
  rtk cargo test --locked egl_renderer::effects::executor
  ```

  Expected: the tests fail because physical identity validation is not present.

- [ ] **Step 3: Implement pass validation and diagnostic resource identity**

  Validate every sampled input/output physical texture pair before issuing capture, stage, or fullscreen draws. Validate non-zero dimensions, finite integer domains, clipped capture domains, checked output mapping, in-bounds scissor rectangles, and scaled Kawase dimensions. For framebuffer-blit capture, require distinct READ and DRAW framebuffer handles, a destination that is not the active output image, and a complete attached scratch FBO. Do not silently clamp invalid metadata.

- [ ] **Step 4: Route invariant errors through the existing fallback**

  Return ordinary `RendererResult` errors from the executor. Preserve `execute_effect_graph` cleanup, ordinary GL state restoration, and `release_graph` behavior on both validation and draw errors. Include the invariant name in the enabled trace.

- [ ] **Step 5: Run focused GREEN and adjacent tests**

  Run:

  ```bash
  rtk cargo test --locked egl_renderer::effects::resources
  rtk cargo test --locked egl_renderer::effects::executor
  rtk cargo test --locked effects::render_graph
  rtk git diff --check
  ```

  Expected: alias/lifetime/FBO/domain tests pass and no existing effect fallback or graph tests regress.

- [ ] **Step 6: Commit the invariant layer**

  ```bash
  rtk git add src/egl_renderer/effects/executor.rs src/egl_renderer/effects/resources.rs src/effects/render_graph.rs
  rtk git commit -m "fix(renderer): validate effect pass resource invariants"
  ```

### Task 3: Add the moving-domain real-GLES/resource regression

**Files:**
- Modify: `src/egl_renderer.rs`
- Modify: `src/egl_renderer/effects/resources.rs`
- Modify: `src/effects/render_graph.rs`
- Test: `src/egl_renderer.rs`

**Interfaces:**
- Reuse `GlesEffectTestHarness` and `EffectGlResourceCache`; do not introduce a mocked pool.
- Add a test-only moving blur scene builder that keeps effect ID, surface/window size, program, pool, and GLES context fixed while changing only `EffectRect.x/y`.

- [ ] **Step 1: Write the moving-domain test before changing behavior**

  Add a test that iterates through adjacent moves, large jumps, all four edge directions, legal partial clipping, reversals, and repeated positions for at least 512 compiled/executed/released frames. For each frame resolve the scene, compile the graph, plan demand, select passes, execute through the real GLES harness, and release the same renderer cache.

  Assert after each completed frame that no checked-out effect texture remains; assert at the end that current cache bytes and texture count are bounded, allocation count plateaus after the required key classes, reuse count rises, all translation-only dimensions remain unchanged, domains equal the current position, and invariant/FBO/scissor counters remain clean. Use representative pixel reads for orientation and sample placement. If client surfaces cannot be supplied by the harness, retain the real GLES graph prefix and separately assert the capture-domain model; state that this is not the NVIDIA freeze reproduction.

- [ ] **Step 2: Run the new test and verify RED for the intended gap**

  Run:

  ```bash
  rtk cargo test --locked egl_renderer::tests::moving_blur_domain_reuses_real_gles_resources -- --exact --nocapture
  ```

  Expected: the test fails because the moving-domain execution assertions or trace/resource counters are not yet implemented, not because the EGL harness cannot initialize. If the harness is unavailable, mark the real-GLES portion skipped by its existing platform convention and keep the deterministic model assertion failing.

- [ ] **Step 3: Implement only the test instrumentation needed for bounded measurements**

  Expose read-only test metrics for allocations, reuses, evictions, current bytes, cached textures, checked-out textures, physical dimensions, and alias/FBO/scissor validation outcomes. Keep production resource ownership unchanged.

- [ ] **Step 4: Run GREEN and check for unexpected allocation growth**

  Run the exact focused command from Step 2 and inspect the per-test summary. If allocation count or current bytes grows for translation-only movement, stop and classify that as a separate pool/lifetime defect before changing the pool. If counts plateau and the test passes, preserve the pool implementation.

- [ ] **Step 5: Commit the regression and measurements**

  ```bash
  rtk git add src/egl_renderer.rs src/egl_renderer/effects/resources.rs src/effects/render_graph.rs
  rtk git commit -m "test(renderer): cover moving blur domains in real GLES"
  ```

### Task 4: Add moving-effect transition damage coverage

**Files:**
- Modify: `src/effects/damage.rs`
- Modify: `src/egl_renderer/damage.rs`
- Modify: `src/egl_renderer/damage_tests.rs`
- Modify: `src/effects/render_graph.rs`

**Interfaces:**
- Reuse `effect_transition_damage`, `plan_effect_damage`, `compile_frame_execution_plan`, and `resolve_effect_execution_for_repaint_plan`.
- Add test helpers that model old domain A/new domain B and expose the current capture texture domain and repair region without changing production domain clipping.

- [ ] **Step 1: Write adjacent and large translation damage tests**

  Add tests for a same-identity blur moving by 1–2 pixels and by a large displacement without a surface-content commit. Assert old and new visible areas are in repair damage, old-only output is ordinary scene repair, new output is effect-composited, the capture domain equals the current region plus footprint clipped to output, and unrelated output is excluded.

- [ ] **Step 2: Run focused damage tests and verify RED**

  ```bash
  rtk cargo test --locked effects::damage
  rtk cargo test --locked egl_renderer::damage_tests
  ```

  Expected: the new assertions fail only where the existing test helpers do not expose/represent the current-domain transition contract.

- [ ] **Step 3: Implement the smallest damage/test helper correction required by the failure**

  Keep transition repair as the union of old/new visible areas and keep capture planning clipped to the current effect region. Do not force full-output capture or alter WaylandAuto identity semantics.

- [ ] **Step 4: Run GREEN and adjacent graph tests**

  ```bash
  rtk cargo test --locked effects::damage
  rtk cargo test --locked egl_renderer::damage_tests
  rtk cargo test --locked effects::render_graph
  ```

- [ ] **Step 5: Commit the transition coverage**

  ```bash
  rtk git add src/effects/damage.rs src/egl_renderer/damage.rs src/egl_renderer/damage_tests.rs src/effects/render_graph.rs
  rtk git commit -m "test(renderer): cover moving effect transition damage"
  ```

### Task 5: Close the logical/sample-coordinate contract tests

**Files:**
- Modify: `src/egl_renderer/effects/blur.rs`
- Modify: `src/egl_renderer/effects/executor.rs`
- Modify: `src/egl_renderer/effects/shader_cache.rs`
- Modify: `src/egl_renderer/program.rs`
- Modify: `src/egl_renderer.rs`

**Interfaces:**
- Preserve `u_effect_target_flip_y`, `u_effect_input_flip_y`, and `u_typhon_input_flip_y` as separate contracts.
- Cover SceneCapture replay, direct framebuffer capture/blit, first/later Kawase downsample, upsample, normalization, final composite, lifecycle capture, and shared helpers.

- [ ] **Step 1: Add failing contract tests for domain translation and orientation independence**

  Extend the pure coordinate tests and real GLES pbuffer tests to compare two equal-size blur regions at different x/y domains. Assert only logical uniforms and output placement change; physical dimensions, backing orientation, and flip decisions remain unchanged. Add direct framebuffer transfer tests for both `BottomLeft` and `TopLeftScanout` and lifecycle copy tests using the same mapping helpers.

- [ ] **Step 2: Run the coordinate suite and verify RED**

  ```bash
  rtk cargo test --locked egl_renderer::effects::blur
  rtk cargo test --locked egl_renderer::effects::executor::coordinate_tests
  rtk cargo test --locked egl_renderer::tests::real_gles
  ```

  Expected: any failure identifies a concrete contract mismatch or missing assertion; do not revert either historical coordinate commit wholesale.

- [ ] **Step 3: Document each pass contract in code and make the smallest correction**

  Add concise comments next to the shared vertex/sample/mapping helpers covering logical origin, physical storage origin, framebuffer origin, vertex UV orientation, input sample orientation, and output mapping. Correct only the contract proven by the failing test.

- [ ] **Step 4: Run all coordinate and lifecycle-adjacent tests**

  ```bash
  rtk cargo test --locked egl_renderer::effects
  rtk cargo test --locked egl_renderer::tests::real_gles_copy_preserves_logical_and_physical_orientation_contracts
  rtk cargo test --locked egl_renderer::tests::real_gles_composite_keeps_logical_domain_and_orientation
  rtk cargo test --locked egl_renderer::tests::real_gles_normalize_uses_shared_logical_uv_and_sample_conversion
  rtk cargo test --locked egl_renderer::tests::real_gles_trusted_wrapper_preserves_logical_context_uv
  ```

- [ ] **Step 5: Commit coordinate-contract coverage**

  ```bash
  rtk git add src/egl_renderer src/egl_renderer/effects
  rtk git commit -m "test(renderer): close moving effect coordinate contracts"
  ```

### Task 6: Qualify the original native reproduction with trace evidence

**Files:**
- Modify only if needed for trace wiring: `src/native_output/scanout/atomic_egl_gbm.rs`, `src/native_output/runtime/frame.rs`, `src/egl_renderer.rs`
- Do not modify: `/home/agony/GitHub/Eclipse`

**Interfaces:**
- Use the original Kitty configuration and the native Atomic EGL/GBM path with Direct Scanout off.
- Use `TYPHON_EFFECT_EXEC_TRACE=1`; leave hardware cursor, KMS worker, and triple-buffer policy unchanged.

- [ ] **Step 1: Build the current tree in place**

  ```bash
  rtk cargo check --locked --all-targets
  ```

- [ ] **Step 2: Run the original reproduction with tracing enabled**

  Start Typhon using the existing native launch procedure, capture stderr, enable Kitty background blur, and rapidly drag the same window. Record the final complete trace chain for the fatal frame, including the last pass begin without an end marker, capture mode, current domains, physical textures/FBOs, alias result, and resource metrics.

- [ ] **Step 3: Decide whether CPU markers localize the failure**

  If a pass has a begin marker with no end marker, compare it with the graph and invariant trace. If all CPU markers complete, do not infer GPU completion; proceed to the temporary prefix limiter in Task 7.

- [ ] **Step 4: Record the evidence answers**

  Answer which phase/pass stopped, replay versus blit capture, logical domains, physical IDs/FBO, alias state, bounded resource counts, dimension/topology changes, and whether capture/Kawase/composite/resource handling was implicated. Explicitly record that Eclipse was unchanged and whether disabling its shell effects leaves Kitty WaylandAuto blur reproducing.

### Task 7: Bisect an ambiguous asynchronous-GL failure, then apply one proven correction

**Files:**
- Modify temporarily: `src/egl_renderer/effects/executor.rs`
- Modify permanently only at the proven source: the exact renderer/effect file identified by Task 6
- Test: the narrow regression for the proven invariant/contract

**Interfaces:**
- `TYPHON_EFFECT_DEBUG_PASS_LIMIT=N` is temporary, opt-in, and submits only the first N selected passes before entering the existing effect fallback path.
- It must release all checked-out textures, restore ordinary framebuffer/GL state, and report fallback rather than success.

- [ ] **Step 1: Add a failing prefix-limiter behavior test if CPU markers are ambiguous**

  Test limits 0, 1, and a graph-sized limit using a deterministic graph, asserting resource release and fallback status. Do not use `glFinish()`.

- [ ] **Step 2: Run the limiter test and verify RED**

  ```bash
  rtk cargo test --locked egl_renderer::effects::executor::pass_prefix_limit
  ```

  Expected: it fails because the temporary debug control is not implemented.

- [ ] **Step 3: Implement the temporary limiter through existing fallback cleanup**

  Check the limit before each selected pass, return the established ordinary renderer error at the limit, and ensure the caller restores state and releases graph resources. Run native limits by binary search until the first additional submitted prefix associated with the hang is identified.

- [ ] **Step 4: Form one evidence-backed hypothesis**

  State exactly one hypothesis, such as physical input/output feedback, invalid FBO transfer, domain/scissor overflow, or a coordinate-state mismatch, and cite the trace/invariant/prefix evidence that supports it. If no hypothesis is proven, stop without a fix and retain the new trace for continued investigation.

- [ ] **Step 5: Write the narrow failing regression for that hypothesis**

  The regression must fail before the correction for the exact alias/lifetime/FBO/domain/orientation defect and must use the actual graph/resource machinery where applicable.

- [ ] **Step 6: Implement only the smallest correction**

  Preserve all listed features and existing fallback behavior. Remove the temporary pass limiter before the production commit unless the native qualification demonstrates a permanent, separately justified diagnostic need.

- [ ] **Step 7: Run focused GREEN and native A/B qualification**

  Run the new regression, adjacent renderer/effects/damage suites, then repeat the original Kitty blur rapid-drag test. Do not call the freeze fixed unless the original NVIDIA Atomic EGL/GBM reproduction remains stable for substantially longer than the previous reproduction window.

### Task 8: Repository-wide verification and final commit

**Files:**
- Modify only files already justified by Tasks 1–7
- Do not modify Eclipse

- [ ] **Step 1: Run the required verification commands in the Typhon directory**

  ```bash
  rtk run -- cargo fmt --check
  rtk run -- cargo check --locked --all-targets
  rtk run -- cargo clippy --locked --all-targets -- -D warnings
  rtk run -- cargo test --locked
  rtk git diff --check
  rtk run -- bash bin/check-source-layout
  ```

- [ ] **Step 2: Run focused suites separately**

  Run effect damage, render graph, resource pool, executor, real GLES renderer, Atomic EGL/GBM boundary, and lifecycle capture suites separately, recording exit status and failures. Do not repair unrelated pre-existing warnings.

- [ ] **Step 3: Review the final diff and source layout**

  Confirm no speculative workaround, no Eclipse change, no `glFinish()` in production blur execution, no full-output capture, no Kitty/NVIDIA special case, no KMS/cursor/scheduler changes, and no accidental revert of `e6715b5`/`917ced5`.

- [ ] **Step 4: Commit the evidence-backed correction and tests**

  ```bash
  rtk git add src docs/superpowers
  rtk git commit -m "fix(renderer): close moving blur execution correctness"
  ```

- [ ] **Step 5: Report the evidence boundary**

  Report the exact stopped phase/pass, capture mode, domains, physical resources/FBO, alias result, resource boundedness, topology/dimension behavior, historical-coordinate-contract result, Eclipse result, and native acceptance matrix. If hardware qualification is unavailable or still freezes, say so explicitly and report the first unproven boundary instead of claiming closure.

