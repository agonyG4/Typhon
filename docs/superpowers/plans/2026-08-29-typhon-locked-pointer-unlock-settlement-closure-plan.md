# Typhon Locked Pointer Unlock Settlement Closure Implementation Plan

> **Execution:** Inline only, as explicitly requested by the user. Do not dispatch subagents or use the `rtk` wrapper. Steps use checkbox syntax for tracking.

**Goal:** Make backend restore acknowledgement a prerequisite to, rather than a replacement for, final locked-pointer unlock settlement, while restoring pending-oneshot legacy compatibility.

**Architecture:** Extend the existing `PendingLockedPointerReveal` with backend-settled epoch and client-warp fields. Small helpers will mark backend settlement, record a validated client warp, settle fallback after the existing dispatch grace, and perform the only final reveal. Cursor-choice handlers remain visual-state-only. The existing native request drain continues to order queued restore/warp/visibility work.

**Tech Stack:** Rust, Smithay/Wayland server and client test harness, Cargo, existing native pointer-constraint backend.

## Global Constraints

- Preserve active locked-pointer warp rejection, exact relative motion, current pointer-enter serial authority, confinement resolution, same-client cross-surface warps, focus leave/enter sequencing, implicit grabs, and mixed v11/legacy delivery.
- Backend ACK is a prerequisite, not final settlement.
- Fallback grace begins at backend restore settlement and uses the existing dispatch-epoch mechanism.
- Do not add sleeps, wall-clock synchronization, application-specific code, new dependency versions, or unrelated refactors.
- Keep active-lock restore legacy behavior event-silent and pending-oneshot hint warp legacy behavior as normal motion plus frame.
- Commit only scoped pointer changes and follow-up documentation; preserve all pre-existing unrelated worktree changes.

---

### Task 1: Add the failing unlock-settlement regressions

**Files:**

- Modify: `src/compositor/tests/input_output/pointer_warp_serial.rs`
- Modify: `src/compositor/tests/input_output/pointer_cursor.rs`
- Modify: `src/compositor/tests/input_output/relative_and_constraints.rs`
- Modify: `src/compositor/tests/support/server_runtime.rs` only if a narrowly scoped pending-state capture command is needed by the assertions.

**Interfaces:**

- Consume the existing controllable server, `ServerCommand::PointerConstraintBackendDeactivated`, `capture_pointer_constraint_backend_requests`, pointer event logs, and `RegistryTestState` fields.
- Produce deterministic tests for ACK-before-warp, warp-before-ACK, cursor-state separation, post-ACK fallback, same-client cross-surface matching, wrong-client rejection, and v7/v11 pending-oneshot delivery.

- [ ] **Step 1: Extend the v11 lock-restore test to prove ACK is not final settlement.**

  In `v11_lock_restore_uses_warp_without_relative_motion`, capture the deactivation id, send `PointerConstraintBackendDeactivated`, process exactly that callback cycle, and assert the event log is empty and no `ApplyCursorVisibility { visible: true }` request is present. Then drive the existing short dispatch grace, assert one `warp + frame` at the committed fallback, zero relative motion, and visibility release.

- [ ] **Step 2: Add a client-warp-before-ACK integration test.**

  Lock a v11 pointer with committed fallback C, destroy the lock, send a valid warp to B before sending the backend ACK, and assert B is the compositor position, the client event is only the normal client-warp delivery, and visibility remains hidden. Send the matching backend ACK and assert queued request order is `WarpPointer(B)` before `ApplyCursorVisibility(true)` with no fallback C event.

- [ ] **Step 3: Add a client-warp-after-ACK integration test.**

  Lock a v11 pointer with fallback C, destroy it, send the backend ACK, process one cycle, and assert no fallback event or visibility release occurred. Send a valid warp to B and assert B is final, the only same-focus reposition event is `warp + frame`, and queued visibility follows the final native warp.

- [ ] **Step 4: Add cursor-state separation coverage.**

  In the pending-unlock window, issue a valid `set_cursor(NULL)` and assert no visibility release occurs. In a separate or extended case, issue a valid cursor shape/surface request before the final warp and backend settlement; assert the cursor choice is accepted but the pending unlock remains hidden until the position transaction settles. Use existing cursor test support and do not add visibility-specific production hooks unless the test cannot observe the behavior otherwise.

- [ ] **Step 5: Add same-client cross-surface pending-unlock coverage.**

  Create surfaces A and B in one client, lock A, destroy the active lock, send a current-enter-serial warp targeting B, and then acknowledge backend restore. Assert the focus log is `leave, frame, enter, frame` as appropriate, B remains the final compositor/focus position, no later fallback event targets A, and visibility releases only after settlement. Keep the existing separate wrong-client test and assert its stale client warp does not resolve the pending transaction.

