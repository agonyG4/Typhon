# Typhon PointerSceneHit Cache and Focus-Churn Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make ordinary pointer scene ownership reflect current input-only state, keep client↔SSD routing on one window without focus churn, and remove avoidable high-frequency hit-test work while preserving instant resize and render-ahead correctness.

**Architecture:** Add a dedicated `pointer_hit_generation` alongside the existing render generation. Input-region commits and the existing surface order/placement invalidation seam advance it before stationary focus refresh; cache validity requires coordinates plus both generations. Retain each visual group's root index and compute hits through borrowed immutable scene data before storing the cache result.

**Tech Stack:** Rust, Smithay/Wayland integration tests, existing controllable compositor server, Cargo, `rtk` command proxy.

## Global Constraints

- Preserve `VisualStackGroup` as shared render/input ordering authority.
- Preserve `PointerSceneHit` as the ordinary pointer scene ownership authority.
- Preserve exact `WindowId`/root ownership for titlebar drag, resize, and buttons.
- Preserve immediate interactive resize and bounded latest-wins configure behavior with outstanding pressure `<= 3`.
- Preserve presentation-domain damage history, buffer-age correctness, `ResolvedNativeFrameScene`, and fullscreen frame-scene authority.
- Do not add a spatial index or unsafe code.
- Do not use destructive Git commands or erase unrelated dirty work.
- Use `rtk` for Git, file inspection/search, and Cargo/test commands.
- Every behavior change follows RED -> GREEN -> REFACTOR; record the failing command before implementation.

---

### Task 1: Reproduce stationary input-region cache staleness

**Files:**
- Modify: `src/compositor/state/window_decoration_tests.rs` for deterministic scene fixtures and cache assertions.
- Modify: `src/compositor/state/hit_testing.rs` only for test-visible helper access if the existing module boundary requires it; do not change production behavior in the RED phase.

**Failing test:** Add a deterministic test with front A and rear B, both mapped and covering P. Install live `SurfaceData` resources or use the existing compositor fixture that can commit input regions. Resolve A at P, commit A's empty input region while stationary through the production commit path, and assert the refresh resolves B. Add the reverse excluded-to-included case.

**Expected pre-fix behavior:** The stationary refresh reuses `(P, scene_render_generation=N)` and returns stale A, or the reverse case remains B. The test must fail for that reason before production code changes.

**Implementation:** None in this task; only test setup and the RED run.

**Verification:** Run the narrow test target with `rtk cargo test --locked <focused-test-filter> -- --exact --nocapture` and capture the stale owner assertion.

**Commit boundary:** Test-only commit boundary: `test(input): reproduce stationary stale pointer scene cache` if commits are requested; do not stage unrelated dirty files.

### Task 2: Audit dependencies and generation ownership

**Files:**
- Modify: `docs/superpowers/specs/2026-08-18-pointer-scene-cache-focus-churn-design.md` if source evidence changes the audit.
- Inspect: `src/compositor/state/hit_testing.rs`, `src/compositor/state/surface_transactions.rs`, `src/compositor/state/subsurfaces.rs`, `src/compositor/state/surface_commits.rs`, `src/compositor/render.rs`, `src/compositor/mod.rs`, `src/xwayland/xwm/events.rs`, and `src/xwayland/xwm/shape.rs`.

**Failing test:** The RED tests from Task 1 are the observable proof that render generation alone is insufficient.

**Expected pre-fix behavior:** Input-region mutation is not represented in cache validity; existing geometry/order invalidation clears the cache directly.

**Implementation:** Establish the mutation table: input region uses the new generation; render-visible geometry/order/map/unmap/topology use existing render generation plus the shared invalidation seam; title-only/damage/frame/presentation events do not advance pointer generation.

**Verification:** Use graph traces and `rtk rg` literal searches, then call `check_index_coverage` for every operated-on source path. Read any flagged ranges directly. Update the design note with CONFIRMED/STRONG HYPOTHESIS/UNPROVEN labels.

**Commit boundary:** Documentation-only boundary if needed; no production commit.

### Task 3: Implement pointer-hit generation and ordering

