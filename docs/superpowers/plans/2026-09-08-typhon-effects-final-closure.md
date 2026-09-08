# Typhon Effects Final Closure Implementation Plan

> **For agentic workers:** Execute this plan inline in the existing Typhon checkout. The user explicitly forbids sub-agents and requires the existing build directory.

**Goal:** Close the remaining Typhon effects-engine correctness and qualification gates while preserving the accepted renderer foundation and explicitly rejecting `StaticTexture` in trusted-effects v1.

**Architecture:** Add a single trusted-generation publication transaction, typed surface-slot ownership, shared semantic visual groups, dependency-aware region-local checkpoints for overlapping effects, explicit GL pass state, executable alpha/color contracts, complete trusted ABI plumbing, and factual diagnostics/documentation. Keep `LegacyScene` exact when no visible effects exist.

**Tech Stack:** Rust, Cargo, GLES via `glow`, Wayland server protocol dispatch, existing Typhon render graph/resource pool, existing `rtk` command wrapper.

## Global Constraints

- Continue the current Typhon effects implementation; do not restart or redesign the effects subsystem.
- Preserve `LegacyScene`, anchored final composition, Dual Kawase pyramid, region-local domains, target-local coordinates, liveness pooling, shared scratch FBO, transition damage, effect scheduling, shader prewarm, one-time blur decode, Direct Scanout blocking, and presentation/buffer ownership.
- Keep `StaticTexture` internal only as needed for IR compatibility; reject it in v1 validation and registry publication with a typed error before frame execution.
- Do not allocate a blank/fallback texture or silently degrade a `StaticTexture` source.
- Trusted config and shader assets must be Typhon-owned and root-constrained; arbitrary client paths, source, GL handles, and texture handles remain forbidden.
- Compile and test in the existing `target` directory; do not create another build directory.
- Use `rtk` for Cargo, git, search, and verification commands where available.
- Run the TDD red-green cycle for every behavior change: write a focused failing test, observe the expected failure, implement the smallest fix, then rerun focused and regression tests.
- Preserve unrelated existing worktree edits and stage only files belonging to this effects closure or its plan/spec documents.
- Do not claim production qualification unless all fresh deterministic gates pass and native qualification has real target evidence.

## File Structure

- `src/effects/validation.rs`: graph-level v1 capability rejection, including `StaticTexture`.
- `src/effects/registry.rs`, `src/effects/config.rs`: candidate generation, trusted reload errors, generation identity, trusted path checks.
- `src/compositor/effects.rs`, `src/compositor/mod.rs`, `src/compositor/state/surfaces.rs`: typed visual binding ownership, generation publication, semantic visual groups, invalidation.
- `src/compositor/protocols/effects_control.rs`: protocol slot conversion, binding/resource lifecycle, authorization, parameter ranges.
- `src/effects/render_graph.rs`: semantic capture/composite dependencies, alpha and working-space metadata, region/liveness tests.
- `src/egl_renderer/effects/executor.rs`, `capture.rs`: checkpoint scheduling, explicit pass state, local capture rectangles, frame ABI uploads.
- `src/egl_renderer/effects/shader_cache.rs`: bounded custom shader wrapper, auxiliary ABI, prewarm/lookup generation behavior.
- `src/egl_renderer/effects/resources.rs`: pool state/accounting and release-on-error behavior.
- `src/egl_renderer/effects/metrics.rs`, `src/egl_renderer.rs`: truthful metrics, generation diagnostics, frame timing/context propagation.
- `src/native_output/runtime/frame.rs`, `src/native/scheduler/pipeline.rs`: compositor-owned effect timing and demand propagation where the current seam requires it.
- `docs/EFFECTS.md`, `docs/EFFECTS_QUALIFICATION.md`: capability-state and qualification evidence.

---

### Task 1: Reject unsupported `StaticTexture` in v1 before publication

