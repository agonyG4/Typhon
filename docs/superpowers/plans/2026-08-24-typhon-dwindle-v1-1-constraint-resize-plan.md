# Typhon Dwindle v1.1 Constraint-Aware Resize Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a pure constraint-aware Dwindle solution and integrate transactional reflow, work-area invalidation, and frame-coalesced split resizing across all existing window backends.

**Architecture:** Keep the WM engine compositor-independent. Split pure constraint solving, client-size resolution, and resize-anchor logic into focused layout modules; make the compositor bridge prepare candidate layouts and apply only complete immutable solutions. Extend the existing `window_interaction`, resize configure flow, visual geometry, layer-shell, workspace, and frame-preparation paths rather than adding parallel authorities.

**Tech Stack:** Rust, existing Typhon Wayland/XWayland compositor, standard library collections and arithmetic, current Cargo target directory, `rtk` command wrapper.

## Global Constraints

- Preserve the dirty worktree; do not reset, restore, checkout over changes, stash, clean, remove unrelated files, create a worktree, run `cargo clean`, stage unrelated files, or commit.
- Preserve `DesktopWindow`, `WindowManagementState`, `WorkspaceLocation`, `LayoutMembership`, `ToplevelVisualGeometry`, existing configure flows, workspace/Special ownership, scene ordering, and O1/KMS scheduling.
- Keep tile partition gap zero; do not add directional focus/movement, gaps, animations, Niri/Infinite Canvas, tiled-by-default policy, or client synchronization barriers.
- Keep `src/wm/layout` pure: only `WindowId` and value types; never store protocol, surface, renderer, or client lifecycle objects.
- Use typed finite ratios with absolute safety range `[0.05, 0.95]`; preserve preferred ratios when constraints temporarily clamp them.
- Raw tiled resize motion is latest-value-only O(1)-style state mutation; only frame preparation may solve/apply/configure.
- Every production behavior change follows a failing focused test first; use `rtk` for reads, searches, git, formatting, checks, and tests.
- Reuse the existing Cargo target/build directory and report environment failures without weakening tests.

## File map

- Create `src/wm/layout/constraints.rs`: validated pure ICCCM constraints, footprint aggregation, legal client-rect resolver, bounded arithmetic helpers.
- Create `src/wm/layout/solve.rs`: O(N) subtree requirement pass, feasible split intervals, immutable `TiledLayoutSolution`, resolved split trace, and typed infeasibility witnesses.
- Create `src/wm/layout/resize.rs`: pure edge-to-ancestor lookup, immutable resize handles, and pointer displacement to requested ratios.
- Modify `src/wm/layout/geometry.rs`, `src/wm/layout/dwindle.rs`, `src/wm/layout/plan.rs`, and `src/wm/layout/mod.rs` to expose 0.05–0.95 ratios, ratio mutation, tree ancestry, client targets, rich errors, and candidate-tree snapshot calculation.
- Create `src/compositor/state/tiled_resize.rs`: one `TiledResizeSession`, one `PendingTiledResize` slot, frame flush, metrics, cancellation, and backend-independent ratio mutation.
- Modify `src/compositor/state/tiled_layout.rs`: solution application, tile/client geometry translation, dirty semantics, candidate migration, constraint/work-area fallback, and dynamic constraint reconciliation.
- Modify `src/compositor/state/frames.rs`, `src/compositor/server.rs`, and `src/compositor/state/scene_work.rs`: expose pending tiled work and flush it before ordinary resize configures.
- Modify `src/compositor/state/window_interaction.rs`, `src/compositor/interaction.rs`, `src/compositor/state/window_resize.rs`, `src/compositor/state/windows.rs`, `src/compositor/state/desktop_windows.rs`, `src/compositor/state/workspaces.rs`, `src/compositor/layer_shell.rs`, and `src/compositor/state/surfaces.rs` to route lifecycle, constraints, migration, work-area changes, and terminal/cancel behavior.
- Modify `src/compositor/desktop_window.rs`, `src/xwayland/xwm/icccm.rs`, `src/compositor/protocols/xdg.rs`, `src/xwayland/xwm/commands.rs`, `src/xwayland/xwm/events.rs`, and `src/compositor/server_xwayland_events.rs` only where existing constraint/configure requests need the shared semantics or tiled routing.
- Modify `src/compositor/decoration/layout.rs`, `src/compositor/state/window_decoration.rs`, `src/compositor/state/hit_testing.rs`, and `src/compositor/state/scene_order.rs` for Minimal logical resize zones and cursor/scene authority.
- Create focused `src/compositor/state/tiled_layout_tests.rs` and `src/compositor/tests/xwayland_tiled.rs`; move task-owned Dwindle tests from oversized generic modules without deleting coverage.
- Create `REPORT-2026-08-23-typhon-dwindle-v1-1-constraint-resize.md` with the 37 required sections.

