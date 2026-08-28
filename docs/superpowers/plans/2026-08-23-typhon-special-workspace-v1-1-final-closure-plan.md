# Typhon Special Workspace v1.1 Final Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the remaining Special Workspace v1.1 correctness, fullscreen composition, interaction lifecycle, visible-work, generation, and task-owned source-layout regressions while preserving the current WM/ActiveScene/SceneWork architecture and dirty working tree.

**Architecture:** Treat Special open/close as a derived `ActiveSceneSelection` transition. Keep `WorkspaceLocation` as canonical WM membership, use the existing canonical parent/transient owner resolver for auxiliary application trees, and keep `SceneWorkIndex` as typed derived bookkeeping. Make fullscreen metrics and native surface selection consume one ActiveScene-aware solitary-application predicate.

**Tech Stack:** Rust, Smithay/Wayland compositor state, existing Typhon test fixtures, Cargo locked builds, `rtk` command wrapper.

## Global Constraints

- Preserve every existing dirty-tree edit, deletion, and untracked artifact.
- Do not add a workspace identity, scene graph, lifecycle owner, layout model, thread, timer, lock, or O1/KMS redesign.
- Do not encode Special as a regular `WorkspaceId`, EWMH desktop, or ext-workspace handle.
- Do not change geometry except the canonical terminal resize configure for an already-active departing resize owner.
- Do not scan hidden global work from active callback-only predicates.
- Add tests before implementation for each behavioral fix; use focused red/green runs.
- Do not weaken tests because of SUN_LEN or other environment failures.

---

## Task 1: Freeze the baseline and design boundary

**Files:** `docs/superpowers/specs/2026-08-23-typhon-special-workspace-v1-1-final-closure-design.md`, this plan, `REPORT-2026-08-23-typhon-special-workspace-v1-1-final-closure.md`.

- [x] Record HEAD, dirty status, diff stat, and recent history.
- [x] Inspect the current Special, ActiveScene, fullscreen, interaction,
  callback, X11 owner, and source-layout code.
- [x] Record the architectural freeze and exact invariants in the design.
- [ ] Keep the final report until implementation and validation evidence exist.

## Task 2: Fix composited fullscreen culling first

**Files:** `src/compositor/state/fullscreen.rs` and the narrowest existing
fullscreen/native scene test modules.

- [ ] Add a regression test that constructs regular fullscreen application A,
  Special application S, opens Special, and asserts A remains fullscreen, S is
  in the native renderable scene, solitary composition is false, and Direct
  Scanout remains rejected.
- [ ] Run the focused test against the current implementation and capture the
  red failure.
- [ ] Add one focused predicate for visible application content outside the
  fullscreen owner's canonical root. Exclude layer-shell overlay roots from
  application-content classification without treating Special as a layer.
- [ ] Make `fullscreen_render_plan_metrics()` and
  `native_frame_renderable_surfaces()` use that same result.
- [ ] Add/retain close-Special coverage proving fullscreen protocol mode and
  geometry/configure counts remain unchanged while solitary eligibility can
  recover.
- [ ] Run the focused fullscreen/direct-scanout/native-frame tests green.

## Task 3: Make Special toggle a precise scene transition

**Files:** `src/compositor/state/active_scene.rs`,
`src/compositor/state/workspaces.rs`, existing interaction/focus tests, and
the smallest relevant test modules.

- [ ] Add `ActiveSceneUpdate` with independent selection and visual-change
  results; compare ordered surfaces, popup IDs, and origins at the cache
  boundary.
- [ ] Add selection visibility helpers that classify a canonical managed owner
  against old and new selections without mutating WM membership.
- [ ] Add failing tests for empty Special generation delta zero, populated
  open/close delta one, hidden-only reorder delta zero, and no duplicate
  rebuild/generation.
- [ ] Add failing interaction tests for Special close during resize/move and
  pointer/grab ownership where fixtures permit; add a regular-interaction
  survival test.
- [ ] In Special toggle, capture old selection, mutate the manager, classify
  departing canonical owner IDs from old/new selection, run canonical terminal
  cleanup before rebuild, rebuild once, reconcile focus/pointer/idle, and
  advance render generation only for a visual change.
- [ ] Preserve exclusive layer-shell focus and restore regular fallback when a
  focused Special application departs.
- [ ] Run focused ActiveScene, workspace, interaction, resize, move, pointer
  constraint, grab, layer focus, and geometry/configure tests green.

## Task 4: Close canonical ordering and visible-work gaps

**Files:** `src/compositor/state/workspaces.rs`,
`src/compositor/state/subsurfaces.rs`, `src/compositor/state/frame_callbacks.rs`,
`src/compositor/state/scene_work.rs`, and focused X11/callback tests.

- [ ] Add an auxiliary X11 tree regression proving a Special-owned auxiliary
  root receives the Special band through the canonical owner helper, while a
  regular auxiliary root remains regular and both stay below layer-shell Top/
  Overlay.
- [ ] Change `renderable_root_stack_key()` to consume the existing canonical
  `SceneWorkOwner` result rather than reading auxiliary management directly.
- [ ] Add callback-only regressions with a visible callback plus hidden
  explicit-sync/surface-tree work, and with visible prepare work.
- [ ] Replace global explicit-sync/tree emptiness checks with the existing
  visibility-aware indexed prepare predicate; retain visible resize/color/
  feedback guards.
- [ ] Verify SceneWorkIndex rebuild/migration remains event-driven and typed,
  with no lifecycle ownership or hidden per-frame scan.
- [ ] Run focused ordering, X11 inheritance, callback, explicit-sync,
  surface-tree, presentation-feedback, and SceneWorkIndex tests green.

## Task 5: Repair only task-owned source-layout regressions

**Files:** `src/compositor/state/scene_order.rs` (new),
`src/compositor/state/subsurfaces.rs`, `src/compositor/state/mod.rs`,
`src/native_output/tests/special_workspace.rs` (new),
`src/native_output/tests/input.rs`, `src/native_output/tests/mod.rs`.

- [ ] Extract focused renderable root/tree ordering methods into
  `scene_order.rs` without changing lifecycle behavior.
- [ ] Move the Special native binding regression to the focused test module;
  preserve its assertions and register the module.
- [ ] Run `bin/check-source-layout`; distinguish these fixed regressions from
  unrelated historical violations.
- [ ] Run formatting and focused tests after extraction.

## Task 6: Independent reviews, validation, and report

- [ ] Review Pass 1: correctness/ownership. Check fullscreen agreement,
  terminal resize/move cleanup, pointer/grab ownership, focus/layer authority,
  canonical auxiliary bands, geometry/protocol invariants, and no duplicate
  owner resolver. Fix every task-owned issue found and rerun affected tests.
- [ ] Review Pass 2: hot path/future tiled. Check hidden-work scans, per-frame
  parent walks, allocations, duplicate rebuilds/generations, index authority,
  and future Regular/Special Tiled/Dwindle compatibility. Fix every task-owned
  issue found and rerun affected tests.
- [ ] Run `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`,
  `rtk git diff --check`, `bin/check-source-layout`, focused tests, and
  `rtk cargo test --locked`.
- [ ] Create the final English report with baseline, dirty-tree summary,
  architecture, bugs, rules, cleanup, ordering, callback/index semantics,
  generation/configure evidence, tests, validation, environment failures,
  both review passes, final status, and explicit Hybrid/Tiled/Dwindle scope.
- [ ] Recheck final status without staging or committing.
