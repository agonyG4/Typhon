# Typhon X11 Iconic Toplevel Lifetime and Restore Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve the Astrea identity of managed X11 windows across Iconic Wayland-surface loss and restore them through the XWM backend boundary without changing genuine withdrawal semantics.

**Architecture:** Keep X11 publication eligibility attached to the admitted `DesktopWindow` and role policy, independent of `x11_surface_id`. Add a narrow compositor backend map command that translates to `XwmCommand::Map` for the persistent X11 handle. Store at most one pending activation intent keyed by `WindowId` and handle, completing it only when that same X11 window receives its replacement association.

**Tech Stack:** Rust, Smithay/Wayland protocol tests, Xwayland XWM command/event model, Cargo locked builds, `rtk` command proxy.

## Global Constraints

- Do not modify Eclipse.
- Do not reopen F09 viewport/damage work.
- Do not add Steam-, Proton-, Wine-, or application-specific behavior.
- Preserve the existing `consume_wm_unmap()` Iconic-versus-withdrawal boundary.
- Do not retain stale or synthetic Wayland surfaces to preserve task identity.
- Do not change XDG eligibility.
- Compile in the repository target directory with the locked toolchain; use `rtk` for shell commands.
- Do not use subagents; execute this plan inline.
- Preserve the unrelated pre-existing worktree modifications and commit only task-owned files.

## File map

- Modify `src/compositor/tests/support/server_runtime.rs` to expose controlled association events and a narrow live X11-window state capture for lifecycle tests.
- Modify `src/compositor/tests/toplevel_management.rs` to add protocol-boundary RED/GREEN tests for Iconic association removal, withdrawal, surface replacement, Restore, and Activate.
- Modify `src/compositor/tests/xwayland.rs` for direct compositor/backend command coverage where the protocol fixture would obscure the command boundary.
- Modify `src/compositor/toplevel_collection.rs` to remove the X11 render-surface requirement while retaining managed role filtering.
- Modify `src/compositor/window_backend.rs`, `src/compositor/state/desktop_windows.rs`, and `src/compositor/server_backend.rs` for the narrow `WindowBackendCommand::Map` path and X11 translation.
- Modify `src/compositor/mod.rs`, `src/compositor/state/windows.rs`, `src/compositor/state/desktop_windows.rs`, and `src/compositor/server_xwayland.rs` for the bounded pending activation intent and its completion/cancellation hooks.
- Modify `src/xwayland/xwm/commands.rs` and `src/xwayland/xwm/events_regression_tests.rs` so a backend map command safely starts a new map epoch for an existing Iconic record without weakening unmap confirmation.
- Do not modify Eclipse or the XWM `consume_wm_unmap()` implementation.

---

### Task 1: Build the asynchronous X11/Astrea lifecycle fixture and add publication regressions

**Files:**
- Modify: `src/compositor/tests/support/server_runtime.rs:300-320, 1470-1485`
- Modify: `src/compositor/tests/toplevel_management.rs:952-end`

**Interfaces:**
- Add `ServerCommand::ApplyXwaylandAssociationEvent { event: XwmAssociationEvent }` and dispatch it with `server.apply_xwayland_association_event(event)`.
- Add `ServerCommand::CaptureX11WindowState { window_id, reply }` returning `Option<(Option<u32>, bool)>`, where the tuple is `(x11_surface_id, minimized)`.
- Tests use the existing `ToplevelClientState` handle map and existing `WindowId`/X11 snapshot fixture.

- [ ] **Step 1: Add the controlled-event and state-capture test support only.**

  Extend the command enum and controllable-server match with the two exact commands above. Add small helper functions adjacent to the existing capture helpers so tests can send them without reaching into the server thread.

- [ ] **Step 2: Add the failing Iconic association-removal protocol test.**

  Add `iconic_managed_x11_association_loss_keeps_the_same_astrea_handle` to `src/compositor/tests/toplevel_management.rs`. Reuse the current real Wayland/Xwayland setup from `authorized_v2_exact_managed_x11_actions_complete_on_the_manager`, bind an authenticated v2 manager, record the sole handle’s proxy id, identifier, kind, and the admitted `WindowId`, then:

  ```rust
  handle.minimize(0, 10);
  manager_connection.flush().unwrap();
  queue.roundtrip(&mut state).unwrap();
  apply_association_event(
      &commands,
      XwmAssociationEvent::Removed {
          generation,
          window: x11_handle,
          surface_id,
      },
  );
  service_astrea_publication(&commands);
  manager_connection.flush().unwrap();
  queue.roundtrip(&mut state).unwrap();
  ```

  Assert the captured X11 state is `(None, true)`, the manager total is still `1`, the original handle is not closed, its identifier/kind are unchanged, its protocol state contains `MINIMIZED`, and no second `Toplevel` event was received. The current implementation must fail because the handle closes after eligibility becomes false.

