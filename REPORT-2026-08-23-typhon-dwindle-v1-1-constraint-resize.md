# Typhon Dwindle v1.1 — Constraint-Aware Split Resize Report

## 1. Baseline HEAD

The implementation started from `9d3fb34b45f6ce4ffc4582c3231e220b3643e959`.

## 2. Initial dirty-tree summary

The worktree was already dirty before this task, with 61 tracked files changed and multiple untracked v1 reports, plans, specs, and source modules. Existing changes were preserved. No reset, checkout, clean, stash, or commit was performed.

## 3. Dwindle v1 architecture found

The existing architecture had a pure `wm::layout` layer backed by `DwindleTree` and a compositor state layer that owned workspace membership, window metadata, output geometry, configure delivery, decorations, hit testing, and interactions. The v1.1 work keeps that separation: the layout solver is pure and transactional, while the compositor owns visibility, protocol routing, persistence, and fallback policy.

## 4. Pre-flight bugs confirmed

The audit confirmed that v1 needed explicit client constraints, subtree feasibility, legal client rectangles distinct from tile rectangles, transactional membership changes, deterministic work-area fallback, and frame-coalesced tiled resize. It also confirmed that Minimal SSD edge hit testing and workspace/output transitions had to enter the same logical resize and layout-reflow paths.

## 5. Layer-shell work-area fix

Output-size and layer-shell reserved-area changes now converge on `reflow_usable_output_geometry`. The helper updates stateful output geometry, cancels an active tiled resize with a work-area reason, reflows visible locations, and marks hidden locations dirty. Visible infeasibility is handled by the deterministic work-area fallback loop.

## 6. `tiled_layout_dirty` final semantics

Visible locations are solved and applied immediately. Hidden locations are not solved or configured during a work-area change; they are inserted into `tiled_layout_dirty`. Workspace activation and special-workspace opening reflow the newly visible tree through the existing batch path, and successful application removes its dirty marker. Membership and constraint updates retain the same visible-versus-hidden rule.

## 7. Complete `LayoutConstraints` model

`LayoutConstraints` now represents minimum width/height, maximum width/height, base width/height, width/height increments, and minimum/maximum aspect ratio. Validation rejects contradictory, non-finite, zero, negative, and malformed values. Normalization is used at the compositor snapshot boundary; the pure solver still reports invalid raw constraints instead of partially applying them.

## 8. `tile_rect`/`client_rect` architecture

Every solved target carries both an exact tile rectangle and a legal centered client rectangle. Split geometry partitions the complete work area without gaps or overlap. Maximum sizes, increments, and aspect limits affect only the client rectangle unless the minimum footprint makes the tile itself infeasible.

## 9. Minimum-footprint calculation

The pure client resolver computes the minimum legal footprint from the minimum/base/increment and aspect constraints, while respecting maximum bounds. Fixed and bounded clients are centered inside their tiles. Impossible constraints produce typed errors rather than clipped or partial geometry.

## 10. Subtree requirements

The solver performs a bottom-up requirement pass. Leaves expose their minimum legal footprint; split nodes aggregate child requirements along the split axis and take the maximum on the orthogonal axis. Each node retains the windows contributing to a requirement so an infeasibility witness can identify the relevant subtree.

## 11. Feasible split range algorithm

For each split, the solver derives a complete feasible ratio interval from the parent rectangle and the two child requirements, intersected with the safe ratio range `[0.05, 0.95]`. The pass is linear in the number of tree nodes and emits a typed `ConstraintInfeasibility` witness when the interval is empty.

## 12. Preferred/effective ratio

The tree stores the preferred split ratio. The solution records the preferred ratio, feasible range, boundary coordinate, and effective ratio. The effective ratio is the preferred value clamped to the feasible interval, with deterministic tie behavior and no mutation of the tree during solving.

## 13. ICCCM client-size resolution

The bounded pure resolver applies ICCCM-style base size, increments, minimum/maximum size, and aspect constraints, including fixed-size and maximum-sized clients. It uses a bounded candidate set plus bounded aspect refinement and returns a centered legal client rectangle. Focused tests cover increments, aspect limits, impossible aspect bounds, fixed sizes, and max-sized clients. Existing X11 ICCCM tests remain green; the report does not claim native DRM/KMS qualification.