- [ ] **Step 6: Add pending-oneshot compatibility matrix coverage.**

  Extend the pending oneshot test setup with a committed valid hint C. For a seat/pointer resource at version 7, destroy before backend activation and assert absolute position C, `WarpPointer(C)`, legacy `motion + frame`, no locked/unlocked events. Repeat with a version-11 pointer and assert `warp + frame`, no motion. Continue asserting the queued activation was canceled.

- [ ] **Step 7: Run the focused test set before production changes.**

  Run:

  ```bash
  cargo test --locked pointer_warp_serial -- --nocapture
  cargo test --locked pointer_cursor -- --nocapture
  cargo test --locked relative_and_constraints -- --nocapture
  cargo test --locked native_pointer_constraint_backend -- --nocapture
  ```

  Expected: the newly strengthened tests fail against the current implementation because backend ACK/cursor requests finalize early, fallback is not ACK-gated, cross-surface matching is rejected, and legacy pending-oneshot delivery is suppressed. Existing unrelated tests must not be modified to make the failures pass.

---

### Task 2: Implement explicit pending-unlock ownership and cause semantics

**Files:**

- Modify: `src/compositor/mod.rs` (`PendingLockedPointerReveal`)
- Modify: `src/compositor/state/pointer_constraints.rs` (causes and settlement helpers)
- Modify: `src/compositor/state/hit_testing.rs` (pending matcher and client-warp recording)
- Modify: `src/compositor/state/input_resources.rs` (remove cursor-driven settlement)

**Interfaces:**

- Consume the RED tests and existing `PointerConstraintBackendId`, `dispatch_epoch`, `OutputPosition`, and cursor visibility state.
- Produce `mark_pending_locked_pointer_backend_settled`, `record_pending_locked_pointer_client_warp`, `try_settle_pending_locked_pointer_reveal`, and fallback settlement behavior through the existing `finalize_pending_locked_pointer_reveal` boundary. Names may follow project conventions, but finalization must only occur after both required facts or post-ACK fallback grace.

- [ ] **Step 1: Add pending ownership fields and initialize them at unlock creation.**

  Replace `created_dispatch_epoch` with `backend_restore_settled: false`, `backend_settled_dispatch_epoch: None`, and `client_warp_position: None`. Keep `backend_id`, pointer, lock surface, and fallback position unchanged. Do not change hint/anchor selection or lock-hidden initialization.

- [ ] **Step 2: Split reposition causes.**

  Define exactly these three causes and string labels:

  ```rust
  enum PointerRepositionCause {
      ClientWarp,
      ActiveLockedPointerRestore,
      PendingOneshotHintWarp,
  }
  ```

  Update all uses and keep `send_pointer_reposition_to_resources` behavior as follows: `ActiveLockedPointerRestore` suppresses legacy events; the other two causes send legacy motion plus frame; v11 resources use warp plus frame for all three. Focus crossings remain enter/leave-only.

- [ ] **Step 3: Make finalization represent actual settlement.**

  Keep `finalize_pending_locked_pointer_reveal` as the only function that clears `lock_hidden_constraint_id` and calls `sync_cursor_visibility_request`. Add a helper that finalizes only when `backend_restore_settled && client_warp_position.is_some()`. Add a fallback helper that, after ACK grace, delivers `fallback_position` once as `ActiveLockedPointerRestore` and then calls finalization. No helper may finalize merely because a cursor request or backend ACK occurred.

- [ ] **Step 4: Gate fallback grace on backend settlement.**

  Update `finalize_pending_locked_pointer_reveal_after_dispatch` to read `backend_settled_dispatch_epoch`. Return while it is `None`; once the existing `saturating_add(2) < dispatch_epoch` threshold is met and no client warp is recorded, settle the fallback. Preserve the existing short dispatch grace and do not introduce a wall-clock timer.

- [ ] **Step 5: Mark matching backend ACKs without publishing fallback immediately.**

  Preserve current id/generation validation, including the pending-record path when the constraint map no longer contains the record. After the native restore action has settled, mark the matching pending record settled at `self.dispatch_epoch`, then attempt only the two-fact client-warp settlement. If there is no client warp, leave it pending for dispatch grace. If an unusual backend deactivation creates the pending record while deactivating an active constraint, mark that newly created matching record settled in the same callback so its fallback grace is valid.

- [ ] **Step 6: Relax only the pending matcher to same-client surfaces.**

  Require the same `wl_pointer` resource and `pending.surface.id().same_client_as(&surface.id())`. Do not require exact surface identity. Keep normal warp validation as the authority for pointer ownership, current enter serial, coordinates, focus, and wrong-client rejection.

