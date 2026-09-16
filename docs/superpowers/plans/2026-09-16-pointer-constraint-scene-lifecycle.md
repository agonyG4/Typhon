# Pointer Constraint Scene-Lifecycle Reconciliation Implementation Plan

> **For agentic workers:** Execute this plan inline in the current workspace; subagents are prohibited by the task instructions. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ensure locked and confined pointer constraints lose input-routing authority when their owning window leaves the active scene, including workspace changes, special-workspace closure, workspace moves, and minimize.

**Architecture:** Add one root-oriented compositor reconciliation helper to the existing pointer-constraint module. It resolves constrained child/subsurface surfaces through `presentation_owner_root_for_surface`, then delegates each match to `deactivate_pointer_constraint_by_id` so backend cancellation, generation checks, lifetime semantics, cursor reveal, and stale callback handling remain centralized. Workspace callers run the helper after rebuilding the new scene while `workspace_scene_transition_active` is true; minimize runs it after removing the minimized root's renderable surfaces and before final pointer-focus reconciliation.

**Tech Stack:** Rust, Smithay/Wayland compositor state, existing controllable compositor integration tests, Cargo, `rtk`.

## Global Constraints

- Compile and test in `/home/agony/GitHub/Typhon` so Cargo uses the repository-local target directory.
- Use `rtk` for repository, search, Cargo, and verification commands where available.
- Do not use subagents.
- Preserve the active locked/confined early returns for ordinary pointer motion.
- Resolve ownership through existing root/presentation ownership helpers; do not add a parallel window/surface model.
- Keep workspace transition deactivation inside `workspace_scene_transition_active == true` and refresh pointer focus once after the flag is cleared.
- Preserve Persistent versus Oneshot semantics, cursor hint/reveal ordering, render-generation ownership, and stale-generation protection.
- Do not change Direct Scanout, Presentation Coverage, VRR, tearing, KMS presentation, Predictive O1, or unrelated XWayland/workspace semantics.
- Do not include the pre-existing modified files in implementation commits.

---

## File map

- Modify `src/compositor/state/pointer_constraints.rs`: deactivation reason, root-oriented owner matching, and focused pointer-constraint regressions/helpers if required by test visibility.
- Modify `src/compositor/state/workspaces.rs`: reconcile departing owners in regular activation, special-workspace toggle/close, and family moves while the transition flag is active.
- Modify `src/compositor/state/windows.rs`: reconcile minimized root ownership after renderable removal and before final pointer/window focus settling.
- Modify `src/compositor/tests/support/server_runtime.rs`: add a controllable test command for regular workspace activation if integration regressions need it.
- Modify `src/compositor/tests/input_output/pointer_constraint_transaction.rs` and/or `src/compositor/tests/input_output/relative_and_constraints.rs`: deterministic pending/active/lifetime workspace regressions and ordinary-motion negative coverage.
- Modify `src/compositor/tests/input_output/pointer_cursor.rs`: extend the existing cursor-settlement regression with scene-departure assertions.
- Modify `src/compositor/state/desktop_window_tests.rs` or `src/compositor/state/window_interaction_tests.rs` only for state-level minimize/move/no-op ordering assertions that cannot be expressed through the integration fixture.
- Create and retain `docs/superpowers/specs/2026-09-16-pointer-constraint-scene-lifecycle-design.md` and this plan as the approved design record.

## Task 1: Add the RED workspace-departure regression

**Files:**
- Test: `src/compositor/tests/input_output/pointer_constraint_transaction.rs`
- Modify test support: `src/compositor/tests/support/server_runtime.rs`

**Interfaces:**
- Consumes existing controllable-server helpers, `PointerConstraintBackendRequest`, `PointerConstraintSurfaceSnapshot`, and pointer-focus capture commands.
- Produces a deterministic failure showing an active persistent locked constraint remains routed after its owning workspace leaves the scene.

- [ ] **Step 1: Add the workspace activation command to the test server.**

Use the existing command pattern:

```rust
ActivateWorkspace { workspace: u32 },
```

and dispatch it by constructing `WorkspaceId::new(workspace)` and calling `server.activate_workspace(workspace_id)` only when the ID is valid. This changes test control plumbing, not compositor behavior.

- [ ] **Step 2: Write the failing persistent-lock workspace test before production edits.**

Build on the existing `create_test_buffered_toplevel`, pointer-motion, constraint commit, `capture_pointer_constraint_backend_requests`, `capture_pointer_constraint_snapshot`, and `capture_pointer_focus_surface_id` helpers. Activate the persistent lock backend, issue `ActivateWorkspace { workspace: 2 }`, then assert the old constraint is inactive, routing focus is `None`, and a `Deactivate` request exists. Name the test:

