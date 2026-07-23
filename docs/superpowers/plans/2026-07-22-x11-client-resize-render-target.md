# X11 Client Resize Render Target Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render unsynchronized client-driven X11 resize at the latest requested size instead of repeatedly exposing old committed extents.

**Architecture:** Add an optional compositor-only destination size to `RenderableSurface`. Set it from ordinary X11 client configure handling and consume it only when deriving render targets, leaving committed content size and existing interactive-resize clipping unchanged.

**Tech Stack:** Rust, X11/Xwayland, Typhon software/GLES render scene, native damage tracking, Cargo tests

## Global Constraints

- `RenderableSurface::width` and `height` continue to describe committed content only.
- Native Wayland and compositor-driven resize behavior must not change.
- X11 client resize uses full source UV and latest constrained requested destination size.
- Existing dirty-worktree changes must be preserved.
- No commit is created unless requested by the user.

---

### Task 1: Model and render the X11 destination override

**Files:**
- Modify: `src/compositor/surface.rs`
- Modify: `src/compositor/render.rs`
- Test: `src/compositor/render.rs`

**Interfaces:**
- Produces: `RenderableSurface::render_target_size: Option<BufferSize>`
- Consumes: `surface_render_space_assignments(&[RenderableSurface], f64)`

- [ ] **Step 1: Write a failing renderer test**

Construct a 640x480 surface with `render_target_size = Some(BufferSize { width: 620, height: 460 })`. Assert that `render_scene_elements_for_surfaces` produces a 620x460 target with `SurfaceUvRect::FULL`, while the element buffer size remains 640x480.

- [ ] **Step 2: Run the renderer test and verify RED**

Run `cargo test x11_render_target_size_scales_committed_content_with_full_uv -- --nocapture`.

Expected: compilation or assertion failure because the destination override does not exist.

- [ ] **Step 3: Add and consume the destination override**

Add `render_target_size: Option<BufferSize>` to `RenderableSurface`. In `surface_render_space_assignments`, choose its width and height when present; otherwise use committed `surface.width` and `surface.height`. Initialize the field to `None` at every production and test constructor.

- [ ] **Step 4: Verify renderer GREEN**

Run `cargo test x11_render_target_size_scales_committed_content_with_full_uv -- --nocapture`.

Expected: one passing test.

### Task 2: Update target size from X11 client geometry

**Files:**
- Modify: `src/compositor/state/desktop_windows.rs`
- Test: `src/compositor/state/desktop_window_tests.rs`
- Test: `src/compositor/tests/xwayland.rs`

**Interfaces:**
- Consumes: `CompositorState::set_x11_geometry(X11WindowHandle, X11Geometry)`
- Produces: immediate root-surface `render_target_size` updates for normal managed X11 configure requests

- [ ] **Step 1: Write a failing state regression**

Admit a normal managed X11 window with committed content, apply geometry width 620 and height 460, then assert committed dimensions are unchanged and `render_target_size` equals 620x460.

- [ ] **Step 2: Run the state regression and verify RED**

Run `cargo test x11_client_geometry_updates_render_target_without_mutating_committed_size -- --nocapture`.

Expected: failure because `set_x11_geometry` does not set a destination override.

- [ ] **Step 3: Apply the target update**

In `set_x11_geometry`, update only the root renderable surface belonging to a normal compositor-managed X11 window. Store the constrained width and height as `render_target_size`, retain placement handling, and advance render generation as `WindowResize` when size changes or `WindowMove` when only position changes.

- [ ] **Step 4: Verify state GREEN and configure integration**

Run both named X11 resize regressions and `cargo test compositor::tests::xwayland`.

Expected: all pass.

### Task 3: Verify damage and full repository behavior

**Files:**
- Test: `src/native_output/tests/output.rs`

**Interfaces:**
- Consumes: `native_output_damage_for_repaint`
- Produces: regression coverage for old/new scaled destination bounds

- [ ] **Step 1: Write a failing damage regression before Task 1 implementation if target changes are not already covered**

Compare the same surface with destination 640x480 and 620x460. Assert output damage covers the union of old and new render targets.

- [ ] **Step 2: Run the damage regression**

Run `cargo test native_output_damage_for_x11_render_target_resize_covers_old_and_new_bounds -- --nocapture`.

Expected before target support: compilation or assertion failure. Expected after Task 1: pass without extra production changes.

- [ ] **Step 3: Run repository verification**

Run `cargo fmt --check`, `cargo test`, `cargo check --all-targets`, and `cargo clippy --all-targets -- -D warnings`.

Expected: zero failures and zero warnings.

- [ ] **Step 4: Build the live release binary**

Run `cargo build --release --bin oblivion-one`.

Expected: successful optimized build at `target/release/oblivion-one`.