- [ ] **Step 3: Run the new test and verify the semantic RED failure.**

  Run:

  ```bash
  rtk cargo test --locked iconic_managed_x11_association_loss_keeps_the_same_astrea_handle -- --exact --nocapture
  ```

  Expected: the test compiles and fails on the assertion that the existing Astrea handle remains live, not on fixture setup or protocol authentication.

- [ ] **Step 4: Add the failing genuine-withdrawal protocol test.**

  Add `withdrawn_managed_x11_closes_the_astrea_handle` using the same admission fixture, then send `XwmEvent::WindowWithdrawn(x11_handle)` through `ApplyXwaylandWindowEvent`. Service publication and roundtrip the manager. Assert `CaptureX11WindowState` returns `None`, the original `ClientToplevel.closed` is true, and the manager’s last `Done.total` is `0`. This test must remain a real withdrawal path, not an association removal.

- [ ] **Step 5: Run the withdrawal test and adjacent existing tests.**

  Run:

  ```bash
  rtk cargo test --locked withdrawn_managed_x11_closes_the_astrea_handle -- --exact --nocapture
  rtk cargo test --locked authorized_v2_exact_managed_x11_actions_complete_on_the_manager -- --exact --nocapture
  ```

  Record the current result; the new withdrawal test should pass before production changes if the existing teardown path is intact, while the Iconic test remains RED.

- [ ] **Step 6: Commit the fixture and regression tests after the intended RED/GREEN boundary is captured.**

  Do not commit a knowingly failing test alone. Keep the test changes in the worktree until the publication fix in Task 2 makes the new Iconic test green, then commit them with the production change that they validate.

### Task 2: Make X11 Astrea eligibility follow `DesktopWindow` lifetime

**Files:**
- Modify: `src/compositor/toplevel_collection.rs:73-101`
- Test: `src/compositor/tests/toplevel_management.rs` from Task 1

**Interfaces:**
- Preserve `astrea_toplevel_kind_if_eligible(window_id) -> Option<AstreaToplevelKind>`.
- Preserve the XDG branch exactly.
- Preserve the X11 role match for `Toplevel` and `Dialog` and the `DesktopWindowKind::Managed` requirement.

- [ ] **Step 1: Change only the X11 predicate after the RED test is recorded.**

  Replace the `x11_surface_id.and_then(surface_resource_by_id).is_some()` condition with the admitted managed-window condition. The resulting branch must be equivalent to:

  ```rust
  (window.kind == DesktopWindowKind::Managed).then_some(kind)
  ```

  The role match remains immediately above it, so auxiliary, override-redirect, notification, and popup roles remain excluded by the existing role policy.

- [ ] **Step 2: Run the Iconic publication test to verify GREEN.**

  Run:

  ```bash
  rtk cargo test --locked iconic_managed_x11_association_loss_keeps_the_same_astrea_handle -- --exact --nocapture
  rtk cargo test --locked withdrawn_managed_x11_closes_the_astrea_handle -- --exact --nocapture
  rtk cargo test --locked authorized_v2_exact_managed_x11_actions_complete_on_the_manager -- --exact --nocapture
  ```

  Expected: the Iconic handle remains open with `MINIMIZED`, the withdrawal test closes it, and the existing exact-action test remains green.

- [ ] **Step 3: Add and run exclusion assertions if the existing suite lacks them.**

  In `src/compositor/tests/xwayland.rs` or the nearest existing role-policy test module, add the smallest direct assertions that associationless override-redirect and auxiliary/client-leader windows still return no Astrea kind. Run those tests with:

  ```bash
  rtk cargo test --locked xwayland -- --nocapture
  ```