**Files:**
- Modify: `src/compositor/mod.rs` to store `pointer_hit_generation`.
- Modify: `src/compositor/state/hit_testing.rs` to include the generation in `PointerSceneHitCache` and cache lookup/store.
- Modify: `src/compositor/state/subsurfaces.rs` so shared surface-order/origin invalidation advances the generation without directly clearing the hit cache.
- Modify: `src/compositor/state/surface_transactions.rs` to advance generation after committed input-region mutation and before stationary refresh.
- Test: `src/compositor/state_data.rs` and focused compositor tests for the cache contract.

**Failing test:** Task 1 stationary A-empty and reverse B/A tests.

**Expected pre-fix behavior:** Same coordinates and render generation return a stale owner after input-only mutation.

**Implementation:** Add a non-zero wrapping `advance_pointer_hit_generation()` helper. Cache validity becomes `x`, `y`, `scene_render_generation`, and `pointer_hit_generation`. In `apply_cached_subsurface_commit`, keep `apply_input_region_change`, then advance pointer generation, then call `refresh_pointer_focus_at_last_position`. Do not advance for unrelated render-only state.

**Verification:** Run the Task 1 tests; add a direct cache effectiveness test proving repeated same-coordinate/same-generations hit the cache and input-region-only mutation changes only pointer generation and recomputes. Run the relevant state-data and compositor tests.

**Commit boundary:** `fix(input): invalidate pointer scene cache on input ownership changes`.

### Task 4: Cover destruction and current geometry parity

**Files:**
- Modify: `src/compositor/state/window_decoration_tests.rs` or the closest existing compositor input test module.
- Modify: `src/compositor/tests/input_output/output_keyboard_cursor.rs` if the production Wayland fixture is required.
- Inspect: `src/compositor/state/window_resize.rs`, `src/compositor/state/subsurfaces.rs`, `src/compositor/state/surface_commits.rs`.

**Failing test:** Add cache hit -> owner unmap/destroy -> stationary refresh -> new owner/None. Add immediate-resize hit parity: mutate current visual target to a new edge, hit before presentation, and assert the new geometry wins. Add combined A client/SSD input-region mutation preserving SSD ownership.

**Expected pre-fix behavior:** A cache entry can survive an ownership-invalidating teardown or a geometry-only visual target change if the relevant generation path is missed.

**Implementation:** Use existing unmap/destruction and visual geometry APIs; only add generation calls if the audit identifies a mutation bypassing the shared seam.

**Verification:** Run focused tests and assert popup-above-SSD and layer-shell ordering remain unchanged. Keep pointer constraints, implicit grabs, popup grabs, and DND precedence tests green.

**Commit boundary:** `test(input): cover ownership teardown and resize hit parity`.

### Task 5: Add real client↔SSD routing stress

**Files:**
- Modify: `src/compositor/tests/input_output/window_interaction.rs`.
- Modify: `src/compositor/tests/support/registry_state.rs` and `src/compositor/tests/support/registry_pointer.rs` only if event counters/timeline data are missing.
- Modify: `src/compositor/tests/support/server_runtime.rs` only if a read-only focus-generation or counter capture is needed.

**Failing test:** Create A front and B behind with A SSD. Send production `ServerCommand::PointerMotion` alternately to A client and A titlebar for at least 1,000 boundary transitions, with repeated titlebar/button/titlebar motion inside the SSD.

**Expected pre-fix behavior:** The test must expose any B enter, duplicate A leave, desktop-focus change, keyboard-focus change, or focus-generation churn. It must record the exact A enter -> leave -> enter sequence for one cycle.

**Implementation:** No direct scene-hit loop; exercise pointer motion dispatch, `PointerSceneHit`, pointer enter/leave, desktop focus, keyboard focus, and focus generation. If counters prove repeated same-window focus reconciliation is doing work, add a same-window no-op guard at the narrowest existing focus helper without suppressing decoration hover state.

**Verification:** Assert focused `WindowId == A`, keyboard owner A, stable focus generation, B pointer enter/activation/keyboard changes all zero, and no duplicate leave during same-SSD motion. Run the focused test repeatedly.

**Commit boundary:** `test(input): stress client decoration focus boundaries`; if a code guard is needed, a separate `fix(input): skip redundant same-window focus reconciliation`.

