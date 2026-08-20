# Neutral Output Background Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove Typhon's artistic compositor wallpaper and replace it with a neutral, bounded fallback without changing repaint or presentation architecture.

**Architecture:** The CPU scene base becomes clipped fills of `OUTPUT_BACKGROUND`; no output-sized wallpaper cache remains. The EGL scene begins with a dedicated 1x1 `ServerFrameColor::OutputBackground` resource and keeps ordinary client/layer ordering. Generic layer-shell, buffer age, render-ahead, fullscreen, Direct Scanout, and presentation history remain unchanged.

**Tech Stack:** Rust, Cargo, QTest-like Rust unit/integration suites, EGL/GLES, Wayland layer-shell test clients, existing native-output harnesses.

## Global Constraints

- Preserve partial repaint, buffer age, presentation-domain history, render-ahead, fullscreen, Direct Scanout, cursor arbitration, and dirty work.
- Do not read Paper state or add Paper-specific compositor behavior.
- Do not introduce another output-sized background texture or buffer.
- Keep real Paper/`astreactl wallpaper` concepts unchanged.

---

### Task T1: Replace the empty-scene gradient expectation with a failing neutral test

**Files:**
- Modify: `src/compositor/render.rs`

**Interfaces:**
- `compose_output()` and `OUTPUT_BACKGROUND` remain the public testable CPU fallback contract.

- [ ] **Step 1: Change the test first**

Update `compose_output_draws_desktop_wallpaper_when_empty` to assert every pixel equals `OUTPUT_BACKGROUND`; remove varying-gradient assertions.

- [ ] **Step 2: Run the focused test to verify red**

Run: `rtk cargo test --locked compose_output_draws_desktop_wallpaper_when_empty --lib`  
Expected: FAIL because the current gradient varies by position.

- [ ] **Step 3: Commit boundary**

Task-owned commit, if commits are requested: `test(render): define neutral output background contract`.

### Task T2: Add CPU partial-repair neutral-background regression

**Files:**
- Modify: `src/compositor/render.rs`

**Interfaces:**
- Uses `DesktopSceneRenderer::compose_reusing_frame()` and existing `OutputRect` damage tracking.

- [ ] **Step 1: Add the failing regression**

Start with a solid client occupying a region, move/remove it, run the existing partial rebuild path, and assert the exposed pixels equal `OUTPUT_BACKGROUND` while unaffected client pixels remain intact.

- [ ] **Step 2: Run the test to verify red**

Run: `rtk cargo test --locked desktop_scene_renderer --lib`  
Expected: FAIL because repair currently copies the gradient cache.

- [ ] **Step 3: Commit boundary**

Task-owned commit, if commits are requested: `test(render): cover neutral partial background repair`.

### Task T3: Implement the CPU solid fallback and remove its gradient cache

**Files:**
- Modify: `src/compositor/render.rs`

**Interfaces:**
- `fill_output_background_rect(scene, frame_width, frame_height, damage_rect)` clips and fills only the requested region.
- `DesktopSceneRenderer` retains scene and damage metadata but no `wallpaper`, dimensions, generation, or `ensure_wallpaper()` state.

- [ ] **Step 1: Implement the minimal full-rebuild path**

Resize/fill `scene` with `OUTPUT_BACKGROUND`; do not allocate a second output-sized fallback buffer.

- [ ] **Step 2: Implement clipped partial repair**

Replace `copy_wallpaper_rect_to_scene()` with `fill_output_background_rect()` and preserve redraw-after-repair ordering.

- [ ] **Step 3: Remove obsolete production gradient code**

Remove corner constants, `Rgb` helpers if unused, `draw_wallpaper()`, gradient interpolation helpers, wallpaper cache fields, and `wallpaper_generation()`.

- [ ] **Step 4: Run focused renderer tests**

Run: `rtk cargo test --locked compositor::render --lib`  
Expected: PASS, including transparent surface and partial-repair tests.

- [ ] **Step 5: Commit boundary**

Task-owned commit, if commits are requested: `refactor(render): replace compositor wallpaper with solid background`.

### Task T4: Add the lower Paper/background partial-repair regression

**Files:**
- Modify: `src/compositor/render.rs`

**Interfaces:**
- Uses ordinary `RenderableSurface` ordering; no Paper namespace or special renderer branch.

- [ ] **Step 1: Add the regression**

Compose a lower opaque background client and an upper window, move the upper window, and assert the exposed region restores lower client pixels rather than `OUTPUT_BACKGROUND`.

- [ ] **Step 2: Run the focused test**

Run: `rtk cargo test --locked compositor::render --lib`  
Expected: PASS after T3; failure indicates redraw ordering or clipping damage.

- [ ] **Step 3: Commit boundary**

Task-owned commit, if commits are requested: `test(render): preserve lower background surface on repair`.

### Task T5: Replace EGL wallpaper texture with a 1x1 solid resource

**Files:**
- Modify: `src/compositor/render.rs`
- Modify: `src/egl_renderer/geometry.rs`
- Modify: `src/egl_renderer.rs`

