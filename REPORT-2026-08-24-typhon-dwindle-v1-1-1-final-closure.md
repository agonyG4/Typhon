# Typhon Dwindle v1.1.1 — Final Closure Report

## 1. Baseline HEAD

The implementation started at `9d3fb34b45f6ce4ffc4582c3231e220b3643e959`.

## 2. Initial dirty-tree summary

The checkout already contained the complete v1/v1.1 dirty working tree: 61 tracked paths were modified or deleted, and multiple reports, design files, compositor modules, tests, and Dwindle sources were untracked. Existing changes were preserved. No reset, restore, stash, clean, worktree, or commit was performed.

## 3. Exact v1.1 architecture discovered

The implementation uses one arena-backed generational `DwindleTree` per `WorkspaceLocation`, canonical `LayoutMembership`, separate `tile_rect` and `client_rect`, `TiledLayoutSolution`, `ToplevelVisualGeometry`, `WindowInteraction`, hidden-location dirty tracking, and one logical `LayoutReflow` batch for visible mutations.

## 4. Confirmed 80x45 false-infeasible bug

The old resolver could reject the legal `84x67` tile case with width `8 + N*3`, height base `5`, and exact `16:9`. The valid result is `80x45`. The named regression now asserts that exact result.

## 5. Old heuristic root cause

`resolve_client_rect_within_tile()` previously examined a small seed set and iterated aspect correction with `MAX_ASPECT_ITERATIONS`. That was neither exhaustive nor a sound infeasibility proof, so a legal lattice point could be skipped.

## 6. LegalDimensionLattice design

`src/wm/layout/lattice.rs` centralizes lower and upper bounds, the base anchor, increment step, align-up, align-down, containment, cardinality, and indexed values. When base is absent, the canonical anchor is the effective lower bound. When base is below min, values begin at the first aligned value at or above min.

## 7. Complete client-size algorithm

The production resolver builds finite width and height lattices. Without aspect constraints it uses direct O(1)-style alignment. With aspect constraints it enumerates only the smaller lattice dimension, derives the other dimension's legal aspect interval analytically, aligns directly to its largest legal value, and uses complete interval subdivision for large ranges. There is no Cartesian width-by-height search and no correctness iteration cap.

## 8. Deterministic maximum-area policy

The selected legal pair maximizes `width * height`; equal-area pairs use deterministic lexicographic width/height ordering. The resulting legal client rectangle is centered in the tile with integer floor offsets. Previous buffer geometry is not an input.

## 9. Exhaustive differential oracle

The constraint tests include a deterministic small-domain brute-force oracle covering base/increment variants, min/max, fixed sizes, one- and two-axis increments, min-only/max-only/exact aspect, and aspect plus increments. Production is compared for existence and exact maximum-area result, not merely `is_some()`.

## 10. Scalar-requirement limitation

`SubtreeRequirements` remains as a cheap independent lower-bound and diagnostic model. It is explicitly not the correctness authority for coupled aspect/increment feasibility.

## 11. Exact subtree feasibility architecture

`solve.rs` now provides pure `subtree_fits`, `minimum_width_for_height`, and `minimum_height_for_width` queries. Recursive split feasibility asks exact child extent queries; leaves use the complete client resolver. Minimized leaves are inactive, and no compositor state enters the pure feasibility layer.

## 12. Exact split feasible-range algorithm

For each split, the solver calculates exact first/second child minima at the shared orthogonal extent, intersects those boundaries with the absolute safety range, and publishes only that interval. The final chosen boundary is clamped into the actual integer boundary interval before child rectangles are created.

## 13. Memoization strategy

Width-at-height and height-at-width minimum queries use solve-local caches keyed by node and fixed orthogonal extent. A failed query at a too-small cap is not persisted as globally impossible. No cache survives a solve, topology change, or constraint generation.

## 14. Preferred/effective ratio behavior

Canonical tree ratios remain preferred user ratios. `ResolvedSplit.effective_ratio` is the current feasible ratio. Temporary constraint or work-area restrictions do not overwrite the preferred ratio. Tiled resize commits only the effective ratio reached after a successful solve.

## 15. Zero-copy RatioOverride resize

`RatioOverride` is derived input to `calculate_tree_with_ratio_overrides()`. Tiled resize flush creates at most two overrides, solves the canonical active tree, and commits affected ratios only after success. `flush_pending_tiled_resize()` contains no `TiledLayoutManager` or `DwindleTree` clone.

## 16. Snapshot locality

Canonical snapshots enumerate `tree.windows()` and perform direct `WindowId` lookups. They no longer scan every `DesktopWindow` and rediscover membership. A debug assertion checks canonical Tiled management/location for committed solves; candidate migration snapshots intentionally use candidate tree membership.

## 17. Resize-frame complexity evidence

Resize flush metrics record active-frame snapshot count and solver node visits. The locality test populates unrelated hidden DesktopWindows while asserting that the active snapshot set contains only active-tree members. Raw pointer updates still replace one fixed pending slot and do not solve, clone, scan, configure, or generate a frame.

## 18. PreparedTiledMigration design

Migration now prepares affected source/destination candidate trees only, computes final candidate solutions, selects incoming-only fallbacks, and computes Floating restore targets before canonical mutation. The typed `PreparedTiledMigration` is then committed and applied separately.

## 19. Zero-side-effect prepare evidence