- [ ] **Step 4: Commit the publication-lifetime fix and tests.**

  ```bash
  rtk git add src/compositor/toplevel_collection.rs src/compositor/tests/support/server_runtime.rs src/compositor/tests/toplevel_management.rs src/compositor/tests/xwayland.rs
  rtk git commit -m "fix: retain Iconic X11 Astrea task identity"
  ```

### Task 3: Add RED coverage for same-window surface replacement and associationless Restore

**Files:**
- Modify: `src/compositor/tests/toplevel_management.rs`
- Modify: `src/compositor/tests/support/server_runtime.rs` only if a helper from Task 1 is insufficient

**Interfaces:**
- Reuse `XwmAssociationEvent::Associated`/`Removed`, `ServerCommand::ApplyXwaylandAssociationEvent`, and the existing Xwayland shell surface creation fixture.
- Capture the existing Astrea handle object id and `WindowId`; do not identify the task by surface id.

- [ ] **Step 1: Add the failing replacement-association test.**

  Add `x11_surface_replacement_reuses_window_id_and_astrea_handle`. Start from a ready managed X11 window with surface A, minimize it, remove A, verify `x11_surface_id == None`, then create and commit surface B, apply `Associated { generation, window: x11_handle, surface_id: B }`, and service publication. Assert:

  ```rust
  assert_eq!(captured_window_id, window_id);
  assert_eq!(state.handles.len(), 1);
  assert_eq!(state.handles[0].id(), original_handle_id);
  assert!(!state.toplevels[&original_handle_id].closed);
  assert!(!state.events.iter().any(|event| *event == "closed"));
  ```

  Also assert the live X11 state reports `Some(B)`, proving the old surface was not retained. The current code closes the Astrea handle before B arrives, so this test must be RED before Task 2 is applied; if Task 2 already makes the publication part green, keep the test’s identity/replacement assertions as the next regression.

- [ ] **Step 2: Add the failing exact Restore test from an associationless Iconic state.**

  Add `exact_restore_maps_an_associationless_iconic_x11_window`. After the same minimize/remove sequence, invoke `handle.restore(0, 20)`, roundtrip, and assert action result `Accepted`, the same handle remains open, the state is no longer `MINIMIZED`, and `CaptureXwaylandBackendCommands` contains `XwmCommand::Map(x11_handle)`. The current code may report `Accepted` but emits no map command, so the command assertion must fail for the expected semantic reason.

- [ ] **Step 3: Run both tests and record RED failures.**

  ```bash
  rtk cargo test --locked x11_surface_replacement_reuses_window_id_and_astrea_handle -- --exact --nocapture
  rtk cargo test --locked exact_restore_maps_an_associationless_iconic_x11_window -- --exact --nocapture
  ```

### Task 4: Add the narrow backend map command and XWM Iconic map authorization

**Files:**
- Modify: `src/compositor/window_backend.rs:6-50`
- Modify: `src/compositor/state/desktop_windows.rs:1478-1525`
- Modify: `src/compositor/server_backend.rs:123-300`
- Modify: `src/xwayland/xwm/commands.rs:38-92, 206-252`
- Modify: `src/compositor/tests/xwayland.rs`
- Modify: `src/xwayland/xwm/events_regression_tests.rs` near the existing Iconic map/unmap regressions

**Interfaces:**
- Add `WindowBackendCommand::Map { window: WindowId }`.
- Translate it only when the target `DesktopWindow` is `WindowBackend::X11(handle)` to `XwmCommand::Map(handle)`; drop it for XDG.
- Include it in `has_pending_xwayland_backend_commands`.

- [ ] **Step 1: Implement the minimal backend-command path after the Restore RED test.**

  Add the enum variant, add its X11 conversion in `take_xwayland_backend_commands`, and include it in the pending-command matcher. Add a focused test in `src/compositor/tests/xwayland.rs` that queues the command for a managed X11 `WindowId` and asserts the translated result is exactly `XwmCommand::Map(handle)`.

- [ ] **Step 2: Run the backend translation test and Restore test.**

  ```bash
  rtk cargo test --locked xwayland -- --nocapture
  rtk cargo test --locked exact_restore_maps_an_associationless_iconic_x11_window -- --exact --nocapture
  ```

  Expected: the backend test is green, while Restore remains RED if the compositor has not queued `Map` yet.