### Task 6: Make visual groups retain root indices

**Files:**
- Modify: `src/compositor/render.rs` (`VisualStackGroup`, `visual_stack_groups`, accessors, and group tests).
- Modify: `src/compositor/state/hit_testing.rs` to consume `root_surface_index` directly.

**Failing test:** Add/adjust a structural hit-test counter assertion requiring zero root linear searches for a group traversal.

**Expected pre-fix behavior:** `pointer_scene_hit_uncached()` calls `.position()` through all renderable surfaces for every group.

**Implementation:** Store `root_surface_index: usize` alongside `root_surface_id` when constructing each group. Preserve group surface order and popup semantics exactly. Replace root rediscovery with indexed `get()` and skip invalid/stale indices safely.

**Verification:** Run visual stack ordering, popup precedence, titlebar overlap, and pointer scene tests. Use `rtk rg` to confirm no root `.position()` remains in `pointer_scene_hit_uncached()`.

**Commit boundary:** `perf(input): remove redundant pointer scene root lookups`.

### Task 7: Remove origin-cache cloning with two-phase hit computation

**Files:**
- Modify: `src/compositor/state/hit_testing.rs`.
- Modify: `src/compositor/state/subsurfaces.rs` only if helper visibility/borrowing must be adjusted.

**Failing test:** Enable the deterministic instrumentation in a focused test and assert origin-cache clones remain zero during ordinary pointer motions.

**Expected pre-fix behavior:** `pointer_scene_hit_uncached()` clones the complete `surface_origin_cache` on every uncached call.

**Implementation:** Borrow `&self.surface_origin_cache`, `&self.visual_stack_groups_cache`, and `&self.renderable_surfaces` immutably during the hit calculation. End those borrows before assigning `pointer_scene_hit_cache`. Keep stable identity cloning only where the returned hit requires ownership.

**Verification:** Run focused hit tests, `cargo fmt --check`, `cargo check --locked --all-targets`, and structural grep. Do not use unsafe or a spatial index.

**Commit boundary:** `perf(input): avoid surface origin cache clone in hit testing`.

### Task 8: Add bounded disabled-by-default hit instrumentation

**Files:**
- Modify: `src/compositor/mod.rs` or `src/compositor/state/support_types.rs` for a bounded debug metrics structure following existing pointer-debug gating.
- Modify: `src/compositor/state/hit_testing.rs` to increment calls, hits, misses, groups/surfaces inspected, root searches, origin clones, and duration only when enabled.
- Modify: `src/compositor/server.rs` and `src/compositor/server_interaction.rs` only if a read-only test capture accessor is required.
- Test: `src/compositor/state/window_decoration_tests.rs`.

**Failing test:** With the debug switch enabled, feed 10,000 deterministic motions across client, SSD, buttons, and client; assert bounded one-entry cache behavior, no focus churn, zero origin clones, zero root linear searches, and a stable final hit. With the switch disabled, assert no debug allocation/counter side effect through the existing test seam.

**Expected pre-fix behavior:** No bounded hit-path counters exist, so clone/root-search costs cannot be proven.

**Implementation:** Keep counters disabled by default, use saturating counters and bounded sample storage if duration samples are needed, and never log/rasterize in normal motion dispatch.

**Verification:** Run the deterministic stress test with and without `TYPHON_POINTER_DEBUG`; record before/after structural metrics in the report without inventing wall-clock requirements.

**Commit boundary:** `perf(input): add bounded pointer hit instrumentation`.

### Task 9: Audit and align relevant resize-pacing tests

**Files:**
- Modify: `src/compositor/tests/windows.rs`.
- Inspect/modify only if needed: `src/compositor/interaction.rs`, `src/compositor/state/window_resize.rs`, `src/compositor/state/resize.rs`, `src/compositor/tests/support/window_ops.rs`.

**Failing test:** Baseline failures are `prepare_frame_flushes_queued_resize_configure_before_present_frame`, `queued_resize_configure_reports_pending_frame_work`, `resize_drag_coalesces_pointer_updates_behind_in_flight_configure`, and `resize_drag_does_not_send_next_configure_without_client_progress`.