### Task 1: Pure validated constraints and legal client rectangles

**Files:** Create `src/wm/layout/constraints.rs`; modify `src/wm/layout/mod.rs`, `src/wm/layout/plan.rs`, `src/compositor/state/window_resize.rs`, and `src/xwayland/xwm/icccm.rs`; test in `constraints.rs` and existing ICCCM tests.

- [ ] Add failing tests for invalid min/max, zero increments, NaN/infinite/non-positive/reversed aspect, fixed size, max size, base plus increments, min/max aspect, increment plus aspect, impossible combinations, and idempotence through the existing X11 clamp.
- [ ] Run `rtk test cargo test --lib wm::layout::constraints` and the ICCCM focused tests; confirm failures are missing pure APIs/behavior rather than test setup errors.
- [ ] Implement a value-only validated constraint type or normalization function and a bounded `resolve_client_rect_within_tile(tile, constraints)` that aligns dimensions to `base + N * increment`, respects hard bounds/aspect, terminates after a fixed iteration cap, and returns a typed infeasible error.
- [ ] Expand `LayoutConstraints` with all ICCCM fields and use `PartialEq` rather than `Eq` where aspects require it; make snapshot validation deterministic.
- [ ] Extract or delegate the existing X11/floating clamp's low-level arithmetic to the same neutral helper, preserving its public behavior and minimum fallback constants.
- [ ] Run the focused tests and existing `window_resize`/`xwayland::xwm::icccm` suites; keep output free of new warnings.

### Task 2: O(N) requirements, solution trace, tile/client targets

**Files:** Create `src/wm/layout/solve.rs`; modify `src/wm/layout/plan.rs`, `src/wm/layout/dwindle.rs`, and `src/wm/layout/mod.rs`; test in pure layout modules.

- [ ] Add failing tests for two-child feasible boundaries (700/500 in width 1920), nested H/V complete-subtree requirements, temporary preferred-ratio clamp and restoration, max/fixed client rectangles centered in unchanged tiles, impossible-root witnesses, and exact tile partitioning.
- [ ] Run `rtk test cargo test --lib wm::layout::plan` and solver-focused tests; verify the expected missing-solver failures.
- [ ] Add `SubtreeRequirements`, `SplitRatioRange`, `ResolvedSplit`, `TiledLayoutSolution`, and a typed `ConstraintInfeasibility` witness carrying split/axis/required/available and contributing windows.
- [ ] Implement one bottom-up active-leaf requirement pass and one top-down resolution pass. Derive boundary intervals directly from subtree widths/heights, intersect safety bounds, clamp effective ratios, and never mutate preferred ratios during calculation.
- [ ] Change `TiledWindowTarget` to expose `tile` and legal `client` rectangles. Keep inactive minimized leaves out of updates while retaining topology.
- [ ] Make `TiledLayoutManager::calculate` return the rich solution (or provide a compatibility wrapper only where the compositor still needs a plan), and ensure no partial solution is exposed on error.
- [ ] Run pure solver/client-rect tests, geometry tests, and the compositor compile check before continuing.

### Task 3: Tree ratio mutation, resize handles, and deterministic stress

**Files:** Create `src/wm/layout/resize.rs`; modify `src/wm/layout/dwindle.rs`, `src/wm/layout/geometry.rs`, and `src/wm/layout/mod.rs`; test in `resize.rs` and `dwindle.rs`.

- [ ] Add failing tests for right/left/bottom/top nearest-ancestor selection, non-adjustable outer edges, corner handles, start-parent pointer conversion without cumulative drift, clamped ratio writes, and topology-stale handles.
- [ ] Add a fixed-sequence stress test with thousands of insert/remove/ratio/minimize/solve/root-size operations; assert `debug_validate()` after every topology mutation and exercise stale IDs.
- [ ] Run the new tests red.
- [ ] Implement typed `TiledResizeAxis` handles, ancestor traversal using the exact child-side rules, `set_split_ratio` validation, and pure requested-ratio conversion from interaction-start boundary plus total pointer displacement.
- [ ] Use checked/saturating arithmetic and keep the tree's preferred ratio canonical; reject stale IDs without mutation.
- [ ] Run all pure layout tests green and inspect allocation/complexity-sensitive paths for no per-event tree solve.