```rust
#[test]
fn persistent_locked_pointer_releases_when_workspace_owner_departs() {
    // Existing setup creates and focuses one buffered toplevel, commits a
    // persistent lock, and settles its ActivateLocked request.
    commands.send(ServerCommand::ActivateWorkspace { workspace: 2 })?;
    wait_for_server_commands(&commands);

    let snapshot = capture_pointer_constraint_snapshot(&commands, constraint_id)
        .expect("persistent constraint remains registered");
    assert!(!snapshot.active);
    assert!(!snapshot.backend_pending);
    assert!(!snapshot.defunct);
    assert_eq!(capture_pointer_focus_surface_id(&commands), None);
    assert!(capture_pointer_constraint_backend_requests(&commands)
        .iter()
        .any(|request| matches!(request, PointerConstraintBackendRequest::Deactivate { .. })));
}
```

The test must retain the constraint object long enough to assert the post-switch snapshot is committed, protocol-alive, and not defunct.

- [ ] **Step 3: Run only the new test and verify the correct RED failure.**

Run:

```bash
rtk cargo test --locked compositor::tests::input_output::pointer_constraint_transaction::persistent_locked_pointer_releases_when_workspace_owner_departs -- --exact
```

Expected: the test compiles and fails because the old active route remains authoritative and no compositor-driven deactivation is queued. If it errors or passes, correct the fixture before writing production code.

- [ ] **Step 4: Commit only the RED test/support setup if it is independently useful.**

```bash
rtk git add src/compositor/tests/input_output/pointer_constraint_transaction.rs src/compositor/tests/support/server_runtime.rs
rtk git commit -m "test(input): reproduce pointer constraint workspace departure"
```

## Task 2: Implement root-oriented compositor deactivation

**Files:**
- Modify: `src/compositor/state/pointer_constraints.rs`
- Test: `src/compositor/tests/input_output/pointer_constraint_transaction.rs`

**Interfaces:**
- Consumes `WindowId` lists and existing `presentation_owner_root_for_surface`, `window_id_for_surface`, and `deactivate_pointer_constraint_by_id` behavior.
- Produces `deactivate_pointer_constraints_for_departing_window_ids(&mut self, window_ids: &[WindowId], reason: PointerConstraintDeactivationReason)` in `CompositorState`.

- [ ] **Step 1: Add the smallest failing assertions for pending activation and child ownership.**

Extend the integration fixture to cover an activation request that is queued but not settled, then switch workspaces and assert the activation request is removed, the snapshot has `backend_pending == false`, and a late `PointerConstraintBackendActivated` for the canceled generation leaves it inactive. Build the child case with the existing subsurface fixture and assert that a constraint on the child is released when the application root departs.

- [ ] **Step 2: Run the pending and child regressions to confirm RED.**

```bash
rtk cargo test --locked compositor::tests::input_output::pointer_constraint_transaction -- --nocapture
```

Expected: the new assertions fail against the old lifecycle; existing transaction tests remain diagnostic for any fixture mistake.

- [ ] **Step 3: Add the stable compositor-driven deactivation reason enum.**

Use explicit variants:

```rust
#[derive(Debug, Clone, Copy)]
enum PointerConstraintDeactivationReason {
    WorkspaceDeparture,
    WindowMovedOffScene,
    WindowMinimized,
}
```

Keep it private to compositor state unless tests need public access. Format the reason into existing pointer debug messages; do not add synchronous logging to input/frame hot paths.

- [ ] **Step 4: Implement owner matching through canonical presentation roots.**

Compute canonical root surface IDs from each departing window's existing `workspace_owner_window_id` and `Window::root_surface_id`. For each constraint, compare:

```rust
self.presentation_owner_root_for_surface(compositor_surface_id(&constraint.surface))
```

with those roots. Collect IDs before mutating the map, then call `deactivate_pointer_constraint_by_id(id, true, true, true)` for each match. Do not call `clear_pointer_focus` or remove the active locked/confined early returns. Do not mark Persistent constraints defunct; pass compositor-driven ownership so Oneshot follows existing defunct behavior.

- [ ] **Step 5: Run the focused transaction tests and verify GREEN for the helper behavior.**

```bash
rtk cargo test --locked compositor::tests::input_output::pointer_constraint_transaction
```

Expected: active/pending workspace departure, child-root matching, persistent resource retention, oneshot defunct behavior, and stale activation rejection pass.

## Task 3: Order reconciliation in every workspace scene-departure path

