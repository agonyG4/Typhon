# Explicit Atomic Framebuffer Origin Implementation Plan

> **For agentic workers:** Execute this plan inline in the current session. Do not dispatch subagents.

**Goal:** Make the EGL renderer express whether its framebuffer is a legacy bottom-left EGL surface or a top-left direct-scanout GBM BO, keeping geometry, partial repaint scissors, and cache state consistent.

**Architecture:** Add a small `OutputFramebufferOrigin` value type in `src/egl_renderer.rs`. The legacy EGL window-surface entry point supplies `BottomLeft`; `EglOutputRenderTarget` supplies `TopLeftScanout` for Atomic EGLImage/FBO rendering. Geometry emission, the scene-command cache key, overlay commands, and `RepaintPlan::render_execution` all receive the same value. Damage remains top-left logical output damage; only its GL scissor conversion depends on the render target.

**Tech Stack:** Rust, GLES/EGL through `glow`, unit tests in renderer modules, Atomic KMS/GBM scanout.

## Global Constraints

- Preserve the legacy EGL window-surface orientation.
- Use `TopLeftScanout` only for the explicit Atomic EGLImage/FBO path.
- Preserve UV semantics, input coordinates, pointer deltas, viewport, scale, transforms, subsurfaces, frame ownership, damage ownership, and buffer-age 2/3 repair.
- Do not use KMS rotation/reflection, rotate the primary plane, disable the hardware cursor, or add a post-render flip copy.
- Preserve the zero-copy EGLImage/FBO → Atomic KMS path.
- Do not commit until the native Atomic `off` orientation validation succeeds.

---

## Files and responsibilities

- Modify `src/egl_renderer.rs`: define the orientation, add it to `EglOutputRenderTarget`, pass it through both render entry points, include it in cached scene identity, and pass it to scene/overlay geometry and repaint execution.
- Modify `src/egl_renderer/geometry.rs`: make quad NDC Y mapping orientation-aware while leaving UV coordinates unchanged; add deterministic top, bottom, and fullscreen tests.
- Modify `src/egl_renderer/damage.rs`: make GL scissor conversion and `RepaintPlan::render_execution` orientation-aware; retain bottom-left EGL swap-damage conversion; add top/bottom tests for both origins.
- Modify `src/native_output/scanout/atomic_egl_gbm.rs`: construct the render target with `TopLeftScanout`.
- Do not modify native input, pointer deltas, KMS plane transforms, cursor-plane code, or the unrelated existing working-tree edits.

## Task 1: Add failing orientation tests

**Files:** `src/egl_renderer/geometry.rs`, `src/egl_renderer/damage.rs`

**Interfaces:** Tests will exercise `OutputFramebufferOrigin::{BottomLeft, TopLeftScanout}` and the orientation-aware conversion signatures planned below.

- [x] **Step 1: Add geometry tests before implementation.** Add tests that call the quad helper with a rectangle at `(0, 0, 20, 10)` and `(0, 90, 20, 10)` in a `20x100` output, asserting the legacy and scanout NDC Y coordinates. Add a fullscreen test asserting legacy vertices use top `+1`/bottom `-1`, scanout vertices use top `-1`/bottom `+1`, and both retain `uv.top == 0.0` at the logical top.
- [x] **Step 2: Add damage tests before implementation.** Add tests for logical top `(4, 0, 9, 11)` and logical bottom `(4, 69, 9, 11)` in a `100x80` output. Assert `BottomLeft` scissors are `[4, 69, 9, 11]` and `[4, 0, 9, 11]`; assert `TopLeftScanout` scissors are `[4, 0, 9, 11]` and `[4, 69, 9, 11]`. Add a `RepaintPlan::render_execution` assertion proving the orientation reaches the scissored execution.
- [x] **Step 3: Run only the new tests and confirm RED.** Run `cargo test egl_renderer::geometry --bin oblivion-one -- --test-threads=1` and the targeted damage tests with `--test-threads=1`. The test compilation failed because the orientation type/signatures did not yet exist; no production change was made until that failure was observed.

## Task 2: Add the orientation contract and Atomic target selection

**Files:** `src/egl_renderer.rs`, `src/native_output/scanout/atomic_egl_gbm.rs`

**Interfaces:**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputFramebufferOrigin {
    BottomLeft,
    TopLeftScanout,
}

