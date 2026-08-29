# Typhon Pointer-Lock Warp Invariant Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task with inline execution. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make active backend-confirmed locked pointers immutable to ordinary client warps while preserving relative motion, teardown restoration, and confinement.

**Architecture:** Keep the existing compositor generation and backend acknowledgment ownership model. Reject an ordinary warp at the compositor boundary before it changes `last_pointer_x/y` or queues backend work, and independently make the native backend reject a normal `WarpPointer` while its active constraint is `Locked`. Lock teardown continues to use its explicit `Deactivate { restore_position }` path.

**Tech Stack:** Rust, Smithay/Wayland server and client test harness, Cargo, `rtk` command wrappers.

## Global Constraints

- Ignore normal client pointer warps while a backend-confirmed locked-pointer generation is active.
- Preserve relative motion deltas exactly and do not synthesize relative motion for ignored warps.
- Do not defer client warps or change pointer-warp capability advertisement.
- Do not change confinement, pointer focus architecture, mouse acceleration, cursor rendering, or unrelated input code.
- Preserve committed cursor-position hint and activation-anchor restoration during authorized lock teardown.
- Preserve typed `PointerConstraintBackendRequest` ownership and generation validation.
- Do not claim Sober/Roblox runtime qualification unless Sober is actually run or the user validates it interactively.

---

## File map

- `src/compositor/tests/input_output/pointer_cursor.rs`: Wayland integration regression that drives a focused surface through lock, valid client warp, relative motion, and unlock.
- `src/native_output/tests/input.rs`: unit tests for the native pointer-constraint backend’s locked and unlocked `WarpPointer` behavior.
- `src/compositor/state/pointer_constraints.rs`: compositor-side early return that preserves authoritative absolute position and avoids normal backend warp work during an active lock.
- `src/native_output/runtime/frame.rs`: native backend defense-in-depth guard using its existing active constraint state.
- Existing files reviewed but not expected to change: capability declarations, pointer focus/hit testing, native input state/routing, lifecycle tests, and existing hint/confinement/generation tests.

## Task 1: Add the deterministic regression tests

**Files:**

- Modify: `src/compositor/tests/input_output/pointer_cursor.rs` near the existing pointer-warp tests.
- Modify: `src/native_output/tests/input.rs` near the existing native pointer-constraint backend tests.

**Interfaces:**

- Consume the existing `spawn_controllable_test_server`, `activate_backend_locked_pointer`, `capture_pointer_constraint_backend_requests`, `RegistryTestState`, `PointerMotionSample`, and `NativePointerConstraintBackend::handle_request` helpers.
- Produce the regression test names `locked_pointer_warp_is_ignored_while_active`, `native_pointer_constraint_backend_ignores_warp_while_locked`, and `native_pointer_constraint_backend_warps_when_unlocked`.

- [ ] **Step 1: Write the compositor integration test without changing production code.**

  In `pointer_cursor.rs`, add `locked_pointer_warp_is_ignored_while_active` using capabilities `{ pointer_constraints: true, pointer_warp: true, relative_pointer: true, ..desktop_baseline() }`. Create a 160x120 buffered toplevel, obtain a pointer, relative pointer, pointer-constraints manager, and pointer-warp manager, and move the server pointer to `FIRST_SURFACE_OFFSET + (20, 14)`. Save the enter serial and the anchor position.

  Create a persistent locked pointer, process the request, and call `activate_backend_locked_pointer` so the test is explicitly after backend confirmation. Clear the captured activation requests and client motion state. Issue `warp_pointer(&surface, &pointer, 80.0, 60.0, serial)`, flush, wait for server work, and roundtrip the client. Capture `last_pointer_x/y` and assert it is exactly the anchor; assert `state.pointer_motion` is false; assert no `WarpPointer` request was captured; assert `state.relative_motion_count == 0`; and assert the lock remains active (`locked_count == 1`, `unlocked_count == 0`).

  Then send one `PointerMotionSample` with an absolute position different from the anchor and relative values `(dx, dy, dx_unaccelerated, dy_unaccelerated) = (9.25, -4.5, 10.75, -6.0)`. Assert the compositor position remains the anchor, `pointer_motion` remains false, exactly one relative event is received, and all four relative values match exactly. This proves an ignored warp does not alter the absolute baseline or create relative motion while physical relative motion still flows.

  Finally commit a valid cursor-position hint at `(70.0, 50.0)`, destroy the lock, flush, wait, and roundtrip. Assert the deactivation request carries the existing hint-derived output position, the relative event count remains one, and the existing resource-destruction path does not synthesize an `unlocked` event. Stop and join the test server on every exit path using the established test style.

