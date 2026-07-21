# X11 Late Auxiliary and Absolute Interaction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Retract late-identified X11 support windows from the desktop and preserve absolute X11 coordinates through repeated move/resize interactions.

**Architecture:** Keep auxiliary classification and lifecycle transitions inside `X11WindowRegistry`, using the existing `WindowWithdrawn` event to remove only compositor-owned desktop state. Keep interaction coordinate semantics inside `SurfacePlacement` by copying the starting placement and replacing coordinates rather than constructing a new cascaded root.

**Tech Stack:** Rust, x11rb XWM, Wayland compositor state, Cargo unit/integration tests.

## Global Constraints

- Do not match Steam names, classes, PIDs, or executable paths.
- Do not unmap or destroy client-owned support windows.
- Auxiliary classification must be reversible when later properties identify a legitimate typed/input window.
- XDG cascaded placement and X11 absolute placement must both retain their original `RootPlacementMode`.
- Use test-first red/green cycles and atomic commits.

---

### Task 1: Reconcile late auxiliary support identity

**Files:**
- Modify: `src/xwayland/xwm/window.rs`
- Modify: `src/xwayland/xwm/properties.rs`
- Test: `src/xwayland/xwm/window.rs`

**Interfaces:**
- Consumes: `X11WindowRecord`, `X11WindowLifecycle`, and the existing `XwmEvent::WindowWithdrawn` compositor contract.
- Produces: `X11WindowRegistry::reconcile_auxiliary(handle) -> AuxiliaryReconciliation`, where the result distinguishes no transition, desktop withdrawal, and readiness restored.

- [ ] **Step 1: Write failing registry tests**

Add tests that build a fully ready tiny managed window, first admit it without support properties, then set a self `client_leader` and assert reconciliation removes its snapshot and returns a withdrawal transition. Add a second test proving a tiny untyped/no-input self leader is auxiliary without a user-time window. Add a third test that sets `window_type = Some(X11WindowType::Normal)` and `accepts_input = Some(true)` after withdrawal, asserts readiness is restored, and verifies `try_ready` creates a new snapshot.

Use an explicit transition enum in the test contract:

```rust
assert_eq!(
    registry.reconcile_auxiliary(window).expect("known window"),
    AuxiliaryReconciliation::WithdrawDesktop,
);
assert!(registry.get(window).expect("window").snapshot.is_none());
assert_eq!(
    registry.get(window).expect("window").lifecycle,
    X11WindowLifecycle::Auxiliary,
);
```

- [ ] **Step 2: Run the focused tests and verify red**

Run:

```bash
cargo test --locked late_auxiliary
cargo test --locked self_client_leader_without_user_time
```

Expected: compilation fails because `AuxiliaryReconciliation` and `reconcile_auxiliary` do not exist, proving the new lifecycle contract is absent.