**Files:**
- Modify: `src/effects/validation.rs`
- Modify: `src/effects/registry.rs`
- Modify: `src/effects/render_graph.rs`
- Test: adjacent `#[cfg(test)]` modules in the three files

**Interfaces:** `validate_effect_program` remains the renderer-independent validation entry point. It returns a typed `EffectValidationError::UnsupportedStaticTexture(StaticTextureId)` for v1 publication. The existing render-graph `UnsupportedStaticTexture` error remains available for direct graph callers.

- [ ] **Step 1: Write the failing validation test.** Construct a one-source program whose source is `EffectSource::StaticTexture(StaticTextureId::new(7).unwrap())`, then assert `validate_effect_program(program)` returns the typed unsupported-source error.

```rust
#[test]
fn static_texture_is_rejected_before_frame_execution() {
    let static_id = StaticTextureId::new(7).unwrap();
    let source = EffectNode::source(
        EffectNodeId::new(1).unwrap(),
        EffectSource::StaticTexture(static_id),
    );
    let program = EffectProgram {
        id: EffectProgramId::new(77).unwrap(),
        nodes: vec![source],
        output: EffectNodeId::new(1).unwrap(),
        working_space: EffectWorkingSpace::OutputEncodedSrgb,
        alpha_mode: EffectAlphaMode::Preserve,
        outsets: EffectOutsets::ZERO,
        frame_demand: EffectFrameDemand::OnDamage,
        failure_policy: EffectFailurePolicy::Passthrough,
    };
    assert_eq!(
        validate_effect_program(program),
        Err(EffectValidationError::UnsupportedStaticTexture(static_id))
    );
}
```

- [ ] **Step 2: Run the focused test and verify it fails because the error variant is missing.**

Run: `rtk cargo test --locked static_texture_is_rejected_before_frame_execution`

Expected: compile failure identifying the missing typed validation result or an assertion mismatch; no renderer code is changed for this red step.

- [ ] **Step 3: Implement the minimal typed rejection.** Add the validation error variant and reject static source nodes before topological validation completes. Make `build_generation` propagate it through `RegistryReloadError::Config` so a trusted candidate cannot be published. Keep the IR enum and the existing render-graph error for future asset-table support.

- [ ] **Step 4: Add the registry publication rejection test and run the focused green tests.** Build an `EffectManifest` containing the static-source program, call `TrustedEffectRegistry::reload`, assert the typed error, and assert the current generation ID and program lookup remain unchanged.

Run: `rtk cargo test --locked static_texture`

Expected: all focused static-texture tests pass, with no GL/resource allocation path invoked.

---

### Task 2: Wire one atomic trusted registry publication transaction

**Files:**
- Modify: `src/effects/registry.rs`, `src/effects/config.rs`
- Modify: `src/egl_renderer.rs`, `src/egl_renderer/effects/shader_cache.rs`
- Modify: `src/compositor/mod.rs`, `src/compositor/effects.rs`, `src/compositor/state/surfaces.rs`
- Test: registry tests plus compositor/renderer model tests

**Interfaces:** Add a coordinator API equivalent to `publish_trusted_effect_generation(candidate, renderer_boundary)` returning `Result<Arc<EffectRegistryGeneration>, RegistryReloadError>`. The renderer boundary prewarms candidate programs before compositor visibility. Both sides expose the same `generation` value.

- [ ] **Step 1: Add failing model tests for initial publication, reload, failure retention, removal, stale lookup, and generation equality.** Use a fake renderer publication boundary that records candidate generation and can fail a selected module.

```rust
#[test]
fn failed_renderer_prewarms_retain_previous_generation_everywhere() {
    let runtime = test_publication_runtime_with_builtin_generation();
    let previous = runtime.generation();
    let candidate = manifest_with_shader_module(11);
    let result = runtime.reload(candidate, |asset| {
        Err(format!("compile failed for {}", asset.module.get()))
    });
    assert!(matches!(result, Err(RegistryReloadError::ShaderCompile { .. })));
    assert_eq!(runtime.compositor_generation(), previous);
    assert_eq!(runtime.renderer_generation(), previous);
    assert!(runtime.resolve_program("previous-name").is_some());
}
```

