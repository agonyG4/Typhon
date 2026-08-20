# Typhon Oversized Resize Presentation Ghosting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (or superpowers:subagent-driven-development) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore pageflip-owned native damage history and make SSD/render/hit-test geometry follow the same visual resize preview so oversized offscreen windows cannot leave stale titlebar or frame pixels.

**Architecture:** Reuse the existing output-token ownership and explicit EGL `EglSceneFrameCommit` lifecycle. Keep one compact `NativeSceneHistory` with a ready candidate, a bounded token-keyed submitted set, and one pageflip-confirmed presented snapshot. Resolve each frame's visual root geometry through the existing accessor and feed the resulting bounds to SSD layout, damage, and hit testing.

**Tech Stack:** Rust, Typhon CPU/GLES scene renderers, GBM/EGL/KMS output paths, Smithay/Wayland compositor state, deterministic framebuffer tests, Cargo, and RTK.

## Global Constraints

- Preserve all unrelated tracked and untracked dirty worktree content.
- Use RTK for every shell command and `apply_patch` for source edits.
- Follow TDD: each production change starts with a focused red test and is verified green before refactoring.
- Keep scene snapshots compact and bounded; do not clone pixels, themes, SVG assets, or serialized compositor state.
- Do not force full-output repaint, disable buffer age, disable triple buffering/render-ahead, disable Direct Scanout, or change clipping without a failing clipping test.
- Keep logical visual bounds un-clipped until output damage is intersected with the target output.
- Native runtime qualification must be reported only when the required DRM/KMS session is actually available.

---

### Task 1: Establish baseline and lock the ownership model

**Files:**
- Read: `src/native_output/runtime/presentation.rs`, `src/native_output/runtime/presentation_worker.rs`, `src/native_output/runtime/cycle/pageflip.rs`, `src/native_output/scanout/output_swapchain.rs`, `src/native_output/scanout/atomic_egl_gbm.rs`
- Read: `src/egl_renderer.rs`, `src/egl_renderer/damage.rs`, `src/compositor/state/window_decoration.rs`, `src/compositor/state/window_resize.rs`
- Create: `docs/superpowers/specs/2026-08-17-oversized-resize-presentation-ghosting-design.md`
- Create: `docs/superpowers/plans/2026-08-17-oversized-resize-presentation-ghosting.md`

**Interfaces:**
- Existing candidate lifecycle: `EglSceneFrameCommit`, `GlesSceneRenderer::commit_presented`, and `GlesSceneRenderer::discard_rendered`.
- Existing output ownership: `RenderedOutputFrame`, `WorkerQueuedOutputFrame`, `SubmittedOutputFrame`, `CompletedOutputFrame`.
- Existing visual authority: `CompositorState::current_visual_root_window_geometry(surface_id)` and `preview_resize_root_window_to`.

- [x] **Step 1: Record baseline.** Capture `git rev-parse HEAD`, `git status --short`, `git diff --stat`, and `git diff --name-only`; preserve the resulting dirty-tree classification in the design journal and final report.
- [x] **Step 2: Verify graph coverage.** Use the codebase-memory project `home-agony-GitHub-Typhon`, search/trace the listed symbols, call `check_index_coverage` for every operated source path, and read `src/native_output/runtime/presentation.rs:108-124` directly because its line 115 is parse-partial.
- [x] **Step 3: Write the design journal and this plan.** Record observed symptom, static evidence, hypothesis/confirmed distinctions, ownership lifecycle, geometry contract, rejected approaches, test architecture, and native qualification limits in English.
- [x] **Step 4: Run the focused baseline tests.** The changed-area native damage, runtime, and decoration suites were run through RTK; the deterministic regression tests were then observed RED before the production changes and GREEN after them.
- [x] **Step 5: Commit boundary.** No documentation-only commit was made because the worktree already contained unrelated tracked and untracked changes.