- [ ] **Step 2: Write the native backend locked-warp test.**

  In `native_output/tests/input.rs`, activate `NativePointerConstraintBackend` with id `{ constraint_id: 13, generation: 1 }` and anchor `(100.25, 80.75)`. Send `PointerConstraintBackendRequest::WarpPointer { position: (240.0, 160.0) }` to `handle_request`. Assert the complete action is `NativePointerConstraintBackendAction::default()`, assert `backend.active_locked()`, and assert `backend.active_constraint_state()` still reports `Locked { anchor }`.

- [ ] **Step 3: Write the native backend unlocked-warp test.**

  In the same native test module, create a new backend without activating a constraint, send `WarpPointer { position: (240.0, 160.0) }`, and assert `action.cursor_position == Some(position)`, with no activation, deactivation, failure, or visibility side effects. This locks in the existing unlocked behavior.

- [ ] **Step 4: Run the new tests and verify the expected RED failures.**

  Run:

  ```bash
  rtk cargo test --locked locked_pointer_warp_is_ignored_while_active -- --exact --nocapture
  rtk cargo test --locked native_pointer_constraint_backend_ignores_warp_while_locked -- --exact --nocapture
  rtk cargo test --locked native_pointer_constraint_backend_warps_when_unlocked -- --exact --nocapture
  ```

  Expected result: the compositor test fails because the accepted warp changes `last_pointer_x/y` and queues `WarpPointer`; the locked native test fails because the backend returns `cursor_position: Some((240.0, 160.0))`; the unlocked native test passes. If a test fails to compile, correct only the test code and rerun until the failure is behavioral and directly demonstrates the existing bug.

- [ ] **Step 5: Commit only the regression tests.**

  ```bash
  rtk git add src/compositor/tests/input_output/pointer_cursor.rs src/native_output/tests/input.rs
  rtk git commit -m "test: cover pointer warp during active lock"
  ```

## Task 2: Implement the compositor and native invariant guards

**Files:**

- Modify: `src/compositor/state/pointer_constraints.rs:1093-1112` (`CompositorState::apply_pointer_warp`).
- Modify: `src/native_output/runtime/frame.rs:624-680` (`NativePointerConstraintBackend::active_locked` and `handle_request`).

**Interfaces:**

- Consume the existing `active_locked_pointer_binding() -> Option<ActiveLockedPointerRouting>` generation-validated query and `NativePointerConstraintBackend::active_locked() -> bool` state query.
- Preserve `apply_pointer_warp(position: OutputPosition, send_motion: bool)` and `handle_request(request, cursor_position) -> NativePointerConstraintBackendAction` signatures.

- [ ] **Step 1: Add the compositor early return.**

  At the first executable line of `apply_pointer_warp`, check `self.active_locked_pointer_binding().is_some()`. When true, emit a pointer-debug message with an explicit ignored/suppressed reason such as `pointer warp ignored reason=active_lock`, then return. The check must precede the `before` snapshot, `update_pointer_position`, `pending_pointer_constraint_backend_requests.push(WarpPointer { ... })`, and `send_pointer_motion_after_warp` call. Leave the request validation and pending-unlock matching logic unchanged; teardown restoration does not call this ordinary path while an active lock owns the pointer.