- [ ] **Step 2: Run the publication tests and observe failures showing that compositor and renderer are independent.**

Run: `rtk cargo test --locked registry_publication`

- [ ] **Step 3: Implement candidate-first publication.** Keep the old generation in both owners until validation and renderer prewarm succeed. Publish renderer GL programs and registry first inside the safe GL boundary, then publish the exact candidate `Arc`/generation to compositor lookup through a non-failing state update. On every success invalidate affected effect scene/presented-damage history once and expose generation/reload diagnostics. On every failure clear candidate GL programs and leave the old generation untouched.

- [ ] **Step 4: Separate prewarm from render lookup.** Keep frame execution on `lookup`; remove any custom-program compile-on-miss route from `execute_fullscreen_stage`. Ensure a candidate failure deletes all newly created programs without clearing the active cache.

- [ ] **Step 5: Run the focused publication suite and source checks.**

Run: `rtk cargo test --locked registry_publication`

Expected: initial publication, successful reload, compile failure retention, removal, stale binding rejection, and compositor/renderer generation equality all pass.

---

### Task 3: Implement typed surface slots and exact protocol binding ownership

**Files:**
- Modify: `src/compositor/protocols/effects_control.rs`
- Modify: `src/compositor/effects.rs`, `src/compositor/mod.rs`, `src/compositor/state/surfaces.rs`
- Modify: `protocols/astrea-effects-v1.xml` only if the current wire enum needs an explicit versioned error
- Test: protocol/compositor effects tests

**Interfaces:** Add `SurfaceEffectSlot::{Background, Content, Foreground}` and `SurfaceEffectBindingKey { surface_id, slot }`. Add binding identity to `AstreaSurfaceEffectData`. Surface mutation accepts a typed slot and binding identity, not a raw string or surface-only key.

- [ ] **Step 1: Add failing lifecycle tests.** Cover duplicate disabled same-slot creation, background+foreground coexistence, disabled ownership, enable/disable/re-enable, destroy disabled/active, stale resource protection, surface teardown releasing all keys, unauthorized create/mutate, and slot-to-anchor mapping.

- [ ] **Step 2: Run the lifecycle tests and verify they fail against surface-only storage and ignored slot strings.**

Run: `rtk cargo test --locked surface_effect`

- [ ] **Step 3: Parse the slot at the protocol boundary.** Reject empty/control/unknown surface slots with a typed protocol error; map the three supported values to the typed enum. Reject `output` on a surface binding instead of approximating it.

- [ ] **Step 4: Store ownership separately from visual state.** Claim `(surface_id, slot)` during `GetSurfaceEffect` before the resource is initialized. Keep the key claimed while disabled. Enable/disable updates only the binding's instance. Destroy and `destroyed` verify the binding identity before removing its visual instance and key. Surface destruction drains every key owned by that surface exactly once.

- [ ] **Step 5: Run focused tests and check no legacy surface-only mutation remains.**

Run: `rtk cargo test --locked surface_effect`
Run: `rtk rg -n 'internal_surface_effects\.contains_key|clear_internal_surface_effect\(data\.surface_id\)' src/compositor`

Expected: lifecycle tests pass and remaining matches are only intentional internal qualification APIs with typed ownership wrappers.

---

### Task 4: Add semantic visual-root/group identity

**Files:**
- Modify: `src/compositor/render.rs`, `src/native_output/runtime/frame.rs`
- Modify: `src/egl_renderer.rs`, `src/egl_renderer/effects/capture.rs`, `src/egl_renderer/effects/executor.rs`
- Modify: `src/compositor/effects.rs`, `src/effects/render_graph.rs`
- Test: compositor/render/effects tests