**Expected pre-fix behavior:** Two tests observe same-dispatch configure flushing rather than an old frame boundary; two tests still expect one-configure serialization (`2` configures) despite the approved bound of `3`.

**Implementation:** First inspect helper timing and configure ledger state. Update obsolete assertions to `outstanding <= 3`, latest target, bounded queued target, final target preservation, and correct ACK/capture ownership. Fix production state only if a failure demonstrates lost final geometry, unbounded pressure, wrong ACK ownership, stale rollback, or incorrect `resizing=false`.

**Verification:** Run all resize flow unit tests, slow-client 1,000-update boundedness, fast-client prompt ACK/commit throughput, and the four previously failing tests. Do not weaken responsiveness assertions or restore serial behavior.

**Commit boundary:** `test(resize): align pacing tests with bounded configure window` or a narrowly named production fix if required.

### Task 10: Run pointer/input/render regression matrix

**Files:**
- Modify tests only when a failure encodes obsolete semantics; otherwise no production changes to render-ahead/fullscreen systems.

**Failing test:** Focused matrix includes stationary input-region both directions, combined SSD/input region, popup above SSD, layer ordering, XWayland current hit behavior, destruction, constraints/grabs, titlebar overlap, resize edge parity, and the existing move/resize framebuffer-reference tests.

**Expected pre-fix behavior:** Any missed generation seam or ordering regression appears as stale owner, B enter/focus, stale resize edge, popup/layer precedence change, or ghosting/buffer-age failure.

**Implementation:** Fix only the scoped pointer-generation/root-index/borrow changes. Do not modify presentation-domain damage journal, buffer-age semantics, or fullscreen frame-scene authority for unrelated failures.

**Verification:** Require age 1/2/3 green where those tests exist, no persistent move/resize ghost reproduction in the deterministic framebuffer regressions, and unchanged popup/layer/constraint precedence.

**Commit boundary:** `test(input): qualify pointer scene cache and resize/render regressions`.

### Task 11: Native qualification if available

**Files:**
- No source changes unless a directly observed scoped failure requires one.

**Failing test:** Native TTY command with the supplied Astrea shell, if DRM/session access is available.

**Expected pre-fix behavior:** Only use this task to observe native titlebar/client/button transitions and resize edge/corner behavior; do not claim native proof from unit tests.

**Implementation:** Run overlapping windows, alternate A client/titlebar/buttons, resize all edges/corners and reverse direction, and record whether DRM `TEST_ONLY` or session permissions block qualification.

**Verification:** Report native titlebar and resize results separately as NATIVE-PROVEN or UNPROVEN/BLOCKED. Never claim success if native access is unavailable.

**Commit boundary:** No commit.

### Task 12: Final verification and report

**Files:**
- Create: `REPORT-2026-08-18-pointer-scene-cache-focus-churn-closure.md`.
- Modify: `docs/superpowers/specs/2026-08-18-pointer-scene-cache-focus-churn-design.md` only for final status corrections.
- Modify: `docs/superpowers/plans/2026-08-18-pointer-scene-cache-focus-churn-closure.md` to mark completed steps if desired.

**Failing test:** Any final validation command that fails remains an explicitly reported blocker; do not turn failures into claims.

**Expected pre-fix behavior:** Baseline is `1654 passed; 40 failed; 2 ignored`; the final report must compare against this exact run and classify every remaining failure group.

**Implementation:** Run, through `rtk` where applicable: `cargo fmt --check`, `cargo check --locked --all-targets`, `cargo test --locked`, `cargo clippy --locked --all-targets -- -D warnings`, `bash bin/check-source-layout`, and `git diff --check`. Capture final `rtk git status --short`, baseline/final HEAD, dirty-state names/stat, and any commits actually made.

**Verification:** The report must answer every requested self-review question with code/test evidence, include client↔SSD 1,000-cycle counts, enter/leave/focus generation results, hot-path before/after counters, resize classification table, render-ahead/buffer-age results, native status, remaining blockers, and final status.

**Commit boundary:** `docs: report pointer scene cache focus churn qualification` only if the user later requests commits; otherwise leave the dirty baseline and report that no commits were created.
