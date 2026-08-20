# WindowVisual Input Authority Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the topmost interactive `WindowVisual` the single owner for pointer motion, decoration buttons, compositor move/resize, and focus transitions while preserving popup, layer, grab, resize, and native rendering semantics.

**Architecture:** Extract the existing renderer's root/subsurface/popup grouping algorithm into a lightweight `VisualStackGroup` primitive. Reuse it in rendering and cached pointer hit testing. Propagate the resolved `PointerSceneHit` into interaction and decoration-button capture so no ordinary compositor path falls back to a lower client surface.

**Tech Stack:** Rust, Wayland server/client integration tests, existing compositor render and native-output test suites, Cargo.

## Global Constraints

- Preserve the current dirty working tree and do not run destructive cleanup.
- Preserve instant visual resize, bounded resize configure windows, presented-scene history, render-ahead damage journals, buffer-age correctness, and fullscreen frame-scene authority.
- Do not modify Eclipse Dock, idle CPU, unrelated rendering performance, Direct Scanout, presentation scheduling, or Wayland protocol breadth.
- Do not build raster assets, font layouts, theme files, or full decoration render plans in pointer hit testing.
- Maintain popup-grab, implicit-grab, locked-pointer, confined-pointer, drag-and-drop, exclusive-layer, CSD, XWayland, and fullscreen behavior.
- Because this checkout contains unrelated dirty work, each task has a review/staging boundary; no commit is created unless explicitly authorized after verification.

### Task 1: Add the failing overlapping titlebar interaction regression

**Files:**
- Modify: `src/compositor/tests/input_output/window_interaction.rs`
- Inspect: `src/compositor/tests/support/server_runtime.rs`

**Failing test:** Extend the existing overlapping server-decoration scenario with a titlebar `BeginMove`, horizontal `UpdateInteraction`, and interaction snapshot/geometry assertions. The test must assert that A's captured `window_id` and `root_surface_id` are retained and B's position is unchanged.

**Expected pre-fix behavior:** The current `surface_id_at()` path selects B underneath A's titlebar, so the snapshot owner is B or A does not move.

**Implementation boundary:** Test-only. Do not change production code in this task.

- [ ] **Step 1: Add the titlebar drag assertions to the existing two-window SSD test.**
- [ ] **Step 2: Run the focused test and confirm it fails because the captured owner is the lower client.**

Run: `rtk cargo test --locked overlapping_server_decoration_does_not_focus_window_underneath -- --exact --nocapture`

**Verification:** The failure names the interaction owner/geometry mismatch, not a setup or compilation error.

**Commit boundary:** Review the test diff only; leave uncommitted with the pre-existing dirty tree.

### Task 2: Add the failing overlapping resize-margin regression

**Files:**
- Modify: `src/compositor/tests/input_output/window_interaction.rs`

**Failing test:** Add a two-window SSD scenario where A's top resize margin overlaps B's client. Begin resize at the exact A decoration hit and assert that the captured owner is A and the resize edge is A's edge.

**Expected pre-fix behavior:** `begin_window_resize_at_with_trigger()` sees B through `surface_id_at()` and captures B or derives the wrong edge.

**Implementation boundary:** Test-only.

- [ ] **Step 1: Add the overlapping top-edge resize test using the existing server/client helpers.**
- [ ] **Step 2: Run it and confirm the pre-fix owner mismatch.**

Run: `rtk cargo test --locked overlapping_server_decoration_resize_preserves_owner -- --exact --nocapture`

**Verification:** The test fails at the captured owner/edge assertion for the expected authority split.

**Commit boundary:** Review the test diff only; leave uncommitted.

### Task 3: Add render/input-ordering and popup-ordering regressions

**Files:**
- Modify: `src/compositor/state/window_decoration_tests.rs`
- Modify: `src/compositor/render.rs`

**Failing tests:** Add an ordinary subsurface that overlaps A's titlebar and assert the pointer hit is `Decoration(A)`. Add/extend the render grouping test to assert that a popup group remains above the parent SSD and an ordinary descendant remains in the decorated group.

**Expected pre-fix behavior:** The raw reverse surface walk tests the ordinary child before reaching A's SSD. The pure renderer grouping test documents the intended order and remains green.

**Implementation boundary:** Test fixtures use real registered Wayland surfaces where needed; no coordinate special cases.

- [ ] **Step 1: Add the ordinary-subsurface overlap fixture and failing pointer assertion.**
- [ ] **Step 2: Run the focused test and confirm it returns the child client before the production fix.**
- [ ] **Step 3: Add the explicit popup-vs-SSD assertion to the render grouping test.**

