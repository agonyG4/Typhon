# Presentation Coverage / Borderless Direct Scanout Correctness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Do not dispatch subagents for this plan.

**Goal:** Correct the semantic fullscreen, rendered-bounds, disabled-policy, and hot-path behavior around Presentation Coverage while retaining borderless Direct Scanout candidacy.

**Architecture:** Keep physical scene analysis in Presentation Coverage and semantic fullscreen policy in the existing `FullscreenCompositionPlan`. Share renderer target-rectangle construction with coverage, reuse ActiveScene origins and decoration inputs, gate expensive candidate analysis on Direct Scanout policy/active assignment state, and make visual grouping/index lookup linear in the number of surfaces/groups.

**Tech Stack:** Rust, Cargo, Wayland/XWayland compositor integration tests, existing compositor state fixtures, codebase-memory structural queries, `rtk` command proxy.

## Global Constraints

- Do not restore semantic fullscreen as a prerequisite for Direct Scanout candidacy.
- Preserve `candidate.is_some() <=> blockers.is_empty()`.
- Preserve all existing physical Direct Scanout authority and composition fallback.
- Preserve `FullscreenCompositionPlan`, XDG/EWMH fullscreen behavior, VRR, Predictive O1, KMS scheduling, generalized plane allocation, Eclipse, and the default Direct Scanout policy.
- Do not add frame-loop logging or synchronous tracing.
- Compile in the current repository folder so build artifacts remain in the existing `target` directory.
- Use TDD: each production change follows a failing test, focused RED run, minimal GREEN implementation, and focused GREEN run.
- Do not stage or modify the unrelated viewport-merge changes in `src/compositor/state/subsurfaces.rs`, `src/compositor/subsurface.rs`, or the unrelated pacing plan.

---

### Task 1: Separate semantic fullscreen async eligibility and gate disabled inspection