**Files:**
- Modify: `src/compositor/state/workspaces.rs`
- Tests: `src/compositor/tests/input_output/pointer_constraint_transaction.rs`, `src/compositor/state/desktop_window_tests.rs` or the existing workspace-focused state test module

**Interfaces:**
- Consumes the helper from Task 2 and existing departing-window calculations.
- Produces one reconciliation during regular workspace activation, special-workspace close/switch, and active-family moves, with no-op workspace activation left untouched.

- [ ] **Step 1: Add RED coverage for confined departure, special close, family move, and no-op activation.**

For each active constraint, assert `active_confined_pointer_routing`/`active_locked_pointer_routing` is no longer authoritative, a backend `Deactivate` is queued, and the persistent snapshot remains committed and non-defunct. For no-op activation, assert no deactivation request is queued and the valid route remains active. Reuse existing `ToggleDefaultSpecialWorkspace`, `MoveFocusedWindowToOrFromSpecialWorkspace`, and `MoveWindowToWorkspace` test commands.

- [ ] **Step 2: Run the new workspace cases to confirm RED.**

```bash
rtk cargo test --locked compositor::tests::input_output::pointer_constraint_transaction
rtk cargo test --locked compositor::tests::state::desktop_window_tests
```

Expected: departure cases fail, and the no-op case stays green or exposes only fixture/setup problems.

- [ ] **Step 3: Set the transition guard before active-scene mutation in membership transitions.**

In `apply_workspace_membership_transition`, after fallible preparation and before membership/scene mutation, set `workspace_scene_transition_active = true` when `transition.active_scene_changed`. Leave it unset for non-scene membership changes and preserve the existing early return path.

- [ ] **Step 4: Reconcile after rebuilding the new active scene and before focus settlement.**

Call the helper after `rebuild_active_scene_view()` and before keyboard/window focus recomputation. Clear the transition flag only after reconciliation and focus settlement, then call `refresh_pointer_focus_at_last_position()` once. The existing deactivation transaction will invoke an intermediate refresh, but the transition guard makes that refresh a no-op.

- [ ] **Step 5: Apply the same placement to `activate_workspace`.**

Keep the existing affected-window calculation before the workspace-manager mutation. After `rebuild_active_scene_view()`, reconcile `affected_windows` with `WorkspaceDeparture`, then settle keyboard/window focus, clear the transition flag, and perform the existing single pointer refresh. Return immediately for `WorkspaceSwitchOutcome::NoChange` as today.

- [ ] **Step 6: Apply the same placement to `toggle_default_special_workspace`.**

Use its existing `departing_window_ids_for_scene_selection` result. After rebuilding the active scene, reconcile departing roots before the close/open focus logic completes. Keep the transition flag active through deactivation and all focus settlement, then refresh once after clearing it.

- [ ] **Step 7: Run the workspace focused tests.**

```bash
rtk cargo test --locked compositor::tests::input_output::pointer_constraint_transaction
rtk cargo test --locked compositor::tests::state::desktop_window_tests
rtk cargo test --locked compositor::tests::state::window_interaction_tests
```

Expected: regular switch, special close/switch, family move, and no-op behavior pass without changing unrelated workspace generation/focus assertions.

## Task 4: Release constraints during minimize while preserving lifecycle visuals

**Files:**
- Modify: `src/compositor/state/windows.rs`
- Tests: `src/compositor/tests/input_output/pointer_constraint_transaction.rs`, `src/compositor/state/window_interaction_tests.rs`

**Interfaces:**
- Consumes the minimized window's canonical `root_surface_id` and Task 2 helper.
- Produces logical route release before final pointer/window focus reconciliation while retaining existing minimize animation ownership.

- [ ] **Step 1: Add RED locked and confined minimize regressions.**

Activate each constraint, issue `MinimizeFocused`, and assert the constraint is inactive, old pointer focus is gone or transferred, and a `Deactivate` request is queued. Capture the existing lifecycle path snapshot where available and assert retained visual animation state is still present independently of pointer routing.

- [ ] **Step 2: Run the minimize tests to confirm RED.**

```bash
rtk cargo test --locked compositor::tests::input_output::pointer_constraint_transaction -- --nocapture
rtk cargo test --locked compositor::tests::state::window_interaction_tests -- --nocapture
```

Expected: the active route remains or the old surface remains focused before the production change.

- [ ] **Step 3: Reconcile the minimized root after renderable-surface removal.**

In `minimize_desktop_window`, after `self.renderable_surfaces` is rebuilt and before the existing `clear_pointer_focus()`/focus settlement branch, call the root-oriented helper with `WindowMinimized`. Do not alter `begin_lifecycle_minimize`, retained visual groups, or animation rendering collections.