**Interfaces:** Add a stable frame-local `VisualGroupId` and a scene group record describing root surface, subsurface descendants, decoration membership, and popup-root policy. Capture commands and resolved effect instances carry the same group identity.

- [ ] **Step 1: Add failing group tests with root + subsurface + decoration + popup.** Assert the documented policy: root/subsurface/decoration are one target group; popup remains a separate visual root unless explicitly included. Assert `TargetContent`, `ReplaceSurface`, `BeforeSurface`, and `AfterSurface` select the same group boundaries.

- [ ] **Step 2: Run the group tests and observe raw-surface-ID behavior failing for child content or popup separation.**

Run: `rtk cargo test --locked visual_group`

- [ ] **Step 3: Implement group identity at scene lowering.** Resolve committed synchronized state before frame freeze, assign group IDs once, and carry them into `ResolvedNativeFrameScene`/EGL commands. Keep popup inclusion explicit in the source category rather than silently inheriting the parent.

- [ ] **Step 4: Make capture selection and final composition consume group IDs.** Replace one-command `position + 1` replacement with group span boundaries and return a typed unsupported/fallback result if a group cannot be resolved safely.

- [ ] **Step 5: Run focused group/renderer tests.**

Run: `rtk cargo test --locked visual_group`
Run: `rtk cargo test --locked effects`

---

### Task 5: Schedule overlapping backdrop dependencies through region-local checkpoints

**Files:**
- Modify: `src/effects/render_graph.rs`
- Modify: `src/egl_renderer/effects/executor.rs`, `src/egl_renderer/effects/capture.rs`, `src/egl_renderer/effects/resources.rs`
- Test: render-graph/executor model tests

**Interfaces:** Extend compiled passes with explicit checkpoint/dependency metadata and region-local domains. Higher `SceneCapture` passes consume the resolved lower-scene checkpoint rather than the raw command list alone.

- [ ] **Step 1: Add failing graph tests for two and three overlapping backdrop stacks.** Assert lower final composites precede higher captures, the higher capture includes the lower effect result, dependency edges are explicit, and no pass reads its output texture.

- [ ] **Step 2: Run the tests and capture the current failure showing all offscreen passes are scheduled before final composites.**

Run: `rtk cargo test --locked overlapping_backdrop`

- [ ] **Step 3: Compile z-ordered checkpoints.** Partition effects by semantic anchor, insert lower composite nodes into the dependency plan, and crop each checkpoint to the higher capture region plus required padding. Preserve graph first/last-use metadata and independent damage rectangles.

- [ ] **Step 4: Execute checkpoints without feedback.** Before a higher capture, bind a distinct region-local checkpoint target, render only the required lower scene/effect ranges, then sample that checkpoint as the capture source. Release it at its last dependent use. Do not copy the full output for each effect.

- [ ] **Step 5: Add damage propagation tests for moving/removing a lower effect.** The higher dependent effect must receive source/capture/output damage while unrelated regions remain clean.

- [ ] **Step 6: Run focused graph/executor/resource tests.**

Run: `rtk cargo test --locked overlapping_backdrop`
Run: `rtk cargo test --locked effects`

---

