# Typhon Locked Cursor Visual Reveal Qualification v1.2.1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve a frozen cursor reveal snapshot when worker cursor-only preparation selects an independent submission instead of a sidecar.

**Architecture:** Keep `CursorRevealTraceSnapshot` in the existing v1.2 ownership chain. Change only the independent `PlaneDeltaPreparationSubmit` construction in `prepare_plane_delta()` to carry the input snapshot unchanged; the existing `queue_plane_delta()` and `KmsBundleOwners` path will then retain it through worker submission and physical binding.

**Tech Stack:** Rust, Cargo, existing Typhon native-output worker and cursor-trace test harness.

## Global Constraints

- Do not implement any visual cursor teleport fix.
- Do not redesign cursor presentation ownership.
- Only trace ownership propagation changes.
- Do not modify cursor visible state, coordinates, image, hotspot, cursor scheduling, plane policy, worker admission policy, sidecar policy, KMS commit ordering, pageflip ordering, or pointer lock behavior.
- Preserve trace-disabled neutrality: `cursor_reveal_trace=None` must remain `None` through preparation and worker ownership.
- Preserve sidecar ownership and promoted-sidecar ownership unchanged.
- Use `/home/agony/.local/bin/rtk` for every shell command.
- Do not use subagents; execute this plan inline in the current checkout.
- Compile in the current checkout folder.

---

### Task 1: Add the failing independent-preparation regression test

**Files:**
- Modify: `src/native_output/runtime/plane_cycle_tests.rs`
- Read: `src/native_output/runtime/plane_cycle.rs`

**Interfaces:**
- Consumes: `prepare_plane_delta()`, `PlaneDeltaPreparation`, `CursorRevealTraceSnapshot`, and the existing `AcceptingExecutor`, `target()`, `test_cursor_for_worker()`, and `KmsValidationBase` helpers.
- Produces: A deterministic test named `independent_plane_delta_preparation_retains_frozen_reveal_without_sidecar` that passes a known `Some(snapshot)` into an independent no-sidecar preparation and asserts the returned `PlaneDeltaPreparationSubmit.cursor_reveal_trace` is that exact snapshot.

- [ ] **Step 1: Add a deterministic snapshot helper and test.**

  In `src/native_output/runtime/plane_cycle_tests.rs`, import `CursorRevealTraceSnapshot`, `CursorCoupling`, `CursorPlanePoint`, `CursorSource`, and `PresentedCursorState`. Build the snapshot with `CursorRevealTraceSnapshot::from_presented()` using authority `{ constraint_id: 17, generation: 117 }`, final position `{ x: 100.0, y: 80.0 }`, `visibility_requested: true`, epoch `Some(23)`, and a visible hardware `PresentedCursorState` whose source is `Some(CursorSource::Client)`. Call `prepare_plane_delta()` with `attachable_primary: None`, `cursor_action: CursorPlaneAction::Independent`, `cursor_delivery: PresentedCursorDelivery::Hardware`, `cursor_surface_damage: None`, and `cursor_reveal_trace: Some(snapshot)`. Match `PlaneDeltaPreparation::Submit(preparation)` and assert `preparation.cursor_reveal_trace == Some(snapshot)`.

- [ ] **Step 2: Run the focused test and verify the RED failure.**

  Run:

  ```bash
  /home/agony/.local/bin/rtk cargo test --locked independent_plane_delta_preparation_retains_frozen_reveal_without_sidecar -- --test-threads=1
  ```

  Expected before implementation: the test compiles and fails because the returned field is `None` instead of `Some(snapshot)`.

- [ ] **Step 3: Commit the RED test.**

  ```bash
  /home/agony/.local/bin/rtk git add src/native_output/runtime/plane_cycle_tests.rs
  /home/agony/.local/bin/rtk git commit -m "test: cover independent cursor reveal retention"
  ```

### Task 2: Retain the snapshot through the independent worker path

**Files:**
- Modify: `src/native_output/runtime/plane_cycle.rs:347-462`
- Test: `src/native_output/runtime/plane_cycle_tests.rs`

