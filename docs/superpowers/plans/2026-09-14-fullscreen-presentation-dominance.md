# Fullscreen Presentation Dominance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace stack-dependent fullscreen solitude with an explicit, shared fullscreen composition policy that filters both presented surfaces and pointer hit testing.

**Architecture:** Keep `FullscreenPresentationState` as the sole owner authority. Derive one bounded `FullscreenCompositionPlan` from that owner, the canonical active scene, existing window-family relationships, workspace visibility, and layer policy. Use the plan for native-frame membership and primary pointer hit testing. Keep the canonical active scene and normal `window_stacking` unchanged.

**Tech Stack:** Rust, Smithay/Wayland, existing Typhon compositor state and integration-test harness.

## Global constraints

- Work directly in the current repository and use `rtk` for repository/search/Cargo commands.
- Keep build artifacts in the repository with `CARGO_TARGET_DIR=target`.
- Do not use subagents.
- Preserve the unrelated existing modification in `src/native_output/tests/input.rs`.
- Do not change Predictive O1, frame scheduling, KMS, explicit sync, pointer constraints, terminal-client lifetime, DMA-BUF, NVIDIA, Proton, Eclipse, or Direct Scanout rules.
- Do not infer fullscreen from application ID or geometry; use the existing authoritative fullscreen owner.
- Do not destructively remove roots from canonical workspace state or normal stacking.

## Task 1: Add the Cyberpunk-shaped RED regression

**Files:** `src/compositor/state/task_05_8_tests.rs`, and only the smallest supporting test helper change if required.

- [ ] Add a deterministic test with ordinary application A, fullscreen application B, and a `Layer::Top` root.
- [ ] Install B as the authoritative fullscreen owner, settle its fullscreen transition, and deliberately raise/reorder A after B in ordinary `window_stacking`.
- [ ] Assert the normal stack still contains A above B, while the desired presented result is B-family only and excludes A and the Top layer.
- [ ] Run the focused test before production changes and record the expected failure from the current stack-dependent solitude helper.
- [ ] Commit the RED regression separately.

## Task 2: Introduce the explicit composition plan

**Files:** `src/compositor/fullscreen.rs`, `src/compositor/state/fullscreen.rs`, `src/compositor/state/scene_order.rs`, `src/compositor/state/task_05_8_tests.rs`.

- [ ] Add compact plan/mode/classification types. Use `Inactive`, `Transitioning`, and `Dominant`; represent explicit allowed application/layer roots, owner-family roots, cull counts, strict solitude, and an above-fullscreen diagnostic reason.
- [ ] Expose the existing canonical window-family resolver to compositor state code without creating a second relationship hierarchy.
- [ ] Classify each active-scene root once using existing popup ownership, subsurface root resolution, `canonical_scene_owner_window_id`, workspace location, `DesktopStackLayer`, and layer-shell `Layer`.
- [ ] Always allow the owner family, including descendants, owner-rooted XDG popups, and valid transient/parent family members.
- [ ] Explicitly allow visible special-workspace applications and product-defined application notification/overlay/above classes; do not allow ordinary stack order by itself.
- [ ] Allow only `Layer::Overlay`; cull `Background`, `Bottom`, and `Top` in dominant fullscreen.
- [ ] Keep unrelated ordinary regular-workspace applications, their ordinary popups, wallpaper, and other global/background roots culled.
- [ ] Make the plan’s membership predicate reusable by render and input paths.
- [ ] Replace metrics’ old binary interpretation with composition-active, transition-pending, allowed-root, culled-root, and reason fields while preserving compatibility fields that existing tests consume.
- [ ] Remove or narrow `has_visible_application_content_outside_fullscreen_owner`; it must no longer decide whether fullscreen culling is globally enabled.

## Task 3: Apply dominance to native presentation

**Files:** `src/compositor/state/fullscreen.rs`, `src/compositor/server.rs` only if the existing command/test plumbing needs a narrow addition, and relevant compositor tests.

- [ ] Build one plan for the active scene resolution used by native frame surface selection.
- [ ] Filter native frame surfaces whenever the plan is `Dominant`, using presentation-family membership rather than normal stack order.
- [ ] Keep `Inactive` broad and keep `Transitioning` broad until the existing physical-settlement invariant is satisfied.
- [ ] Preserve owner-family descendants and explicitly allowed overlays/special-workspace applications.
- [ ] Ensure ordinary root commits cannot reintroduce culled roots into the presented fullscreen scene.
- [ ] Add the ordinary-app restack regression and verify that exiting fullscreen restores the canonical scene and normal stack without reconstruction.

## Task 4: Share membership with hit testing

**Files:** `src/compositor/state/hit_testing.rs`, `src/compositor/state/task_05_8_tests.rs`, and focused integration tests.

- [ ] Add a RED input regression with an unrelated ordinary app and Top layer geometrically above the fullscreen owner.
- [ ] Consult the same plan in pointer scene locality, uncached hit selection, root hit testing, and visual-root target selection where those paths choose a presented target.
- [ ] Skip culled roots before decoration/surface hit evaluation so hidden surfaces cannot win due to canonical stack order.
- [ ] Keep grabbed-surface fallback and pointer-constraint behavior unchanged.
- [ ] Verify owner hits normally, Top is not interactive, and an allowed Overlay can receive the hit.

## Task 5: Add policy regressions

**Files:** `src/compositor/tests/windows.rs`, `src/compositor/tests/layer_shell.rs`, `src/compositor/state/task_05_8_tests.rs`, and existing XWayland fullscreen tests only if the harness makes parity coverage direct.

- [ ] Update the special-workspace regression: closed gives owner-only solitude; open gives owner plus special app with composition active but non-solitary, while ordinary app and Top remain culled.
- [ ] Add/adjust the layer policy regression: Background/Bottom/Top absent, Overlay present, and matching input behavior.
- [ ] Add owner-popup/transient coverage: owner family remains visible, unrelated app remains culled, and a popup may make strict solitude false without disabling composition.
- [ ] Add application-category coverage for explicitly allowed notification/overlay/above classes and prove ordinary `keep above`/restack is not an implicit permission unless existing policy says so.
- [ ] Add focused XWayland parity coverage if existing owner setup is inexpensive; do not create an XDG-specific implementation.
- [ ] Keep the physical-settlement transition tests green and assert the exact `Transitioning` to `Dominant` change.

## Task 6: Verify and hand off

- [ ] Run focused fullscreen state, native-frame, scene-order, hit-testing, workspace, layer-shell, XDG, XWayland, and Direct Scanout tests.
- [ ] Discover and run the repository source-layout gate.
- [ ] Run `CARGO_TARGET_DIR=target rtk cargo fmt --check`.
- [ ] Run `CARGO_TARGET_DIR=target rtk cargo check --locked --all-targets`.
- [ ] Run `CARGO_TARGET_DIR=target rtk cargo clippy --locked --all-targets -- -D warnings`.
- [ ] Run `CARGO_TARGET_DIR=target rtk cargo test --locked`.
- [ ] Run `rtk git diff --check`, inspect the diff, and preserve unrelated worktree changes.
- [ ] Commit implementation/test changes in focused commits and report their hashes.
- [ ] Report any baseline failures separately.
- [ ] Do not claim native Cyberpunk resolution until a real session confirms Steam and Eclipse Top are absent from the presented fullscreen scene; report that hardware validation as remaining if unavailable.