Run: `rtk cargo test --locked pointer_scene_hit_prefers_ssd_over_ordinary_subsurface -- --exact --nocapture`

**Verification:** The ordinary-subsurface regression fails for ordering, while the popup grouping expectation is explicit.

**Commit boundary:** Review test-only diff; leave uncommitted.

### Task 4: Extract the shared lightweight visual-stack primitive

**Files:**
- Modify: `src/compositor/render.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/mod.rs` re-export list only if required by state modules

**Implementation boundary:** Introduce `VisualStackGroup { root_surface_id, surface_indices, popup }` and `visual_stack_groups(surfaces, popup_surface_ids)`. Preserve `WindowVisualGroup`'s public render-facing decoration-index API by mapping from the new primitive. The group algorithm must retain `render_placement`, ordinary descendant grouping, popup splitting, and back-to-front group order.

- [ ] **Step 1: Write unit assertions for root/child grouping and popup splitting.**
- [ ] **Step 2: Run the render grouping tests and confirm the new primitive preserves existing results.**
- [ ] **Step 3: Implement the primitive and adapt renderer grouping to consume it.**
- [ ] **Step 4: Run the render grouping tests again.**

Run: `rtk cargo test --locked compositor::render::tests::popup_surfaces_paint_above_ssd_but_ordinary_subsurfaces_stay_with_the_window -- --exact`

**Verification:** Renderer tests pass and no decoration render-plan type enters the new primitive.

**Commit boundary:** Review `render.rs` API and tests; leave uncommitted.

### Task 5: Make pointer scene hit testing consume the shared order

**Files:**
- Modify: `src/compositor/state/hit_testing.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/subsurfaces.rs` cache invalidation only if required

**Implementation boundary:** Add a scene-order cache keyed/invalidateable by meaningful scene changes. Populate popup IDs only when rebuilding the cache. Traverse groups front-to-back: popup client surfaces, decorated SSD, then ordinary client surfaces. Reuse current immediate visual geometry and input regions. Add a per-position scene-hit cache so motion/button dispatch can reuse one resolved hit without hot-path allocations.

- [ ] **Step 1: Add the cache fields and invalidation hook without changing hit semantics.**
- [ ] **Step 2: Write the shared-order pointer regression and run it red.**
- [ ] **Step 3: Replace the raw reverse-surface walk with cached group traversal.**
- [ ] **Step 4: Run ordinary-subsurface, popup, CSD, fullscreen, and existing decoration tests.**

Run: `rtk cargo test --locked pointer_scene_hit -- --nocapture`

**Verification:** Ordinary SSD wins over ordinary descendants, real popups win above SSD, CSD/fullscreen produce no fake SSD, and no render asset construction is reachable from pointer hit testing.

**Commit boundary:** Review hit authority and cache invalidation; leave uncommitted.

### Task 6: Make move/resize consume `PointerSceneHit`

**Files:**
- Modify: `src/compositor/state/window_interaction.rs`
- Modify: `src/compositor/state/hit_testing.rs` client-only helper documentation/rename if needed

**Implementation boundary:** Add a compact scene-hit-to-interaction-target path. Native move/resize must resolve once, preserve A's `WindowId` and root, use exact decoration resize edges, and use the client surface only for client-content bindings. `begin_window_interaction_at()` must no longer derive an owner from client-only `surface_id_at()`.

- [ ] **Step 1: Add unit coverage for decoration and client interaction target derivation.**
- [ ] **Step 2: Run titlebar and resize regressions and confirm they fail only until this boundary is implemented.**
- [ ] **Step 3: Implement exact decoration/client branches and rejection for `None`.**
- [ ] **Step 4: Run focused interaction tests.**

Run: `rtk cargo test --locked window_interaction -- --nocapture`

**Verification:** No production move/resize path uses `surface_id_at()` as its owner authority; client bindings still retain their original pointer-motion surface.

**Commit boundary:** Review interaction ownership changes; leave uncommitted.

### Task 7: Pass exact decoration capture identity into buttons and titlebar actions

**Files:**
- Modify: `src/compositor/state/window_decoration.rs`
- Modify: `src/compositor/state/input_dispatch.rs`

**Implementation boundary:** Resolve the normal scene hit once in `send_pointer_button()`, pass it into `handle_decoration_button()`, and start titlebar move/resize directly from that exact hit. Button release may use the resolved current-event hit for same-button activation; it must not query a second authority. Preserve higher-level grabs by resolving the scene only for ordinary scene input.