### Task 6: Establish explicit GL state and replace-write semantics

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs`
- Modify: `src/egl_renderer/effects/resources.rs`, `src/egl_renderer.rs`
- Test: executor/state-model tests

**Interfaces:** Add a scoped effect pass-state guard or equivalent structured restore helper. Each pass declares target, viewport, scissor, blend/write mode, program, texture units, working space, and alpha mode.

- [ ] **Step 1: Add a failing pooled-texture reuse regression test.** Seed a destination model with nonzero premultiplied RGB/alpha, run a translucent ordinary offscreen stage, and assert the result depends only on stage input, not the old destination.

- [ ] **Step 2: Run the test and observe stale-destination blending under the current globally enabled source-over state.**

Run: `rtk cargo test --locked pooled_destination_is_independent`

- [ ] **Step 3: Implement explicit pass setup.** Disable blending for capture and ordinary stage replacement writes, establish scissor/viewport per target, bind only validated textures, and use source-over only for final composition or explicit framebuffer blend nodes.

- [ ] **Step 4: Restore state on all exits.** Ensure error paths restore output framebuffer, viewport, scissor, blend, program, active texture, and texture bindings before returning to legacy scene rendering.

- [ ] **Step 5: Run focused executor/state tests.**

Run: `rtk cargo test --locked pooled_destination_is_independent`
Run: `rtk cargo test --locked egl_renderer`

---

### Task 7: Enforce premultiplied alpha and real mask/blend math

**Files:**
- Modify: `src/effects/model.rs`, `src/effects/render_graph.rs`
- Modify: `src/egl_renderer/effects/executor.rs`, `src/egl_renderer/effects/shader_cache.rs`
- Test: pure effect math/render-graph tests and shader-source contract tests

**Interfaces:** Carry `EffectAlphaMode` into final composite metadata. Add tested pure premultiplied helpers for mask and blend reference behavior; GLSL uses the same contract.

- [ ] **Step 1: Add failing tests for opaque/preserve output, transparent/opaque/fractional/inverted mask, saturated colored translucent pixels, and all four blend modes.** Include alpha-near-zero inputs and assert finite output.

- [ ] **Step 2: Run the math tests and confirm mask currently changes alpha without correcting premultiplied RGB.**

Run: `rtk cargo test --locked premultiplied`

- [ ] **Step 3: Implement `Opaque` alpha forcing only at the final effect boundary.** Preserve intermediate alpha until final composition; force output alpha to one for opaque programs.

- [ ] **Step 4: Correct mask math.** For coverage masks, multiply both premultiplied RGB and alpha; for inverted alpha remapping, safely unpremultiply nonzero input, apply the new alpha, and premultiply again. Clamp near-zero paths without NaN/Inf.

- [ ] **Step 5: Define and implement premultiplied `SourceOver`, `Add`, `Multiply`, and `Screen` behavior, using safe straight-color conversion only where required by the selected definition.**

- [ ] **Step 6: Run focused alpha/shader tests.**

Run: `rtk cargo test --locked premultiplied`
Run: `rtk cargo test --locked mask`

---

### Task 8: Make working-space transitions executable and exact

**Files:**
- Modify: `src/effects/render_graph.rs`, `src/effects/validation.rs`
- Modify: `src/egl_renderer/effects/executor.rs`, `src/egl_renderer/effects/shader_cache.rs`
- Test: render-graph/color tests

**Interfaces:** Compiled textures and passes carry `EffectWorkingSpace`; conversion is an explicit pass or a validated final-boundary decision.

- [ ] **Step 1: Add failing tests for encoded source-only backdrop, encoded TargetContent source-only, linear blur, local color stage, custom linear stage, and illegal mismatched graph spaces.**

- [ ] **Step 2: Run the tests and observe source-only double-encode or repeated-decode behavior.**

Run: `rtk cargo test --locked working_space`

- [ ] **Step 3: Add explicit decode/encode boundaries.** Decode encoded input once when first entering linear math, keep later blur/stage inputs linear, and encode exactly once when a linear result reaches encoded output. An encoded source-only result bypasses encode.

- [ ] **Step 4: Validate custom program declarations.** Reject impossible stage/output spaces before registry publication or insert a conversion node that is represented in the graph and resource liveness.

- [ ] **Step 5: Run focused color tests and verify blur shader variants are selected by input working space rather than pass number alone.**

Run: `rtk cargo test --locked working_space`
Run: `rtk cargo test --locked effects`

---

### Task 9: Preserve disjoint capture regions and target-local origins

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs`, `src/egl_renderer/effects/capture.rs`
- Modify: `src/effects/render_graph.rs`
- Test: capture-coordinate tests

