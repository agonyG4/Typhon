# Typhon Dwindle v1.1.2 — Final Fix Closure Report

## 1. Baseline HEAD

The authoritative checkout started at `9d3fb34b45f6ce4ffc4582c3231e220b3643e959` on the existing `main` branch. No reset, restore, stash, clean, worktree creation, or commit was performed.

## 2. Initial dirty-tree summary

The checkout already contained the complete v1/v1.1/v1.1.1 dirty working tree: 61 tracked paths were modified or deleted, with untracked Dwindle sources, compositor modules, tests, plans, and reports. All existing changes were preserved and this closure was layered onto them.

## 3. Exact remaining bugs confirmed

The source review confirmed the three v1.1.2 issues from the brief: equal-aspect disjoint lattices could enter enormous candidate enumeration; scalar lower bounds were still named and documented too ambiguously around coupled aspect feasibility; and successful workspace migration committed/cancelled an active tiled resize before opening the outer `LayoutReflow` batch.

## 4. Pathological lattice root cause

For exact `1:1`, odd width values and even height values have overlapping continuous intervals but no common legal integer value. The previous interval solver could not prove that modular mismatch and could repeatedly inspect candidates across a near-`u32` range. The issue was arithmetic incompleteness, not a Dwindle topology problem.

## 5. Exact-aspect bounded solver design

Added `src/wm/layout/exact_aspect.rs`. Equal finite aspect ranges are converted to a reduced rational ratio and solved as `width = k*p`, `height = k*q`. The solver calculates the valid `k` interval from both lattices and selects the largest legal `k` directly. It performs no Cartesian search and has no arbitrary attempt limit that can produce a false `Infeasible` result.

The existing one-sided and non-equal aspect interval path remains in place. Its recursive branches have strict interval subdivision and the candidate adjustment loops now stop when the next aligned value leaves the mathematically valid aspect interval. `DIRECT_LATTICE_CANDIDATES` remains only a local search strategy threshold, never an infeasibility cap.

## 6. GCD/congruence/intersection method

For each dimension, `k*p ≡ anchor_width (mod width_step)` and `k*q ≡ anchor_height (mod height_step)` are solved as linear congruences. Unsatisfiable divisibility is rejected with `gcd`; compatible congruences are combined with a generalized CRT that supports non-coprime moduli. All products and interval calculations use `u128` intermediates before conversion back to `u32`, preventing near-`u32` overflow. The largest congruent `k` is selected arithmetically.

## 7. Huge infeasible operation-count evidence

`huge_exact_aspect_disjoint_lattices_are_bounded` uses near-`u32` odd-width and even-height lattices with exact `1:1`. It returns `ClientRectError::Infeasible` through congruence arithmetic and asserts no more than 8 exact-aspect probes, 1,000 dimension candidates, and 1,000 interval nodes. The test passes without enumerating the billions-scale lattice.

## 8. Huge feasible operation-count evidence

`huge_exact_aspect_lattice_selects_the_largest_solution_without_enumeration` uses a near-`u32` feasible odd/odd exact `1:1` lattice. It returns the largest legal pair without overflow and asserts the same bounded probe, candidate, and subdivision thresholds. The selected result is the exact maximum legal dimension on both axes.

## 9. Preserved 80x45 regression

The existing `84x67` tile regression remains present and green: width base 8 with increment 3, height base 5, and exact `16:9` resolve to client size `80x45`.

## 10. Corrected lower-bound semantics

`LayoutConstraints::independent_lower_bounds()` now returns the first independently legal width and height lattice values only. It does not call the maximum-area client resolver and therefore does not turn unconstrained exact `1:1` into a misleading `u32::MAX x u32::MAX` minimum. The old `minimum_tile_size()` name remains as a compatibility wrapper with documentation identifying the independent-bound semantics.

`SubtreeLowerBounds` is now the authoritative name in `solve.rs`; `SubtreeRequirements` remains only as a compatibility type alias. The documentation states that these values support pruning and diagnostics, not coupled feasibility proof.

## 11. Exact feasibility relationship

`FeasibilityContext` remains authoritative for aspect/increment coupling. Leaf `can_fit()` calls the complete client resolver on the candidate rectangle, while nested split queries use exact monotonic extent searches and exact child boundary ranges. Independent lower bounds are not substituted for `can_fit()` and existing nested-aspect, monotonicity, and split-range tests remain green.

