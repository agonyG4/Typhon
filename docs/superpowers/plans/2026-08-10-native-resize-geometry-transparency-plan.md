# Native Resize Geometry and Transparency Regression Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stabilize native XDG CSD resize geometry and transparency while retaining only confirmed XWayland opaque backing behavior.

**Architecture:** Keep `surface_placements`/`RenderableSurface::placement` as compositor-owned frame placement and `surface_window_geometries` as committed XDG metadata. Centralize the derived root render placement calculation and invoke it after first renderable publication, committed geometry changes, and compositor placement changes. Add explicit `SurfaceRenderBackend` identity to renderables so native Wayland cannot enter the XWayland backing predicate; reuse that predicate for CPU, GLES, server-frame inspection, and damage planning.

**Tech Stack:** Rust, Smithay/Wayland protocol state, CPU compositor renderer, native EGL/GLES renderer, deterministic Rust tests.

## Global Constraints

- Work directly on `main`; do not create branches or worktrees.
- Preserve the approved design and recent native geometry/focus/stacking invariants.
- Do not change KMS, pageflip, triple buffering, direct scanout, explicit-sync ownership, XWayland lifecycle, tiling, or global alpha policy.
- Do not advertise or implement fake server-side decorations.
- Do not add application-specific hacks or first-resize flags.
- Native Wayland apertures must never produce opaque synthetic backing.
- Keep the XWayland backing only if focused tests demonstrate the current stale opaque X11 compatibility behavior requires it.
- Run focused tests during implementation and all repository validation before the final commit.

## File Map

- Modify `src/compositor/surface.rs`: define explicit render-backend identity on `RenderableSurface` and default native publication.
- Modify `src/compositor/mod.rs`: re-export the identity type used by renderer/state tests.
- Modify `src/compositor/state_data.rs`: mark generic renderable construction as native and provide the XWayland marking seam.
- Modify `src/compositor/state/xwayland_windows.rs`: mark confirmed XWayland renderables on both initial adoption and normal publication.
- Modify `src/compositor/state/window_resize.rs`: add the single pure frame-to-render placement derivation helper and tests.
- Modify `src/compositor/state/surface_commits.rs`: initialize the derived placement after the first native root renderable is inserted and on later native commits.
- Modify `src/compositor/render.rs`: require explicit XWayland identity, use one backing predicate for server-frame decisions, add native transparency/CSD and backend-parity tests, and update fixtures.
- Modify `src/egl_renderer.rs`: call the explicit-identity backing predicate without changing render order or alpha behavior.
- Modify `src/compositor/state/desktop_windows.rs`: make the existing test helper visible within the compositor crate so the current suite compiles.
- Modify `src/compositor/state/task_05_8_tests.rs` and `src/compositor/tests/xwayland_resize_visual.rs`: add first-renderable/resize lifecycle and identity assertions where existing fixtures already model those flows.
- Modify `src/native_output/tests/output.rs` only if a focused XWayland/native damage assertion needs its fixture identity made explicit.
- Modify `docs/ARCHITECTURE.md`: record the future SSD ownership boundary and the narrow XWayland backing invariant.

### Task 1: Unblock the existing test suite and add the geometry red tests

**Files:**
- Modify `src/compositor/state/desktop_windows.rs`.
- Modify `src/compositor/state/window_resize.rs`.
- Modify `src/compositor/state/task_05_8_tests.rs`.

**Interfaces:**
- Consume `SurfacePlacement`, `XdgWindowGeometry`, `RenderableSurface`, `CompositorState::apply_committed_window_geometry`, and `CompositorState::update_toplevel_visual_render_assignment`.
- Produce `derive_root_render_placement(frame, committed_geometry)` as the only frame-to-buffer offset helper and deterministic tests for the `(100,100)` frame / `(16,10,1000,700)` geometry example.

- [x] **Step 1: Apply only the existing visibility unblock.** Change `desktop_window_frame` from private module visibility to `pub(in crate::compositor)`; do not alter its behavior.

```rust
pub(in crate::compositor) fn desktop_window_frame(
    &self,
    window_id: WindowId,
) -> Option<(i32, i32, u32, u32)> {
```

- [x] **Step 2: Add the pure geometry test before adding the helper implementation.** In the `window_resize` test module, assert that a frame placement at `(100,100)` and committed XDG geometry `(16,10,1000,700)` derive `root_at(84,90)` and preserve the frame root mode.

```rust
#[test]
fn root_render_placement_is_frame_origin_minus_committed_window_geometry() {
    assert_eq!(
        derive_root_render_placement(
            SurfacePlacement::absolute_root_at(100, 100),
            Some(XdgWindowGeometry::new(16, 10, 1000, 700)),
        ),
        SurfacePlacement::absolute_root_at(84, 90),
    );
}
```