### Task 4: Compositor solution bridge and exact tile/client authority

**Files:** Modify `src/compositor/state/tiled_layout.rs`, `src/compositor/mod.rs`, `src/compositor/state/surfaces.rs`, `src/compositor/state/xwayland_mode.rs`, and `src/compositor/state/windows.rs`; test in focused state modules.

- [ ] Add failing tests for legal client geometry inside a larger tile, one-solve/one-generation multi-window application, unchanged-target deduplication, and Normal restore preferring the current client target.
- [ ] Run focused compositor tests red.
- [ ] Translate `TiledWindowTarget.client` into compositor `WindowGeometry` while preserving tile partitioning for neighboring windows; make XDG/X11 configure and `ToplevelVisualGeometry` use the legal client target.
- [ ] Apply only complete solutions. Keep hidden locations unconfigured, clear dirty state only after visible success, and retain one outer `LayoutReflow` batch for all affected locations.
- [ ] Preserve saved floating geometry and existing mode/minimize behavior; make current tiled geometry read from the solved client target.
- [ ] Run focused tiled layout, visual geometry, XDG, X11, minimize/fullscreen/maximize tests.

### Task 5: Work-area centralization and hidden dirty semantics

**Files:** Modify `src/compositor/layer_shell.rs`, `src/compositor/state/surfaces.rs`, `src/compositor/state/tiled_layout.rs`, `src/compositor/state/workspaces.rs`, `src/compositor/state/scene_work.rs`; test in new focused layout tests and layer/workspace suites.

- [ ] Add failing tests for Dock exclusive-zone changes at unchanged output resolution, visible Regular plus Special one-batch reflow, hidden Regular dirty/no-configure, and activation solving the latest root once.
- [ ] Run focused layer-shell/workspace tests red.
- [ ] Add one shared usable-area-change helper called from output-size and layer arrangement flows. Batch stateful mode reconfiguration, visible Dwindle solves, and one final scene/render generation.
- [ ] Mark every hidden tree dirty on root change; visible successful solves clear their location. Activation/Special opening only reflows dirty or unresolved locations.
- [ ] Add work-area infeasibility fallback using solver witnesses and most-recent insertion order, downgrading only necessary leaves while preserving one fitting Tiled window where possible.
- [ ] Run work-area, layer-shell, workspace, Special, scene-order, and generation tests.

### Task 6: Transactional insertion/migration and dynamic constraints

**Files:** Modify `src/compositor/state/tiled_layout.rs`, `src/compositor/state/workspaces.rs`, `src/compositor/state/windows.rs`, `src/compositor/state/desktop_windows.rs`, `src/compositor/desktop_window.rs`, and `src/compositor/mod.rs`; test in focused state modules.

- [ ] Add failing tests for infeasible Super+V insertion, candidate migration with feasible existing destination and infeasible incoming family, no partial topology/location commit, Tiled-without-Leaf prevention, dynamic XDG clamp/auto-float, dynamic X11 constraint reconciliation, and typed fallback metrics/reasons.
- [ ] Run the focused tests red.
- [ ] Introduce prepared candidate migration state: clone manager, remove/insert candidate leaves, snapshot candidate tree membership directly, solve all affected trees, decide incoming-only fallbacks, then commit topology/location/membership atomically.
- [ ] Add `TiledFallbackReason` and use it for insertion, migration, constraint-update, and work-area fallback paths; preserve floating restore geometry and hidden quiescence.
- [ ] Extend pending XDG constraints to all fields and reconcile after canonical update; extend X11 `Constraints` delta with the same helper and auto-float only the culprit when its new constraints make the tree impossible.
- [ ] Run migration, workspace/Special, XDG, X11, lifecycle, and scene tests.

### Task 7: Coalesced Tiled resize lifecycle and metrics

**Files:** Create `src/compositor/state/tiled_resize.rs`; modify `src/compositor/state/window_interaction.rs`, `src/compositor/interaction.rs`, `src/compositor/state/frames.rs`, `src/compositor/server.rs`, `src/compositor/state/scene_work.rs`, `src/compositor/mod.rs`, and resize-flow modules; test in focused interaction/frame modules.

