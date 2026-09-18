# Typhon X11 Iconic Late-Unmap Race Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve outstanding WM-owned `UnmapNotify` confirmations across Iconic remap requests so rapid restore/activate cannot become a real client withdrawal.

**Architecture:** Keep lifecycle-counter ownership in `X11WindowRegistry::mark_map_requested()`, which serves both compositor `XwmCommand::Map` and client `MapRequest` paths. For an Iconic remap, start the new map epoch without clearing `inflight_wm_unmaps`; retain existing reset behavior for initial maps and genuine Withdrawn remaps. Add production-path event regressions in the existing XWM regression module and leave compositor, Eclipse, Astrea identity semantics, and the existing Iconic solution unchanged.

**Tech Stack:** Rust, Cargo, x11rb, Typhon XWM test fixtures, `rtk` command wrappers.

## Global Constraints

- Compile and test in `/home/agony/GitHub/Typhon` so build artifacts stay in the same folder.
- Use `rtk` for shell, Cargo, and Git commands.
- Do not use subagents.
- Do not modify Eclipse or Astrea task identity semantics.
- Do not redesign the completed `a23cfe6` Iconic lifetime solution.
- Preserve unrelated existing working-tree changes.
- Commit the scoped change because this is a Git repository.

---

### Task 1: Add and prove the production-command late-Unmap regression

**Files:**
- Modify: `src/xwayland/xwm/events_regression_tests.rs`
- Inspect: `src/xwayland/xwm/events.rs`, `src/xwayland/xwm/commands.rs`, `src/xwayland/xwm/window.rs`

**Interfaces:**
- Consumes: `test_fixture`, `prepare_managed_window`, `commands::execute`, `normalize`, `unmap_event`, `XwmCommand`, `XwmEvent`, and `X11WindowRecord` lifecycle fields.
- Produces: a regression named `wm_unmap_command_then_map_command_consumes_late_confirmation` that exercises `Unmap → Map → delayed UnmapNotify` through production command/event paths.

- [ ] **Step 1: Write the failing test**

Add a test beside the existing Iconic XWM regressions:

```rust
#[test]
fn wm_unmap_command_then_map_command_consumes_late_confirmation() {
    let generation = generation(32);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 132, true, true, true);
    xwm.windows
        .confirm_map_notify(handle)
        .expect("initial MapNotify");
    xwm.windows
        .try_ready(handle)
        .expect("known window")
        .expect("ready window");

    super::super::commands::execute(&mut xwm, XwmCommand::Unmap(handle))
        .expect("WM unmap command");
    let record = xwm.windows.get(handle).expect("window after WM unmap");
    assert_eq!(record.lifecycle, X11WindowLifecycle::Iconic);
    assert_eq!(record.inflight_wm_unmaps, 1);

    super::super::commands::execute(&mut xwm, XwmCommand::Map(handle))
        .expect("restore map command");
    normalize(&mut xwm, unmap_event(handle.xid())).expect("late WM UnmapNotify");

    let events = ready_events(&mut xwm);
    assert!(!events
        .iter()
        .any(|event| matches!(event, XwmEvent::WindowWithdrawn(window) if *window == handle)));
    let record = xwm.windows.get(handle).expect("restoring window survives");
    assert!(record.snapshot.is_some());
    assert_eq!(record.inflight_wm_unmaps, 0);
    assert_eq!(record.lifecycle, X11WindowLifecycle::MapCommanded);
}
```