**Files:**
- Modify: `src/compositor/state/fullscreen.rs` — expose a read-only semantic solitary-fullscreen query backed by the existing `FullscreenCompositionPlan`.
- Modify: `src/compositor/server.rs` — forward the query to native-output code.
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs` — use the semantic query for `AsyncEligibility::solitary_fullscreen`.
- Modify: `src/native_output/runtime/presentation_direct.rs` — carry `NativeDirectScanoutPreference`, decide whether candidate analysis is needed, and force composition when policy is off.
- Modify: `src/native_output/runtime/presentation_cycle.rs` — pass the policy into `DirectPresentationInputs`.
- Test: `src/compositor/presentation_modes.rs` — borderless Vsync/NotSolitaryFullscreen and real solitary-fullscreen Async regressions.
- Test: `src/native_output/runtime/presentation_direct.rs` — policy gate decision tests.
- Test: `src/compositor/tests/direct_scanout.rs` — retain/extend the normal output-sized candidate and semantic fullscreen-negative assertions.

**Interfaces:**
- Produce `OwnCompositorServer::direct_scanout_solitary_fullscreen(root_surface_id: u32) -> bool`, implemented from `fullscreen_composition_plan()` owner identity plus `solitary_owner_only`.
- Produce `should_inspect_direct_scanout(preference: NativeDirectScanoutPreference, direct_active: bool) -> bool` with `preference.enabled() || direct_active`.
- Extend `DirectPresentationInputs` with `direct_scanout_preference: NativeDirectScanoutPreference`.

- [ ] **Step 1: Add the failing async-policy regression.**

  In `src/compositor/presentation_modes.rs`, add a test that supplies `TearingPolicy::Auto`, async metadata, and every independent `AsyncEligibility` requirement true except `solitary_fullscreen: false`; assert `Vsync` and `Some(AsyncBlocker::NotSolitaryFullscreen)`. Add the matching true case and assert `Async` with no blocker.

- [ ] **Step 2: Run the focused RED test.**

  Run `cargo test --locked compositor::presentation_modes --lib` (or the narrow package filter accepted by this crate). The new negative must fail only if the intended assertion is not already represented; fix test setup errors before implementation.

- [ ] **Step 3: Add the failing inspection-gate tests.**

  In `presentation_direct.rs` tests, assert `should_inspect_direct_scanout(Off, false) == false`, `should_inspect_direct_scanout(ExperimentalAuto, false) == true`, and `should_inspect_direct_scanout(Off, true) == true`.

- [ ] **Step 4: Run the focused RED gate tests.**

  Run `cargo test --locked native_output::runtime::presentation_direct --lib`; confirm the missing helper causes the expected compile failure.

- [ ] **Step 5: Implement the semantic query and inspection gate.**

  Add the server/state forwarding method without adding fullscreen state. In inspection, derive the candidate only when the helper allows it; clear the stored key when analysis is skipped, avoid reporting a new direct path while inactive and policy-off, and make `composition_required` true when policy is off or an active direct assignment is no longer eligible. Pass the preference from the cycle. In Atomic direct scanout, replace `candidate.root_surface_id == candidate.surface_id` with `server.direct_scanout_solitary_fullscreen(candidate.root_surface_id)`.

- [ ] **Step 6: Run focused GREEN tests and the existing direct-scanout tests.**

  Run `cargo test --locked compositor::presentation_modes --lib`, `cargo test --locked native_output::runtime::presentation_direct --lib`, and `cargo test --locked compositor::tests::direct_scanout --lib`. Confirm the real fullscreen candidate remains eligible and the normal output-sized candidate remains a candidate while fullscreen semantic state is absent.

- [ ] **Step 7: Commit the task.**

  Stage only the Task 1 files and commit with `fix(scanout): separate coverage from fullscreen tearing policy`.

### Task 2: Reuse renderer-authoritative rendered bounds in Presentation Coverage

**Files:**
- Modify: `src/compositor/render.rs` — factor target rectangle construction into a helper that accepts authoritative origins and uses `render_target_size`.
- Modify: `src/compositor/presentation_coverage.rs` — consume render-space targets rather than `surface.width/height` rectangles.
- Modify: `src/compositor/state/presentation_coverage.rs` — pass ActiveScene origins and the shared targets; pass cached origins into decoration construction.
- Modify: `src/compositor/state/window_decoration.rs` — share the existing decoration materialization implementation with a caller-provided origin slice.
- Test: `src/compositor/presentation_coverage.rs` — rendered-target intersection and fully off-output negative tests.
- Test: `src/compositor/state/task_05_8_tests.rs` or the existing presentation-coverage state test module — candidate/blocker integration assertion if required by fixture visibility.

**Interfaces:**
- Produce `render::surface_render_space_targets(surfaces: &[RenderableSurface], origins: &[(i32, i32)], output_scale: f64) -> Vec<SurfaceTargetRect>`.
- Extend `analyze_presentation_coverage` to receive the authoritative target rectangles.
- Keep `native_decoration_render_instances_for_scale` behavior unchanged for existing callers while adding an origin-reusing internal path.

- [ ] **Step 1: Add the failing rendered-bounds regression.**

  Build a covering XRGB application and an above application whose committed surface is placed just outside the output on the negative-x side with a small committed width, then set `render_target_size` wide enough to reach into the output. Assert `visible_content_above` contains `Application`. Keep the existing completely outside surface assertion and add `DirectScanoutSceneRejection::ApplicationContentAbove`/candidate-none coverage at the state layer if the fixture supports it.

- [ ] **Step 2: Run the focused RED test.**

  Run `cargo test --locked compositor::presentation_coverage --lib` and confirm the target-size test fails because the old committed `width/height` rectangle is used.

- [ ] **Step 3: Implement the shared renderer target helper.**

  Move the `render_target_size.map(...).unwrap_or((surface.width, surface.height))` choice and `render_space_rect_from_logical` call into the helper. Have `surface_render_space_assignments` consume it so renderer and coverage use the same target dimensions and scaling math. In the state coverage path, use `active_scene_surface_origins()` and the existing output-coordinate scale used by coverage rather than recomputing origins.

- [ ] **Step 4: Reuse ActiveScene origins for decorations.**

  Refactor only the decoration helper’s origin acquisition so coverage can pass the cached ActiveScene origins; preserve all decoration filtering, mode, chrome policy, layout, and render-plan behavior.

- [ ] **Step 5: Run focused GREEN tests.**

  Run `cargo test --locked compositor::presentation_coverage --lib`, `cargo test --locked compositor::state::task_05_8_tests --lib`, and `cargo test --locked compositor::tests::direct_scanout --lib`. Confirm the render-target case blocks and the completely off-output case does not.

- [ ] **Step 6: Commit the task.**

  Stage only the Task 2 files and commit with `fix(compositor): use rendered bounds for presentation coverage`.

### Task 3: Bound visual group and decoration lookup costs

**Files:**
- Modify: `src/compositor/render.rs` — bucket surface indices by computed root and index decorations by root while preserving back-to-front group order.
- Test: `src/compositor/render.rs` existing visual group tests — extend deterministic grouping coverage if needed; do not add tracing or per-frame counters.
- Modify: `docs/superpowers/specs/2026-09-15-presentation-coverage-direct-scanout-correctness-design.md` only if the implementation reveals a precise remaining pre-existing cost that must be documented.

**Interfaces:**
- Preserve `visual_stack_groups`, `WindowVisualGroup::stack_order_with_popups`, group order, surface index order, popup splitting, and decoration indices exactly.

- [ ] **Step 1: Add a deterministic behavior test if current tests do not cover multiple groups with interleaved subsurfaces/popups.**

  Assert the optimized grouping returns the same roots and surface index vectors for ordinary subsurfaces and popup roots as the current renderer tests.

- [ ] **Step 2: Run the focused RED test if a new case was added.**

  Run `cargo test --locked compositor::render --lib`; the test should fail only if the new assertion exposes a materialization-order issue.

- [ ] **Step 3: Implement one-pass bucket materialization and root-indexed decorations.**

  Build `HashMap<root_index, Vec<surface_index>>` during the existing `visual_roots` enumeration, then emit groups in the existing first-seen root order. Build one decoration map before mapping visual groups. Do not change unrelated renderer paths.

- [ ] **Step 4: Run focused GREEN and scene tests.**

  Run `cargo test --locked compositor::render --lib` and `cargo test --locked compositor::tests --lib`.

- [ ] **Step 5: Commit the task.**

  Stage only the renderer file and any directly added renderer test, then commit with `perf(compositor): bound visual group materialization`.

### Task 4: Add XWayland boundary and negative coverage

**Files:**
- Modify: `src/compositor/tests/xwayland.rs` or the narrowest existing XWayland state test module — add the XWayland-backed normal-window state regression.
- Modify: `src/compositor/tests/direct_scanout.rs` — expose the candidate/semantic assertions in the motivating path if the integration fixture can publish the needed XRGB dmabuf.
- Modify: `src/compositor/tests/support/server_runtime.rs` only if a minimal capture command is required.
- Modify: `src/compositor/state/task_05_8_tests.rs` or `src/compositor/state/xwayland_mode.rs` tests — add the state-level XWayland SurfaceRenderBackend/ownership fixture if integration dmabuf is unavailable.

**Interfaces:**
- The test must create/represent a real `SurfaceRole::Xwayland`/`SurfaceRenderBackend::Xwayland` root associated with an X11-owned normal window, semantic fullscreen false, output-sized visual geometry, XRGB dmabuf metadata, identity scale/transform/viewport, no decorations/effects/popups/additional surfaces, and a valid presentation generation.

- [ ] **Step 1: Identify the existing narrowest XWayland fixture.**

  Read the exact X11 snapshot/admission and XWayland surface publication helpers. Prefer a state-level test if the integration fixture cannot cheaply publish a dmabuf.

- [ ] **Step 2: Add the failing XWayland tests.**

  Add a positive normal borderless case asserting semantic fullscreen remains false, root coverage is found, no scene blockers exist, and a candidate is produced. Add negatives for visible SSD (`ServerSideDecorationVisible`), render-target content above (`ApplicationContentAbove`), and ARGB/unknown opacity (no candidate). Do not set semantic fullscreen to make the positive case pass.

- [ ] **Step 3: Run the focused RED tests.**

  Run `cargo test --locked compositor::tests::xwayland --lib` plus the selected state test filter. Fix only fixture/setup errors before production changes.

- [ ] **Step 4: Implement the narrowest missing XWayland test support.**

  Reuse existing X11 ownership/admission and publication helpers; do not create fake semantic fullscreen state or a second XWayland model.

- [ ] **Step 5: Run focused GREEN XWayland and direct-scanout tests.**

  Run the XWayland filters and `cargo test --locked compositor::tests::direct_scanout --lib`.

- [ ] **Step 6: Commit the task.**

  Stage only the XWayland/direct-scanout test support files and commit with `test(compositor): cover XWayland borderless scanout boundary`.

### Task 5: Full verification, source-layout baseline comparison, and final review

**Files:**
- No additional production files unless verification exposes a test-specific defect.

- [ ] **Step 1: Refresh graph evidence for the final changed paths.**

  Run codebase-memory change detection and `check_index_coverage` for every changed source path. Read any newly reported partial ranges directly before relying on graph claims.

- [ ] **Step 2: Run the required verification commands in the current folder.**

  Run, recording exit status and relevant output:

  ```bash
  cargo fmt --check
  cargo check --locked --all-targets
  cargo clippy --locked --all-targets -- -D warnings
  cargo test --locked
  ./bin/check-source-layout
  ```

- [ ] **Step 3: Compare source-layout violations.**

  Run the source-layout gate on the baseline commit before the implementation and on the final tree (or compare the recorded baseline output). Report both counts; claim globally green only if the final count is zero.

- [ ] **Step 4: Review the direct path end-to-end.**

  Confirm the final diff follows semantic window state → PresentationCoverageAnalysis → DirectScanoutSceneAnalysis → candidate → EffectivePresentation/AsyncEligibility → physical validation → Atomic TEST_ONLY → submit/pageflip or composition fallback. Explicitly confirm borderless candidates stay Vsync-only unless an independent semantic policy grants async.

- [ ] **Step 5: Run `rtk git diff --check`, inspect status, and commit the final verification/documentation changes.**

  Do not stage the unrelated viewport-merge files or pacing plan. Use a final commit only when the required checks and baseline comparison are recorded.