Preparation does not cancel interaction, clear pending resize state, mutate canonical trees or membership, send configure commands, touch visual geometry, or advance generation. Interaction cancellation and canonical tree replacement occur only in `commit_prepared_tiled_migration()` after preparation succeeds. A focused failed-preparation test verifies canonical source/destination trees and memberships remain unchanged.

## 20. Migration fallback behavior

Destination preparation preserves existing destination Tiled members and removes incoming members deterministically from the candidate until a feasible final state exists. If only a pre-existing destination member remains infeasible, preparation fails instead of silently changing canonical state.

## 21. Visible Floating restore

Every prepared fallback gets a target from saved Floating geometry, current visual/root geometry, or a safe work-area fallback. Visible migration and work-area fallbacks apply survivor plans and all Floating restores inside the same outer reflow batch.

## 22. Hidden Floating restore/defer behavior

Hidden fallback restores are recorded in the focused `tiled_floating_restores` map and do not emit eager configure traffic. Activation consumes the deferred restore before final scene presentation; a hidden migration regression verifies that the deferred target is not lost.

## 23. Final-state work-area fallback

Usable-area reconciliation now prepares each visible location using a cloned candidate tree, removes deterministic witness leaves only in candidate state, and computes the final survivor solution before committing or applying anything. Multiple fallbacks therefore do not expose intermediate survivor geometry.

## 24. Regular+Special batching

Visible Regular and Special locations are both prepared before candidate commits. Their final solutions and Floating restores are applied through one outer layout batch, avoiding interleaved per-location generation and configure states.

## 25. tiled_layout_dirty final semantics

Dirty state now has read semantics. Hidden tree/constraint/work-area changes mark the location; activation reflows only when the location is dirty. Clean activation does not solve solely because visibility changed. Empty trees clear stale dirty state unless a deferred Floating restore remains.

## 26. Dynamic XDG constraint results

XDG constraint updates continue through `reconcile_tiled_constraints()`. The complete lattice solver prevents the known legal `80x45` case from auto-floating. Genuine infeasibility still produces a typed fallback and survivor solve; candidate validation occurs before the canonical window is changed.

## 27. X11 ICCCM parity evidence

The lattice anchor rule matches the existing Floating resize semantics for min/base/increment alignment, including increment-without-base. Pure Tiled tests cover representative base/increment, fixed, min/max, aspect, and combined cases. Existing X11 ICCCM and tiled X11 suites remain green.

## 28. Configure-count evidence

Focused resize coalescing remains green: many raw updates replace one pending value and produce one frame flush. Visible migration/work-area fallback application is candidate-final and batched. The full suite's failures are unrelated Xwayland/AstreaCtl environment tests, not tiled configure assertions.

## 29. Layout-generation evidence

The existing `LayoutReflow` batching mechanism remains the generation authority. Prepared migration and work-area paths commit/apply through one outer batch; clean activation does not create a layout solve or generation solely from visibility.

## 30. Source-layout result

`bin/check-source-layout` reports only the historical oversized files: `src/compositor/tests/windows.rs`, `src/compositor/state/desktop_windows.rs`, `src/compositor/state/windows.rs`, `src/compositor/mod.rs`, `src/compositor/server.rs`, `src/native_output/runtime/bootstrap.rs`, and `src/xwayland/xwm/events.rs`. No new Dwindle closure file violates the configured limits.

## 31. Focused tests

Passed with `rtk`: layout/lattice/solver suites (34), tiled layout/migration/dirty/locality tests (7), resize interaction/coalescing tests (60), decoration layout tests (7), desktop-window tests (71), X11 tiled authority (1), and X11 resize-visual tests (10).

## 32. Full suite results

`rtk cargo test --locked` completed with 1,815 passed, 35 failed, and 2 ignored. No focused Dwindle, lattice, subtree, migration, dirty-state, or tiled-resize test failed.

## 33. Known environment/pre-existing failures

The 35 failures are the known AstreaCtl discovery and Xwayland filesystem/socket/process families, including `SUN_LEN` path-length constraints and cascading `PoisonError` cases. They were not weakened or suppressed. No native DRM/KMS hardware qualification was performed or claimed.

## 34. Review Pass 1 — constraint/transaction correctness

The review found and fixed the fixed-candidate resolver, inconsistent base-without-increment legality, scalar minima being used as coupled proof, resize mutation before successful solve, migration cancellation during preparation, full-manager candidate cloning, missing Floating restore targets, hidden restore loss, and intermediate work-area fallback application. Differential and focused transaction tests were rerun.

## 35. Review Pass 2 — hot path/resource usage

The review found no remaining manager/tree clone in tiled resize, no global DesktopWindow scan in active snapshot collection, no raw-event feasibility work, no persistent feasibility cache, no locks/timers/worker layout path, no acknowledgement barrier, and no Cartesian client-size search. Active-frame snapshot and node-visit counters were added for structural evidence. The existing one-solve-per-frame coalescing tests were rerun.

## 36. Final git status

The repository remains intentionally dirty with the pre-existing v1/v1.1 work plus this v1.1.1 closure. Existing deletions and untracked reports/design/source files were preserved. No unrelated changes were staged and no commit was created.

## 37. Explicit next roadmap phase

The next phase is Typhon Dwindle v1.2: directional focus and movement, split controls, gaps and smart gaps, and tiled-by-default policy. This task does not begin v1.2 work.
