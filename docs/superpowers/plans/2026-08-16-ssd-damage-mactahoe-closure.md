# Typhon SSD Damage and MacTahoe Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Typhon server-side decorations a compositor-owned window visual with correct old/new damage, real MacTahoe SVG assets, real font titles, correct geometry, polished input, and equivalent CPU/GLES semantics.

**Architecture:** Build one immutable decoration visual representation from the package theme loader, containing raster assets and measured title layout. Build per-frame instances with stable `WindowId`/root-surface identity, logical origin, outer bounds, and visual signature. Feed current and previous decoration scene snapshots into the same logical damage calculation used by CPU frame copies and GLES buffer-age repaint planning.

**Tech Stack:** Rust 2024, Cargo, `resvg`/`usvg`/`tiny-skia` for bounded in-process SVG rasterization, existing `ab_glyph` plus `fontdb` for cached title font selection and rasterization, existing CPU compositor and EGL/GLES renderer, deterministic unit/integration tests.

## Execution status

Tasks 1–9 are implemented in the working tree. The final verification and native-session qualification in Task 10 remain evidence-gathering steps; this plan intentionally keeps the detailed checklists below as the implementation audit trail.

## Global Constraints

- Preserve all pre-existing dirty and untracked work; never reset, restore, clean, stash, or overwrite unrelated files.
- Keep SSD buttons on the RIGHT in the exact order Minimize → Maximize/Restore → Close.
- Use `render::surface_origins()` and its `render_placement`/absolute/cascaded/XWayland contract for render, hit-test, and damage geometry.
- Do not use full-output repaint as the final ghosting fix; preserve partial repaint and buffer-age behavior.
- Do not parse SVGs, read theme files, discover fonts, or upload textures in the frame hot path.
- Keep theme activation last-known-good when package parsing, asset decoding, font selection, or rasterization fails.
- Do not globally disable Direct Scanout; true fullscreen must have no SSD, while ordinary decorated windows remain compositor-composited.
- All documentation and source comments added by this closure must be in English.

---

### Task 1: Record the baseline and reproduce native SSD ghosting

**Files:**
- Inspect: `src/native_output/output/damage.rs`, `src/native_output/runtime/presentation.rs`, `src/native_output/runtime/frame.rs`, `src/compositor/render.rs`
- Test: `src/native_output/tests/output.rs`, `src/compositor/state/window_decoration_tests.rs`

**Interfaces:**
- Consumes: `RenderableSurface`, `DecorationRenderInstance`, `NativeOutputDamage`, and `DesktopSceneRenderer`.
- Produces: a deterministic failing pixel-level repeated-move test that compares partial updates against a full-reference render.

- [ ] **Step 1: Capture the baseline.** Run `git status --short`, `git rev-parse HEAD`, `git diff --stat`, and `git log --oneline -8`; record the results in the design note without staging them.
- [ ] **Step 2: Write the failing test.** Simulate a decorated 300×200 root surface moving through at least 30 absolute positions. At each position render a full reference and a reused-buffer partial update with damage ages 1, 2, and 3. Assert all pixels match and explicitly sample old titlebar-only points outside the client rectangle.
- [ ] **Step 3: Run the test against `3d835df`.** Use `TMPDIR=/tmp/t rtk cargo test native_output_damage --lib` and the focused ghost test. Expected failure: old titlebar pixels survive because only client surface bounds are included.
- [ ] **Step 4: Add a temporary diagnostic only if needed.** Compare the same test with forced full-output damage to prove stale damage is the cause; remove the diagnostic before implementation proceeds.

### Task 2: Introduce stable decoration scene identity and snapshots

**Files:**
- Modify: `src/compositor/render.rs`
- Modify: `src/compositor/state/window_decoration.rs`
- Modify: `src/compositor/server.rs`
- Test: `src/compositor/state/window_decoration_tests.rs`

**Interfaces:**
- Consumes: current authoritative render origins and `DecorationRenderPlan`.
- Produces: `DecorationSceneSnapshot { window_id, root_surface_id, origin, bounds, visual_signature }`, plus `DecorationRenderInstance::scene_snapshot()`.