### Task 2: Prove the native presented-history failure with a red pixel test

**Files:**
- Modify: `src/native_output/output/damage.rs` only for a test-facing pure history helper if existing APIs cannot express the schedule.
- Modify: `src/native_output/tests/output.rs` for the deterministic regression.
- Read: `src/compositor/render.rs` test helpers and existing native `paint_ssd_scene` fixture.

**Interfaces:**
- Produce a pure test model with `presented: FrameSceneSnapshot`, `rendered: Vec<FrameSceneSnapshot>`, and `promote_presented(frame_id)` semantics.
- Consume `native_output_damage_for_scene_and_cursor_with_decorations` and the existing CPU full-reference compositor fixture.

- [x] **Step 1: Write the failing test.** Added `render_ahead_oversized_ssd_repair_matches_full_reference` with the 1920x1080 output, oversized A/B/C widths, old-frame pixel samples, and a clean full-reference comparison.
- [x] **Step 2: Verify RED.** The test failed before the history fix because B-to-C damage left A-only titlebar/frame pixels in the reused framebuffer.
- [x] **Step 3: Add schedule coverage while still red.** Added exact-token history tests for rendered-but-unpresented frames, replacement/discard, stale token regression, and an oversized shrink sequence through 800px.
- [x] **Step 4: Commit boundary.** The test and production changes remain uncommitted to preserve the pre-existing dirty-tree ownership boundary.

### Task 3: Attach frame-owned scene snapshots to native output ownership

**Files:**
- Modify: `src/native_output/runtime/mod.rs` to replace the two independently advanced native history fields with one bounded presented snapshot and candidate ownership container.
- Create: `src/native_output/runtime/scene_history.rs` for compact ready/submitted/presented ownership and exact-token promotion.
- Modify: `src/native_output/runtime/presentation.rs` and `src/native_output/runtime/presentation_worker.rs` to capture candidates at render/queue boundaries without promoting them.
- Modify: `src/native_output/runtime/cycle/pageflip.rs` to promote the completed frame snapshot after exact pageflip identity validation.
- Modify: `src/native_output/runtime/kms_worker/rejection.rs` and `src/native_output/runtime/session_io.rs` to discard unpresented candidates on failure/recovery.
- Test: `src/native_output/runtime/scene_history.rs` and `src/native_output/tests/output.rs` schedule/ownership tests.

**Interfaces:**
- `NativeFrameSceneSnapshot { frame_id, render_generation, scene: NativeSceneSnapshot, cursor_damage }`.
- `NativeSceneSnapshot` contains surface IDs, mapped bounds/damage, and decoration metadata; it intentionally excludes `RenderableSurface::buffer`.
- `NativeSceneHistory::replace_ready`, `queue_submission`, `promote_pageflip`, and discard methods express the ready/submitted/presented lifecycle.
- The existing output/pageflip token is the identity used to select and promote a submitted snapshot; no second output queue is introduced.

- [x] **Step 1: Write the failing ownership test.** Added `rendered_snapshot_advances_presented_history_only_on_matching_pageflip`, plus replacement/discard and stale-token tests.
- [x] **Step 2: Verify RED.** The A/B/C pixel test failed against the pre-fix render-order history.
- [x] **Step 3: Implement the minimum candidate transport.** Replaced the two independently advanced native scene vectors with compact `NativeSceneHistory` ready/submitted ownership keyed by the existing output token; no client buffers are copied.
- [x] **Step 4: Promote only on confirmed presentation.** The exact completed pageflip token promotes the matching snapshot; render completion, queue admission, and KMS submission only create/queue candidates.
- [x] **Step 5: Retire non-presented candidates.** Worker rejection, compatibility errors, replacement, and session quarantine discard ready/submitted snapshots.
- [x] **Step 6: Verify GREEN.** The history tests and full native binary suite pass; the A/B/C pixel reference is equal.
- [x] **Step 7: Commit boundary.** No separate commit was made because the worktree was already dirty.