pub(crate) struct EglOutputRenderTarget {
    pub(crate) framebuffer: glow::Framebuffer,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) buffer_age: BufferAge,
    pub(crate) framebuffer_origin: OutputFramebufferOrigin,
}
```

- [x] **Step 1: Define the orientation enum and store it on `EglOutputRenderTarget`.** Derive the small value-type traits and place the field alongside framebuffer dimensions and age.
- [x] **Step 2: Make the legacy entry point select `BottomLeft`.** `draw_scene` calls the internal render function with `OutputFramebufferOrigin::BottomLeft`.
- [x] **Step 3: Make `draw_scene_to_target` pass `target.framebuffer_origin`.** Keep framebuffer binding, viewport setup, buffer-age selection, and unbinding unchanged.
- [x] **Step 4: Set the explicit Atomic target to `TopLeftScanout`.** Do not alter framebuffer import, GBM allocation, EGLImage attachment, or fence/pageflip ownership.
- [x] **Step 5: Run `cargo test egl_renderer --bin oblivion-one -- --test-threads=1`.** The renderer test target compiled and passed after the contract was threaded through.

## Task 3: Propagate orientation through cached geometry

**Files:** `src/egl_renderer.rs`, `src/egl_renderer/geometry.rs`

**Interfaces:**

```rust
fn push_textured_quad(
    vertices: &mut Vec<EglTexturedVertex>,
    rect: EglRect,
    uv: EglUvRect,
    output_width: u32,
    output_height: u32,
    framebuffer_origin: OutputFramebufferOrigin,
)
```

- [x] **Step 1: Add the orientation parameter to both draw-command helpers and `push_textured_quad`.** Pass it unchanged through every helper call.
- [x] **Step 2: Implement only the Y mapping change.** Use the existing bottom-left equations for `BottomLeft`; use `rect.y / output_height * 2.0 - 1.0` and `(rect.y + rect.height) / output_height * 2.0 - 1.0` for `TopLeftScanout`. Keep X mapping, triangle order, and every UV assignment unchanged.
- [x] **Step 3: Include the orientation in `EglSceneCacheKey`.** Add it to `new` and `is_current`, and use it for both candidate/presented scene identity and command-cache invalidation. A target-origin change must not reuse vertices built for another origin.
- [x] **Step 4: Pass the origin through `rebuild_scene_commands`, `rebuild_overlay_commands`, and `push_egl_surface_commands`.** This covers wallpaper, normal surfaces, server-side decorations, external overlays, and software/client cursor surfaces with one target contract.
- [x] **Step 5: Run the geometry tests and the existing scene-cache tests.** Both origin mappings, UV preservation, and legacy cache behavior pass.

## Task 4: Propagate orientation through damage/scissor execution

**Files:** `src/egl_renderer/damage.rs`, `src/egl_renderer.rs`

**Interfaces:**

```rust
pub(crate) fn to_gl_scissors(
    &self,
    output_width: u32,
    output_height: u32,
    framebuffer_origin: OutputFramebufferOrigin,
) -> Option<Vec<[i32; 4]>>

pub(crate) fn render_execution(
    &self,
    output_width: u32,
    output_height: u32,
    framebuffer_origin: OutputFramebufferOrigin,
) -> Option<RenderExecution>
```

- [x] **Step 1: Generalize the internal damage conversion.** `BottomLeft` computes `output_height - rect.y - rect.height`; `TopLeftScanout` uses `rect.y`. Keep clipping, half-open rectangles, full damage, ordering, and checked conversions unchanged.
- [x] **Step 2: Keep `to_egl_rects` on the legacy bottom-left conversion.** EGL window-surface swap damage remains unchanged.
- [x] **Step 3: Pass the origin from `draw_scene_with_buffer_age` into `draw_textured_layers`, then into `RepaintPlan::render_execution`.** No input or logical damage coordinate is transformed elsewhere.
- [x] **Step 4: Update existing legacy tests to pass `BottomLeft` explicitly where the signature changed, preserving their expected values.** Add explicit scanout assertions for top and bottom partial damage.
- [x] **Step 5: Run focused damage tests with `--test-threads=1`.** Legacy and scanout scissor mappings pass, including full damage and skip behavior.

## Checkpoint: Renderer correctness

- [x] `cargo fmt --check`
- [x] `cargo test egl_renderer --bin oblivion-one -- --test-threads=1`
- [x] `cargo check --all-targets`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `git diff --check`

## Task 5: Full verification and native validation

**Files:** No additional source files; review the complete diff for scope.

- [x] Run the complete test suite with `--test-threads=1`: 706 library tests, 270 binary tests, 3 native-only integration tests, and 10 launcher integration tests passed.
- [x] Run `./bin/check-source-layout`.
- [x] Run `cargo build --release`.
- [x] Inspect the diff to verify no input, pointer-delta, KMS rotation/reflection, cursor-plane, post-render-copy, or ownership changes were introduced by this fix.
- [ ] Repeat the native Atomic `off` test with hardware cursor and verify upright wallpaper/windows, upright cursor, upward movement/dragging, correct click/resize edges, no partial-damage artifacts, no stale regions, and correct fullscreen return. Blocked here: this environment has no `/dev/dri` and is not a TTY.
- [ ] Repeat briefly with `OBLIVION_ONE_CURSOR=software` and verify the software cursor matches hardware cursor logical positioning and orientation. Blocked by the same missing native DRM session.
- [ ] Only after native validation succeeds, present the final diff and ask whether to commit; until then leave all changes uncommitted.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Cached vertices are reused across target origins | One path can remain vertically flipped after switching targets | Include origin in both scene cache identity and command-cache validity. |
| Geometry is fixed but scissor conversion is not | Partial repaint repairs the mirrored region | Test top/bottom damage and pass origin into `render_execution`. |
| UVs are changed while fixing positions | Surface content becomes vertically inverted | Keep UV fields and triangle order byte-for-byte equivalent. |
| Existing dirty native changes are overwritten | User work is lost | Inspect and preserve the current diff; only add the target-origin changes. |