**Interfaces:**
- Consumes: The existing `cursor_reveal_trace: Option<CursorRevealTraceSnapshot>` parameter of `prepare_plane_delta()`.
- Produces: `PlaneDeltaPreparationSubmit.cursor_reveal_trace` carrying the same `Option<CursorRevealTraceSnapshot>` for the independent path, with `queue_plane_delta()` continuing to call `owners.set_cursor_trace_reveal(cursor_reveal_trace)`.

- [ ] **Step 1: Make the minimal implementation change.**

  In the final `PlaneDeltaPreparationSubmit` literal in `prepare_plane_delta()`, replace:

  ```rust
  cursor_reveal_trace: None,
  ```

  with:

  ```rust
  cursor_reveal_trace,
  ```

  Do not change the promoted-sidecar branch, `try_offer_cursor_sidecar()`, snapshot construction, or worker owner APIs.

- [ ] **Step 2: Run the focused GREEN test and adjacent plane-cycle tests.**

  Run:

  ```bash
  /home/agony/.local/bin/rtk cargo test --locked independent_plane_delta_preparation_retains_frozen_reveal_without_sidecar -- --test-threads=1
  /home/agony/.local/bin/rtk cargo test --locked plane_cycle -- --test-threads=1
  ```

  Expected: both commands pass, including the existing promoted-sidecar validation test.

- [ ] **Step 3: Run the worker and trace-focused regression groups.**

  Run:

  ```bash
  /home/agony/.local/bin/rtk cargo test --locked worker -- --test-threads=1
  /home/agony/.local/bin/rtk cargo test --locked cursor_reveal -- --test-threads=1
  /home/agony/.local/bin/rtk cargo test --locked sidecar -- --test-threads=1
  ```

  Expected: all tests pass; no snapshot is fabricated when the input is `None`, sidecar trace ownership remains intact, and existing physical/pageflip trace tests remain green.

- [ ] **Step 4: Commit the implementation.**

  ```bash
  /home/agony/.local/bin/rtk git add src/native_output/runtime/plane_cycle.rs src/native_output/runtime/plane_cycle_tests.rs
  /home/agony/.local/bin/rtk git commit -m "fix: retain worker cursor reveal ownership"
  ```

### Task 3: Update the v1.2 report and perform final verification

**Files:**
- Modify: `docs/superpowers/specs/2026-09-04-typhon-locked-cursor-visual-reveal-qualification-v1.1-report.md`

**Interfaces:**
- Consumes: The committed implementation and focused test results.
- Produces: The existing English report updated to v1.2.1 with the worker cursor-only/no-sidecar closure and exact verification results.

- [ ] **Step 1: Update the report without reopening Gate 2.**

  Add a v1.2.1 subsection documenting that the worker cursor-only snapshot survives both sidecar and independent transport selection, that sidecar rejection cannot discard frozen reveal identity, and that the first hidden-to-visible worker cursor pageflip retains exact reveal attribution. Preserve the explicit statement that no Gate 2 semantic cursor correction was implemented.

- [ ] **Step 2: Run the complete required verification commands.**

  Run each command from the current checkout:

  ```bash
  /home/agony/.local/bin/rtk cargo fmt --check
  /home/agony/.local/bin/rtk cargo check --locked --all-targets
  /home/agony/.local/bin/rtk cargo clippy --locked --all-targets -- -D warnings
  /home/agony/.local/bin/rtk cargo test --locked
  /home/agony/.local/bin/rtk git diff --check
  ```

  Record exact pass/failure counts. If a command fails because of unrelated newer work already present in the checkout, report that failure separately rather than changing unrelated files.

- [ ] **Step 3: Commit the report.**

  ```bash
  /home/agony/.local/bin/rtk git add docs/superpowers/specs/2026-09-04-typhon-locked-cursor-visual-reveal-qualification-v1.1-report.md
  /home/agony/.local/bin/rtk git commit -m "docs: close worker cursor reveal retention"
  ```

- [ ] **Step 4: Inspect final ownership and worktree state.**

  Run:

  ```bash
  /home/agony/.local/bin/rtk git status --short
  /home/agony/.local/bin/rtk git log -6 --oneline --decorate
  ```

  Confirm the v1.2.1 commits contain only the scoped test, implementation, and report changes; leave unrelated user changes unstaged and untouched.