- [ ] **Step 4: Run focused minimize tests and check cursor settlement.**

```bash
rtk cargo test --locked compositor::tests::input_output::pointer_constraint_transaction
rtk cargo test --locked compositor::tests::input_output::pointer_cursor
rtk cargo test --locked compositor::tests::state::window_interaction_tests
```

Expected: locked reveal remains backend-settled through the existing pending reveal path, confined visibility/focus transfers normally, and lifecycle animation assertions remain unchanged.

## Task 5: Preserve ordinary motion and lifetime invariants

**Files:**
- Tests: `src/compositor/tests/input_output/relative_and_constraints.rs`, `src/compositor/tests/input_output/pointer_constraint_transaction.rs`, `src/compositor/tests/input_output/pointer_cursor.rs`
- Production changes are complete before this task; this task adds only invariant regressions.

**Interfaces:**
- Consumes existing active route and backend request capture helpers.
- Produces negative regressions proving an eligible visible constraint still blocks ordinary focus escape and persistent constraints can reactivate after returning to an eligible scene.

- [ ] **Step 1: Add the ordinary-motion negative regression first.**

With a visible active lock/confine, send pointer motion to the boundary/outside region and assert no compositor-driven `Deactivate` request is queued and the route remains active. Use existing relative-motion and confined-region fixtures rather than altering hit-test production code.

- [ ] **Step 2: Add return/reactivation and reveal assertions.**

After a persistent constraint is deactivated by departure and backend deactivation settles, return the owner to the active scene, refresh pointer focus through the normal path, and assert a new activation can be queued. For locked pointers, assert pending reveal/cursor visibility follows the existing `PointerConstraintBackendDeactivated` settlement rather than a direct visibility assignment; assert no stale lock-hidden constraint ID remains.

- [ ] **Step 3: Run focused input tests.**

```bash
rtk cargo test --locked compositor::tests::input_output::relative_and_constraints
rtk cargo test --locked compositor::tests::input_output::pointer_constraint_transaction
rtk cargo test --locked compositor::tests::input_output::pointer_cursor
```

Expected: ordinary constrained motion remains constrained, persistent routes remain eligible, oneshot routes do not reactivate, pending activations cannot resurrect through late callbacks, and cursor reveal remains transactional.

## Task 6: Verification, manual reproduction attempt, and focused commit

**Files:**
- Verify all files changed by Tasks 1–5; do not stage unrelated pre-existing changes.

- [ ] **Step 1: Run the requested focused tests.**

```bash
rtk cargo test --locked compositor::tests::input_output::relative_and_constraints
rtk cargo test --locked compositor::tests::input_output::pointer_constraint_transaction
rtk cargo test --locked compositor::tests::input_output::pointer_cursor
rtk cargo test --locked compositor::tests::state::desktop_window_tests
rtk cargo test --locked compositor::tests::state::window_interaction_tests
```

- [ ] **Step 2: Run repository gates in the same workspace folder.**

```bash
rtk cargo fmt --all -- --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk run ./bin/check-source-layout
rtk git diff --check
```

- [ ] **Step 3: Inspect the final diff and codebase coverage.**

Use `rtk git status --short`, `rtk git diff --stat`, and a targeted diff. Re-run the codebase graph impact/coverage checks for every changed runtime source file; if a changed file is reported with a partial range, read that range directly before relying on graph evidence.

- [ ] **Step 4: Attempt the real manual reproduction.**

Run Typhon using the repository’s documented local launch path, create or use two workspaces, activate a real locked/confined client surface, switch workspaces, and verify the old surface no longer owns pointer focus, the cursor is revealed through normal settlement, and the new scene accepts pointer input. Then verify the inverse by keeping the client visible and moving normally within/outside its constraint. If the environment lacks the required nested compositor/game/client, report the exact limitation rather than claiming manual acceptance.

- [ ] **Step 5: Commit only the implementation and its regressions.**

```bash
rtk git add src/compositor/state/pointer_constraints.rs src/compositor/state/workspaces.rs src/compositor/state/windows.rs src/compositor/tests/support/server_runtime.rs src/compositor/tests/input_output/pointer_constraint_transaction.rs src/compositor/tests/input_output/relative_and_constraints.rs src/compositor/tests/input_output/pointer_cursor.rs src/compositor/state/desktop_window_tests.rs src/compositor/state/window_interaction_tests.rs
rtk git commit -m "fix(input): release pointer constraints on scene departure"
```

Do not stage `.codebase-memory/`, the pre-existing pacing/buffering files, or unrelated plan files.