- [x] **Step 3: Add the first-renderable lifecycle red test.** Extend the existing `task_05_8` fixture to model committed geometry being applied while no root renderable exists, then insert the first root renderable and invoke the production publication seam. Assert that the first renderable has `render_placement == Some(root_at(84,90))` before any preview geometry or resize interaction is installed.

- [x] **Step 4: Add the resize-stability red test.** Reuse the same fixture to record the derived placement, begin/complete 100 deterministic preview cycles whose frame and committed geometry are unchanged, and assert the derived root placement is unchanged at every cycle. Use a separate movement assertion to prove that only a changed frame placement changes the derived origin.

- [x] **Step 5: Run the red tests.** Run:

```text
cargo test --locked root_render_placement_is_frame_origin_minus_committed_window_geometry -- --exact --test-threads=1
cargo test --locked task_05_8 -- --test-threads=1
```

Expected before production implementation: the visibility-unblocked suite compiles, while the new helper/lifecycle assertions fail because the centralized derivation/publication behavior is not yet present.

### Task 2: Add explicit native/XWayland render identity and red backing tests

**Files:**
- Modify `src/compositor/surface.rs`.
- Modify `src/compositor/mod.rs`.
- Modify `src/compositor/state_data.rs`.
- Modify `src/compositor/state/xwayland_windows.rs`.
- Modify `src/compositor/render.rs`.
- Modify all `RenderableSurface` fixtures in `src/compositor/render.rs`, `src/compositor/state/*.rs`, `src/compositor/tests/*.rs`, and `src/native_output/tests/*.rs` that construct the struct directly.

**Interfaces:**
- Produce `SurfaceRenderBackend::{NativeWayland,Xwayland}` with native as the generic constructor default and an explicit XWayland marker used only by confirmed XWayland publication.
- Consume that identity from `xwayland_visual_backing_target` and `server_frame_rects_for_surface`.

- [x] **Step 1: Add the failing identity assertions to existing render tests.** Mark a confirmed XWayland fixture explicitly and add a native absolute-root aperture fixture that must return no backing. Assert the native case and the XWayland case separately, so the old placement-only predicate cannot pass the native test.

```rust
assert_eq!(
    xwayland_visual_backing_target(&native, target, native.visual_clip.as_ref()),
    None,
);
assert!(xwayland_visual_backing_target(&xwayland, target, xwayland.visual_clip.as_ref()).is_some());
```

- [x] **Step 2: Run the backing tests red after the fixture identity is expressed.** Run:

```text
cargo test --locked xwayland_grow_preview_renders_black_backing_without_scaling -- --exact --test-threads=1
cargo test --locked ordinary_xdg_and_managed_x11_surfaces_emit_no_server_frame_primitives -- --exact --test-threads=1
```

Expected: the new native-vs-confirmed-identity assertion fails against the placement-only predicate; existing compilation errors from the new field are resolved only by adding the field and explicit fixture defaults in the minimal implementation step.

