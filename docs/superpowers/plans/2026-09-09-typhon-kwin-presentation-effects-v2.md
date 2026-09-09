# Typhon KWin-Inspired Presentation Effects v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Execute this plan inline with focused review checkpoints. Do not dispatch subagents or create a second worktree. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make geometry transitions start from explicit pre-mutation source rectangles, make interaction interruption preserve canonical window/decorations, and add the short fixed-duration KDE geometry policy.

**Architecture:** Keep `PresentationAnimator`, native-frame sampling, transition identity, physical settlement, `WindowVisualGroup`, and canonical layout ownership unchanged. Add a typed source-bearing visual-install contract, route XDG/tiled/X11 callers through it, narrow interaction takeover to origin rebasing, and add an immediate maximized restore handoff for direct pointer movement.

**Tech Stack:** Rust, existing compositor test fixtures, `PresentationRect`/`WindowGeometry`, Cargo locked checks, `rtk` workflow.

## Global Constraints

- Work in `/home/agony/GitHub/Typhon` on the approved current checkout; do not reset, checkout, discard, or overwrite unrelated renderer/Effects work.
- Use the existing `target/` directory; never create another build directory.
- Use `rtk` for git, build, test, formatting, lint, and source-layout commands.
- Do not use subagents or a separate worktree.
- Keep Markdown, comments, reports, and design notes in English.
- Do not add another timer, scene solve, presentation sample site, frame-history queue, scene graph, or retained-content lifetime mechanism.
- Keep direct pointer move/resize immediate and keep pageflip plus `TransitionId` acknowledgement as the physical settlement authority.
- Commit focused changes as the plan reaches stable checkpoints; stage only files belonging to the current checkpoint.

## File map

- `src/presentation_animation.rs`: add analytic cubic easing variants and tests; keep absolute sampling and spring behavior intact.
- `src/presentation_animation_policy.rs`: add `Kde`, default/alias parsing, and the fixed-duration geometry table while retaining `Macos` springs.
- `src/compositor/state/window_resize.rs`: make XDG visual installation consume an explicit `VisualGeometryTransition` source.
- `src/compositor/state/xwayland_mode.rs`: make X11 visual installation consume the same explicit source contract and semantic mode kinds.
- `src/compositor/state/active_scene.rs`: keep animation sampling generic; replace broad takeover with origin-only interaction rebasing and preserve visual-group transforms.
- `src/compositor/state/windows.rs`: capture source before XDG mutation and add shared immediate interactive restore plumbing.
- `src/compositor/state/tiled_layout.rs`: pass the pre-mutation `current` geometry into layout transitions.
- `src/compositor/state/window_interaction.rs`: reject maximized resize/fullscreen interactions and implement maximized restore-then-move.
- `src/compositor/window_state.rs`: add a read-only restore-geometry accessor.
- `src/compositor/state/tiled_resize.rs`: preserve and expose existing physical-edge rebase semantics without canonical visual takeover.
- `src/compositor/state/window_decoration.rs`, `src/compositor/state/surfaces.rs`, `src/compositor/state/fullscreen.rs`, `src/compositor/render.rs`: update explicit immediate/animated call sites only where required; preserve ownership and coverage rules.
- `src/compositor/tests/windows.rs`, `src/compositor/tests/xwayland_resize_visual.rs`, `src/compositor/state/window_interaction_tests.rs`, `src/compositor/state/tiled_layout_tests.rs`, `src/compositor/state/task_05_8_tests.rs`, `src/native_output/tests/fullscreen_frame_scene.rs`: add real-path source, policy, visual-group, and interruption regressions.
- `docs/reports/2026-09-09-typhon-kwin-presentation-effects-v2.md`: record symptoms, architecture, tests, verification, KWin observations versus Typhon adaptations, and hardware status.

### Task 1: Freeze the current RED baseline

**Files:**

- Test: `src/compositor/tests/windows.rs`
- Test: `src/compositor/state/tiled_layout_tests.rs`
- Test: `src/compositor/tests/xwayland_resize_visual.rs`
- Test: `src/compositor/state/window_interaction_tests.rs`

**Interfaces:**

