# macOS Presentation Animation Policy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add one macOS-style geometry-animation policy layer and route explicit animation kinds through Typhon's existing presentation transition engine without changing presentation authority, canonical geometry ownership, or pointer interaction semantics.

**Architecture:** Keep `PresentationAnimator` generic. Add `PresentationAnimationKind`, `PresentationAnimationStyle`, and `PresentationAnimationPolicy` to `src/presentation_animation.rs`; store one policy on `CompositorState`; make the compositor animation entry point require a kind; and route mode, layout, managed XWayland, and compositor-requested geometry paths through animation-capable visual installers. Non-scope setup/output/client paths use the non-animated installer wrapper.

**Tech Stack:** Rust, existing `AnimationCurve`/`SpringSpec` presentation engine, compositor state modules, Rust unit/integration tests, Cargo locked verification, `rtk` command wrapper.

## Global Constraints

- Work in `/home/agony/GitHub/Typhon` and reuse its existing `target/` directory.
- Use `rtk` for repository commands and test/build commands.
- Do not create a second build directory or animation timer/thread.
- Do not use subagents or a separate worktree; execute this plan inline.
- Preserve unrelated in-progress renderer/effects files and stage only files belonging to this task.
- Keep all documentation and comments in English.
- Do not reset, checkout, or discard existing user changes.
- Commit the design, plan, implementation, and report changes as focused git commits.

## 1. Freeze the baseline and inspect the implementation seam

- [x] Record `rtk git status --short`, the current `HEAD`, and the existing focused presentation-animation test result.
- [x] Confirm the hardcoded spring is still only at the geometry entry point and list every visual-geometry installer caller with `rtk rg`.
- [x] Treat the existing renderer/effects modifications as unrelated and do not include them in any stage.

## 2. RED: add policy-model tests

**Files:** `src/presentation_animation.rs`

- [x] Add a test table covering all eight `PresentationAnimationKind` values and the expected macOS `(stiffness, damping)` pairs from the design.
- [x] Add tests that unset/empty/`default`/`macos` style values resolve to the macOS preset and unknown values fall back to it.
- [x] Add a test that `PresentationAnimationPolicy::curve_for` is the single semantic lookup used by the model.
- [x] Run the new filtered tests and verify they fail because the policy types/parser do not yet exist.

## 3. GREEN: implement the centralized policy and state ownership

**Files:** `src/presentation_animation.rs`, `src/compositor/mod.rs`

- [x] Add `PresentationAnimationKind` with the eight explicit variants, plus copy/debug/equality derives.
- [x] Add `PresentationAnimationStyle` with the initial `Macos` style and a parser for `OBLIVION_ONE_ANIMATION_STYLE` (`default` aliases `macos`, unknown values diagnose and fall back).
- [x] Add `PresentationAnimationPolicy` with `Default`, environment construction, and one `curve_for` match containing the macOS spring table.
- [x] Add the smallest test-only animator inspection seam needed to assert routed curve selection without exposing mutable transition state.
- [x] Store `presentation_animation_policy: PresentationAnimationPolicy` beside `presentation_animator` in `CompositorState` and re-export the policy types from `compositor`.
- [x] Run the policy tests and the existing `presentation_animation` tests.

## 4. RED/GREEN: make the entry point explicit

**Files:** `src/compositor/state/active_scene.rs`, `src/compositor/state/window_resize.rs`, `src/compositor/state/xwayland_mode.rs`, affected tests

- [x] Add/update a focused test that invokes the entry point with an explicit kind and observes the corresponding policy curve.
- [x] Change `animate_toplevel_visual_geometry` to accept `PresentationAnimationKind`.
- [x] Replace the local `SpringSpec::new(180.0, 24.0)` with `self.presentation_animation_policy.curve_for(kind)`.
- [x] Preserve the existing time selection, pointer-owned cancellation, start/retarget continuity, and no-geometry cancellation branches exactly.
- [x] Add animation-capable installer variants accepting `Option<PresentationAnimationKind>`; keep a no-policy wrapper for setup/output/test callers and cancel stale transitions on explicit opt-out.
- [x] Run focused animation-entry and geometry tests.

## 5. Wire tiled/Dwindle reflow

**Files:** `src/compositor/state/tiled_layout.rs`, `src/compositor/state/surfaces.rs`, `src/compositor/state/tiled_layout_tests.rs`

- [x] Add a RED assertion that a solved tiled geometry starts with the `LayoutReflow` curve.
- [x] Pass `Some(PresentationAnimationKind::LayoutReflow)` for XDG and managed XWayland geometry updates in `apply_layout_geometry`.
- [x] Preserve the existing `layout_animation_epoch`, one solve, configure count, canonical geometry, and active tiled-resize ownership behavior.
- [x] Keep output-size reconciliation non-animated and update fixture calls to the no-policy wrapper where needed.
- [x] Run tiled-layout and focused presentation tests.

## 6. Wire maximize/fullscreen mode transitions

**Files:** `src/compositor/state/windows.rs`, `src/compositor/state/fullscreen.rs`, `src/compositor/state/window_decoration_tests.rs`, `src/compositor/state/task_05_8_tests.rs`, relevant frame/fullscreen tests