- [x] **Step 3: Add `SurfaceRenderBackend` and default generic renderable construction to `NativeWayland`.** Keep the type small and copyable; do not include decoration or window-management state in it.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SurfaceRenderBackend {
    #[default]
    NativeWayland,
    Xwayland,
}
```

- [x] **Step 4: Mark only confirmed XWayland publication.** Add a narrow `mark_xwayland` method and call it immediately after `to_renderable_surface` in both `adopt_current_xwayland_surface_content` and `commit_xwayland_surface_buffer`. Leave native XDG, popup, layer, cursor, and generic test construction native.

- [x] **Step 5: Make backing eligibility identity-first.** Require `surface.render_backend == SurfaceRenderBackend::Xwayland`, root placement, and an aperture. Keep the opaque color and rectangle behavior unchanged until XWayland tests establish whether it can be removed.

- [x] **Step 6: Route server-frame inspection through the same predicate.** `server_frame_rects_for_surface` must call the same backing helper rather than repeat an independent placement-only condition.

- [x] **Step 7: Run focused render and XWayland tests green.** Run the two tests from Step 2 plus:

```text
cargo test --locked xwayland_resize_visual -- --test-threads=1
cargo test --locked render::tests -- --test-threads=1
```

### Task 3: Centralize geometry derivation and initialize first renderables

**Files:**
- Modify `src/compositor/state/window_resize.rs`.
- Modify `src/compositor/state/surface_commits.rs`.
- Modify `src/compositor/state/task_05_8_tests.rs`.

**Interfaces:**
- Consume `surface_window_geometries`, authoritative `surface_placements`, `toplevel_visual_geometries`, and the new `derive_root_render_placement` helper.
- Produce renderables whose derived root placement is correct after creation and after every authoritative frame/geometry update.

- [x] **Step 1: Implement the pure helper minimally.** Return the frame placement unchanged when no committed XDG rectangle exists; otherwise subtract signed geometry x/y with saturating arithmetic and preserve the root mode.

```rust
pub(crate) fn derive_root_render_placement(
    frame: SurfacePlacement,
    committed: Option<XdgWindowGeometry>,
) -> SurfacePlacement {
    let Some(committed) = committed else { return frame; };
    SurfacePlacement {
        parent_surface_id: None,
        local_x: frame.local_x.saturating_sub(committed.x),
        local_y: frame.local_y.saturating_sub(committed.y),
        root_mode: frame.root_mode,
    }
}
```

- [x] **Step 2: Replace the inline subtraction in `update_toplevel_visual_render_assignment`.** Feed the selected visual/frame placement and committed geometry to the helper; do not add any branch based on resize interaction state.

- [x] **Step 3: Reapply the assignment after native root publication.** In `commit_surface_buffer`, after inserting or updating the renderable and before final scene reorder completion, call `update_toplevel_visual_render_assignment(root_surface_id)` whenever the root is an XDG toplevel. This must cover the first renderable even when `toplevel_visual_geometries` is empty.

- [x] **Step 4: Preserve compositor placement ownership.** Keep `set_surface_placement_with_cause` as the only writer of authoritative frame placement; its existing render-assignment refresh remains the path for move/mode/resize frame changes.

- [x] **Step 5: Run the geometry focused tests green.** Run:

```text
cargo test --locked root_render_placement_is_frame_origin_minus_committed_window_geometry -- --exact --test-threads=1
cargo test --locked task_05_8 -- --test-threads=1
cargo test --locked native_geometry -- --test-threads=1
```

### Task 4: Verify CPU/GLES parity, aperture alpha, and decoration policy

**Files:**
- Modify `src/egl_renderer.rs`.
- Modify `src/compositor/render.rs` tests.
- Modify `src/compositor/protocols/xdg.rs` tests only if an existing decoration fixture is available.
- Modify `docs/ARCHITECTURE.md`.

- [x] **Step 1: Use the shared identity predicate in both GLES backing call sites.** Keep draw order and `surface_render_plans_with_aperture` unchanged; only ensure the helper now receives/uses explicit identity.

- [x] **Step 2: Add a native transparent CSD render regression.** Build a native absolute-root renderable with alpha pixels, non-zero committed geometry, and a preview aperture. Compose over a known wallpaper pixel and assert the result equals the scene below for transparent source pixels, while `backing_target()` and `server_frame_rects_for_surface()` are empty.

- [x] **Step 3: Add CPU/GLES scene-decision parity assertions.** For the same native and confirmed XWayland fixtures, compare CPU scene-element `backing_target` and GLES command layers; native must have no `Solid(XwaylandBacking)`, confirmed XWayland must agree with the CPU element.

- [x] **Step 4: Add the Firefox-like grow/shrink geometry assertions.** Use buffer extents larger than the logical window geometry and verify stale valid content stays at the derived origin while the aperture only controls valid content regions. Assert no native black fill is present for both grow and shrink targets.

- [x] **Step 5: Document the future SSD boundary and retained XWayland invariant.** Add a concise architecture note stating that CSD remains client-owned and that the current backing is XWayland-only for stale opaque X11 target coverage; list the future SSD ownership dimensions from the approved design.

- [x] **Step 6: Run focused renderer/decorations tests.** Run:

```text
cargo test --locked render::tests -- --test-threads=1
cargo test --locked xwayland_resize_visual -- --test-threads=1
cargo test --locked xdg -- --test-threads=1
cargo test --locked native_output::tests::output -- --test-threads=1
```

### Task 5: Full validation, diff review, and commit

**Files:** No additional source files beyond the focused changes above.

- [x] **Step 1: Run all required validation commands fresh.**

```text
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
./bin/check-source-layout
git diff --check
```

- [x] **Step 2: Review the final diff and source layout.** Confirm no generated artifacts, no application-specific symbols, no first-resize flags, no generic placement-based XWayland predicate, no SSD advertisement, and no unrelated subsystem changes.

- [x] **Step 3: Record manual qualification honestly.** If Firefox/Zen/Kitty/Steam cannot be launched in this environment, report manual qualification as unavailable and do not claim visual confirmation from unit tests.

- [x] **Step 4: Commit the completed fix directly on `main`.**

```bash
git add docs/superpowers/plans/2026-08-10-native-resize-geometry-transparency-plan.md docs/ARCHITECTURE.md src/compositor src/egl_renderer.rs src/native_output/tests/output.rs
git commit -m "fix(compositor): stabilize CSD resize geometry and transparency"
```