**Interfaces:** Add target-local conversion from output-space `EffectRegion` to physical GL rectangles and make capture draw/scissor accept the rectangle list.

- [ ] **Step 1: Add failing tests for two disjoint rectangles with a hole, nonzero capture domain, 1/2 and 1/4 scale, each output edge, and BottomLeft offscreen target under top-left final output.**

- [ ] **Step 2: Run capture tests and observe the current `bounding_rect()` hole repaint.**

Run: `rtk cargo test --locked capture_region`

- [ ] **Step 3: Implement one conversion function.** Intersect each output rect with the texture's output-space domain, translate to local coordinates, scale with checked arithmetic, and convert to the offscreen target's GL origin. Return a conservative full-target rectangle only for the explicit conservative-full region state.

- [ ] **Step 4: Iterate converted rectangles in capture and stage scissor paths.** Keep clear/write behavior deterministic and do not use final framebuffer height for offscreen origin conversion.

- [ ] **Step 5: Run focused capture tests.**

Run: `rtk cargo test --locked capture_region`
Run: `rtk cargo test --locked egl_renderer`

---

### Task 10: Complete trusted custom shader time, auxiliary inputs, and parameter ranges

**Files:**
- Modify: `src/egl_renderer/effects/shader_cache.rs`, `src/egl_renderer/effects/executor.rs`
- Modify: `src/compositor/protocols/effects_control.rs`, `src/effects/config.rs`, `src/effects/model.rs`
- Modify: `src/egl_renderer.rs`, `src/native_output/runtime/frame.rs`
- Test: shader wrapper, protocol range, frame-time, and registry tests

**Interfaces:** Extend the resolved frame context with monotonic `time` and clamped `delta`; expose fixed-size `u_typhon_aux[]` and `u_typhon_aux_count`; validate deterministic auxiliary count/order; apply float ranges component-wise to vectors.

- [ ] **Step 1: Add failing wrapper tests for auxiliary declarations, reserved-name rejection, deterministic binding order/count, and missing/extra input rejection.**

- [ ] **Step 2: Add failing timing tests for monotonic time, delta after a stall capped at the chosen safe maximum, and no frame demand for static effects.**

- [ ] **Step 3: Add failing parameter tests for Vec2/Vec3/Vec4 component-wise float ranges and explicit v1 handling for unsupported mutable wire types.**

- [ ] **Step 4: Run focused tests and observe the current zero time/delta, primary-only wrapper, and scalar-only range behavior.**

Run: `rtk cargo test --locked shader_wrapper`
Run: `rtk cargo test --locked parameter_range`
Run: `rtk cargo test --locked continuous_effect`

- [ ] **Step 5: Implement the bounded ABI and frame context.** Upload compositor monotonic values only for executable custom stages, bind auxiliary textures in validated order within the fixed unit budget, and reject any count mismatch before publication. Clamp delta after stalls/suspend without converting static effects into continuous demand.

- [ ] **Step 6: Run focused ABI/timing/range tests.**

Run: `rtk cargo test --locked shader_wrapper`
Run: `rtk cargo test --locked parameter_range`
Run: `rtk cargo test --locked continuous_effect`

---

### Task 11: Make diagnostics truthful and documentation factual

**Files:**
- Modify: `src/egl_renderer/effects/metrics.rs`, `src/egl_renderer.rs`, `src/egl_renderer/effects/resources.rs`, `src/egl_renderer/effects/shader_cache.rs`
- Modify: `src/native_output/scanout/backend.rs` only if an existing effect field needs corrected propagation
- Modify: `docs/EFFECTS.md`, `docs/EFFECTS_QUALIFICATION.md`
- Test: metrics/diagnostic tests and documentation checks

- [ ] **Step 1: Add failing diagnostics tests.** Assert cache hits increment only on cache reuse, output pixels equal effect influence/output work rather than full output texture area, capture pixels equal planned capture domains, pool peak/current/budget remain bounded, and generation/fallback/Direct Scanout/continuous demand are observable.