- Consumes the existing real compositor fixtures and `capture_presentation_transition_sample`/curve helpers.
- Produces failing assertions that prove the current installer source is reconstructed after mutation and that maximized move is currently accepted.

- [ ] **Step 1: Add the XDG fullscreen source-authority regression.** Use the existing buffered XDG mode fixture with a non-zero starting geometry and capture the transition sample at the fixture's exact transition-start epoch. Assert the source rectangle is `(640, 320, 800, 600)`, assert its `x` and `y` are non-zero, and assert the target is output-sized `(0, 0, output_width, output_height)`.
- [ ] **Step 2: Add equivalent real-path maximize/restore assertions.** Capture each transition sample immediately after the real command entry point and assert maximize starts from the floating rectangle, while restore starts from the maximized rectangle rather than stored normal restore geometry.
- [ ] **Step 3: Add the tiled and managed X11 source assertions.** In the existing reflow fixture assert the transition start equals the pre-solve visual geometry; in the existing X11 mode fixture assert the transition start equals the X11 frame before `set_x11_frame_geometry`.
- [ ] **Step 4: Run only these new tests.** Run `rtk run -- cargo test --locked <focused test filters>` and record the expected RED failures. If a test passes before production changes, fix the test so it exercises the real entry point and fails for the source-authority reason.
- [ ] **Step 5: Add the policy/easing RED tests.** Add tests for `EaseInCubic`, `EaseOutCubic`, `EaseInOutCubic`, `PresentationAnimationStyle::Kde`, default environment aliases, and the seven KDE durations. Run the filters and confirm they fail because the new variants/table do not exist.
- [ ] **Step 6: Commit the RED tests.** Run `rtk git diff --check`, stage only the four test files, and commit `test: expose presentation source and interruption defects`.

### Task 2: Make animated visual installation source-explicit

**Files:**

- Modify: `src/compositor/state/window_resize.rs`
- Modify: `src/compositor/state/xwayland_mode.rs`
- Modify: all callers found by the already-verified installer trace
- Test: focused source-authority tests from Task 1

**Interfaces:**

- Consumes `WindowGeometry` and `PresentationAnimationKind` from current callers.
- Produces `VisualGeometryTransition::{Immediate, Animated { source, kind }}` and installers that never query mutable geometry to infer an animated source.

- [ ] **Step 1: Define the transition contract in the compositor state module.** Use a small `Copy` enum with `Immediate` and `Animated { source: WindowGeometry, kind: PresentationAnimationKind }`; keep `Option<PresentationAnimationKind>` out of the animated installer API.
- [ ] **Step 2: Change the XDG installer.** Accept the contract, install the target visual geometry, and call `animate_toplevel_visual_geometry` only with the supplied source. Preserve render-generation, pointer-hit-generation, target-clearing, and cancellation behavior. Keep a clearly named immediate wrapper for setup/non-animated callers.
- [ ] **Step 3: Apply the identical contract to the X11 installer.** Preserve pending XWayland content handling and immediate wrapper behavior; remove its late `current_visual_root_window_geometry` lookup.
- [ ] **Step 4: Update non-transition callers.** Pass `Immediate` for initial/setup/output/client paths. Pass `Animated` only after an explicit source capture at the caller boundary.
- [ ] **Step 5: Run compilation and the focused RED tests.** Run `rtk run -- cargo test --locked <source filters>`; expect source assertions to remain RED until callers are corrected, while compile errors are resolved completely.
- [ ] **Step 6: Commit the typed API checkpoint.** Run `rtk run -- cargo fmt --check` and `rtk git diff --check`, then commit `refactor: require explicit visual transition sources`.

### Task 3: Capture XDG, tiled, and XWayland sources before mutation

**Files:**

- Modify: `src/compositor/state/windows.rs`
- Modify: `src/compositor/state/tiled_layout.rs`
- Modify: `src/compositor/state/xwayland_mode.rs`
- Modify: `src/compositor/window_state.rs`
- Test: `src/compositor/tests/windows.rs`, `src/compositor/state/tiled_layout_tests.rs`, `src/compositor/tests/xwayland_resize_visual.rs`

**Interfaces:**

- Consumes the typed installer contract from Task 2.
- Produces source-correct transitions for XDG mode changes, restores, Dwindle reflow, and managed X11 mode changes; provides `WindowState::restore_geometry(&self)` for read-only interactive planning.