- [ ] **Step 2: Make the native active-lock query production-usable.**

  Remove only the `#[cfg(test)]` restriction from `NativePointerConstraintBackend::active_locked`. Keep it as the single query over `self.active` and the `PointerConstraintMode::Locked` value; do not introduce another lock field or duplicate generation state.

- [ ] **Step 3: Guard normal native warps.**

  In the `PointerConstraintBackendRequest::WarpPointer` arm, check `self.active_locked()` before constructing the cursor-position action. If locked, log a lazy native pointer-debug message including `reason=active_lock` and return `NativePointerConstraintBackendAction::default()`. If unlocked, preserve the existing debug message and `cursor_position: Some(position)` action exactly.

- [ ] **Step 4: Run the new tests GREEN.**

  ```bash
  rtk cargo test --locked locked_pointer_warp_is_ignored_while_active -- --exact --nocapture
  rtk cargo test --locked native_pointer_constraint_backend_ignores_warp_while_locked -- --exact --nocapture
  rtk cargo test --locked native_pointer_constraint_backend_warps_when_unlocked -- --exact --nocapture
  ```

  Expected result: all three pass. Also run the adjacent pointer-lock and warp tests to verify existing behavior remains intact:

  ```bash
  rtk cargo test --locked compositor::tests::input_output::pointer_cursor -- --nocapture
  rtk cargo test --locked compositor::tests::input_output::relative_and_constraints -- --nocapture
  rtk cargo test --locked native_pointer_constraint_backend -- --nocapture
  ```

- [ ] **Step 5: Review the diff and commit the production fix.**

  Confirm the diff contains only the two guards, their focused tests, and no capability or unrelated input changes, then run:

  ```bash
  rtk run "git diff --check"
  rtk git add src/compositor/state/pointer_constraints.rs src/native_output/runtime/frame.rs
  rtk git commit -m "fix: ignore pointer warps during active locks"
  ```

## Task 3: Verify lifecycle coverage and repository quality

**Files:**

- Inspect only: existing lock lifecycle, hint restoration, confinement, and stale-generation tests.

**Interfaces:**

- Verify existing tests continue to cover persistent deactivate/reactivate, oneshot constraints, pointer/surface destruction, focus loss, backend failure/cancellation, stale acknowledgments, pending reveal, committed hints, activation-anchor fallback, and confinement.

- [ ] **Step 1: Run the focused lifecycle tests.**

  ```bash
  rtk cargo test --locked locked_pointer_destroy_restores_committed_cursor_position_hint -- --exact --nocapture
  rtk cargo test --locked locked_pointer_unlock_without_hint_restores_exact_activation_anchor -- --exact --nocapture
  rtk cargo test --locked locked_unlock_does_not_reveal_committed_hint_before_followup_warp -- --exact --nocapture
  rtk cargo test --locked confined_pointer_motion_beyond_window_border_clamps_without_leave_or_unconfined -- --exact --nocapture
  rtk cargo test --locked native_pointer_constraint_backend_mismatched_deactivation_cannot_unlock_newer_lock -- --exact --nocapture
  ```

- [ ] **Step 2: Run the repository quality gates through `rtk`.**

  ```bash
  rtk cargo fmt -- --check
  rtk cargo check --locked --all-targets
  rtk cargo clippy --locked --all-targets -- -D warnings
  rtk cargo test --locked
  rtk run "./bin/check-source-layout"
  rtk run "git diff --check"
  ```

  Record each command’s exit status and exact blocker if the environment prevents it. Do not suppress warnings or weaken tests.

- [ ] **Step 3: Review the final changed-file set and report qualification boundaries.**

  Use `rtk git status --short` and `rtk git diff HEAD~2 --stat` to confirm the two pointer-lock commits are independently reviewable and the pre-existing frame/damage worktree edits were not staged. Report the RED failure, GREEN results, all gates, the confirmed state sequence, the KWin/Hyprland comparison, the unbundled pointer-warp enter-serial follow-up, and whether Sober was actually runtime-tested.
