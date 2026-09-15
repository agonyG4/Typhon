# Backdrop Diagnostic Matrix Implementation Plan

**Goal:** Add a Typhon-only diagnostic matrix that independently switches ordinary backdrop capture between scene replay and real-framebuffer capture, and Dual Kawase execution between partial and full internal demand.

**Architecture:** Keep the existing replay+partial behavior as the default. Add process-scoped debug policy parsing, propagate full-Kawase policy through the render-graph demand planner, and build one spatially consistent scene-work region before framebuffer-backed SceneCapture. Reuse the existing direct framebuffer capture path, preserve SurfaceCapture semantics, and expose stable trace fields. Do not implement persistent backdrop caching until native matrix results resolve the root cause.

**Tech Stack:** Rust, Typhon typed render graph, GLES renderer, existing effect trace and test harness, `rtk`-wrapped Cargo/Git commands.

## Global Constraints

- Modify Typhon only; do not modify Eclipse.
- Preserve all unrelated dirty changes and deleted files; stage only this task.
- Defaults remain `capture=replay` and `kawase=partial`.
- Do not use `OBLIVION_ONE_FORCE_FULL_REPAINT=1` as a solution.
- Keep bounded effect regions and the one-pass reverse traversal; add no fixpoint loop or global full repaint.
- Keep final visible output constrained by existing output-influence and bounded-clip fixes.
- Do not change SurfaceCapture semantics or implement PersistentBackdropCache in this diagnostic phase.

---

### Task 1: Add RED coverage for debug policy and planning behavior

**Files:**
- Modify: `src/egl_renderer/effects/trace.rs`
- Modify: `src/effects/render_graph.rs`
- Test: existing unit-test modules in those files

- [ ] Add tests for default/invalid process-policy parsing, full-Kawase demand making every internal Kawase pass cover its complete graph texture while retaining the final visible clip, and trace formatting of the selected policies.
- [ ] Run the focused tests and record the expected failures before production implementation.

### Task 2: Implement process-scoped diagnostic policies

**Files:**
- Modify: `src/egl_renderer/effects/trace.rs`
- Modify: `src/egl_renderer/effects/mod.rs`

- [ ] Parse `TYPHON_EFFECT_DEBUG_CAPTURE_MODE` and `TYPHON_EFFECT_DEBUG_KAWASE_MODE` once with `OnceLock`, accept only `replay|framebuffer` and `partial|full`, warn once per invalid value, and fall back to defaults.
- [ ] Export the policy types/accessor to the renderer and executor without adding Astrea configuration.

### Task 3: Implement planner-level full internal Kawase policy

**Files:**
- Modify: `src/effects/render_graph.rs`
- Modify: the existing planner call site in `src/egl_renderer.rs`

- [ ] Preserve the old planner wrapper as partial mode and add a policy-aware entry point.
- [ ] Seed selected Dual Kawase internal pass regions with their full graph texture domains before the existing bounded reverse traversal so capture demand is propagated consistently upstream.
- [ ] Keep Composite/OutputPostProcess constrained by their existing demand and visible output-influence rules.

### Task 4: Implement framebuffer-backed ordinary SceneCapture ordering

**Files:**
- Modify: `src/egl_renderer/effects/executor.rs`
- Modify: `src/egl_renderer.rs` only where the existing renderer API must expose the shared clearing/capture behavior

- [ ] Compute one bounded scene-work region equal to ordinary repaint work plus selected ordinary backdrop capture domains when framebuffer mode is active.
- [ ] Clear that region in the active output framebuffer, advance the single scene cursor over that same region before every ordinary backdrop SceneCapture, and use the existing direct framebuffer blit for the capture.
- [ ] Leave replay mode and SurfaceCapture behavior unchanged; retain lifecycle/checkpoint direct-capture behavior.
- [ ] Add ordering/event regressions for background/A/B/C, stacked backdrops, and translucent source equivalence.

### Task 5: Extend trace observability and deterministic regressions

**Files:**
- Modify: `src/egl_renderer/effects/trace.rs`
- Modify: `src/egl_renderer/effects/executor.rs`
- Modify: existing GLES effect harness tests where practical

- [ ] Emit stable capture/Kawase policy fields, scene-work rectangles/bounding box, and explicit `capture_mode`/`debug_full_kawase` fields under effect tracing.
- [ ] Add framebuffer-mode partial-repair and full-Kawase equivalence tests, preserving the existing sentinel-independence regression.
- [ ] Add frame-1721 physical dependency coverage and deterministic dimension/boundary/scaling coverage if the current raster-aware helpers are directly reusable here.

### Task 6: Refactor, verify, and commit only this task

**Files:**
- Modify only files required by Tasks 1–5

- [ ] Run focused tests, formatting, check, clippy, full tests, and diff checks with `rtk`.
- [ ] Attempt native qualification without forced full repaint; if unavailable, stop after documenting exact commands and do not claim the visual defect is fixed.
- [ ] Inspect the diff, verify unrelated dirty state is preserved, and create a commit containing only diagnostic-matrix hunks.