- [ ] **Step 1: Capture XDG source first.** In `set_root_window_mode`, resolve current visual/root geometry before any mode/configure/placement mutation and store it in a local named `source_geometry`; use restore bookkeeping separately.
- [ ] **Step 2: Pass source through XDG mode entry.** After canonical mode/configure/fullscreen-owner/placement updates, install `Animated { source: source_geometry, kind }`. Preserve restore geometry as the pre-normal geometry only.
- [ ] **Step 3: Fix programmatic XDG restore.** Capture the current source before clearing fullscreen ownership, changing mode, or consuming restore geometry. Use `Animated` for floating restore and `Immediate`/`LayoutReflow` only for layout-owned restore as the existing architecture requires.
- [ ] **Step 4: Fix tiled reflow.** Pass the already-computed `current` value directly into the animated installer after configure and canonical placement. Do not run another Dwindle solve or query geometry again.
- [ ] **Step 5: Fix managed X11 mode transitions.** Capture the X11 visual/frame source before `set_x11_frame_geometry` and placement mutation. Map mode changes through the shared `mode_transition_animation_kind` semantic function so maximize/fullscreen enter/exit match XDG. Remove `XwaylandModeChange` if no narrow caller remains; otherwise retain it only for a documented non-semantic case.
- [ ] **Step 6: Add and run source assertions.** Run the focused XDG, tiled, and XWayland filters and confirm every new transition starts at the pre-mutation geometry, including non-zero origins.
- [ ] **Step 7: Commit source-authority GREEN.** Run `rtk run -- cargo fmt --check` and the focused locked tests, then commit `fix: capture geometry sources before canonical mutation`.

### Task 4: Add analytic cubic easing and KDE policy

**Files:**

- Modify: `src/presentation_animation.rs`
- Modify: `src/presentation_animation_policy.rs`
- Modify: `src/compositor/mod.rs` only if re-exports need updating
- Test: policy/easing tests from Task 1 and existing presentation tests

**Interfaces:**

- Consumes the existing `AnimationCurve::Easing` sampler and spring policy.
- Produces `EaseInCubic`, `EaseOutCubic`, `EaseInOutCubic`, `PresentationAnimationStyle::{Kde, Macos}`, and the fixed-duration KDE geometry policy.

- [ ] **Step 1: Implement analytic cubic evaluation.** Add the three enum variants and return `(p, dp/dt)` directly: `t^3`, `1-(1-t)^3`, and the standard continuous piecewise cubic ease-in-out. Clamp only the caller's normalized progress as today; do not add a lookup table.
- [ ] **Step 2: Make cubic RED tests pass.** Assert endpoints, midpoint values, derivative endpoints, and monotonic sampled progression for all three curves.
- [ ] **Step 3: Add KDE policy constructors and parser.** Make `kde` the default for unset, empty, and `default`; accept explicit `kde` and `macos`; retain the global on/off switch and use a clear diagnostic fallback for unknown style values.
- [ ] **Step 4: Implement the KDE timing table.** Use `160 ms` move, `200 ms` resize/reflow, and `250 ms` maximize/fullscreen with `EaseOutCubic`. Keep `Macos`'s existing spring constants and `XwaylandModeChange` only while it has a real caller.
- [ ] **Step 5: Run policy and animator tests.** Run `rtk run -- cargo test --locked presentation_animation` and the policy filters. Verify explicit `macos` still yields the exact spring curve and KDE reaches the exact target only at fixed duration, without changing pageflip retirement logic.
- [ ] **Step 6: Commit the policy checkpoint.** Run `rtk run -- cargo fmt --check`, `rtk git diff --check`, and commit `feat: add KDE cubic presentation policy`.

### Task 5: Narrow interaction takeover and preserve visual-group coherence

**Files:**

- Modify: `src/compositor/state/active_scene.rs`
- Modify: `src/compositor/state/window_interaction.rs`
- Modify: `src/compositor/state/tiled_resize.rs` only if a helper signature must be adjusted
- Test: `src/compositor/state/window_interaction_tests.rs`, `src/compositor/state/task_05_8_tests.rs`, `src/native_output/tests/fullscreen_frame_scene.rs`

