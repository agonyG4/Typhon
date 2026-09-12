# Native Window Titlebar Regression Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Restore compositor-owned server-side titlebars for ordinary Floating XDG and XWayland windows by preserving their decoration resources through the native EGL frame path.

**Architecture:** Keep the existing decoration policy, layout, frame-scene ownership, visual grouping, command generation, Effects ordering, and Direct Scanout policy unchanged unless verification identifies a separate defect. Repair the proven renderer resource boundary by reconciling canonical-frame and lifecycle decoration instances in one resource pass, so lifecycle setup cannot evict resources needed by the visible canonical scene.

**Tech Stack:** Rust, Cargo, Wayland/XDG, XWayland, EGL/GLES renderer, existing Typhon test fixtures, `rtk` command wrapper.

## Global Constraints

- Work in the current Typhon checkout and preserve unrelated pre-existing changes.
- Reuse the existing `target/` directory; do not create another Cargo target or build tree.
- Use `rtk` for Cargo, Git, and repository inspection commands.
- Do not modify Eclipse, Blur Assignment, Blur Policy, Lamp, or EGL bootstrap specifications.
- Preserve Floating `WindowChromePolicy::Full` and Tiled `WindowChromePolicy::Minimal` behavior.
- Do not use subagents.

---

### Task 1: Capture the verified pipeline diagnosis and baseline

**Files:** None (read-only investigation)

1. Reconfirm the working-tree state and existing Cargo target directory without touching unrelated document deletions.
2. Trace one ordinary Floating XDG path and one ordinary Floating XWayland path through decoration policy, layout, resolved frame scene, EGL draw request, visual grouping, command generation, effects execution, and scanout eligibility using the existing source and fixtures.
3. Record the verified values: effective mode `ServerSide`, Floating membership, Full chrome policy, positive titlebar height, decoration instance ownership, visual-group attachment, EGL command emission, and the resource eviction boundary.
4. Run the focused baseline decoration and EGL renderer tests before editing.

### Task 2: Add a failing renderer resource-lifetime regression

**Files:** `src/egl_renderer.rs`

1. Add a deterministic unit test for the resource requirement collector using one ordinary canonical decoration and one lifecycle decoration.
2. Assert that one reconciliation pass retains the resource keys required by both instance sets.
3. Run only this focused test and confirm it fails before the production helper/repair exists, demonstrating the test protects the diagnosed contract.

### Task 3: Repair canonical/lifecycle decoration resource reconciliation

**Files:** `src/egl_renderer.rs`

1. Extract a small production helper that collects solid-color and raster-asset resource requirements from an arbitrary decoration-instance iterator.
2. Change `ensure_decoration_resources` to consume an iterator and use the collected requirements for stale-resource removal and creation.
3. Replace the two sequential canonical/lifecycle calls with one call over the union of both instance collections.
4. Run the focused renderer regression and the relevant existing decoration/resource tests.

### Task 4: Strengthen deterministic scene-boundary coverage

**Files:** `src/compositor/render.rs`, `src/egl_renderer/effects/executor.rs`, `src/native_output/runtime/frame.rs`

1. Add or extend a production grouping test proving a valid root decoration receives `decoration_index = Some(...)` and is not orphaned.
2. Add an Effects executor regression with background, root-surface, and decoration commands in one visual group, verifying a `BeforeSurface(root)` background-blur assignment still executes both root and decoration ranges.
3. Add or extend a real resolved-frame-scene regression for an ordinary Floating ServerSide window, proving the canonical root and its decoration survive into `ResolvedNativeFrameScene`.
4. Keep these tests deterministic and preserve the existing Tiled Minimal policy.

### Task 5: Verify protocol, XWayland, scanout, and native acceptance behavior

**Files:** Existing fixtures/tests only if a narrowly scoped assertion is required; otherwise no additional files.

1. Re-run the existing XDG and XWayland decoration tests, confirming Unset/default resolves according to current policy, explicit CSD remains CSD, normal X11 frame extents report top `26`, and fullscreen behavior remains unchanged.
2. Verify normal Floating decorated windows are not eligible for a bypass path that omits compositor-owned decorations; do not add a second scanout policy.
3. Attempt the requested native acceptance checks for Floating XDG and XWayland windows, including titlebar interaction, frame extents, minimize/restore, effect-bearing rendering, and the intentional Tiled Minimal case. Report unavailable native-environment checks explicitly.

### Task 6: Run fresh verification and commit only the repair

**Files:** Only the plan, production changes, and regression tests from Tasks 2–4

1. Run the complete requested verification with the existing Cargo target directory:

   ```bash
   rtk cargo fmt --all -- --check
   rtk cargo check --locked --all-targets
   rtk cargo clippy --locked --all-targets -- -D warnings
   rtk cargo test --locked window_decoration
   rtk cargo test --locked xwayland_decoration
   rtk cargo test --locked egl_renderer
   rtk cargo test --locked native_output
   rtk cargo test --locked effects
   rtk cargo test --locked
   rtk bin/qualify-presentation --dry-run
   ```

2. Run `git diff --check`, inspect the final diff, and confirm no protected subsystem or unrelated dirty file changed.
3. Commit the scoped repair and tests in this Git repository.
4. Report the exact first failing boundary, affected backends, policy state, instance/group/command state, root cause, changed files, fresh counts, and native visual results without claiming unavailable visual checks passed.