## 12. Migration batch sequencing fix

`migrate_tiled_layouts()` remains fully prepare-only. In `apply_workspace_membership_transition()`, preparation now completes first; only after it succeeds does the caller begin the outer `LayoutReflow` batch. `commit_prepared_tiled_migration()` then terminates invalid interactions and installs candidate topology inside that batch, followed by workspace membership commit and final prepared apply. Failed preparation still returns before batch entry or any external mutation.

Required terminal resize behavior remains intact. The active-resize migration regression observes one terminal `FinalizeResize` command while the final migrated state is committed through one outer logical layout transition.

## 13. Active-resize migration configure/generation evidence

`active_tiled_resize_migration_commits_inside_one_outer_layout_batch` installs two tiled windows, an active tiled resize session, a pending resize value, and a visual resize geometry. It verifies preparation leaves interaction, pending resize, topology, and generations unchanged. The successful move clears the interaction and pending resize, moves the topology and membership correctly, emits one terminal resize command, increments render generation once, increments layout generation once, and ends with `RenderGenerationCause::LayoutReflow`.

## 14. Focused tests

All focused suites passed with `rtk`: 42 pure layout/lattice/solver tests; 8 tiled-layout, migration, dirty-state, locality, and active-resize migration tests; 60 window-interaction/resize-coalescing tests; 71 desktop-window tests; 7 decoration-layout tests; 1 Xwayland tiled-authority test; and 10 Xwayland resize-visual tests.

## 15. Source-layout result

`bin/check-source-layout` reports only the existing oversized files: `src/compositor/tests/windows.rs`, `src/compositor/state/desktop_windows.rs`, `src/compositor/state/windows.rs`, `src/compositor/mod.rs`, `src/compositor/server.rs`, `src/native_output/runtime/bootstrap.rs`, and `src/xwayland/xwm/events.rs`. No new exact-aspect, lattice, solver, migration, or test closure file exceeds a configured limit.

## 16. Validation results

The following passed with `rtk`: `cargo fmt --check`, `cargo check --locked --all-targets`, and `git diff --check`. The full `cargo test --locked` run completed with 1,823 passed, 36 failed, and 2 ignored. No Dwindle-focused test failed.

## 17. Environment/pre-existing failures

The full-suite failures are outside this closure: the known AstreaCtl/Xwayland filesystem, socket, and process families fail under the environment's `SUN_LEN` path limit and then cascade through shared `PoisonError` locks. One existing native KMS test also fails its explicit atomic-flip fixture assertion. Tests were not weakened or suppressed, and no native DRM/KMS hardware qualification is claimed.

## 18. Review Pass 1 — correctness findings and fixes

The correctness review checked rational recovery, congruence divisibility, generalized CRT compatibility, `u128` arithmetic, lattice lower/upper bounds, base/increment anchors, deterministic maximum selection, lower-bound/exact-feasibility separation, prepare side effects, batch ordering, terminal resize preservation, generation count, and topology/membership consistency. Task-owned issues were covered by the exact-aspect, lower-bound, and active-resize migration regressions and passed.

## 19. Review Pass 2 — boundedness and scope findings

The boundedness/scope review found no exact-aspect linear scan, Cartesian width-by-height search, raw pointer-event solver work, new resize clone, persistent feasibility cache, or unrelated compositor performance refactor. The only remaining `TiledLayoutManager` clone is the pre-existing non-hot-path toggle-to-floating preservation path. No directional focus, gaps, animation, O1, KMS, scanout, profiler, ActiveScene, damage, or other Performance v1 work was added.

## 20. Final git status

The repository remains intentionally dirty with the pre-existing work plus this v1.1.2 plan, exact-aspect module, lower-bound changes, migration sequencing change, focused regression, and report. Existing deletions and unrelated untracked files were preserved. Nothing was staged and no commit was created.

## 21. Typhon Performance v1 exclusion and closure

Typhon Performance v1 was explicitly not implemented here. This task closes the Dwindle topology, exact ICCCM geometry, complete exact-aspect lattice solving, independent lower bounds, exact subtree feasibility, tiled split resize, and migration transaction sequencing. The next project phase remains Typhon Performance v1.