**Interfaces:**

- Consumes physically presented geometry for locality and existing tiled physical-edge rebase.
- Produces a narrow `rebase_interaction_to_presented_origin` operation that cancels animation and retains canonical width/height; preserves one `PresentationGroupTransform` for client/subsurface/SSD.

- [ ] **Step 1: Rename/restrict takeover.** Replace `take_over_presented_visual_geometry` with a narrow origin-rebase helper that takes the current canonical geometry, uses presented `x/y` when valid, retains canonical width/height, cancels the transition, and updates canonical placement/visual assignment only as required for translation continuity.
- [ ] **Step 2: Keep tiled resize separate.** In `begin_window_interaction_for_root`, rebase the existing tiled resize preparation from physically presented client edges before cancelling the old transition; do not pass a presented `WindowGeometry` to any helper that writes canonical width/height.
- [ ] **Step 3: Make direct interaction authoritative.** Ensure after the handoff no presentation transition remains for the root and pointer move/resize paths do not start an animation. Preserve active resize configure flow and Dwindle authority.
- [ ] **Step 4: Add floating interruption regressions.** Cover translation-only move, size-changing move, and normal resize during presentation. Assert physically visible origin continuity, canonical width/height retention, canonical SSD metrics, and no competing transition.
- [ ] **Step 5: Add decorated visual-group regression.** Sample a size-plus-translation transform for a root with a child and SSD; assert all decoration/client/button render inputs use the same `PresentationGroupTransform`, and identity leaves normal decoration geometry unchanged.
- [ ] **Step 6: Add tiled interruption regression.** Assert physical edge rebasing and first pointer delta continuity while canonical Dwindle solution and window size remain untouched.
- [ ] **Step 7: Run focused interaction/frame tests and commit.** Run `rtk run -- cargo test --locked <interaction and fullscreen filters>`; then commit `fix: keep presentation scale out of interaction geometry`.

### Task 6: Implement immediate maximized restore-then-move

**Files:**

- Modify: `src/compositor/state/window_interaction.rs`
- Modify: `src/compositor/state/windows.rs`
- Modify: `src/compositor/window_state.rs`
- Test: `src/compositor/state/window_interaction_tests.rs`, `src/compositor/tests/windows.rs`

**Interfaces:**

- Consumes physically presented rect, pointer coordinates, read-only restore geometry, and existing backend configure/state helpers.
- Produces `Normal` mode plus an immediate restored geometry under the pointer before the `Move` interaction is installed.

- [ ] **Step 1: Add read-only restore access.** Implement `WindowState::restore_geometry(&self) -> Option<WindowGeometry>` without duplicating state or consuming it.
- [ ] **Step 2: Reject unsupported interactions.** Update `window_interaction_allowed_for_mode` so `Maximized + Resize` and all `Fullscreen + Move/Resize` return false; keep normal move/resize behavior and tiled restrictions intact.
- [ ] **Step 3: Add the shared immediate restore helper.** Extract the canonical mode/configure/backend update sequence used by normal restore so programmatic restore remains animated while interactive restore accepts a source/target and installs `Immediate`.
- [ ] **Step 4: Compute pointer anchor in presentation/window space.** Use the physically presented rect to derive horizontal ratio and vertical offset, map the stored restore geometry through `presentation_rect_for_geometry`, and apply the desired presentation-origin delta to the restore placement. Preserve `RootPlacementMode` and allow integer rounding.
- [ ] **Step 5: Begin maximized move through the helper.** Capture physical geometry and pointer anchor before cancelling animation; transition mode to `Normal`, reposition restore geometry, send/update backend state, install immediately, then create the direct Move interaction using the restored canonical width/height.
- [ ] **Step 6: Add maximized drag/resize regressions.** Assert midway maximize drag cancels, mode becomes `Normal`, restore geometry is under the pointer, titlebar remains coherent, direct move owns subsequent position, and maximized resize is rejected.
- [ ] **Step 7: Run focused tests and commit.** Run the locked interaction/windows filters and commit `fix: restore maximized windows before pointer move`.

### Task 7: Audit callers and preserve accepted foundation invariants

**Files:**

