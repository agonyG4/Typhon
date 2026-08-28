# Typhon Dwindle v1 Layout Authority Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a pure per-`WorkspaceLocation` Dwindle engine and integrate it with Typhon's existing visual, workspace, decoration, scene, and configure authorities.

**Architecture:** `src/wm/layout` contains `LayoutRect`, validated ratios, a generational arena-backed `DwindleTree`, immutable snapshots/plans, and `TiledLayoutManager`. `src/compositor/state/tiled_layout.rs` owns the bridge and batch application; existing state modules call it at membership, lifecycle, mode, minimize, workspace, and work-area boundaries.

**Tech Stack:** Rust, existing Typhon Wayland/XWayland compositor, std collections only for the layout domain, existing Cargo target directory.

## Global Constraints

- Preserve the dirty working tree and do not reset, stash, clean, checkout, create a worktree, or commit.
- Keep `WorkspaceLocation`, `LayoutMembership`, `ToplevelMode`, `ToplevelVisualGeometry`, workspace/Special ownership, and XDG/XWayland lifecycle ownership intact.
- Do not add a generic layout plugin framework, external arena dependency, gaps, animations, tiled-by-default policy, O1/KMS changes, or client barriers.
- Use `rtk` for shell reads, searches, git, formatting, checks, and tests where available.
- Every production behavior change has a failing test first; re-run focused tests after each green cycle.

### Task 1: Pure geometry and ratio values

**Files:** Create `src/wm/layout/geometry.rs`; modify `src/wm/layout/mod.rs`; test in `src/wm/layout/geometry.rs`.

- [ ] Write tests for positive `LayoutRect`, deterministic boundary helpers, and valid/invalid normalized `SplitRatio`.
- [ ] Run `rtk test cargo test wm::layout::geometry` and verify the new tests fail because the module/types are absent.
- [ ] Implement `LayoutRect`, `LayoutPoint`, `SplitAxis`, and `SplitRatio` without compositor dependencies.
- [ ] Run the focused test and verify it passes.

### Task 2: Arena-backed Dwindle tree

**Files:** Create `src/wm/layout/dwindle.rs`; modify `src/wm/layout/mod.rs`; test in `src/wm/layout/dwindle.rs`.

- [ ] Write tests for first/second/third insertion, wide/tall orientation, pointer side, duplicate/unknown removal, sibling promotion, stale node IDs, and invariant validation.
- [ ] Run the focused test and verify expected failures.
- [ ] Implement generational node IDs, arena slots, leaf/split nodes, deterministic insertion, canonical removal, and `debug_validate`.
- [ ] Add minimized subtree collapse in calculation while retaining leaves.
- [ ] Run focused tests plus deterministic stress and verify they pass.

### Task 3: Immutable plans and per-location manager

**Files:** Create `src/wm/layout/plan.rs`; modify `src/wm/layout/mod.rs`; test in `src/wm/layout/plan.rs`.

- [ ] Write tests for immutable plans, exact zero-gap partitioning, location isolation, snapshot minimization, and manager insert/remove/plan behavior.
- [ ] Run the focused tests and verify red failures.
- [ ] Implement `LayoutConstraints`, `LayoutWindowSnapshot`, `TiledWindowTarget`, `TiledLayoutPlan`, and lazy `TiledLayoutManager` APIs.
- [ ] Run focused tests and verify green.

### Task 4: Compositor bridge and batch generation

**Files:** Create `src/compositor/state/tiled_layout.rs`; modify `src/compositor/mod.rs`, `src/compositor/state/mod.rs`, `src/compositor/state/surfaces.rs`, `src/compositor/state/window_resize.rs`, `src/compositor/state/xwayland_mode.rs`; add focused state tests.

- [ ] Write failing tests for one-window layout application, unchanged-target deduplication, and one visible `LayoutReflow` generation.
- [ ] Run those tests and verify red failures.
- [ ] Add the manager, typed generation cause, batch-depth guard, and target translation through existing visual geometry and configure paths.
- [ ] Re-run focused tests and existing visual-geometry tests.

### Task 5: Super+V and lifecycle reconciliation

**Files:** Modify `src/compositor/state/window_actions.rs`, `src/compositor/state/desktop_windows.rs`, `src/compositor/state/windows.rs`; add/extend state integration tests.

- [ ] Write failing Super+V, removal, minimize/restore, and Normal-mode current-target tests.
- [ ] Run focused tests red.
- [ ] Make Super+V transactional, remove leaves before destruction, preserve minimized/fullscreen/maximized topology, and reapply current Tiled plans on Normal restore.
- [ ] Run focused tests and existing lifecycle suites green.

### Task 6: Workspace/Special and work-area integration

**Files:** Modify `src/compositor/state/workspaces.rs`, `src/compositor/state/active_scene.rs`, `src/compositor/layer_shell.rs`; add/extend workspace/Special tests.

- [ ] Write failing source/destination tree and hidden-location tests.
- [ ] Run focused tests red.
- [ ] Plan tree mutations before committing membership, dirty hidden locations, apply visible plans during scene transitions, and reflow active Regular plus visible Special on usable-area changes.
- [ ] Run focused tests and existing workspace/layer-shell suites green.

### Task 7: Minimal chrome, scene bands, and interaction gating

**Files:** Modify `src/compositor/decoration/layout.rs`, `src/compositor/state/window_decoration.rs`, `src/compositor/state/scene_order.rs`, `src/compositor/state/window_interaction.rs`, `src/compositor/state/input_dispatch.rs`; add focused decoration/scene/interaction tests.

- [ ] Write failing Minimal chrome, CSD preservation, X11 extents, scene ordering, and tiled move/resize rejection tests.
- [ ] Run focused tests red.
- [ ] Resolve chrome from canonical layout membership, make all render/hit-test consumers use Minimal, order Tiled below Floating within each location, and reject floating interaction starts for Tiled Normal windows.
- [ ] Run focused tests plus XDG/X11 regression suites green.

### Task 8: Report and validation/reviews

**Files:** Create `REPORT-2026-08-23-typhon-dwindle-v1-layout-authority.md`.

- [ ] Run `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`, `rtk git diff --check`, `bin/check-source-layout`, focused tests, and `rtk test cargo test --locked`.
- [ ] Perform independent correctness/ownership and performance/future-interaction reviews against the brief; fix task-owned issues and rerun affected tests.
- [ ] Write the report with baseline, dirty-tree evidence, architecture, behavior, test/validation output, environment limitations, review results, final status, and explicit future work.
