# Typhon Dwindle v1.1.2 Final Correctness Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the remaining Dwindle correctness issues by making exact-aspect solving arithmetically bounded, making lower bounds genuinely independent, and sequencing successful workspace migration inside one outer layout batch.

**Architecture:** Preserve the existing `LegalDimensionLattice`, pure `TiledLayoutManager`/`FeasibilityContext`, `PreparedTiledMigration`, `PendingTiledResize`, `WindowInteraction`, and `LayoutReflow` architecture. Add a focused pure exact-aspect arithmetic module that solves the equal-aspect lattice intersection with bounded modular arithmetic, route only exact equal-aspect constraints through it, and move migration batch entry before the existing commit/cancel phase while keeping preparation side-effect-free.

**Tech Stack:** Rust, Cargo, existing unit/integration tests, `rtk` command proxy, arena-backed Dwindle layout.

## Global Constraints

- Treat the complete dirty working tree as authoritative; do not reset, restore, stash, clean, delete unrelated files, create a worktree, run `cargo clean`, stage unrelated files, or commit.
- Preserve Dwindle topology, `LayoutMembership`, `WorkspaceLocation`, `ToplevelVisualGeometry`, `WindowInteraction`, `PendingTiledResize`, `PreparedTiledMigration`, `LayoutReflow`, `ActiveScene`, and `SceneWorkIndex`.
- Do not add compositor-wide Performance v1 work or new Dwindle features.
- Do not use an arbitrary attempt cap to return `Infeasible`; exact-aspect arithmetic must remain complete and deterministic.
- Reuse the existing Cargo target/build directory and use `rtk` for inspection and validation.

---

### Task 1: Add bounded exact-aspect lattice arithmetic

**Files:**
- Create: `src/wm/layout/exact_aspect.rs`
- Modify: `src/wm/layout/mod.rs`
- Modify: `src/wm/layout/constraints.rs`
- Test: `src/wm/layout/exact_aspect.rs` and `src/wm/layout/constraints.rs`

**Interfaces:**
- Consumes `LegalDimensionLattice` and the existing `LayoutConstraints` equal `min_aspect`/`max_aspect` representation.
- Produces `ExactAspectResolution { width, height }` plus bounded arithmetic stats for the existing `ClientResolutionStats`.

- [ ] **Step 1: Write failing tests** for odd-width/even-height exact `1:1` disjoint lattices at near-`u32` bounds and for a huge feasible exact `1:1` lattice whose largest result is selected without overflow. Add a test-only assertion that arithmetic probes remain small.
- [ ] **Step 2: Run the focused constraints tests** and confirm the new tests fail because equal-aspect constraints still use interval enumeration or lack the new arithmetic path.
- [ ] **Step 3: Implement modular arithmetic** using reduced rational `p/q`, `u128` intermediates, linear congruence solving, generalized CRT for non-coprime moduli, and an interval calculation for the largest valid `k` in `width=k*p`, `height=k*q`. Recover common rational aspect values such as `1.0`, `4.0/3.0`, and `16.0/9.0` from the existing `f64` API without changing compositor constraint storage.
- [ ] **Step 4: Route equal finite aspect ranges through the arithmetic path** before interval subdivision; retain the existing interval/lattice path for one-sided and non-equal aspect ranges. Count arithmetic probes separately and keep candidate/subdivision counters intact.
- [ ] **Step 5: Run the exact-aspect tests**, the preserved `84x67 -> 80x45` test, and the exhaustive small-domain oracle. Refactor only after all pass.

### Task 2: Correct cheap lower-bound semantics

**Files:**
- Modify: `src/wm/layout/constraints.rs`
- Modify: `src/wm/layout/solve.rs`
- Modify: `src/wm/layout/mod.rs`
- Test: `src/wm/layout/constraints.rs` and `src/wm/layout/solve.rs`

**Interfaces:**
- Consumes the canonical lattice alignment rule.
- Produces independently aligned leaf bounds under a terminology such as `LeafLowerBounds`/`SubtreeLowerBounds`; exact coupled feasibility remains in `FeasibilityContext`.