**Interfaces:**
- Add `ServerFrameColor::OutputBackground` with `pixel() == OUTPUT_BACKGROUND` and include it in `ALL`.
- Replace `EglDrawLayer::Wallpaper` with `EglDrawLayer::Solid(ServerFrameColor::OutputBackground)`.

- [ ] **Step 1: Add the failing structural test**

Extend the existing EGL command/resource test style to assert the first scene command is `Solid(OutputBackground)`; assert no wallpaper resource field/symbol remains through source-level test or compile-time structure checks.

- [ ] **Step 2: Run the test to verify red**

Run: `rtk cargo test --locked --bin oblivion-one egl_renderer`  
Expected: FAIL because the current first command is `Wallpaper` and the resource is output-sized.

- [ ] **Step 3: Implement the minimal EGL change**

Remove `wallpaper_resource`, `ensure_wallpaper_resource()`, resize/destroy paths, and the output-sized pixel upload. Let `ensure_frame_resources()` create the 1x1 `OutputBackground` resource and emit it as the first command.

- [ ] **Step 4: Run EGL tests**

Run: `rtk cargo test --locked --bin oblivion-one egl_renderer`  
Expected: PASS; no output-sized fallback upload is present.

- [ ] **Step 5: Commit boundary**

Task-owned commit, if commits are requested: `perf(egl): remove output-sized wallpaper texture`.

### Task T6: Preserve transparent, fullscreen, scanout, and terminology contracts

**Files:**
- Modify: `src/compositor/render.rs` only for renderer terminology/tests.
- Modify: `src/compositor/fullscreen.rs` only if a narrowly scoped background-culling name is required.
- Modify: `src/native_output/runtime/metrics.rs` only if adding a compatibility alias is safe.
- Test: `src/native_output/tests/fullscreen_frame_scene.rs`
- Test: `src/native_output/tests/scanout.rs`

**Interfaces:**
- External `astreactl wallpaper` remains untouched.
- Existing `fullscreen_wallpaper_culled` metric remains as a compatibility name unless a scoped alias can be added without breaking consumers.

- [ ] **Step 1: Update transparent/empty tests**

Assert transparent pixels reveal lower client pixels or `OUTPUT_BACKGROUND`; remove only obsolete gradient fixture construction.

- [ ] **Step 2: Run focused correctness suites**

Run: `rtk cargo test --locked compositor::render --lib`, `rtk cargo test --locked native_output::tests::fullscreen_frame_scene --lib`, and `rtk cargo test --locked native_output::tests::scanout --lib`.

- [ ] **Step 3: Confirm no forbidden renderer symbols**

Run: `rtk run -c 'rg -n "WALLPAPER_|draw_wallpaper|wallpaper_resource|ensure_wallpaper_resource|EglDrawLayer::Wallpaper|wallpaper: Vec" src/compositor src/egl_renderer'`  
Expected: no production renderer matches; legitimate Paper/`astreactl` matches elsewhere are not in scope.

- [ ] **Step 4: Commit boundary**

Task-owned commit, if commits are requested: `test(render): preserve neutral fallback and scanout contracts`.

### Task T7: Final Typhon validation and report

**Files:**
- Create/modify: `REPORT-2026-08-20-neutral-output-background-closure.md`
- Validate: all Task T1–T6 files.

- [ ] **Step 1: Run format, locked checks, tests, clippy, and layout checks**

Run: `rtk run -c 'cargo fmt --check'`; `rtk cargo check --locked --all-targets`; `rtk cargo test --locked`; `rtk cargo clippy --locked --all-targets -- -D warnings`; `rtk run -c 'bash bin/check-source-layout'`.

- [ ] **Step 2: Run focused layer-shell and renderer suites**

Run the layer-shell creation-order/resize regression, compositor render tests, EGL tests, damage/buffer-age tests, recent presentation-history tests, fullscreen frame tests, and scanout tests using existing targets.

- [ ] **Step 3: Record evidence and blockers**

Document baseline, removed CPU/EGL work, neutral architecture, partial repair, history/fullscreen/scanout results, native Paper qualification status, final RTK commands, and final `rtk git status --short`. Do not claim a native session result if unavailable.

- [ ] **Step 4: Commit boundary**

Task-owned report commit, if commits are requested: `docs(render): record neutral output background closure`.

## Execution record (2026-08-20)

- [x] T1–T6 implemented, including CPU neutral fill/repair, EGL solid first command, and generic layer-shell coverage.
- [x] Focused locked suites pass: render **52**, EGL **74**, layer-shell **51**, damage **66**, fullscreen **3**, and scanout **196**.
- [x] `cargo fmt --check` and `cargo check --locked --all-targets` pass.
- [x] Broad test/lint/layout results are recorded in `REPORT-2026-08-20-neutral-output-background-closure.md`; unrelated baseline failures remain.
- [ ] Native Astrea session qualification remains pending; no visual claim is made.