## 14. Migration transaction

Workspace migration clones the layout manager, removes and inserts the affected tiled membership in the candidate, snapshots candidate tree membership rather than canonical compositor membership, and solves every affected candidate destination before committing. Canonical window locations are changed only after candidate feasibility succeeds.

## 15. Migration fallback

If the incoming window makes the destination infeasible, the candidate is not committed. The incoming window alone falls back to floating, retains or captures restore geometry, and the typed `WorkspaceMigration` reason is recorded. Existing destination members remain tiled and valid.

## 16. Dynamic XDG constraints

XDG pending-toplevel constraint updates now reconcile tiled membership after the new metadata is assigned. An active tiled resize is cancelled before reconciliation. A feasible update produces one visible layout application or marks a hidden location dirty; an infeasible update removes only the culprit from the tiled tree and auto-floats it without exposing a partial candidate.

## 17. Dynamic X11 constraints

X11 size-hint metadata deltas use the same `reconcile_tiled_constraints` path. X11 mode and minimize transitions also cancel tiled resize state. This keeps XDG and X11 constraint changes subject to the same pure solver, transactional fallback, and visible/hidden policy.

## 18. Work-area fallback

When a visible work-area shrink makes a tree infeasible, the solver witness is matched against leaves in reverse tree/insertion order. The most recent applicable witness window is auto-floated, the surviving candidate is solved again, and the loop continues until feasible or safely aborts. Hidden trees are never entered into this loop during the shrink event.

## 19. Ancestor selection

Tiled resize preparation resolves the nearest applicable ancestor split from the solved tree and the actual resolved boundary. It does not use the root as a proxy when a nearer ancestor controls the selected edge. Invalid or stale topology is rejected before mutation.

## 20. Corner resize

Corner resize can select one horizontal and one vertical ancestor split. Both axes are represented in one handle and one pending value. A flush solves the candidate once, applies both effective ratios transactionally, and deduplicates unchanged results.

## 21. `PendingTiledResize`

Raw pointer updates now compute only rounded deltas and requested ratios, replacing one latest-value slot. They do not clone the layout, solve, allocate a candidate, send configures, or wait for acknowledgements. Cancellation clears both the pending value and the session.

## 22. `prepare_frame`

Frame preparation flushes the pending tiled resize before ordinary pending resize configure work. The pending value is visible to scheduler/frame preparation, so at most one tiled solve/reflow is performed per frame for a given interaction.

## 23. XDG `Resizing`

For XDG tiled resize, the owner receives the resize configure through the owner resize path with the `Resizing` state. Siblings receive ordinary layout configures. The owner path is selected from the active tiled resize session rather than inferred from arbitrary geometry equality.

## 24. X11 moveresize

X11 tiled resize uses the same logical split-resize route and the existing X11 configure/moveresize backend delivery. Native and Minimal paths share the pure split preparation and flush semantics; no acknowledgement barrier was added to the hot path.

## 25. SSD Minimal

Minimal SSD exposes a six-pixel logical edge/corner resize zone without inventing a titlebar or rendered border. Hit testing gives the tiled logical edge precedence over overlapping client input, and the returned edges are filtered to axes that have an adjustable ancestor split. CSD behavior remains unchanged.

## 26. Configure-count evidence

`apply_layout_geometry` routes the active tiled owner through the resize configure queue and routes siblings through ordinary configure delivery. The coalescing test performs two raw updates, replaces one pending value, and verifies one frame flush. Focused interaction, desktop-window, and Xwayland resize suites pass; this is protocol-flow instrumentation evidence, not a hardware or compositor-latency benchmark.

## 27. Layout-generation evidence

Layout reflows use the existing batch depth and scene-effect accumulator. Nested callers do not publish intermediate layout generations; the outer batch publishes the resulting scene effect and advances `LayoutGeneration` once. Workspace, special-workspace, output, layer-shell, migration, and resize callers use this batching path.

## 28. Raw-event/frame counters