- [ ] **Step 1: Write failing lower-bound tests** for unconstrained windows (`1x1`), explicit minimums, base/increment alignment, and exact `1:1` without explicit minimums. Assert exact subtree feasibility still rejects an impossible coupled leaf even when its independent lower bounds fit.
- [ ] **Step 2: Run the lower-bound and solver tests** and confirm the exact-aspect lower-bound test exposes the current maximum-area behavior.
- [ ] **Step 3: Replace aspect-coupled `minimum_tile_size()` use** with a documented independent-bound helper that returns the first legal width and height lattice values only. Rename the subtree scalar type and accessors where needed, retaining compatibility aliases only if existing call sites require them.
- [ ] **Step 4: Keep `FeasibilityContext::can_fit()` and split boundary queries authoritative** for aspect/increment coupling; update diagnostics to consume lower bounds without treating them as proof.
- [ ] **Step 5: Run focused layout, exact feasibility, split-range, and lower-bound tests, including the existing nested monotonicity and ratio-override tests.

### Task 3: Sequence migration commit/apply atomically around active resize termination

**Files:**
- Modify: `src/compositor/state/workspaces.rs`
- Modify: `src/compositor/state/tiled_layout.rs` only if a narrowly scoped migration termination helper is required
- Test: `src/compositor/state/tiled_layout_tests.rs` and relevant resize/interaction tests

**Interfaces:**
- Consumes side-effect-free `migrate_tiled_layouts()`, existing `commit_prepared_tiled_migration()`, `apply_prepared_tiled_migration()`, and `LayoutReflow` depth tracking.
- Produces the sequence `PREPARE -> begin outer batch -> terminate invalid interaction -> commit -> membership commit -> apply`, while preserving required terminal resize configure behavior and failed-prepare zero-side-effect behavior.

- [ ] **Step 1: Write a failing active-resize migration regression** that installs a real tiled resize session and interaction, performs a supported workspace move, and records pending resize, topology, membership, configure/generation state before and after.
- [ ] **Step 2: Run the regression** and confirm the current code cancels before the outer batch and exposes an extra intermediate resize generation/configure state.
- [ ] **Step 3: Begin the outer layout batch only after preparation succeeds and before `commit_prepared_tiled_migration()`; preserve the existing nested apply batch and final `finish_layout_reflow_batch()` balance.
- [ ] **Step 4: If the integration test demonstrates an old-tile terminal geometry/configure is still emitted as a distinct transition, add a migration-scoped interaction termination path that preserves the protocol-correct final non-resizing configure while allowing the final migrated geometry to be the applied terminal target.
- [ ] **Step 5: Keep and extend the failed-preparation regression to assert interaction, pending resize, tree, membership, visual geometry, configure queue, and generations remain unchanged.
- [ ] **Step 6: Run focused tiled-layout, workspace migration, window-interaction, resize-flow, XDG terminal-resize, and X11 tiled tests.

### Task 4: Independent reviews, validation, and closure report

**Files:**
- Create: `REPORT-2026-08-24-typhon-dwindle-v1-1-2-final-fix-closure.md`

**Interfaces:**
- Consumes implementation and test evidence from Tasks 1–3.
- Produces the requested English report covering baseline, three bugs, arithmetic method, operation counts, lower bounds, migration sequencing, focused/full validation, source layout, two review passes, final status, and explicit Performance v1 exclusion.

- [ ] **Step 1: Run an independent correctness review** for congruence completeness, overflow, lattice bounds, deterministic maximum area, lower-bound/exact-feasibility separation, prepare side effects, migration order, terminal resize semantics, generation count, and topology/membership consistency.
- [ ] **Step 2: Run an independent boundedness/scope review** for billion-scale exact-aspect scans, Cartesian search, unbounded subdivision, raw-event work, resize cloning, unrelated performance work, and out-of-scope Dwindle features.
- [ ] **Step 3: Run `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`, `rtk git diff --check`, `bin/check-source-layout`, all focused tests, and then `rtk cargo test --locked`.
- [ ] **Step 4: Record known environment/pre-existing failures separately without weakening tests, verify the report has all 21 required content items, and leave the dirty tree untouched with no commit.