Use the existing fixture’s `prepare_managed_window` setup and the existing record fields; do not add test-only production APIs.

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
rtk cargo test --locked wm_unmap_command_then_map_command_consumes_late_confirmation -- --exact
```

Expected: FAIL because current `mark_map_requested()` clears `inflight_wm_unmaps`, so the delayed `UnmapNotify` is treated as a real withdrawal and emits `WindowWithdrawn`.

- [ ] **Step 3: Commit the test-only RED checkpoint**

Do not commit production changes at this point; retain the failing test in the working tree for the next task.

### Task 2: Add and prove the client-MapRequest late-Unmap regression

**Files:**
- Modify: `src/xwayland/xwm/events_regression_tests.rs`

**Interfaces:**
- Consumes: the ready-window fixture from Task 1, `XwmCommand::Unmap`, `map_request_event`, `normalize`, and `unmap_event`.
- Produces: a regression named `client_map_request_then_late_wm_unmap_preserves_identity` covering the other caller of `mark_map_requested()`.

- [ ] **Step 1: Write the failing test**

Add a test beside Task 1:

```rust
#[test]
fn client_map_request_then_late_wm_unmap_preserves_identity() {
    let generation = generation(33);
    let (mut xwm, _peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 133, true, true, true);
    xwm.windows
        .confirm_map_notify(handle)
        .expect("initial MapNotify");
    xwm.windows
        .try_ready(handle)
        .expect("known window")
        .expect("ready window");

    super::super::commands::execute(&mut xwm, XwmCommand::Unmap(handle))
        .expect("WM unmap command");
    assert_eq!(
        xwm.windows
            .get(handle)
            .expect("iconic window")
            .inflight_wm_unmaps,
        1
    );

    normalize(&mut xwm, map_request_event(handle.xid())).expect("client MapRequest");
    normalize(&mut xwm, unmap_event(handle.xid())).expect("late WM UnmapNotify");

    let events = ready_events(&mut xwm);
    assert!(!events
        .iter()
        .any(|event| matches!(event, XwmEvent::WindowWithdrawn(window) if *window == handle)));
    let record = xwm.windows.get(handle).expect("client remap survives");
    assert_eq!(record.inflight_wm_unmaps, 0);
    assert_ne!(record.lifecycle, X11WindowLifecycle::Withdrawn);
    assert_ne!(record.lifecycle, X11WindowLifecycle::Destroyed);
}
```

- [ ] **Step 2: Run both focused regressions and verify the second RED failure**

Run:

```bash
rtk cargo test --locked wm_unmap_command_then_map_command_consumes_late_confirmation -- --exact
rtk cargo test --locked client_map_request_then_late_wm_unmap_preserves_identity -- --exact
```

Expected: both regressions fail for the same counter-reset reason.

### Task 3: Preserve the confirmed normal minimize path

**Files:**
- Modify: `src/xwayland/xwm/events_regression_tests.rs`

**Interfaces:**
- Consumes: `XwmCommand::Unmap`, `unmap_event`, `normalize`, and `ready_events`.
- Produces: a regression named `confirmed_wm_unmap_remains_iconic_before_restore` proving ordinary confirmed minimization remains Iconic.

- [ ] **Step 1: Add the normal confirmed-minimize regression**

Use a ready managed fixture, execute `XwmCommand::Unmap(handle)`, normalize the immediate `unmap_event(handle.xid())`, assert `inflight_wm_unmaps == 0`, assert `lifecycle == Iconic`, and assert no `WindowWithdrawn` event is emitted.

- [ ] **Step 2: Run the three XWM regressions before production changes**

Run:

```bash
rtk cargo test --locked wm_unmap_command_then_map_command_consumes_late_confirmation -- --exact
rtk cargo test --locked client_map_request_then_late_wm_unmap_preserves_identity -- --exact
rtk cargo test --locked confirmed_wm_unmap_remains_iconic_before_restore -- --exact
```

Expected: the two late-confirmation tests fail and the ordinary confirmed-minimize regression passes.

### Task 4: Make the smallest lifecycle-counter correction

**Files:**
- Modify: `src/xwayland/xwm/window.rs:283-313`

**Interfaces:**
- Consumes: the failing production-path regressions from Tasks 1–2.
- Produces: `mark_map_requested()` behavior that preserves `inflight_wm_unmaps` only while remapping an Iconic record.

- [ ] **Step 1: Change only the counter reset condition**

Keep the existing `remapping_iconic` detection and change the unconditional reset to:

```rust
if !remapping_iconic {
    record.inflight_wm_unmaps = 0;
}
```

Do not remove the explicit Iconic call from `XwmCommand::Map` unless the focused source/tests prove it is redundant; no other command/event/compositor file should change for this fix.

- [ ] **Step 2: Run the three regressions and verify GREEN**

Run the Task 3 command. Expected: all three pass, with the late old `UnmapNotify` consumed and no `WindowWithdrawn`.

### Task 5: Run the existing XWM and Iconic lifecycle suite

**Files:**
- Inspect: `src/xwayland/xwm/window.rs`, `src/xwayland/xwm/events.rs`, `src/xwayland/xwm/events_regression_tests.rs`, and previous Astrea/X11 compositor tests.

- [ ] **Step 1: Run focused XWM tests**

Run the existing tests covering Iconic, `UnmapNotify`, `MapRequest`, map commands, late WM confirmations, surface removal/replacement, withdrawal, and destruction; include the three new tests and the existing `late_wm_unmap_confirmation_does_not_cancel_a_restore_map` regression.

- [ ] **Step 2: Run the previous compositor X11/Astrea regression tests**

Run the focused tests covering associationless Iconic publication, WindowId preservation, replacement association, Restore/Activate, cancellation, withdrawal, destruction, and generation cleanup. Do not delete or weaken any existing regression.

- [ ] **Step 3: Run repository verification**

Run:

```bash
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk git diff --check
```

Run the full test suite if the environment permits, recording exact pass/failure counts and separating unrelated pre-existing failures.

### Task 6: Build, qualify, and commit

- [ ] **Step 1: Build release in the repository folder**

Run:

```bash
rtk cargo build --release --locked
```

- [ ] **Step 2: Perform hardware qualification**

Exercise normal minimize→wait→restore and repeated immediate Dock restore/Activate on X11. Confirm the same WindowId and Dock task survive without duplicate/disappearing tasks, and confirm a real close still removes the task.

- [ ] **Step 3: Review the scoped diff**

Run `rtk git diff -- src/xwayland/xwm/window.rs src/xwayland/xwm/events_regression_tests.rs` and verify Eclipse, compositor behavior, Astrea identity semantics, and unrelated dirty files are untouched.

- [ ] **Step 4: Commit only the scoped lifecycle closure**

Stage the plan, XWM regression tests, and registry correction without staging unrelated existing modifications, then commit:

```bash
rtk git add docs/superpowers/plans/2026-09-18-typhon-x11-iconic-late-unmap-race-closure.md src/xwayland/xwm/events_regression_tests.rs src/xwayland/xwm/window.rs
rtk git commit -m "fix: preserve late Iconic WM unmap confirmations"
```