### Task 4: Integrate native damage with presented transitions and buffer age

**Files:**
- Modify: `src/native_output/runtime/presentation_worker.rs` and `src/native_output/runtime/presentation.rs` to pass the single presented snapshot into damage calculation.
- Modify: `src/native_output/output/damage.rs` only for a pure bounded transition accumulator if required by existing buffer-age ownership.
- Test: `src/native_output/tests/output.rs`, `src/egl_renderer/damage_tests.rs`, and relevant scanout lifecycle tests.

**Interfaces:**
- `native_scene_damage_for_snapshot_transition(output_width, output_height, presented, current, cursor) -> NativeOutputDamage`.
- `PresentedSceneSnapshot` remains the only predecessor for native scene damage; accumulated age repair continues to use `PartialRepaintPlanner` history and exact current damage transitions.

- [ ] **Step 1: Write red age tests.** A dedicated age-1/2/3 schedule table remains follow-up work; the implemented regression covers the presented-history transition and a 2200-to-800 shrink sequence.
- [ ] **Step 2: Verify RED.** No separate buffer-age RED table was added.
- [x] **Step 3: Implement transition accumulation.** `native_output_damage_for_scene_snapshots` uses the presented snapshot, current mapped damage, complete old/new logical bounds, decoration changes, cursor state, and clips only at the output boundary.
- [x] **Step 4: Verify GREEN.** Native damage, partial framebuffer, scene-history, and full binary tests pass; no full-repaint, buffer-age-disable, render-ahead-disable, or Direct Scanout workaround was added.
- [x] **Step 5: Commit boundary.** Left uncommitted with the snapshot lifecycle because the worktree ownership did not permit an isolated commit.

### Task 5: Resolve one visual geometry snapshot for SSD, render, damage, and hit testing

**Files:**
- Modify: `src/compositor/state/window_decoration.rs` and `src/compositor/server.rs` to resolve visual root geometry before `DecorationLayout::for_window`.
- Modify: `src/compositor/render.rs` only if `RenderableSurface`/`WindowVisual` needs one compact resolved geometry field.
- Modify: `src/compositor/state/hit_testing.rs` and related input state only if the existing hit-test path still reads committed dimensions after the red test.
- Test: `src/compositor/state/window_decoration_tests.rs`, `src/compositor/render.rs`, `src/compositor/tests/windows.rs`, and relevant XWayland visual tests.

**Interfaces:**
- `CompositorState::current_visual_root_window_geometry(surface_id) -> Option<WindowGeometry>` is the single visual-size authority.
- `DecorationLayout::for_window(visual_width, visual_height, ...)` receives resolved visual dimensions.
- `DecorationRenderInstance` and `DecorationSceneSnapshot` retain bounds derived from the same layout and origin used for the frame.

- [x] **Step 1: Write the failing preview test.** Added `ssd_layout_follows_resize_preview_without_client_commit` with committed 1800x1000 geometry and a narrower preview anchored at the right edge.
- [x] **Step 2: Verify RED.** The SSD render bounds initially remained 1800px wide while the preview client was 1400px wide.
- [x] **Step 3: Write the left-edge parity test.** Added `ssd_left_edge_preview_keeps_visual_geometry_for_render_and_hit_test` to verify client/titlebar/button-right parity for a left-origin preview.
- [x] **Step 4: Implement the minimum visual accessor use.** SSD render instances and hit testing now use `current_visual_root_window_geometry` with committed geometry as the fallback.
- [x] **Step 5: Align hit testing and damage.** The render-instance and hit-test paths share the visual geometry dimensions; native decoration snapshots are built from those instances.
- [x] **Step 6: Verify GREEN.** The decoration suite passes (11 tests), and the full native binary suite passes (900 tests).
- [x] **Step 7: Commit boundary.** Left uncommitted with native presentation ownership due the pre-existing dirty worktree.