- [ ] **Step 1: Add titlebar/button capture assertions with B beneath A.**
- [ ] **Step 2: Run them red against the old re-hit path.**
- [ ] **Step 3: Implement the resolved-hit parameter and exact-owner calls.**
- [ ] **Step 4: Run button, double-click, drag-crossing, and grab tests.**

Run: `rtk cargo test --locked decoration -- --nocapture`

**Verification:** Titlebar, resize margin, minimize, maximize/restore, close, double-click, and press-drag-release all retain A; popup/implicit/locked/confined routes remain higher-level authorities.

**Commit boundary:** Review button and capture ownership; leave uncommitted.

### Task 8: Remove same-window decoration focus churn and make repeated clear cheap

**Files:**
- Modify: `src/compositor/state/windows.rs`
- Modify: `src/compositor/state/hit_testing.rs`
- Modify: `src/compositor/state/input_resources.rs` only if motion hit propagation requires it

**Implementation boundary:** Return `WindowFocusOutcome::NoChange` for a decoration whose owner is already the valid focused desktop/root window. Make `clear_pointer_focus()` return cheaply when client pointer focus, entered surfaces, and client cursor claims are already empty. Preserve real mismatch repair and constraint teardown.

- [ ] **Step 1: Add a same-window focus-generation/no-duplicate-leave regression.**
- [ ] **Step 2: Run it red or demonstrate the old full-focus path through observable counters.**
- [ ] **Step 3: Implement the guarded no-op and idempotent clear.**
- [ ] **Step 4: Run pointer focus sequence tests.**

Run: `rtk cargo test --locked pointer -- --nocapture`

**Verification:** A client→SSD→client keeps desktop focus A, leaves/enters exactly once, never focuses B, and does not repeat focus generation or constraint work.

**Commit boundary:** Review focus-only changes; leave uncommitted.

### Task 9: Add 1000-cycle focus/ownership stress coverage

**Files:**
- Modify: `src/compositor/tests/input_output/window_interaction.rs`
- Modify: `src/compositor/state/window_decoration_tests.rs` if the fast deterministic loop belongs there

**Implementation boundary:** Run 1000 synthetic A-client↔A-SSD transitions with B below, assert focus generation and desktop owner stability, and record that B receives no pointer enter or activation. Do not introduce timing sleeps.

- [ ] **Step 1: Add the bounded synthetic loop.**
- [ ] **Step 2: Run it and verify it catches any A→B→A churn.**

Run: `rtk cargo test --locked client_ssd_focus_stress -- --exact --nocapture`

**Verification:** The loop is deterministic and remains within the existing test runtime budget.

**Commit boundary:** Review stress test only; leave uncommitted.

### Task 10: Run resize, rendering, and full validation suites

**Files:**
- No intended source changes; inspect only the focused files and existing native-output tests.

- [ ] **Step 1: Run focused compositor interaction, decoration, render, and resize tests with `rtk cargo test`.**
- [ ] **Step 2: Run `rtk cargo fmt --check`.**
- [ ] **Step 3: Run `rtk cargo check --locked --all-targets`.**
- [ ] **Step 4: Run `rtk cargo test --locked`.**
- [ ] **Step 5: Run `rtk cargo clippy --locked --all-targets -- -D warnings`.**
- [ ] **Step 6: Run `rtk run -c 'bash bin/check-source-layout'`.**
- [ ] **Step 7: Run `rtk git diff --check` and inspect `rtk git status --short`.**

**Verification:** Record pre-existing/environmental failures separately and do not classify new failures without evidence. Confirm resize-latency, ghosting, buffer-age, presented-scene, and fullscreen frame-scene tests remain green.

**Commit boundary:** No commit; preserve unrelated dirty work.

### Task 11: Native qualification and final report

**Files:**
- Create: `REPORT-2026-08-18-windowvisual-input-authority-closure.md`

- [ ] **Step 1: Run the requested native command if the session and dependencies are available.**
- [ ] **Step 2: Exercise overlapping client/titlebar/button/resize paths at least 100 times.**
- [ ] **Step 3: Record native results, unavailable prerequisites, and all validation commands without overstating unproven behavior.**
- [ ] **Step 4: Record the final dirty status and explicitly list commits (expected: none unless separately authorized).**

**Verification:** The report answers every final self-review invariant, distinguishes `NATIVE-PROVEN`, `CONFIRMED`, `STRONG HYPOTHESIS`, and `UNPROVEN`, and includes focused/full-suite results.

**Commit boundary:** No commit; final handoff preserves the existing dirty checkout.