- [ ] **Step 1: Write identity/bounds tests.** Assert two frames with only an origin change retain identity but have old/new bounds; title, active state, maximize state, theme generation, and asset identities affect `visual_signature`.
- [ ] **Step 2: Run the tests red.** Confirm the current anonymous instance cannot expose the required stable identity and signature.
- [ ] **Step 3: Implement the snapshot type.** Store stable identity and logical outer bounds alongside the immutable render plan. Derive the signature deterministically from plan generation, layout, colors, text, and asset identities.
- [ ] **Step 4: Build instances from one resolved origin vector.** Keep `render::surface_origins(surfaces)` as the only root-origin source and preserve `render_placement` previews.
- [ ] **Step 5: Verify.** Run the focused decoration/render tests and inspect that no decoration path uses raw `RenderableSurface::x/y` as a global origin.

### Task 3: Integrate old/new SSD bounds into native logical damage

**Files:**
- Modify: `src/native_output/output/damage.rs`
- Modify: `src/native_output/runtime/mod.rs`
- Modify: `src/native_output/runtime/bootstrap.rs`
- Modify: `src/native_output/runtime/presentation.rs`
- Modify: `src/native_output/runtime/cycle.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Test: `src/native_output/tests/output.rs`

**Interfaces:**
- Consumes: previous/current `Vec<DecorationSceneSnapshot>` and existing `NativeDamageAccumulator`.
- Produces: `NativeDamageAccumulator::from_decoration_bounds_changes()` and a combined scene damage function used for every repaint path.

- [ ] **Step 1: Write direct damage tests.** Cover move old/new bounds, a point in the old titlebar outside the client, resize extents, fullscreen/disappearance, destroy/disappearance, and state-only signature changes.
- [ ] **Step 2: Run the tests red.** The old implementation must omit the titlebar-only old bound and report no damage for decoration-only changes.
- [ ] **Step 3: Implement bounds conversion.** Convert each logical decoration outer bound through the same scale/output clipping rules as surface damage, union previous and current bounds by stable `(WindowId, root_surface_id)` identity, and include disappeared entries.
- [ ] **Step 4: Store previous decoration snapshots with the same lifecycle as `last_renderable_surfaces`.** Initialize them at bootstrap, update them after presented/direct-suppressed state transitions exactly where surfaces are updated, and keep them unchanged on skipped frames.
- [ ] **Step 5: Combine surface and decoration damage.** Preserve the current cursor damage logic and empty/full fallback policy; do not force full-output damage for ordinary SSD changes.
- [ ] **Step 6: Verify.** Run native output damage tests and the repeated pixel ghost regression; the partial result must equal the full-reference render.

### Task 4: Feed one logical damage definition to CPU and GLES

**Files:**
- Inspect/modify: `src/native_output/runtime/frame.rs`
- Inspect/modify: `src/native_output/scanout/gbm_cpu.rs`
- Inspect/modify: `src/native_output/scanout/dumb.rs`
- Inspect/modify: `src/native_output/scanout/egl_gbm.rs`
- Inspect/modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Inspect/modify: `src/egl_renderer.rs`
- Test: `src/egl_renderer/damage_tests.rs`, `src/native_output/tests/frame.rs`

**Interfaces:**
- Consumes: the combined `NativeOutputDamage` from Task 3.
- Produces: CPU frame copies and GLES buffer-age/scissor repairs that cover the same old/current decoration bounds.

- [ ] **Step 1: Write parity tests.** For identical surface/decorations and damage, assert CPU copy rectangles and GLES `OutputDamage` rectangles contain the same logical regions; assert no full-output promotion for a bounded move.
- [ ] **Step 2: Run tests red.** Confirm the existing CPU/GLES paths disagree or omit decoration-only regions.
- [ ] **Step 3: Thread combined damage through both paths.** Ensure `render_server_frame`/CPU copy and `egl_scene_draw_request`/GLES repaint consume the same `NativeOutputDamage` without a renderer-specific damage definition.
- [ ] **Step 4: Include decoration state in scene cache invalidation.** Use the stable visual signature and current damage rather than vector index or filename parsing.
- [ ] **Step 5: Verify.** Run renderer, EGL damage, native output, and repeated ghost tests at buffer ages 1–3.

### Task 5: Make MacTahoe-Dark load the real bundled package

**Files:**
- Modify: `src/compositor/decoration/theme.rs`
- Modify: `resources/decorations/MacTahoe-Dark/theme.json`
- Replace/add: `resources/decorations/MacTahoe-Dark/assets/*.svg`
- Add: `resources/decorations/MacTahoe-Dark/NOTICE`
- Test: `src/compositor/decoration/theme.rs`

**Interfaces:**
- Consumes: the existing package loader and safe relative asset resolver.
- Produces: `load_theme_by_name("MacTahoe-Dark", generation)` returning the package source and actual asset bytes, with only a separately named internal emergency fallback.

- [ ] **Step 1: Write loader tests.** Assert MacTahoe selection resolves the bundled package source, includes active/inactive/hover assets for minimize/maximize/restore/close, and does not return the old builtin snapshot.
- [ ] **Step 2: Run tests red.** The current special case reports `source == "builtin"` and has no asset bytes.
- [ ] **Step 3: Copy the supplied local Labwc assets into the package.** Map `iconify` to minimize, `max` to maximize, `max_toggled` to restore, and `close` to close; preserve provenance in `NOTICE` without modifying `/home/agony/.themes/MacTahoe-Dark`.
- [ ] **Step 4: Update the schema.** Use 26 px titlebar metrics, right-side button order, exact colors, active/inactive border colors, and explicit hover/pressed fallback semantics. Bump schema version only if required by incompatible field changes.
- [ ] **Step 5: Remove the name bypass.** Resolve MacTahoe through the same package search/validation path as external themes and retain the last-known-good snapshot on activation failure.
- [ ] **Step 6: Verify.** Load the actual package in tests and assert generation/source/error behavior.

### Task 6: Add immutable bounded SVG rasterization and CPU/GLES asset resources

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`
- Add/modify: `src/compositor/decoration/raster.rs`
- Modify: `src/compositor/decoration/mod.rs`
- Modify: `src/compositor/decoration/theme.rs`
- Modify: `src/compositor/decoration/render_plan.rs`
- Modify: `src/compositor/render.rs`
- Modify: `src/egl_renderer.rs`
- Test: `src/compositor/decoration/raster.rs`, `src/compositor/decoration/render_plan.rs`

**Interfaces:**
- Consumes: validated SVG bytes from `DecorationThemeSnapshot`.
- Produces: bounded `DecorationRasterAsset { asset_id, width, height, rgba_premultiplied }` referenced by image primitives; CPU alpha composition and GLES texture upload from the same pixels.

- [ ] **Step 1: Write raster tests.** Parse/rasterize a real bundled SVG, assert non-empty pixels, transparent corners, active/inactive differences, hover differences, and maximize/restore differences; reject external-resource SVGs before rasterization.
- [ ] **Step 2: Run tests red.** The current plan only stores filenames and both renderers draw procedural geometry.
- [ ] **Step 3: Add `resvg` with bounded options.** Parse only in theme activation/reload, use an in-memory `usvg::Options` with no filesystem/network resource resolution, enforce maximum SVG bytes and raster dimensions, and cache immutable scale-keyed assets.
- [ ] **Step 4: Replace image primitive filenames with stable asset identities/data.** Keep renderer-independent visual semantics in the plan and make CPU/GLES consume the same raster pixels.
- [ ] **Step 5: Implement CPU alpha composition.** Composite premultiplied RGBA over the titlebar without opaque black/white boxes.
- [ ] **Step 6: Implement GLES RGBA textures.** Upload cached raster assets only when theme generation/asset/scale changes, release stale generation resources, and use the existing blend state correctly.
- [ ] **Step 7: Verify.** Run raster, CPU render, GLES command, and parity tests at scales 1.0, 1.25, 1.5, and 2.0.

### Task 7: Replace placeholder title glyphs with measured font rendering

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`
- Add/modify: `src/compositor/decoration/text.rs`
- Modify: `src/compositor/decoration/theme.rs`
- Modify: `src/compositor/decoration/render_plan.rs`
- Modify: `src/compositor/render.rs`
- Modify: `src/egl_renderer.rs`
- Test: `src/compositor/decoration/text.rs`, `src/compositor/decoration/render_plan.rs`

**Interfaces:**
- Consumes: theme font family/style/size/alignment and title text.
- Produces: cached measured/rasterized title runs with actual advances, Unicode-safe ellipsis, clip bounds, and shared CPU/GLES glyph pixels/placement.

- [ ] **Step 1: Write typography tests.** Cover Inter/Noto Sans/generic sans fallback, actual font size, Unicode titles, measured ellipsis, centered placement, narrow controls, and no placeholder block glyphs.
- [ ] **Step 2: Run tests red.** The current `/ 8` truncation and 5×7 glyph switch must fail these tests.
- [ ] **Step 3: Discover fonts outside the frame path.** Use `fontdb` and `ab_glyph` with preference order SF Pro Text/Display, Inter, Noto Sans, generic sans; cache the selected font bytes and metrics during theme activation.
- [ ] **Step 4: Implement measured layout.** Center relative to the outer frame, clip against a safe region protecting right controls, reserve symmetric opposite-side safety space, and ellipsize by measured glyph advances.
- [ ] **Step 5: Render shared title pixels.** Remove `decoration_glyph()` and `egl_decoration_glyph()` from the production path; CPU and GLES consume equivalent cached glyph/raster data and scale deterministically.
- [ ] **Step 6: Verify.** Run typography, CPU/GLES parity, Unicode, clipping, and fractional-scale tests.

### Task 8: Correct maximize/frame geometry and placement constraints

**Files:**
- Modify: `src/compositor/decoration/layout.rs`
- Modify: `src/compositor/state/fullscreen.rs`
- Modify: `src/compositor/state/desktop_windows.rs`
- Modify: `src/compositor/state/window_resize.rs`
- Modify: `src/compositor/state/window_interaction.rs`
- Test: `src/compositor/state/desktop_window_tests.rs`, `src/compositor/state/task_05_8_tests.rs`

**Interfaces:**
- Consumes: final decoration extents and authoritative usable output geometry.
- Produces: client geometry whose decorated outer frame equals usable output in maximized mode, with exact restore geometry and no cumulative drift.

- [ ] **Step 1: Write the failing maximized-frame test.** Assert `outer == usable_output` for SSD maximize and that CSD/maximize behavior remains unchanged.
- [ ] **Step 2: Run it red.** The current client uses the full usable height and then adds titlebar extents.
- [ ] **Step 3: Move frame-extents accounting into the authoritative geometry calculation.** Subtract SSD extents before configuring the client; do not clip away an oversized titlebar.
- [ ] **Step 4: Add placement/constraint tests.** Verify initial decorated placement remains visible, keep-in-area uses outer frame, move/resize previews use the same bounds, and XWayland frame extents remain correct.
- [ ] **Step 5: Add 100-cycle maximize/restore stress.** Assert exact initial client and outer geometry after every cycle.
- [ ] **Step 6: Verify.** Run geometry, XDG, XWayland resize, and fullscreen/direct-scanout tests.

### Task 9: Polish input ownership and interaction semantics

**Files:**
- Modify: `src/compositor/decoration/layout.rs`
- Modify: `src/compositor/state/window_decoration.rs`
- Modify: `src/compositor/state/window_interaction.rs`
- Modify: `src/compositor/state/desktop_windows.rs`
- Test: `src/compositor/state/window_decoration_tests.rs`, `src/compositor/state/window_interaction_tests.rs`

**Interfaces:**
- Consumes: stable captured `WindowId`, decoration layout hit regions, and logical pointer coordinates.
- Produces: button-first hit precedence, pressed visual cancellation on leave, exact release semantics, spatial double-click threshold, and no action retargeting.

- [ ] **Step 1: Write failing interaction tests.** Cover press/leave/re-enter/release, top-edge button-vs-resize precedence, double-click distance, and rear-window minimize/maximize/close/move with focus changes.
- [ ] **Step 2: Run tests red.** Confirm current pressed state and time-only double-click behavior fail the new cases.
- [ ] **Step 3: Track hover-inside separately from capture.** Keep capture for action ownership, but only show pressed artwork while the pointer is over the captured button; release outside performs no action.
- [ ] **Step 4: Make button hits precede resize-edge hits.** Preserve resize edges everywhere outside control hit regions and test the top few pixels of each control.
- [ ] **Step 5: Add a logical spatial threshold to double-click detection.** Require same titlebar/window and distance within the threshold in addition to the existing 500 ms window.
- [ ] **Step 6: Verify.** Run input, pointer, interaction, and decoration tests.

### Task 10: Add native qualification and final documentation

**Files:**
- Add: `docs/superpowers/specs/2026-08-16-ssd-damage-mactahoe-closure-design.md`
- Modify: `docs/ARCHITECTURE.md` if the final ownership model changes public architecture
- Test/qualification: `astreactl` decoration commands and the project’s normal native launch path

**Interfaces:**
- Consumes: all completed damage/theme/geometry/input behavior.
- Produces: an English engineering note, qualification record, and final evidence distinguishing deterministic tests from native-session observations.

- [ ] **Step 1: Write the design note.** Document the confirmed ghost root cause, KWin ownership principle, Typhon snapshot/damage model, package/raster/text pipeline, geometry/input decisions, and intentional shadow/radius limitations.
- [ ] **Step 2: Exercise the real theme loader.** Use `astreactl decoration list`, `set-theme MacTahoe-Dark`, `status`, and `reload`; verify package source and generation changes without changing `/home/agony/.themes/MacTahoe-Dark`.
- [ ] **Step 3: Detect native-session availability.** If the normal launcher and display devices are available, test Kitty, Qt, XWayland, Firefox/Zen if present, repeated movement/resizes/maximize/fullscreen/focus; record negotiated CSD/SSD behavior and 100+ movement updates. If unavailable, record the exact blocker and do not claim live qualification.
- [ ] **Step 4: Review the full diff.** Search for full-output repaint workarounds, missing old/disappeared bounds, CPU/GLES divergence, raw global `x/y`, filename-based icon semantics, 5×7 glyphs, package bypasses, frame-loop parsing, fake fonts, resize stealing, global Direct Scanout disablement, and unrelated staged files.
- [ ] **Step 5: Run final gates.** Run `cargo fmt --check`, `cargo check --locked --all-targets`, `cargo clippy --locked --all-targets -- -D warnings`, `TMPDIR=/tmp/t cargo test --locked`, `./bin/check-source-layout`, `git diff --check`, and all focused damage/theme/renderer/interaction/geometry/XDG/XWayland/Direct Scanout groups.
- [ ] **Step 6: Commit focused changes.** Stage only closure files, use focused commit messages by subsystem, and report every commit hash plus final `git status --short`.

## Self-review checklist

- [ ] The first test fails on the current tree before the damage fix.
- [ ] Old and current decoration bounds are both damaged, including disappearance.
- [ ] CPU frame copies and GLES buffer-age damage consume the same logical damage.
- [ ] Decoration render, hit-test, and damage bounds share `surface_origins()`.
- [ ] MacTahoe is loaded through the package loader and real SVG bytes determine pixels.
- [ ] CPU/GLES use shared raster assets with correct alpha and scale behavior.
- [ ] Titles use measured font metrics and Unicode-safe clipping.
- [ ] Maximize outer geometry fits the usable output and restore is drift-free.
- [ ] Input captures exact `WindowId`, prioritizes buttons, and uses spatial double-click detection.
- [ ] Direct Scanout remains eligible only when true fullscreen hides SSD.
- [ ] Native qualification is reported only if actually performed.