### Task 6: Add oversized/offscreen and mode regression coverage

**Files:**
- Modify: `src/native_output/tests/output.rs` for width/edge/full-reference stress.
- Modify: `src/compositor/tests/windows.rs` and `src/compositor/tests/xwayland_resize_visual.rs` for resize geometry and managed XWayland coverage.
- Modify: `src/compositor/render.rs` only for test helpers/assertions that expose complete logical bounds.

**Interfaces:**
- Stress fixture accepts output size, initial/current logical window geometry, resize edge, presentation schedule, and client type; it returns partial and clean framebuffer pixels plus sampled stale-pixel coordinates.

- [x] **Step 1: Add width stress.** Added 31 shrinking states from 2200 to 800 on the 1920x1080 offscreen fixture.
- [x] **Step 2: Add all resize edges.** Added logical old/new bounds coverage for left, right, top, bottom, top-left, top-right, bottom-left, and bottom-right transitions.
- [ ] **Step 3: Add XWayland/CSD/mode cases.** Cover managed XWayland backing and `_NET_FRAME_EXTENTS`, override-redirect undecorated behavior, CSD zero server extents, oversized floating→maximized→restore, oversized floating→fullscreen→restore, and Direct Scanout eligibility.
- [x] **Step 4: Verify GREEN.** The oversized partial framebuffer fixture equals clean full-reference output pixel-for-pixel across the 31-state shrink sequence, and all-edge logical damage coverage passes.
- [ ] **Step 5: Commit boundary.** Commit only stress/regression tests and any required test-only helper changes.

### Task 7: Add bounded diagnostics and qualify native behavior

**Files:**
- Modify: existing bounded tracing in `src/native_output/runtime/presentation.rs`, `src/native_output/runtime/cycle/pageflip.rs`, and `src/native_output/runtime/metrics.rs` only if needed.
- Update: `docs/superpowers/specs/REPORT-2026-08-17-oversized-resize-presentation-ghosting.md`.

**Interfaces:**
- Trace fields: logical/render generation, frame snapshot ID, visual geometry generation, framebuffer/swapchain slot, buffer age, rendered, queued, submitted, pageflip token, presented snapshot ID, and damage summary.

- [x] **Step 1: Add bounded trace assertions.** The exact-token history tests assert that stale/mismatched pageflip tokens cannot fabricate or regress presented history.
- [x] **Step 2: Detect native availability.** `/dev/dri/card0` and `renderD128` exist, but `astreactl status` reports no Typhon instance and no native qualification process is available.
- [ ] **Step 3: Run native qualification when available.** Blocked by the absent Typhon native instance; no native visual claim is made.
- [x] **Step 4: Write the report.** The report distinguishes deterministic proof, static geometry evidence, environment failures, and the unavailable native qualification.

### Task 8: Final verification and self-review

**Files:**
- Update: `docs/superpowers/specs/REPORT-2026-08-17-oversized-resize-presentation-ghosting.md`
- Stage/commit: only task-owned corrective files/hunks.

- [x] **Step 1: Run required checks.** Formatting, all-target check, full native binary tests, focused library tests, and diff checks pass. Full library tests have 1,664 passes and 20 environment failures; clippy has the known pre-existing XWayland enum-size error; source-layout has three pre-existing compositor-file violations.
- [x] **Step 2: Compare baselines.** The failures are isolated to missing Astrea control entry points, overlong XWayland test socket names, the existing XWayland clippy finding, and existing compositor source-layout limits.
- [x] **Step 3: Answer the final audit.** Render/queue/submission do not promote presented history; exact completed pageflip tokens do; discarded/stale tokens do not. Logical bounds remain unclipped until output damage conversion, SSD render/hit geometry share the visual accessor, and the dirty tree was preserved.
- [x] **Step 4: Commit boundary.** No commit or staging action was taken because the worktree contained unrelated user changes.