- [ ] Add failing tests for tiled move rejection/resize acceptance, no raw-event solve, latest-value replacement after 1000 motions, one solve per frame, constraint stop deduplication, corner one-batch mutation, release-before-frame final flush, cancellation discard, topology/work-area/constraint invalidation, generation delta, and metrics.
- [ ] Run the focused interaction/frame tests red.
- [ ] Add pure-valued `TiledResizeSession` and exactly one `PendingTiledResize` slot to the canonical interaction state. Capture handles, start parent rects, start ratios, and stable location/topology/constraint identity at begin.
- [ ] Route raw motion to O(1) pending ratio replacement only. Add `has_pending_frame_prepare_work()` awareness and flush pending tiled resize before ordinary configure flushing, with one combined solve/apply batch.
- [ ] Reuse `ResizeInteractionId`, `ResizeConfigureFlow`, capacity limits, owner Resizing state, and terminal configure semantics. Siblings receive ordinary layout configures; unchanged targets receive none.
- [ ] On release force-flush the latest pending intent, apply final geometry, send the owner terminal configure without Resizing, clear the slot/session, and refresh pointer focus once. On non-user cancellation discard unflushed intent and retain the last applied ratio.
- [ ] Run focused interaction, resize-flow, frame-preparation, XDG, X11, ToplevelVisualGeometry, minimize/fullscreen/maximize, workspace/Special cancellation tests.

### Task 8: Backend routing and Minimal/CSD edge interaction

**Files:** Modify `src/compositor/protocols/xdg.rs`, `src/xwayland/xwm/commands.rs`, `src/xwayland/xwm/events.rs`, `src/compositor/server_xwayland_events.rs`, `src/compositor/state/xwayland_windows.rs`, `src/compositor/decoration/layout.rs`, `src/compositor/state/window_decoration.rs`, `src/compositor/state/hit_testing.rs`, and `src/compositor/state/window_interaction.rs`; add `src/compositor/tests/xwayland_tiled.rs` and focused decoration tests.

- [ ] Add failing tests for XDG resize on CSD Tiled windows, X11 `_NET_WM_MOVERESIZE` resize versus move, SSD Minimal adjustable/non-adjustable edges and cursor axis, no titlebar/move affordance, and no direct geometry escape.
- [ ] Run focused backend/decoration tests red.
- [ ] Route accepted resize requests into the same Tiled split begin path; reject/no-op if no corresponding ancestor and never fall back to floating geometry. Preserve button/serial/staleness checks.
- [ ] Make Minimal hit testing expose only logical adjustable split edges/corners; derive cursor from actual adjustable axes and preserve CSD client pixels without fabricating SSD.
- [ ] Extract task-owned Tiled XWayland authority tests from oversized generic test files, wire the focused module, and keep coverage unchanged.
- [ ] Run backend, decoration, scene-order, and existing floating regression suites.

### Task 9: Source-layout cleanup, stress evidence, report, and validation

**Files:** Create focused test modules and `REPORT-2026-08-23-typhon-dwindle-v1-1-constraint-resize.md`; modify only task-owned oversized modules needed to move tests.

- [ ] Add synthetic pure solves for 1, 10, 50, 100, and 500 windows with operation/visit/solve counters and no wall-clock assertions.
- [ ] Run focused tests for geometry, Dwindle, solver, client rectangles, resize anchors, stress, tiled bridge, work-area/layer-shell, constraints, migration, WindowInteraction, resize flow, visual geometry, Minimal hit testing, XDG/X11 routing, mode/lifecycle regressions, and scene ordering.
- [ ] Extract Dwindle lifecycle tests to `src/compositor/state/tiled_layout_tests.rs` and Tiled XWayland tests to `src/compositor/tests/xwayland_tiled.rs`; run `rtk run bin/check-source-layout` and fix task-owned limit violations without raising limits or using `include!`.
- [ ] Perform Review Pass 1 for correctness/transaction ownership, fix every task-owned finding, and rerun affected focused tests.
- [ ] Perform Review Pass 2 for raw-input/frame hot paths, allocation/complexity, coalescing, dirty semantics, and future-feature boundaries; fix every task-owned finding and rerun performance/coalescing tests.
- [ ] Run fresh `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`, `rtk git diff --check`, `rtk run bin/check-source-layout`, and `rtk run cargo test --locked`; record exact counts and environment/pre-existing failures.
- [ ] Write the report sections required by the brief: baseline, dirty tree, architecture, pre-flight bugs, solver/client/migration/work-area/resize design, backend/generation/configure/metric evidence, stress/synthetic evidence, source cleanup, validation, limitations, both reviews, final status, and roadmap.
- [ ] Verify final `rtk git status --short` and do not claim completion until every required command has fresh evidence.