- [ ] **Step 2: Run the diagnostics tests and record current counter mismatches.**

Run: `rtk cargo test --locked effect_metrics`

- [ ] **Step 3: Update counter sources.** Increment cache hits at the actual reuse path, compute effect output pixels from planned effect regions, preserve capture-domain accounting, expose generation/reload and typed fallback fields, and report GPU timing as `UNAVAILABLE` without synchronous waits when timer queries are unavailable.

- [ ] **Step 4: Update documentation only to match proven state.** Mark modeled/implemented/deterministically tested/hardware-qualified/production-default separately. State `StaticTexture` is unsupported in trusted-effects v1 and that hardware qualification is pending unless the native matrix produces evidence.

- [ ] **Step 5: Run focused diagnostics and documentation scans.**

Run: `rtk cargo test --locked effect_metrics`
Run: `rtk rg -n 'StaticTexture|hardware-qualified|production-default|UNAVAILABLE' docs/EFFECTS.md docs/EFFECTS_QUALIFICATION.md`

---

### Task 12: Fresh deterministic gates and native qualification

**Files:**
- No production file changes are authorized by this task unless a verification failure identifies a root cause; fixes return to the owning task above.
- Evidence: existing `target` build output, command logs, `docs/EFFECTS_QUALIFICATION.md`

- [ ] **Step 1: Run formatting and inspect the full result.**

Run: `rtk cargo fmt --all -- --check`

- [ ] **Step 2: Run locked compilation and clippy with warnings denied.**

Run: `rtk cargo check --locked --all-targets`
Run: `rtk cargo clippy --locked --all-targets -- -D warnings`

- [ ] **Step 3: Run the required focused and full suites.**

Run: `rtk cargo test --locked effects`
Run: `rtk cargo test --locked egl_renderer`
Run: `rtk cargo test --locked native_output`
Run: `rtk cargo test --locked`

- [ ] **Step 4: Run the presentation dry run.**

Run: `rtk bin/qualify-presentation --dry-run`

- [ ] **Step 5: If and only if all deterministic commands exit 0, inspect native capability and run the documented 1920x1080@165 matrix on the RTX 3060 Ti target.** Record CPU p50/p95/p99, non-stalling GPU timing or `UNAVAILABLE`, missed-vblank/target-slip, effect passes/draws/binds, capture/output pixels, resource peak/budget/reuse/eviction, fallbacks, and Direct Scanout block/recovery across all required scenarios.

- [ ] **Step 6: Update qualification documentation with actual results or an explicit environment blocker.** If no real TTY/DRM/NVIDIA run is possible, state deterministic verification status and stop short of production-qualified claims.

## Execution order and checkpoints

Execute Tasks 1–3 first because registry publication and protocol ownership define the trusted surface. Execute Tasks 4–9 in order because visual grouping and dependency scheduling determine capture correctness before GL/state and color results can be trusted. Execute Task 10 only after the publication path and source capabilities are bounded. Execute Task 11 after behavior is proven. Execute Task 12 last.

At each task boundary, inspect `git diff --check`, run the focused tests named in the task, and confirm unrelated existing edits remain present. If a test fails, perform root-cause investigation before changing code; do not stack speculative fixes.

## Plan self-review

- Every finishing-prompt gate maps to at least one task: registry (2), slots/ownership (3), visual groups (4), overlap dependencies (5), GL state (6), alpha (7), color (8), capture regions (9), ABI/static rejection (1 and 10), diagnostics/docs (11), deterministic/native qualification (12).
- The accepted renderer foundation is explicitly preserved in the global constraints and Task 12 regression scope.
- `StaticTexture` is rejected before registry publication, never allocated as a blank texture, tested, and documented.
- Each behavior-changing task has a red test step before implementation and a green focused verification step.
- No task uses placeholder wording or defers a required v1 decision.