- Modify only if required: `src/compositor/render.rs`, `src/compositor/state/fullscreen.rs`, `src/compositor/state/window_decoration.rs`, `src/compositor/state/surfaces.rs`
- Test: existing presentation, fullscreen, render, resize, popup, and direct-scanout tests

**Interfaces:**

- Consumes the source-explicit installer and interaction contracts.
- Produces no new architecture; verifies one frame-local sample/target set, existing fullscreen coverage, physical input/pageflip authority, hidden dormancy, and direct-scanout blockers remain unchanged.

- [ ] **Step 1: Search all visual installer calls.** Run `rtk run -- rg -n -F 'install_toplevel_visual_geometry' src` and the equivalent X11 search; classify every call as Immediate or explicit Animated with a pre-mutation source.
- [ ] **Step 2: Search all canonical writes of visual geometry.** Run `rtk run -- rg -n -F 'toplevel_visual_geometries' src/compositor`; inspect every pointer-interruption path and confirm no presented animated width/height is written into canonical state.
- [ ] **Step 3: Run the foundation regression filters.** Cover one presentation target per native frame, pageflip authority, hidden transition dormancy, fullscreen background retention, popup owner separation, CSD/window space, tiled physical-edge rebase, stale ACK rejection, and direct-scanout blockers.
- [ ] **Step 4: Commit only if source changes were necessary.** Use `rtk git diff --check`; if no production changes are needed, record the audit in the report rather than creating an empty commit.

### Task 8: Write the report and perform final verification

**Files:**

- Create: `docs/reports/2026-09-09-typhon-kwin-presentation-effects-v2.md`

- [ ] **Step 1: Write the English report.** Document the real 165 Hz symptoms, both root causes, maximized-mode inconsistency, KWin source areas studied, observed KWin behavior versus Typhon-specific adaptations, explicit source architecture, final interruption semantics, KDE table, RED/GREEN evidence, and the exact scope exclusions. State hardware qualification status only from actual execution.
- [ ] **Step 2: Perform the final source audit.** Verify there is no generic path converting presented animated width/height into canonical geometry, that visual-group transforms remain shared, that interaction cancels transitions, and that semantic XDG/X11 mode kinds match.
- [ ] **Step 3: Run the required closure commands in the existing build directory.** Run exactly:

```bash
rtk run -- cargo fmt --check
rtk run -- cargo check --locked --all-targets
rtk run -- cargo clippy --locked --all-targets -- -D warnings
rtk run -- cargo test --locked
rtk git diff --check
rtk run -- bash bin/check-source-layout
```

- [ ] **Step 4: Separate failures honestly.** If renderer/Effects formatting, dead-code, or source-layout debt fails independently, record the exact command/output and distinguish it from task-owned failures. Never mask or claim a failed command passed.
- [ ] **Step 5: Recheck codebase-memory coverage.** Call `check_index_coverage` for every operated source path, inspect any flagged range directly, and record any limitation in the report.
- [ ] **Step 6: Review diff and commit the report.** Stage only task-owned files, run `rtk git diff --cached --check`, and commit `docs: report KWin-inspired presentation effects v2`.
- [ ] **Step 7: Perform final status/diff review.** Use `rtk git status --short` and `rtk git log` to confirm focused commits and leave unrelated renderer/Effects changes untouched.

## Completion checklist

- [ ] New geometry transitions cannot infer an animated source after target mutation.
- [ ] XDG maximize/fullscreen/restore, tiled reflow, and managed X11 transitions start from pre-mutation geometry.
- [ ] Root/client/subsurface/SSD share one presentation transform, including identity.
- [ ] Interaction interruption cannot bake animated presentation size into canonical geometry.
- [ ] Tiled resize rebases physical edges while Dwindle remains authority.
- [ ] Maximized move restores to `Normal` immediately under the pointer; maximized resize and fullscreen move/resize are rejected.
- [ ] KDE uses short fixed-duration cubic geometry transitions; explicit macOS still selects springs.
- [ ] Final exact target still waits for pageflip and transition acknowledgement.
- [ ] No lifecycle effects, crossfade, new scene graph, extra timer, or additional layout solve was added.
- [ ] Verification commands and hardware qualification status are reported with fresh evidence.