- [ ] **Step 3: Implement the minimal registry transition**

Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuxiliaryReconciliation {
    Unchanged,
    WithdrawDesktop,
    ReadinessRestored,
}
```

Change the support predicate to require managed kind, dimensions at most 16x16, a self client leader, no window type, and no input hint. Do not require `_NET_WM_USER_TIME_WINDOW`.

Implement `reconcile_auxiliary` so that:

```rust
if is_auxiliary_client_leader(handle, record) {
    if record.snapshot.take().is_some() {
        record.lifecycle = X11WindowLifecycle::Auxiliary;
        return Ok(AuxiliaryReconciliation::WithdrawDesktop);
    }
    record.lifecycle = X11WindowLifecycle::Auxiliary;
    return Ok(AuxiliaryReconciliation::Unchanged);
}
if record.lifecycle == X11WindowLifecycle::Auxiliary {
    record.lifecycle = X11WindowLifecycle::Observed;
    self.update_pending_lifecycle(handle)?;
    return Ok(AuxiliaryReconciliation::ReadinessRestored);
}
```

Keep `try_ready` using the same predicate for initial admission.

- [ ] **Step 4: Connect completed property refreshes to withdrawal**

After `maybe_finish_refresh` has committed staged properties in `complete_property`, call `xwm.windows.reconcile_auxiliary(pending.handle)`. On `WithdrawDesktop`, push `XwmEvent::WindowWithdrawn(pending.handle)`. On `ReadinessRestored`, rely on the existing `poll_replies` call to `emit_ready_if_complete`; on `Unchanged`, do nothing.

Do not emit withdrawal before the property value is committed, and do not emit it more than once for the same admitted snapshot.

- [ ] **Step 5: Run focused and XWM tests and verify green**

Run:

```bash
cargo test --locked late_auxiliary
cargo test --locked self_client_leader_without_user_time
cargo test --locked xwayland::xwm
```

Expected: all selected tests pass with zero failures.

- [ ] **Step 6: Commit the lifecycle fix**

```bash
git add src/xwayland/xwm/window.rs src/xwayland/xwm/properties.rs
git commit -m "fix(xwayland): retract late auxiliary support windows"
```

---

### Task 2: Preserve root-placement mode in move and resize

**Files:**
- Modify: `src/compositor/state/window_interaction.rs`
- Modify: `src/compositor/state/window_resize.rs`
- Test: `src/compositor/state/window_interaction_tests.rs`

**Interfaces:**
- Consumes: `WindowInteraction::start_placement`, `SurfacePlacement`, `PendingInteractiveResizeUpdate`, and `WindowBackendCommand`.
- Produces: interaction updates whose placement coordinates change while `parent_surface_id` and `root_mode` remain unchanged.

- [ ] **Step 1: Write failing absolute-placement interaction tests**

Create an X11 desktop window whose stored placement is `SurfacePlacement::absolute_root_at(40, 50)`. Begin a move interaction, update the pointer, and assert the stored/queued placement is still `RootPlacementMode::Absolute`. Repeat for a resize interaction through `apply_pending_interactive_resize_update`, asserting both the visual geometry and queued backend `Configure` geometry are absolute.

The key assertions are:

```rust
assert_eq!(geometry.placement.root_mode, RootPlacementMode::Absolute);
assert_eq!(geometry.placement, SurfacePlacement::absolute_root_at(expected_x, expected_y));
```

- [ ] **Step 2: Run the focused tests and verify red**

Run:

```bash
cargo test --locked absolute_x11_move_preserves_root_placement_mode
cargo test --locked absolute_x11_resize_preserves_root_placement_mode
```

Expected: assertions report `CascadedWindow` instead of `Absolute`.

- [ ] **Step 3: Implement coordinate replacement without mode replacement**

In the move branch, replace `SurfacePlacement::root_at(...)` with:

```rust
let placement = SurfacePlacement {
    local_x: interaction.start_placement.local_x + dx,
    local_y: interaction.start_placement.local_y + dy,
    ..interaction.start_placement
};
```

In the resize branch, create the pending placement with:

```rust
placement: SurfacePlacement {
    local_x: resize.x,
    local_y: resize.y,
    ..interaction.start_placement
},
```

Update X11-only test helpers in `window_resize.rs` to use `absolute_root_at` because X11 geometry is root-relative. Do not change XDG test helpers or generic rendering cascade behavior.

- [ ] **Step 4: Run interaction, desktop-window, and render tests**

Run:

```bash
cargo test --locked absolute_x11_
cargo test --locked compositor::state::window_interaction_tests
cargo test --locked compositor::state::desktop_window_tests
cargo test --locked compositor::render::tests
```

Expected: all selected tests pass with zero failures.

- [ ] **Step 5: Commit the coordinate fix**

```bash
git add src/compositor/state/window_interaction.rs src/compositor/state/window_resize.rs src/compositor/state/window_interaction_tests.rs
git commit -m "fix(xwayland): preserve absolute placement during interaction"
```

---

### Task 3: Full and native verification

**Files:**
- Verify only; no planned source modifications.

**Interfaces:**
- Consumes: the two atomic commits from Tasks 1 and 2.
- Produces: a release binary and live evidence for support-window withdrawal, popup visibility, and stable repeated resize.

- [ ] **Step 1: Run all project gates**

```bash
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked -- --test-threads=1
./bin/check-source-layout
git diff --check
```

Expected: every command exits zero. The serial test run avoids the repository's known parallel FD-ownership test interference.

- [ ] **Step 2: Build a provably fresh release binary**

```bash
cargo clean --release -p oblivion-one
cargo build --locked --release
```

Expected: Cargo reports `Compiling oblivion-one`, and the release executable has an inode/hash different from the currently running pre-fix process.

- [ ] **Step 3: Restart and verify Steam natively**

After restart, launch Steam and inspect:

```bash
xprop -root _NET_CLIENT_LIST _NET_CLIENT_LIST_STACKING
xprop -id <support-xid> WM_CLIENT_LEADER _NET_WM_USER_TIME_WINDOW
xwininfo -id <steam-main-xid>
```

Expected: the main Steam XID is the only normal Steam EWMH client; 10x10 support windows remain alive in Xwayland but are absent from desktop publication. Opening a menu shows the same non-black pixels captured directly from Xwayland. Repeated moves/resizes keep the main window viewable, preserve absolute root geometry, and do not drift or duplicate windows.

- [ ] **Step 4: Confirm repository state**

```bash
git status --short
git log -5 --oneline
```

Expected: clean worktree with the design, lifecycle, and coordinate commits at the tip of `main`.