`ResizeFlowMetrics` records tiled interaction starts, raw updates, pending replacements, frame flushes, ratio clamps, unchanged flushes, constraint reflows, constraint auto-floats, work-area reflows, and migration fallbacks. The focused coalescing test asserts `raw_updates == 2`, `pending_replaced == 1`, and `frame_flushes == 1`; cancellation asserts no flush occurs.

## 29. Stress

The pure layout tests include 4,000 deterministic insert/remove/ratio/minimize operations. Each mutation validates the Dwindle tree and repeatedly attempts a solution against changing roots. The stress run passed.

## 30. Large solver evidence

Synthetic trees with 1, 10, 50, 100, and 500 leaves solve successfully. The test asserts a complete target for every leaf and bounds solver node visits by `4*N + 2`, demonstrating linear behavior for the tested balanced construction.

## 31. Source-layout debt fixed

The Dwindle lifecycle tests were extracted from `src/compositor/state/desktop_window_tests.rs` into `src/compositor/state/tiled_layout_tests.rs`. The tiled Xwayland test was extracted from `src/compositor/tests/xwayland.rs` into `src/compositor/tests/xwayland_tiled.rs`. No source-layout limit was raised.

## 32. Validation results

The following checks passed after the final work-area and interaction-state patches:

- `cargo fmt --check`
- `cargo check --locked --all-targets`
- `git diff --check`
- `cargo test --lib 'wm::layout::'`: 27 passed
- `cargo test --lib compositor::state::window_interaction_tests`: 60 passed
- `cargo test --lib compositor::state::tiled_layout_tests`: 4 passed
- `cargo test --lib compositor::decoration::layout::tests`: 7 passed
- `cargo test --lib compositor::state::desktop_window_tests`: 71 passed
- `cargo test --lib compositor::tests::xwayland::xwayland_tiled`: 1 passed
- `cargo test --lib compositor::tests::xwayland::xwayland_resize_visual`: 10 passed

## 33. Environment/pre-existing failures

The required full `cargo test --locked` run completed with 1,805 passed, 35 failed, and 2 ignored. The failures are concentrated in existing AstreaCtl discovery and Xwayland filesystem/socket/process tests; the known environment failure is the platform path-length condition (`path must be shorter than SUN_LEN`), followed by poisoned shared test state. No focused Dwindle, tiled-resize, desktop-window, decoration, or Xwayland tiled test failed. The source-layout checker reports only these existing oversized files: `src/compositor/tests/windows.rs` (2081/2000), `src/compositor/state/desktop_windows.rs` (1516/1500), `src/compositor/state/windows.rs` (1690/1500), `src/compositor/mod.rs` (854/800), `src/compositor/server.rs` (1582/1500), `src/native_output/runtime/bootstrap.rs` (1504/1500), and `src/xwayland/xwm/events.rs` (1551/1500).

## 34. Review pass 1 — correctness and transaction

The first independent review checked candidate migration commit order, candidate membership snapshots, witness-directed fallback, tile/client separation, topology-generation invalidation, visible/hidden behavior, cancellation, tiled-move rejection, and owner-versus-sibling configure routing. The material issue found was hidden work-area trees being solved during the initial implementation; it was fixed by continuing to the dirty marker before the fallback solver. Cursor-generation suppression was also extended to work-area cancellation.

## 35. Review pass 2 — hot path and 165 Hz behavior

The second review checked raw-event work, allocation boundaries, solve frequency, configure routing, batching, bounded constraint resolution, stale-session handling, and future extensibility. The final path keeps raw updates to latest-value replacement, flushes once in frame preparation, performs no timer or acknowledgement wait, and skips unchanged effective ratios. Future directional moves, gaps, split controls, tiled-by-default policy, and broader layout policies remain outside this v1.1 scope.

## 36. Final git status

The worktree remains intentionally dirty. Existing modifications and untracked artifacts were preserved, and the new v1.1 design, plan, implementation modules, extracted tests, and this report are uncommitted. No branch switch, commit, or push was performed.

## 37. Remaining roadmap

The next safe increments are native protocol/hardware qualification, explicit migration-fallback coverage, broader live compositor configure-count instrumentation, and future directional movement/gaps/split-control policy. Those should build on the pure constraint solver and transactional resize path rather than bypassing them.