- [x] Add RED assertions for maximize enter/exit and fullscreen enter/exit curve kinds.
- [x] Determine the previous mode before mutating it and select the matching enter/exit kind explicitly.
- [x] Pass the selected kind to the XDG visual installer for mode entry and restore.
- [x] Use `LayoutReflow` when a restore path delegates to a tiled solve; do not double-start a mode animation.
- [x] Keep fullscreen presentation-owner, native membership, culling, and physical settlement code unchanged.
- [x] Run focused mode, decoration, fullscreen, and frame tests.

## 7. Wire managed XWayland transitions and non-interactive floating geometry

**Files:** `src/compositor/state/xwayland_mode.rs`, `src/compositor/state/desktop_windows.rs`, `src/compositor/state/resize.rs`, `src/compositor/state/windows.rs`, XWayland tests

- [x] Add RED assertions for `XwaylandModeChange` and for pure non-interactive managed floating move/resize routing.
- [x] Pass `Some(XwaylandModeChange)` only from `transition_x11_window_mode`.
- [x] Keep initial X11 state and client/setup geometry non-animated.
- [x] In the non-interactive managed X11 geometry path, classify a pure position update as `ProgrammaticMove` and a size update as `ProgrammaticResize`; leave pointer-owned resize paths uncoupled.
- [x] Extend the compositor-requested floating resize target path to install the requested temporary visual geometry with `ProgrammaticResize` after sending its configure, while retaining configure/commit ownership and takeover cancellation.
- [x] If a target path cannot safely retain geometry until client commit, leave it non-animated and document the limitation rather than introducing retention.
- [x] Run XWayland geometry/mode tests and focused resize tests.

## 8. RED/GREEN: preserve pointer takeover and transition invariants

**Files:** `src/compositor/state/active_scene.rs`, `src/compositor/state/window_interaction.rs`, `src/compositor/state/window_interaction_tests.rs`, `src/compositor/state/task_05_8_tests.rs`, existing native-output tests

- [x] Add or extend tests proving an active move cancels any pending programmatic transition before pointer geometry is authoritative.
- [x] Add or extend tests proving an active resize cancels/takes over without a spring under the pointer.
- [x] Re-run tests covering retarget continuity, exact transition acknowledgement, hidden-transition dormancy, direct-scanout blockers, fullscreen-entry culling, and native-frame membership.
- [x] Do not add a second sample site, timer, thread, layout tree, or lifecycle retention mechanism.

## 9. Update the report and perform final verification

**Files:** `docs/reports/2026-09-08-typhon-presentation-animation-closure.md`

- [x] Add an English dated section describing the policy model, environment selector, routed kinds, preserved invariants, tests, and any paths intentionally left out of scope.
- [x] Run `rtk run -- cargo fmt --check`.
- [x] Run `rtk run -- cargo check --locked --all-targets`.
- [x] Run `rtk run -- cargo clippy --locked --all-targets -- -D warnings`.
- [x] Run `rtk run -- cargo test --locked`.
- [x] Run `rtk git diff --check`.
- [x] Run `rtk run -- bash bin/check-source-layout` and report existing oversized-file debt honestly.
- [x] Re-run codebase-memory coverage for every operated source/report path and inspect any newly reported gap directly.
- [x] Review the final diff, stage only this task's files, and commit with a focused message. Leave unrelated renderer/effects changes unstaged.

## Expected completion evidence

- One centralized macOS spring table and explicit semantic kind at every animated geometry entry point.
- No pointer-owned geometry animation.
- Existing presentation/physical-settlement/fullscreen/direct-scanout invariants remain covered by passing regressions.
- Any all-target or source-layout failure caused by unrelated work or known repository debt is recorded rather than masked.

## v1.1 tuning follow-up (2026-09-09)

This additive follow-up keeps the v1 spring stiffness/damping table and tunes
only policy-owned settlement semantics and route coverage.

- [x] Keep the eight v1 stiffness/damping pairs unchanged and retain the
  generic `SpringSpec::new` settlement defaults.
- [x] Add policy-owned `0.5 px` displacement and `8 px/s` velocity settlement
  constants through one shared macOS spring helper.
- [x] Add immutable `SpringSpec` settlement getters and extend policy tests to
  assert kind, stiffness, damping, displacement epsilon, and velocity epsilon.
- [x] Add deterministic actual-sampler tests for all eight kinds at `t=0`, an
  early timestamp, and a final exact-target/zero-velocity sample by `800 ms`.
- [x] Record the representative deterministic settlement envelope for 200,
  500, 1,000, and 1,920 px displacements.
- [x] Add real XDG maximize/fullscreen enter and exit route regressions and
  retain the existing real XWayland route regression.
- [x] Add a real tiled/Dwindle `reflow_tiled_location` route regression.
- [x] Update the closure report with the rationale, values, envelope, route
  coverage, preserved physical-settlement invariants, and verification.
- [x] Run the locked format, check, test, diff, source-layout, and coverage
  checks; the library-only `-D warnings` clippy check passes.
- [x] Record the all-target clippy limitation: three unrelated EGL renderer
  dead-code warnings are promoted to errors by `-D warnings`.
- [x] Record the full `cargo fmt --check` limitation: unrelated existing
  formatting in renderer/state files; direct `rustfmt --check` passes for all
  task-owned Rust files.
- [x] Record that live 165 Hz hardware qualification was not performed.
- [ ] Retune stiffness/damping from live hardware evidence; deferred because
  no hardware qualification data is available in this task.
