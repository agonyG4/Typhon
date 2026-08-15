# Typhon Atomic Rendering & Predictive Triple Buffering Performance Closure

> **For the executing agent:** REQUIRED SUB-SKILL: Use `test-driven-development`
> for every production behavior slice. Use `verification-before-completion`
> before claiming completion.

**Goal:** Correct explicit Atomic EGL/GBM repaint lineage and worker pacing identity,
add low-overhead observability, and qualify Predictive Triple against Reactive
Double without disabling features or touching unrelated dirty work.

**Current checkout:** `/home/agony/GitHub/Typhon`, branch `main`, HEAD `2498ae0`.

**Validation principle:** Every production change starts with a focused failing
test, then the smallest implementation, then focused tests. Use the existing
`target/` cache; never run `cargo clean`.

## Task 1: Immutable pacing reservation settlement

**Files:** `src/native_output/pacing.rs`, related runtime worker files only if
required by compiler/tests.

1. Add a deterministic test where a worker captures frame `N`, active moves to
   ready before acknowledgement, and exact submit settles `N` successfully.
2. Add tests for cancellation after the same transition, stale-job rejection,
   exact-once settlement, and newer-frame preservation.
3. Run the focused pacing tests and capture the expected failure.
4. Implement exact-ID removal from active or ready roles; retain generation,
   token, and stale protections and keep trace observational.
5. Run pacing, worker rejection, and presentation-ready focused tests.

## Task 2: Bounded render/repaint telemetry

**Files:** existing runtime metrics/trace and control snapshot modules, plus
focused tests in their existing locations.

1. Add tests for render timing percentile snapshots, repaint mode/reason counts,
   age buckets, and repair/full pixel totals.
2. Implement bounded counters using `TimingSummary` and existing paint/frame
   statistics; do not allocate strings or write per-frame output in the render
   path.
3. Extend the existing diagnostic/control snapshot with one focused performance
   result, preserving compatibility for existing commands and clients.
4. Run focused metrics/control tests and inspect the hot-path diff for locks,
   syscalls, allocations, and logging.

## Task 3: Independent partial-render capability

**Files:** `src/egl_renderer/damage.rs`, capability detection and both EGL/GBM
  scanout backends, plus damage tests.

1. Add a failing planner test proving buffer age + render repair can select
   Partial when EGL swap-damage submission is unavailable.
2. Add a failing test proving unsupported render repair still selects Full with a
   reason.
3. Split the capability representation and make the planner require render
   repair, not EGLSurface swap damage.
4. Keep legacy EGLSurface swap-damage semantics intact; explicitly declare Atomic
   render repair only behind the proven slot/lineage invariants.
5. Run damage and both backend-focused tests.

## Task 4: Slot-local age and render-ahead lineage

**Files:** `src/egl_renderer/damage.rs`, `src/native_output/scanout/output_slot.rs`,
`output_swapchain.rs`, `atomic_egl_gbm.rs`, and existing lifecycle/model tests.

1. Replace the pending-invalidates-age test with a failing slot-local test:
   unrelated pending slot does not change the acquired valid slot's age.
2. Add model tests for age 0/1/2/history exhaustion and the A-current/B-pending/
   C-free render-ahead sequence.
3. Add tests that presented history advances only after pageflip, while failed,
   rejected, quarantined, generation-reset, resize, mode, and direct-scanout
   transitions invalidate only unproven lineage.
4. Implement the smallest slot-local age change and preserve existing ownership,
   fencing, TEST_ONLY, generation, and fallback behavior.
5. Run focused damage/swapchain/Atomic lifecycle tests and visual-equivalence
   model assertions (Partial repair coverage equals the Full reference damage).

## Task 5: Qualification, regression checks, and report

1. Run focused tests, `cargo fmt --check`, `cargo check --locked --all-targets`,
   `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`,
   `./bin/check-source-layout`, and `git diff --check`.
2. Run native Wayland `vkcube`, vkmark FIFO, Kitty idle/rapid-scroll, and the
   three fixed benchmark configurations when the current TTY/session supports
   launching the native Typhon compositor. Capture kernel, driver, governor,
   GPU, refresh, HEAD, binary hash, environment, and exact run results.
3. Do not tune the predictor unless post-fix repeatable data exceeds the stated
   regression threshold and reason counters identify the cause.
4. Review the complete diff against the KWin/Hyprland invariant, search for
   stale-lineage and pacing races, inspect hot-path costs, and verify no
   unrelated protocol work entered the patch.
5. Write `/home/agony/Typhon-perf/REPORT-2026-08-15-atomic-rendering-closure.md`
   with before/after metrics and `N/A` for unavailable measurements.
6. Commit only task files in reviewable commits, preserving all pre-existing
   unrelated modifications, then report branch, HEAD, commits, and final status.