- [ ] **Step 7: Record the resolved client warp and remove cursor settlement.**

  Have `apply_pointer_warp` return the resolved output position, or use an equivalent existing-project result type, so a confined resolution cannot be recorded as the unbounded request. Record that result in the pending record and call the two-fact settlement helper. Remove all `finalize_pending_locked_pointer_reveal` calls and pending-resolution booleans from `set_pointer_cursor` and `set_pointer_shape`; retain their normal cursor-choice, render-generation, and visibility-state updates.

- [ ] **Step 8: Run focused GREEN tests.**

  Run the exact focused commands from Task 1. Expected: all new settlement, cursor, cross-surface, and oneshot matrix tests pass, with no active-lock or existing pointer test regressions.

---

### Task 3: Verify native ordering and stale-generation boundaries

**Files:**

- Modify: `src/native_output/input/routing.rs` only if implementation evidence shows an ordering change is required.
- Modify: `src/native_output/runtime/cycle_dispatch.rs` only if implementation evidence shows the existing dispatch boundary cannot service the ACK epoch.
- Modify: `src/native_output/tests/input.rs` only for a focused request-order or stale-ACK assertion not covered by existing tests.

**Interfaces:**

- Consume `process_native_pointer_constraint_backend_requests` and the existing `NativePointerConstraintBackendAction` tests.
- Produce evidence that native requests are drained in ordered batches and repeated for newly queued work, with stale backend ids ignored.

- [ ] **Step 1: Inspect the focused GREEN request traces/assertions.**

  Confirm the compositor queue presents `Deactivate` before a pre-ACK `WarpPointer`, and `WarpPointer` before the visibility request created by settlement. Confirm the no-warp path does not enqueue a duplicate fallback warp after the native restore action.

- [ ] **Step 2: Add only necessary native assertions.**

  Preserve the existing active-lock warp no-op, unlocked warp, confinement, deactivation, and stale-generation backend tests. If an assertion is needed, add it to `src/native_output/tests/input.rs` against real `NativePointerConstraintBackend::handle_request` behavior; do not modify the backend ownership model.

- [ ] **Step 3: Run surrounding pointer/native tests.**

  Run:

  ```bash
  cargo test --locked pointer -- --nocapture
  cargo test --locked input -- --nocapture
  ```

  Expected: zero failures, with no new native absolute movement while an active lock owns the pointer.

---

### Task 4: Update the follow-up plan and perform repository verification

**Files:**

- Modify: `docs/superpowers/specs/2026-08-29-typhon-locked-pointer-unlock-settlement-closure-design.md` only if implementation naming or an observed boundary needs correction.
- Modify: this plan to check completed steps and record actual commands/results.

- [ ] **Step 1: Run formatting and static checks.**

  ```bash
  cargo fmt --check
  cargo check --locked --all-targets
  cargo clippy --locked --all-targets -- -D warnings
  ```

- [ ] **Step 2: Run the full test suite and whitespace check.**

  ```bash
  cargo test --locked
  git diff --check
  ```

  Record exact failures or environmental blockers; do not claim green results from partial output.

- [ ] **Step 3: Search the final code for premature finalization and cause coverage.**

  ```bash
  rg -n 'finalize_pending_locked_pointer_reveal|PointerRepositionCause::(ClientWarp|ActiveLockedPointerRestore|PendingOneshotHintWarp)' src/compositor
  ```

  Verify finalization is reached only from actual two-fact settlement or post-ACK fallback, cursor handlers contain no settlement call, and every cause has the intended legacy/v11 matrix.

- [ ] **Step 4: Audit all requested invariants and lifecycle cleanup.**

  Check active lock, enter serial, same-client crossing, confinement, implicit grab, v11/legacy delivery, active restore versus pending oneshot behavior, ACK ordering, warp before/after ACK, cursor requests, fallback, visibility ordering, surface/pointer/client destruction, stale generations, and restore-time relative-motion silence against source and tests.

- [ ] **Step 5: Commit only scoped follow-up changes.**

  Use explicit paths in `git add` and `git commit`; do not stage the pre-existing DMA-BUF/render files. Suggested commit message:

  ```bash
  git add src/compositor/mod.rs src/compositor/state/pointer_constraints.rs src/compositor/state/hit_testing.rs src/compositor/state/input_resources.rs src/compositor/tests/input_output/pointer_cursor.rs src/compositor/tests/input_output/pointer_warp_serial.rs src/compositor/tests/input_output/relative_and_constraints.rs src/compositor/tests/support/server_runtime.rs src/native_output/input/routing.rs src/native_output/runtime/cycle_dispatch.rs src/native_output/tests/input.rs docs/superpowers/plans/2026-08-29-typhon-locked-pointer-unlock-settlement-closure-plan.md
  git commit -m "fix: close locked pointer unlock settlement"
  ```

  Omit files with no diff from the actual staging command and verify the commit stat contains no unrelated paths.