- [ ] **Step 3: Queue Map when restoring an associationless X11 window.**

  In `restore_minimized_desktop_window_contents`, capture whether the target is `WindowBackend::X11(_)` with `x11_surface_id == None`. After `WindowState::restore_minimized()` succeeds, queue `WindowBackendCommand::Map { window: window_id }` for that case, keep `queue_backend_state(window_id)` for X11 state publication, and continue to mark the Astrea snapshot dirty. Do not retain or recreate the old surface.

- [ ] **Step 4: Make `XwmCommand::Map` start the existing Iconic map epoch.**

  In the `XwmCommand::Map` execution branch, after command normalization confirms the handle exists and before `map_command_is_new`, call the XWM-owned `mark_map_requested(handle)` operation. This is idempotent for the existing first-map path and transitions an Iconic record into the normal remap gate. Keep `consume_wm_unmap`, `mark_wm_unmap_requested`, and `consume_wm_unmap` unchanged.

- [ ] **Step 5: Add the XWM regression for mapping an Iconic associationless record.**

  Add `map_command_reauthorizes_an_iconic_window_for_a_new_map_epoch` beside `wm_unmap_confirmation_is_iconic_and_restore_needs_a_new_buffer`. Prepare a managed record, put it through WM unmap confirmation and Wayland association removal, execute `XwmCommand::Map(handle)`, and assert the record is map-commanded/pending rather than rejected as a duplicate. Assert the real unmap confirmation test still emits no `WindowWithdrawn`.

- [ ] **Step 6: Run the Restore and XWM tests to verify GREEN.**

  ```bash
  rtk cargo test --locked exact_restore_maps_an_associationless_iconic_x11_window -- --exact --nocapture
  rtk cargo test --locked map_command_reauthorizes_an_iconic_window_for_a_new_map_epoch -- --exact --nocapture
  rtk cargo test --locked wm_unmap_confirmation_is_iconic_and_restore_needs_a_new_buffer -- --exact --nocapture
  ```

- [ ] **Step 7: Commit the restore/remap backend path.**

  ```bash
  rtk git add src/compositor/window_backend.rs src/compositor/state/desktop_windows.rs src/compositor/server_backend.rs src/xwayland/xwm/commands.rs src/compositor/tests/xwayland.rs src/xwayland/xwm/events_regression_tests.rs src/compositor/tests/toplevel_management.rs
  rtk git commit -m "fix: remap associationless Iconic X11 windows"
  ```

### Task 5: Add RED Activate coverage and bounded replacement-map activation

**Files:**
- Modify: `src/compositor/mod.rs:657-780`
- Modify: `src/compositor/state/windows.rs:19-114, 1371-1504`
- Modify: `src/compositor/server_xwayland.rs:74-168`
- Modify: `src/compositor/state/desktop_windows.rs:224-251` only if cancellation belongs at the common removal boundary
- Modify: `src/compositor/tests/toplevel_management.rs`

**Interfaces:**
- Add a private `PendingX11Activation { window_id: WindowId, handle: X11WindowHandle }` field with `Option` storage in `CompositorState`, so at most one activation intent exists.
- Add private state methods with exact behavior: `queue_pending_x11_activation`, `complete_pending_x11_activation(handle) -> bool`, and `cancel_pending_x11_activation(window_id)`.
- Do not expose pending activation through Astrea or introduce a generic focus queue.

- [ ] **Step 1: Add the failing exact Activate test.**

  Add `exact_activate_accepts_an_associationless_iconic_x11_window`. Start from minimized `x11_surface_id == None`, invoke `handle.activate(0, 30)`, and assert the action result is `Accepted` plus a `Map(x11_handle)` backend request. Assert the original handle remains live and no second toplevel is announced. Before this task’s production change, the current `surface_resource_by_id` guard returns `Unavailable`, so the test must fail on the action result.

- [ ] **Step 2: Run the Activate test and verify RED.**

  ```bash
  rtk cargo test --locked exact_activate_accepts_an_associationless_iconic_x11_window -- --exact --nocapture
  ```

- [ ] **Step 3: Implement associationless X11 activation.**

  In `activate_desktop_window`, preserve the existing workspace, managed-kind, and normal-role gates. Restore a minimized target before requiring a surface. If it is X11, has no current attachment, and the restore/remap path was requested, queue `PendingX11Activation { window_id, handle }` and return `WindowActivationOutcome::Changed`; keep the existing synchronous focus/raise path for targets with a current surface and for XDG.

- [ ] **Step 4: Complete the intent only on the intended replacement association.**

  In `apply_xwayland_association_event` after `attach_x11_surface`, call `complete_pending_x11_activation(window)`. It must compare both `WindowId` lookup and the stored `X11WindowHandle`, require the attached surface resource to be current, invoke the normal activation/focus/raise path, and clear the `Option` only after that path is not `Unavailable`. Do not use the surface id as the durable key.

- [ ] **Step 5: Cancel the intent on teardown and generation loss.**

  Clear the intent when `remove_x11_desktop_window` removes its target, which covers `WindowWithdrawn`, `WindowDestroyed`, and `clear_xwayland_generation`’s existing removal loop. Ensure a plain association `Removed` event does not cancel it while the X11 window record survives for remap.

- [ ] **Step 6: Run Activate, replacement, and cancellation tests to verify GREEN.**

  ```bash
  rtk cargo test --locked exact_activate_accepts_an_associationless_iconic_x11_window -- --exact --nocapture
  rtk cargo test --locked x11_surface_replacement_reuses_window_id_and_astrea_handle -- --exact --nocapture
  rtk cargo test --locked withdrawn_managed_x11_closes_the_astrea_handle -- --exact --nocapture
  rtk cargo test --locked wm_unmap_confirmation_is_iconic_and_restore_needs_a_new_buffer -- --exact --nocapture
  ```

  Assert the replacement association completes focus for the stored `WindowId`, consumes the intent, and cannot focus a later window after withdrawal or generation removal.

- [ ] **Step 7: Commit bounded activation.**

  ```bash
  rtk git add src/compositor/mod.rs src/compositor/state/windows.rs src/compositor/server_xwayland.rs src/compositor/state/desktop_windows.rs src/compositor/tests/toplevel_management.rs
  rtk git commit -m "fix: activate restored Iconic X11 tasks"
  ```

### Task 6: Run adjacent lifecycle coverage and final verification

**Files:**
- Modify only if a test assertion or formatting fix is required in the task-owned files above.

- [ ] **Step 1: Run the focused XWM and compositor lifecycle suites.**

  ```bash
  rtk cargo test --locked xwayland::xwm::events -- --nocapture
  rtk cargo test --locked xwayland::xwm::events_regression_tests -- --nocapture
  rtk cargo test --locked compositor::tests::xwayland -- --nocapture
  rtk cargo test --locked compositor::tests::toplevel_management -- --nocapture
  rtk cargo test --locked compositor::state::desktop_window_tests -- --nocapture
  ```

  Record exact passed/failed counts and distinguish task-local failures from pre-existing failures.

- [ ] **Step 2: Run the required static and locked checks.**

  ```bash
  rtk cargo fmt --check
  rtk cargo check --locked --all-targets
  rtk git diff --check
  ```

- [ ] **Step 3: Run the full test suite when the environment permits it.**

  ```bash
  rtk cargo test --locked --all-targets
  ```

  Report the exact test count and every failure category; do not label the full suite green if unrelated existing diagnostics remain.

- [ ] **Step 4: Inspect the final diff for forbidden scope.**

  ```bash
  rtk git diff --stat HEAD~3..HEAD
  rtk git diff --check HEAD~3..HEAD
  rtk rg -n "steam|proton|wine|Eclipse|F09|consume_wm_unmap" src/compositor src/xwayland
  ```

  Confirm no Eclipse files changed, no application-specific condition was added, and `consume_wm_unmap` was not modified.

- [ ] **Step 5: Build the release binary in the existing repository target directory.**

  ```bash
  rtk cargo build --release --locked
  ```

  If the native session is available, launch the user’s `oblivion-one-tty` Typhon instance and verify with `./target/release/astreactl --json --instance oblivion-one-tty windows` that minimize preserves the same `WindowId` with `mapped=false`, `minimized=true`, and `skipTaskbar=false`; activate from the Dock, verify the same id remaps without a duplicate task, then close the application and verify the `DesktopWindow` and Astrea handle disappear.

- [ ] **Step 6: Commit only after fresh verification evidence.**

  ```bash
  rtk git status --short --branch
  rtk git log -6 --oneline --decorate
  ```

  Ensure unrelated pre-existing modifications remain unstaged and the task commits contain only the approved lifecycle work.
